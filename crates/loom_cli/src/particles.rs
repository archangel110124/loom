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
            // **Not authorable, and deliberately.** A shutter belongs to a
            // substance, not to a knob: smoke and fire are not smeared by one
            // and a droplet is. `splash_template` is what says "these are the
            // water's droplets", which is a fact about where the emitter sits
            // in the scene rather than a field an author has to know to set.
            shutter: 0.0,
        },
    )
}

/// The mist bank at the foot of the scene's cascade, if it has one — ADR 0054.
///
/// **What was measured first, because the design pass asked for it.** The plan
/// was to reuse ADR 0020/0050's marched soot volume with a bright albedo, and
/// the stated risk was cost: the march early-outs when transmittance falls
/// below `SMOKE_T_MIN`, and a bright low-density medium was predicted to
/// early-out later and therefore cost more.
///
/// **It cannot.** `T *= 1 − a` with `a = 1 − exp(−σ·dt)`, and `σ` is
/// `SMOKE_SIGMA · density · alpha`. The albedo — `color_start`/`color_end`,
/// which reaches the shader as `in.color.rgb` — appears only in `acc`, never in
/// `T`. Measured anyway on `plume.loom` at 1920x1080, three runs at its
/// authored soot albedo against three at mist-white: **0.747 / 0.752 /
/// 0.804 ms** against **0.776 / 0.746 / 0.770 ms**. One number, inside
/// run-to-run noise.
///
/// **The volume was then built, rendered, and rejected on its shape.**
/// `sootEnvelope` is a chimney: height `2R`, greatest radius `0.61R`, and a
/// foot radius of `0.09R` because `SMOKE_R0` is 0.16. Standing one at the
/// impact draws a narrow wisp rising *over* the cliff — a column of steam, not
/// a bank of mist. Widening it means changing the envelope every fire and plume
/// in the engine shares, which is a much larger claim than this slice is
/// making, and stacking several is N marches for N chimneys.
///
/// So the mist is **ordinary alpha sprites**, which is what the sprite path is
/// for and what a broad soft haze actually is. The measurement above is kept
/// because it is the useful half: a bright marched volume is free, and the
/// reason to reach for one is shape, not cost.
///
/// **What is derived and what is authored.** The position is the fall's foot —
/// `lip + fall_direction · X(drop)`, out of the same hydraulics the sheet is
/// drawn from, which is 0.89 m forward on `spout` and 4.17 m on `cascade` and
/// is not something an author can place by hand once the discharge changes. The
/// spread is the lip's own; the rate and the puff size scale with the lip and
/// the drop. What is left is `Cascade::mist`, a multiplier, because how much
/// water an impact throws into the air depends on what it hits and a free-fall
/// model does not know.
fn cascade_mist(
    world: &World,
    wind: &loom_field::wind::Wind,
) -> Option<(loom_particles::Emitter, Visual, [f32; 3])> {
    // **The cheap question first.** Every scene in the repository reaches this
    // function twice, and `water_of` derives a wave spectrum from the wind;
    // paying for that in a scene with no waterfall would be a cost this feature
    // charges to everything that does not use it.
    world.cascade()?;
    let surface = crate::weather::water_of(world, wind)?.surface_height;
    let c = crate::resolve_cascade(world, surface)?;
    if c.authored.mist <= 0.0 || c.drop <= 0.0 {
        return None;
    }
    // Both terms matter and neither alone is enough: a wide low weir throws a
    // broad shallow bank and a narrow tall fall throws a thin high one.
    let scale = c.authored.mist * c.half_span.mul_add(0.35, 0.16 * c.drop);
    let span = c.half_span.max(0.25);
    // Through `parse` rather than by building an `Emitter` and a `Visual`
    // directly, so the bank reads its defaults from exactly the same place an
    // authored emitter does and cannot drift away from them.
    let (emitter, visual) = parse(&serde_json::json!({
        // Proportional to the lip: twice the weir, twice the water landing.
        "rate": f64::from((c.authored.mist * 16.0 * span).min(200.0)),
        "lifetime": 2.6,
        "lifetime_jitter": 0.45,
        // Thrown up out of the impact and stopped almost at once by drag, which
        // is what makes a bank rather than a fountain.
        "speed": 2.4,
        "spread_degrees": 60.0,
        "radius": f64::from(span),
        "gravity": 0.5,
        "drag": 1.5,
        "turbulence": 0.9,
        "turbulence_scale": 0.4,
        // **Zero, and this is ADR 0045's trap clause in miniature.** The bank
        // stands where the water lands. Letting the scene's wind carry it would
        // walk it off the impact over a long run, and the impact is the only
        // thing anchoring it.
        "wind_response": 0.0,
        "size": [f64::from(scale * 1.1), f64::from(scale * 3.4)],
        // Thin, and many: a bank is an accumulation of nearly-transparent
        // puffs. Opaque ones read as cotton wool.
        "alpha": [0.22, 0.0],
        // Mist is water, so it is nearly white — but not 1.0, or it clips
        // against a bright sky and loses its own shading.
        "color_start": [0.90, 0.92, 0.95],
        "color_end": [0.86, 0.89, 0.93],
        "additive": false,
        "flame": false,
        // Fixed rather than salted: there is one cascade per scene, so there is
        // nothing for a salt to tell apart.
        "seed": 0x_ca5c_ade5_u64,
    }));
    Some((emitter, visual, c.foot))
}

