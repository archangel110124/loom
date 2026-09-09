//! Play mode: the simulation running inside the editor.
//!
//! Unity's Play button, and the same discipline the headless `loom sim` path
//! has. **The simulation never sees the frame time.** It advances in whole
//! fixed ticks or not at all, so what the human watches in the window and what
//! the agent asserts on in `loom sim --assert` are the same run — which is the
//! only reason a determinism hash is worth anything (never-do #8, §7.5).
//!
//! Play mode never writes the scene file. Unity's oldest usability wound is
//! edits made in play mode quietly vanishing at Stop; here nothing is at risk
//! because nothing was written.

use loom_ecs::World;
use loom_physics::{Physics, RigidBodyHandle};
use loom_render::glam::{Mat4, Quat, Vec3};

/// The fixed tick. Simulation time is counted in these, never in seconds of
/// wall clock.
pub const TICK_SECONDS: f32 = 1.0 / 60.0;

/// Ticks between the cascade snapshots spray reads — [`loom_water::ocean::Ocean::keep`].
///
/// **The stride is here because it is a tick count and `loom_water` has no tick.** The
/// pair that matters is this and `ocean::KEPT`: `KEPT - 1` gaps of this many ticks must
/// span `spray::SPRAY_LIFETIME`, or a droplet still in the air asks for a surface the
/// ring has already dropped and is silently never thrown. Eight ticks x seven gaps is
/// 0.933 s over the 0.9 s needed — `the_sea_ring_spans_a_droplets_life` asserts it.
///
/// **Eight rather than one**, because a snapshot is the whole tile stack: three cascades
/// of eleven 128² tiles is 2.06 MB, so keeping every tick to reach back 0.9 s would be
/// 111 MB and eight of them is 16.5 MB.
///
/// What the stride costs is that a crown launches from the surface as it was up to
/// 0.133 s ago. Measured on `ocean_fft_storm.loom` at (3, −7) between ticks 400 and 408,
/// which is exactly one stride: the water there moved **0.34 m horizontally and 9 mm
/// vertically** — under a quarter of a `SPRAY_CELL`. The jitter that keeps crowns off a
/// metronome lives in `born` and is *not* snapped with the sample, so the crowns still
/// appear at their own moments; only the water they read is quantised.
const SEA_KEEP_TICKS: u64 = 8;

/// How far a goal must move before the route is worth recomputing, squared.
const REPLAN_DISTANCE: f32 = 1.5 * 1.5;
/// How close counts as having reached a waypoint, squared.
const ARRIVED: f32 = 0.6 * 0.6;

/// Distance ignoring height.
///
/// **Arriving at a waypoint is a horizontal question.** A route's points sit
/// on the floor and a character's position is its capsule's centre, most of a
/// metre above it — so a 3D comparison never reads as arrived, the route never
/// advances, and the character grinds against its first waypoint forever at a
/// fraction of its speed. It looks exactly like a movement bug and is not one.
fn squared_distance_flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dz) = (a[0] - b[0], a[2] - b[2]);
    dx * dx + dz * dz
}

/// A node's scene path, or empty when it has none.
fn path_of(world: &World, entity: loom_ecs::Entity) -> String {
    world.path(entity).unwrap_or_default().to_owned()
}

/// Event kinds the engine itself raises. Everything else is a game's own
/// vocabulary and is never named in here.
const BLAST: &str = "blast";
const DAMAGE: &str = "damage";
/// A body went into the water, and came back out of it.
///
/// **Two edges of one state, not two facts.** Both come out of the same
/// hysteretic flag, so neither can fire without the other having been true, and
/// the splash is the reaction to the first — which is why there is no separate
/// `splash` event to keep in step with this one.
const SUBMERGED: &str = "submerged";
const SURFACED: &str = "surfaced";
/// The surface broke — W9.
///
/// **Not the same event as [`SUBMERGED`], and that is the point.** That pair is
/// a hysteretic gameplay state: 0.6 of the body under before it counts as gone
/// in, 0.3 before it counts as out. A splash is the instant the surface parts,
/// which is the first tick any of the body is wet, and the two can be many
/// ticks apart or — on `assets/test/pool.loom` — never both happen at all. See
/// the trigger in `Sim::float` for the measurement.
///
/// Carries `speed` (positive downward) and `radius` (the body's waterplane
/// radius), which are what size the crown.
const SPLASH: &str = "splash";

/// How far a character's aim ray reaches, in metres.
///
/// Long enough to cross any blockout and short enough that a miss is a miss:
/// a ray with no limit would report an aim point kilometres away, and a script
/// that detonates at it would set off explosions in empty sky.
const AIM_RANGE: f32 = 250.0;

/// A physics world built from a scene, ready to be stepped.
pub struct Sim {
    physics: Physics,
    /// Where anyone can walk. Baked once, after the first step, because the
    /// tree the probes cast against does not exist before it.
    nav: Option<loom_physics::NavGrid>,
    /// Which character a human drives, as an index into `characters`.
    /// Everyone else hunts it; it hunts nobody.
    player: Option<usize>,
    /// Entities with a body, and the body they got.
    dynamic: Vec<(loom_ecs::Entity, RigidBodyHandle)>,
    /// Entities with a `CharacterController`, and their walking state.
    characters: Vec<Walker>,
    /// The scene's water, with its waves resolved — the same resolution the
    /// renderer does, so a crate floats on the sea it is drawn on.
    ///
    /// Resolved once at load, and re-resolved on the fixed tick when a mood
    /// stage ramps the wind. See [`Sim::reweather`].
    water: Option<loom_scene::components::WaterBody>,
    /// Whether [`Self::water`]'s waves came from the wind rather than the file.
    ///
    /// **Read once, before the resolution.** After `water_of` has run there is
    /// no way to tell a derived wave list from an authored one, and a weather
    /// ramp that overwrote an authored sea would silently delete the one thing
    /// a scene said explicitly about its water.
    derived_waves: bool,
    /// The ground under the scene's voxel terrain, baked once at load.
    ///
    /// **The same grid the water shader reads**, through the same
    /// `loom_voxel::heightfield` lookup — which is what makes the depth a crate
    /// floats in the depth the shoreline is drawn from. `None` when the scene
    /// has no voxel terrain, and every depth is then bottomless.
    ///
    /// **Baked at load and never rebaked**, which is the honest W6 answer to
    /// §5.2: carve the lake bed mid-run and the water this `Sim` sees is the
    /// water the bed used to make. Reloading the scene — which is what the
    /// viewer does on every file change, and what `loom explode` produces — is
    /// what picks it up. See `scene_terrain_field` for the cost.
    terrain: Option<loom_voxel::heightfield::HeightField>,
    /// The river's current over that bed, or `None` for water that goes
    /// nowhere — which is every ocean, every lake, and every scene authored
    /// before rivers existed.
    ///
    /// **Baked from `terrain` at load and never rebaked**, exactly as the bed
    /// is and with the same consequence: blow the bank out mid-run and the
    /// current is the one the old bank made. Reloading picks it up.
    flow: Option<loom_water::flow::FlowGrid>,
    /// The cinematic tier's volumetric solver — ADR 0053, ADR 0057.
    ///
    /// **`None` for every scene in this repository**, which is the acceptance
    /// criterion ADR 0053 §1 states rather than hopes for: a body that does not
    /// opt in never reaches this and every existing render is bit-identical.
    ///
    /// When it is `Some`, the deterministic buoyancy path below is switched off
    /// entirely for bodies inside the domain — one water, one force, no
    /// double-dipping.
    fluid: Option<loom_render::FluidSolver>,
    /// Scratch for the solver's per-tick inputs, so the fixed step does not
    /// allocate.
    fluid_solids: Vec<loom_render::FluidSolid>,
    fluid_probes: Vec<loom_render::FluidProbe>,
    /// Last tick's wetness at each probe, in probe order.
    ///
    /// **The whole submersion detector, and it needs no threshold.** A probe
    /// whose ring of columns found no water last tick and finds some this tick
    /// is a body entering the water at that point; the deterministic tier gets
    /// the same event out of the buoyancy solver's submerged fraction, and this
    /// is the volumetric answer to the same question.
    fluid_wetness: Vec<f32>,
    /// Milliseconds the device round trip cost, summed over the run, and the
    /// ticks that paid it — ADR 0053 §4 asks for this to be reported.
    fluid_cost: (f64, f64, u64),
    /// The tick [`Self::fluid_draw`] last marched, and what it produced.
    ///
    /// **The surface is a function of the tick and nothing else**, which is the
    /// whole tier's licence (ADR 0053 §6) and is therefore also permission to
    /// not compute it twice. At 140 fps the fixed step runs about every other
    /// frame, so half of all frames were re-reading a 128 KB density field and
    /// re-marching an unchanged lattice for a byte-identical answer.
    fluid_drawn: Option<(u64, Vec<loom_render::FluidVertex>, Vec<loom_render::ParticleInstance>)>,
    /// The interactive wavelet events — ADR 0056.
    ///
    /// **The one piece of stepped state this simulation owns besides `rapier`,
    /// and it is on the force path.** Present for every scene with water,
    /// because unlike the grid it replaced there is nothing to author: an
    /// event is a point, a time, a volume and a radius.
    ///
    /// **There is no domain, so there is nothing to anchor** — ADR 0045's trap
    /// clause is about a grid that could follow the camera, and this has no
    /// grid. `Sim` cannot see a camera at all either way.
    wavelets: loom_water::wavelet::WaveletField,
    /// The advected foam field — ADR 0055. Built for any water body: a foam
    /// field is the near field of whatever water is in shot.
    foam: Option<loom_water::foam::FoamField>,
    /// The FFT cascade — ADR 0076. `None` for a `gerstner` body, which is every
    /// scene but `ocean_fft`.
    ///
    /// **This is the ownership decision, and it is the thing a later reader will
    /// get wrong.** The simulation owns the ocean and nothing else builds one:
    /// [`Sim::evolve_sea`] fills its tiles once at the top of every fixed step,
    /// from `self.tick` alone, *before* buoyancy, the foam field or a script can
    /// read the surface. `loom water --at` and `water@x,z` clone this one rather
    /// than building a second (`weather::water_probe`), and the renderer will
    /// read this one too. A second ocean anywhere is a second opinion about
    /// where the surface is — the defect `loom_water`'s header opens on.
    /// `loom water --sim 0` is the one exception and has to be: it runs no
    /// simulation, so there is nothing to clone.
    ///
    /// **It anchors to the water body and never to the camera** — ADR 0045's
    /// stated trap, which binds here exactly as it binds [`Sim::foam`], and
    /// harder than it binds the spray: this surface pushes rapier bodies. There
    /// is in fact nothing to anchor. `Ocean::sample` wraps a world XZ into the
    /// patch, so the tiles are pinned to world coordinates and the sea is the
    /// same sea however far the eye is from it. `Sim` cannot see a camera at
    /// all, and it must stay that way.
    ///
    /// **Unlike the two fields above it, this is not state.** The tiles at tick
    /// `t` are a pure function of `(scene, t)` — ADR 0076's whole licence for
    /// putting an FFT ocean on the force path — so it is held per-tick for
    /// *cost*, not for memory, and rewinding it is free.
    sea: Option<loom_water::ocean::Ocean>,
    /// Bodies that float, in scene order, with their pontoons already in body
    /// space. **Order is fixed at load and never sorted**: the forces are
    /// summed as floats, and a different visiting order is a different number
    /// in the determinism hash.
    floating: Vec<Floating>,
    /// Bodies under thrust, in load order, with the force in their OWN frame.
    ///
    /// Separate from `floating` rather than a field on it, because a
    /// `Propulsion` is not about water: a body can be driven whether or not it
    /// floats, and folding it into `Floating` would make the component a
    /// silent no-op on anything that does not — which is the failure mode the
    /// component registry's own comments warn about.
    ///
    /// **Never sorted**, for the same reason `floating` is not.
    ///
    /// **Live, not authored-once.** A body's row is its thrust *now*: the
    /// authored `Propulsion` seeds it, and a helm script standing on that body
    /// overwrites it (`loom_script::Helm`). A body nobody drives keeps the row
    /// it loaded with, so every scene written before the helm existed behaves
    /// exactly as it did. A row appears the first time something asks for
    /// thrust, which is why a boat with no authored `Propulsion` can still be
    /// driven.
    propelled: Vec<(RigidBodyHandle, [f32; 3], [f32; 3])>,
    /// Water events raised this step and not yet collected: entries and exits.
    ///
    /// Held rather than pushed straight into the log because the log belongs to
    /// [`Runner`], and a `Sim` stepped on its own — `loom render --sim` with no
    /// scripts — still has to be steppable. Drained by whoever owns a log, and
    /// stamped with the tick then, so every event in it counts ticks the one
    /// way (see [`Sim::drain_water_events`]).
    water_events: Vec<loom_script::Event>,
    /// Ticks stepped so far. Water is a function of position and this, times
    /// the fixed timestep — never of a wall clock (never-do #8).
    tick: u64,
}

/// One buoyant body: what floats, where its spheres are, and how wet it is.
struct Floating {
    body: RigidBodyHandle,
    /// Scene path, resolved at load. The event log names nodes, and `float`
    /// runs without a `World` in hand.
    path: String,
    buoyancy: loom_scene::components::Buoyancy,
    /// The two thresholds the state flips at. See `loom_scene::Submersion`.
    submersion: loom_scene::components::Submersion,
    /// Fraction of this body under the surface, as of the last step. **Not a
    /// second opinion** — it is what `buoyancy::solve` divided the displaced
    /// volume by while computing the force it applied.
    fraction: f32,
    /// The debounced answer, which is what everything else reads.
    submerged: bool,
    /// Scratch, reused every tick so the solver's input does not allocate
    /// inside the fixed step.
    states: Vec<loom_water::buoyancy::PontoonState>,
}

/// One entry into the water, for whoever is drawing the splash.
///
/// Four numbers rather than two because the crown is sized by the impact: the
/// speed sets the droplets' velocity and how many bands there are, and the
/// radius is where the ring starts. Read off the event log, never tracked
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Splash {
    pub tick: u64,
    /// On the surface above the body — the wake included.
    pub at: [f32; 3],
    /// How hard it hit, m/s, positive downward.
    pub speed: f32,
    /// The waterplane radius of the thing that hit. See `waterplane_radius`.
    pub radius: f32,
}

/// How wide the hole this body punched in the surface is, in metres.
///
/// The furthest a pontoon reaches from the splash point, plus its own radius —
/// the pontoons are where the buoyancy solver thinks the body is, so they are
/// the right answer to "how much water did it move aside" without this file
/// needing to know what shape the collider is.
///
/// Never zero: a body with no pontoons at all still made a splash, and a crown
/// of radius zero is a spout out of a point.
fn waterplane_radius(floating: &Floating, at: [f32; 3]) -> f32 {
    floating
        .states
        .iter()
        .map(|s| (s.at[0] - at[0]).hypot(s.at[2] - at[2]) + s.radius)
        .fold(0.05_f32, f32::max)
}

/// Build the cinematic solver for a scene that opts into the tier.
///
/// **The domain is centred on the water node and sized by `extent`** — ADR
/// 0053 §5, anchored to sim state and never to the camera. `None` when the
/// device refuses, and the caller says so loudly rather than silently
/// rendering an empty tank.
fn build_fluid(
    world: &World,
    body: &loom_scene::components::WaterBody,
    obstacles: &[loom_render::FluidSolid],
) -> Option<loom_render::FluidSolver> {
    let extent = body.extent?;
    let centre = world
        .water_node()
        .and_then(|(entity, _)| world.global_transform(entity))
        .map_or([0.0, 0.0, 0.0], |g| [g.matrix[12], g.matrix[13], g.matrix[14]]);
    let (dims, cell) = loom_render::fluid_grid(extent);
    let origin = [
        centre[0] - extent[0] * 0.5,
        centre[1] - extent[1] * 0.5,
        centre[2] - extent[2] * 0.5,
    ];

    // The static bake, once: everything the scene calls scenery, rasterised at
    // cell centres. The tank's own walls are the domain boundary and need no
    // authoring — the solver treats everything outside the grid as solid.
    let mut solid = vec![0_u8; dims[0] * dims[1] * dims[2]];
    for obstacle in obstacles {
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    #[allow(clippy::cast_precision_loss)]
                    let p = [
                        origin[0] + (i as f32 + 0.5) * cell,
                        origin[1] + (j as f32 + 0.5) * cell,
                        origin[2] + (k as f32 + 0.5) * cell,
                    ];
                    let hit = if obstacle.ball {
                        let d = [
                            p[0] - obstacle.centre[0],
                            p[1] - obstacle.centre[1],
                            p[2] - obstacle.centre[2],
                        ];
                        d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2]))
                            <= obstacle.radius * obstacle.radius
                    } else {
                        (0..3).all(|a| (p[a] - obstacle.centre[a]).abs() <= obstacle.half[a])
                    };
                    if hit {
                        solid[i + dims[0] * (j + dims[1] * k)] = 1;
                    }
                }
            }
        }
    }

    match loom_render::FluidSolver::new(
        loom_render::FluidDomain {
            centre,
            extent,
            fill: body.fill,
            inflow: cascade_inflow(world, body, cell),
        },
        &solid,
    ) {
        Ok(solver) => Some(solver),
        Err(e) => {
            crate::log::warn(format!(
                "the cinematic tier needs a Vulkan compute device and there is none ({e}).                  ADR 0053: this scene's water is GPU-stateful by its own request, so there                  is no CPU fallback to demote to."
            ));
            None
        }
    }
}

/// The cascade pouring into a cinematic domain, as the solver wants it.
///
/// **The scene authors a `Cascade` and nothing else** — ADR 0057 addendum. The
/// lip, the brink depth and the exit speed all come out of
/// `loom_water::nappe::brink`, which is the same hydraulics slice 5's falling
/// sheet is drawn from and the same `resolve_cascade` the environment buffer
/// reads. There is deliberately no second place to author a flow rate: a
/// discharge in m²/s over a lip of a known length is a volume per second, and a
/// volume per second over a known particle volume is a particle count.
///
/// `None` when the scene has no cascade, which is a closed tank.
fn cascade_inflow(
    world: &World,
    body: &loom_scene::components::WaterBody,
    cell: f32,
) -> Option<loom_render::FluidInflow> {
    let c = crate::resolve_cascade(world, body.surface_height)?;
    let n = loom_water::nappe::brink(c.authored.discharge, c.authored.spread, c.authored.breakup);
    let length = (c.lip_b[0] - c.lip_a[0]).hypot(c.lip_b[2] - c.lip_a[2]);

    // **The lip's box, as an axis-aligned one.**
    //
    // `ponytail:` exact for a lip that runs along an axis, which is every
    // cascade in the repository; a diagonal lip gets a box wider than the sheet
    // it stands for and pours a slightly broader ribbon. The upgrade is to
    // place along the lip's own parameter and offset by its normal, which is
    // three lines here and none of them earn their place until a scene needs a
    // diagonal spout.
    let brink = n.brink_depth;
    let top = c.lip_a[1].max(c.lip_b[1]);
    let lo = [
        c.lip_a[0].min(c.lip_b[0]) - cell * 0.5 * c.fall[0].abs(),
        top - brink,
        c.lip_a[2].min(c.lip_b[2]) - cell * 0.5 * c.fall[1].abs(),
    ];
    let hi = [
        c.lip_a[0].max(c.lip_b[0]) + cell * 0.5 * c.fall[0].abs(),
        top,
        c.lip_a[2].max(c.lip_b[2]) + cell * 0.5 * c.fall[1].abs(),
    ];

    // Discharge to particles: `q·L` m³/s over one tick, divided by the volume
    // one particle stands for.
    #[allow(clippy::cast_precision_loss)]
    let particle_volume = cell * cell * cell / loom_render::FLUID_PER_CELL as f32;
    let rate = c.authored.discharge * length * TICK_SECONDS / particle_volume;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let per_tick = rate.round().max(0.0) as u32;
    if per_tick == 0 {
        crate::log::warn(format!(
            "the cascade over this cinematic water discharges {} m^2/s over {length:.2} m, \
             which is under one particle a tick at a {cell:.3} m cell — nothing will pour",
            c.authored.discharge
        ));
        return None;
    }
    Some(loom_render::FluidInflow {
        lo,
        hi,
        velocity: [c.fall[0] * n.exit_speed, 0.0, c.fall[1] * n.exit_speed],
        per_tick,
    })
}

/// One character, its velocity, and its script's memory.
///
/// The velocity lives here rather than in the physics character because it is
/// the *movement model's* state, not the collider's: the controller is handed
/// one each tick and reports what survived, and whoever chose it decides what
/// to do with that.
struct Walker {
    entity: loom_ecs::Entity,
    character: loom_physics::Character,
    velocity: [f32; 3],
    grounded: bool,
    memory: loom_script::ScriptMemory,
    /// Where this character last said it wanted to go, and the route there.
    ///
    /// Kept rather than recomputed every tick: A* over a grid is not free, and
    /// a goal that has not moved has not changed its answer. Re-planned when
    /// the goal moves off the end of the route or the route runs out.
    goal: Option<[f32; 3]>,
    route: Vec<[f32; 3]>,
}

/// Read a `CharacterController` component into the shape physics wants.
///
/// The file says *total standing height* because that is what a level is
/// measured in; the capsule wants the half-height of its straight section.
/// A radius at or above half the height would give a negative straight
/// section, so it degenerates to a sphere rather than to nonsense.
fn character_shape(component: &serde_json::Value) -> loom_physics::CharacterShape {
    // The component's own defaults, so an omitted field means the same thing
    // here as it does in the schema the agent reads.
    let default = loom_scene::components::CharacterController::default();
    #[allow(clippy::cast_possible_truncation)]
    let scalar = |name: &str, fallback: f32| {
        component
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .map_or(fallback, |v| v as f32)
    };

    let radius = scalar("radius", default.radius).max(1e-3);
    let height = scalar("height", default.height);
    loom_physics::CharacterShape {
        half_height: (height * 0.5 - radius).max(1e-3),
        radius,
        max_slope_degrees: scalar("max_slope_degrees", default.max_slope_degrees),
        step_height: scalar("step_height", default.step_height).max(0.0),
    }
}

/// Read a node's `Buoyancy`, with its pontoons resolved.
///
/// **An empty pontoon list is the documented default, made real.** Four
/// spheres at the body's horizontal corners displacing exactly its own volume
/// — which is both halves of the water doc's §5.6 trap answered by
/// construction: one pontoon would give no righting torque, and hand-placed
/// corner spheres large enough to look right displace several times what the
/// object does and make it float like a cork.
///
/// A sphere is the exception and gets one pontoon of its own radius, because a
/// sphere *is* a pontoon: four corner spheres would be a worse approximation of
/// it, and a body with no orientation has nothing to right.
fn buoyancy_of(
    world: &World,
    entity: loom_ecs::Entity,
    half: [f32; 3],
    ball: bool,
) -> Option<loom_scene::components::Buoyancy> {
    let mut component = serde_json::from_value::<loom_scene::components::Buoyancy>(
        world.buoyancy(entity)?.clone(),
    )
    .ok()?;
    if component.pontoons.is_empty() {
        component.pontoons = if ball {
            vec![loom_scene::components::Pontoon {
                offset: [0.0; 3],
                radius: half[0].max(half[1]).max(half[2]),
            }]
        } else {
            loom_water::buoyancy::default_pontoons(half)
        };
    }
    // The schema caps this and the validator enforces it; truncating here as
    // well means a hand-built component cannot make the cost unbounded.
    component
        .pontoons
        .truncate(loom_scene::components::MAX_PONTOONS);
    Some(component)
}

