# The FFT ocean, hardened: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the FFT ocean's test suite able to fail. It currently cannot catch a wrong
dispersion relation, a wrong displacement magnitude, a transposed axis, a
cascade-count-dependent sea, or libm drift — and the suite is about to become the only
thing standing between a floating-point mistake and a boat.

**Architecture:** Four tasks. One pins golden values so arithmetic changes stop being
invisible. One asserts the physics is *right* rather than merely self-consistent. One
makes cascades partition the spectrum instead of double-counting it — a correctness fix,
not a test. One closes two alignment conventions that would silently misplace the surface.

**Tech Stack:** Rust, `loom_water`, no new dependencies.

**Spec:** ADR 0076 (`docs/decisions/0076-...md`) and `docs/design/SEA-REBUILD.md` §3.
The findings this plan answers are the "what the test suite still cannot catch" section of
the FFT core's final review, recorded in
`.superpowers/sdd/SEA-FFT-CORE-PLAN/progress.md`.

**Branch:** `sea/rebuild`.

**Why before integration.** Integration wires this ocean to `sample_water`, the GPU and the
determinism hash. After that, every defect below becomes a moved hash and a re-blessed
reference rather than a failing unit test. Harden first.

## Global Constraints

- **No new dependencies.** The transform is hand-rolled because a crate bump must never be
  able to change the sim hash; the same rule forbids a hashing crate here — write the
  small FNV-1a in the test module.
- Never `thread_rng`, `HashMap`/`HashSet` iteration, or an unannotated wall clock in
  simulation code.
- **Do not touch `spectrum.rs`'s `wave_set`, `wave_set_fetch`, `bands` or
  `significant_height`.** They are on the force path and bit-order-preserving.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, move, revert or edit them.** Never `git add -A`.
- Anything summed on the force path stays summed in a fixed, documented order.
- Green: `cargo clippy --workspace --all-targets -- -D warnings` (the form
  `scripts/green.sh` actually runs) and `cargo test -p loom_water`.
- The three-way normalisation coupling is load-bearing: `ifft_2d` is unnormalised,
  `amplitude_field` is calibrated in variance, `ocean.rs` applies no `1/N²`. Correct only
  together. Do not "fix" one.

## File Structure

| file | responsibility |
|---|---|
| `crates/loom_water/src/fft.rs` | gains a golden pin on the twiddle table. |
| `crates/loom_water/src/spectrum.rs` | gains a golden pin on one fixed-seed `amplitude_field`. `wave_set*` untouched. |
| `crates/loom_water/src/ocean.rs` | gains band-limited cascades (the one real code change), the physics assertions, and the alignment tests. |

---

### Task 1: Golden values, so arithmetic stops being invisible

Every existing test here is a self-comparison, an inequality, or a statistical band. **Not
one pins a number.** That is why the core plan's own tests caught none of its seven bugs.

**Files:** `crates/loom_water/src/fft.rs`, `crates/loom_water/src/spectrum.rs` (tests only)

**Interfaces:** none — test-module additions.

- [ ] **Step 1: Write a hash helper in each test module**

FNV-1a over `f32::to_bits`, written out rather than pulled from a crate:

```rust
    /// FNV-1a over the raw bits, so a single changed ULP anywhere changes the digest.
    ///
    /// **Hand-written rather than taken from a crate for the same reason the transform
    /// is**: this digest is what future determinism arguments will cite, and a crate bump
    /// must never be able to move it.
    fn digest(values: impl Iterator<Item = f32>) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in values {
            for b in v.to_bits().to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }
```

- [ ] **Step 2: Pin the twiddle table**

```rust
    /// **The one thing in this crate that is not IEEE-exact.** `Twiddles::new` builds from
    /// `f32::sin_cos`, which is libm and may move by a ULP across a glibc version or a
    /// different machine — and ADR 0076 sells cross-machine reproducibility of a surface
    /// that carries force. `loom_field::noise` is an integer lattice hash for exactly this
    /// reason and is compared exactly; this table cannot be, so it is pinned instead.
    ///
    /// **If this fails, do not re-bless it casually.** It means every water sim hash on
    /// this machine differs from the one that produced the recorded value, and the right
    /// response is to find out why before changing the literal.
    #[test]
    fn the_twiddle_table_is_pinned() {
        let tw = Twiddles::new(256);
        assert_eq!(digest(tw.values().iter().flat_map(|c| [c.re, c.im])), 0x0000_0000_0000_0000,
            "the twiddle table moved — read this test's doc comment before touching the literal");
    }
```

