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

/// Euler degrees to a quaternion `(x, y, z, w)`, in the scene format's
/// convention: intrinsic Y-X-Z, ordered `[pitch_x, yaw_y, roll_z]`.
///
/// Composed from the three axis quaternions rather than written out as a
/// closed form: the closed form is where sign errors hide.
#[must_use]
pub fn quat_from_euler(euler: [f32; 3]) -> [f32; 4] {
    let (half_pitch, half_yaw, half_roll) = (
        euler[0].to_radians() * 0.5,
        euler[1].to_radians() * 0.5,
        euler[2].to_radians() * 0.5,
    );
    let qx = [half_pitch.sin(), 0.0, 0.0, half_pitch.cos()];
    let qy = [0.0, half_yaw.sin(), 0.0, half_yaw.cos()];
    let qz = [0.0, 0.0, half_roll.sin(), half_roll.cos()];
    // Y then X then Z, matching the extraction order in `euler_from_quat`.
    quat_mul(quat_mul(qy, qx), qz)
}

/// A quaternion back to euler degrees, the exact inverse of
/// [`quat_from_euler`]. A round-trip test holds the pair together.
#[must_use]
pub fn euler_from_quat(q: [f32; 4]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    // The pitch term is clamped because floating-point drift can push it just
    // past ±1, and asin of that is NaN — which would silently poison every
    // transform downstream.
    let sin_pitch = (2.0 * (w * x - y * z)).clamp(-1.0, 1.0);
    let pitch = sin_pitch.asin();

    // Straight up or straight down, yaw and roll describe the same rotation and
    // the general formulas both collapse to `atan2(0, 0)` — which returns zero
    // and silently re-orients the node by however much roll it actually had.
    // Fold the pair into yaw and zero the roll, which is the conventional and
    // reversible choice.
    if sin_pitch.abs() > 0.999_999 {
        // `2*atan2(y, w)` for this composition order — derived numerically
        // against `quat_from_euler` at both poles rather than copied from a
        // reference for some other convention, which is how the first attempt
        // here silently dropped the yaw entirely.
        let yaw = 2.0 * y.atan2(w);
        return [pitch.to_degrees(), yaw.to_degrees(), 0.0];
    }

    let yaw = (2.0 * (w * y + x * z)).atan2(1.0 - 2.0 * (x * x + y * y));
    let roll = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (x * x + z * z));
    [pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()]
}

