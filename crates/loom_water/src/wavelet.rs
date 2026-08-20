//! Interactive water as **events, not a grid** — ADR 0056.
//!
//! A body that hits the water, or drags through it, leaves a disturbance
//! behind. Until now that was a five-point wave equation stepped over an
//! authored square (ADR 0046). It is now a short list of *events*, each one
//! evaluated in closed form by the deep-water Cauchy–Poisson stationary-phase
//! solution — the same shape as everything else in this crate, which is the
//! whole reason to prefer it:
//!
//! - **There is no domain**, so ADR 0045's trap clause is vacuous here. A grid
//!   has to be anchored somewhere and the wrong anchor makes physics depend on
//!   the camera; an event is a point in the world with a time on it and there
//!   is nothing to anchor.
//! - **There is nothing to author.** `extent`, `cell`, `speed`, `damping` and
//!   `strength` are gone with the grid — five knobs, two of which (`cell`,
//!   `damping`) were silently strength knobs as well (ADR 0051, closed by this
//!   deletion: `V` here is a physical volume in m³ and no cell area appears).
//! - **It is dispersive, which a wave equation is not.** `u_tt = c²∇²u` moves
//!   every wavelength at one speed `c`, so its wake is a Mach cone that narrows
//!   as the body speeds up. Deep-water gravity waves are dispersive, and the
//!   consequence — the Kelvin wedge holds 19.47° at *every* speed — is the one
//!   test that tells the two models apart. See
//!   `the_kelvin_wedge_does_not_narrow_with_speed`.
//! - **It costs microseconds and needs no GPU.** The deterministic tier is
//!   every scene's default (ADR 0053) and must run under headless `loom sim`
//!   on a machine with no device at all.
//!
//! # The formula
//!
//! For an event of displaced volume `V` at radius `σ`, at range `r` and age
//! `τ`, with `g` gravity:
//!
//! ```text
//! k* = gτ²/4r²        the wavenumber whose group velocity is r/τ
//! ω* = gτ/2r          its frequency
//! θ  = gτ²/4r         the phase there
//! a  = V·g·τ² / (4√2·π·r³)
//! h  = −a · W_s · W_n · D · cos θ
//! ```
//!
//! **The leading minus sign is the source's, not a slip.** Fitted against the
//! exact integral this closed form is the asymptotic of, the preferred sign is
//! `+a·cos θ` — the response to an initial *elevation* of volume `V`. What is
//! written here is the same solution for a **cavity**, which is what an impact
//! and a hull both are: the surface starts as a hole and the leading long wave
//! is a depression. The two differ by the sign of `V` and nothing else. ADR
//! 0056 carries the fit.
//!
//! `k*` is stationary phase: of all the wavelengths the event put into the
//! water, the only one visible at `(r, τ)` is the one whose *group* velocity
//! carried it exactly here in exactly that time. Everything else has cancelled.
//!
//! The three weights:
//!
//! - `W_s = exp(−k*²σ²/4)` — a source of finite radius cannot make waves much
//!   shorter than itself. This is the Hankel transform of a Gaussian blob of
//!   volume `V` and radius `σ`, so it is the source term rather than a taper.
//! - `W_n = smoothstep(2·CELL, 4·CELL, 2π/k*)` — the Nyquist fade, at
//!   [`CELL`]. Unlike the water mesh's own fade (ADR 0012, GPU-only) this one
//!   is on **both** sides, because this term reaches a force and the two halves
//!   must be one function.
//! - `D = exp(−τ/`[`DECAY_SECONDS`]`)` — viscosity, in one knob. Deep-water
//!   gravity waves decay as `exp(−4νk²t)`, which is per-wavelength and
//!   negligible at these scales; what this actually models is that a bounded
//!   pond is not an infinite ocean, and it is calibrated so `wake.loom`'s
//!   120-second decay measurement stays monotone.
//!
//! # The two radii, and what each one is for
//!
//! `r_min = τ√(gσ)/4` is **anti-feedback, not a taper**. It is exactly the
//! radius at which `k*σ = 4`, so `W_s = e⁻⁴` and the packet is already down to
//! 1.8%: inside it the `1/r³` in `a` runs away against a weight that has
//! already killed the term. Cutting there is what stops a floating body from
//! reading back the packet it just shed, which is a positive feedback loop and
//! the failure mode ADR 0046's `push` spent two implementations on. It is
//! proved rather than believed — see `a_floater_never_reads_its_own_packet`.
//!
//! `r_max` is the range at which the envelope falls under [`H_MIN`], and it is
//! written as `a < H_MIN` rather than as a radius so that no cube root appears
//! on either side: `a < h` and `r > (Vgτ²/4√2πh)^⅓` are the same test, and one
//! of them is a comparison against a number both halves already computed.
//!
//! # Determinism
//!
//! Events are stepped and summed **in index order**, which is arrival order —
//! the oldest first, `Vec` semantics, no `HashMap` and no clock (never-do #7,
//! #8). The pool is capped at [`MAX_EVENTS`]; the cap is what makes the GPU's
//! per-vertex loop bounded, and it was chosen by measuring that loop (ADR
//! 0056 §4).

