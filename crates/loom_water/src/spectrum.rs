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

use loom_scene::components::{GerstnerWave, MAX_WAVES, WaveSet};

use crate::GRAVITY;

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
    let u10 = if u10.is_finite() { u10.min(MAX_WIND_SPEED) } else { 0.0 };
    if u10 < MIN_WIND_SPEED {
        return WaveSet::default();
    }
    // A non-positive or non-finite fetch is "unlimited", which is PM. Guarded
    // rather than clamped because zero fetch is not a flat sea asymptotically —
    // it is a division by zero in the peak law.
    if !fetch.is_finite() || fetch <= 0.0 {
        return wave_set(u10, direction);
    }
    let dimensionless = GRAVITY * fetch / (u10 * u10);
    if dimensionless >= FULLY_DEVELOPED_FETCH {
        return wave_set(u10, direction);
    }
    let along = normalise(direction).unwrap_or([1.0, 0.0]);

    let hs = 0.0016 * dimensionless.sqrt() * u10 * u10 / GRAVITY;
    // Hs = 4√m0, the same relation `significant_height` inverts.
    let m0 = (hs / 4.0) * (hs / 4.0);

    let omega_peak =
        std::f32::consts::TAU * 3.5 * (GRAVITY / u10) * dimensionless.powf(-0.33);
    let u = PM_PEAK * GRAVITY / omega_peak;

    bands(m0, u, along)
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
