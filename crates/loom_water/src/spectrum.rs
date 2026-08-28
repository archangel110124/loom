//! Wind → waves: the Pierson–Moskowitz spectrum, sampled into a wave set.
//!
//! **Nobody authors amplitudes.** A sea has one honest input — how hard the
//! wind is blowing — and every other number about it follows. Hand-tuning
//! sixteen amplitudes produces a sea that is right at one wind speed and wrong
//! at every other, and there is nothing in the file to say which one it was
//! tuned for.
//!
//! # The reference-height trap
//!
//! Pierson–Moskowitz is defined against the wind at **19.5 m** above the
//! surface. JONSWAP and every modern reference use **U10**, at 10 m. Mixing
//! them silently mis-sizes the sea by 5–10% in wind and 10–20% in wave height,
//! with no error anywhere, which is why it is written down three times here:
//!
//! - **[`wave_set`] takes U10**, because that is what a Beaufort number, a
//!   forecast and every published table mean by "wind speed".
//! - It converts to U19.5 with [`U19_5_PER_U10`] before touching the spectrum.
//! - **`loom_field`'s authored `Wind::speed` is neither.** That field's height
//!   profile saturates toward `speed` as height goes to infinity, so `speed` is
//!   a free-stream value and is roughly 10% *above* U10. Convert it with
//!   `loom_field::wind::Wind::mean_speed_at(10.0)`; never pass it here raw.
//!
//! # The cells that are forced real, and why
//!
//! [`amplitude_field`] hands back a Hermitian field: `h0(−k) = conj(h0(k))`,
//! written rather than hoped for, because that is what makes the inverse
//! transform's output real. Four cells of an even-`n` grid are **their own
//! mirror** under the index map `(n − x) % n`, and Hermitian symmetry then
//! demands they equal their own conjugate — so they are forced real, with
//! their imaginary part dropped:
//!
//! ```text
//! index (z, x)        k                its case
//! (n/2, n/2)          (0, 0)           also zeroed outright — a mean offset is not a wave
//! (n/2, 0)            (k_nyq, 0)       on the Nyquist column, across the row of k_z = 0
//! (0, n/2)            (0, k_nyq)       on the Nyquist row, across the column of k_x = 0
//! (0, 0)              (k_nyq, k_nyq)   both axes at Nyquist
//! ```
//!
//! Index `n/2` carries `k = 0` and index `0` carries the Nyquist wavenumber
//! `−(n/2)·Δk`, whose negation is off the grid — `+k_nyq` and `−k_nyq` are one
//! bin. That aliasing is also why [`crate::ocean::Ocean`] freezes the whole
//! Nyquist row and column in *time*: any quantity that must be odd in `k` has
//! no value there. This module's job stops at the amplitude, which is even in
//! `k`, so only the four self-mirror cells are special here.
//!
//! # Why there is no Slang twin
//!
//! This runs on the CPU, once, when the wind changes — it produces the sixteen
//! waves that the shader then evaluates through [`crate::sample_water`], which
//! *does* have a twin and is the only thing per-vertex. Generating the
//! spectrum on the GPU would mean sixteen `ln`s and a `sqrt` chain per vertex
//! to recompute a constant.
//!
//! # Fetch, and why a fully-developed sea is the wrong default for a game
//!
//! [`wave_set`] is Pierson–Moskowitz, which is the *fully-developed* limit —
//! the wind has blown forever over unlimited water — and that limit is
//! **scale-invariant**. `m0 ∝ U⁴` and `ω_p ∝ 1/U` give `A ∝ U²` and
//! `k ∝ U⁻²`, so `k·A` — slope, steepness, every quantity a normal, a
//! whitecap or a hull responds to — is *algebraically independent of the
//! wind*. Measured over the whole legal range, `Σ Q·k·A` is 0.327 at every
//! wind speed there is. **Raising the wind zooms the sea; it never roughens
//! it**, and at U10 = 18 the shortest wave in the world is 94 m and a
//! nineteen-metre boat is a cork on one tilting plane.
//!
//! [`wave_set_fetch`] is the fix and it is honest physics, not a fudge: a sea
//! that has only crossed `F` metres of open water is smaller than the
//! fully-developed one **and steeper**, and steepens as `U^⅓`. One scene
//! fact — how much open water is upwind — buys the thing a game wants from
//! wind and PM refuses to sell.
//!
//! This module's earlier note said a fetch "would be a number with no source,
//! tuned by hand, which is the thing this module exists to abolish". That
//! holds for amplitudes and not for fetch: sixteen amplitudes have no
//! referent, and a coastal fishing ground genuinely has a distance to the
//! shore. It is one number, in metres, with a meaning outside this file.
//!
//! # What is deliberately not built
//!
//! **JONSWAP's peak enhancement, `γ = 3.3`.** It narrows the spectrum about
//! its peak and moves neither `Hs` nor `ω_p` — the only two numbers sixteen
//! equal-energy bands can carry — and it has no closed-form cumulative, which
//! is exactly what makes [`bands`] exact rather than approximate.
//! [`wave_set_fetch`] is therefore *fetch-limited PM*: PM's shape, JONSWAP's
//! two parameters.

use loom_field::noise::hash;
use loom_scene::components::{GerstnerWave, MAX_WAVES, WaveSet};

use crate::GRAVITY;
use crate::fft::Complex;

/// Below this U10, in m/s, the sea is a mirror.
///
/// Not taste: at 0.5 m/s the shortest sampled wave is 7 cm and the significant
/// wave height is 6 mm, and any slower makes the wavelength fall through the
/// schema's 0.05 m floor. Beaufort 1 ("light air") starts at 0.5 m/s, so the
/// number the physics forces and the number the scale uses are the same one.
pub const MIN_WIND_SPEED: f32 = 0.5;

/// Above this U10, in m/s, the wind is treated as "as rough as it gets".
///
/// The longest sampled wave is `10.53·U19.5²/g` metres and the schema caps a
/// wavelength at 2000 m, which is reached at U10 ≈ 42. That is half again a
/// category-5 hurricane; clamping is a better answer than authoring a wave the
/// validator rejects.
pub const MAX_WIND_SPEED: f32 = 42.0;

/// How long the sea takes to come round when the wind turns, in seconds.
///
/// **The sea must not snap.** Swell carries the old direction after the wind
/// has left it, and a wave field that pivots with the wind vector is the single
/// most recognisable "this is not water" artifact in motion — invisible in a
/// still. One time constant is 63% of the turn, three is 95%.
///
/// A real sea takes hours. A minute is the compromise between "snaps" and
/// "never arrives inside a play session"; it is an argument to
/// [`SeaState::advance`] rather than a constant so a scene can disagree.
pub const SLEW_SECONDS: f32 = 60.0;

/// Pierson–Moskowitz `α`, the equilibrium-range constant.
const ALPHA: f32 = 8.1e-3;

/// Pierson–Moskowitz `β`, in the exponential that cuts the spectrum off below
/// the peak.
const BETA: f32 = 0.74;

/// U19.5 / U10 — the reference-height conversion, applied once, deliberately.
///
/// **Derived from the published relation rather than from a profile law.** The
/// standard statement of the fully-developed significant wave height is
/// `H⅓ = 0.21·U19.5²/g ≈ 0.22·U10²/g` (WikiWaves, *Ocean-Wave Spectra*, after
/// Stewart's *Introduction to Physical Oceanography* §16.1). Those two forms
/// describe one sea, so they pin the ratio between the two wind speeds:
/// `0.21·U19.5² = 0.22·U10²` gives `U19.5/U10 = √(22/21) = 1.0235`.
///
/// Taking it from the relation, not from a `1/7` power law (which would give
/// 1.100) or a log profile, is the point: those laws describe a different
/// boundary layer with a roughness this code has no way to know, and picking
/// one would put the derived sea 10% away from the very table it is checked
/// against. `significant_wave_height_matches_the_published_relation` is that
/// check.
pub const U19_5_PER_U10: f32 = 1.023_533;

