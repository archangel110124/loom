//! Is it raining here, and how hard.
//!
//! **The authoritative half of Phase 4, and the only half that is in the
//! simulation.** The streaks a later slice draws are stateless GPU noise that
//! nothing reads back; what gameplay, wetness, splashes and audio all key off
//! is this — a rate in millimetres per hour at a world point, and the sky
//! exposure it was scaled by.
//!
//! **The occlusion answer is [`loom_voxel::exposure::sky_exposure`] and nothing
//! else.** S3 already marches the voxel SDF to say how open a direction is;
//! ADR 0007 records why it is a march rather than a ray query. A second
//! occlusion answer here would let rain fall through a roof that blocks the
//! wind and muffles a footstep, with nothing in the code to blame — which is
//! the divergence this project has paid for more than once.
//!
//! Audio's `openness` is deliberately *not* this number. It casts against the
//! collision world; this marches the voxel volume, and a scene with no voxels
//! would read as wide open to one and correctly enclosed to the other.

use loom_field::wind::Wind;
use loom_scene::components::Rain;
use loom_voxel::Volume;

/// What stands between a point and the sky.
///
/// A voxel volume plus where its node sits in the world, because a `Volume`
/// has no idea where it is — [`loom_voxel::exposure`] works in the volume's own
/// frame, so somebody has to subtract the node's translation and it should be
/// here rather than at every call site.
///
/// **Rotation and scale on the node are ignored**, exactly as
/// `loom_voxel::heightfield` and the grass path ignore them. Nothing authors a
/// rotated terrain volume, and handling one means putting the whole query into
/// the volume's local frame rather than only its translation.
#[derive(Debug, Clone, Copy)]
pub struct Sky<'a> {
    /// The field to march.
    pub volume: &'a Volume,
    /// World position of the volume's origin corner.
    pub offset: [f32; 3],
}

/// What the rain is doing at one point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainSample {
    /// Rain actually reaching this point, in millimetres per hour.
    ///
    /// The authored intensity scaled by [`Self::exposure`], so it is zero under
    /// a sealed roof, full under open sky, and a fraction at the lip of an
    /// overhang. This is the number to assert on and the number a later slice
    /// scales its streak count by.
    pub rate: f32,
    /// Fraction of the sky this point can see, from 0 (sealed) to 1 (clear).
    ///
    /// Reported alongside the rate rather than folded away, because the things
    /// downstream need both: a wall dries at a rate set by how open it is even
    /// when the rate is zero, and rain audio is low-passed by exposure rather
    /// than by how hard it is raining.
    pub exposure: f32,
    /// The wind at this point, in m/s — what a streak leans along.
    ///
    /// P1's field, sheltered by the very same [`Self::exposure`] through
    /// [`Wind::sheltered`], so the lean dies exactly where the rain does
    /// instead of somewhere nearby. This crate computes no wind of its own.
    pub wind: [f32; 3],
}

/// The rain at a world point, at a time.
///
/// `rain` is the scene's authored component, or `None` for a scene that
/// authors none — **which means dry, not a default drizzle.** Every scene
/// written before rain existed must simulate exactly as it did, and a
/// component that silently switched itself on would change all of them.
///
/// `sky` is `None` for a scene with no voxel volume: there is nothing to shelter
/// under, so exposure is 1. That is the same answer [`loom_voxel::exposure`]
/// gives for a point outside the field it was handed.
///
/// **Nothing here is cached.** The march runs against the volume as it is at
/// the moment of the call, so carving an overhang open with a CSG op lets rain
/// in on the next query rather than after a reload — the phase's sharpest exit
/// criterion, and the reason this takes a `&Volume` rather than anything baked
/// from one.
#[must_use]
pub fn sample_rain(
    rain: Option<&Rain>,
    wind: &Wind,
    sky: Option<Sky>,
    at: [f32; 3],
    t: f32,
) -> RainSample {
    let exposure = sky.map_or(1.0, |sky| {
        let local = [
            at[0] - sky.offset[0],
            at[1] - sky.offset[1],
            at[2] - sky.offset[2],
        ];
        loom_voxel::exposure::sky_exposure(sky.volume, local)
    });
    // A negative rate is not a thing weather does. The schema range already
    // rejects one, but this value is multiplied into wetness and particle
    // counts downstream and a sign flip there has no visible symptom.
    //
    // A shower that has ended is not raining either. `duration` is `None` for
    // rain that never stops, which is every scene authored before it existed.
    let intensity = rain.map_or(0.0, |rain| {
        if t > rain.duration.unwrap_or(f32::INFINITY) {
            0.0
        } else {
            rain.intensity.max(0.0)
        }
    });

    RainSample {
        rate: intensity * exposure,
        exposure,
        wind: wind.sheltered(at, t, exposure),
    }
}