impl Sim {
    /// Build colliders and bodies for everything in `world`.
    ///
    /// Static geometry becomes a collider, dynamic nodes get a body. Both the
    /// editor's Play button and the headless `loom sim` come through here, so
    /// there is one description of what a scene means physically.
    #[must_use]
    pub fn new(world: &World) -> Self {
        let mut physics = Physics::new(TICK_SECONDS);
        let mut dynamic = Vec::new();
        let mut characters = Vec::new();
        let mut floating: Vec<Floating> = Vec::new();
        let mut propelled: Vec<(RigidBodyHandle, [f32; 3], [f32; 3])> = Vec::new();
        let mut obstacles: Vec<loom_render::FluidSolid> = Vec::new();
        // Colliders that belong to a body built later in this same loop.
        let mut pending: Vec<(loom_ecs::Entity, Mat4, [f32; 3])> = Vec::new();
        // Whether anything will ask about the bed. Read before the loop so the
        // voxel branch can decide to bake without a second pass over the world.
        let floats = world.water().is_some();
        let mut terrain = None;

        for entity in world.entities() {
            let Some(global) = world.global_transform(*entity) else {
                continue;
            };
            // **Everything from the same space.** Position used to come from
            // the global matrix while rotation and half-extents came from the
            // local transform, so an ancestor's rotation or scale never
            // reached the collider: a crate inside a turned rig collided
            // axis-aligned while it was drawn turned.
            let matrix = Mat4::from_cols_array(&global.matrix);
            let (world_scale, world_rotation, world_position) =
                matrix.to_scale_rotation_translation();
            let pos = world_position.to_array();
            let quat = [
                world_rotation.x,
                world_rotation.y,
                world_rotation.z,
                world_rotation.w,
            ];
            // A node with no local transform is not a thing physics can
            // place; the global above would be meaningless for it.
            if world.transform(*entity).is_none() {
                continue;
            }
            // An authored `BoxCollider` wins over the mesh's scale. It is a
            // documented, schema-validated component that the simulation used
            // to ignore entirely, so a node could declare one size and collide
            // as another with nothing reporting the discrepancy.
            //
            // Half-extents follow the *world* scale for the same reason: a
            // unit box scaled by an ancestor is drawn at the ancestor's size.
            //
            // **And they did not, which is the bug this line fixes.** The comment
            // above has said "world scale" since the component was wired, the
            // fallback arm below multiplies by it, and
            // `main.rs::rain_collision_field` — the *other* reader of the same
            // component — has always written `half[a] * scale[a]`. Only this arm
            // returned the authored numbers raw, so a collider was in local space
            // here and in world space everywhere else. `plough_cinematic`'s hull
            // is a 0.7 m crate authored as `scale 0.35` with `half_extents 1`: it
            // collided, and rasterised into the fluid's solid mask, as a 2 m box
            // — 23x the volume. The ramp under it was a 4 m cube instead of a
            // 6 x 0.3 x 2 slab.
            //
            // `half_extents` is therefore **local**, like every other authored
            // geometric field under a transform, and the unit box mesh has a
            // local half-extent of 1 — which is why `[1, 1, 1]` is the value that
            // makes a collider match its mesh. Three scenes had compensated by
            // authoring world numbers (`blockout`, `tower`, `dripping`); they are
            // re-authored in this commit so their *world* collider is unchanged.
            let half = world.collider_half_extents(*entity).map_or_else(
                || {
                    [
                        world_scale.x.abs().max(1e-3),
                        world_scale.y.abs().max(1e-3),
                        world_scale.z.abs().max(1e-3),
                    ]
                },
                |h| {
                    [
                        (h[0] * world_scale.x).abs().max(1e-3),
                        (h[1] * world_scale.y).abs().max(1e-3),
                        (h[2] * world_scale.z).abs().max(1e-3),
                    ]
                },
            );

            // **The collider follows the mesh.** Everything used to get a
            // cuboid, so a sphere rested on whichever face was down: tilted,
            // its centre settled at `radius * sqrt(2)` and the drawn sphere
            // sank into whatever it landed on. The simulation was
            // self-consistent and the picture was a lie.
            //
            // `ponytail:` keyed off the mesh alias rather than an explicit
            // collider component, because the shape a thing *is* is the shape
            // it should collide as, and scenes already say that. Add a
            // `SphereCollider` when something needs to differ from its mesh.
            // Every primitive that has a matching shape gets it. A cylinder
            // on a square footprint and a capsule that will not roll are the
            // same bug as the sphere-as-cuboid, just less obvious.
            let mesh = world.mesh_asset(*entity);
            let ball = mesh == Some("sphere");
            let round = matches!(mesh, Some("capsule" | "cylinder"));
            let capped = mesh == Some("capsule");
            // The enclosing radius, not the smallest: a non-uniformly scaled
            // sphere is an ellipsoid that no ball matches, and of the two
            // wrong answers a collider that contains the drawn shape is the
            // one that does not let geometry poke through.
            let radius = half[0].max(half[1]).max(half[2]);

            // A character is a capsule that walks, not scenery to collide
            // with. Taken before every other branch: falling through to the
            // static-box case would give it a box collider standing exactly
            // where it is, so the first thing it collided with would be
            // itself, and it would never move.
            if let Some(component) = world.character(*entity) {
                characters.push(Walker {
                    entity: *entity,
                    character: physics.add_character(pos, character_shape(component)),
                    velocity: [0.0; 3],
                    grounded: false,
                    memory: loom_script::ScriptMemory::default(),
                    goal: None,
                    route: Vec::new(),
                });
                continue;
            }

            // A voxel volume is terrain, not a box. Its recipe rides on the
            // world (never-do #11: the scene stores the op list, never the
            // voxels), so the field is rebuilt here and handed to parry as
            // solid cells. Static only: a destructible hillside is scenery,
            // and a trimesh on a dynamic body is never-do #10.
            if let Some(recipe) = world.voxel_recipe(*entity) {
                match crate::build_volume(recipe) {
                    Ok(volume) => {
                        let cells = volume.solid_cells();
                        // The whole transform, not just the position: the mesh
                        // is drawn with the node's global matrix, so a rotated
                        // or scaled volume whose collider is axis-aligned and
                        // unscaled is the same lie as a sphere colliding as a
                        // cube — the bug most of today's physics work was about.
                        let sized = [
                            volume.voxel_size * world_scale.x.abs(),
                            volume.voxel_size * world_scale.y.abs(),
                            volume.voxel_size * world_scale.z.abs(),
                        ];
                        // **The water's bed, off the same rebuilt volume.**
                        // Baked here rather than in a second pass because the
                        // volume is expensive to build and already in hand —
                        // and only when the scene has water, because the march
                        // is not free and nothing else in the sim reads it.
                        if floats {
                            terrain = Some(crate::terrain_field(
                                &volume,
                                [pos[0], pos[1], pos[2]],
                            ));
                        }
                        if physics
                            .add_static_voxels(pos, quat, sized, &cells)
                            .is_none()
                        {
                            crate::log::warn(format!(
                                "{}: voxel volume has no solid cells; nothing to collide with",
                                world.path(*entity).unwrap_or("?")
                            ));
                        }
                    }
                    Err(e) => crate::log::warn(format!(
                        "{}: {e}; terrain will not collide",
                        world.path(*entity).unwrap_or("?")
                    )),
                }
                continue;
            }

            if world.is_dynamic(*entity) {
                let mass = world.body_mass(*entity);
                let handle = if ball {
                    physics.add_ball_body(pos, quat, radius, mass)
                } else if round {
                    // Radius from the horizontal axes, height from Y — which is
                    // how both primitives are drawn.
                    physics.add_round_body(
                        pos,
                        quat,
                        half[1],
                        half[0].max(half[2]),
                        capped,
                        mass,
                    )
                } else {
                    physics.add_box_body(pos, quat, half, mass)
                };
                dynamic.push((*entity, handle));
                // Any dynamic body, not only a floating one.
                //
                // **A zero thrust is dropped here rather than applied as
                // zero**, so an inert `Propulsion` costs nothing per tick.
                // The default has to be free by *value* as well as by
                // absence, because `assets/prefabs/jib_vi.loom` carries an
                // inert one purely so an instance has something to override —
                // a prefab instance may only deviate through
                // `[node.overrides]`, and an override needs a target.
                //
                // **The stronger claim would be that this is required for
                // determinism, and it is not — measured.**
                // `apply_force_torque` ends in `apply_impulse(.., wake_up =
                // true)`, so it looked as though a zero vector could wake a
                // sleeping body and change the hash. Removing this guard
                // leaves `a_zero_thrust_is_free_by_value_and_not_only_by
                // _absence` passing, because a buoyant body is receiving a
                // force every tick and never sleeps. Kept for the cost and
                // because a `Propulsion` on a crate sitting on land is not
                // covered by that argument.
                if let Some(p) = world
                    .propulsion(*entity)
                    .and_then(|v| {
                        serde_json::from_value::<loom_scene::components::Propulsion>(v.clone()).ok()
                    })
                    .filter(|p| p.force != [0.0; 3])
                {
                    propelled.push((handle, p.force, [0.0; 3]));
                }
                if let Some(buoyancy) = buoyancy_of(world, *entity, half, ball) {
                    floating.push(Floating {
                        body: handle,
                        path: path_of(world, *entity),
                        // Absent means the defaults, like every other
                        // component: a body that floats has a submersion state
                        // whether or not anyone authored the thresholds, or
                        // half the systems that read it would have to handle
                        // "this body has no answer".
                        submersion: world
                            .submersion(*entity)
                            .and_then(|v| {
                                serde_json::from_value::<
                                    loom_scene::components::Submersion,
                                >(v.clone())
                                .ok()
                            })
                            .unwrap_or_default(),
                        fraction: 0.0,
                        submerged: false,
                        states: vec![
                            loom_water::buoyancy::PontoonState {
                                at: [0.0; 3],
                                radius: 0.0,
                                velocity: [0.0; 3],
                                ground: loom_voxel::heightfield::NO_GROUND,
                                flow: [0.0; 3],
                                wavelet: [0.0; 3],
                            };
                            buoyancy.pontoons.len()
                        ],
                        buoyancy,
                    });
                }
            } else if world.collider_half_extents(*entity).is_some()
                && dynamic_ancestor(world, *entity).is_some()
            {
                // **An authored collider under a dynamic body is a part of
                // that body**, not scenery and not a second body. A hull is
                // one box for its inertia and two dozen for the deck a player
                // walks on, and those are not the same shape.
                //
                // Keyed on an *explicitly authored* `BoxCollider` rather than
                // on `is_renderable`, so this arm is a guaranteed no-op on
                // every scene that predates it: the only mesh-less
                // `BoxCollider` in the repository is the one on this hull.
                // Relaxing it would also change what the cinematic solver
                // bakes as obstacles, and that is its own decision.
                //
                // Deferred rather than attached here, because the body it
                // belongs to may not have been built yet. `world.entities()`
                // is documented parents-first and this does not lean on it —
                // the failure if that ever stopped being true is a deck that
                // silently is not there, which is the whole class of fault
                // this arm exists to fix.
                pending.push((*entity, matrix, half));
            } else if world.is_renderable(*entity)
                && dynamic_ancestor(world, *entity).is_none()
                && !character_ancestor(world, *entity)
            {
                if ball {
                    physics.add_static_ball(pos, radius);
                } else if round {
                    physics.add_static_round(pos, quat, half[1], half[0].max(half[2]), capped);
                } else {
                    physics.add_static_box(pos, quat, half);
                }
                // And what the cinematic solver's obstacle bake is made of.
                // Collected here rather than walked again, because this branch
                // already knows which nodes are static scenery and what shape
                // each one is.
                obstacles.push(loom_render::FluidSolid {
                    centre: pos,
                    half,
                    radius,
                    ball,
                    velocity: [0.0; 3],
                });
            }
        }

        // **The deferred attachments, in scene order.** Collider handle
        // indices stay a deterministic function of the file: the loop above
        // inserts in `world.entities()` order and this inserts in the order it
        // collected, which is the same order.
        // **A body that authored a deck keeps its own box for mass alone.**
        // Done before any attachment so it can only ever demote the box the
        // scene authored on the body itself, never one of these. See
        // `Physics::demote_to_mass_only`: without it the hull's inertia brick
        // is still a solid lid a metre above the deck, and everything below is
        // unreachable — which is the bug the deck boxes exist to fix, still
        // there.
        let mut demoted: Vec<loom_ecs::Entity> = Vec::new();
        for (entity, ..) in &pending {
            let Some(ancestor) = dynamic_ancestor(world, *entity) else {
                continue;
            };
            if demoted.contains(&ancestor) {
                continue;
            }
            demoted.push(ancestor);
            if let Some((_, handle)) = dynamic.iter().find(|(e, _)| *e == ancestor) {
                physics.demote_to_mass_only(*handle);
            }
        }

        for (entity, matrix, half) in pending {
            let Some(ancestor) = dynamic_ancestor(world, entity) else {
                continue;
            };
            let Some((_, handle)) = dynamic.iter().find(|(e, _)| *e == ancestor) else {
                crate::log::warn(format!(
                    "{}: its dynamic ancestor has no body, so this collider is not there",
                    world.path(entity).unwrap_or("?")
                ));
                continue;
            };
            let Some(global) = world.global_transform(ancestor) else {
                continue;
            };
            // **The ancestor's rotation and translation, with its scale
            // dropped.** A rapier body has no scale, so a collider on it is
            // posed in world units; inverting the full matrix would divide the
            // offset by a scaled parent's scale and put the deck somewhere
            // else. The half-extents already carry world scale, from the same
            // place every other arm reads it.
            let (_, rotation, translation) =
                Mat4::from_cols_array(&global.matrix).to_scale_rotation_translation();
            let local =
                invertible_parent(Mat4::from_rotation_translation(rotation, translation)) * matrix;
            let (_, local_rotation, local_position) = local.to_scale_rotation_translation();
            physics.attach_box(
                *handle,
                local_position.to_array(),
                [
                    local_rotation.x,
                    local_rotation.y,
                    local_rotation.z,
                    local_rotation.w,
                ],
                half,
            );
        }

        let player = world.player_character().and_then(|entity| {
            characters.iter().position(|w| w.entity == entity)
        });

        // Provenance before resolution — see `Sim::derived_waves`.
        let derived_waves = world.water().is_some_and(|value| {
            serde_json::from_value::<loom_scene::components::WaterBody>(value.clone())
                .is_ok_and(|body| body.waves.waves.is_empty())
        });
        let water = crate::weather::water_of(world, &crate::weather::wind_of_world(world));
        // The current, off the bed that was just baked. Both halves have to be
        // in hand: a river with no terrain has nothing to run down, and terrain
        // with no river has nothing to say.
        let flow = terrain
            .as_ref()
            .zip(water.as_ref())
            .and_then(|(bed, water)| crate::river_flow(bed, water));
        if water.as_ref().is_some_and(|w| w.flow.is_some()) && flow.is_none() {
            crate::log::warn(
                "the WaterBody authors flow but the scene has no voxel terrain; \
                 a river's course comes from the ground and there is none"
                    .to_owned(),
            );
        }
        // **The foam field, anchored to the water node's own world position,
        // never the camera and never the bodies** (ADR 0045's trap clause,
        // which binds harder here because an assertion can read this field).
        let foam = water.as_ref().map(|body| {
            let centre = world
                .entities()
                .iter()
                .find(|e| world.is_water(**e))
                .and_then(|e| world.global_transform(*e))
                .map_or([0.0, 0.0], |g| [g.matrix[12], g.matrix[14]]);
            loom_water::foam::FoamField::new(centre, body)
        });
        // **The ocean, built once from the scene's own wind** — the same `u10` and the
        // same heading `water_of` derives the sixteen waves from, through the one
        // function that reads them (`weather::sea_of`), because two spellings of that
        // is how a scene comes to report one sea and float on another.
        let sea = crate::weather::sea_of(world, &crate::weather::wind_of_world(world));
        if water.is_none() && !floating.is_empty() {
            crate::log::warn(
                "the scene has Buoyancy but no WaterBody; nothing will float".to_owned(),
            );
        }
        // Submersion thresholds on a node that does not float would validate
        // cleanly and be read by nothing — the silent no-op the type registry
        // exists to stop, one level up. The fraction they threshold comes out
        // of the buoyancy solver, so no `Buoyancy` is no fraction.
        for entity in world.entities() {
            if world.submersion(*entity).is_some() && world.buoyancy(*entity).is_none() {
                crate::log::warn(format!(
                    "{}: Submersion needs Buoyancy on the same node; \
                     the submerged fraction comes from its pontoons",
                    world.path(*entity).unwrap_or("?")
                ));
            }
        }

        // **Joints last, because a constraint needs two bodies and the second
        // may be authored later in the file.** Everything above builds bodies;
        // nothing above can safely resolve `connected`.
        for entity in world.entities() {
            let Some(raw) = world.joint(*entity) else {
                continue;
            };
            let joint: loom_scene::components::Joint =
                match serde_json::from_value(raw.clone()) {
                    Ok(j) => j,
                    Err(e) => {
                        crate::log::warn(format!(
                            "{}: Joint is malformed ({e}); it constrains nothing",
                            world.path(*entity).unwrap_or("?")
                        ));
                        continue;
                    }
                };
            let Some((_, body1)) = dynamic.iter().find(|(e, _)| e == entity) else {
                crate::log::warn(format!(
                    "{}: Joint needs a dynamic RigidBody on its own node;                      a joint on a static body constrains nothing",
                    world.path(*entity).unwrap_or("?")
                ));
                continue;
            };
            let body2 = if joint.connected.is_empty() {
                // The world end of the constraint: a fixed body at this node's
                // own position, with no collider. See `add_world_anchor`.
                let at = world
                    .global_transform(*entity)
                    .map(|g| {
                        let m = Mat4::from_cols_array(&g.matrix);
                        m.to_scale_rotation_translation().2.to_array()
                    })
                    .unwrap_or([0.0, 0.0, 0.0]);
                physics.add_world_anchor(at)
            } else {
                let other = world
                    .entities()
                    .iter()
                    .find(|e| world.path(**e) == Some(joint.connected.as_str()))
                    .and_then(|e| dynamic.iter().find(|(d, _)| d == e));
                match other {
                    Some((_, handle)) => *handle,
                    None => {
                        crate::log::warn(format!(
                            "{}: Joint names `{}`, which has no dynamic body;                              it constrains nothing",
                            world.path(*entity).unwrap_or("?"),
                            joint.connected
                        ));
                        continue;
                    }
                }
            };
            physics.add_joint(*body1, body2, &joint);
        }

        let fluid = water
            .as_ref()
            .filter(|w| w.simulation == loom_scene::components::WaterSimTier::Cinematic)
            .and_then(|w| build_fluid(world, w, &obstacles));

        let mut sim = Self {
            physics,
            nav: None,
            player,
            dynamic,
            characters,
            fluid,
            fluid_solids: Vec::new(),
            fluid_probes: Vec::new(),
            fluid_wetness: Vec::new(),
            fluid_cost: (0.0, 0.0, 0),
            fluid_drawn: None,
            water,
            derived_waves,
            terrain,
            flow,
            wavelets: loom_water::wavelet::WaveletField::new(),
            foam,
            sea,
            floating,
            propelled,
            water_events: Vec::new(),
            tick: 0,
        };
        // The invariant `evolve_sea` states, established: the ocean holds tick 0's
        // instant before anything can read it, including a query on a run of no ticks.
        sim.evolve_sea();
        sim
    }

    /// Re-derive the sea from a ramped wind speed — the weather ladder.
    ///
    /// **On the fixed tick, from the tick's own `dread`, and nowhere else.**
    /// Waves push rigid bodies, so this is inside the deterministic core: it
    /// must be a pure function of (scene, tick) and it is, because `dread` is
    /// a fixed-tick script number. A ramp riding the viewer's frame clock
    /// would pass the image gate, pass `cargo xtask repeat`, and be wrong only
    /// in the window the human judges water in.
    ///
    /// A no-op for an authored wave list ([`Self::derived_waves`]) and for a
    /// scene with no water. Cost is sixteen `ln`/`sqrt` chains, once a tick.
    pub(crate) fn reweather(&mut self, world: &World, speed: f32) {
        if !self.derived_waves {
            return;
        }
        let Some(body) = self.water.as_mut() else {
            return;
        };
        let wind = crate::weather::wind_of_world_at(world, Some(speed));
        let params = wind.params();
        let u10 = wind.mean_speed_at(10.0);
        let direction = [params.get("dir_x"), params.get("dir_z")];
        // The same two arms `water_of` takes. Not a call to `water_of` itself:
        // that re-reads and re-deserialises the whole component every tick to
        // rebuild a struct this already holds, and the only field that moves
        // is this one.
        body.waves = match body.fetch {
            Some(fetch) => loom_water::spectrum::wave_set_fetch(u10, direction, fetch),
            None => loom_water::spectrum::wave_set(u10, direction),
        };
    }

    /// Advance the cinematic solver and let it push the bodies — ADR 0053.
    ///
    /// **The force law is not duplicated.** The pontoons, the spherical cap,
    /// the buoyant force, the damping and the drag are all
    /// `loom_water::buoyancy::solve`, exactly as the deterministic tier uses
    /// them. What changes is where the surface and the water's velocity come
    /// from: a GPU readback rather than a closed form, arriving through the
    /// same two `PontoonState` fields the wavelet field and the river already
    /// use. One force law, two sources of surface.
    ///
    /// **The tier does not leak** (ADR 0053 §5). A pontoon whose ring of
    /// columns found no water reads no surface and gets no force, so a body
    /// outside the domain is pushed by nothing at all rather than falling back
    /// to the analytic sea — which would be the same body forced by two waters.
    fn float_cinematic(&mut self) {
        let (Some(water), Some(solver)) = (self.water.as_ref(), self.fluid.as_mut()) else {
            return;
        };
        self.fluid_solids.clear();
        self.fluid_probes.clear();

        // The pontoons are what the fluid sees of a body, in both directions:
        // the solid it must flow round, and the place it is asked what it is
        // doing. One description, so the push and the lift cannot disagree
        // about where the body is.
        for floating in &mut self.floating {
            let (Some(position), Some(rotation)) = (
                self.physics.position(floating.body),
                self.physics.rotation_quat(floating.body),
            ) else {
                continue;
            };
            let rotation = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
            for (state, pontoon) in floating.states.iter_mut().zip(&floating.buoyancy.pontoons) {
                let at = Vec3::from_array(position) + rotation * Vec3::from_array(pontoon.offset);
                state.at = at.to_array();
                state.radius = pontoon.radius;
                state.velocity = self
                    .physics
                    .velocity_at_point(floating.body, state.at)
                    .unwrap_or([0.0; 3]);
                self.fluid_solids.push(loom_render::FluidSolid {
                    centre: state.at,
                    half: [pontoon.radius; 3],
                    radius: pontoon.radius,
                    ball: true,
                    velocity: state.velocity,
                });
                self.fluid_probes.push(loom_render::FluidProbe {
                    at: state.at,
                    radius: pontoon.radius,
                });
            }
        }

        let out = solver.step(&loom_render::FluidInputs {
            solids: &self.fluid_solids,
            probes: &self.fluid_probes,
        });
        self.fluid_cost.0 += out.step_ms;
        self.fluid_cost.1 += out.fence_wait_ms;
        self.fluid_cost.2 += 1;

        // A flat copy of the body: the solver's free surface is the whole
        // surface, so letting `sample_water` add a Gerstner swell on top would
        // be two seas at once.
        let mut flat = water.clone();
        flat.waves.waves.clear();
        // **And no cascade either.** The solver's free surface is the whole surface, so
        // an FFT sea summed on top would be two seas at once for exactly the reason
        // sixteen Gerstner waves would. `gerstner` with an empty wave list is the mirror
        // this wants; leaving `spectrum` here would reach the same still surface through
        // a `None` ocean, which is the right answer arrived at by accident.
        flat.wave_model = loom_scene::components::WaveModel::Gerstner;
        // **Whitewater, out of the readback and onto the CPU field the shader
        // already reads** — ADR 0057 addendum, and it needs no new machinery at
        // all. The probes come back as plain `f32` inside the fixed step, so
        // they are CPU facts in a tier that has already opted out of
        // portability; `loom_water::foam` advects and decays them exactly as it
        // does a deterministic hull's, and `waterFragmentMain` already floors
        // its coverage on `loom_foam_at`. What arrives is foam that *persists*
        // and *drifts* rather than a highlight welded to the body — the
        // distinction ADR 0055 was built around.
        self.fluid_wetness.resize(self.fluid_probes.len(), 0.0);
        let mut probe = 0;
        for floating in &mut self.floating {
            let Some(centre) = self.physics.centre_of_mass(floating.body) else {
                continue;
            };
            for state in &mut floating.states {
                let Some(read) = out.probes.get(probe) else { break };
                let was = self.fluid_wetness[probe];
                self.fluid_wetness[probe] = read.wetness;
                if let Some(field) = self.foam.as_mut() {
                    if read.wetness > 0.0 {
                        let v = read.velocity;
                        let rel = [
                            v[0] - state.velocity[0],
                            v[1] - state.velocity[1],
                            v[2] - state.velocity[2],
                        ];
                        // **Relative speed, not the water's own.** A hull
                        // drifting with the flow entrains nothing; what makes
                        // white water is shear between the body and what it is
                        // in. Two metres a second is fully white, which is the
                        // same scale `FOAM_HULL_SPEED` sets for the
                        // deterministic tier's hull wake.
                        let shear = rel[0].hypot(rel[1]).hypot(rel[2]);
                        field.deposit_disc(
                            [state.at[0], state.at[2]],
                            state.radius,
                            (shear * 0.5).clamp(0.0, 1.0),
                        );
                    }
                    // The entry. Full strength for the reason the deterministic
                    // impact deposit is full strength: an impact is the whitest
                    // foam a scene makes, and the field's decay is what takes it
                    // away rather than a smaller number here.
                    if was <= 0.0 && read.wetness > 0.0 {
                        field.deposit_disc(
                            [state.at[0], state.at[2]],
                            state.radius * 1.5,
                            loom_water::foam::FOAM_IMPACT,
                        );
                    }
                }
                // **And the crown, through slice 3's own event path.** A
                // submersion the *solver* resolved is a better trigger than the
                // analytic one it replaces — it fires where the water actually
                // is, including where the surface is heaped a hull's width above
                // the still level.
                if was <= 0.0 && read.wetness > 0.0 {
                    let speed = state.velocity[0]
                        .hypot(state.velocity[1])
                        .hypot(state.velocity[2]);
                    if speed > 0.5 {
                        self.water_events.push(loom_script::Event {
                            tick: 0,
                            kind: SPLASH.to_owned(),
                            at: [state.at[0], read.surface, state.at[2]],
                            node: floating.path.clone(),
                            values: [
                                ("speed".to_owned(), f64::from(speed)),
                                ("radius".to_owned(), f64::from(state.radius)),
                            ]
                            .into_iter()
                            .collect(),
                        });
                    }
                }
                probe += 1;
                state.ground = loom_voxel::heightfield::NO_GROUND;
                state.flow = read.velocity;
                // The readback, in the one field `sample_water` already adds to
                // the still-water level. Dry reads a surface far below the
                // body, which is what "no water here" means to the cap.
                state.wavelet = if read.wetness > 0.0 {
                    [read.surface - flat.surface_height, 0.0, 0.0]
                } else {
                    [-1.0e9, 0.0, 0.0]
                };
            }
            let wrench = loom_water::buoyancy::solve(
                &flat,
                None,
                &floating.buoyancy,
                &floating.states,
                centre,
                self.physics.mass(floating.body).unwrap_or(0.0),
                0.0,
            );
            self.physics
                .apply_force_torque(floating.body, wrench.force, wrench.torque);
            floating.fraction = wrench.submerged;
            floating.submerged = loom_water::buoyancy::is_submerged(
                floating.submerged,
                wrench.submerged,
                floating.submersion.enter,
                floating.submersion.exit,
            );
            // **This tier contributes no `Hull` at all, and that is the whole
            // of it.** It deposits per station a few dozen lines above, from
            // the solver's own shear, which is a better source than any
            // whole-body estimate. The entry that used to be here existed for
            // the advection drag in `foam::velocity_at`; that term is deleted,
            // so all it did afterwards was lay a second, body-scale disc — the
            // 244 m^2 pancake the deterministic tier stopped laying when it
            // grew stations. Removing it moves not one pixel of
            // `plough_cinematic`, `jib_vi_painted`, `slosh` or `fishing`.
        }

        // **The field is stepped here rather than in `float`**, which returned
        // before reaching it. Same call, same order, same one place: deposits
        // first, advection last.
        #[allow(clippy::cast_precision_loss)]
        let t = self.tick as f32 * TICK_SECONDS;
        if let Some(field) = self.foam.as_mut() {
            let terrain = self.terrain.as_ref();
            let ground = move |x: f32, z: f32| {
                terrain.map_or(loom_voxel::heightfield::NO_GROUND, |t| t.at(x, z))
            };
            field.step(&flat, None, t, self.flow.as_ref(), &[], &ground);
        }
    }

    /// What the device round trip cost this run: total ms, fence ms, ticks.
    #[must_use]
    pub fn fluid_cost(&self) -> (f64, f64, u64) {
        self.fluid_cost
    }

    /// Everything the cinematic tier draws this frame: the marched free
    /// surface, then the spray.
    ///
    /// **One call rather than two, because the order is load-bearing.** The
    /// march is what fills the density field the particle path culls its spray
    /// against; asking for the particles first draws every one of them,
    /// including the ones inside the mesh. That used to be a doc comment on two
    /// public methods, which is a rule a caller can read and then not follow —
    /// and `loom run` was about to become the second caller.
    ///
    /// The spray goes through the one particle renderer (ADR 0047's rule):
    /// there is no second particle path and no fluid draw path.
    ///
    /// Empty on both halves for every scene outside the tier.
    ///
    /// **It reports its three parts and not one total**, because one total is
    /// what sent a night at the wrong suspect. The caller's line used to read
    /// `27706 triangles, marched in 19.2 ms` over the whole call, and the word
    /// `marched` named the smallest of the three: the density readback and the
    /// instance readback were two thirds of it and were a shader waiting on
    /// PCIe. A timer whose label names one of the things it covers will be read
    /// as if it covered only that.
    pub fn fluid_draw(
        &mut self,
    ) -> (Vec<loom_render::FluidVertex>, Vec<loom_render::ParticleInstance>, FluidDrawCost) {
        let Some(solver) = self.fluid.as_mut() else {
            return (Vec::new(), Vec::new(), FluidDrawCost::default());
        };
        // Same tick, same answer — see `fluid_drawn`. The cost is reported as
        // zero rather than as last tick's, because a mean over frames drawn is
        // what the caller prints and repeating a number would inflate it.
        if let Some((tick, surface, spray)) = self.fluid_drawn.as_ref()
            && *tick == self.tick
        {
            return (surface.clone(), spray.clone(), FluidDrawCost::default());
        }
        let (dims, cell) = solver.grid();
        let (origin, _) = solver.bounds();
        // **Never-do #8 says simulation must not read the wall clock, and this
        // does not.** Nothing here feeds the solve; this is the presentation
        // half, and the clock is read for the same reason `FluidSolver::step`
        // reads it (ADR 0053 §4).
        #[allow(clippy::disallowed_methods)]
        let t0 = std::time::Instant::now();
        let density = solver.density();
        #[allow(clippy::disallowed_methods)]
        let t1 = std::time::Instant::now();
        let surface = loom_render::fluid_surface::march(density, dims, cell, origin);
        #[allow(clippy::disallowed_methods)]
        let t2 = std::time::Instant::now();
        // **The scene asks for none, so it gets none.** `WaterBody::spray`
        // defaults to 0.0 and documents "`0` is none", and this tier never
        // read it: every cinematic scene in the repository authors nothing and
        // was handed 65,536 instance slots and a 3.15 MB blocking readback per
        // frame drawn. The human's complaint about the tank being covered in
        // pale blue beads is a feature the file switched off.
        //
        // Skipping the call skips the dispatch, the copy and its fence, which
        // is where most of `fluid_draw`'s cost lived on a settled pool.
        //
        // **`spray` means something narrower here than it does on the
        // deterministic tier, and that drift is deliberate rather than
        // hidden**: there it is a multiplier on how many droplets a crest
        // throws, and here the count is a property of the solve, so this is an
        // on/off. See the field's own doc comment, which now says so.
        let spray = if self.water.as_ref().is_some_and(|w| w.spray > 0.0) {
            solver.instances()
        } else {
            Vec::new()
        };
        #[allow(clippy::disallowed_methods)]
        let cost = FluidDrawCost {
            density_ms: t1.duration_since(t0).as_secs_f64() * 1000.0,
            march_ms: t2.duration_since(t1).as_secs_f64() * 1000.0,
            spray_ms: t2.elapsed().as_secs_f64() * 1000.0,
        };
        self.fluid_drawn = Some((self.tick, surface.clone(), spray.clone()));
        (surface, spray, cost)
    }

    /// Apply this tick's buoyancy, before the solver runs.
    ///
    /// **Every rule that protects the determinism hash applies in here**, which
    /// is what makes buoyancy different from the water mesh and the grass:
    /// bodies in load order, pontoons in component order, one force and one
    /// torque per body applied once, and the surface read from
    /// `loom_water::sample_water` on the CPU — never from the GPU (§5.1).
    fn float(&mut self) {
        if self.fluid.is_some() {
            self.float_cinematic();
            return;
        }
        let Some(water) = &self.water else {
            return;
        };
        #[allow(clippy::cast_precision_loss)]
        let t = self.tick as f32 * TICK_SECONDS;

        // **Asked once, before any body is walked**, so every hull in the
        // scene sheds on the same ticks and the pool's contents do not depend
        // on load order.
        let shedding = self.wavelets.shedding();

        // What the foam field is told about the bodies this tick, gathered in
        // the same body order everything else here uses.
        let mut hulls: Vec<loom_water::foam::Hull> = Vec::new();

        for floating in &mut self.floating {
            let (Some(position), Some(rotation), Some(centre)) = (
                self.physics.position(floating.body),
                self.physics.rotation_quat(floating.body),
                self.physics.centre_of_mass(floating.body),
            ) else {
                continue;
            };
            let rotation = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);

            for (state, pontoon) in floating.states.iter_mut().zip(&floating.buoyancy.pontoons) {
                // The offset is in the body's own space and turns with it —
                // which is the whole mechanism by which a tilted crate is
                // righted. Applied at the node's origin rather than the centre
                // of mass, because that is the space the offsets were authored
                // in.
                let at = Vec3::from_array(position)
                    + rotation * Vec3::from_array(pontoon.offset);
                state.at = at.to_array();
                state.radius = pontoon.radius;
                state.velocity = self
                    .physics
                    .velocity_at_point(floating.body, state.at)
                    .unwrap_or([0.0; 3]);
                // The bed under this pontoon, from the shared height field.
                // Without it a crate in a foot of water rides the open sea's
                // swell, and the surface it is drawn on is not the one it is
                // floating on.
                state.ground = self
                    .terrain
                    .as_ref()
                    .map_or(loom_voxel::heightfield::NO_GROUND, |t| {
                        t.at(state.at[0], state.at[2])
                    });
                // The current here, off the grid derived from that same bed.
                // Per pontoon rather than per body: they are metres apart, so
                // a crate half in the channel and half in the slack water on
                // the inside of a bend is turned by the difference.
                state.flow = self
                    .flow
                    .as_ref()
                    .map_or([0.0; 3], |f| f.at(state.at[0], state.at[2]));
                // **The wake, read here and applied by the solver.** This is
                // W6's whole selling point in one line: what a *previous*
                // tick's disturbance left in the water is added to the Gerstner
                // surface inside `sample_water`, so a barrel rocks in a crate's
                // wake and `loom sim --assert` can see it happen.
                //
                // **The orbital velocity goes into `flow`, not beside it.**
                // `sample_water` has one water-velocity argument by design (see
                // `WaterSample::velocity`): a second would be a second force
                // path with its own coefficient, free to disagree with the
                // first about what the water is doing. Horizontal only, because
                // that is what a wave's orbital motion contributes to drag on a
                // floating body; the vertical half is already in the surface it
                // rides.
                let wavelet = self.wavelets.at(state.at[0], state.at[2], t);
                state.wavelet = wavelet.surface();
                state.flow[0] += wavelet.velocity[0];
                state.flow[2] += wavelet.velocity[1];
            }

            let wrench = loom_water::buoyancy::solve(
                water,
                self.sea.as_ref(),
                &floating.buoyancy,
                &floating.states,
                centre,
                self.physics.mass(floating.body).unwrap_or(0.0),
                t,
            );
            self.physics
                .apply_force_torque(floating.body, wrench.force, wrench.torque);

            // **And what the foam field sees of this body: one station per
            // pontoon.** It used to be one hull at the body origin with the
            // *waterplane* radius, and the comment here argued that per-pontoon
            // "would lay the same foam three times and mean nothing
            // different". That is true of a 0.7 m crate, whose pontoons are
            // closer together than one foam cell, and false of a nineteen-metre
            // boat: the single disc was 8.81 m in radius, 244 m^2, and on
            // `jib_vi_float.loom` it read 0.01881403848528862 at (0,0), (0,3)
            // and (8,0) — identical to sixteen digits, eight metres apart. A
            // hull-shaped thing laid a circle.
            //
            // Twelve stations of 1.093 m are 45 m^2, so this deposits *less*,
            // not more, and it deposits it where the hull actually is.
            //
            // **In authored pontoon order, never sorted** — same rule as
            // `floating` itself. A sort on a float key would be stable within a
            // run, so `cargo xtask repeat` would pass while the order silently
            // depended on geometry.
            if self.foam.is_some() {
                // The waterline centroid, which is what "outward" is measured
                // from. Cheaper and steadier than the body origin: it does not
                // move when a pontoon lifts clear of the water.
                #[allow(clippy::cast_precision_loss)]
                let n = floating.states.len() as f32;
                let (mut cx, mut cz) = (0.0_f32, 0.0_f32);
                for state in &floating.states {
                    cx += state.at[0];
                    cz += state.at[2];
                }
                let (cx, cz) = (cx / n, cz / n);
                for state in &floating.states {
                    // Outward waterline normal at this station.
                    let (dx, dz) = (state.at[0] - cx, state.at[2] - cz);
                    let reach = dx.hypot(dz);
                    // Through the water, not over the ground — a hull drifting
                    // with a current opens nothing.
                    let (ux, uz) = (
                        state.velocity[0] - state.flow[0],
                        state.velocity[2] - state.flow[2],
                    );
                    // **The dot product is the bow moustache, the dark flank
                    // and the trailing wake, all three.** A bow station's
                    // normal points into the direction of travel, so it opens
                    // water and foams; an amidships flank's normal is
                    // perpendicular to it and scores nearly zero; a stern
                    // station's is opposed and clamps to zero, because a stern
                    // closes water rather than opening it. What appears astern
                    // is what `foam::velocity_at` dragged back from the bow.
                    //
                    // A single centred pontoon has no outward direction at all
                    // — a sphere is its own waterline — so it falls back to
                    // plain speed, which is what this line did for every body
                    // before stations existed.
                    let opening = if reach > 1.0e-4 {
                        (ux * dx + uz * dz) / reach
                    } else {
                        ux.hypot(uz)
                    };
                    hulls.push(loom_water::foam::Hull {
                        at: state.at,
                        velocity: state.velocity,
                        opening: opening.max(0.0),
                        radius: state.radius,
                        wetted: wrench.submerged,
                    });
                }
            }

            // **The other half of the coupling: the body pushes back.** One
            // packet per hull per shedding tick — Havelock's construction, from
            // which the Kelvin wedge emerges rather than being drawn.
            //
            // The construction itself is `loom_water::wavelet::shed_source`,
            // which is where its arithmetic is documented and where the test
            // that a hull does not dig a hole under itself lives.
            if shedding {
                // **The source is read off the pontoons, in `loom_water`.**
                // Everything it needs — where each sphere is, how fast it is
                // moving, and what the water under it is doing — was filled in
                // by the loop above, so this path passes nothing twice and
                // authors nothing new. `None` is a body out of the water, with
                // no pontoons, or drifting slower than `SHED_MIN_SPEED`
                // *through the water*.
                //
                // Per *body* rather than per pontoon: the waterline is one
                // waterline, and twelve pontoons shedding twelve packets at
                // twelve points would be twelve wakes behind one boat — and
                // would evict the whole 128-slot ring in ten ticks.
                if let Some(shed) =
                    loom_water::wavelet::shed_source(&floating.states, wrench.submerged)
                {
                    self.wavelets.emit(
                        [position[0], position[2]],
                        t,
                        shed.volume,
                        shed.sigma,
                    );
                }
            }

            // **The surface broke this tick.** An edge on the fraction itself,
            // not on the hysteretic flag below — and the difference is the
            // whole of W9.
            //
            // The flag needs 0.6 of the body under before it counts as *gone
            // in*, which is the right question for gameplay and the wrong one
            // for a splash: the splash happens when the surface parts, which is
            // the first instant any of the body is wet. On `pool.loom` the two
            // are not even the same event — a sphere dropped from three metres
            // fired no `submerged` at all under the grid this replaced, because
            // its own dent tracked the body down and held the local fraction
            // under the threshold. So the hysteresis flag can never be the
            // splash trigger, and no amount of tuning either number would have
            // made it one.
            let entered = floating.fraction <= 0.0 && wrench.submerged > 0.0;
            // **The same number that scaled the force is the gameplay state.**
            // Not a second query: a body cannot be pushed up by water it is not
            // in, and this is what "one answer" means in practice.
            floating.fraction = wrench.submerged;
            let was = floating.submerged;
            floating.submerged = loom_water::buoyancy::is_submerged(
                was,
                wrench.submerged,
                floating.submersion.enter,
                floating.submersion.exit,
            );
            let crossed = floating.submerged != was;
            if entered || crossed {
                // Hoisted above both consumers, so the splash and the
                // submersion event cannot disagree about where the surface was.
                //
                // Where the splash goes: on the surface above the body, not at
                // its centre, which by then is under the water.
                let ground = self
                    .terrain
                    .as_ref()
                    .map_or(loom_voxel::heightfield::NO_GROUND, |t| {
                        t.at(position[0], position[2])
                    });
                // No current: this asks only where the surface is, so the splash
                // lands on the water rather than inside the body. A horizontal
                // current does not move it up or down.
                let surface = loom_water::sample_water(
                    water,
                    self.sea.as_ref(),
                    [position[0], position[2]],
                    t,
                    ground,
                    [0.0; 3],
                    // The wake included, so the splash lands on the surface
                    // the body actually broke rather than on the one under it.
                    self.wavelets.at(position[0], position[2], t).surface(),
                );
                let velocity = self
                    .physics
                    .velocity_at_point(floating.body, position)
                    .unwrap_or([0.0; 3]);
                let at = [position[0], surface.height, position[2]];
                // Positive going down, so a splash can be scaled by how hard the
                // thing hit rather than by which way it was travelling.
                let speed = -velocity[1];
                if crossed {
                    self.water_events.push(loom_script::Event {
                        // Stamped on the way out; see `drain_water_events`.
                        tick: 0,
                        kind: if floating.submerged { SUBMERGED } else { SURFACED }.to_owned(),
                        at,
                        node: floating.path.clone(),
                        values: [
                            ("fraction".to_owned(), f64::from(wrench.submerged)),
                            ("speed".to_owned(), f64::from(speed)),
                        ]
                        .into_iter()
                        .collect(),
                    });
                }
                // **The gate is what makes an entry an impact.** Without it a
                // body authored already floating splashes on its first solve:
                // `fraction` starts at zero, so the first time the solver runs
                // reads as a crossing. It is also barely moving, which is
                // exactly what tells the two apart.
                if entered && speed >= loom_water::spray::SPLASH_MIN_SPEED {
                    // **The impact rings the water** — ADR 0056. The same
                    // swept-volume construction the shed packets use, with the
                    // impact speed and the waterplane radius the crown already
                    // rises from: one rule for "water pushed aside", not two.
                    self.wavelets.emit(
                        [at[0], at[2]],
                        t,
                        loom_water::wavelet::swept_volume(
                            waterplane_radius(floating, position),
                            speed,
                        ),
                        waterplane_radius(floating, position),
                    );
                    // **The impact deposit** — a ring of white out to one and
                    // a half times the waterplane radius, which is where the
                    // cavity rim throws it. Full strength: an impact is the
                    // whitest foam a scene makes, and the field's decay is
                    // what takes it away rather than a smaller number here.
                    if let Some(field) = self.foam.as_mut() {
                        field.deposit_disc(
                            [at[0], at[2]],
                            1.5 * waterplane_radius(floating, position),
                            loom_water::foam::FOAM_IMPACT,
                        );
                    }
                    self.water_events.push(loom_script::Event {
                        tick: 0,
                        kind: SPLASH.to_owned(),
                        at,
                        node: floating.path.clone(),
                        values: [
                            ("speed".to_owned(), f64::from(speed)),
                            // The waterplane radius of the thing that hit, so
                            // the crown can rise from the rim of the cavity
                            // rather than from the body's centre.
                            ("radius".to_owned(), f64::from(waterplane_radius(floating, position))),
                        ]
                        .into_iter()
                        .collect(),
                    });
                }
            }
        }

