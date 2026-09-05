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
//! `s` must be exactly antisymmetric **under the mirror the transform actually uses**, or
//! the surface stops being real. That mirror is the index map `(n − x) % n`, and it is
//! `k → −k` only away from index 0: index 0 carries the Nyquist wavenumber `−(n/2)·Δk`,
//! whose negation is off the grid, so `(n − 0) % n` is 0 again and that axis' `k` comes
//! back unflipped. Away from the Nyquist row and column, `s` is the sign of `k·ŵ`, which
//! *is* exact under negation in `f32` (`(−a)·c + (−b)·d` is the bit-exact negation of
//! `a·c + b·d`). On the Nyquist row and column it is forced to zero, because no nonzero
//! antisymmetric function exists on a cell whose mirror is not its negation.
//!
//! **That is `2n − 1` frozen cells, not three, and the difference was a live bug.** An
//! earlier version froze only the four cells that are their own mirror and asserted the
//! sign was "exactly antisymmetric under `k → −k`" — which the rest of the Nyquist row
//! and column disproved: their pairs evolved on the same branch, `ifft_2d` returned a
//! complex field, and `evolve`'s `.re` discarded an imaginary residue measured at
//! **0.46% to 5.9%** of the real RMS. `the_evolved_field_has_no_imaginary_part` is the
//! test; it now reads 2.5e-7.
//!
//! Freezing them rather than beating them at `2cos(ωt)` costs nothing worth having: a
//! Nyquist mode is a standing pattern this grid cannot resolve the travel of — `+k_nyq`
//! and `−k_nyq` are one bin — and the row and column sit at the resolution limit, where
//! the spectrum has least energy. Freezing them moved the 200-seed mean `Hs` from
//! **6.072 m to 6.073 m** — measured on the single 1024 m / 128² cascade that
//! `the_realised_sea_is_the_size_the_spectrum_promised` ran at the time. **That is not
//! the configuration the test runs now.** Task 3 replaced it with `shipping_stack`'s
//! three cascades, which read 6.100 m, so a reader who runs the named test to look for
//! 6.072/6.073 finds a different sea and not a regression.
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
    /// The half-open band of wavenumbers this cascade carries, `[k_lo, k_hi)` in rad/m.
    ///
    /// **A cascade is a band, not another copy of the sea.** Left unbanded, every
    /// cascade in a stack samples the whole spectrum, so a wavenumber two grids can both
    /// carry is drawn once per cascade and the realised sea grows with the cascade count
    /// — which is a sea whose size depends on a rendering decision.
    ///
    /// **The bands must be chosen to tile, and are not implied by the patch sizes.** Two
    /// cascades' resolvable ranges do not meet: `patch` 1024 at `n` 128 reaches a Nyquist
    /// of `0.393 rad/m` while `patch` 256 at the same `n` starts its fundamental at
    /// `0.025` — they overlap over a factor of sixteen. So the tiling is stated here, in
    /// the type, and [`Ocean::new`] asserts it: consecutive cascades must satisfy
    /// `k_hi[i] == k_lo[i + 1]` exactly, or the stack has a gap or a double-count.
    /// Where the stack *starts and ends* is not asserted — a stack that stops short of
    /// the chop simply carries a smaller sea, and `significant_height` is what says so.
    ///
    /// Each cascade must also be able to *resolve* its band — `2π/patch <= k_lo` and
    /// `k_hi <= π·n/patch`, up to the corner reach — but that is a quality question
    /// rather than a correctness one and is not asserted: a band outside the grid's
    /// range is simply empty, and the energy is lost rather than misplaced.
    ///
    /// [`Cascade::whole`] is the unbanded single-cascade case.
    pub band: [f32; 2],
}

impl Cascade {
    /// A lone cascade, carrying every wavenumber its grid can reach.
    ///
    /// The right thing for one tile and the wrong thing for a stack — see [`Self::band`].
    #[must_use]
    pub fn whole(patch: f32, n: usize) -> Self {
        Self { patch, n, band: crate::spectrum::WHOLE_BAND }
    }
}

/// The wavenumber a `patch × patch` grid of `n` cells stops resolving at: `π·n/patch`.
///
/// **The boundary this file bands its stacks at**, and it is a choice rather than a
/// derivation — see [`Cascade::band`]. A coarse cascade carries everything up to what
/// it can resolve and the next one takes over exactly there, which tiles as long as the
/// finer cascade's own fundamental `2π/patch` is below it. That holds for every stack
/// here and is not automatic: it needs `patch_coarse / patch_fine <= n / 2`.
#[must_use]
pub fn nyquist(patch: f32, n: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let n = n as f32;
    std::f32::consts::PI * n / patch
}

/// Cells per side of every shipping cascade — ADR 0076's proven rung.
///
/// The ADR measured 128² × 3 cascades × 3 fields at **1.24 ms** of a 16.67 ms tick and
/// 256² at 7.15 ms, and called 128 "the proven fallback if the estimate does not survive
/// measurement". This crate's `cost_of_evolve` prints what it actually costs, now that
/// there are six fields rather than three; 256 is a constant away and is not a redesign.
pub const SHIPPING_N: usize = 128;

/// The stack the engine ships: swell, sea and chop, banded at each cascade's own
/// Nyquist so they tile `[0, ∞)`.
///
/// **One definition, used by the size test, the cost test and every scene.** They were
/// two different configurations before — the `Hs` gate ran a single 1024 m cascade while
/// the timing ran three — so the sea that was measured for size was not the sea that was
/// measured for cost, and neither was the sea that would ship.
///
/// **The layout is the engine's and not the scene's** (ADR 0076, and
/// `WaveModel::Spectrum`'s own docs): band edges that must meet *exactly* are the kind of
/// number an author gets wrong silently, and [`Ocean::new`] asserts they meet.
#[must_use]
pub fn shipping_stack(n: usize) -> [Cascade; 3] {
    let (swell, sea, chop) = (2048.0, 256.0, 32.0);
    [
        Cascade { patch: swell, n, band: [0.0, nyquist(swell, n)] },
        Cascade { patch: sea, n, band: [nyquist(swell, n), nyquist(sea, n)] },
        Cascade { patch: chop, n, band: [nyquist(sea, n), f32::INFINITY] },
    ]
}

/// The seed every scene's ocean is drawn with.
///
/// **A constant, because the sea is part of the scene and a scene is reproducible.** The
/// draw is what turns a spectrum into one particular set of crests; a seed off a clock,
/// an address or a scene id would make `loom sim --assert`, `cargo xtask repeat` and the
/// pinned hashes all answer differently on the next run or the next machine. A scene that
/// wants a different sea gets a different wind, not a different draw.
const SEA_SEED: u32 = 0x5EA_0076;

/// Everything about one cascade that does not change with time.
#[derive(Clone)]
struct Layer {
    patch: f32,
    n: usize,
    /// The longest wavelength this cascade actually carries, in metres:
    /// `2π/k_lo`, never above the patch.
    ///
    /// **Held for the renderer and for nothing else.** A cascade is a band, so the
    /// water mesh cannot fade it wave by wave the way it fades the sixteen Gerstner
    /// ones — it needs one representative length per cascade, and this is the end of
    /// the band that survives longest as the sampling coarsens. `k_lo = 0` means the
    /// cascade reaches down to its own fundamental, whose wavelength is the patch.
    longest: f32,
    /// The frozen amplitude field, Hermitian by construction. Never written after
    /// construction: this is what makes `evolve` a pure function of `t`.
    h0: Vec<Complex>,
    twiddles: Twiddles,
    /// `∓√(g·k)` per cell — the dispersion relation with the half-plane sign already
    /// folded in (negative on the downwind side, so a mode's phase is `k·x − ωt`), which
    /// leaves `evolve` one multiply and one `sin_cos` and no branch. Zero on the whole
    /// Nyquist row and column — the `2n − 1` cells whose mirror index does not carry
    /// `−k` — which is what keeps the evolved field Hermitian.
    omega: Vec<f32>,
    /// `k̂` per cell, zero at `k = 0`. The horizontal pinch is `i·k̂·h`.
    khat: Vec<[f32; 2]>,
    /// The six transformed working grids, in [`Ocean::tiles`] order — held so a per-tick
    /// evolve allocates nothing. Fully overwritten before they are read.
    grid: [Vec<Complex>; TRANSFORMED],
    scratch: Vec<Complex>,
    /// Where this layer's [`TILES_PER_CASCADE`] tiles start in [`Ocean::tiles`].
    offset: usize,
}

/// How many of a cascade's tiles come out of a transform, and their order in
/// [`Ocean::tiles`]: `height, dx, dz, vy, vx, vz`.
///
/// **Six rather than three, and the three new ones are `∂/∂t`.** A mode's time
/// evolution is `h·e^{i·s·ωt}`, so its time derivative is `i·s·ω·h` — one multiply on
/// the spectrum, and then the same inverse transform. It is done spectrally rather than
/// by differencing two ticks for the same reason [`Ocean::evolve`] is stateless: a
/// difference needs the surface at a second time, and asking for one would make the
/// velocity at tick `t` depend on which other tick was evaluated.
const TRANSFORMED: usize = 6;

/// How many `n²` tiles a cascade contributes to [`Ocean::tiles`].
///
/// The [`TRANSFORMED`] six, then five more finite-differenced from them —
/// `∂h/∂x, ∂h/∂z, Sxx, Szz, Sxz`. See [`Ocean::evolve`].
pub const TILES_PER_CASCADE: usize = TRANSFORMED + 5;

/// How many past instants [`Ocean::keep`] retains.
///
/// **Eight, and it is spray's number.** [`crate::spray::spray`] samples the surface at
/// each droplet's *birth* time, and a droplet older than
/// [`crate::spray::SPRAY_LIFETIME`] has already gone — so the ring has to reach 0.9 s
/// back and no further. Nothing else reads it, and a scene that authors no spray never
/// calls [`Ocean::keep`] at all, so it costs those scenes nothing.
///
/// The *stride* between snapshots is the caller's, because it is a tick count and this
/// crate has no tick: `Sim::evolve_sea` keeps one every `SEA_KEEP_TICKS = 8`, which puts
/// seven gaps at 0.933 s over the 0.9 s needed, and a test beside it asserts that pair
/// spans [`crate::spray::SPRAY_LIFETIME`] rather than trusting this comment.
pub const KEPT: usize = 8;

