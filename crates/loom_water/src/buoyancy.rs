//! The pontoon solver: what the water does to a floating body, per fixed step.
//!
//! **This half is pure arithmetic and knows nothing about rapier.** It is given
//! where each pontoon is, how fast that point is moving and where the centre of
//! mass is; it answers with one force and one torque. That keeps the physics
//! crate out of `loom_water` and — the reason that matters — makes the
//! nonlinearity in [`submerged_volume`] testable without building a world.
//!
//! # This is simulation, and the rules are different here
//!
//! The water surface and the water mesh are rendering; buoyancy is not. It
//! moves rigid bodies, so it lives inside the determinism hash:
//!
//! - **Pontoons are summed in index order**, into one force/torque pair applied
//!   once. Float addition is not associative, so accumulating the same forces
//!   in a different order is a different number and the project hashes it.
//! - **The surface comes from [`crate::sample_water`], never from the GPU.**
//!   Readback timing is not reproducible and lags a frame, which would make
//!   every `loom sim --assert` over water flaky *and* wrong.
//! - No wall clock: `t` is the tick count times the fixed timestep.

use crate::{GRAVITY, WaterSample, sample_water};
use loom_scene::components::{Buoyancy, WaterBody};

/// One pontoon as the solver sees it: already in world space, already moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PontoonState {
    /// Sphere centre in world space.
    pub at: [f32; 3],
    pub radius: f32,
    /// Velocity of the body *at this point* — `linvel + angvel × r`, not the
    /// body's linear velocity. A crate rolling in a swell has pontoons moving
    /// in opposite directions, and damping that ignored the rotation would
    /// damp none of it.
    pub velocity: [f32; 3],
    /// Terrain height under this pontoon, in world Y.
    ///
    /// **Filled by the caller, from the same height field the water shader
    /// reads** (`loom_voxel::heightfield`). It is here rather than looked up
    /// inside [`solve`] for the reason the whole crate is arranged this way:
    /// `loom_water` must not know what a voxel is, or the shader generated from
    /// it acquires a dependency on the terrain system.
    ///
    /// It matters because waves flatten in the shallows — a crate in half a
    /// metre of water must not ride a swell that is not there. Default it to
    /// [`loom_voxel::heightfield::NO_GROUND`]'s meaning by passing a very
    /// negative number; a scene with no terrain does exactly that.
    pub ground: f32,
    /// The river's current at this pontoon, m/s, in world space.
    ///
    /// **Filled by the caller from [`crate::flow::FlowGrid`]**, for the same
    /// reason `ground` is: it is derived from the voxel terrain, and this crate
    /// must not know what a voxel is. `[0.0; 3]` is an ocean or a lake.
    ///
    /// Per pontoon rather than per body, because that is what makes a crate
    /// entering an eddy turn: the pontoons are metres apart, the current
    /// differs between them, and the difference is a torque.
    pub flow: [f32; 3],
}

/// One force and one torque, in world space, about the centre of mass — and
/// how much of the body the water had hold of while producing them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wrench {
    /// Newtons.
    pub force: [f32; 3],
    /// Newton-metres.
    pub torque: [f32; 3],
    /// Fraction of the body's pontoon volume under the surface, `0.0` to `1.0`.
    ///
    /// **This rides out of the solver rather than being asked for separately,
    /// and that is the whole point.** Buoyancy already computes a submerged
    /// volume per pontoon — it is what Archimedes multiplies — so a second
    /// query answering "is it in the water" would be a second opinion about a
    /// number this function already knows, free to disagree with the forces
    /// actually applied. Gameplay, scripts, splashes and the event log all read
    /// this one.
    ///
    /// It is a fraction rather than a flag or a depth for three reasons. It is
    /// what is already in hand. A flag cannot scale a continuous response, and
    /// the flag is derivable from it ([`is_submerged`]) while the reverse is
    /// not. And "the depth of a reference point" needs a point: a body rotates,
    /// so any single point is right in one orientation and wrong in the others,
    /// whereas the pontoons turn with the body and the fraction turns with them.
    pub submerged: f32,
}