        // **The pool is swept last, after every body has read it and shed into
        // it.** One place, one order: read -> force -> shed -> expire. Any fixed
        // order is deterministic; this one is the one that makes a wake a tick
        // old rather than a tick early, and it is the order the hash is pinned
        // against.
        self.wavelets.step(t);

        // **And the foam field last of all.** One order, fixed, and the order
        // the assertions are written against.
        if let Some(field) = self.foam.as_mut() {
            let terrain = self.terrain.as_ref();
            let ground = move |x: f32, z: f32| {
                terrain.map_or(loom_voxel::heightfield::NO_GROUND, |t| t.at(x, z))
            };
            field.step(water, self.sea.as_ref(), t, self.flow.as_ref(), &hulls, &ground);
        }
    }

    /// This tick's wavelet events, for whoever is drawing the surface.
    ///
    /// **Read-only, and one direction only.** The CPU pool is authoritative;
    /// the renderer is handed a copy of it and never writes one back (ADR 0045
    /// clause 2). An empty pool is water nothing has touched, and the shader
    /// then returns zero from every lookup — the plain Gerstner surface, bit
    /// for bit.
    #[must_use]
    pub fn wavelets(&self) -> &loom_water::wavelet::WaveletField {
        &self.wavelets
    }

    /// This tick's foam field, for whoever is drawing the surface or asserting
    /// on it. Read-only and one direction only, exactly as the event pool is.
    #[must_use]
    pub fn foam(&self) -> Option<&loom_water::foam::FoamField> {
        self.foam.as_ref()
    }

    /// Water entries and exits since the last call, stamped with `tick`.
    ///
    /// The stamp happens here rather than in `float` because `Sim` counts its
    /// own ticks from zero while a [`Runner`] counts from one, and an event log
    /// whose ticks come from two clocks is a replay that cannot be compared
    /// against another run.
    fn drain_water_events(&mut self, tick: u64) -> Vec<loom_script::Event> {
        self.water_events
            .drain(..)
            .map(|mut event| {
                event.tick = tick;
                event
            })
            .collect()
    }

    /// Every floating body, how much of it is under, and whether that counts.
    ///
    /// In load order, like everything else the simulation iterates.
    #[must_use]
    pub fn submersion(&self) -> Vec<(String, f32, bool)> {
        self.floating
            .iter()
            .map(|f| (f.path.clone(), f.fraction, f.submerged))
            .collect()
    }

    /// Whether a point is under the water — the listener's question.
    ///
    /// Same water body, same clock and same bed as the buoyancy solver, so the
    /// tick a crate's deck goes under is the tick the sound goes muffled.
    #[must_use]
    pub fn submerged_at(&self, at: [f32; 3]) -> bool {
        let Some(water) = &self.water else {
            return false;
        };
        #[allow(clippy::cast_precision_loss)]
        let t = self.tick as f32 * TICK_SECONDS;
        let ground = self
            .terrain
            .as_ref()
            .map_or(loom_voxel::heightfield::NO_GROUND, |g| g.at(at[0], at[2]));
        let wavelet = self.wavelets.at(at[0], at[2], t).surface();
        loom_water::buoyancy::submersion_at(water, self.sea.as_ref(), at, 0.0, t, ground, wavelet)
            > 0.5
    }

    /// Apply every live thrust, in load order, once per fixed step.
    ///
    /// Both vectors are in the body's own frame and are rotated by the body's
    /// current orientation, so a hull that yaws pushes the way its bow now
    /// points rather than the way it was pointing when the scene was written.
    ///
    /// **The torque is how a boat steers**, and it is the second half of the
    /// sentence this comment used to end on: "steering is a script writing the
    /// vector rather than a second authored field". `Propulsion` still authors
    /// no torque — a shove is a shove — and the only thing that can put one
    /// here is [`loom_script::Helm`], from a script standing on the body.
    fn propel(&mut self) {
        for (body, force, torque) in &self.propelled {
            let Some(r) = self.physics.rotation_quat(*body) else {
                continue;
            };
            let rotation = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
            let push = rotation * Vec3::from_array(*force);
            let twist = rotation * Vec3::from_array(*torque);
            self.physics
                .apply_force_torque(*body, push.to_array(), twist.to_array());
        }
    }

    /// Set a body's live thrust, replacing whatever it had.
    ///
    /// Appends a row for a body that had none, so a hull with no authored
    /// `Propulsion` — which is every `jib_vi` but `jib_vi_underway` — becomes
    /// drivable without the scene declaring an inert component first.
    fn set_thrust(
        propelled: &mut Vec<(RigidBodyHandle, [f32; 3], [f32; 3])>,
        body: RigidBodyHandle,
        helm: loom_script::Helm,
    ) {
        if let Some(row) = propelled.iter_mut().find(|(b, ..)| *b == body) {
            row.1 = helm.force;
            row.2 = helm.torque;
        } else {
            propelled.push((body, helm.force, helm.torque));
        }
    }

    /// Fill the FFT cascade's tiles for the tick `self.tick` now names — ADR 0076.
    ///
    /// **The invariant: the ocean always holds `self.tick × TICK_SECONDS`.** It is
    /// established at construction and restored at the end of every fixed step, so the
    /// surface is filled *before* anything in a tick reads it and is still that tick's
    /// surface after [`Self::step`] returns — which is what `loom water --at --sim N` and
    /// a `water@` assertion clone. The tiles are one instant and carry no clock a caller
    /// could check against, so `sample_water` asserts the `t` it was asked for against
    /// the `t` they hold; that is what makes this invariant checkable rather than a
    /// convention.
    ///
    /// **Once per tick and inside the fixed step**, like the wavelet pool's sweep and the
    /// foam field's advection, and for the same reason: a surface that produces a force
    /// is filled on the simulation's clock or not at all.
    ///
    /// **`t` comes from `self.tick` and nothing else** (never-do #8). Evolving is a pure
    /// function of it — nothing accumulates — so a rewind costs exactly what a step
    /// forward does, which is the property `loom render --sim N` rests on.
    ///
    /// Free for every scene without a `spectrum` body, which is all of them but
    /// `ocean_fft.loom` and `ocean_fft_storm.loom`.
    fn evolve_sea(&mut self) {
        // **Kept on a tick count, so the ring is a function of the tick and nothing
        // else** — which is what makes `--sim N` in one jump and stepping to N leave the
        // same spray in the air. Decided before the borrow because it reads `water`.
        //
        // **And only for a scene that authors spray**, which is the only reader: an
        // `ocean_fft.loom` that draws no droplets keeps no tiles and grows by nothing.
        let keep = self.tick.is_multiple_of(SEA_KEEP_TICKS)
            && self.water.as_ref().is_some_and(|w| w.spray > 0.0);
        if let Some(sea) = self.sea.as_mut() {
            #[allow(clippy::cast_precision_loss)]
            sea.evolve(self.tick as f32 * TICK_SECONDS);
            if keep {
                sea.keep();
            }
        }
    }

    /// The tick's cascade, for whoever is drawing or asserting about the surface.
    ///
    /// Read-only and one direction only, exactly as [`Self::wavelets`] and [`Self::foam`]
    /// are: the simulation owns the ocean and evolves it, and a caller that wants the sea
    /// at another instant is asking a question only the simulation can answer.
    #[must_use]
    pub fn sea(&self) -> Option<&loom_water::ocean::Ocean> {
        self.sea.as_ref()
    }

    /// The scene's water body with its wave set resolved, as this run holds it.
    ///
    /// **Handed out rather than re-read from the world**, because
    /// [`Sim::reweather`] moves it: a scene whose `Environment.stages` ramp the
    /// wind has a different wave set here than `weather::water_of` would build
    /// off the file, and that difference is the whole ladder.
    #[must_use]
    pub fn water(&self) -> Option<&loom_scene::components::WaterBody> {
        self.water.as_ref()
    }

    /// Advance whole ticks.
    pub fn step(&mut self, ticks: u32) {
        for _ in 0..ticks {
            // Forces first, inside the same fixed step, before the solver runs
            // — a force applied after `step` would take effect a tick late and
            // the buoyancy would visibly lag the surface.
            //
            // **Thrust before buoyancy**, and outside `float`: `float` returns
            // early on a scene with no water and on the cinematic tier, and a
            // body under power must be driven on both.
            self.propel();
            self.float();
            self.physics.step();
            self.tick += 1;
            // **The sea last, for the tick just entered.** See `evolve_sea`: the
            // invariant is that the tiles always hold `self.tick`'s instant, which makes
            // them ready for the next iteration's forces and correct for every query
            // made after this call returns.
            self.evolve_sea();
        }
        // Baked after the first step, once, for the reason every query here
        // has the same caveat: the broad-phase tree is built during the step,
        // and probing before it finds no floor anywhere.
        //
        // `ponytail:` never rebuilt. A level whose walls move during play
        // would route against the old one — a real limitation, and cheap to
        // fix when something actually moves a wall, by re-baking on the
        // transaction that did it.
        if self.nav.is_none() && !self.characters.is_empty() {
            self.nav = Some(loom_physics::NavGrid::bake(
                &self.physics,
                [-64.0, -64.0],
                [64.0, 64.0],
                0.5,
                200.0,
            ));
        }
    }

    #[must_use]
    pub fn character_count(&self) -> usize {
        self.characters.len()
    }

    /// Advance every character by one tick and write where they ended up.
    ///
    /// `velocity_for` is the movement model — normally a script. It is given
    /// the character's state and its own persistent memory, and answers with
    /// the velocity it wants for this tick. Everything after that is
    /// collision: the capsule is swept, slid along what it hits, stepped up
    /// small ledges, and the velocity that *survived* is what the model sees
    /// next tick.
    ///
    /// Call after [`Self::step`]. The broad-phase tree characters collide
    /// against is built during the step, so a character moved before the
    /// first one falls through the floor.
    ///
    /// # Errors
    /// Whatever `velocity_for` returns. A failing script stops the tick
    /// rather than being skipped: a movement model that threw is not a model
    /// that meant "stand still", and silently freezing the character would
    /// hide the error behind a plausible picture.
    pub fn drive_characters<E>(
        &mut self,
        world: &mut World,
        tick: u64,
        input: loom_script::Motion,
        // What each other character is being asked to do — ADR 0091. Empty
        // in a single-player game.
        driven: &std::collections::BTreeMap<usize, loom_script::Motion>,
        mut velocity_for: impl FnMut(
            loom_ecs::Entity,
            &loom_script::Motion,
            &mut loom_script::ScriptMemory,
        ) -> Result<loom_script::Motive, E>,
    ) -> Result<(Vec<loom_script::Detonation>, Vec<loom_script::Event>), E> {
        let mut detonations = Vec::new();
        let mut raised = Vec::new();

        // **Whoever carries the camera is the one a human drives.** That is
        // what "the target" means to everyone else: an enemy hunts the player,
        // and the player hunts nobody. Taking the first character instead made
        // it depend on file order, so adding an enemy above the player in the
        // scene swapped the two.
        let player_index = self.player;
        let player = player_index
            .and_then(|i| self.characters.get(i))
            .map(|w| (w.character.position(), w.character.body(), w.entity));
        let nav = self.nav.as_ref();

        for (index, walker) in self.characters.iter_mut().enumerate() {
            let target = if Some(index) == player_index { None } else { player };
            // State from the controller, input from whoever is driving this
            // one. **Co-op is the reason this is a lookup** — see ADR 0091.
            // A character nobody is driving gets `input`, which is what a
            // single-player scene has always done and is all-zero for a
            // character no human is possessing.
            let asked = driven.get(&index).unwrap_or(&input);
            let motion = loom_script::Motion {
                tick,
                dt: TICK_SECONDS,
                position: walker.character.position(),
                velocity: walker.velocity,
                grounded: walker.grounded,
                ..asked.clone()
            };

            // Where this character is looking, resolved here because a script
            // cannot cast a ray: the sandbox has no physics world, and giving
            // it one would hand agent-authored code a live borrow of the
            // simulation.
            //
            // From the eye rather than the capsule's centre, so a shot leaves
            // where the view does. The character is excluded by `raycast`
            // only if it is not in the way — it is, being the thing the ray
            // starts inside — so the ray starts a little in front of it.
            let eye = [
                motion.position[0] + input.forward[0] * (walker.character.shape().radius + 0.05),
                motion.position[1] + walker.character.shape().half_height,
                motion.position[2] + input.forward[2] * (walker.character.shape().radius + 0.05),
            ];
            // **Along the look direction, not the movement one.** `forward` is
            // flattened so that looking up does not walk you into the sky; a
            // shot fired along it can never hit anything above or below eye
            // level, which is most of a level.
            let hit = self.physics.raycast(eye, input.aim, AIM_RANGE);
            // What this character can perceive, and where its route goes.
            // Both are casts, and a script has no physics world.
            let can_see_target = target.is_some_and(|(at, body, _)| {
                // Neither capsule is cover: not theirs from them, and not
                // this character's own from itself.
                self.physics
                    .line_of_sight_between(eye, at, walker.character.body(), body)
            });
            let target_at = target.map_or(motion.position, |(at, ..)| at);
            let target_distance = target.map_or(f32::INFINITY, |(at, ..)| {
                let d = [
                    at[0] - motion.position[0],
                    at[1] - motion.position[1],
                    at[2] - motion.position[2],
                ];
                d.iter().map(|c| c * c).sum::<f32>().sqrt()
            });

            // What is under the feet. One extra shape cast per character per
            // tick, and it is the same probe the carry uses — reading it here
            // rather than threading it out of `move_character` keeps the two
            // answers to "am I aboard" from being two pieces of code.
            let stand = self.physics.support(&walker.character);
            let stand_node = stand
                .and_then(|body| self.dynamic.iter().find(|(_, h)| *h == body))
                .map(|(entity, _)| path_of(world, *entity))
                .unwrap_or_default();
            // Into the body's frame, by hand out of its pose, because that is
            // the frame a station on something that moves is fixed in. Same
            // quantity `--assert Node.local_y` reports.
            let stand_local = stand
                .and_then(|body| {
                    let r = self.physics.rotation_quat(body)?;
                    let t = self.physics.position(body)?;
                    let at = Vec3::from_array(motion.position) - Vec3::from_array(t);
                    Some((Quat::from_xyzw(r[0], r[1], r[2], r[3]).inverse() * at).to_array())
                })
                .unwrap_or([0.0; 3]);

            let motion = loom_script::Motion {
                stand_node,
                stand_local,
                can_see_target,
                target_at,
                target_distance,
                target_node: target
                    .map(|(_, _, entity)| path_of(world, entity))
                    .unwrap_or_default(),
                path_next: walker.route.first().copied().unwrap_or(motion.position),
                path_found: !walker.route.is_empty(),
                aim_point: hit.map_or(
                    [
                        eye[0] + input.aim[0] * AIM_RANGE,
                        eye[1] + input.aim[1] * AIM_RANGE,
                        eye[2] + input.aim[2] * AIM_RANGE,
                    ],
                    |h| h.point,
                ),
                aim_distance: hit.map_or(AIM_RANGE, |h| h.distance),
                aim_hit: hit.is_some(),
                ..motion
            };

            let mut motive = velocity_for(walker.entity, &motion, &mut walker.memory)?;
            let velocity = motive.velocity;

            // Route to wherever it asked to go. Re-planned only when the goal
            // has actually moved or the route has run out — A* every tick for
            // every character is the cost that makes navigation expensive,
            // and a goal that has not moved has not changed its answer.
            if let Some(goal) = motive.goal {
                let moved = walker
                    .goal
                    .is_none_or(|old| squared_distance_flat(old, goal) > REPLAN_DISTANCE);
                if moved || walker.route.is_empty() {
                    walker.goal = Some(goal);
                    walker.route = nav.map_or_else(Vec::new, |grid| {
                        grid.path(walker.character.position(), goal, loom_physics::NavAgent::default())
                    });
                }
            } else {
                walker.goal = None;
                walker.route.clear();
            }
            // Arrived at the next waypoint, so aim at the one after it.
            if walker
                .route
                .first()
                .is_some_and(|step| {
                    squared_distance_flat(*step, walker.character.position()) < ARRIVED
                })
            {
                walker.route.remove(0);
            }
            if let Some(detonation) = motive.detonate {
                detonations.push(detonation);
            }
            // **The helm, and it can only reach the deck underfoot.** Applied
            // to `propelled` rather than to the body directly, so the thrust
            // goes through `propel` inside the next fixed step with everything
            // else — one place that turns a body-frame force into an impulse,
            // one visiting order in the determinism hash. The cost is that a
            // helm command takes effect on the following tick, which is 16 ms
            // and is below what a hull of 43.8 tonnes can express.
            if let (Some(helm), Some(body)) = (motive.helm, stand) {
                Self::set_thrust(&mut self.propelled, body, helm);
            }
            // Whatever the character's own script raised. Stamped with the
            // node it came from so a rule can tell which character shouted.
            for event in &mut motive.emitted {
                if event.node.is_empty() {
                    event.node = path_of(world, walker.entity);
                }
            }
            raised.append(&mut motive.emitted);

            let moved = self
                .physics
                .move_character(&mut walker.character, velocity, TICK_SECONDS);
            walker.velocity = moved.velocity;
            walker.grounded = moved.grounded;

            // World space out of physics, local space into the node — the
            // same conversion `write_back` does, and for the same reason: a
            // character parented to anything would otherwise be re-composed
            // at the parent's offset and drift by it every tick.
            let parent_inverse = world
                .parent(walker.entity)
                .and_then(|parent| world.global_transform(parent))
                .map_or(Mat4::IDENTITY, |g| {
                    invertible_parent(Mat4::from_cols_array(&g.matrix))
                });
            let local = parent_inverse.transform_point3(Vec3::from_array(moved.position));
            if let Some(transform) = world.transform_mut(walker.entity) {
                transform.pos = local.to_array();
            }
        }
        world.propagate_transforms();
        Ok((detonations, raised))
    }

    /// Copy body positions back onto the world's transforms.
    pub fn write_back(&self, world: &mut World) {
        for (entity, handle) in &self.dynamic {
            let (Some(pos), Some(quat)) = (
                self.physics.position(*handle),
                self.physics.rotation_quat(*handle),
            ) else {
                continue;
            };

            // **The solver works in world space; a node stores local.** This
            // used to write the world pose straight into the local slot, which
            // is only correct when the parent is identity. Under a parent
            // offset by ten metres the body was re-composed twenty metres out,
            // and walked one parent-offset further every tick.
            //
            // The parent's global is inverted rather than assumed, so a rig
            // that is moved, turned or scaled behaves the same as one at the
            // origin.
            let parent_inverse = world
                .parent(*entity)
                .and_then(|parent| world.global_transform(parent))
                .map_or(Mat4::IDENTITY, |g| {
                    invertible_parent(Mat4::from_cols_array(&g.matrix))
                });

            let body_world = Mat4::from_rotation_translation(
                Quat::from_xyzw(quat[0], quat[1], quat[2], quat[3]),
                Vec3::from_array(pos),
            );
            let local = parent_inverse * body_world;
            let (_, local_rotation, local_position) = local.to_scale_rotation_translation();

            if let Some(transform) = world.transform_mut(*entity) {
                transform.pos = local_position.to_array();
                // Rotation too, or a toppling crate slides instead of tipping
                // — the simulation would be right and the picture a lie.
                transform.rot_euler = loom_physics::euler_from_quat([
                    local_rotation.x,
                    local_rotation.y,
                    local_rotation.z,
                    local_rotation.w,
                ]);
                // Scale is authored, never simulated: the solver has no
                // opinion about it and overwriting it here would silently
                // resize whatever the physics touched.
            }
        }
        world.propagate_transforms();
    }

    /// Which characters an explosion at `centre` reaches, and how exposed
    /// each was. Cover counts, the same way it does for the shove.
    #[must_use]
    pub fn characters_in_blast(
        &self,
        centre: [f32; 3],
        radius: f32,
    ) -> Vec<(loom_ecs::Entity, [f32; 3], f32)> {
        let standing: Vec<(usize, [f32; 3], loom_physics::RigidBodyHandle)> = self
            .characters
            .iter()
            .enumerate()
            .map(|(index, walker)| {
                (index, walker.character.position(), walker.character.body())
            })
            .collect();
        self.physics
            .blast_exposure(centre, radius, &standing)
            .into_iter()
            .filter_map(|(index, exposure)| {
                let walker = self.characters.get(index)?;
                Some((walker.entity, walker.character.position(), exposure))
            })
            .collect()
    }

    /// Set off a blast in the simulated world.
    pub fn apply_blast(&mut self, centre: [f32; 3], radius: f32, impulse: f32) -> usize {
        self.physics.apply_blast(centre, radius, impulse)
    }

    /// The collision world itself.
    #[must_use]
    pub fn world(&self) -> &Physics {
        &self.physics
    }

    #[must_use]
    pub fn body_count(&self) -> usize {
        self.physics.body_count()
    }

    /// The determinism hash — the same number `loom sim` prints.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        self.physics.state_hash()
    }

    /// Everything the simulation carries that a save has to keep — ADR 0088.
    ///
    /// **Keyed by node path, not by handle.** A `RigidBodyHandle` is an index
    /// into a set built at load, so it is stable within one run and meaningless
    /// across two. A path is what the scene calls the thing, which is the only
    /// name a save and a fresh load can both resolve.
    #[must_use]
    pub fn save_state(&self, world: &loom_ecs::World) -> serde_json::Value {
        let mut bodies = serde_json::Map::new();
        for (entity, handle) in &self.dynamic {
            let (Some(path), Some(state)) =
                (world.path(*entity), self.physics.body_state(*handle))
            else {
                continue;
            };
            bodies.insert(path.to_owned(), serde_json::json!(state));
        }
        let mut memories = serde_json::Map::new();
        for walker in &self.characters {
            let Some(path) = world.path(walker.entity) else {
                continue;
            };
            // **A character is not in `dynamic` and has a body all the same.**
            // Saving only `dynamic` left every walker — including the player —
            // wherever the scene spawns them, which the state hash caught
            // immediately: 4d7dfc1d against ffc7c7d0 on a `deeper_demo` save
            // that looked complete.
            let body_pose = self.physics.body_state(walker.character.body()).map_or(
                serde_json::Value::Null,
                |state| serde_json::json!({ "position": state.position, "rotation": state.rotation }),
            );
            memories.insert(
                path.to_owned(),
                serde_json::json!({
                    "memory": walker.memory.to_json(),
                    // **Pose only: a kinematic body's velocity is derived, not
                    // owned.** rapier recomputes it each step from the pending
                    // target and ignores anything written to it, so keeping it
                    // here would put a number in the file that loading cannot
                    // reproduce — and it did: a game saved after it had ended
                    // never steps again, so the reloaded velocities stayed zero
                    // while the run they continued still held its last values.
                    "body": body_pose,
                    "velocity": walker.velocity,
                    "grounded": walker.grounded,
                    "char_pos": walker.character.position(),
                    "char_grounded": walker.character.is_grounded(),
                    "goal": walker.goal,
                    "route": walker.route,
                }),
            );
        }
        let (packed, shed_tick) = self.wavelets.snapshot();
        serde_json::json!({
            // **The sim's own tick, which is not the caller's.** Water is a
            // function of position and `self.tick * TICK_SECONDS`, and this
            // counter only ever counted up from zero — so a loaded game put its
            // boat on the wave phase of `t = 0` while the run it continued was
            // ten seconds into the swell. The boat lost 0.29 m/s in the first
            // tick after an otherwise exact load.
            "tick": self.tick,
            // **Live thrust, which is a tick behind by design.** The step runs
            // before the scripts that steer, so every step applies the helm
            // vector the *previous* tick wrote. A load that started from the
            // authored `Propulsion` dropped one tick of the propeller: 934 kN
            // on a 57 t hull, which came out as the boat being 0.27 m/s slow
            // one tick after an otherwise exact load.
            "propelled": self
                .propelled
                .iter()
                .filter_map(|(body, force, torque)| {
                    let entity = self.dynamic.iter().find(|(_, h)| h == body)?.0;
                    Some((world.path(entity)?.to_owned(), serde_json::json!([force, torque])))
                })
                .collect::<serde_json::Map<_, _>>(),
            "bodies": bodies,
            "characters": memories,
            "wavelets": { "events": packed, "sigma": self.wavelets.snapshot_sigma(), "shed": shed_tick },
            "floating": self
                .floating
                .iter()
                .map(|f| (f.path.clone(), serde_json::json!([f.fraction, f.submerged])))
                .collect::<serde_json::Map<_, _>>(),
        })
    }

    /// Put the simulation back where a save says it was.
    ///
    /// **A body the save does not name is left alone rather than reset.** A
    /// save written before a scene gained a crate should load into that scene
    /// with the crate where the scene puts it, not teleported to the origin —
    /// and a name the scene no longer has is skipped rather than being an
    /// error, because a scene may legitimately have lost a node since.
    pub fn restore_state(&mut self, world: &loom_ecs::World, value: &serde_json::Value) {
        if let Some(tick) = value.get("tick").and_then(serde_json::Value::as_u64) {
            self.tick = tick;
        }
        if let Some(saved) = value.get("propelled").and_then(serde_json::Value::as_object) {
            for (entity, body) in self.dynamic.clone() {
                let Some(pair) = world
                    .path(entity)
                    .and_then(|path| saved.get(path))
                    .and_then(serde_json::Value::as_array)
                else {
                    continue;
                };
                let vec3 = |i: usize| {
                    pair.get(i)
                        .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                        .unwrap_or([0.0; 3])
                };
                Self::set_thrust(
                    &mut self.propelled,
                    body,
                    loom_script::Helm { force: vec3(0), torque: vec3(1) },
                );
            }
        }
        if let Some(w) = value.get("wavelets") {
            let packed = w
                .get("events")
                .and_then(|v| serde_json::from_value::<Vec<[f32; 4]>>(v.clone()).ok())
                .unwrap_or_default();
            let sigma = w
                .get("sigma")
                .and_then(|v| serde_json::from_value::<Vec<f32>>(v.clone()).ok())
                .unwrap_or_default();
            let shed = w.get("shed").and_then(serde_json::Value::as_u64).unwrap_or(0);
            self.wavelets.restore(&packed, &sigma, u32::try_from(shed).unwrap_or(0));
        }
        if let Some(saved) = value.get("floating").and_then(serde_json::Value::as_object) {
            for floating in &mut self.floating {
                let Some(pair) = saved.get(&floating.path).and_then(serde_json::Value::as_array)
                else {
                    continue;
                };
                if let Some(f) = pair.first().and_then(serde_json::Value::as_f64) {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        floating.fraction = f as f32;
                    }
                }
                if let Some(s) = pair.get(1).and_then(serde_json::Value::as_bool) {
                    floating.submerged = s;
                }
            }
        }
        if let Some(bodies) = value.get("bodies").and_then(serde_json::Value::as_object) {
            for (entity, handle) in &self.dynamic {
                let Some(path) = world.path(*entity) else {
                    continue;
                };
                let Some(state) = bodies
                    .get(path)
                    .and_then(|v| serde_json::from_value::<loom_physics::BodyState>(v.clone()).ok())
                else {
                    continue;
                };
                self.physics.set_body_state(*handle, &state);
            }
        }
        if let Some(memories) = value
            .get("characters")
            .and_then(serde_json::Value::as_object)
        {
            for walker in &mut self.characters {
                let Some(path) = world.path(walker.entity) else {
                    continue;
                };
                let Some(saved) = memories.get(path) else {
                    continue;
                };
                if let Some(memory) = saved.get("memory") {
                    walker.memory.restore(memory);
                }
                if let Some(body) = saved.get("body") {
                    let pose = |key: &str| {
                        body.get(key)
                            .and_then(|v| serde_json::from_value::<Vec<f32>>(v.clone()).ok())
                    };
                    if let (Some(position), Some(rotation)) = (pose("position"), pose("rotation")) {
                        let state = loom_physics::BodyState {
                            position: [position[0], position[1], position[2]],
                            rotation: [rotation[0], rotation[1], rotation[2], rotation[3]],
                            linear: [0.0; 3],
                            angular: [0.0; 3],
                        };
                        self.physics.set_body_state(walker.character.body(), &state);
                    }
                }
                if let Some(v) = saved
                    .get("velocity")
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                {
                    walker.velocity = v;
                }
                if let Some(g) = saved.get("grounded").and_then(serde_json::Value::as_bool) {
                    walker.grounded = g;
                }
                // The character's own position, which is the authority the
                // body follows — see `Character::place`.
                if let Some(p) = saved
                    .get("char_pos")
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                {
                    let g = saved
                        .get("char_grounded")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    // The body is already back in place from `body` above; the
                    // character's position is where the next step must carry it.
                    walker.character.place(p, g);
                    self.physics.arm_kinematic(walker.character.body(), p);
                }
                // The route is a cache, but not a re-derivable one: a walker
                // partway along a path replans from where it stands, and the
                // replan is not the tail of the original route.
                if let Some(goal) = saved.get("goal") {
                    walker.goal = serde_json::from_value(goal.clone()).unwrap_or(None);
                }
                if let Some(route) = saved
                    .get("route")
                    .and_then(|v| serde_json::from_value::<Vec<[f32; 3]>>(v.clone()).ok())
                {
                    walker.route = route;
                }
            }
        }
    }
}