/// A drip source with its floor already found — ADR 0054.
///
/// **The raycast happens once, here, and never again.** A drip's whole
/// trajectory is a closed form in how far it has to fall, and how far it has to
/// fall is a static fact about the scene: the lip does not move and neither
/// does the floor under it. Casting per drop, or per frame, would be the same
/// ray answered a hundred times a second for one number.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DripSite {
    origin: [f32; 3],
    floor_y: f32,
    rate: f32,
    lip_radius: f32,
    seed: u32,
}

/// How far down a drip source looks for something to land on, in metres.
///
/// **A limit rather than infinity, and a miss is a miss.** A source with
/// nothing under it inside this distance draws nothing at all, which is the
/// honest answer: a drip falling forever is not a drip, and the alternative —
/// landing at some default depth — puts a crown in mid-air where no gate can
/// see it.
const DRIP_REACH: f32 = 60.0;

/// How far below the lip the ray starts, in metres.
///
/// **Not zero, and this cost a render.** A drip source is authored on the
/// *underside* of something — a pipe, a beam, a ledge — so its node sits
/// exactly on that collider's face, and `rapier` reports a ray beginning inside
/// a solid as an immediate hit at distance zero. Every source in
/// `dripping.loom` found its floor at its own height and drew nothing, with no
/// error anywhere. Two centimetres clears any lip and is four drop diameters;
/// it moves the ray's start and not the drop's, so the landing point is
/// unchanged.
const DRIP_RAY_BIAS: f32 = 0.02;

/// Every `DripSource` in the scene, with the floor under it.
///
/// `physics` is the collision world the run is holding. **Without one there are
/// no drips**, rather than drips falling to a guessed floor — see `DRIP_REACH`.
pub(crate) fn drip_sites(
    world: &World,
    physics: Option<&loom_physics::Physics>,
) -> Vec<DripSite> {
    let Some(physics) = physics else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entity in world.entities() {
        let (Some(component), Some(global)) =
            (world.drip_source(*entity), world.global_transform(*entity))
        else {
            continue;
        };
        let Ok(source) =
            serde_json::from_value::<loom_scene::components::DripSource>(component.clone())
        else {
            continue;
        };
        if source.rate <= 0.0 {
            continue;
        }
        let origin = [global.matrix[12], global.matrix[13], global.matrix[14]];
        let mut start = [origin[0], origin[1] - DRIP_RAY_BIAS, origin[2]];
        let mut hit = physics.raycast(start, [0.0, -1.0, 0.0], DRIP_REACH);
        // **A lip is on the UNDERSIDE of something, so the ray usually starts
        // inside it.** `raycast` reports a ray beginning in a solid as an
        // immediate hit at distance zero — right for a muzzle clipping a wall,
        // wrong here — so leave the solid through `raycast_exit` and cast again
        // from just past its far face. Every source in `dripping.loom` found
        // its floor at its own height until this existed, and drew nothing,
        // with no error anywhere.
        if hit.as_ref().is_some_and(|h| h.distance < DRIP_RAY_BIAS) {
            let Some(exit) = physics.raycast_exit(start, [0.0, -1.0, 0.0], DRIP_REACH) else {
                continue;
            };
            start = [origin[0], exit.point[1] - DRIP_RAY_BIAS, origin[2]];
            hit = physics.raycast(start, [0.0, -1.0, 0.0], DRIP_REACH);
        }
        let Some(hit) = hit else {
            continue;
        };
        out.push(DripSite {
            origin,
            floor_y: hit.point[1],
            rate: source.rate,
            lip_radius: source.lip_radius,
            // **Salted by where it is**, so a row of identical sources along a
            // pipe does not drip in lockstep — the same argument the grass
            // clump hash and the flicker phase both make.
            seed: (origin[0].to_bits() ^ origin[2].to_bits().rotate_left(13))
                ^ entity.index(),
        });
    }
    out
}

/// What every drip source in the scene has in the air at `now` seconds.
fn drip_instances(sites: &[DripSite], visual: &Visual, now: f32, out: &mut Vec<ParticleInstance>) {
    for site in sites {
        for d in &loom_water::drip::drops(
            site.origin,
            site.floor_y,
            site.rate,
            site.lip_radius,
            now,
            site.seed,
        ) {
            out.push(drawn_drop(d, visual));
        }
    }
}

/// How a drip is drawn.
///
/// **`scale` carries the drop's diameter in metres**, so the visual's own size
/// is 1.0 and the physics is what decides how big a drip looks — Tate's law
/// reaching the screen with nothing in between to override it. Every other user
/// of `drawn_drop` uses `scale` as a multiplier on an authored size; this one
/// authors no size, which is the point.
fn drip_visual() -> Visual {
    Visual {
        size: [1.0, 1.0],
        // Water, lit brightly enough to read against a dark wall. A drip is
        // seen by its highlight, not its body.
        color_start: [0.86, 0.90, 0.95],
        color_end: [0.86, 0.90, 0.95],
        alpha: [1.0, 1.0],
        additive: false,
        flame: false,
        // **A drip is exactly the thing the shutter exists for.** At 6 m/s a
        // 5 mm drop is smeared to five centimetres, which is the thin bright
        // streak reference image 63 is made of; without it, a bead.
        shutter: WATER_SHUTTER,
    }
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
    /// Seconds of motion one quad stands for. **Zero is a disc**, which is
    /// every emitter in the engine except the water's own splash — see
    /// [`WATER_SHUTTER`].
    shutter: f32,
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
    /// Drip sources with their floors already found — ADR 0054. Resolved once
    /// at construction, because the ray they need is a static fact about the
    /// scene and its answer is a closed form away from every droplet.
    drips: Vec<DripSite>,
    /// Impact crowns in the air, each with the `elapsed` it was thrown at.
    ///
    /// Not systems: a crown is a closed form, so there is nothing to step. They
    /// are dropped from here when the last band has landed.
    crowns: Vec<(crate::play::Splash, f32)>,
}