/// How wet a surface is, as the two numbers a wet surface actually has.
///
/// **Two, and that is the whole point.** Water on a surface is a film sitting
/// on top of it and water in a surface is soaked into its pores, and they
/// behave differently: the film is what makes a road shine, and it is gone
/// minutes after the rain stops; the soak is what makes it dark, and it is
/// still there a quarter of an hour later. Collapse them into one number and
/// drying becomes a crossfade back to dry — every property recovering together,
/// which is exactly what real drying does not look like.
///
/// Both are fractions from 0 (bone dry) to 1 (as wet as this rain gets it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wetness {
    /// The water film. Drives **roughness** — a wet surface is smoother, so its
    /// highlight tightens and brightens.
    pub film: f32,
    /// Water absorbed into the material. Drives **albedo darkening**, scaled by
    /// the material's own porosity.
    pub soak: f32,
}

/// Rain, in mm/h, at which a surface ends up as wet as it is going to get.
///
/// **Low, and deliberately not the streak layer's `RAIN_FULL_RATE`.** Those are
/// different questions that happen to be measured in the same unit: how many
/// drops to draw scales with the rate all the way up to a downpour, but a
/// surface is a surface — light rain wets it completely given time, and heavy
/// rain does not make it wetter than wet, only faster. Sharing one constant
/// would mean a drizzle could never darken anything.
pub const SOAKING_RATE: f32 = 2.0;

/// Seconds for the water film to build to 1 - 1/e of its level. A surface
/// glosses over almost as soon as it starts raining.
const FILM_WET: f32 = 4.0;
/// Seconds for the film to drain and evaporate away. Minutes, not hours.
const FILM_DRY: f32 = 30.0;
/// Seconds for water to soak into the material. Slower than the film by an
/// order of magnitude — it has to get past the surface first.
const SOAK_WET: f32 = 45.0;
/// Seconds for the soak to dry out.
///
/// **Ten times [`FILM_DRY`], and that ratio is the effect.** Specular vanishes
/// faster than the darkening recovers (Lagarde; fxguide's write-up of the same
/// model), and it is what makes a drying surface read as drying rather than as
/// a dissolve back to its dry self: for the first minute after the rain the
/// sheen is going while the stain is not.
const SOAK_DRY: f32 = 300.0;

/// `1 - e^{-t/tau}`, the approach to equilibrium.
fn approach(t: f32, tau: f32) -> f32 {
    1.0 - (-t / tau).exp()
}