/// The inverse of a parent's global transform, or identity when it has none
/// that can be inverted.
///
/// A node scaled to zero on any axis — `scale = [1, 0, 1]`, which the format
/// permits — gives a singular matrix whose `inverse()` is all NaN. That NaN
/// then flows into the child's position and rotation, through
/// `propagate_transforms` into the whole subtree, and surfaces to the agent as
/// `"actual": null` on an assertion, which reads as "no such node" rather than
/// "your scene is degenerate".
pub(crate) fn invertible_parent(matrix: Mat4) -> Mat4 {
    // Invert, then ask whether the answer is usable — rather than guessing
    // from the determinant. A determinant threshold is scale-dependent: a node
    // uniformly scaled by 1e-5 has determinant 1e-15 and is perfectly
    // invertible, so a 1e-12 cutoff would have silently snapped a legitimately
    // tiny node back to its parent's origin. Testing the result is
    // scale-independent and tests the property that actually matters.
    let inverse = matrix.inverse();
    if inverse.is_finite() {
        inverse
    } else {
        Mat4::IDENTITY
    }
}

/// Which ancestor of this node is a dynamic body, if any.
///
/// A renderable child of a moving body used to get its own *static* collider,
/// frozen at the position it spawned in — an invisible wall left behind
/// wherever the parent started, that other bodies then collided with. The
/// child moves because its parent does; it is not scenery.
///
/// **Which one it is now matters as well as whether there is one.** An
/// explicitly authored `BoxCollider` under a dynamic node is a *part* of that
/// body — a deck plate on a hull — so the answer has to name the body the
/// collider gets attached to.
pub(crate) fn dynamic_ancestor(
    world: &World,
    entity: loom_ecs::Entity,
) -> Option<loom_ecs::Entity> {
    let mut current = world.parent(entity);
    // Bounded rather than `while let`: a malformed hierarchy with a cycle
    // would otherwise hang the editor on load, and the scene layer's cycle
    // check is one layer away from here.
    for _ in 0..64 {
        let node = current?;
        if world.is_dynamic(node) {
            return Some(node);
        }
        current = world.parent(node);
    }
    None
}

/// True when some node above this one is a `CharacterController`.
///
/// **A renderable under a character is his body, not scenery.** The character
/// node itself is already taken before every other branch — it becomes a
/// walking capsule, never a box, "or the first thing it collided with would be
/// itself". A *descendant* of it used to fall through to the static-box arm
/// anyway, which is the same bug one level down: an articulated player made of
/// separate rigid parts arrived as two dozen invisible boxes welded to the
/// spawn point, at their rest pose, never moving when the puppet animates, and
/// entering the cinematic water solver's obstacle bake. He walks into his own
/// chest.
///
/// A no-op on everything that predates it, and that is measured rather than
/// assumed: of the eleven scenes in `assets/` with a `CharacterController`,
/// none has a renderable descendant.
fn character_ancestor(world: &World, entity: loom_ecs::Entity) -> bool {
    let mut current = world.parent(entity);
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if world.character(node).is_some() {
            return true;
        }
        current = world.parent(node);
    }
    false
}

/// Gravity, and nothing else — what a character with no script does.
///
/// Deliberately not a walk. The movement model belongs in a script, and
/// inventing a default one in Rust would mean every character in every scene
/// silently inherits this file's opinion about acceleration and top speed.
///
/// **Grounded means stationary, not "coasting with no friction".** The
/// horizontal components used to be carried forward untouched, which is right
/// in the air and wrong on the floor: `move_character` returns the velocity
/// that *survived* the sweep, so a capsule standing on ground half a degree off
/// level keeps whatever `g·sin(theta)` it picked up and creeps along it
/// forever. On flat ground that term is identically zero and nothing changes;
/// on a boat it is the difference between standing on the deck and sliding off
/// it. The component's own doc says a scriptless character "falls, and does
/// nothing else", and creeping is something else.
///
/// `ponytail:` no friction model, because a character that is meant to slide
/// wants a script anyway — and a script is where the coefficient would have to
/// be authored. Add one the first time a scene needs ice.
fn fall_only(motion: &loom_script::Motion) -> [f32; 3] {
    if motion.grounded {
        return [0.0, 0.0, 0.0];
    }
    [
        motion.velocity[0],
        motion.velocity[1] - 9.81 * motion.dt,
        motion.velocity[2],
    ]
}

/// One place that knows what "run this scene for a tick" means.
///
/// It used to be two. `loom sim` stepped physics and ran scripts in a loop;
/// `loom render --sim` called a separate helper that stepped physics N times
/// in one go and ran no scripts at all. So a character walked in the numbers
/// and stood still in the picture — the two verification channels the agent
/// has (brief §5) disagreed about the same scene, which makes both useless.
pub struct Runner {
    physics: Sim,
    host: loom_script::ScriptHost,
    /// Scripts on nodes that are characters. These are *movement models*: they
    /// answer with a velocity and the controller does the moving.
    character_scripts: std::collections::BTreeMap<loom_ecs::Entity, String>,
    /// Scripts on ordinary nodes, which write a transform directly.
    node_scripts: Vec<(loom_ecs::Entity, String)>,
    /// The game's rules, if the scene has any, and the state they keep.
    rules: Option<String>,
    state: loom_script::GameState,
    /// The on-screen button held on the previous tick — ADR 0111. Part of the
    /// run, so a save that restores mid-hold does not raise a second press.
    ui_was: u8,
    /// The mood ladder, parsed once, **only when some rung ramps the wind**.
    ///
    /// Empty for every scene that does not — which is every scene written
    /// before this — so the per-tick weather costs exactly one `is_empty`
    /// there. Parsed at load rather than inside `mood_weather_of` because that
    /// function now runs on the fixed tick.
    weather_stages: Vec<loom_scene::components::MoodStage>,
    /// Blasts that have not gone off yet: the tick they fire on, and what
    /// they do. Sorted by tick and drained from the front.
    ///
    /// Resolved once at load rather than scanned for every tick, and the
    /// position is taken then too — a blast is an event at a place, and the
    /// place is where the node was when the scene started.
    pending_blasts: Vec<(u64, [f32; 3], f32, f32)>,
    /// Everything that has happened, in order.
    ///
    /// One queue rather than a field per kind. Detonations used to have their
    /// own — a `Vec<(tick, Detonation)>`, its own filter, its own slot in the
    /// rules' view, its own replay path for the particles. Damage would have
    /// needed the same again, and death after that. This is that shape, once.
    events: loom_script::EventLog,
    /// This tick's player input, folded into every character's `Motion`.
    ///
    /// Headless it stays at its default of all-zero, which is right: nobody is
    /// pressing keys for `loom sim`, and a scene must simulate the same way
    /// whether or not a window is open.
    pub input: loom_script::Motion,
    /// What each *other* character is being asked to do, by character index —
    /// ADR 0091.
    ///
    /// **Empty in a single-player game**, which is why `input` is still the
    /// field everything else writes: one human driving one character needs no
    /// map. Co-op is the reason there is now more than one driver, and lockstep
    /// is what makes it safe — every peer fills this with the same intents on
    /// the same tick, so every machine drives every character identically.
    pub driven: std::collections::BTreeMap<usize, loom_script::Motion>,
}

impl Runner {
    /// Compile every script the scene names and build its physics world.
    ///
    /// # Errors
    /// A JSON line ready to print, for a script that will not read or compile.
    pub fn new(world: &World, base: &std::path::Path) -> Result<Self, String> {
        let mut host = loom_script::ScriptHost::default();
        let mut character_scripts = std::collections::BTreeMap::new();
        let mut node_scripts = Vec::new();

        for entity in world.entities() {
            let Some(script) = world.script_path(*entity) else {
                continue;
            };
            let source = loom_asset::pack::read_text(&base.join(script)).map_err(|e| {
                crate::json_line(&serde_json::json!({
                    "error": "io_error", "script": script, "constraint": e.to_string(),
                }))
            })?;
            host.compile(script, &source).map_err(|e| crate::json_line(&e))?;

            // Which entry point a script gets is decided by the node, not by
            // the file: a character's script is its movement model, anything
            // else's moves a transform. Running a character's script the
            // second way as well would apply its position writes directly,
            // which skips collision — it would walk through walls.
            if world.character(*entity).is_some() {
                character_scripts.insert(*entity, script.to_owned());
            } else {
                node_scripts.push((*entity, script.to_owned()));
            }
        }

        // The rules script, compiled like any other. At most one: a game has
        // one set of rules, and two scripts both deciding whether it is over
        // is a race with no winner.
        //
        // **A refusal, not a warning, and the change is deliberate.** This used
        // to log "ignoring {path}" and carry on, which is the worst available
        // behaviour: the second script's win condition, its HUD line and its
        // whole `state` silently do nothing, and *every gate stays green* —
        // `validate` never loads a script, `sim --assert` reads the surviving
        // rules and passes, and the image gate photographs a scene whose rules
        // are half absent. It is the exact failure class the gameplay block in
        // `scripts/green.sh` exists to catch and it is invisible to it.
        //
        // The demo walked into it in round 4 and this is what it hit: the hub's
        // rules held the slot and the fishing fight wanted the same one. The
        // refusal did its job — it arrived as a named error on the first run
        // rather than as a fishing loop that quietly was not there — and the
        // answer was to merge them into `deeper_rules.rhai`, whose hub half is
        // behind `"Rig/Player" in positions` and unreachable from the five
        // fight benches. That is the outcome this message asks for, in the
        // order it asks for it.
        let mut rules: Option<String> = None;
        for entity in world.entities() {
            let Some(path) = world.rules_path(*entity) else {
                continue;
            };
            if let Some(first) = &rules {
                return Err(crate::json_line(&serde_json::json!({
                    "error": "duplicate_game_rules",
                    "script": path,
                    "constraint": format!(
                        "a scene has one GameRules; `{first}` already holds it. \
                         Merge the two rules scripts, or move one of them into a \
                         node `Script` — which is also the only kind that sees \
                         input."
                    ),
                })));
            }
            let source = loom_asset::pack::read_text(&base.join(path)).map_err(|e| {
                crate::json_line(&serde_json::json!({
                    "error": "io_error", "script": path, "constraint": e.to_string(),
                }))
            })?;
            host.compile(path, &source).map_err(|e| crate::json_line(&e))?;
            rules = Some(path.to_owned());
        }

        // Blasts, in the order they go off.
        let mut pending_blasts = Vec::new();
        for entity in world.entities() {
            let (Some(component), Some(global)) =
                (world.blast(*entity), world.global_transform(*entity))
            else {
                continue;
            };
            #[allow(clippy::cast_possible_truncation)]
            let scalar = |name: &str, fallback: f32| {
                component
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .map_or(fallback, |v| v as f32)
            };
            let defaults = loom_scene::components::Blast::default();
            // A dormant blast is a prefab, not an event: it waits to be set
            // off rather than going off on its own.
            if !component
                .get("armed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(defaults.armed)
            {
                continue;
            }
            let delay = scalar("delay", defaults.delay).max(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            // Ticks, not seconds, because that is what the simulation counts.
            // Rounded up so a blast never fires early, which would put the
            // force ahead of the flash it is meant to share an instant with.
            let at_tick = (delay / TICK_SECONDS).ceil() as u64 + 1;
            pending_blasts.push((
                at_tick,
                [global.matrix[12], global.matrix[13], global.matrix[14]],
                scalar("radius", defaults.radius),
                scalar("impulse", defaults.impulse),
            ));
        }
        pending_blasts.sort_by_key(|(tick, ..)| std::cmp::Reverse(*tick));

        Ok(Self {
            // Built from the world as authored. A script that moves a body
            // after this point is not fed back into the solver; scripted
            // dynamic bodies are not wired yet, and that is a gap rather than
            // a silent approximation.
            physics: Sim::new(world),
            host,
            character_scripts,
            node_scripts,
            rules,
            ui_was: 0,
            state: loom_script::GameState::default(),
            // Parsed only when a rung actually ramps the wind, so this stays
            // empty — and the per-tick weather stays one `is_empty` — for
            // every scene that does not have a weather ladder.
            weather_stages: world
                .environment()
                .and_then(|c| c.get("stages"))
                .and_then(|v| {
                    serde_json::from_value::<Vec<loom_scene::components::MoodStage>>(v.clone()).ok()
                })
                .filter(|stages| stages.iter().any(|s| s.wind_speed.is_some()))
                .unwrap_or_default(),
            pending_blasts,
            events: loom_script::EventLog::default(),
            input: loom_script::Motion::default(),
            driven: std::collections::BTreeMap::new(),
        })
    }

    /// Everything a save has to keep — ADR 0088.
    ///
    /// The rules' state, every dynamic body, and every character's script
    /// memory: the three places a running game keeps anything a script can read
    /// back. The tick is the caller's, because a `Runner` does not own a clock.
    #[must_use]
    pub fn save_state(&self, world: &loom_ecs::World) -> serde_json::Value {
        // **Node transforms, because a node script's memory *is* its
        // transform.** `host.tick` reads the node's current position and
        // returns the next one; it keeps nothing between ticks. So the
        // deckhand's 44 parts, the wheel and every other scripted node remember
        // where they are only by being there, and a save that omitted this
        // reloaded them at their scene pose — which the state hash caught.
        let mut transforms = serde_json::Map::new();
        for entity in world.entities() {
            let (Some(path), Some(t)) = (world.path(*entity), world.transform(*entity)) else {
                continue;
            };
            transforms.insert(
                path.to_owned(),
                serde_json::json!({
                    "pos": t.pos,
                    "rot": t.rot_euler,
                    "scale": t.scale,
                }),
            );
        }
        serde_json::json!({
            "rules": self.state.to_json(),
            "sim": self.physics.save_state(world),
            "transforms": transforms,
            "events": self.events.to_json(),
        })
    }

    /// Restore it.
    ///
    /// **Takes the world mutably and leaves it consistent.** Bodies move
    /// underneath the scene graph, so a restore that stopped at the physics set
    /// would leave every node transform — and therefore the determinism hash,
    /// every `--assert`, and the picture — describing where things were before
    /// the save was loaded. Writing back here rather than asking the caller to
    /// remember is what makes "loaded" and "consistent" the same event.
    pub fn restore_state(&mut self, world: &mut loom_ecs::World, value: &serde_json::Value) {
        if let Some(rules) = value.get("rules") {
            self.state.restore(rules);
        }
        // Transforms first, then physics: `write_back` overwrites the nodes
        // that have bodies, and it should win — a body's position is the
        // authority for anything that has one.
        if let Some(transforms) = value.get("transforms").and_then(serde_json::Value::as_object) {
            for entity in world.entities().to_vec() {
                let Some(path) = world.path(entity).map(str::to_owned) else {
                    continue;
                };
                let Some(saved) = transforms.get(&path) else {
                    continue;
                };
                if let Some(t) = world.transform_mut(entity) {
                    if let Some(v) = saved.get("pos").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                        t.pos = v;
                    }
                    if let Some(v) = saved.get("rot").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                        t.rot_euler = v;
                    }
                    if let Some(v) = saved.get("scale").and_then(|v| serde_json::from_value(v.clone()).ok()) {
                        t.scale = v;
                    }
                }
            }
            world.propagate_transforms();
        }
        if let Some(events) = value.get("events") {
            self.events.restore(events);
        }
        if let Some(sim) = value.get("sim") {
            self.physics.restore_state(world, sim);
            self.physics.write_back(world);
        }
    }
    /// The game's state: status, message and whatever the rules are keeping.
    #[must_use]
    pub fn state(&self) -> &loom_script::GameState {
        &self.state
    }

    /// Everything that has happened, in order.
    #[must_use]
    pub fn events(&self) -> &loom_script::EventLog {
        &self.events
    }

    /// Where and when each explosion went off, derived from the log rather
    /// than tracked alongside it — one record of what happened, not two.
    #[must_use]
    pub fn fired(&self) -> Vec<(u64, [f32; 3])> {
        self.events
            .all()
            .iter()
            .filter(|e| e.kind == BLAST)
            .map(|e| (e.tick, e.at))
            .collect()
    }

    /// Every entry into the water, for the splash.
    ///
    /// Derived from the log for the same reason `fired` is: one record of what
    /// happened. A splash tracked alongside the event that caused it is two
    /// records that can disagree, and the one the human sees would be the one
    /// no assertion checks.
    ///
    /// **Reads [`SPLASH`], not [`SUBMERGED`]** — see that constant. It used to
    /// read the hysteretic one, which is why `pool.loom` drew nothing at all.
    #[must_use]
    pub fn splashed(&self) -> Vec<Splash> {
        self.events
            .all()
            .iter()
            .filter(|e| e.kind == SPLASH)
            .map(|e| Splash {
                tick: e.tick,
                at: e.at,
                #[allow(clippy::cast_possible_truncation)]
                speed: e.values.get("speed").copied().unwrap_or(0.0) as f32,
                #[allow(clippy::cast_possible_truncation)]
                radius: e.values.get("radius").copied().unwrap_or(0.0) as f32,
            })
            .collect()
    }

    /// This tick's wavelet events, for whoever is drawing the surface.
    ///
    /// Straight through to [`Sim::wavelets`] — the simulation owns them and
    /// the renderer is only shown it.
    #[must_use]
    pub fn wavelets(&self) -> &loom_water::wavelet::WaveletField {
        self.physics.wavelets()
    }

    /// Straight through to [`Sim::foam`] — the simulation owns the field and
    /// the renderer is handed a copy of it.
    #[must_use]
    pub fn foam(&self) -> Option<&loom_water::foam::FoamField> {
        self.physics.foam()
    }

    /// The FFT cascade this run has reached — straight through to [`Sim::sea`].
    ///
    /// The simulation owns the ocean and evolves it; this hands it out read-only, the
    /// same arrangement `wavelets` and `foam` are under.
    #[must_use]
    pub fn sea(&self) -> Option<&loom_water::ocean::Ocean> {
        self.physics.sea()
    }

    /// Straight through to [`Sim::water`], under the same arrangement.
    #[must_use]
    pub fn water(&self) -> Option<&loom_scene::components::WaterBody> {
        self.physics.water()
    }

    /// The collision world this run is holding, for anything that has to cast a
    /// ray against what is actually there — the drips, in practice (ADR 0054).
    #[must_use]
    pub fn collision_world(&self) -> &loom_physics::Physics {
        self.physics.world()
    }

    /// The cinematic tier's free surface and spray — see [`Sim::fluid_draw`].
    ///
    /// Empty for every scene that does not opt into the tier.
    pub fn fluid_draw(
        &mut self,
    ) -> (Vec<loom_render::FluidVertex>, Vec<loom_render::ParticleInstance>, FluidDrawCost) {
        self.physics.fluid_draw()
    }

    /// What the cinematic tier's device round trip cost: total ms, fence ms,
    /// ticks. Zero when there is no solver.
    #[must_use]
    pub fn fluid_cost(&self) -> (f64, f64, u64) {
        self.physics.fluid_cost()
    }

    /// A runner that steps physics and runs nothing, for when the scripts
    /// could not be loaded.
    #[must_use]
    pub fn physics_only(world: &World) -> Self {
        Self {
            physics: Sim::new(world),
            host: loom_script::ScriptHost::default(),
            character_scripts: std::collections::BTreeMap::new(),
            node_scripts: Vec::new(),
            rules: None,
            ui_was: 0,
            state: loom_script::GameState::default(),
            weather_stages: Vec::new(),
            pending_blasts: Vec::new(),
            events: loom_script::EventLog::default(),
            input: loom_script::Motion::default(),
            driven: std::collections::BTreeMap::new(),
        }
    }

    /// Advance one fixed tick: physics, then characters, then node scripts.
    ///
    /// Characters move after the step because the tree they collide against
    /// is built during it, and before node scripts so a script reading a
    /// character's position sees this tick's, not last tick's.
    ///
    /// # Errors
    /// [`loom_script::ScriptError`] from whichever script failed.
    pub fn tick(&mut self, world: &mut World, tick: u64) -> Result<(), loom_script::ScriptError> {
        // **A press becomes an ordinary event** — ADR 0111. It arrives as the
        // number that travelled on the wire and leaves as the button's node
        // path, resolved through the scene both peers share; a rules script
        // matches `e.kind == "ui"` against `e.node` and never sees an index.
        //
        // Before the step, so a script that starts a game on a press has the
        // same tick to act in that a key press would give it.
        // **The edge is taken here, not by the caller.** `Intent.ui` is a level —
        // which button the player is holding — because that is what a fixed
        // frame can carry and what a lockstep peer replays. Left as a level, a
        // finger resting on START pressed it sixty times a second; measured, a
        // fifteen-tick hold raised sixteen events. Doing it in the runner means
        // the window, `loom sim --hold` and a remote peer all get one press from
        // one press, rather than three callers each remembering to.
        let pressed = self.input.ui != 0 && self.input.ui != self.ui_was;
        self.ui_was = self.input.ui;
        if pressed
            && let Some(path) = crate::hud::button_path(world, self.input.ui)
        {
            self.events.push(loom_script::Event {
                tick,
                kind: "ui".to_owned(),
                at: [0.0; 3],
                node: path,
                values: std::collections::BTreeMap::new(),
            });
        }
        // **The weather, before the step it acts on.** The rules wrote `dread`
        // at the end of the last tick, so the sea this step is solved against
        // is one tick behind the scalar — deterministic, and irrelevant
        // against a ramp that takes tens of seconds to cross.
        if !self.weather_stages.is_empty() {
            #[allow(clippy::cast_possible_truncation)]
            let dread = self.state.number("dread").unwrap_or(0.0) as f32;
            if let (Some(speed), _) = crate::mood_weather_of(&self.weather_stages, dread) {
                self.physics.reweather(world, speed);
            }
        }
        self.physics.step(1);

        // What the water did during the step, into the one log. Before the
        // rules run, so a rule sees a body go under on the tick it went under
        // rather than the tick after.
        for event in self.physics.drain_water_events(tick) {
            self.events.push(event);
        }

        // Blasts go off after the step, because the tree the cover check walks
        // is built during it — before the first step every body is in the open
        // and a wall shields nothing.
        //
        // Sorted descending, so the ones that are due are at the end and pop
        // in order. A scheduled event is not scanned for.
        while self
            .pending_blasts
            .last()
            .is_some_and(|(at, ..)| *at <= tick)
        {
            let (_, centre, radius, impulse) = self.pending_blasts.pop().expect("just peeked");
            self.physics.apply_blast(centre, radius, impulse);
        }

        self.physics.write_back(world);

        if self.physics.character_count() > 0 {
            let host = &self.host;
            let scripts = &self.character_scripts;
            let input = self.input.clone();
            let driven = self.driven.clone();
            let (detonations, raised) =
                self.physics
                    .drive_characters(world, tick, input, &driven, |entity, motion, memory| {
                        match scripts.get(&entity) {
                            Some(script) => host.motion(script, motion, memory),
                            // No script, so no movement model — it falls and
                            // does nothing else. Not a default walk: inventing
                            // one in Rust is what the script seam exists to
                            // avoid, and a character that mysteriously strolls
                            // off is worse than one that visibly stands still.
                            None => Ok(loom_script::Motive {
                                velocity: fall_only(motion),
                                detonate: None,
                                emitted: Vec::new(),
                                goal: None,
                                helm: None,
                            }),
                        }
                    })?;

            for event in raised {
                self.events.push(event);
            }

            // What a script asked for, done. After the move, so a blast set
            // off at the character's own feet acts on where it ended up.
            for blast in detonations {
                self.physics
                    .apply_blast(blast.at, blast.radius, blast.impulse);
                self.events.push(loom_script::Event {
                    tick,
                    kind: BLAST.to_owned(),
                    at: blast.at,
                    node: String::new(),
                    values: [
                        ("radius".to_owned(), f64::from(blast.radius)),
                        ("impulse".to_owned(), f64::from(blast.impulse)),
                    ]
                    .into_iter()
                    .collect(),
                });

                // Who it caught. The engine reports this because it is the
                // part a script cannot do — deciding who is in range needs the
                // same cover cast the shove uses. What being caught *costs* is
                // a rule, so this carries exposure and says nothing about
                // health, armour or whether it hurts at all.
                for (entity, at, exposure) in
                    self.physics.characters_in_blast(blast.at, blast.radius)
                {
                    self.events.push(loom_script::Event {
                        tick,
                        kind: DAMAGE.to_owned(),
                        at,
                        node: world.path(entity).unwrap_or_default().to_owned(),
                        values: [("exposure".to_owned(), f64::from(exposure))]
                            .into_iter()
                            .collect(),
                    });
                }
            }
        }

        // Built once, not per node: `emberfall` has one node script and the
        // rod has twenty-four, and this is the same map for all of them.
        let rules_numbers = self.state.numbers();
        for (entity, script) in &self.node_scripts {
            let Some(transform) = world.transform(*entity).cloned() else {
                continue;
            };
            let state = loom_script::NodeState {
                position: transform.pos,
                rotation: transform.rot_euler,
                scale: transform.scale,
            };
            let next = self.host.tick(script, tick, &state, &rules_numbers)?;
            if let Some(t) = world.transform_mut(*entity) {
                t.pos = next.position;
                t.rot_euler = next.rotation;
                t.scale = next.scale;
            }
        }
        world.propagate_transforms();

        // **The rules run last.** They judge the tick, so they have to see it
        // finished: a rule reading a position before the character moved is
        // reading last tick's world and would call the game a tick early.
        if let Some(rules) = &self.rules {
            let positions = world.positions();
            let happened = self.events.on_tick(tick);
            let submersion = self.physics.submersion();
            let view = loom_script::WorldView {
                positions: &positions,
                events: &happened,
                submersion: &submersion,
            };
            let raised = self
                .host
                .rules(rules, tick, TICK_SECONDS, &view, &mut self.state)?;
            // Raised after the rules read the tick, so a rule cannot see its
            // own event this tick and loop on it. It lands on the next one.
            for event in raised {
                self.events.push(event);
            }
        }
        Ok(())
    }
}

/// What one call to [`Sim::fluid_draw`] cost, in milliseconds, by part.
///
/// Three numbers rather than one because the one was misread. See the method.
#[derive(Debug, Clone, Copy, Default)]
pub struct FluidDrawCost {
    /// The density field: two dispatches, a submit, a fence and a readback.
    pub density_ms: f64,
    /// The CPU marching tetrahedra in `loom_render::fluid_surface`.
    pub march_ms: f64,
    /// The spray instances: one dispatch, a submit, a fence and a readback.
    pub spray_ms: f64,
}

/// What the human is pressing this frame, sampled by the viewer.
///
/// Sampled per *frame* and applied per *tick*. Those differ, and the tick is
/// what the simulation counts (never-do #8) — so a jump is latched here and
/// consumed by the first tick that runs, rather than being missed because the
/// key went up between two ticks.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInput {
    /// `[strafe, forward]`, each `-1..=1`.
    pub move_axis: [f32; 2],
    pub jump: bool,
    pub sprint: bool,
    pub fire: bool,
    /// The interact key (E). **Level here, edge there**: this is "is E down",
    /// and [`Play::set_input`] turns it into one press. Holding it therefore
    /// interacts once, whether the caller is the window sampling a `pressed`
    /// binding or a test that never lets go.
    pub interact: bool,
    /// The inventory key (Tab). Level here, edge there, exactly as `interact`
    /// is — a held Tab must open the creel once, not sixty times a second.
    pub bag: bool,
    /// Which on-screen button is being pressed — ADR 0111.
    ///
    /// The 1-based index of a `Hud` button among the scene's, in world order;
    /// zero for none. **A number here and a node path in the script**, resolved
    /// through the scene both lockstep peers share, because `loom_net`'s
    /// `Intent` is a fixed-width frame and a name is not.
    ///
    /// Level here, edge there, exactly as `interact` is: a held mouse button
    /// must press a menu item once rather than sixty times a second.
    pub ui: u8,
}

/// Play mode as the editor holds it: a scene's world, its simulation, and how
/// far it has been run.
pub struct Play {
    pub world: World,
    /// The same per-tick runner `loom sim` and `loom render --sim` use.
    ///
    /// It used to be a bare `Sim`, stepped and written back — no scripts at
    /// all. So the editor's Play button advanced the clock while every
    /// scripted thing stood still, and the window disagreed with `loom sim`
    /// about the same scene. Play mode's entire justification is that the two
    /// are one run.
    runner: Runner,
    /// Whole ticks run. Time is counted here, not in seconds.
    pub ticks: u32,
    pub paused: bool,
    /// Left over from the last frame's real elapsed time. Fixed timestep with
    /// an accumulator: the wall clock decides *how many* ticks to run, and
    /// never what a tick is worth.
    leftover: f32,

