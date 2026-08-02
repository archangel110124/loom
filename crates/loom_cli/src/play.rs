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

/// A physics world built from a scene, ready to be stepped.
pub struct Sim {
    physics: Physics,
    /// Entities with a body, and the body they got.
    dynamic: Vec<(loom_ecs::Entity, RigidBodyHandle)>,
    /// Entities with a `CharacterController`, and their walking state.
    characters: Vec<Walker>,
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
                    Some((volume, ())) => {
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
                    None => crate::log::warn(format!(
                        "{}: voxel recipe did not rebuild; terrain will not collide",
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

        Self {
            physics,
            dynamic,
            characters,
        }
    }

    /// Advance whole ticks.
    pub fn step(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.physics.step();
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
        mut velocity_for: impl FnMut(
            loom_ecs::Entity,
            &loom_script::Motion,
            &mut loom_script::ScriptMemory,
        ) -> Result<[f32; 3], E>,
    ) -> Result<(), E> {
        for walker in &mut self.characters {
            let motion = loom_script::Motion {
                tick,
                dt: TICK_SECONDS,
                position: walker.character.position(),
                velocity: walker.velocity,
                grounded: walker.grounded,
            };
            let velocity = velocity_for(walker.entity, &motion, &mut walker.memory)?;

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
        Ok(())
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

/// Play mode as the editor holds it: a scene's world, its simulation, and how
/// far it has been run.
pub struct Play {
    pub world: World,
    sim: Sim,
    /// Whole ticks run. Time is counted here, not in seconds.
    pub ticks: u32,
    pub paused: bool,
    /// Left over from the last frame's real elapsed time. Fixed timestep with
    /// an accumulator: the wall clock decides *how many* ticks to run, and
    /// never what a tick is worth.
    leftover: f32,
}

impl Play {
    #[must_use]
    pub fn start(world: World) -> Self {
        let sim = Sim::new(&world);
        Self {
            world,
            sim,
            ticks: 0,
            paused: false,
            leftover: 0.0,
        }
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
    pub fn run(&mut self, ticks: u32) {
        self.sim.step(ticks);
        self.sim.write_back(&mut self.world);
        self.ticks += ticks;
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
        self.sim.body_count()
    }

    #[must_use]
    pub fn state_hash(&self) -> u64 {
        self.sim.state_hash()
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
        let mut play = Play::start(world);

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
        let mut play = Play::start(world);

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
        let mut play = Play::start(world);

        play.run(400);
        let y = height(&play.world, "Terrain/Probe");

        // The hill's summit is around y = 15.5; the ground slab spans 0..6.
        // Anything above zero means it landed on the terrain rather than
        // through it.
        assert!(y > 1.0, "the probe fell through the voxel terrain: y = {y}");
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
            let mut play = Play::start(world);
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
        let mut play = Play::start(world);
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
            let mut play = Play::start(world);
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
        let mut play = Play::start(world);
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
            let mut play = Play::start(world);
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
        let mut play = Play::start(world);
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

    #[test]
    fn playing_makes_a_crate_fall() {
        let mut play = Play::start(world());
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
        let mut stuttering = Play::start(world());
        // Roughly a second, delivered in uneven lumps.
        for dt in [0.05_f32, 0.002, 0.13, 0.008, 0.24, 0.06, 0.21, 0.09, 0.21] {
            stuttering.advance(dt);
        }
        assert!(stuttering.ticks > 50, "about a second ran: {}", stuttering.ticks);

        // The same number of ticks, delivered as a clean 60 fps.
        let mut steady = Play::start(world());
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
        let mut play = Play::start(world());
        play.paused = true;

        play.advance(1.0);

        assert_eq!(play.ticks, 0);
        // Step works anyway — that is what a step button is.
        play.run(1);
        assert_eq!(play.ticks, 1);
    }

    /// A stall must not turn into a thousand catch-up ticks.
    #[test]
    fn a_long_stall_is_clamped() {
        let mut play = Play::start(world());

        play.advance(30.0);

        assert!(play.ticks <= 15, "clamped, not caught up: {}", play.ticks);
    }
}
