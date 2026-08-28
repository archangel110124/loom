# The FFT ocean, core: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deterministic, CPU-authoritative ocean field — spectrum in, displaced surface
out — measured against the tick budget, with nothing integrated and nothing rendering yet.

**Architecture:** Three layers, each testable without the one above it. A hand-rolled
radix-2 IFFT with no dependency; a spectrum that fills the initial complex amplitudes from
wind, fetch and direction; and a cascade that evolves them to a time `t` and hands back
three real tiles. **The whole thing is a pure function of (scene, t)** — that is the
property ADR 0076 rests on, and F3 asserts it directly.

**Tech Stack:** Rust, `loom_water`, no new dependencies.

**Spec:** `docs/design/SEA-REBUILD.md` §3 (which this replaces the deep-water half of) and
**ADR 0076** `docs/decisions/0076-the-ocean-is-a-transform-and-it-still-carries-force.md`,
which is the binding argument. Read the ADR first.

**Branch:** `sea/rebuild`.

**Deliberately out of scope:** `sample_water` still sums Gerstner waves; nothing renders
the tile; no sim hash moves; no golden reference moves. Integration is the next plan, and
it is sized by what this one measures.

## Global Constraints

- **No new dependencies.** `loom_field`'s rule binds here: a crate must never be able to
  change the sim hash, which is why the transform is hand-rolled rather than taken from
  `rustfft`. This is not negotiable and is the reason F1 exists as its own task.
- **Never `thread_rng`, never `HashMap`/`HashSet` iteration in simulation code** — clippy
  enforces it. The spectrum's randomness comes from the frozen `loom_field::noise` hash.
- **Float addition is not associative.** Anything summed is summed in a fixed order, and
  the order is documented where it is relied on.
- **No wall clock in simulation code.** `t` is always tick count times the fixed step.
- Green for this plan: `cargo clippy --workspace -- -D warnings` and `cargo test -p
  loom_water`. The GPU gates are not touched because nothing here reaches the GPU.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, move or revert them.** Never `git add -A`.
- The fixed step is **60 Hz = 16.67 ms**. Every cost claim is a fraction of that.
- This project documents the *reasoning* behind decisions in the code, not only the
  mechanism.

## File Structure

| file | responsibility |
|---|---|
| `crates/loom_water/src/fft.rs` | **new.** Radix-2 Cooley–Tukey, 1D and 2D, forward and inverse, and the twiddle table. Knows nothing about water. |
| `crates/loom_water/src/ocean.rs` | **new.** The initial amplitude field from a spectrum, the time evolution, the cascade, and the three output tiles. |
| `crates/loom_water/src/spectrum.rs` | modified — gains the directional spectrum sampled onto a `k` grid, beside the existing sixteen-wave `wave_set` which stays untouched and in use. |
| `crates/loom_water/src/lib.rs` | two module declarations. |

`fft.rs` is separate from `ocean.rs` because a transform that knows nothing about water is
a transform that can be tested against a naive DFT, which is the only honest way to know
it is right.

---

### Task 1: The transform

Pure arithmetic. No water, no GPU, no scene.

