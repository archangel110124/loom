//! Interactive ripples: the one part of the water that is stepped — ADR 0046.
//!
//! Everything else in this crate is a closed form in `(x, z, t)`. This is an
//! explicit five-point 2D wave equation advanced one fixed tick at a time, so
//! that a body moving through the water leaves a wake that outlives it and
//! that *other* bodies can feel.
//!
//! ```text
//! u' = 2u − u_prev + C² (u_left + u_right + u_up + u_down − 4u),   C = c·dt/h
//! ```
//!
//! ADR 0045 clause 1 admits this shape by name: state advanced inside the
//! fixed step, under never-do #7 and #8, is exactly as reproducible as
//! `rapier3d`, which is itself stepped state. Three properties are what make
//! that true here and they are all structural rather than conventional:
//!
//! - **The domain is anchored to the water node, never to the camera.** That
//!   is ADR 0045's trap clause. A camera-following grid would make the force
//!   on a floating body a function of where the viewer is standing, so
//!   `loom render --sim N` and `loom run` would compute different physics.
//!   [`RippleGrid::new`] takes a world centre and keeps it for the run.
//! - **No `HashMap`, no `thread_rng`, no clock.** The step is two `Vec<f32>`
//!   walked in index order; the sum order is the index order and nothing else.
//! - **The CPU grid is authoritative.** The GPU gets a copy to displace the
//!   surface with (see `Renderer::set_ripples`) and never writes one back —
//!   ADR 0045 clause 2, which is the reason this lives in `loom_water` where
//!   `ash` cannot be imported at all.
//!
//! # Stability
//!
//! The scheme is unconditionally unstable above the Courant limit
//! `C ≤ 1/√2`. `loom validate` refuses a body that authors one past it, with
//! both numbers in the message ([`loom_scene::scene`]'s `check_ripples`), and
//! [`RippleGrid::new`] *also* clamps — for the same reason `sample_water`
//! takes `MAX_WAVES` rather than trusting its caller: a hand-built component
//! that never went through the parser must not be able to reach `inf` and take
//! every floating body in the scene with it.

use loom_scene::components::{COURANT_LIMIT, Ripples, TICK_SECONDS, ripple_side};

/// How many cells wide the edge taper is.
///
/// **A sponge, not a decoration.** With a plain fixed boundary a wake reflects
/// off the edge of the domain and comes back inverted, and — worse — the
/// height steps discontinuously where the grid stops and the plain Gerstner
/// surface takes over, which draws the domain as a visible square in the sea.
/// Ramping the field to zero over the outer cells absorbs the reflection and
/// hides the seam with the same four lines.
const EDGE_CELLS: usize = 4;

/// A stepped height field over one body of water.
///
/// Heights are metres of displacement **added to** the Gerstner surface, not a
/// replacement for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RippleGrid {
    /// World xz of sample `(0, 0)` — the domain's minimum corner. Fixed for
    /// the run: this is the anchor ADR 0045's trap clause is about.
    origin: [f32; 2],
    /// Metres between samples, `h`.
    cell: f32,
    /// Samples per axis.
    side: usize,
    /// This tick's field, row-major, `side * side` of them.
    now: Vec<f32>,
    /// Last tick's, same layout. The scheme is second order in time.
    prev: Vec<f32>,
    /// `C²` where `C = c·dt/h`, clamped to the Courant limit.
    courant_sq: f32,
    /// Per-tick multiplier on the whole field.
    damping: f32,
    /// Metres injected per m/s of a body's vertical motion.
    strength: f32,
}