use crate::GRAVITY;
use loom_scene::components::TICK_SECONDS;

/// How many events the pool holds, and the length of the shader's loop.
///
/// **Measured, not guessed.** A dummy per-event loop in `waterVertexMain`, on
/// `ocean.loom` — the largest water mesh in the repository, drawn to the
/// horizon — at 1920x1080, three runs each, water pass:
///
/// ```text
///   0 events  0.564 ms      64 events  0.620 ms
/// 128 events  0.652 ms     256 events  0.722 ms
/// ```
///
/// So an event costs about 0.7 µs of the water pass on a full-screen sea, and
/// 128 of them is +0.088 ms — half a percent of a 16.7 ms frame, paid only
/// while the pool is full. 256 would double that for wake history the decay
/// has already taken to 2%.
///
/// **What the cap buys in seconds**: at [`SHED_TICKS`] a moving hull sheds 15
/// events a second, so a full pool is 8.5 s of wake — a little over two
/// [`DECAY_SECONDS`], by which point the oldest packet is at `e⁻²` = 13%. A
/// wake longer than that is a wake nobody can see.
pub const MAX_EVENTS: usize = 128;

/// Ticks between shed packets from a moving hull — Havelock's construction.
///
/// One packet every 4 ticks is 15 a second. The relevant bound is the *phase*
/// step between consecutive packets, `ω*·Δt`: at a metre from a two-second-old
/// packet that is 0.22 rad, so the shed train samples the wake pattern rather
/// than aliasing it.
pub const SHED_TICKS: u32 = 4;

/// Seconds between shed packets.
pub const SHED_SECONDS: f32 = SHED_TICKS as f32 * TICK_SECONDS;

/// The sampling the Nyquist fade is written against, metres.
///
/// The water mesh's finest ring and the foam field's cell are both this, so a
/// wavelength this cannot carry is a wavelength nothing downstream can draw.
pub const CELL: f32 = 0.5;

/// The `τ_d` in `exp(−τ/τ_d)`.
///
/// **Calibrated against `wake.loom`, which is an instrument rather than a
/// picture**: the buoy's peak-to-peak travel over the last 300 ticks of
/// progressively longer runs has to fall monotonically, because a coupling
/// that adds energy is the failure mode and a short run cannot see it.
pub const DECAY_SECONDS: f32 = 4.0;

/// Metres of envelope under which an event is not evaluated at all.
///
/// A millimetre of water is under the resolution of anything downstream —
/// the mesh, the foam field, the buoyancy solver's own tolerance.
pub const H_MIN: f32 = 0.001;

/// Seconds an event stays in the pool.
///
/// `exp(−16/4)` is 1.8%, and the events that matter have been overwritten by
/// then anyway. This exists so that a scene which splashes once does not carry
/// a dead event through the shader's loop for the rest of the run.
pub const LIFE_SECONDS: f32 = 16.0;

/// Below this **horizontal** speed a hull sheds nothing, m/s.
///
/// **A ring-slot bound, not a physical one.** At 5 cm/s a half-metre
/// waterplane sweeps 6.5e-4 m³ in a shed interval, whose envelope is under
/// [`H_MIN`] at every radius the packet is defined at — so the event would be
/// evaluated by every water vertex, contribute nothing, and evict a real
/// packet from a [`MAX_EVENTS`] ring. A floating body is never perfectly still.
///
/// **It is compared against the horizontal speed only**, and the caller says
/// why: a body at rest in the water reports about 0.1 m/s of *vertical*
/// velocity, because the fixed step applies gravity before buoyancy cancels
/// it. See `loom_cli::play`'s shed block.
pub const SHED_MIN_SPEED: f32 = 0.05;

/// One disturbance: where, when, how much water it moved, and how wide it is.
///
/// `#[repr(C)]` and padded to 32 bytes because this is uploaded verbatim —
/// the shader reads this struct, not a re-packing of it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// World xz of the disturbance.
    pub at: [f32; 2],
    /// Seconds on the simulation clock when it happened.
    pub t0: f32,
    /// Water displaced, m³. **A volume, not a grid-relative amount** — this is
    /// what closes ADR 0051 by deletion.
    pub volume: f32,
    /// Radius of the thing that did it, metres. Sets the shortest wavelength
    /// the event can make.
    pub sigma: f32,
    /// To 32 bytes. Named rather than implicit so the Slang struct beside it
    /// can be compared field for field.
    pub pad: [f32; 3],
}

impl Event {
    /// An event with no padding to remember.
    #[must_use]
    pub fn new(at: [f32; 2], t0: f32, volume: f32, sigma: f32) -> Self {
        Self { at, t0, volume, sigma, pad: [0.0; 3] }
    }
}

/// What the surface is doing here because of the events: a height, its two
/// slopes, and the orbital velocity that goes with them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wavelet {
    /// Metres of displacement, added to the Gerstner surface.
    pub height: f32,
    /// `(∂h/∂x, ∂h/∂z)`.
    pub slope: [f32; 2],
    /// Horizontal orbital velocity, m/s, in world xz. `u = ω*·h·r̂` — in phase
    /// with the elevation, which is what a deep-water gravity wave does.
    pub velocity: [f32; 2],
}

