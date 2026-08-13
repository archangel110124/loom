//! Where a river runs, and how fast — read off the ground rather than drawn.
//!
//! Unreal's rivers are splines: an artist draws the path, and the engine writes
//! velocity from the spline's control points into a flow map. Loom does not,
//! for the reason the water doc's §5.8 gives — an agent cannot place spline
//! control points well, and the terrain already knows the answer. Water runs
//! downhill and gathers, so **route it downhill and count what gathers**. The
//! river ends up where a river would be, and an author who tries to make it run
//! up a hill has nothing to type it into.
//!
//! # The two halves, from two different measurements
//!
//! Direction and speed come from different properties of the same grid, and
//! mixing them up gives a visibly worse river either way:
//!
//! - **Speed is the drainage.** The bed's gradient *magnitude* is the wrong
//!   number entirely: it is steepest on the banks, which is where a river is
//!   slowest. What makes water fast is how much of it there is, and that is
//!   what accumulation counts.
//! - **Direction is the neighbourhood's discharge, not the local slope.** This
//!   one was measured rather than reasoned, and the first version got it wrong:
//!   the bed's own downhill direction at a point half a metre off the middle of
//!   a channel points *across* the channel, because the bank's cross-slope is
//!   several times the channel's own fall. A crate straddling the middle then
//!   gets two pontoons pushed toward each other and travels a metre in ten
//!   seconds — measured, on the first draft of `river.loom`.
//!
//!   The fix is the physical one. What carries a floating body is the current
//!   of the channel, which is near enough uniform across its width; the bed's
//!   microstructure is not the water's direction. So the direction at a point
//!   is taken from the **sum of the flow vectors around it** over
//!   [`SMOOTHING_RADIUS`] — and because every one of those vectors is already
//!   scaled by its own drainage, that sum is dominated by the channel and the
//!   two banks' opposing contributions cancel. It fixes the staircase in the
//!   same stroke: D8 has only eight directions, so a crate crossing between two
//!   differently-routed cells would otherwise turn 45° in one tick.
//!
//! # Baked once, sampled bilinearly
//!
//! The alternative — evaluating drainage per query — is not available: flow
//! accumulation is a whole-grid sweep in height order, so there is no closed
//! form to sample at a point the way [`crate::sample_water`] samples a wave.
//! So it is baked at load beside [the height field it is derived
//! from](loom_voxel::heightfield), off the same grid, at the same spacing, and
//! read back with the same bilinear — one velocity per node, interpolated.
//!
//! Baking it also means the *vectors* are what get interpolated, which smooths
//! the direction across a cell rather than snapping at each boundary.
//!
//! **The bed it is derived from is the one baked at load.** Blow the bank out
//! mid-run and the current is the current the old bank made, exactly as the
//! depth is (W6, §5.2). Reloading the scene picks it up.
//!
//! # Determinism
//!
//! This feeds buoyancy, so it is inside the hash. Cells are visited in
//! descending height order with the flat index breaking ties, which is a total
//! order over `f32` including the sentinels; accumulation is added in that
//! order and only that order; and there is no map to iterate.

/// Where a river runs and how fast, one horizontal velocity per grid node.
///
/// Same geometry as `loom_voxel::heightfield::HeightField` — same origin, same
/// spacing, same side — because it is derived from one and the two are sampled
/// at the same points. It is not *stored* alongside it because a scene with no
/// river has no reason to carry one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlowGrid {
    /// World X and Z of node `(0, 0)`.
    pub origin: [f32; 2],
    /// Metres between nodes.
    pub spacing: f32,
    /// Nodes per axis. Zero is a scene with no river, and samples as still.
    pub side: usize,
    /// Horizontal velocity in m/s per node, row-major. Still water is `[0, 0]`.
    velocity: Vec<[f32; 2]>,
}

/// How far, in metres, the direction of the current is averaged over.
///
/// **A calibration knob, and the one number in here that is not read off the
/// ground.** It stands for the width of a channel: the direction at a point is
/// the discharge-weighted sum of the flow around it out to this radius, so it
/// has to reach across the river to pick up the middle of it. Three metres is a
/// stream you could jump. Too small and a body floating off-centre is pushed at
/// the bank instead of downstream — the failure this exists to fix. Too large
/// and two arms of a river a few metres apart average into one direction that
/// belongs to neither.
///
/// It is not authorable, because there is no evidence yet that any scene needs
/// a different one. Promote it to [`loom_scene::components::FlowField`] the
/// first time one does.
pub const SMOOTHING_RADIUS: f32 = 3.0;