const T_HEIGHT: usize = 0;
const T_DX: usize = 1;
const T_DZ: usize = 2;
const T_VY: usize = 3;
const T_VX: usize = 4;
const T_VZ: usize = 5;
const T_DHDX: usize = 6;
const T_DHDZ: usize = 7;
const T_SXX: usize = 8;
const T_SZZ: usize = 9;
const T_SXZ: usize = 10;

/// One tile value, by component and integer cell. No wrapping: callers wrap.
#[inline]
fn cell(tiles: &[f32], offset: usize, n: usize, component: usize, x: usize, z: usize) -> f32 {
    tiles[offset + component * n * n + z * n + x]
}

/// The five differenced tiles, from the six transformed ones.
///
/// **The slopes and the breaking criterion, by central differences on the tile, once per
/// tick.** [`crate::WaterSample::mu_max`] is the largest eigenvalue of the symmetric part
/// of the horizontal Jacobian, and the matrix it is the eigenvalue of is `S = −∂D/∂x`
/// where `D` is the horizontal displacement — which is exactly what the Gerstner path's
/// `Σ Q·k·A·sin φ · dᵢdⱼ` is, term by term, differentiated in closed form. The cascade
/// has no closed form to differentiate, so it is differenced.
///
/// **What differencing costs, measured.** A central difference over one cell returns
/// `sin(k·Δ)/Δ` where the exact derivative returns `k`, per axis — so a mode at the tile's
/// Nyquist reads *zero* and the top of every cascade's band is under-read. Summed over
/// [`shipping_stack`] at `U10 = 18` and 440 km, the rms of the compression's trace is
/// **0.1276 differenced against 0.1630 exact — 78.2% kept** at `n = 128`, and 79.9% at
/// `n = 256`. So the sea reads about a fifth less steep than it is, everywhere, and the
/// foam that thresholds it is correspondingly thinner.
///
/// It is the reason each cascade differences its *own* tile at its *own* spacing rather
/// than one summed field being differenced at the finest: a cascade carries only the band
/// it can resolve, so the worst attenuation a mode sees is set by its own grid and not by
/// the coarsest in the stack.
///
/// `ponytail:` the exact form is five more inverse transforms per cascade — multiply
/// grids 0, 1 and 2 by `i·k` before transforming instead of differencing after — which is
/// **eleven transforms a tick against six**, near enough twice this file's whole cost. Do
/// that, not a wider stencil, if foam ever needs the missing 22%.
///
/// `Sxz` is the symmetric part `−½(∂dx/∂z + ∂dz/∂x)`. Analytically the two halves are
/// equal — both are `IFFT(k·k̂x·k̂z·h)` — but two different stencils on a discrete tile
/// are not, and an asymmetric matrix has no real eigenvalue guarantee at all.
fn derive(tiles: &mut [f32], offset: usize, n: usize, patch: f32) {
    #[allow(clippy::cast_precision_loss)]
    let span = 2.0 * patch / n as f32;
    for z in 0..n {
        let (zp, zm) = ((z + 1) % n, (z + n - 1) % n);
        for x in 0..n {
            let (xp, xm) = ((x + 1) % n, (x + n - 1) % n);
            let d = |component: usize, ax: usize, az: usize, bx: usize, bz: usize| {
                (cell(tiles, offset, n, component, ax, az)
                    - cell(tiles, offset, n, component, bx, bz))
                    / span
            };
            let dhdx = d(T_HEIGHT, xp, z, xm, z);
            let dhdz = d(T_HEIGHT, x, zp, x, zm);
            let sxx = -d(T_DX, xp, z, xm, z);
            let szz = -d(T_DZ, x, zp, x, zm);
            // Summed in this order and only in this order: float addition is not
            // associative and this reaches the foam a scene draws.
            let sxz = -0.5 * (d(T_DX, x, zp, x, zm) + d(T_DZ, xp, z, xm, z));
            let i = z * n + x;
            for (component, value) in
                [(T_DHDX, dhdx), (T_DHDZ, dhdz), (T_SXX, sxx), (T_SZZ, szz), (T_SXZ, sxz)]
            {
                tiles[offset + component * n * n + i] = value;
            }
        }
    }
}

/// Everything the cascade knows at one world point, every tile summed over every layer.
///
/// The fields carry [`crate::WaterSample`]'s meanings, not new ones — this is the shape
/// the FFT half of [`crate::sample_water`] reads and nothing else consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OceanSample {
    /// `[dx, height, dz]` — the order [`crate::WaterSample::displacement`] uses, so a
    /// caller that already speaks the Gerstner path needs no second layout.
    pub displacement: [f32; 3],
    /// `[vx, vy, vz]`, m/s: the orbital motion, with no current in it.
    pub velocity: [f32; 3],
    /// `[∂h/∂x, ∂h/∂z]` in *base* space — the derivative with respect to the sample
    /// point, not to the displaced position, which is what the Gerstner path's
    /// `slope_x`/`slope_z` are too.
    pub slope: [f32; 2],
    /// `Sxx` of the horizontal compression `−∂D/∂x`. See [`derive`].
    pub sxx: f32,
    /// `Szz` of the same matrix.
    pub szz: f32,
    /// `Sxz` of the same matrix, symmetrised.
    pub sxz: f32,
}

/// A stack of FFT tiles evaluated at one time.
///
/// Construct once per sea state; call [`Self::evolve`] per tick and [`Self::sample`] per
/// query.
///
/// **`Clone` copies the tiles, the tables and the kept ring, and it is how a query gets
/// one.** It is not cheap — a few megabytes at the shipping size, and up to [`KEPT`]
/// tile stacks more on a sea that keeps — and it is not how the simulation should
/// ever get one: there is one ocean in a run and the simulation owns it. It exists so
/// `loom water --at` and `water@x,z` can hold the sea the run ended on, exactly as they
/// hold the foam field and the wavelet pool.
#[derive(Clone)]
pub struct Ocean {
    layers: Vec<Layer>,
    /// Every layer's [`TILES_PER_CASCADE`] tiles, concatenated in layer order:
    /// `[height, dx, dz, vy, vx, vz, dh/dx, dh/dz, Sxx, Szz, Sxz]` per layer, each `n²`
    /// long, row-major with index `z·n + x`.
    ///
    /// One flat buffer rather than one per field because it is what a caller uploads,
    /// hashes or walks — and because the accessor the tests read has to be a single
    /// contiguous slice for a bitwise comparison to mean "the whole ocean".
    tiles: Vec<f32>,
    /// The `t` the tiles hold, or `NaN` before the first [`Self::evolve`].
    ///
    /// **Read by [`crate::sample_water`], which asserts it against its own `t`.** The
    /// tiles are the surface at one instant and nothing in them says which; a caller
    /// that evolved to one tick and sampled at another would get a plausible, wrong,
    /// silent answer — the boat-above-its-water failure this crate's header opens on.
    /// `NaN` compares false against every `t`, so an ocean nobody evolved fails that
    /// assertion rather than reading as a flat sea at `t = 0`.
    t: f32,
    /// Past tile buffers and the `t` each holds, oldest first, at most [`KEPT`] of them.
    ///
    /// **The one piece of state in this file, and it is why a spectrum sea can throw
    /// spray at all.** Everything else here is a pure function of `t` — but evaluating
    /// [`Self::evolve`] costs a whole tile stack, and spray asks about *thousands* of
    /// distinct birth instants a frame, so a droplet born 0.4 s ago cannot have its
    /// birth surface recomputed on demand: `ocean_fft_storm.loom`'s fixed step measures
    /// 4.11 ms a tick end to end.
    /// It falls out of work the fixed step has already done instead: [`Self::keep`]
    /// copies the current tiles aside, the caller strides those copies, and
    /// [`Self::at_kept`] reads the newest one at or before an asked-for instant.
    ///
    /// **Empty unless a caller asks for it**, so no existing scene grows a byte, and
    /// filled only from the fixed step — which is what makes its contents a pure
    /// function of the tick count and therefore identical under `--sim N` in one jump
    /// and under stepping there.
    kept: Vec<(f32, Vec<f32>)>,
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
    /// If any cascade's `n` is not a power of two, or if two consecutive cascades' bands
    /// do not meet — see [`Cascade::band`].
    #[must_use]
    pub fn new(
        cascades: &[Cascade],
        u10: f32,
        fetch: f32,
        direction: [f32; 2],
        seed: u32,
    ) -> Self {
        Self::with_swell(cascades, u10, fetch, direction, None, seed)
    }