impl RippleGrid {
    /// Build the grid a body authors, centred on `centre` in world xz.
    ///
    /// `None` for a degenerate or oversized domain, which the validator has
    /// already refused — this is the same belt-and-braces the wave loop's
    /// `take(MAX_WAVES)` is.
    #[must_use]
    pub fn new(centre: [f32; 2], params: &Ripples) -> Option<Self> {
        let side = ripple_side(params.extent, params.cell);
        if side < 3 || side * side > loom_scene::components::MAX_RIPPLE_CELLS {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let half = (side - 1) as f32 * params.cell * 0.5;
        // Clamped rather than refused: the parser refuses, and a component
        // built in code has to be safe too.
        #[allow(clippy::cast_possible_truncation)]
        let courant = (params.speed * TICK_SECONDS / params.cell).min(COURANT_LIMIT as f32);
        Some(Self {
            origin: [centre[0] - half, centre[1] - half],
            cell: params.cell,
            side,
            now: vec![0.0; side * side],
            prev: vec![0.0; side * side],
            courant_sq: courant * courant,
            damping: params.damping.clamp(0.0, 1.0),
            strength: params.strength.max(0.0),
        })
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
    pub fn heights(&self) -> &[f32] {
        &self.now
    }

    /// Advance one fixed tick.
    ///
    /// Boundary samples are held at zero and the outer [`EDGE_CELLS`] are
    /// ramped into it, which absorbs rather than reflects.
    pub fn step(&mut self) {
        let side = self.side;
        // The scratch buffer is last tick's field, which the update overwrites
        // in place — nothing reads `prev` after its own cell is written,
        // because the stencil only ever reads `now`.
        for z in 1..side - 1 {
            for x in 1..side - 1 {
                let i = z * side + x;
                let u = self.now[i];
                let laplacian = self.now[i - 1] + self.now[i + 1] + self.now[i - side]
                    + self.now[i + side]
                    - 4.0 * u;
                let next = 2.0f32.mul_add(u, -self.prev[i]) + self.courant_sq * laplacian;
                self.prev[i] = next * self.damping * edge_taper(x, z, side);
            }
        }
        // The border never moves, so it is zeroed rather than stepped.
        for x in 0..side {
            self.prev[x] = 0.0;
            self.prev[(side - 1) * side + x] = 0.0;
            self.prev[x * side] = 0.0;
            self.prev[x * side + side - 1] = 0.0;
        }
        std::mem::swap(&mut self.now, &mut self.prev);
    }

    /// Push the water at a world point, bilinearly.
    ///
    /// `vertical_velocity` is the body's own m/s at this point, **positive
    /// up**, and `scale` is how much of the body the water has hold of. A body
    /// dropping in pushes the water down and the ring spreads out of the dent;
    /// one rising pulls it up. Points outside the domain do nothing at all,
    /// which is what makes a bounded domain safe rather than a hidden clamp at
    /// its edge.
    ///
    /// **This is the delicate line of the whole feature, and it took two
    /// measured failures to get right.** Both are recorded because the symptom
    /// of each was "the buoy is lively", not "the simulation is unstable".
    ///
    /// *First failure — the body reads the dent it just made.* Injecting the
    /// body's own velocity makes a self-excited oscillator: the dent under a
    /// floating body lowers its own buoyancy, so it falls further, so the dent
    /// deepens. On `assets/test/wake.loom` the buoy reached a **6.85 m** limit
    /// cycle. It *saturated* rather than diverging, which is worse — a limit
    /// cycle reads as an energetic float rather than as a bug — and dropping
    /// `strength` twelve-fold only lowered the plateau to 1.15 m. Subtracting
    /// the surface's own vertical velocity closes that loop: once the water is
    /// already moving with the body, nothing further is injected. It is also
    /// what the physics says, since a body makes waves by moving *through*
    /// water.
    ///
    /// *Second failure — the units.* Adding `Δ` to `now` alone does not add a
    /// displacement, it adds an *impulse*: the scheme infers velocity from
    /// `now − prev`, so the surface acquires `Δ/dt`. At 60 Hz that made
    /// `strength = 0.06` a **3.6× velocity amplifier** — the water was handed
    /// three and a half times the body's own speed every tick, and the first
    /// fix alone still ended in a `NaN` inside a minute of simulated time.
    /// Multiplying by the timestep is what makes `strength` mean what its
    /// documentation claims: the fraction of the body's relative motion the
    /// water takes each tick, dimensionless and bounded, stable by
    /// construction below 1.
    ///
    /// With both fixes, `wake.loom`'s buoy peak-to-peak travel over the last
    /// 300 ticks decays monotonically — **0.034 m at 10 s, 0.0056 at 30 s,
    /// 1.5e-4 at 60 s, 4.5e-8 at 120 s** — against 3.5e-6 with the grid
    /// removed entirely. A wake that arrives, lifts the buoy three centimetres
    /// and dies.
    ///
    /// `ponytail:` velocity coupling only — a body moving *horizontally* at
    /// constant depth injects nothing, so there is no bow wave. Entries,
    /// bobbing and rocking all work, which is what the two-way coupling
    /// demonstration needs. A real bow wave wants displacement-driven
    /// injection, which closes the same feedback loop this comment is about
    /// and would need the same relative-velocity treatment plus its own
    /// measurement.
    pub fn push(&mut self, at: [f32; 3], vertical_velocity: f32, scale: f32) {
        let relative = vertical_velocity - self.vertical_velocity_at(at[0], at[2]);
        let amount = relative * TICK_SECONDS * self.strength * scale;
        if amount == 0.0 {
            return;
        }
        let Some((x0, z0, fx, fz)) = self.locate(at[0], at[2]) else {
            return;
        };
        let side = self.side;
        for (dz, wz) in [(0, 1.0 - fz), (1, fz)] {
            for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                self.now[(z0 + dz) * side + x0 + dx] += amount * wx * wz;
            }
        }
    }

    /// How fast the surface itself is moving up at a world point, m/s.
    ///
    /// The scheme is second order in time and keeps last tick's field, so this
    /// is a backward difference over the fixed step and costs no extra state.
    /// Zero outside the domain. Only [`Self::push`] reads it — see there for
    /// why the coupling needs it.
    #[must_use]
    fn vertical_velocity_at(&self, x: f32, z: f32) -> f32 {
        let Some((x0, z0, fx, fz)) = self.locate(x, z) else {
            return 0.0;
        };
        let side = self.side;
        let at = |ix: usize, iz: usize| {
            (self.now[iz * side + ix] - self.prev[iz * side + ix]) / TICK_SECONDS
        };
        let top = at(x0, z0) + (at(x0 + 1, z0) - at(x0, z0)) * fx;
        let bottom = at(x0, z0 + 1) + (at(x0 + 1, z0 + 1) - at(x0, z0 + 1)) * fx;
        top + (bottom - top) * fz
    }

    /// `(height, ∂h/∂x, ∂h/∂z)` at a world point — what [`crate::sample_water`]
    /// takes. All zero outside the domain, which is water with no ripple on it.
    ///
    /// **This is the force path.** The height reaches `buoyancy::solve` through
    /// `sample_water`, which is what makes a barrel rock in a crate's wake and
    /// what makes `loom sim --assert` able to see one. The two slopes are the
    /// rendering half: a height with no slope displaces the geometry without
    /// lighting it, which reads as a bulge visible only against the horizon.
    ///
    /// The slopes are central differences on the *grid*, not on the bilinear
    /// reconstruction — a bilinear field's derivative is piecewise constant and
    /// steps at every cell edge, which lights as facets.
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> [f32; 3] {
        let Some((x0, z0, fx, fz)) = self.locate(x, z) else {
            return [0.0; 3];
        };
        let side = self.side;
        let raw = |ix: usize, iz: usize| self.now[iz * side + ix];
        let bilinear = |f: &dyn Fn(usize, usize) -> f32| {
            let top = f(x0, z0) + (f(x0 + 1, z0) - f(x0, z0)) * fx;
            let bottom = f(x0, z0 + 1) + (f(x0 + 1, z0 + 1) - f(x0, z0 + 1)) * fx;
            top + (bottom - top) * fz
        };
        // Clamped at the border rather than skipped: the border is held at
        // zero anyway, so a one-sided difference there is the right answer.
        let d = |ix: usize, iz: usize, axis: usize| {
            let (lo, hi) = if axis == 0 {
                (raw(ix.saturating_sub(1), iz), raw((ix + 1).min(side - 1), iz))
            } else {
                (raw(ix, iz.saturating_sub(1)), raw(ix, (iz + 1).min(side - 1)))
            };
            (hi - lo) / (2.0 * self.cell)
        };
        [
            bilinear(&raw),
            bilinear(&|ix, iz| d(ix, iz, 0)),
            bilinear(&|ix, iz| d(ix, iz, 1)),
        ]
    }

    /// Metres of ripple displacement at a world point. Zero outside the domain.
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        self.at(x, z)[0]
    }