    /// Where the player is looking, in radians. Yaw turns the character; pitch
    /// only tilts the camera, because a capsule that leans back is a ragdoll.
    yaw: f32,
    pitch: f32,
    /// Held keys, replaced every frame by the viewer.
    input: PlayerInput,
    /// A jump press waiting for a tick to consume it. Separate from `input`
    /// because a press lasts one *frame* and might land between two ticks —
    /// dropping it is the "sometimes jump does nothing" bug.
    jump_pending: bool,
    /// A fire press waiting for a tick, latched for the same reason as `jump`.
    fire_pending: bool,
    /// An interact press waiting for a tick, latched like the two above — and
    /// *only* set on the rising edge, which those two are not.
    ///
    /// `jump` and `fire` lean on their bindings being `trigger = "pressed"`,
    /// so a held key is already one press by the time it gets here. That is
    /// true of the window and false of everything else: `Play::set_input` is
    /// also how a test and a scripted pilot drive a character, and either can
    /// hold a flag true forever. A door that opens sixty times a second is the
    /// result. So the edge is taken here, where every caller passes.
    interact_pending: bool,
    /// Whether interact was down last time [`Play::set_input`] was called —
    /// the other half of that edge.
    interact_was_down: bool,
    /// The same pair for the inventory key. Its own latch and not a shared
    /// "any pressed key" one: E and Tab are pressed in the same second all the
    /// time, and one flag would swallow whichever arrived second.
    bag_pending: bool,
    bag_was_down: bool,
    /// The on-screen button waiting to be consumed by a tick — ADR 0111.
    /// Latched like the others: a click lasts one frame and the tick that
    /// consumes it may not have run yet.
    ui_pending: u8,
    ui_was: u8,
    /// The character a human drives, and the node the view comes from.
    /// Resolved once at Play: neither can appear mid-run.
    player: Option<loom_ecs::Entity>,
    eye: Option<loom_ecs::Entity>,
    /// Mass for the view. See [`CameraSpring`].
    spring: CameraSpring,
}

/// Just short of straight up or down. At exactly ±90° the forward vector is
/// parallel to world up, `right` degenerates, and strafing snaps around.
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// Mass for the view. The mouse writes a target; this decides how the picture
/// gets there.
///
/// # Tuning it — the whole knob set, in one place
///
/// Four numbers, all read from the environment once at startup ([`from_env`]),
/// so the person who can actually judge this can change them between two runs
/// of the same binary. **Nothing else in this file is a taste decision.**
///
/// ```text
///                      default  range     what it is, in plain words
/// LOOM_CAMERA_HZ           4.0  2 – 6     how quickly the view catches up.
///                                         LOWER IS HEAVIER. This is the knob.
/// LOOM_CAMERA_DAMPING     0.55  0.4 – 1   how much it bounces at the end of a
///                                         turn. Lower bounces more; 1 does not
///                                         bounce at all.
/// LOOM_CAMERA_WEIGHT       1.0  0 – 1     how much of the whole effect you get.
///                                         0 is off, exactly.
/// LOOM_CAMERA_RESPONSE     0.5  0 – 1     how hard it works to keep up with a
///                                         steady turn. Leave it alone.
/// ```
///
/// **Heavier:** `LOOM_CAMERA_HZ=3`, then `=2.5`, then `=2`. One knob; lowering
/// it is the whole of "more sluggish". At 2 a flick takes 417 ms to come to
/// rest, which is about twice as long as you can consciously notice.
/// **Sharper:** `LOOM_CAMERA_HZ=5`, then `=6`. At 6 there is almost nothing
/// left to feel, which is why that is the ceiling.
/// **More of a lurch at the end of a turn:** `LOOM_CAMERA_DAMPING=0.45`.
/// **Less:** `=0.75`. **None:** `=1.0`.
/// **Off, to compare against nothing:** `LOOM_CAMERA_WEIGHT=0`.
/// **What plain input lag feels like, for contrast:** `LOOM_CAMERA_RESPONSE=0`.
/// Do not ship it; it is here so the difference can be felt rather than argued.
///
/// Every range is a floor and a ceiling on *usability*, not taste: outside them
/// the camera stops being one — 7.3 seconds to settle at the bottom, unaimable
/// jitter at the top. Out-of-range values are clamped **with a line in the log**
/// rather than in silence, because a tuning knob that quietly disagrees with
/// what you typed teaches the wrong lesson about the feel. See [`from_env`].
///
/// # Why a spring with feedforward, and not a lerp
///
/// A damped spring with a **feedforward** term, and the feedforward is the
/// whole reason this is weight rather than lag. A plain spring — or any
/// low-pass — trails a sustained turn by a fixed angle, so a steady pan sits
/// permanently behind your hand: at the defaults below, 27.1 ms of equivalent
/// latency, more than the 16.7 ms a mouse event already waits for a tick.
/// Feeding the target's own velocity forward cancels that: **5.22 ms**, at
/// every turn rate, so the lag is under one tick and everything you feel lives
/// in the transients — the view accelerates from rest, overshoots ~3° on a 60°
/// flick, and settles in 117 ms. Sluggish, and it still arrives where you
/// pointed it.
///
/// **[`Self::MAX_LAG`] bounds all of it.** Overshoot is proportional to how
/// fast you were turning, so the 3° a conversational flick produces is 18° at
/// mouse-flick speed and 47° at a spin — degrees at which "weight" is no longer
/// the word for it. The lag clamp is what makes "it still gets to the places
/// where it needs to go" a property of the code rather than of the turn rate
/// the acceptance test happened to pick.
///
/// Stepped **once per simulation tick**, so `dt` is the compile-time constant
/// [`TICK_SECONDS`] and the framerate-independence hazard that sinks every
/// `lerp(a, b, 0.1)` camera simply does not exist. Four players on four
/// machines at four frame rates get the same camera.
///
/// Yaw only. Pitch is deliberately rigid: it is the axis with the lowest
/// motion-sickness threshold, it is the only one hard-clamped (a spring
/// integrating against that wall winds up velocity it discharges when you look
/// back down), and on a boat a vertically swimming horizon is indistinguishable
/// from the sea moving, which is the one artifact this scene cannot afford.
#[derive(Clone, Copy, Debug)]
pub struct CameraSpring {
    /// 0 turns it off exactly — bypassed, not run at a gain of zero. Blends
    /// the filtered angle back toward the raw one in between.
    weight: f32,
    /// Natural frequency, Hz. Lower is heavier and slower to settle.
    hz: f32,
    /// Damping ratio. Below 1 overshoots; 1 arrives with no settle at all.
    zeta: f32,
    /// Feedforward. 0 is a plain spring and reads as input lag. Above ~0.7 the
    /// view *leads* your hand, which is a different and worse artifact.
    response: f32,
    /// The filtered angle, radians.
    angle: f32,
    /// Its velocity. Retained — it is what makes a turn start from rest and a
    /// stop overshoot.
    vel: f32,
    /// Last tick's raw target, for the finite difference the feedforward needs.
    prev: f32,
}

impl CameraSpring {
    /// Chosen against a measured acceptance criterion rather than by taste:
    /// equivalent latency (steady-state error ÷ turn rate) must stay under
    /// 20 ms, the floor of the band where flick-aim data can still measure a
    /// difference. `4.0 / 0.55 / 0.5` gives 5.22 ms, 3.07° of overshoot and a
    /// 117 ms settle. 3 Hz is 167 ms and you can consciously wait it out; 6 Hz
    /// leaves nothing to feel; ζ above 0.75 is a first-order feel bought with
    /// a second-order system; r = 0.8 crosses zero into leading the hand.
    const DEFAULT: Self = Self {
        weight: 1.0,
        hz: 4.0,
        zeta: 0.55,
        response: 0.5,
        angle: 0.0,
        vel: 0.0,
        prev: 0.0,
    };

    /// How far the picture may ever be from the mouse. **A bound, not a taste
    /// knob** — which is why it is a constant and not a fifth environment
    /// variable.
    ///
    /// A spring's overshoot is proportional to the angular velocity it was
    /// carrying, and nothing in the design caps that. Measured on the shipped
    /// filter at the defaults, past a flick's endpoint:
    ///
    /// ```text
    ///    300 °/s (a look across the deck)    3.07°
    ///   1800 °/s (an ordinary mouse flick)  18.42°
    ///   7200 °/s (a fast 360)               47.28°
    /// ```
    ///
    /// The acceptance test only ever bounded the first row, so the feature
    /// shipped asserted at a fifth of the speed it is used at.
    ///
    /// 6° because it is above every number this camera produces at the speeds a
    /// player talks and walks at — the 300 °/s row is **bit-identical** clamped
    /// and unclamped, so the feel that was tuned is the feel that ships — and
    /// below the ~13° bucket that `deeper_player.rhai`'s heading caption
    /// quantises to. It also *shortens* a fast flick's settle (350 → 283 ms at
    /// 1800 °/s) and *reduces* sustained error above 800 °/s, because a bound
    /// on the lag is a bound in both directions.
    const MAX_LAG: f32 = 0.104_719_76; // 6°

    /// The range each knob is clamped to. Named rather than written as four
    /// literals inside [`Self::from_env`], so that
    /// `every_setting_a_knob_can_reach_is_stable` can sweep the actual box the
    /// human can reach instead of a box a test hard-coded to agree with itself.
    const WEIGHT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;
    /// See [`Self::from_env`]. The ceiling is where semi-implicit Euler stops
    /// being stable at a 60 Hz step; the floor is where a camera stops being a
    /// camera. At `hz = 2` a 60° flick settles in 417 ms at the loosest damping
    /// allowed, which is already twice human reaction time and as heavy as
    /// anyone could want; at `hz = 0.5` it is **7.3 seconds**.
    const HZ_RANGE: std::ops::RangeInclusive<f32> = 2.0..=6.0;
    /// Above 1.0 is overdamped, and it drags the `hz` cliff down with it. Below
    /// 0.4 the ring outlasts the turn — 1.8 s at `hz = 2`, against 417 ms at
    /// 0.4. Every corner of `HZ_RANGE × DAMPING_RANGE` settles inside 417 ms.
    const DAMPING_RANGE: std::ops::RangeInclusive<f32> = 0.4..=1.0;
    /// Above 1.0 the view leads the hand by more than the hand is moving.
    const RESPONSE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;

    /// Explicit parameters. Tests use this; the game reads [`Self::from_env`].
    #[must_use]
    pub fn new(weight: f32, hz: f32, zeta: f32, response: f32) -> Self {
        Self {
            weight,
            hz,
            zeta,
            response,
            ..Self::DEFAULT
        }
    }

    /// The four knobs, from the environment, read once when play starts. The
    /// table of what each one does is on [`CameraSpring`] itself.
    ///
    /// Environment rather than a settings table because not one of these
    /// constants has been *felt* yet, and a schema written before that is a
    /// schema for the wrong three floats. It is tunable without a rebuild,
    /// which is the property that matters this week.
    ///
    /// **Every knob is clamped to a range, and the ceiling on `hz` is a
    /// stability limit rather than a matter of taste.** Semi-implicit Euler at
    /// a 60 Hz step diverges above a natural frequency that *moves with the
    /// damping*: the last stable `hz` is 11.41 at ζ = 0.55, 8.04 at ζ = 1 and
    /// 4.60 at ζ = 2. So the two cannot be bounded independently. `hz ≤ 6` with
    /// `ζ ≤ 1` is stable across the whole box with the nearest cliff at 8.04 —
    /// a 1.34× margin — and it costs nothing, because 6 Hz already leaves
    /// almost nothing to feel and ζ above 1 is overdamped.
    ///
    /// **What "unstable" looks like changed when [`Self::MAX_LAG`] landed, and
    /// the weaker symptom is the one to design against.** Before the lag clamp,
    /// `LOOM_CAMERA_HZ=12` made the camera NaN outright and `=-4` sent it to
    /// 2.7e24 degrees. With the clamp both are bounded — and *still* unusable:
    /// at `hz = 12` the view sits pinned against the bound, jittering 11.6°
    /// tick to tick, and settles 5.6° from where the mouse points. Finite,
    /// bounded, and a camera nobody can aim. This range is what keeps that out
    /// of reach on the knob whose entire purpose is being swept by hand.
    ///
    /// **Clamping is reported, not silent.** A tuning knob that quietly
    /// disagrees with what you typed teaches the wrong lesson about the feel.
    #[must_use]
    pub fn from_env() -> Self {
        fn read(key: &str, fallback: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
            let asked = std::env::var(key).ok().and_then(|v| v.parse::<f32>().ok());
            CameraSpring::knob(key, asked, fallback, range)
        }
        let d = Self::DEFAULT;
        Self::new(
            read("LOOM_CAMERA_WEIGHT", d.weight, &Self::WEIGHT_RANGE),
            read("LOOM_CAMERA_HZ", d.hz, &Self::HZ_RANGE),
            read("LOOM_CAMERA_DAMPING", d.zeta, &Self::DAMPING_RANGE),
            read("LOOM_CAMERA_RESPONSE", d.response, &Self::RESPONSE_RANGE),
        )
    }

    /// One knob's worth of "what the human typed" → "what the camera uses".
    ///
    /// Split out of [`Self::from_env`] so it can be tested at all. Environment
    /// variables are process-global and these tests run in parallel, so a test
    /// that sets one flakes against every other test that starts a [`Play`] —
    /// and with the reading and the clamping in one function, that left the
    /// clamping untested. It was: deleting it was injected as a fault and every
    /// test passed, on the exact line the whole range-clamp defect was about.
    fn knob(
        key: &str,
        asked: Option<f32>,
        fallback: f32,
        range: &std::ops::RangeInclusive<f32>,
    ) -> f32 {
        // A missing knob and an unreadable one are the same thing: the default.
        // NaN and infinity are filtered here rather than clamped, because
        // `f32::clamp` panics on a NaN bound and returns NaN for a NaN input.
        let Some(asked) = asked.filter(|v| v.is_finite()) else {
            return fallback;
        };
        let used = asked.clamp(*range.start(), *range.end());
        if used != asked {
            crate::log::warn(format!(
                "{key}={asked} is outside {}..={}; using {used}",
                range.start(),
                range.end()
            ));
        }
        used
    }

    /// Start settled on `target`, so pressing Play does not spring the view in
    /// from wherever zero happens to be.
    fn seed(&mut self, target: f32) {
        self.angle = target;
        self.prev = target;
        self.vel = 0.0;
    }

    /// Advance one tick toward `target`.
    ///
    /// Semi-implicit: velocity is integrated first and position uses the new
    /// velocity, which is what keeps a spring this stiff stable at 60 Hz.
    fn settle(&mut self, target: f32) {
        if self.weight == 0.0 {
            // Bypassed, not damped to nothing. A zero that still runs the
            // filter is a zero that lies.
            self.seed(target);
            return;
        }
        let w = std::f32::consts::TAU * self.hz;
        let target_vel = (target - self.prev) / TICK_SECONDS;
        self.prev = target;
        self.vel += (w * w * (target - self.angle)
            - 2.0 * self.zeta * w * (self.vel - self.response * target_vel))
            * TICK_SECONDS;
        self.angle += self.vel * TICK_SECONDS;

        // Never further from the mouse than [`Self::MAX_LAG`], in either
        // direction.
        //
        // The velocity is trimmed to the target's own at the wall — and *not*
        // for the reason you would guess. It was written against a kick coming
        // off the stop, and measurement says there is no kick: bounding the
        // position bounds the error whatever the velocity is doing, and the
        // worst error after a hard stop from 600–7200 °/s is 6.000° with the
        // trim and 6.000° without it. What it actually stops is `vel` growing
        // without bound while pinned, which at an out-of-range frequency ends
        // in a NaN — and a NaN fails *both* comparisons below, so it would walk
        // straight through the clamp. Two lines that keep the range clamp from
        // being the only thing between a mis-set knob and a dead camera. See
        // `the_lag_clamp_survives_a_frequency_the_range_clamp_forbids`.
        let (behind, ahead) = (target - Self::MAX_LAG, target + Self::MAX_LAG);
        if self.angle < behind {
            self.angle = behind;
            self.vel = self.vel.max(target_vel);
        } else if self.angle > ahead {
            self.angle = ahead;
            self.vel = self.vel.min(target_vel);
        }
    }

    /// The angle to look along. Derived rather than stored, so the picture and
    /// the aim ray cannot drift apart by holding two copies of one number.
    fn view(&self, target: f32) -> f32 {
        target + self.weight * (self.angle - target)
    }
}

impl Play {
    /// The running game as a save file — ADR 0088, reachable from the editor.
    ///
    /// **The same snapshot `loom sim --save` writes**, so a game saved from the
    /// editor loads in the CLI and the other way round. Play mode is a real run
    /// of the real runner; there is no second format for it.
    #[must_use]
    pub fn save_game(&self, tick: u64) -> serde_json::Value {
        serde_json::json!({
            "format": 1,
            "tick": tick,
            "state": self.runner.save_state(&self.world),
        })
    }

    /// Put a saved game back into the running world — ADR 0088.
    pub fn load_game(&mut self, value: &serde_json::Value) {
        let state = value.get("state").unwrap_or(value);
        self.runner.restore_state(&mut self.world, state);
    }

    /// Begin simulating. `base` is the directory scripts resolve against —
    /// the scene file's own directory.
    ///
    /// A script that will not load is reported and play continues without it,
    /// rather than refusing to start. The editor is where a half-written
    /// script is normal, and a Play button that does nothing at all teaches
    /// less than a moving scene with one thing missing and a line in the log.
    #[must_use]
    pub fn start(world: World, base: &std::path::Path) -> Self {
        let runner = Runner::new(&world, base).unwrap_or_else(|json| {
            crate::log::warn(format!("scripts did not load: {json}"));
            Runner::physics_only(&world)
        });
        // The authored camera's rotation is where the view starts, so
        // pressing Play does not snap the human somewhere else.
        let eye = world.active_camera_entity();
        let (yaw, pitch) = eye
            .and_then(|e| world.transform(e))
            .map_or((0.0, 0.0), |t| {
                (t.rot_euler[1].to_radians(), t.rot_euler[0].to_radians())
            });

        let mut spring = CameraSpring::from_env();
        spring.seed(yaw);

        Self {
            player: world.player_character(),
            eye,
            world,
            runner,
            ticks: 0,
            paused: false,
            leftover: 0.0,
            spring,
            yaw,
            pitch: pitch.clamp(-MAX_PITCH, MAX_PITCH),
            input: PlayerInput::default(),
            jump_pending: false,
            fire_pending: false,
            interact_pending: false,
            interact_was_down: false,
            bag_pending: false,
            ui_pending: 0,
            ui_was: 0,
            bag_was_down: false,
        }
    }

    /// Replace this frame's held input.
    pub fn set_input(&mut self, input: PlayerInput) {
        // Latched, not overwritten: a press lasts one frame and the tick that
        // consumes it may not have run yet.
        self.jump_pending |= input.jump;
        self.fire_pending |= input.fire;
        // Rising edge, not level — see `interact_pending`.
        self.interact_pending |= input.interact && !self.interact_was_down;
        self.interact_was_down = input.interact;
        self.bag_pending |= input.bag && !self.bag_was_down;
        self.bag_was_down = input.bag;
        // A press, not a hold — ADR 0111. `ui_was` carries the last level so a
        // finger resting on a menu item fires it once.
        if input.ui != 0 && input.ui != self.ui_was {
            self.ui_pending = input.ui;
        }
        self.ui_was = input.ui;
        self.input = input;
    }

    /// Turn the view by a mouse delta, in radians.
    pub fn look(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw -= yaw_delta;
        self.pitch = (self.pitch - pitch_delta).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// The view this scene is played from, if it authored a camera.
    #[must_use]
    pub fn camera(&self) -> Option<loom_ecs::CameraView> {
        self.world.active_camera()
    }

    /// The game's status, message and numbers.
    #[must_use]
    pub fn state(&self) -> &loom_script::GameState {
        self.runner.state()
    }

    /// Where and when each explosion went off.
    #[must_use]
    pub fn fired(&self) -> Vec<(u64, [f32; 3])> {
        self.runner.fired()
    }

    /// Every entry into the water.
    #[must_use]
    pub fn splashed(&self) -> Vec<Splash> {
        self.runner.splashed()
    }

    /// Everything that has happened, in order.
    #[must_use]
    pub fn events(&self) -> &loom_script::EventLog {
        self.runner.events()
    }

    /// The collision world, for anything that needs to cast against what the
    /// simulation is actually holding — the acoustics, in practice.
    #[must_use]
    pub fn physics(&self) -> &loom_physics::Physics {
        self.runner.physics.world()
    }

    /// Whether the listener is under the water. What the ears hear, not what
    /// any body is doing — see `Sim::submerged_at`.
    #[must_use]
    pub fn submerged_at(&self, at: [f32; 3]) -> bool {
        self.runner.physics.submerged_at(at)
    }

    /// This tick's wavelet events, for the window to displace the surface with.
    ///
    /// Empty in edit mode as well as for untouched water, because there
    /// is no simulation then and so nothing has stepped a grid — which is the
    /// same flat surface every scene had before ADR 0046.
    #[must_use]
    pub fn wavelets(&self) -> &loom_water::wavelet::WaveletField {
        self.runner.wavelets()
    }

    /// The foam field this play session's simulation has reached.
    #[must_use]
    pub fn foam(&self) -> Option<&loom_water::foam::FoamField> {
        self.runner.foam()
    }

    /// The FFT cascade this play session's simulation has reached — ADR 0076.
    ///
    /// **Beside `foam` because it is the same kind of thing**: CPU water state
    /// the window has to be handed, on the tick that produced it.
    /// `Sim::evolve_sea`'s invariant is that the tiles always hold exactly
    /// `tick x TICK_SECONDS`, so what this returns is never a frame stale and
    /// never a frame early.
    ///
    /// `None` for a Gerstner body, which is every scene that has not opted in.
    #[must_use]
    pub fn sea(&self) -> Option<&loom_water::ocean::Ocean> {
        self.runner.sea()
    }

    /// What the sea sounds like this tick, for the audio bed.
    ///
    /// **Assembled here rather than in the window** so the one line the viewer
    /// needs is `sound.set_sea(play.sea_state())`, and so the three numbers
    /// come off the run that owns them: the cascade, the foam field and the
    /// wave set the ladder may have moved. See [`crate::weather::sea_state`]
    /// for why not one of them is re-derived.
    ///
    /// A scene with no water answers a flat calm, which the bed renders as
    /// exact zeros.
    ///
    /// **Only the tests call it in this commit**, and the `allow` says so
    /// rather than hiding it: the caller is the window's `update_sound` in
    /// `run.rs`, the human's own uncommitted file, and the hunk to paste is in
    /// `.superpowers/sdd/SEA-SOUND-PLAN/task-2-report.md`. **Delete this
    /// attribute when that lands.**
    #[must_use]
    #[allow(dead_code)]
    pub fn sea_state(&self) -> loom_audio::sea::SeaState {
        crate::weather::sea_state(
            self.runner.water(),
            self.runner.sea(),
            self.runner.foam(),
            crate::weather::wind_of_world(&self.world).mean_speed_at(10.0),
        )
    }

    /// The cinematic tier's free surface and spray — see [`Sim::fluid_draw`].
    ///
    /// **This is the window's half of it.** `loom render --sim N` has asked
    /// since ADR 0057; `loom run` stepped the same solver and drew nothing of
    /// the result, which is defect 5 of the water rebuild review and the third
    /// time this project has shipped a water effect on one path only.
    pub fn fluid_draw(
        &mut self,
    ) -> (Vec<loom_render::FluidVertex>, Vec<loom_render::ParticleInstance>, FluidDrawCost) {
        self.runner.fluid_draw()
    }

    /// What the cinematic tier's device round trip has cost this session: total
    /// ms, fence ms, ticks. Zero when there is no solver.
    #[must_use]
    pub fn fluid_cost(&self) -> (f64, f64, u64) {
        self.runner.fluid_cost()
    }

    /// Whether a human can drive anything here.
    #[must_use]
    pub fn has_player(&self) -> bool {
        self.player.is_some()
    }

    /// Write the look angles onto the rig before the tick reads them.
    ///
    /// Yaw goes on the character and pitch on the camera, which is what makes
    /// a camera parented to the character a first-person rig: turning the body
    /// carries the view with it, and looking up does not tip the capsule over.
    /// With no character the camera takes both, so a scene with only a camera
    /// is still free to look around.
    fn apply_look(&mut self) {
        // The picture's yaw is the sprung one — the mouse writes the target,
        // the spring decides how the view gets there. Pitch is rigid on
        // purpose; see [`CameraSpring`].
        self.spring.settle(self.yaw);
        let (yaw, pitch) = (self.view_yaw().to_degrees(), self.pitch.to_degrees());
        match self.player {
            Some(player) => {
                if let Some(t) = self.world.transform_mut(player) {
                    t.rot_euler[1] = yaw;
                }
                if let Some(t) = self.eye.and_then(|e| self.world.transform_mut(e)) {
                    t.rot_euler[0] = pitch;
                }
            }
            None => {
                if let Some(t) = self.eye.and_then(|e| self.world.transform_mut(e)) {
                    t.rot_euler[0] = pitch;
                    t.rot_euler[1] = yaw;
                }
            }
        }
        self.world.propagate_transforms();
    }

    /// The yaw the picture is actually drawn at, once the spring has had it.
    fn view_yaw(&self) -> f32 {
        self.spring.view(self.yaw)
    }

    /// The full look direction, pitch included. What a shot travels along.
    ///
    /// **Sprung, like the picture.** The crosshair sits at screen centre and
    /// must never lie about where a shot goes; an unfiltered ray is several
    /// degrees off it for the ~120 ms a flick takes to settle, which is over a
    /// hundred pixels of visible drift at 1080p. Inert in the fishing demo,
    /// which casts from a state machine and reads no aim ray at all, and live
    /// in `proving_ground` via `fps.rhai` — where a flick-shot now lands where
    /// the picture pointed rather than where the mouse did.
    fn aim(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.view_yaw().sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        [
            -sin_yaw * cos_pitch,
            sin_pitch,
            -cos_yaw * cos_pitch,
        ]
    }

    /// The horizontal basis the player's input is expressed in.
    fn basis(&self) -> ([f32; 3], [f32; 3]) {
        // **The scene format's forward is −Z**, not +Z. These are exactly the
        // camera node's own axes at this yaw — its local −Z and local +X — so
        // "forward" for the script and "forward" for the view are the same
        // direction by construction. Copying the fly camera's convention here
        // instead (yaw 0 looking down +Z) made W walk backwards, and the
        // symptom was a character that moonwalks away from where you look.
        //
        // Flat, because looking at the sky must not make W walk into it.
        //
        // **The sprung yaw, not the raw one.** Letting the feet lead the eyes
        // during a turn was tried and
        // `forward_for_the_script_is_where_the_camera_looks` rejected it in
        // one run — that invariant is older than this filter and it is what
        // stops the two conventions drifting apart again. So the whole rig
        // reads one number: the picture, the aim ray and the walk direction
        // are the same yaw, and nothing in the game can disagree with what you
        // see. The price is a transient: you walk somewhere slightly off where
        // you ended up pointing, for the ~120 ms a flick takes to settle.
        // **Bounded by [`CameraSpring::MAX_LAG`] at 6°** — a guarantee from the
        // clamp rather than a measurement of one flick, which is what the
        // earlier "~8°" here was, and it was measured at a fifth of flick speed
        // besides. At full walk speed 6° is 2.3 cm of lateral drift over a 60°
        // flick and 11 cm over a 360° spin. On a narrow deck with a rail that
        // is the thing to watch, and those are the numbers to watch it against.
        let (sin, cos) = self.view_yaw().sin_cos();
        ([-sin, 0.0, -cos], [cos, 0.0, -sin])
    }

    /// Advance by real elapsed time, in whole ticks.
    ///
    /// Returns whether anything moved, so the caller only re-derives draw calls
    /// when there is something new to draw.
    pub fn advance(&mut self, dt: f32) -> bool {
        if self.paused {
            return false;
        }
        // Clamped: a stall must not cause a thousand catch-up ticks, which
        // would stall the next frame too and spiral.
        self.leftover += dt.min(0.25);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ticks = (self.leftover / TICK_SECONDS) as u32;
        if ticks == 0 {
            return false;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.leftover -= ticks as f32 * TICK_SECONDS;
        }
        self.run(ticks);
        true
    }

    /// Advance exactly `ticks`, ignoring pause. The Step button.
    ///
    /// One tick at a time, not one batch: scripts run per tick, and a
    /// character's collide-and-slide has to happen between two steps rather
    /// than after all of them.
    ///
    /// A script that throws pauses play and says so. Carrying on would run the
    /// same failure sixty times a second, and the log line that matters would
    /// scroll away inside its own repeats.
    pub fn run(&mut self, ticks: u32) {
        // A finished game does not keep running. Play stays open on the last
        // frame so the human can see how it ended; Stop resets it.
        if self.runner.state().status().is_over() {
            return;
        }
        for _ in 0..ticks {
            self.ticks += 1;
            self.apply_look();
            let (forward, right) = self.basis();
            self.runner.input = loom_script::Motion {
                move_axis: self.input.move_axis,
                forward,
                right,
                aim: self.aim(),
                // Consumed here, so one press is one jump however the frames
                // and ticks happen to line up.
                jump: std::mem::take(&mut self.jump_pending),
                fire: std::mem::take(&mut self.fire_pending),
                interact: std::mem::take(&mut self.interact_pending),
                bag: std::mem::take(&mut self.bag_pending),
                // Consumed here like the rest — ADR 0111. A click lands in a
                // frame and the tick that takes it may not have run yet, so it
                // is latched by `set_input` and released exactly once.
                ui: std::mem::take(&mut self.ui_pending),
                sprint: self.input.sprint,
                ..loom_script::Motion::default()
            };
            if let Err(e) = self.runner.tick(&mut self.world, u64::from(self.ticks)) {
                crate::log::warn(format!("{}: {} — paused", e.script, e.message));
                self.paused = true;
                return;
            }
            if self.runner.state().status().is_over() {
                return;
            }
        }
    }

    #[must_use]
    pub fn seconds(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        {
            self.ticks as f32 * TICK_SECONDS
        }
    }

    #[must_use]
    pub fn bodies(&self) -> usize {
        self.runner.physics.body_count()
    }

    /// How many characters and scripts this run will actually drive.
    ///
    /// Logged at Play, because "nothing is moving" has two very different
    /// causes — a scene where nothing was ever going to move, and a scene
    /// where something should have — and the human cannot tell them apart by
    /// watching a clock advance.
    #[must_use]
    pub fn moving_parts(&self) -> (usize, usize) {
        (
            self.runner.character_scripts.len(),
            self.runner.node_scripts.len(),
        )
    }

    #[must_use]
    pub fn state_hash(&self) -> u64 {
        self.runner.physics.state_hash()
    }
}

#[cfg(test)]
mod tests {

    /// **The editor's flagship, tested at last.** Editing a field while Play is
    /// running works by rebuilding `Play` from the edited scene and restoring
    /// the simulation from an ADR 0088 snapshot — see `App::reapply_to_play`.
    /// That function had one call site and no test, which is exactly how the
    /// gate in front of it shipped disabled twice without anybody noticing.
    ///
    /// This is the mechanism it stands on: the rebuilt world must carry **both**
    /// the new value and the old simulation.
    #[test]
    fn an_edit_during_play_keeps_the_simulation_and_takes_the_new_value() {
        let path = std::path::Path::new("../../assets/test/jib_vi_drift.loom");
        let base = path.parent().expect("a directory");
        let source = std::fs::read_to_string(path).expect("fixture");

        let scene = Scene::parse(&source).expect("valid scene");
        let world = World::from_scene(&scene);
        let mut play = Play::start(world, base);
        play.run(90);

        let find = |play: &Play| {
            play.world
                .entities()
                .iter()
                .copied()
                .find(|e| play.world.path(*e) == Some("Sea/Boat"))
                .expect("the boat is in the scene")
        };
        let boat = |play: &Play| {
            let entity = find(play);
            let g = play.world.global_transform(entity).expect("a transform");
            [g.matrix[12], g.matrix[13], g.matrix[14]]
        };
        let drifted = boat(&play);
        assert!(
            drifted[1] != 0.40,
            "90 ticks should have moved the boat off its authored height"
        );
        let snapshot = play.save_game(90);

        // The edit a human would make while watching: retune the hull's mass.
        let edited = source.replace("mass = 57636.0", "mass = 12345.0");
        assert_ne!(edited, source, "the fixture must contain the anchor");

        let scene = Scene::parse(&edited).expect("the edited scene is still valid");
        let world = World::from_scene(&scene);
        let mut rebuilt = Play::start(world, base);
        rebuilt.load_game(&snapshot);

        // The simulation survived the rebuild...
        let restored = boat(&rebuilt);
        for axis in 0..3 {
            assert!(
                (restored[axis] - drifted[axis]).abs() < 1e-3,
                "the boat was put back where it was: {restored:?} against {drifted:?}"
            );
        }
        // ...and the new value is the one the rebuilt world was built from.
        let mass = rebuilt.world.body_mass(find(&rebuilt));
        assert!((mass - 12345.0).abs() < 1.0, "the edited mass reached the world: {mass}");
    }

