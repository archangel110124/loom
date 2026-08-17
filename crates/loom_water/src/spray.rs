//! Spray thrown off a crest that is breaking — W5's first source.
//!
//! **The same quantity the whitecaps are painted from.** `WaterSample::fold` is
//! `Σ Q·k·A·sin φ`, undivided, so 1.0 is a cusp and the validator caps the sea
//! below it (see [`crate::WaterSample::fold`]). W2 renders foam by
//! `smoothstep(WATER_FOAM_WET, WATER_FOAM_BREAK, fold)` in the water shader;
//! this throws droplets from the *same* threshold, so the spray leaves the
//! crests that are already going white rather than from a second opinion about
//! where a wave is breaking.
//!
//! **Closed form, no state, and never read by anything.** A droplet is a
//! function of `(body, t, region)` — the cell it came from, the slot of time it
//! was born in, and a ballistic arc from there. Nothing accumulates, nothing is
//! read back, nothing produces a force, and no assertion can see it. That is
//! ADR 0045 clause 1 satisfied by not having any state to argue about, and it
//! is the same shape as `loom_rain::splashes`, deliberately.
//!
//! **The region follows the eye, and that is allowed here specifically.**
//! Spray is drawn and nothing else, so ADR 0045's trap clause — a *force*
//! grid must anchor to sim state, never the camera — does not apply. The eye is
//! how the population stays bounded on an unbounded ocean, exactly as
//! `loom_rain::splashes` bounds itself.

use crate::sample_water;
use loom_field::noise::hash;
use loom_scene::components::WaterBody;

/// Fold at which a crest starts throwing droplets.
///
/// **`WATER_FOAM_BREAK` in `scene.slang`, spelled again.** The two numbers are
/// one decision — where a crest is breaking — and if they drift the spray comes
/// off water that is not white and the foam appears where nothing sprays. Set
/// from σ(fold) = 0.189 on the seven-wave `ocean`, which makes this 1.75σ: the
/// steepest few percent of the surface, not every crest.
pub const SPRAY_BREAK: f32 = 0.33;

/// Metres per candidate cell. One crown per cell per [`SPRAY_PERIOD`] at most.
pub const SPRAY_CELL: f32 = 1.5;

/// Seconds between a cell's chances to throw.
pub const SPRAY_PERIOD: f32 = 0.5;

/// How long a droplet lives, in seconds.
///
/// Under [`SPRAY_PERIOD`] × 2, which is what lets the search below look at only
/// two slots: a droplet born three slots ago is already gone.
pub const SPRAY_LIFETIME: f32 = 0.9;

/// How far from the eye spray is thrown, in metres.
///
/// Beyond this a droplet is well under a pixel and costs a sample to decide
/// not to draw. The same argument `loom_rain::SPLASH_RANGE` makes.
pub const SPRAY_RANGE: f32 = 34.0;

/// Droplets per crown.
///
/// §2.2e: "a crown is a ring emitted with outward+upward velocity". Seven
/// rather than eight so the ring does not read as a square from above.
pub const SPRAY_CROWN: usize = 7;

/// Metres per second outward and upward at a fold of exactly 1.0 — the cusp.
///
/// Scaled by how far past [`SPRAY_BREAK`] the crest actually is, so a swell
/// that barely breaks lifts a puff and a storm crest throws.
const SPRAY_OUT: f32 = 1.9;
const SPRAY_UP: f32 = 3.4;

/// Gravity on a droplet. Plain `g` — a water droplet this size is ballistic
/// over a metre, and air drag on it is a term nobody can see.
const SPRAY_GRAVITY: f32 = crate::GRAVITY;

/// One droplet in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Droplet {
    pub position: [f32; 3],
    /// How far through its life it is, in `[0, 1)`. Drives the fade.
    pub fraction: f32,
}

