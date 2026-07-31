//! `rapier3d` integration, and the physical sanity checks that matter more.
//!
//! Graphics doc §C.5 names the problem exactly: **in an AI-authored engine,
//! physics robustness is substantially a validation problem, not a solver
//! problem.** An agent authoring scenes is an uncontrolled-content generator.
//! It will produce extreme mass ratios, hundred-unit thin colliders, and
//! bodies spawned interpenetrating — with no idea anything is wrong, because
//! nothing in a text scene file looks unusual. The symptom is "the physics is
//! broken", and it is unattributable.
//!
//! So [`sanity`] runs first and reports in the same structured shape as every
//! other rejection, because that is where the agent can act on it.

pub mod sanity;

pub use sanity::{Severity, check_scene};
// Re-exported so callers can hold a body handle without taking a direct
// dependency on rapier. The engine choice stays behind this crate's door.
pub use rapier3d::prelude::RigidBodyHandle;

use rapier3d::prelude::*;

/// A physics world stepped at a fixed rate.
pub struct Physics {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    integration: IntegrationParameters,
    gravity: Vector,
}

impl Default for Physics {
    fn default() -> Self {
        Self::new(1.0 / 60.0)
    }
}


/// Quaternion product, `(x, y, z, w)`.
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Euler degrees to rapier's scaled axis-angle, in the scene format's
/// convention: intrinsic Y-X-Z, ordered `[pitch_x, yaw_y, roll_z]`.
///
/// The exact inverse of [`Physics::rotation_euler`], and a round-trip test
/// holds them to it. Without this, a node authored with a rotation simulated
/// as though it were upright — a tilted crate snapped flat the moment physics
/// touched it, and the render disagreed with the simulation about where its
/// faces were.
///
/// Composed from the three axis quaternions rather than written out as a
/// closed form: the closed form is where sign errors hide, and this is a
/// handful of multiplies once per body.
fn scaled_axis_from_euler(euler: [f32; 3]) -> AngVector {
    let (half_pitch, half_yaw, half_roll) = (
        euler[0].to_radians() * 0.5,
        euler[1].to_radians() * 0.5,
        euler[2].to_radians() * 0.5,
    );
    let qx = [half_pitch.sin(), 0.0, 0.0, half_pitch.cos()];
    let qy = [0.0, half_yaw.sin(), 0.0, half_yaw.cos()];
    let qz = [0.0, 0.0, half_roll.sin(), half_roll.cos()];
    // Y then X then Z, matching the extraction order in `rotation_euler`.
    let [x, y, z, w] = quat_mul(quat_mul(qy, qx), qz);

    // Scaled axis-angle is what `RigidBodyBuilder::rotation` wants in 3D
    // (rapier's own doc: "axis * angle"). Near identity the axis is
    // degenerate, so return zero rather than dividing by ~0.
    let sin_half = (1.0 - w * w).max(0.0).sqrt();
    if sin_half < 1e-6 {
        return AngVector::new(0.0, 0.0, 0.0);
    }
    let angle = 2.0 * w.clamp(-1.0, 1.0).acos();
    AngVector::new(
        x / sin_half * angle,
        y / sin_half * angle,
        z / sin_half * angle,
    )
}