/// How many frequencies the continuous spectrum is sampled at.
///
/// The waves are split as frequencies × directions, and the product is the cap
/// exactly — sixteen is what [`MAX_WAVES`] allows and per-vertex cost is linear
/// in it, so there is nothing to gain by using fewer.
const FREQUENCY_BANDS: usize = 4;

/// How many directions the spread is sampled at. See [`FREQUENCY_BANDS`].
const DIRECTIONS: usize = MAX_WAVES / FREQUENCY_BANDS;

const _: () = assert!(FREQUENCY_BANDS * DIRECTIONS == MAX_WAVES);

/// The `2s` in `cos^2s(θ)` directional spreading. `2` is the standard
/// cosine-squared spread; larger is a more directional sea.
const SPREAD_POWER: i32 = 2;

/// What fraction of the fold budget the derived sea is allowed to spend.
///
/// The validator rejects a wave at `Q·N·k·A > 1`; every wave here is given
/// `Q·N·k·A = min(this, N·k·A)`, so the whole set is inside the limit by
/// construction at any wind speed rather than by testing afterwards. `0.75`
/// leaves a quarter of the budget as headroom and still pinches the crests.
const STEEPNESS_BUDGET: f32 = 0.75;

/// The sea a wind speed and direction produce, as sixteen Gerstner waves.
///
/// `u10` is the mean wind speed at **10 m** above the surface, in m/s — see the
/// module docs before passing anything else. `direction` is where the wind
/// blows *toward*, on the XZ plane, in `loom_field`'s convention; any length
/// works.
///
/// # How the spectrum becomes sixteen waves
///
/// The PM spectrum has a closed-form cumulative variance,
/// `F(ω) = exp(−β·(g/(U·ω))⁴)`, so it can be split into bands of **equal
/// energy** exactly: band `i` of `N` is represented by the frequency at
/// `F = (i+½)/N`. That is what makes the total variance — and therefore the
/// significant wave height — come out at the published value rather than near
/// it, whatever the band count.
///
/// The price is that equal-energy banding ignores the spectral tail, where
/// there is little energy and all the fine chop. Sixteen Gerstner waves cannot
/// carry both the swell and the ripples; the ripples are a normal-map job for
/// the water material, not a wave.
#[must_use]
pub fn wave_set(u10: f32, direction: [f32; 2]) -> WaveSet {
    let u10 = if u10.is_finite() { u10.min(MAX_WIND_SPEED) } else { 0.0 };
    if u10 < MIN_WIND_SPEED {
        return WaveSet::default();
    }
    let along = normalise(direction).unwrap_or([1.0, 0.0]);

    // The one place the two reference heights meet, and the only place.
    let u = u10 * U19_5_PER_U10;

    // Total variance of the surface: m0 = ∫S(ω)dω = α·U⁴/(4βg²), which falls
    // out of the spectrum in closed form under u = ω⁻⁴. Hs = 4√m0.
    let m0 = ALPHA * (u * u) * (u * u) / (4.0 * BETA * GRAVITY * GRAVITY);
    bands(m0, u, along)
}

/// The sixteen waves that carry variance `m0` around a spectrum peaked as PM's
/// is at wind `u` (U19.5), fanned about `along`.
///
/// **Split out of [`wave_set`] verbatim, and that is the point.** It is the
/// banding, not the parameterisation: any spectrum of PM's *shape* is two
/// numbers — how much energy and where the peak is — so a fetch-limited sea
/// reaches the same loop with different ones rather than through a second
/// implementation of it. Moving this code rather than rewriting it is what
/// keeps every pinned water hash where it was.
fn bands(m0: f32, u: f32, along: [f32; 2]) -> WaveSet {
    let band_variance = m0 / FREQUENCY_BANDS as f32;

    // cos²(θ) spreading, sampled at the midpoints of DIRECTIONS equal slices of
    // its support (±90°) and normalised to sum to one — so the spread fans the
    // energy out without changing how much there is.
    let mut spread = [(0.0_f32, 0.0_f32); DIRECTIONS];
    let slice = std::f32::consts::PI / DIRECTIONS as f32;
    for (index, slot) in spread.iter_mut().enumerate() {
        let theta = (index as f32 + 0.5) * slice - std::f32::consts::FRAC_PI_2;
        *slot = (theta, theta.cos().powi(SPREAD_POWER));
    }
    let total: f32 = spread.iter().map(|&(_, weight)| weight).sum();
    for slot in &mut spread {
        slot.1 /= total;
    }

    let mut waves = Vec::with_capacity(MAX_WAVES);
    let mut max_height = 0.0_f32;
    let mut longest = 0.0_f32;

    for band in 0..FREQUENCY_BANDS {
        // Inverting F(ω): ω = (g/U)·(β / ln(1/F))^¼. Written as two `sqrt`s
        // rather than `powf(0.25)` — the same number, one libm call fewer, and
        // no exponent to mistype.
        let fraction = (band as f32 + 0.5) / FREQUENCY_BANDS as f32;
        let omega = (GRAVITY / u) * (BETA / (1.0 / fraction).ln()).sqrt().sqrt();
        // Deep water: ω² = g·k.
        let k = omega * omega / GRAVITY;
        let wavelength = std::f32::consts::TAU / k;
        longest = longest.max(wavelength);

        for &(theta, weight) in &spread {
            // Variance A²/2 per wave, so A = √(2·share). Summed over the grid
            // this is exactly m0 again, which is the whole point of banding by
            // energy.
            let amplitude = (2.0 * band_variance * weight).sqrt();
            let steepness = (STEEPNESS_BUDGET / (MAX_WAVES as f32 * k * amplitude)).min(1.0);
            let (sin, cos) = theta.sin_cos();
            waves.push(GerstnerWave {
                wavelength,
                amplitude,
                steepness,
                direction: [
                    along[0] * cos - along[1] * sin,
                    along[0] * sin + along[1] * cos,
                ],
                speed_scale: 1.0,
            });
            max_height += amplitude;
        }
    }

    WaveSet {
        waves,
        // Deep-water waves stop feeling the bottom at about half a wavelength,
        // so the longest wave is what sets where the shallows begin.
        attenuation_depth: longest * 0.5,
        // The mesh bounds. Every wave at its crest at once — vanishingly
        // unlikely and exactly what a bound is for.
        max_height,
    }
}

/// Where the PM spectrum's peak sits, as a multiple of `g/U19.5`.
///
/// Derived here rather than quoted: `S(ω) ∝ ω⁻⁵·exp(−β(g/(Uω))⁴)` has
/// `d/dω[−5·ln ω − β(g/U)⁴ω⁻⁴] = 0` at `ω⁴ = 0.8·β·(g/U)⁴`, so
/// `ω_p = (0.8β)^¼·g/U`. With `β = 0.74` that is 0.8772, and the published
/// figure is `0.877·g/U19.5` — which is the check that this file's `β` and
/// the literature's are the same constant.
const PM_PEAK: f32 = 0.877_18;

/// Dimensionless fetch `g·F/U10²` at which a fetch-limited sea has grown into
/// the fully-developed one.
///
/// Both laws are scale-free in `U`, so their crossing is a single number:
/// `0.0016·√F̃ = 0.22` gives `F̃ = 18_906`. **Past it, [`wave_set_fetch`] is
/// [`wave_set`]** — which is correct physics and a footgun, because the
/// fully-developed sea is the scale-invariant one this function exists to
/// escape. In metres that crossing is `18_906·U10²/g`: about 14 km at a
/// light breeze and 640 km at storm force, so a scene with a big number in
/// its `fetch` gets the flat sea back at its *calm* end first.
const FULLY_DEVELOPED_FETCH: f32 = 18_906.0;