impl Plumes {
    /// Build from a world and warm every plume to its settled population.
    pub(crate) fn new(
        world: &World,
        wind: loom_field::wind::Wind,
        physics: Option<&loom_physics::Physics>,
    ) -> Self {
        let mut plumes = Self {
            live: Vec::new(),
            instances: Vec::new(),
            wind,
            elapsed: 0.0,
            drips: drip_sites(world, physics),
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
        // The cascade's mist, which is not an entity — ADR 0054. **Both
        // particle paths raise it**, this one and `simulate`; W9's rule, and
        // the reason is that the last water effect wired on one path only left
        // the window drawing flat water over a wake it was feeling.
        if let Some((emitter, visual, origin)) = cascade_mist(world, &plumes.wind) {
            let mut system = loom_particles::System::new(emitter.seed);
            // Warmed to its settled population like any other steady emitter,
            // or the window opens on a waterfall with one puff at its foot.
            // **In still air, not the scene's wind**, matching `wind_response`
            // of zero — see `cascade_mist`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let warm = ((emitter.lifetime * 2.0) / DT).ceil() as u32;
            for _ in 0..warm {
                system.step_in_wind(DT, &emitter, origin, &|_| [0.0; 3]);
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
        // Drips first, and re-evaluated rather than stepped: `loom_water::drip`
        // is a pure function of the clock, so there is nothing to advance and
        // nothing to retire.
        drip_instances(&self.drips, &drip_visual(), self.elapsed, &mut self.instances);
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
            // Crown, then jet, then satellites — one clock, one call, in blend
            // order. See `loom_water::spray::impact`.
            let drops =
                loom_water::spray::impact(splash.at, splash.speed, splash.radius, age, seed);
            for d in &drops {
                instances.push(drawn_drop(d, &visual));
            }
            // **Retired on the whole impact, not on the crown.** The jet fires
            // after the rim has started to collapse and outlives it, so
            // retiring on the crown alone would cut the splash off before its
            // tallest moment.
            !drops.is_empty()
        });
    }
}

/// One particle's drawable form: size and colour interpolated over its life.
fn instance(p: &loom_particles::Particle, visual: &Visual) -> ParticleInstance {
    drawn_at(p.position, p.velocity, p.fraction(), 1.0, 1.0, visual)
}

/// The same, for a droplet the water threw — those carry their own size
/// multiplier, which is what stops a band of a crown reading as identical
/// beads. See `loom_water::spray::Droplet::scale`.
fn drawn_drop(d: &loom_water::spray::Droplet, visual: &Visual) -> ParticleInstance {
    drawn_at(d.position, d.velocity, d.fraction, d.scale, d.alpha, visual)
}

/// Seconds one water droplet's smear stands for.
///
/// **1/120 s: a real 60 fps frame with a 180-degree shutter.** Rain takes
/// double that (`RAIN_SHUTTER = 0.03`) because a streak is what makes a wall
/// of falling water read as weather at all; a droplet thrown off a crown is a
/// *thing*, and doubling its smear turns a crown into a starburst.
///
/// It is the whole switch. Zero here — which is every other particle in the
/// engine — is a disc, bit for bit as before.
const WATER_SHUTTER: f32 = 1.0 / 120.0;

/// The same, for anything that knows where it is and how old it is without
/// being a `loom_particles::Particle` — the water's spray, which is a closed
/// form and has no system behind it.
fn drawn_at(
    position: [f32; 3],
    velocity: [f32; 3],
    t: f32,
    scale: f32,
    opacity: f32,
    visual: &Visual,
) -> ParticleInstance {
    // Smoke expands and pales as it cools and mixes with air; a plume whose
    // particles keep their birth size and colour reads as a stream of blobs.
    let size = (visual.size[0] + (visual.size[1] - visual.size[0]) * t) * scale;
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
    // **A marched plume's node position is its BASE, not its centre.** Every
    // other particle is a puff centred where it is, but a soot volume is a
    // column of a known height whose bottom belongs to a fire — and the fire's
    // node is the only place the author can name that is *derived* rather than
    // typed. `plume.loom` used to hand-type the centre at y = 5.5 with the
    // flame's tip at y = 1.3, and nothing tied the two together; the gap was
    // authored, so no gate could see it.
    //
    // The instance still carries the CENTRE, so `origin.w`, the whole fragment
    // path and the back-to-front sort key are untouched.
    let centre_y = if visual.flame && !visual.additive {
        position[1] + size * 0.5
    } else {
        position[1]
    };
    ParticleInstance {
        position: [position[0], centre_y, position[2], radius],
        color: [
            // The SIGN of red selects the flame field in the shader. Free,
            // because the schema clamps authored colour to [0, 1] so the bit
            // was unused — the same trick the sign of the radius plays for
            // `additive` three lines above.
            if visual.flame { -1.0 } else { 1.0 }
                * lerp(visual.color_start[0], visual.color_end[0]).max(1e-3),
            lerp(visual.color_start[1], visual.color_end[1]),
            lerp(visual.color_start[2], visual.color_end[2]),
            // `opacity` is the crown's fingering and nothing else — 1.0 for
            // every particle in the engine that is not a droplet off a rim, so
            // it is an exact no-op everywhere else. See
            // `loom_water::spray::Droplet::alpha`.
            lerp(visual.alpha[0], visual.alpha[1]) * fade * opacity,
        ],
        // **Zero shutter is a disc**, which is every emitter but the water's
        // own droplets: a shutter open for no time records no smear, so the
        // physical reading and the compatibility reading are one number and
        // there is no second flag to fall out of step with it.
        velocity: [velocity[0], velocity[1], velocity[2], visual.shutter],
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
    sea: Option<&loom_water::ocean::Ocean>,
    ground: &dyn Fn(f32, f32) -> f32,
    eye: [f32; 3],
    seconds: f32,
) -> Vec<ParticleInstance> {
    let droplets = loom_water::spray::spray(water, sea, eye, seconds, ground);
    if droplets.is_empty() {
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
        //
        // **Asked only on a frame that threw nothing**, which is what makes the
        // spectrum path affordable: the cascade's ceiling is a walk of 98,304 tile
        // values, not a sixteen-term sum. It loses no warning that was printed before —
        // a ceiling under the threshold means every crest test failed, so the frame was
        // empty by construction — and an empty frame is the only moment an author is
        // asking this question anyway.
        //
        // **Not on the cinematic tier**, where the field means something else. A
        // cinematic body has no Gerstner waves to fold, so `peak_fold` is
        // identically zero and this would fire on every scene that opts in — a
        // warning that is always wrong is worse than none. There `spray` switches
        // on drawing the solver's own thrown particles; see `WaterBody::spray`.
        let spectrum = water.wave_model == loom_scene::components::WaveModel::Spectrum;
        if water.spray > 0.0 && water.simulation != loom_scene::components::WaterSimTier::Cinematic
        {
            // **The two models carry different ceilings and the sentence printed below
            // has to be the one that is true.** A wave list's `Σ Q·k·A` is analytic and
            // over all time — that sea can be told it will *never* throw. A cascade's is
            // the largest fold its tiles reach *at this instant*
            // (`Ocean::peak_fold`), and a sea that is not breaking now may break in ten
            // seconds: measured over 40 quarter-second steps of a 50 km fetch, the
            // ceiling wanders 0.212–0.289 at `u10 = 2` and 1.34–1.51 on
            // `ocean_fft_storm.loom`, about ±15% either way. So the spectrum arm says
            // "is not breaking", never "cannot break".
            //
            // `NaN` before the first `evolve` and on a spectrum body with no cascade at
            // all, and `NaN < SPRAY_BREAK` is false — a sea nobody has evolved yet is
            // not a sea that has been measured and found flat.
            let peak = if spectrum {
                sea.map_or(f32::NAN, loom_water::ocean::Ocean::peak_fold)
            } else {
                loom_water::spray::peak_fold(water)
            };
            if peak < loom_water::spray::SPRAY_BREAK {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| {
                    let break_at = loom_water::spray::SPRAY_BREAK;
                    let (authored, tail) = (water.spray, "and no droplet is thrown.");
                    crate::log::warn(if spectrum {
                        format!(
                            "the WaterBody authors spray = {authored:.2} but its cascade is \
                             folding at most {peak:.3} anywhere on its tiles this tick, \
                             under the {break_at:.2} a crest has to reach to break — so \
                             nothing on this sea is breaking {tail} Raise the scene's \
                             `Wind.speed` or its `fetch`. (A spectrum ceiling is this \
                             instant's, not the run's: a sea this gentle now can break \
                             later.)"
                        )
                    } else {
                        format!(
                            "the WaterBody authors spray = {authored:.2} but its waves peak \
                             at a fold of {peak:.3}, under the {break_at:.2} a crest has \
                             to reach to break — so no crest can ever break {tail} Raise \
                             `steepness` or `amplitude`, or shorten `wavelength`."
                        )
                    });
                });
            }
        }
        return Vec::new();
    }
    let visual = droplet_visual(world);
    droplets
        .iter()
        .map(|d| drawn_drop(d, &visual))
        .collect()
}

/// The emitters of the scene's splash, if the water authors one.
fn splash_template(world: &World) -> Vec<(loom_particles::Emitter, Visual)> {
    world
        .entities()
        .iter()
        .filter(|e| world.emitter(**e).is_some_and(|c| !is_gpu(c)) && in_water(world, **e))
        .filter_map(|e| world.emitter(*e).map(parse))
        // **This is where a shutter is decided, and the test is structural.**
        // An emitter parented under a `WaterBody` *is* that water's splash —
        // the scene comment in `splash.loom` says so and `in_water` is what
        // enforces it. So these particles are droplets, and droplets smear.
        // A plume, a flame or an explosion reaches this function never.
        .map(|(emitter, visual)| (emitter, Visual { shutter: WATER_SHUTTER, ..visual }))
        .collect()
}

pub(crate) fn simulate(
    world: &World,
    wind: &loom_field::wind::Wind,
    ticks: Option<u32>,
    fired: &[(u64, [f32; 3])],
    splashed: &[crate::play::Splash],
    physics: Option<&loom_physics::Physics>,
) -> Vec<ParticleInstance> {
    let mut out = Vec::new();

    // Drips — ADR 0054. Evaluated rather than stepped: `loom_water::drip` is a
    // pure function of the clock, so `--sim N` and a window that has been open
    // for `N` ticks draw the same drops with nothing warmed up.
    #[allow(clippy::cast_precision_loss)]
    let now = ticks.unwrap_or(0) as f32 * DT;
    let sites = drip_sites(world, physics);
    drip_instances(&sites, &drip_visual(), now, &mut out);

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

    // The cascade's mist — ADR 0054. Not an entity, so it is raised here as
    // well as in `Plumes::new`; see the note there.
    if let Some((emitter, visual, origin)) = cascade_mist(world, wind) {
        let mut system = loom_particles::System::new(emitter.seed);
        // Same rule every emitter in this function follows: `--sim N` steps N,
        // and a still with no `--sim` steps far enough to look settled.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let steps = ticks.unwrap_or_else(|| ((emitter.lifetime * 2.0) / DT).ceil() as u32);
        for _ in 0..steps {
            system.step_in_wind(DT, &emitter, origin, &|_| [0.0; 3]);
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
    // The same salt the authored template is seeded by, narrowed: two bodies
    // going in on one tick turn their rings differently.
    #[allow(clippy::cast_possible_truncation)]
    let seed = salt(splash.tick, splash.at) as u32;
    // **One call, so this path and `Plumes::rebuild_instances` cannot draw
    // different splashes.** The last time a water effect reached one path only
    // (`set_ripples`, ADR 0046 §7) the window drew flat water over a wake it
    // was nevertheless feeling; two call sites composing the same three
    // populations by hand is the same hazard with a longer fuse.
    loom_water::spray::impact(splash.at, splash.speed, splash.radius, age, seed)
        .iter()
        .map(|d| drawn_drop(d, &visual))
        .collect()
}

/// What a droplet looks like: the water's own splash when the scene authors
/// one, and a small pale droplet when it does not.
///
/// Shared by the crest spray and the impact crown, because they are the same
/// substance and an author who described one meant both.
fn droplet_visual(world: &World) -> Visual {
    splash_template(world).into_iter().next().map_or_else(default_droplet, |(_, visual)| visual)
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
        shutter: WATER_SHUTTER,
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
        let quiet = simulate(&range(), &calm(), Some(60), &[], &[], None);

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

        let out = simulate(&world, &calm(), Some(30), &fired, &[], None);

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
        let plumes = Plumes::new(&explosion(), calm(), None);

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

        let first = Plumes::new(&world, calm(), None);
        let second = Plumes::new(&world, calm(), None);

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

        let plumes = Plumes::new(&world, calm(), None);

        assert!(!plumes.instances().is_empty(), "a chimney should preview");
    }

    /// **Neither FFT scene is a sea that cannot break, and the warning above must never
    /// say it is.** The gap this closed was a `peak_fold` of exactly zero on a spectrum
    /// body — a number that said nothing about the sea, so the warning had to be switched
    /// off on the one model a storm scene wants and an author with no spray in frame had
    /// nothing to read. `Ocean::peak_fold` answers instead, and this pins both halves:
    /// the ceiling is a real number well over `SPRAY_BREAK` on both scenes at every tick
    /// looked at (so no false warning), and it is measured off the scenes as they are
    /// loaded rather than off a hand-built body.
    ///
    /// Measured, at ticks 0 / 120 / 400 / 900:
    ///
    /// ```text
    /// ocean_fft.loom        1.376  1.363  1.288  1.217     Hs  5.897
    /// ocean_fft_storm.loom  1.507  1.508  1.459  1.378     Hs 12.286
    /// ```
    ///
    /// The wander is the tiles moving, not noise in the measurement: this ceiling is a
    /// max over the tiles that exist at one instant, which is exactly what separates it
    /// from the Gerstner `Σ Q·k·A` that is fixed for the run.
    #[test]
    fn both_fft_scenes_have_a_ceiling_far_over_the_spray_threshold() {
        for (scene, floor, roof) in
            [("ocean_fft.loom", 1.2_f32, 1.4_f32), ("ocean_fft_storm.loom", 1.3, 1.6)]
        {
            let source = std::fs::read_to_string(format!("../../assets/test/{scene}"))
                .expect("fixture");
            let world = World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"));
            let wind = crate::weather::wind_of_world(&world);
            let body = crate::weather::water_of(&world, &wind).expect("the scene has water");
            let mut sea = crate::weather::sea_of_body(&body, &wind).expect("a spectrum sea");
            assert!(sea.peak_fold().is_nan(), "{scene} answered before it was evolved");

            for tick in [0_u16, 120, 400, 900] {
                sea.evolve(f32::from(tick) / 60.0);
                let ceiling = sea.peak_fold();
                assert!(
                    ceiling > loom_water::spray::SPRAY_BREAK,
                    "{scene} at tick {tick} reads a ceiling of {ceiling}, under \
                     SPRAY_BREAK — the warning would tell an author this sea cannot break"
                );
                assert!(
                    (floor..roof).contains(&ceiling),
                    "{scene} at tick {tick} folds at most {ceiling}, outside {floor}..{roof}"
                );
            }

            // And the other side of the warning, through the function that prints it: the
            // same scene under a wind too light to break throws nothing, and the arm that
            // measures why is the one this walks. Not asserted on the log — the store is
            // global and the `Once` is per process, so a test that read it would race
            // every other test in the binary; the claim the message makes is asserted in
            // `loom_water::spray`'s own `the_cascade_ceiling_bounds_every_sample_and_
            // predicts_a_dry_sea`, which is where the Gerstner half is asserted too.
            //
            // **The swell goes with the wind, or the control is not a control.** These
            // scenes author a 420 km / 900 km swell that carries its own `u10` and takes
            // nothing from the argument below — leave it on and a 2 m/s wind still folds
            // at 0.891, because the swell is doing the folding.
            let gentle = loom_scene::components::WaterBody {
                spray: 8.0,
                swell: None,
                fetch: Some(50_000.0),
                ..body.clone()
            };
            // **1.0 m/s, and it was 2.0 until `SPRAY_BREAK` moved to 0.24.** A
            // 2 m/s cascade's ceiling is 0.2633 — under the old 0.33 and over
            // the new gate, so the final assertion below stopped holding while
            // the loop above it still passed. That is the assertion doing its
            // job: it is there to say this control is *provably* dry rather
            // than dry by luck over 24 ticks, and a control that only just
            // clears the gate is not one. At 1.0 m/s the ceiling is 0.0773.
            let mut calm_sea =
                loom_water::ocean::Ocean::for_body(&gentle, 1.0, [1.0, 0.0]).expect("a cascade");
            let deep = |_x: f32, _z: f32| -1000.0_f32;
            for tick in 0..24_u16 {
                let t = f32::from(tick) / 60.0;
                calm_sea.evolve(t);
                if tick % 8 == 0 {
                    calm_sea.keep();
                }
                assert!(
                    spray(&world, &gentle, Some(&calm_sea), &deep, [0.0, 4.0, 0.0], t).is_empty(),
                    "a sea folding at most {} threw spray",
                    calm_sea.peak_fold()
                );
            }
            assert!(
                calm_sea.peak_fold() < loom_water::spray::SPRAY_BREAK,
                "the gentle control is not gentle: {}",
                calm_sea.peak_fold()
            );
        }
    }

    /// **`heave.loom` peaks where its header says it does, or the scene is a lie.**
    ///
    /// The whole point of that scene is that a *single* Gerstner wave makes the
    /// fold a clean sinusoid: the ceiling is `Q·k·A` exactly and the crest
    /// recurs on the wave's own period, so `--sim 405` can be aimed at the
    /// event. Every one of those claims is arithmetic on four authored numbers,
    /// and every one of them dies silently if somebody tidies `126.4661` to
    /// `126` — the picture still renders, the spray still fires somewhere, and
    /// only the tick in the header is wrong.
    ///
    /// So this pins the four numbers the header quotes, measured off the scene
    /// as it loads:
    ///
    /// ```text
    /// tick     135      405        350 / 460       349 / 461
    /// fold  -0.30000  +0.30000   over SPRAY_BREAK  under it
    /// ```
    ///
    /// `peak_fold` is asserted beside them because it is the *analytic* ceiling
    /// — `Σ Q·k·A`, true before a tick has run — and the sample at 405 is the
    /// surface actually reaching it. Two routes to one number is what says the
    /// header's arithmetic and the engine's agree.
    #[test]
    fn heave_peaks_at_the_tick_its_header_names() {
        const CEILING: f32 = 0.30;
        const CREST: u16 = 405;
        const TROUGH: u16 = 135;

        let source = std::fs::read_to_string("../../assets/test/heave.loom").expect("fixture");
        let world = World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"));
        let wind = crate::weather::wind_of_world(&world);
        let body = crate::weather::water_of(&world, &wind).expect("heave has water");

        assert_eq!(body.waves.waves.len(), 1, "heave is one wave or it is not this scene");
        let analytic = loom_water::spray::peak_fold(&body);
        assert!(
            (analytic - CEILING).abs() < 1e-4,
            "the authored wave folds at most {analytic}, not {CEILING}"
        );

        // The reference column is the camera's own, which is what makes the
        // droplet count peak on the same tick the fold does — see the header.
        let fold = |tick: u16| {
            loom_water::sample_water(
                &body,
                None,
                [0.0, 0.0],
                f32::from(tick) / 60.0,
                loom_voxel::heightfield::NO_GROUND,
                [0.0; 3],
                [0.0; 3],
            )
            .fold
        };

        assert!(
            (fold(CREST) - CEILING).abs() < 1e-4,
            "tick {CREST} reads {}, not the ceiling {CEILING}",
            fold(CREST)
        );
        assert!(
            (fold(TROUGH) + CEILING).abs() < 1e-4,
            "tick {TROUGH} reads {}, not the trough {}",
            fold(TROUGH),
            -CEILING
        );

        // And the gate is *swept*, which is the scene: 111 ticks of every 540
        // over `SPRAY_BREAK` and the rest under it. The edges are asserted one
        // tick either side, so a wave that got steeper or flatter fails here
        // rather than quietly widening the band the header counts.
        for on in [350_u16, CREST, 460] {
            assert!(
                fold(on) > loom_water::spray::SPRAY_BREAK,
                "tick {on} should be throwing and folds only {}",
                fold(on)
            );
        }
        for off in [349_u16, 461, TROUGH] {
            assert!(
                fold(off) <= loom_water::spray::SPRAY_BREAK,
                "tick {off} should be dry and folds {}",
                fold(off)
            );
        }
    }

    /// **A Beaufort-4 sea does not spit, and it is a real scene that says so.**
    ///
    /// `SPRAY_BREAK` is the whole population gate, so lowering it is a
    /// multiple rather than a nudge — `ocean_fft_storm` went from 147 droplets
    /// to 1,008 at one step of the sweep — and the failure mode of moving it
    /// too far is not a cost, it is a gentle sea throwing water. That is the
    /// same failure `weather::the_gentle_sea_still_foams_nowhere` guards for
    /// the foam gate, and this is the spray half of it.
    ///
    /// **`ocean_tropical.loom` and not a hand-built body.** Its wind is 6.6 m/s
    /// free-stream, which is `U10 = 5.99` — Beaufort 4, the sea the scene's own
    /// header calls a light chop. It authors no `spray`, so as it ships it is
    /// dry by the *authoring* switch and proves nothing about the threshold;
    /// this forces `spray` to the schema's ceiling of 8.0, which is the only
    /// way the gate under test is the one being measured.
    ///
    /// **Watched failing.** At `SPRAY_BREAK = 0.05` this scene throws 63
    /// droplets at tick 240 and the assertion names the ceiling it read.
    /// **A Beaufort-4 sea must not start spitting**, and the guard is a ratio
    /// rather than a zero, because a zero is not true of this sea and never
    /// was.
    ///
    /// `SPRAY_BREAK` is the whole population gate, so moving it is a multiple:
    /// `ocean_fft_storm` went 147 -> 1,008 droplets at `--sim 400` on the step
    /// this commit takes. The failure mode of going too far is not a cost, it
    /// is a gentle sea throwing water — the same failure
    /// `weather::the_gentle_sea_still_foams_nowhere` guards for the foam gate.
    ///
    /// **The obvious test is wrong and measuring it is what showed that.**
    /// `ocean_tropical` is Beaufort 4 (`Wind.speed = 6.6` free-stream, so
    /// `U10 = 5.99`) and it authors no `spray`, so as it ships it is dry by the
    /// *authoring* switch and proves nothing. Force `spray` to the schema's
    /// ceiling and its cascade ceiling is **0.6256** — over `SPRAY_BREAK` at
    /// 0.33 as well as at 0.24 — and it throws on 53 of 241 ticks *at the old
    /// threshold*. An `assert_eq!(thrown, 0)` here would have been red at HEAD.
    /// A short cascade band is steep at any wind; what a light wind buys is
    /// that very little of the surface is in it.
    ///
    /// So the property that is actually true, and actually the one worth
    /// keeping, is that the two seas stay far apart. Droplets summed over 241
    /// ticks from one eye, both at `spray = 8.0`, as a percentage of the
    /// storm's:
    ///
    /// ```text
    /// SPRAY_BREAK   0.33    0.24    0.20    0.16    0.13    0.10
    /// tropical %   0.116   0.940   2.354   6.270  11.677  19.532
    /// ```
    ///
    /// **2% is one step below what ships**, the same margin `13e824b` left
    /// itself on the foam pair. Watched failing at `SPRAY_BREAK = 0.20`, which
    /// reads 2.354%.
    ///
    /// `pool` and `mirrorpool` are the other end and they are exact: flat water
    /// has no fold at all, so no threshold this side of zero can make them
    /// throw.
    #[test]
    fn a_beaufort_four_sea_stays_a_hundred_times_drier_than_a_storm() {
        /// Percent of the storm's droplets a Beaufort-4 sea may throw.
        const CEILING: f64 = 2.0;

        // 241 ticks — four seconds, several crown lifetimes, and long enough
        // that a lull in either sea is averaged over rather than sampled.
        let thrown_by = |scene: &str| -> usize {
            let source = std::fs::read_to_string(format!("../../assets/test/{scene}.loom"))
                .expect("fixture");
            let world = World::from_scene(&loom_scene::Scene::parse(&source).expect("valid"));
            let wind = crate::weather::wind_of_world(&world);
            let body = loom_scene::components::WaterBody {
                // **Forced on, or the gate under test is the authoring switch
                // rather than the threshold.** Every one of these scenes but
                // the storm authors no spray.
                spray: 8.0,
                ..crate::weather::water_of(&world, &wind).expect("the scene has water")
            };
            let mut sea = crate::weather::sea_of_body(&body, &wind);
            let deep = |_x: f32, _z: f32| -1000.0_f32;
            (0..=240_u16)
                .map(|tick| {
                    let t = f32::from(tick) / 60.0;
                    if let Some(sea) = sea.as_mut() {
                        sea.evolve(t);
                        // The ring `spray` reads a droplet's birth instant out
                        // of is filled on `Sim::evolve_sea`'s stride and no
                        // other; skipping it measures a sea with no past.
                        if tick % 8 == 0 {
                            sea.keep();
                        }
                    }
                    spray(&world, &body, sea.as_ref(), &deep, [0.0, 3.0, 0.0], t).len()
                })
                .sum()
        };

        let storm = thrown_by("ocean_fft_storm");
        let gentle = thrown_by("ocean_tropical");
        assert!(storm > 0, "the storm threw nothing, so the ratio below means nothing");
        #[allow(clippy::cast_precision_loss)]
        let share = gentle as f64 * 100.0 / storm as f64;
        assert!(
            share < CEILING,
            "a Beaufort-4 sea threw {gentle} droplets against the storm's {storm} — \
             {share:.3}% of it, past the {CEILING}% this gate allows. SPRAY_BREAK is \
             {}, and it has been lowered far enough to flatten the difference between \
             a light chop and a storm.",
            loom_water::spray::SPRAY_BREAK
        );

        // Flat water is exact at any threshold, which is the other end of the
        // same guard and the one that cannot drift.
        for still in ["pool", "mirrorpool"] {
            assert_eq!(thrown_by(still), 0, "{still} threw spray");
        }
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
        let quiet = simulate(&sea(), &calm(), Some(60), &[], &[], None);

        assert!(
            quiet.is_empty(),
            "{} particles from a splash nobody made",
            quiet.len()
        );
        // And the same through the viewer's path, which builds its own state.
        assert!(Plumes::new(&sea(), calm(), None).instances().is_empty());
    }

    /// And the other half: it plays where the thing went in.
    #[test]
    fn a_splash_plays_where_something_entered_the_water() {
        let at = [3.0, 0.4, -6.0];

        let out = simulate(&sea(), &calm(), Some(20), &[], &[entry(10, at)], None);

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
            simulate(&world, &calm(), Some(11), &[], &[entry(10, at)], None)
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
            assert!(spray(&world, &body, None, &deep, eye, t).is_empty(), "tick {tick}");
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
                spray(&world, &body, None, &deep, eye, t).is_empty(),
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
                spray(&world, &body, None, &deep, eye, t).len()
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
                spray(&world, &body, None, &deep, eye, t)
            })
            .collect();
        let first = drops.first().expect("droplets");
        assert!(first.color[0] > 0.5 && first.color[2] > 0.5, "{first:?} is not spray-coloured");
    }

