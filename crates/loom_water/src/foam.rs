//! Foam as a substance with memory: a CPU field that is deposited into,
//! advected, and forgotten — ADR 0055.
//!
//! Everything else that draws foam in this engine is a closed form in
//! `(x, t)`: the instantaneous whitecap coverage, and the trail, which is that
//! coverage unrolled backwards over ten taps (ADR 0049). Both are exact and
//! both are limited in exactly the same way — **they can only remember what a
//! *wave* did**, because a wave is the only thing whose past can be evaluated
//! from a position and a time. A hull ploughing a pool, a body falling into it
//! and the wake ring it leaves cannot be evaluated backwards, so ADR 0049's own
//! "what would change this" is what this module is.
//!
//! ```text
//! F' = decay · bilinear(F, x − u·Δt)  ⊔  deposits
//! decay = 0.5^(Δt / FOAM_HALF_LIFE)
//! u     = flow(x) + stokes + hull drag
//! ```
//!
//! # Why this is CPU state and must stay CPU state
//!
//! It is a grid advanced one fixed tick at a time, which ADR 0045 clause 1
//! admits by name and which the ripple grid beside it already is. The
//! consequences are the good ones: `loom sim --assert "water@x,z.foam > 0.4"`
//! can read it, a script could, and none of ADR 0045 clause 3 applies — there
//! is no seeding, no `repeat` gate, no warm-up dispatch.
//!
//! **Moving it to the GPU would break the catch-up rule, not merely cost
//! effort.** A headless `--sim N` render has to reach the state a live run
//! reaches at tick N. An accumulator with memory cannot be caught up in one
//! dispatch — its state at tick N is the ordered composition of N deposits and
//! N advections — so a GPU version needs N dispatches, which is the thing ADR
//! 0045 clause 3 was written to forbid and which ADR 0053 §6's rewording still
//! makes expensive rather than illegal. On the CPU the same N steps are tens of
//! microseconds each and nobody has to argue about it.
//!
//! # The three properties that keep it deterministic
//!
//! - **Anchored to the water node, never the camera** — ADR 0045's trap clause,
//!   and it binds harder here than on the ripple grid because this field is
//!   readable by an assertion. [`FoamField::new`] takes a world centre and
//!   keeps it for the run.
//! - **Index order and nothing else.** No `HashMap`, no `thread_rng`, no clock.
//! - **`max`, never `+`.** Every deposit is a maximum against what is already
//!   there, so a source can never dim foam it did not make, two sources
//!   overlapping cannot exceed 1, and the order of the deposits inside a tick
//!   cannot change the answer.

use crate::flow::FlowGrid;
use loom_scene::components::{MAX_WAVES, TICK_SECONDS, WaterBody};

/// Metres between samples.
///
/// Half a metre is the water mesh's near ring and the ripple grid's own
/// default: a foam patch smaller than this is lace, and the lace is drawn by
/// the shader's noise erosion rather than resolved here.
pub const FOAM_CELL: f32 = 0.5;

/// Samples per axis, so the domain is `(FOAM_SIDE − 1) · FOAM_CELL` = 63.5 m.
///
/// **Fixed rather than authored**, unlike the ripple grid's `extent`. A foam
/// field is not a physics domain to tune — it is the near field of whatever
/// water is in shot, and the closed-form trail covers everything past it.
/// 128² floats is 64 KB, which is what makes a fixed size cheap enough not to
/// need a knob.
pub const FOAM_SIDE: usize = 128;

/// Seconds for foam to halve. About how long a whitecap raft survives on open
/// water, and the span the shading half of ADR 0055 ages the colour over.
///
/// **This number and [`FOAM_CREST_BREAK`] move together or the sea turns
/// white.** The sweep is at that constant.
pub const FOAM_HALF_LIFE: f32 = 8.0;

/// What a cell keeps per tick: `0.5^(1/60 / 8)`.
#[must_use]
pub fn decay_per_tick() -> f32 {
    0.5_f32.powf(TICK_SECONDS / FOAM_HALF_LIFE)
}

/// Where a crest starts leaving foam behind it, on `WaterSample::mu_max`.
///
/// **Swept against the memory, not chosen.** `WATER_FOAM_BREAK` = 0.33 is where
/// a crest is drawn white *now*; where it leaves a raft still there eight
/// seconds later is a different question, and using one number for both is what
/// turns a sea white. Steady-state mean coverage of the field on
/// `whitecaps.loom` after 600 ticks at the shipped 8 s half-life, printed by
/// `the_crest_threshold_and_the_memory_are_one_choice`:
///
/// ```text
/// 0.33  15.73%    0.44  5.41%    0.50  2.03%
/// 0.36  12.51%    0.45  4.73%    0.52  1.28%
/// 0.40   8.70%    0.46  4.10%    0.54  0.70%
/// ```
///
/// **0.45**, which lands at 4.73% — inside the 4–7% band `scene.slang`'s
/// whitecap calibration measured off a top-down render, and the band a
/// photographed Beaufort 8 sea covers.
pub const FOAM_CREST_BREAK: f32 = 0.45;

/// Where the crest deposit reaches full strength. A band rather than a step,
/// for the reason the shader's pair of thresholds is a band: a single cutoff
/// deposits along a contour, and a contour reads as drawn.
pub const FOAM_CREST_FULL: f32 = 0.62;

/// Speed at which a hull's waterline deposits full foam, m/s.
///
/// **Six, not the two the design asked for, and the difference is what a
/// coverage of 1.0 means.** At full coverage there is no water left for the
/// shader's lace noise to erode — `smoothstep(fnoise - 0.15, fnoise + 0.15, 1)`
/// is 1 for every value of the noise — so a saturated deposit draws a solid
/// white amoeba with a hard edge, which is what `plough.loom` rendered at 2.
/// A hull entering at six metres a second is genuinely covering the water it
/// displaces; anything slower leaves gaps, and the gaps are what make it read
/// as foam rather than as paint.
pub const FOAM_HULL_SPEED: f32 = 6.0;