    /// The same, with a **swell** summed into every cascade beside the wind sea.
    ///
    /// See [`crate::spectrum::Swell`] and [`amplitude_field`]'s "Two seas in one
    /// grid": the energies are summed, not the amplitudes, and `swell = None` is
    /// [`Self::new`] bit for bit.
    ///
    /// **The swell is summed into the same stack, not given a stack of its own.**
    /// A cascade is a band of wavenumbers rather than a sea, so the swell's
    /// energy lands in whichever cascade can resolve it — the long cascade for a
    /// groundswell, the sea and chop cascades for its tail — and the tiling
    /// assertion above keeps that from being double-counted. A second stack would
    /// double the six transforms a tick and could not be banded against the first.
    ///
    /// **One half-plane for both seas**, from
    /// [`crate::spectrum::running_direction`], which is the same vector
    /// `amplitude_field` drew the hashes on. A cell holds one complex amplitude
    /// and therefore one direction of travel, so a swell authored more than 90°
    /// off the wind is realised on the right axis running the wind's way along
    /// it — see [`crate::spectrum::Swell`] for the ceiling and the upgrade.
    #[must_use]
    pub fn with_swell(
        cascades: &[Cascade],
        u10: f32,
        fetch: f32,
        direction: [f32; 2],
        swell: Option<crate::spectrum::Swell>,
        seed: u32,
    ) -> Self {
        // **The tiling, asserted rather than assumed.** A stack whose bands overlap
        // draws the overlapping wavenumbers once per cascade and realises a sea bigger
        // than the spectrum it came from; a stack with a gap realises a smaller one.
        // Neither is visible in any picture — the sea just is the wrong size — so it is
        // checked here, at the one place a stack exists, and exactly rather than within
        // a tolerance: a band edge is a number an author writes down twice, not one that
        // arithmetic arrives at.
        //
        // **Only that consecutive bands meet.** Where the stack as a whole starts and
        // ends is the author's business — a stack that stops short of the chop loses
        // energy it never claimed to carry, which is the same class of thing as a
        // cascade whose grid cannot resolve its own band, and is checked by measuring
        // `Hs` rather than by an assertion here.
        for (i, pair) in cascades.windows(2).enumerate() {
            assert!(
                pair[0].band[1] == pair[1].band[0],
                "cascades {i} and {} do not tile: {i} ends at {} rad/m and {} starts at {}",
                i + 1,
                pair[0].band[1],
                i + 1,
                pair[1].band[0]
            );
        }

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
            let h0 = amplitude_field(
                n,
                cascade.patch,
                u10,
                fetch,
                direction,
                swell,
                cascade.band,
                cascade_seed,
            );

            // **The same function `amplitude_field` drew the hashes on**, so the
            // half-plane split here agrees with the side the energy is on — and
            // when the wind has died and only a swell is left, both follow the
            // swell rather than a wind direction that means nothing. This was
            // the same normalisation written out inline in both files, which was
            // one rule in two places the moment there were two seas to choose
            // between.
            let along = crate::spectrum::running_direction(u10, fetch, direction, swell);
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

                    // **The one place the two conventions in this file are reconciled.**
                    // `amplitude_field` pairs cell `(z, x)` with `((n − z) % n,
                    // (n − x) % n)` and writes the conjugate there; the `k` grid is
                    // `(index − n/2)·Δk`. Those agree — the mirror index carries `−k` —
                    // everywhere except index 0, which holds the Nyquist wavenumber
                    // `−(n/2)·Δk`, whose negation is off the grid: `(n − 0) % n` is 0
                    // again, so that axis' `k` comes back *unflipped*.
                    //
                    // So on the Nyquist row and column — `2n − 1` cells — the grid cannot
                    // represent `−k` at all, and **every quantity that must be odd in
                    // `k` is therefore zero there.** Two are: the half-plane sign folded
                    // into `omega`, and `khat`. Both are handled by the single flag
                    // below rather than by a case each, because they fail for one reason.
                    //
                    // What it costs to get wrong, measured: with only the four
                    // *self*-mirror cells frozen, mirror pairs on that row and column
                    // evolve on the same branch, `ifft_2d` returns a complex field, and
                    // the `.re` in `evolve` silently discards an imaginary residue of
                    // 4.6e-3 to 5.9e-2 of the real RMS.
                    // `the_evolved_field_has_no_imaginary_part` is the test.
                    //
                    // Freezing them is honest rather than a patch: `+k_nyq` and `−k_nyq`
                    // are one bin, so a Nyquist mode is a standing pattern whose travel
                    // this grid cannot resolve, and it sits where the spectrum has least
                    // energy. `k = 0` needs no case of its own — `√(g·0)` is already zero
                    // and `khat` is already guarded on `k > 0`.
                    let mirror_carries_minus_k = x != 0 && z != 0;
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
                    let sign = if !mirror_carries_minus_k {
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
                    khat.push(if k > 0.0 && mirror_carries_minus_k {
                        [kx / k, kz / k]
                    } else {
                        // Odd in `k`, so it is zero wherever the mirror cannot carry
                        // `−k` — see above. The pinch `i·k̂·h` is Hermitian only because
                        // `k̂(−k) = −k̂(k)`; leaving `k̂` alone here and freezing `ω`
                        // alone fixes the height field and leaves the two displacement
                        // grids carrying a 5.5e-2 imaginary residue, which is what
                        // `the_evolved_field_has_no_imaginary_part` reported at the
                        // half-done fix. The Nyquist row and column therefore carry no
                        // horizontal pinch, which is the same simplification as
                        // carrying no travel.
                        [0.0, 0.0]
                    });
                }
            }