`Twiddles` currently has no accessor; add a `pub(crate) fn values(&self) -> &[Complex]`
for this. Run the test once, take the printed actual value, and put it in the literal —
that is the only acceptable way to obtain it, and record in the commit message that it was
generated on this machine.

- [ ] **Step 3: Pin one `amplitude_field`**

Same shape, in `spectrum.rs`'s test module, over
`amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], 7)`. Its doc comment must say
that this covers the `ln` and `sin_cos` in `box_muller` as well as the spectrum arithmetic,
and that a failure means the *field* moved, not necessarily that it is wrong.

- [ ] **Step 4: Verify both fail on a deliberate ULP change**

Perturb one twiddle by one ULP (`f32::from_bits(x.to_bits() + 1)`), confirm both tests
fail, revert. **Report the result** — a golden pin that cannot detect a ULP is not a pin.

- [ ] **Step 5: `cargo test -p loom_water`, `cargo clippy --workspace --all-targets -- -D warnings`, commit**

```bash
git add crates/loom_water/src/fft.rs crates/loom_water/src/spectrum.rs
git commit -m "test(water): pin the two tables libm could move under us"
```

---

### Task 2: Assert the physics, not just the self-consistency

Three defects pass the entire current suite. Each gets a test that fails against it.

**Files:** `crates/loom_water/src/ocean.rs` (tests only)

**Interfaces:** may need a test-only constructor taking an explicit `h0` field, so a
single-mode ocean can be built. If so, gate it `#[cfg(test)]` and say why.

- [ ] **Step 1: The dispersion relation's magnitude**

Scaling `ω` by any positive constant currently passes all nine ocean tests:
`the_sea_travels_downwind` asserts only a sign, `Hs` is time-invariant, and the memory and
rewind tests compare the ocean to itself. A sea with the right height and the wrong period
is invisible in a still — and period is what a hull's pitch and heave resonance are made of.

Build a **single-mode** ocean (one `k` cell with energy) and assert the surface returns to
itself after exactly one period `T = 2π/√(g·k)`:

```rust
    /// **This is the only test that pins how fast the sea moves.** Multiply `omega` by any
    /// positive constant and every other test in this file still passes.
    #[test]
    fn a_single_mode_returns_after_exactly_one_period() {
        // ... single-mode ocean at a known k ...
        let t0 = o_at(0.0);
        let period = 2.0 * std::f32::consts::PI / (GRAVITY * k).sqrt();
        assert!(close(t0, o_at(period)), "the mode did not return after one period");
        assert!(!close(t0, o_at(period * 0.5)), "the mode is period-independent — omega is not being applied");
    }
```

The second assertion is the load-bearing one: without it the test passes against a
stationary sea.

- [ ] **Step 2: The horizontal displacement's magnitude**

`the_pinch_sharpens_the_crests` asserts `cov(h, div D) < 0`, which for `D = i·k̂·h` is
`−Σ|k|·|h(k)|²` — negative for **any** positive radial factor. Replace `k̂` with `k`, or
with `100·k̂`, and it still passes. This is the field `mu_max` reads, so it is the input to
the whole foam and breaking story.

For a single mode of amplitude `A` at wavenumber `k`, the steepness is `k·A` and is
analytic. Assert the realised steepness against it, and confirm by fault injection that
scaling the displacement by 2 fails the test.

- [ ] **Step 3: Which axis is which**

A transpose anywhere in `h0 → ifft_2d → tiles → sample` passes eight of nine ocean tests —
`Hs`, seam, memory, rewind, finiteness and the pinch are all transpose-symmetric — and
reduces the ninth to a coin flip. `two_dimensions_agree_with_two_passes_of_one` cannot see
it either: it compares `ifft_2d` against `ifft_2d` written longhand.

The clean catcher: **a field with energy only in `kx` must be exactly constant along `z`.**
Assert that, then fault-inject a transpose and confirm the test fails.

- [ ] **Step 4: Verify, commit**

Each of the three must be shown to fail against the specific defect it targets. Report all
three fault-injection results; a test that was not seen to fail is not yet a test.

```bash
git add crates/loom_water/src/ocean.rs
git commit -m "test(water): the sea now has to be right, not merely consistent"
```

---

### Task 3: Cascades must partition the spectrum, not double-count it

**This is a correctness fix, and it is the reason this task is not last.**