    use super::*;
    use loom_scene::Scene;

    /// **The cinematic tier reaches BOTH draw paths.**
    ///
    /// The model is `particles.rs`'s
    /// `the_jet_reaches_both_paths_and_arrives_after_the_crown`, and the debt
    /// is the same: `set_ripples` shipped tested-and-uncalled (ADR 0046 §7),
    /// the splash crown was correct headless and drawn nowhere on the window
    /// path, and the cinematic free surface made it three — the solver stepped
    /// inside `loom run`'s fixed step for four slices while nothing marched or
    /// uploaded the result (defect 5 of the water rebuild review, found the way
    /// the other two were: by a human opening the window).
    ///
    /// **It reads source text, and that is deliberate rather than lazy.** What
    /// broke is *wiring*, and wiring is what a unit test cannot reach here:
    /// stepping the solver needs a Vulkan device, and `Viewer` needs a
    /// swapchain on top of that, so neither draw path can be constructed in
    /// `cargo test` at all. What a judge did by hand — grep `run.rs` for a
    /// fluid reference and find none — is therefore the strongest check
    /// available, and it is precisely the check that would have failed. The
    /// precedent for the technique is `loom_agent`'s
    /// `every_tool_wraps_a_real_subcommand`.
    ///
    /// Two layers per path, because the defect was two layers deep: the CLI has
    /// to *ask* the simulation for the surface, and the renderer it hands it to
    /// has to *draw* it.
    #[test]
    fn the_cinematic_surface_reaches_both_draw_paths() {
        let cli_headless = include_str!("main.rs");
        let cli_window = include_str!("run.rs");
        let offscreen = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../loom_render/src/renderer.rs"));
        let window = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../loom_render/src/viewer.rs"));
        let required = [
            // The headless still and the fly-through.
            (cli_headless, "loom render", "fluid_draw"),
            (cli_headless, "loom render", "set_fluid_surface("),
            // The window the human actually watches.
            (cli_window, "loom run", "fluid_draw"),
            (cli_window, "loom run", "set_fluid_surface("),
            // And the renderer behind each: an upload feeding a draw nobody
            // records is the same defect one floor down.
            (offscreen, "the offscreen renderer", "fluid_verts"),
            (window, "the window renderer", "fluid_verts"),
        ];
        for (source, path, needle) in required {
            assert!(
                source.contains(needle),
                "{path} does not mention `{needle}` — the cinematic tier draws on one path only, \
                 which is the defect this test exists to make impossible (ADR 0046 §7)"
            );
        }
    }

    const FALLING: &str = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = { pos = [0.0, -0.5, 0.0], scale = [10.0, 0.5, 10.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Crate"
parent = "Stage"
transform = { pos = [0.0, 6.0, 0.0], scale = [0.5, 0.5, 0.5] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

  [node.components.RigidBody]
  dynamic = true
  mass = 10.0
"#;


    /// **The window's own reading of the sea, which nothing else can check.**
    /// `loom run` is the only caller of `Play::sea_state` and it cannot be run
    /// headless, so without this the live half of the sea's sound is asserted
    /// by nobody.
    ///
    /// Three claims, and the third is the one worth having: a scene with no
    /// water reads a flat calm; a breaking sea reports a breaking fraction; and
    /// an FFT sea's height is the **cascade's** `4√m0` and not the sixteen
    /// derived waves' analytic value. Those two answer for different seas and
    /// only one of them is under the boat.
    #[test]
    fn the_sea_state_is_read_off_the_run() {
        let load = |name: &str| {
            let src = std::fs::read_to_string(format!("../../assets/test/{name}.loom"))
                .expect("fixture");
            World::from_scene(&Scene::parse(&src).expect("valid scene"))
        };
        let base = std::path::Path::new("../../assets/test");

        let mut dry = Play::start(load("cave"), base);
        dry.run(30);
        assert_eq!(
            dry.sea_state(),
            loom_audio::sea::SeaState::default(),
            "a scene with no water has to read a flat calm, or it will not be silent"
        );

        let mut breaking = Play::start(load("whitecaps"), base);
        breaking.run(300);
        let state = breaking.sea_state();
        assert!(state.hs > 1.0, "whitecaps reported a millpond: Hs {}", state.hs);
        assert!(
            state.breaking > 0.0,
            "a sea named for its whitecaps reported nothing breaking: {}",
            state.breaking
        );

        let mut fft = Play::start(load("ocean_fft"), base);
        fft.run(60);
        let state = fft.sea_state();
        let cascade = fft.runner.sea().expect("ocean_fft opts into the cascade").significant_height();
        let derived = loom_water::spectrum::significant_height(
            &fft.runner.water().expect("a water body").waves,
        );
        assert!(
            (state.hs - cascade).abs() < 1e-6,
            "the bed was handed {} where the cascade is at {cascade}",
            state.hs
        );
        assert!(
            (cascade - derived).abs() > 1e-3,
            "the two heights agree to {}, so this test could not tell them apart",
            (cascade - derived).abs()
        );
    }

    /// **The crossing sea `ocean_fft` authors, and the one measurement this
    /// engine has never been able to take.**
    ///
    /// Three claims, and the third is the one worth having.
    ///
    /// 1. **The swell reaches the ocean.** A `swell` table that the scene
    ///    parses, the schema accepts and `Ocean::for_body` drops on the floor
    ///    would look exactly like a swell that works — same `ok: true`, same
    ///    picture at a glance. Here the split sea is *measured* against the
    ///    single sea it replaced.
    /// 2. **The height did not move, and the breaking did.** 440 km in one sea
    ///    against 420 + 20 split between a swell and a wind sea is the same
    ///    `m0` by construction, because `Hs ∝ √F` at a fixed wind and fetches
    ///    therefore add in energy. So `Hs` is held to 2% while the fraction of
    ///    the surface past `FOAM_CREST_BREAK` goes from nothing at all to
    ///    something — one fetch cannot carry both the height and the short
    ///    steep tail that breaks.
    /// 3. **The trace and the eigenvalue disagree, on a real crossing sea.**
    ///    `WaterSample::mu_max` is the largest eigenvalue of the horizontal
    ///    compression rather than its trace (`WaterSample::fold`) precisely to
    ///    handle a sea running two ways at once, and until this scene existed
    ///    there was no crossing sea in the repository to prove it on: the
    ///    12.32%-against-0.00% figure in `mu_max`'s docs is a synthetic pair
    ///    of swells, not a sea anything floats in.
    ///
    /// # Measured, at `SIDE = 401` over the 2048 m tile, tick 400
    ///
    /// ```text
    ///                          Hs        mean mu_max   past 0.45 by
    ///                                                  eigenvalue   trace
    /// 440 km, one sea          5.957 m   0.0515        0.0006%      0.0093%
    /// 420 + 20, swell along    5.977 m   0.0843        0.1766%      0.9129%
    /// 420 + 20, swell at 60°   5.897 m   0.0842        0.1007%      0.8800%
    /// ```
    ///
    /// **`Hs` is a single draw and the three rows differ by 1%, which is noise
    /// rather than energy.** This spectrum is narrow, so one seed is a small
    /// sample: `loom_water::ocean`'s `the_realised_sea_is_the_size_the_spectrum
    /// _promised` measures the single-draw spread at 0.35x to 1.68x and needs
    /// 200 seeds to land on the analytic 6.100 m. The 2% bound below is a check
    /// that the *split* did not move the energy — which it cannot, because
    /// `m0 ∝ F` — not a claim that a single draw is the analytic height.
    ///
    /// **The middle row is the control, and it is what makes the third row
    /// mean anything.** A spectrum sea carries a `cos^p` directional spread,
    /// so its compression is never purely one-axis and the trace over-reports
    /// a little on *any* of these. What isolates the crossing is turning the
    /// swell 60° and changing nothing else: **the eigenvalue falls 43%
    /// (0.1766 → 0.1007) and the trace falls 3.6% (0.9129 → 0.8800)**. The
    /// trace is very nearly blind to the difference between a sea running one
    /// way and a sea running two, which is exactly the failure `mu_max` was
    /// made an eigenvalue to avoid: it would paint the same whitecaps on both,
    /// and on the crossing sea 89% of them (0.8800 against 0.1007) would be
    /// on water where neither axis has folded.
    ///
    /// The two are 8.7x apart on the crossing sea against 5.2x on the aligned
    /// one — the gap widens with the crossing, which is the direction the
    /// argument predicts and the reason the assertion below is a ratio.
    ///
    /// **`SIDE` is prime and that is a measurement rather than a preference.**
    /// Every cascade's patch is a power of two, so a power-of-two side steps
    /// the 32 m chop tile by a whole number of cells and samples one
    /// sublattice of it forever. Measured on this scene's crossing sea:
    ///
    /// ```text
    /// side   mean mu_max   eigenvalue   trace
    ///  128   0.0786        0.0000%      0.9338%     <- power of two
    ///  512   0.0917        0.1644%      1.5812%     <- power of two
    ///  257   0.0842        0.1014%      0.8933%
    ///  401   0.0842        0.1007%      0.8800%     <- this
    ///  601   0.0842        0.0963%      0.9028%
    /// 1009   0.0842        0.0978%      0.9010%
    /// ```
    ///
    /// The four primes agree on the mean to four decimals and on the tail to
    /// about 5%. **128 reports that this sea does not break at all** and 512
    /// reports 63% more breaking than it has, from nothing but where their
    /// points landed. An aliased sample of a periodic tile is not a small
    /// error, it is a different sea.
    #[test]
    fn the_crossing_sea_breaks_and_its_trace_over_reports_it() {
        const SIDE: u16 = 401;
        let src = std::fs::read_to_string("../../assets/test/ocean_fft.loom")
            .expect("the scene");
        let world = World::from_scene(&Scene::parse(&src).expect("valid scene"));
        let wind = crate::weather::wind_of_world(&world);
        let split = crate::weather::water_of(&world, &wind).expect("the scene has water");
        assert!(split.swell.is_some(), "the scene stopped authoring a swell");

        // The scene as it was before the split: one 440 km sea, no swell. The
        // two fetches sum to it exactly, which is what makes `Hs` the control.
        let single = loom_scene::components::WaterBody {
            fetch: Some(440_000.0),
            swell: None,
            ..split.clone()
        };

        // The instant `loom water --at ... --sim 400` reports on, so the
        // numbers here and the numbers on the command line are one measurement.
        let t = 400.0 / 60.0;
        // Across the largest cascade's patch, so the swell is sampled over
        // whole periods rather than over one flank of one crest.
        let step = 2048.0 / f32::from(SIDE);
        // `(Hs, past-threshold on mu_max, past-threshold on fold, mean mu_max)`.
        let measure = |body: &loom_scene::components::WaterBody| {
            let mut sea = crate::weather::sea_of_body(body, &wind).expect("a spectrum sea");
            sea.evolve(t);
            let (mut eigen, mut trace, mut n, mut sum) = (0_u32, 0_u32, 0_u32, 0.0_f64);
            for iz in 0..SIDE {
                for ix in 0..SIDE {
                    let at = [f32::from(ix) * step, f32::from(iz) * step];
                    let s = loom_water::sample_water(
                        body,
                        Some(&sea),
                        at,
                        t,
                        -1000.0,
                        [0.0; 3],
                        [0.0; 3],
                    );
                    n += 1;
                    sum += f64::from(s.mu_max);
                    if s.mu_max > loom_water::foam::FOAM_CREST_BREAK {
                        eigen += 1;
                    }
                    if s.fold > loom_water::foam::FOAM_CREST_BREAK {
                        trace += 1;
                    }
                }
            }
            let total = f64::from(n);
            let pct = |c: u32| f64::from(c) * 100.0 / total;
            (sea.significant_height(), pct(eigen), pct(trace), sum / total, eigen, trace, n)
        };

        // **The control that attributes the gap to the crossing.** The same
        // split with the swell laid along the wind instead of 60° across it.
        // Without it the comparison proves nothing: a spectrum sea carries a
        // `cos^p` directional spread, so its compression is never purely
        // one-axis and the trace over-reports a little even on one sea. What
        // the eigenvalue exists for is the crossing, and this row is what
        // isolates it.
        let mut aligned = split.clone();
        let params = wind.params();
        if let Some(swell) = aligned.swell.as_mut() {
            swell.direction = [params.get("dir_x"), params.get("dir_z")];
        }

        let (hs_one, eigen_one, trace_one, mean_one, ne_one, nt_one, n) = measure(&single);
        let (hs_al, eigen_al, trace_al, mean_al, ne_al, nt_al, _) = measure(&aligned);
        let (hs_two, eigen_two, trace_two, mean_two, ne_two, nt_two, _) = measure(&split);
        println!(
            "at side {SIDE} ({n} points), past FOAM_CREST_BREAK = 0.45:\n\
             440 km, one sea:        Hs {hs_one:.3} m  mean mu_max {mean_one:.4}  \
             eigenvalue {ne_one} ({eigen_one:.4}%)  trace {nt_one} ({trace_one:.4}%)\n\
             420 + 20, swell along:  Hs {hs_al:.3} m  mean mu_max {mean_al:.4}  \
             eigenvalue {ne_al} ({eigen_al:.4}%)  trace {nt_al} ({trace_al:.4}%)\n\
             420 + 20, swell at 60:  Hs {hs_two:.3} m  mean mu_max {mean_two:.4}  \
             eigenvalue {ne_two} ({eigen_two:.4}%)  trace {nt_two} ({trace_two:.4}%)"
        );

        assert!(
            (hs_two - hs_one).abs() / hs_one < 0.02,
            "the split moved the sea's height: {hs_one:.4} m to {hs_two:.4} m. \
             Fetches add in energy, so 420 + 20 has to be 440 — a height that \
             moved means the swell is not being summed as a second variance"
        );
        // 0.005% is eight of the 160,801 points; the measured figure is one.
        // Not zero, because a floor at exactly zero would fail on a lattice
        // that happened to land on the single crest this sea does have.
        assert!(
            eigen_one < 0.005,
            "the single 440 km sea already breaks at {eigen_one:.4}%, so this \
             test cannot show that the split is what makes it break"
        );
        // Half the measured 0.1007%, which is 168x the row above it.
        assert!(
            eigen_two > 0.05,
            "the crossing sea puts {eigen_two:.4}% past FOAM_CREST_BREAK, \
             which is the same nothing the single sea puts there — the swell \
             is not reaching the amplitude field"
        );
        assert!(
            trace_two > eigen_two * 4.0,
            "the trace calls {trace_two:.4}% breaking against the eigenvalue's \
             {eigen_two:.4}%, measured 8.7x apart. If they agree here, either \
             the swell is absent or `mu_max` has quietly become the trace and \
             the whole reason it is an eigenvalue is gone"
        );
        // **The gap has to WIDEN with the crossing**, or the trace's
        // over-report is just the directional spread and this sea proves
        // nothing about eigenvalues. Measured **12.2x aligned against 15.6x at
        // 60°** — the trace counts the same 1387 cells either way and cannot
        // tell the two seas apart at all, while the eigenvalue falls 114 -> 89.
        //
        // Those figures were 5.2x and 8.7x while `spectrum::SPREAD_PEAK` was a
        // flat `cos²`. Concentrating the peak made each sea more one-axis, so
        // the trace's blindness to heading costs it more, not less.
        assert!(
            trace_two / eigen_two > trace_al / eigen_al,
            "turning the swell 60° across the wind left the trace and the \
             eigenvalue no further apart than laying it along: {:.2}x aligned, \
             {:.2}x crossing. The crossing is then doing nothing to the \
             breaking criterion, which is the one thing this scene exists to \
             show",
            trace_al / eigen_al,
            trace_two / eigen_two
        );
        // **0.85, and it was 0.75 against a flat `cos²` spread.** The drop is
        // 22% now (114 -> 89) where it was 43%, and the reason is the same
        // narrowing that widened the ratio above: a more directional sea folds
        // harder along its *own* axis, so the crossing case's larger eigenvalue
        // keeps more of what the aligned case has. The claim being made here is
        // only that the eigenvalue notices the crossing **at all** — the trace,
        // at an identical 1387 cells either way, does not. The size of the
        // notice is the assertion above, and it got stronger.
        assert!(
            eigen_two < eigen_al * 0.85,
            "the eigenvalue read {eigen_two:.4}% on the crossing sea against \
             {eigen_al:.4}% on the aligned one, measured 22% down. A crossing \
             sea compresses two axes at once and neither has folded as far as \
             the one axis did — an eigenvalue that does not notice is a trace"
        );
    }

    fn world() -> World {
        World::from_scene(&Scene::parse(FALLING).expect("valid scene"))
    }

    /// Run the crate scene and report where `Sea/Crate` is every tick.
    fn crate_trajectory(source: &str, ticks: u32) -> Vec<[f32; 3]> {
        let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));
        (0..ticks)
            .map(|_| {
                play.run(1);
                let entity = play
                    .world
                    .entities()
                    .iter()
                    .copied()
                    .find(|e| play.world.path(*e) == Some("Sea/Crate"))
                    .expect("the crate");
                play.world.transform(entity).expect("a transform").pos
            })
            .collect()
    }

    /// How far the sea itself rises and falls at one spot over a tick range —
    /// the thing a floating crate's motion has to be compared against, because
    /// a crate on a real sea is supposed to move.
    fn sea_travel(source: &str, at: [f32; 3], ticks: std::ops::Range<u32>) -> f32 {
        let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
        let water = crate::weather::water_of(&world, &crate::weather::wind_of_world(&world))
            .expect("the scene has water");
        #[allow(clippy::cast_precision_loss)]
        let heights: Vec<f32> = ticks
            .map(|tick| {
                loom_water::sample_water(&water, None, [at[0], at[2]], tick as f32 * TICK_SECONDS, 0.0, [0.0; 3], [0.0; 3])
                    .height
            })
            .collect();
        peak_to_peak(&heights)
    }

    /// Peak-to-peak vertical travel over a slice of a run, in metres.
    fn peak_to_peak(window: &[f32]) -> f32 {
        let high = window.iter().copied().fold(f32::MIN, f32::max);
        let low = window.iter().copied().fold(f32::MAX, f32::min);
        high - low
    }

    fn height(world: &World, path: &str) -> f32 {
        world
            .entities()
            .iter()
            .find(|e| world.path(**e) == Some(path))
            .and_then(|e| world.transform(*e))
            .map(|t| t.pos[1])
            .expect("node exists")
    }

    const TILTED_BALL: &str = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = { pos = [0.0, -0.5, 0.0], scale = [10.0, 0.5, 10.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Ball"
parent = "Stage"
transform = { pos = [0.0, 4.0, 0.0], rot_euler = [0.0, 0.0, 45.0], scale = [0.7, 0.7, 0.7] }

  [node.components.MeshRenderer]
  mesh = { asset = "sphere" }

  [node.components.RigidBody]
  dynamic = true
  mass = 10.0
"#;

    /// **The collider has to match the mesh.** A sphere simulated as a cube
    /// rests on whichever face is down, so a tilted one settles its centre at
    /// `radius * sqrt(2)` instead of `radius` — and the rendered sphere then
    /// hangs in the air or sinks into whatever it landed on, which is exactly
    /// what a screenshot of the tower scene showed.
    ///
    /// Rotation is what makes this test discriminating: an axis-aligned cube
    /// and a ball of the same radius rest at the same height, so an upright
    /// sphere proves nothing.
    #[test]
    fn a_sphere_rests_at_its_radius_however_it_is_turned() {
        let world = World::from_scene(&Scene::parse(TILTED_BALL).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));

        play.run(240);
        let y = height(&play.world, "Stage/Ball");

        // Ground top is y = 0, so a ball of radius 0.7 rests at 0.7.
        // A 45-degree cube of half-extent 0.7 would rest at 0.99.
        assert!(
            (y - 0.7).abs() < 0.05,
            "a ball should rest at its radius, not at a cube corner: y = {y}"
        );
    }

    /// The other half of the same rule: a box must keep its box. Turning every
    /// collider into a ball would pass the test above and be just as wrong.
    #[test]
    fn a_tilted_box_still_rests_on_its_corner() {
        let source = TILTED_BALL.replace("asset = \"sphere\"", "asset = \"box\"");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));

        play.run(240);
        let y = height(&play.world, "Stage/Ball");

        assert!(
            y > 0.85,
            "a cube tilted 45 degrees rests on an edge, higher than its half-extent: y = {y}"
        );
    }

    /// **Destructible terrain has to be solid.** A `VoxelVolume` node carried
    /// no collider matching its shape — it was marked renderable, so it got a
    /// cuboid sized from its `scale`, which for an untransformed node is a
    /// 1x1x1 box standing in for a 32x24x32 hillside. Everything else fell
    /// straight through it, in the editor and in `loom sim` alike.
    ///
    /// `CLAUDE.md`'s locked decision has said "voxel colliders for terrain"
    /// since M0; this is that, finally wired up.
    #[test]
    fn a_voxel_volume_is_solid_to_physics() {
        let cave = std::fs::read_to_string("../../assets/test/cave.loom").expect("fixture");
        let source = format!(
            "{cave}\n[[node]]\nname = \"Probe\"\nparent = \"Terrain\"\n\
             transform = {{ pos = [16.0, 26.0, 16.0], scale = [0.5, 0.5, 0.5] }}\n\n\
               [node.components.MeshRenderer]\n  mesh = {{ asset = \"box\" }}\n\n\
               [node.components.RigidBody]\n  dynamic = true\n  mass = 5.0\n"
        );
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));

        play.run(400);
        let y = height(&play.world, "Terrain/Probe");

