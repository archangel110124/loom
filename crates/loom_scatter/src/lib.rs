//! Deterministic scatter: where a million things go, as a function rather than
//! as a list.
//!
//! Phase 5's first slice. Nothing here reads a scene or a heightmap; it answers
//! "what is at this patch of ground" from the coordinates alone, which is what
//! makes the rest of the phase possible.
//!
//! # The algorithm is not Bridson, and that is deliberate
//!
//! The implementation order asks for "deterministic Bridson Poisson-disk (fixed
//! RNG stream and fixed active-list pop order — the vanilla algorithm pops
//! randomly and will silently break your hashes)". Fixing the pop order does
//! make Bridson reproducible. It does **not** make it order-*independent*, and
//! that is the property the phase actually needs:
//!
//! > Dirty-region incremental regeneration over destructible voxels. […] a CSG
//! > op marks a scatter cell dirty, only that cell regenerates, and because
//! > seeds are position-derived the result is identical to a full regen with no
//! > seams.
//!
//! Bridson grows a point set from an active list: whether a candidate is
//! accepted depends on which points were accepted before it, across the whole
//! domain. Regenerating one cell in isolation cannot see that history, so its
//! output differs from the full run — not by a seam, but everywhere in the
//! cell. Position-derived seeds cannot fix it, because what varies is not the
//! random numbers but *which candidates survive*.
//!
//! So this is Poisson-disk **by elimination**: every grid cell proposes one
//! jittered candidate, every candidate carries a priority, and a candidate
//! survives if no higher-priority candidate lies within the radius. Acceptance
//! depends only on a 7×7 neighbourhood of cells, each of which is a pure
//! function of its own coordinates — so a region regenerated alone is
//! bit-identical to the same region inside a full run, which is the exit
//! criterion, and the whole thing is embarrassingly parallel besides.
//!
//! # What that costs, stated as a number rather than as "slightly"
//!
//! Eliminating every candidate *simultaneously* against every other is a Matérn
//! type II hard-core process, and its limiting intensity is exactly `1/(pi r²)`
//! — about **31% of hexagonal packing**, against roughly 65% for Bridson. The
//! test below pins the measured density against that analytic value rather than
//! against packing, because 31% is not a shortfall to be tuned away: it is what
//! simultaneous elimination is.
//!
//! In practice that means instances average about 1.8x the *minimum* spacing
//! apart. The minimum itself is guaranteed exactly as hard as Bridson's.
//!
//! **Doubling it is possible and is not free.** A Matérn type III process —
//! accept unless a higher-priority *accepted* point is within the radius —
//! reaches about 55-65% of packing and is still order-independent, because the
//! order is priority and priority is position-derived. But acceptance becomes
//! recursive: deciding one cell needs its neighbours' decisions, which need
//! theirs. That is a bounded-depth search rather than a fixed 7×7 scan, and it
//! is worth doing when a scene wants the density, not before.
//!
//! # Everything is seeded from quantized position
//!
//! Never from generation order, and never from an index. That is item 2 of the
//! phase and it is what the paragraph above rests on.

use loom_field::noise::hash;

/// What the ground is doing where an instance would stand.
///
/// **The same four fields `loom_grass::Ground` carries, declared again rather
/// than imported.** Scatter is the more general system of the two and should
/// not depend on grass to describe a hillside; the caller converts, which is
/// four lines. If a third reader ever wants this shape it moves somewhere both
/// can see it — two is not yet a pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ground {
    /// Surface height, metres.
    pub height: f32,
    /// Surface normal. Its Y component is the slope test.
    pub normal: [f32; 3],
    /// `1` bare rock or no ground at all, `0` soil.
    pub rock: f32,
    /// Flow accumulation, normalised. High in gullies, low on ridges.
    pub flow: f32,
}

impl Default for Ground {
    fn default() -> Self {
        Self { height: 0.0, normal: [0.0, 1.0, 0.0], rock: 0.0, flow: 0.0 }
    }
}

/// A scatter instance: where, which way, how big.
///
/// **No identity and no index.** An instance is not a thing that persists — it
/// is a value derived from a position, and the same position always yields the
/// same instance. That is what lets a scene hold "pine_forest — 1.2M instances"
/// as one node instead of a million (the phase's hierarchy rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    /// World position on the XZ plane. The caller lifts it onto the ground.
    pub at: [f32; 2],
    /// Facing, radians.
    pub yaw: f32,
    /// Uniform scale, around 1.
    pub scale: f32,
    /// **The instance's own random stream.** Derived from its quantized
    /// position, so anything downstream that wants more variation — a mesh
    /// variant, a tint, a lean — draws from here and stays order-independent
    /// with it.
    pub seed: u32,
}