Measured on the shipped code: adding cascades raises the realised `Hs` — 6.07 m at one,
7.61 m at two, 8.60 m at three — against a target of 6.10 m. That is recorded in the
ledger as "cascades stack variance", but stacking is the symptom. **Each cascade is
sampling the whole spectrum**, so overlapping wavenumbers are counted once per cascade.

A cascade is a *band*. Cascade `i` covers `[k_lo_i, k_hi_i)`, the bands tile `k`-space
without overlap or gap, and the sum of their energies is the spectrum's energy. Then:

**The invariant, and it is the test:** the realised `Hs` is **independent of how many
cascades the spectrum is sliced into**. One cascade, two, or three over the same total
wavenumber range must all land on the same `Hs`, within sampling noise.

**Files:** `crates/loom_water/src/ocean.rs`, `crates/loom_water/src/spectrum.rs`
(`amplitude_field` gains a band restriction; `wave_set*` untouched).

- [ ] **Step 1: Write the failing invariant test**

Three configurations spanning the same wavenumber range with one, two and three cascades;
assert their realised `Hs` agree within the sampling band Task 3 of the core plan
established (200 seeds, SEM ~0.021 m). Run it — it must fail on the current code, roughly
6.07 / 7.61 / 8.60. **Record those numbers in the report**; they are the evidence that the
fix is a fix.

- [ ] **Step 2: Give `amplitude_field` a band**

Add `k_lo` and `k_hi` bounds; cells outside are zero. The natural boundary for a cascade of
patch `L` and size `n` is its own resolvable range, but the bands must be *chosen* to tile,
not assumed to — adjacent cascades' Nyquist and fundamental do not meet by default. State
the tiling rule in a comment and make it explicit in the `Cascade` type rather than
implied by the patch sizes.

- [ ] **Step 3: Make the invariant pass**

- [ ] **Step 4: Re-derive the shipping target**

With bands tiling, the three-cascade `Hs` should land on the same 6.10 m the single
cascade does. Confirm it, and **update the core plan's single-cascade `Hs` test to run the
shipping configuration instead** — the sea that ships must be the sea that is tested.

- [ ] **Step 5: Verify, commit**

`cargo test -p loom_water`, `cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add crates/loom_water/src/ocean.rs crates/loom_water/src/spectrum.rs
git commit -m "fix(water): a cascade is a band, not another copy of the whole sea"
```

---

### Task 4: Two alignment conventions that would silently misplace the surface

**Files:** `crates/loom_water/src/ocean.rs`, `crates/loom_water/src/spectrum.rs` (docs)

- [ ] **Step 1: `sample` at an exact node must equal that node's tile value**

A half-cell offset between `sample`'s interpolation and the tile's indexing is invisible in
every existing test and would misalign the CPU surface from the uploaded GPU tile at
integration — the buoyancy-versus-rendering mismatch `loom_water`'s module docs open by
warning about. Assert `sample(node_x, node_z)` equals `tiles()[node]` for several nodes,
including on the seams.

- [ ] **Step 2: Document the frozen Nyquist cells**

`amplitude_field` forces the self-mirror cells real; the review noted this is undocumented
in the module's own docs. Say which cells (`k = 0`, and the Nyquist row and column where a
cell is its own mirror), and why they must be real.

- [ ] **Step 3: Verify, commit**

```bash
git add crates/loom_water/src/ocean.rs crates/loom_water/src/spectrum.rs
git commit -m "test(water): the surface is where the tile says it is"
```

---

## Self-Review

**Coverage.** The final review named six gaps: dispersion magnitude (Task 2 Step 1),
displacement magnitude (2.2), axis identity (2.3), multi-cascade calibration (Task 3),
cross-machine reproducibility (Task 1), and the alignment/Nyquist pair (Task 4). All six
have a task.

**The one thing that is not a test.** Task 3 changes behaviour. It is here rather than in
the integration plan because the alternative is shipping a sea whose size depends on its
cascade count, and then discovering it as a moved determinism hash.

**Placeholder scan.** Tasks 1 and 2 carry code or exact contracts; Task 3 and 4 specify
tests by their invariant and fault-injection condition rather than by finished body,
because the shape of the band restriction is the implementer's to choose and the test
follows it. Every task names what must be *seen to fail*, which is the part that matters.

**A risk worth naming.** Task 1's golden pins will fail on a different machine or a
different libm — that is their purpose, but it makes them the first tests in this crate
that are not portable. Their doc comments must say so loudly, or the next person will
re-bless a literal that is telling them something real.
