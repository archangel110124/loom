//! Ray-traced acoustics: what a place *sounds* like, worked out by casting.
//!
//! The usual way to do this is to hand-place reverb volumes — a box around the
//! cathedral, another around the corridor — and cross-fade between them. That
//! is authored data describing geometry that is already in the scene, and it
//! goes stale the moment anyone moves a wall. It is also exactly the kind of
//! bookkeeping an agent authoring a level will forget.
//!
//! So nothing here is authored. Every number is measured from the geometry
//! that is actually there, by casting rays through it:
//!
//! 1. **Occlusion** — rays from the listener toward the source. The fraction
//!    that arrive is how clearly it is heard.
//! 2. **Permeation** — for the ones that do not, how much *material* they had
//!    to cross. A curtain and a metre of concrete both block a ray; only one
//!    of them should block the sound.
//! 3. **Reverb** — rays fired outward from the listener. Where they land and
//!    whether they can see the listener again gives the delay (how big the
//!    room is) and the intensity (how much comes back).
//! 4. **Openness** — the fraction of those outward rays that hit nothing at
//!    all, which is the difference between a room and a field.
//!
//! **Deterministic, and that is not incidental.** Ray directions come from a
//! Fibonacci sphere rather than a random number generator: same scene, same
//! listener, same answer, every run and on every machine. That makes an
//! acoustic claim assertable the same way a position is — which is the only
//! reason to trust it (never-do #7, brief §7.5).
//!
//! It is also useful before a single sound plays. "Can the guard hear you from
//! there" is this function, and it is a gameplay question as much as an audio
//! one.

pub mod clip;
pub mod device;
pub mod mix;

pub use clip::{Clip, ClipError};
pub use device::{Audio, AudioError};
pub use mix::{Mixer, Voice};

use loom_physics::Physics;

/// What a listener hears from a source, and from the room around them.
///
/// Everything normalised except the delay, which is seconds, because that is
/// what a delay line wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acoustics {
    /// `0.0` heard clearly, `1.0` nothing gets through.
    pub occlusion: f32,
    /// `0.0` no material in the way, `1.0` as much as the model accounts for.
    ///
    /// Separate from `occlusion` because they are different physical facts and
    /// want different filters: occlusion is how much sound arrives, muffling
    /// is how much of its top end survived the trip. A doorway just out of
    /// sight and a brick wall occlude alike and sound nothing alike.
    pub muffling: f32,
    /// Seconds before the first reflection arrives — the size of the space.
    pub reverb_delay: f32,
    /// How much of the sound comes back, `0.0` to `1.0`.
    pub reverb_gain: f32,
    /// `0.0` sealed room, `1.0` open sky.
    pub openness: f32,
    /// Metres to the source, in a straight line.
    pub distance: f32,
}

/// How muffled everything is with the listener's head under the water.
///
/// **Not tuned by ear, because there are no ears in CI.** It is a low-pass
/// nearly as closed as the one a solid wall produces, which is the right end of
/// the range: the water surface reflects almost all airborne sound, and what
/// crosses it arrives with its top end gone. Left short of `1.0` so a source
/// under the water with you is still distinguishable from a wall — and so the
/// number reads as a strong effect rather than as a mute.
const UNDERWATER_MUFFLING: f32 = 0.9;

impl Acoustics {
    /// The same measurement, heard from under the water.
    ///
    /// **Submersion enters where every other acoustic fact does.** `muffling`
    /// already means "how much of its top end survived the trip" and already
    /// drives the low-pass in [`mix`] — so the underwater case is a bigger value
    /// for the quantity that exists, rather than a second filter beside it with
    /// its own state, its own cutoff and its own way of disagreeing.
    ///
    /// It only ever raises: a source behind a wall *and* under the water is not
    /// heard more clearly than one behind the wall alone.
    #[must_use]
    pub fn underwater(self) -> Self {
        Self {
            muffling: self.muffling.max(UNDERWATER_MUFFLING),
            ..self
        }
    }
}