/// The largest `fold` this wave set can ever reach, anywhere, ever.
///
/// `fold` is `Σ Q·k·A·sin φ` and the phases are independent, so the ceiling is
/// `Σ Q·k·A` — reached only at a point where every wave crests together, which
/// a real sea approaches and never quite hits. Under [`SPRAY_BREAK`] no point
/// of this sea can break at any place or instant, so [`spray`] returns nothing
/// for the whole run however large `WaterBody::spray` is.
///
/// **This exists because that no-op is invisible.** Authoring `spray = 2.0` on
/// a gentle swell produces no droplet, no warning and no error, and the author
/// concludes the feature is broken. It was: every sea in this repository was
/// under the threshold when spray shipped, which is how the threshold came to
/// be measured at all.
///
/// Computed at full depth, which is the ceiling: `shoal` only ever *reduces*
/// an amplitude, so a shelving sea folds less than this and never more.
#[must_use]
pub fn peak_fold(body: &WaterBody) -> f32 {
    body.waves
        .waves
        .iter()
        .take(loom_scene::components::MAX_WAVES)
        .filter(|w| w.wavelength > 0.0)
        .map(|w| w.steepness * (std::f32::consts::TAU / w.wavelength) * w.amplitude)
        .sum()
}

/// Every droplet in the air around `eye` at time `t`.
///
/// `ground` answers the bed height under a point, for the same reason
/// [`sample_water`] takes one: this crate does not know what a voxel is. A
/// closure that returns [`loom_voxel::heightfield::NO_GROUND`]'s value — any
/// large negative — is an open sea.
///
/// Returned in cell order and never sorted: the order is what the renderer
/// draws in, and a different order is a different additive sum.
#[must_use]
pub fn spray(
    body: &WaterBody,
    eye: [f32; 3],
    t: f32,
    ground: &dyn Fn(f32, f32) -> f32,
) -> Vec<Droplet> {
    let mut out = Vec::new();
    // A mirror throws nothing, and a sea that authors no spray throws nothing —
    // which is what keeps every reference image of every sea unmoved. Asking
    // costs a grid of `sample_water` calls, so both are checked before the loop.
    if body.spray <= 0.0 || body.waves.waves.is_empty() || t < 0.0 {
        return out;
    }

    #[allow(clippy::cast_possible_truncation)]
    let cell_of = |v: f32| (v / SPRAY_CELL).floor() as i32;
    let (x0, x1) = (cell_of(eye[0] - SPRAY_RANGE), cell_of(eye[0] + SPRAY_RANGE));
    let (z0, z1) = (cell_of(eye[2] - SPRAY_RANGE), cell_of(eye[2] + SPRAY_RANGE));
    #[allow(clippy::cast_possible_truncation)]
    let slot_now = (t / SPRAY_PERIOD).floor() as i32;

    for iz in z0..=z1 {
        for ix in x0..=x1 {
            // Two slots is the whole history: a droplet outlives its slot by
            // less than one more.
            for slot in (slot_now - 1)..=slot_now {
                crown_in(body, [ix, iz], slot, eye, t, ground, &mut out);
            }
        }
    }
    out
}

