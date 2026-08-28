//! The cascade: a spectrum, a transform, and an ocean that remembers nothing.
//!
//! **This is the file ADR 0076's licence rests on.** The ADR admits an FFT ocean onto
//! the force path — buoyancy reading it, `loom sim --assert` answering about it, the
//! determinism hash covering it — on exactly one argument: *the ocean is stateless, so
//! the surface at tick `t` is a pure function of (scene, t)*. Everything here is written
//! to keep that true. [`Ocean::evolve`] overwrites every cell of every working grid from
//! `t` and the immutable per-cascade tables before any of them is read, so there is no
//! path by which the surface at `t = 15` can depend on whether `t = 0` and `t = 7.5`
//! happened first. `the_ocean_has_no_memory` and `time_can_run_backwards` assert exactly
//! that, bit for bit, rather than leaving it as prose.
//!
//! # The time evolution, and why the textbook formula collapses here
//!
//! Tessendorf's form is `h(k, t) = h0(k)·e^{iωt} + conj(h0(−k))·e^{−iωt}`. Its whole job
//! is to make `h` Hermitian — `h(−k, t) = conj(h(k, t))`, which is what makes the
//! transformed surface real — out of an `h0` that is *not*, because the two halves of
//! each mirror pair are independent draws.
//!
//! [`crate::spectrum::amplitude_field`] does not hand back such an `h0`. It is Hermitian
//! **by construction**: it draws one cell per mirror pair, from whichever side the wind
//! actually reaches, and *writes* the conjugate into the other. So `conj(h0(−k))` is
//! `h0(k)`, the two terms are the same number, and the formula degenerates to
//! `2·h0(k)·cos(ωt)` — a standing sea that pulses in place and never runs downwind. It
//! is real, finite, and the right size to within a factor of two, so every test in this
//! file except the moving one would still pass.
//!
//! The two forms are reconciled by noticing what the cone in `amplitude_field` already
//! did. A Tessendorf `h0` under that directional cutoff is *zero* on the upwind side of
//! every pair, so of the formula's two terms exactly one is ever nonzero:
//!
//! ```text
//! k downwind:  h(k, t) = h0(k)·e^{−iωt}      (the other term's conj(h0(−k)) is zero)
//! k upwind:    h(k, t) = conj(h0(−k))·e^{+iωt}
//! ```
//!
//! and `conj(h0(−k))` on the upwind side is precisely the conjugate `amplitude_field`
//! stored there. So **the literal formula, evaluated against a proper Tessendorf `h0`,
//! is `field[k]·e^{i·s·ωt}` with `s = ∓1` by half-plane** — which is what
//! [`Ocean::evolve`] computes. Nothing is dropped; the conjugate term is the other
//! branch.
//!
//! The signs are `e^{−iωt}` downwind rather than `e^{+iωt}` because
//! [`crate::fft::ifft_2d`] synthesises with `e^{+ik·x}`: a mode's phase is then
//! `k·x − ωt` and it runs along `+k`, which is the side `amplitude_field`'s cone drew.
//! With the other sign every wave in the sea marches into the wind — measured, and the
//! reason `the_sea_travels_downwind` exists.
//!
//! `s` must be exactly antisymmetric or the surface stops being real. It is taken from
//! the sign of `k·ŵ`, which is exact under negation in `f32` (`(−a)·c + (−b)·d` is the
//! bit-exact negation of `a·c + b·d`), and forced to zero on the four cells that are
//! their own mirror — `k = 0` and the Nyquist row/column — because those are real and a
//! real number times `e^{iωt}` is not. Freezing three Nyquist cells rather than beating
//! them at `2cos(ωt)` is a deliberate and invisible simplification: they sit at the
//! grid's resolution limit and carry essentially no energy.
//!
//! # The scale, and the one place this deviates from the brief
//!
//! [`crate::fft::ifft_2d`] is unnormalised, and the plan's step 3 asked for a `1/N²`
//! applied once to its output. **That scale is not applied here, because it is wrong for
//! this `h0`.** `amplitude_field` is calibrated so that `Σ_k |h0(k)|²` *is* the sea's
//! variance `m0` — that is what `amplitude_field_agrees_with_wave_set_fetch` measures,
//! against `wave_set_fetch`'s sixteen waves. Parseval for this transform's convention
//! (`u(j) = Σ_k H(k)·e^{2πik·j/N}`) gives `mean_j |u|² = Σ_k |H|²`, so the *unnormalised*
//! output already has variance `m0` and `Hs = 4√m0` comes out at the size the spectrum
//! promised. Dividing by `N²` would land a twenty-foot sea at 0.37 mm.
//!
//! # The centred spectrum, and the checkerboard it would otherwise leave behind
//!
//! `amplitude_field`'s grid is **centred**: index `x` carries wavenumber
//! `2π(x − N/2)/patch`, so `k = 0` sits at `N/2` and index 0 holds the most negative
//! wavenumber. An ordinary inverse transform assumes index `x` carries wavenumber `x`, so
//! feeding a centred spectrum to it straight produces `(−1)^(jx + jz)` times the true
//! field — a modulation by the Nyquist frequency, which is a sign flip on every other
//! cell in a checkerboard.
//!
//! **That is not a shift and it is not cosmetic**, which is what an earlier draft of this
//! file claimed. It survives every test in the brief: the field stays real, stays finite,
//! stays stateless, and Parseval does not care about signs, so `Hs` comes out exactly
//! right. What it destroys is the *surface* — neighbouring cells disagree in sign, so
//! bilinear interpolation between them is meaningless and buoyancy would read a sea made
//! of alternating spikes. `the_pinch_sharpens_the_crests` is what caught it: it is the
//! only test here that asks two neighbouring cells to be consistent with each other.
//!
//! The cure is to undo the modulation on the way out, which is the same `(−1)^(jx + jz)`
//! — a sign flip per cell, applied where the real part is written into the tile. Rotating
//! the input array by `N/2` in both axes is the equivalent alternative and costs a copy.