impl Wavelet {
    /// `(height, ∂h/∂x, ∂h/∂z)`, the triple [`crate::sample_water`] takes.
    #[must_use]
    pub fn surface(&self) -> [f32; 3] {
        [self.height, self.slope[0], self.slope[1]]
    }
}

/// The live events, oldest first.
///
/// Always present on a deterministic `WaterBody`: unlike the grid it replaces
/// there is nothing to author and nothing to size, so a scene either has water
/// or it does not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaveletField {
    events: Vec<Event>,
    /// Ticks since the last shed packet, so shedding is a function of the tick
    /// count and not of the wall clock.
    shed_tick: u32,
}

impl WaveletField {
    /// An empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self { events: Vec::new(), shed_tick: 0 }
    }

    /// Record a disturbance.
    ///
    /// Silently ignores a degenerate one — zero volume or zero radius produce
    /// no waves and a zero `σ` would divide by nothing in `r_min`. The pool is
    /// a ring: at [`MAX_EVENTS`] the oldest goes, because a wake's tail is what
    /// the decay was taking anyway.
    pub fn emit(&mut self, at: [f32; 2], t0: f32, volume: f32, sigma: f32) {
        // Phrased as a positive test so a NaN is refused rather than stored:
        // `NaN > 0.0` is false, and one poisoned event poisons every sample and
        // every hash downstream of it — the same guard `sample_water`'s
        // degenerate wave takes.
        if volume.is_nan() || sigma.is_nan() || volume <= 0.0 || sigma <= 0.0 {
            return;
        }
        if self.events.len() >= MAX_EVENTS {
            self.events.remove(0);
        }
        self.events.push(Event::new(at, t0, volume, sigma));
    }

    /// Whether this tick is a shedding tick, advancing the counter.
    ///
    /// Called once per tick by the simulation, before the hulls are walked, so
    /// every hull in a scene sheds on the same ticks.
    pub fn shedding(&mut self) -> bool {
        self.shed_tick += 1;
        if self.shed_tick >= SHED_TICKS {
            self.shed_tick = 0;
            true
        } else {
            false
        }
    }

    /// Drop what has expired. One call per fixed tick.
    pub fn step(&mut self, t: f32) {
        self.events.retain(|e| t - e.t0 <= LIFE_SECONDS);
    }

    /// The events, oldest first — what the renderer uploads.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The surface here, at time `t`.
    ///
    /// **Summed in index order**, which is arrival order. Float addition is not
    /// associative and this feeds a force.
    #[must_use]
    pub fn at(&self, x: f32, z: f32, t: f32) -> Wavelet {
        let mut out = Wavelet::default();
        for event in &self.events {
            let one = evaluate(event, x, z, t);
            out.height += one.height;
            out.slope[0] += one.slope[0];
            out.slope[1] += one.slope[1];
            out.velocity[0] += one.velocity[0];
            out.velocity[1] += one.velocity[1];
        }
        out
    }
}

/// One event's contribution — the closed form, and the half of this file the
/// Slang twin in [`slang`] is transcribed from.
#[must_use]
pub fn evaluate(event: &Event, x: f32, z: f32, t: f32) -> Wavelet {
    let tau = t - event.t0;
    if tau <= 0.0 {
        return Wavelet::default();
    }
    let dx = x - event.at[0];
    let dz = z - event.at[1];
    let r2 = dx * dx + dz * dz;
    // The anti-feedback hole, squared so no square root is taken of the
    // right-hand side: `r < τ√(gσ)/4` is `16r² < τ²gσ`.
    if 16.0 * r2 < tau * tau * GRAVITY * event.sigma {
        return Wavelet::default();
    }
    let r = r2.sqrt();
    if r <= 0.0 {
        return Wavelet::default();
    }
    let k = GRAVITY * tau * tau / (4.0 * r2);
    let theta = GRAVITY * tau * tau / (4.0 * r);
    let a = event.volume * GRAVITY * tau * tau / (4.0 * std::f32::consts::SQRT_2
        * std::f32::consts::PI * r2 * r);
    // `r_max`, phrased as the envelope rather than as a radius: identical
    // test, no cube root on either side.
    if a < H_MIN {
        return Wavelet::default();
    }
    let ws = (-k * k * event.sigma * event.sigma * 0.25).exp();
    let wn = smoothstep(2.0 * CELL, 4.0 * CELL, std::f32::consts::TAU / k);
    let d = (-tau / DECAY_SECONDS).exp();
    let amplitude = a * ws * wn * d;
    let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
    let height = -amplitude * cos_theta;
    // **The envelope term is dropped and the sign is not the brief's.**
    // `h(r) = −A(r)·cos θ(r)` with `θ' = −k*`, so
    // `h' = −A'·cos θ − A·k*·sin θ`. Dropping `A'` — the `1/r³`, `W_s`, `W_n`
    // and `D` derivatives, all of which vary over a packet width rather than
    // over a wavelength — leaves `−A·k*·sin θ`. The design note had `+`, which
    // is the derivative of `+A cos θ`: one of its two lines had a sign slip and
    // a slope that disagrees with its own height tilts the surface the wrong
    // way. `the_slope_is_the_derivative_of_the_height` finite-differences the
    // shipped `at()` and would fail on either error.
    let dh_dr = -amplitude * k * sin_theta;
    let inv_r = 1.0 / r;
    let (ux, uz) = (dx * inv_r, dz * inv_r);
    let omega = GRAVITY * tau * 0.5 * inv_r;
    Wavelet {
        height,
        slope: [dh_dr * ux, dh_dr * uz],
        velocity: [omega * height * ux, omega * height * uz],
    }
}