/// The same sea, fetch-limited: the wind has only crossed `fetch` metres of
/// open water.
///
/// `u10` and `direction` mean exactly what they mean in [`wave_set`] — read
/// the module's reference-height note before passing anything else. `fetch` is
/// in metres.
///
/// # The two laws
///
/// Hasselmann et al. 1973 (the JONSWAP experiment), in terms of the
/// dimensionless fetch `F̃ = g·F/U10²`:
///
/// ```text
///     Hs  = 0.0016·√F̃ · U10²/g
///     f_p = 3.5·(g/U10) · F̃^−0.33
/// ```
///
/// Those are the only two numbers [`bands`] needs, so the peak frequency is
/// fed back in as **the PM wind that would have produced it** and the banding
/// is reached unchanged. One expression of the spectrum, two ways of
/// parameterising it — which is what stops this being a second implementation
/// that can drift from the first.
///
/// # What it buys
///
/// At a 3 km fetch, `Σ Q·k·A` runs 0.39 → 0.62 across the legal wind range
/// where PM's is pinned at 0.327 forever, and `Hs` tops out near a metre
/// instead of near thirty. That is the whole difference between a wind knob
/// that zooms and one that roughens.
#[must_use]
pub fn wave_set_fetch(u10: f32, direction: [f32; 2], fetch: f32) -> WaveSet {
    let along = normalise(direction).unwrap_or([1.0, 0.0]);
    match spectrum_shape(u10, fetch) {
        Some((m0, u)) => bands(m0, u, along),
        None => WaveSet::default(),
    }
}

/// PM's own total variance for the wind `u` (U19.5): `m0 = α·U⁴/(4βg²)`, the
/// same expression [`wave_set`] evaluates for the unlimited-fetch case.
/// Factored out so [`spectrum_shape`] and [`amplitude_field`] share it rather
/// than re-deriving it a second and third time.
fn natural_m0(u: f32) -> f32 {
    ALPHA * (u * u) * (u * u) / (4.0 * BETA * GRAVITY * GRAVITY)
}

/// The PM-shaped spectrum a wind and fetch reduce to: total variance `m0` and
/// the PM19.5 wind `u` that sets its shape — the two numbers [`bands`] needs,
/// however they were reached.
///
/// **The one place a wind and a fetch become a spectrum.** [`wave_set_fetch`]
/// hands this pair straight to [`bands`], which integrates it into sixteen
/// equal-energy bands; [`amplitude_field`] reuses the exact same derivation
/// and evaluates the underlying density continuously instead. Same `u10`
/// clamp, same fetch law, same constants — the sixteen-wave path and the grid
/// path can disagree about resolution and never about physics.
///
/// `None` below [`MIN_WIND_SPEED`] — a mirror has no spectrum to speak of.
fn spectrum_shape(u10: f32, fetch: f32) -> Option<(f32, f32)> {
    let u10 = if u10.is_finite() { u10.min(MAX_WIND_SPEED) } else { 0.0 };
    if u10 < MIN_WIND_SPEED {
        return None;
    }

    // A non-positive or non-finite fetch is "unlimited", which is PM. Guarded
    // rather than clamped because zero fetch is not a flat sea asymptotically —
    // it is a division by zero in the peak law.
    if fetch.is_finite() && fetch > 0.0 {
        let dimensionless = GRAVITY * fetch / (u10 * u10);
        if dimensionless < FULLY_DEVELOPED_FETCH {
            let hs = 0.0016 * dimensionless.sqrt() * u10 * u10 / GRAVITY;
            // Hs = 4√m0, the same relation `significant_height` inverts.
            let m0 = (hs / 4.0) * (hs / 4.0);

            let omega_peak =
                std::f32::consts::TAU * 3.5 * (GRAVITY / u10) * dimensionless.powf(-0.33);
            let u = PM_PEAK * GRAVITY / omega_peak;
            return Some((m0, u));
        }
    }

    let u = u10 * U19_5_PER_U10;
    Some((natural_m0(u), u))
}

/// The sea's memory of the wind: what [`wave_set`] is actually built from.
///
/// **Wave inertia.** The wind is a field that can change between one tick and
/// the next; the sea cannot. This holds the sea's own speed and direction and
/// walks them toward the wind's with a time constant, so a squall that swings
/// 90° leaves the swell running the old way and turning.
///
/// Deterministic and stateful, which is the whole job: the same tick sequence
/// gives the same sea, and no wall clock is read — [`SeaState::advance`] is
/// handed the fixed timestep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeaState {
    direction: [f32; 2],
    u10: f32,
}

impl SeaState {
    /// A sea already in equilibrium with this wind — what a scene starts at.
    #[must_use]
    pub fn new(direction: [f32; 2], u10: f32) -> Self {
        Self {
            direction: normalise(direction).unwrap_or([1.0, 0.0]),
            u10: if u10.is_finite() { u10.clamp(0.0, MAX_WIND_SPEED) } else { 0.0 },
        }
    }

    /// Step the sea toward the wind by one tick of `dt` seconds.
    ///
    /// A damped follow: each step closes `1 − e^(−dt/τ)` of the remaining gap,
    /// in both speed and heading, which makes the approach exactly exponential
    /// and independent of the timestep.
    ///
    /// **The heading turns by the signed angle between the two**, not by a lerp
    /// of the two vectors. A lerp shrinks toward zero length as the turn
    /// approaches 180° — the sea would flatten mid-turn and the direction would
    /// become undefined at the crossing. Rotating by `atan2(cross, dot)·α`
    /// keeps the vector on the unit circle the whole way round and picks a
    /// consistent side at exactly 180°.
    pub fn advance(&mut self, wind_direction: [f32; 2], u10: f32, dt: f32, time_constant: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let alpha =
            if time_constant > 0.0 { 1.0 - (-dt / time_constant).exp() } else { 1.0 };

        if u10.is_finite() {
            self.u10 += (u10.clamp(0.0, MAX_WIND_SPEED) - self.u10) * alpha;
        }

        if let Some(target) = normalise(wind_direction) {
            let dot = self.direction[0] * target[0] + self.direction[1] * target[1];
            let cross = self.direction[0] * target[1] - self.direction[1] * target[0];
            let (sin, cos) = (cross.atan2(dot) * alpha).sin_cos();
            let turned = [
                self.direction[0] * cos - self.direction[1] * sin,
                self.direction[0] * sin + self.direction[1] * cos,
            ];
            // Renormalised every step: a rotation is length-preserving in
            // exact arithmetic and slowly is not in `f32`.
            self.direction = normalise(turned).unwrap_or(self.direction);
        }
    }

    /// Where the waves are running, as a unit vector on XZ.
    #[must_use]
    pub fn direction(&self) -> [f32; 2] {
        self.direction
    }

    /// The wind speed at 10 m the sea is currently built for, in m/s.
    #[must_use]
    pub fn u10(&self) -> f32 {
        self.u10
    }

    /// The waves this sea state produces.
    #[must_use]
    pub fn waves(&self) -> WaveSet {
        wave_set(self.u10, self.direction)
    }
}

/// Significant wave height of a wave set: `4√m0`, with `m0` the summed
/// variance `ΣA²/2`. Summed in index order, like everything else.
///
/// **The one number that says how big a sea is.** It is what the published
/// fully-developed relation is stated in, so it is what
/// `significant_wave_height_matches_the_published_relation` checks [`wave_set`]
/// against — and it is the number an agent that just authored a `WaterBody`
/// wants back, because "sixteen waves" is not an answer to "how rough is it".
/// `loom water` reports it for authored wave sets too: the formula is a
/// property of a sum of sinusoids, not of this spectrum.
#[must_use]
pub fn significant_height(waves: &WaveSet) -> f32 {
    let m0: f32 = waves.waves.iter().map(|w| w.amplitude * w.amplitude / 2.0).sum();
    4.0 * m0.sqrt()
}

/// The Pierson–Moskowitz density `S(ω) = (α·g²/ω⁵)·exp(−β(g/(Uω))⁴)`.
///
/// `bands()` only ever needs this density's closed-form *cumulative*,
/// `F(ω) = exp(−β(g/(Uω))⁴)`, to place its sixteen frequencies — the density
/// itself never appears in this file until here, because a continuous grid
/// needs it directly rather than through its integral. Same `ALPHA`/`BETA`,
/// so a change to either moves the sixteen-wave sea and the grid together.
fn pm_density(omega: f32, u: f32) -> f32 {
    let cutoff = BETA * (GRAVITY / (u * omega)).powi(4);
    ALPHA * GRAVITY * GRAVITY / omega.powi(5) * (-cutoff).exp()
}