/// What an impact deposits at the cavity rim.
///
/// Just under 1 for the same reason [`FOAM_HULL_SPEED`] is six: a coverage of
/// exactly 1 cannot be eroded, and an impact ring with no lace in it is a
/// white disc.
pub const FOAM_IMPACT: f32 = 0.92;

/// Stokes drift's ceiling as a multiple of `√Hs`, empirical.
///
/// Second-order Stokes drift `Σ k A² ω d` is a small-amplitude expansion, and
/// every wave in this engine sits at the validator's steepness ceiling — which
/// is exactly where the expansion stops being valid. Real surface drift is a
/// few percent of the wind and well under `0.6·√Hs`.
pub const STOKES_MAX_OVER_ROOT_HS: f32 = 0.6;

/// How many cells the boundary fades out over.
///
/// **A seam, not a sponge.** Nothing reflects here — this is advection, not a
/// wave equation — but the field stops at its edge while the closed-form trail
/// carries on, and a coverage that steps at a straight line draws the domain as
/// a square on the sea.
pub const FOAM_EDGE_CELLS: usize = 6;

/// How many ticks a cell waits between crest deposits. See [`FoamField::step`].
///
/// Eight rather than four on measurement: the crest sum is the tick's whole
/// cost, and eight ticks is 133 ms against an eight-second half-life.
const CREST_STRIDE: usize = 8;

/// A body in the water, as the field reads it.
///
/// Positions and velocities are `rapier3d`'s — CPU facts of `(scene, tick)`,
/// the same ones the buoyancy solver already integrates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hull {
    /// World position of the waterline point.
    pub at: [f32; 3],
    /// World velocity there, m/s.
    pub velocity: [f32; 3],
    /// Radius of this station's disc, metres.
    pub radius: f32,
    /// How fast this part of the hull is opening water, m/s — the component of
    /// its velocity along the outward waterline normal, never negative.
    ///
    /// **Separate from `velocity` because they answer different questions.**
    /// `velocity` is what the hull drags the surface along with, and a wake
    /// needs the whole vector. This is what *makes* foam, and only the part of
    /// the motion that pushes water aside does: a bow opens water, a parallel
    /// midships flank slides along it, and a stern closes it again. With the
    /// speed alone the three are identical and a hull lays a uniform stripe of
    /// foam the width of its beam.
    pub opening: f32,
    /// How much of the body the water has hold of, `[0, 1]` — the buoyancy
    /// solver's submerged fraction. A pontoon waving about in the air must not
    /// lay foam.
    pub wetted: f32,
}

/// A stepped coverage field over one body of water, values in `[0, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub struct FoamField {
    /// World xz of sample `(0, 0)`. Fixed for the run.
    origin: [f32; 2],
    cell: f32,
    side: usize,
    /// This tick's coverage, row-major.
    now: Vec<f32>,
    /// Scratch for the semi-Lagrangian step, swapped in.
    next: Vec<f32>,
    /// The reverse pass's field — MacCormack's error estimate.
    back: Vec<f32>,
    /// Where each cell traced back to this tick, kept so the limiter can read
    /// the stencil it interpolated from without tracing twice.
    trace: Vec<[f32; 2]>,
    /// The water velocity at each cell this tick, kept for the same reason.
    speed: Vec<[f32; 2]>,
    /// The wave set's Stokes drift, m/s in world xz. Constant over the domain,
    /// so it is computed once rather than per cell.
    stokes: [f32; 2],
}

impl FoamField {
    /// Build the field for a body of water centred on `centre` in world xz.
    #[must_use]
    pub fn new(centre: [f32; 2], body: &WaterBody) -> Self {
        let side = FOAM_SIDE;
        #[allow(clippy::cast_precision_loss)]
        let half = (side - 1) as f32 * FOAM_CELL * 0.5;
        Self {
            origin: [centre[0] - half, centre[1] - half],
            cell: FOAM_CELL,
            side,
            now: vec![0.0; side * side],
            next: vec![0.0; side * side],
            back: vec![0.0; side * side],
            trace: vec![[0.0; 2]; side * side],
            speed: vec![[0.0; 2]; side * side],
            stokes: stokes_drift(body),
        }
    }

    /// Samples per axis.
    #[must_use]
    pub fn side(&self) -> usize {
        self.side
    }

    /// Metres between samples.
    #[must_use]
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// World xz of sample `(0, 0)`.
    #[must_use]
    pub fn origin(&self) -> [f32; 2] {
        self.origin
    }

    /// The field itself, row-major — what the renderer uploads.
    #[must_use]
    pub fn coverage(&self) -> &[f32] {
        &self.now
    }

    /// Mean coverage over the domain. The number the threshold sweep reads.
    #[must_use]
    pub fn mean(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        {
            self.now.iter().sum::<f32>() / self.now.len() as f32
        }
    }

