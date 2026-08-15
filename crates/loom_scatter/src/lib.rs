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
}

impl Default for Rules {
    fn default() -> Self {
        Self { spacing: 4.0, jitter: 1.0, scale: [0.8, 1.25], seed: 0 }
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

/// Whether the candidate in cell `(ix, iz)` survives elimination.
///
/// **The whole determinism argument lives in this function.** It reads only
/// cells within [`REACH`] of its own, and every cell is a pure function of its
/// coordinates — so the answer does not depend on what has been generated
/// before, or on how the domain was divided up.
fn survives(ix: i32, iz: i32, rules: &Rules) -> bool {
    let (at, priority, _) = candidate(ix, iz, rules);
    let spacing = rules.spacing.max(1e-3);
    for dz in -REACH..=REACH {
        for dx in -REACH..=REACH {
            if dx == 0 && dz == 0 {
                continue;
            }
            let (other, other_priority, _) = candidate(ix + dx, iz + dz, rules);
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
            if !survives(ix, iz, rules) {
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
        Rules { spacing: 3.0, jitter: 1.0, scale: [0.8, 1.25], seed: 7 }
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