/// `∫_{−π/2}^{π/2} cosᵖ θ dθ` for even `p`, by Wallis' product formula.
///
/// The continuous counterpart of the discrete sum-to-one normalisation
/// [`bands`] does over its sixteen fixed directions — needed here because a
/// grid cell's angle is whatever `atan2` gives it, not one of sixteen
/// midpoints, so there is no finite set of weights to normalise in advance.
fn cos_power_integral(power: i32) -> f32 {
    debug_assert!(power >= 0 && power % 2 == 0, "only defined for even p");
    let mut product = 1.0_f32;
    let mut k = 1;
    while 2 * k <= power {
        product *= (2 * k - 1) as f32 / (2 * k) as f32;
        k += 1;
    }
    std::f32::consts::PI * product
}

/// A value uniformly in `(0, 1)`, exact and never exactly `0` or `1` — the
/// upper 24 bits of a hash scale exactly to `f32`, and the `+0.5` keeps a
/// Box–Muller `ln` away from `ln(0)`.
fn unit_interval(h: u32) -> f32 {
    ((h >> 8) as f32 + 0.5) * (1.0 / 16_777_216.0)
}

/// Two independent standard-normal draws for grid cell `(x, z)`, by
/// Box–Muller from [`loom_field::noise::hash`] seeded by `(seed, x, z)`.
///
/// **Never `thread_rng`, never a crate RNG.** `hash` is frozen ABI (see
/// `loom_field::noise`'s module docs) precisely so that a dependency bump can
/// never reseed a sea underneath a determinism hash.
fn box_muller(seed: u32, x: u32, z: u32) -> (f32, f32) {
    let h1 = hash(hash(hash(seed).wrapping_add(x)).wrapping_add(z));
    // A second, independent draw from the same lattice point rather than a
    // second call into `(seed, x, z)` again, which would just repeat `h1`.
    let h2 = hash(h1 ^ 0x9E37_79B9);

    let (u1, u2) = (unit_interval(h1), unit_interval(h2));
    let radius = (-2.0 * u1.ln()).sqrt();
    let (sin, cos) = (std::f32::consts::TAU * u2).sin_cos();
    (radius * cos, radius * sin)
}

/// Every wavenumber the grid can carry — the `band` a single, unstacked
/// [`amplitude_field`] wants.
///
/// The upper bound is infinite rather than the grid's Nyquist because the
/// square grid's *corners* reach `√2` times it, and those cells carry real
/// energy that a lone cascade has no reason to throw away.
pub const WHOLE_BAND: [f32; 2] = [0.0, f32::INFINITY];

