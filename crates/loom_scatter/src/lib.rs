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

/// One named scatter rule in a list.
///
/// **A flat list, evaluated in order, and deliberately not a node graph.** The
/// research the phase rests on settled that empirically: Unreal's PCG graphs
/// are binary-only with no text export and no diff tool, and Epic's own advice
/// is to partition content to avoid merge conflicts rather than resolve them.
/// Blender's Geometry Nodes have the same problem. Two mature ecosystems failed
/// to make a node graph reviewable in a diff, and a named list recovers the two
/// or three genuinely-DAG features without any of that.
#[derive(Debug, Clone, Copy, Default)]
pub struct Layer<'a> {
    /// What later layers call this one.
    pub name: &'a str,
    pub rules: Rules,
    /// Layers this one keeps away from.
    pub exclude: &'a [Exclude<'a>],
    /// The biome this layer belongs to, or `None` for everywhere.
    pub biome: Option<&'a str>,
}

/// A region of the world with its own character, and how strongly it claims it.
///
/// **Priority and bounds, which is all the phase asks for.** Where two biomes
/// overlap the higher priority wins; the loser's layers simply do not place
/// there. That is a scalar comparison rather than a blend of parameters, and it
/// is deliberate: blending two rule *sets* means deciding what the average of
/// "pines at 4 m" and "oaks at 9 m" is, which has no answer.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biome<'a> {
    pub name: &'a str,
    /// Axis-aligned bounds on the XZ plane.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Higher wins where biomes overlap.
    pub priority: f32,
    /// Metres over which membership fades at the edge.
    ///
    /// **The fade is resolved per instance, not by blending densities.** Within
    /// the band an instance belongs with a probability, decided by its own
    /// position hash — so two biomes interleave across the boundary and the
    /// transition is a mixed wood rather than a line with a forest on one side.
    /// A blend that scaled both densities would give a *thin* strip of both,
    /// which reads as a mown verge.
    pub blend: f32,
}

/// How strongly `biome` claims this point, 0 to 1, before priority.
///
/// **The fade reaches *outward* from the bounds, not inward.** Inward was tried
/// first and is wrong in a way that only shows on a map: two biomes that abut
/// then both weaken as they approach the shared edge, leaving a strip that
/// neither claims — a bald line exactly where the transition was supposed to
/// be. Reaching outward makes abutting biomes overlap by twice the blend, which
/// is the transition.
fn claim(biome: &Biome, at: [f32; 2]) -> f32 {
    let blend = biome.blend.max(0.0);
    let axis = |v: f32, lo: f32, hi: f32| {
        if v >= lo && v <= hi {
            return 1.0;
        }
        if blend <= 0.0 {
            return 0.0;
        }
        let out = if v < lo { lo - v } else { v - hi };
        (1.0 - out / blend).clamp(0.0, 1.0)
    };
    axis(at[0], biome.min[0], biome.max[0]) * axis(at[1], biome.min[1], biome.max[1])
}

/// Whether `at` belongs to `name`, given every biome competing for it.
///
/// **Priority is absolute; the fade only settles ties.** The highest priority
/// claiming the point at all takes it outright — a strong biome is not diluted
/// by a weak one overlapping it. Among equals, one is chosen with probability
/// proportional to its claim, per instance.
///
/// That per-instance draw is what makes a boundary a *mixed wood* rather than a
/// line. Picking the strongest claim instead would draw a hard edge wherever
/// two equals meet, and scaling both densities down would give a thin strip of
/// each — which reads as a mown verge.
fn in_biome(name: &str, biomes: &[Biome], at: [f32; 2], seed: u32, base: u32) -> bool {
    let mut top = f32::NEG_INFINITY;
    let mut total = 0.0_f32;
    for b in biomes {
        let c = claim(b, at);
        if c <= 0.0 {
            continue;
        }
        if b.priority > top {
            top = b.priority;
            total = 0.0;
        }
        if b.priority >= top {
            total += c;
        }
    }
    if total <= 0.0 {
        // No biome claims this point, so a layer that belongs to one cannot
        // place here.
        return false;
    }

    // Deterministic weighted choice among the equals, from the instance's own
    // stream. The list order is the author's and is stable, so this is stable.
    let roll = (f32::from_bits((hash(base ^ seed ^ 0xB5A5_3D1F) >> 8) | 0x3F80_0000) - 1.0)
        * total;
    let mut running = 0.0_f32;
    for b in biomes {
        let c = claim(b, at);
        if c <= 0.0 || b.priority < top {
            continue;
        }
        running += c;
        if roll < running {
            return b.name == name;
        }
    }
    // Floating point can leave `roll` a hair past the last edge.
    biomes
        .iter()
        .rev()
        .find(|b| claim(b, at) > 0.0 && b.priority >= top)
        .is_some_and(|b| b.name == name)
}