/// How wet a surface at this exposure is, after `t` seconds of the scene's rain.
///
/// **Closed form, and there is no accumulator anywhere.** A scene's rain is one
/// authored intensity that runs for one authored stretch, so the differential
/// equation this would otherwise be integrated from has an exact solution — and
/// evaluating it is both cheaper than stepping a per-surface accumulator and
/// impossible to desynchronise from the value the renderer is handed. A stored
/// accumulator would be a second representation of a number the formula already
/// gives, free to drift from it, and it would put wetness in the world's state
/// hash for nothing.
///
/// It is nonetheless **CPU state in the sense that matters**: it is
/// deterministic, it is a function of the simulated clock and never a wall
/// clock, `loom sim --assert "wetness@x,y,z.film"` reads it, and a script asking
/// "is this wet" would read the same call. Nothing on the GPU computes it and
/// nothing on the GPU writes it back — the shader is handed two floats.
///
/// `exposure` is [`RainSample::exposure`], S3's march. Passing 1 asks what a
/// surface in the open would be.
///
/// **What it cannot express is history.** Wetness here is a function of the
/// *current* exposure, so a floor that spent five minutes under a roof and was
/// then carved open reads as though it had been out in the rain the whole time,
/// instead of starting dry and wetting up. Fixing that needs stored per-surface
/// state — a grid or a texture with an update path — and ADR 0014's trigger list
/// is where to record it if a scene ever needs it.
#[must_use]
pub fn wetness(rain: Option<&Rain>, exposure: f32, t: f32) -> Wetness {
    let Some(rain) = rain else {
        // The same rule `sample_rain` follows: a scene that authors no rain is
        // dry, so every scene written before this phase renders unchanged.
        return Wetness { film: 0.0, soak: 0.0 };
    };
    let rate = rain.intensity.max(0.0) * exposure.clamp(0.0, 1.0);
    let equilibrium = (rate / SOAKING_RATE).min(1.0);

    // `None` is rain that never stops, so the wet phase is the whole run and
    // the dry phase is empty — `t - inf` clamps to zero and both decays are 1.
    let falling = rain.duration.unwrap_or(f32::INFINITY).max(0.0);
    let wet = t.max(0.0).min(falling);
    let dry = (t - falling).max(0.0);

    Wetness {
        film: equilibrium * approach(wet, FILM_WET) * (-dry / FILM_DRY).exp(),
        soak: equilibrium * approach(wet, SOAK_WET) * (-dry / SOAK_DRY).exp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_voxel::{CsgMode, VoxelOp};

    /// A 64 m cube of world with a flat slab roof over the middle of it, at the
    /// origin — the same shape S3's own tests use, so a failure here is a
    /// failure in the combining rather than in the march.
    fn roofed() -> Volume {
        let mut volume = Volume::new([2, 2, 2], 1.0);
        volume.bake(&[VoxelOp::Box {
            center: [32.0, 20.0, 32.0],
            half_extents: [10.0, 1.0, 10.0],
            mode: CsgMode::Union,
        }]);
        volume
    }

    fn heavy() -> Rain {
        Rain { intensity: 16.0, duration: None }
    }

    #[test]
    fn open_sky_gets_the_whole_authored_rate() {
        let volume = roofed();
        let sky = Some(Sky { volume: &volume, offset: [0.0; 3] });

        let out = sample_rain(Some(&heavy()), &Wind::default(), sky, [5.0, 10.0, 5.0], 1.0);

        assert!(out.rate > 15.9, "open sky read {} mm/h", out.rate);
        assert!(out.exposure > 0.99);
    }

    /// **The first exit criterion, at the state level.** Rain stops under an
    /// overhang.
    #[test]
    fn an_overhang_stops_it() {
        let volume = roofed();
        let sky = Some(Sky { volume: &volume, offset: [0.0; 3] });

        let out = sample_rain(Some(&heavy()), &Wind::default(), sky, [32.0, 10.0, 32.0], 1.0);

        assert!(out.rate < 0.16, "under a solid roof read {} mm/h", out.rate);
    }

    /// **The sharp exit criterion.** Carving the overhang open with a CSG op
    /// lets rain in *in the same tick* — no reload, no rebake, no second call
    /// to anything but the query itself. This is what a cached exposure would
    /// fail, and caching it is the obvious optimisation somebody will reach for.
    #[test]
    fn carving_the_roof_lets_rain_in_the_same_tick() {
        let mut volume = roofed();
        let rain = heavy();
        let wind = Wind::default();
        let at = [32.0, 10.0, 32.0];

        let before = sample_rain(
            Some(&rain),
            &wind,
            Some(Sky { volume: &volume, offset: [0.0; 3] }),
            at,
            1.0,
        );
        let touched = volume.edit(&VoxelOp::Box {
            center: [32.0, 20.0, 32.0],
            half_extents: [4.0, 3.0, 4.0],
            mode: CsgMode::Subtract,
        });
        let after = sample_rain(
            Some(&rain),
            &wind,
            Some(Sky { volume: &volume, offset: [0.0; 3] }),
            at,
            1.0,
        );

        assert!(!touched.is_empty(), "the carve touched no chunks");
        assert!(before.rate < 0.16, "should have started sheltered: {before:?}");
        assert!(after.rate > 15.9, "should be soaked after carving: {after:?}");
    }

    /// The node's translation moves its shelter with it. Ignoring the offset
    /// leaves the roof at the world origin no matter where the scene put it,
    /// and every rain query in a scene whose terrain is not at `[0, 0, 0]` is
    /// then quietly wrong.
    #[test]
    fn the_shelter_moves_with_the_node() {
        let volume = roofed();
        let offset = [-16.0, 0.0, -16.0];
        let sky = Some(Sky { volume: &volume, offset });
        let rain = heavy();
        let wind = Wind::default();

        // Local [32, 10, 32] — under the slab — is world [16, 10, 16].
        let under = sample_rain(Some(&rain), &wind, sky, [16.0, 10.0, 16.0], 1.0);
        // The world point that *was* sheltered when the offset was zero is now
        // out from under the slab entirely.
        let beside = sample_rain(Some(&rain), &wind, sky, [32.0, 10.0, 32.0], 1.0);

        assert!(under.rate < 0.16, "the roof did not move with the node: {under:?}");
        assert!(beside.rate > 15.9, "the roof stayed at the origin: {beside:?}");
    }

    /// **A scene that authors no rain is dry**, which is what keeps every
    /// simulation hash written before this phase unchanged. Exposure is still
    /// reported: drying and audio need it in fair weather too.
    #[test]
    fn a_scene_without_rain_is_dry() {
        let volume = roofed();
        let sky = Some(Sky { volume: &volume, offset: [0.0; 3] });

        let out = sample_rain(None, &Wind::default(), sky, [5.0, 10.0, 5.0], 1.0);

        assert_eq!(out.rate, 0.0);
        assert!(out.exposure > 0.99, "exposure is still an answer: {out:?}");
    }

    /// No voxels in the scene is not the same as sealed. Returning 0 here
    /// would stop rain in every scene that never grew terrain.
    #[test]
    fn a_scene_without_voxels_is_open_sky() {
        let out = sample_rain(Some(&heavy()), &Wind::default(), None, [0.0, 0.0, 0.0], 1.0);

        assert_eq!(out.exposure, 1.0);
        assert!(out.rate > 15.9);
    }

    /// The lean dies exactly where the rain does, because it is the same
    /// exposure doing both. A streak leaning under a roof it cannot reach is
    /// the visible symptom of two occlusion answers.
    #[test]
    fn the_lean_dies_with_the_rain() {
        let volume = roofed();
        let sky = Some(Sky { volume: &volume, offset: [0.0; 3] });
        let wind = Wind::default();

        let under = sample_rain(Some(&heavy()), &wind, sky, [32.0, 10.0, 32.0], 1.0);
        let open = sample_rain(Some(&heavy()), &wind, sky, [5.0, 10.0, 5.0], 1.0);

        assert_eq!(under.wind, [0.0, 0.0, 0.0], "sealed, but the air moves");
        assert_eq!(open.wind, wind.at([5.0, 10.0, 5.0], 1.0), "open air is P1's own field");
    }

    /// Rain is a scalar the whole scene shares, so a heavier scene rains harder
    /// everywhere rather than differently in different places.
    #[test]
    fn the_authored_intensity_reaches_the_rate() {
        let drizzle = Rain { intensity: 0.5, duration: None };
        let downpour = Rain { intensity: 50.0, duration: None };
        let at = [5.0, 10.0, 5.0];

        let light = sample_rain(Some(&drizzle), &Wind::default(), None, at, 0.0);
        let heavy = sample_rain(Some(&downpour), &Wind::default(), None, at, 0.0);

        assert!((light.rate - 0.5).abs() < 1e-4, "{light:?}");
        assert!((heavy.rate - 50.0).abs() < 1e-4, "{heavy:?}");
    }

    /// A shower with a `duration` stops. Nothing else in the engine can turn
    /// rain off, so without this the second half of "wetness accumulates and
    /// dries" is not reachable from a scene file at all.
    #[test]
    fn a_shower_with_a_duration_ends() {
        let shower = Rain { intensity: 8.0, duration: Some(60.0) };

        let during = sample_rain(Some(&shower), &Wind::default(), None, [0.0; 3], 59.0);
        let after = sample_rain(Some(&shower), &Wind::default(), None, [0.0; 3], 61.0);

        assert!(during.rate > 7.9, "{during:?}");
        assert_eq!(after.rate, 0.0, "the shower did not end: {after:?}");
    }

    /// **The exit criterion, at the state level: wetness accumulates and
    /// dries.** Every number below is `wetness` evaluated at a simulated
    /// second; nothing here is a screenshot.
    #[test]
    fn wetness_accumulates_and_dries() {
        let shower = Rain { intensity: 8.0, duration: Some(60.0) };
        let at = |t| wetness(Some(&shower), 1.0, t);

        assert_eq!(at(0.0), Wetness { film: 0.0, soak: 0.0 }, "wet before it rained");

        // Rising, both of them, all the way to the moment it stops.
        let (a, b, c) = (at(5.0), at(20.0), at(60.0));
        assert!(a.film < b.film && b.film < c.film, "film did not rise: {a:?} {b:?} {c:?}");
        assert!(a.soak < b.soak && b.soak < c.soak, "soak did not rise: {a:?} {b:?} {c:?}");
        assert!(c.film > 0.99, "an hour of rain and no film: {c:?}");

        // Falling, both of them, once it has.
        let (d, e) = (at(90.0), at(150.0));
        assert!(e.film < d.film && d.film < c.film, "film did not dry: {c:?} {d:?} {e:?}");
        assert!(e.soak < d.soak && d.soak < c.soak, "soak did not dry: {c:?} {d:?} {e:?}");
        assert!(e.film < 0.05, "the sheen outlasted 90 seconds: {e:?}");
    }

    /// **The two-rate dry-down, as an inequality rather than as a vibe.** The
    /// asymmetry is the whole effect: if both decayed together, drying would be
    /// a crossfade back to the dry material and would read as one. Ninety
    /// seconds after the rain the film is nearly gone and most of the stain is
    /// still there.
    #[test]
    fn the_specular_dries_faster_than_the_darkening() {
        let shower = Rain { intensity: 8.0, duration: Some(60.0) };
        let stopped = wetness(Some(&shower), 1.0, 60.0);
        let later = wetness(Some(&shower), 1.0, 150.0);

        let film_left = later.film / stopped.film;
        let soak_left = later.soak / stopped.soak;
        assert!(
            film_left < soak_left * 0.25,
            "the film is not outpacing the soak: {film_left} against {soak_left}"
        );
        assert!(soak_left > 0.6, "the darkening vanished with the sheen: {soak_left}");
    }

    /// **Accumulation is gated by exposure**, and by the same number the rate
    /// is — S3's march, not a second occlusion answer. A sheltered surface does
    /// not wet however long it rains.
    #[test]
    fn a_sheltered_surface_does_not_wet() {
        let rain = heavy();

        let open = wetness(Some(&rain), 1.0, 600.0);
        let sheltered = wetness(Some(&rain), 0.0, 600.0);

        assert!(open.film > 0.99 && open.soak > 0.99, "{open:?}");
        assert_eq!(sheltered, Wetness { film: 0.0, soak: 0.0 });
    }

    /// **A scene that authors no rain is bone dry, forever.** This is what
    /// keeps every reference image written before this phase byte-identical:
    /// the shader multiplies by these, and zero is exactly zero.
    #[test]
    fn a_scene_without_rain_never_wets() {
        let out = wetness(None, 1.0, 10_000.0);

        assert_eq!(out, Wetness { film: 0.0, soak: 0.0 });
    }

    /// Drizzle wets a surface completely, given time — it only takes longer to
    /// look like anything. A downpour does not make it wetter than wet.
    #[test]
    fn heavier_rain_does_not_wet_past_wet() {
        let drizzle = Rain { intensity: SOAKING_RATE, duration: None };
        let downpour = Rain { intensity: 50.0, duration: None };

        let a = wetness(Some(&drizzle), 1.0, 600.0);
        let b = wetness(Some(&downpour), 1.0, 600.0);

        assert!((a.film - b.film).abs() < 1e-5, "{a:?} against {b:?}");
        assert!(a.soak > 0.99, "drizzle never soaked it: {a:?}");
    }
}
