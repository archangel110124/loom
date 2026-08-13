//! Turning `ParticleEmitter` components into particles the renderer can draw.
//!
//! **Stepped, not sampled.** A particle system has no meaningful state at tick
//! zero — every particle is newly born at the emitter, which draws as a dot.
//! So the plume is simulated forward before it is rendered, either for the
//! ticks `--sim` asked for or, failing that, for long enough to reach the
//! steady population it will hold from then on. A still render of a smoke
//! plume should look like smoke, not like the instant somebody lit it.

use loom_ecs::World;
use loom_render::ParticleInstance;

/// The simulation's fixed timestep. Matches physics, and is a constant rather
/// than a measured frame time for the reason never-do #8 exists.
const DT: f32 = 1.0 / 60.0;

/// Read an emitter component into the simulation's parameter struct.
fn parse(component: &serde_json::Value) -> (loom_particles::Emitter, Visual) {
    let f = |name: &str, fallback: f32| {
        #[allow(clippy::cast_possible_truncation)]
        component
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .map_or(fallback, |v| v as f32)
    };
    let v = |name: &str, fallback: [f32; 3]| {
        let Some(list) = component.get(name).and_then(serde_json::Value::as_array) else {
            return fallback;
        };
        let mut out = fallback;
        for (slot, value) in out.iter_mut().zip(list) {
            if let Some(x) = value.as_f64() {
                #[allow(clippy::cast_possible_truncation)]
                {
                    *slot = x as f32;
                }
            }
        }
        out
    };
    let pair = |name: &str, fallback: [f32; 2]| {
        let got = v(name, [fallback[0], fallback[1], 0.0]);
        [got[0], got[1]]
    };

    let defaults = loom_particles::Emitter::default();
    (
        loom_particles::Emitter {
            rate: f("rate", defaults.rate),
            lifetime: f("lifetime", defaults.lifetime),
            lifetime_jitter: f("lifetime_jitter", defaults.lifetime_jitter),
            speed: f("speed", defaults.speed),
            spread_degrees: f("spread_degrees", defaults.spread_degrees),
            radius: f("radius", defaults.radius),
            gravity: f("gravity", defaults.gravity),
            drag: f("drag", defaults.drag),
            turbulence: f("turbulence", defaults.turbulence),
            turbulence_scale: f("turbulence_scale", defaults.turbulence_scale),
            wind_response: f("wind_response", defaults.wind_response),
            #[allow(clippy::cast_possible_truncation)]
            burst: component
                .get("burst")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32,
            delay: f("delay", defaults.delay),
            duration: f("duration", defaults.duration),
            seed: component
                .get("seed")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1),
        },
        Visual {
            size: pair("size", [0.6, 3.2]),
            color_start: v("color_start", [0.32, 0.30, 0.29]),
            color_end: v("color_end", [0.62, 0.62, 0.64]),
            alpha: pair("alpha", [0.55, 0.0]),
            additive: component
                .get("additive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        },
    )
}

/// The part of an emitter that only the renderer cares about.
struct Visual {
    size: [f32; 2],
    color_start: [f32; 3],
    color_end: [f32; 3],
    alpha: [f32; 2],
    additive: bool,
}

/// One emitter, kept alive across frames.
struct Live {
    system: loom_particles::System,
    emitter: loom_particles::Emitter,
    visual: Visual,
    origin: [f32; 3],
}

/// Every emitter in a scene, simulated once and then advanced.
///
/// **This exists because the obvious thing is quadratic.** The offscreen
/// renderer computes a plume by simulating from tick zero, which is right
/// there: it runs once and the result is a pure function of the scene and the
/// tick count, so `--sim 300` means exactly one thing.
///
/// Doing that *per frame* in the viewer costs the whole history again every
/// frame. With a six-second lifetime that is 720 ticks of several hundred
/// particles, sixty times a second — it measured 9.5 ms per frame on a scene
/// with three props on a box, against 2.2 ms for a 67-million-voxel terrain.
/// A scene with nothing in it was four times more expensive than one with
/// 778,000 triangles.
///
/// So the viewer keeps the state and steps it forward by one tick per frame.
/// The consequence is honest and worth stating: the plume in the window now
/// depends on how many frames have been drawn, so it is a *live* view rather
/// than a reproducible one. `loom render --sim N` is unchanged and remains
/// exact, and that is the path assertions are made against.
pub(crate) struct Plumes {
    live: Vec<Live>,
    instances: Vec<ParticleInstance>,
    /// The air these plumes sit in. Sampled per particle, per step.
    wind: loom_field::wind::Wind,
    /// Seconds simulated, from the tick count — never a clock (never-do #8).
    elapsed: f32,
}

impl Plumes {
    /// Build from a world and warm every plume to its settled population.
    pub(crate) fn new(world: &World, wind: loom_field::wind::Wind) -> Self {
        let mut plumes =
            Self { live: Vec::new(), instances: Vec::new(), wind, elapsed: 0.0 };
        for entity in world.entities() {
            let (Some(component), Some(global)) = (world.emitter(*entity), world.global_transform(*entity))
            else {
                continue;
            };
            // A dormant explosion is a description, not an event: its
            // emitters play where one is triggered, never where it sits. A
            // splash under the water is the same shape of thing.
            if in_dormant_blast(world, *entity) || in_water(world, *entity) {
                continue;
            }
            let (emitter, visual) = parse(component);
            let origin = [global.matrix[12], global.matrix[13], global.matrix[14]];
            let mut system = loom_particles::System::new(emitter.seed);
            // Warm up once, here, rather than every frame. An emitter opened
            // cold is a single dot at its origin, which reads as broken.
            //
            // **Not a one-shot, though.** Warming an explosion runs its burst
            // and lets it die before the window ever draws a frame, so opening
            // the scene would show the aftermath of a blast nobody saw. A
            // steady plume wants to be caught mid-flow; an event wants to be
            // caught at its beginning.
            let one_shot = emitter.burst > 0 || emitter.duration > 0.0 || emitter.delay > 0.0;
            if !one_shot {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let warm = ((emitter.lifetime * 2.0) / DT).ceil() as u32;
                // Warmed *in the wind*, so a plume opens already bent
                // downwind rather than standing straight up and then
                // toppling over in the first second anyone watches.
                for tick in 0..warm {
                    #[allow(clippy::cast_precision_loss)]
                    let t = tick as f32 * DT;
                    let wind = &plumes.wind;
                    system.step_in_wind(DT, &emitter, origin, &|at| wind.at(at, t));
                }
            }
            plumes.live.push(Live { system, emitter, visual, origin });
        }
        plumes.rebuild_instances();
        plumes
    }

    /// Set off the scene's dormant explosion at a point.
    ///
    /// The systems are added live and then age out on their own, so a scene
    /// can be shot up for as long as the human likes without anything to
    /// clean up: a burst with a finite lifetime empties itself.
    pub(crate) fn detonate(&mut self, world: &World, at: [f32; 3], seed_salt: u64) {
        self.play(blast_template(world), at, seed_salt);
    }

    /// Throw up the scene's splash where something went into the water.
    ///
    /// Salted by *where* as well as *when*, so two things going in on the same
    /// tick throw different spray. The salt is the same one the headless path
    /// uses, or the window and `loom render --sim` would draw two splashes.
    pub(crate) fn splash(&mut self, world: &World, at: [f32; 3], tick: u64) {
        self.play(splash_template(world), at, salt(tick, at));
    }

    fn play(&mut self, template: Vec<(loom_particles::Emitter, Visual)>, at: [f32; 3], seed_salt: u64) {
        for (emitter, visual) in template {
            self.live.push(Live {
                system: loom_particles::System::new(emitter.seed ^ seed_salt),
                emitter,
                visual,
                origin: at,
            });
        }
    }

    /// Advance every plume by `ticks` steps.
    pub(crate) fn advance(&mut self, ticks: u32) {
        if self.live.is_empty() || ticks == 0 {
            return;
        }
        let wind = &self.wind;
        for live in &mut self.live {
            let mut t = self.elapsed;
            for _ in 0..ticks {
                live.system.step_in_wind(DT, &live.emitter, live.origin, &|at| wind.at(at, t));
                t += DT;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.elapsed += ticks as f32 * DT;
        }
        self.rebuild_instances();
    }

    pub(crate) fn instances(&self) -> &[ParticleInstance] {
        &self.instances
    }

    fn rebuild_instances(&mut self) {
        self.instances.clear();
        for live in &self.live {
            for p in live.system.particles() {
                self.instances.push(instance(p, &live.visual));
            }
        }
    }
}

/// One particle's drawable form: size and colour interpolated over its life.
fn instance(p: &loom_particles::Particle, visual: &Visual) -> ParticleInstance {
    let t = p.fraction();
    // Smoke expands and pales as it cools and mixes with air; a plume whose
    // particles keep their birth size and colour reads as a stream of blobs.
    let size = visual.size[0] + (visual.size[1] - visual.size[0]) * t;
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    // Fade in as well as out. Particles that appear at full opacity pop, and
    // the pop is at the emitter, where the eye already is.
    let fade = (t * 8.0).min(1.0);
    // Negative radius marks an additive particle. See `ParticleInstance` in
    // scene.slang: a radius is never legitimately negative, so its sign
    // carries the flag and the instance stays 32 bytes.
    let radius = if visual.additive { -size * 0.5 } else { size * 0.5 };
    ParticleInstance {
        position: [p.position[0], p.position[1], p.position[2], radius],
        color: [
            lerp(visual.color_start[0], visual.color_end[0]),
            lerp(visual.color_start[1], visual.color_end[1]),
            lerp(visual.color_start[2], visual.color_end[2]),
            lerp(visual.alpha[0], visual.alpha[1]) * fade,
        ],
    }
}

/// Simulate every emitter in the world and return what to draw.
///
/// `ticks` of `None` means "long enough to look settled": a plume reaches its
/// steady population after one particle lifetime, so twice that is comfortably
/// past the transient.
#[must_use]
/// Whether this node sits inside an explosion that has not been set off.
///
/// A dormant `Blast` marks a prefab: the emitters under it describe what an
/// explosion of that kind *looks like*, and must not play where the prefab
/// happens to sit. They play where one is triggered.
///
/// Bounded rather than `while let`, like every other ancestor walk here: a
/// malformed hierarchy with a cycle must not hang a frame.
pub(crate) fn in_dormant_blast(world: &World, entity: loom_ecs::Entity) -> bool {
    let mut current = Some(entity);
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if let Some(blast) = world.blast(node)
            && !blast
                .get("armed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        {
            return true;
        }
        current = world.parent(node);
    }
    false
}

/// Whether this node sits under the scene's `WaterBody`.
///
/// **A `ParticleEmitter` under the water is the scene's splash**, and a splash
/// is an event: it plays where something went in, never where the water node
/// happens to sit. Exactly the arrangement a dormant `Blast` already has, for
/// exactly the same reason — an explosion prefab that played at its own
/// position would park a permanent fireball in every scene carrying a weapon,
/// and a splash template that played at its own position would leave one
/// permanent spout in the middle of the sea.
///
/// It is where the water is rather than a flag on the emitter because the
/// question an author is answering is "what does *this water* splash like",
/// and the hierarchy already says which water.
///
/// Bounded rather than `while let`, like every other ancestor walk here.
pub(crate) fn in_water(world: &World, entity: loom_ecs::Entity) -> bool {
    let mut current = Some(entity);
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if world.is_water(node) {
            return true;
        }
        current = world.parent(node);
    }
    false
}