/// "Not within `radius` of anything in `layer`."
#[derive(Debug, Clone, Copy)]
pub struct Exclude<'a> {
    pub layer: &'a str,
    pub radius: f32,
}

/// Why a layer list could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScatterError {
    /// A layer excluded from a name nothing declares.
    UnknownLayer { layer: String, referenced: String },
    /// A layer excluded from itself or from one declared later.
    ///
    /// **Backwards references only, and that is what makes cycles
    /// impossible** — there is no cycle check anywhere else because there
    /// cannot be one.
    ForwardReference { layer: String, referenced: String },
    /// Two layers with the same name: a later reference would be ambiguous.
    DuplicateName { layer: String },
    /// A layer named a biome nothing declares.
    UnknownBiome { layer: String, biome: String },
}

impl std::fmt::Display for ScatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownLayer { layer, referenced } => {
                write!(f, "layer {layer:?} excludes from {referenced:?}, which does not exist")
            }
            Self::ForwardReference { layer, referenced } => write!(
                f,
                "layer {layer:?} excludes from {referenced:?}, which is declared later — \
                 references must point backwards"
            ),
            Self::DuplicateName { layer } => write!(f, "two layers are named {layer:?}"),
            Self::UnknownBiome { layer, biome } => {
                write!(f, "layer {layer:?} is in biome {biome:?}, which does not exist")
            }
        }
    }
}

impl std::error::Error for ScatterError {}

/// Resolve a list of layers over a region.
///
/// Returns each layer's instances, in declaration order.
///
/// # Errors
/// [`ScatterError`] if a name is unknown, duplicated, or points forwards.
pub fn scatter<'a>(
    min: [f32; 2],
    max: [f32; 2],
    layers: &[Layer<'a>],
    ground: &dyn Fn(f32, f32) -> Ground,
) -> Result<Vec<(&'a str, Vec<Instance>)>, ScatterError> {
    scatter_in(min, max, layers, &[], ground)
}

