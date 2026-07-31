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

/// The fixed tick. Simulation time is counted in these, never in seconds of
/// wall clock.
pub const TICK_SECONDS: f32 = 1.0 / 60.0;

/// A physics world built from a scene, ready to be stepped.
pub struct Sim {
    physics: Physics,
    /// Entities with a body, and the body they got.
    dynamic: Vec<(loom_ecs::Entity, RigidBodyHandle)>,
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

        for entity in world.entities() {
            let Some(global) = world.global_transform(*entity) else {
                continue;
            };
            let pos = [global.matrix[12], global.matrix[13], global.matrix[14]];
            // Half-extents come from the node's scale: a unit box scaled by s
            // has half-extents s, which is what the renderer draws.
            let Some(transform) = world.transform(*entity) else {
                continue;
            };
            // An authored `BoxCollider` wins over the mesh's scale. It is a
            // documented, schema-validated component that the simulation used
            // to ignore entirely, so a node could declare one size and collide
            // as another with nothing reporting the discrepancy.
            let half = world.collider_half_extents(*entity).map_or_else(
                || {
                    [
                        transform.scale[0].abs().max(1e-3),
                        transform.scale[1].abs().max(1e-3),
                        transform.scale[2].abs().max(1e-3),
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
            let rotation = transform.rot_euler;
            let ball = world.mesh_asset(*entity) == Some("sphere");
            // The enclosing radius, not the smallest: a non-uniformly scaled
            // sphere is an ellipsoid that no ball matches, and of the two
            // wrong answers a collider that contains the drawn shape is the
            // one that does not let geometry poke through.
            let radius = half[0].max(half[1]).max(half[2]);

            // A voxel volume is terrain, not a box. Its recipe rides on the
            // world (never-do #11: the scene stores the op list, never the
            // voxels), so the field is rebuilt here and handed to parry as
            // solid cells. Static only: a destructible hillside is scenery,
            // and a trimesh on a dynamic body is never-do #10.
            if let Some(recipe) = world.voxel_recipe(*entity) {
                match crate::build_volume(recipe) {
                    Some((volume, ())) => {
                        let cells = volume.solid_cells();
                        if physics
                            .add_static_voxels(pos, volume.voxel_size, &cells)
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
                    physics.add_ball_body(pos, rotation, radius, mass)
                } else {
                    physics.add_box_body(pos, rotation, half, mass)
                };
                dynamic.push((*entity, handle));
            } else if world.is_renderable(*entity) {
                if ball {
                    physics.add_static_ball(pos, radius);
                } else {
                    physics.add_static_box(pos, rotation, half);
                }
            }
        }

        Self { physics, dynamic }
    }

    /// Advance whole ticks.
    pub fn step(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.physics.step();
        }
    }

    /// Copy body positions back onto the world's transforms.
    pub fn write_back(&self, world: &mut World) {
        for (entity, handle) in &self.dynamic {
            let Some(pos) = self.physics.position(*handle) else {
                continue;
            };
            let rotation = self.physics.rotation_euler(*handle);
            if let Some(transform) = world.transform_mut(*entity) {
                // Written back as a LOCAL transform. Correct only for nodes
                // whose parent is the root, which is every dynamic body a
                // blockout makes today. Nested dynamic bodies need the
                // parent's inverse — noted rather than silently wrong.
                transform.pos = pos;
                // Rotation too, or a toppling crate slides instead of tipping
                // — the simulation would be right and the picture a lie.
                if let Some(rot) = rotation {
                    transform.rot_euler = rot;
                }
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
