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
pub const SPRAY_CELL: f32 = 3.0;

/// Seconds between a cell's chances to throw.
pub const SPRAY_PERIOD: f32 = 0.7;

/// How long a droplet lives, in seconds.
///
/// Under [`SPRAY_PERIOD`] × 2, which is what lets the search below look at only
/// two slots: a droplet born three slots ago is already gone.
pub const SPRAY_LIFETIME: f32 = 1.1;

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
        out.push(Droplet {
            position: [
                base[0] + v[0] * age,
                base[1] + v[1] * age - 0.5 * SPRAY_GRAVITY * age * age,
                base[2] + v[2] * age,
            ],
            fraction: age / SPRAY_LIFETIME,
        });
    }
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
}
