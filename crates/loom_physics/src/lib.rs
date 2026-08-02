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
pub use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};

use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude::*;

/// A physics world stepped at a fixed rate.
/// What a ray struck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The collider that was hit, for mapping back to whatever owns it.
    pub collider: ColliderHandle,
    /// Metres along the ray.
    pub distance: f32,
    /// Where it struck, in world space.
    pub point: [f32; 3],
    /// The surface normal there — which is what an impact decal, a ricochet
    /// and a spray of debris all need to be oriented by.
    pub normal: [f32; 3],
}

/// The capsule a character occupies, and what it is able to climb.
///
/// Shape and mobility together because they are not separable: the step a
/// character can walk up and the width that has to fit through a doorway are
/// the same measurements from the level designer's side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterShape {
    /// Half the capsule's straight section. Total standing height is
    /// `2 * (half_height + radius)`.
    pub half_height: f32,
    pub radius: f32,
    /// Steepest floor still counted as ground, in degrees. Anything steeper
    /// is a wall to slide down rather than a ramp to walk up.
    pub max_slope_degrees: f32,
    /// Tallest ledge stepped over without jumping. Zero disables stepping.
    pub step_height: f32,
}

impl Default for CharacterShape {
    fn default() -> Self {
        Self {
            // 1.9 m standing, which puts an eye node at the 1.7 m that reads
            // as human height in a rendered scene.
            half_height: 0.6,
            radius: 0.35,
            max_slope_degrees: 50.0,
            step_height: 0.35,
        }
    }
}

/// A character capsule in the world, and where it currently is.
///
/// Position is held here rather than read back from the body: the body is
/// kinematic and only catches up during the next `step`, so reading it would
/// lag a tick behind and make two moves in one tick sweep from the wrong place.
pub struct Character {
    body: RigidBodyHandle,
    controller: KinematicCharacterController,
    shape: CharacterShape,
    position: [f32; 3],
    grounded: bool,
}

impl Character {
    #[must_use]
    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    #[must_use]
    pub fn is_grounded(&self) -> bool {
        self.grounded
    }
}