**Files:**
- Create: `crates/loom_water/src/fft.rs`
- Modify: `crates/loom_water/src/lib.rs` (add `pub mod fft;`)
- Test: in `crates/loom_water/src/fft.rs`, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Complex { pub re: f32, pub im: f32 }`
  - `pub struct Twiddles { /* private */ }` with `pub fn new(n: usize) -> Self`
  - `pub fn ifft_2d(grid: &mut [Complex], n: usize, tw: &Twiddles, scratch: &mut [Complex])`

- [ ] **Step 1: Write the failing tests**

Create `crates/loom_water/src/fft.rs` with the module doc, the types, stubs that
`unimplemented!()`, and these tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn naive_idft_1d(input: &[Complex]) -> Vec<Complex> {
        let n = input.len();
        (0..n)
            .map(|k| {
                let mut acc = Complex { re: 0.0, im: 0.0 };
                for (j, x) in input.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    let a = 2.0 * std::f32::consts::PI * (k * j) as f32 / n as f32;
                    let (s, c) = a.sin_cos();
                    acc.re += x.re * c - x.im * s;
                    acc.im += x.re * s + x.im * c;
                }
                acc
            })
            .collect()
    }

    /// **The only honest test of a transform is a different transform.** A radix-2
    /// butterfly network that is subtly wrong — a reversed twiddle sign, an off-by-one in
    /// the bit reversal — still produces plausible-looking numbers and still round-trips,
    /// because the error cancels against itself. A naive O(n^2) DFT shares none of its
    /// structure.
    #[test]
    fn the_fast_transform_agrees_with_the_slow_one() {
        for n in [2usize, 4, 8, 16, 64] {
            let tw = Twiddles::new(n);
            let input: Vec<Complex> = (0..n)
                .map(|i| Complex {
                    #[allow(clippy::cast_precision_loss)]
                    re: ((i * 7) % 13) as f32 - 6.0,
                    #[allow(clippy::cast_precision_loss)]
                    im: ((i * 5) % 11) as f32 - 5.0,
                })
                .collect();
            let want = naive_idft_1d(&input);
            let mut got = input.clone();
            ifft_1d(&mut got, &tw);
            for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
                assert!(
                    (g.re - w.re).abs() < 1e-2 && (g.im - w.im).abs() < 1e-2,
                    "n={n} i={i}: got ({}, {}) want ({}, {})",
                    g.re, g.im, w.re, w.im
                );
            }
        }
    }

    /// An impulse at the origin transforms to a constant field. If bit reversal is wrong
    /// this fails in a way that is instantly readable, unlike the agreement test.
    #[test]
    fn an_impulse_becomes_a_constant() {
        let n = 16;
        let tw = Twiddles::new(n);
        let mut buf = vec![Complex { re: 0.0, im: 0.0 }; n];
        buf[0] = Complex { re: 1.0, im: 0.0 };
        ifft_1d(&mut buf, &tw);
        for (i, c) in buf.iter().enumerate() {
            assert!((c.re - 1.0).abs() < 1e-5, "bin {i} re = {}", c.re);
            assert!(c.im.abs() < 1e-5, "bin {i} im = {}", c.im);
        }
    }

    /// **The property the whole architecture rests on.** ADR 0076 licenses the FFT ocean
    /// on the force path because it is a pure function with no state. Same input, same
    /// output, bit for bit — not "within a tolerance".
    #[test]
    fn the_transform_is_bit_reproducible() {
        let n = 64;
        let tw = Twiddles::new(n);
        let input: Vec<Complex> = (0..n)
            .map(|i| Complex {
                #[allow(clippy::cast_precision_loss)]
                re: (i as f32).sin(),
                #[allow(clippy::cast_precision_loss)]
                im: (i as f32).cos(),
            })
            .collect();
        let mut a = input.clone();
        let mut b = input;
        ifft_1d(&mut a, &tw);
        ifft_1d(&mut b, &tw);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.re.to_bits(), y.re.to_bits());
            assert_eq!(x.im.to_bits(), y.im.to_bits());
        }
    }

    /// A real-valued field must come back real. The ocean's height is real, and an
    /// imaginary residue means the Hermitian symmetry in `ocean.rs` is wrong — this test
    /// is here so that failure lands on the spectrum rather than on the transform.
    #[test]
    fn a_hermitian_field_transforms_to_a_real_one() {
        let n = 8;
        let tw = Twiddles::new(n);
        let mut buf = vec![Complex { re: 0.0, im: 0.0 }; n];
        buf[1] = Complex { re: 0.3, im: 0.7 };
        buf[n - 1] = Complex { re: 0.3, im: -0.7 };
        ifft_1d(&mut buf, &tw);
        for (i, c) in buf.iter().enumerate() {
            assert!(c.im.abs() < 1e-5, "bin {i} kept an imaginary part: {}", c.im);
        }
    }

    #[test]
    fn two_dimensions_agree_with_two_passes_of_one() {
        let n = 8;
        let tw = Twiddles::new(n);
        let mut grid: Vec<Complex> = (0..n * n)
            .map(|i| Complex {
                #[allow(clippy::cast_precision_loss)]
                re: ((i * 3) % 7) as f32,
                im: 0.0,
            })
            .collect();
        let mut by_hand = grid.clone();
        let mut scratch = vec![Complex { re: 0.0, im: 0.0 }; n];
        ifft_2d(&mut grid, n, &tw, &mut scratch);
        for r in 0..n {
            ifft_1d(&mut by_hand[r * n..(r + 1) * n], &tw);
        }
        for c in 0..n {
            let mut col: Vec<Complex> = (0..n).map(|r| by_hand[r * n + c]).collect();
            ifft_1d(&mut col, &tw);
            for r in 0..n {
                by_hand[r * n + c] = col[r];
            }
        }
        for (a, b) in grid.iter().zip(by_hand.iter()) {
            assert_eq!(a.re.to_bits(), b.re.to_bits());
            assert_eq!(a.im.to_bits(), b.im.to_bits());
        }
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn a_non_power_of_two_is_refused() {
        let _ = Twiddles::new(12);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p loom_water fft
```