/// The `h0(k)` field a wind, fetch and direction produce over an `n × n` grid
/// on a `patch`-metre-square patch, for [`crate::fft::ifft_2d`] to turn into
/// a height field. Row-major, index `z * n + x`.
///
/// `u10` is the wind at **10 m**, exactly as in [`wave_set`] and
/// [`wave_set_fetch`] — read this module's reference-height note before
/// passing anything else, and never `loom_field::wind::Wind::speed`, which is
/// a free-stream value roughly 10% *above* U10; convert it with
/// `Wind::mean_speed_at(10.0)` first.
///
/// # How this and the sixteen-wave path agree
///
/// [`spectrum_shape`] is the one place a wind and a fetch become a PM shape —
/// a total variance `m0` and the PM19.5 wind `u` that sets where its energy
/// sits. [`wave_set_fetch`] hands that pair to [`bands`], which integrates it
/// into sixteen equal-energy bands; this evaluates the *density* that same
/// cumulative is built from, [`pm_density`], at every grid cell's own `k`
/// instead — scaled so its own integral is `m0` too, and fanned by the same
/// `cosᵖ(θ)` spread with the same [`SPREAD_POWER`]. Two resolutions of one
/// spectrum, not two spectra — `amplitude_field_agrees_with_wave_set_fetch`
/// checks the two against each other directly, in `Hs`, because a density and
/// a band scheme agreeing on paper is not the same as their totals agreeing
/// in `f32`. A grid cell is not a point, either: the density is multiplied by
/// the cell's own area in k-space, `(Δk)²`, before it becomes a variance —
/// omitting that was round 1's bug, and it scaled the total with `patch`
/// alone, a purely numerical parameter with no counterpart in the sixteen
/// waves at all.
///
/// # k = 0 and the Hermitian mirror
///
/// The `k = 0` cell — `x == z == n/2`, since `k = 2π·(x − n/2, z − n/2) /
/// patch` — is left zero: a mean offset is not a wave. Every other cell's
/// conjugate is *written*, not hoped for: this draws one hashed cell per
/// mirror pair and copies its conjugate into the other, so `h0(−k) =
/// conj(h0(k))` holds by construction rather than by relying on the hash to
/// land there on its own. **Which of the pair gets hashed is not the
/// lexicographically smaller index** — a first version of this function used
/// that, and *its* directional cone was a hard ±90° cutoff rather than a
/// magnitude, so exactly one of `{k, −k}` generically fell inside it and the
/// other was zero by construction. Picking the grid-index-smaller one at
/// random relative to wind direction hashed the *zero* side about half the
/// time and threw its mirror's true energy away outright — this hashes
/// whichever side the wind actually reaches, decided by the **sign of
/// `k · ŵ`**. A cell that is its own mirror — `k = 0`, or a Nyquist row/column
/// when `n` is even — is forced real, because Hermitian symmetry demands it
/// equal its own conjugate.
///
/// **That dot product is the same rule, evaluated the same way, as
/// [`crate::ocean::Ocean::new`]'s half-plane sign**, down to the `dot == 0.0`
/// tie-break on grid index. An earlier version decided the same question here
/// through `atan2` then `cos` — a second implementation of one boolean, on a
/// different floating-point path, in a different file. It is one expression
/// now so that there is nothing to drift.
///
/// It is worth being exact about what that drift would have cost, because it
/// is less than it looks: Hermitian symmetry forces both members of a pair to
/// share a magnitude, so the live side chosen here only decides *which cell's
/// hash is drawn*, and the direction a mode travels is set in `ocean.rs`
/// alone. Flipping this comparison and leaving `ocean.rs` alone moves no test.
/// The hazard is latent rather than live — it becomes real the moment the
/// cone goes back to being a hard cutoff (`cos.max(0)`) instead of the
/// magnitude below, which is the shape it had when picking the wrong side
/// threw a pair's energy away outright.
///
/// # The band, and why a cascade is a slice rather than a copy
///
/// `band` is `[k_lo, k_hi)` in rad/m: a cell whose `|k|` falls outside it is
/// left zero. **Half-open, deliberately** — that is what lets adjacent bands
/// meet at one number and tile `k`-space with no cell counted twice and none
/// skipped. [`WHOLE_BAND`] is the whole spectrum, which is what a lone cascade
/// wants.
///
/// It exists because [`crate::ocean::Cascade`]s stack. Without it every
/// cascade samples the *entire* spectrum, so a wavenumber both grids can carry
/// is drawn once per cascade and the realised variance grows with the cascade
/// count: measured **6.073 m of `Hs` at one cascade, 8.598 m at two and
/// 10.559 m at three** over the same total wavenumber range, against one
/// answer the spectrum has for all three.
/// `cascades_are_a_band_not_another_copy_of_the_sea` is that invariant, in
/// `ocean.rs`; it reads 6.069 / 6.096 / 6.128 m once the bands are applied.
///
/// The calibration is unaffected and that is the point: `s_k_one_sided` is
/// scaled so that summing it over the whole grid reproduces `m0`, so summing
/// it over a set of bands that tile reproduces `m0` too. Bands that overlap
/// double-count; bands that gap lose energy. Neither is checked here — the
/// tiling is a property of a *stack* of cascades, and [`crate::ocean::Ocean`]
/// is where a stack exists, so that is where the assertion lives.
///
/// # Why the calibration is in variance
///
/// `Σ_k |h0(k)|²` is the sea's variance `m0`, not `m0/N²`, **because
/// [`crate::fft::ifft_2d`] is deliberately unnormalised** and `ocean.rs`
/// applies no `1/N²` on the way out. Parseval for that convention makes the
/// raw transform's variance exactly this sum. The three decisions are only
/// correct together; changing any one of them without the other two lands a
/// twenty-foot sea at 0.37 mm.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::similar_names)]
pub fn amplitude_field(
    n: usize,
    patch: f32,
    u10: f32,
    fetch: f32,
    direction: [f32; 2],
    band: [f32; 2],
    seed: u32,
) -> Vec<Complex> {
    let mut field = vec![Complex::default(); n * n];
    let Some((m0, u)) = spectrum_shape(u10, fetch) else {
        return field; // Below MIN_WIND_SPEED: a flat sea, every cell zero.
    };
    if n == 0 || !patch.is_finite() || patch <= 0.0 {
        return field;
    }

    let scale_to_m0 = m0 / natural_m0(u);
    let along = normalise(direction).unwrap_or([1.0, 0.0]);
    let along_theta = along[1].atan2(along[0]);
    let spread_norm = cos_power_integral(SPREAD_POWER);
    let delta_k = std::f32::consts::TAU / patch;
    // `s_k` below is a *density* — energy per unit area of k-space — and a
    // grid cell covers `(Δk)²` of that area, not a point in it. Skipping this
    // is the bug this file's `spectrum_shape` was built to make impossible
    // and got away with it anyway: `Σ|h0|²` came out scaling with `patch`
    // alone (300x too hot at the plan's own 200 m test patch, 16000x at
    // 800 m), because `patch` sets `Δk` and nothing was multiplying by it.
    // With it, `Σ|h0|²` tracks `wave_set_fetch`'s `m0` regardless of `patch`
    // or `n` — see `amplitude_field_agrees_with_wave_set_fetch`.
    let cell_area = delta_k * delta_k;
    let half = (n / 2) as f32;

    for z in 0..n {
        for x in 0..n {
            let (mz, mx) = ((n - z) % n, (n - x) % n);
            // Each unordered {cell, mirror} pair is drawn once, from its
            // lexicographically smaller index — the mirror is never drawn
            // from its own hash, only copied as a conjugate below.
            if (z, x) > (mz, mx) {
                continue;
            }

            let kx = (x as f32 - half) * delta_k;
            let kz = (z as f32 - half) * delta_k;
            let k = (kx * kx + kz * kz).sqrt();

            let value = if k <= 0.0 || k < band[0] || k >= band[1] {
                // Out of this cascade's band — see the docs above. `k <= 0.0`
                // is the `k = 0` cell and is not a band question: a mean
                // offset is not a wave at any wavenumber.
                Complex::default()
            } else {
                // The cosᵖ spread is π-periodic (even power), but the ±90°
                // cone that zeroes anything running against the wind is not:
                // of a mirrored pair `{k, −k}`, exactly one generically falls
                // inside it. Hashing whichever index happens to be
                // lexicographically smaller — as the first version of this
                // function did — picks the *upwind*, zero-spread side about
                // half the time, and that half's true energy (its mirror's)
                // is never drawn at all; it is silently thrown away rather
                // than merely misplaced. So this hashes whichever side the
                // wind actually reaches.
                //
                // **Which side is live is `sign(k · ŵ)`, written exactly as
                // `ocean.rs` writes it**, tie-break included. One rule, one
                // expression: deciding it a second way — `atan2` then `cos`,
                // as an earlier version did — is two floating-point paths to
                // one boolean, and see this function's docs for what that can
                // and cannot cost.
                //
                // It also removes the angle wrap outright rather than dodging
                // it. `atan2` returns `(-π, π]`, so a difference of two of
                // them lands anywhere in `(-2π, 2π)`: an `|θ| <= π/2` cone
                // rejects *both* sides of a pair for a wind near ±π and
                // silently zeroes it (measured `m0` 1.27 against a target of
                // 1.99 for `[-1, 0]`, invisible along `[1, 0]`). A dot product
                // has no branch cut to wrap around.
                let dot = kx * along[0] + kz * along[1];
                let live_is_c = dot > 0.0 || (dot == 0.0 && (z, x) < (mz, mx));
                let (hash_x, hash_z) = if live_is_c {
                    (x as u32, z as u32)
                } else {
                    (mx as u32, mz as u32)
                };
                // Only the spread *magnitude* still needs the angle: the live
                // side's `|cos(θ − θ_wind)|`. The mirror's angle is `θ ± π`,
                // so its cosine is `−cos θ` and the magnitude is shared.
                //
                // **`.abs()`, and never `.max(0.0)` — this line is where that
                // edit gets made.** `kx` and `kz` are *this* cell's, and the
                // loop visits the lexicographically smaller index of each pair,
                // which is the upwind member about half the time. Clamping
                // instead of taking the magnitude therefore zeroes the spread
                // for about half the pairs in the grid and throws their energy
                // away outright rather than misplacing it — the historical bug,
                // and invisible in a still, because the sea simply comes out
                // small. The function's docs say the same thing from the other
                // end; this is the keystroke.
                let cos_live = (kz.atan2(kx) - along_theta).cos().abs();

                let (g1, g2) = box_muller(seed, hash_x, hash_z);

                let omega = (GRAVITY * k).sqrt();
                let s_omega = pm_density(omega, u) * scale_to_m0;
                let jacobian_dw_dk = GRAVITY / (2.0 * omega); // ω = √(gk)
                // **There is no ±90° cutoff here and nothing to clamp.** This
                // is a magnitude: `|cos|^p` is zero only for a cell lying
                // exactly across the wind, and falls off smoothly either side.
                // Both members of a mirror pair carry it, because Hermitian
                // symmetry forces them to share a magnitude — which side the
                // energy *travels* is `ocean.rs`'s half-plane sign and is not
                // decided by this factor. See `cos_live` above before editing.
                let spread = cos_live.powi(SPREAD_POWER) / spread_norm;
                // 2D areal density: the 1D-in-k density, divided by k to
                // spread it over a ring, fanned by direction — then scaled by
                // the cell's own area in k-space, `(Δk)²`, to turn a density
                // into the variance the downwind cone actually carries.
                let s_k_one_sided = s_omega * jacobian_dw_dk / k * spread * cell_area;
                // `s_k_one_sided` is calibrated (via `scale_to_m0`) so that
                // summing it once per pair, over the downwind cone alone,
                // reproduces `wave_set_fetch`'s `m0` — that's what
                // `amplitude_field_agrees_with_wave_set_fetch` checks. But
                // both cells of a Hermitian pair end up with the *same*
                // magnitude, the mirror being a conjugate and not a second
                // wave, so Parseval summing the whole grid counts every
                // pair's variance twice unless each cell only carries half
                // of it. This is `S(k)` as the formula below means it: the
                // two-sided density, after that halving.
                let s_k = s_k_one_sided * 0.5;
                let amplitude = (0.5 * s_k).max(0.0).sqrt();
                let hashed = Complex { re: g1 * amplitude, im: g2 * amplitude };
                // Whichever of {c, mirror} was live carries the drawn value;
                // the other is its conjugate, same as the self-paired case.
                if live_is_c { hashed } else { Complex { re: hashed.re, im: -hashed.im } }
            };

            if (z, x) == (mz, mx) {
                // Self-paired: k = 0, or a Nyquist row/column at even n.
                // Hermitian symmetry forces this cell to be real.
                field[z * n + x] = Complex { re: value.re, im: 0.0 };
            } else {
                field[z * n + x] = value;
                field[mz * n + mx] = Complex { re: value.re, im: -value.im };
            }
        }
    }

    field
}