/// The emitters of the scene's dormant explosion, if it has one.
///
/// One template per scene for now. A second would need the script to say
/// which, and nothing yet has two kinds of explosion in it.
fn blast_template(world: &World) -> Vec<(loom_particles::Emitter, Visual)> {
    world
        .entities()
        .iter()
        .filter(|e| world.emitter(**e).is_some() && in_dormant_blast(world, **e))
        .filter_map(|e| world.emitter(*e).map(parse))
        .collect()
}

/// The emitters of the scene's splash, if the water authors one.
fn splash_template(world: &World) -> Vec<(loom_particles::Emitter, Visual)> {
    world
        .entities()
        .iter()
        .filter(|e| world.emitter(**e).is_some() && in_water(world, **e))
        .filter_map(|e| world.emitter(*e).map(parse))
        .collect()
}

pub(crate) fn simulate(
    world: &World,
    wind: &loom_field::wind::Wind,
    ticks: Option<u32>,
    fired: &[(u64, [f32; 3])],
    splashed: &[(u64, [f32; 3])],
) -> Vec<ParticleInstance> {
    let mut out = Vec::new();

    for entity in world.entities() {
        let Some(component) = world.emitter(*entity) else {
            continue;
        };
        let Some(global) = world.global_transform(*entity) else {
            continue;
        };
        // A prefab describes an explosion; it is not one. Nor is the water's
        // splash a fountain in the middle of the sea.
        if in_dormant_blast(world, *entity) || in_water(world, *entity) {
            continue;
        }
        let (emitter, visual) = parse(component);
        // Column 3 of the model matrix is its translation.
        let origin = [global.matrix[12], global.matrix[13], global.matrix[14]];

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let steps = ticks.unwrap_or_else(|| ((emitter.lifetime * 2.0) / DT).ceil() as u32);
        let mut system = loom_particles::System::new(emitter.seed);
        for tick in 0..steps {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * DT;
            system.step_in_wind(DT, &emitter, origin, &|at| wind.at(at, t));
        }

        for p in system.particles() {
            out.push(instance(p, &visual));
        }
    }

    // Explosions a script set off during the run, and splashes the water
    // raised, replayed from the tick each happened on. Deterministic for the
    // same reason everything else here is: the tick is data, not a clock
    // reading, so `--sim N` means one thing.
    for (template, events, by_place) in [
        // Blasts are salted by tick alone, which is what they have always been
        // salted by. Two set off on the same tick therefore look alike — a real
        // if minor limitation, left as it is because changing it would move
        // every committed reference image of a scene that fires one, which is a
        // bigger claim than this step is making.
        (blast_template(world), fired, false),
        (splash_template(world), splashed, true),
    ] {
        if template.is_empty() {
            continue;
        }
        for (at_tick, at) in events {
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = ticks
                .unwrap_or(0)
                .saturating_sub(u32::try_from(*at_tick).unwrap_or(u32::MAX));
            let seed_salt = if by_place { salt(*at_tick, *at) } else { *at_tick };
            for (emitter, visual) in &template {
                let mut system = loom_particles::System::new(emitter.seed ^ seed_salt);
                for _ in 0..elapsed {
                    system.step_in_wind(DT, emitter, *at, &|p| wind.at(p, 0.0));
                }
                for p in system.particles() {
                    out.push(instance(p, visual));
                }
            }
        }
    }

    out
}