/// A quaternion as rapier's scaled axis-angle (`axis * angle` in 3D).
fn scaled_axis_from_quat(q: [f32; 4]) -> AngVector {
    let [x, y, z, w] = q;
    // Near identity the axis is degenerate, so return zero rather than
    // dividing by ~0.
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
        rotation: [f32; 4],
        half_extents: [f32; 3],
    ) -> ColliderHandle {
        let collider = ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2])
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
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
        rotation: [f32; 4],
        half_extents: [f32; 3],
        mass: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
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
        rotation: [f32; 4],
        radius: f32,
        mass: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
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
        rotation: [f32; 4],
        voxel_size: [f32; 3],
        grid: &[[i32; 3]],
    ) -> Option<ColliderHandle> {
        if grid.is_empty() {
            return None;
        }
        let cells: Vec<IVector> = grid.iter().map(|c| IVector::new(c[0], c[1], c[2])).collect();
        // Per-axis voxel size, so a node's scale reaches the collider. parry's
        // voxel shape takes a `Vector` rather than a scalar, so even a
        // non-uniformly scaled volume is representable exactly.
        let size = Vector::new(
            voxel_size[0].max(1e-4),
            voxel_size[1].max(1e-4),
            voxel_size[2].max(1e-4),
        );
        let collider = ColliderBuilder::voxels(size, &cells)
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
            .build();
        Some(self.colliders.insert(collider))
    }

    /// A dynamic cylinder or capsule, upright along Y.
    ///
    /// The last of the mesh/collider mismatches: `capsule` and `cylinder`
    /// meshes were both simulated as cuboids, so a cylinder stood on a square
    /// footprint and a capsule refused to roll. `half_height` is the straight
    /// section for a capsule — rapier adds the two hemispheres on top, exactly
    /// as `loom_asset::primitives::capsule` draws them.
    pub fn add_round_body(
        &mut self,
        position: [f32; 3],
        rotation: [f32; 4],
        half_height: f32,
        radius: f32,
        capped: bool,
        mass: f32,
    ) -> RigidBodyHandle {
        let body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
            .build();
        let handle = self.bodies.insert(body);
        let (half_height, radius) = (half_height.max(1e-3), radius.max(1e-3));
        // `half_height` is already the straight section — `primitives::capsule`
        // draws two hemispheres of `radius` at ±`half_height`, so the drawn
        // shape spans `half_height + radius`. Subtracting the radius here made
        // every capsule one radius too short, and at the default scale
        // collapsed the straight section to nothing.
        let shape = if capped {
            ColliderBuilder::capsule_y(half_height, radius)
        } else {
            ColliderBuilder::cylinder(half_height, radius)
        };
        let collider = shape.mass(mass.max(0.001)).build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        handle
    }

    /// A static cylinder or capsule, upright along Y.
    pub fn add_static_round(
        &mut self,
        position: [f32; 3],
        rotation: [f32; 4],
        half_height: f32,
        radius: f32,
        capped: bool,
    ) -> ColliderHandle {
        let (half_height, radius) = (half_height.max(1e-3), radius.max(1e-3));
        // See `add_round_body`: the straight section is what the caller passes.
        let shape = if capped {
            ColliderBuilder::capsule_y(half_height, radius)
        } else {
            ColliderBuilder::cylinder(half_height, radius)
        };
        let collider = shape
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(scaled_axis_from_quat(rotation))
            .build();
        self.colliders.insert(collider)
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
        self.rotation_quat(handle).map(euler_from_quat)
    }

    /// A body's orientation as a quaternion `(x, y, z, w)`.
    ///
    /// The boundary representation: composing a body's pose with a parent's
    /// inverse needs a rotation that composes, and euler triples do not.
    #[must_use]
    pub fn rotation_quat(&self, handle: RigidBodyHandle) -> Option<[f32; 4]> {
        let q = self.bodies.get(handle)?.rotation();
        Some([q.x, q.y, q.z, q.w])
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
            let handle = physics.add_box_body([0.0, 0.0, 0.0], quat_from_euler(euler), [0.5, 0.5, 0.5], 1.0);
            if let Some(body) = physics.bodies.get_mut(handle) {
                body.set_linvel(super::Vector::new(velocity, 0.0, 0.0), true);
            }
            physics.state_hash()
        };

        let base = spun([0.0, 0.0, 0.0], 0.0);
        assert_ne!(base, spun([0.0, 45.0, 0.0], 0.0), "rotation must count");
        assert_ne!(base, spun([0.0, 0.0, 0.0], 3.0), "velocity must count");
    }

    /// The writer and the reader must be inverses **as rotations**.
    ///
    /// Not as euler triples: a triple is a spelling, not a rotation, and at
    /// gimbal lock several spellings denote the same orientation. Comparing
    /// components would fail on a correct implementation and pass on one that
    /// merely echoed its input. Re-encoding what came back and comparing
    /// quaternions tests the property that actually matters.
    #[test]
    fn euler_survives_a_round_trip_as_a_rotation() {
        for euler in [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 45.0],
            [30.0, 0.0, 0.0],
            [0.0, 90.0, 0.0],
            [15.0, -40.0, 70.0],
            [-25.0, 160.0, -80.0],
            // Gimbal lock: the general extraction degenerates here.
            //
            // Roll must be non-zero and the south pole must carry yaw, or the
            // cases prove nothing: with roll = 0 and yaw = 0 almost any
            // formula returns the right answer, and a review demonstrated that
            // by swapping the pole branch for one 180 degrees wrong at the
            // south pole and watching the suite stay green.
            [90.0, 0.0, 0.0],
            [-90.0, 0.0, 0.0],
            [90.0, 30.0, 0.0],
            [90.0, 0.0, 40.0],
            [90.0, 25.0, -35.0],
            [-90.0, 60.0, 0.0],
            [-90.0, 0.0, -50.0],
            [-90.0, -110.0, 20.0],
        ] {
            let mut physics = super::Physics::new(1.0 / 60.0);
            let handle = physics.add_box_body(
                [0.0, 0.0, 0.0],
                super::quat_from_euler(euler),
                [0.5, 0.5, 0.5],
                1.0,
            );
            let round_tripped = super::quat_from_euler(
                physics.rotation_euler(handle).expect("body exists"),
            );
            let original = super::quat_from_euler(euler);

            // q and -q are the same rotation, so compare |dot|.
            let dot: f32 = (0..4).map(|i| original[i] * round_tripped[i]).sum();
            assert!(
                dot.abs() > 0.9999,
                "{euler:?} came back as a different rotation (dot {dot})"
            );
        }
    }

    use super::*;

    /// **The M7 exit criterion.** A capsule falls onto a floor and stays on it.
    #[test]
    fn a_capsule_comes_to_rest_on_a_floor() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -0.5, 0.0], quat_from_euler([0.0, 0.0, 0.0]), [10.0, 0.5, 10.0]);
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
        physics.add_static_box([0.0, 0.0, 0.0], quat_from_euler([0.0, 0.0, 0.0]), [10.0, 0.05, 10.0]);
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
        physics.add_static_box([0.0, -0.5, 0.0], quat_from_euler([0.0, 0.0, 0.0]), [20.0, 0.5, 20.0]);
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
            physics.add_static_box([0.0, -0.5, 0.0], quat_from_euler([0.0, 0.0, 0.0]), [10.0, 0.5, 10.0]);
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