/// One cell's crown for one slot of time, if it threw one.
fn crown_in(
    body: &WaterBody,
    cell: [i32; 2],
    slot: i32,
    eye: [f32; 3],
    t: f32,
    ground: &dyn Fn(f32, f32) -> f32,
    out: &mut Vec<Droplet>,
) {
    #[allow(clippy::cast_sign_loss)]
    let seed = hash(
        hash(hash(cell[0] as u32).wrapping_add(cell[1] as u32)).wrapping_add(slot as u32),
    );
    #[allow(clippy::cast_precision_loss)]
    let unit = |shift: u32| ((hash(seed ^ shift) >> 8) as f32) * (1.0 / 16_777_216.0);

    // Born somewhere inside its cell and somewhere inside its slot, so crowns
    // do not appear on a lattice on a metronome.
    #[allow(clippy::cast_precision_loss)]
    let born = (slot as f32 + unit(1)) * SPRAY_PERIOD;
    let age = t - born;
    if !(0.0..SPRAY_LIFETIME).contains(&age) {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let at = [
        (cell[0] as f32 + unit(2)) * SPRAY_CELL,
        (cell[1] as f32 + unit(3)) * SPRAY_CELL,
    ];
    // Round, not square: the cell grid is axis-aligned and a square region
    // around the eye puts spray 1.4× further away on the diagonals, which is
    // where it is thinnest and least worth the samples.
    if (at[0] - eye[0]).hypot(at[1] - eye[2]) > SPRAY_RANGE {
        return;
    }

    // **Sampled at the moment of birth, not now.** The crown is thrown by the
    // crest that was there when it left; evaluating the surface at `t` would
    // make a droplet's arc depend on water it is no longer touching.
    let surface = sample_water(body, at, born, ground(at[0], at[1]), [0.0; 3], [0.0; 3]);
    if surface.fold <= SPRAY_BREAK {
        return;
    }
    // Dry land throws nothing: `sample_water` flattens the waves to nothing at
    // the shoreline, but the fold of a wave set is not zero at zero amplitude
    // until `shoal` has taken every one of them, and a crown standing on a
    // beach is the artifact.
    if surface.depth <= 0.0 {
        return;
    }
    let strength = ((surface.fold - SPRAY_BREAK) / (1.0 - SPRAY_BREAK)).clamp(0.0, 1.0);
    // Not every breaking crest throws. Without this the whole steep half of a
    // storm sea sprays at once, which reads as a fog rather than as spray.
    //
    // **The author's multiplier is on the population, not on the throw.** A
    // droplet's arc is what the crest that threw it can do; how many crests
    // throw is a look. Scaling the velocity instead would put spray in the air
    // that the surface underneath it cannot account for.
    if unit(4) > strength * body.spray {
        return;
    }

    // The crest's own velocity carries the crown downwind — the orbital motion
    // `sample_water` already computed, not a second wind term.
    let drift = [surface.velocity[0], surface.velocity[2]];
    let base = [
        at[0] + surface.displacement[0],
        surface.height,
        at[1] + surface.displacement[2],
    ];
    let spin = unit(5) * std::f32::consts::TAU;
    for i in 0..SPRAY_CROWN {
        #[allow(clippy::cast_precision_loss)]
        let angle = spin + std::f32::consts::TAU * (i as f32) / (SPRAY_CROWN as f32);
        let outward = SPRAY_OUT * strength;
        let up = SPRAY_UP * strength;
        let v = [
            drift[0] + angle.cos() * outward,
            up,
            drift[1] + angle.sin() * outward,
        ];
        out.push(Droplet { position: ballistic(base, v, age), fraction: age / SPRAY_LIFETIME });
    }
}

/// Where a droplet launched at `v` from `base` is after `age` seconds.
///
/// The one line both crowns share. Plain `g` and no drag, for the reason
/// [`SPRAY_GRAVITY`] gives.
fn ballistic(base: [f32; 3], v: [f32; 3], age: f32) -> [f32; 3] {
    [
        base[0] + v[0] * age,
        v[1].mul_add(age, base[1]) - 0.5 * SPRAY_GRAVITY * age * age,
        base[2] + v[2] * age,
    ]
}

// ---------------------------------------------------------------------------
// W9: the impact crown — the *other* source §2.2e names.
// ---------------------------------------------------------------------------
//
// **W5 specified two sources and shipped one.** Everything above is driven by
// `WaterSample::fold`, the crest-steepness term, so a flat pool throws nothing
// however hard something falls into it — `assets/test/pool.loom` exists to make
// that visible and did. This half is driven by the *event*: a body broke the
// surface at a speed, and the impact sets the droplets' velocity.
//
// It is the same kind of object as the one above — a pure function of
// `(where, how hard, how old, a seed)`, no state, no readback, nothing an
// assertion can see. ADR 0045 clause 1 satisfied by having nothing to argue
// about, exactly as the crest crown satisfies it.

/// Downward speed, m/s, below which an entry is a settling rather than a splash.
///
/// **The gate that suppresses the phantom at tick zero as well.** A body
/// authored already floating has `fraction == 0.0` before its first solve, so
/// its first solve reads as an entry; it is also barely moving, so this is what
/// tells the two apart. Both jobs are one number on purpose — a second constant
/// would be a second thing to keep in step.
pub const SPLASH_MIN_SPEED: f32 = 1.0;

/// Impact speed at which the crown is at full size, m/s.
///
/// `pool.loom`'s sphere enters at about 7.1 m/s measured, so a three-metre drop
/// throws very nearly the largest crown there is and a half-metre drop throws a
/// visibly smaller one. That ratio is the acceptance test, not the absolute.
pub const SPLASH_FULL_SPEED: f32 = 8.0;

/// Fraction of the impact speed a droplet leaves with, upward and outward.
///
/// **Not tuned by eye: the ratio is what sets the crown's height**, and the
/// lifetime falls out of it ballistically rather than being a second authored
/// number that can disagree. At `SPLASH_FULL_SPEED` the upward fraction gives
/// 2.7 m/s, so the tallest droplet reaches 0.37 m and is gone in 0.55 s — a
/// crown, on the scale of the thing that made it, rather than a fountain.
pub const SPLASH_UP_FRAC: f32 = 0.34;
/// Outward fraction. Under the upward one: a crown is taller than it is wide,
/// which is what distinguishes it from a ring of spray.
pub const SPLASH_OUT_FRAC: f32 = 0.22;

/// Droplets per band of the crown.
pub const SPLASH_RING: usize = 8;
/// Bands at full strength. A marginal entry throws fewer — see [`crown`].
pub const SPLASH_BANDS: usize = 3;

/// The crown an impact throws, `age` seconds after it happened.
///
/// `at` is the point on the surface the body broke — the surface *including*
/// the wake, which the caller already computed for the submersion event.
/// `speed` is positive **downward**: how hard it hit. `radius` is the body's
/// waterplane radius, and it is the geometric point of this function —
///
/// **the ring starts at the rim of the cavity, never at the centre.** A crown
/// rises from the edge of the hole the body punched; droplets launched from the
/// centre would be launched from inside the body, and on anything wider than a
/// droplet the crown reads as a spout coming out of the object rather than as
/// water thrown aside by it.
///
/// Empty below [`SPLASH_MIN_SPEED`], and empty once every band has landed.
/// Returned in band-then-ring order and never sorted, like [`spray`].
#[must_use]
pub fn crown(at: [f32; 3], speed: f32, radius: f32, age: f32, seed: u32) -> Vec<Droplet> {
    let strength =
        ((speed - SPLASH_MIN_SPEED) / (SPLASH_FULL_SPEED - SPLASH_MIN_SPEED)).clamp(0.0, 1.0);
    if strength <= 0.0 || age < 0.0 {
        return Vec::new();
    }
    // **Count follows the impact, not only speed.** A marginal entry throws one
    // band and a hard one throws three, so how much water is in the air is a
    // reading of how hard the thing hit rather than a constant with a scale on
    // it. `ceil` rather than `round`: anything past the gate throws something.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bands = (strength * SPLASH_BANDS as f32).ceil().max(1.0) as usize;
    #[allow(clippy::cast_precision_loss)]
    let spin = ((hash(seed) >> 8) as f32) * (1.0 / 16_777_216.0) * std::f32::consts::TAU;
    // **Saturated, not extrapolated.** The velocity is a fraction of the impact
    // speed, so an unbounded speed is an unbounded crown: a body arriving at
    // 40 m/s would throw droplets thirteen metres up and leave them in the air
    // for nearly three seconds, which is a geyser rather than a splash and
    // would sit in a `--sim N` still long after the thing that caused it. The
    // ramp already stops counting at this speed; the arc stops with it.
    let impact = speed.min(SPLASH_FULL_SPEED);

    let mut out = Vec::new();
    for band in 0..bands {
        // 0 for the innermost band, approaching 1 for the outermost. The outer
        // water is thrown out and the inner water is thrown up, which is the
        // shape of a real crown and also what makes it collapse from the
        // outside in: a flatter arc lands sooner.
        #[allow(clippy::cast_precision_loss)]
        let tilt = (band as f32 + 0.5) / bands as f32;
        let up = impact * SPLASH_UP_FRAC * 0.45f32.mul_add(-tilt, 1.0);
        let outward = impact * SPLASH_OUT_FRAC * 0.9f32.mul_add(tilt, 0.55);
        // Ballistic, so the lifetime is not a second authored number that can
        // disagree with the velocity: up, over, and back to the surface.
        let lifetime = 2.0 * up / SPRAY_GRAVITY;
        if age >= lifetime {
            continue;
        }
        for i in 0..SPLASH_RING {
            // Half-step per band so the bands interleave rather than stacking
            // into eight radial spokes.
            #[allow(clippy::cast_precision_loss)]
            let angle = std::f32::consts::TAU
                .mul_add((i as f32 + 0.5 * band as f32) / SPLASH_RING as f32, spin);
            let (dir_x, dir_z) = (angle.cos(), angle.sin());
            // The rim. See the doc comment: this is the whole geometry.
            let base = [dir_x.mul_add(radius, at[0]), at[1], dir_z.mul_add(radius, at[2])];
            let v = [dir_x * outward, up, dir_z * outward];
            out.push(Droplet { position: ballistic(base, v, age), fraction: age / lifetime });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_scene::components::{GerstnerWave, WaveSet};

    /// A sea steep enough to break, and one that is not.
    fn sea(amplitude: f32, steepness: f32) -> WaterBody {
        WaterBody {
            spray: 1.0,
            waves: WaveSet {
                waves: vec![
                    GerstnerWave {
                        wavelength: 9.0,
                        amplitude,
                        steepness,
                        direction: [1.0, 0.2],
                        speed_scale: 1.0,
                    },
                    GerstnerWave {
                        wavelength: 5.0,
                        amplitude: amplitude * 0.6,
                        steepness,
                        direction: [0.7, -0.7],
                        speed_scale: 1.0,
                    },
                ],
                ..WaveSet::default()
            },
            ..WaterBody::default()
        }
    }

    fn deep(_x: f32, _z: f32) -> f32 {
        -1000.0
    }

    /// **A mirror throws nothing**, which is also the test that every scene
    /// with flat water in it renders as it did before spray existed.
    #[test]
    fn still_water_throws_no_spray() {
        let flat = WaterBody { spray: 4.0, ..WaterBody::default() };
        for tick in 0..120 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 / 60.0;
            assert!(spray(&flat, [0.0, 2.0, 0.0], t, &deep).is_empty());
        }
    }

    /// **And the unauthored default throws nothing however hard it blows**,
    /// which is the whole of the compatibility story: every sea committed
    /// before W5 has `spray = 0` and renders bit for bit as it did.
    #[test]
    fn a_sea_that_authors_no_spray_throws_none() {
        let storm = WaterBody { spray: 0.0, ..sea(0.55, 0.85) };
        for tick in 0..600 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 / 60.0;
            assert!(spray(&storm, [0.0, 2.0, 0.0], t, &deep).is_empty());
        }
    }

    /// A gentle swell never reaches the break threshold; a steep sea does. The
    /// point is that the threshold is a threshold and not a formality.
    #[test]
    fn only_a_breaking_sea_sprays() {
        let calm = sea(0.12, 0.2);
        let storm = sea(0.55, 0.85);
        let count = |body: &WaterBody| {
            (0..240)
                .map(|tick| spray(body, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep).len())
                .sum::<usize>()
        };

        assert_eq!(count(&calm), 0, "a swell that never breaks threw spray");
        assert!(count(&storm) > 0, "a breaking sea threw none");
    }

    /// The same instant twice is the same spray — the property everything in
    /// this file rests on, and the one a clock would destroy.
    #[test]
    fn the_same_second_gives_the_same_spray() {
        let storm = sea(0.55, 0.85);
        let a = spray(&storm, [3.0, 2.0, -4.0], 5.25, &deep);
        let b = spray(&storm, [3.0, 2.0, -4.0], 5.25, &deep);

        assert!(!a.is_empty(), "the sea threw nothing to compare");
        assert_eq!(a, b);
    }

    /// Droplets are ballistic: a crown thrown a moment ago is above the crest
    /// it left, and one thrown a second ago is on its way back down.
    #[test]
    fn a_crown_goes_up_and_comes_back() {
        let storm = sea(0.55, 0.85);
        // Every droplet's height above the still-water line, by age band.
        let mut early = f32::NEG_INFINITY;
        let mut late = f32::NEG_INFINITY;
        for tick in 0..600 {
            for d in spray(&storm, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep) {
                if d.fraction < 0.15 {
                    early = early.max(d.position[1]);
                } else if d.fraction > 0.85 {
                    late = late.max(d.position[1]);
                }
            }
        }

        assert!(early.is_finite() && late.is_finite(), "no droplets in one band");
        assert!(late < early, "a droplet at the end of its life is still rising: {late} vs {early}");
    }

    /// Nothing is thrown outside the region, which is what bounds the cost on
    /// an ocean with no edges.
    #[test]
    fn spray_stays_near_the_eye() {
        let storm = sea(0.55, 0.85);
        let eye = [40.0, 2.0, -20.0];
        let out = spray(&storm, eye, 9.0, &deep);
        assert!(!out.is_empty());
        for d in &out {
            let distance = (d.position[0] - eye[0]).hypot(d.position[2] - eye[2]);
            // The crown is born inside the range and travels a little further.
            assert!(distance < SPRAY_RANGE + 6.0, "{d:?} is {distance} m from {eye:?}");
        }
    }

    /// **`peak_fold` is a ceiling, and the warning built on it is sound.**
    ///
    /// Two halves, because a bound that is not a bound sends the author to fix
    /// waves that were fine: no sample anywhere in the field may exceed it, and
    /// a sea whose ceiling is under `SPRAY_BREAK` must throw nothing — which is
    /// exactly the claim `particles::spray` prints.
    #[test]
    fn peak_fold_bounds_every_sample_and_predicts_a_dry_sea() {
        for body in [sea(0.12, 0.2), sea(0.55, 0.85), sea(0.3, 0.5)] {
            let ceiling = peak_fold(&body);
            let mut worst = f32::NEG_INFINITY;
            for tick in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let t = tick as f32 / 60.0;
                for i in -20_i16..20 {
                    for j in -20_i16..20 {
                        let at = [f32::from(i) * 0.7, f32::from(j) * 0.7];
                        worst = worst
                            .max(sample_water(&body, at, t, -400.0, [0.0; 3], [0.0; 3]).fold);
                    }
                }
            }
            assert!(
                worst <= ceiling + 1e-4,
                "fold reached {worst}, over the {ceiling} ceiling peak_fold promised"
            );
            if ceiling < SPRAY_BREAK {
                let thrown: usize = (0..240)
                    .map(|tick| {
                        #[allow(clippy::cast_precision_loss)]
                        let t = tick as f32 / 60.0;
                        spray(&body, [0.0, 2.0, 0.0], t, &deep).len()
                    })
                    .sum();
                assert_eq!(thrown, 0, "a sea under the ceiling threw {thrown} droplets");
            }
        }
    }

    /// Dry land does not spray. The waves flatten toward a shoreline, so the
    /// fold falls off with them — but this asserts it rather than trusting it,
    /// because a crown standing on a beach is the artifact everyone sees.
    #[test]
    fn spray_stops_at_the_shoreline() {
        let mut shore = sea(0.55, 0.85);
        shore.waves.attenuation_depth = 8.0;
        // A bed above the surface everywhere: the whole region is dry.
        let land = |_x: f32, _z: f32| 1.0_f32;

        for tick in 0..240 {
            let out = spray(&shore, [0.0, 2.0, 0.0], tick as f32 / 60.0, &land);
            assert!(out.is_empty(), "spray on dry land: {out:?}");
        }
    }

    // -- W9, the impact crown -------------------------------------------------

    /// **A gentle entry throws nothing**, which is the gate that keeps a body
    /// authored already floating from splashing on its first solve: its
    /// fraction starts at zero, so the edge detector reads an entry, and this
    /// is what tells that apart from an impact.
    #[test]
    fn a_settling_body_throws_no_crown() {
        for speed in [0.0_f32, 0.2, 0.9, SPLASH_MIN_SPEED] {
            assert!(
                crown([0.0, 0.0, 0.0], speed, 0.5, 0.0, 1).is_empty(),
                "an entry at {speed} m/s threw a crown"
            );
        }
    }

    /// **The crown starts at the rim, not at the centre.** Every droplet is at
    /// the body's own waterplane radius at birth, which is the correction that
    /// keeps a crown from reading as a spout out of the object.
    #[test]
    fn the_ring_starts_at_the_cavity_rim() {
        let radius = 0.5;
        let out = crown([3.0, 0.25, -2.0], 7.1, radius, 0.0, 7);
        assert!(!out.is_empty());
        for d in &out {
            let r = (d.position[0] - 3.0).hypot(d.position[2] + 2.0);
            assert!((r - radius).abs() < 1e-4, "born {r} m out, not at the {radius} m rim");
            assert!((d.position[1] - 0.25).abs() < 1e-6, "born off the surface: {d:?}");
        }
    }

    /// **Size follows the impact.** A three-metre drop and a half-metre drop
    /// differ in droplet count and in peak height by about the speed ratio —
    /// which is the whole reason the event carries a speed.
    #[test]
    fn a_harder_impact_throws_a_bigger_crown() {
        let peak = |speed: f32| {
            let mut best: f32 = 0.0;
            let mut count = 0;
            for step in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let age = step as f32 / 60.0;
                let out = crown([0.0, 0.0, 0.0], speed, 0.5, age, 3);
                count = count.max(out.len());
                for d in &out {
                    best = best.max(d.position[1]);
                }
            }
            (best, count)
        };
        // 3 m and 0.55 m of free fall: 7.1 m/s measured on `pool.loom`, 3.3 m/s.
        let (hard, hard_n) = peak(7.1);
        let (soft, soft_n) = peak(3.3);
        assert!(hard > soft * 2.0, "peak {hard} m against {soft} m is not a harder impact");
        assert!(hard_n > soft_n, "{hard_n} droplets against {soft_n}");
        assert!(soft_n > 0, "a 0.55 m drop threw nothing at all");
    }

    /// **It ends.** Every band is ballistic, so the crown empties itself rather
    /// than needing a lifetime nobody can see — and a still at `--sim 120` must
    /// not have last second's splash hanging in the air.
    #[test]
    fn the_crown_lands() {
        let full = crown([0.0, 0.0, 0.0], SPLASH_FULL_SPEED, 0.5, 0.0, 5);
        assert_eq!(full.len(), SPLASH_RING * SPLASH_BANDS, "a full crown is every band");
        // The tallest band is 2*up/g at the saturated impact speed: 0.55 s.
        assert!(crown([0.0, 0.0, 0.0], 40.0, 0.5, 1.0, 5).is_empty(), "the crown never landed");
    }

    /// **Two impacts on one tick are two different crowns.** Same shape, turned
    /// differently — the seed is the caller's `salt(tick, at)`, so two bodies
    /// going in together do not throw one splash drawn twice.
    #[test]
    fn the_seed_turns_the_ring() {
        let a = crown([0.0, 0.0, 0.0], 6.0, 0.4, 0.1, 11);
        let b = crown([0.0, 0.0, 0.0], 6.0, 0.4, 0.1, 12);
        assert_eq!(a.len(), b.len());
        assert!(a != b, "two seeds threw the same crown");
    }
}