use crate::GRAVITY;
use crate::fft::{Complex, Twiddles, ifft_2d};
use crate::spectrum::amplitude_field;

/// One tile of the ocean: an `n × n` transform over a `patch`-metre square.
///
/// Several of these summed is the cascade — a large patch for the swell and a small one
/// for the chop, each periodic on its own scale, so the sea does not visibly repeat at
/// the largest tile's period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cascade {
    /// The tile's side, in metres. Sets `Δk = 2π/patch`, so it decides which wavelengths
    /// this cascade can carry at all.
    pub patch: f32,
    /// Cells per side. **Must be a power of two** — [`Twiddles::new`] panics otherwise.
    pub n: usize,
}

/// Everything about one cascade that does not change with time.
struct Layer {
    patch: f32,
    n: usize,
    /// The frozen amplitude field, Hermitian by construction. Never written after
    /// construction: this is what makes `evolve` a pure function of `t`.
    h0: Vec<Complex>,
    twiddles: Twiddles,
    /// `∓√(g·k)` per cell — the dispersion relation with the half-plane sign already
    /// folded in (negative on the downwind side, so a mode's phase is `k·x − ωt`), which
    /// leaves `evolve` one multiply and one `sin_cos` and no branch. Zero on the four
    /// cells that are their own mirror, which freezes them real.
    omega: Vec<f32>,
    /// `k̂` per cell, zero at `k = 0`. The horizontal pinch is `i·k̂·h`.
    khat: Vec<[f32; 2]>,
    /// The three working grids — height, displacement x, displacement z — held so a
    /// per-tick evolve allocates nothing. Fully overwritten before they are read.
    grid: [Vec<Complex>; 3],
    scratch: Vec<Complex>,
    /// Where this layer's three tiles start in [`Ocean::tiles`].
    offset: usize,
}