    /// **The jet reaches BOTH paths, and it arrives after the crown.**
    ///
    /// This is the test the `set_ripples` defect (ADR 0046 §7) says this
    /// repository owes every water effect: a feature wired into `simulate` and
    /// not into `Plumes` is present, tested, and invisible in the window the
    /// human actually watches. So both are asked, and both are asked for the
    /// thing that distinguishes the new anatomy from the old mushroom — water
    /// standing above the droplets' own ceiling, *and not there yet* six ticks
    /// in.
    ///
    /// `pool.loom` is the fixture because it authors no `ParticleEmitter` under
    /// its water, which is the branch the impact anatomy lives on.
    #[test]
    fn the_jet_reaches_both_paths_and_arrives_after_the_crown() {
        let world = {
            let source = std::fs::read_to_string("../../assets/test/pool.loom").expect("fixture");
            World::from_scene(&loom_scene::Scene::parse(&source).expect("valid scene"))
        };
        let at = [0.0, 0.0, 0.0];
        let splash = entry(0, at);
        let top = |ticks: u32| {
            let headless = simulate(&world, &calm(), Some(ticks), &[], &[splash], None);
            let mut plumes = Plumes::new(&world, calm(), None);
            plumes.splash(&world, splash);
            plumes.advance(ticks);
            let window = plumes.instances().to_vec();
            assert_eq!(
                headless.len(),
                window.len(),
                "the two paths drew different populations for one entry at tick {ticks}"
            );
            let high = |out: &[loom_render::ParticleInstance]| {
                out.iter().fold(0.0_f32, |best, p| best.max(p.position[1]))
            };
            let (a, b) = (high(&headless), high(&window));
            assert!((a - b).abs() < 1e-6, "headless topped out at {a} m, the window at {b} m");
            a
        };

        // The crown's own ceiling — the tallest droplet the rim can ever reach.
        let ceiling = (0..120)
            .flat_map(|step| {
                #[allow(clippy::cast_precision_loss)]
                loom_water::spray::crown(at, splash.speed, splash.radius, step as f32 * DT, 0)
            })
            .fold(0.0_f32, |best, d| best.max(d.position[1]));
        assert!(ceiling > 0.0, "the crown threw nothing to compare against");

        // Six ticks in — inside the crown, before the jet fires at ~13.6.
        assert!(
            top(6) < ceiling,
            "something was already above the crown's {ceiling} m ceiling at tick 6, so the \
             jet is firing with the rim rather than after it"
        );
        // Thirty ticks in — the jet is up and past the rim, on both paths. The
        // jet apex itself is at tick ~35 (0.59 s after the entry), which is
        // half a second after the crown; that gap is the anatomy.
        assert!(
            top(30) > ceiling,
            "neither path drew anything above the crown's {ceiling} m ceiling at tick 30"
        );
    }

    /// Nothing fired means nothing drawn, even in a scene that has a template.
    #[test]
    fn no_shot_means_no_fireball() {
        let world = range();

        let none = simulate(&world, &calm(), Some(30), &[], &[], None);
        let one = simulate(
            &world,
            &calm(),
            Some(30),
            &[(5, [0.0, 1.0, 0.0])],
            &[],
            None,
        );

        assert!(none.len() < one.len(), "firing must add particles");
    }
}