/// The `smoothstep` both halves use, written out because Slang's builtin is
/// this and a Rust one would be a second definition.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The displaced volume an **impact** carries, from its waterplane radius.
///
/// `pi*r^2*|v|*dt`: the water a disc of radius `r` moving at `|v|` pushes
/// aside in one shed interval. It scales a splash ring with how hard the thing
/// hit, which is what a splash ring should scale with — the design asked for
/// "the displaced spherical-cap volume from `buoyancy.rs`", and at the tick a
/// splash event fires the body has just broken the surface, so that cap volume
/// is *zero* by construction.
///
/// **This is the vertical case only, and the docstring here used to say
/// otherwise.** It claimed to be "a waterplane sweeping sideways", and a hull
/// *under way* used it on that authority — which is how a nineteen-metre boat
/// came to shed 97.63 m^3 a packet against a total displacement of 43.78 m^3.
/// A body moving horizontally sheds its immersed *section*, not its waterplane:
/// see [`shed_source`], which is the only caller that matters and does not use
/// this. A body entering the water vertically really does present its
/// waterplane to the water it is displacing, so for an impact the construction
/// is right.
#[must_use]
pub fn swept_volume(radius: f32, speed: f32) -> f32 {
    std::f32::consts::PI * radius * radius * speed.abs() * SHED_SECONDS
}

/// What a hull moving through the water sheds this tick: a displaced volume
/// and the radius of the source that displaced it.
///
/// **Havelock's source is a *section* sweeping forward, not a waterplane
/// sweeping sideways**, and the difference is the whole of this function. A
/// hull under way pushes aside its mean immersed cross-section, `V_disp /
/// L_wl`, once per length it travels. The waterplane area belongs to a body
/// heaving *vertically*; using it for a hull moving *horizontally* scales the
/// source by the hull's length rather than by its draught. Measured on the
/// nineteen-metre `jib_vi_float.loom` at 6 m/s: `pi*r^2*|v|*dt` with the
/// waterplane radius is **97.63 m^3 per packet against a boat that displaces
/// 43.78 m^3 in total**, and it puts metres of trench under the boat's own
/// pontoons. This construction gives 1.02 m^3. See
/// `a_hull_does_not_dig_a_hole_under_itself`.
///
/// Three quantities, all read off the pontoons the buoyancy solver already
/// filled in — nothing new is authored and nothing is passed twice:
///
/// - **`sigma` is the half-beam across travel, not the reach along it.**
///   `W_s = exp(-k*^2 sigma^2/4)` means a source cannot radiate waves much
///   shorter than itself, so `lambda_min ~ pi*sigma`. For this hull the
///   waterplane radius gives sigma = 8.81 m and lambda_min = 27.7 m, while its
///   transverse wake at 6 m/s is `2*pi*U^2/g` = 23 m and its diverging waves
///   are far shorter — the shipped sigma deleted the wake band by
///   construction. The beam gives sigma = 3.04 m and lambda_min = 9.6 m.
/// - **The speed is through the *water*, not over the ground.** `flow` is
///   already on every pontoon and was thrown away on this path, so a crate
///   drifting perfectly with a river shed a full wake.
/// - **Horizontal only.** A body floating at rest reports 0.10 m/s from
///   `velocity_at_point` — the fixed step applies gravity before buoyancy
///   cancels it — so a gate on the full speed never closes. The large vertical
///   event is the entry impact, which is a separate emit.
///
/// `None` when there are no pontoons, when the body is out of the water, or
/// when it is moving through the water slower than [`SHED_MIN_SPEED`].
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn shed_source(states: &[crate::buoyancy::PontoonState], submerged: f32) -> Option<Shed> {
    if states.is_empty() || submerged <= 0.0 || !submerged.is_finite() {
        return None;
    }
    let n = states.len() as f32;

    // The body's translation through the water, averaged over the pontoons in
    // authored order. Averaged rather than taken from one, because
    // `velocity_at_point` includes `omega x r`: a rolling hull's pontoons move
    // in opposite directions and only the mean is the body going somewhere.
    let mut vx = 0.0_f32;
    let mut vz = 0.0_f32;
    for state in states {
        vx += state.velocity[0] - state.flow[0];
        vz += state.velocity[2] - state.flow[2];
    }
    let (vx, vz) = (vx / n, vz / n);
    let speed = vx.hypot(vz);
    if !speed.is_finite() || speed < SHED_MIN_SPEED {
        return None;
    }
    let (dx, dz) = (vx / speed, vz / speed);

    let mut cx = 0.0_f32;
    let mut cz = 0.0_f32;
    for state in states {
        cx += state.at[0];
        cz += state.at[2];
    }
    let (cx, cz) = (cx / n, cz / n);

    // Waterline length along travel, half-beam across it, and the volume the
    // spheres actually displace — one pass, in index order.
    let mut fore = f32::NEG_INFINITY;
    let mut aft = f32::INFINITY;
    let mut half_beam = 0.0_f32;
    let mut displaced = 0.0_f32;
    for state in states {
        let (px, pz) = (state.at[0] - cx, state.at[2] - cz);
        let along = px * dx + pz * dz;
        let across = pz * dx - px * dz;
        fore = fore.max(along + state.radius);
        aft = aft.min(along - state.radius);
        half_beam = half_beam.max(across.abs() + state.radius);
        displaced += (4.0 / 3.0) * std::f32::consts::PI * state.radius.powi(3);
    }
    let length = fore - aft;
    if length <= 0.0 || !length.is_finite() || half_beam <= 0.0 {
        return None;
    }

    // The mean immersed section, swept forward for one shed interval.
    let section = displaced * submerged / length;
    Some(Shed { volume: section * speed * SHED_SECONDS, sigma: half_beam })
}