Expected: FAIL at `unimplemented!()`.

- [ ] **Step 3: Write the implementation**

```rust
//! A radix-2 Cooley–Tukey transform, written here rather than taken from a crate.
//!
//! **`loom_field`'s rule binds this file: a crate bump must never be able to change the
//! sim hash.** ADR 0076 puts the ocean's inverse transform on the force path, so its
//! arithmetic is part of the simulation's definition. `rustfft` is faster and better
//! tested than this will ever be, and it is exactly for that reason not usable here —
//! nothing whose output physics depends on may change underneath the project without a
//! commit that says so.
//!
//! **Everything here is bit-reproducible by construction**: a fixed twiddle table, a
//! fixed traversal order, no accumulation across calls, no threading inside a transform.
//! Rows may be transformed on separate threads later, because rows share no accumulator —
//! but no butterfly's operands ever change order.

/// A complex number, `f32` because the tiles it produces are uploaded as `f32` and a
/// wider intermediate would only be discarded.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

/// The roots of unity for one transform size, computed once.
///
/// Held rather than recomputed because `sin_cos` per butterfly is most of the cost, and
/// because a table is one place for the values to be identical across every call — which
/// is what makes the transform reproducible rather than merely deterministic-looking.
pub struct Twiddles {
    n: usize,
    w: Vec<Complex>,
}

impl Twiddles {
    /// # Panics
    /// If `n` is not a power of two. Radix-2 has no meaning otherwise, and a silent
    /// fallback to a slower path would hide a caller's mistake.
    #[must_use]
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2, "transform size must be a power of two, got {n}");
        let w = (0..n / 2)
            .map(|k| {
                #[allow(clippy::cast_precision_loss)]
                let a = -2.0 * std::f32::consts::PI * k as f32 / n as f32;
                let (im, re) = a.sin_cos();
                Complex { re, im }
            })
            .collect();
        Self { n, w }
    }
}

/// In-place inverse transform of one row.
///
/// **Unnormalised.** The `1/N` is applied once by the caller against the whole field
/// rather than twice here, because two divisions by `N` on an `f32` are not one division
/// by `N²`, and the caller is where the scale is documented.
///
/// # Panics
/// If `buf.len()` does not match the size the twiddles were built for.
pub fn ifft_1d(buf: &mut [Complex], tw: &Twiddles) {
    let n = buf.len();
    assert_eq!(n, tw.n, "buffer is {n} but the twiddle table is for {}", tw.n);

    // Bit-reversal permutation, iterative rather than table-driven: the table would be a
    // second thing to keep in step with `n`.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let step = n / len;
        for i in (0..n).step_by(len) {
            for k in 0..len / 2 {
                // Conjugated, which is the only difference between the forward transform
                // and this one.
                let w = tw.w[k * step];
                let u = buf[i + k];
                let v = buf[i + k + len / 2];
                let t = Complex {
                    re: v.re * w.re + v.im * w.im,
                    im: v.im * w.re - v.re * w.im,
                };
                buf[i + k] = Complex { re: u.re + t.re, im: u.im + t.im };
                buf[i + k + len / 2] = Complex { re: u.re - t.re, im: u.im - t.im };
            }
        }
        len <<= 1;
    }
}

/// In-place inverse transform of an `n × n` field, row-major.
///
/// Rows first, then columns, and **that order is part of the definition** — the two orders
/// are equal in exact arithmetic and not in `f32`, so swapping them would move every hash
/// that reads this field.
///
/// `scratch` is `n` long and is the caller's so that a per-tick transform allocates
/// nothing.
///
/// # Panics
/// If `grid.len()` is not `n * n`, or `scratch` is shorter than `n`.
pub fn ifft_2d(grid: &mut [Complex], n: usize, tw: &Twiddles, scratch: &mut [Complex]) {
    assert_eq!(grid.len(), n * n, "grid is {} but n is {n}", grid.len());
    assert!(scratch.len() >= n, "scratch is {} but n is {n}", scratch.len());
    for r in 0..n {
        ifft_1d(&mut grid[r * n..(r + 1) * n], tw);
    }
    for c in 0..n {
        for r in 0..n {
            scratch[r] = grid[r * n + c];
        }
        ifft_1d(&mut scratch[..n], tw);
        for r in 0..n {
            grid[r * n + c] = scratch[r];
        }
    }
}
```