/// How hard to listen.
///
/// Ray counts are the whole cost of this, and they trade smoothness for time:
/// too few and the numbers jump as the listener walks, which is audible as
/// the reverb stuttering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ears {
    /// Rays from the listener toward the source.
    pub occlusion_rays: u32,
    /// Rays fired outward to measure the room.
    pub room_rays: u32,
    /// How far to listen. Past this, a ray counts as having escaped.
    pub range: f32,
    /// Radius of the source, for occlusion sampling. A point source is either
    /// fully visible or fully hidden and pops as the listener steps past a
    /// doorway; a source with size fades.
    pub source_radius: f32,
    /// Metres of solid material that fully muffles a sound.
    pub muffling_depth: f32,
    /// Metres per second. Not a constant, because a scene is free to be on
    /// another planet.
    pub speed_of_sound: f32,
}

impl Default for Ears {
    fn default() -> Self {
        Self {
            // Enough that one ray is a few percent rather than a step you can
            // hear, and few enough to run every tick for several sources.
            occlusion_rays: 24,
            room_rays: 64,
            range: 60.0,
            source_radius: 0.4,
            muffling_depth: 1.5,
            speed_of_sound: 343.0,
        }
    }
}

impl Acoustics {
    /// What a source at `source` sounds like from `listener`.
    ///
    /// # The world it queries is the one the last `step` left
    ///
    /// Same caveat as every other query here: the tree these rays walk is
    /// built during `Physics::step`. Before the first one, every sound is in
    /// the open.
    #[must_use]
    pub fn solve(physics: &Physics, listener: [f32; 3], source: [f32; 3], ears: &Ears) -> Self {
        let to_source = sub(source, listener);
        let distance = length(to_source);

        let (occlusion, muffling) = occlusion(physics, listener, source, distance, ears);
        let (reverb_delay, reverb_gain, openness) = room(physics, listener, ears);

        Self {
            occlusion,
            muffling,
            reverb_delay,
            reverb_gain,
            openness,
            distance,
        }
    }
}

/// Mechanisms 1 and 2: how much arrives, and how much material it crossed.
fn occlusion(
    physics: &Physics,
    listener: [f32; 3],
    source: [f32; 3],
    distance: f32,
    ears: &Ears,
) -> (f32, f32) {
    if ears.occlusion_rays == 0 {
        return (0.0, 0.0);
    }
    // Standing on top of the source. Nothing can be between them.
    if distance < 1e-3 {
        return (0.0, 0.0);
    }

    let mut clear = 0_u32;
    let mut thickness_total = 0.0;
    for index in 0..ears.occlusion_rays {
        // Spread the aim points over the source's own size rather than all at
        // its centre, so a source half behind a doorway is half heard.
        let offset = scale(sphere_point(index, ears.occlusion_rays), ears.source_radius);
        let target = add(source, offset);
        let direction = sub(target, listener);
        let span = length(direction);
        if span < 1e-4 {
            clear += 1;
            continue;
        }

        // Pulled back so the source's own collider, if it has one, does not
        // count as the thing blocking it.
        match physics.raycast(listener, direction, span - 1e-2) {
            None => clear += 1,
            Some(_) => thickness_total += thickness(physics, listener, direction, span),
        }
    }

    let rays = f32::from(u16::try_from(ears.occlusion_rays).unwrap_or(u16::MAX));
    let occlusion = 1.0 - (f32::from(u16::try_from(clear).unwrap_or(u16::MAX)) / rays);

    // Averaged over the blocked rays only. Averaging over all of them would
    // make a mostly-open doorway report thin walls, when what it actually has
    // is few of them.
    let blocked = rays - f32::from(u16::try_from(clear).unwrap_or(u16::MAX));
    let muffling = if blocked < 1.0 {
        0.0
    } else {
        (thickness_total / blocked / ears.muffling_depth).clamp(0.0, 1.0)
    };

    (occlusion, muffling)
}