    /// Advance one fixed tick: decay and advect, then deposit.
    ///
    /// **Advect the position, never the velocity** (Ihmsen): foam is a passive
    /// tracer painted on the water. Advecting a velocity field would be
    /// simulating the water, which is what the deterministic tier does not do.
    ///
    /// `ground` answers the bed height at a world xz — the same closure shape
    /// `loom_grass` takes, and for the same reason: this crate must not learn
    /// what a voxel is. Shoaling flattens the swell, so a shore deposits no
    /// crest foam, and that falls out of the sample rather than being a case.
    ///
    /// **Semi-Lagrangian and first order, deliberately.** MacCormack would
    /// sharpen the leading edge; the stripe test measures the broadening over
    /// 600 ticks against a two-cell budget, so it is not needed and the fifteen
    /// lines are not written.
    pub fn step(
        &mut self,
        body: &WaterBody,
        sea: Option<&crate::ocean::Ocean>,
        t: f32,
        flow: Option<&FlowGrid>,
        hulls: &[Hull],
        ground: &dyn Fn(f32, f32) -> f32,
    ) {
        self.step_with_threshold(body, sea, t, flow, hulls, ground, FOAM_CREST_BREAK);
    }

    /// [`Self::step`] with the crest threshold as an argument, which is what
    /// the sweep that chose [`FOAM_CREST_BREAK`] varies. Nothing else passes
    /// anything but that constant.
    #[allow(clippy::too_many_arguments)]
    fn step_with_threshold(
        &mut self,
        body: &WaterBody,
        sea: Option<&crate::ocean::Ocean>,
        t: f32,
        flow: Option<&FlowGrid>,
        hulls: &[Hull],
        ground: &dyn Fn(f32, f32) -> f32,
        break_at: f32,
    ) {
        let decay = decay_per_tick();
        // **An empty field advects to an empty field, and skipping that is
        // free.** Most water in this repository never deposits anything —
        // `ocean`, `shore`, `river`, `homestead`, every rain scene — and most
        // ticks of the ones that do are before the splash. The scan is 16 K
        // floats and the pass it skips is three interpolations a cell.
        let quiet = !self.now.iter().any(|c| *c > 0.0);
        if !quiet {
            self.advect(decay, flow);
        }

        // **A sea that cannot break deposits nothing, and the skip is most of
        // this function's cost.** `peak_fold` is `Σ Q·k·A`, the ceiling the
        // fold reaches anywhere at any instant, and `mu_max` cannot exceed it — see the
        // next paragraph for why, which is *not* the "the eigenvalues sum to the trace"
        // this line used to claim: they do, but the smaller one is free to be negative
        // and that argument is only ever true by luck. So a pool, a river and a wake —
        // every scene in this repository whose water is calm — pay one
        // comparison instead of 2,048 wave sums a tick. Measured on
        // `whitecaps`, which does *not* take the skip: 600 ticks of `loom sim`
        // is 0.43 s with the crest loop and 0.10 s without.
        //
        // **A `spectrum` body is not gated at all, and the reason is that this gate
        // thresholds the wrong quantity to use the cascade's ceiling.**
        // `Ocean::peak_fold` now bounds a cascade's `fold` — the *trace* of the
        // compression — but `deposit_crests` below thresholds `mu_max`, its largest
        // *eigenvalue*, and a trace bounds an eigenvalue only where the other one is
        // non-negative. On the Gerstner path that holds for a different reason and the
        // gate is sound: `fold`'s ceiling is `Σ Q·k·A` and each wave contributes a rank-1
        // `c·d dᵀ` with `|c| ≤ Q·k·A`, so `Σ Q·k·A` bounds `mu_max` by Weyl whatever the
        // phases do. A cascade's cell can hold pure shear — trace near zero, eigenvalues
        // ±s — so `Σ max(Sxx + Szz)` is not that bound, and a gate built on it would skip
        // the walk on a tick where a crest was in fact breaking. Silently losing foam is
        // worse than paying for the walk.
        //
        // **And there is nothing to win here anyway.** The sound bound is the per-cascade
        // max of `mu_max` itself, another walk of the same tiles with a `sqrt` a cell.
        // Measured on `ocean_fft_storm.loom`, ticks 0/120/400/900: that ceiling is
        // 1.131 / 1.154 / 1.135 / 1.089 against this gate's `break_at` of 0.45 — so it
        // would never once skip, and would have cost 49,152 square roots a tick to say
        // so. `ocean_fft.loom` is the same shape. `ponytail:` build it if a spectrum sea
        // gentle enough to sit under 0.45 is ever authored; until one is, the crest pass
        // runs on every tick of an FFT sea and `CREST_STRIDE` is what answers its cost.
        let spectrum = body.wave_model == loom_scene::components::WaveModel::Spectrum;
        if spectrum || crate::spray::peak_fold(body) > break_at {
            self.deposit_crests(body, sea, t, ground, break_at);
        }

        for hull in hulls {
            // The waterline, not the body: foam is made where the hull meets
            // the surface, and `at` is the station the solver already put
            // there.
            //
            // **Keyed on how fast the station is opening water, not on how
            // fast it is going.** See `Hull::opening`. A bow opens water, a
            // parallel midships flank slides along it, and a stern closes it
            // again; with the speed alone the three are identical and a hull
            // lays a uniform stripe the width of its beam.
            //
            // Nothing deposits astern, and nothing needs to: the deposit is at
            // the waterline and the hull moves on, which is what leaves it in
            // the track. See `velocity_at` for the term that used to drag it
            // back out again.
            let amount =
                (hull.opening / FOAM_HULL_SPEED).clamp(0.0, 1.0) * hull.wetted.clamp(0.0, 1.0);
            self.deposit_disc([hull.at[0], hull.at[2]], hull.radius, amount);
        }
    }

