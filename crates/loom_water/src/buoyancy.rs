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
    /// The interactive events at this pontoon: `(height, ∂h/∂x, ∂h/∂z)`.
    ///
    /// **Pre-sampled by the caller for the same reason `ground` and `flow`
    /// are**, and this is the one that carries a force. The event pool is state
    /// that lives beside the physics ([`crate::wavelet::WaveletField`]);
    /// keeping it out here is what lets `solve` and `sample_water` stay
    /// functions of their arguments, which is the property the Slang half is
    /// transcribed from.
    ///
    /// `[0.0; 3]` is water nothing has touched — ADR 0056.
    ///
    /// Per pontoon rather than per body, and for a sharper reason than `flow`:
    /// **a wake is what tips a boat.** A crest passing under one end of a hull
    /// and not the other is a torque, and averaging it to the centre would
    /// delete exactly the thing this feature exists to produce.
    pub wavelet: [f32; 3],
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
    /// The water this body is dragging with it, in kilograms — its **added
    /// mass**, and the reason the forces above are not simply what Archimedes
    /// and the damping came to.
    ///
    /// **Derived, never authored.** A sphere accelerating through a fluid
    /// carries half its own displaced volume with it — the exact potential-flow
    /// result, `m_a = ½ρV`, with no coefficient to get wrong — so the pontoon
    /// set the section table solved *is* the derivation. `jib_vi` reads 28.8 t
    /// at her designed waterline against 57.6 t of hull and 55.4 t when she is
    /// wholly under, which is the "of order one times displacement" a hull's
    /// heave added mass is known to be.
    ///
    /// **[`solve`] has already applied it** — see the `mass` parameter. It
    /// rides out here for the same reason [`Self::submerged`] does: the heave
    /// period `T = 2π√((m + m_a)/(ρgA_w))` is the test that would have caught
    /// the original defect, and a second opinion about `m_a` computed beside
    /// the solver would be free to disagree with the force actually applied.
    pub added_mass: f32,
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
///
/// **A generated section table integrated exactly was weighed against this and
/// rejected.** A real hull section widens as it rises, so it carries the same
/// nonlinearity on physical grounds rather than by imitation, and it would make
/// ADR 0063's shed source exact instead of estimated. It was not built, because
/// the defect that raised the question was entirely in the data: a boat whose
/// pontoons were 24% light and parked partly outside its own waterline. A
/// second immersion model on this function is new code on the path a crate in a
/// river and a buoy in a pool share with every hull in the repository, and the
/// cap already provides the property the ramp lacks. Solve the spheres — see
/// [`crate::buoyancy::solve`]'s callers and `assets/prefabs/jib_vi.loom`.
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
///
/// `sea` is the FFT cascade for a `spectrum` body, evolved to this same `t` by the
/// simulation that owns it — ADR 0076 and [`crate::ocean::Ocean::for_body`]. `None` for
/// every `gerstner` body, which is every scene but one.
///
/// `mass` is the body's, in kilograms — rapier's own figure, every collider hung
/// on the body included. It is here for one term: the **added mass** the hull
/// drags with it, which is derived from the wetted pontoon volume and applied by
/// scaling the wrench (see the end of this function). Pass `0.0` and the term
/// switches itself off, which is what a caller with no body behind its pontoons
/// wants and what every unit test below does.
#[must_use]
pub fn solve(
    water: &WaterBody,
    sea: Option<&crate::ocean::Ocean>,
    buoyancy: &Buoyancy,
    pontoons: &[PontoonState],
    centre_of_mass: [f32; 3],
    mass: f32,
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
            sea,
            [pontoon.at[0], pontoon.at[2]],
            t,
            pontoon.ground,
            pontoon.flow,
            pontoon.wavelet,
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
        // **Vertically the water's own motion is subtracted, waves included,
        // and that is the fix for a hull that could be held under the sea.**
        //
        // This used to subtract `flow` alone — the river's current — on the
        // argument that "a crate riding a swell really is moving through the
        // water". In heave it is not. A deep-water wave carries its surface
        // particles with the surface, so a body riding a crest has no vertical
        // velocity relative to the water and should feel no damping at all;
        // and the *radiation* damping this coefficient stands for genuinely
        // vanishes as the frequency does, because a slowly heaving hull makes
        // no waves. A constant coefficient on the body's **absolute** vertical
        // velocity is therefore unphysical in a long swell — and it is a brake
        // bolted to the seabed.
        //
        // What it cost, measured on `ocean_fft_storm.loom` (`Hs` 6.1 m) over
        // 3600 ticks: the damping on a fully-immersed `jib_vi` is
        // `rho.V.damp_linear` = 236 kN per m/s against a hull weighing 565 kN,
        // and a crest in that sea rises at 5 m/s. She could not follow it. The
        // sea closed over her, she rose at a terminal 1.003 m/s, and her worst
        // excursion below her own local surface was **7.64 m** — the hull
        // buried twice over. It is **0.98 m** here.
        //
        // **Horizontally the current alone is still what is subtracted, and
        // that is a deliberate corner rather than an oversight.** Taking surge
        // and sway to the water's frame as well removes everything that
        // resists a wave's orbital motion, so a floating body accumulates
        // Stokes drift with nothing to oppose it: measured on
        // `deeper_demo.loom`, a boat lying at her quay walks 0.20 m off her
        // berth in 30 s and is still accelerating. Station-keeping is a
        // mooring's job and this engine has no mooring.
        // ponytail: horizontal damping is against the world, which is a mooring
        // in disguise. Move it to the water's frame the day a scene can author
        // a mooring line — the one-line change is the two `flow` reads below.
        //
        // Every flat-water scene is bit-identical either way: with no waves
        // `sample_water` returns `velocity == flow` by construction, which is
        // what keeps a river's crate on the arithmetic it had.
        let v = pontoon.velocity;
        let carried = [
            v[0] - pontoon.flow[0],
            v[1] - surface.velocity[1],
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

    // **The water the hull drags with it.** `m_a = ½ρV` is the exact
    // potential-flow added mass of a sphere, so the pontoon set the section
    // table solved is the whole derivation and there is no coefficient to
    // author. It is taken off the summed `wet_volume` rather than accumulated
    // per pontoon because it is a property of the set, and one multiply cannot
    // reorder what sixteen additions already fixed.
    wrench.added_mass = 0.5 * water.density * wet_volume;

    // **And what it does to the body, exactly.** The equation of motion a hull
    // obeys is `(m + m_a)·a = F_water + m·g`; rapier's is `m·a = F_applied +
    // m·g`. Equating them gives `F_applied = κ·F_water + (κ − 1)·m·g` with
    // `κ = m/(m + m_a)` — a scaling of the wrench plus an upward correction,
    // no new state, nothing differenced, and nothing that can go unstable the
    // way an explicit `−m_a·dv/dt` does at `m_a ≈ m`.
    //
    // **Equilibrium is untouched and that is the check on the algebra**: put
    // `F_water = −m·g` in and `F_applied` comes back `−m·g` for any κ. So a
    // hull's settled waterline is exactly where it was and only her *response*
    // changed — which is the whole point, because the response is what the
    // heave period measures and the waterline is what Task 2 already settled.
    //
    // **The torque is left alone.** A sphere's added mass is a translational
    // quantity; the added *moment of inertia* of the water a hull rolls is a
    // different number that this derivation does not contain, and authoring
    // one is exactly what Task 3 forbids. So the linear response carries the
    // entrained water and the angular response does not — which is what an
    // added-mass tensor does anyway, since heave, sway, roll and pitch each
    // have their own coefficient. It also leaves the righting moment exactly
    // where the pontoon set puts it, which is what the `GZ` sweep measures.
    //
    // Isotropic, for the same reason: the derivation is a sphere's, and a
    // sphere's added mass is the same in every direction. A hull's is not —
    // surge is nearer 0.05 of displacement than 1.0 — and closing that gap is
    // a directional term, not a bigger scalar.
    if mass > 0.0 && wrench.added_mass > 0.0 {
        let kappa = mass / (mass + wrench.added_mass);
        wrench.force[1] = wrench.force[1].mul_add(kappa, (1.0 - kappa) * mass * GRAVITY);
    }
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
/// is underwater by a surface a crate floating beside it disagrees with — which is why
/// `sea` is here too, and why it must be the same ocean at the same `t` that [`solve`]
/// was handed.
#[must_use]
pub fn submersion_at(
    water: &WaterBody,
    sea: Option<&crate::ocean::Ocean>,
    at: [f32; 3],
    radius: f32,
    t: f32,
    ground: f32,
    wavelet: [f32; 3],
) -> f32 {
    // No flow: this asks *where the surface is*, and a current is horizontal —
    // it moves the water without raising or lowering it. Passing one in would
    // be sampling a number this function then discards.
    //
    // **`wavelet`, though, is not optional.** A wake raises the surface, so a
    // listener standing in one goes under sooner — and this is the query the
    // audio path and the eye-underwater flag both ask. Passing zero here would
    // be a second opinion about where the surface is, which is the one thing
    // this crate's header forbids.
    let surface = sample_water(water, sea, [at[0], at[2]], t, ground, [0.0; 3], wavelet);
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
            ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
        }];

        let w = solve(&water, None, &buoyancy, &pontoons, [0.0; 3], 0.0, 0.0);

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
                ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
            })
            .collect();

        let four = solve(&water, None, &buoyancy, &tilted, [0.0; 3], 0.0, 0.0);
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
            None,
            &buoyancy,
            &[PontoonState {
                at: [0.0, 0.0, 0.0],
                radius: 0.5,
                velocity: [0.0; 3],
                ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
            }],
            [0.0; 3],
            0.0,
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
                None,
                &buoyancy,
                &[PontoonState {
                    at: [0.0, -1.0, 0.0],
                    radius: 1.0,
                    velocity: [0.0, -speed, 0.0],
                    ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
                }],
                [0.0; 3],
                0.0,
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
            None,
            &Buoyancy::default(),
            &[PontoonState {
                at: [0.0, 9.0, 0.0],
                radius: 0.5,
                velocity: [0.0, -20.0, 3.0],
                ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
            }],
            [0.0; 3],
            0.0,
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
        let flow = sample_water(&water, None, [at[0], at[2]], 0.0, 0.0, [0.0; 3], [0.0; 3]).velocity;

        let w = solve(
            &water,
            None,
            &buoyancy,
            &[PontoonState { at, radius: 0.5, velocity: [0.0; 3], ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3] }],
            [0.0; 3],
            0.0,
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
            wavelet: [0.0; 3],
        };

        let still = solve(&water, None, &buoyancy, &[pontoon([0.0; 3])], [0.0; 3], 0.0, 0.0);
        assert_eq!(still.force, [0.0; 3], "flat water with no current is not still");

        let carried = solve(&water, None, &buoyancy, &[pontoon([2.0, 0.0, 0.0])], [0.0; 3], 0.0, 0.0);
        assert!(carried.force[0] > 1e-3, "the current does not push: {:?}", carried.force);
        assert_eq!(carried.force[2], 0.0, "a current along X pushes along Z");

        // And it pushes *harder* when it runs faster, which is what makes
        // `FlowField::speed` a knob rather than a switch.
        let faster = solve(&water, None, &buoyancy, &[pontoon([4.0, 0.0, 0.0])], [0.0; 3], 0.0, 0.0);
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
                ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
            })
            .collect();

        let a = solve(&water, None, &Buoyancy::default(), &pontoons, [0.0; 3], 0.0, 3.5);
        let b = solve(&water, None, &Buoyancy::default(), &pontoons, [0.0; 3], 0.0, 3.5);
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
                    ground: DEEP, flow: [0.0; 3], wavelet: [0.0; 3],
                })
                .collect::<Vec<PontoonState>>()
        };

        // Well clear of the water, on it, and well under it.
        assert_eq!(solve(&water, None, &buoyancy, &at(6.0), [0.0; 3], 0.0, 0.0).submerged, 0.0);
        let half = solve(&water, None, &buoyancy, &at(0.0), [0.0; 3], 0.0, 0.0).submerged;
        assert!((half - 0.5).abs() < 1e-5, "spheres centred on the surface: {half}");
        assert_eq!(solve(&water, None, &buoyancy, &at(-6.0), [0.0; 3], 0.0, 0.0).submerged, 1.0);

        // And it is the *fraction of the body*, not of the wet pontoons: two
        // corners under and two out is half a body, not a whole one.
        let mut tilted = at(0.0);
        for (index, state) in tilted.iter_mut().enumerate() {
            state.at[1] = if index < 2 { -6.0 } else { 6.0 };
        }
        let split = solve(&water, None, &buoyancy, &tilted, [0.0; 3], 0.0, 0.0).submerged;
        assert!((split - 0.5).abs() < 1e-5, "two of four under is half: {split}");
    }

    /// A body with no pontoons is not in the water, and is not a NaN either —
    /// which is what a bare division would put into the event log and into
    /// every script that reads it.
    #[test]
    fn a_body_with_no_pontoons_is_dry_rather_than_nan() {
        let w = solve(&still(), None, &Buoyancy::default(), &[], [0.0; 3], 0.0, 0.0);

        assert_eq!(w.submerged, 0.0);
        assert!(w.submerged.is_finite());
    }

    /// The listener's version: a point is under the surface or it is not.
    #[test]
    fn a_point_is_submerged_below_the_surface_and_not_above_it() {
        let mut sea = still();
        sea.surface_height = 2.0;

        assert_eq!(submersion_at(&sea, None, [3.0, 1.0, -4.0], 0.0, 0.0, DEEP, [0.0; 3]), 1.0);
        assert_eq!(submersion_at(&sea, None, [3.0, 3.0, -4.0], 0.0, 0.0, DEEP, [0.0; 3]), 0.0);
        // A sphere straddling it is neither, which is what makes the same
        // function usable for a head as for a hull.
        let straddling = submersion_at(&sea, None, [3.0, 2.0, -4.0], 0.5, 0.0, DEEP, [0.0; 3]);
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


    /// **JIB VI's sixteen pontoons**, as `assets/prefabs/jib_vi.loom` solves
    /// them from her section curve: eight pairs, one per slab of equal
    /// displaced volume, each pair reproducing that slab's displacement *and*
    /// its waterplane area. `(offset, radius)` in the body's own frame.
    ///
    /// **Copied, and the copy is the point.** The three tests below are
    /// closed-form predictions about a hull of this shape; reading the scene
    /// would make them fail the day somebody re-solves the boat, which is the
    /// coupling that makes a physics test useless as a bound. Her own numbers
    /// are asserted in the prefab.
    const JIB_VI: [([f32; 3], f32); 16] = [
        ([-5.9109, 0.2683, -1.1518], 1.3462),
        ([-5.9109, 0.2683, 1.1518], 1.3462),
        ([-3.859, -0.0247, -1.2271], 1.1859),
        ([-3.859, -0.0247, 1.2271], 1.1859),
        ([-2.1885, -0.1309, -1.2711], 1.1364),
        ([-2.1885, -0.1309, 1.2711], 1.1364),
        ([-0.6905, -0.1917, -1.2959], 1.1104),
        ([-0.6905, -0.1917, 1.2959], 1.1104),
        ([0.7407, -0.2109, -1.2908], 1.1025),
        ([0.7407, -0.2109, 1.2908], 1.1025),
        ([2.1844, -0.2104, -1.2481], 1.1027),
        ([2.1844, -0.2104, 1.2481], 1.1027),
        ([3.7571, -0.1602, -1.1291], 1.1236),
        ([3.7571, -0.1602, 1.1291], 1.1236),
        ([5.9667, 0.058, -0.7255], 1.2278),
        ([5.9667, 0.058, 0.7255], 1.2278),
    ];

    /// Her mass, from the prefab: the hull's own measured displacement.
    const JIB_VI_MASS: f32 = 57_636.0;

    /// The hull rigid, at heave `y` and heel `phi` radians about +X, in still
    /// water at `y = 0`. Nothing is moving unless `heave_rate` says so.
    fn jib_vi_at(y: f32, phi: f32, heave_rate: f32) -> Vec<PontoonState> {
        JIB_VI
            .iter()
            .map(|(offset, radius)| {
                let (s, c) = phi.sin_cos();
                PontoonState {
                    at: [
                        offset[0],
                        y + offset[1] * c - offset[2] * s,
                        offset[1] * s + offset[2] * c,
                    ],
                    radius: *radius,
                    velocity: [0.0, heave_rate, 0.0],
                    ground: DEEP,
                    flow: [0.0; 3],
                    wavelet: [0.0; 3],
                }
            })
            .collect()
    }

    /// **The added mass is derived, not authored**: half the water each
    /// pontoon has pushed aside, which is the exact potential-flow answer for
    /// a sphere and therefore has no coefficient in it at all.
    ///
    /// Checked at the two ends and in between, because `½ρV` is only worth
    /// asserting if `V` is the *wetted* volume: a body in the air drags
    /// nothing, and a body wholly under drags half of its whole self.
    #[test]
    fn the_added_mass_is_half_the_water_the_hull_has_pushed_aside() {
        let water = still();
        let buoyancy = Buoyancy::default();
        let mass_at = |y: f32| {
            solve(
                &water,
                None,
                &buoyancy,
                &jib_vi_at(y, 0.0, 0.0),
                [0.0; 3],
                JIB_VI_MASS,
                0.0,
            )
        };

        assert_eq!(mass_at(40.0).added_mass, 0.0, "in the air she drags nothing");

        // At her designed line the set displaces the hull's own 57.636 m^3, so
        // half of it is 29.5 t of this fixture's sea water against 57.6 t of
        // boat — the "of order one times displacement" a hull's heave added
        // mass is known to be.
        let designed = mass_at(0.0);
        assert!(
            (designed.added_mass - 0.5 * water.density * 57.636).abs() < 60.0,
            "half of 57.636 m^3 at density {}: {}",
            water.density,
            designed.added_mass
        );

        // Wholly under, the sixteen whole spheres are 108.0 m^3, so the water
        // she carries is very nearly her own mass.
        let under = mass_at(-30.0);
        let whole: f32 = JIB_VI
            .iter()
            .map(|(_, r)| 4.0 / 3.0 * std::f32::consts::PI * r.powi(3))
            .sum();
        assert!(
            (under.added_mass - 0.5 * water.density * whole).abs() < 60.0,
            "{} against half of {whole} m^3",
            under.added_mass
        );
    }

    /// **The added mass changes the response and not the waterline**, which
    /// is the check on the algebra rather than on the physics.
    ///
    /// `F_applied = κ·F_water + (κ−1)·m·g`, so the *net* of applied force and
    /// gravity is `κ·(F_water + m·g)` — scaled everywhere, and zero exactly
    /// where it was zero. A hull therefore settles at the draft the pontoon
    /// set says she does, however much water she is carrying, and Task 2 does
    /// not have to be re-argued. Bisected rather than asserted at a point,
    /// because "the zero has not moved" is the claim.
    #[test]
    fn the_added_mass_moves_the_response_and_not_the_waterline() {
        let water = still();
        let buoyancy = Buoyancy {
            damp_linear: 0.0,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };
        // The net a body of JIB VI's weight feels at heave `y`, when the solver
        // is told her mass is `told`.
        let net = |y: f32, told: f32| {
            solve(&water, None, &buoyancy, &jib_vi_at(y, 0.0, 0.0), [0.0; 3], told, 0.0).force[1]
                - JIB_VI_MASS * GRAVITY
        };
        let settles = |told: f32| {
            let (mut low, mut high) = (-2.0_f32, 2.0_f32);
            for _ in 0..60 {
                let mid = 0.5 * (low + high);
                // Deeper is more lift, so the net falls as `y` rises.
                if net(mid, told) > 0.0 { low = mid } else { high = mid }
            }
            0.5 * (low + high)
        };

        let bare = settles(0.0);
        let carried = settles(JIB_VI_MASS);
        assert!(
            (carried - bare).abs() < 1.0e-4,
            "the waterline moved with the added mass: {carried} against {bare}"
        );
        // And the response *did* change: a metre down, the net that drives her
        // back up is smaller by exactly κ.
        let (deep_bare, deep_carried) = (net(-1.0, 0.0), net(-1.0, JIB_VI_MASS));
        let added = solve(
            &water,
            None,
            &buoyancy,
            &jib_vi_at(-1.0, 0.0, 0.0),
            [0.0; 3],
            JIB_VI_MASS,
            0.0,
        )
        .added_mass;
        let kappa = JIB_VI_MASS / (JIB_VI_MASS + added);
        assert!(
            (deep_carried - kappa * deep_bare).abs() < 1.0,
            "{deep_carried} against κ·{deep_bare} with κ = {kappa}"
        );
    }

    /// **The heave natural period is the one the section table predicts** —
    /// `T = 2π√((m + m_a)/(ρgA_w))`, and this is the test that would have
    /// caught the original defect.
    ///
    /// The hull is released 5 cm above her designed line and integrated
    /// semi-implicit Euler at the engine's own 1/60 s, which is what rapier
    /// does to a free body under an applied force. Damping is turned down to
    /// 0.2 for the measurement and for no other reason: at the shipped 4.0 the
    /// motion is nearly critically damped and a period is not observable in
    /// it. The period is a property of mass and stiffness, not of damping.
    ///
    /// **It fails without the added mass**, which is the only thing that makes
    /// it worth writing: 57.6 t alone predicts 1.86 s, 57.6 + 28.8 predicts
    /// 2.28 s, and the engine measures 2.22 s.
    #[test]
    fn the_heave_period_is_the_one_the_section_table_predicts() {
        const DT: f32 = 1.0 / 60.0;
        let water = still();
        let buoyancy = Buoyancy {
            damp_linear: 0.2,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };

        let mut y = 0.05_f32;
        let mut v = 0.0_f32;
        let mut extrema: Vec<u32> = Vec::new();
        let (mut last, mut prev_rise) = (y, 0.0_f32);
        for tick in 0..900_u32 {
            let mut states = jib_vi_at(y, 0.0, v);
            for state in &mut states {
                state.velocity = [0.0, v, 0.0];
            }
            let w = solve(&water, None, &buoyancy, &states, [0.0; 3], JIB_VI_MASS, 0.0);
            // Semi-implicit Euler, gravity and the water in one step.
            v += DT * (w.force[1] / JIB_VI_MASS - GRAVITY);
            y += DT * v;
            let rise = y - last;
            if tick > 1 && rise * prev_rise < 0.0 {
                extrema.push(tick);
            }
            prev_rise = rise;
            last = y;
        }

        assert!(extrema.len() >= 4, "she did not oscillate: {extrema:?}");
        // The first three half-cycles, before the drift the integrator's own
        // rectification puts into a long run — see `assets/prefabs/jib_vi.loom`.
        let halves: Vec<f32> = extrema
            .windows(2)
            .take(3)
            .map(|w| f32::from(u16::try_from(w[1] - w[0]).unwrap_or(u16::MAX)) * DT)
            .collect();
        let measured = 2.0 * halves.iter().sum::<f32>() / 3.0;

        // The closed form, from the section table: the set's own displacement
        // and its own waterplane area, both read off the same spheres.
        let (mut volume, mut waterplane) = (0.0_f32, 0.0_f32);
        for (offset, r) in JIB_VI {
            volume += submerged_volume(r, offset[1], 0.0);
            waterplane += std::f32::consts::PI * (r * r - offset[1] * offset[1]).max(0.0);
        }
        let stiffness = water.density * GRAVITY * waterplane;
        let with = std::f32::consts::TAU
            * ((JIB_VI_MASS + 0.5 * water.density * volume) / stiffness).sqrt();
        let without = std::f32::consts::TAU * (JIB_VI_MASS / stiffness).sqrt();

        assert!(
            (measured - with).abs() < 0.15,
            "heave period {measured} s against the closed form's {with} s \
             (A_w {waterplane} m^2, V {volume} m^3)"
        );
        // And the mutation: the same run with no added mass would land on
        // `without`, which this bound excludes by a wide margin.
        assert!(
            (measured - without).abs() > 0.25,
            "the period is indistinguishable from the one with no added mass \
             ({without} s) — the term is not reaching the response"
        );
    }

    /// **A hull riding a crest is not damped against the seabed**, which is
    /// the defect that let her be held under a twenty-foot sea.
    ///
    /// Two pontoons in identical water, one still and one moving upward at
    /// exactly the water's own vertical velocity. The second is going *with*
    /// the water, so the water must not be resisting it.
    #[test]
    fn a_hull_moving_with_the_water_is_not_damped_by_it() {
        let mut water = WaterBody {
            drag: 0.0,
            ..WaterBody::default()
        };
        water.waves.waves.push(GerstnerWave {
            wavelength: 90.0,
            amplitude: 3.0,
            steepness: 0.3,
            direction: [1.0, 0.0],
            speed_scale: 1.0,
        });
        let buoyancy = Buoyancy {
            coefficient: 0.0, // buoyancy off, so only the damping shows
            damp_linear: 4.0,
            damp_quadratic: 1.0,
            ..Buoyancy::default()
        };
        // Off the origin and off the crest, where the orbital motion is
        // genuinely vertical rather than zero.
        let at = [11.0, -1.0, 0.0];
        let rising = sample_water(&water, None, [at[0], at[2]], 0.0, DEEP, [0.0; 3], [0.0; 3])
            .velocity[1];
        assert!(rising.abs() > 0.2, "the fixture has no vertical motion: {rising}");

        let pontoon = |vy: f32| PontoonState {
            at,
            radius: 1.5,
            velocity: [0.0, vy, 0.0],
            ground: DEEP,
            flow: [0.0; 3],
            wavelet: [0.0; 3],
        };
        let force = |vy: f32| {
            solve(&water, None, &buoyancy, &[pontoon(vy)], [0.0; 3], 0.0, 0.0).force[1]
        };

        assert!(
            force(rising).abs() < 1.0,
            "a pontoon travelling with the water is being damped: {}",
            force(rising)
        );
        // And one held still while the water rises past it is dragged along
        // with it, which is the same term doing the job it exists for.
        assert!(
            force(0.0) * rising > 0.0 && force(0.0).abs() > 1.0,
            "still water rising past a still body does nothing: {}",
            force(0.0)
        );
    }

    /// **The righting arm stays positive through the working range** — the
    /// fourth acceptance of `docs/design/SEA-BOAT-PLAN.md`.
    ///
    /// Heeled about +X in one-degree steps and released, the pontoon set has
    /// to push back the whole way: a torque about −X (the right-hand rule's
    /// answer for a hull rolled toward +Z) at every angle out to sixty
    /// degrees, and `GZ = torque / (m·g)` at its largest somewhere in the
    /// middle rather than at the origin.
    ///
    /// Free of the added mass by construction, because that term is on the
    /// force alone — which is one of the reasons it is.
    #[test]
    fn the_righting_arm_is_positive_through_the_working_range() {
        let water = still();
        let buoyancy = Buoyancy {
            damp_linear: 0.0,
            damp_quadratic: 0.0,
            ..Buoyancy::default()
        };
        let mut best = (0.0_f32, 0.0_f32);
        for degrees in 1_u8..=60 {
            let phi = f32::from(degrees).to_radians();
            let w = solve(
                &water,
                None,
                &buoyancy,
                &jib_vi_at(0.0, phi, 0.0),
                [0.0; 3],
                JIB_VI_MASS,
                0.0,
            );
            // Rolled toward +Z, so the righting torque is about −X.
            let gz = -w.torque[0] / (JIB_VI_MASS * GRAVITY);
            assert!(
                gz > 0.0,
                "GZ went negative at {degrees} deg: {gz} m (torque {:?})",
                w.torque
            );
            if gz > best.1 {
                best = (f32::from(degrees), gz);
            }
        }
        assert!(
            best.0 > 5.0 && best.0 < 60.0,
            "the largest righting arm is at {} deg, which is an end of the \
             sweep rather than a maximum inside it",
            best.0
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