/// What a scatter rule asks for, before the ground has its say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Minimum metres between any two instances. **Guaranteed**, not average.
    pub spacing: f32,
    /// How far a candidate may move inside its cell, 0 to 1.
    ///
    /// Zero is a lattice, which reads as a plantation. One fills the cell and
    /// is what makes the result look scattered rather than planted — the
    /// spacing guarantee holds either way, because it is enforced after the
    /// jitter rather than by the grid.
    pub jitter: f32,
    /// Scale range, multiplied onto whatever the instance's mesh is.
    pub scale: [f32; 2],
    /// The rule's own seed, so two rules over the same ground do not stack
    /// their instances on top of each other.
    pub seed: u32,
    /// Steepest ground anything will stand on, in degrees.
    ///
    /// The phase's exit criterion names 22 degrees. Instances thin out over the
    /// last few degrees rather than stopping at a line, for the reason grass
    /// learned the hard way: a hard cutoff on slope draws a clean curve across
    /// a hillside, and a clean curve is the synthetic tell.
    pub max_slope_degrees: f32,
    /// How many degrees the thinning is spread over, below the limit.
    pub slope_fade: f32,
    /// Height band anything will grow in. Infinite by default.
    pub altitude: [f32; 2],
    /// How much the ground's moisture matters, 0 (not at all) to 1 (only grows
    /// where water collects).
    pub moisture: f32,
    /// Overall thinning, 0 to 1. What a rule turns down to make a copse rather
    /// than a forest without changing how far apart the trees are.
    pub density: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            spacing: 4.0,
            jitter: 1.0,
            scale: [0.8, 1.25],
            seed: 0,
            max_slope_degrees: 22.0,
            slope_fade: 6.0,
            altitude: [f32::NEG_INFINITY, f32::INFINITY],
            moisture: 0.0,
            density: 1.0,
        }
    }
}

/// Cell side for a given spacing.
///
/// **Half the spacing, not `spacing / sqrt(2)`.** The larger cell is the
/// textbook choice — it is the biggest that can hold at most one accepted point
/// — and it makes the result far too sparse: one candidate per 4.5 m² cell
/// gives elimination almost nothing to choose between, and the measured density
/// came out at 30% of hexagonal packing where Bridson reaches about 65%.
///
/// Halving it doubles the candidate density in each axis, so elimination picks
/// the best of four times as many. It costs a wider neighbourhood scan — see
/// [`REACH`] — and that cost is a bake-time one.
fn cell_size(spacing: f32) -> f32 {
    spacing.max(1e-3) * 0.5
}

/// How many cells out the elimination test looks, in each direction.
///
/// A candidate can sit anywhere in its cell, so two candidates `REACH` cells
/// apart are at least `(REACH - 1) * cell_size` apart. With `cell_size` at half
/// the spacing, three cells guarantees everything within one spacing is seen.
const REACH: i32 = 3;

/// The candidate a cell proposes, or `None` where the cell is empty.
///
/// **One candidate per cell, always.** Density is controlled by the caller
/// rejecting instances (slope, flow, a mask), not by cells declining to
/// propose — a cell that sometimes proposes nothing would make density a
/// property of the grid rather than of the rules.
fn candidate(ix: i32, iz: i32, rules: &Rules) -> ([f32; 2], u32, u32) {
    #[allow(clippy::cast_sign_loss)]
    let base = hash(
        hash(hash(ix as u32).wrapping_add(iz as u32)).wrapping_add(rules.seed),
    );
    // Three independent draws from one hash chain. Each `hash` call is a full
    // mix, so consecutive values are uncorrelated — the same construction
    // `loom_field::noise::lattice` uses.
    let jx = hash(base);
    let jz = hash(jx);
    let priority = hash(jz);

    let unit = |h: u32| f32::from_bits((h >> 8) | 0x3F80_0000) - 1.0;
    let size = cell_size(rules.spacing);
    let jitter = rules.jitter.clamp(0.0, 1.0);
    // Centred jitter, so zero jitter is the cell centre rather than its corner.
    #[allow(clippy::cast_precision_loss)]
    let at = [
        (ix as f32 + 0.5 + (unit(jx) - 0.5) * jitter) * size,
        (iz as f32 + 0.5 + (unit(jz) - 0.5) * jitter) * size,
    ];
    (at, priority, base)
}