/// [`scatter`], with biomes deciding which layers place where.
///
/// # Errors
/// [`ScatterError`], including a layer naming a biome that does not exist.
pub fn scatter_in<'a>(
    min: [f32; 2],
    max: [f32; 2],
    layers: &[Layer<'a>],
    biomes: &[Biome<'a>],
    ground: &dyn Fn(f32, f32) -> Ground,
) -> Result<Vec<(&'a str, Vec<Instance>)>, ScatterError> {
    for layer in layers {
        if let Some(name) = layer.biome
            && !biomes.iter().any(|b| b.name == name)
        {
            return Err(ScatterError::UnknownBiome {
                layer: layer.name.to_owned(),
                biome: name.to_owned(),
            });
        }
    }
    // Names first, so a typo is an error rather than a silently ignored rule.
    for (i, layer) in layers.iter().enumerate() {
        if layers[..i].iter().any(|l| l.name == layer.name) {
            return Err(ScatterError::DuplicateName { layer: layer.name.to_owned() });
        }
        for e in layer.exclude {
            let found = layers[..i].iter().position(|l| l.name == e.layer);
            if found.is_none() {
                return Err(if layers.iter().any(|l| l.name == e.layer) {
                    ScatterError::ForwardReference {
                        layer: layer.name.to_owned(),
                        referenced: e.layer.to_owned(),
                    }
                } else {
                    ScatterError::UnknownLayer {
                        layer: layer.name.to_owned(),
                        referenced: e.layer.to_owned(),
                    }
                });
            }
        }
    }

    // **How far outside the region each layer must be generated.**
    //
    // A layer that something else keeps 8 m away from has to be known 8 m
    // beyond the edge, or an instance just outside the region would fail to
    // push its neighbour out and the result would depend on where the region
    // was cut. Margins compound backwards: if C avoids B by 5 and B avoids A by
    // 8, then A is needed 13 m out.
    //
    // Getting this wrong does not crash and does not look wrong — it makes the
    // answer depend on how the world was divided up, which is the one property
    // this crate exists to have.
    let mut margin = vec![0.0_f32; layers.len()];
    for j in (0..layers.len()).rev() {
        for e in layers[j].exclude {
            let i = layers.iter().position(|l| l.name == e.layer).expect("checked above");
            margin[i] = margin[i].max(e.radius.max(0.0) + margin[j]);
        }
    }

    // **Resolved in declaration order, each layer filtered before the next
    // reads it.** A layer keeps away from what the layer it names *actually
    // put down*, not from that layer's raw candidates — a road removed for
    // being on a cliff must not go on repelling trees.
    //
    // Generating them all first and filtering afterwards is the obvious
    // shape and is wrong in exactly that way. It is also not a subtle wrong:
    // in a chain of three it left the last layer completely empty, because it
    // was avoiding every candidate of a layer that had itself been almost
    // entirely removed.
    //
    // Each layer is resolved over its own *expanded* region, so a later layer
    // reading it has the margin it needs; the restriction to the caller's
    // region happens at the end.
    let mut resolved: Vec<Vec<Instance>> = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let m = margin[i];
        let raw =
            region_on([min[0] - m, min[1] - m], [max[0] + m, max[1] + m], &layer.rules, ground);
        let kept: Vec<Instance> = raw
            .into_iter()
            .filter(|inst| match layer.biome {
                // **Before the exclusion filter**, so a layer that loses a
                // biome contest never repels anything. A tree that was never
                // planted cannot shade a bush.
                Some(name) => in_biome(name, biomes, inst.at, layer.rules.seed, inst.seed),
                None => true,
            })
            .filter(|inst| {
                layer.exclude.iter().all(|e| {
                    let source =
                        layers.iter().position(|l| l.name == e.layer).expect("checked above");
                    let r = e.radius.max(0.0);
                    // `ponytail:` brute force. A quadtree matters when a layer
                    // has a hundred thousand instances; at that point the cheap
                    // fix is to reuse the elimination grid, which already
                    // buckets by cell.
                    !resolved[source].iter().any(|other| {
                        let (dx, dz) = (other.at[0] - inst.at[0], other.at[1] - inst.at[1]);
                        dx.mul_add(dx, dz * dz) < r * r
                    })
                })
            })
            .collect();
        resolved.push(kept);
    }

    Ok(layers
        .iter()
        .zip(resolved)
        .map(|(layer, instances)| {
            let inside = instances
                .into_iter()
                .filter(|inst| {
                    inst.at[0] >= min[0]
                        && inst.at[0] < max[0]
                        && inst.at[1] >= min[1]
                        && inst.at[1] < max[1]
                })
                .collect();
            (layer.name, inside)
        })
        .collect())
}

/// How far a change in the ground can reach, in metres.
///
/// **Step 6, and it is the number the whole feature turns on.** A CSG op that
/// alters the terrain inside some box does not only change the instances
/// standing in that box: a candidate that appears or vanishes stops or starts
/// eliminating its neighbours, and those neighbours are up to `REACH` cells
/// away. If the layer is named by another, the change carries further again.
///
/// Too small and a rebuilt patch differs from a full rebuild — which is not a
/// crash and not a seam, but the wrong forest. Too large only costs time.
#[must_use]
pub fn reach_of(layers: &[Layer]) -> f32 {
    // How far a ground change moves this layer's own instances: the
    // elimination neighbourhood, `REACH` cells of `spacing / 2`.
    #[allow(clippy::cast_precision_loss)]
    let own = |r: &Rules| REACH as f32 * cell_size(r.spacing);

    let mut influence: Vec<f32> = layers.iter().map(|l| own(&l.rules)).collect();
    // Declaration order is dependency order — references point backwards — so
    // one forward pass is enough.
    for j in 0..layers.len() {
        for e in layers[j].exclude {
            if let Some(i) = layers.iter().position(|l| l.name == e.layer) {
                let carried = influence[i] + e.radius.max(0.0) + own(&layers[j].rules);
                influence[j] = influence[j].max(carried);
            }
        }
    }
    influence.into_iter().fold(0.0_f32, f32::max)
}