        // The hill's summit is around y = 15.5; the ground slab spans 0..6.
        // Anything above zero means it landed on the terrain rather than
        // through it.
        assert!(y > 1.0, "the probe fell through the voxel terrain: y = {y}");
    }

    /// **W8's exit criterion, as a test rather than a command line.** A crate
    /// dropped upstream arrives downstream, on a river whose course nobody
    /// drew: the channel is carved into a voxel volume, the drainage is
    /// computed off the bed that carving produced, and the current reaches the
    /// crate through the drag term the solver already had.
    ///
    /// Measured over 600 ticks — ten seconds — from x = 6.0:
    ///
    /// - with the flow: **x = 17.7**, and the crate stays in the channel
    /// - with `speed = 0.0`: **x = 6.00**, which is where it was dropped
    ///
    /// The second half is the mutation check, and it is in here rather than in
    /// a comment because an assertion that cannot fail is decoration. The
    /// threshold sits at 14 — well clear of both numbers — so a real slowdown
    /// fails it and float noise does not.
    /// **The claim networking actually makes.** ADR 0091: two machines that
    /// step the same scene with the same inputs have the same world, so the
    /// wire carries intents and never state.
    ///
    /// This is that sentence as a test. Two real `Session`s over a real socket,
    /// two independent `Runner`s, different intents from each peer, and the
    /// only thing tying them together is the exchange. If the worlds ever
    /// disagree, lockstep is not a design this engine can use — so nothing
    /// about it is worth trusting until this passes.
    #[test]
    fn two_lockstep_peers_simulate_the_same_world() {
        use loom_net::{Intent, Session};

        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        for _ in 0..200 {
            host.poll(0);
            client.poll(0);
            if client.me() != u8::MAX && host.roster().len() == 2 && client.roster().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(host.roster().len(), 2, "both peers are on the roster");

        let source =
            std::fs::read_to_string("../../assets/games/proving_ground.loom").expect("fixture");
        let base = std::path::Path::new("../../assets/games");
        let mut seen = std::collections::BTreeSet::new();
        let mut peers = Vec::new();
        for _ in 0..2 {
            let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
            let runner = Runner::new(&world, base).expect("scripts load");
            peers.push((world, runner));
        }

        // Two humans doing different things, changing over time — a constant
        // input would pass even if the intents were being dropped.
        let intent = |peer: u8, tick: u64| {
            let phase = (tick as f32) * 0.05 + f32::from(peer);
            Intent {
                move_axis: [phase.sin(), phase.cos()],
                forward: [0.0, 0.0, 1.0],
                right: [1.0, 0.0, 0.0],
                aim: [0.0, 0.0, 1.0],
                buttons: 0,
                // A menu press now rides the same frame, and varies per peer
                // and per tick like everything else here — a constant would
                // pass even if the byte were being dropped.
                #[allow(clippy::cast_possible_truncation)]
                ui: ((tick + u64::from(peer)) % 4) as u8,
            }
            .with_buttons(tick % 37 == u64::from(peer), false, tick.is_multiple_of(53), false)
        };

        for tick in 0..(loom_net::INPUT_DELAY + 120) {
            host.send_intent(tick, intent(0, tick));
            client.send_intent(tick, intent(1, tick));

            if tick < loom_net::INPUT_DELAY {
                continue;
            }

            // Wait for the tick to be complete on both machines. A missing
            // input is a wait, not a skip.
            let mut ready = None;
            for _ in 0..500 {
                host.poll(tick);
                client.poll(tick);
                if let (Some(a), Some(b)) = (host.ready(tick), client.ready(tick)) {
                    assert_eq!(a, b, "both machines see the same inputs at {tick}");
                    ready = Some(a);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let inputs = ready.unwrap_or_else(|| panic!("tick {tick} never completed"));

            let mut hashes = Vec::new();
            for (world, runner) in &mut peers {
                runner.driven.clear();
                for (peer, asked) in &inputs {
                    let motion = loom_script::Motion {
                        move_axis: asked.move_axis,
                        forward: asked.forward,
                        right: asked.right,
                        aim: asked.aim,
                        jump: asked.jump(),
                        sprint: asked.sprint(),
                        fire: asked.fire(),
                        interact: asked.interact(),
                        ..loom_script::Motion::default()
                    };
                    if *peer == 0 {
                        runner.input = motion.clone();
                    }
                    runner.driven.insert(usize::from(*peer), motion);
                }
                runner.tick(world, tick).expect("tick");
                hashes.push(world.state_hash());
            }

            assert_eq!(
                hashes[0], hashes[1],
                "the two peers disagree about the world at tick {tick}"
            );
            seen.insert(hashes[0]);
            host.report(tick, hashes[0]);
            client.report(tick, hashes[1]);
        }

        // **Two worlds that never moved agree trivially.** Without this the
        // test would pass on a scene with no characters, a runner that failed
        // to load, or a loop that never ran — which is the shape of a check
        // that proves nothing.
        assert!(
            seen.len() > 100,
            "the world should be changing every tick; saw {} distinct states",
            seen.len()
        );

        // And the sessions agree too, which is the check a real game ships with.
        let desyncs: Vec<_> = host
            .drain()
            .into_iter()
            .chain(client.drain())
            .filter(|e| matches!(e, loom_net::Event::Desync { .. }))
            .collect();
        assert!(desyncs.is_empty(), "hash exchange reported a desync: {desyncs:?}");
    }

    #[test]
    fn a_crate_dropped_in_a_river_ends_up_downstream() {
        let source = std::fs::read_to_string("../../assets/test/river.loom").expect("fixture");
        let travel = |source: &str| {
            let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
            let mut play = Play::start(world, std::path::Path::new("."));
            play.run(600);
            let entity = play
                .world
                .entities()
                .iter()
                .copied()
                .find(|e| play.world.path(*e) == Some("River/Crate"))
                .expect("the crate is in the scene");
            play.world.transform(entity).expect("it has a transform").pos
        };

        let carried = travel(&source);
        assert!(
            carried[0] > 14.0,
            "the river did not carry the crate downstream: x = {}",
            carried[0]
        );
        // And it stayed in the river rather than being pushed at the bank,
        // which is the failure the flow field's direction smoothing exists to
        // prevent — the channel runs down z = 24.
        assert!(
            (carried[2] - 24.0).abs() < 1.5,
            "the crate was pushed out of the channel: z = {}",
            carried[2]
        );

        // Zero the one authored number and the claim has to stop holding.
        let still = travel(&source.replace(
            "[node.components.WaterBody.flow]\n  speed = 3.0",
            "[node.components.WaterBody.flow]\n  speed = 0.0",
        ));
        assert!(
            still[0] < 7.0,
            "a river with no speed still moved the crate: x = {} — \
             the mutation did not take, so the assertion above proves nothing",
            still[0]
        );
    }

    /// `BoxCollider` is documented and schema-validated, and the simulation
    /// ignored it — collider size always came from the node's scale. A scene
    /// could declare a collider twice the size of its mesh and collide as the
    /// mesh, with nothing reporting the discrepancy.
    ///
    /// **The half-extents are LOCAL and are multiplied by the node's world
    /// scale**, which is what the comment at the call site has always said and
    /// what `rain_collision_field` — the other reader of the same component —
    /// has always done. This test used to assert the opposite by accident: at
    /// `scale 0.5` with `half_extents [0.5, 2.0, 0.5]` it expected the crate to
    /// rest at 2.0, which is only true if the scale is dropped. The world half
    /// height is `2.0 × 0.5 = 1.0`, so it rests at 1.0, and the number is the
    /// falsifier — the old behaviour lands at 2.0 and fails here.
    #[test]
    fn an_authored_box_collider_beats_the_mesh_scale() {
        let scene = |collider: &str| {
            format!(
                r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = {{ pos = [0.0, -0.5, 0.0], scale = [10.0, 0.5, 10.0] }}

  [node.components.MeshRenderer]
  mesh = {{ asset = "box" }}

[[node]]
name = "Crate"
parent = "Stage"
transform = {{ pos = [0.0, 6.0, 0.0], scale = [0.5, 0.5, 0.5] }}

  [node.components.MeshRenderer]
  mesh = {{ asset = "box" }}
{collider}
  [node.components.RigidBody]
  dynamic = true
  mass = 4.0
"#
            )
        };

        let rest = |source: &str| {
            let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
            let mut play = Play::start(world, std::path::Path::new("."));
            play.run(400);
            height(&play.world, "Stage/Crate")
        };

        let from_scale = rest(&scene(""));
        let declared = rest(&scene(
            "\n  [node.components.BoxCollider]\n  half_extents = [1.0, 2.0, 1.0]\n",
        ));

        assert!((from_scale - 0.5).abs() < 0.05, "scale-sized: {from_scale}");
        assert!(
            (declared - 1.0).abs() < 0.05,
            "the declared collider is 2.0 half-tall in local space under a scale \
             of 0.5, so it rests at 1.0, not {declared}"
        );
    }

    /// **World and local are different spaces.** Colliders were built from the
    /// global matrix for position but the local transform for rotation and
    /// scale, and results were written back into the local transform as if it
    /// were world. Under a parent that is moved at all, the two disagree: a
    /// crate under a parent at x = 10 was written back at world x = 10 into a
    /// local slot, which re-composes to world x = 20, and it walks away one
    /// parent-offset per tick.
    #[test]
    fn a_body_under_a_moved_parent_stays_where_the_physics_put_it() {
        let source = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Root"

[[node]]
name = "Ground"
parent = "Root"
transform = { pos = [0.0, -0.5, 0.0], scale = [40.0, 0.5, 40.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Rig"
parent = "Root"
transform = { pos = [10.0, 0.0, -4.0] }

[[node]]
name = "Crate"
parent = "Root/Rig"
transform = { pos = [0.0, 6.0, 0.0], scale = [0.5, 0.5, 0.5] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

  [node.components.RigidBody]
  dynamic = true
  mass = 4.0
"#;
        let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));
        play.run(240);

        let entity = *play
            .world
            .entities()
            .iter()
            .find(|e| play.world.path(**e) == Some("Root/Rig/Crate"))
            .expect("crate exists");
        let global = play.world.global_transform(entity).expect("has a global");
        let (x, y, z) = (global.matrix[12], global.matrix[13], global.matrix[14]);

        // It was dropped at world (10, 6, -4) over a floor whose top is y = 0.
        // It should land under itself, not slide one parent-offset per tick.
        assert!((x - 10.0).abs() < 0.2, "drifted in x: {x}");
        assert!((z + 4.0).abs() < 0.2, "drifted in z: {z}");
        assert!((y - 0.5).abs() < 0.1, "should rest on the floor: {y}");
    }

    /// A cylinder tipped past its balance point rolls off a ledge; a cuboid of
    /// the same size sits there. Same bug as sphere-as-cuboid, one step less
    /// obvious.
    #[test]
    fn a_cylinder_is_round_to_physics() {
        let scene = |mesh: &str| {
            format!(
                r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = {{ pos = [0.0, -0.5, 0.0], scale = [20.0, 0.5, 20.0] }}

  [node.components.MeshRenderer]
  mesh = {{ asset = "box" }}

[[node]]
name = "Roller"
parent = "Stage"
transform = {{ pos = [0.0, 3.0, 0.0], rot_euler = [0.0, 0.0, 80.0], scale = [0.5, 0.5, 0.5] }}

  [node.components.MeshRenderer]
  mesh = {{ asset = "{mesh}" }}

  [node.components.RigidBody]
  dynamic = true
  mass = 6.0
"#
            )
        };
        let travel = |mesh: &str| {
            let world = World::from_scene(&Scene::parse(&scene(mesh)).expect("valid scene"));
            let mut play = Play::start(world, std::path::Path::new("."));
            play.run(300);
            let entity = *play
                .world
                .entities()
                .iter()
                .find(|e| play.world.path(**e) == Some("Stage/Roller"))
                .expect("node");
            let g = play.world.global_transform(entity).expect("global");
            g.matrix[12].abs() + g.matrix[14].abs()
        };

        let cylinder = travel("cylinder");
        let cube = travel("box");
        assert!(
            cylinder > cube + 0.05,
            "a tipped cylinder should roll further than a cube: {cylinder} vs {cube}"
        );
    }

    /// A child of a moving body is not scenery. It used to get its own static
    /// collider at the position it spawned in — an invisible wall left where
    /// the parent started, which everything else then bumped into.
    #[test]
    fn a_child_of_a_dynamic_body_leaves_no_ghost_behind() {
        let source = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = { pos = [0.0, -0.5, 0.0], scale = [20.0, 0.5, 20.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Lift"
parent = "Stage"
transform = { pos = [0.0, 8.0, 0.0], scale = [0.5, 0.5, 0.5] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

  [node.components.RigidBody]
  dynamic = true
  mass = 4.0

[[node]]
name = "Flag"
parent = "Stage/Lift"
transform = { pos = [0.0, 2.0, 0.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Probe"
parent = "Stage"
transform = { pos = [0.0, 20.0, 0.0], scale = [0.3, 0.3, 0.3] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

  [node.components.RigidBody]
  dynamic = true
  mass = 2.0
"#;
        let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));
        play.run(400);

        // The flag spawned at world y = 10 and rode its parent down. Nothing
        // should still be blocking that height: the probe must reach the pile
        // on the floor, not perch on a ghost.
        let y = height(&play.world, "Stage/Probe");
        assert!(y < 4.0, "the probe stopped on something that is not there: {y}");
    }

    /// A voxel volume is drawn with its node's full transform, so its collider
    /// needs the same one. Passing only the position left a scaled hillside
    /// colliding at its unscaled size — the same lie as a sphere colliding as
    /// a cube, which is what most of today's physics work was about.
    #[test]
    fn a_scaled_voxel_volume_collides_at_its_drawn_size() {
        let scene = |scale: f32, drop_from: f32| {
            format!(
                r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Hill"
parent = "Stage"
transform = {{ scale = [{scale}, {scale}, {scale}] }}

  [node.components.VoxelVolume]
  voxel_size = 0.25
  chunks = [1, 1, 1]

    [[node.components.VoxelVolume.ops]]
    kind = "box"
    center = [4.0, 1.0, 4.0]
    half_extents = [3.0, 1.0, 3.0]
    mode = "union"

[[node]]
name = "Probe"
parent = "Stage"
transform = {{ pos = [4.0, {drop_from}, 4.0], scale = [0.25, 0.25, 0.25] }}

  [node.components.MeshRenderer]
  mesh = {{ asset = "box" }}

  [node.components.RigidBody]
  dynamic = true
  mass = 3.0
"#
            )
        };
        let rest = |scale: f32, drop_from: f32| {
            let world = World::from_scene(&Scene::parse(&scene(scale, drop_from)).expect("valid"));
            let mut play = Play::start(world, std::path::Path::new("."));
            play.run(500);
            height(&play.world, "Stage/Probe")
        };

        // Unscaled the slab's top is y = 2; doubled it is y = 4.
        let plain = rest(1.0, 8.0);
        let doubled = rest(2.0, 12.0);

        assert!((plain - 2.25).abs() < 0.3, "unscaled top ~2.0: {plain}");
        assert!(
            doubled > plain + 1.0,
            "a volume scaled 2x must collide twice as tall: {doubled} vs {plain}"
        );
    }

    /// The capsule collider must be as tall as the capsule that is drawn.
    /// `primitives::capsule` puts hemispheres of `radius` at ±`half_height`,
    /// so the shape spans `half_height + radius`; subtracting the radius before
    /// handing it to parry made every capsule one radius short and, at the
    /// default scale, collapsed the straight section to nothing.
    #[test]
    fn a_capsule_rests_at_the_height_it_is_drawn() {
        let source = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Stage"

[[node]]
name = "Ground"
parent = "Stage"
transform = { pos = [0.0, -0.5, 0.0], scale = [20.0, 0.5, 20.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Pill"
parent = "Stage"
transform = { pos = [0.0, 6.0, 0.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "capsule" }

  [node.components.RigidBody]
  dynamic = true
  mass = 5.0
"#;
        let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("."));
        play.run(400);

        // Unit capsule: straight half-height 1, radius 1, so it spans ±2 and
        // its centre rests at 2 on a floor whose top is y = 0.
        let y = height(&play.world, "Stage/Pill");
        assert!(
            (y - 2.0).abs() < 0.15,
            "a unit capsule rests at 2.0, not {y} — the collider is the wrong height"
        );
    }

    /// A determinant threshold is scale-dependent and this is not. A node
    /// uniformly scaled by 1e-5 has determinant 1e-15 — below the 1e-12 cutoff
    /// that used to guard this — and is perfectly invertible; snapping it to
    /// identity would have quietly moved a legitimately tiny node to its
    /// parent's origin. Only a genuinely singular matrix falls back.
    #[test]
    fn only_a_truly_singular_parent_falls_back() {
        use loom_render::glam::{Mat4, Quat, Vec3};

        for scale in [1.0_f32, 1e-2, 1e-4, 1e-5, 1e-6] {
            let m = Mat4::from_scale_rotation_translation(
                Vec3::splat(scale),
                Quat::IDENTITY,
                Vec3::new(3.0, 4.0, 5.0),
            );
            let inverse = super::invertible_parent(m);
            // Asserting `inverse * m == identity` would be testing f32
            // precision, not the fallback: at scale 1e-6 the inverse carries
            // entries around 1e6 and the product drifts. The property here is
            // simply that it did NOT give up and return identity.
            assert!(inverse.is_finite(), "scale {scale} produced a non-finite inverse");
            assert_ne!(
                inverse,
                Mat4::IDENTITY,
                "scale {scale} is invertible and must not fall back"
            );
        }

        // Flattened on one axis: genuinely singular, and identity is the only
        // finite answer available.
        let flat = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 0.0, 1.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        assert_eq!(super::invertible_parent(flat), Mat4::IDENTITY);
        assert!(super::invertible_parent(flat).is_finite());
    }

    fn axis_rot(world: &World, path: &str, index: usize) -> f32 {
        world
            .entities()
            .iter()
            .find(|e| world.path(**e) == Some(path))
            .and_then(|e| world.transform(*e))
            .map(|t| t.rot_euler[index])
            .expect("node exists")
    }

    fn axis(world: &World, path: &str, index: usize) -> f32 {
        world
            .entities()
            .iter()
            .find(|e| world.path(**e) == Some(path))
            .and_then(|e| world.transform(*e))
            .map(|t| t.pos[index])
            .expect("node exists")
    }

    /// **Play mode ran no scripts at all.** `Play::run` stepped physics and
    /// wrote bodies back, and that was the whole tick — so pressing Play in
    /// the editor advanced the clock while every scripted thing stood still.
    /// `loom sim` ran scripts and the editor did not, which is the same class
    /// of split that had `render --sim` disagreeing with `sim`.
    #[test]
    fn playing_runs_a_characters_movement_script() {
        let source = std::fs::read_to_string("../../assets/test/walker.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));
        let before = axis(&play.world, "Level/Walker", 0);

        play.run(90);

        assert!(
            axis(&play.world, "Level/Walker", 0) > before + 3.0,
            "the character did not walk: x went {before} -> {}",
            axis(&play.world, "Level/Walker", 0)
        );
    }

    /// The other half of the same gap: an ordinary node's script moves its
    /// transform, and that never ran in the editor either.
    #[test]
    fn playing_runs_a_plain_node_script() {
        let dir = std::env::temp_dir().join("loom_play_script_test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("rise.rhai"), "position[1] = tick.to_float() * 0.1;")
            .expect("write script");
        let scene = "[scene]\nformat = 1\nid = \"3a7c9e15-4b28-4d63-8f10-5e2b74c9a086\"\n\n\
             [[node]]\nname = \"Root\"\ntransform = { pos = [0.0, 0.0, 0.0] }\n\n\
               [node.components.Script]\n  path = \"rise.rhai\"\n";
        let world = World::from_scene(&Scene::parse(scene).expect("valid scene"));
        let mut play = Play::start(world, dir.as_path());

        play.run(10);

        let y = axis(&play.world, "Root", 1);
        assert!((y - 1.0).abs() < 1e-4, "script did not run: y = {y}");
    }

    /// The player rig end to end: a scene with a character and a camera, a
    /// key held, and the capsule walks where the camera is looking.
    fn fps_scene() -> Play {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        Play::start(world, std::path::Path::new("../../assets/test"))
    }

    #[test]
    fn holding_forward_walks_the_character_where_it_is_looking() {
        let mut play = fps_scene();
        assert!(play.has_player(), "the scene must have a character to drive");
        let before = axis(&play.world, "Level/Player", 2);

        play.set_input(PlayerInput {
            move_axis: [0.0, 1.0],
            ..PlayerInput::default()
        });
        play.run(60);

        // Authored facing is -Z, so forward is -Z.
        assert!(
            axis(&play.world, "Level/Player", 2) < before - 2.0,
            "did not walk: z went {before} -> {}",
            axis(&play.world, "Level/Player", 2)
        );
    }

    /// **The invariant that makes the rig coherent**: what the script is told
    /// is forward and where the camera actually points are one direction. They
    /// came from two different conventions at first — the fly camera's yaw and
    /// the scene format's −Z — and the character walked backwards.
    #[test]
    fn forward_for_the_script_is_where_the_camera_looks() {
        let mut play = fps_scene();

        for turn in [0.0_f32, 0.7, -1.9, 3.0] {
            play.look(turn, 0.0);
            play.run(1);

            let view = play.camera().expect("camera");
            let looking = [
                view.target[0] - view.eye[0],
                view.target[2] - view.eye[2],
            ];
            let (forward, _) = play.basis();
            assert!(
                (forward[0] - looking[0]).abs() < 1e-3
                    && (forward[2] - looking[1]).abs() < 1e-3,
                "after turning {turn}: script forward {forward:?} vs view {looking:?}"
            );
        }
    }

    /// Turning has to change what "forward" means, or the character walks a
    /// fixed compass direction whatever the human is looking at.
    #[test]
    fn looking_right_changes_which_way_forward_is() {
        let mut play = fps_scene();
        let start = (
            axis(&play.world, "Level/Player", 0),
            axis(&play.world, "Level/Player", 2),
        );

        // A quarter turn to the right, then walk. Facing −Z to start, so
        // turning right faces +X.
        play.look(std::f32::consts::FRAC_PI_2, 0.0);
        play.set_input(PlayerInput {
            move_axis: [0.0, 1.0],
            ..PlayerInput::default()
        });
        play.run(60);

        let moved_x = axis(&play.world, "Level/Player", 0) - start.0;
        let moved_z = axis(&play.world, "Level/Player", 2) - start.1;
        assert!(
            moved_x > 2.0 && moved_z.abs() < 1.0,
            "turned right should walk +X, not {moved_x} / {moved_z}"
        );
    }

    /// The view has to ride the body, or the human walks and the picture does
    /// not follow. This is the whole reason the camera is a child node.
    #[test]
    fn the_camera_follows_the_character_it_is_parented_to() {
        let mut play = fps_scene();
        let before = play.camera().expect("the scene authors a camera").eye;

        play.set_input(PlayerInput {
            move_axis: [0.0, 1.0],
            ..PlayerInput::default()
        });
        play.run(60);

        let after = play.camera().expect("still there").eye;
        assert!(
            (after[2] - before[2]) < -2.0,
            "camera stayed put while the body walked: {before:?} -> {after:?}"
        );
    }

    /// Looking up must tilt the view without tipping the capsule over — the
    /// reason pitch goes on the camera node and yaw on the character.
    #[test]
    fn looking_up_tilts_the_view_and_not_the_body() {
        let mut play = fps_scene();

        play.look(0.0, -0.6);
        play.run(1);

        let view = play.camera().expect("camera");
        assert!(
            view.target[1] > view.eye[1] + 0.3,
            "the view did not tilt up: {view:?}"
        );
        let body_pitch = axis_rot(&play.world, "Level/Player", 0);
        assert!(body_pitch.abs() < 1e-3, "the capsule leaned: {body_pitch}");
    }

    /// A press lasts one frame; ticks run on their own schedule. Latching is
    /// what stops "sometimes jump does nothing".
    #[test]
    fn a_jump_press_survives_until_a_tick_consumes_it() {
        let mut play = fps_scene();
        // Land first.
        play.run(30);
        let resting = axis(&play.world, "Level/Player", 1);

        play.set_input(PlayerInput {
            jump: true,
            ..PlayerInput::default()
        });
        // The button is already released by the time the next tick runs.
        play.set_input(PlayerInput::default());
        play.run(12);

        assert!(
            axis(&play.world, "Level/Player", 1) > resting + 0.3,
            "the jump was dropped: y {resting} -> {}",
            axis(&play.world, "Level/Player", 1)
        );
    }

    /// **The interact channel reaches the tick.** Nothing in the engine acts
    /// on `interact`, so the furthest an engine test can follow it is into the
    /// `Motion` the scripts are handed — `loom_script`'s
    /// `a_movement_script_sees_the_interact_button` picks it up from there,
    /// and `assets/test/interact_probe.loom` joins the two ends under
    /// `scripts/green.sh`.
    #[test]
    fn an_interact_press_reaches_the_tick() {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        play.set_input(PlayerInput { interact: true, ..PlayerInput::default() });
        // Released before the tick runs, which is the case that loses a press
        // that is read rather than latched.
        play.set_input(PlayerInput::default());
        play.run(1);

        assert!(play.runner.input.interact, "the press was dropped");
    }

    /// **A door must not open sixty times a second.** `jump` and `fire` lean on
    /// their bindings being `pressed`; this one takes the edge in
    /// `set_input`, so a caller that never lets go — a test, a scripted pilot,
    /// or a rebind to `held` — still gets one press.
    #[test]
    fn holding_interact_is_one_press() {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        let mut pressed = 0;
        for _ in 0..60 {
            play.set_input(PlayerInput { interact: true, ..PlayerInput::default() });
            play.run(1);
            pressed += i32::from(play.runner.input.interact);
        }
        assert_eq!(pressed, 1, "a second of holding E was {pressed} interactions");

        // And letting go arms it again, or the second door never opens.
        play.set_input(PlayerInput::default());
        play.run(1);
        play.set_input(PlayerInput { interact: true, ..PlayerInput::default() });
        play.run(1);
        assert!(play.runner.input.interact, "a second press did not register");
    }

    /// **The trigger, end to end.** A button press reaches a script, the
    /// script asks for an explosion where the host says it is aiming, and
    /// something in the world moves because of it. Every link in that chain is
    /// new and none of it is observable except at the far end.
    #[test]
    fn firing_sets_off_an_explosion_where_the_player_is_aiming() {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));
        // Settle on the floor first, so the shot is not fired mid-fall.
        play.run(20);
        // Down the corridor, which is where the shot goes and therefore the
        // direction the target is thrown. Not up: the blast lands on the face
        // of the target nearest the shooter, so it pushes away, not skyward.
        let before = axis(&play.world, "Level/Marker", 2);

        play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
        play.run(30);

        assert_eq!(play.fired().len(), 1, "exactly one shot from one press");
        assert!(
            axis(&play.world, "Level/Marker", 2) < before - 0.3,
            "the blast did not throw the target: z {before} -> {}",
            axis(&play.world, "Level/Marker", 2)
        );
    }

    /// **A shot goes where the crosshair is.** The aim ray used `forward`,
    /// which is deliberately flattened so that looking at the sky does not
    /// walk you into it. Fired along that, nothing above or below eye level
    /// could ever be hit: aiming at the floor put the explosion on a wall
    /// twelve metres away.
    #[test]
    fn aiming_down_puts_the_shot_on_the_floor() {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));
        play.run(20);

        // Steeply down, at the ground just in front of the player.
        play.look(0.0, 1.2);
        play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
        play.run(2);

        let (_, blast_at) = *play.fired().first().expect("it fired");
        let at = axis(&play.world, "Level/Player", 1);
        assert!(
            blast_at[1] < at,
            "the shot landed at {blast_at:?}, above a player standing at y = {at}"
        );
        // The floor is right there; a flat ray would have carried on to the
        // far end of the corridor instead.
        let away = (blast_at[2] - axis(&play.world, "Level/Player", 2)).abs();
        assert!(away < 4.0, "landed {away} m down the corridor, not underfoot");
    }

    /// A weapon that fires every tick the button is down is not a weapon.
    /// The reload lives in the script, which is the whole point — but it has
    /// to actually be consulted.
    #[test]
    fn holding_fire_does_not_detonate_every_tick() {
        let source = std::fs::read_to_string("../../assets/test/camera.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));
        play.run(20);

        // Held down for a full second.
        for _ in 0..60 {
            play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
            play.run(1);
        }

        let shots = play.fired().len();
        assert!(
            (1..=2).contains(&shots),
            "a second of holding fire produced {shots} explosions"
        );
    }

    // ---------------------------------------------------------------
    // The game loop.
    // ---------------------------------------------------------------

    fn range() -> Play {
        let source = std::fs::read_to_string("../../assets/test/turret_range.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        Play::start(world, std::path::Path::new("../../assets/test"))
    }

    /// **The reported bug.** Every character is handed the same input,
    /// including the look direction — so the "turret" aimed wherever the human
    /// pointed the mouse. Look up at the sky before its firing tick and its
    /// ray hits nothing, `aim_hit` is false, and because it only ever tried on
    /// one exact tick it then never fired at all. Ten seconds later the game
    /// reported LOST with zero shots, which is what the screenshot showed.
    #[test]
    fn a_turret_still_fires_after_the_view_has_been_moved() {
        let source = std::fs::read_to_string("../../assets/test/turret_range.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        // Turn away from the targets, the way a human does the moment the
        // pointer is captured, and hold it there past the firing tick.
        play.look(std::f32::consts::PI, 0.0);
        play.run(60);
        // Then look back. It should still take its shot.
        play.look(std::f32::consts::PI, 0.0);
        play.run(120);

        assert!(
            !play.fired().is_empty(),
            "it never fired: {:?}",
            play.state().numbers()
        );
    }

    fn firing_range() -> Play {
        let source = std::fs::read_to_string("../../assets/test/range.loom").expect("fixture");
        let world = World::from_scene(&Scene::parse(&source).expect("valid scene"));
        Play::start(world, std::path::Path::new("../../assets/test"))
    }

    // ---------------------------------------------------------------
    // Enemies.
    // ---------------------------------------------------------------

    /// **An enemy closes the distance.** Perception and a route are the
    /// engine's half — both are casts, and a script has no physics world.
    /// Deciding that seeing the player means chasing is `enemy.rhai`'s.
    #[test]
    fn an_enemy_hunts_the_player() {
        let mut play = firing_range();
        let start = squared_distance_flat(
            [axis(&play.world, "Range/Hunter", 0), 0.0, axis(&play.world, "Range/Hunter", 2)],
            [axis(&play.world, "Range/Player", 0), 0.0, axis(&play.world, "Range/Player", 2)],
        );

        play.run(240);

        let now = squared_distance_flat(
            [axis(&play.world, "Range/Hunter", 0), 0.0, axis(&play.world, "Range/Hunter", 2)],
            [axis(&play.world, "Range/Player", 0), 0.0, axis(&play.world, "Range/Player", 2)],
        );
        assert!(
            now < start - 4.0,
            "it did not close: {} -> {}",
            start.sqrt(),
            now.sqrt()
        );
    }

    /// **Routing, isolated.** A knee-high wall is the one obstacle that
    /// separates seeing from walking: an enemy can see straight over it and
    /// cannot step over it. Without a route it walks into the wall and grinds
    /// there — which is exactly what the straight-line fallback does, so this
    /// is the case that proves the path is real and being followed.
    #[test]
    fn an_enemy_walks_around_what_it_can_see_over() {
        let source = std::fs::read_to_string("../../assets/test/range.loom").expect("fixture");
        // A metre-high wall across the enemy's straight line to the player,
        // open at one end.
        let walled = source.replace(
            "[[node]]\nname = \"Ground\"",
            "[[node]]\nname = \"LowWall\"\nparent = \"Range\"\n             transform = { pos = [-3.0, 0.5, 1.0], scale = [7.0, 0.5, 0.4] }\n\n               [node.components.MeshRenderer]\n  mesh = { asset = \"box\" }\n\n             [[node]]\nname = \"Ground\"",
        );
        let world = World::from_scene(&Scene::parse(&walled).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        play.run(420);

        // Past the wall entirely: it had to go round the open end.
        assert!(
            axis(&play.world, "Range/Hunter", 2) > 2.0,
            "stuck at the wall: z = {}",
            axis(&play.world, "Range/Hunter", 2)
        );
    }

    /// And having closed, it hurts you — through the event queue, so the
    /// rules decide what a hit costs exactly as they do for a blast.
    #[test]
    fn an_enemy_that_reaches_you_hurts_you() {
        let mut play = firing_range();

        play.run(600);

        assert!(
            play.events().count_of("damage") >= 1,
            "never landed a hit: {:?}",
            play.events().counts()
        );
        let health = play.state().number("health").expect("rules keep health");
        assert!(health < 100.0, "took no damage: {health} HP");
    }

    /// **The chain the event queue exists for.** A blast goes off, the engine
    /// reports who it caught — the one part a script cannot work out, because
    /// it needs the same cover cast the shove uses — and a rule turns that
    /// into health and a death. None of health, damage or dying is a concept
    /// the engine has.
    #[test]
    fn shooting_your_own_feet_kills_you() {
        let mut play = firing_range();
        play.run(20);
        assert_eq!(play.state().number("health"), Some(100.0), "starts whole");

        // Straight down, point blank.
        play.look(0.0, 1.5);
        play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
        play.run(20);

        assert!(
            play.events().count_of("damage") >= 1,
            "the blast caught nobody: {:?}",
            play.events().counts()
        );
        assert_eq!(play.state().status(), loom_script::Status::Lost);
        assert!(
            play.state().message().contains("blew yourself up"),
            "message was {:?}",
            play.state().message()
        );
    }

    /// And the other way: a shot at something far away hurts nobody. Damage
    /// that ignored distance would make the weapon unusable, and the falloff
    /// is the engine's half of the split.
    #[test]
    fn shooting_something_far_away_leaves_you_unharmed() {
        let mut play = firing_range();
        play.run(20);

        // Level, down the range at the backstop nineteen metres away.
        play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
        play.run(30);

        assert!(play.events().count_of("blast") >= 1, "it did not fire");
        assert_eq!(play.events().count_of("damage"), 0, "hurt at nineteen metres");
        assert_eq!(play.state().number("health"), Some(100.0));
    }

    /// Falloff, which is the engine's half of the split. Point blank kills;
    /// a few metres off has to *wound* — otherwise the only two outcomes are
    /// unharmed and dead, and a blast radius means nothing inside itself.
    #[test]
    fn a_blast_a_few_metres_away_wounds_without_killing() {
        let mut play = firing_range();
        play.run(20);

        // A shallow angle down, so the shot lands a few metres ahead rather
        // than underfoot.
        play.look(0.0, 0.5);
        play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
        play.run(20);

        let health = play.state().number("health").expect("rules keep health");
        assert!(
            play.events().count_of("damage") >= 1,
            "no damage at all: {:?}",
            play.events().counts()
        );
        assert!(
            health > 0.0 && health < 100.0,
            "expected a wound, got {health} HP"
        );
        assert_eq!(play.state().status(), loom_script::Status::Playing, "survivable");
    }

    /// **The log is a replay.** Two runs of the same scene must produce the
    /// same events in the same order — a stronger claim than the same final
    /// hash, and one that says *where* two runs diverged rather than only
    /// that they did.
    #[test]
    fn the_event_log_replays_identically() {
        let run = || {
            let mut play = firing_range();
            play.run(20);
            play.look(0.0, 1.5);
            play.set_input(PlayerInput { fire: true, ..PlayerInput::default() });
            play.run(40);
            play.events()
                .all()
                .iter()
                .map(|e| (e.tick, e.kind.clone(), e.node.clone()))
                .collect::<Vec<_>>()
        };

        let first = run();
        assert!(!first.is_empty(), "nothing happened, so nothing was compared");
        assert_eq!(first, run());
    }

    /// **The whole loop.** A turret fires, a blast throws two targets off the
    /// platform, and rules nobody compiled into the engine decide that means
    /// the game is won.
    #[test]
    fn a_game_can_be_won() {
        let mut play = range();

        play.run(200);

        assert_eq!(play.state().status(), loom_script::Status::Won);
        assert_eq!(play.state().number("destroyed"), Some(2.0));
        assert_eq!(play.state().number("score"), Some(200.0));
        assert!(
            play.state().message().contains("cleared"),
            "message was {:?}",
            play.state().message()
        );
    }

    /// A won game stops. Left running, the world drifts on after the result
    /// was decided — so a generous `--ticks` would report a different final
    /// state than an exact one, and both would claim to be the same run.
    #[test]
    fn a_finished_game_stops_advancing() {
        let mut play = range();
        play.run(200);
        let ended_at = play.ticks;

        play.run(400);

        assert_eq!(play.ticks, ended_at, "the game kept running after it ended");
        assert!(ended_at < 200, "it should have ended early: {ended_at}");
    }

    /// The other outcome. The range always wins, so losing needs a scene whose
    /// targets are never touched — the rules' clock has to be able to run out.
    #[test]
    fn a_game_can_be_lost_on_time() {
        let source = std::fs::read_to_string("../../assets/test/turret_range.loom").expect("fixture");
        // The turret fires at tick 30; moving that past the limit means the
        // targets are never hit and the clock decides it.
        let never = source.replace("turret.rhai", "idle.rhai");
        std::fs::write(
            std::path::Path::new("../../assets/scripts/idle.rhai"),
            "// A character that does nothing, so the rules' clock can run out.\n             velocity = [0.0, velocity[1] - 24.0 * dt, 0.0];\n",
        )
        .expect("write");
        let world = World::from_scene(&Scene::parse(&never).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        play.run(700);

        assert_eq!(play.state().status(), loom_script::Status::Lost);
        assert_eq!(play.state().number("destroyed"), Some(0.0));
        assert!(
            play.state().message().contains("out of time"),
            "message was {:?}",
            play.state().message()
        );
    }

    /// **Rules run last, and that is load-bearing.** They judge a tick, so
    /// they have to see it finished: `detonations` holds what went off on
    /// *this* tick, and a rules pass that ran before the characters acted
    /// would find it empty every time. The game would simply never end, and
    /// nothing about the ordering would look wrong in the code.
    #[test]
    fn rules_see_what_happened_on_the_tick_they_judge() {
        let source = std::fs::read_to_string("../../assets/test/turret_range.loom").expect("fixture");
        let watching = source.replace("rules.rhai", "rules_on_blast.rhai");
        let world = World::from_scene(&Scene::parse(&watching).expect("valid scene"));
        let mut play = Play::start(world, std::path::Path::new("../../assets/test"));

        play.run(200);

        assert_eq!(
            play.state().status(),
            loom_script::Status::Won,
            "the rules never saw the shot the turret fired"
        );
        // The turret fires at tick 30; the game must end on that tick, not a
        // tick later and not never.
        assert_eq!(play.ticks, 30, "ended on the wrong tick");
    }

    #[test]
    fn playing_makes_a_crate_fall() {
        let mut play = Play::start(world(), std::path::Path::new("."));
        let before = height(&play.world, "Stage/Crate");

        play.run(60);

        assert!(
            height(&play.world, "Stage/Crate") < before - 1.0,
            "a second of gravity should be visible"
        );
    }

    /// The property the whole module is for: what the human watches and what
    /// `loom sim --assert` checks are the same run. Frame times differ between
    /// the two; the answer must not.
    ///
    /// Compared at equal tick counts rather than equal wall time, because the
    /// tick count is the thing that determines the state — accumulating float
    /// seconds two different ways is allowed to land a tick apart, and that is
    /// the accumulator working, not a determinism failure.
    #[test]
    fn pacing_does_not_change_the_outcome_only_the_tick_count_does() {
        let mut stuttering = Play::start(world(), std::path::Path::new("."));
        // Roughly a second, delivered in uneven lumps.
        for dt in [0.05_f32, 0.002, 0.13, 0.008, 0.24, 0.06, 0.21, 0.09, 0.21] {
            stuttering.advance(dt);
        }
        assert!(stuttering.ticks > 50, "about a second ran: {}", stuttering.ticks);

        // The same number of ticks, delivered as a clean 60 fps.
        let mut steady = Play::start(world(), std::path::Path::new("."));
        while steady.ticks < stuttering.ticks {
            steady.advance(TICK_SECONDS);
        }

        assert_eq!(steady.ticks, stuttering.ticks);
        assert_eq!(
            steady.state_hash(),
            stuttering.state_hash(),
            "same ticks, bit-identical state, whatever the frame times were"
        );
    }

    #[test]
    fn pausing_stops_time() {
        let mut play = Play::start(world(), std::path::Path::new("."));
        play.paused = true;

        play.advance(1.0);

        assert_eq!(play.ticks, 0);
        // Step works anyway — that is what a step button is.
        play.run(1);
        assert_eq!(play.ticks, 1);
    }

    /// The scene the exit criterion runs on, read from disk so the test and
    /// `loom sim assets/test/water_crate.loom` are measuring the same crate.
    fn water_crate() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/test/water_crate.loom"),
        )
        .expect("the W5 test scene should exist")
    }

    /// **W6 on the physics side: the crate feels the bottom.** The same scene
    /// with a voxel shelf a metre and a half under the surface, and an
    /// authored `attenuation_depth`, so the waves the buoyancy solver samples
    /// are the shallow ones.
    ///
    /// This is the whole CPU path in one assertion — `Sim` bakes the height
    /// field off the volume it is already building a collider from, fills each
    /// pontoon's `ground` from it, and `sample_water` tapers every amplitude by
    /// `tanh(k·d)/tanh(k·D)`. Any link missing and the crate rides the open
    /// sea's swell in five feet of water, which is what it did before W6.
    #[test]
    fn a_crate_in_the_shallows_rides_a_smaller_sea() {
        let deep = water_crate();
        let shallow = deep
            .replace(
                "  drag = 2.0\n",
                "  drag = 2.0\n\n  [node.components.WaterBody.waves]\n  \
                 attenuation_depth = 4.0\n  max_height = 1.5\n",
            )
            // A shelf under the whole area the crate drifts over: 16 m square,
            // its top at y = −1.5, which is 1.5 m of water. Low enough that the
            // crate never touches it — it floats with its underside at about
            // −0.65 — and shallow enough that the taper is about a half.
            + "\n[[node]]\nname = \"Shelf\"\nparent = \"Sea\"\n\
               transform = { pos = [-8.0, -7.5, -14.0] }\n\n  \
               [node.components.VoxelVolume]\n  voxel_size = 0.25\n  chunks = [2, 1, 2]\n\n  \
               [[node.components.VoxelVolume.ops]]\n  kind = \"box\"\n  \
               center = [8.0, 3.0, 8.0]\n  half_extents = [10.0, 3.0, 10.0]\n  \
               mode = \"union\"\n";

        let deep_y: Vec<f32> = crate_trajectory(&deep, 1800).iter().map(|p| p[1]).collect();
        let shallow_y: Vec<f32> =
            crate_trajectory(&shallow, 1800).iter().map(|p| p[1]).collect();

        let open = peak_to_peak(&deep_y[1500..1800]);
        let inshore = peak_to_peak(&shallow_y[1500..1800]);
        assert!(
            inshore < open * 0.75,
            "the crate rides the same sea in 1.5 m of water as in the open: \
             {inshore:.3} m against {open:.3} m"
        );
        // And it is still floating rather than sunk or beached — a taper that
        // zeroed the surface entirely would also pass the line above.
        let mean = shallow_y[1500..1800].iter().sum::<f32>() / 300.0;
        assert!(
            (-0.5..0.5).contains(&mean),
            "the crate is not at the waterline any more: y = {mean:.3}"
        );
    }

    /// **W5's exit criterion, as a number.** A crate dropped four metres into
    /// the sea and left alone for 1800 ticks — thirty seconds, because
    /// resonance is not visible in five.
    ///
    /// *Does not resonate* is defined here as: over the last five seconds the
    /// crate's peak-to-peak vertical travel is **no more than the sea's own
    /// travel at that spot**, and no larger than it was on entry. A crate on a
    /// real sea is supposed to move, so the reference is the surface it is on
    /// rather than a number — riding the waves is the pass, out-travelling them
    /// is the fail. An undamped pontoon takes energy out of the wave field
    /// every cycle with nothing to give it back to, and ends up moving several
    /// times as far as the water under it.
    #[test]
    fn a_floating_crate_settles_rather_than_resonating() {
        let scene = water_crate();
        let track = crate_trajectory(&scene, 1800);
        let y: Vec<f32> = track.iter().map(|p| p[1]).collect();

        // Once it is in the water: the drop takes about half a second and the
        // entry transient a couple more.
        let entry = peak_to_peak(&y[300..600]);
        let settled = peak_to_peak(&y[1500..1800]);
        let sea = sea_travel(&scene, track[1799], 1500..1800);
        // **Archimedes, measured rather than assumed.** At the waterline it
        // settles on, the crate's four pontoons displace the crate's own mass
        // of water — 518 kg. Any factor slip in the volume, the density or the
        // force would leave it floating high or swamped, and every other
        // assertion here would still pass.
        let mean = y[1500..1800].iter().sum::<f32>() / 300.0;
        let displaced: f32 = loom_water::buoyancy::default_pontoons([0.6, 0.6, 0.6])
            .iter()
            .map(|p| loom_water::buoyancy::submerged_volume(p.radius, mean + p.offset[1], 0.0))
            .sum::<f32>()
            * 1000.0;
        assert!(
            (displaced - 518.0).abs() < 30.0,
            "resting at y = {mean:.3} it displaces {displaced:.0} kg of water, \
             and it weighs 518 kg"
        );

        assert!(
            settled <= entry,
            "the bob is growing, not decaying: {entry:.3} m early, {settled:.3} m late"
        );
        // A quarter of slack, because the crate drifts across the surface
        // while the reference is a fixed point — so it samples a slightly
        // different run of crests than the spot it ends on. An undamped crate
        // is nearly three times the sea's travel, so this costs nothing.
        assert!(
            settled <= sea * 1.25,
            "the crate out-travels the sea it is floating on: {settled:.3} m \
             against the surface's {sea:.3} m"
        );
        // And it is riding the waves rather than resting on an invisible floor
        // — a crate that stopped moving entirely would pass both tests above
        // and would mean the buoyancy had died.
        assert!(
            settled > sea * 0.25,
            "the crate is barely moving on a sea that travels {sea:.3} m: {settled:.3} m"
        );
    }

    /// The other half of the same claim: **remove the damping and the
    /// assertion above has to fail.** Without this the test only asserts that
    /// a crate exists.
    #[test]
    fn without_damping_the_same_crate_resonates() {
        let undamped = water_crate().replace(
            "[node.components.Buoyancy]",
            "[node.components.Buoyancy]\n  damp_linear = 0.0\n  damp_quadratic = 0.0",
        );
        let track = crate_trajectory(&undamped, 1800);
        let y: Vec<f32> = track.iter().map(|p| p[1]).collect();

        let settled = peak_to_peak(&y[1500..1800]);
        let sea = sea_travel(&undamped, track[1799], 1500..1800);
        assert!(
            settled > sea,
            "damping was removed and the crate still rides the sea ({settled:.3} m \
             against the surface's {sea:.3} m) — then the test above proves nothing"
        );
    }

    /// **Why there are four pontoons and not one.** Force applied at an offset
    /// is a torque; force applied through the centre of mass is not. The crate
    /// is dropped in on its side, 60° over: four pontoons right it, because the
    /// deeper ones push harder than the shallower ones and the difference acts
    /// at an offset. One pontoon at the centre of mass has no offset to act
    /// through, so the water cannot turn it at all and it stays over.
    ///
    /// "Cannot turn it" rather than "spins it" is the honest version of the
    /// doc's warning: a single central pontoon does not tumble a crate, it
    /// leaves its attitude to whatever else happens to touch it — which in open
    /// water is nothing.
    #[test]
    fn four_pontoons_right_a_capsized_crate_and_one_does_not() {
        let capsized = water_crate().replace(
            "rot_euler = [0.0, 12.0, 0.0]",
            "rot_euler = [0.0, 12.0, 60.0]",
        );
        let tilt_after = |source: &str, ticks: u32| {
            let world = World::from_scene(&Scene::parse(source).expect("valid scene"));
            let mut play = Play::start(world, std::path::Path::new("."));
            play.run(ticks);
            let entity = play
                .world
                .entities()
                .iter()
                .copied()
                .find(|e| play.world.path(*e) == Some("Sea/Crate"))
                .expect("the crate");
            let rot = play.world.transform(entity).expect("a transform").rot_euler;
            // Pitch and roll only: yaw is free for a floating box and says
            // nothing about whether it is the right way up.
            rot[0].abs().max(rot[2].abs())
        };

        // The wave sum's steepest slope is about 20°, and a crate that follows
        // the surface reaches it — so level is the wrong bar and upright is the
        // right one.
        let four = tilt_after(&capsized, 900);
        assert!(four < 30.0, "four pontoons should right it: {four} degrees");

        // Radius chosen so the single sphere displaces the same volume the four
        // do: this is a test of *where* the force lands, not of how much.
        let one = tilt_after(
            &capsized.replace(
                "[node.components.Buoyancy]",
                "[node.components.Buoyancy]\n  \
                 pontoons = [{ offset = [0.0, 0.0, 0.0], radius = 0.7444 }]",
            ),
            900,
        );
        assert!(
            one > 45.0,
            "one central pontoon can exert no righting torque, so the crate \
             should still be over: {one} degrees against {four} for four"
        );
    }

    /// A stall must not turn into a thousand catch-up ticks.
    #[test]
    fn a_long_stall_is_clamped() {
        let mut play = Play::start(world(), std::path::Path::new("."));

        play.advance(30.0);

        assert!(play.ticks <= 15, "clamped, not caught up: {}", play.ticks);
    }

    // ---------------------------------------------------------------------
    // Camera weight. The maths is the half of a feel feature that can be
    // proved; everything about whether it *feels* right needs a window.
    // ---------------------------------------------------------------------

    /// Turn at a constant rate for two seconds and report how far behind the
    /// view ends up, expressed as milliseconds of equivalent latency —
    /// steady-state error ÷ turn rate. For any first-order lag this is exactly
    /// its time constant, so it is directly comparable to every input-latency
    /// number in the literature.
    fn equivalent_latency_ms(spring: &mut CameraSpring, degrees_per_second: f32) -> f32 {
        let step = degrees_per_second.to_radians() * TICK_SECONDS;
        let mut target = 0.0;
        for _ in 0..120 {
            target += step;
            spring.settle(target);
        }
        1000.0 * (target - spring.view(target)).to_degrees() / degrees_per_second
    }

    /// `degrees` of input spread over `ticks` ticks, then the mouse stops.
    /// Returns the view angle in degrees on every tick, input and coast
    /// together.
    #[allow(clippy::cast_precision_loss)]
    fn flick_at(spring: &mut CameraSpring, degrees: f32, ticks: u32) -> Vec<f32> {
        let step = degrees.to_radians() / ticks as f32;
        let mut target = 0.0;
        (0..ticks + 180)
            .map(|tick| {
                if tick < ticks {
                    target += step;
                }
                spring.settle(target);
                spring.view(target).to_degrees()
            })
            .collect()
    }

    /// A 60° flick over twelve ticks — 300 °/s, a look across the deck. The
    /// speed the defaults were tuned at, and *not* the speed a mouse flick
    /// happens at; see `no_speed_of_turn_throws_the_view_further_than_the_bound`.
    fn flick(spring: &mut CameraSpring) -> Vec<f32> {
        flick_at(spring, 60.0, 12)
    }

    /// **The acceptance criterion the whole design is built around.** Weight
    /// that costs accuracy is not weight, it is lag — so the sustained-turn
    /// error must stay under 20 ms, which is the floor of the band where
    /// flick-aim data can measure a difference at all, and it must not grow
    /// with how fast you turn.
    ///
    /// A plain damped spring fails this at 27.1 ms; so does every low-pass, by
    /// construction. The feedforward term is what buys it, and this test is
    /// the reason that term is not optional.
    #[test]
    fn a_sustained_turn_leaves_the_view_under_one_tick_behind() {
        for rate in [100.0_f32, 200.0, 400.0, 800.0] {
            let mut spring = CameraSpring::DEFAULT;
            let latency = equivalent_latency_ms(&mut spring, rate);
            assert!(
                (0.0..20.0).contains(&latency),
                "{rate}°/s: {latency:.2} ms of equivalent latency — \
                 negative leads the hand, ≥20 ms reads as input lag"
            );
            assert!(
                (latency - 5.22).abs() < 0.2,
                "{rate}°/s: {latency:.2} ms, and this number must not depend on \
                 the turn rate — a rate-dependent one means the feedforward is gone"
            );
        }
    }

    /// The weight you can see: the view arrives past where you pointed it and
    /// comes back. Bounded on both sides — too little and the second-order
    /// system has quietly degenerated into a first-order one, too much and it
    /// reads as a camera that missed rather than a camera with mass.
    ///
    /// The settle must finish inside human reaction time, so it is a texture
    /// rather than a delay you can consciously wait through.
    #[test]
    fn a_flick_overshoots_a_little_and_settles_fast() {
        let mut spring = CameraSpring::DEFAULT;
        let view = flick(&mut spring);

        let peak = view.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            (2.0..4.0).contains(&(peak - 60.0)),
            "overshoot {:.2}° past a 60° flick",
            peak - 60.0
        );

        let settled = view
            .iter()
            .rposition(|v| (v - 60.0).abs() > 0.5)
            .expect("it overshoots at all");
        #[allow(clippy::cast_precision_loss)]
        let ms = (settled + 1 - 12) as f32 * TICK_SECONDS * 1000.0;
        assert!(ms < 150.0, "settled within half a degree after {ms:.0} ms");
    }

    /// **The bound, at the speeds a mouse actually moves.** The test above
    /// checks 300 °/s, and overshoot is proportional to the angular velocity
    /// the spring was carrying — so an ordinary flick, at 1000–3000 °/s, threw
    /// the view 18° past where it was pointed and a fast 360 threw it 47°,
    /// entirely unasserted. [`CameraSpring::MAX_LAG`] is what bounds it, and
    /// this is the test that notices if it is removed or made a taste knob.
    ///
    /// Two things are asserted together and both matter: the view never leaves
    /// the bound, and it still **lands exactly where the mouse asked**. A clamp
    /// that bought its bound by losing the endpoint would be a worse bug than
    /// the overshoot — that is the whole "still gets to the places where it
    /// needs to go" property, and a clamp is the obvious way to break it.
    #[test]
    fn no_speed_of_turn_throws_the_view_further_than_the_bound() {
        let bound = CameraSpring::MAX_LAG.to_degrees();
        for (degrees, ticks) in [(60.0_f32, 12_u32), (60.0, 6), (180.0, 6), (180.0, 3), (360.0, 3)] {
            let mut spring = CameraSpring::DEFAULT;
            let view = flick_at(&mut spring, degrees, ticks);
            #[allow(clippy::cast_precision_loss)]
            let rate = degrees / (ticks as f32 * TICK_SECONDS);

            // Against the *moving* target, not the endpoint: during the ramp
            // the view is legitimately far from where the flick will finish,
            // and the bound is on the gap to the mouse right now.
            #[allow(clippy::cast_precision_loss)]
            let worst = view
                .iter()
                .enumerate()
                .map(|(tick, v)| {
                    let target = degrees * (tick as u32 + 1).min(ticks) as f32 / ticks as f32;
                    (v - target).abs()
                })
                .fold(f32::MIN, f32::max);
            assert!(
                worst <= bound + 0.01,
                "{rate:.0}°/s: the view got {worst:.2}° from a {degrees}° flick, \
                 past the {bound:.2}° bound — overshoot is proportional to turn \
                 rate and nothing else caps it"
            );
            let landed = view.last().copied().expect("the flick produced ticks");
            assert!(
                (landed - degrees).abs() < 0.01,
                "{rate:.0}°/s: it settled at {landed:.3}° instead of {degrees}° — \
                 a bound that costs you the endpoint is lag wearing a clamp"
            );
        }
    }

    /// **The clamp holds even at a setting the range clamp is there to make
    /// unreachable**, which is what the velocity trim in [`CameraSpring::settle`]
    /// buys and the only thing it buys.
    ///
    /// The trim was written for a kick coming off the wall, and *that turned
    /// out not to exist*: measured against a version without it, the worst
    /// error after a hard stop from 600–7200 °/s is 6.000° either way, because
    /// bounding the position bounds the error whatever the velocity is doing.
    /// So the fault injection slipped straight through the test written for it,
    /// which is what put this measurement on the record.
    ///
    /// What the trim actually does is stop `vel` growing without bound while
    /// pinned. Held target, 100k ticks, `hz = 20`: **NaN without the trim, and
    /// exactly 0.0 with it** — and a NaN passes *both* comparisons in the
    /// clamp, so it would sail through the bound untouched and take the camera
    /// with it. Two lines that turn the range clamp from the only defence into
    /// the first one; deleting them makes a mis-set ceiling fatal instead of
    /// merely ugly. `hz = 20` here comes through [`CameraSpring::new`]
    /// deliberately — [`CameraSpring::from_env`] cannot reach it, and that is
    /// exactly the assumption under test.
    #[test]
    fn the_lag_clamp_survives_a_frequency_the_range_clamp_forbids() {
        let bound = CameraSpring::MAX_LAG.to_degrees();
        for hz in [12.0_f32, 20.0, 40.0] {
            let mut spring = CameraSpring::new(1.0, hz, 0.55, 0.5);
            let mut target = 0.0_f32;
            for tick in 0..20_000 {
                if tick < 6 {
                    target += 30.0_f32.to_radians();
                }
                spring.settle(target);
                let error = (spring.view(target) - target).to_degrees();
                assert!(
                    error.abs() <= bound + 0.01,
                    "hz {hz}, tick {tick}: {error:.3}° off the {bound:.2}° \
                     bound — a NaN fails both halves of a clamp and walks \
                     through it"
                );
            }
        }
    }

    /// **A knob the human sets can only ever produce a setting inside the box**
    /// the test below sweeps — which is what makes that sweep a statement about
    /// the shipped camera rather than about an arbitrary set of floats.
    ///
    /// Every one of these rows was a real behaviour before the clamp landed:
    /// `LOOM_CAMERA_HZ=12` made the camera NaN, `=-4` sent the view 2.7e24
    /// degrees round, and `LOOM_CAMERA_DAMPING=-1` diverged. The knobs exist to
    /// be swept by hand between two runs, so "don't type that" is not a design.
    #[test]
    fn a_knob_cannot_be_set_outside_the_box() {
        let d = CameraSpring::DEFAULT;
        let hz = &CameraSpring::HZ_RANGE;
        for asked in [None, Some(f32::NAN), Some(f32::INFINITY), Some(f32::NEG_INFINITY)] {
            let got = CameraSpring::knob("LOOM_CAMERA_HZ", asked, d.hz, hz);
            assert_eq!(got, d.hz, "{asked:?} is not a setting; it is the default");
        }
        for (asked, want) in [
            (12.0, *hz.end()),
            (1e30, *hz.end()),
            (-4.0, *hz.start()),
            (0.0, *hz.start()),
            (3.0, 3.0),
            (*hz.start(), *hz.start()),
            (*hz.end(), *hz.end()),
        ] {
            let got = CameraSpring::knob("LOOM_CAMERA_HZ", Some(asked), d.hz, hz);
            assert!(
                (got - want).abs() < 1e-6,
                "LOOM_CAMERA_HZ={asked} became {got}, wanted {want}"
            );
        }
        // And the other three ranges are actually wired to their own knobs — a
        // knob reading the wrong range still compiles and still clamps.
        assert_eq!(
            CameraSpring::knob("W", Some(9.0), d.weight, &CameraSpring::WEIGHT_RANGE),
            *CameraSpring::WEIGHT_RANGE.end()
        );
        assert_eq!(
            CameraSpring::knob("D", Some(-1.0), d.zeta, &CameraSpring::DAMPING_RANGE),
            *CameraSpring::DAMPING_RANGE.start()
        );
        assert_eq!(
            CameraSpring::knob("R", Some(3.0), d.response, &CameraSpring::RESPONSE_RANGE),
            *CameraSpring::RESPONSE_RANGE.end()
        );
    }

    /// **Every setting the human can reach arrives, and then stays put.**
    ///
    /// The knobs exist to be swept by hand, and semi-implicit Euler at a fixed
    /// 60 Hz step diverges above a natural frequency that *moves with the
    /// damping* — 11.41 Hz at ζ = 0.55 but only 8.04 at ζ = 1 — so the two
    /// cannot be bounded independently and no per-knob check would find it.
    ///
    /// **Asserting "it did not diverge" is not enough, and that is measured
    /// rather than argued.** Widening `HZ_RANGE` to 12 was injected as a fault
    /// and a divergence check passed it, because [`CameraSpring::MAX_LAG`]
    /// converts divergence into *chatter*: at `hz = 12` the view is finite,
    /// pinned to the bound, jittering 11.6° tick to tick and settling 5.6° away
    /// from where the mouse is pointing. Finite, bounded, and unusable. So what
    /// is asserted is the property the feature is actually for — it gets where
    /// it needs to go, and then it holds still.
    ///
    /// Swept through [`CameraSpring::new`] rather than the environment on
    /// purpose: env vars are process-global and these tests run in parallel, so
    /// an env-mutating test would flake against every other test that starts a
    /// [`Play`]. What is asserted here is that the box is safe;
    /// `a_knob_cannot_be_set_outside_the_box` is what asserts a human cannot
    /// leave it.
    #[test]
    fn every_setting_a_knob_can_reach_arrives_and_then_holds_still() {
        let steps = |r: &std::ops::RangeInclusive<f32>| {
            let (lo, hi) = (*r.start(), *r.end());
            (0..=8_i8).map(move |i| lo + (hi - lo) * f32::from(i) / 8.0)
        };
        let d = CameraSpring::DEFAULT;
        assert!(
            CameraSpring::WEIGHT_RANGE.contains(&d.weight)
                && CameraSpring::HZ_RANGE.contains(&d.hz)
                && CameraSpring::DAMPING_RANGE.contains(&d.zeta)
                && CameraSpring::RESPONSE_RANGE.contains(&d.response),
            "the defaults must themselves be reachable settings"
        );

        for hz in steps(&CameraSpring::HZ_RANGE) {
            for zeta in steps(&CameraSpring::DAMPING_RANGE) {
                for response in steps(&CameraSpring::RESPONSE_RANGE) {
                    for weight in [0.0_f32, 1.0] {
                        let mut spring = CameraSpring::new(weight, hz, zeta, response);
                        let view = flick_at(&mut spring, 180.0, 3);
                        let knobs =
                            format!("weight {weight} hz {hz} damping {zeta} response {response}");

                        let landed = view.last().copied().expect("the flick produced ticks");
                        assert!(
                            (landed - 180.0).abs() < 0.05,
                            "{knobs}: settled at {landed:.3}° instead of 180°"
                        );
                        // And it arrives *soon enough to be a camera*. This is
                        // what the floor of each range buys, and without the
                        // assertion the floor is a comment: at hz 0.5 / ζ 0.1
                        // — inside the first ranges written here — a 60° flick
                        // took 7.3 seconds to settle. Stable, correct, useless.
                        let settled = view
                            .iter()
                            .rposition(|v| (v - 180.0).abs() > 0.5)
                            .unwrap_or(0);
                        #[allow(clippy::cast_precision_loss)]
                        let ms = (settled.saturating_sub(2) as f32) * TICK_SECONDS * 1000.0;
                        assert!(
                            ms < 600.0,
                            "{knobs}: {ms:.0} ms to settle — every reachable \
                             setting has to still be a camera"
                        );
                        // The last quarter second must be motionless. Chatter
                        // against the lag clamp is finite and bounded and still
                        // an unusable camera.
                        let tail = &view[view.len() - 15..];
                        let swing = tail
                            .windows(2)
                            .map(|w| (w[1] - w[0]).abs())
                            .fold(f32::MIN, f32::max);
                        assert!(
                            swing < 0.05,
                            "{knobs}: the view is still moving {swing:.3}° a tick \
                             a quarter second after the mouse stopped"
                        );
                    }
                }
            }
        }
    }

    /// The mass tell, and the test that fails if anyone "simplifies" the
    /// spring back to a lerp.
    ///
    /// A spring starts from *zero velocity*, so the first tick of a turn barely
    /// moves. A first-order filter tuned to the same steady-state latency
    /// covers 96% of the first tick — there is no way to have both. This is
    /// the difference between a camera with weight and a camera with lag, and
    /// it is visible in one frame.
    #[test]
    fn the_first_tick_of_a_turn_barely_moves() {
        let mut spring = CameraSpring::DEFAULT;
        let view = flick(&mut spring);
        let rigid = 60.0 / 12.0;
        assert!(
            view[0] < rigid * 0.5,
            "first tick moved {:.2}° of a rigid camera's {rigid:.2}°",
            view[0]
        );
        assert!(
            view[0] > 0.0,
            "it must still start moving on the first tick, not sit still"
        );
    }

    /// Zero is off, exactly — the filter is bypassed, not run at a gain of
    /// nothing. A zero that still runs the filter is a zero that lies, and it
    /// is the setting a player reaches for when the feature makes them sick.
    #[test]
    fn weight_zero_is_bit_identical_to_no_filter_at_all() {
        let mut spring = CameraSpring::new(0.0, 4.0, 0.55, 0.5);
        let mut target = 0.0_f32;
        for tick in 0..600 {
            #[allow(clippy::cast_precision_loss)]
            {
                target += (tick as f32 * 0.017).sin() * 0.01;
            }
            spring.settle(target);
            assert_eq!(
                spring.view(target),
                target,
                "tick {tick}: off must mean off"
            );
            // And the filter must genuinely not be *running*, not merely be
            // multiplied by nothing at the end. Deleting the bypass leaves the
            // line above passing — `0.0 * anything` is 0.0 — right up until
            // `anything` is an infinity from a pathological `LOOM_CAMERA_HZ`,
            // at which point `0.0 * inf` is a NaN and the camera dies with the
            // feature switched off. This is the assertion that notices.
            assert_eq!(
                spring.angle, target,
                "tick {tick}: off must mean the spring is not integrating at all"
            );
        }
    }

    /// A stalled frame delivers six ticks at once, and the spring must not
    /// ring, blow up, or produce a NaN — the classic failure of a stiff spring
    /// integrated with a variable `dt`. It cannot happen here *because* the
    /// step is fixed and the stall only changes how ticks are batched, which
    /// is the entire argument for putting this on the tick clock. Asserted
    /// rather than assumed.
    #[test]
    fn a_hundred_millisecond_frame_does_not_shake_the_camera() {
        let mut spring = CameraSpring::DEFAULT;
        let mut target = 0.0_f32;
        for _ in 0..40 {
            // 100 ms of mouse arrives, then six ticks catch up at once.
            target += 30.0_f32.to_radians();
            for _ in 0..6 {
                spring.settle(target);
            }
            let view = spring.view(target).to_degrees();
            assert!(view.is_finite(), "the spring diverged");
            assert!(
                (view - target.to_degrees()).abs() < 15.0,
                "the view is {:.1}° off a target it has had six ticks to reach",
                view - target.to_degrees()
            );
        }
    }

    /// **Framerate independence, end to end through `Play::advance`.** Turn 90°
    /// over half a second at 30, 60 and 144 fps and measure two things: where
    /// the view lands, and how many *seconds* it takes to get there once the
    /// mouse stops. Four players on four machines must get one camera.
    ///
    /// The endpoint alone would prove nothing — every convergent filter arrives
    /// eventually, including the broken ones. The settle *time in seconds* is
    /// the discriminating half: a camera stepped once per frame instead of once
    /// per tick settles 2.4× faster at 144 fps than at 60, which is the entire
    /// failure this design's fixed step exists to make impossible. Verified by
    /// injecting exactly that fault; see the report.
    #[test]
    fn frame_rate_changes_neither_where_the_view_lands_nor_how_long_it_takes() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        fn turn(fps: f32) -> (f32, f32) {
            let mut play = Play::start(world(), std::path::Path::new("."));
            let frames = (fps * 0.5) as u32; // half a second of turning
            #[allow(clippy::cast_precision_loss)]
            let per_frame = std::f32::consts::FRAC_PI_2 / frames as f32;
            for _ in 0..frames {
                play.look(per_frame, 0.0);
                play.advance(1.0 / fps);
            }
            // Then the mouse stops and the spring is allowed to arrive.
            let mut settled = 0.0;
            for frame in 0..(fps as u32) {
                play.advance(1.0 / fps);
                if (play.view_yaw().to_degrees() + 90.0).abs() > 0.5 {
                    #[allow(clippy::cast_precision_loss)]
                    {
                        settled = (frame + 1) as f32 / fps;
                    }
                }
            }
            (play.view_yaw().to_degrees(), settled)
        }

        let [(a, ta), (b, tb), (c, tc)] = [turn(30.0), turn(60.0), turn(144.0)];
        assert!(
            (a - b).abs() < 0.01 && (b - c).abs() < 0.01,
            "30/60/144 fps landed at {a:.4}° / {b:.4}° / {c:.4}°"
        );
        assert!(
            (a + 90.0).abs() < 0.01,
            "and all three arrived at the 90° the mouse asked for, not {a:.4}°"
        );
        assert!(
            (ta - tb).abs() < 0.02 && (tb - tc).abs() < 0.02,
            "the settle took {ta:.3} s / {tb:.3} s / {tc:.3} s at 30/60/144 fps — \
             a camera whose weight depends on the frame rate is four cameras"
        );
    }

    /// The knobs are the tuning loop — a feel feature whose constants need a
    /// rebuild to change cannot be tuned by the one person who can judge it.
    /// Asserted on the direction of each, because a knob wired to the wrong
    /// field still compiles.
    #[test]
    fn each_knob_moves_the_feel_the_way_it_says_it_does() {
        let latency = |mut s: CameraSpring| equivalent_latency_ms(&mut s, 200.0);
        let overshoot = |mut s: CameraSpring| {
            flick(&mut s).iter().copied().fold(f32::MIN, f32::max) - 60.0
        };
        let d = CameraSpring::DEFAULT;

        assert!(
            latency(CameraSpring::new(1.0, 2.0, d.zeta, d.response)) > latency(d),
            "lower frequency is the heavier, laggier camera"
        );
        assert!(
            overshoot(CameraSpring::new(1.0, d.hz, 0.35, d.response)) > overshoot(d),
            "less damping overshoots more"
        );
        assert!(
            latency(CameraSpring::new(1.0, d.hz, d.zeta, 0.0)) > 20.0,
            "no feedforward is exactly the input lag this design exists to avoid"
        );
        assert!(
            overshoot(CameraSpring::new(0.5, d.hz, d.zeta, d.response)) < overshoot(d),
            "half weight is half the sensation"
        );
    }

    /// **The stride and the ring depth are one decision made in two crates**, so this
    /// asserts the pair rather than trusting the comment on either. `KEPT - 1` gaps of
    /// `SEA_KEEP_TICKS` have to reach back a whole `SPRAY_LIFETIME`; if they do not, a
    /// droplet still in the air asks for a surface the ring has dropped and is silently
    /// never thrown — which is the exact shape of the no-op this whole slice removed.
    #[test]
    fn the_sea_ring_spans_a_droplets_life() {
        #[allow(clippy::cast_precision_loss)]
        let span = (loom_water::ocean::KEPT as u64 - 1) as f32
            * SEA_KEEP_TICKS as f32
            * TICK_SECONDS;
        assert!(
            span >= loom_water::spray::SPRAY_LIFETIME,
            "the ring reaches {span} s back and a droplet lives {} s",
            loom_water::spray::SPRAY_LIFETIME
        );
    }

    /// **`evolve_sea` keeps, and that wiring is the only thing between an FFT storm and
    /// no spray at all.**
    ///
    /// `loom_water` can prove the ring works and cannot see whether anything fills it —
    /// which is precisely how a `spectrum` body came to author `spray` and throw nothing.
    /// So this steps the real simulation and asks the sea about an instant half a second
    /// behind the tick it holds.
    #[test]
    fn the_running_sea_remembers_the_instants_spray_reads() {
        const SRC: &str = r#"
[scene]
format = 1
id = "b1d4e9a7-3c25-4f81-9e60-72ab5d8c4f13"

[[node]]
name = "Sea"

  [node.components.Wind]
  direction_degrees = 15.0
  speed = 27.5
  ground_drag = 0.45

[[node]]
name = "Water"
parent = "Sea"

  [node.components.WaterBody]
  kind = "ocean"
  wave_model = "spectrum"
  surface_height = 0.0
  spray = 8.0
  fetch = 50000.0
"#;
        let world = World::from_scene(&Scene::parse(SRC).expect("valid scene"));
        let mut sim = Sim::new(&world);
        // Past the ring's own depth, so it has rolled at least once.
        let ticks = 96;
        sim.step(ticks);
        let sea = sim.sea().expect("the scene opts into the cascade");
        #[allow(clippy::cast_precision_loss)]
        let now = ticks as f32 * TICK_SECONDS;

        assert_eq!(sea.evolved_at(), now, "the tiles are not on the tick");
        assert!(
            sea.at_kept(now - 0.5, 3.0, -7.0).is_some(),
            "the sea forgot half a second ago; spray reads back a whole SPRAY_LIFETIME"
        );
        assert!(
            sea.at_kept(now - 5.0, 3.0, -7.0).is_none(),
            "the sea answered about five seconds ago, which it cannot know"
        );
        // And what it remembers is a surface it actually held, not the current one.
        assert_ne!(
            sea.at_kept(now - 0.5, 3.0, -7.0),
            Some(sea.at(3.0, -7.0)),
            "the past reads identical to the present"
        );
    }
}