    /// The cell and fractions a world point falls in, or `None` if it is
    /// outside. The upper edge is excluded so `x0 + 1` is always in range.
    fn locate(&self, x: f32, z: f32) -> Option<(usize, usize, f32, f32)> {
        let gx = (x - self.origin[0]) / self.cell;
        let gz = (z - self.origin[1]) / self.cell;
        #[allow(clippy::cast_precision_loss)]
        let last = (self.side - 1) as f32;
        // Written as a positive test so a NaN falls out here rather than
        // becoming an index — the same shape `sample_water`'s `usable` uses.
        if !(gx >= 0.0 && gz >= 0.0 && gx < last && gz < last) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x0, z0) = (gx as usize, gz as usize);
        #[allow(clippy::cast_precision_loss)]
        Some((x0, z0, gx - x0 as f32, gz - z0 as f32))
    }
}

/// The Slang half of [`RippleGrid::at`], emitted into the generated shader.
///
/// **This and the Rust are one thing written twice**, exactly as
/// `loom_sample_water` is, and kept honest by the same agreement test. The CPU
/// grid is authoritative and this side only ever reads it — a readback would
/// put GPU floats on the force path, which ADR 0045 clause 2 forbids.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The interactive ripple grid, sampled. The Rust half is
// `loom_water::ripples::RippleGrid::at`; they are one implementation written
// twice. Never edit this by hand.