/// The region that must be re-scattered when the ground changes inside
/// `[min, max)`.
///
/// Feed it a CSG op's bounds; scatter the result; splice it over the old
/// answer. **That splice is exactly a full rebuild**, which is the phase's exit
/// criterion and is what the test below asserts rather than assumes.
///
/// **The caller intersects this with its own world bounds.** Near an edge the
/// reach runs past them, and scattering the unclipped region yields instances
/// the full run never had — which looks like a broken splice and is not. This
/// function does not know where the world ends; only the caller does.
#[must_use]
pub fn dirty_region(
    min: [f32; 2],
    max: [f32; 2],
    layers: &[Layer],
) -> ([f32; 2], [f32; 2]) {
    let r = reach_of(layers);
    ([min[0] - r, min[1] - r], [max[0] + r, max[1] + r])
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

    fn flat() -> impl Fn(f32, f32) -> Ground {
        |_, _| Ground::default()
    }

    /// The phase's own example: trees, but not where the roads are.
    #[test]
    fn a_layer_keeps_away_from_the_one_it_names() {
        let roads = Layer {
            name: "roads",
            rules: Rules { spacing: 14.0, seed: 1, ..Rules::default() },
            exclude: &[],
            ..Layer::default()
        };
        let trees = Layer {
            name: "trees",
            rules: Rules { spacing: 3.0, seed: 2, ..Rules::default() },
            exclude: &[Exclude { layer: "roads", radius: 6.0 }],
            ..Layer::default()
        };
        let out = scatter([0.0, 0.0], [80.0, 80.0], &[roads, trees], &flat()).expect("resolves");
        let (road_pts, tree_pts) = (&out[0].1, &out[1].1);
        assert!(!road_pts.is_empty() && !tree_pts.is_empty(), "a layer is empty");

        for t in tree_pts {
            for r in road_pts {
                let d = (t.at[0] - r.at[0]).hypot(t.at[1] - r.at[1]);
                assert!(d >= 6.0, "a tree is {d:.2} m from a road, inside the 6 m exclusion");
            }
        }

        // And it must not have removed everything, or the test above is
        // vacuous. The baseline is the same rule with the exclusion dropped —
        // not the same `Layer` in a shorter list, which correctly refuses to
        // resolve because it still names a layer that is no longer there.
        let unrestricted = Layer { exclude: &[], ..trees };
        let alone =
            scatter([0.0, 0.0], [80.0, 80.0], &[unrestricted], &flat()).expect("resolves");
        assert!(
            tree_pts.len() * 2 > alone[0].1.len(),
            "exclusion removed more than half the trees: {} of {}",
            tree_pts.len(),
            alone[0].1.len()
        );
        assert!(tree_pts.len() < alone[0].1.len(), "exclusion removed nothing at all");
    }

    /// **The margin test, and it is the one that matters.**
    ///
    /// An excluded layer has to be generated beyond the region's edge, or an
    /// instance just outside fails to push its neighbour out and the answer
    /// depends on where the region was cut. That is not a crash and does not
    /// look wrong — it is the loss of the one property this crate exists for.
    #[test]
    fn an_excluded_patch_matches_the_full_run() {
        let roads = Layer {
            name: "roads",
            rules: Rules { spacing: 11.0, seed: 3, ..Rules::default() },
            exclude: &[],
            ..Layer::default()
        };
        let trees = Layer {
            name: "trees",
            rules: Rules { spacing: 3.0, seed: 4, ..Rules::default() },
            exclude: &[Exclude { layer: "roads", radius: 7.0 }],
            ..Layer::default()
        };
        let list = [roads, trees];

        let whole = scatter([-40.0, -40.0], [40.0, 40.0], &list, &flat()).expect("resolves");
        let patch = scatter([-5.0, 1.0], [12.0, 18.0], &list, &flat()).expect("resolves");

        let inside: Vec<Instance> = whole[1]
            .1
            .iter()
            .copied()
            .filter(|i| i.at[0] >= -5.0 && i.at[0] < 12.0 && i.at[1] >= 1.0 && i.at[1] < 18.0)
            .collect();
        assert!(!patch[1].1.is_empty(), "the patch is empty — the test proves nothing");
        assert_eq!(patch[1].1, inside, "exclusion made the result depend on the region");
    }

    /// Margins compound: C avoids B, B avoids A, so A is needed further out
    /// than either radius alone. A chain is where a margin bug hides.
    #[test]
    fn a_chain_of_exclusions_still_matches_the_full_run() {
        let a = Layer {
            name: "a",
            // **Dense enough and far-reaching enough to have statistical
            // power.** A first version used spacing 13 with an 8 m radius and
            // passed with the compounding margin deliberately removed: `a` was
            // so sparse that the narrow band where the defect bites usually
            // held no instance at all. A test that relies on a coincidence is
            // a test that reports what the coincidence did.
            rules: Rules { spacing: 9.0, seed: 5, ..Rules::default() },
            exclude: &[],
            ..Layer::default()
        };
        let b = Layer {
            name: "b",
            rules: Rules { spacing: 3.0, seed: 6, ..Rules::default() },
            exclude: &[Exclude { layer: "a", radius: 10.0 }],
            ..Layer::default()
        };
        let c = Layer {
            name: "c",
            rules: Rules { spacing: 2.5, seed: 7, ..Rules::default() },
            exclude: &[Exclude { layer: "b", radius: 6.0 }],
            ..Layer::default()
        };
        let list = [a, b, c];

        // **Tiled into many small patches, not one.** A single patch was tried
        // first and could not see a missing compounding margin: the defect only
        // bites where an `a` instance falls in a narrow band outside the patch
        // *and* a `b` candidate sits within its radius *and* a `c` instance is
        // within range of that — a coincidence rare enough that one patch
        // usually misses it. Sixteen patches expose sixteen times the boundary,
        // which is also exactly what dirty-region regeneration does.
        let whole = scatter([-20.0, -20.0], [20.0, 20.0], &list, &flat()).expect("resolves");

        let mut tiled: Vec<Vec<Instance>> = vec![Vec::new(); 3];
        for gz in 0_i16..4 {
            for gx in 0_i16..4 {
                let lo = [-20.0 + f32::from(gx) * 10.0, -20.0 + f32::from(gz) * 10.0];
                let hi = [lo[0] + 10.0, lo[1] + 10.0];
                let part = scatter(lo, hi, &list, &flat()).expect("resolves");
                for (layer, out) in tiled.iter_mut().enumerate() {
                    out.extend_from_slice(&part[layer].1);
                }
            }
        }

        let key = |i: &Instance| (i.at[0].to_bits(), i.at[1].to_bits());
        for layer in 0..3 {
            let mut from_whole: Vec<_> = whole[layer].1.iter().map(key).collect();
            let mut from_tiles: Vec<_> = tiled[layer].iter().map(key).collect();
            from_whole.sort_unstable();
            from_tiles.sort_unstable();
            assert_eq!(
                from_tiles, from_whole,
                "layer {layer} differs between a tiled rebuild and a whole one"
            );
        }
        assert!(!tiled[2].is_empty(), "the deepest layer is empty");
    }

    /// Names are checked rather than silently ignored — a typo that scattered
    /// nothing would look like a rule that simply did not match.
    #[test]
    fn bad_names_are_rejected() {
        let a = Layer { name: "a", ..Layer::default() };
        let typo = Layer {
            name: "b",
            exclude: &[Exclude { layer: "rodes", radius: 1.0 }],
            ..Layer::default()
        };
        assert!(matches!(
            scatter([0.0, 0.0], [10.0, 10.0], &[a, typo], &flat()),
            Err(ScatterError::UnknownLayer { .. })
        ));

        // Forward references are the cycle check: there is no other one,
        // because with backwards-only references a cycle cannot be written.
        let first = Layer {
            name: "first",
            exclude: &[Exclude { layer: "second", radius: 1.0 }],
            ..Layer::default()
        };
        let second = Layer { name: "second", ..Layer::default() };
        assert!(matches!(
            scatter([0.0, 0.0], [10.0, 10.0], &[first, second], &flat()),
            Err(ScatterError::ForwardReference { .. })
        ));

        let dup = Layer { name: "a", ..Layer::default() };
        assert!(matches!(
            scatter([0.0, 0.0], [10.0, 10.0], &[a, dup], &flat()),
            Err(ScatterError::DuplicateName { .. })
        ));

        // And a layer may not exclude from itself, which is the same rule.
        let selfref = Layer {
            name: "x",
            exclude: &[Exclude { layer: "x", radius: 1.0 }],
            ..Layer::default()
        };
        assert!(scatter([0.0, 0.0], [10.0, 10.0], &[selfref], &flat()).is_err());
    }

    /// Two biomes, and the stronger one takes the overlap.
    #[test]
    fn the_higher_priority_biome_takes_the_overlap() {
        let pine = Biome {
            name: "highland",
            min: [-50.0, -50.0],
            max: [10.0, 50.0],
            priority: 2.0,
            blend: 0.0,
        };
        let oak = Biome {
            name: "lowland",
            min: [-10.0, -50.0],
            max: [50.0, 50.0],
            priority: 1.0,
            blend: 0.0,
        };
        let pines = Layer {
            name: "pines",
            rules: Rules { spacing: 4.0, seed: 11, ..Rules::default() },
            biome: Some("highland"),
            ..Layer::default()
        };
        let oaks = Layer {
            name: "oaks",
            rules: Rules { spacing: 4.0, seed: 12, ..Rules::default() },
            biome: Some("lowland"),
            ..Layer::default()
        };
        let out = scatter_in([-50.0, -50.0], [50.0, 50.0], &[pines, oaks], &[pine, oak], &flat())
            .expect("resolves");

        // The overlap is -10..10. Highland wins it, so no oak may stand there,
        // and pines must.
        assert!(
            out[1].1.iter().all(|i| i.at[0] >= 10.0),
            "an oak stood inside the highland's claim"
        );
        assert!(
            out[0].1.iter().any(|i| i.at[0] > -10.0 && i.at[0] < 10.0),
            "no pine in the contested strip, so the contest proved nothing"
        );
        assert!(!out[1].1.is_empty(), "the losing biome has nothing anywhere");
    }

    /// **The boundary interleaves rather than blending densities.** In the fade
    /// band both biomes place, mixed — a transition that scaled both densities
    /// down would give a thin strip of each, which reads as a mown verge.
    #[test]
    fn a_blended_boundary_interleaves_the_two() {
        let west = Biome {
            name: "west",
            min: [-60.0, -60.0],
            max: [0.0, 60.0],
            priority: 1.0,
            blend: 18.0,
        };
        let east = Biome {
            name: "east",
            min: [0.0, -60.0],
            max: [60.0, 60.0],
            priority: 1.0,
            blend: 18.0,
        };
        let a = Layer {
            name: "a",
            rules: Rules { spacing: 3.0, seed: 21, ..Rules::default() },
            biome: Some("west"),
            ..Layer::default()
        };
        let b = Layer {
            name: "b",
            rules: Rules { spacing: 3.0, seed: 22, ..Rules::default() },
            biome: Some("east"),
            ..Layer::default()
        };
        let out = scatter_in([-60.0, -60.0], [60.0, 60.0], &[a, b], &[west, east], &flat())
            .expect("resolves");

        let band = |v: &[Instance]| {
            v.iter().filter(|i| i.at[0] > -12.0 && i.at[0] < -2.0).count()
        };
        assert!(band(&out[0].1) > 0 && band(&out[1].1) > 0, "the band is not mixed");

        // Deep inside the west, the east must have nothing at all.
        assert!(
            out[1].1.iter().all(|i| i.at[0] > -18.0),
            "the east placed deep inside the west"
        );
    }

    /// **Phase 5's exit criterion, asserted rather than argued.**
    ///
    /// Blow a hole in the terrain, re-scatter only the region the change can
    /// reach, splice it over the old answer — and get exactly what a full
    /// rebuild would have produced.
    #[test]
    fn regenerating_only_the_dirty_region_equals_a_full_rebuild() {
        let roads = Layer {
            name: "roads",
            // Dense enough that a crater reliably contains several, so the
            // annulus where a short reach bites is reliably occupied. At
            // spacing 9 a crater held about one road and whether the defect
            // showed was a coin flip.
            rules: Rules { spacing: 5.0, seed: 31, ..Rules::default() },
            ..Layer::default()
        };
        let trees = Layer {
            name: "trees",
            rules: Rules { spacing: 3.0, seed: 32, ..Rules::default() },
            exclude: &[Exclude { layer: "roads", radius: 10.0 }],
            ..Layer::default()
        };
        let list = [roads, trees];
        let (world_min, world_max) = ([-60.0_f32, -60.0], [60.0_f32, 60.0]);

        // Before: flat everywhere. After: a crater — unclimbable inside it.
        let before = |_: f32, _: f32| Ground::default();
        // **Several craters, not one.** A single crater passed with the
        // dirty reach deliberately shortened: whether the shortfall bites
        // depends on a road happening to sit in the band between the wrong
        // radius and the right one. One sample reports what that coincidence
        // did; a handful reports the rule.
        for crater in [[6.0_f32, -4.0], [-22.0, 15.0], [24.0, -21.0], [-9.0, -18.0]] {
        let radius = 9.0_f32;
        let after = move |x: f32, z: f32| {
            if (x - crater[0]).hypot(z - crater[1]) < radius {
                slope(75.0)
            } else {
                Ground::default()
            }
        };

        let old = scatter(world_min, world_max, &list, &before).expect("resolves");
        let full = scatter(world_min, world_max, &list, &after).expect("resolves");

        // Only the ground inside the crater's bounds changed.
        let (dirty_min, dirty_max) = dirty_region(
            [crater[0] - radius, crater[1] - radius],
            [crater[0] + radius, crater[1] + radius],
            &list,
        );
        // Clipped to the world, as `dirty_region` says the caller must. The
        // crater at [31, -28] is close enough to the edge that the unclipped
        // region runs past it, and splicing in instances from outside the world
        // reads as a broken rebuild.
        let dirty_min = [dirty_min[0].max(world_min[0]), dirty_min[1].max(world_min[1])];
        let dirty_max = [dirty_max[0].min(world_max[0]), dirty_max[1].min(world_max[1])];
        let patch = scatter(dirty_min, dirty_max, &list, &after).expect("resolves");

        let outside = |i: &Instance| {
            i.at[0] < dirty_min[0]
                || i.at[0] >= dirty_max[0]
                || i.at[1] < dirty_min[1]
                || i.at[1] >= dirty_max[1]
        };
        let key = |i: &Instance| (i.at[0].to_bits(), i.at[1].to_bits());

        for layer in 0..2 {
            // The splice: everything the change could not reach, kept from the
            // old answer, plus the rebuilt patch.
            let mut spliced: Vec<_> =
                old[layer].1.iter().filter(|i| outside(i)).map(key).collect();
            spliced.extend(patch[layer].1.iter().map(key));
            spliced.sort_unstable();

            let mut rebuilt: Vec<_> = full[layer].1.iter().map(key).collect();
            rebuilt.sort_unstable();

            assert_eq!(
                spliced, rebuilt,
                "crater {crater:?}, layer {layer}: a spliced rebuild differs from a full one"
            );
        }

        // And the crater actually did something, or the test compares two
        // identical worlds and proves nothing.
        // **Sets, not counts.** Comparing lengths said "changed nothing" for a
        // crater that had in fact moved instances and happened to end with the
        // same number.
        // **Checked on the roads, not the trees.** A crater always removes the
        // roads standing in it; whether any *tree* changes depends on whether a
        // surviving road outside still excludes the same ground, which for a
        // dense road layer is often true. Asserting on trees said "changed
        // nothing" about a crater that had plainly changed something.
        let old_keys: Vec<_> = old[0].1.iter().map(key).collect();
        let new_keys: Vec<_> = full[0].1.iter().map(key).collect();
        assert_ne!(
            old_keys, new_keys,
            "the crater at {crater:?} changed no instances at all"
        );
        }
    }

    /// The reach must actually grow with the chain, or `dirty_region` is a
    /// constant wearing a function's name.
    #[test]
    fn the_dirty_reach_grows_with_the_exclusion_chain() {
        let a = Layer { name: "a", ..Layer::default() };
        let b = Layer {
            name: "b",
            exclude: &[Exclude { layer: "a", radius: 20.0 }],
            ..Layer::default()
        };
        assert!(
            reach_of(&[a, b]) > reach_of(&[a]) + 20.0,
            "a 20 m exclusion did not carry into the reach"
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