/// A unit vector, or `None` for anything that has no direction.
fn normalise(v: [f32; 2]) -> Option<[f32; 2]> {
    let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if length > 0.0 && length.is_finite() {
        Some([v[0] / length, v[1] / length])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The whole `h0` field, pinned bit for bit.
    ///
    /// **This covers more than the spectrum arithmetic.** [`box_muller`] calls `ln` and
    /// `sin_cos`, and [`spectrum_shape`] a chain of `exp`/`powf` — all libm, none of them
    /// IEEE-exact, any of them free to move by a ULP across a glibc version or a different
    /// machine. `loom_field::noise` is an integer lattice hash for exactly this reason and
    /// is compared exactly; this field cannot be, so it is pinned instead. That is what
    /// makes the cross-machine reproducibility ADR 0076 sells checkable rather than
    /// asserted.
    ///
    /// **A failure means the field moved — not that it is wrong.** Every other test here
    /// is a self-comparison, an inequality or a statistical band, and all of them will
    /// still pass with the field a ULP away; this is the only one that can see it. What it
    /// tells you is that every water simulation hash on this machine now differs from the
    /// one that produced the recorded value.
    ///
    /// **Do not re-bless it casually.** After a system update the quick fix is to paste the
    /// new number in, and that destroys the only evidence that anything changed. Find out
    /// *what* moved first — toolchain, `ldd --version`, the machine — and say so in the
    /// commit that changes the literal.
    #[test]
    fn the_amplitude_field_is_pinned() {
        let field = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 7);
        assert_eq!(
            digest(field.iter().flat_map(|c| [c.re, c.im])),
            0x1f52_da46_b012_dfcd,
            "the amplitude field moved — read this test's doc comment before touching the literal"
        );
    }

    /// One water body's worth of TOML, so the real validator judges the real
    /// derived set rather than a re-implementation of its rules.
    fn water_scene(waves: &WaveSet) -> String {
        let mut src = String::from(
            "[scene]\nformat = 1\n\n[[node]]\nname = \"Ocean\"\n\n\
             [node.components.WaterBody]\nsurface_height = 0.0\n\n\
             [node.components.WaterBody.waves]\n",
        );
        src.push_str(&format!(
            "attenuation_depth = {:?}\nmax_height = {:?}\n",
            waves.attenuation_depth, waves.max_height
        ));
        for w in &waves.waves {
            src.push_str(&format!(
                "\n[[node.components.WaterBody.waves.waves]]\n\
                 wavelength = {:?}\namplitude = {:?}\nsteepness = {:?}\n\
                 direction = [{:?}, {:?}]\nspeed_scale = {:?}\n",
                w.wavelength,
                w.amplitude,
                w.steepness,
                w.direction[0],
                w.direction[1],
                w.speed_scale
            ));
        }
        src
    }

    /// **The trap, measured.** A sea that is internally consistent proves
    /// nothing about whether it is the right size. The published relation for a
    /// fully developed sea is `H⅓ = 0.21·U19.5²/g ≈ 0.22·U10²/g` — WikiWaves,
    /// *Ocean-Wave Spectra*, after Stewart's *Introduction to Physical
    /// Oceanography* §16.1 — and this is the derivation checked against it at
    /// five wind speeds.
    ///
    /// Getting the reference height wrong is exactly what this catches: feeding
    /// U10 straight into a spectrum written for U19.5 would come out 4.8% low
    /// in wave height, and using a `1/7` power law for the conversion instead
    /// would come out 15% high.
    #[test]
    fn significant_wave_height_matches_the_published_relation() {
        for u10 in [5.0_f32, 10.0, 15.0, 20.0, 25.0] {
            let derived = significant_height(&wave_set(u10, [1.0, 0.0]));
            let published = 0.22 * u10 * u10 / GRAVITY;
            let error = (derived - published).abs() / published;
            assert!(
                error < 0.01,
                "at U10 = {u10} m/s the derived sea is Hs = {derived:.3} m, \
                 the published fully-developed value is {published:.3} m ({:.2}% off)",
                error * 100.0
            );
        }
    }

    /// The peak of the spectrum is inside the sampled range rather than off one
    /// end of it. `ω_p = 0.877·g/U19.5` is where PM has its maximum; a set whose
    /// frequencies all sat above it would be chop with the swell missing.
    #[test]
    fn the_sampled_band_straddles_the_spectral_peak() {
        let u10 = 12.0_f32;
        let peak = 0.877 * GRAVITY / (u10 * U19_5_PER_U10);
        let omega = |w: &GerstnerWave| (GRAVITY * std::f32::consts::TAU / w.wavelength).sqrt();

        let waves = wave_set(u10, [1.0, 0.0]);
        let lowest = omega(waves.waves.first().expect("a sea at 12 m/s has waves"));
        let highest = omega(waves.waves.last().expect("a sea at 12 m/s has waves"));

        assert!(lowest < peak, "the longest wave {lowest} is above the peak {peak}");
        assert!(highest > peak, "the shortest wave {highest} is below the peak {peak}");
    }

    /// **Every derived sea validates, light airs through a gale.** A derivation
    /// that produces a self-intersecting surface is a bug in the derivation, so
    /// this walks the whole Beaufort scale through the real scene validator —
    /// which checks the steepness limit, the wave cap and every schema range.
    #[test]
    fn every_sea_from_light_airs_to_a_hurricane_validates() {
        let mut speed = MIN_WIND_SPEED;
        while speed <= MAX_WIND_SPEED {
            let waves = wave_set(speed, [0.8, -0.6]);
            assert_eq!(waves.waves.len(), MAX_WAVES, "at {speed} m/s");
            if let Err(errors) = loom_scene::Scene::parse(&water_scene(&waves)) {
                panic!("the sea derived at U10 = {speed} m/s does not validate: {errors:#?}");
            }
            speed += 0.25;
        }
    }

    /// Below the mirror threshold there are no waves at all, and the surface is
    /// flat rather than covered in waves the schema would reject.
    #[test]
    fn a_dead_calm_is_a_mirror() {
        for u10 in [0.0_f32, 0.1, 0.49, -3.0, f32::NAN] {
            assert!(wave_set(u10, [1.0, 0.0]).waves.is_empty(), "at {u10} m/s");
        }
        assert!(!wave_set(MIN_WIND_SPEED, [1.0, 0.0]).waves.is_empty());
    }

    /// The sea spreads around the wind instead of marching along it: every wave
    /// is within 90° of the wind, the fan is symmetric about it, and most of the
    /// energy is near the middle.
    #[test]
    fn the_waves_spread_around_the_wind() {
        let along = [0.0_f32, 1.0];
        let waves = wave_set(14.0, along);

        let mut headings = Vec::new();
        let mut energy_x = 0.0_f32;
        let mut energy_z = 0.0_f32;
        for w in &waves.waves {
            let dot = w.direction[0] * along[0] + w.direction[1] * along[1];
            assert!(dot > 0.0, "a wave runs against the wind: {:?}", w.direction);
            headings.push(dot);
            let energy = w.amplitude * w.amplitude;
            energy_x += w.direction[0] * energy;
            energy_z += w.direction[1] * energy;
        }

        // Symmetric: the energy-weighted mean direction is the wind's.
        assert!(energy_x.abs() < 1e-4 * energy_z.abs(), "the fan is lopsided: {energy_x}");
        assert!(energy_z > 0.0);
        // And actually spread — not sixteen waves pointing one way.
        let widest = headings.iter().fold(1.0_f32, |a, &b| a.min(b));
        assert!(widest < 0.5, "the widest wave is only {widest} off the wind");
    }

    /// A harder wind builds a bigger, longer sea. Monotone in both, because an
    /// agent turning the wind up and getting a smaller sea has no way to tell
    /// that from a bug in its own scene.
    #[test]
    fn a_harder_wind_builds_a_bigger_sea() {
        let mut previous = (0.0_f32, 0.0_f32);
        for u10 in [1.0_f32, 3.0, 6.0, 10.0, 16.0, 24.0, 32.0] {
            let waves = wave_set(u10, [1.0, 0.0]);
            let height = significant_height(&waves);
            let longest = waves.waves[0].wavelength;
            assert!(height > previous.0, "Hs fell from {} to {height} at {u10} m/s", previous.0);
            assert!(longest > previous.1, "λ fell from {} to {longest} at {u10} m/s", previous.1);
            previous = (height, longest);
        }
    }

    /// **The motion artifact this step exists to prevent.** The wind swings 90°
    /// in one tick; the sea must lag it and converge, not snap.
    ///
    /// The follow is exactly exponential, so the numbers are checkable rather
    /// than eyeballed: the remaining angle is `90°·e^(−t/τ)`, which is 56.9° of
    /// the turn done at one time constant and 95.0% at three.
    #[test]
    fn the_sea_does_not_snap_when_the_wind_turns() {
        let dt = 1.0 / 60.0;
        let mut sea = SeaState::new([1.0, 0.0], 12.0);
        let turned = |sea: &SeaState| {
            let d = sea.direction();
            d[1].atan2(d[0]).to_degrees()
        };

        // One tick after the shift the sea has barely moved: this is the assert
        // that fails if anyone replaces the follow with an assignment.
        sea.advance([0.0, 1.0], 12.0, dt, SLEW_SECONDS);
        assert!(turned(&sea) < 0.1, "the sea snapped {}° in one tick", turned(&sea));

        let mut previous = turned(&sea);
        let mut at = Vec::new();
        for tick in 1..(300 * 60) {
            sea.advance([0.0, 1.0], 12.0, dt, SLEW_SECONDS);
            let now = turned(&sea);
            assert!(now >= previous - 1e-4, "the turn reversed: {previous} then {now}");
            assert!(now <= 90.0 + 1e-3, "the sea overshot the wind: {now}");
            previous = now;
            if tick % (60 * 60) == 0 {
                at.push(now);
            }
        }

        // 1τ, 2τ, 3τ, 4τ: 90·(1 − e^(−n)).
        for (index, &measured) in at.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let expected = 90.0 * (1.0 - (-(index as f32 + 1.0)).exp());
            assert!(
                (measured - expected).abs() < 0.5,
                "after {}τ the sea has turned {measured}°, exponential says {expected}°",
                index + 1
            );
        }
        assert!(previous > 89.0, "after five minutes the sea has only turned {previous}°");
    }

    /// The height lags too. A wind that drops to nothing leaves a sea that
    /// takes a while to lie down, which is the same inertia seen on the other
    /// axis.
    #[test]
    fn the_sea_height_lags_the_wind_as_well() {
        let dt = 1.0 / 60.0;
        let mut sea = SeaState::new([1.0, 0.0], 20.0);
        let big = significant_height(&sea.waves());

        for _ in 0..60 {
            sea.advance([1.0, 0.0], 0.0, dt, SLEW_SECONDS);
        }
        let after_a_second = significant_height(&sea.waves());
        assert!(after_a_second > big * 0.9, "the sea collapsed in a second: {after_a_second}");

        for _ in 0..(60 * 300) {
            sea.advance([1.0, 0.0], 0.0, dt, SLEW_SECONDS);
        }
        assert!(sea.waves().waves.is_empty(), "the sea never lay down: {:?}", sea.u10());
    }

    /// A time constant of zero is "follow instantly", which is what a scene
    /// that wants no inertia would author. It must not divide by zero into a
    /// NaN direction.
    #[test]
    fn a_zero_time_constant_follows_instantly() {
        let mut sea = SeaState::new([1.0, 0.0], 8.0);
        sea.advance([0.0, -1.0], 3.0, 1.0 / 60.0, 0.0);

        assert!((sea.direction()[0]).abs() < 1e-6, "{:?}", sea.direction());
        assert!((sea.direction()[1] + 1.0).abs() < 1e-6, "{:?}", sea.direction());
        assert!((sea.u10() - 3.0).abs() < 1e-6);
    }

    /// Degenerate inputs give a sea rather than a NaN — a zero wind vector, a
    /// negative timestep, a non-finite speed. One NaN here poisons every
    /// determinism hash downstream.
    #[test]
    fn degenerate_inputs_do_not_poison_the_sea() {
        let mut sea = SeaState::new([0.0, 0.0], f32::NAN);
        sea.advance([0.0, 0.0], f32::INFINITY, -1.0, SLEW_SECONDS);
        sea.advance([f32::NAN, 1.0], 9.0, f32::NAN, SLEW_SECONDS);
        sea.advance([0.0, 0.0], 9.0, 1.0, SLEW_SECONDS);

        assert!(sea.direction().iter().all(|c| c.is_finite()), "{:?}", sea.direction());
        assert!(sea.u10().is_finite());
        for w in &sea.waves().waves {
            assert!(w.wavelength.is_finite() && w.amplitude.is_finite() && w.steepness.is_finite());
        }
    }

    /// The same inputs give bit-identical waves. The wave set feeds a
    /// simulation and a determinism hash, so "near enough" is not enough.
    #[test]
    fn the_derivation_is_reproducible() {
        let once = wave_set(13.5, [0.3, 0.9]);
        let again = wave_set(13.5, [0.3, 0.9]);

        assert_eq!(once, again);
    }

    /// Total steepness, the quantity the validator bounds at 1.
    fn fold(waves: &WaveSet) -> f32 {
        waves
            .waves
            .iter()
            .map(|w| {
                w.steepness * (std::f32::consts::TAU / w.wavelength) * w.amplitude
            })
            .sum()
    }

    /// **The bug this whole feature exists for, stated as a test.**
    ///
    /// A fully-developed sea is scale-invariant, so its total steepness is the
    /// *same number* at a light air and at a hurricane — which is why raising
    /// the wind in a PM scene makes the sea bigger and never rougher. The
    /// fetch-limited sea at a fixed fetch does what a player expects instead.
    #[test]
    fn a_fully_developed_sea_never_gets_steeper_and_a_fetch_limited_one_does() {
        let flat: Vec<f32> =
            [3.0_f32, 8.0, 16.0, 30.0].iter().map(|&u| fold(&wave_set(u, [1.0, 0.0]))).collect();
        for pair in flat.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() < 1e-4,
                "PM steepness moved with the wind: {flat:?}"
            );
        }

        let limited: Vec<f32> = [3.0_f32, 8.0, 16.0, 30.0]
            .iter()
            .map(|&u| fold(&wave_set_fetch(u, [1.0, 0.0], 3000.0)))
            .collect();
        for pair in limited.windows(2) {
            assert!(pair[1] > pair[0] + 0.01, "fetch-limited sea did not steepen: {limited:?}");
        }
        assert!(
            limited[0] > flat[0],
            "even the calm end should be steeper than PM: {limited:?} vs {flat:?}"
        );
    }

    /// Past the fully-developed fetch there is no such thing as a fetch, and
    /// the two functions must be the *same* function — not merely close.
    ///
    /// This is the footgun `FULLY_DEVELOPED_FETCH` documents, pinned so that a
    /// future change to either law cannot make the seam discontinuous.
    #[test]
    fn an_unlimited_fetch_is_exactly_pierson_moskowitz() {
        for &u in &[1.0_f32, 5.0, 12.0, 25.0, 42.0] {
            let unlimited = wave_set_fetch(u, [0.3, 0.9], 1.0e9);
            assert_eq!(unlimited, wave_set(u, [0.3, 0.9]), "at u10 {u}");
        }
        // And zero or nonsense is unlimited rather than a division by zero.
        assert_eq!(wave_set_fetch(9.0, [1.0, 0.0], 0.0), wave_set(9.0, [1.0, 0.0]));
        assert_eq!(wave_set_fetch(9.0, [1.0, 0.0], f32::NAN), wave_set(9.0, [1.0, 0.0]));
        assert!(wave_set_fetch(9.0, [1.0, 0.0], 3000.0).waves.iter().all(|w| {
            w.wavelength.is_finite() && w.amplitude.is_finite() && w.steepness.is_finite()
        }));
    }

    /// **What keeps the demo's boat inside its own storm.**
    ///
    /// The hull is 19.07 m with a 1.65 m draft and twelve pontoons that
    /// saturate over 2.19 m of submersion, so what breaks it is *height*, and
    /// a fully-developed sea has no ceiling on height at all — `Hs ∝ U²`, up
    /// to twenty-nine metres inside the legal wind range. At a 3 km fetch the
    /// whole range fits inside 1.18 m, which is the property the demo
    /// leans on and the reason it can raise the wind at all.
    ///
    /// Deliberately a bound on `Hs` and not on a derived period: the four
    /// bands here carry *equal energy* by construction, so there is no single
    /// wave to call the peak and any "Tp" read off this set is an arbitrary
    /// pick dressed up as a measurement. The hull's real response is measured
    /// against the real solver, not approximated here.
    #[test]
    fn a_three_kilometre_fetch_keeps_the_whole_wind_range_inside_a_metre() {
        for &u in &[3.0_f32, 10.0, 20.0, 30.0, MAX_WIND_SPEED] {
            let hs = significant_height(&wave_set_fetch(u, [1.0, 0.0], 3000.0));
            // 1.175 m at the U10 clamp of 42 is the measured worst case; the
            // bound is that plus a little, so a change to either law that
            // moved the ceiling by ten per cent would be caught.
            assert!(hs < 1.25, "3 km fetch at U10 {u} gives Hs {hs} m");
        }
        // Fault injection: the same measurement on the sea this replaces has
        // to fail, or the bound is measuring nothing.
        let pm = significant_height(&wave_set(20.0, [1.0, 0.0]));
        assert!(pm > 5.0, "PM at U10 20 should be enormous, got {pm}");
    }
}

