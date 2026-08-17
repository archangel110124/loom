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
            flame: component
                .get("flame")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        },
    )
}

/// Whether this emitter is simulated on the device (ADR 0047).
///
/// Read on its own as well as through [`parse`], because the *first* thing
/// every caller here does with a GPU emitter is skip it: it has no CPU
/// particles to step, and stepping it anyway would draw the plume twice.
fn is_gpu(component: &serde_json::Value) -> bool {
    component
        .get("gpu")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The scene's GPU emitter, if it authors one.
///
/// **At most one**, which `loom_scene` refuses at load rather than leaving to
/// this `find` to silently decide — the renderer owns exactly one pool, and a
/// second emitter that quietly did not draw is the failure mode no gate in this
/// project can see.
///
/// A dormant blast's emitters and the water's splash template are skipped for
/// the reason they are skipped everywhere else: they describe an event and play
/// where one is triggered, never where they sit.
pub(crate) fn gpu_emitter(world: &World) -> Option<loom_render::GpuEmitter> {
    let entity = *world.entities().iter().find(|e| {
        world.emitter(**e).is_some_and(is_gpu)
            && !in_dormant_blast(world, **e)
            && !in_water(world, **e)
    })?;
    let component = world.emitter(entity)?;
    let global = world.global_transform(entity)?;
    let (emitter, visual) = parse(component);
    Some(loom_render::GpuEmitter {
        // Column 3 of the model matrix is its translation.
        origin: [global.matrix[12], global.matrix[13], global.matrix[14]],
        radius: emitter.radius,
        speed: emitter.speed,
        // Converted here rather than in the shader, so the degree-to-radian
        // constant lives on the side that owns the component's units.
        spread: emitter.spread_degrees.to_radians(),
        gravity: emitter.gravity,
        drag: emitter.drag,
        turbulence: emitter.turbulence,
        turbulence_scale: emitter.turbulence_scale,
        wind_response: emitter.wind_response,
        lifetime: emitter.lifetime,
        lifetime_jitter: emitter.lifetime_jitter,
        rate: emitter.rate,
        delay: emitter.delay,
        duration: emitter.duration,
        burst: emitter.burst,
        size: visual.size,
        color_start: visual.color_start,
        color_end: visual.color_end,
        alpha: visual.alpha,
        additive: visual.additive,
        flame: visual.flame,
        #[allow(clippy::cast_possible_truncation)]
        seed: emitter.seed as u32,
        // The one number the renderer will not recompute: the pool that holds
        // this emitter's live population without lapping. `loom_scene` has
        // already refused anything over the ceiling, with the number in the
        // error.
        pool: loom_particles::pool_size(&emitter),
    })
}

/// The part of an emitter that only the renderer cares about.
struct Visual {
    size: [f32; 2],
    color_start: [f32; 3],
    color_end: [f32; 3],
    alpha: [f32; 2],
    additive: bool,
    /// Draw a flame field over the quad instead of a sprite.
    flame: bool,
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
    /// Impact crowns in the air, each with the `elapsed` it was thrown at.
    ///
    /// Not systems: a crown is a closed form, so there is nothing to step. They
    /// are dropped from here when the last band has landed.
    crowns: Vec<(crate::play::Splash, f32)>,
}

impl Plumes {
    /// Build from a world and warm every plume to its settled population.
    pub(crate) fn new(world: &World, wind: loom_field::wind::Wind) -> Self {
        let mut plumes = Self {
            live: Vec::new(),
            instances: Vec::new(),
            wind,
            elapsed: 0.0,
            crowns: Vec::new(),
        };
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
            // **A GPU emitter has no CPU particles at all.** Stepping it here
            // as well would draw the plume twice, once from each simulation,
            // which reads as a denser plume rather than as a bug.
            if is_gpu(component) {
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
    ///
    /// **A scene that authors no splash gets the impact crown**, kept here
    /// beside the live systems rather than replayed: the window has no tick to
    /// replay from, only elapsed time. Same default and same closed form as
    /// `simulate`'s branch — W9's rule is that both paths reach it, because the
    /// last time a water effect was wired on one path only (`set_ripples`,
    /// ADR 0046 §7) the window drew flat water over a wake it was nevertheless
    /// feeling.
    pub(crate) fn splash(&mut self, world: &World, splash: crate::play::Splash) {
        let template = splash_template(world);
        if template.is_empty() {
            self.crowns.push((splash, self.elapsed));
        } else {
            self.play(template, splash.at, salt(splash.tick, splash.at));
        }
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
    ///
    /// **The crowns count as something to advance, and leaving them out of this
    /// guard was a shipped no-op.** `pool.loom` authors no `ParticleEmitter` at
    /// all — it is the scene the impact crown exists for — so `live` was empty,
    /// this returned before `elapsed` moved, and `rebuild_instances` was never
    /// reached. The splash was registered, the closed form was correct, and the
    /// window drew nothing at all. That is the third time this repository has
    /// found a water effect that is present, tested and invisible on one path
    /// (ADR 0046 §7), and the test above is what makes it the last.
    pub(crate) fn advance(&mut self, ticks: u32) {
        if ticks == 0 || (self.live.is_empty() && self.crowns.is_empty()) {
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
        // Crowns are re-evaluated rather than stepped, and a crown that has
        // landed returns nothing — which is also how it is retired, so there is
        // no second opinion about when it ends.
        let visual = default_droplet();
        let instances = &mut self.instances;
        let elapsed = self.elapsed;
        self.crowns.retain(|(splash, born)| {
            let age = elapsed - born;
            #[allow(clippy::cast_possible_truncation)]
            let seed = salt(splash.tick, splash.at) as u32;
            let drops =
                loom_water::spray::crown(splash.at, splash.speed, splash.radius, age, seed);
            // **The water itself, drawn before the droplets that leave it.**
            // Order is what the renderer blends in, so the sheet goes down
            // first and the droplets read as being in front of it.
            let column =
                loom_water::spray::column(splash.at, splash.speed, splash.radius, age, seed);
            let sheet = column_visual(splash.radius);
            for d in &column {
                instances.push(drawn_at(d.position, d.fraction, &sheet));
            }
            for d in &drops {
                instances.push(drawn_at(d.position, d.fraction, &visual));
            }
            // **Retired on the union, not on the crown.** The sheet outlives
            // the droplets — 0.72 s against 0.46 s at `pool.loom`'s entry — so
            // retiring on the crown alone would cut the water off halfway up.
            !drops.is_empty() || !column.is_empty()
        });
    }
}

/// One particle's drawable form: size and colour interpolated over its life.
fn instance(p: &loom_particles::Particle, visual: &Visual) -> ParticleInstance {
    drawn_at(p.position, p.fraction(), visual)
}

/// The same, for anything that knows where it is and how old it is without
/// being a `loom_particles::Particle` — the water's spray, which is a closed
/// form and has no system behind it.
fn drawn_at(position: [f32; 3], t: f32, visual: &Visual) -> ParticleInstance {
    // Smoke expands and pales as it cools and mixes with air; a plume whose
    // particles keep their birth size and colour reads as a stream of blobs.
    let size = visual.size[0] + (visual.size[1] - visual.size[0]) * t;
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    // Fade in as well as out. Particles that appear at full opacity pop, and
    // the pop is at the emitter, where the eye already is.
    // **No fade-in for a flame.** A flame is one long-lived static quad, so
    // `t` is essentially zero forever and an 8x ramp would leave `color.a` at
    // a few ten-thousandths — the authored alpha would be dead and the
    // brightness would drift with `--sim`, which is the kind of thing a golden
    // image blesses without anyone noticing.
    let fade = if visual.flame { 1.0 } else { (t * 8.0).min(1.0) };
    // Negative radius marks an additive particle. See `ParticleInstance` in
    // scene.slang: a radius is never legitimately negative, so its sign
    // carries the flag and the instance stays 32 bytes.
    let radius = if visual.additive { -size * 0.5 } else { size * 0.5 };
    ParticleInstance {
        position: [position[0], position[1], position[2], radius],
        color: [
            // The SIGN of red selects the flame field in the shader. Free,
            // because the schema clamps authored colour to [0, 1] so the bit
            // was unused — the same trick the sign of the radius plays for
            // `additive` three lines above.
            if visual.flame { -1.0 } else { 1.0 }
                * lerp(visual.color_start[0], visual.color_end[0]).max(1e-3),
            lerp(visual.color_start[1], visual.color_end[1]),
            lerp(visual.color_start[2], visual.color_end[2]),
            lerp(visual.alpha[0], visual.alpha[1]) * fade,
        ],
    }
}

// **The CPU splash crowns are gone — ADR 0015, trigger 3.**
//
// They were a closed form over the rate and the baked height field: a position
// exposure said a drop *should* have reached, at a rate derived from how open
// the column was. That is a good approximation of where rain lands on terrain
// and a wrong one everywhere else — under a mesh, on a sloped face, in the lee
// of anything, and anywhere the height field flattens an overhang away.
//
// A splash is now a collision the drop simulation actually resolved, appended
// to a GPU ring by `rain_sim.slang` and drawn indirectly. It carries the impact
// point and the surface normal, neither of which the CPU could know without
// re-deriving the whole field. `loom_rain::splashes` survives as the CPU
// answer for anything that needs to *reason* about impacts without a GPU —
// which nothing currently does, and which is why it has no caller here.

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
        // **A GPU emitter cannot be a template.** A template is *played*, once
        // per event, at wherever the event happened; the pool is one emitter at
        // one origin. So a `gpu = true` emitter under a dormant blast is
        // dropped here rather than silently CPU-simulated under a name that
        // says otherwise.
        .filter(|e| {
            world.emitter(**e).is_some_and(|c| !is_gpu(c)) && in_dormant_blast(world, **e)
        })
        .filter_map(|e| world.emitter(*e).map(parse))
        .collect()
}

/// The droplets a breaking sea is throwing right now — W5's first source.
///
/// **Closed form, so there is nothing to step and nothing to keep.** Unlike
/// every other emitter in this file, spray has no `System` behind it: a droplet
/// is a function of the wave set, the tick and where the eye is, so the whole
/// population is recomputed each time it is asked for and the answer is the
/// same in the viewer as in `loom render --sim N`. That is the same shape
/// `loom_rain::splashes` has, and the reason neither needs the `repeat` gate.
///
/// **What a droplet looks like is the water's own splash**, when the scene
/// authors one — the `ParticleEmitter` under the `WaterBody` that already says
/// what this water throws when something falls in it. A sea that authors spray
/// but no splash gets a plain white droplet rather than nothing, because the
/// authoring switch for spray is `WaterBody::spray` and a feature that silently
/// needs a second component is the kind of no-op this project keeps finding.
pub(crate) fn spray(
    world: &World,
    water: &loom_scene::components::WaterBody,
    ground: &dyn Fn(f32, f32) -> f32,
    eye: [f32; 3],
    seconds: f32,
) -> Vec<ParticleInstance> {
    // **A sea too gentle to break can never spray, and nothing else would say
    // so.** `fold` is `Σ Q·k·A·sin φ`, so `Σ Q·k·A` is its ceiling; under
    // `SPRAY_BREAK` the crest test inside `spray()` fails at every point and
    // every instant, and the author sees an empty sky and concludes the
    // feature is broken. It was not hypothetical — *every* sea in this
    // repository was under the threshold when spray shipped.
    //
    // Warned from here rather than refused at load, because a sea whose waves
    // are raised later is a legitimate thing to author, and this is a look
    // rather than a correctness matter. Once per process: it is asked every
    // frame, and a warning sixty times a second is one nobody reads.
    if water.spray > 0.0 {
        let peak = loom_water::spray::peak_fold(water);
        if peak < loom_water::spray::SPRAY_BREAK {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| {
                crate::log::warn(format!(
                    "the WaterBody authors spray = {:.2} but its waves peak at a fold of \
                     {peak:.3}, under the {:.2} a crest has to reach to break — so no \
                     droplet can ever be thrown. Raise `steepness` or `amplitude`, or \
                     shorten `wavelength`.",
                    water.spray,
                    loom_water::spray::SPRAY_BREAK,
                ));
            });
        }
    }
    let droplets = loom_water::spray::spray(water, eye, seconds, ground);
    if droplets.is_empty() {
        return Vec::new();
    }
    let visual = droplet_visual(world);
    droplets
        .iter()
        .map(|d| drawn_at(d.position, d.fraction, &visual))
        .collect()
}

/// The emitters of the scene's splash, if the water authors one.
fn splash_template(world: &World) -> Vec<(loom_particles::Emitter, Visual)> {
    world
        .entities()
        .iter()
        .filter(|e| world.emitter(**e).is_some_and(|c| !is_gpu(c)) && in_water(world, **e))
        .filter_map(|e| world.emitter(*e).map(parse))
        .collect()
}

pub(crate) fn simulate(
    world: &World,
    wind: &loom_field::wind::Wind,
    ticks: Option<u32>,
    fired: &[(u64, [f32; 3])],
    splashed: &[crate::play::Splash],
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
        // The device owns this one; see `Plumes::new` above, which skips it for
        // the same reason — stepping it here as well would draw the plume
        // twice, once from each simulation.
        if is_gpu(component) {
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

    // Explosions a script set off during the run, replayed from the tick each
    // happened on. Deterministic for the same reason everything else here is:
    // the tick is data, not a clock reading, so `--sim N` means one thing.
    //
    // Blasts are salted by tick alone, which is what they have always been
    // salted by. Two set off on the same tick therefore look alike — a real if
    // minor limitation, left as it is because changing it would move every
    // committed reference image of a scene that fires one, which is a bigger
    // claim than this step is making.
    let blast = blast_template(world);
    for (at_tick, at) in fired {
        for (emitter, visual) in &blast {
            let mut system = loom_particles::System::new(emitter.seed ^ *at_tick);
            for _ in 0..elapsed_since(ticks, *at_tick) {
                system.step_in_wind(DT, emitter, *at, &|p| wind.at(p, 0.0));
            }
            for p in system.particles() {
                out.push(instance(p, visual));
            }
        }
    }

    // And the splashes the water raised, salted by place as well as tick so two
    // things going in together do not throw one burst drawn twice.
    let template = splash_template(world);
    for s in splashed {
        let steps = elapsed_since(ticks, s.tick);
        if template.is_empty() {
            out.extend(crown(world, s, age_of(steps)));
            continue;
        }
        for (emitter, visual) in &template {
            let mut system = loom_particles::System::new(emitter.seed ^ salt(s.tick, s.at));
            for _ in 0..steps {
                system.step_in_wind(DT, emitter, s.at, &|p| wind.at(p, 0.0));
            }
            for p in system.particles() {
                out.push(instance(p, visual));
            }
        }
    }

    out
}

/// Ticks between an event and the frame being drawn.
fn elapsed_since(ticks: Option<u32>, at_tick: u64) -> u32 {
    #[allow(clippy::cast_possible_truncation)]
    ticks.unwrap_or(0).saturating_sub(u32::try_from(at_tick).unwrap_or(u32::MAX))
}

/// Seconds, from a tick count. Never a clock (never-do #8).
fn age_of(steps: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    {
        steps as f32 * DT
    }
}

/// **The impact crown, which is what a scene that authors no splash gets.**
///
/// The same arrangement `spray` above has and for the same reason: the
/// authoring switch for a splash is the entry itself, and a feature that
/// silently needs a second component to produce anything is the no-op class
/// this project keeps finding. `pool.loom` authors no `ParticleEmitter` under
/// its water and is the scene W9 exists for.
///
/// Closed form — a function of the event and how old it is — so it needs no
/// system, no state and no `repeat` gate, exactly like the crest spray.
fn crown(world: &World, splash: &crate::play::Splash, age: f32) -> Vec<ParticleInstance> {
    let visual = droplet_visual(world);
    let sheet = column_visual(splash.radius);
    // The same salt the authored template is seeded by, narrowed: two bodies
    // going in on one tick turn their rings differently.
    #[allow(clippy::cast_possible_truncation)]
    let seed = salt(splash.tick, splash.at) as u32;
    // **The rising water first, then the droplets that came off it** — W12,
    // and the order is the blend order. Both halves are wired here *and* in
    // `Plumes::rebuild_instances`, because the last time a water effect reached
    // one path only (`set_ripples`, ADR 0046 §7) the window drew flat water
    // over a wake it was nevertheless feeling.
    let mut out: Vec<ParticleInstance> =
        loom_water::spray::column(splash.at, splash.speed, splash.radius, age, seed)
            .iter()
            .map(|d| drawn_at(d.position, d.fraction, &sheet))
            .collect();
    out.extend(
        loom_water::spray::crown(splash.at, splash.speed, splash.radius, age, seed)
            .iter()
            .map(|d| drawn_at(d.position, d.fraction, &visual)),
    );
    out
}

/// What a droplet looks like: the water's own splash when the scene authors
/// one, and a small pale droplet when it does not.
///
/// Shared by the crest spray and the impact crown, because they are the same
/// substance and an author who described one meant both.
fn droplet_visual(world: &World) -> Visual {
    splash_template(world).into_iter().next().map_or_else(default_droplet, |(_, visual)| visual)
}

/// What the rising sheet is made of — W12.
///
/// **Not the droplet visual scaled up, and not the water's authored splash
/// either.** A droplet is a bead: small, bright, and it keeps its opacity to
/// the end of its arc. A sheet is aerated water, so it has to be broad enough
/// that adjacent quads overlap into a surface, and it disappears by going
/// transparent rather than by shrinking.
///
/// **The size is a function of the cavity, and that is the whole reason this
/// takes an argument.** A fixed size was written first and it is wrong for
/// every body but the one it was set on: the ring spacing is
/// `2π · COLUMN_FLARE · radius / COLUMN_RING`, so a quad that closes the sheet
/// on `pool.loom`'s 0.5 m sphere leaves visible gaps on `water_crate`'s wider
/// crate — a necklace of beads, which is precisely the artifact the sheet
/// exists to remove. 0.84 is that expression with the constants in it, so the
/// rim just closes at the widest the sheet ever gets and the base overlaps
/// heavily.
///
/// **It does not shrink.** A sheet that shrank like a droplet would open the
/// same gaps halfway through its life, which is when it is largest on screen.
///
/// **Deliberately not `droplet_visual`.** That function answers "what does this
/// water's *spray* look like", and a scene authoring a splash emitter is
/// describing droplets; taking the sheet from it would make a scene with an
/// authored smoke-coloured splash raise a column of smoke.
fn column_visual(radius: f32) -> Visual {
    let quad = 0.84 * radius;
    Visual {
        size: [quad, quad],
        color_start: [0.88, 0.93, 0.97],
        color_end: [0.72, 0.81, 0.88],
        alpha: [0.72, 0.0],
        additive: false,
        flame: false,
    }
}

/// Small, shrinking, and white going to a pale blue-grey: a droplet is not a
/// smoke puff and the default `Visual` is one.
fn default_droplet() -> Visual {
    Visual {
        size: [0.16, 0.05],
        color_start: [0.92, 0.96, 1.0],
        color_end: [0.70, 0.80, 0.88],
        alpha: [0.9, 0.0],
        additive: false,
        flame: false,
    }
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

    /// One entry into the water, hard enough to throw a full crown.
    fn entry(tick: u64, at: [f32; 3]) -> crate::play::Splash {
        crate::play::Splash { tick, at, speed: 7.1, radius: 0.5 }
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

        let out = simulate(&sea(), &calm(), Some(20), &[], &[entry(10, at)]);

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
            simulate(&world, &calm(), Some(11), &[], &[entry(10, at)])
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

    /// **The spray a breaking sea throws, drawn with the water's own splash.**
    /// `splash.loom`'s sea authors no spray, so the first half of this is also
    /// the compatibility check: turning it on is the only thing that produces a
    /// droplet.
    #[test]
    fn a_breaking_sea_sprays_and_a_calm_one_does_not() {
        let world = sea();
        let mut body = crate::weather::water_of(&world, &calm()).expect("splash.loom has water");
        let deep = |_x: f32, _z: f32| -1000.0_f32;
        let eye = [0.0, 1.6, 0.0];

        // As authored: `spray` defaults to zero and nothing is thrown, at any
        // moment of the run.
        for tick in [0_u32, 60, 300, 900] {
            #[allow(clippy::cast_precision_loss)]
            let t = f32::from(u16::try_from(tick).expect("small")) / 60.0;
            assert!(spray(&world, &body, &deep, eye, t).is_empty(), "tick {tick}");
        }

        // **Authored on is not enough — the sea also has to break, and
        // `splash.loom`'s does not.** Measured rather than assumed: its three
        // waves sum to `Σ Q·k·A = 0.156`, which is the *supremum* of `fold`
        // and reached only if every crest aligns; `SPRAY_BREAK` is 0.33. So
        // `spray = 4.0` on this sea throws nothing at any tick, and a test
        // that stopped here would have been asserting against a threshold no
        // scene in the repository can reach — the exact shape of gate that
        // reports a pass without ever looking at the subject. The steep set
        // below is `spindrift.loom`'s, at `Σ Q·k·A = 0.75`.
        body.spray = 4.0;
        for tick in [0_u32, 120, 600] {
            #[allow(clippy::cast_precision_loss)]
            let t = f32::from(u16::try_from(tick).expect("small")) / 60.0;
            assert!(
                spray(&world, &body, &deep, eye, t).is_empty(),
                "a sea whose fold never reaches SPRAY_BREAK sprayed at tick {tick}"
            );
        }

        for (wave, (wavelength, amplitude, steepness)) in body
            .waves
            .waves
            .iter_mut()
            .zip([(17.0, 1.0, 0.676), (9.0, 0.5, 0.716), (5.0, 0.28, 0.71)])
        {
            wave.wavelength = wavelength;
            wave.amplitude = amplitude;
            wave.steepness = steepness;
        }
        let thrown: usize = (0..600)
            .map(|tick| {
                #[allow(clippy::cast_precision_loss)]
                let t = tick as f32 / 60.0;
                spray(&world, &body, &deep, eye, t).len()
            })
            .sum();
        assert!(thrown > 0, "a breaking sea threw no spray in ten seconds");

        // And it is drawn as droplets rather than as smoke: the splash
        // template's own colour, which is nearly white.
        body.spray = 8.0;
        let drops: Vec<ParticleInstance> = (0..600)
            .flat_map(|tick| {
                #[allow(clippy::cast_precision_loss)]
                let t = tick as f32 / 60.0;
                spray(&world, &body, &deep, eye, t)
            })
            .collect();
        let first = drops.first().expect("droplets");
        assert!(first.color[0] > 0.5 && first.color[2] > 0.5, "{first:?} is not spray-coloured");
    }

    /// **The rising water reaches BOTH paths — W12.**
    ///
    /// This is the test the `set_ripples` defect (ADR 0046 §7) says this
    /// repository owes every water effect: a feature wired into `simulate` and
    /// not into `Plumes` is present, tested, and invisible in the window the
    /// human actually watches. So both are asked, and both are asked for the
    /// thing that distinguishes a column from a crown — water standing *above*
    /// the droplets' own ceiling.
    ///
    /// `pool.loom` is the fixture because it authors no `ParticleEmitter` under
    /// its water, which is the branch the crown and the column live on.
    #[test]
    fn the_rising_water_reaches_both_the_headless_and_the_window_path() {
        let world = {
            let source = std::fs::read_to_string("../../assets/test/pool.loom").expect("fixture");
            World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"))
        };
        let at = [0.0, 0.0, 0.0];
        // Six ticks after the entry — the frame `pool.loom`'s golden reference
        // is taken on, so this is the population that image protects.
        let ticks = 6;
        let splash = entry(0, at);
        #[allow(clippy::cast_precision_loss)]
        let age = ticks as f32 * DT;
        // The tallest droplet the crown can ever reach at this entry. Anything
        // above it is water, not spray.
        let ceiling = loom_water::spray::crown(at, splash.speed, splash.radius, age, 0)
            .iter()
            .fold(0.0_f32, |best, d| best.max(d.position[1]));
        assert!(ceiling > 0.0, "the crown threw nothing to compare against");

        let headless = simulate(&world, &calm(), Some(ticks), &[], &[splash]);

        let mut plumes = Plumes::new(&world, calm());
        plumes.splash(&world, splash);
        plumes.advance(ticks);
        let window = plumes.instances().to_vec();

        for (path, out) in [("headless", &headless), ("window", &window)] {
            let top = out.iter().fold(0.0_f32, |best, p| best.max(p.position[1]));
            assert!(
                top > ceiling,
                "{path} drew nothing above the crown's {ceiling} m ceiling — top was {top} m, \
                 so the water is not rising on that path"
            );
        }
        assert_eq!(
            headless.len(),
            window.len(),
            "the two paths drew different populations for one entry"
        );
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