/// A stack of FFT tiles evaluated at one time.
///
/// Construct once per sea state; call [`Self::evolve`] per tick and [`Self::sample`] per
/// query.
pub struct Ocean {
    layers: Vec<Layer>,
    /// Every layer's three tiles, concatenated in layer order: `[height, dx, dz]` per
    /// layer, each `n²` long, row-major with index `z·n + x`.
    ///
    /// One flat buffer rather than three per layer because it is what a caller uploads,
    /// hashes or walks — and because the accessor the tests read has to be a single
    /// contiguous slice for a bitwise comparison to mean "the whole ocean".
    tiles: Vec<f32>,
}

impl Ocean {
    /// Build the amplitude fields for a wind, and leave every tile zero until the first
    /// [`Self::evolve`].
    ///
    /// `u10` is the wind at 10 m, `fetch` the distance in metres it has blown over open
    /// water, `direction` the heading on XZ. See [`crate::spectrum`]'s module docs for
    /// the reference-height trap before passing anything that came out of
    /// `loom_field::wind`.
    ///
    /// # Panics
    /// If any cascade's `n` is not a power of two.
    #[must_use]
    pub fn new(
        cascades: &[Cascade],
        u10: f32,
        fetch: f32,
        direction: [f32; 2],
        seed: u32,
    ) -> Self {
        let mut layers = Vec::with_capacity(cascades.len());
        let mut offset = 0;
        for (index, cascade) in cascades.iter().enumerate() {
            let n = cascade.n;
            let cells = n * n;
            // **A different seed per cascade, or the tiles are the same sea twice.**
            // Two cascades handed the same seed draw the same standard normals at the
            // same grid indices, and summing two scaled copies of one field is one
            // field — the whole point of a cascade is that the small tile carries
            // detail the large one has never heard of.
            #[allow(clippy::cast_possible_truncation)]
            let cascade_seed = seed.wrapping_add((index as u32).wrapping_mul(0x9E37_79B9));
            let h0 = amplitude_field(n, cascade.patch, u10, fetch, direction, cascade_seed);

            let along = {
                let length = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
                if length > 0.0 && length.is_finite() {
                    [direction[0] / length, direction[1] / length]
                } else {
                    // The same fallback `amplitude_field` uses, so the half-plane split
                    // agrees with the side the energy was drawn on.
                    [1.0, 0.0]
                }
            };
            let delta_k = std::f32::consts::TAU / cascade.patch;
            #[allow(clippy::cast_precision_loss)]
            let half = (n / 2) as f32;

            let mut omega = Vec::with_capacity(cells);
            let mut khat = Vec::with_capacity(cells);
            for z in 0..n {
                for x in 0..n {
                    #[allow(clippy::cast_precision_loss)]
                    let kx = (x as f32 - half) * delta_k;
                    #[allow(clippy::cast_precision_loss)]
                    let kz = (z as f32 - half) * delta_k;
                    let k = (kx * kx + kz * kz).sqrt();

                    // Which side of the wind this cell is on. Exactly antisymmetric
                    // under `k → −k`, which is what keeps the evolved field Hermitian.
                    //
                    // **`amplitude_field` decides the same thing with the same
                    // expression**, tie-break included — one rule, one place, rather
                    // than a dot product here and an `atan2`-then-`cos` there, which is
                    // what it used to be and is two floating-point paths to one
                    // boolean.
                    //
                    // **What a disagreement would and would not cost, measured.**
                    // Flipping `amplitude_field`'s comparison outright and leaving this
                    // one alone changes no test in this file: Hermitian symmetry forces
                    // both members of a pair to share a magnitude, so which side is
                    // "live" there now only picks *which cell's hash* is drawn, and the
                    // direction the sea runs is set here and only here. So this is a
                    // hygiene fix, not a live bug — the drift it prevents is the one
                    // that bites if the cone ever goes back to being a hard cutoff
                    // (`cos.max(0)`) rather than a magnitude, which is the shape it had
                    // when picking the wrong side threw a pair's energy away.
                    let (mz, mx) = ((n - z) % n, (n - x) % n);
                    let dot = kx * along[0] + kz * along[1];
                    let sign = if (z, x) == (mz, mx) {
                        // Its own mirror — `k = 0`, or a Nyquist row/column. Hermitian
                        // symmetry forces such a cell real, and a real number times
                        // `e^{iωt}` is not. Frozen.
                        0.0
                    } else if dot > 0.0 || (dot == 0.0 && (z, x) < (mz, mx)) {
                        // The `dot == 0.0` tie-break matters only on the axis exactly
                        // across the wind, where the directional cone has already taken
                        // the amplitude to zero — but a rule that is antisymmetric
                        // *except* on a measure-zero set is a rule that is not
                        // antisymmetric.
                        1.0
                    } else {
                        -1.0
                    };

                    // **Negated, and that is the direction the sea runs.** The
                    // synthesis here is `Σ h(k)·e^{+ik·x}`, so a mode carrying
                    // `e^{+iωt}` has phase `k·x + ωt` and travels *against* `k` —
                    // straight into the wind, because `amplitude_field`'s cone puts
                    // every drawn cell on the downwind side. Physics wants
                    // `k·x − ωt`. `the_sea_travels_downwind` is the test, and it read
                    // −34.2 downwind against +23.6 upwind before this sign moved.
                    omega.push(-sign * (GRAVITY * k).sqrt());
                    khat.push(if k > 0.0 { [kx / k, kz / k] } else { [0.0, 0.0] });
                }
            }

            layers.push(Layer {
                patch: cascade.patch,
                n,
                h0,
                twiddles: Twiddles::new(n),
                omega,
                khat,
                grid: [
                    vec![Complex::default(); cells],
                    vec![Complex::default(); cells],
                    vec![Complex::default(); cells],
                ],
                scratch: vec![Complex::default(); n],
                offset,
            });
            offset += 3 * cells;
        }
        Self { layers, tiles: vec![0.0; offset] }
    }