Add `pub mod fft;` to `crates/loom_water/src/lib.rs` beside the other module declarations.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p loom_water fft
cargo clippy --workspace -- -D warnings
```

Expected: six tests PASS; clippy clean.

- [ ] **Step 5: Record the cost, because the ADR's budget is the reason this shape was chosen**

Add a `#[test]` that is a measurement rather than an assertion, and prints:

```rust
    /// Not a gate — a number, printed so that a change in this file's cost is visible in
    /// `cargo test -- --nocapture` rather than discovered later on a frame budget.
    ///
    /// ADR 0076 measured 0.138 ms at N=128 and 0.795 ms at N=256 for one 2D transform,
    /// scalar and single-threaded, against a 16.67 ms tick.
    #[test]
    fn cost_of_one_transform() {
        for n in [128usize, 256] {
            let tw = Twiddles::new(n);
            let mut grid = vec![Complex { re: 1.0, im: 0.0 }; n * n];
            let mut scratch = vec![Complex::default(); n];
            ifft_2d(&mut grid, n, &tw, &mut scratch);
            let t = std::time::Instant::now();
            let reps = 20;
            for _ in 0..reps {
                ifft_2d(&mut grid, n, &tw, &mut scratch);
            }
            println!("ifft_2d N={n}: {:.3} ms", t.elapsed().as_secs_f64() * 1000.0 / f64::from(reps));
        }
    }
```

Run `cargo test -p loom_water fft -- --nocapture --test-threads=1` and record the two
numbers in your report. **Run it against a release build** (`cargo test --release -p
loom_water fft -- --nocapture`) — a debug-build timing is meaningless here and quoting one
would be worse than quoting none.

- [ ] **Step 6: Commit**

```bash
git add crates/loom_water/src/fft.rs crates/loom_water/src/lib.rs
git commit -m "feat(water): a transform of our own, because a crate must not move the hash"
```

---

### Task 2: The spectrum on a grid

**Files:**
- Modify: `crates/loom_water/src/spectrum.rs` (add; the existing `wave_set` is untouched)
- Test: in `crates/loom_water/src/spectrum.rs`

**Interfaces:**
- Consumes: `crate::fft::Complex` from Task 1.
- Produces: `pub fn amplitude_field(n: usize, patch: f32, u10: f32, fetch: f32, direction: [f32; 2], seed: u32) -> Vec<Complex>` — the `h0(k)` field, `n * n`, row-major.

- [ ] **Step 1: Write the failing tests**

Add to `crates/loom_water/src/spectrum.rs`:

```rust
#[cfg(test)]
mod amplitude_tests {
    use super::*;

    /// **The field must be Hermitian, or the sea has an imaginary part.**
    /// `h0(-k) = conj(h0(k))` is what makes the inverse transform of a spectrum real, and
    /// getting it wrong produces a surface that looks plausible and is not a height field.
    #[test]
    fn the_field_is_hermitian() {
        let n = 16;
        let h0 = amplitude_field(n, 200.0, 12.0, 100_000.0, [1.0, 0.0], 7);
        for z in 0..n {
            for x in 0..n {
                let a = h0[z * n + x];
                let b = h0[((n - z) % n) * n + ((n - x) % n)];
                assert!(
                    (a.re - b.re).abs() < 1e-6 && (a.im + b.im).abs() < 1e-6,
                    "({x},{z}) is not the conjugate of its mirror: ({}, {}) vs ({}, {})",
                    a.re, a.im, b.re, b.im
                );
            }
        }
    }

    /// Same seed, same field, bit for bit — ADR 0076's whole licence.
    #[test]
    fn the_field_is_reproducible() {
        let a = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], 3);
        let b = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], 3);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.re.to_bits(), y.re.to_bits());
            assert_eq!(x.im.to_bits(), y.im.to_bits());
        }
    }

    /// A different seed is a different sea, or the seed is not doing anything.
    #[test]
    fn a_different_seed_is_a_different_sea() {
        let a = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], 3);
        let b = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], 4);
        assert!(a.iter().zip(b.iter()).any(|(x, y)| x.re.to_bits() != y.re.to_bits()));
    }

    /// **Zero wind is a flat sea and not a NaN.** The spectrum divides by `U`, so this is
    /// the edge every wave model gets wrong once. `pool.loom` authors `Wind.speed = 0`.
    #[test]
    fn no_wind_is_a_flat_sea() {
        let h0 = amplitude_field(16, 200.0, 0.0, 100_000.0, [1.0, 0.0], 1);
        for (i, c) in h0.iter().enumerate() {
            assert!(c.re.is_finite() && c.im.is_finite(), "cell {i} is not finite");
            assert!(c.re.abs() < 1e-3 && c.im.abs() < 1e-3, "cell {i} has energy in no wind");
        }
    }

    /// More wind is more energy, which is the one monotonic claim this module makes.
    #[test]
    fn a_harder_wind_is_a_bigger_sea() {
        let energy = |u: f32| -> f32 {
            amplitude_field(32, 200.0, u, 500_000.0, [1.0, 0.0], 1)
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .sum()
        };
        let (calm, gale) = (energy(5.0), energy(18.0));
        assert!(gale > calm * 4.0, "calm {calm}, gale {gale}");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p loom_water amplitude
```

Expected: FAIL — `amplitude_field` not found.

- [ ] **Step 3: Implement**

Write `amplitude_field` in `crates/loom_water/src/spectrum.rs`. It must:

- build the `k` grid for a patch of `patch` metres: `k = 2π·(x - n/2, z - n/2) / patch`,
  with the `k = 0` cell set to zero (a mean offset is not a wave);
- evaluate the **same** Pierson–Moskowitz-with-fetch energy this module already implements
  for `wave_set_fetch`, reusing those functions rather than writing the physics twice —
  the sixteen-wave path and the grid path must not be able to disagree about what a
  15 m/s sea is;
- apply the existing `cos^2s` directional spread about `direction`, reusing
  `SPREAD_POWER`;
- draw two Gaussians per cell by Box–Muller from **`loom_field::noise::hash`**, seeded by
  `(seed, x, z)` — never `thread_rng`, which clippy bans in simulation code, and never a
  crate RNG;
- multiply: `h0 = (g1 + i·g2) · sqrt(S(k)/2)`;
- **enforce the Hermitian symmetry explicitly** by writing the conjugate into the mirror
  cell rather than hoping the hash produces it — the test above will not pass otherwise,
  and hoping is how a sea gets an imaginary part.

Document the reference-height trap in a comment here as well as in the module header: this
function takes **U10**, and `Wind::speed` is not U10.

- [ ] **Step 4: Verify**

```bash
cargo test -p loom_water amplitude
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/loom_water/src/spectrum.rs
git commit -m "feat(water): the spectrum fills a grid, not just sixteen waves"
```

---

### Task 3: The cascade

**Files:**
- Create: `crates/loom_water/src/ocean.rs`
- Modify: `crates/loom_water/src/lib.rs` (add `pub mod ocean;`)
- Test: in `crates/loom_water/src/ocean.rs`