/// The seed offset for one played template, from the tick and place it played.
///
/// **Every bit of this is simulation state**, which is what makes a splash as
/// reproducible as the crate that caused it: the tick is counted, never read
/// off a clock, and the position came out of the fixed step. Two bodies going
/// in on the same tick get different spray because their positions differ, and
/// the same body in two runs of the same scene gets the same spray because
/// nothing here is drawn from anywhere else.
///
/// The position goes in by its bits rather than by a hash of them: a float's
/// bit pattern is already well spread across the low bits, and this is a seed
/// offset rather than a hash table key.
fn salt(tick: u64, at: [f32; 3]) -> u64 {
    tick ^ (u64::from(at[0].to_bits()) << 32) ^ u64::from(at[2].to_bits())
}

#[cfg(test)]
mod tests {
    /// Still air, so a test measures the emitter rather than the weather.
    fn calm() -> loom_field::wind::Wind {
        loom_field::wind::Wind::new(0.0, 0.0, 0.0, 0.0, 1.0)
    }

    use super::*;

    fn range() -> World {
        let source = std::fs::read_to_string("../../assets/test/turret_range.loom").expect("fixture");
        World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"))
    }

    /// **A prefab is a description, not an event.** The scene's dormant
    /// explosion sits at a real position with real emitters on it; if those
    /// played there, every scene carrying a weapon's explosion would have a
    /// permanent fireball parked somewhere in it.
    #[test]
    fn a_dormant_explosion_does_not_play_where_it_sits() {
        let quiet = simulate(&range(), &calm(), Some(60), &[], &[]);

        assert!(
            quiet.is_empty(),
            "{} particles from an explosion nobody set off",
            quiet.len()
        );
    }