#[cfg(test)]
mod amplitude_tests {
    use super::*;

    /// **The field must be Hermitian, or the sea has an imaginary part.**
    /// `h0(-k) = conj(h0(k))` is what makes the inverse transform of a spectrum real, and
    /// getting it wrong produces a surface that looks plausible and is not a height field.
    ///
    /// Not a completeness check on its own — an all-zero field is trivially Hermitian too,
    /// so this only proves symmetry, not that there is a sea here at all.
    /// `a_harder_wind_is_a_bigger_sea` and `amplitude_field_agrees_with_wave_set_fetch`
    /// are what rule that out.
    #[test]
    fn the_field_is_hermitian() {
        let n = 16;
        let h0 = amplitude_field(n, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 7);
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
        let a = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 3);
        let b = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 3);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.re.to_bits(), y.re.to_bits());
            assert_eq!(x.im.to_bits(), y.im.to_bits());
        }
    }

    /// A different seed is a different sea, or the seed is not doing anything.
    #[test]
    fn a_different_seed_is_a_different_sea() {
        let a = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 3);
        let b = amplitude_field(32, 200.0, 12.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 4);
        assert!(a.iter().zip(b.iter()).any(|(x, y)| x.re.to_bits() != y.re.to_bits()));
    }

    /// **Zero wind is a flat sea and not a NaN.** The spectrum divides by `U`, so this is
    /// the edge every wave model gets wrong once. `pool.loom` authors `Wind.speed = 0`.
    #[test]
    fn no_wind_is_a_flat_sea() {
        let h0 = amplitude_field(16, 200.0, 0.0, 100_000.0, [1.0, 0.0], WHOLE_BAND, 1);
        for (i, c) in h0.iter().enumerate() {
            assert!(c.re.is_finite() && c.im.is_finite(), "cell {i} is not finite");
            assert!(c.re.abs() < 1e-3 && c.im.abs() < 1e-3, "cell {i} has energy in no wind");
        }
    }

    /// More wind is more energy, which is the one monotonic claim this module makes.
    #[test]
    fn a_harder_wind_is_a_bigger_sea() {
        let energy = |u: f32| -> f32 {
            amplitude_field(32, 200.0, u, 500_000.0, [1.0, 0.0], WHOLE_BAND, 1)
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .sum()
        };
        let (calm, gale) = (energy(5.0), energy(18.0));
        assert!(gale > calm * 4.0, "calm {calm}, gale {gale}");
    }

    /// **The field's total energy must agree with `wave_set_fetch`'s, or the two paths
    /// disagree about what an 18 m/s sea is** — the constraint the brief opens with, and
    /// the one round 1's bug broke by orders of magnitude (patch-dependent, up to 16000x
    /// at 800 m). `4·√(Σ|h0|²)` is the field's implied `Hs`, by the same `Hs = 4√m0`
    /// relation `significant_height` uses — `m0` there is `ΣA²/2` over the sixteen waves,
    /// `Σ|h0|²` here is its Parseval equivalent for the grid.
    ///
    /// Checked across three `patch`es and two `n`s because that is exactly the bug's
    /// signature: it moved with `patch` and didn't move with anything about the wind.
    /// Also checked across five wind headings, including two whose `along_theta` sits
    /// near ±π — see the comment in the body: pinned to `[1, 0]`, this test could not
    /// see the missing angle wrap in the directional cone at all.
    ///
    /// Each point **averages 200 seeds**, not one — a fetch-limited spectrum this narrow
    /// puts most of its energy in a handful of grid cells (`k_peak ≈ 0.031` against a
    /// `Δk` of `0.01–0.03` at these patches), so a single draw's `Σ|h0|²` swings up to
    /// ±70% around its own expectation by chance alone; two hundred draws bring that
    /// under the tolerance below without hiding a real, non-random disagreement. **15%**,
    /// because even the mean still truncates the spectrum below the grid's lowest
    /// resolvable `k` and above its Nyquist — exact agreement was never the target, and
    /// demanding it would make this flaky rather than sharp. Measured (seeds 0–199,
    /// `u10 = 18`, `fetch = 376 km`): every one of the thirty points lands within 8.0% of
    /// the target, with no trend toward either edge as `patch`, `n` or heading changes —
    /// see the task report for the full table and for the numbers before each fix.
    #[test]
    fn amplitude_field_agrees_with_wave_set_fetch() {
        let u10 = 18.0_f32;
        // The fetch is arbitrary here and only has to be the *same* fetch on both
        // sides — this test asks whether `amplitude_field` and `wave_set_fetch` agree,
        // not how big the sea is. (376 km is a 5.638 m sea, not the twenty-foot one an
        // earlier version of this comment claimed; 6.10 m needs 440 km, which is what
        // `the_realised_sea_is_the_size_the_spectrum_promised` uses.)
        let fetch = 376_000.0_f32;
        // **Directions, not just `[1, 0]`.** The energy a heading carries cannot
        // depend on the heading, so every one of these must land on the same
        // `m0` — and `[-1, 0]` / `[-1, -1]` are the headings whose `along_theta`
        // sits near ±π, where an unwrapped angle difference falls outside the
        // ±90° cone on *both* sides of a mirror pair and silently zeroes it.
        // Pinning this test to `[1, 0]`, as its first version did, is why that
        // bug survived a round of review: measured `m0` was 2.04 along `+x` and
        // 1.27 along `-x` against a target of 1.99, and nothing here could see it.
        for &dir in &[[1.0_f32, 0.0], [0.0, 1.0], [-1.0, 0.0], [-1.0, -1.0], [-1.0, 0.05]] {
            let target_m0: f32 = {
                let hs = significant_height(&wave_set_fetch(u10, dir, fetch));
                (hs / 4.0) * (hs / 4.0)
            };

            for &n in &[32usize, 64] {
                for &patch in &[200.0_f32, 400.0, 800.0] {
                    // 200 seeds, not 24. Thirty (direction, n, patch) points
                    // instead of six is thirty chances for one coarse grid's
                    // handful of dominant cells to draw badly, and 24 was
                    // already marginal: `[0, 1]` at n=64/patch=200 came out
                    // 23% high on 24 seeds and 4.4% high on 200. Expectation
                    // is direction-independent by symmetry, so that spread was
                    // sampling noise, not a heading the grid dislikes. At 200
                    // the worst point is 8.0% (`[1, 0]`, n=32, patch=200 —
                    // genuine truncation, `Δk ≈ k_peak` there) and re-running
                    // every point at seed offset +9000 moves the worst to
                    // 6.3%, so the margin under 15% is real rather than lucky.
                    // Costs 0.85 s for all thirty points.
                    let trials = 200_u32;
                    let mean_m0: f32 = (0..trials)
                        .map(|seed| {
                            amplitude_field(n, patch, u10, fetch, dir, WHOLE_BAND, seed)
                                .iter()
                                .map(|c| c.re * c.re + c.im * c.im)
                                .sum::<f32>()
                        })
                        .sum::<f32>()
                        / trials as f32;

                    let error = (mean_m0 - target_m0).abs() / target_m0;
                    assert!(
                        error < 0.15,
                        "dir={dir:?} n={n} patch={patch}: field m0 ({trials}-seed mean) is \
                         {mean_m0:.3}, wave_set_fetch's is {target_m0:.3} ({:.1}% off)",
                        error * 100.0
                    );
                }
            }
        }
    }
}