            // The band's low end as a wavelength, clamped to the patch: a tile cannot
            // carry a wave longer than its own fundamental however low the band
            // reaches, and `k_lo = 0` has no wavelength at all.
            let longest = if cascade.band[0] > 0.0 {
                (std::f32::consts::TAU / cascade.band[0]).min(cascade.patch)
            } else {
                cascade.patch
            };
            layers.push(Layer {
                patch: cascade.patch,
                n,
                longest,
                h0,
                twiddles: Twiddles::new(n),
                omega,
                khat,
                grid: std::array::from_fn(|_| vec![Complex::default(); cells]),
                scratch: vec![Complex::default(); n],
                offset,
            });
            offset += TILES_PER_CASCADE * cells;
        }
        Self { layers, tiles: vec![0.0; offset], t: f32::NAN, kept: Vec::new() }
    }

    /// The ocean a scene's water body asks for, or `None` for one that did not ask.
    ///
    /// # Who owns this, and who evolves it
    ///
    /// **The simulation owns it: `loom_cli::play::Sim`, beside the wavelet pool and the
    /// foam field, evolved once at the top of every fixed step before any body reads the
    /// surface.** Nothing else evolves one. `loom water --at` and `water@x,z` assertions
    /// read the sim's, cloned off the runner exactly as they clone the foam field; the
    /// renderer will read the sim's too.
    ///
    /// **It anchors to the water body and never to the camera** — ADR 0045's stated
    /// trap, which binds here for the same reason it binds the foam field: this surface
    /// produces a force on a rapier body, and a domain that followed the viewer would
    /// make physics depend on where somebody stood. There is in fact nothing to anchor:
    /// [`Self::sample`] wraps a world XZ into the patch, so the tiles are pinned to world
    /// coordinates and the sea is the same sea at every distance from every eye. Any
    /// future windowing of this — a moving high-detail region, a per-camera cascade — is
    /// the trap, not an optimisation.
    ///
    /// **`u10` and `direction` are the wind's, and they must be the same two numbers the
    /// sixteen-wave path is derived from** (`loom_cli::weather::water_of`), or a scene
    /// reports one sea and floats on another. `u10` is the wind at 10 m — never
    /// `Wind::speed`, which is a free-stream value about 10% above it; see
    /// [`crate::spectrum`]'s reference-height trap.
    ///
    /// **The sea does not ride the weather ladder yet.** `Sim::reweather` re-derives the
    /// sixteen waves from a ramped wind every tick; rebuilding the amplitude field costs
    /// three `amplitude_field` passes and cannot be done per tick, so a `spectrum` body
    /// keeps the sea its scene's wind built at load. No scene both ramps and asks for the
    /// spectrum; when one does, the ramp is a slice of its own.
    ///
    /// **`u10`, `fetch` and `direction` are the wind sea's; the swell brings its own
    /// three.** `WaterBody::swell` is the authored remnant of a wind that blew
    /// somewhere else, so it takes nothing from the scene's `Wind` — that division is
    /// the physical one and it is what makes the default free: a body with no swell
    /// passes `None` here, [`amplitude_field`] sums `0.0 + x`, and every scene written
    /// before the field existed realises exactly the surface it did.
    ///
    /// A swell on a `gerstner` body is refused at load rather than dropped here
    /// (`loom_scene`'s `swell_on_gerstner_water`), so the `None` this function returns
    /// for one can never be a swell going quietly missing.
    #[must_use]
    pub fn for_body(
        body: &loom_scene::components::WaterBody,
        u10: f32,
        direction: [f32; 2],
    ) -> Option<Self> {
        if body.wave_model != loom_scene::components::WaveModel::Spectrum {
            return None;
        }
        // Absent is unlimited fetch — the fully-developed sea — on both seas.
        // `spectrum_shape` reads a non-finite fetch as unlimited, which is the same arm
        // `wave_set` takes for the same `None`.
        let unlimited = |fetch: Option<f32>| fetch.unwrap_or(f32::INFINITY);
        Some(Self::with_swell(
            &shipping_stack(SHIPPING_N),
            u10,
            unlimited(body.fetch),
            direction,
            body.swell.map(|s| crate::spectrum::Swell {
                u10: s.u10,
                fetch: unlimited(s.fetch),
                direction: s.direction,
            }),
            SEA_SEED,
        ))
    }

    /// Fill every tile for simulation time `t`, in seconds.
    ///
    /// `t` is seconds since the simulation started — tick count times the fixed timestep,
    /// **never a wall clock** (never-do #8). Calling it with a `t` already visited
    /// reproduces that field bit for bit; calling it with a smaller `t` is a rewind and
    /// costs exactly what a step forward does.
    pub fn evolve(&mut self, t: f32) {
        // Destructured so the tile buffer and the layers are two disjoint borrows.
        let Self { layers, tiles, t: at, kept: _ } = self;
        *at = t;
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
                // **The velocity, differentiated on the spectrum rather than in time.**
                // `∂h/∂t` of `h0·e^{i·s·ωt}` is `i·s·ω·h`, so the vertical rate is one
                // multiply on the mode this loop already built, and the two horizontal
                // rates are that same `∂h/∂t` put through the identical pinch. Three
                // more inverse transforms and no second instant, which is what keeps
                // the velocity as stateless as the surface.
                let dh = Complex { re: -layer.omega[i] * h.im, im: layer.omega[i] * h.re };
                layer.grid[3][i] = dh;
                layer.grid[4][i] = Complex { re: -kx * dh.im, im: kx * dh.re };
                layer.grid[5][i] = Complex { re: -kz * dh.im, im: kz * dh.re };
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
            derive(tiles, layer.offset, layer.n, layer.patch);
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
        let p = Self::probe(&self.layers, &self.tiles, x, z);
        [p[T_DX], p[T_HEIGHT], p[T_DZ]]
    }

    /// Everything the cascade knows at a world XZ, in one lookup.
    ///
    /// The same bilinear taps [`Self::sample`] makes, over all
    /// [`TILES_PER_CASCADE`] tiles rather than three — which is one query per
    /// [`crate::sample_water`] call rather than four, and one place where the wrap and
    /// the cascade order are decided.
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> OceanSample {
        Self::read(&self.layers, &self.tiles, x, z)
    }

    /// [`Self::at`] against a given tile buffer, which is the current one or one
    /// [`Self::keep`] set aside. The layers are the same either way: a snapshot is the
    /// tiles alone, because everything else about a cascade is frozen at construction.
    fn read(layers: &[Layer], tiles: &[f32], x: f32, z: f32) -> OceanSample {
        let p = Self::probe(layers, tiles, x, z);
        OceanSample {
            displacement: [p[T_DX], p[T_HEIGHT], p[T_DZ]],
            velocity: [p[T_VX], p[T_VY], p[T_VZ]],
            slope: [p[T_DHDX], p[T_DHDZ]],
            sxx: p[T_SXX],
            szz: p[T_SZZ],
            sxz: p[T_SXZ],
        }
    }

    /// The simulation time the tiles hold, or `NaN` before the first [`Self::evolve`].
    #[must_use]
    pub fn evolved_at(&self) -> f32 {
        self.t
    }

    /// Copy the instant the tiles currently hold into the ring [`Self::at_kept`] reads.
    ///
    /// **Called from the fixed step and from nowhere else**, on a stride the caller
    /// chooses — see [`KEPT`]. Called with the same `t` twice it stores it twice, which
    /// is not a case the fixed step can produce and not one worth a branch: the caller
    /// evolves once per tick and keeps on a tick count.
    ///
    /// Steady state allocates nothing: the oldest buffer is taken off the front, filled
    /// from the current tiles and pushed back on. `KEPT` is eight, so the shift is seven
    /// pointer moves and the copy is one `memcpy` of the tile stack.
    pub fn keep(&mut self) {
        if self.kept.len() == KEPT {
            let mut oldest = self.kept.remove(0);
            oldest.0 = self.t;
            oldest.1.copy_from_slice(&self.tiles);
            self.kept.push(oldest);
        } else {
            self.kept.push((self.t, self.tiles.clone()));
        }
    }

    /// Everything the cascade knew at a world XZ at the newest kept instant **at or
    /// before** `t` — [`Self::at`], asked about the past.
    ///
    /// **The answer is snapped back to the ring's stride, never interpolated**, so what
    /// comes out is a surface this simulation genuinely held rather than a blend of two
    /// it held. The caller that wants it — [`crate::spray::spray`] — is choosing which
    /// crests threw and where the crown left from, and one stride's worth of lag is
    /// small against a crown: measured on `ocean_fft_storm.loom` at (3, −7) across one
    /// eight-tick stride, the water moved 0.34 m horizontally and 9 mm vertically,
    /// under a quarter of a `SPRAY_CELL`.
    ///
    /// `None` when the ring does not reach back that far, which is a run's first second
    /// and a sea nobody called [`Self::keep`] on. A caller that gets `None` throws
    /// nothing; it must not fall back to the current tick, which would put a droplet's
    /// launch point on water it is no longer touching.
    #[must_use]
    pub fn at_kept(&self, t: f32, x: f32, z: f32) -> Option<OceanSample> {
        // Oldest first and short, so the newest match is the last one that qualifies.
        let (_, tiles) = self.kept.iter().rev().find(|(kept, _)| *kept <= t)?;
        Some(Self::read(&self.layers, tiles, x, z))
    }

    fn probe(layers: &[Layer], tiles: &[f32], x: f32, z: f32) -> [f32; TILES_PER_CASCADE] {
        let mut out = [0.0_f32; TILES_PER_CASCADE];
        for layer in layers {
            let n = layer.n;
            #[allow(clippy::cast_precision_loss)]
            let cell = layer.patch / n as f32;
            // **Wrapped into the patch before the divide.** Not for the sign — `wrap`
            // below is `rem_euclid` on `i64` and already sends a negative index to the
            // far side of the tile, so an earlier version of this line credited this
            // `rem_euclid` with work that was already done. What it does buy is *range*:
            // `f32 as i64` saturates in Rust rather than wrapping, so a coordinate whose
            // `x / cell` exceeds `i64::MAX` would floor, cast to `i64::MAX` and index a
            // constant cell for every such `x` — measured, `1e20_f32 as i64` is
            // `9223372036854775807`. This keeps `fx` in `[0, n)`, so the cast never sees
            // a number it cannot hold. Delete it and that returns.
            //
            // **It does not buy far-field accuracy, and an earlier version of this
            // comment claimed it did** — a mantissa argument about a boat ten
            // kilometres out sampling a stepped surface. Measured over 200
            // realisations of `test_ocean`, the near/far disagreement is
            // median 5.05e-5 / p90 8.99e-5 / max 1.52e-4 **with** this wrap and
            // median 5.01e-5 / p90 8.99e-5 / max 1.51e-4 **without** it. Identical, and
            // `the_far_field_is_the_near_field`'s own docs already said so. The
            // quantisation lives in the `f32` coordinate before `sample` is called:
            // `10243.7_f32` is not `3.7 + 20` patches, and no arithmetic here can put
            // that information back.
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
            for (component, slot) in out.iter_mut().enumerate() {
                let tile = &tiles[layer.offset + component * cells..][..cells];
                let a = tile[iz0 * n + ix0];
                let b = tile[iz0 * n + ix1];
                let c = tile[iz1 * n + ix0];
                let d = tile[iz1 * n + ix1];
                let top = a + (b - a) * tx;
                let bottom = c + (d - c) * tx;
                *slot += top + (bottom - top) * tz;
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

    /// The largest [`crate::WaterSample::fold`] this sea can reach right now, anywhere.
    ///
    /// On the spectrum path `fold` is `Sxx + Szz` summed over the stack, and
    /// [`Self::probe`] interpolates a cascade's tiles bilinearly — a convex combination
    /// of four cells, so no sample of a cascade exceeds that cascade's largest cell.
    /// The per-cascade maxima summed is therefore a true ceiling on the fold at every
    /// world point, which is what [`crate::spray::peak_fold`] is for a wave list.
    ///
    /// **It is not the same kind of number as that one, and the difference is the whole
    /// reason this is documented rather than merely written.** `Σ Q·k·A` is *analytic*
    /// and over all time: it is true before a tick has run and it stays true forever, so
    /// a Gerstner sea under [`crate::spray::SPRAY_BREAK`] can be told it will never
    /// break. This is a ceiling over **the tiles that exist now** — one instant of one
    /// draw. A cascade under the threshold this second is a sea that is not breaking
    /// *yet*, not a sea that cannot; the honest sentence a caller can build on it is
    /// "this sea is not breaking", never "this sea will never break".
    ///
    /// `NaN` before the first [`Self::evolve`], for [`Self::evolved_at`]'s reason: the
    /// tiles are zero then, and a flat sea and a sea nobody has evolved yet must not
    /// answer the same. A comparison against `NaN` is false, so a caller that forgets
    /// stays quiet rather than warning about a sea it has not looked at.
    ///
    /// Cascades add in index order, on the same non-associativity grounds as
    /// [`Self::sample`]. **One walk of two tiles per cascade** — the same cost as
    /// [`Self::significant_height`] — so ask it when an author needs an answer, not
    /// every frame.
    #[must_use]
    pub fn peak_fold(&self) -> f32 {
        if self.t.is_nan() {
            return f32::NAN;
        }
        let mut ceiling = 0.0_f32;
        for layer in &self.layers {
            let cells = layer.n * layer.n;
            let sxx = &self.tiles[layer.offset + T_SXX * cells..][..cells];
            let szz = &self.tiles[layer.offset + T_SZZ * cells..][..cells];
            ceiling += sxx
                .iter()
                .zip(szz)
                .map(|(sxx, szz)| sxx + szz)
                .fold(f32::NEG_INFINITY, f32::max);
        }
        ceiling
    }

    /// Every tile, concatenated: all [`TILES_PER_CASCADE`] of them per cascade, in
    /// cascade order — the layout [`Self::tiles`]' own field documents.
    ///
    /// The whole ocean as one slice, so "did this change" is one comparison rather than
    /// a walk over a structure.
    ///
    /// (This sentence used to say `[height, dx, dz]`, which was true when a cascade
    /// carried three tiles and has been false since it carried eleven.)
    #[must_use]
    pub fn tiles(&self) -> &[f32] {
        &self.tiles
    }

    /// The tiles the water mesh draws from, and the numbers it needs to index them.
    ///
    /// **Eight tiles of the eleven, and the three left behind are the velocity
    /// triple.** Nothing on the GPU reads a water velocity: `WaterOutput` carries no
    /// such field and `waterVertexMain` already passes `float3(0, 0, 0)` where the
    /// current would go, with a comment saying the surface a river is drawn at is the
    /// surface a still lake is drawn at. **This is an upload decision and not a model
    /// decision** — the CPU keeps all eleven, because buoyancy's drag reads
    /// [`crate::WaterSample::velocity`], so the day something on the GPU wants the
    /// orbital motion the tiles are already there and only this function changes.
    ///
    /// Order per cascade, which is [`super::slang`]'s `LOOM_OCEAN_TILES` order and the
    /// only place it is written down for the upload:
    /// `height, dx, dz, ∂h/∂x, ∂h/∂z, Sxx, Szz, Sxz`.
    ///
    /// **A stack whose cascades do not share one `n` returns nothing at all**, because
    /// the upload indexes every cascade at one stride and a per-cascade offset table
    /// would be three more numbers in the environment buffer for a stack this engine
    /// does not build — [`shipping_stack`] is uniform and is the only stack there is.
    /// An empty return is water with no cascade, and the mesh then draws the Gerstner
    /// sum exactly as it always did.
    #[must_use]
    pub fn render_tiles(&self) -> RenderTiles {
        let Some(first) = self.layers.first() else {
            return RenderTiles::default();
        };
        if self.layers.iter().any(|l| l.n != first.n) {
            return RenderTiles::default();
        }
        let n = first.n;
        let cells = n * n;
        let mut tiles = Vec::with_capacity(self.layers.len() * RENDER_TILES_PER_CASCADE * cells);
        for layer in &self.layers {
            // Two runs rather than eight, because the kept tiles are contiguous either
            // side of the velocity triple: 0..3 then 6..11.
            tiles.extend_from_slice(&self.tiles[layer.offset..][..T_VY * cells]);
            tiles.extend_from_slice(
                &self.tiles[layer.offset + T_DHDX * cells..]
                    [..(TILES_PER_CASCADE - T_DHDX) * cells],
            );
        }
        RenderTiles {
            tiles,
            patch: self.layers.iter().map(|l| l.patch).collect(),
            longest: self.layers.iter().map(|l| l.longest).collect(),
            n,
        }
    }
}

/// How many of a cascade's [`TILES_PER_CASCADE`] tiles the renderer is given.
///
/// See [`Ocean::render_tiles`] for which eight and why the other three stay home.
pub const RENDER_TILES_PER_CASCADE: usize = 8;

/// What the water mesh needs to draw a cascade — [`Ocean::render_tiles`].
///
/// A struct rather than a tuple because four returns of two different shapes is
/// exactly the arrangement a caller gets wrong silently.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderTiles {
    /// [`RENDER_TILES_PER_CASCADE`] tiles of `n²` per cascade, in cascade order.
    pub tiles: Vec<f32>,
    /// Each cascade's patch, in metres — the wrap the GPU's lookup does.
    pub patch: Vec<f32>,
    /// Each cascade's longest carried wavelength, in metres. **The mesh's Nyquist
    /// fade reads this and nothing else**: see `Layer::longest`.
    pub longest: Vec<f32>,
    /// Cells per side, shared by every cascade.
    pub n: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROFILE;

    fn test_ocean() -> Ocean {
        let cut = nyquist(512.0, 64);
        Ocean::new(
            &[
                Cascade { patch: 512.0, n: 64, band: [0.0, cut] },
                Cascade { patch: 64.0, n: 64, band: [cut, f32::INFINITY] },
            ],
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

    /// **The surface is real, and `evolve` throws away the evidence when it is not.**
    ///
    /// `the_surface_is_real_and_finite` reads [`Ocean::tiles`], which is the `.re` of the
    /// transform's output — so a field that came out complex looks perfectly finite and
    /// perfectly plausible there, with its imaginary half already discarded. This reads
    /// the working grids instead, which after [`Ocean::evolve`] hold `ifft_2d`'s complex
    /// output verbatim, and asserts the imaginary part is at the noise floor relative to
    /// the real one.
    ///
    /// **What it caught.** `omega`'s half-plane sign was forced to zero only on the four
    /// cells that are their own mirror, and `khat` was not forced anywhere. But the
    /// mirror index `(n − x) % n` carries `−k` only when `x != 0`: index 0 holds the
    /// Nyquist wavenumber, whose negation is off the grid, so every cell of the Nyquist
    /// row and column mirrors to one whose `k` is *not* `−k`. Their `ω` did not flip
    /// sign, the pair evolved on one branch, and the field stopped being Hermitian.
    ///
    /// Measured before the fix, over the nine (size, heading) pairs below: **4.6e-3 to
    /// 5.9e-2** of the real RMS, worst at the smallest grid because the frozen set is
    /// `2n − 1` cells of `n²`. At `n = 64` the height grid alone reads 1.6e-2 to 1.9e-2.
    /// After: **2.5e-7**, which is `f32` transform noise.
    ///
    /// **Both halves of the fix are needed and this test says which.** Freezing `omega`
    /// alone takes grid 0 to the noise floor and leaves grids 1 and 2 — the horizontal
    /// pinch `i·k̂·h` — at 5.5e-2, because `k̂` is odd in `k` for the same reason `ω`'s
    /// sign is. That intermediate state still passes `the_surface_is_real_and_finite`.
    ///
    /// The band is 1e-5: about 1.6 orders above the realised 2.5e-7 and 2.7 below the
    /// smallest defect reading, so it is neither flaky nor able to miss the bug's return.
    #[test]
    fn the_evolved_field_has_no_imaginary_part() {
        let mut worst = 0.0_f32;
        for n in [32usize, 64, 128] {
            for dir in HEADINGS {
                let mut o = Ocean::new(&[Cascade::whole(512.0, n)], 14.0, 300_000.0, dir, 9);
                o.evolve(7.0);
                let layer = &o.layers[0];
                for (g, grid) in layer.grid.iter().enumerate() {
                    // Summed in `f64` because the ratio is the point: an `f32`
                    // accumulator over 16k squares is itself at the 1e-7 the fixed
                    // field reads, and the test would then be measuring its own sum.
                    let (re, im) = grid.iter().fold((0.0_f64, 0.0_f64), |(a, b), c| {
                        (a + f64::from(c.re) * f64::from(c.re), b + f64::from(c.im) * f64::from(c.im))
                    });
                    assert!(re > 0.0, "grid {g} at n={n}, {dir:?} is empty — a vacuous pass");
                    #[allow(clippy::cast_possible_truncation)]
                    let ratio = (im / re).sqrt() as f32;
                    worst = worst.max(ratio);
                    assert!(
                        ratio < 1e-5,
                        "grid {g} at n={n}, heading {dir:?} carries an imaginary residue of \
                         {ratio:.3e} of its real RMS — the evolved field is not Hermitian"
                    );
                }
            }
        }
        println!("worst imaginary/real RMS over the evolved grids: {worst:.3e}");
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
    /// **It runs [`shipping_stack`], and that is the point of running it.** A gate on the
    /// sea's size that measures a configuration nothing ships is a gate on a different
    /// sea; this used to run one 1024 m cascade and read **6.073 m**, three centimetres
    /// light because a single grid cannot carry both the 226 m swell and the chop. The
    /// three tiling cascades read **6.100 m** against the analytic 6.10.
    ///
    /// **The grid is not the risk here.** The stack's longest wave is the swell
    /// cascade's own **2048 m** and its shortest is **0.354 m** — the corner of the chop
    /// cascade, `k = 17.77` rad/m on a 32 m / 128 grid, with 0.50 m along its axes. 4 m
    /// is where the chop's band *opens* (`k_lo = π·128/256 = 1.5708`), not where the stack
    /// stops, and an earlier version of this line quoted it as the latter. Against a peak
    /// wavelength of **226 m** at this wind and fetch the peak sits at index 9 of the
    /// swell cascade — comfortably resolved either way. If this test fails it is the
    /// spectrum, the banding or the symmetry, not the resolution.
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
    /// and is not one. At 200 its worst point is 8.0%. Averaging is cheap here — three
    /// 128² cascades evolve in 1.5 ms in release, so 200 draws cost a few seconds even in
    /// this debug build — and a flaky test on the force path is expensive.
    #[test]
    fn the_realised_sea_is_the_size_the_spectrum_promised() {
        let mut total = 0.0_f32;
        let seeds = 200;
        for seed in 0..seeds {
            let mut o = Ocean::new(&shipping_stack(128), 18.0, 440_000.0, [1.0, 0.0], seed);
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

    /// **A cascade is a band, and the sea is the same size however many bands it is
    /// sliced into.** Left unbanded, every cascade samples the whole spectrum, so a
    /// wavenumber two grids can both carry is drawn once per cascade and the realised
    /// `Hs` grows with the cascade count — a sea whose size depends on a rendering
    /// decision, which is the defect this asserts away.
    ///
    /// The three stacks below tile the **same** wavenumber range, `[0, π·128/1024)`, into
    /// one, two and three bands, and every cascade can resolve the top of the band it is
    /// given (`π·n/patch >= k_hi`, which holds for all six cascades here). The matching
    /// lower condition `2π/patch <= k_lo` is **not** true and an earlier version of this
    /// line stated it as if measured: the leading cascade of all three stacks has
    /// `k_lo = 0`, which no grid's `Δk` is at or below. It is vacuous rather than wrong —
    /// there is nothing below `k_lo` for that cascade to fail to resolve — but it is not
    /// a fact about these stacks. So there is one right answer for all three and it is
    /// the energy the spectrum puts in that range.
    ///
    /// **Measured with the bands replaced by [`crate::spectrum::WHOLE_BAND`]** — which is
    /// exactly the shipped behaviour, every cascade sampling everything — the same three
    /// stacks read **6.073 / 8.598 / 10.559 m** — the sea grows by three quarters when it
    /// is drawn in three parts. Banded they read **6.069 / 6.096 / 6.128 m**.
    ///
    /// **200 seeds, and the band is derived not guessed.** A single realisation of this
    /// spectrum spreads over 0.35x to 1.68x, so one draw is not a measurement;
    /// `the_realised_sea_is_the_size_the_spectrum_promised` measured the 200-seed mean's
    /// standard error at 0.021 m and the three stacks here are independent draws, so a
    /// difference of two means has about 0.03 m of standard error. The realised spread
    /// is 0.059 m, so 0.10 m is about three of those — wide enough not to flake, and a
    /// twenty-fifth of the 2.53 m the two-cascade double-count moves.
    #[test]
    fn cascades_are_a_band_not_another_copy_of_the_sea() {
        let cut_hi = nyquist(1024.0, 128);
        // Interior edges, chosen so each cascade resolves its own band. They are written
        // as multiples of the coarsest cascade's own `Δk` because a band edge is a number
        // an author picks, and picking one the coarse grid already has a cell at is the
        // one choice that costs nothing.
        let dk = std::f32::consts::TAU / 1024.0;
        let (two_cut, three_a, three_b) = (16.0 * dk, 8.0 * dk, 32.0 * dk);
        let stacks: [Vec<Cascade>; 3] = [
            vec![Cascade { patch: 1024.0, n: 128, band: [0.0, cut_hi] }],
            vec![
                Cascade { patch: 1024.0, n: 128, band: [0.0, two_cut] },
                Cascade { patch: 256.0, n: 128, band: [two_cut, cut_hi] },
            ],
            vec![
                Cascade { patch: 1024.0, n: 128, band: [0.0, three_a] },
                Cascade { patch: 512.0, n: 128, band: [three_a, three_b] },
                Cascade { patch: 256.0, n: 128, band: [three_b, cut_hi] },
            ],
        ];

        let seeds = 200_u32;
        let mut realised = [0.0_f32; 3];
        for (slot, stack) in realised.iter_mut().zip(&stacks) {
            let mut total = 0.0_f32;
            for seed in 0..seeds {
                let mut o = Ocean::new(stack, 18.0, 440_000.0, [1.0, 0.0], seed);
                o.evolve(20.0);
                total += o.significant_height();
            }
            #[allow(clippy::cast_precision_loss)]
            {
                *slot = total / seeds as f32;
            }
        }
        println!(
            "Hs over {seeds} seeds, one/two/three cascades tiling one range: \
             {:.3} / {:.3} / {:.3} m",
            realised[0], realised[1], realised[2]
        );

        for (i, a) in realised.iter().enumerate() {
            for (j, b) in realised.iter().enumerate().skip(i + 1) {
                assert!(
                    (a - b).abs() < 0.10,
                    "slicing the same spectrum into {} and {} cascades gives \
                     {a:.3} m and {b:.3} m — the bands are not partitioning it",
                    i + 1,
                    j + 1
                );
            }
        }
    }

    /// **A wind sea breaks where a swell of the same height does not — the whole
    /// reason two spectra are summed into one grid.**
    ///
    /// Whitecapping is driven by *steepness*, and fetch buys height by making the sea
    /// *longer*: a single 440 km sea — which is what `ocean_fft` authored before it
    /// authored a swell — realises 6.1 m, a genuine twenty-foot sea,
    /// at a 226 m peak wavelength, so `Hs/λp ≈ 0.027`, and it barely breaks. Per wave
    /// `k·A` is what a crest responds to, and a short wave carries it out of all
    /// proportion to its height.
    ///
    /// **The two seas here carry the same energy, redistributed, and that is exact
    /// rather than tuned.** A fetch-limited `Hs` is `0.0016·√(gF/U²)·U²/g`, which is
    /// `∝ √F` at fixed wind, so `m0 ∝ F` and fetches add: 440 km of swell against
    /// 420 km of swell plus 20 km of wind sea is the same `m0` by arithmetic, with no
    /// number chosen to make it so. Both realise **6.127 m and 6.129 m**, asserted
    /// below, because a breaking comparison at two different heights would prove
    /// nothing.
    ///
    /// **Same heading for both**, deliberately. A crossing sea changes the compression
    /// matrix's anisotropy and therefore its largest eigenvalue — which is the case
    /// `WaterSample::mu_max` was built as an eigenvalue rather than a trace to handle,
    /// and which the next task of the plan gives it — but it is a second effect, and
    /// measuring two at once is not a measurement. This isolates steepness.
    ///
    /// The threshold is [`crate::foam::FOAM_CREST_BREAK`] — **the engine's own, not one
    /// picked for this test**; it is what the foam a scene draws is thresholded on.
    /// Measured over 8 seeds × 181² points spanning the 2048 m swell tile:
    ///
    /// ```text
    ///                    Hs      mu_max mean   max     past 0.22   past 0.45
    /// swell alone        6.127   0.0508        0.429   1.000%      0.000%
    /// swell + wind sea   6.129   0.0827        0.731   9.902%      0.133%
    /// ```
    ///
    /// So the sea goes from **not breaking anywhere** — zero samples of 262,088 past
    /// the threshold, its highest reading 0.429 against 0.45 — to breaking over about
    /// one part in 750 of its surface, with 63% more mean compression behind it. The
    /// bounds below are stated as absolutes rather than as a ratio because the "before"
    /// is exactly zero and a ratio against zero says nothing; both have room, and every
    /// input here is fixed, so there is no run-to-run spread to leave room for.
    ///
    /// **`side` must be coprime with the cascade patches, and 181 is prime for that
    /// reason.** A sample lattice of `side` points across 2048 m visits the chop
    /// cascade's 32 m tile at phases `64·i mod side` and the sea cascade's 256 m tile at
    /// `8·i mod side`, so a `side` sharing a factor with 64 revisits a handful of fixed
    /// phases instead of covering the tile. Measured: `side = 128` — two samples per
    /// chop period — reads a mean `mu_max` of **0.0681 against 0.0508**, 34% high, from
    /// nothing but where its points landed. 181, 257 and 401 agree to the fourth decimal
    /// (0.0508 / 0.0509 / 0.0509 and 0.0827 throughout), which is what says the number
    /// is the surface's and not the lattice's.
    #[test]
    fn a_wind_sea_breaks_where_a_swell_of_the_same_height_does_not() {
        use crate::spectrum::Swell;
        // 440 km of fetch, split. `m0 ∝ fetch`, so the two seas are the same size.
        let (swell_fetch, wind_fetch) = (420_000.0_f32, 20_000.0_f32);
        let u10 = 18.0_f32;
        let heading = [1.0_f32, 0.0];
        let seeds = 8_u32;
        let side = 181_usize;
        let span = 2048.0_f32; // The swell cascade's own patch.

        // Measured at four thresholds rather than one, because the shape of the rise
        // is the evidence and a single number is not. `FOAM_CREST_BREAK` is the one
        // asserted on.
        let thresholds = [0.15_f32, 0.22, 0.30, crate::foam::FOAM_CREST_BREAK];
        let mut report = [[0.0_f32; 4]; 2];
        let mut hs = [0.0_f32; 2];
        let mut mean_mu = [0.0_f32; 2];
        let mut max_mu = [0.0_f32; 2];

        for (case, swell) in [
            None,
            Some(Swell { u10, fetch: swell_fetch, direction: heading }),
        ]
        .into_iter()
        .enumerate()
        {
            let fetch = if swell.is_some() { wind_fetch } else { swell_fetch + wind_fetch };
            for seed in 0..seeds {
                let mut o =
                    Ocean::with_swell(&shipping_stack(SHIPPING_N), u10, fetch, heading, swell, seed);
                o.evolve(20.0);
                hs[case] += o.significant_height();

                let mut sum = 0.0_f32;
                let mut past = [0_usize; 4];
                for iz in 0..side {
                    for ix in 0..side {
                        #[allow(clippy::cast_precision_loss)]
                        let at = |i: usize| span * i as f32 / side as f32;
                        let s = o.at(at(ix), at(iz));
                        // The same eigenvalue `sample_cascade` hands the foam field and
                        // the shader — not a second definition of breaking.
                        let (mu, _) = crate::breaking(s.sxx, s.szz, s.sxz);
                        sum += mu;
                        max_mu[case] = max_mu[case].max(mu);
                        for (slot, t) in past.iter_mut().zip(&thresholds) {
                            if mu > *t {
                                *slot += 1;
                            }
                        }
                    }
                }
                #[allow(clippy::cast_precision_loss)]
                let cells = (side * side) as f32;
                mean_mu[case] += sum / cells;
                #[allow(clippy::cast_precision_loss)]
                for (slot, count) in report[case].iter_mut().zip(past) {
                    *slot += count as f32 / cells;
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let n = seeds as f32;
            hs[case] /= n;
            mean_mu[case] /= n;
            for slot in &mut report[case] {
                *slot /= n;
            }
        }

        for (case, label) in ["swell alone", "swell + wind sea"].iter().enumerate() {
            println!(
                "{label:16}  Hs {:.3} m  mu_max mean {:.4} max {:.4}  \
                 past {:?}: {:.5} {:.5} {:.5} {:.5}",
                hs[case],
                mean_mu[case],
                max_mu[case],
                thresholds,
                report[case][0],
                report[case][1],
                report[case][2],
                report[case][3],
            );
        }

        // **Matched height, or the comparison is not a comparison.** 2%: the two are
        // 0.03% apart and each is a fixed 8-seed mean, so this is a guard against a
        // future change to the fetch split rather than a tolerance anything sits near.
        let mismatch = (hs[0] - hs[1]).abs() / hs[0];
        assert!(
            mismatch < 0.02,
            "the two seas are not the same size: {:.3} m against {:.3} m",
            hs[0], hs[1]
        );

        // **Asserted at the threshold the engine reads, as well as at the tail.**
        // `WATER_FOAM_BREAK` and `SPRAY_BREAK` are both 0.24, so `thresholds[1]` is
        // where foam and spray are actually decided and it is the number this test
        // is about. The 0.45 bucket below is the rare tail — kept because a sea that
        // stopped producing *any* extreme fold would be a real loss, but it is a
        // hundredth the size and moves for reasons the visible sea does not.
        assert!(
            report[1][1] > 0.05 && report[1][1] > report[0][1] * 5.0,
            "at the threshold foam and spray read ({}), the wind sea covers {:.3}%              against the swell's {:.3}% — that ratio is the whole effect",
            thresholds[1],
            report[1][1] * 100.0,
            report[0][1] * 100.0
        );

        let (before, after) = (report[0][3], report[1][3]);
        assert!(
            before < 1.0e-4,
            "the swell-only sea already breaks over {:.4}% of its surface — the defect \
             this test is the acceptance for is not present",
            before * 100.0
        );
        // **4e-4, and it was 1e-3 while the directional spread was a flat `cos²`.**
        // Concentrating the peak (`spectrum::SPREAD_PEAK`) thinned this tail by
        // more than half and left everything the engine reads alone — measured, at
        // the four thresholds above, wind sea before against after:
        //
        //     0.15   23.86% -> 24.92%      0.30   2.92% -> 2.25%
        //     0.22    9.90% ->  9.39%      0.45  0.133% -> 0.059%
        //
        // with `mu_max` mean identical at 0.083. The extreme fold is many
        // differently-headed waves crossing at one point, and a narrower peak has
        // fewer such crossings to offer; the broadened short-wave tail is what puts
        // the 0.15 bucket *up*. The distribution tightened rather than weakened, so
        // the bound follows the tail down rather than pinning the sea to a shape it
        // no longer has.
        assert!(
            after > 4.0e-4,
            "the wind sea breaks over only {:.4}% of the surface against {:.4}% without \
             it — a wind sea is supposed to be what makes a sea break",
            after * 100.0,
            before * 100.0
        );
        // The mean is the noise-free half of the same claim: it does not depend on
        // where a threshold sits at all.
        assert!(
            mean_mu[1] > 1.4 * mean_mu[0],
            "mean mu_max {:.4} with a wind sea against {:.4} without",
            mean_mu[1], mean_mu[0]
        );
    }

    /// A calm sea is calm. Guards the same divide-by-U edge the spectrum test does, one
    /// layer up.
    #[test]
    fn no_wind_is_a_flat_ocean() {
        let mut o = Ocean::new(&[Cascade::whole(256.0, 32)], 0.0, 100_000.0, [1.0, 0.0], 2);
        o.evolve(9.0);
        assert!(o.significant_height() < 0.05, "Hs {} in no wind", o.significant_height());
    }

    /// **The configuration that ships, held to the spectrum it claims to be.**
    ///
    /// `spectrum::amplitude_field_agrees_with_wave_set_fetch` asks the same question
    /// of a *single* grid at `n` 32–64 and patches of 200–800 m, where `Δk` is
    /// within a factor of a few of `k_peak` and a concentrated directional lobe is
    /// sampled by a handful of cells — so its coarse points carry a 30% bound.
    /// Nothing held the three-cascade stack to anything, and that is the one an
    /// author actually looks at: its peak sits in a 2048 m cascade at
    /// `Δk = 0.0031 rad/m`, hundreds of cells out, and it lands within 2%.
    ///
    /// Written when `SPREAD_PEAK` made the peak lobe narrower than `cos²`. That
    /// change moved the coarse test to 22% and this one not at all, which is the
    /// measurement that said the 22% was the grid and not the sea.
    #[test]
    fn the_shipping_stack_carries_the_spectrums_own_height() {
        for (u10, fetch) in [(12.0_f32, 200_000.0_f32), (18.0, 440_000.0), (25.0, 500_000.0), (35.0, 600_000.0)] {
            let mut sea = Ocean::new(&shipping_stack(SHIPPING_N), u10, fetch, [1.0, 0.0], 7);
            sea.evolve(0.0);
            let target = crate::spectrum::significant_height(&crate::spectrum::wave_set_fetch(
                u10,
                [1.0, 0.0],
                fetch,
            ));
            let error = (sea.significant_height() - target).abs() / target;
            assert!(
                error < 0.05,
                "u10 {u10}: the stack carries Hs {} where the spectrum says {target} ({:.1}% off)",
                sea.significant_height(),
                error * 100.0
            );
        }
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
    /// The residual is that quantisation times the local slope, and it is *the same* with
    /// and without the `rem_euclid` wrap in [`Ocean::sample`] — which is the measurement
    /// saying the far-field error lives in the coordinate and not in the interpolant.
    ///
    /// **5e-4, and it was 1e-4, which was marginal.** The drift is a property of the
    /// realisation, not of the arithmetic: across 250 draws — 200 seeds at `[1, 0]` and
    /// 50 at each of [`HEADINGS`] — it runs **median 5.1e-5, p90 9.0e-5, worst 1.61e-4**.
    /// A 1e-4 band therefore sits *inside* the spread and fails on about one realisation
    /// in fifty, which is what it did: an unrelated fault injection elsewhere in this
    /// crate moved seed 11's draw and this test read 1.29e-4 and failed for a reason that
    /// was not the fault. 5e-4 is about three times the worst of 250 draws, and still an
    /// order of magnitude below `the_tile_wraps_without_a_seam`'s 0.05 m.
    ///
    /// **What it does and does not catch.** It sees a wrap that has actually broken — a
    /// coordinate that lands on the wrong tile reads a different sea entirely. It cannot
    /// see a *consistent* misalignment: injecting a half-cell offset into `fx` shifts
    /// near and far identically and reads 3.7e-5, which is below the correct code's own
    /// drift. That is `sample_on_a_node_is_the_node`'s job, not this one's.
    #[test]
    fn the_far_field_is_the_near_field() {
        let mut o = test_ocean();
        o.evolve(8.0);
        for (x, z) in [(3.7_f32, 11.3_f32), (-6.25, 0.4), (100.1, -55.9)] {
            let near = o.sample(x, z);
            let far = o.sample(x + 10_240.0, z - 10_240.0);
            for i in 0..3 {
                assert!(
                    (near[i] - far[i]).abs() < 5e-4,
                    "component {i} at ({x}, {z}) reads {} near and {} ten km out",
                    near[i], far[i]
                );
            }
        }
    }

    /// **[`Ocean::sample`] on a grid node reads that node, and nothing between.**
    ///
    /// A half-cell offset between `sample`'s interpolation and the tile's own indexing is
    /// invisible in every other test in this file — `Hs`, the seam, the pinch and the
    /// axis identity are all shift-invariant, and `the_far_field_is_the_near_field`
    /// shifts near and far by the *same* half cell and reads 3.7e-5, below its own
    /// arithmetic noise. **Fault-injected**: `+ 0.5` on `fx` in [`Ocean::sample`] fails
    /// this test and leaves the other 135 in the crate green, including
    /// `the_tile_wraps_without_a_seam` and `the_far_field_is_the_near_field`, which are
    /// the two that look as though they cover it. What it would cost is the thing
    /// `loom_water`'s module docs open
    /// by warning about: the CPU surface buoyancy reads sitting half a cell from the GPU
    /// tile that is drawn, so the boat floats beside the wave.
    ///
    /// Node `i` sits at `i · patch/n`, which for a power-of-two `n` and this patch is
    /// exact in `f32`, so the interpolant's weights are exactly 0 and the comparison is
    /// exact rather than banded — `a + (b − a)·0.0` is `a`.
    ///
    /// One cascade, because that is the instrument: with two, `sample` returns a sum and
    /// a half-cell error in one layer could be cancelled by the other.
    ///
    /// **The seams are in the list on purpose.** Node `n − 1` interpolates toward node
    /// `0`, and `-cell` must land back on node `n − 1` — the two places an off-by-one in
    /// the wrap shows up and nowhere else does.
    #[test]
    fn sample_on_a_node_is_the_node() {
        let cascade = Cascade::whole(512.0, 64);
        let n = cascade.n;
        let mut o = Ocean::new(&[cascade], 14.0, 300_000.0, [1.0, 0.0], 4);
        o.evolve(6.0);
        #[allow(clippy::cast_precision_loss)]
        let cell = cascade.patch / n as f32;
        let cells = n * n;
        let tiles = o.tiles();

        // Grid nodes, plus two coordinates that must wrap onto one: `-cell` is node
        // `n − 1`, and one whole patch out is node 0 again.
        #[allow(clippy::cast_precision_loss)]
        let cases: [(f32, f32, usize, usize); 8] = [
            (0.0, 0.0, 0, 0),
            (cell, 0.0, 1, 0),
            (0.0, cell, 0, 1),
            (7.0 * cell, 23.0 * cell, 7, 23),
            ((n - 1) as f32 * cell, (n - 1) as f32 * cell, n - 1, n - 1),
            ((n - 1) as f32 * cell, 0.0, n - 1, 0),
            (-cell, -cell, n - 1, n - 1),
            (cascade.patch, cascade.patch, 0, 0),
        ];
        for (x, z, ix, iz) in cases {
            let got = o.sample(x, z);
            // `sample` returns `[dx, height, dz]`; the tiles are `[height, dx, dz]`.
            for (component, slot) in [(0usize, 1usize), (1, 0), (2, 2)] {
                let want = tiles[component * cells + iz * n + ix];
                assert_eq!(
                    got[slot], want,
                    "sample({x}, {z}) component {component} reads {} where node \
                     ({ix}, {iz}) of the tile holds {want}",
                    got[slot]
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
            let mut o = Ocean::new(&[Cascade::whole(512.0, 64)], 14.0, 300_000.0, dir, 5);
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
            let mut o = Ocean::new(&[Cascade::whole(512.0, 64)], 14.0, 300_000.0, dir, 7);
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

    /// An ocean whose amplitude field is handed in rather than drawn from a spectrum.
    ///
    /// **The three tests below are analytic and a random sea has no analytic answer.** A
    /// single `k` has a closed-form period, a closed-form amplitude and one axis; a
    /// spectrum-drawn field has a period per cell and an amplitude that is a draw. Only
    /// `h0` is replaced — `omega`, `khat`, the transform, the checkerboard and the tiles
    /// are all [`Ocean::new`]'s, so these tests exercise the shipping path rather than a
    /// second implementation of it. The wind arguments are therefore dead except for
    /// `direction`, which still chooses the half-plane sign.
    fn ocean_from_h0(cascade: Cascade, direction: [f32; 2], h0: Vec<Complex>) -> Ocean {
        let mut o = Ocean::new(&[cascade], 14.0, 300_000.0, direction, 0);
        assert_eq!(h0.len(), o.layers[0].h0.len(), "h0 is the wrong size for this cascade");
        o.layers[0].h0 = h0;
        o
    }

    /// One mirror pair alive: `c` at `+m·Δk` along **x**, its conjugate at `−m·Δk`.
    ///
    /// `c` is real, so the conjugate is `c` again. Under this crate's convention the
    /// realised height is then exactly `2c·cos(k·x − ωt)` with `ω = √(g·k)` — a single
    /// travelling mode with no `z` dependence whatsoever. `kz = 0`, so the pair sits in the
    /// centred grid's middle row.
    fn single_mode_h0(n: usize, m: usize, c: f32) -> Vec<Complex> {
        let mut h0 = vec![Complex::default(); n * n];
        let half = n / 2;
        h0[half * n + half + m] = Complex { re: c, im: 0.0 };
        h0[half * n + half - m] = Complex { re: c, im: 0.0 };
        h0
    }

    /// The wavenumber `single_mode_h0`'s pair carries.
    fn mode_k(patch: f32, m: usize) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let m = m as f32;
        m * std::f32::consts::TAU / patch
    }

    fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0_f32, f32::max)
    }

    /// **This is the only test that pins how fast the sea moves.** Multiply `omega` by any
    /// positive constant and every other test in this file still passes:
    /// `the_sea_travels_downwind` asserts a sign and not a speed, `Hs` is time-invariant,
    /// and the memory and rewind tests compare the ocean to itself. A sea with the right
    /// height and the wrong period is invisible in a still — and period is exactly what a
    /// hull's pitch and heave resonance are made of, so this is the defect that reaches the
    /// player as "the boat doesn't float right".
    ///
    /// A single mode returns to itself after `T = 2π/√(g·k)` and not after `T/2`. **That
    /// second assertion is load-bearing**: without it a stationary sea — the `ω = 0`
    /// degenerate, which is a plausible way to get this wrong — passes the first.
    ///
    /// **The pair of them still admits `ω·c` for odd integer `c`**, which is why the third
    /// assertion is here and is the one that actually pins the magnitude: at an arbitrary
    /// fraction of a period the surface has a closed-form value, `A·cos(ω·t)`, and no scale
    /// but 1 reproduces it. The two above are kept because they say in one line what the
    /// closed form says in three, and because they fail with a readable number.
    ///
    /// **Fault-injected, four ways.** `ω × 2` returns at `T/2` as well and trips the second
    /// assertion (1.8e-7 where a moving sea reads 2.0); `ω × 3` survives both period
    /// assertions — it is the odd-multiple hole — and reads **−0.848 against +0.652** on the
    /// third; `ω = 0`, the stationary sea, trips the second; `ω × 1.05` trips the third.
    ///
    /// **One correction to the premise.** The review held that scaling `ω` passes all nine
    /// existing tests. It does for a *plausible* miscalibration — at `× 1.05` and `× 0.9`
    /// this is the only test in the file that fails — but `× 1.5` and `× 2` do additionally
    /// trip `the_sea_travels_downwind`, whose 3 s / 20 m correlation stops holding once the
    /// swell moves far enough. The gap is real; it is a band, not the whole line.
    ///
    /// **The expected period is a literal, and that is load-bearing.** Deriving it from
    /// `crate::GRAVITY` — as this test did — makes the expectation and the subject the
    /// same constant, so they move together and the test cannot see `g` itself change.
    /// Measured: set `GRAVITY` to Mars' **3.7** and the computed-period version of this
    /// test passes *identically* — `T = 14.743 s`, drift 1.79e-7, the closed form matched
    /// to four figures — which is the exact "right height, wrong period" sea it was
    /// written to catch, sailing straight through its own gate.
    ///
    /// **The rest of the crate is not a substitute.** `GRAVITY = 3.7` does fail 18 tests
    /// in `loom_water` — in `drip`, `nappe`, `spray`, `wavelet`, `foam` and `spectrum`,
    /// each of which reads `g` for something of its own — but not one of them is about
    /// the ocean's dispersion, so a wrong `g` reaching `Ocean` had no gate of its own.
    /// Pinned against the literal below it does: `GRAVITY = 3.7` trips the first
    /// assertion at a drift of **1.87** where a mode that returns reads 1.8e-7, and the
    /// line printed above it reads `0.8635 against a closed-form 0.6518`.
    #[test]
    fn a_single_mode_returns_after_exactly_one_period() {
        let cascade = Cascade::whole(512.0, 64);
        let (m, amplitude) = (4, 1.0_f32);
        let mut o = ocean_from_h0(cascade, [1.0, 0.0], single_mode_h0(cascade.n, m, amplitude / 2.0));
        let at = |o: &mut Ocean, t: f32| -> Vec<f32> {
            o.evolve(t);
            o.tiles().to_vec()
        };

        // **A literal, and the one line in this test that must not be computed.** Written
        // as `TAU / (GRAVITY * mode_k(cascade.patch, m)).sqrt()` this reads the same
        // constant `Ocean::new` reads, so the two move together and the test is blind to
        // the only thing it exists to pin — set `GRAVITY` to Mars' 3.7 and every assertion
        // below still passes, on a sea with the right height and the wrong period.
        //
        // The arithmetic, once, at `g = 9.81 m/s²`: `k = m·2π/patch = 4·2π/512 = 2π/128 =
        // 0.049087 rad/m`; `ω = √(g·k) = √(9.81 · 0.049087) = 0.693937 rad/s`;
        // `T = 2π/ω = 9.054415 s`. `patch` and `m` are powers of two, so `k` is exact in
        // `f32` and re-deriving this by hand lands on the same number.
        let period = 9.054_415_f32;
        let t0 = at(&mut o, 0.0);
        let full = max_abs_diff(&t0, &at(&mut o, period));
        let half = max_abs_diff(&t0, &at(&mut o, period * 0.5));
        // An arbitrary fraction — not a half, third or quarter, so that no integer multiple
        // of the true `ω` lands back on this phase.
        let fraction = 0.137_f32;
        let expected = amplitude * (std::f32::consts::TAU * fraction).cos();
        let realised = at(&mut o, period * fraction)[0];
        println!(
            "single mode, T = {period:.3} s: drift over T {full:.2e}, over T/2 {half:.3}, \
             at {fraction}·T {realised:.4} against a closed-form {expected:.4}"
        );

        // The band is the phase error in `sin_cos(ω·T)` where `ω·T` is `2π` to within an
        // `f32` — parts in 1e-6 of an amplitude-1 mode; measured 1.8e-7, banded at 1e-4.
        assert!(
            full < 1e-4 * amplitude,
            "the mode did not return after one period T = {period} s: worst cell moved {full}"
        );
        // Half a period is a sign flip, so a sea that moves at all reads ~2·amplitude here.
        assert!(
            half > amplitude,
            "the mode is period-independent — omega is not being applied: \
             the surface at T/2 differs from t=0 by only {half}"
        );
        // `η(0, t) = A·cos(ω·t)` for this `h0` — the magnitude of `ω`, in one number.
        assert!(
            (realised - expected).abs() < 1e-4 * amplitude,
            "at {fraction} of a period the surface reads {realised} where √(g·k) says \
             {expected} — omega has the wrong magnitude"
        );
    }

    /// **The pinch has a size, and `the_pinch_sharpens_the_crests` cannot see it.** That
    /// test asserts `cov(h, div D) < 0`, which for `D = i·k̂·h` is `−Σ|k|·|h(k)|²` —
    /// negative for *any* positive radial factor. Replace `k̂` with `k`, or with `100·k̂`,
    /// and it still passes. This is the field `mu_max` reads, so it is the input to the
    /// whole foam and breaking story and its magnitude is not decoration.
    ///
    /// For a single mode `η = A·cos(k·x)` the displacement is `D = −A·sin(k·x)·k̂` and the
    /// steepness `k·A` — the classic Gerstner limit, `k·A = 1` being the cusp. So the
    /// realised `max|D|` must equal the realised `max|η|`, and both must equal the `A` the
    /// amplitude field was built with, which also pins the transform's normalisation.
    ///
    /// **Fault-injected twice, and both leave the other twelve tests green.** Scaling
    /// `grid[1]`/`grid[2]` by 2.0 reads a steepness of **0.0982 against an analytic
    /// 0.0491**; replacing `k̂` with `k` — the review's other named defect, and the one
    /// `the_pinch_sharpens_the_crests` provably cannot see — reads **0.00241**.
    #[test]
    fn a_single_mode_has_the_steepness_its_amplitude_implies() {
        let cascade = Cascade::whole(512.0, 64);
        // `n / (4m)` is an integer, so a grid node lands exactly on the crest and exactly
        // on the steepest point — `max` over the tile is the true amplitude, not a sample
        // of the cosine somewhere near its peak.
        let (m, amplitude) = (4, 1.0_f32);
        let mut o = ocean_from_h0(cascade, [1.0, 0.0], single_mode_h0(cascade.n, m, amplitude / 2.0));
        o.evolve(0.0);

        let cells = cascade.n * cascade.n;
        let peak = |tile: &[f32]| tile.iter().fold(0.0_f32, |a, v| a.max(v.abs()));
        let height = peak(&o.tiles()[..cells]);
        let displacement = peak(&o.tiles()[cells..2 * cells]);

        let k = mode_k(cascade.patch, m);
        let analytic = k * amplitude;
        let realised = k * displacement;
        println!(
            "single mode k={k:.4}: height {height:.4} m, |D| {displacement:.4} m, \
             steepness {realised:.4} against analytic {analytic:.4}"
        );

        assert!(
            (height - amplitude).abs() < 1e-3,
            "the realised height is {height} m where the amplitude field says {amplitude} m"
        );
        assert!(
            (realised - analytic).abs() < 1e-3 * analytic,
            "the realised steepness is {realised} where k·A is {analytic} — \
             the horizontal displacement is the wrong size by a factor of {}",
            realised / analytic
        );
    }

    /// **Which axis is which.** A transpose applied *consistently* through the whole of
    /// `h0 → ifft_2d → tiles → sample` cancels and is unobservable, which is why "eight
    /// of nine tests pass a transpose" — what an earlier version of this comment claimed
    /// — is a statement about a defect nobody can have. A transpose in **one link** is
    /// the real hazard, and the rest of the suite catches it thinly: each injection below
    /// trips exactly three tests in this crate, and this test is one of the three in both
    /// cases. `Hs`, the seam, memory, rewind and finiteness are all
    /// transpose-symmetric and see nothing, and
    /// `two_dimensions_agree_with_two_passes_of_one` cannot see it either: it compares
    /// `ifft_2d` against `ifft_2d` written longhand.
    ///
    /// The clean catcher is a field with energy only in `kx`: it has no `z` dependence at
    /// all, so every column of the tile must be one repeated number. A transposed pipeline
    /// produces exactly the opposite — constant along `x`, varying along `z` — and cannot
    /// satisfy this at any tolerance.
    ///
    /// Both directions are checked, because "constant along z" alone is also true of a
    /// field that is constant everywhere, which a broken transform can easily be.
    ///
    /// Fault-injected two ways, one at each end of the chain. Reading
    /// `grid[x * layer.n + z]` in `evolve`'s tile write additionally trips
    /// `the_pinch_sharpens_the_crests` and `the_sea_travels_downwind`; swapping `ix`/`iz`
    /// in [`Ocean::sample`]'s four taps additionally trips the pinch and
    /// `sample_on_a_node_is_the_node`. Both read a spread of **1.99 along z** here,
    /// against a 1e-4 band, and both leave the along-x spread that should read 1.986 at
    /// 0.000. Measured spread along z on the correct pipeline is **exactly zero**, so
    /// this is the clean catcher of the three — no tolerance is being leaned on.
    #[test]
    fn energy_in_kx_alone_is_constant_along_z() {
        let cascade = Cascade::whole(512.0, 64);
        let n = cascade.n;
        let (m, amplitude) = (4, 1.0_f32);
        let mut o = ocean_from_h0(cascade, [1.0, 0.0], single_mode_h0(n, m, amplitude / 2.0));
        o.evolve(3.0);

        // **Read through `sample`, not through `tiles`.** The chain the brief names is
        // `h0 → ifft_2d → tiles → sample`, and a swap in the last link is as wrong as one
        // in the first. Sampling exactly on grid nodes makes the interpolant a no-op, so
        // what this measures is the tile and not the bilinear filter.
        #[allow(clippy::cast_precision_loss)]
        let cell = cascade.patch / n as f32;
        #[allow(clippy::cast_precision_loss)]
        let at = |o: &Ocean, i: usize, j: usize| o.sample(i as f32 * cell, j as f32 * cell)[1];

        // The widest any single line of the surface gets. Along z that must be zero; along
        // x it is the mode.
        let spread = |line: &[f32]| -> f32 {
            let lo = line.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = line.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            hi - lo
        };
        let mut along_z = 0.0_f32;
        let mut along_x = 0.0_f32;
        for i in 0..n {
            let column: Vec<f32> = (0..n).map(|j| at(&o, i, j)).collect();
            let row: Vec<f32> = (0..n).map(|j| at(&o, j, i)).collect();
            along_z = along_z.max(spread(&column));
            along_x = along_x.max(spread(&row));
        }
        println!("kx-only mode: spread along z {along_z:.2e}, along x {along_x:.3}");

        assert!(
            along_z < 1e-4 * amplitude,
            "a field with energy only in kx varies along z by {along_z} — an axis is transposed"
        );
        // The mode has to be visible along x, or "constant along z" is the trivial pass a
        // dead transform would also give.
        assert!(
            along_x > amplitude,
            "the kx mode is not present along x at all (spread {along_x}) — \
             this test would have passed vacuously"
        );
    }


    /// Not a gate — the two numbers that choose the shipping grid size. ADR 0076
    /// predicts ~1.24 ms at N=128 and ~7.15 ms at N=256 for nine 2D transforms, against
    /// a fixed step of 16.67 ms.
    ///
    /// **It measures eighteen transforms now, not nine, and the ADR's figures no longer
    /// bound it.** Putting the cascade on the force path (ADR 0076, Task 2) needed the
    /// water's *velocity* as well as its position, and `∂/∂t` of the three fields is
    /// three more inverse transforms per cascade — six per cascade rather than three —
    /// plus five finite-differenced tiles for the normal and the breaking criterion.
    /// Measured here, release: **3.710 ms/tick at N=128 (22.3% of a tick)** and
    /// 21.105 ms at N=256 (126.6%). So `SHIPPING_N = 128` is not the fallback the ADR
    /// called it; **it is the only rung that fits**, and 256 is now out of reach for the
    /// same reason 512 always was. The three optimisations the ADR names — real-field
    /// packing, the Hermitian half-transform, threading by rows — are still unspent and
    /// still exact, and they are what would buy 256 back.
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
                &shipping_stack(n),
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