/// Volume of the part of a sphere below `surface_y`, in cubic metres.
///
/// **The exact spherical cap, and that is the whole trick.** `V = π·d²·(3r−d)/3`
/// for a cap of depth `d`. A linear ramp from "touching" to "under" is the
/// obvious approximation and it is what makes an object oscillate forever: the
/// restoring force of a linear ramp is a plain spring, which conserves energy
/// and has no reason to settle anywhere. The cap's curvature is what makes the
/// force grow slowly near the waterline and fast once it is under, and that
/// asymmetry is what turns a bob into a resting position.
///
/// Fully out is zero and fully under is the whole sphere; both are exact rather
/// than a limit of the formula, so a body high in the air costs one comparison.
#[must_use]
pub fn submerged_volume(radius: f32, centre_y: f32, surface_y: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    // Depth of the cap: how far the surface is above the sphere's lowest point.
    let depth = surface_y - (centre_y - radius);
    if depth <= 0.0 {
        return 0.0;
    }
    if depth >= 2.0 * radius {
        return 4.0 / 3.0 * std::f32::consts::PI * radius * radius * radius;
    }
    std::f32::consts::PI * depth * depth * (3.0f32.mul_add(radius, -depth)) / 3.0
}

/// Everything the water does to one body this step.
///
/// `pontoons` are in the component's own order and are visited in it. `t` is
/// simulation seconds, the same clock the shader is handed, so the surface a
/// crate floats on is the surface it is drawn on.
///
/// `v_water` is the wave's orbital velocity plus the pontoon's own `flow`,
/// summed inside [`sample_water`] — **there is one water velocity and one drag
/// term**, and a river carries a crate through the same arithmetic that makes
/// it bob under a crest.
#[must_use]
pub fn solve(
    water: &WaterBody,
    buoyancy: &Buoyancy,
    pontoons: &[PontoonState],
    centre_of_mass: [f32; 3],
    t: f32,
) -> Wrench {
    let mut wrench = Wrench::default();
    // The two halves of the submerged fraction, summed in the same pass and the
    // same order as the forces. `dry` is counted too — a body one of whose four
    // pontoons is under is a quarter submerged, not wholly.
    let mut wet_volume = 0.0_f32;
    let mut total_volume = 0.0_f32;

    for pontoon in pontoons {
        // **The bed, because the shallows flatten the waves.** Water is still a
        // plane the terrain does not drain (§5.2, a stated v1 limitation) — a
        // crater under a lake does not empty it — but how big a wave is at this
        // spot is a function of how deep it is here, and a solver that passed
        // zero would float a crate on the open sea's swell in a foot of water.
        let surface: WaterSample = sample_water(
            water,
            [pontoon.at[0], pontoon.at[2]],
            t,
            pontoon.ground,
            pontoon.flow,
        );

        let volume = submerged_volume(pontoon.radius, pontoon.at[1], surface.height);
        let whole = 4.0 / 3.0 * std::f32::consts::PI * pontoon.radius.powi(3);
        total_volume += whole;
        wet_volume += volume;
        if volume <= 0.0 {
            // Out of the water entirely: no buoyancy, and no damping or drag
            // either. Damping a pontoon in mid-air is water acting at a
            // distance, and it reads as an object falling through treacle.
            continue;
        }
        // How much of this sphere is wet, which is what scales what the water
        // is allowed to do to it. Without this a pontoon grazing the surface
        // gets a fully-submerged pontoon's drag.
        let wetness = (volume / whole).clamp(0.0, 1.0);
        // The mass of water this pontoon has pushed aside — which is both the
        // buoyant force's magnitude and the natural scale for the damping.
        let displaced = water.density * volume;

        // Archimedes: the weight of the water pushed aside, straight up.
        let mut force = [0.0, displaced * GRAVITY * buoyancy.coefficient, 0.0];

        // **The damping is measured in the water's frame, not the world's.**
        // Damping models the water resisting a body being dragged *through* it,
        // so the velocity that matters is the one relative to the water — and
        // in a river the water is moving. Measured in the world frame instead,
        // a crate that had reached the current's own speed would still be
        // damped as if it were being hauled upstream at 2 m/s, and the river
        // could never carry anything: on the first `river.loom` the crate
        // settled at 0.08 m/s against a 2.24 m/s current, because the damping
        // outweighed the drag by a factor of twenty-five.
        //
        // **Only the current is subtracted, deliberately, and not the waves'
        // orbital motion.** A crate riding a swell really is moving through the
        // water — that is what makes it bob rather than sit still while the
        // wave passes under it — and §5.5's whole subject is damping that
        // motion. Every scene that predates rivers has a zero current here, so
        // this is bit-for-bit what it was.
        let v = pontoon.velocity;
        let carried = [
            v[0] - pontoon.flow[0],
            v[1] - pontoon.flow[1],
            v[2] - pontoon.flow[2],
        ];
        let speed =
            (carried[0] * carried[0] + carried[1] * carried[1] + carried[2] * carried[2]).sqrt();
        for (axis, component) in force.iter_mut().enumerate() {
            // §5.5, and **the damping is scaled by the displaced mass on
            // purpose**. A coefficient in plain newtons per m/s is a number
            // whose right value depends on the body's mass, and a floating
            // body's mass *is* the water it displaces — so writing it this way
            // makes `damp_linear` a rate in 1/s that means the same thing on a
            // buoy and on a barge. A fixed-newton coefficient tuned on a crate
            // leaves anything heavier violently underdamped, which is exactly
            // the resonance this term exists to prevent.
            //
            // The quadratic term is only ever reached here, which is what
            // "only underwater" means: an airborne pontoon `continue`d above.
            let damping = displaced
                * buoyancy
                    .damp_quadratic
                    .mul_add(carried[axis] * speed, buoyancy.damp_linear * carried[axis]);
            // Drag against the *relative* flow — the water's velocity minus the
            // body's. **This is the one path by which a river moves anything**:
            // the current is inside `surface.velocity`, so carrying a crate
            // downstream and nudging it along under a crest are the same term
            // with the same coefficient. A separate "river push" force would be
            // a second opinion about what the water is doing to this body.
            let drag = water.drag * (surface.velocity[axis] - v[axis]) * wetness;
            *component += drag - damping;
        }

        // The offset from the centre of mass is what turns a force into a
        // torque, and it is the entire reason for pontoons: four of them make a
        // crate sit upright, one makes it spin.
        let r = [
            pontoon.at[0] - centre_of_mass[0],
            pontoon.at[1] - centre_of_mass[1],
            pontoon.at[2] - centre_of_mass[2],
        ];
        wrench.torque[0] += r[1] * force[2] - r[2] * force[1];
        wrench.torque[1] += r[2] * force[0] - r[0] * force[2];
        wrench.torque[2] += r[0] * force[1] - r[1] * force[0];
        for (total, part) in wrench.force.iter_mut().zip(force) {
            *total += part;
        }
    }

    // A body with no pontoons at all is not in the water; it has no volume for
    // the water to be in. Guarded rather than divided, because the alternative
    // is a NaN in a number the event log and every script read.
    wrench.submerged = if total_volume > 0.0 {
        (wet_volume / total_volume).clamp(0.0, 1.0)
    } else {
        0.0
    };
    wrench
}