    /// Fill every tile for simulation time `t`, in seconds.
    ///
    /// `t` is seconds since the simulation started — tick count times the fixed timestep,
    /// **never a wall clock** (never-do #8). Calling it with a `t` already visited
    /// reproduces that field bit for bit; calling it with a smaller `t` is a rewind and
    /// costs exactly what a step forward does.
    pub fn evolve(&mut self, t: f32) {
        // Destructured so the tile buffer and the layers are two disjoint borrows.
        let Self { layers, tiles } = self;
        for layer in layers.iter_mut() {
            let cells = layer.n * layer.n;
            for i in 0..cells {
                // `h(k, t) = field[k]·e^{i·s·ωt}` — see this module's docs for why that
                // is the full two-term formula and not half of it.
                let (sin, cos) = (layer.omega[i] * t).sin_cos();
                let h0 = layer.h0[i];
                let h = Complex {
                    re: h0.re * cos - h0.im * sin,
                    im: h0.re * sin + h0.im * cos,
                };
                layer.grid[0][i] = h;
                // The horizontal pinch, `i·k̂·h`: multiplying by `i` is a swap and a
                // sign. This sign is the one that *sharpens crests* — under this
                // transform's `e^{+ik·x}` convention a single mode `η = a·cos(kx)` gets
                // `D = −a·sin(kx)·k̂`, which pulls the surface toward the crest. The
                // other sign gives round crests and sharp troughs, which is water
                // upside down and is the classic way to get this wrong.
                let [kx, kz] = layer.khat[i];
                layer.grid[1][i] = Complex { re: -kx * h.im, im: kx * h.re };
                layer.grid[2][i] = Complex { re: -kz * h.im, im: kz * h.re };
            }
            for (g, grid) in layer.grid.iter_mut().enumerate() {
                ifft_2d(grid, layer.n, &layer.twiddles, &mut layer.scratch);
                // **No `1/N²`, and one sign flip.** The scale: see the module docs —
                // `amplitude_field` is calibrated in variance, so Parseval already makes
                // the unnormalised transform the right size. The sign: `amplitude_field`'s
                // spectrum is centred on `k = 0` rather than starting there, and undoing
                // that costs exactly this checkerboard. Without it the surface is the
                // true one modulated by the Nyquist frequency.
                let base = layer.offset + g * cells;
                for z in 0..layer.n {
                    for x in 0..layer.n {
                        let i = z * layer.n + x;
                        let value = grid[i].re;
                        tiles[base + i] = if (x + z) % 2 == 0 { value } else { -value };
                    }
                }
            }
        }
    }