/// The eight neighbours, and the reciprocal of the distance to each in units of
/// the grid spacing.
///
/// Fixed order, so the tie between two equally steep descents is broken the
/// same way on every run and every machine.
const NEIGHBOURS: [(isize, isize, f32); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    // 1/√2, written out rather than computed so both the constant and the
    // rounding are visible.
    (-1, -1, 0.707_106_77),
    (1, -1, 0.707_106_77),
    (-1, 1, 0.707_106_77),
    (1, 1, 0.707_106_77),
];

impl FlowGrid {
    /// Derive the current over a height grid.
    ///
    /// `height` is row-major, `side` by `side`, spaced `spacing` metres apart
    /// from `origin` — the layout of `loom_voxel::heightfield::HeightField`.
    /// **A column with no ground is a `NaN`**, not that crate's large negative
    /// sentinel: the sentinel exists so the *shader's* bilinear can stay
    /// branchless, and routing water into a cell a billion metres down would
    /// make one imaginary cell the outlet for the whole map. `NaN` is what the
    /// grass path already converts it to for the same reason. Everything here
    /// tests `is_finite`, so either kind of hole is simply not a cell.
    ///
    /// `speed` is [`loom_scene::components::FlowField::speed`]: metres per
    /// second where the drainage is heaviest.
    #[must_use]
    pub fn bake(
        origin: [f32; 2],
        spacing: f32,
        side: usize,
        height: &[f32],
        speed: f32,
    ) -> Self {
        // Three nodes is the minimum a central difference can be taken in, and
        // a river with no speed is water that does not move — both are empty
        // grids rather than special cases downstream.
        // Phrased positively so a `NaN` fails it rather than sneaking past a
        // negated comparison — the same shape `sample_water`'s `usable` test
        // uses, and for the same reason.
        let usable = speed > 0.0 && spacing > 0.0;
        if side < 3 || height.len() != side * side || !usable {
            return Self::default();
        }
        let accumulation = accumulate(height, side);
        // Normalised by the log rather than linearly, and the difference is the
        // whole length of the river. Accumulation is roughly the catchment area
        // draining through a cell, so it grows by orders of magnitude from the
        // headwaters to the outlet: divided linearly by its maximum, every
        // cell but the last few metres of the river reads as nearly still. The
        // log compresses that into a channel that runs everywhere and runs
        // fastest at the bottom, which is what a river does.
        let peak = accumulation
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .ln_1p();
        let drains = peak > 0.0;
        if !drains {
            return Self::default();
        }

        // **Discharge, as a vector**: how much water goes through this node,
        // pointed the way it goes. Weighted by the raw accumulation and not by
        // the speed derived from it, because this is about to be summed over a
        // neighbourhood and the *ratio* is what decides the vote — the middle
        // of a channel carries a hundred times what a cell on the bank does,
        // and the logarithm that turns discharge into a sensible speed would
        // compress that hundred into a two.
        let mut flux = vec![[0.0_f32; 2]; side * side];
        // Metres per second at each node, from its own drainage. Zero where
        // there is no ground or no slope.
        let mut fast = vec![0.0_f32; side * side];
        for j in 0..side {
            for i in 0..side {
                let index = j * side + i;
                // The bed's own slope, by central differences over whichever
                // neighbours exist — the same stencil the grass path takes its
                // normal from, and for the same reason: the SDF's gradient is
                // quantized to 127 steps across a voxel and is noisy at exactly
                // this scale, whereas the surface is what the water is on.
                let (Some(dx), Some(dz)) = (
                    difference(height, side, i, j, 1, 0, spacing),
                    difference(height, side, i, j, 0, 1, spacing),
                ) else {
                    continue;
                };
                // Downhill is the *negative* gradient. Normalised, because its
                // magnitude is the steepness and the speed comes from the
                // drainage instead.
                let length = (dx * dx + dz * dz).sqrt();
                let sloped = length > 0.0;
                if !sloped {
                    // Dead flat: the water is here but has nowhere to go, which
                    // is a pool rather than a river.
                    continue;
                }
                // Downhill is the *negative* gradient, normalised: its magnitude
                // is the steepness, and the speed comes from the drainage.
                let unit = [-dx / length, -dz / length];
                flux[index] = [unit[0] * accumulation[index], unit[1] * accumulation[index]];
                // Normalised by the log rather than linearly, and the
                // difference is the whole length of the river: accumulation
                // grows by orders of magnitude from the headwaters to the
                // outlet, so dividing by its maximum would leave every cell but
                // the last few metres reading as nearly still.
                fast[index] = speed * accumulation[index].ln_1p() / peak;
            }
        }

        // **Direction from the neighbourhood, speed from the node.** Taking
        // both from the smoothed field would dilute a channel's speed with the
        // dry ground beside it; taking both from the node points a body at the
        // bank. See the module note — this split is the whole correction.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let reach = (SMOOTHING_RADIUS / spacing).round() as usize;
        let smoothed = smooth(&flux, side, reach);
        let velocity = flux
            .iter()
            .zip(&smoothed)
            .zip(&fast)
            .map(|((flux, sum), fast)| {
                let length = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
                if *fast <= 0.0 {
                    [0.0; 2]
                } else if length > 0.0 {
                    [sum[0] / length * fast, sum[1] / length * fast]
                } else {
                    // Nothing around it to take a direction from — a single wet
                    // cell in a dry field. Its own slope is all there is.
                    let own = (flux[0] * flux[0] + flux[1] * flux[1]).sqrt();
                    if own > 0.0 {
                        [flux[0] / own * fast, flux[1] / own * fast]
                    } else {
                        [0.0; 2]
                    }
                }
            })
            .collect();

        Self { origin, spacing, side, velocity }
    }