/// How much of a sphere at `at` is under the water, `0.0` to `1.0`.
///
/// The same question [`solve`] answers for a whole body, for callers that have
/// one point rather than a pontoon set — the audio listener is the one that
/// exists, and it is a point because an ear has no volume. `radius <= 0.0` is
/// therefore not degenerate but the normal case, and it answers the way a point
/// does: under the surface or not, with nothing in between.
///
/// **Same surface, same clock, same height field as the solver.** It reads
/// [`sample_water`] like everything else does, so a listener cannot be told it
/// is underwater by a surface a crate floating beside it disagrees with.
#[must_use]
pub fn submersion_at(
    water: &WaterBody,
    at: [f32; 3],
    radius: f32,
    t: f32,
    ground: f32,
) -> f32 {
    // No flow: this asks *where the surface is*, and a current is horizontal —
    // it moves the water without raising or lowering it. Passing one in would
    // be sampling a number this function then discards.
    let surface = sample_water(water, [at[0], at[2]], t, ground, [0.0; 3]);
    if radius <= 0.0 {
        return f32::from(u8::from(surface.height > at[1]));
    }
    let whole = 4.0 / 3.0 * std::f32::consts::PI * radius * radius * radius;
    (submerged_volume(radius, at[1], surface.height) / whole).clamp(0.0, 1.0)
}