    /// And the other half: set off, it plays at the point it was set off at,
    /// not at the prefab's parked position.
    #[test]
    fn a_triggered_explosion_plays_where_it_was_set_off() {
        let world = range();
        let at = [2.0, 1.0, -7.0];
        let fired = [(10, at)];

        let out = simulate(&world, &calm(), Some(30), &fired, &[]);

        assert!(!out.is_empty(), "the explosion produced nothing");
        // The prefab is parked at x = -14; every particle should be near the
        // detonation instead. Generous, because a burst expands fast.
        for p in &out {
            assert!(
                (p.position[0] - at[0]).abs() < 8.0,
                "particle at {:?} is nowhere near the blast at {at:?}",
                p.position
            );
        }
    }

    fn explosion() -> World {
        let source = std::fs::read_to_string("../../assets/test/explosion.loom").expect("fixture");
        World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"))
    }

    /// **Opening a scene must not set off its explosions.** A one-shot emitter
    /// is an event; building the viewer's particle state is not that event.
    /// The editor showed the blast the moment the file was opened, before Play
    /// had been pressed.
    #[test]
    fn building_a_plume_does_not_fire_a_one_shot() {
        let plumes = Plumes::new(&explosion(), calm());

        assert!(
            plumes.instances().is_empty(),
            "{} particles from a scene nobody has played",
            plumes.instances().len()
        );
    }

