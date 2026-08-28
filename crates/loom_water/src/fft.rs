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
/// **Unnormalised, and no caller normalises it either.** There is no `1/N` anywhere on
/// this path, by design rather than by omission: `spectrum::amplitude_field` is calibrated in
/// *variance*, drawing each `h0` with the standard deviation that makes the **unnormalised**
/// transform land on the spectrum's `m0`. Parseval is what ties the two ends together — the
/// `N²` the forward-and-back pair would divide out is already absorbed into the amplitudes
/// on the way in. Dividing here would shrink the sea by `N`.
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

    /// An impulse at the origin transforms to a constant field: it proves the DC term and
    /// the overall scaling are right, and it fails in a way that is instantly readable if
    /// they are not — unlike the agreement test's per-bin diffs.
    ///
    /// **It cannot see a broken bit-reversal permutation.** `brev(0) = 0` for every
    /// transform size, so bin 0 is a fixed point of the permutation and this test passes
    /// identically whether it runs or not. `the_fast_transform_agrees_with_the_slow_one`
    /// is the one that guards the permutation — its inputs are nonzero at every bin, at
    /// sizes where the permutation is nontrivial.
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

    /// Not a gate — a number, printed so that a change in this file's cost is visible in
    /// `cargo test -- --nocapture` rather than discovered later on a frame budget.
    ///
    /// ADR 0076 measured 0.138 ms at N=128 and 0.795 ms at N=256 for one 2D transform,
    /// scalar and single-threaded, against a 16.67 ms tick.
    // `Instant::now` is on `clippy.toml`'s disallowed list because **simulation** must
    // not read the wall clock (never-do #8). A cost measurement is the one thing that
    // has to, and it is in `cfg(test)` where no tick can reach it.
    #[allow(clippy::disallowed_methods)]
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
}