    /// The current at a world XZ, in m/s. `[0, 0, 0]` off the grid.
    ///
    /// Three components with `y` always zero, because that is the shape
    /// [`crate::sample_water`] adds it to — a river runs along the ground and
    /// has no vertical current in this model, and giving it one would fight the
    /// buoyancy that decides where the surface is.
    ///
    /// **The same bilinear as the height field**, deliberately: the current is
    /// sampled at the same points, off a grid with the same geometry, and two
    /// interpolation schemes over one grid is a difference nobody would ever
    /// trace back.
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> [f32; 3] {
        if self.side < 2 {
            return [0.0; 3];
        }
        let gx = (x - self.origin[0]) / self.spacing;
        let gz = (z - self.origin[1]) / self.spacing;
        #[allow(clippy::cast_precision_loss)]
        let limit = (self.side - 1) as f32;
        if !(gx >= 0.0 && gz >= 0.0 && gx < limit && gz < limit) {
            return [0.0; 3];
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (i, j) = (gx as usize, gz as usize);
        #[allow(clippy::cast_precision_loss)]
        let (fx, fz) = (gx - i as f32, gz - j as f32);

        let node = |i: usize, j: usize| self.velocity[j * self.side + i];
        // Plain multiplies rather than `mul_add`, matching every other
        // interpolation in the water path.
        let lerp = |a: [f32; 2], b: [f32; 2], t: f32| {
            [(b[0] - a[0]) * t + a[0], (b[1] - a[1]) * t + a[1]]
        };
        let low = lerp(node(i, j), node(i + 1, j), fx);
        let high = lerp(node(i, j + 1), node(i + 1, j + 1), fx);
        let v = lerp(low, high, fz);

        // **The current fades out over the grid's last few cells**, and without
        // this it does not fade at all — it stops.
        //
        // Outside the grid there is no terrain, so there is no drainage and no
        // current, and returning zero out there is right. What was wrong was
        // the step: measured on `river.loom`, `flow.x` read 3.000 m/s at
        // x = 47.5 and 0.000 at x = 48. A crate carried downstream did not
        // reach a river mouth and disperse, it hit a wall of still water at the
        // volume's edge and stopped dead — 48.37 m at tick 2400 and 48.36 at
        // tick 3600.
        //
        // This is a discontinuity in a field the *simulation* reads, which is
        // worse than the rendering discontinuities elsewhere in this phase: a
        // body's velocity is integrated, so a step in the force is a visible
        // kink in a trajectory rather than a seam in a picture. Tapering over
        // `EDGE_FADE` cells turns the stop into a deceleration, which is what a
        // river discharging into open sea actually does.
        let edge = self.edge_weight(gx, gz, limit);
        [v[0] * edge, 0.0, v[1] * edge]
    }

    /// How much of the current survives this close to the grid's edge.
    ///
    /// Smoothstep rather than a linear ramp so the *derivative* is continuous
    /// too — a body crossing the taper feels no step in acceleration either.
    fn edge_weight(&self, gx: f32, gz: f32, limit: f32) -> f32 {
        /// Metres over which the current dies at the grid's edge.
        ///
        /// **In metres, not cells, and that distinction was measured.** Four
        /// *cells* at the shipped 0.25 m voxel is one metre, which a crate
        /// moving at 3 m/s crosses in a third of a second — still a stop, just
        /// a shorter one. The taper has to be long enough that a body
        /// decelerates over a time a viewer can see, and that is a distance in
        /// the world rather than a count of samples.
        ///
        /// Four metres is a second and a bit at this scene's current. It is
        /// also comfortably clear of `river.loom`'s drop point at x = 6, so the
        /// crate starts in full flow rather than in the ramp.
        const EDGE_FADE_METRES: f32 = 4.0;
        let fade_cells = (EDGE_FADE_METRES / self.spacing).max(1.0);
        let inset = gx.min(gz).min(limit - gx).min(limit - gz);
        let t = (inset / fade_cells).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The fastest current anywhere on the grid, m/s. For tests and reporting.
    #[must_use]
    pub fn peak_speed(&self) -> f32 {
        self.velocity
            .iter()
            .map(|v| (v[0] * v[0] + v[1] * v[1]).sqrt())
            .fold(0.0, f32::max)
    }
}

/// One central difference of the height field, or `None` where a neighbour is
/// missing on both sides.
///
/// One-sided at the edge of the ground rather than nothing, so the bank of a
/// channel that runs off the edge of the volume still has a direction. Falling
/// back to zero instead would put a stripe of dead water down the boundary.
fn difference(
    height: &[f32],
    side: usize,
    i: usize,
    j: usize,
    di: usize,
    dj: usize,
    spacing: f32,
) -> Option<f32> {
    let at = |i: usize, j: usize| -> Option<f32> {
        let h = *height.get(j.checked_mul(side)?.checked_add(i)?)?;
        (i < side && j < side && h.is_finite()).then_some(h)
    };
    let here = at(i, j)?;
    let plus = at(i + di, j + dj);
    let minus = i.checked_sub(di).zip(j.checked_sub(dj)).and_then(|(a, b)| at(a, b));
    match (plus, minus) {
        (Some(p), Some(m)) => Some((p - m) / (2.0 * spacing)),
        (Some(p), None) => Some((p - here) / spacing),
        (None, Some(m)) => Some((here - m) / spacing),
        (None, None) => None,
    }
}

/// Sum each node's neighbours within `reach` nodes, separably.
///
/// A box sum rather than a Gaussian, and a *sum* rather than a mean: what comes
/// out is only ever normalised for its direction, so the weighting that matters
/// is the one already in the vectors — each is its node's discharge, so the
/// middle of a channel outvotes the trickle on the bank by the ratio of the
/// water in them. Separable because a 25-wide window over a 193² grid is 1.8
/// million adds this way and 23 million the other, at load, for the same answer.
///
/// Accumulation is float addition in a fixed order here as everywhere else: the
/// rows run in index order and the window runs low to high.
fn smooth(field: &[[f32; 2]], side: usize, reach: usize) -> Vec<[f32; 2]> {
    if reach == 0 {
        return field.to_vec();
    }
    let mut pass = vec![[0.0_f32; 2]; field.len()];
    for j in 0..side {
        for i in 0..side {
            let (mut x, mut z) = (0.0_f32, 0.0_f32);
            for n in i.saturating_sub(reach)..=(i + reach).min(side - 1) {
                let v = field[j * side + n];
                x += v[0];
                z += v[1];
            }
            pass[j * side + i] = [x, z];
        }
    }
    let mut out = vec![[0.0_f32; 2]; field.len()];
    for j in 0..side {
        for i in 0..side {
            let (mut x, mut z) = (0.0_f32, 0.0_f32);
            for n in j.saturating_sub(reach)..=(j + reach).min(side - 1) {
                let v = pass[n * side + i];
                x += v[0];
                z += v[1];
            }
            out[j * side + i] = [x, z];
        }
    }
    out
}

/// D8 flow accumulation: how much of the map drains through each cell.
///
/// Every cell starts with its own unit of rain and passes the whole of what it
/// holds to its steepest-descending neighbour. Visiting in descending height
/// order is what makes one pass enough — a cell can only receive from cells
/// above it, and every one of those has already been visited.
///
/// **Single-flow-direction (D8), not D-infinity.** D8 puts the whole of a
/// cell's water into one neighbour, which draws slightly thinner channels than
/// splitting it between two would; the direction the river is *drawn* in does
/// not come from here at all (see the module note), so the extra term would
/// only smooth a number that is about to be run through a logarithm.
///
/// A cell with no lower neighbour — a pit, or a dead-flat plateau — keeps what
/// it has. Pit-filling is what a hydrology library would do next and is left
/// out deliberately: a filled pit is a lake, and a lake in this engine is a
/// `WaterBody`, authored.
fn accumulate(height: &[f32], side: usize) -> Vec<f32> {
    let mut accumulation: Vec<f32> = height
        .iter()
        .map(|h| if h.is_finite() { 1.0 } else { 0.0 })
        .collect();

    // Descending height, with the flat index breaking ties. **The tiebreak is
    // not cosmetic**: a flat plateau is thousands of exactly equal heights, and
    // an unstable sort left to its own devices would order them differently
    // between builds and put a different number in the hash.
    let mut order: Vec<usize> = (0..height.len()).filter(|i| height[*i].is_finite()).collect();
    order.sort_unstable_by(|a, b| height[*b].total_cmp(&height[*a]).then(a.cmp(b)));

    for index in order {
        let (i, j) = (index % side, index / side);
        let here = height[index];
        let mut best: Option<(usize, f32)> = None;
        for (di, dj, reciprocal) in NEIGHBOURS {
            let Some((ni, nj)) = neighbour(i, j, di, dj, side) else {
                continue;
            };
            let there = height[nj * side + ni];
            if !there.is_finite() || there >= here {
                continue;
            }
            // Slope, not drop: the diagonal neighbour is √2 further away, and
            // ignoring that biases every channel toward running diagonally.
            let slope = (here - there) * reciprocal;
            if best.is_none_or(|(_, steepest)| slope > steepest) {
                best = Some((nj * side + ni, slope));
            }
        }
        if let Some((into, _)) = best {
            accumulation[into] += accumulation[index];
        }
    }
    accumulation
}

/// A neighbour's coordinates, or `None` off the grid.
fn neighbour(i: usize, j: usize, di: isize, dj: isize, side: usize) -> Option<(usize, usize)> {
    let ni = i.checked_add_signed(di)?;
    let nj = j.checked_add_signed(dj)?;
    (ni < side && nj < side).then_some((ni, nj))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `side`-square grid whose height is a function of its coordinates.
    fn grid(side: usize, height: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        (0..side * side).map(|k| height(k % side, k / side)).collect()
    }

    /// A plane tilted down toward +X. The current has to run down it.
    #[test]
    fn water_runs_downhill() {
        let side = 16;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, _| 10.0 - i as f32 * 0.1);
        let flow = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 2.0);

        let v = flow.at(7.5, 7.5);
        assert!(v[0] > 0.1, "the current does not run downhill: {v:?}");
        assert!(v[0].abs() > v[2].abs() * 10.0, "it runs sideways: {v:?}");
        assert_eq!(v[1], 0.0, "a river has no vertical current");
    }