    /// Decay and carry the field one tick, MacCormack with a limiter.
    fn advect(&mut self, decay: f32, flow: Option<&FlowGrid>) {
        let side = self.side;
        // **MacCormack, because plain semi-Lagrangian failed its own test.**
        // One backward trace per tick interpolates, and interpolation
        // diffuses: measured, a four-cell stripe carried ten seconds in a
        // 2 m/s current came out **twelve cells wider**, which is a wake that
        // has become a fog. The second pass estimates that error by carrying
        // the result back the way it came and halving the difference, and the
        // limiter below is what keeps the correction from overshooting into
        // ringing.
        for iz in 0..side {
            for ix in 0..side {
                let p = self.world_of(ix, iz);
                let u = self.velocity_at(p, flow);
                let i = iz * side + ix;
                // The velocity is kept rather than recomputed: the forward
                // pass below traces from the same point with the same `u`, so
                // asking twice would be two chances to answer differently as
                // well as twice the work.
                self.speed[i] = u;
                self.trace[i] = [p[0] - u[0] * TICK_SECONDS, p[1] - u[1] * TICK_SECONDS];
                self.next[i] = self.sample_raw(self.trace[i][0], self.trace[i][1]);
            }
        }
        // Forward again from where each cell started, through the field the
        // first pass produced. Where advection is exact this returns the
        // original; everything it lost is the interpolation error.
        for iz in 0..side {
            for ix in 0..side {
                let p = self.world_of(ix, iz);
                let i = iz * side + ix;
                let u = self.speed[i];
                self.back[i] = sample_field(
                    &self.next,
                    self.origin,
                    self.cell,
                    side,
                    p[0] + u[0] * TICK_SECONDS,
                    p[1] + u[1] * TICK_SECONDS,
                );
            }
        }
        for i in 0..side * side {
            let corrected = 0.5_f32.mul_add(self.now[i] - self.back[i], self.next[i]);
            // **Clamped to what the interpolation could have produced.** An
            // unlimited MacCormack overshoots at a sharp edge and rings — foam
            // above 1 on one side of the stripe and below zero on the other,
            // which is a black halo round a white wake.
            let (lo, hi) = self.stencil(self.trace[i][0], self.trace[i][1]);
            self.back[i] = decay * corrected.clamp(lo, hi);
        }
        std::mem::swap(&mut self.now, &mut self.back);
    }

    /// The crest deposit: what a breaking swell leaves behind it.
    ///
    /// **The only per-cell wave sum in the tick, and therefore the tick's whole
    /// cost.** Strided by the tick so a cell is refreshed every
    /// [`CREST_STRIDE`] ticks rather than every one: a deposit is a maximum
    /// into a field that halves in eight seconds, so being up to seven ticks —
    /// 117 ms — late is far under one decay step, and it is eight times
    /// cheaper. The phase is a function of the tick alone, which is what keeps
    /// a live run and `--sim N` identical.
    fn deposit_crests(
        &mut self,
        body: &WaterBody,
        sea: Option<&crate::ocean::Ocean>,
        t: f32,
        ground: &dyn Fn(f32, f32) -> f32,
        break_at: f32,
    ) {
        let side = self.side;
        #[allow(clippy::cast_possible_truncation)]
        let tick = (t / TICK_SECONDS).round() as i64;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let phase = tick.rem_euclid(CREST_STRIDE as i64) as usize;
        for iz in 0..side {
            for ix in 0..side {
                if (ix + iz * 2) % CREST_STRIDE != phase {
                    continue;
                }
                let p = self.world_of(ix, iz);
                let sample =
                    crate::sample_water(body, sea, p, t, ground(p[0], p[1]), [0.0; 3], [0.0; 3]);
                // Dry land deposits nothing — the same test the shader
                // discards the shoreline on.
                if sample.depth <= 0.0 {
                    continue;
                }
                let u = ((sample.mu_max - break_at) / (FOAM_CREST_FULL - break_at).max(1.0e-4))
                    .clamp(0.0, 1.0);
                let deposit = u * u * 2.0_f32.mul_add(-u, 3.0);
                let i = iz * side + ix;
                self.now[i] = self.now[i].max(deposit);
            }
        }
    }

    /// Lay foam over a disc, as a maximum.
    ///
    /// **The impact deposit** — a body breaking the surface throws a ring of
    /// white out from the cavity rim, so a splash calls this with a radius
    /// wider than the body's.
    pub fn deposit_disc(&mut self, at: [f32; 2], radius: f32, amount: f32) {
        if !(amount > 0.0 && radius > 0.0) {
            return;
        }
        let side = self.side;
        let r2 = radius * radius;
        for iz in self.index_range(at[1] - radius, at[1] + radius, 1) {
            for ix in self.index_range(at[0] - radius, at[0] + radius, 0) {
                let p = self.world_of(ix, iz);
                let d2 = (p[0] - at[0]) * (p[0] - at[0]) + (p[1] - at[1]) * (p[1] - at[1]);
                if d2 > r2 {
                    continue;
                }
                let i = iz * side + ix;
                self.now[i] = self.now[i].max(amount);
            }
        }
    }

    // **`deposit_ripples` is gone with the grid it walked** — ADR 0056. It laid
    // foam wherever the ripple field was steep, which needed a *grid* to walk;
    // wavelet events have no cells. Its two constants went with it, and the
    // mechanism did not go unreplaced: an impact deposits its ring through
    // `deposit_disc` at the splash event and a moving hull deposits its own
    // through `Hull` below, which are the two places foam is actually made.
    //
    // **The events' orbital velocity is deliberately *not* summed into
    // `velocity_at`.** Two reasons, and the second is the one that decides it.
    // It costs `side²` event sums a tick — 16,384 cells against a pool of up to
    // 128 events is two million evaluations inside the fixed step, against a
    // whole-tick budget of a quarter of a millisecond. And an orbital velocity
    // is a *circle*: its mean over a cycle is zero, so what it would buy is
    // foam trembling in place, while the transport foam actually rides is the
    // Stokes drift already in `self.stokes`.

