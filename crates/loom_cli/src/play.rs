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
    /// The scene's water, with its waves resolved once — the same resolution
    /// the renderer does, so a crate floats on the sea it is drawn on.
    water: Option<loom_scene::components::WaterBody>,
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
    /// The interactive ripple grid, when the body authors one — ADR 0046.
    ///
    /// **The one piece of stepped state this simulation owns besides `rapier`,
    /// and it is on the force path.** It is `None` for every scene authored
    /// before ADR 0046, which is what keeps both determinism hashes unmoved.
    ///
    /// **Anchored to the water node's world position, never to the camera.**
    /// ADR 0045's trap clause: a domain that followed the eye would make the
    /// buoyant force on a crate a function of where the viewer was standing,
    /// so `loom render --sim N` and `loom run` would disagree about physics.
    /// `Sim` cannot see a camera at all, which is the structural half of that
    /// guarantee.
    ripples: Option<loom_water::ripples::RippleGrid>,
    /// Bodies that float, in scene order, with their pontoons already in body
    /// space. **Order is fixed at load and never sorted**: the forces are
    /// summed as floats, and a different visiting order is a different number
    /// in the determinism hash.
    floating: Vec<Floating>,
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
            let half = world.collider_half_extents(*entity).map_or_else(
                || {
                    [
                        world_scale.x.abs().max(1e-3),
                        world_scale.y.abs().max(1e-3),
                        world_scale.z.abs().max(1e-3),
                    ]
                },
                |h| [h[0].abs().max(1e-3), h[1].abs().max(1e-3), h[2].abs().max(1e-3)],
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
                                ripple: [0.0; 3],
                            };
                            buoyancy.pontoons.len()
                        ],
                        buoyancy,
                    });
                }
            } else if world.is_renderable(*entity) && !has_dynamic_ancestor(world, *entity) {
                if ball {
                    physics.add_static_ball(pos, radius);
                } else if round {
                    physics.add_static_round(pos, quat, half[1], half[0].max(half[2]), capped);
                } else {
                    physics.add_static_box(pos, quat, half);
                }
            }
        }

        let player = world.player_character().and_then(|entity| {
            characters.iter().position(|w| w.entity == entity)
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
        // **Centred on the water node, which is the anchor ADR 0045 demands.**
        // The node's own world position rather than the scene's origin or the
        // bounds of what floats: it is a thing the author placed and can move,
        // it is in the file, and it does not depend on what happens during the
        // run — a domain that tracked the bodies would move when one sank.
        let ripples = water.as_ref().and_then(|body| body.ripples).and_then(|params| {
            let centre = world
                .entities()
                .iter()
                .find(|e| world.is_water(**e))
                .and_then(|e| world.global_transform(*e))
                .map_or([0.0, 0.0], |g| [g.matrix[12], g.matrix[14]]);
            let grid = loom_water::ripples::RippleGrid::new(centre, &params);
            if grid.is_none() {
                crate::log::warn(
                    "the WaterBody authors ripples the grid could not be built from; \
                     the surface is the plain Gerstner one"
                        .to_owned(),
                );
            }
            grid
        });
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

        Self {
            physics,
            nav: None,
            player,
            dynamic,
            characters,
            water,
            terrain,
            flow,
            ripples,
            floating,
            water_events: Vec::new(),
            tick: 0,
        }
    }

    /// Apply this tick's buoyancy, before the solver runs.
    ///
    /// **Every rule that protects the determinism hash applies in here**, which
    /// is what makes buoyancy different from the water mesh and the grass:
    /// bodies in load order, pontoons in component order, one force and one
    /// torque per body applied once, and the surface read from
    /// `loom_water::sample_water` on the CPU — never from the GPU (§5.1).
    fn float(&mut self) {
        let Some(water) = &self.water else {
            return;
        };
        #[allow(clippy::cast_precision_loss)]
        let t = self.tick as f32 * TICK_SECONDS;

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
                // W6's whole selling point in one line: the height a *previous*
                // tick's disturbance left in the grid is added to the Gerstner
                // surface inside `sample_water`, so a barrel rocks in a crate's
                // wake and `loom sim --assert` can see it happen.
                state.ripple = self
                    .ripples
                    .as_ref()
                    .map_or([0.0; 3], |r| r.at(state.at[0], state.at[2]));
            }

            let wrench = loom_water::buoyancy::solve(
                water,
                &floating.buoyancy,
                &floating.states,
                centre,
                t,
            );
            self.physics
                .apply_force_torque(floating.body, wrench.force, wrench.torque);

            // **The other half of the coupling: the body pushes back.** In
            // pontoon order, like everything else here, and scaled by how much
            // of the body the water actually had hold of — a pontoon waving
            // about in the air must not stir the surface under it.
            if let Some(grid) = self.ripples.as_mut() {
                for state in &floating.states {
                    grid.push(state.at, state.velocity[1], wrench.submerged);
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
            // fires no `submerged` at all, because **the ripple grid's own dent
            // tracks the body down and holds the local fraction under the
            // threshold**. Measured: `events.submerged` is 0 with the `[ripples]`
            // table, >= 1 with it stripped, and >= 1 again at `strength = 0`.
            // So the hysteresis flag can never be the splash trigger, and no
            // amount of tuning either number would have made it one.
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
                    [position[0], position[2]],
                    t,
                    ground,
                    [0.0; 3],
                    // The wake included, so the splash lands on the surface
                    // the body actually broke rather than on the one under it.
                    self.ripples
                        .as_ref()
                        .map_or([0.0; 3], |r| r.at(position[0], position[2])),
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

        // **The grid advances last, after every body has read it and pushed
        // into it.** One place, one order: read → force → inject → step. Any
        // fixed order is deterministic; this one is the one that makes a wake
        // a tick old rather than a tick early, and it is the order the hash is
        // pinned against.
        if let Some(grid) = self.ripples.as_mut() {
            grid.step();
        }
    }

    /// This tick's ripple grid, for whoever is drawing the surface.
    ///
    /// **Read-only, and one direction only.** The CPU grid is authoritative;
    /// the renderer is handed a copy of where it got to and never writes one
    /// back (ADR 0045 clause 2, ADR 0046 §4). `None` is water with no ripples,
    /// which is every scene authored before ADR 0046, and the shader then
    /// returns zero from every lookup — the plain Gerstner surface, bit for
    /// bit.
    #[must_use]
    pub fn ripples(&self) -> Option<&loom_water::ripples::RippleGrid> {
        self.ripples.as_ref()
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
        let ripple = self.ripples.as_ref().map_or([0.0; 3], |r| r.at(at[0], at[2]));
        loom_water::buoyancy::submersion_at(water, at, 0.0, t, ground, ripple) > 0.5
    }

    /// Advance whole ticks.
    pub fn step(&mut self, ticks: u32) {
        for _ in 0..ticks {
            // Forces first, inside the same fixed step, before the solver runs
            // — a force applied after `step` would take effect a tick late and
            // the buoyancy would visibly lag the surface.
            self.float();
            self.physics.step();
            self.tick += 1;
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
            // State from the controller, input from whoever is driving.
            // Every character gets the same input for now: "which character
            // the human is possessing" is a game-state question, and there is
            // exactly one character in a scene until there is a reason for
            // more.
            let motion = loom_script::Motion {
                tick,
                dt: TICK_SECONDS,
                position: walker.character.position(),
                velocity: walker.velocity,
                grounded: walker.grounded,
                ..input.clone()
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

            let motion = loom_script::Motion {
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

/// Whether any ancestor of this node is a dynamic body.
///
/// A renderable child of a moving body used to get its own *static* collider,
/// frozen at the position it spawned in — an invisible wall left behind
/// wherever the parent started, that other bodies then collided with. The
/// child moves because its parent does; it is not scenery.
fn has_dynamic_ancestor(world: &World, entity: loom_ecs::Entity) -> bool {
    let mut current = world.parent(entity);
    // Bounded rather than `while let`: a malformed hierarchy with a cycle
    // would otherwise hang the editor on load, and the scene layer's cycle
    // check is one layer away from here.
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if world.is_dynamic(node) {
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
fn fall_only(motion: &loom_script::Motion) -> [f32; 3] {
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
            let source = std::fs::read_to_string(base.join(script)).map_err(|e| {
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
        let mut rules = None;
        for entity in world.entities() {
            let Some(path) = world.rules_path(*entity) else {
                continue;
            };
            if rules.is_some() {
                crate::log::warn(format!(
                    "more than one GameRules in this scene; ignoring {path}"
                ));
                continue;
            }
            let source = std::fs::read_to_string(base.join(path)).map_err(|e| {
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
            state: loom_script::GameState::default(),
            pending_blasts,
            events: loom_script::EventLog::default(),
            input: loom_script::Motion::default(),
        })
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

    /// This tick's ripple grid, for whoever is drawing the surface.
    ///
    /// Straight through to [`Sim::ripples`] — the simulation owns the grid and
    /// the renderer is only shown it.
    #[must_use]
    pub fn ripples(&self) -> Option<&loom_water::ripples::RippleGrid> {
        self.physics.ripples()
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
            state: loom_script::GameState::default(),
            pending_blasts: Vec::new(),
            events: loom_script::EventLog::default(),
            input: loom_script::Motion::default(),
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
            let (detonations, raised) =
                self.physics
                    .drive_characters(world, tick, input, |entity, motion, memory| {
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

        for (entity, script) in &self.node_scripts {
            let Some(transform) = world.transform(*entity).cloned() else {
                continue;
            };
            let state = loom_script::NodeState {
                position: transform.pos,
                rotation: transform.rot_euler,
                scale: transform.scale,
            };
            let next = self.host.tick(script, tick, &state)?;
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
    /// The character a human drives, and the node the view comes from.
    /// Resolved once at Play: neither can appear mid-run.
    player: Option<loom_ecs::Entity>,
    eye: Option<loom_ecs::Entity>,
}

/// Just short of straight up or down. At exactly ±90° the forward vector is
/// parallel to world up, `right` degenerates, and strafing snaps around.
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl Play {
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

        Self {
            player: world.player_character(),
            eye,
            world,
            runner,
            ticks: 0,
            paused: false,
            leftover: 0.0,
            yaw,
            pitch: pitch.clamp(-MAX_PITCH, MAX_PITCH),
            input: PlayerInput::default(),
            jump_pending: false,
            fire_pending: false,
        }
    }

    /// Replace this frame's held input.
    pub fn set_input(&mut self, input: PlayerInput) {
        // Latched, not overwritten: a press lasts one frame and the tick that
        // consumes it may not have run yet.
        self.jump_pending |= input.jump;
        self.fire_pending |= input.fire;
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

    /// This tick's ripple grid, for the window to displace the surface with.
    ///
    /// `None` in edit mode as well as for water with no ripples, because there
    /// is no simulation then and so nothing has stepped a grid — which is the
    /// same flat surface every scene had before ADR 0046.
    #[must_use]
    pub fn ripples(&self) -> Option<&loom_water::ripples::RippleGrid> {
        self.runner.ripples()
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
        let (yaw, pitch) = (self.yaw.to_degrees(), self.pitch.to_degrees());
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

    /// The full look direction, pitch included. What a shot travels along.
    fn aim(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
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
        let (sin, cos) = self.yaw.sin_cos();
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
    use super::*;
    use loom_scene::Scene;

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
                loom_water::sample_water(&water, [at[0], at[2]], tick as f32 * TICK_SECONDS, 0.0, [0.0; 3], [0.0; 3])
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
            "\n  [node.components.BoxCollider]\n  half_extents = [0.5, 2.0, 0.5]\n",
        ));

        assert!((from_scale - 0.5).abs() < 0.05, "scale-sized: {from_scale}");
        assert!(
            (declared - 2.0).abs() < 0.05,
            "the declared collider is 2.0 tall, so it rests at 2.0, not {declared}"
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
}