    /// And the drag case, which is the same bug seen twice: the editor drops
    /// its particle state whenever the scene changes, so every frame of a
    /// gizmo drag rebuilt it. If building is idempotent, a drag cannot
    /// re-detonate anything.
    #[test]
    fn rebuilding_a_plume_from_the_same_scene_gives_the_same_thing() {
        let world = explosion();

        let first = Plumes::new(&world, calm());
        let second = Plumes::new(&world, calm());

        assert_eq!(
            first.instances().len(),
            second.instances().len(),
            "a rebuild must not be an event"
        );
    }

    /// A continuous emitter still previews, warmed to its settled population,
    /// because placing a chimney needs to show where its smoke goes. It is
    /// the *advancing* that belongs to Play, not the preview.
    #[test]
    fn a_continuous_emitter_still_previews_without_playing() {
        let source = std::fs::read_to_string("../../assets/test/smoke.loom").expect("fixture");
        let world = World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"));

        let plumes = Plumes::new(&world, calm());

        assert!(!plumes.instances().is_empty(), "a chimney should preview");
    }

    fn sea() -> World {
        let source = std::fs::read_to_string("../../assets/test/splash.loom").expect("fixture");
        World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"))
    }

    /// **The water's splash is a description, not a fountain.** The emitter
    /// under the `WaterBody` sits at the water node's position; if it played
    /// there, every scene with water in it would have one permanent spout in
    /// the middle of the sea.
    #[test]
    fn the_water_s_splash_does_not_play_where_it_sits() {
        let quiet = simulate(&sea(), &calm(), Some(60), &[], &[]);

        assert!(
            quiet.is_empty(),
            "{} particles from a splash nobody made",
            quiet.len()
        );
        // And the same through the viewer's path, which builds its own state.
        assert!(Plumes::new(&sea(), calm()).instances().is_empty());
    }

    /// And the other half: it plays where the thing went in.
    #[test]
    fn a_splash_plays_where_something_entered_the_water() {
        let at = [3.0, 0.4, -6.0];

        let out = simulate(&sea(), &calm(), Some(20), &[], &[(10, at)]);

        assert!(!out.is_empty(), "entering the water produced no splash");
        for p in &out {
            assert!(
                (p.position[0] - at[0]).abs() < 4.0 && (p.position[2] - at[2]).abs() < 4.0,
                "droplet at {:?} is nowhere near the entry at {at:?}",
                p.position
            );
        }
    }

    /// **Two things going in on the same tick throw different spray**, because
    /// the seed is salted by where as well as when. Without the position in it
    /// they would be the same burst drawn twice, which reads as one splash
    /// mirrored — and the fix must not be a clock, or the picture stops being
    /// reproducible.
    #[test]
    fn two_entries_on_one_tick_do_not_produce_the_same_splash() {
        let world = sea();
        // **One step after the burst**, so every droplet is still exactly where
        // it was spawned. Any later and the turbulence — which is sampled by
        // world position — would separate the two splashes on its own, and the
        // test would pass with the seed ignored entirely.
        let spray = |at: [f32; 3]| {
            simulate(&world, &calm(), Some(11), &[], &[(10, at)])
                .iter()
                .map(|p| {
                    [
                        p.position[0] - at[0],
                        p.position[1] - at[1],
                        p.position[2] - at[2],
                    ]
                })
                .collect::<Vec<[f32; 3]>>()
        };

        let left = spray([-3.0, 0.0, -6.0]);
        let right = spray([3.0, 0.0, -6.0]);
        assert!(!left.is_empty(), "the burst produced nothing");
        assert_eq!(left.len(), right.len(), "the same template, so the same count");
        assert_ne!(left, right, "both splashes are the same burst in two places");

        // Deterministic all the same: the same entry, twice, is the same spray.
        assert_eq!(left, spray([-3.0, 0.0, -6.0]));
    }

    /// Nothing fired means nothing drawn, even in a scene that has a template.
    #[test]
    fn no_shot_means_no_fireball() {
        let world = range();

        let none = simulate(&world, &calm(), Some(30), &[], &[]);
        let one = simulate(
            &world,
            &calm(),
            Some(30),
            &[(5, [0.0, 1.0, 0.0])],
            &[],
        );

        assert!(none.len() < one.len(), "firing must add particles");
    }
}