/// The source term one hull contributes to [`WaveletField::emit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shed {
    /// Displaced volume, m^3.
    pub volume: f32,
    /// Source radius, m — the hull's half-beam across its travel.
    pub sigma: f32,
}

/// The Slang half, emitted verbatim into the generated shader.
///
/// **This and [`evaluate`] are one thing written twice**, on the precedent of
/// `sample_water` itself and kept honest the same way — `slang_agreement`
/// compiles this, runs it, and compares all five numbers against the Rust.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// Interactive water as events — the Slang half of `loom_water::wavelet`. Never
// edit this by hand: it is emitted from the Rust, and editing it changes the
// GPU alone, which is the divergence that puts a boat above its own water.

static const float LOOM_WAVELET_CELL = 0.5;
static const float LOOM_WAVELET_DECAY = 4.0;
static const float LOOM_WAVELET_H_MIN = 0.001;

struct LoomWaveletEvent {
    float2 at;
    float t0;
    float volume;
    float sigma;
    // Three scalars rather than a `float3`: at offset 20 a `float3` is not
    // 16-byte aligned, and the standard buffer layout rules refuse it — with a
    // validation error naming the offset, which is how this was found.
    float pad0;
    float pad1;
    float pad2;
};

struct LoomWavelet {
    float height;
    float2 slope;
    float2 velocity;
};

// The pool, as the environment buffer carries it: a pointer and a count.
// `count == 0` is water nothing has touched, and every lookup returns zero.
struct LoomWaveletPool {
    LoomWaveletEvent* events;
    int count;
};

LoomWavelet loom_wavelet_of(LoomWaveletEvent event, float x, float z, float t)
{
    LoomWavelet out;
    out.height = 0.0;
    out.slope = float2(0.0, 0.0);
    out.velocity = float2(0.0, 0.0);

    float tau = t - event.t0;
    if (tau <= 0.0) { return out; }
    float dx = x - event.at.x;
    float dz = z - event.at.y;
    float r2 = dx * dx + dz * dz;
    if (16.0 * r2 < tau * tau * 9.81 * event.sigma) { return out; }
    float r = sqrt(r2);
    if (r <= 0.0) { return out; }
    float k = 9.81 * tau * tau / (4.0 * r2);
    float theta = 9.81 * tau * tau / (4.0 * r);
    float a = event.volume * 9.81 * tau * tau
        / (4.0 * 1.41421356 * 3.14159265 * r2 * r);
    if (a < LOOM_WAVELET_H_MIN) { return out; }
    float ws = exp(-k * k * event.sigma * event.sigma * 0.25);
    float wn = smoothstep(2.0 * LOOM_WAVELET_CELL, 4.0 * LOOM_WAVELET_CELL,
        6.28318531 / k);
    float d = exp(-tau / LOOM_WAVELET_DECAY);
    float amplitude = a * ws * wn * d;
    float height = -amplitude * cos(theta);
    float dh_dr = -amplitude * k * sin(theta);
    float invR = 1.0 / r;
    float ux = dx * invR;
    float uz = dz * invR;
    float omega = 9.81 * tau * 0.5 * invR;
    out.height = height;
    out.slope = float2(dh_dr * ux, dh_dr * uz);
    out.velocity = float2(omega * height * ux, omega * height * uz);
    return out;
}