/// Metres of solid material along one blocked line.
///
/// Marches surface to surface: hit a face, step just inside, ask where that
/// solid ends, step just past, repeat. Bounded because a sealed scene could
/// otherwise walk forever, and four walls between two points is already an
/// unusual amount of building.
fn thickness(physics: &Physics, from: [f32; 3], direction: [f32; 3], span: f32) -> f32 {
    const SKIN: f32 = 1e-3;
    const MAX_WALLS: usize = 8;

    let unit = normalize(direction);
    let mut travelled = 0.0;
    let mut inside = 0.0;

    for _ in 0..MAX_WALLS {
        let remaining = span - travelled;
        if remaining <= SKIN {
            break;
        }
        let at = add(from, scale(unit, travelled));
        let Some(entry) = physics.raycast(at, unit, remaining) else {
            break;
        };

        // Just inside the face, then ask where this solid ends.
        travelled += entry.distance + SKIN;
        let at = add(from, scale(unit, travelled));
        let Some(exit) = physics.raycast_exit(at, unit, span - travelled) else {
            // No far side within reach: the rest of the line is material.
            inside += span - travelled;
            break;
        };
        inside += exit.distance;
        travelled += exit.distance + SKIN;
    }

    inside
}

/// Mechanisms 3 and 4: the size of the room, how much comes back, and whether
/// there is a room at all.
fn room(physics: &Physics, listener: [f32; 3], ears: &Ears) -> (f32, f32, f32) {
    if ears.room_rays == 0 {
        return (0.0, 0.0, 1.0);
    }

    let mut escaped = 0_u32;
    let mut returned = 0_u32;
    let mut path_total = 0.0;

    const SKIN: f32 = 1e-2;

    for index in 0..ears.room_rays {
        let direction = sphere_point(index, ears.room_rays);
        let Some(first) = physics.raycast(listener, direction, ears.range) else {
            // Nothing out there. This is what makes a field a field.
            escaped += 1;
            continue;
        };

        // **Bounce, then look back.** Asking whether the *first* surface can
        // see the listener is asking whether a straight line is straight: it
        // is the first thing the ray hit, so of course it can. That check was
        // tautological and the whole mechanism did nothing.
        //
        // A first-order reflection is two legs. The sound leaves the ear,
        // strikes a wall, goes wherever that wall sends it, and only reaches
        // the ear again if the *second* surface has a clear line back. That
        // is where a room with an alcove differs from a bare box.
        let bounce = reflect(direction, first.normal);
        let from = add(first.point, scale(first.normal, SKIN));
        let Some(second) = physics.raycast(from, bounce, ears.range - first.distance) else {
            // The reflection left the space. It is not coming back.
            continue;
        };

        let home = sub(listener, second.point);
        let back = length(home);
        if physics.line_of_sight(second.point, listener) {
            returned += 1;
            // Every leg of the path the sound actually travelled.
            path_total += first.distance + second.distance + back;
        }
    }

    let rays = f32::from(u16::try_from(ears.room_rays).unwrap_or(u16::MAX));
    let openness = f32::from(u16::try_from(escaped).unwrap_or(u16::MAX)) / rays;
    let back = f32::from(u16::try_from(returned).unwrap_or(u16::MAX));
    let reverb_gain = back / rays;
    let reverb_delay = if back < 1.0 {
        0.0
    } else {
        path_total / back / ears.speed_of_sound
    };

    (reverb_delay, reverb_gain, openness)
}

/// The `index`-th of `count` points spread evenly over a unit sphere.
///
/// A Fibonacci spiral, not a random sample. Two runs of the same scene must
/// agree exactly — an acoustic value that jitters between runs cannot be
/// asserted on, and an assertion that is flaky trains everyone to ignore it
/// (brief §7.5). It is also better distributed than random for small counts,
/// which is the case that matters here.
fn sphere_point(index: u32, count: u32) -> [f32; 3] {
    // The golden angle. Successive points land as far from each other as a
    // spiral can manage.
    const GOLDEN: f32 = 2.399_963_2;

    let count = count.max(1) as f32;
    let index = index as f32;
    // Evenly spaced in *height*, which is what makes the areas equal: spacing
    // the polar angle evenly instead bunches points at the poles.
    let y = 1.0 - 2.0 * (index + 0.5) / count;
    let radius = (1.0 - y * y).max(0.0).sqrt();
    let theta = GOLDEN * index;
    [theta.cos() * radius, y, theta.sin() * radius]
}