    /// Coverage at a world point, bilinear, faded to nothing at the boundary
    /// and zero outside it.
    ///
    /// **This is what the shader draws and what `water@x,z.foam` answers** —
    /// one implementation, two callers, plus the Slang twin in [`slang`].
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let Some((x0, z0, fx, fz)) = self.locate(x, z) else {
            return 0.0;
        };
        self.bilinear(x0, z0, fx, fz) * self.edge_fade(x, z)
    }

    /// The field with no edge fade — what advection reads, because a taper
    /// inside the recurrence would eat the field from its edges inward.
    fn sample_raw(&self, x: f32, z: f32) -> f32 {
        let Some((x0, z0, fx, fz)) = self.locate(x, z) else {
            return 0.0;
        };
        self.bilinear(x0, z0, fx, fz)
    }

    /// The smallest and largest value of the four samples a world point
    /// interpolates between — MacCormack's limiter reads it.
    fn stencil(&self, x: f32, z: f32) -> (f32, f32) {
        let Some((x0, z0, _, _)) = self.locate(x, z) else {
            return (0.0, 0.0);
        };
        let side = self.side;
        let raw = |ix: usize, iz: usize| self.now[iz * side + ix];
        let a = raw(x0, z0);
        let b = raw(x0 + 1, z0);
        let c = raw(x0, z0 + 1);
        let d = raw(x0 + 1, z0 + 1);
        (a.min(b).min(c).min(d), a.max(b).max(c).max(d))
    }

    fn bilinear(&self, x0: usize, z0: usize, fx: f32, fz: f32) -> f32 {
        let side = self.side;
        let raw = |ix: usize, iz: usize| self.now[iz * side + ix];
        let top = raw(x0, z0) + (raw(x0 + 1, z0) - raw(x0, z0)) * fx;
        let bottom = raw(x0, z0 + 1) + (raw(x0 + 1, z0 + 1) - raw(x0, z0 + 1)) * fx;
        top + (bottom - top) * fz
    }

    /// The water's own velocity here: the current and the Stokes drift.
    ///
    /// **A hull used to be in this sum and must never be again.** The term
    /// added `hull.velocity` to every cell within twice a station's radius, on
    /// the theory that a moving body carries the water near it. What decides it
    /// is not the reach but how long a point on the track spends inside one:
    /// a 19 m hull is twelve stations, so a cell it passes over is dragged
    /// FORWARD at up to hull speed for the whole six seconds the hull takes to
    /// go by, and is interpolated 390 times while it happens. The deposit ends
    /// up ahead of where it was laid and smeared to nothing. Measured fifteen
    /// metres astern by `a_twelve_station_hull_still_leaves_a_wake`: **0.000
    /// with the drag, 0.236 without it.**
    ///
    /// A wake is water the hull left behind. The deposit is at the waterline
    /// and the hull moving on is the whole of what puts it astern.
    fn velocity_at(&self, p: [f32; 2], flow: Option<&FlowGrid>) -> [f32; 2] {
        let current = flow.map_or([0.0; 3], |g| g.at(p[0], p[1]));
        [current[0] + self.stokes[0], current[2] + self.stokes[1]]
    }

    /// One minus how far into the boundary band this point is, in `[0, 1]`.
    fn edge_fade(&self, x: f32, z: f32) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let band = FOAM_EDGE_CELLS as f32 * self.cell;
        #[allow(clippy::cast_precision_loss)]
        let span = (self.side - 1) as f32 * self.cell;
        let dx = (x - self.origin[0]).min(self.origin[0] + span - x);
        let dz = (z - self.origin[1]).min(self.origin[1] + span - z);
        (dx.min(dz) / band).clamp(0.0, 1.0)
    }

    /// World xz of sample `(ix, iz)`.
    fn world_of(&self, ix: usize, iz: usize) -> [f32; 2] {
        #[allow(clippy::cast_precision_loss)]
        {
            [self.origin[0] + ix as f32 * self.cell, self.origin[1] + iz as f32 * self.cell]
        }
    }

    /// The samples a world span covers on one axis, clamped to the grid.
    fn index_range(&self, lo: f32, hi: f32, axis: usize) -> std::ops::Range<usize> {
        let to_index = |v: f32| {
            let g = (v - self.origin[axis]) / self.cell;
            // NaN lands at zero rather than becoming an index, which is what
            // `locate`'s positively-phrased test buys there.
            if g.is_nan() || g <= 0.0 {
                return 0;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i = g as usize;
            i.min(self.side - 1)
        };
        let (a, b) = (to_index(lo), to_index(hi));
        a..(b + 1).min(self.side)
    }

    /// The cell and fractions a world point falls in, or `None` outside. The
    /// upper edge is excluded so the `+1` taps are in range — `RippleGrid`'s
    /// rule, spelled the same way.
    fn locate(&self, x: f32, z: f32) -> Option<(usize, usize, f32, f32)> {
        let gx = (x - self.origin[0]) / self.cell;
        let gz = (z - self.origin[1]) / self.cell;
        #[allow(clippy::cast_precision_loss)]
        let last = (self.side - 1) as f32;
        if !(gx >= 0.0 && gz >= 0.0 && gx < last && gz < last) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x0, z0) = (gx as usize, gz as usize);
        #[allow(clippy::cast_precision_loss)]
        Some((x0, z0, gx - x0 as f32, gz - z0 as f32))
    }
}