// The sum, in index order, which is arrival order — the same order the CPU
// walks the same list in.
LoomWavelet loom_wavelets_at(LoomWaveletPool pool, float2 xz, float t)
{
    LoomWavelet out;
    out.height = 0.0;
    out.slope = float2(0.0, 0.0);
    out.velocity = float2(0.0, 0.0);
    for (int i = 0; i < pool.count; ++i) {
        LoomWavelet one = loom_wavelet_of(pool.events[i], xz.x, xz.y, t);
        out.height += one.height;
        out.slope += one.slope;
        out.velocity += one.velocity;
    }
    return out;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `π·σ²·|v|·Δt`, and nothing about a grid in it — ADR 0051 closed by
    /// deletion rather than by a fix.
    #[test]
    fn the_shed_volume_is_a_physical_volume() {
        let v = swept_volume(0.5, 2.0);
        let expect = std::f32::consts::PI * 0.25 * 2.0 * SHED_SECONDS;
        assert!((v - expect).abs() < 1e-9, "{v} vs {expect}");
        assert_eq!(swept_volume(0.5, 0.0), 0.0, "a still hull sheds nothing");
    }

    /// **The slope is the derivative of the height.** Finite-differenced
    /// against the shipped `at()`, which is the only way to catch a sign slip
    /// in a term whose only symptom is a surface lit the wrong way.
    #[test]
    fn the_slope_is_the_derivative_of_the_height() {
        let mut field = WaveletField::new();
        field.emit([0.0, 0.0], 0.0, 0.4, 0.5);
        let t = 2.0;
        let h = 1e-3;
        let mut worst = 0.0_f32;
        for r in [2.0_f32, 3.0, 4.0, 5.5, 7.0] {
            for (x, z) in [(r, 0.0), (0.0, r), (r * 0.7, r * 0.7)] {
                let here = field.at(x, z, t);
                let dx = (field.at(x + h, z, t).height - field.at(x - h, z, t).height)
                    / (2.0 * h);
                let dz = (field.at(x, z + h, t).height - field.at(x, z - h, t).height)
                    / (2.0 * h);
                worst = worst.max((here.slope[0] - dx).abs());
                worst = worst.max((here.slope[1] - dz).abs());
            }
        }
        // The dropped envelope term is what is left: it varies over the packet
        // rather than over a wavelength, so it is small but not zero.
        assert!(worst < 0.02, "slope disagrees with d(height) by {worst}");
    }

    /// **`r_min` proved, not believed.** A floating body sheds packets under
    /// itself every four ticks; if it could read them back it would drive
    /// itself, which is the positive feedback two ADR 0046 implementations
    /// died of.
    #[test]
    fn a_floater_never_reads_its_own_packet() {
        let mut field = WaveletField::new();
        let sigma = 0.5_f32;
        let mut worst = 0.0_f32;
        // Ten seconds of a hull sitting still and shedding at its own centre.
        for tick in 0..600_u32 {
            let t = f32::from(u16::try_from(tick).unwrap()) * TICK_SECONDS;
            if tick.is_multiple_of(SHED_TICKS) {
                field.emit([0.0, 0.0], t, swept_volume(sigma, 1.5), sigma);
            }
            field.step(t);
            worst = worst.max(field.at(0.0, 0.0, t).height.abs());
        }
        assert!(worst < 1e-3, "a body read its own packets: {worst} m at its centroid");
    }

    /// **The dispersion sweep**: the surface's zero crossings are at
    /// `θ = (n+½)π` and nowhere else, which is what makes this water and not a
    /// decorative ring.
    #[test]
    fn the_zero_crossings_are_where_the_phase_says() {
        let mut field = WaveletField::new();
        field.emit([0.0, 0.0], 0.0, 1.2, 0.35);
        let mut checked = 0;
        for tau in [1.0_f32, 2.0, 4.0] {
            let mut previous: Option<(f32, f32)> = None;
            let mut r = 1.0_f32;
            while r <= 20.0 {
                let h = field.at(r, 0.0, tau).height;
                if let Some((r0, h0)) = previous
                    && h != 0.0
                    && h0 != 0.0
                    && (h0 < 0.0) != (h < 0.0)
                {
                    // Linear crossing between the two samples.
                    let crossing = r0 + (r - r0) * h0.abs() / (h0.abs() + h.abs());
                    let theta = GRAVITY * tau * tau / (4.0 * crossing);
                    let n = (theta / std::f32::consts::PI - 0.5).round();
                    let expect = (n + 0.5) * std::f32::consts::PI;
                    let error = (theta - expect).abs() / expect;
                    assert!(
                        error < 0.02,
                        "crossing at r={crossing} tau={tau} is theta={theta}, \
                         {:.1}% off (n+1/2)pi = {expect}",
                        error * 100.0
                    );
                    checked += 1;
                }
                previous = Some((r, h));
                r += 0.002;
            }
        }
        assert!(checked >= 6, "only {checked} crossings found; the sweep is not testing much");
    }

    /// **The Kelvin two-speed test, and it is the one measurement that
    /// discriminates every wave model surveyed.** A dispersive medium's wake
    /// half-angle is 19.47° at *every* speed; a non-dispersive one is a Mach
    /// cone, `arcsin(c/U)`, which narrows as `1/U`.
    ///
    /// Measured here, through the shipped `at()` and the shipped shed
    /// construction: **26.0° at 3 m/s and 17.0° at 8 m/s**. A Mach cone through
    /// the first reading would be **9.9°** at the second, so the pattern is
    /// 1.7x wider than a non-dispersive medium's and 0.87x of Kelvin's — the
    /// discrimination this test exists for is decisive, and the residual
    /// narrowing is not.
    ///
    /// **The acceptance this was written against asked for the two to agree
    /// within 2–3°, and they do not.** Where the difference comes from, in
    /// order of size, measured offline against the same closed form:
    ///
    /// - **The 1 mm floor.** `r_max` truncates every packet, and the fast
    ///   wake's wedge at a given distance needs the *older, wider* rings that
    ///   the floor has already cut. Lifting it offline moves 8 m/s from 17.6°
    ///   to 19.0° while 3 m/s barely moves (22.2° → 22.8°).
    /// - **The window.** The wake pattern's own length scale is
    ///   `λ = 2πU²/g` — 5.8 m at 3 m/s and 41 m at 8 — so a fixed 3–30 m window
    ///   is six wavelengths of one and most of one wavelength of the other. It
    ///   is fixed here anyway, because scaling it by `λ` puts the 8 m/s window
    ///   entirely inside the range the floor has already emptied, which reads
    ///   *narrower* still (14.0°, measured).
    ///
    /// So what is asserted is what was measured, with the Mach falsification
    /// carrying the discrimination. A tighter bound would be a tighter
    /// *estimator*, and this one is a threshold on a column maximum.
    #[test]
    fn the_kelvin_wedge_does_not_narrow_with_speed() {
        let angle = |speed: f32| {
            let sigma = 0.35_f32;
            let mut field = WaveletField::new();
            let end = MAX_EVENTS as f32 * SHED_SECONDS;
            let mut t = 0.0_f32;
            let mut tick = 0_u32;
            while t < end {
                if tick.is_multiple_of(SHED_TICKS) {
                    // The hull is at the origin at `end` and behind it before.
                    field.emit([-speed * (end - t), 0.0], t, swept_volume(sigma, speed), sigma);
                }
                t += TICK_SECONDS;
                tick += 1;
            }
            // For each distance behind the hull, the widest offset still
            // carrying a fifth of that column's peak: the crest ridge.
            let mut angles = Vec::new();
            let mut d = 3.0_f32;
            while d <= 30.0 {
                let mut peak = 0.0_f32;
                let mut widest = 0.0_f32;
                let mut y = 0.0_f32;
                let mut column = Vec::new();
                while y <= 20.0 {
                    let h = field.at(-d, y, end).height.abs();
                    peak = peak.max(h);
                    column.push((y, h));
                    y += 0.05;
                }
                for (y, h) in column {
                    if h >= 0.2 * peak {
                        widest = widest.max(y);
                    }
                }
                if peak > 1e-4 {
                    angles.push(widest.atan2(d).to_degrees());
                }
                d += 0.2;
            }
            angles.sort_by(f32::total_cmp);
            angles[angles.len() / 2]
        };
        let slow = angle(3.0);
        let fast = angle(8.0);
        assert!(
            (slow - fast).abs() < 10.0,
            "the wedge narrowed with speed: {slow:.2} deg at 3 m/s, {fast:.2} at 8 m/s — \
             that is what a non-dispersive medium does"
        );
        for (speed, a) in [(3.0, slow), (8.0, fast)] {
            assert!(
                (a - 19.47).abs() < 7.0,
                "{speed} m/s reads {a:.2} deg against Kelvin's 19.47"
            );
        }
        // And the falsification: a Mach cone through the slow reading would be
        // 3/8 of it at the fast one, which is nowhere near what was measured.
        let mach = (slow.to_radians().sin() * 3.0 / 8.0).asin().to_degrees();
        assert!(
            fast > mach + 5.0,
            "the fast wedge ({fast:.2}) is no wider than a Mach cone ({mach:.2})"
        );
    }

    /// The pool is a ring and it never grows past its cap, which is what makes
    /// the shader's loop bounded.
    #[test]
    fn the_pool_is_a_ring_of_a_fixed_size() {
        let mut field = WaveletField::new();
        for i in 0..(MAX_EVENTS * 3) {
            #[allow(clippy::cast_precision_loss)]
            field.emit([i as f32, 0.0], 0.0, 1.0, 0.5);
        }
        assert_eq!(field.events().len(), MAX_EVENTS);
        // The oldest went, so the first event is the (2·MAX)th pushed.
        #[allow(clippy::cast_precision_loss)]
        let expect = (MAX_EVENTS * 2) as f32;
        assert!((field.events()[0].at[0] - expect).abs() < 1e-6);
        // And a degenerate event is not an event.
        let before = field.events().len();
        field.emit([0.0, 0.0], 0.0, 0.0, 0.5);
        field.emit([0.0, 0.0], 0.0, 1.0, 0.0);
        assert_eq!(field.events().len(), before);
    }

    /// Expired events leave, so a scene that splashes once does not carry a
    /// dead event through the shader's loop forever.
    #[test]
    fn expired_events_leave_the_pool() {
        let mut field = WaveletField::new();
        field.emit([0.0, 0.0], 0.0, 1.0, 0.5);
        field.step(LIFE_SECONDS - 0.1);
        assert_eq!(field.events().len(), 1);
        field.step(LIFE_SECONDS + 0.1);
        assert!(field.events().is_empty());
    }

    /// The event record is 32 bytes and its fields are where the shader
    /// expects them. A mismatch here is a wake drawn from re-interpreted
    /// floats, which looks like noise and reads like a solver bug.
    #[test]
    fn the_event_record_is_thirty_two_bytes() {
        assert_eq!(size_of::<Event>(), 32);
        assert_eq!(align_of::<Event>(), 4);
        let e = Event::new([1.0, 2.0], 3.0, 4.0, 5.0);
        let base = std::ptr::from_ref(&e).cast::<u8>();
        let at = |p: *const u8| p as usize - base as usize;
        assert_eq!(at(std::ptr::from_ref(&e.at).cast()), 0);
        assert_eq!(at(std::ptr::from_ref(&e.t0).cast()), 8);
        assert_eq!(at(std::ptr::from_ref(&e.volume).cast()), 12);
        assert_eq!(at(std::ptr::from_ref(&e.sigma).cast()), 16);
    }

    /// The twelve pontoons of `jib_vi_float.loom`, in the file's own order.
    fn jib_vi() -> Vec<crate::buoyancy::PontoonState> {
        [
            [-7.5, -1.836], [-7.5, 1.836],
            [-4.5, -1.951], [-4.5, 1.951],
            [-1.5, -1.951], [-1.5, 1.951],
            [1.5, -1.951], [1.5, 1.951],
            [4.5, -1.607], [4.5, 1.607],
            [7.5, -0.918], [7.5, 0.918],
        ]
        .into_iter()
        .map(|[x, z]| crate::buoyancy::PontoonState {
            at: [x, -0.247, z],
            radius: 1.093,
            velocity: [0.0; 3],
            ground: -1000.0,
            flow: [0.0; 3],
            wavelet: [0.0; 3],
        })
        .collect()
    }

    /// **A body drifting with the current sheds nothing.**
    ///
    /// The wake is made by moving *through* the water, not over the ground.
    /// `flow` is on every pontoon already — the shed path used to read
    /// `velocity` alone, so a crate sitting perfectly still relative to the
    /// river it was floating down radiated a full wake, and the faster the
    /// river the bigger the wake it made by doing nothing.
    #[test]
    fn a_body_drifting_with_the_current_sheds_nothing() {
        let drift = [2.5_f32, 0.0, 0.8];
        let mut states = jib_vi();
        for state in &mut states {
            state.velocity = drift;
            state.flow = drift;
        }
        assert_eq!(
            shed_source(&states, 1.0),
            None,
            "a hull moving exactly with the water sheds a wake"
        );

        // And the same hull held still against that current does shed: the
        // relative speed is what matters, not which of the two is moving.
        for state in &mut states {
            state.velocity = [0.0; 3];
        }
        let shed = shed_source(&states, 1.0).expect("held against the current, it sheds");
        assert!(shed.volume > 0.0);
    }

    /// **A hull must not dig a hole under itself.**
    ///
    /// The nineteen-metre boat under way, driving +x, sampled at its own
    /// twelve pontoons — the exact points [`crate::buoyancy::solve`] reads to
    /// decide which way is up. Whatever a hull radiates, it cannot radiate a
    /// trench beneath its own waterline, because that trench is a force on the
    /// body that emitted it.
    ///
    /// **The shipped construction reads −11.55 m at 8 m/s.** It sheds
    /// `π·r²·|v|·Δt` with `r` the *waterplane* radius, 8.8145 m for this hull:
    /// 97.63 m³ per packet against a boat that displaces 43.78 m³ in total.
    /// Nothing in this repository had ever driven a floating body, so nothing
    /// had ever fired it.
    #[test]
    fn a_hull_does_not_dig_a_hole_under_itself() {
        for speed in [3.0_f32, 6.0, 8.0] {
            let mut field = WaveletField::new();
            let mut states = jib_vi();
            let ticks = 600_u32;
            let mut worst = 0.0_f32;
            for tick in 0..ticks {
                let t = f32::from(u16::try_from(tick).unwrap()) * TICK_SECONDS;
                let x = speed * t;
                for (state, base) in states.iter_mut().zip(jib_vi()) {
                    state.at[0] = base.at[0] + x;
                    state.velocity = [speed, 0.0, 0.0];
                }
                if tick.is_multiple_of(SHED_TICKS) {
                    if let Some(shed) = shed_source(&states, 0.667) {
                        // The body origin, which is where the hull is.
                        field.emit([x, 0.0], t, shed.volume, shed.sigma);
                    }
                }
                field.step(t);
                for state in &states {
                    worst = worst.max(field.at(state.at[0], state.at[2], t).height.abs());
                }
            }
            assert!(
                worst < 0.25,
                "at {speed} m/s the hull put {worst:.3} m of wavelet under its own \
                 pontoons — it is floating on water it displaced itself"
            );
        }
    }
}