/// How readily this ground would carry an instance, 0 to 1.
///
/// A fraction rather than a yes or no, so every boundary is a thinning rather
/// than a line. Grass paid for that lesson twice: a cutoff at any threshold,
/// softened by any amount, still draws a clean curve across a hillside, and a
/// clean curve is what makes ground read as generated.
#[must_use]
pub fn viability(rules: &Rules, ground: &Ground) -> f32 {
    // No ground at all is said as `rock = 1`, the same convention grass uses —
    // so a hole blown clean through the terrain grows nothing, out of the same
    // query rather than as a special case.
    if ground.rock >= 1.0 {
        return 0.0;
    }
    let smoothstep = |edge0: f32, edge1: f32, x: f32| {
        if (edge1 - edge0).abs() < 1e-6 {
            return if x < edge0 { 0.0 } else { 1.0 };
        }
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };

    // Slope from the normal's Y, which is the cosine of the ground's angle.
    let slope = ground.normal[1].clamp(-1.0, 1.0).acos().to_degrees();
    let limit = rules.max_slope_degrees.max(0.0);
    let fade = rules.slope_fade.max(0.0);
    let by_slope = 1.0 - smoothstep(limit - fade, limit, slope);

    // Altitude, faded over a tenth of the band at each end so a treeline is a
    // treeline rather than a contour.
    let [low, high] = rules.altitude;
    let by_altitude = if low.is_finite() || high.is_finite() {
        let span = (high - low).abs().max(1.0) * 0.1;
        let above = if low.is_finite() { smoothstep(low, low + span, ground.height) } else { 1.0 };
        let below = if high.is_finite() { 1.0 - smoothstep(high - span, high, ground.height) } else { 1.0 };
        above * below
    } else {
        1.0
    };

    let by_moisture = 1.0 - rules.moisture.clamp(0.0, 1.0) * (1.0 - ground.flow.clamp(0.0, 1.0));

    (rules.density.clamp(0.0, 1.0) * by_slope * by_altitude * by_moisture).clamp(0.0, 1.0)
}

/// Whether this ground can carry anything at all.
///
/// **The gate applied *before* elimination, and it is deliberately only the
/// zero test.** A candidate the ground refuses outright must not compete, or a
/// cliff sterilises the ground beside it; a candidate on merely *poor* ground
/// must compete normally, and is thinned afterwards instead.
fn habitable(at: [f32; 2], rules: &Rules, ground: &dyn Fn(f32, f32) -> Ground) -> bool {
    viability(rules, &ground(at[0], at[1])) > 0.0
}

/// Whether a survivor is kept, given how good its ground is.
///
/// **Applied *after* elimination, and that is not interchangeable with the gate
/// above.** Matérn II density is set by the disk radius, not by how many
/// candidates competed — so thinning candidates beforehand barely thins the
/// result. Measured: at 18 degrees on a 22-degree limit, pre-thinning produced
/// *more* instances than flat ground (290 against 280), because the survivors
/// simply had less competition. `density = 0.5` would not have halved a forest
/// either.
///
/// Thinning the survivors does what both those knobs are supposed to mean.
/// Position-derived, so it stays order-independent.
fn kept(base: u32, at: [f32; 2], rules: &Rules, ground: &dyn Fn(f32, f32) -> Ground) -> bool {
    let v = viability(rules, &ground(at[0], at[1]));
    if v >= 1.0 {
        return true;
    }
    let roll = f32::from_bits((hash(base ^ 0x2545_F491) >> 8) | 0x3F80_0000) - 1.0;
    roll < v
}