    /// The summed displacement at a world XZ position: `[dx, height, dz]`.
    ///
    /// Ordered to match [`crate::WaterSample::displacement`], so a caller that already
    /// speaks the Gerstner path does not have to remember a second layout.
    ///
    /// **Cascades are summed in index order and only in index order.** Float addition is
    /// not associative, this sum reaches buoyancy, and the sum's order is therefore part
    /// of the surface's definition — the same rule `sample_water` follows for its waves.
    ///
    /// Each tile is periodic, so the lookup wraps in both axes, **and the bilinear
    /// interpolation wraps its upper neighbour too**: the cell at `n − 1` interpolates
    /// toward index `0`, not off the end. Without that a boat crossing the seam steps.
    #[must_use]
    pub fn sample(&self, x: f32, z: f32) -> [f32; 3] {
        let mut out = [0.0_f32; 3];
        for layer in &self.layers {
            let n = layer.n;
            #[allow(clippy::cast_precision_loss)]
            let cell = layer.patch / n as f32;
            // **Wrapped into the patch before the divide, not after.** The interpolant
            // is `fx - fx.floor()`, and an `f32` ten kilometres out on a 32 m patch has
            // ~0.001 m of resolution left in its mantissa — the fraction quantises to
            // about 1/128 of a cell, so a boat far from the origin samples a stepped
            // surface. `rem_euclid` first keeps the whole computation inside `[0, patch)`
            // where the mantissa is spent on the part that matters, and it costs nothing:
            // the wrap had to happen anyway, one line further down.
            let (fx, fz) = (x.rem_euclid(layer.patch) / cell, z.rem_euclid(layer.patch) / cell);
            let (x0, z0) = (fx.floor(), fz.floor());
            let (tx, tz) = (fx - x0, fz - z0);
            // `rem_euclid` on the integer rather than the float: a negative coordinate
            // must land on the far side of the tile, and `%` would give it a negative
            // index.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let wrap = |v: f32| -> usize { (v as i64).rem_euclid(n as i64) as usize };
            let (ix0, iz0) = (wrap(x0), wrap(z0));
            let (ix1, iz1) = ((ix0 + 1) % n, (iz0 + 1) % n);

            let cells = n * n;
            // Height is tile 0 and lands in slot 1: the return is `[dx, height, dz]`.
            for (component, out_slot) in [(0usize, 1usize), (1, 0), (2, 2)] {
                let tile = &self.tiles[layer.offset + component * cells..][..cells];
                let a = tile[iz0 * n + ix0];
                let b = tile[iz0 * n + ix1];
                let c = tile[iz1 * n + ix0];
                let d = tile[iz1 * n + ix1];
                let top = a + (b - a) * tx;
                let bottom = c + (d - c) * tx;
                out[out_slot] += top + (bottom - top) * tz;
            }
        }
        out
    }