    /// **The point of accumulation rather than slope.** A valley floor and its
    /// banks have the same downhill direction and wildly different amounts of
    /// water; the banks are *steeper*, so a current taken from the gradient's
    /// magnitude would run fastest where a river is slowest.
    #[test]
    fn the_channel_runs_faster_than_its_banks() {
        let side = 32;
        let middle = 16.0_f32;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, j| {
            // A V-shaped valley draining toward +X, banks rising steeply away
            // from the centreline.
            10.0 - i as f32 * 0.05 + (j as f32 - middle).abs() * 0.4
        });
        let flow = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 2.0);

        let channel = flow.at(24.0, middle);
        let bank = flow.at(24.0, middle + 6.0);
        let fast = |v: [f32; 3]| (v[0] * v[0] + v[2] * v[2]).sqrt();
        assert!(
            fast(channel) > fast(bank) * 2.0,
            "the channel ({channel:?}) is not faster than the bank ({bank:?})"
        );
        assert!(channel[0] > 0.0, "the channel runs the wrong way: {channel:?}");
    }

    /// **The bug this module was rewritten for.** Half a metre off the middle
    /// of a channel, the bed's own downhill direction points at the bank, not
    /// downstream: the cross-slope of a channel is several times its fall. A
    /// crate straddling the middle then gets its two pontoons pushed toward
    /// each other and goes nowhere, which is exactly what `river.loom` measured
    /// — a metre of travel in ten seconds.
    #[test]
    fn a_body_off_the_middle_is_still_pushed_downstream() {
        // A parabolic channel two metres wide, falling 1 in 20 toward +X — a
        // cross-slope five times the fall, which is a mild river.
        let side = 64;
        let middle = 32.0_f32;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, j| {
            let across = (j as f32 - middle) * 0.25;
            10.0 - i as f32 * 0.25 * 0.05 + across * across * 0.5
        });
        let flow = FlowGrid::bake([0.0, 0.0], 0.25, side, &heights, 3.0);

        // Two pontoons of a crate straddling the middle, half a metre apart.
        for offset in [-0.5, 0.5] {
            let v = flow.at(8.0, middle * 0.25 + offset);
            assert!(
                v[0] > 4.0 * v[2].abs(),
                "the pontoon at {offset} m off the middle is pushed sideways: {v:?}"
            );
        }
    }

    /// Speed scales with what was authored, and zero is still water — which is
    /// the mutation the river scene's assertion is checked against.
    #[test]
    fn the_authored_speed_is_the_scale() {
        let side = 16;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, _| 10.0 - i as f32 * 0.1);
        let fast = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 4.0);
        let slow = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 2.0);
        let still = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 0.0);

        let at = |g: &FlowGrid| g.at(7.5, 7.5)[0];
        assert!((at(&fast) - 2.0 * at(&slow)).abs() < 1e-5, "speed is not linear");
        assert_eq!(still.at(7.5, 7.5), [0.0; 3], "zero speed is not still");
        assert!(fast.peak_speed() <= 4.0 + 1e-5, "the peak exceeds the authored speed");
    }

    /// A hole in the terrain is not an outlet. Routing into the height field's
    /// sentinel would make one imaginary cell drain the whole map; `NaN` is how
    /// "no ground" arrives here, and it must simply not be a cell.
    #[test]
    fn a_hole_in_the_ground_is_not_a_cell() {
        let side = 16;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, j| {
            if j < 4 { f32::NAN } else { 10.0 - i as f32 * 0.1 }
        });
        let flow = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 2.0);

        for j in 0..3 {
            #[allow(clippy::cast_precision_loss)]
            let v = flow.at(7.5, j as f32);
            assert_eq!(v, [0.0; 3], "the hole at row {j} has a current: {v:?}");
        }
        assert!(flow.at(7.5, 9.5)[0] > 0.1, "the ground beside it stopped flowing");
    }

    /// Off the grid is still water, not the nearest edge — the same answer the
    /// height field gives, and for the same reason: a river 200 m away is not
    /// this river.
    #[test]
    fn off_the_grid_is_still() {
        let side = 16;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, _| 10.0 - i as f32 * 0.1);
        let flow = FlowGrid::bake([0.0, 0.0], 1.0, side, &heights, 2.0);

        for (x, z) in [(-1.0, 8.0), (8.0, 900.0), (15.5, 8.0)] {
            assert_eq!(flow.at(x, z), [0.0; 3], "({x}, {z}) is not off the grid");
        }
    }

    /// **The hash depends on this.** Accumulation adds floats in visiting
    /// order, and the order is decided by a sort over heights that are exactly
    /// equal across a plateau — so the tiebreak has to make the whole bake a
    /// function of its input and nothing else.
    #[test]
    fn the_bake_is_reproducible() {
        let side = 24;
        #[allow(clippy::cast_precision_loss)]
        let heights = grid(side, |i, j| {
            // A plateau with a channel cut through it: thousands of exactly
            // equal heights, which is what a sort can reorder.
            if j == 12 { 5.0 - i as f32 * 0.1 } else { 8.0 }
        });
        let a = FlowGrid::bake([0.0, 0.0], 0.5, side, &heights, 3.0);
        let b = FlowGrid::bake([0.0, 0.0], 0.5, side, &heights, 3.0);
        assert_eq!(a, b, "two bakes of one grid disagree");
    }

    /// A grid too small to take a difference in, or one whose length does not
    /// match its side, is still water rather than a panic.
    #[test]
    fn a_degenerate_grid_is_still() {
        assert_eq!(FlowGrid::bake([0.0; 2], 1.0, 2, &[0.0; 4], 2.0).at(0.5, 0.5), [0.0; 3]);
        assert_eq!(FlowGrid::bake([0.0; 2], 1.0, 8, &[0.0; 4], 2.0).at(0.5, 0.5), [0.0; 3]);
        assert_eq!(FlowGrid::bake([0.0; 2], 0.0, 8, &[0.0; 64], 2.0).at(0.5, 0.5), [0.0; 3]);
        assert_eq!(FlowGrid::default().at(0.0, 0.0), [0.0; 3]);
    }
}