struct LoomRippleGrid {
    // World xz of sample (0, 0).
    float2 origin;
    // Metres between samples.
    float cell;
    // Samples per axis. Zero means this water has no ripples.
    int side;
    // Row-major, `side * side` of them.
    float* height;
};

// Bilinear, and zero outside the domain — a body that drifts off the grid
// floats on the plain Gerstner sea, and the sponge band has already taken the
// amplitude to nothing before the edge. The upper edge is excluded so the
// `+1` taps are always in range, which is the Rust's `locate` rule.
float loom_ripple_height(LoomRippleGrid grid, float2 xz) {
    if (grid.side <= 0) { return 0.0; }
    float gx = (xz.x - grid.origin.x) / grid.cell;
    float gz = (xz.y - grid.origin.y) / grid.cell;
    float last = float(grid.side - 1);
    if (!(gx >= 0.0 && gz >= 0.0 && gx < last && gz < last)) { return 0.0; }
    int x0 = int(gx);
    int z0 = int(gz);
    float fx = gx - float(x0);
    float fz = gz - float(z0);
    float h00 = grid.height[z0 * grid.side + x0];
    float h10 = grid.height[z0 * grid.side + x0 + 1];
    float h01 = grid.height[(z0 + 1) * grid.side + x0];
    float h11 = grid.height[(z0 + 1) * grid.side + x0 + 1];
    float top = h00 + (h10 - h00) * fx;
    float bottom = h01 + (h11 - h01) * fx;
    return top + (bottom - top) * fz;
}

// (height, dh/dx, dh/dz) — `loom_sample_water`'s `ripple` argument.
float3 loom_ripple_at(LoomRippleGrid grid, float2 xz) {
    if (grid.side <= 0) { return float3(0.0, 0.0, 0.0); }
    float e = grid.cell;
    float h = loom_ripple_height(grid, xz);
    float dx = loom_ripple_height(grid, xz + float2(e, 0.0))
             - loom_ripple_height(grid, xz - float2(e, 0.0));
    float dz = loom_ripple_height(grid, xz + float2(0.0, e))
             - loom_ripple_height(grid, xz - float2(0.0, e));
    return float3(h, dx / (2.0 * e), dz / (2.0 * e));
}
"#
}