/// What one step of movement actually did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterMove {
    pub position: [f32; 3],
    /// The velocity that survived collision — see [`Physics::move_character`].
    pub velocity: [f32; 3],
    pub grounded: bool,
}

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
    /// Cast a ray and return the nearest hit.
    ///
    /// **The foundation for most of what a shooter does.** A hitscan weapon is
    /// this. So is deciding whether a character is standing on something, what
    /// the crosshair is over, whether a grenade has line of sight, and where an
    /// explosion's blast is blocked. Building any of those without it means
    /// each one inventing its own intersection test against a different idea of
    /// the world.
    ///
    /// Against the *physics* world, not the render world. They are separate on
    /// purpose — a shot must hit what the simulation says is there, not what
    /// happens to be drawn — but that only pays off if colliders and geometry
    /// agree, which is what `check_scene` exists to police.
    ///
    /// `direction` need not be normalized; distances are reported in metres
    /// along it either way.
    ///
    /// # The world it queries is the one the last `step` left
    ///
    /// The acceleration structure this walks is built during [`Self::step`],
    /// so a collider added since then is **invisible to a ray**. In the normal
    /// loop that is exactly right and costs nothing — gameplay queries run
    /// after the tick that placed everything. It is a trap only when adding a
    /// collider and immediately casting against it, which returns `None` and
    /// looks like a miss rather than a mistake. Step first.
    pub fn raycast(&self, origin: [f32; 3], direction: [f32; 3], max_distance: f32) -> Option<RayHit> {
        let dir = Vector::new(direction[0], direction[1], direction[2]);
        // A zero direction is a degenerate ray, not a hit at the origin. Rapier
        // would normalise it into a NaN and report nonsense.
        let length = dir.length();
        if !length.is_finite() || length < 1e-6 || !max_distance.is_finite() {
            return None;
        }
        let ray = Ray::new(Vector::new(origin[0], origin[1], origin[2]), dir / length);

        let query = self.broad_phase.as_query_pipeline(
            self.narrow_phase.query_dispatcher(),
            &self.bodies,
            &self.colliders,
            QueryFilter::default(),
        );
        // `solid: true` — a ray starting inside a shape hits immediately at
        // distance zero rather than passing through and striking the far wall
        // from within. That is what a muzzle already clipping a wall should do.
        let (collider, hit) = query.cast_ray_and_get_normal(&ray, max_distance, true)?;

        let point = ray.point_at(hit.time_of_impact);
        Some(RayHit {
            collider,
            distance: hit.time_of_impact,
            point: [point.x, point.y, point.z],
            normal: [hit.normal.x, hit.normal.y, hit.normal.z],
        })
    }

    /// Whether anything blocks the segment between two points.
    ///
    /// Line of sight, and the test an explosion needs before it deals damage
    /// through a wall.
    #[must_use]
    pub fn line_of_sight(&self, from: [f32; 3], to: [f32; 3]) -> bool {
        let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let distance = delta.iter().map(|c| c * c).sum::<f32>().sqrt();
        // Pulled back slightly so a target standing *on* the surface it is
        // checked against does not occlude itself.
        self.raycast(from, delta, distance - 1e-3).is_none()
    }

    /// Put a character capsule in the world.
    ///
    /// The body is **kinematic**, not dynamic. A dynamic capsule is pushed
    /// around by the solver: it slides down ramps, gets shoved by anything it
    /// touches, and accelerates and decelerates on its own terms. That is
    /// correct for a barrel and wrong for a character, where the movement
    /// model is the thing being authored and the solver must not have opinions
    /// about it. `add_capsule` above is the dynamic one, still there for
    /// ragdolls and anything that should be thrown.
    ///
    /// It carries a real collider even so, so everything else in the world can
    /// see it — a shot fired at the player has to hit something.
    pub fn add_character(&mut self, position: [f32; 3], shape: CharacterShape) -> Character {
        let body = RigidBodyBuilder::kinematic_position_based()
            .translation(Vector::new(position[0], position[1], position[2]))
            .build();
        let body = self.bodies.insert(body);
        let collider =
            ColliderBuilder::capsule_y(shape.half_height.max(1e-3), shape.radius.max(1e-3)).build();
        self.colliders
            .insert_with_parent(collider, body, &mut self.bodies);

        let mut controller = KinematicCharacterController {
            up: Vector::Y,
            slide: true,
            max_slope_climb_angle: shape.max_slope_degrees.to_radians(),
            // Slightly under the climb angle: a character that starts sliding
            // at exactly the angle it can still walk up jitters between the
            // two on any real floor.
            min_slope_slide_angle: (shape.max_slope_degrees + 5.0).to_radians(),
            ..KinematicCharacterController::default()
        };
        // Rapier leaves autostep off because it is expensive. It is not
        // optional here: without it a 15 cm lip stops a character dead, and
        // every staircase in a level becomes a wall.
        controller.autostep = (shape.step_height > 0.0).then_some(CharacterAutostep {
            max_height: CharacterLength::Absolute(shape.step_height),
            min_width: CharacterLength::Absolute(shape.radius * 0.5),
            include_dynamic_bodies: true,
        });

        Character {
            body,
            controller,
            shape,
            position,
            grounded: false,
        }
    }

    /// Move a character by `velocity` for one step, colliding and sliding.
    ///
    /// **The velocity is the caller's business and the collision is this
    /// function's.** Nothing here applies gravity, friction, acceleration or a
    /// speed limit — those are the movement model, and the movement model is
    /// authored (in a script, usually) rather than baked into the engine. What
    /// this owns is the part a script cannot get right: sweeping the capsule,
    /// sliding it along what it hits, stepping it up small ledges, and saying
    /// whether it ended up on the ground.
    ///
    /// The returned velocity is the one that **survived** the move, so a
    /// caller that keeps integrating its own velocity finds out it hit
    /// something. Walk into a wall and the component into the wall is gone;
    /// stand on the floor and the accumulated fall is gone. Handing back the
    /// requested velocity instead is the classic bug where three seconds of
    /// standing still builds up −40 m/s that silently eats the next jump.
    ///
    /// # The world it collides against is the one the last `step` left
    ///
    /// Same caveat as [`Self::raycast`], for the same reason: the broad-phase
    /// tree is built during [`Self::step`]. A character moved before anything
    /// has stepped collides with nothing and falls through the floor.
    pub fn move_character(
        &mut self,
        character: &mut Character,
        velocity: [f32; 3],
        dt: f32,
    ) -> CharacterMove {
        let requested = Vector::new(velocity[0], velocity[1], velocity[2]);
        if !requested.is_finite() || !dt.is_finite() || dt <= 0.0 {
            return CharacterMove {
                position: character.position,
                velocity: [0.0; 3],
                grounded: character.grounded,
            };
        }

        let pose = Pose::from_translation(Vector::new(
            character.position[0],
            character.position[1],
            character.position[2],
        ));
        let capsule = Capsule::new_y(
            character.shape.half_height.max(1e-3),
            character.shape.radius.max(1e-3),
        );

        let movement = {
            let query = self.broad_phase.as_query_pipeline(
                self.narrow_phase.query_dispatcher(),
                &self.bodies,
                &self.colliders,
                // Excluding itself. Rapier's sweeps currently ignore a shape
                // they start already penetrating, so removing this changes
                // nothing any test here can see — it is kept because that is
                // an implementation detail rather than a promise, and the
                // failure if it ever changes is a character that cannot move
                // or is permanently grounded on itself. Neither reports an
                // error; both look like broken input.
                QueryFilter::default().exclude_rigid_body(character.body),
            );
            character
                .controller
                .move_shape(dt, &query, &capsule, &pose, requested * dt, |_| {})
        };

        let moved = pose.translation + movement.translation;
        character.position = [moved.x, moved.y, moved.z];
        character.grounded = movement.grounded;
        // Keep the collider where the character is, so the rest of the world
        // sees it there. Kinematic, so rapier derives its velocity from the
        // move and pushes dynamic bodies out of the way properly.
        if let Some(body) = self.bodies.get_mut(character.body) {
            body.set_next_kinematic_translation(moved);
        }

        let mut survived = movement.translation / dt;
        // Ground snapping is part of `translation` and can be much larger than
        // the fall that earned it — stepping off a curb would report tens of
        // metres per second downward. Standing on something means no vertical
        // speed, whatever the sweep had to do to get there.
        if movement.grounded && survived.y <= 0.0 {
            survived.y = 0.0;
        }

        CharacterMove {
            position: character.position,
            velocity: [survived.x, survived.y, survived.z],
            grounded: movement.grounded,
        }
    }

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
    use super::*;

    /// A shot straight down at a floor must land on the floor, at the distance
    /// the geometry says — not somewhere near it.
    #[test]
    fn a_ray_hits_a_floor_at_the_right_distance() {
        let mut physics = Physics::new(1.0 / 60.0);
        // Top surface at y = 1.0.
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [10.0, 1.0, 10.0]);
        physics.step();

        let hit = physics
            .raycast([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 100.0)
            .expect("a floor directly below should be hit");

        assert!((hit.distance - 4.0).abs() < 1e-3, "distance was {}", hit.distance);
        assert!((hit.point[1] - 1.0).abs() < 1e-3, "point was {:?}", hit.point);
        // The normal faces back up the ray, which is what a decal or a
        // ricochet is oriented by.
        assert!(hit.normal[1] > 0.9, "normal was {:?}", hit.normal);
    }

    #[test]
    fn a_ray_pointing_away_hits_nothing() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [10.0, 1.0, 10.0]);
        physics.step();

        assert!(physics.raycast([0.0, 5.0, 0.0], [0.0, 1.0, 0.0], 100.0).is_none());
    }

    /// Range is what separates a rifle from a knife, so it has to actually
    /// bound the query rather than being applied to the result afterwards.
    #[test]
    fn a_ray_stops_at_its_range() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [10.0, 1.0, 10.0]);
        physics.step();

        assert!(physics.raycast([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 3.0).is_none());
        assert!(physics.raycast([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 4.5).is_some());
    }

    /// An unnormalised direction is the common case — a target position minus
    /// a muzzle position — and must not scale the reported distance.
    #[test]
    fn direction_length_does_not_change_the_distance() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [10.0, 1.0, 10.0]);
        physics.step();

        let unit = physics.raycast([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 100.0).unwrap();
        let long = physics.raycast([0.0, 5.0, 0.0], [0.0, -37.0, 0.0], 100.0).unwrap();

        assert!((unit.distance - long.distance).abs() < 1e-3);
    }

    /// A zero direction is degenerate. Normalising it gives NaN, and a NaN ray
    /// reports hits at meaningless places rather than failing.
    #[test]
    fn a_degenerate_ray_is_refused() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [10.0, 1.0, 10.0]);
        physics.step();

        assert!(physics.raycast([0.0, 5.0, 0.0], [0.0, 0.0, 0.0], 100.0).is_none());
        assert!(physics.raycast([0.0, 5.0, 0.0], [f32::NAN, -1.0, 0.0], 100.0).is_none());
    }

    #[test]
    fn line_of_sight_is_blocked_by_a_wall() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [0.5, 5.0, 5.0]);
        physics.step();

        assert!(!physics.line_of_sight([-4.0, 0.0, 0.0], [4.0, 0.0, 0.0]), "wall between");
        assert!(physics.line_of_sight([-4.0, 8.0, 0.0], [4.0, 8.0, 0.0]), "clear above it");
    }

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

    // -- character controller -------------------------------------------
    //
    // A room to walk around in: a floor with its top surface at y = 0, and a
    // wall standing on it. Every character test uses the same one so the
    // numbers below mean the same thing in all of them.
    fn room() -> Physics {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -1.0, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 1.0, 20.0]);
        // Wall at x = 4, facing back down -X.
        physics.add_static_box([4.0, 1.5, 0.0], [0.0, 0.0, 0.0, 1.0], [0.5, 1.5, 20.0]);
        physics.step();
        physics
    }

    /// Where a resting capsule's centre sits: half the cylinder plus a cap.
    fn rest_height(shape: CharacterShape) -> f32 {
        shape.half_height + shape.radius
    }

    /// Gravity for one tick, the way a script would apply it.
    fn fall(velocity: &mut [f32; 3], dt: f32) {
        velocity[1] -= 9.81 * dt;
    }

    #[test]
    fn a_character_falls_and_lands_on_the_floor() {
        let mut physics = room();
        let shape = CharacterShape::default();
        let mut character = physics.add_character([0.0, 3.0, 0.0], shape);
        let dt = 1.0 / 60.0;

        let mut velocity = [0.0; 3];
        let mut moved = physics.move_character(&mut character, velocity, dt);
        for _ in 0..120 {
            fall(&mut velocity, dt);
            moved = physics.move_character(&mut character, velocity, dt);
            velocity = moved.velocity;
        }

        assert!(moved.grounded, "it should be standing on the floor");
        assert!(
            (moved.position[1] - rest_height(shape)).abs() < 0.05,
            "rested at {:?}, expected about {}",
            moved.position,
            rest_height(shape)
        );
    }

    /// The bug this exists for: standing still, gravity accumulates every tick
    /// into a downward velocity that the floor silently absorbs. Jump after a
    /// few seconds and the built-up −40 m/s eats the jump. The controller has
    /// to report the velocity that *survived* the move, not the one asked for.
    #[test]
    fn standing_still_does_not_accumulate_downward_velocity() {
        let mut physics = room();
        let mut character = physics.add_character([0.0, 1.2, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        let mut velocity = [0.0; 3];
        for _ in 0..180 {
            fall(&mut velocity, dt);
            velocity = physics.move_character(&mut character, velocity, dt).velocity;
        }

        assert!(
            velocity[1].abs() < 0.2,
            "three seconds of standing built up {} m/s",
            velocity[1]
        );
    }

    #[test]
    fn a_character_walks_into_a_wall_and_stops_at_it() {
        let mut physics = room();
        let shape = CharacterShape::default();
        let mut character = physics.add_character([0.0, 1.2, 0.0], shape);
        let dt = 1.0 / 60.0;

        let mut moved = physics.move_character(&mut character, [0.0; 3], dt);
        for _ in 0..240 {
            let mut velocity = [6.0, moved.velocity[1] - 9.81 * dt, 0.0];
            if moved.grounded && velocity[1] < 0.0 {
                velocity[1] = 0.0;
            }
            moved = physics.move_character(&mut character, velocity, dt);
        }

        // Wall's near face is at x = 3.5. Stopped before it, not through it.
        assert!(
            moved.position[0] < 3.5 && moved.position[0] > 3.5 - shape.radius - 0.2,
            "ended at x = {}, wall face is at 3.5",
            moved.position[0]
        );
    }

    /// Walking into a wall at an angle must keep the along-wall component.
    /// Without sliding a character sticks to every wall it brushes, which is
    /// the single most obvious way a controller feels broken.
    #[test]
    fn a_character_slides_along_a_wall_it_hits_at_an_angle() {
        let mut physics = room();
        let mut character = physics.add_character([0.0, 1.2, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        let mut moved = physics.move_character(&mut character, [0.0; 3], dt);
        for _ in 0..240 {
            moved = physics.move_character(&mut character, [6.0, -1.0, 3.0], dt);
        }

        assert!(moved.position[0] < 3.5, "should not be inside the wall");
        assert!(
            moved.position[2] > 4.0,
            "pressed against the wall it should still travel along it; z = {}",
            moved.position[2]
        );
    }

    #[test]
    fn a_grounded_character_can_jump_off_the_floor() {
        let mut physics = room();
        let shape = CharacterShape::default();
        let mut character = physics.add_character([0.0, 1.2, 0.0], shape);
        let dt = 1.0 / 60.0;

        // Settle.
        let mut moved = physics.move_character(&mut character, [0.0, -1.0, 0.0], dt);
        for _ in 0..60 {
            moved = physics.move_character(&mut character, [0.0, -1.0, 0.0], dt);
        }
        assert!(moved.grounded, "must be grounded before jumping");
        let floor_height = moved.position[1];

        // One jump, then coast under gravity.
        let mut velocity = [0.0, 5.0, 0.0];
        let mut peak = floor_height;
        for _ in 0..40 {
            let moved = physics.move_character(&mut character, velocity, dt);
            velocity = moved.velocity;
            fall(&mut velocity, dt);
            peak = peak.max(moved.position[1]);
        }

        assert!(
            peak > floor_height + 0.8,
            "jumped to {peak}, floor is {floor_height}"
        );
    }

    /// A curb. Without autostep a character stops dead at a 15 cm lip, which
    /// makes every staircase in a level impassable.
    #[test]
    fn a_character_steps_up_a_low_obstacle() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -1.0, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 1.0, 20.0]);
        // A step whose top is 0.2 m above the floor.
        physics.add_static_box([3.0, 0.1, 0.0], [0.0, 0.0, 0.0, 1.0], [2.0, 0.1, 20.0]);
        physics.step();

        let mut character = physics.add_character([0.0, 1.2, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        let mut moved = physics.move_character(&mut character, [0.0; 3], dt);
        for _ in 0..180 {
            moved = physics.move_character(&mut character, [3.0, -2.0, 0.0], dt);
        }

        assert!(
            moved.position[0] > 2.0,
            "stopped at the lip: x = {}",
            moved.position[0]
        );
        assert!(
            moved.position[1] > 0.2,
            "did not climb onto the step: y = {}",
            moved.position[1]
        );
    }

    /// The character must exist to everything else, not only to itself — a
    /// shot fired at the player has to hit something.
    #[test]
    fn a_character_is_visible_to_a_raycast() {
        let mut physics = room();
        let mut character = physics.add_character([0.0, 1.2, 0.0], CharacterShape::default());
        physics.move_character(&mut character, [0.0; 3], 1.0 / 60.0);
        physics.step();

        let hit = physics
            .raycast([-6.0, 1.2, 0.0], [1.0, 0.0, 0.0], 20.0)
            .expect("the character stands between the ray and the wall");

        assert!(
            hit.distance < 6.0,
            "hit at {} — that is the wall, not the character",
            hit.distance
        );
    }

    /// Nothing under it, so nothing to stand on. A character that counts its
    /// own capsule as ground is grounded in mid-air — which means infinite
    /// jumps and no falling, from a bug with no visible cause.
    #[test]
    fn a_character_in_empty_space_is_never_grounded() {
        let mut physics = Physics::new(1.0 / 60.0);
        // A floor far below, so the world is not empty and the broad phase
        // has something in it — the character is simply nowhere near it.
        physics.add_static_box([0.0, -200.0, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 1.0, 20.0]);
        physics.step();

        let mut character = physics.add_character([0.0, 20.0, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;
        let mut velocity = [0.0; 3];

        for _ in 0..60 {
            fall(&mut velocity, dt);
            let moved = physics.move_character(&mut character, velocity, dt);
            velocity = moved.velocity;
            assert!(!moved.grounded, "grounded at y = {}", moved.position[1]);
            physics.step();
        }

        assert!(velocity[1] < -8.0, "should be falling freely: {velocity:?}");
    }

    /// Jump under a low ceiling: the head stops, and so must the velocity.
    /// Reporting the *requested* velocity instead pins the character against
    /// the ceiling for as long as the jump would have lasted.
    #[test]
    fn hitting_a_ceiling_kills_the_upward_velocity() {
        let mut physics = room();
        // Ceiling with its underside at y = 2.6.
        physics.add_static_box([0.0, 3.1, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 0.5, 20.0]);
        physics.step();

        let mut character = physics.add_character([0.0, 0.95, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        let mut velocity = [0.0, 9.0, 0.0];
        let mut hit_ceiling = false;
        for _ in 0..20 {
            let moved = physics.move_character(&mut character, velocity, dt);
            velocity = moved.velocity;
            if velocity[1] < 1.0 {
                hit_ceiling = true;
                break;
            }
        }

        assert!(hit_ceiling, "never stopped going up: {velocity:?}");
    }

    /// Walking off a low step, the sweep snaps the capsule down to the floor
    /// in one tick. That snap is a much bigger drop than the fall that earned
    /// it, and reported as velocity it reads as tens of metres per second —
    /// a script integrating it would think it was in freefall while walking.
    #[test]
    fn a_ground_snap_is_not_reported_as_a_fall() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -1.0, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 1.0, 20.0]);
        // A platform 0.3 m up, ending at x = 1.0. Walking off it drops the
        // character back to the floor.
        physics.add_static_box([0.0, 0.15, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 0.15, 20.0]);
        physics.step();

        let mut character = physics.add_character([0.0, 1.3, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        // Settle on the platform, then walk off the edge.
        let mut moved = physics.move_character(&mut character, [0.0, -1.0, 0.0], dt);
        for _ in 0..30 {
            moved = physics.move_character(&mut character, [0.0, -1.0, 0.0], dt);
        }
        assert!(moved.grounded, "should start on the platform");

        let mut worst: f32 = 0.0;
        for _ in 0..90 {
            moved = physics.move_character(&mut character, [2.0, -1.0, 0.0], dt);
            if moved.grounded {
                worst = worst.max(-moved.velocity[1]);
            }
        }

        assert!(
            worst < 2.0,
            "reported {worst} m/s downward while walking on the ground"
        );
    }

    /// Its own capsule must not block it. A controller that collides with
    /// itself cannot move at all, and the symptom looks like broken input.
    #[test]
    fn a_character_does_not_collide_with_itself() {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -1.0, 0.0], [0.0, 0.0, 0.0, 1.0], [20.0, 1.0, 20.0]);
        physics.step();
        let mut character = physics.add_character([0.0, 1.2, 0.0], CharacterShape::default());
        let dt = 1.0 / 60.0;

        let mut moved = physics.move_character(&mut character, [0.0; 3], dt);
        for _ in 0..60 {
            moved = physics.move_character(&mut character, [4.0, -1.0, 0.0], dt);
            physics.step();
        }

        assert!(moved.position[0] > 3.0, "went nowhere: x = {}", moved.position[0]);
    }
}