    /// `Hs = 4√m0`, the standard significant wave height, over every cascade.
    ///
    /// The variance is the mean of squares rather than the mean of squared deviations,
    /// because the field's mean is zero by construction: `amplitude_field` zeroes the
    /// `k = 0` cell, and the DC term of an inverse transform *is* that cell.
    ///
    /// Cascades' variances add, in index order, on the same non-associativity grounds as
    /// [`Self::sample`].
    #[must_use]
    pub fn significant_height(&self) -> f32 {
        let mut m0 = 0.0_f32;
        for layer in &self.layers {
            let cells = layer.n * layer.n;
            let height = &self.tiles[layer.offset..][..cells];
            let sum: f32 = height.iter().map(|h| h * h).sum();
            #[allow(clippy::cast_precision_loss)]
            let mean = sum / cells as f32;
            m0 += mean;
        }
        4.0 * m0.sqrt()
    }

    /// Every tile, concatenated: `[height, dx, dz]` per cascade in cascade order.
    ///
    /// The whole ocean as one slice, so "did this change" is one comparison rather than
    /// a walk over a structure.
    #[must_use]
    pub fn tiles(&self) -> &[f32] {
        &self.tiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROFILE;

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
    /// twenty feet — at U10 = 18 with **440 km** of fetch. The realised field must agree
    /// with the analytic significant height it was built from, or the spectrum is not
    /// being sampled correctly.
    ///
    /// **440 km is where 6.10 m lives, and the way to know that is to ask.** This pairing
    /// is confirmed by putting the question to `spectrum` directly —
    /// `significant_height(wave_set_fetch(18.0, [1.0, 0.0], f))` answers **5.638 m at
    /// 376 km** and **6.099 m at 440 km**.
    ///
    /// **There *is* a textbook parameterisation underneath, and an earlier version of
    /// this comment denied it.** `spectrum_shape` is the SPM/CERC fetch-limited relation
    /// `Hs = 0.0016·√F̃·u10²/g` (`spectrum.rs`'s `spectrum_shape`), and
    /// `SEA-REBUILD.md` §3.6 says so. What is true is narrower and is the part worth
    /// keeping: this test previously stood at 376 km because a *different* hand formula —
    /// Pierson-Moskowitz-with-fetch — was assumed to be the one in `spectrum.rs`, and it
    /// is not. **Do not re-derive this constant from whichever fetch law comes to mind.
    /// Run the function.** A hand formula that disagrees with `spectrum.rs` is a fact
    /// about the formula, and the sea this engine builds is the one `spectrum.rs`
    /// describes.
    ///
    /// **The grid is not the risk here.** Computed against the analytic spectrum, a 128²
    /// patch of 1024 m spans wavelengths 16-1024 m and captures **99.6%** of the
    /// variance, against a peak wavelength of **226 m** at this wind and fetch. If this
    /// test fails it is the spectrum or the symmetry, not the resolution.
    ///
    /// 226 m is `spectrum_shape`'s own peak law — `ω_p = 2π·3.5·(g/u10)·F̃^-0.33`, then
    /// `λ = 2πg/ω_p²` — evaluated at 440 km. It reads 204 m at 376 km. An earlier version
    /// of this line said 241 m, which is a JONSWAP peak law this engine does not use; the
    /// margin is wide enough that it changed no conclusion, which is exactly why a stale
    /// number can sit in a comment for a review round.
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
                440_000.0,
                [1.0, 0.0],
                seed,
            );
            o.evolve(20.0);
            total += o.significant_height();
        }
        #[allow(clippy::cast_precision_loss)]
        let hs = total / seeds as f32;
        println!("realised Hs, {seeds}-seed mean at U10=18 / 440 km: {hs:.3} m (analytic 6.10)");
        // **0.25 m, and it used to be 0.92.** The wide band was absorbing a 0.46 m error
        // in the *target*, so the test passed for the wrong reason. The seed set is fixed
        // at `0..200`, so there is no run-to-run noise at all; what the band has to leave
        // room for is the spread of the 200-seed mean itself, and that is measured — the
        // realised `m0` has a coefficient of variation of **0.097**, so `Hs` has about
        // half that and the mean of 200 draws has a standard error of **0.021 m**. 0.25 m
        // is a dozen of those, and it still catches a 4% miscalibration where 0.92 m hid
        // a 7.5% one.
        assert!(
            (hs - 6.10).abs() < 0.25,
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

    /// Every heading this file's directional tests run at.
    ///
    /// **`[1, 0]` alone is a blind spot, and it is a proven one.** Every test here used
    /// to pass that and only that, which is the identical gap that let an angle-wrap bug
    /// in `spectrum.rs`'s directional cone survive a full round of review: a suite pinned
    /// to one heading cannot see a heading bug. `[-1, 0]` and `[-1, -1]` are the two whose
    /// `atan2` sits near ±π, where anything that differences two angles without wrapping
    /// falls apart, and `[-1, -1]` is additionally off-axis so a rule that happens to work
    /// on the grid axes cannot hide there either.
    const HEADINGS: [[f32; 2]; 3] = [[1.0, 0.0], [-1.0, 0.0], [-1.0, -1.0]];

    /// **Ten kilometres out is the same sea as at the origin, near enough.** Both patches
    /// here divide 512 m, so a 10,240 m offset is a whole number of tile periods and the
    /// two samples describe the same point.
    ///
    /// **Not bit-identical, and the reason is worth knowing before someone tightens this
    /// band.** `10243.7_f32` is not `3.7 + 20 patches`; an `f32` at 10 km steps by
    /// 0.98 mm, so the argument has already lost the information before `sample` sees it.
    /// Measured drift is 4.2e-5 at worst, and it is *the same* with and without the
    /// `rem_euclid` wrap in [`Ocean::sample`] (1.79e-5 against 1.82e-5 at the third
    /// point) — which is the measurement saying the far-field quantisation lives in the
    /// coordinate and not in the interpolant. 1e-4 catches a wrap that is actually broken
    /// and asserts nothing this arithmetic cannot deliver.
    #[test]
    fn the_far_field_is_the_near_field() {
        let mut o = test_ocean();
        o.evolve(8.0);
        for (x, z) in [(3.7_f32, 11.3_f32), (-6.25, 0.4), (100.1, -55.9)] {
            let near = o.sample(x, z);
            let far = o.sample(x + 10_240.0, z - 10_240.0);
            for i in 0..3 {
                assert!(
                    (near[i] - far[i]).abs() < 1e-4,
                    "component {i} at ({x}, {z}) reads {} near and {} ten km out",
                    near[i], far[i]
                );
            }
        }
    }

    /// **The sea runs downwind, and the standing-wave failure is what this catches.**
    /// The literal Tessendorf formula against this crate's Hermitian `h0` collapses to
    /// `2·h0·cos(ωt)` — real, finite, right size, and completely stationary. Every other
    /// test in this file passes under it. This one does not: a travelling sea correlates
    /// better with itself shifted *downwind* than upwind.
    ///
    /// It samples *along the wind* rather than along `+x` so that it means the same
    /// thing at every heading in [`HEADINGS`]. **Fault-injected to prove it can see a
    /// heading bug**: replacing [`Ocean::new`]'s `k · ŵ` with a bare `kx` — a half-plane
    /// that assumes the wind blows along `+x` — passes at `[1, 0]` and fails at
    /// `[-1, 0]` with +20 m correlating −11.6 against −20 m's +8.8. Pinned to one
    /// heading, this test would have reported a pass.
    #[test]
    fn the_sea_travels_downwind() {
        for dir in HEADINGS {
            let mut o = Ocean::new(&[Cascade { patch: 512.0, n: 64 }], 14.0, 300_000.0, dir, 5);
            let length = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
            let along = [dir[0] / length, dir[1] / length];
            o.evolve(0.0);
            let at = |o: &Ocean, d: f32| o.sample(d * along[0], d * along[1])[1];
            let before: Vec<f32> = (0..64_u8).map(|i| at(&o, f32::from(i) * 8.0)).collect();
            // Long enough for the dominant swell to move a few metres and no further.
            o.evolve(3.0);

            // Correlation of the old profile against the new one, shifted each way.
            let correlate = |shift: f32| -> f32 {
                before
                    .iter()
                    .enumerate()
                    .map(|(i, h)| {
                        #[allow(clippy::cast_precision_loss)]
                        let d = i as f32 * 8.0;
                        h * at(&o, d + shift)
                    })
                    .sum()
            };
            let downwind = correlate(20.0);
            let upwind = correlate(-20.0);
            assert!(
                downwind > upwind,
                "the sea is not running downwind at {dir:?}: \
                 +20 m correlates {downwind}, −20 m {upwind}"
            );
        }
    }

    /// **The pinch sharpens crests rather than troughs**, which is the sign of the `i`
    /// in `i·k̂·h` and is invisible in every other test here. A surface that displaces
    /// *away* from its crests is water upside down: rounded peaks, cusped troughs.
    ///
    /// Measured as the correlation between height and the horizontal displacement's
    /// divergence, which is negative exactly when the surface converges on its crests.
    #[test]
    fn the_pinch_sharpens_the_crests() {
        for dir in HEADINGS {
            let mut o = Ocean::new(&[Cascade { patch: 512.0, n: 64 }], 14.0, 300_000.0, dir, 7);
            o.evolve(4.0);
            let step = 4.0_f32;
            let mut covariance = 0.0_f32;
            for zi in 0..32_u8 {
                for xi in 0..32_u8 {
                    let (x, z) = (f32::from(xi) * 16.0, f32::from(zi) * 16.0);
                    let height = o.sample(x, z)[1];
                    let divergence = (o.sample(x + step, z)[0] - o.sample(x - step, z)[0]
                        + o.sample(x, z + step)[2]
                        - o.sample(x, z - step)[2])
                        / (2.0 * step);
                    covariance += height * divergence;
                }
            }
            assert!(
                covariance < 0.0,
                "the horizontal displacement spreads the crests instead of pinching them \
                 at {dir:?}: cov(h, div D) = {covariance}"
            );
        }
    }

    /// Not a gate — the two numbers that choose the shipping grid size. ADR 0076
    /// predicts ~1.24 ms at N=128 and ~7.15 ms at N=256 for nine 2D transforms, against
    /// a fixed step of 16.67 ms.
    ///
    /// **ADR 0076's figures are release figures and `cargo test` is a debug build**, so
    /// the line says which profile produced it. Unlabelled, `cargo test -p loom_water --
    /// --nocapture` printed `19.963 ms/tick (119.8% of a 16.67 ms tick)` directly beneath
    /// the paragraph above, and the only conclusion available to a reader was that the
    /// ocean does not fit in a frame. Release is 1.498 ms and 8.202 ms — it fits with
    /// room to spare. Marking both cost tests `#[ignore]` was the alternative and would
    /// have hidden the number rather than qualified it; see [`crate::PROFILE`].
    // `Instant::now` is on `clippy.toml`'s disallowed list because **simulation** must
    // not read the wall clock (never-do #8). A cost measurement is the one thing that
    // has to, and it is in `cfg(test)` where no tick can reach it.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn cost_of_evolve() {
        for n in [128usize, 256] {
            let mut o = Ocean::new(
                &[
                    Cascade { patch: 2048.0, n },
                    Cascade { patch: 256.0, n },
                    Cascade { patch: 32.0, n },
                ],
                14.0,
                300_000.0,
                [1.0, 0.0],
                3,
            );
            o.evolve(0.0);
            let reps = 20_u8;
            let start = std::time::Instant::now();
            for i in 0..reps {
                o.evolve(f32::from(i) * (1.0 / 60.0));
            }
            let each = start.elapsed().as_secs_f64() * 1000.0 / f64::from(reps);
            let percent = each * 100.0 / 16.666_666;
            println!(
                "evolve, 3 cascades at N={n}: {each:.3} ms/tick ({percent:.1}% of a 16.67 ms tick) [{PROFILE}]"
            );
        }
    }
}