impl Physics {
    /// A world stepping at `dt` seconds.
    ///
    /// The timestep is fixed and comes from the caller (never-do #8): a
    /// simulation that reads the clock cannot be replayed, and replay is what
    /// makes `loom sim --assert` trustworthy.
    #[must_use]
    pub fn new(dt: f32) -> Self {
        let integration = IntegrationParameters {
            dt,
            ..IntegrationParameters::default()
        };
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            integration,
            gravity: Vector::new(0.0, -9.81, 0.0),
        }
    }

    /// A static box collider — the shape a `BoxCollider` component becomes.
    pub fn add_static_box(
        &mut self,
        position: [f32; 3],
        rotation: [f32; 3],
        half_extents: [f32; 3],
    ) -> ColliderHandle {
        let collider = ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2])
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_euler(rotation))
            .build();
        self.colliders.insert(collider)
    }

    /// A dynamic capsule — the character shape.
    ///
    /// A capsule, not a box: it does not catch on the seams between floor
    /// colliders, which is the classic reason a character stutters walking
    /// over flat ground.
    pub fn add_capsule(
        &mut self,
        position: [f32; 3],
        half_height: f32,
        radius: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            // A character should not tip over. Locking rotation is what makes
            // a capsule behave like a character rather than a barrel.
            .lock_rotations()
            .build();
        let handle = self.bodies.insert(body);
        let collider = ColliderBuilder::capsule_y(half_height, radius).build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        handle
    }

    /// A dynamic box — what a `RigidBody { dynamic = true }` node becomes.
    ///
    /// Rotation is NOT locked here, unlike the character capsule: a falling
    /// crate tumbling is correct, and a crate that refuses to tip looks wrong.
    pub fn add_box_body(
        &mut self,
        position: [f32; 3],
        rotation: [f32; 3],
        half_extents: [f32; 3],
        mass: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_euler(rotation))
            .build();
        let handle = self.bodies.insert(body);
        let collider =
            ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2])
                .mass(mass.max(0.001))
                .build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        handle
    }

    /// A static sphere.
    pub fn add_static_ball(&mut self, position: [f32; 3], radius: f32) -> ColliderHandle {
        let collider = ColliderBuilder::ball(radius.max(1e-3))
            .translation(Vector::new(position[0], position[1], position[2]))
            .build();
        self.colliders.insert(collider)
    }

    /// A dynamic sphere.
    ///
    /// **The shape has to match the mesh.** A sphere simulated as a cuboid
    /// rests on whichever face is down, so a tilted one settles its centre at
    /// `radius * sqrt(2)` rather than `radius` — and the drawn sphere then
    /// hangs above the thing it landed on or sinks into it. The simulation is
    /// self-consistent and the picture is a lie, which is the failure mode the
    /// brief cares most about.
    pub fn add_ball_body(
        &mut self,
        position: [f32; 3],
        rotation: [f32; 3],
        radius: f32,
        mass: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_euler(rotation))
            .build();
        let handle = self.bodies.insert(body);
        let collider = ColliderBuilder::ball(radius.max(1e-3))
            .mass(mass.max(0.001))
            .build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        handle
    }

    /// A static collider made of solid voxel cells.
    ///
    /// **This is the locked decision in CLAUDE.md finally wired up**: "rapier3d;
    /// voxel colliders for terrain". Before it, a `VoxelVolume` node was given
    /// a cuboid sized from its `scale` — a 1x1x1 box standing in for a whole
    /// hillside — and everything fell through the terrain.
    ///
    /// A trimesh is the obvious alternative and the wrong one (never-do #10):
    /// a surface-extracted mesh is an infinitely thin membrane, so a fast body
    /// tunnels through it and a body that ends up inside has nothing pushing
    /// it out. Voxels are solid volume, and parry culls the interior itself.
    ///
    /// `grid` holds the integer cell coordinates of solid voxels. parry places
    /// cell `k` at `(k + 0.5) * voxel_size`, which is exactly what
    /// `loom_voxel::Volume::world_of` computes — so the collider lands on the
    /// same coordinates as the drawn surface with no offset applied here.
    /// `voxel_grid_matches_parry` in `loom_voxel` holds those two conventions
    /// together.
    pub fn add_static_voxels(
        &mut self,
        position: [f32; 3],
        voxel_size: f32,
        grid: &[[i32; 3]],
    ) -> Option<ColliderHandle> {
        if grid.is_empty() {
            return None;
        }
        let cells: Vec<IVector> = grid.iter().map(|c| IVector::new(c[0], c[1], c[2])).collect();
        let collider = ColliderBuilder::voxels(Vector::splat(voxel_size.max(1e-4)), &cells)
            .translation(Vector::new(position[0], position[1], position[2]))
            .build();
        Some(self.colliders.insert(collider))
    }

    /// Spawn a debris chunk with an initial velocity.
    ///
    /// **Convex, never a trimesh** (never-do #10, voxel doc §4): a
    /// surface-extracted mesh is an infinitely thin membrane with nothing
    /// behind it, and putting one on a dynamic body gives ghost collisions and
    /// tunnelling. A box is solid volume and cannot do either.
    ///
    /// Returns `None` once `cap` bodies exist. **Uncapped debris is the
    /// classic way a destructible game dies** — the voxel doc is blunt about
    /// pooling aggressively with a hard limit, so the cap is a parameter here
    /// rather than a thing callers are trusted to remember.
    pub fn spawn_debris(
        &mut self,
        position: [f32; 3],
        half_extent: f32,
        velocity: [f32; 3],
        cap: usize,
    ) -> Option<RigidBodyHandle> {
        if self.bodies.len() >= cap {
            return None;
        }
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .linvel(Vector::new(velocity[0], velocity[1], velocity[2]))
            // Debris tumbling is most of what sells destruction, so rotation
            // stays free — unlike the character capsule, which locks it.
            .angular_damping(0.4)
            .build();
        let handle = self.bodies.insert(body);
        let collider = ColliderBuilder::cuboid(half_extent, half_extent, half_extent)
            .friction(0.8)
            .restitution(0.05)
            .build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        Some(handle)
    }

    /// A static collider built from a mesh's triangles.
    ///
    /// Legal here and **only** here: this is static terrain. Rapier's own
    /// guidance is against trimesh colliders on DYNAMIC bodies, and never-do
    /// #10 forbids it outright — debris uses boxes.
    pub fn add_static_trimesh(&mut self, vertices: &[[f32; 3]], indices: &[u32]) -> Option<ColliderHandle> {
        if indices.len() < 3 {
            return None;
        }
        // rapier 0.34 takes `Vector`, not `Point` — glam-backed, not nalgebra.
        let points: Vec<Vector> = vertices
            .iter()
            .map(|v| Vector::new(v[0], v[1], v[2]))
            .collect();
        let triangles: Vec<[u32; 3]> = indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let collider = ColliderBuilder::trimesh(points, triangles).ok()?.build();
        Some(self.colliders.insert(collider))
    }

    /// How many bodies exist, for the debris cap.
    #[must_use]
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Advance one fixed step.
    pub fn step(&mut self) {
        self.pipeline.step(
            self.gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
    }

    /// Where a body is now.
    #[must_use]
    pub fn position(&self, handle: RigidBodyHandle) -> Option<[f32; 3]> {
        let t = self.bodies.get(handle)?.translation();
        Some([t.x, t.y, t.z])
    }

    /// A body's orientation as euler degrees, in the scene format's
    /// convention: intrinsic Y-X-Z, ordered `[pitch_x, yaw_y, roll_z]`
    /// (`docs/format/README.md` §1.1).
    ///
    /// Converted here rather than exposing a quaternion, so the physics
    /// boundary speaks the same language as the scene files and nothing
    /// downstream has to know which convention rapier uses.
    #[must_use]
    pub fn rotation_euler(&self, handle: RigidBodyHandle) -> Option<[f32; 3]> {
        let q = self.bodies.get(handle)?.rotation();
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);

        // Y-X-Z extraction. The pitch term is clamped because floating-point
        // drift can push it just past ±1, and asin of that is NaN — which
        // would silently poison every transform downstream.
        let sin_pitch = (2.0 * (w * x - y * z)).clamp(-1.0, 1.0);
        let pitch = sin_pitch.asin();
        let yaw = (2.0 * (w * y + x * z)).atan2(1.0 - 2.0 * (x * x + y * y));
        let roll = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (x * x + z * z));

        Some([pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()])
    }

    /// Fold every body's position into a hash, for the determinism check.
    ///
    /// Bit patterns, in handle order, matching `loom_ecs::World::state_hash`.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let eat = |hash: &mut u64, bytes: &[u8]| {
            for byte in bytes {
                *hash ^= u64::from(*byte);
                *hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        };
        // `RigidBodySet` iteration is by handle index, which is assigned in
        // insertion order — deterministic for a given scene load.
        for (_, body) in self.bodies.iter() {
            let t = body.translation();
            let r = body.rotation();
            let v = body.linvel();
            let w = body.angvel();
            // **Position alone is not the state.** A hash over translation only
            // called two runs identical when a body had settled in the same
            // place spinning a different way, or was passing through the same
            // point at a different speed — so `loom sim`'s determinism hash
            // could agree while the simulations genuinely disagreed. Rotation
            // and both velocities are part of what the next tick depends on,
            // so they are part of what the hash has to cover.
            for value in [
                t.x, t.y, t.z, r.x, r.y, r.z, r.w, v.x, v.y, v.z, w.x, w.y, w.z,
            ] {
                eat(&mut hash, &value.to_bits().to_le_bytes());
            }
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    /// The hash has to notice everything the next tick depends on. It covered
    /// translation only, so a body resting in the same place with a different
    /// orientation — or moving through it at a different speed — hashed the
    /// same, and `loom sim`'s determinism check would have missed a real
    /// divergence.
    #[test]
    fn the_state_hash_notices_rotation_and_velocity() {
        let spun = |euler: [f32; 3], velocity: f32| {
            let mut physics = super::Physics::new(1.0 / 60.0);
            let handle = physics.add_box_body([0.0, 0.0, 0.0], euler, [0.5, 0.5, 0.5], 1.0);
            if let Some(body) = physics.bodies.get_mut(handle) {
                body.set_linvel(super::Vector::new(velocity, 0.0, 0.0), true);
            }
            physics.state_hash()
        };

        let base = spun([0.0, 0.0, 0.0], 0.0);
        assert_ne!(base, spun([0.0, 45.0, 0.0], 0.0), "rotation must count");
        assert_ne!(base, spun([0.0, 0.0, 0.0], 3.0), "velocity must count");
    }

    /// The writer and the reader must be inverses. They are hand-rolled Y-X-Z
    /// conversions on opposite sides of the physics boundary, and nothing else
    /// would notice if they drifted: a scene would simply simulate at a
    /// slightly different angle than it was authored at.
    #[test]
    fn euler_survives_a_round_trip_through_the_body() {
        for euler in [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 45.0],
            [30.0, 0.0, 0.0],
            [0.0, 90.0, 0.0],
            [15.0, -40.0, 70.0],
            [-25.0, 160.0, -80.0],
        ] {
            let mut physics = super::Physics::new(1.0 / 60.0);
            let handle = physics.add_box_body([0.0, 0.0, 0.0], euler, [0.5, 0.5, 0.5], 1.0);
            let back = physics.rotation_euler(handle).expect("body exists");
            for axis in 0..3 {
                assert!(
                    (back[axis] - euler[axis]).abs() < 0.01,
                    "{euler:?} came back as {back:?}"
                );
            }
        }
    }

    use super::*;

    /// **The M7 exit criterion.** A capsule falls onto a floor and stays on it.
    #[test]
    fn a_capsule_comes_to_rest_on_a_floor() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -0.5, 0.0], [0.0, 0.0, 0.0], [10.0, 0.5, 10.0]);
        let capsule = physics.add_capsule([0.0, 4.0, 0.0], 0.5, 0.4);

        for _ in 0..240 {
            physics.step();
        }

        let y = physics.position(capsule).expect("capsule exists")[1];
        // Floor top is y=0; capsule half-height 0.5 plus radius 0.4 puts its
        // centre at ~0.9 when resting.
        assert!(
            (0.6..1.2).contains(&y),
            "capsule should rest on the floor, got y={y}"
        );
    }

    /// It must not tunnel through a thin floor, which is the failure that
    /// makes physics feel broken (graphics doc §C.4).
    #[test]
    fn a_falling_capsule_does_not_tunnel_through_a_thin_floor() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [10.0, 0.05, 10.0]);
        let capsule = physics.add_capsule([0.0, 6.0, 0.0], 0.5, 0.4);

        for _ in 0..300 {
            physics.step();
        }

        let y = physics.position(capsule).expect("capsule exists")[1];
        assert!(y > -1.0, "capsule fell through the floor to y={y}");
    }

    /// The debris cap is not advisory. Uncapped debris is how a destructible
    /// game dies, so the limit is enforced where debris is created.
    #[test]
    fn debris_stops_spawning_at_the_cap() {
        let mut physics = Physics::new(1.0 / 60.0);
        let mut spawned = 0;
        for i in 0..50 {
            #[allow(clippy::cast_precision_loss)]
            if physics
                .spawn_debris([i as f32, 5.0, 0.0], 0.2, [0.0, 1.0, 0.0], 12)
                .is_some()
            {
                spawned += 1;
            }
        }

        assert_eq!(spawned, 12, "the cap must hold");
    }

    #[test]
    fn debris_launched_upward_actually_moves() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -0.5, 0.0], [0.0, 0.0, 0.0], [20.0, 0.5, 20.0]);
        let chunk = physics
            .spawn_debris([0.0, 1.0, 0.0], 0.2, [4.0, 9.0, 0.0], 100)
            .expect("under the cap");

        for _ in 0..15 {
            physics.step();
        }

        let p = physics.position(chunk).unwrap();
        assert!(p[1] > 1.5, "should have risen, y={}", p[1]);
        assert!(p[0] > 0.3, "should have travelled, x={}", p[0]);
    }

    /// **The other half of M7's exit criterion**: determinism survives physics.
    #[test]
    fn the_same_simulation_twice_produces_the_same_hash() {
        let run = || {
            let mut physics = Physics::new(1.0 / 60.0);
            physics.add_static_box([0.0, -0.5, 0.0], [0.0, 0.0, 0.0], [10.0, 0.5, 10.0]);
            for i in 0..8 {
                #[allow(clippy::cast_precision_loss)]
                physics.add_capsule([i as f32 * 0.35, 3.0 + i as f32, 0.0], 0.5, 0.4);
            }
            for _ in 0..180 {
                physics.step();
            }
            physics.state_hash()
        };

        assert_eq!(run(), run(), "physics must replay identically");
    }
}