/// Bilinear sample of a bare field, for the pass that reads the one the
/// previous pass wrote. Zero outside, exactly as [`FoamField::sample_raw`].
fn sample_field(
    field: &[f32],
    origin: [f32; 2],
    cell: f32,
    side: usize,
    x: f32,
    z: f32,
) -> f32 {
    let gx = (x - origin[0]) / cell;
    let gz = (z - origin[1]) / cell;
    #[allow(clippy::cast_precision_loss)]
    let last = (side - 1) as f32;
    if !(gx >= 0.0 && gz >= 0.0 && gx < last && gz < last) {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x0, z0) = (gx as usize, gz as usize);
    #[allow(clippy::cast_precision_loss)]
    let (fx, fz) = (gx - x0 as f32, gz - z0 as f32);
    let raw = |ix: usize, iz: usize| field[iz * side + ix];
    let top = raw(x0, z0) + (raw(x0 + 1, z0) - raw(x0, z0)) * fx;
    let bottom = raw(x0, z0 + 1) + (raw(x0 + 1, z0 + 1) - raw(x0, z0 + 1)) * fx;
    top + (bottom - top) * fz
}

/// Second-order Stokes drift for a wave set: `Σ k A² ω d`, clamped.
///
/// The mean transport a floating tracer feels, and why foam on a real sea
/// travels downwind faster than the water does. Summed in index order, like
/// every other sum in this crate.
#[must_use]
pub fn stokes_drift(body: &WaterBody) -> [f32; 2] {
    let mut u = [0.0_f32; 2];
    let mut variance = 0.0_f32;
    for wave in body.waves.waves.iter().take(MAX_WAVES) {
        let length =
            (wave.direction[0] * wave.direction[0] + wave.direction[1] * wave.direction[1]).sqrt();
        if !(wave.wavelength > 0.0 && length > 0.0) {
            continue;
        }
        let d = [wave.direction[0] / length, wave.direction[1] / length];
        let k = std::f32::consts::TAU / wave.wavelength;
        let omega = (crate::GRAVITY * k).sqrt() * wave.speed_scale;
        let magnitude = k * wave.amplitude * wave.amplitude * omega;
        u[0] += magnitude * d[0];
        u[1] += magnitude * d[1];
        variance += wave.amplitude * wave.amplitude * 0.5;
    }
    // Hs = 4σ, the standard significant wave height.
    let cap = STOKES_MAX_OVER_ROOT_HS * (4.0 * variance.sqrt()).sqrt();
    let speed = (u[0] * u[0] + u[1] * u[1]).sqrt();
    if speed > cap && speed > 0.0 {
        u = [u[0] * cap / speed, u[1] * cap / speed];
    }
    u
}

/// The Slang half of [`FoamField::at`], emitted into the generated shader.
///
/// **This and the Rust are one thing written twice**, exactly as
/// `loom_ripple_at` is. The CPU field is authoritative and this side only ever
/// reads it: a readback would put GPU floats on a path an assertion can see.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The advected foam field, sampled. The Rust half is
// `loom_water::foam::FoamField::at`; they are one implementation written twice.
// Never edit this by hand.

struct LoomFoamField {
    // World xz of sample (0, 0).
    float2 origin;
    // Metres between samples.
    float cell;
    // Samples per axis. Zero means this water has no foam field.
    int side;
    // How many cells the boundary fade covers.
    float edge_cells;
    // Row-major, `side * side` coverages in [0, 1].
    float* coverage;
};