/// Whether the candidate in cell `(ix, iz)` survives elimination.
///
/// **The whole determinism argument lives in this function.** It reads only
/// cells within [`REACH`] of its own, and every cell is a pure function of its
/// coordinates — so the answer does not depend on what has been generated
/// before, or on how the domain was divided up.
fn survives(ix: i32, iz: i32, rules: &Rules, ground: &dyn Fn(f32, f32) -> Ground) -> bool {
    let (at, priority, _) = candidate(ix, iz, rules);
    let spacing = rules.spacing.max(1e-3);
    for dz in -REACH..=REACH {
        for dx in -REACH..=REACH {
            if dx == 0 && dz == 0 {
                continue;
            }
            let (other, other_priority, _) = candidate(ix + dx, iz + dz, rules);
            // **A neighbour the ground rejected does not compete.** This is the
            // ordering decision of this slice, and it is visible: reject after
            // elimination and a cliff sterilises the ground beside it, because
            // the candidates that lost to a doomed neighbour are gone too. The
            // result is a thinned fringe along every boundary — the same
            // artifact as grass's shaved ring, arrived at from the other
            // direction.
            //
            // It costs a ground query per neighbour rather than per instance,
            // which is a bake-time cost and the reason `region_on` exists
            // separately from `region`.
            if !habitable(other, rules, ground) {
                continue;
            }
            // **Ties broken by coordinate, not skipped.** Two cells with equal
            // priority would otherwise both survive or both die depending on
            // which asked — and a hash collision is rare rather than
            // impossible, so "rare" would mean a defect nobody could reproduce.
            let loses = other_priority > priority
                || (other_priority == priority && (dz, dx) < (0, 0));
            if !loses {
                continue;
            }
            let (dx2, dz2) = (other[0] - at[0], other[1] - at[1]);
            if dx2.mul_add(dx2, dz2 * dz2) < spacing * spacing {
                return false;
            }
        }
    }
    true
}

/// Every instance whose position lies in `[min, max)` on the XZ plane.
///
/// **Half-open on purpose.** Two abutting regions tile exactly, with no
/// instance dropped between them and none counted twice — which is what makes
/// the regeneration test below able to compare a patch against the whole.
#[must_use]
pub fn region(min: [f32; 2], max: [f32; 2], rules: &Rules) -> Vec<Instance> {
    region_on(min, max, rules, &|_, _| Ground::default())
}

/// [`region`], with the ground having its say.
///
/// `ground` is a closure rather than a terrain type, which is the seam that
/// keeps this crate free of `loom_voxel` — the same one `loom_grass::tile`
/// uses. It is called for every *candidate* in and around the region, not only
/// for the survivors; see the note in `survives`.
#[must_use]
pub fn region_on(
    min: [f32; 2],
    max: [f32; 2],
    rules: &Rules,
    ground: &dyn Fn(f32, f32) -> Ground,
) -> Vec<Instance> {
    let size = cell_size(rules.spacing);
    // A candidate can be jittered up to half a cell out of its own cell, so the
    // scan reaches one cell beyond the region on every side.
    #[allow(clippy::cast_possible_truncation)]
    let lo = [
        (min[0] / size).floor() as i32 - 1,
        (min[1] / size).floor() as i32 - 1,
    ];
    #[allow(clippy::cast_possible_truncation)]
    let hi = [
        (max[0] / size).ceil() as i32 + 1,
        (max[1] / size).ceil() as i32 + 1,
    ];

    let mut out = Vec::new();
    for iz in lo[1]..=hi[1] {
        for ix in lo[0]..=hi[0] {
            let (at, _, base) = candidate(ix, iz, rules);
            if at[0] < min[0] || at[0] >= max[0] || at[1] < min[1] || at[1] >= max[1] {
                continue;
            }
            if !habitable(at, rules, ground) {
                continue;
            }
            if !survives(ix, iz, rules, ground) {
                continue;
            }
            // Last, so poor ground thins the forest rather than merely
            // reshuffling which candidates won.
            if !kept(base, at, rules, ground) {
                continue;
            }
            let yaw = hash(base ^ 0x51ED_270B);
            let scale = hash(yaw);
            let unit = |h: u32| f32::from_bits((h >> 8) | 0x3F80_0000) - 1.0;
            out.push(Instance {
                at,
                yaw: unit(yaw) * std::f32::consts::TAU,
                scale: rules.scale[0] + (rules.scale[1] - rules.scale[0]) * unit(scale),
                // The instance's stream, from its cell rather than from its
                // position in this vector.
                seed: hash(base ^ 0x9E37_79B9),
            });
        }
    }
    out
}