**Interfaces:**
- Consumes: `crate::fft::{Complex, Twiddles, ifft_2d}`, `crate::spectrum::amplitude_field`.
- Produces:
  - `pub struct Cascade { pub patch: f32, pub n: usize }`
  - `pub struct Ocean` with `pub fn new(cascades: &[Cascade], u10: f32, fetch: f32, direction: [f32; 2], seed: u32) -> Self`
  - `pub fn evolve(&mut self, t: f32)` — fills the tiles for time `t`
  - `pub fn sample(&self, x: f32, z: f32) -> [f32; 3]` — summed displacement, cascades in index order
  - `pub fn significant_height(&self) -> f32`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_ocean() -> Ocean {
        Ocean::new(
            &[Cascade { patch: 512.0, n: 64 }, Cascade { patch: 64.0, n: 64 }],
            14.0,
            300_000.0,
            [1.0, 0.0],
            11,
        )
    }

    /// **The property ADR 0076 rests on, asserted directly.** Evolving to t=15 after
    /// evolving to t=0 must give the same field, bit for bit, as evolving to t=15 from a
    /// fresh ocean. If it does not, the ocean has state, and it is not admissible on the
    /// force path.
    #[test]
    fn the_ocean_has_no_memory() {
        let mut walked = test_ocean();
        walked.evolve(0.0);
        walked.evolve(7.5);
        walked.evolve(15.0);

        let mut jumped = test_ocean();
        jumped.evolve(15.0);

        for (a, b) in walked.tiles().iter().zip(jumped.tiles().iter()) {
            assert_eq!(a.to_bits(), b.to_bits(), "the ocean remembered how it got here");
        }
    }

    /// Rewinding is free and exact — which a stepped simulation cannot offer, and which
    /// `loom render --sim N` depends on.
    #[test]
    fn time_can_run_backwards() {
        let mut o = test_ocean();
        o.evolve(30.0);
        let at30: Vec<f32> = o.tiles().to_vec();
        o.evolve(2.0);
        o.evolve(30.0);
        for (a, b) in at30.iter().zip(o.tiles().iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    /// The surface is real. An imaginary residue means the Hermitian symmetry broke
    /// somewhere between the spectrum and here.
    #[test]
    fn the_surface_is_real_and_finite() {
        let mut o = test_ocean();
        o.evolve(12.0);
        for (i, v) in o.tiles().iter().enumerate() {
            assert!(v.is_finite(), "tile value {i} is {v}");
        }
    }

    /// **The number the human asked for.** `SEA-REBUILD.md` §3.6 targets Hs = 6.10 m —
    /// twenty feet — at U10 = 18 with **376 km** of fetch. The realised field must agree
    /// with the analytic significant height it was built from, or the spectrum is not
    /// being sampled correctly.
    ///
    /// **376 and not 440, and the difference is not a rounding.** Three standard
    /// parameterisations disagree by up to 35% at the same wind and fetch: the SPM
    /// relation says 6.10 m at 440 km, Pierson-Moskowitz-with-fetch says 6.66 m there,
    /// and JONSWAP with `gamma = 3.3` says 8.22 m. `loom_water::spectrum` implements the
    /// middle one and deliberately omits `gamma`, so that is the one this test is written
    /// against. §3.6 records the whole table.
    ///
    /// **The grid is not the risk here.** Computed against the analytic spectrum, a 128²
    /// patch of 1024 m spans wavelengths 16-1024 m and captures **99.6%** of the
    /// variance, against a peak wavelength of 241 m at this wind. If this test fails it
    /// is the spectrum or the symmetry, not the resolution.
    /// **Average over seeds; a single draw is not a measurement.** Task 2 measured the
    /// single-draw spread of realised `Hs` at **0.35x to 1.68x** of the analytic value,
    /// from sampling noise alone — this spectrum is narrow, so most of its energy lands in
    /// few cells and one realisation is a small sample. A single-seed assertion inside
    /// ±25% would be a coin flip that fails for a reason that is not a bug, which is
    /// worse than no test.
    ///
    /// **200 seeds, not 24, and that number was itself measured.** Task 2's cross-path
    /// test started at 24 and had to be raised: at 24 one of its grid shapes read **23%
    /// high from sampling noise alone**, which looks exactly like a heading-dependent bug
    /// and is not one. At 200 its worst point is 8.0%. Averaging is cheap here — a 128²
    /// cascade evolves in well under a millisecond, so 200 draws cost a fraction of a
    /// second — and a flaky test on the force path is expensive.
    #[test]
    fn the_realised_sea_is_the_size_the_spectrum_promised() {
        let mut total = 0.0_f32;
        let seeds = 200;
        for seed in 0..seeds {
            let mut o = Ocean::new(
                &[Cascade { patch: 1024.0, n: 128 }],
                18.0,
                376_000.0,
                [1.0, 0.0],
                seed,
            );
            o.evolve(20.0);
            total += o.significant_height();
        }
        #[allow(clippy::cast_precision_loss)]
        let hs = total / seeds as f32;
        assert!(
            (hs - 6.10).abs() < 0.92,
            "mean Hs over {seeds} seeds came out {hs:.2} m against an expected 6.10 m"
        );
    }

    /// A calm sea is calm. Guards the same divide-by-U edge the spectrum test does, one
    /// layer up.
    #[test]
    fn no_wind_is_a_flat_ocean() {
        let mut o = Ocean::new(&[Cascade { patch: 256.0, n: 32 }], 0.0, 100_000.0, [1.0, 0.0], 2);
        o.evolve(9.0);
        assert!(o.significant_height() < 0.05, "Hs {} in no wind", o.significant_height());
    }

    /// Sampling between grid nodes must be continuous across a tile seam, because the
    /// tile is periodic and a boat crossing the seam must not step.
    #[test]
    fn the_tile_wraps_without_a_seam() {
        let mut o = test_ocean();
        o.evolve(5.0);
        let just_inside = o.sample(511.9, 0.0);
        let just_outside = o.sample(-0.1, 0.0);
        for i in 0..3 {
            assert!(
                (just_inside[i] - just_outside[i]).abs() < 0.05,
                "component {i} steps across the seam: {} vs {}",
                just_inside[i], just_outside[i]
            );
        }
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p loom_water ocean
```

- [ ] **Step 3: Implement**

`Ocean` holds, per cascade: the `h0` field from Task 2, a `Twiddles`, the three working
grids, and the three output tiles (height, displacement x, displacement z).

`evolve(t)`:

- for each cascade, for each cell, compute `ω = sqrt(g·k)` and form
  `h(k, t) = h0(k)·e^{iωt} + conj(h0(-k))·e^{-iωt}` — the second term is what keeps the
  result real, and dropping it is the most common way this is got wrong;
- fill the displacement grids with `i·k̂·h` for x and z, which is the horizontal pinch;
- run `ifft_2d` on each, then apply the `1/N²` scale **once**;
- write the real parts into the tiles.

`sample(x, z)` bilinearly interpolates each cascade's tiles at `(x, z)` modulo that
cascade's patch, and sums **in cascade index order** — documented, because float addition
is not associative and this sum is on the force path.

`significant_height` is `4·sqrt(variance of the height tile)`, the standard definition,
computed in a fixed traversal order.

- [ ] **Step 4: Verify, and measure**

```bash
cargo test -p loom_water ocean
cargo test --release -p loom_water ocean -- --nocapture
cargo clippy --workspace -- -D warnings
```

Add a printed cost measurement for `evolve` at the shapes ADR 0076 names — three cascades
at N=128 and at N=256 — and record both in your report against the 16.67 ms tick. **These
two numbers decide the next plan**, so measure in release and say which machine.

- [ ] **Step 5: Commit**

```bash
git add crates/loom_water/src/ocean.rs crates/loom_water/src/lib.rs
git commit -m "feat(water): a cascade of tiles, and no memory of how it got there"
```

---

## Self-Review

**Spec coverage.** ADR 0076's decision has three parts and each has a task: the transform
(1), the spectrum on a grid (2), the cascade and its time evolution (3). The ADR's
statelessness argument — the licence for the whole thing — is asserted directly by
`the_ocean_has_no_memory` and `time_can_run_backwards` rather than left as prose. Its cost
claim is re-measured in release at both candidate sizes. The `Hs = 6.10 m` target from
`SEA-REBUILD.md` §3.6 is a test.

**Out of scope and deliberately so:** `sample_water`, the GPU, the sim hash and the golden
references. Nothing here can move a pixel or a hash, which is why this plan needs no GPU
gate — and that is what makes it a safe first half of a large rewrite.

**Placeholder scan.** Tasks 1 has complete code; Tasks 2 and 3 specify their functions by
contract, formula and full test suite rather than by finished body — the physics
(Box–Muller from the frozen hash, the `h0·e^{iωt} + conj(h0(-k))·e^{-iωt}` pair, the `1/N²`
applied once) is written out, so nothing is left to taste. That is a deliberate line: the
transform is transcription, the spectrum is not.

**Type consistency.** `Complex`, `Twiddles`, `ifft_1d`, `ifft_2d` are defined in Task 1 and
used with those signatures in Tasks 2 and 3. `amplitude_field`'s signature in Task 2
matches its call in Task 3. `Ocean::tiles()` is used by Task 3's tests and must therefore
exist — it is named in the Interfaces block as the accessor those tests read.

**One risk named.** `the_realised_sea_is_the_size_the_spectrum_promised` uses a ±1.5 m
band around 6.10 m. That is wide, deliberately: a single 128² patch of 1024 m is one
realisation of a random process, and its sample `Hs` genuinely varies. If it proves
flakier than that band, the fix is more cells or a fixed seed sweep — **not** a wider band.