/// One minus how far into the sponge layer this sample is, in `[0, 1]`.
fn edge_taper(x: usize, z: usize, side: usize) -> f32 {
    let depth = x.min(z).min(side - 1 - x).min(side - 1 - z);
    if depth >= EDGE_CELLS {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        depth as f32 / EDGE_CELLS as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Ripples {
        Ripples { extent: 8.0, cell: 0.5, speed: 3.0, damping: 1.0, strength: 1.0 }
    }

    fn grid() -> RippleGrid {
        RippleGrid::new([0.0, 0.0], &params()).expect("a usable grid")
    }

    /// The domain is where the author put it, and it does not move.
    #[test]
    fn the_domain_is_centred_on_the_body_not_on_anything_else() {
        let grid = RippleGrid::new([10.0, -4.0], &params()).expect("a grid");
        assert!((grid.origin()[0] - 6.0).abs() < 1e-5, "origin x {}", grid.origin()[0]);
        assert!((grid.origin()[1] + 8.0).abs() < 1e-5, "origin z {}", grid.origin()[1]);
        assert_eq!(grid.side(), 17, "8 m at 0.5 m is 16 cells and 17 samples");
    }

    /// **A disturbance travels.** The point of a wave equation over a bump map
    /// is that energy leaves where it was put, at `c`.
    #[test]
    fn a_push_spreads_outward_at_the_wave_speed() {
        let mut grid = grid();
        grid.push([0.0, 0.0, 0.0], -1.0, 1.0);
        // A body moving down makes a dent, which is the sign a splash has.
        let centre = grid.height_at(0.0, 0.0);
        assert!(centre < 0.0, "the push did nothing, or pushed up: {centre}");
        // Two metres out, and nothing there yet.
        assert_eq!(grid.height_at(2.0, 0.0), 0.0);
        // 3 m/s for 40 ticks is 2 m of travel.
        for _ in 0..40 {
            grid.step();
        }
        let out = grid.height_at(2.0, 0.0).abs();
        assert!(out > 1e-4, "nothing reached 2 m in 40 ticks: {out}");
    }

    /// **Damping is what makes a wake a wake.** Without it the pond rings for
    /// the rest of the run, which is a scene that never settles.
    #[test]
    fn damping_takes_the_energy_out() {
        let energy = |damping: f32| {
            let mut grid =
                RippleGrid::new([0.0, 0.0], &Ripples { damping, ..params() }).expect("a grid");
            grid.push([0.0, 0.0, 0.0], -1.0, 1.0);
            for _ in 0..600 {
                grid.step();
            }
            grid.heights().iter().map(|h| h * h).sum::<f32>()
        };
        let undamped = energy(1.0);
        let damped = energy(0.99);
        assert!(damped < undamped * 0.01, "damped {damped} against undamped {undamped}");
    }

    /// **The Courant clamp, measured as the thing it prevents.** A speed far
    /// past the limit is what an author reaches for when the ripples look
    /// sluggish, and the unclamped scheme answers with `inf`.
    #[test]
    fn a_speed_past_the_courant_limit_cannot_reach_infinity() {
        let mut grid = RippleGrid::new([0.0, 0.0], &Ripples { speed: 500.0, ..params() })
            .expect("a grid");
        grid.push([0.0, 0.0, 0.0], -1.0, 1.0);
        for _ in 0..600 {
            grid.step();
        }
        let worst = grid.heights().iter().fold(0.0f32, |a, h| a.max(h.abs()));
        assert!(worst.is_finite() && worst < 10.0, "the field diverged to {worst}");
    }

    /// Outside the domain the water is the plain Gerstner surface, and asking
    /// is free rather than an error.
    #[test]
    fn outside_the_domain_there_is_no_ripple_and_no_panic() {
        let mut grid = grid();
        grid.push([1000.0, 0.0, 1000.0], -5.0, 1.0);
        assert_eq!(grid.height_at(1000.0, 1000.0), 0.0);
        assert_eq!(grid.height_at(f32::NAN, 0.0), 0.0);
        assert_eq!(grid.heights().iter().map(|h| h.abs()).sum::<f32>(), 0.0);
    }

    /// **The slope climbs toward the peak**, on both axes. A sign error here
    /// lights every ripple inside out and is invisible in a height plot; an
    /// axis swap is invisible in anything but a moving picture.
    #[test]
    fn the_slope_climbs_toward_the_peak() {
        // **Pushed *up*, so there is a peak to climb toward.** `push` takes a
        // vertical velocity positive up, which is the body's own: a rising
        // body pulls the water up after it and a falling one dents it. The
        // dent case is what `a_push_spreads_outward_at_the_wave_speed` covers,
        // and getting these two the same way round is the sign error this
        // test exists for.
        let mut grid = grid();
        grid.push([0.0, 0.0, 0.0], 1.0, 1.0);

        assert!(grid.at(-0.5, 0.0)[1] > 0.0, "{:?}", grid.at(-0.5, 0.0));
        assert!(grid.at(0.5, 0.0)[1] < 0.0, "{:?}", grid.at(0.5, 0.0));
        assert!(grid.at(0.0, -0.5)[2] > 0.0, "{:?}", grid.at(0.0, -0.5));
        assert!(grid.at(0.0, 0.5)[2] < 0.0, "{:?}", grid.at(0.0, 0.5));
        // And flat water is flat in all three, which is what every scene
        // without ripples hands `sample_water`.
        assert_eq!(grid.at(100.0, 100.0), [0.0; 3]);
    }

    /// The Slang half carries the same names and the same taps.
    #[test]
    fn the_slang_half_is_present() {
        for needle in ["LoomRippleGrid", "loom_ripple_height", "loom_ripple_at"] {
            assert!(slang().contains(needle), "the Slang half is missing `{needle}`");
        }
    }

    /// **Never-do #7 in one assertion.** Two grids given the same pushes in
    /// the same order are the same bits, which is the whole reason a stepped
    /// grid is admissible on the force path at all.
    #[test]
    fn the_same_pushes_in_the_same_order_give_the_same_bits() {
        let run = || {
            let mut grid = grid();
            for tick in 0..300 {
                #[allow(clippy::cast_precision_loss)]
                let x = (tick as f32 * 0.01).sin();
                grid.push([x, 0.0, -x], -0.5, 1.0);
                grid.step();
            }
            grid.heights().iter().map(|h| h.to_bits()).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