/// The `i`th point of a Halton sequence in two dimensions, in `[0, 1)²`.
///
/// **For fixed-count coverage**, which Poisson-disk cannot give: elimination
/// answers "as many as fit at this spacing", and some rules want "exactly forty
/// of these, spread out". Halton is the standard answer — low discrepancy,
/// no state, and the `i`th point is the same whatever order they are asked for,
/// which is the same order-independence the rest of this crate is built on.
#[must_use]
pub fn halton(i: u32) -> [f32; 2] {
    fn radical(mut i: u32, base: u32) -> f32 {
        let mut f = 1.0_f32;
        let mut out = 0.0_f32;
        while i > 0 {
            f /= base as f32;
            out += f * (i % base) as f32;
            i /= base;
        }
        out
    }
    // Bases 2 and 3, the usual pair: the first two primes give the best-spread
    // low-dimensional Halton points and need no scrambling at these counts.
    [radical(i + 1, 2), radical(i + 1, 3)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules { spacing: 3.0, jitter: 1.0, seed: 7, ..Rules::default() }
    }

    /// Ground tilted by `degrees`, everywhere.
    fn slope(degrees: f32) -> Ground {
        let r = degrees.to_radians();
        Ground { normal: [r.sin(), r.cos(), 0.0], ..Ground::default() }
    }

    /// The guarantee the whole thing exists for.
    #[test]
    fn no_two_instances_are_closer_than_the_spacing() {
        let r = rules();
        let all = region([-40.0, -40.0], [40.0, 40.0], &r);
        assert!(all.len() > 100, "too few instances to be a real test: {}", all.len());
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                let (dx, dz) = (a.at[0] - b.at[0], a.at[1] - b.at[1]);
                let d = dx.hypot(dz);
                assert!(
                    d >= r.spacing - 1e-4,
                    "{:?} and {:?} are {d} apart, closer than {}",
                    a.at,
                    b.at,
                    r.spacing
                );
            }
        }
    }

    /// **The exit criterion: a patch regenerated alone matches the full run.**
    ///
    /// This is what the elimination algorithm buys and Bridson could not. If it
    /// ever fails, dirty-region regeneration is producing different vegetation
    /// from a full rebuild and the seams will be visible.
    #[test]
    fn a_patch_regenerated_alone_is_identical_to_the_full_run() {
        let r = rules();
        let whole = region([-40.0, -40.0], [40.0, 40.0], &r);
        let patch = region([-6.0, 2.0], [11.0, 19.0], &r);

        let inside: Vec<Instance> = whole
            .into_iter()
            .filter(|i| {
                i.at[0] >= -6.0 && i.at[0] < 11.0 && i.at[1] >= 2.0 && i.at[1] < 19.0
            })
            .collect();

        assert!(!patch.is_empty(), "the patch is empty — the test proves nothing");
        assert_eq!(
            patch, inside,
            "regenerating a patch alone gave different instances from the same \
             patch inside a full run"
        );
    }

    /// Abutting regions tile exactly: no instance lost at the join, none twice.
    #[test]
    fn adjacent_regions_tile_without_gaps_or_duplicates() {
        let r = rules();
        let whole = region([0.0, 0.0], [30.0, 30.0], &r);
        let mut halves = region([0.0, 0.0], [15.0, 30.0], &r);
        halves.extend(region([15.0, 0.0], [30.0, 30.0], &r));

        let key = |i: &Instance| (i.at[0].to_bits(), i.at[1].to_bits());
        let mut a: Vec<_> = whole.iter().map(key).collect();
        let mut b: Vec<_> = halves.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "splitting the region changed which instances exist");
    }

    /// Same input, same output — and different seeds really do differ, or the
    /// seed is decorative.
    #[test]
    fn it_is_deterministic_and_the_seed_matters() {
        let r = rules();
        assert_eq!(region([0.0, 0.0], [20.0, 20.0], &r), region([0.0, 0.0], [20.0, 20.0], &r));

        let other = Rules { seed: 8, ..r };
        assert_ne!(
            region([0.0, 0.0], [20.0, 20.0], &r),
            region([0.0, 0.0], [20.0, 20.0], &other),
            "changing the seed changed nothing"
        );
    }

    /// **Density is pinned to the analytic value, not to a taste threshold.**
    ///
    /// Simultaneous priority elimination is a Matérn type II hard-core process,
    /// whose limiting intensity is `1/(pi r²)`. Measuring against hexagonal
    /// packing — which is what a first draft of this test did — fails for a
    /// process that cannot reach it by construction, and invites "fixing" the
    /// algorithm toward a number it is not allowed to have.
    ///
    /// A real regression shows up here as a *departure* from the analytic
    /// value in either direction: too sparse means the neighbourhood test is
    /// over-rejecting, too dense means the spacing is not being enforced.
    #[test]
    fn density_matches_the_matern_ii_limit() {
        let r = rules();
        let side = 90.0_f32;
        let all = region([0.0, 0.0], [side, side], &r);
        #[allow(clippy::cast_precision_loss)]
        let density = all.len() as f32 / (side * side);
        let matern = 1.0 / (std::f32::consts::PI * r.spacing * r.spacing);
        let ratio = density / matern;
        assert!(
            (0.85..=1.15).contains(&ratio),
            "density {density} per m² is {ratio:.2}x the Matern II limit of {matern}"
        );
        // And well under packing, which is the sanity end of it.
        let packed = 2.0 / (3.0_f32.sqrt() * r.spacing * r.spacing);
        assert!(density < packed, "denser than hexagonal packing, which is impossible");
    }

    /// Jitter must actually move things, or the result is a lattice wearing a
    /// scatter's name.
    #[test]
    fn jitter_breaks_the_lattice() {
        let planted = region([0.0, 0.0], [30.0, 30.0], &Rules { jitter: 0.0, ..rules() });
        let scattered = region([0.0, 0.0], [30.0, 30.0], &rules());

        // **Counting distinct values is the wrong test and the first draft made
        // it**: with jitter almost every survivor has a unique x, so the count
        // is bounded by the number of survivors rather than by the jitter, and
        // the ratio it produces says nothing. Ask the actual question instead —
        // does a point sit at its cell's centre?
        let size = cell_size(rules().spacing);
        let on_lattice = |v: &[Instance]| {
            v.iter()
                .filter(|i| {
                    let cells = i.at[0] / size - 0.5;
                    (cells - cells.round()).abs() < 1e-3
                })
                .count()
        };
        assert_eq!(
            on_lattice(&planted),
            planted.len(),
            "with jitter 0 every instance must sit on the cell lattice"
        );
        assert!(
            on_lattice(&scattered) * 20 < scattered.len(),
            "{} of {} jittered instances are still on the lattice",
            on_lattice(&scattered),
            scattered.len()
        );
    }

    /// The exit criterion's own sentence: under 22 degrees, and not above.
    #[test]
    fn nothing_stands_on_ground_steeper_than_the_limit() {
        let r = rules();
        for degrees in [30.0_f32, 45.0, 70.0] {
            let on = region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| slope(degrees));
            assert!(on.is_empty(), "{degrees} degrees grew {} instances", on.len());
        }
        let gentle = region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| slope(5.0));
        assert!(!gentle.is_empty(), "5 degrees grew nothing");
    }

    /// The slope boundary is a fade, not a line. A rule with only on and off
    /// would draw a clean curve across a hillside, which is the tell grass
    /// spent a slice removing.
    #[test]
    fn the_slope_boundary_thins_rather_than_cutting() {
        let r = rules();
        let count = |d: f32| region_on([0.0, 0.0], [90.0, 90.0], &r, &|_, _| slope(d)).len();
        let (flat, mid, edge) = (count(5.0), count(18.0), count(21.0));
        assert!(flat > mid && mid > edge, "not monotonic: {flat}, {mid}, {edge}");
        assert!(mid > 0, "18 degrees grew nothing at all, so it is a cut not a fade");

        // **Pinned to the curve, not to a threshold I picked.** `viability` is
        // the documented shape of the fade; the count has to follow it, which
        // is a much stronger claim than "fewer than flat ground" and catches a
        // fade that is the wrong shape rather than merely absent.
        let r = rules();
        #[allow(clippy::cast_precision_loss)]
        let measured = mid as f32 / flat as f32;
        let expected = viability(&r, &slope(18.0));
        assert!(
            (measured - expected).abs() < 0.1,
            "18 degrees kept {measured:.2} of flat ground, but the fade curve says {expected:.2}"
        );
    }

    /// **Ground with no soil grows nothing, and that is how "no ground" is
    /// said** — the same convention grass uses, so a hole blown through the
    /// terrain needs no special case.
    #[test]
    fn bare_rock_grows_nothing() {
        let r = rules();
        let bare = Ground { rock: 1.0, ..Ground::default() };
        assert!(region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| bare).is_empty());
    }

    /// **The whole reason ground rejection happens before elimination.**
    ///
    /// Half the world is a cliff. Density in a strip hard against the cliff
    /// must match density in a strip well away from it. Reject *after*
    /// elimination instead and the candidates that lost to a doomed neighbour
    /// are gone too, leaving a thinned fringe along every boundary — the same
    /// artifact as grass's shaved ring, arrived at from the other direction.
    #[test]
    fn a_cliff_does_not_thin_the_ground_beside_it() {
        let r = rules();
        let world = |x: f32, _z: f32| if x > 0.0 { slope(80.0) } else { slope(0.0) };

        // **One spacing wide, and that width is the test.** A twelve-metre
        // strip was tried first and could not see the defect: the sterilised
        // band is only about one spacing across, so measuring it inside a strip
        // four times that wide diluted a 30% hole into a 7% dip, well inside
        // the tolerance. A test that averages over the region where the bug
        // is *not* will pass while the bug is there.
        let near = region_on([-3.0, -240.0], [0.0, 240.0], &r, &world).len();
        let far = region_on([-63.0, -240.0], [-60.0, 240.0], &r, &world).len();
        assert!(near > 0 && far > 0, "one of the strips is empty: {near}, {far}");

        #[allow(clippy::cast_precision_loss)]
        let ratio = near as f32 / far as f32;
        assert!(
            (0.8..=1.25).contains(&ratio),
            "the strip against the cliff has {ratio:.2}x the density of one away \
             from it — the cliff is sterilising the ground beside it"
        );
    }

    /// Moisture confines instances to where water collects.
    #[test]
    fn moisture_prefers_where_water_collects() {
        let r = Rules { moisture: 1.0, ..rules() };
        let dry = region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| Ground::default()).len();
        let wet = region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| Ground {
            flow: 1.0,
            ..Ground::default()
        })
        .len();
        assert_eq!(dry, 0, "with moisture 1 and no flow, nothing should grow");
        assert!(wet > 0, "with moisture 1 and full flow, something must grow");
    }

    /// Altitude bands, for a treeline.
    #[test]
    fn the_altitude_band_is_respected() {
        let r = Rules { altitude: [10.0, 40.0], ..rules() };
        let at = |h: f32| {
            region_on([0.0, 0.0], [60.0, 60.0], &r, &|_, _| Ground {
                height: h,
                ..Ground::default()
            })
            .len()
        };
        assert_eq!(at(0.0), 0, "below the band");
        assert_eq!(at(60.0), 0, "above the band");
        assert!(at(25.0) > 0, "inside the band");
    }

    /// **Order-independence has to survive the ground**, or step 6 is lost.
    /// The same claim as the earlier patch test, now with terrain in the loop.
    #[test]
    fn a_patch_on_real_ground_still_matches_the_full_run() {
        let r = Rules { moisture: 0.4, ..rules() };
        let world = |x: f32, z: f32| Ground {
            height: (x * 0.05).sin() * 8.0 + (z * 0.03).cos() * 5.0,
            normal: {
                let s = ((x * 0.05).cos() * 0.25).atan();
                [s.sin(), s.cos(), 0.0]
            },
            rock: 0.0,
            flow: ((z * 0.04).sin() * 0.5 + 0.5).clamp(0.0, 1.0),
        };

        let whole = region_on([-40.0, -40.0], [40.0, 40.0], &r, &world);
        let patch = region_on([-6.0, 2.0], [11.0, 19.0], &r, &world);
        let inside: Vec<Instance> = whole
            .into_iter()
            .filter(|i| i.at[0] >= -6.0 && i.at[0] < 11.0 && i.at[1] >= 2.0 && i.at[1] < 19.0)
            .collect();
        assert!(!patch.is_empty(), "the patch is empty — the test proves nothing");
        assert_eq!(patch, inside, "the ground made scatter order-dependent");
    }

    /// Halton must cover the square rather than clustering, which is the only
    /// reason to prefer it over a hash.
    #[test]
    fn halton_spreads_over_the_unit_square() {
        let mut buckets = [0_u32; 16];
        for i in 0..256 {
            let [x, y] = halton(i);
            assert!((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y), "{x},{y} escaped");
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let b = ((y * 4.0) as usize) * 4 + (x * 4.0) as usize;
            buckets[b.min(15)] += 1;
        }
        // 256 points over 16 buckets is 16 each; low discrepancy means every
        // bucket is close to that, which a hash would not guarantee.
        for (i, n) in buckets.iter().enumerate() {
            assert!((12..=20).contains(n), "bucket {i} got {n} of 256, expected ~16");
        }
    }
}