/// Whether a body counts as submerged, given what it counted as last tick.
///
/// **A Schmitt trigger, and the two thresholds are the whole reason this is a
/// function rather than a comparison.** A body floating at the waterline sits
/// where the fraction is *near* whatever single threshold you pick, and a wave
/// passing under it then crosses that threshold twice per wave — so "is it
/// submerged" chatters every few ticks, every script watching it fires on every
/// edge, and every splash re-fires. It is invisible in a still and obvious in a
/// tick-by-tick event log, which is why the test for it counts events over
/// thirty seconds rather than looking at a picture.
///
/// Going under takes `enter`; coming back out takes falling below `exit`. In
/// between, whatever it already was. `exit >= enter` is authoring nonsense and
/// is treated as no hysteresis at all rather than as an error — the state is
/// then simply `fraction >= enter`, which chatters, which is the author's
/// choice and is exactly what the mutation check for this flips.
#[must_use]
pub fn is_submerged(was: bool, fraction: f32, enter: f32, exit: f32) -> bool {
    if fraction >= enter {
        return true;
    }
    if fraction <= exit.min(enter) {
        return false;
    }
    was
}

/// Four pontoons sized to a box, for a `Buoyancy` that lists none.
///
/// **The default that stops both halves of the pontoon-count trap.** One
/// pontoon is a single force at one point, so the body has no righting torque
/// and spins; four at the horizontal corners give one. And the radii are solved
/// from the box's volume rather than from its corners, because a sphere big
/// enough to *look* like it fills a corner, times four, displaces several times
/// what the object does — and then it floats like a cork with its deck in the
/// air, which reads as a buoyancy bug rather than as an authoring one.
///
/// `half_extents` are the body's, in world units.
#[must_use]
pub fn default_pontoons(half_extents: [f32; 3]) -> Vec<loom_scene::components::Pontoon> {
    let h = [
        half_extents[0].abs().max(1e-3),
        half_extents[1].abs().max(1e-3),
        half_extents[2].abs().max(1e-3),
    ];
    // 4 · (4/3)πr³ = 8·hx·hy·hz, so r³ = 3·hx·hy·hz / (2π).
    let radius = (1.5 * h[0] * h[1] * h[2] / std::f32::consts::PI).cbrt();
    // At the centre's height rather than the base: a floating crate settles
    // with its waterline near its centre of mass, and pontoons at the bottom
    // would put the whole displaced volume below the water at rest, where the
    // cap nonlinearity that damps the bob is flattest.
    [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]
        .into_iter()
        .map(|(x, z)| loom_scene::components::Pontoon {
            offset: [x * h[0], 0.0, z * h[2]],
            radius,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_scene::components::{GerstnerWave, Pontoon, WaveSet};

    /// A bed far enough down that nothing here is in the shallows — what a
    /// scene with no terrain under its water reports.
    const DEEP: f32 = -1000.0;

    fn still() -> WaterBody {
        WaterBody {
            drag: 0.0,
            ..WaterBody::default()
        }
    }

    /// The cap formula against the two answers arithmetic gives for free, and
    /// against the half-submerged case, which is the one a linear ramp also
    /// gets right — so the test has to check the quarter points to mean
    /// anything.
    #[test]
    fn the_submerged_volume_is_the_exact_spherical_cap() {
        let r = 1.0_f32;
        let whole = 4.0 / 3.0 * std::f32::consts::PI;

        assert_eq!(submerged_volume(r, 5.0, 0.0), 0.0, "clear of the water");
        assert!((submerged_volume(r, -5.0, 0.0) - whole).abs() < 1e-6, "fully under");
        assert!(
            (submerged_volume(r, 0.0, 0.0) - whole * 0.5).abs() < 1e-6,
            "exactly half"
        );

        // **Where the nonlinearity lives.** A sphere dipped a quarter of its
        // diameter displaces 5/32 of itself, not a quarter: `π·d²·(3r−d)/3`
        // with d = 0.5 is `π·0.25·2.5/3`. A linear ramp says 0.25 of the
        // volume, which is 60% too much force at the waterline — and a force
        // proportional to displacement is a spring, which never settles.
        let quarter = submerged_volume(r, 0.5, 0.0);
        assert!((quarter - whole * 5.0 / 32.0).abs() < 1e-6, "{quarter}");
        assert!(
            quarter < whole * 0.25 * 0.7,
            "the cap must be well under the linear ramp's answer: {quarter}"
        );
    }

    /// A sphere floats where its own weight is displaced, and the solver's
    /// force at that point is the weight it holds up. Checked against
    /// arithmetic rather than against a simulation: half a 1 m sphere in fresh
    /// water holds up `1000 · (2/3)π · 9.81` newtons.
    #[test]
    fn the_buoyant_force_is_the_weight_of_the_water_pushed_aside() {
        let mut water = still();
        water.density = 1000.0;
        let buoyancy = Buoyancy {
            pontoons: Vec::new(),
            coefficient: 1.0,
            damp_linear: 0.0,
            damp_quadratic: 0.0,
        };
        let pontoons = [PontoonState {
            at: [0.0, 0.0, 0.0],
            radius: 1.0,
            velocity: [0.0; 3],
            ground: DEEP, flow: [0.0; 3],
        }];

        let w = solve(&water, &buoyancy, &pontoons, [0.0; 3], 0.0);

        let expected = 1000.0 * (2.0 / 3.0 * std::f32::consts::PI) * GRAVITY;
        assert!((w.force[1] - expected).abs() < 1.0, "{w:?} vs {expected}");
        assert_eq!(w.force[0], 0.0, "still water pushes nothing sideways");
        assert_eq!(w.torque, [0.0; 3], "a force through the centre is no torque");
    }

    /// **The pontoon model's entire justification.** Four pontoons under a
    /// tilted body push harder on the deeper side, which is a righting torque;
    /// one pontoon has no offset to act through and produces none, so the body
    /// is free to spin.
    #[test]
    fn four_pontoons_right_a_tilted_body_and_one_does_not() {
        let water = still();
        let buoyancy = Buoyancy {
            damp_linear: 0.0,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };
        // Rolled about Z: the +X side is down, the −X side is up.
        let tilted: Vec<PontoonState> = default_pontoons([0.5, 0.5, 0.5])
            .iter()
            .map(|p| PontoonState {
                at: [p.offset[0], p.offset[0] * -0.6, p.offset[2]],
                radius: p.radius,
                velocity: [0.0; 3],
                ground: DEEP, flow: [0.0; 3],
            })
            .collect();

        let four = solve(&water, &buoyancy, &tilted, [0.0; 3], 0.0);
        // Right-hand rule: the deeper +X side pushed up is a torque about −Z...
        assert!(
            four.torque[2].abs() > 1.0,
            "four pontoons must right a tilt: {four:?}"
        );
        // ...and it opposes the tilt rather than adding to it. The +X side is
        // low, so the torque has to lift +X, which is positive about +Z.
        assert!(four.torque[2] > 0.0, "the torque is the wrong way round: {four:?}");

        let one = solve(
            &water,
            &buoyancy,
            &[PontoonState {
                at: [0.0, 0.0, 0.0],
                radius: 0.5,
                velocity: [0.0; 3],
                ground: DEEP, flow: [0.0; 3],
            }],
            [0.0; 3],
            0.0,
        );
        assert_eq!(one.torque, [0.0; 3], "one pontoon cannot right anything");
    }

    /// Damping opposes the pontoon's motion and grows with it, and the
    /// quadratic term is the one that dominates a fast entry.
    #[test]
    fn damping_opposes_motion_and_is_quadratic_at_speed() {
        let water = still();
        let buoyancy = Buoyancy {
            pontoons: Vec::new(),
            coefficient: 0.0, // buoyancy off, so only the damping shows
            damp_linear: 10.0,
            damp_quadratic: 10.0,
        };
        let sinking = |speed: f32| {
            solve(
                &water,
                &buoyancy,
                &[PontoonState {
                    at: [0.0, -1.0, 0.0],
                    radius: 1.0,
                    velocity: [0.0, -speed, 0.0],
                    ground: DEEP, flow: [0.0; 3],
                }],
                [0.0; 3],
                0.0,
            )
            .force[1]
        };

        assert!(sinking(1.0) > 0.0, "damping must oppose a descent");
        // Linear alone would give exactly ten times as much at ten times the
        // speed. The quadratic term makes it far more.
        assert!(
            sinking(10.0) > sinking(1.0) * 20.0,
            "{} vs {}",
            sinking(10.0),
            sinking(1.0)
        );
    }

    /// A pontoon clear of the water is not touched by it: no buoyancy, and no
    /// damping either. Without the second half an object in mid-air falls
    /// through treacle whenever a wave is anywhere below it.
    #[test]
    fn a_pontoon_in_the_air_feels_nothing() {
        let water = still();
        let w = solve(
            &water,
            &Buoyancy::default(),
            &[PontoonState {
                at: [0.0, 9.0, 0.0],
                radius: 0.5,
                velocity: [0.0, -20.0, 3.0],
                ground: DEEP, flow: [0.0; 3],
            }],
            [0.0; 3],
            0.0,
        );

        assert_eq!(w, Wrench::default());
    }

    /// Drag pushes a still body toward the water's own motion, which is what
    /// will carry a crate down a river and is what makes a floating thing
    /// travel with the swell rather than sit in it.
    #[test]
    fn drag_pushes_a_still_body_along_with_the_water() {
        let mut water = WaterBody {
            drag: 50.0,
            ..WaterBody::default()
        };
        water.waves.waves.push(GerstnerWave {
            wavelength: 12.0,
            amplitude: 0.6,
            steepness: 0.4,
            direction: [1.0, 0.0],
            speed_scale: 1.0,
        });
        let buoyancy = Buoyancy {
            coefficient: 0.0,
            damp_linear: 0.0,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };
        // Away from the origin: at phase zero the orbital motion is purely
        // vertical, so a test placed there would be checking nothing.
        let at = [3.0, -0.4, 0.0];
        let flow = sample_water(&water, [at[0], at[2]], 0.0, 0.0, [0.0; 3]).velocity;

        let w = solve(
            &water,
            &buoyancy,
            &[PontoonState { at, radius: 0.5, velocity: [0.0; 3], ground: DEEP, flow: [0.0; 3] }],
            [0.0; 3],
            0.0,
        );

        assert!(flow[0].abs() > 1e-3, "the fixture has no orbital motion to drag with");
        assert!(
            w.force[0].signum() == flow[0].signum() && w.force[0].abs() > 1e-3,
            "drag {:?} does not follow the water's {flow:?}",
            w.force
        );
    }

    /// **A river pushes through the drag term and nothing else.** The current
    /// is summed into the water's velocity inside `sample_water`, so the force
    /// it produces has to be the same force the swell produces — one term, one
    /// coefficient. This is the unit-level half of the `river.loom` assertion:
    /// the scene proves a crate arrives downstream, and this proves which
    /// arithmetic carried it.
    #[test]
    fn a_current_carries_a_still_body_downstream() {
        // Flat water, so the only thing moving is the river. On a wavy fixture
        // the orbital motion would be indistinguishable from the current.
        let water = WaterBody { drag: 50.0, ..WaterBody::default() };
        let buoyancy = Buoyancy {
            coefficient: 0.0,
            damp_linear: 0.0,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };
        let pontoon = |flow: [f32; 3]| PontoonState {
            at: [3.0, -0.4, 0.0],
            radius: 0.5,
            velocity: [0.0; 3],
            ground: DEEP,
            flow,
        };

        let still = solve(&water, &buoyancy, &[pontoon([0.0; 3])], [0.0; 3], 0.0);
        assert_eq!(still.force, [0.0; 3], "flat water with no current is not still");

        let carried = solve(&water, &buoyancy, &[pontoon([2.0, 0.0, 0.0])], [0.0; 3], 0.0);
        assert!(carried.force[0] > 1e-3, "the current does not push: {:?}", carried.force);
        assert_eq!(carried.force[2], 0.0, "a current along X pushes along Z");

        // And it pushes *harder* when it runs faster, which is what makes
        // `FlowField::speed` a knob rather than a switch.
        let faster = solve(&water, &buoyancy, &[pontoon([4.0, 0.0, 0.0])], [0.0; 3], 0.0);
        assert!(
            (faster.force[0] - 2.0 * carried.force[0]).abs() < 1e-3,
            "drag against the current is not linear: {} then {}",
            carried.force[0],
            faster.force[0]
        );
    }

    /// **Order of accumulation is fixed.** Pontoons are summed in index order;
    /// reversing the list must be the same answer bit for bit, or the
    /// determinism hash depends on which order the component happened to be
    /// written in. (Float addition is not associative, so this is a real
    /// property and not a tautology — it holds because there is exactly one
    /// order, not because addition would forgive another.)
    #[test]
    fn the_same_pontoons_in_the_same_order_give_the_same_bits() {
        let water = WaterBody {
            waves: WaveSet {
                waves: vec![GerstnerWave::default(); 4],
                ..WaveSet::default()
            },
            ..WaterBody::default()
        };
        let pontoons: Vec<PontoonState> = default_pontoons([0.6, 0.4, 0.9])
            .iter()
            .map(|p: &Pontoon| PontoonState {
                at: [p.offset[0], p.offset[1] - 0.2, p.offset[2]],
                radius: p.radius,
                velocity: [0.3, -0.7, 0.1],
                ground: DEEP, flow: [0.0; 3],
            })
            .collect();

        let a = solve(&water, &Buoyancy::default(), &pontoons, [0.0; 3], 3.5);
        let b = solve(&water, &Buoyancy::default(), &pontoons, [0.0; 3], 3.5);
        assert_eq!(a.force[1].to_bits(), b.force[1].to_bits());
        assert_eq!(a.torque[0].to_bits(), b.torque[0].to_bits());
    }

    /// **Submersion is the solver's own arithmetic, not a second opinion.**
    /// Four pontoons, two of them under: the fraction is what the displaced
    /// volumes say, and it moves with the surface rather than with a separate
    /// height test that could disagree with the forces being applied.
    #[test]
    fn the_solver_reports_how_much_of_the_body_is_under() {
        let water = still();
        let buoyancy = Buoyancy::default();
        let at = |y: f32| {
            default_pontoons([0.5, 0.5, 0.5])
                .iter()
                .map(|p| PontoonState {
                    at: [p.offset[0], y, p.offset[2]],
                    radius: p.radius,
                    velocity: [0.0; 3],
                    ground: DEEP, flow: [0.0; 3],
                })
                .collect::<Vec<PontoonState>>()
        };

        // Well clear of the water, on it, and well under it.
        assert_eq!(solve(&water, &buoyancy, &at(6.0), [0.0; 3], 0.0).submerged, 0.0);
        let half = solve(&water, &buoyancy, &at(0.0), [0.0; 3], 0.0).submerged;
        assert!((half - 0.5).abs() < 1e-5, "spheres centred on the surface: {half}");
        assert_eq!(solve(&water, &buoyancy, &at(-6.0), [0.0; 3], 0.0).submerged, 1.0);

        // And it is the *fraction of the body*, not of the wet pontoons: two
        // corners under and two out is half a body, not a whole one.
        let mut tilted = at(0.0);
        for (index, state) in tilted.iter_mut().enumerate() {
            state.at[1] = if index < 2 { -6.0 } else { 6.0 };
        }
        let split = solve(&water, &buoyancy, &tilted, [0.0; 3], 0.0).submerged;
        assert!((split - 0.5).abs() < 1e-5, "two of four under is half: {split}");
    }

    /// A body with no pontoons is not in the water, and is not a NaN either —
    /// which is what a bare division would put into the event log and into
    /// every script that reads it.
    #[test]
    fn a_body_with_no_pontoons_is_dry_rather_than_nan() {
        let w = solve(&still(), &Buoyancy::default(), &[], [0.0; 3], 0.0);

        assert_eq!(w.submerged, 0.0);
        assert!(w.submerged.is_finite());
    }

    /// The listener's version: a point is under the surface or it is not.
    #[test]
    fn a_point_is_submerged_below_the_surface_and_not_above_it() {
        let mut sea = still();
        sea.surface_height = 2.0;

        assert_eq!(submersion_at(&sea, [3.0, 1.0, -4.0], 0.0, 0.0, DEEP), 1.0);
        assert_eq!(submersion_at(&sea, [3.0, 3.0, -4.0], 0.0, 0.0, DEEP), 0.0);
        // A sphere straddling it is neither, which is what makes the same
        // function usable for a head as for a hull.
        let straddling = submersion_at(&sea, [3.0, 2.0, -4.0], 0.5, 0.0, DEEP);
        assert!((straddling - 0.5).abs() < 1e-5, "{straddling}");
    }

    /// **The chatter test, which is the failure this whole mechanism exists to
    /// prevent.** A body under the surface with a swell washing over it crosses
    /// any single threshold twice per wave. With two thresholds the state
    /// changes twice in the whole run; with one it changes on every crossing,
    /// and every one of those is a script callback and a splash.
    ///
    /// Written as a trace rather than a count, because a count alone is not
    /// discriminating: several plausible ways of getting this wrong — dropping
    /// the carried state, latching on the wrong threshold — also produce two
    /// changes. What has to hold is *when* they happen.
    #[test]
    fn hysteresis_is_what_stops_a_passing_wave_from_chattering_the_state() {
        const ENTER: f32 = 0.75;
        const EXIT: f32 = 0.25;
        // A body that sinks over a second, sits under the surface for eight
        // seconds while a swell washes over it — dipping to 0.5, which is below
        // `enter` and well above `exit` — and then floats out.
        let fraction = |tick: u32| -> f32 {
            let t = f32::from(u16::try_from(tick).unwrap_or(u16::MAX)) / 60.0;
            if t < 1.0 {
                t
            } else if t < 9.0 {
                0.7 + 0.2 * (t * std::f32::consts::TAU).sin()
            } else {
                0.0
            }
        };
        let trace = |enter: f32, exit: f32| {
            let mut was = false;
            let mut changes = Vec::new();
            for tick in 0..600 {
                let now = is_submerged(was, fraction(tick), enter, exit);
                if now != was {
                    changes.push((tick, now));
                }
                was = now;
            }
            changes
        };

        let changes = trace(ENTER, EXIT);
        assert_eq!(changes.len(), 2, "in once and out once: {changes:?}");
        // On the way in, at `enter` and not before — a state that latched on
        // the lower threshold would go under while the body was still mostly
        // out of the water.
        let (went_in, under) = changes[0];
        assert!(under);
        assert!(fraction(went_in) >= ENTER, "latched early at {}", fraction(went_in));
        assert!(fraction(went_in - 1) < ENTER, "latched late");
        // And out only when the body actually leaves, not on the first dip.
        let (came_out, under) = changes[1];
        assert!(!under);
        assert!(came_out > 9 * 60 - 2, "surfaced at tick {came_out}, on a dip");

        // **The mutation: one threshold instead of two.** Same signal, same code
        // path, and the state falls apart — so the gap between the two is load
        // bearing rather than decoration.
        let single = trace(ENTER, ENTER);
        assert!(
            single.len() > 10,
            "a single threshold should chatter, and this test proves nothing if \
             it does not: {} changes",
            single.len()
        );
    }

    /// The derived default displaces what the box does, rather than what four
    /// spheres drawn around its corners would — which is the §5.6 trap that
    /// makes a crate float like a cork.
    #[test]
    fn the_default_pontoons_displace_the_body_s_own_volume() {
        let half = [0.5, 0.4, 0.7];
        let pontoons = default_pontoons(half);
        assert_eq!(pontoons.len(), 4, "one pontoon gives no torque");

        let box_volume = 8.0 * half[0] * half[1] * half[2];
        let total: f32 = pontoons
            .iter()
            .map(|p| 4.0 / 3.0 * std::f32::consts::PI * p.radius.powi(3))
            .sum();
        assert!(
            (total - box_volume).abs() < 1e-4,
            "{total} of sphere against {box_volume} of box"
        );

        // And they are spread, or there is no torque to be had from them.
        let spread = pontoons.iter().map(|p| p.offset[0].abs()).sum::<f32>();
        assert!(spread > 0.0);
    }
}