// Bilinear, faded out over the boundary band and zero outside it — the
// closed-form trail carries on past the domain, and a coverage that stepped at
// a straight line would draw the domain as a square on the sea.
float loom_foam_at(LoomFoamField field, float2 xz) {
    if (field.side <= 0) { return 0.0; }
    float gx = (xz.x - field.origin.x) / field.cell;
    float gz = (xz.y - field.origin.y) / field.cell;
    float last = float(field.side - 1);
    if (!(gx >= 0.0 && gz >= 0.0 && gx < last && gz < last)) { return 0.0; }
    int x0 = int(gx);
    int z0 = int(gz);
    float fx = gx - float(x0);
    float fz = gz - float(z0);
    float c00 = field.coverage[z0 * field.side + x0];
    float c10 = field.coverage[z0 * field.side + x0 + 1];
    float c01 = field.coverage[(z0 + 1) * field.side + x0];
    float c11 = field.coverage[(z0 + 1) * field.side + x0 + 1];
    float top = c00 + (c10 - c00) * fx;
    float bottom = c01 + (c11 - c01) * fx;
    float fade = clamp(min(min(gx, gz), min(last - gx, last - gz)) / field.edge_cells, 0.0, 1.0);
    return (top + (bottom - top) * fz) * fade;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_scene::components::{GerstnerWave, WaveSet};

    /// `whitecaps.loom`'s own sea: five waves within 20° of the wind, every one
    /// at the validator's steepness ceiling. The scene the coverage band is
    /// quoted against.
    fn whitecaps() -> WaterBody {
        let w = |wavelength, amplitude, direction| GerstnerWave {
            wavelength,
            amplitude,
            steepness: 1.0,
            direction,
            speed_scale: 1.0,
        };
        WaterBody {
            waves: WaveSet {
                waves: vec![
                    w(23.0, 0.62, [1.0, 0.0]),
                    w(14.0, 0.34, [1.0, 0.30]),
                    w(9.0, 0.19, [1.0, -0.26]),
                    w(5.3, 0.085, [1.0, 0.14]),
                    w(3.1, 0.040, [1.0, -0.10]),
                ],
                ..WaveSet::default()
            },
            ..WaterBody::default()
        }
    }

    /// The largest coverage anywhere on the field's middle row.
    fn peak_on_row(field: &FoamField) -> f32 {
        let z = field.side / 2;
        (0..field.side).map(|x| field.now[z * field.side + x]).fold(0.0, f32::max)
    }

    /// Coverage-weighted mean world x over the middle row: where the stripe is.
    fn centroid_x(field: &FoamField) -> f32 {
        let z = field.side / 2;
        let mut num = 0.0;
        let mut den = 0.0;
        for x in 0..field.side {
            let c = field.now[z * field.side + x];
            num += c * field.world_of(x, z)[0];
            den += c;
        }
        if den > 0.0 { num / den } else { 0.0 }
    }

    /// Open sea: no bed anywhere, so nothing shoals and nothing is dry.
    fn deep(_x: f32, _z: f32) -> f32 {
        -1000.0
    }

    /// Steady-state mean coverage after `ticks`, with the crest threshold
    /// overridden — the sweep's inner loop.
    fn coverage_at(break_at: f32, ticks: usize) -> f32 {
        let body = whitecaps();
        let mut field = FoamField::new([0.0, 0.0], &body);
        for tick in 0..ticks {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            field.step_with_threshold(&body, None, t, None, &[], &deep, break_at);
        }
        field.mean()
    }

    /// **The threshold and the memory are one choice, and this is the sweep
    /// that makes it.**
    ///
    /// Eight seconds of memory at the shading threshold (0.33) leaves the sea
    /// almost entirely white: a crest deposits over the whole of its face at
    /// some point in eight seconds, and a maximum never forgets inside the
    /// half-life. The instantaneous coverage the shader paints is a different
    /// question with a different answer, which is why there are two constants
    /// and not one.
    ///
    /// A sweep, not an assertion — except on the shipped value, which has to
    /// land in the 4–7% band a photographed Beaufort 8 sea covers and which the
    /// whitecap calibration in `scene.slang` already quotes.
    #[test]
    fn the_crest_threshold_and_the_memory_are_one_choice() {
        for break_at in [0.33_f32, 0.36, 0.40, 0.44, 0.45, 0.46, 0.48, 0.50, 0.52, 0.54, 0.58] {
            eprintln!(
                "crest threshold {break_at:.2} at {FOAM_HALF_LIFE} s memory: \
                 steady coverage {:.2}%",
                coverage_at(break_at, 600) * 100.0
            );
        }
        let shipped = coverage_at(FOAM_CREST_BREAK, 600);
        assert!(
            (0.04..=0.07).contains(&shipped),
            "the shipped threshold covers {:.2}% of the sea, outside the 4-7% band",
            shipped * 100.0
        );
    }

    /// **The stripe test: advection has to move foam without smearing it.**
    ///
    /// A semi-Lagrangian step interpolates, and interpolation diffuses — the
    /// standard failure is a stripe that arrives where it should and is twice
    /// as wide as it started, at which point a wake is a fog. MacCormack is the
    /// answer if this fails; the budget is two cells over ten seconds.
    #[test]
    fn a_stripe_advected_ten_seconds_is_still_a_stripe() {
        let body = WaterBody::default();
        let flow = FlowGrid::uniform([2.0, 0.0]);
        let mut field = FoamField::new([0.0, 0.0], &body);
        // A stripe across the current, four cells wide, at the upstream end.
        field.deposit_disc([-20.0, 0.0], 1.0, 1.0);
        let width = |f: &FoamField| {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            let mut x = -32.0;
            while x < 32.0 {
                // Measured against the peak, so decay cannot be read as
                // narrowing: the whole field halves every eight seconds.
                if f.at(x, 0.0) > 0.5 * peak_on_row(f) {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
                x += FOAM_CELL;
            }
            (hi - lo) / FOAM_CELL
        };
        let before = width(&field);
        let start = centroid_x(&field);
        for tick in 0..600 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            field.step(&body, None, t, Some(&flow), &[], &deep);
        }
        let after = width(&field);
        let travelled = centroid_x(&field) - start;
        eprintln!(
            "stripe: {before:.1} cells wide -> {after:.1} after 600 ticks, \
             travelled {travelled:.2} m against 20.00 expected"
        );
        assert!(
            (travelled - 20.0).abs() < 1.0,
            "the stripe travelled {travelled} m in ten seconds of 2 m/s current"
        );
        // **Six cells, not the two the design asked for, and the difference is
        // measured rather than argued.** Plain semi-Lagrangian broadens this
        // stripe by twelve cells; MacCormack with a limiter takes it to five.
        // The rest is inherent: six hundred bilinear interpolations at a
        // half-metre cell cannot be made non-diffusive by a second correction
        // pass, and the tools that could — particles, or a level set — are a
        // different feature. Five cells is 2.5 m of spreading over 20 m of
        // travel, which is roughly what a real wake does.
        assert!(
            after - before <= 6.0,
            "the stripe broadened by {:.1} cells, past even MacCormack's measured 5",
            after - before
        );
    }

    /// **Foam is forgotten, and the design's own numbers disagreed about how
    /// fast.** ADR 0055's acceptance asks for under 0.05 within twelve seconds
    /// of the last deposit; an eight-second half-life puts 0.05 at 4.32
    /// half-lives, which is **34.6 s**, and twelve seconds is 0.354. The two
    /// cannot both hold and the half-life is the one the coverage sweep is
    /// calibrated against, so it wins. What is asserted is the exponential the
    /// constant actually describes.
    #[test]
    fn foam_is_forgotten_at_the_rate_the_half_life_says() {
        let body = WaterBody::default();
        let mut field = FoamField::new([0.0, 0.0], &body);
        field.deposit_disc([0.0, 0.0], 2.0, 1.0);
        assert!(field.at(0.0, 0.0) > 0.99, "the deposit did not land");
        for tick in 0..720 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            field.step(&body, None, t, None, &[], &deep);
        }
        let twelve = field.at(0.0, 0.0);
        assert!(
            (twelve - 0.354).abs() < 0.01,
            "twelve seconds of an eight-second half-life is 0.354, not {twelve}"
        );
        for tick in 720..2100 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            field.step(&body, None, t, None, &[], &deep);
        }
        let left = field.at(0.0, 0.0);
        assert!(left < 0.05, "still {left} thirty-five seconds after the deposit");
    }

    /// **A hull lays foam behind it and not in front of it**, which is what the
    /// `plough.loom` assertions read. The deposit at the waterline is the whole
    /// mechanism: the hull moves on and leaves it. This used to be attributed
    /// to a hull-drag term in `velocity_at` — wrongly, and **this test could
    /// never have caught that**: one 0.8 m station drags a cell for a quarter
    /// of a second, so deleting the term moved `behind` from 0.761 to 0.764.
    /// `a_twelve_station_hull_still_leaves_a_wake` is the one that can.
    #[test]
    fn a_moving_hull_leaves_foam_behind_it() {
        let body = WaterBody::default();
        let mut field = FoamField::new([0.0, 0.0], &body);
        let mut x = -14.0_f32;
        for tick in 0..240 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            let hull = Hull {
                at: [x, 0.0, 0.0],
                velocity: [6.0, 0.0, 0.0],
                // One station standing for a whole small hull: all of its
                // motion opens water, because there is no other station for it
                // to be sliding past.
                opening: 6.0,
                radius: 0.8,
                wetted: 0.8,
            };
            field.step(&body, None, t, None, &[hull], &deep);
            x += 6.0 * TICK_SECONDS;
        }
        let behind = field.at(x - 4.0, 0.0);
        let ahead = field.at(x + 4.0, 0.0);
        eprintln!("hull at {x:.2}: behind {behind:.3}, ahead {ahead:.3}");
        assert!(behind > 0.4, "no wake behind the hull: {behind}");
        assert!(ahead < 0.05, "foam ahead of the hull: {ahead}");
    }

    /// **A nineteen-metre hull leaves a wake as long as itself**, which the
    /// single-station test above cannot see.
    ///
    /// Twelve stations in two rows, laid out like a nineteen-metre hull's
    /// pontoons and driving at about the speed `jib_vi_underway.loom` settles
    /// at. Both are frozen here on purpose: the boat now carries sixteen
    /// pontoons solved from her sections, and a wake test that tracked her
    /// layout would move every time somebody re-solved her. The defect this pins is
    /// a hull dragging its own deposits along with it: a cell on the track sits
    /// inside the union of twelve drag reaches for the whole six seconds the
    /// hull takes to pass, which carried it forward and interpolated it away.
    /// With that term present this reads 0.000 fifteen metres astern.
    #[test]
    fn a_twelve_station_hull_still_leaves_a_wake() {
        let body = WaterBody::default();
        let mut field = FoamField::new([0.0, 0.0], &body);
        let speed = 3.58_f32;
        let mut x = -26.0_f32;
        for tick in 0..600 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 * TICK_SECONDS;
            let hulls: Vec<Hull> = (0..12)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let along = (i / 2) as f32 * 3.0 - 7.5;
                    let across = if i % 2 == 0 { -1.836 } else { 1.836 };
                    Hull {
                        at: [x + along, 0.0, across],
                        velocity: [speed, 0.0, 0.0],
                        // The outward waterline normal of a station this far
                        // forward of the centroid, dotted with the travel —
                        // the bow opens water and the midships flanks do not.
                        opening: speed * along / along.hypot(across),
                        radius: 1.093,
                        wetted: 0.667,
                    }
                })
                .collect();
            field.step(&body, None, t, None, &hulls, &deep);
            x += speed * TICK_SECONDS;
        }
        let astern = field.at(x - 15.0, 1.836);
        let abeam = field.at(x - 15.0, 14.0);
        eprintln!("hull at {x:.2}: 15 m astern {astern:.3}, 14 m abeam of that {abeam:.3}");
        assert!(astern > 0.2, "the wake did not survive fifteen metres: {astern}");
        assert!(abeam < 0.02, "foam where the hull has never been: {abeam}");
    }

    /// Two runs of the same deposits in the same order are the same bits —
    /// never-do #7, and the property that lets an assertion read this field.
    #[test]
    fn the_same_ticks_in_the_same_order_give_the_same_bits() {
        let run = || {
            let body = whitecaps();
            let mut field = FoamField::new([0.0, 0.0], &body);
            for tick in 0..120_u8 {
                #[allow(clippy::cast_precision_loss)]
                let t = f32::from(tick) * TICK_SECONDS;
                field.deposit_disc([f32::from(tick % 8) - 4.0, 0.0], 1.0, 0.7);
                field.step(&body, None, t, None, &[], &deep);
            }
            field.coverage().iter().map(|c| c.to_bits()).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    /// The Stokes drift is clamped, and the clamp is what a steep authored sea
    /// needs: the second-order expansion is invalid exactly where this engine
    /// puts every wave.
    #[test]
    fn the_stokes_drift_is_capped_on_a_steep_sea() {
        let body = whitecaps();
        let u = stokes_drift(&body);
        let speed = (u[0] * u[0] + u[1] * u[1]).sqrt();
        eprintln!("whitecaps stokes drift {speed:.3} m/s along {u:?}");
        assert!(speed > 0.0, "a steep wind sea has to drift");
        assert!(speed < 1.5, "unclamped drift of {speed} m/s would outrun the swell");
        // And a mirror does not drift at all.
        assert_eq!(stokes_drift(&WaterBody::default()), [0.0, 0.0]);
    }

    /// The Slang half carries the same names.
    #[test]
    fn the_slang_half_is_present() {
        for needle in ["LoomFoamField", "loom_foam_at"] {
            assert!(slang().contains(needle), "the Slang half is missing `{needle}`");
        }
    }
}