/// A direction bounced off a surface.
fn reflect(direction: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    let d = normalize(direction);
    let dot = d[0] * normal[0] + d[1] * normal[1] + d[2] * normal[2];
    sub(d, scale(normal, 2.0 * dot))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let len = length(a);
    if len < 1e-6 {
        return [0.0, 0.0, 1.0];
    }
    scale(a, 1.0 / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// Open ground and nothing else. Every acoustic number has a value here
    /// and they are all the "outdoors" end of their range.
    fn field() -> Physics {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -1.0, 0.0], FLAT, [200.0, 1.0, 200.0]);
        physics.step();
        physics
    }

    /// A sealed box, 8 x 4 x 8 inside.
    fn room() -> Physics {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -0.5, 0.0], FLAT, [4.5, 0.5, 4.5]);
        physics.add_static_box([0.0, 4.5, 0.0], FLAT, [4.5, 0.5, 4.5]);
        physics.add_static_box([-4.5, 2.0, 0.0], FLAT, [0.5, 2.5, 4.5]);
        physics.add_static_box([4.5, 2.0, 0.0], FLAT, [0.5, 2.5, 4.5]);
        physics.add_static_box([0.0, 2.0, -4.5], FLAT, [4.5, 2.5, 0.5]);
        physics.add_static_box([0.0, 2.0, 4.5], FLAT, [4.5, 2.5, 0.5]);
        physics.step();
        physics
    }

    #[test]
    fn a_sound_in_the_open_is_heard_clearly() {
        let physics = field();

        let heard = Acoustics::solve(&physics, [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &Ears::default());

        assert!(heard.occlusion < 0.05, "occluded in a field: {heard:?}");
        assert!(heard.muffling < 0.05, "muffled in a field: {heard:?}");
        assert!((heard.distance - 6.0).abs() < 1e-3);
    }

    /// **Mechanism 1.** A wall between listener and source blocks the rays,
    /// and the fraction blocked is the occlusion.
    #[test]
    fn a_wall_between_them_occludes() {
        let mut physics = field();
        physics.add_static_box([3.0, 2.0, 0.0], FLAT, [0.3, 2.0, 8.0]);
        physics.step();

        let heard = Acoustics::solve(&physics, [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &Ears::default());

        assert!(heard.occlusion > 0.9, "the wall did nothing: {heard:?}");
    }

    /// Partial cover reads as partial. A source at the edge of a wall must not
    /// be all-or-nothing, or the sound pops as the listener steps sideways.
    #[test]
    fn a_source_at_the_edge_of_cover_is_partly_heard() {
        let mut physics = field();
        // A wall that ends level with the source, so half the sampled points
        // on it are visible and half are not.
        physics.add_static_box([3.0, 2.0, -4.0], FLAT, [0.3, 2.0, 4.0]);
        physics.step();

        let ears = Ears { source_radius: 1.2, ..Ears::default() };
        let heard = Acoustics::solve(&physics, [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &ears);

        assert!(
            heard.occlusion > 0.05 && heard.occlusion < 0.95,
            "should be partial, was {}",
            heard.occlusion
        );
    }

    /// **Mechanism 2.** A thick wall and a thin one both stop every ray, so
    /// occlusion cannot tell them apart. Muffling is the number that can.
    #[test]
    fn a_thick_wall_muffles_more_than_a_thin_one() {
        let thin = {
            let mut physics = field();
            physics.add_static_box([3.0, 2.0, 0.0], FLAT, [0.05, 2.0, 8.0]);
            physics.step();
            Acoustics::solve(&physics, [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &Ears::default())
        };
        let thick = {
            let mut physics = field();
            physics.add_static_box([3.0, 2.0, 0.0], FLAT, [1.0, 2.0, 8.0]);
            physics.step();
            Acoustics::solve(&physics, [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &Ears::default())
        };

        assert!(
            (thin.occlusion - thick.occlusion).abs() < 0.1,
            "both are fully blocked: {} vs {}",
            thin.occlusion,
            thick.occlusion
        );
        assert!(
            thick.muffling > thin.muffling + 0.3,
            "thickness did not register: thin {} thick {}",
            thin.muffling,
            thick.muffling
        );
    }

    /// **Mechanism 4.** The difference between a room and a field, measured
    /// rather than authored as a volume somebody has to remember to place.
    #[test]
    fn a_room_is_enclosed_and_a_field_is_not() {
        let indoors = Acoustics::solve(&room(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());
        let outdoors =
            Acoustics::solve(&field(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());

        assert!(indoors.openness < 0.05, "the room leaked: {indoors:?}");
        assert!(outdoors.openness > 0.4, "the field was closed in: {outdoors:?}");
    }

    /// **Mechanism 3.** Reverb comes back indoors and does not outdoors,
    /// because outdoors there is nothing to come back off.
    #[test]
    fn a_room_echoes_and_a_field_does_not() {
        let indoors = Acoustics::solve(&room(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());
        let outdoors =
            Acoustics::solve(&field(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());

        assert!(indoors.reverb_gain > 0.8, "no echo indoors: {indoors:?}");
        assert!(
            outdoors.reverb_gain < indoors.reverb_gain * 0.7,
            "a field echoed like a room: {} vs {}",
            outdoors.reverb_gain,
            indoors.reverb_gain
        );
    }

    /// **A reflection has to find its way back.** The first surface a ray
    /// strikes can always see the listener — it is the first thing on a
    /// straight line — so checking *that* was asking whether a straight line
    /// is straight. It was tautological, and the whole mechanism did nothing;
    /// deleting it changed no number in this suite. A first-order reflection
    /// is two legs, and it is the second surface that can be hidden.
    ///
    /// Compared against the same room without the obstruction rather than
    /// against a threshold: both rooms are sealed and both are reverberant,
    /// and what the visibility check buys is that the obstructed one is
    /// measurably less so. Everything here is deterministic, so a difference
    /// this small is a fact rather than noise.
    #[test]
    fn a_reflection_that_cannot_get_back_does_not_count() {
        let at = [-3.5, 1.5, 0.0];
        let source = [-3.8, 1.5, 0.0];
        let plain = Acoustics::solve(&room(), at, source, &Ears::default());

        let pillared = {
            let mut physics = room();
            // Rays passing either side of the pillar strike the far wall and
            // bounce to points the pillar hides from the listener.
            physics.add_static_box([0.0, 2.0, 0.0], FLAT, [1.6, 2.5, 1.6]);
            physics.step();
            Acoustics::solve(&physics, at, source, &Ears::default())
        };

        assert!(plain.openness < 0.05 && pillared.openness < 0.05, "both sealed");
        assert!(
            pillared.reverb_gain < plain.reverb_gain - 0.02,
            "the pillar hid nothing: plain {} vs pillared {}",
            plain.reverb_gain,
            pillared.reverb_gain
        );
    }

    /// The delay is the size of the space. A big hall must not sound like a
    /// cupboard, and this is the number that says so.
    #[test]
    fn a_bigger_room_has_a_longer_delay() {
        let hall = {
            let mut physics = Physics::new(1.0 / 60.0);
            physics.add_static_box([0.0, -0.5, 0.0], FLAT, [30.0, 0.5, 30.0]);
            physics.add_static_box([0.0, 20.5, 0.0], FLAT, [30.0, 0.5, 30.0]);
            physics.add_static_box([-30.0, 10.0, 0.0], FLAT, [0.5, 10.5, 30.0]);
            physics.add_static_box([30.0, 10.0, 0.0], FLAT, [0.5, 10.5, 30.0]);
            physics.add_static_box([0.0, 10.0, -30.0], FLAT, [30.0, 10.5, 0.5]);
            physics.add_static_box([0.0, 10.0, 30.0], FLAT, [30.0, 10.5, 0.5]);
            physics.step();
            Acoustics::solve(&physics, [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default())
        };
        let cupboard =
            Acoustics::solve(&room(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());

        assert!(
            hall.reverb_delay > cupboard.reverb_delay * 2.0,
            "hall {} vs cupboard {}",
            hall.reverb_delay,
            cupboard.reverb_delay
        );
    }

    /// **The property that makes any of this assertable.** Ray directions come
    /// from a spiral, not a generator: the same scene must give the same
    /// answer every time, or an acoustic assertion is a coin toss.
    #[test]
    fn the_same_scene_sounds_the_same_twice() {
        let physics = room();
        let ears = Ears::default();

        let first = Acoustics::solve(&physics, [1.0, 1.5, 2.0], [-2.0, 1.0, 1.0], &ears);
        let second = Acoustics::solve(&physics, [1.0, 1.5, 2.0], [-2.0, 1.0, 1.0], &ears);

        assert_eq!(first, second, "acoustics must replay identically");
    }

    /// The spiral has to cover the sphere. Bunched at a pole it would measure
    /// the ceiling and call it the room.
    #[test]
    fn the_probe_directions_cover_the_whole_sphere() {
        let count = 64;
        let points: Vec<[f32; 3]> = (0..count).map(|i| sphere_point(i, count)).collect();

        for p in &points {
            assert!((length(*p) - 1.0).abs() < 1e-3, "not a unit vector: {p:?}");
        }
        // The mean of an even covering is near the origin. A hemisphere's
        // would sit half a radius out.
        let mut mean = [0.0_f32; 3];
        for p in &points {
            mean = add(mean, scale(*p, 1.0 / count as f32));
        }
        assert!(length(mean) < 0.05, "lopsided: {mean:?}");
    }

    /// **Going under muffles what you hear, and nothing else changes.** The
    /// distance and the room are still whatever they were; it is the top end
    /// that stops arriving.
    #[test]
    fn a_submerged_listener_hears_the_same_sound_muffled() {
        let heard = Acoustics::solve(&field(), [0.0, 1.5, 0.0], [6.0, 1.5, 0.0], &Ears::default());
        let under = heard.underwater();

        assert!(heard.muffling < 0.1, "the fixture starts unmuffled: {heard:?}");
        assert!(under.muffling > 0.8, "going under did not muffle: {under:?}");
        assert_eq!(under.distance, heard.distance, "distance is not a water fact");
        assert_eq!(under.reverb_delay, heard.reverb_delay);
        assert_eq!(under.occlusion, heard.occlusion, "muffling is not a volume knob");
    }

    /// A wall is already worse than the water. Surfacing must not be how you
    /// hear through it.
    #[test]
    fn water_never_makes_a_blocked_sound_clearer() {
        let mut walled = Acoustics::solve(&room(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &Ears::default());
        walled.muffling = 0.97;

        assert!(walled.underwater().muffling >= walled.muffling, "the water un-muffled a wall");
    }

    /// Asking for no rays is a degenerate request, not a crash and not a
    /// division by zero producing NaN that then poisons a mixer.
    #[test]
    fn zero_rays_is_answered_rather_than_divided_by() {
        let ears = Ears { occlusion_rays: 0, room_rays: 0, ..Ears::default() };

        let heard = Acoustics::solve(&room(), [0.0, 1.5, 0.0], [1.0, 1.5, 0.0], &ears);

        assert!(
            heard.occlusion.is_finite()
                && heard.muffling.is_finite()
                && heard.reverb_delay.is_finite()
                && heard.reverb_gain.is_finite()
                && heard.openness.is_finite(),
            "{heard:?}"
        );
    }
}
