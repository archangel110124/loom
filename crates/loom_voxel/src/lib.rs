//! Destructible smooth voxels: `i8` SDF chunks, CSG op lists, sparse storage.
//!
//! # Runtime cost is the design constraint
//!
//! Every decision here is a memory or time decision, and each is measured by a
//! test rather than asserted in a comment:
//!
//! | Technique | What it buys |
//! | --- | --- |
//! | `i8` distances, not `f32` | 4× memory. Extraction only needs distance *near* the surface; far-field magnitude never generates geometry. |
//! | Uniform-chunk collapse | A chunk that is entirely solid or entirely air costs a discriminant, not 32 KB. Most chunks in any real volume are uniform. |
//! | Chunk-major bake with early-out | A chunk whose centre is further from every op than its own radius **cannot** contain a surface. It is filled in O(1) instead of evaluating 32,768 voxels. This is the difference between baking a 256³ volume in milliseconds and in seconds. |
//! | Edit layer, not a rewritten field | Runtime destruction stores only the chunks actually touched (voxel doc §1.1). An untouched world costs nothing; forty craters cost forty craters. |
//! | Meshing only surface chunks | Uniform chunks are skipped in O(1), which is most of them. |
//!
//! # And three rules
//!
//! **SDF, not occupancy.** A 0/1 grid cannot represent curves.
//!
//! **Op lists, never voxel arrays** (never-do #11). A 512³ volume is 134
//! million voxels and must never enter a `.loom` file. Scenes store the recipe.
//! That keeps them diffable and small, and gives determinism for free: the same
//! ops with the same seed produce bit-identical voxels.
//!
//! **Order is not commutative.** Subtract-then-union differs from
//! union-then-subtract, and an agent will assume otherwise unless told.

pub mod exposure;
pub mod heightfield;
pub mod mesh;

pub use mesh::{Mesher, SurfaceNets};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Chunk edge length. 32³ is the voxel doc's sweet spot for destructible
/// terrain: small enough to remesh cheaply on edit, large enough that
/// per-chunk overhead stays amortised.
pub const CHUNK: usize = 32;
const VOXELS: usize = CHUNK * CHUNK * CHUNK;

/// Distance encoded per `i8` step, in voxels.
///
/// `i8` spans ±1 voxel at 1/127 precision — ample, since the mesher only
/// interpolates across the zero crossing. Getting this factor wrong yields a
/// surface that is subtly offset or stair-stepped, so it is round-trip tested.
pub const SDF_SCALE: f32 = 1.0 / 127.0;

/// Quantise a distance in voxels to storage.
#[must_use]
pub fn quantize(distance: f32) -> i8 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (distance / SDF_SCALE).clamp(-127.0, 127.0).round() as i8
    }
}

/// Decode stored distance back to voxels.
#[must_use]
pub fn dequantize(value: i8) -> f32 {
    f32::from(value) * SDF_SCALE
}

/// One chunk of the field.
///
/// `Solid` and `Air` carry no allocation at all — the enum is a discriminant
/// plus a pointer, 16 bytes, versus 32 KB for the detailed case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    /// Entirely inside a solid.
    Solid,
    /// Entirely empty.
    Air,
    /// Contains a surface.
    Detailed(Box<[i8; VOXELS]>),
}

impl Chunk {
    /// Distance at a local coordinate.
    #[must_use]
    pub fn get(&self, x: usize, y: usize, z: usize) -> i8 {
        match self {
            // Far field: the sign is what matters, magnitude saturates.
            Self::Solid => -127,
            Self::Air => 127,
            Self::Detailed(v) => v[index(x, y, z)],
        }
    }

    /// Write a distance, promoting a uniform chunk only if the value differs.
    ///
    /// The equality check is what keeps a bake from promoting every chunk it
    /// merely visits.
    pub fn set(&mut self, x: usize, y: usize, z: usize, value: i8) {
        if let Self::Detailed(v) = self {
            v[index(x, y, z)] = value;
            return;
        }
        let fill = if matches!(self, Self::Solid) { -127 } else { 127 };
        if fill == value {
            return;
        }
        let mut voxels = Box::new([fill; VOXELS]);
        voxels[index(x, y, z)] = value;
        *self = Self::Detailed(voxels);
    }

    /// Collapse back to `Solid`/`Air` when no surface remains.
    ///
    /// Run after editing, or a region that was carved and then refilled keeps
    /// paying 32 KB for a chunk with nothing in it.
    pub fn collapse(&mut self) {
        let Self::Detailed(voxels) = self else {
            return;
        };
        let inside = voxels[0] < 0;
        if voxels.iter().any(|v| (*v < 0) != inside) {
            return;
        }
        *self = if inside { Self::Solid } else { Self::Air };
    }

    /// Whether this chunk can contain a surface.
    #[must_use]
    pub fn has_surface(&self) -> bool {
        matches!(self, Self::Detailed(_))
    }

    /// Heap bytes this chunk occupies.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        if self.has_surface() { VOXELS } else { 0 }
    }
}

fn index(x: usize, y: usize, z: usize) -> usize {
    x + y * CHUNK + z * CHUNK * CHUNK
}

/// How an operation combines with what is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsgMode {
    Union,
    Subtract,
    Intersect,
}

/// Noise displacement pushed into a primitive's surface.
///
/// **This is what makes a rock a rock.** Every primitive here is analytic and
/// smooth, and union is a hard minimum, so a shape assembled from them has
/// detail at exactly one scale — the size of the pieces placed — and creases
/// where they meet. Measured: fifty ops of spheres, capsules and subtractions
/// read as a cluster of soap bubbles. A real rock has detail at every scale,
/// which is what a fractal displacement supplies and what no arrangement of
/// primitives can.
///
/// Applied as `d(p) - amplitude * noise(p * frequency)`, so the surface moves
/// outward where the noise is positive and inward where it is negative.
///
/// **`ridged` is usually the one you want.** fBm is a sum of smooth bumps and
/// turns a sphere into a potato; ridged noise creases where it crosses zero,
/// and creases at several scales at once is what fractured stone is.
///
/// Costs one noise evaluation per voxel per octave, on top of an analytic
/// distance that was a handful of arithmetic ops — see [`VoxelOp::lipschitz`]
/// for the other price, which is that the early-out has to work harder.
///
/// # The recipe: start here, for a rock of radius R
///
/// ```text
/// amplitude = 0.20 * R      frequency = 0.6 / R      ridged = true
/// octaves   = floor(log2(2 * amplitude / voxel_size)) + 1   <- and never more
/// voxel_size <= R / 20                                      <- or don't bother
/// ```
///
/// **Every number there was measured, not chosen by eye** —
/// `cargo run --release -p loom_voxel --example rockcalib` prints the sweep and
/// `loom measure --shape` reports it for one shape. The statistic is **concave
/// surface-area fraction** (`loom_asset::shape`), because that is what
/// separates fractured stone from a blob: a union of analytic primitives bulges
/// outward everywhere and creases inward only on a curve. At `voxel_size` 0.03
/// on a 3 m boulder:
///
/// ```text
/// three photogrammetry scans          concave 42.3-44.2%   spread 1.16-1.50
/// this recipe (A = 0.20R, x5)                 43.2%               1.50
/// A = 0.25R (was the eyeballed value)         47.0%               1.53
/// fbm instead of ridged                       27.2%               1.19
/// fifty primitives, hard min, no displace      8.7%               0.53
/// ```
///
/// The recipe is scale-free by construction and verified so: R = 0.4, 1.5 and
/// 6 m at proportional `voxel_size` all measure 43.2% / 1.50 / 95,182 triangles,
/// identically.
///
/// **`spread` is the half you cannot cheat.** Carving subtract-spheres into the
/// blob raises its concave fraction to 32.7% while its spread stays at 0.70 —
/// concavity with detail at one scale. Real stone and a ridged displacement are
/// both above 1.1. Judge a shape on both columns.
///
/// **Report-only.** Discrete curvature is resolution-dependent, so the same rock
/// reads 25.7% at `voxel_size` 0.12 and 51.6% at 0.02. Never threshold it, and
/// only ever compare shapes measured at equal `voxel_size`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Displace {
    /// How far the surface moves, in world units. Roughly the depth of the
    /// deepest crevice.
    ///
    /// **`0.20 * radius`.** Measured against the scans: 0.05R is 9.4% concave
    /// and reads as a dimpled ball, 0.20R is 43.2% and lands inside the scan
    /// band, 0.50R is 53.5% and has stopped being the shape it started as. It
    /// is also the cheapest parameter to raise — 0.05R to 0.50R is +14% bake —
    /// so it is the first one to reach for.
    pub amplitude: f32,
    /// Cycles per world unit of the coarsest octave. `1.0 / frequency` is the
    /// size of the largest feature.
    ///
    /// **`0.6 / radius`**, i.e. the largest feature is about 1.7 radii, so the
    /// coarsest octave is what makes the rock *lopsided* rather than what
    /// roughens it. Below 0.3/R the rock is a sphere with dents (12.9% at
    /// 0.2/R); above 1/R it is gravel, and it costs — 2.0/R draws 1.7x the
    /// triangles of 0.6/R for one point of concavity.
    pub frequency: f32,
    /// Detail levels, each half the amplitude and twice the frequency.
    ///
    /// **Cap it at `floor(log2(2 * amplitude / voxel_size)) + 1`.** Two separate
    /// limits bite and this is the tighter one: octave `k` has amplitude
    /// `A/2^k`, and once that is below half a voxel it cannot place a vertex
    /// the octave above it did not already. (The other is wavelength — octave
    /// `k` needs `1/(f*2^k) >= 2*voxel_size` or it is below the sampling rate.
    /// At the recipe's `A`/`f` ratio the amplitude limit is always the smaller,
    /// by one octave.)
    ///
    /// Measured at `voxel_size` 0.03 on R = 1.5, where the cap is 5: octaves
    /// 3/4/5/6/7/8 give 43.6 / 45.9 / 47.0 / 47.2 / 47.4 / 47.4% concave for
    /// 333 / 458 / 591 / 714 / 872 / 1010 ms of bake. So the last three octaves
    /// cost **+71% bake for +0.4 points**. At `voxel_size` 0.06 the cap drops
    /// to 4 and the curve flattens one octave earlier, which is the check that
    /// the rule is a rule rather than a fit to one curve.
    ///
    /// The cap is also worth more than the per-voxel cost suggests, because
    /// [`Displace::gradient_bound`] grows linearly in octaves — capping octaves
    /// caps `reach`, which shrinks the set of chunks the bake must visit.
    pub octaves: u32,
    pub seed: u64,
    /// Ridged rather than fBm. Creased and fractured instead of lumpy.
    ///
    /// **Leave it true.** Same amplitude, same frequency, same octaves: ridged
    /// measures 47.0% concave over 1.53 decades and fbm 27.2% over 1.19, and
    /// the scans are 42-44% over 1.16-1.50. fbm is a sum of smooth bumps, so it
    /// makes a potato; a crease is where a rock broke.
    pub ridged: bool,
}

impl Default for Displace {
    fn default() -> Self {
        // Zero amplitude is the identity, so a `Displace` that arrives with
        // fields missing changes nothing rather than deforming the shape by
        // surprise.
        Self { amplitude: 0.0, frequency: 1.0, octaves: 4, seed: 0, ridged: true }
    }
}

impl Displace {
    /// The displacement at a point: how far to push the surface outward.
    #[must_use]
    pub fn at(&self, p: [f32; 3]) -> f32 {
        if self.amplitude.abs() < 1e-6 {
            return 0.0;
        }
        let (x, y, z) = (p[0], p[1], p[2]);
        let n = if self.ridged {
            loom_terrain::noise::ridged3(x, y, z, self.frequency, self.octaves, self.seed)
        } else {
            loom_terrain::noise::fbm3(x, y, z, self.frequency, self.octaves, 2.0, 0.5, self.seed)
        };
        self.amplitude * n
    }

    /// Upper bound on how fast [`Self::at`] can change, per world unit.
    ///
    /// **The early-out in `bake` assumes a 1-Lipschitz field** and a displaced
    /// primitive is not one: subtracting a noise field adds its gradient to the
    /// distance's. Understating this punches holes in the surface, which is the
    /// same failure the heightfield's own bound exists to prevent — and it
    /// fails in the direction that looks like broken geometry rather than slow
    /// baking, so it errs high on purpose.
    ///
    /// **Ridged noise is four times steeper than fBm at the same parameters**,
    /// and this bound was written for fBm alone. [`loom_terrain::noise::ridged3`]
    /// is not a weighted sum of `value3`: each octave is `(1 - |v|)^2` times the
    /// previous octave's weight, and the sum is then mapped from `[0, 1]` back
    /// to `[-1, 1]`. Those are two separate factors of two — `d/dv (1-|v|)^2` is
    /// `2(1-|v|)`, at most 2, and the output remap is another 2 — so a per-octave
    /// factor of `4 * 4.0` covers both.
    ///
    /// **It does not cover the third term analytically, and that is stated
    /// rather than hidden.** `weight` carries each octave's value into the next,
    /// so `dn_i` picks up a `2*dn_{i-1}` that grows with octave count in the
    /// worst case. What settles it is measurement, in the direction the term
    /// predicts: headroom against this bound *widens* with octaves rather than
    /// closing, so the feedback is nowhere near its worst case on real
    /// coefficients. If a future octave count ever runs out of headroom, this is
    /// the term that ate it.
    ///
    /// Measured, because the analytic argument had been made once already and
    /// was wrong: `cargo run --release -p loom_voxel --example gradcheck` takes
    /// the largest `|grad at|` over six seeds and a refined grid search. Against
    /// the fBm factor the ridged field exceeded its own bound at **every** octave
    /// count — 3.28x at one octave, 1.62x at the recommended five, 1.22x at
    /// eight — while fBm stayed under it at 0.42-0.83x. With this factor the
    /// worst ratio is 0.82, at one octave, falling to 0.30 at eight.
    /// `--example holecheck` closes the loop on what the gap cost: at
    /// `voxel_size = R/200`, which the recipe's `<= R/20` permits, the early-out
    /// was filling chunks that held surface — 353 voxels on the wrong side of it
    /// across the probe, and 0 with this factor.
    #[must_use]
    pub fn gradient_bound(&self) -> f32 {
        if self.amplitude.abs() < 1e-6 {
            return 0.0;
        }
        // See above: the squared ridge and the [0,1] -> [-1,1] remap.
        let shape = if self.ridged { 4.0 } else { 1.0 };
        let mut slope = 0.0;
        let mut amp = 1.0_f32;
        let mut freq = self.frequency;
        let mut norm = 0.0;
        for _ in 0..self.octaves.max(1) {
            // **4.0, not the heightfield's 3.0, and the two differ on purpose.**
            // They interpolate with different curves: `loom_field::noise` uses
            // Hermite smoothstep `3t^2-2t^3`, whose derivative peaks at 1.5, and
            // 3.0 is that over a [-1,1] range. `loom_terrain::noise` — which is
            // what `Displace` samples — uses *smootherstep* `t^3(6t^2-15t+10)`,
            // peaking at 1.875, so the per-axis bound is 3.75.
            //
            // Measured, not only derived: `cargo run -p loom_terrain --example
            // gradbound` finds a largest |grad value3| of 3.249 over 24 seeds
            // and a 61^3 grid inside one cell, against 2.409 for the 2D one. So
            // the heightfield's 3.0 has room to spare and this path did not — it
            // was copied from there and understated its own bound, which is the
            // direction that punches holes.
            slope += amp * freq * 4.0 * shape;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        self.amplitude.abs() * slope / norm.max(1e-6)
    }
}

/// A `loom_terrain` recipe, baked, with the two numbers derived from the bake.
///
/// **The whole of the join between the 2D pipeline and the 3D field.** Erosion,
/// spline carves, flatten discs and the corridor guarantee are all 2D
/// algorithms operating on a height array; what a voxel volume needs from them
/// is `height_at`, and this is where that array lives once it has been
/// computed. It is behind an `Arc` because two ops naming the same recipe — or
/// three calls to `build_volume` in one process — must share one 231 ms bake.
pub struct TerrainMap {
    pub map: loom_terrain::Heightmap,
    /// Exact Lipschitz bound of `p.y - h(x, z)` over the bilinear surface.
    lipschitz: f32,
    /// `map.range()`, kept because [`VoxelOp::bounds`] is called per edit and
    /// the range is a full pass over the array.
    range: (f32, f32),
}

impl std::fmt::Debug for TerrainMap {
    /// Dimensions, not data. The derived one prints every height in the map,
    /// which is 16k floats in the smallest recipe anyone would author and turns
    /// any `{:?}` of an op list into pages of noise.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TerrainMap({}x{}, {:.1}..{:.1} m, L = {:.2})",
            self.map.width, self.map.height, self.range.0, self.range.1, self.lipschitz
        )
    }
}

impl PartialEq for TerrainMap {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl TerrainMap {
    /// Measure a baked map.
    #[must_use]
    pub fn new(map: loom_terrain::Heightmap) -> Self {
        // **The exact bound of the bilinear interpolant, not a percentile of
        // it.** Along X at fixed Y the bilinear derivative is a lerp between
        // the two edge slopes of the cell, so it never exceeds the largest
        // adjacent-sample difference on that axis; the same holds on Z. So
        // taking the max of each axis separately and combining them bounds
        // `|grad h|` everywhere, including inside cells, in one pass.
        //
        // A high percentile plus a safety factor was the other candidate,
        // because one spline-carved cliff cell sets the max and the max sets
        // `reach`. It is not a bound, though, and understating a bound is the
        // direction that punches holes. **Measured on `vale.loom`**, which is
        // 4.43-Lipschitz: the exact bound bakes in 31.1 ms against 17.2 ms for
        // a pinned 1.0, and the pinned one leaves 178 surface chunks where the
        // sound one finds 181 — three chunks of hole, which is the whole
        // argument in one number. 14 ms against a 39 ms recipe bake and a
        // 67 ms mesh is not where this scene's time goes.
        let [mx, mz] = map.metres_per_pixel();
        let (mut gx, mut gz) = (0.0_f32, 0.0_f32);
        for y in 0..map.height {
            for x in 0..map.width {
                let here = map.get(x, y);
                if x + 1 < map.width {
                    gx = gx.max((map.get(x + 1, y) - here).abs() / mx.max(1e-6));
                }
                if y + 1 < map.height {
                    gz = gz.max((map.get(x, y + 1) - here).abs() / mz.max(1e-6));
                }
            }
        }
        Self {
            lipschitz: gz.mul_add(gz, gx.mul_add(gx, 1.0)).sqrt(),
            range: map.range(),
            map,
        }
    }

    /// Height at a world position, given the rect the map covers.
    #[must_use]
    pub fn height(&self, rect: [f32; 4], x: f32, z: f32) -> f32 {
        let [mx, mz] = self.map.metres_per_pixel();
        self.map
            .sample((x - rect[0]) / mx.max(1e-6), (z - rect[1]) / mz.max(1e-6))
    }
}

/// Baked recipes, keyed by content hash.
///
/// The hash is the recipe's own [`loom_terrain::Recipe::content_hash`], so two
/// scenes naming the same landform share one bake and an edited recipe misses
/// automatically. Keying by *path* instead would serve a stale map after an
/// edit, which is the failure mode a cache is worth having only if it avoids.
///
/// `ponytail:` unbounded, and never evicted. It holds one map per distinct
/// recipe in a process; a tool that walked a library of hundreds would want an
/// LRU.
static BAKED: Mutex<BTreeMap<String, Arc<TerrainMap>>> = Mutex::new(BTreeMap::new());

/// Bake the recipe every [`VoxelOp::Terrain`] names, resolving paths against
/// `base` — the directory of the scene file that authored them.
///
/// **Ops that came from a scene must go through this before they are baked**,
/// and [`Volume::bake`] asserts that they did. A `Terrain` op arrives from
/// `serde` with no map at all, because the map is derived and a scene stores
/// recipes rather than derived arrays (never-do #11).
///
/// # Errors
/// If a recipe is missing or invalid, or if the rect it is placed over is not
/// the size the recipe says it covers.
pub fn resolve(ops: &mut [VoxelOp], base: &std::path::Path) -> Result<(), String> {
    for (index, op) in ops.iter_mut().enumerate() {
        let VoxelOp::Terrain {
            recipe, rect, map, ..
        } = op
        else {
            continue;
        };
        let path = base.join(&*recipe);
        let loaded = loom_terrain::Recipe::load(&path)
            .map_err(|e| format!("op {index} (kind = \"terrain\"): {}: {e}", path.display()))?;

        // **The world rect and the recipe's `world_scale` are two independent
        // coordinate systems**, and disagreeing about them renders a plausible
        // landscape at the wrong scale — the one failure here that looks like
        // an art decision. So they are required to agree, with both numbers in
        // the message: `world_scale` means "world units this map covers", and
        // a rect of a different size silently redefines it. It also keeps
        // `loom terrain`'s slope and buildable numbers true of the scene,
        // since `analyze` measures them through `world_scale`.
        let extent = [rect[2] - rect[0], rect[3] - rect[1]];
        let scale = loaded.world_scale;
        if (extent[0] - scale[0]).abs() > 1e-3 * scale[0].abs().max(1.0)
            || (extent[1] - scale[1]).abs() > 1e-3 * scale[1].abs().max(1.0)
        {
            return Err(format!(
                "op {index} (kind = \"terrain\"): rect {rect:?} is {extent:?} m across but \
                 {} says world_scale = {scale:?}. They must agree — the rect places the \
                 recipe, it does not rescale it.",
                path.display()
            ));
        }

        *map = Some(baked(&loaded));
    }
    Ok(())
}

/// The baked map for a recipe, from the cache or into it.
fn baked(recipe: &loom_terrain::Recipe) -> Arc<TerrainMap> {
    let key = recipe.content_hash();
    let mut cache = BAKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(hit) = cache.get(&key) {
        return Arc::clone(hit);
    }
    let entry = Arc::new(TerrainMap::new(recipe.bake()));
    cache.insert(key, Arc::clone(&entry));
    entry
}

/// One authored edit.
///
/// **This is what a scene stores**, never the resulting voxels. "Carve a cave
/// through this hillside" is a handful of capsule subtractions — a request the
/// agent can express correctly on the first try (voxel doc §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VoxelOp {
    Sphere {
        center: [f32; 3],
        radius: f32,
        mode: CsgMode,
        /// Optional noise displacement. **Absent in every scene authored
        /// before it existed, and `#[serde(default)]` is what keeps those
        /// files loading unchanged** — an absent displacement is the identity,
        /// so their voxels and therefore the sim hash do not move.
        #[serde(default)]
        displace: Option<Displace>,
        /// Stretch the sphere along each axis, in world units, by sliding the
        /// sample point instead of scaling it: `p - clamp(p, -h, h)`.
        ///
        /// **Exactly distance-preserving**, so the result is still a true
        /// distance field and `lipschitz` stays 1. One non-zero axis is a
        /// capsule, two a stadium slab, three a rounded box — one primitive
        /// covering a family that would otherwise be three.
        ///
        /// It is on `Sphere` alone because that is the only place it is not
        /// redundant: elongating a `Box` along its own axes is the same shape
        /// as widening `half_extents`, and a `Capsule` is already a sphere
        /// elongated along an arbitrary segment.
        #[serde(default)]
        elongate: [f32; 3],
    },
    Box {
        center: [f32; 3],
        half_extents: [f32; 3],
        mode: CsgMode,
        #[serde(default)]
        displace: Option<Displace>,
        /// Rotation about Y, in **degrees**, counted the same way as a node's
        /// `Quat::from_rotation_y` so a rotated box and a rotated mesh agree.
        ///
        /// **Every building, quay, shed and plinth in the library is axis
        /// aligned because the author had no choice.** A `Capsule` gets
        /// arbitrary orientation free from its two endpoints, so the engine
        /// could angle a tunnel but not a wall. One yaw covers essentially
        /// every architectural case; full Euler or a quaternion is a larger
        /// schema for cases nobody has needed.
        ///
        /// **Degrees, not radians, and the name says so.** `ops` is a
        /// `Vec<serde_json::Value>` with no field-level schema, so `yaw = 45`
        /// meant as degrees against code reading radians is a factor of 57
        /// with no error and a plausible-looking render.
        #[serde(default)]
        yaw_degrees: f32,
        /// Fillet radius, in world units. `d - r` on a box shrunk by `r`, so
        /// **`half_extents` keeps meaning the outer extent** — round it and it
        /// does not silently grow. Clamped to the smallest half-extent, which
        /// is the largest radius that can be taken out of the box's own
        /// thickness.
        ///
        /// Still a true distance field: subtracting a constant shifts every
        /// level set outward by `r` and leaves the gradient alone.
        #[serde(default)]
        round: f32,
    },
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
        mode: CsgMode,
        #[serde(default)]
        displace: Option<Displace>,
    },
    /// Ground: everything below a noise surface is solid.
    ///
    /// **The only op that fills a volume rather than placing something in it.**
    /// A terrain built from spheres and boxes needs hundreds of ops to cover
    /// ground this covers with one, and the scene file stays four lines
    /// (never-do #11: the recipe, never the voxels).
    ///
    /// Unbounded in X and Z on purpose — it is ground, and a volume is a window
    /// onto it rather than a container for it. Two volumes side by side using
    /// the same seed meet seamlessly, because the height at a world position
    /// does not depend on which volume asked.
    Heightfield {
        /// World Y the surface varies around.
        base: f32,
        /// Half the peak-to-trough swing, in world units.
        amplitude: f32,
        /// Cycles per world unit in the first octave. Lower is broader hills.
        frequency: f32,
        /// Detail levels. Each is half the amplitude and twice the frequency
        /// of the last, so past about 8 the rest is below one voxel.
        octaves: u32,
        seed: u64,
        mode: CsgMode,
    },
    /// Ground from a `loom_terrain` recipe: fBm, ridged, domain warp, spline
    /// carve, flatten disc, peak, corridor guarantee, and **the two erosion
    /// passes a 3D field cannot run at all**.
    ///
    /// [`Self::Heightfield`] reaches past the recipe system for a single
    /// function, `loom_terrain::noise::fbm`, so every landscape built on it is
    /// un-eroded fBm. This is the same op with the whole pipeline behind it:
    /// gullies where water actually ran, talus at the angle it rests at, a
    /// flattened pad that was *asserted* buildable, and a walkable route that
    /// was guaranteed rather than hoped for.
    ///
    /// It is also the only op with an assertable feedback channel:
    /// `loom terrain <scene.loom>` reports `buildable_pct`, `slope_mean`,
    /// `largest_flat` and `reachable` without rendering anything.
    ///
    /// **Bounded, unlike a heightfield** — a recipe is a finite array, so the
    /// rect is the whole of where it says anything.
    Terrain {
        /// Recipe path, relative to the scene file that names it.
        recipe: String,
        /// The world rect the map covers, `[x0, z0, x1, z1]`. Must be the size
        /// the recipe's own `world_scale` claims; [`resolve`] checks it.
        rect: [f32; 4],
        /// World Y the recipe's `height_range` is measured from.
        base_y: f32,
        mode: CsgMode,
        /// Baked by [`resolve`] at load, **never in the scene file**: it is a
        /// derived array, and a scene stores recipes (never-do #11). Absent
        /// until then, and [`Volume::bake`] refuses an op that never was.
        #[serde(skip)]
        map: Option<Arc<TerrainMap>>,
    },
}

impl VoxelOp {
    #[must_use]
    pub fn mode(&self) -> CsgMode {
        match self {
            Self::Sphere { mode, .. }
            | Self::Box { mode, .. }
            | Self::Capsule { mode, .. }
            | Self::Heightfield { mode, .. }
            | Self::Terrain { mode, .. } => *mode,
        }
    }

    /// Whether every input this op needs is in hand.
    ///
    /// False only for a [`Self::Terrain`] that never went through [`resolve`].
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        !matches!(self, Self::Terrain { map: None, .. })
    }

    /// World-space bounds this op can possibly affect.
    ///
    /// An edit costs the chunks it touches, not the chunks that exist. Without
    /// this, carving one crater walks every chunk in the volume — 4096 of them
    /// in a 512³ world, to change 34.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let (lo, hi) = self.undisplaced_bounds();
        // **Grown by the displacement's amplitude**, because an op costs the
        // chunks it touches and a displaced surface touches more of them. Left
        // ungrown, the outermost crest of a rock falls outside the ops's own
        // bounds, the bake never visits that chunk, and the shape is cleanly
        // sliced off at a chunk boundary — which looks like a modelling
        // mistake rather than a bookkeeping one.
        let grow = self.displace().map_or(0.0, |d| d.amplitude.abs());
        if grow <= 0.0 {
            return (lo, hi);
        }
        (
            [lo[0] - grow, lo[1] - grow, lo[2] - grow],
            [hi[0] + grow, hi[1] + grow, hi[2] + grow],
        )
    }

    fn undisplaced_bounds(&self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Sphere {
                center,
                radius,
                elongate,
                ..
            } => {
                // The elongation slides the surface out along each axis by its
                // own amount, so the extent is the radius plus it. Left out,
                // the far end of an elongated sphere falls outside the op's
                // own bounds and gets sliced off at a chunk boundary.
                let e = [
                    radius + elongate[0].abs(),
                    radius + elongate[1].abs(),
                    radius + elongate[2].abs(),
                ];
                (
                    [center[0] - e[0], center[1] - e[1], center[2] - e[2]],
                    [center[0] + e[0], center[1] + e[1], center[2] + e[2]],
                )
            }
            Self::Box {
                center,
                half_extents,
                yaw_degrees,
                ..
            } => {
                // **The rotated extent, in closed form** — the support function
                // of a box under a Y rotation, not a loop over eight corners.
                // `Volume::edit` culls chunk spans by this, so an AABB that
                // does not cover the rotated shape makes a runtime crater miss
                // chunks entirely and leaves a seam or a floating slab at the
                // crater's edge. `round` needs no term: it shrinks the box by
                // the same radius it then adds back.
                let (s, c) = yaw_degrees.to_radians().sin_cos();
                let (s, c) = (s.abs(), c.abs());
                let e = [
                    c.mul_add(half_extents[0], s * half_extents[2]),
                    half_extents[1],
                    s.mul_add(half_extents[0], c * half_extents[2]),
                ];
                (
                    [center[0] - e[0], center[1] - e[1], center[2] - e[2]],
                    [center[0] + e[0], center[1] + e[1], center[2] + e[2]],
                )
            }
            Self::Capsule { a, b, radius, .. } => (
                [
                    a[0].min(b[0]) - radius,
                    a[1].min(b[1]) - radius,
                    a[2].min(b[2]) - radius,
                ],
                [
                    a[0].max(b[0]) + radius,
                    a[1].max(b[1]) + radius,
                    a[2].max(b[2]) + radius,
                ],
            ),
            // Unbounded horizontally, bounded vertically. The vertical bound
            // is the useful half: it is what lets an edit skip every chunk of
            // sky above the ground and every chunk of rock below it.
            Self::Heightfield {
                base, amplitude, ..
            } => (
                [f32::MIN, base - amplitude.abs(), f32::MIN],
                [f32::MAX, base + amplitude.abs(), f32::MAX],
            ),
            // Bounded on every axis, which a heightfield is not: a recipe is a
            // finite array over a stated rect, so an edit can cull against it
            // horizontally as well as vertically.
            Self::Terrain {
                rect, base_y, map, ..
            } => {
                let (lo, hi) = map.as_ref().map_or((f32::MIN, f32::MAX), |m| {
                    (base_y + m.range.0, base_y + m.range.1)
                });
                ([rect[0], lo, rect[1]], [rect[2], hi, rect[3]])
            }
        }
    }

    /// How fast this op's distance field can change, per world unit.
    ///
    /// **`bake`'s early-out assumes a 1-Lipschitz field**: it fills a chunk in
    /// O(1) when the distance at the centre exceeds the chunk's own radius,
    /// which is only sound if distance cannot fall faster than one unit per
    /// unit travelled. Spheres, boxes and capsules are true distance fields and
    /// satisfy that, and **so do all three shape transforms**: a yaw is an
    /// isometry, an elongation maps each axis with slope at most one, and a
    /// fillet subtracts a constant, which moves every level set and no
    /// gradient. None of them appears below, and a test samples the ratio over
    /// random point pairs rather than taking that on faith.
    /// `y - height(x, z)` does not — on a steep slope it
    /// *overstates* how far away the surface is, and the early-out then skips
    /// chunks that do contain surface, punching holes in the terrain.
    ///
    /// Multiplying the early-out's threshold by this restores the guarantee.
    /// Erring high is safe in one direction only: too large means chunks are
    /// evaluated that need not be, which costs time; too small means holes,
    /// which costs correctness.
    ///
    /// Measured, not assumed. Across seven terrain configurations an
    /// unwidened early-out wrongly skipped between 9 and 44 chunks each; with
    /// this applied, none.
    #[must_use]
    pub fn lipschitz(&self) -> f32 {
        // A displacement adds its own gradient to whatever the shape's was.
        // Additive rather than multiplied: the fields are summed, so their
        // gradients are, and the bound has to hold for the sum.
        if let Some(d) = self.displace() {
            return self.undisplaced_lipschitz() + d.gradient_bound();
        }
        self.undisplaced_lipschitz()
    }

    fn undisplaced_lipschitz(&self) -> f32 {
        // Measured off the baked array rather than derived from parameters:
        // erosion, spline carves and flatten discs all change the slope after
        // the noise that set it, so there is nothing to derive it from. See
        // [`TerrainMap::new`] for why it is an exact bound and not a
        // percentile.
        if let Self::Terrain { map, .. } = self {
            return map.as_ref().map_or(1.0, |m| m.lipschitz);
        }
        let Self::Heightfield {
            amplitude,
            frequency,
            octaves,
            ..
        } = self
        else {
            return 1.0;
        };
        // `fbm` normalises by the amplitude sum, so octave i contributes
        // `gain^i / norm` of the total swing at `lacunarity^i` times the
        // frequency. The 3.0 bounds the slope of one octave of value noise
        // across its unit cell: the smoothstep used to interpolate it peaks at
        // 1.5, over a range of 2.
        let (mut amp, mut freq, mut slope, mut norm) = (1.0_f32, *frequency, 0.0_f32, 0.0_f32);
        for _ in 0..(*octaves).max(1) {
            slope += amp * freq * 3.0;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        let gradient = amplitude.abs() * slope / norm.max(1e-6);
        gradient.mul_add(gradient, 1.0).sqrt()
    }

    /// The surface height at a horizontal position, for a heightfield.
    ///
    /// Split out because it is **the only part of an op's distance that does
    /// not depend on Y**, and a chunk asks for the same column 32 times. At a
    /// billion voxels that redundancy was most of the bake.
    ///
    /// `None` for every other op: their distance depends on all three axes and
    /// there is nothing to hoist.
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        // The same hoist, and the reason it is worth as much here: one bilinear
        // lookup replaces five octaves of fBm, and a chunk asks for the same
        // column 32 times either way.
        if let Self::Terrain {
            rect, base_y, map, ..
        } = self
        {
            return map.as_ref().map(|m| base_y + m.height(*rect, x, z));
        }
        let Self::Heightfield {
            base,
            amplitude,
            frequency,
            octaves,
            seed,
            ..
        } = self
        else {
            return None;
        };
        Some(amplitude.mul_add(
            loom_terrain::noise::fbm(x, z, *frequency, *octaves, 2.0, 0.5, *seed),
            *base,
        ))
    }

    /// Signed distance from `p` to this shape, in world units.
    #[must_use]
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        // **Subtracted from the analytic distance, not blended into it.**
        // `d(p) - A*n(p)` moves the whole surface along its own normal, which
        // is what a displacement means; scaling `d` instead would move the
        // surface toward or away from the centre and turn a boulder into a
        // balloon.
        //
        // The price is that the result is no longer a true distance — the
        // gradient can exceed one — and the bake's early-out assumes it is not.
        // `lipschitz` below is what pays it.
        self.analytic_distance(p) - self.displace().map_or(0.0, |d| d.at(p))
    }

    /// The displacement attached to this op, if any.
    #[must_use]
    pub fn displace(&self) -> Option<&Displace> {
        match self {
            Self::Sphere { displace, .. }
            | Self::Box { displace, .. }
            | Self::Capsule { displace, .. } => displace.as_ref(),
            Self::Heightfield { .. } | Self::Terrain { .. } => None,
        }
    }

    /// Distance to the undisplaced shape.
    fn analytic_distance(&self, p: [f32; 3]) -> f32 {
        match self {
            Self::Sphere {
                center,
                radius,
                elongate,
                ..
            } => length(elongated(sub(p, *center), *elongate)) - radius,
            Self::Box {
                center,
                half_extents,
                yaw_degrees,
                round,
                ..
            } => {
                let d = yawed(sub(p, *center), *yaw_degrees);
                // **Shrink by the fillet radius before adding it back.** The
                // authored `half_extents` is the outer extent either way, so
                // rounding a box does not move its faces — and `bounds()` needs
                // no rounding term. The clamp is what makes that true for any
                // radius an author writes.
                let thinnest = half_extents[0].min(half_extents[1]).min(half_extents[2]);
                let r = round.max(0.0).min(thinnest.max(0.0));
                let q = [
                    d[0].abs() - (half_extents[0] - r),
                    d[1].abs() - (half_extents[1] - r),
                    d[2].abs() - (half_extents[2] - r),
                ];
                let outside = length([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
                let inside = q[0].max(q[1]).max(q[2]).min(0.0);
                outside + inside - r
            }
            Self::Capsule { a, b, radius, .. } => {
                let pa = sub(p, *a);
                let ba = sub(*b, *a);
                let h = (dot(pa, ba) / dot(ba, ba).max(1e-6)).clamp(0.0, 1.0);
                length(sub(pa, scale(ba, h))) - radius
            }
            Self::Heightfield { .. } | Self::Terrain { .. } => {
                // Height varies with X and Z; Y is up. Positive above ground.
                // `height_at` is `Some` for exactly these two variants — and
                // for an unresolved `Terrain` it is `None`, which lands on the
                // fallback below and reads as surface everywhere. `bake`
                // refuses that op rather than letting it be a shape.
                let height = self.height_at(p[0], p[2]).unwrap_or(p[1]);
                // Returned raw. The early-out's soundness is restored by
                // widening its threshold with `lipschitz`, not by shrinking
                // this: dividing here would compress the whole field toward
                // zero, and since it is quantised to an i8 that would throw
                // away the precision the mesher interpolates its zero-crossing
                // from. Correctness belongs in the test, not in the data.
                p[1] - height
            }
        }
    }
}

/// Combine one op's distance into an accumulated field value.
fn combine(accumulated: f32, op: &VoxelOp, p: [f32; 3]) -> f32 {
    accumulate(accumulated, op.mode(), op.distance(p))
}

/// As [`combine`], for a distance that has already been computed.
///
/// Split so the bake's inner loop can supply a distance it derived from a
/// cached column height instead of re-deriving it per voxel.
fn accumulate(accumulated: f32, mode: CsgMode, d: f32) -> f32 {
    match mode {
        CsgMode::Union => accumulated.min(d),
        CsgMode::Subtract => accumulated.max(-d),
        CsgMode::Intersect => accumulated.max(d),
    }
}

/// A voxel volume: chunks, the ops that produced them, and runtime edits.
#[derive(Debug, Clone)]
pub struct Volume {
    /// Baked field. Uniform chunks carry no allocation.
    chunks: Vec<Chunk>,
    /// Runtime destruction, stored **separately and sparsely**.
    ///
    /// The two-layer trick from `bevy_voxel_world` (voxel doc §1.1): a
    /// procedural or baked base, plus only the chunks a player actually
    /// changed. An untouched world costs nothing; forty craters cost forty
    /// craters, and a save file is the op list plus this map.
    edits: BTreeMap<usize, Box<[i8; VOXELS]>>,
    dims: [usize; 3],
    pub voxel_size: f32,
}

impl Volume {
    /// An empty volume `dims` chunks across.
    #[must_use]
    pub fn new(dims: [usize; 3], voxel_size: f32) -> Self {
        Self {
            chunks: vec![Chunk::Air; dims[0] * dims[1] * dims[2]],
            edits: BTreeMap::new(),
            dims,
            voxel_size,
        }
    }

    /// Integer coordinates of every solid cell, for a voxel collider.
    ///
    /// Solid is `sdf < 0` — the same sign the mesher and the bake early-out
    /// use, so the collider and the drawn surface agree on what is inside.
    ///
    /// Returned as grid coordinates rather than world positions because that
    /// is what parry's voxel shape wants, and because it places cell `k` at
    /// `(k + 0.5) * voxel_size` — identical to [`Self::world_of`]. The test
    /// `voxel_grid_matches_parry` pins those two conventions together; if
    /// either side ever changes, the collider would silently sit half a cell
    /// away from the geometry.
    #[must_use]
    pub fn solid_cells(&self) -> Vec<[i32; 3]> {
        let [rx, ry, rz] = self.resolution();
        let mut cells = Vec::new();
        for z in 0..rz {
            for y in 0..ry {
                for x in 0..rx {
                    if self.voxel(x, y, z) < 0 {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                        cells.push([x as i32, y as i32, z as i32]);
                    }
                }
            }
        }
        cells
    }

    /// Whether any of the six neighbouring chunks is solid where this one is
    /// air, or the reverse — i.e. whether a surface runs along a shared face.
    ///
    /// A `Detailed` neighbour counts too: its surface may reach the boundary,
    /// and meshing one extra chunk is cheaper than a hole in the world.
    fn neighbour_differs(&self, cx: usize, cy: usize, cz: usize) -> bool {
        let solid = matches!(self.chunks[self.chunk_index(cx, cy, cz)], Chunk::Solid);
        let air = matches!(self.chunks[self.chunk_index(cx, cy, cz)], Chunk::Air);
        if !solid && !air {
            return false;
        }
        let offsets: [[isize; 3]; 6] = [
            [-1, 0, 0], [1, 0, 0],
            [0, -1, 0], [0, 1, 0],
            [0, 0, -1], [0, 0, 1],
        ];
        for [dx, dy, dz] in offsets {
            let (Some(nx), Some(ny), Some(nz)) = (
                cx.checked_add_signed(dx),
                cy.checked_add_signed(dy),
                cz.checked_add_signed(dz),
            ) else {
                continue;
            };
            if nx >= self.dims[0] || ny >= self.dims[1] || nz >= self.dims[2] {
                continue;
            }
            match &self.chunks[self.chunk_index(nx, ny, nz)] {
                Chunk::Detailed(_) => return true,
                Chunk::Solid if air => return true,
                Chunk::Air if solid => return true,
                _ => {}
            }
        }
        false
    }

    /// Voxels per axis.
    #[must_use]
    pub fn resolution(&self) -> [usize; 3] {
        [
            self.dims[0] * CHUNK,
            self.dims[1] * CHUNK,
            self.dims[2] * CHUNK,
        ]
    }

    /// Bake an op list into the field.
    ///
    /// **Chunk-major with an early-out.** A chunk whose centre is further from
    /// every op than the chunk's own bounding radius cannot contain a surface,
    /// so it is filled in O(1) rather than evaluating 32,768 voxels. On a
    /// sphere in a 256³ volume that skips the overwhelming majority of chunks,
    /// and it is the difference between milliseconds and seconds.
    ///
    /// Order matters: subtract-then-union is not union-then-subtract.
    ///
    /// # Panics
    /// If a [`VoxelOp::Terrain`] never went through [`resolve`]. That is a
    /// missing call in engine code rather than a bad scene, and it is loud
    /// here because it is silent everywhere else: an op with no map answers
    /// "surface" at every point, which bakes a full volume of plausible
    /// nonsense and validates clean. It is the prefab defect and the dropped-op
    /// defect in a third costume.
    pub fn bake(&mut self, ops: &[VoxelOp]) {
        assert!(
            ops.iter().all(VoxelOp::is_resolved),
            "a `terrain` op reached bake unresolved — call `loom_voxel::resolve(ops, base)` \
             after parsing a scene's op list",
        );
        #[allow(clippy::cast_precision_loss)]
        // Half-diagonal of a chunk in world units — the furthest any voxel in
        // it can be from its centre.
        let chunk_radius = (CHUNK as f32 * 0.5 * self.voxel_size) * 1.7320508;
        // How fast the steepest op's field can change. For spheres, boxes and
        // capsules this is 1 and the threshold is unchanged; a heightfield is
        // steeper than a true distance field and needs more room. Taking the
        // max over ops is right because min/max combination cannot make the
        // result vary faster than its fastest input.
        let reach = ops
            .iter()
            .map(VoxelOp::lipschitz)
            .fold(1.0_f32, f32::max)
            * chunk_radius;

        for cz in 0..self.dims[2] {
            for cy in 0..self.dims[1] {
                for cx in 0..self.dims[0] {
                    let index = self.chunk_index(cx, cy, cz);
                    let centre = self.world_of(
                        cx * CHUNK + CHUNK / 2,
                        cy * CHUNK + CHUNK / 2,
                        cz * CHUNK + CHUNK / 2,
                    );

                    let mut d = f32::MAX;
                    for op in ops {
                        d = combine(d, op, centre);
                    }

                    // THE EARLY-OUT. Conservative: only skips when the whole
                    // chunk is provably on one side of every surface.
                    if d.abs() > reach {
                        self.chunks[index] = if d < 0.0 { Chunk::Solid } else { Chunk::Air };
                        continue;
                    }

                    self.bake_chunk(index, cx, cy, cz, ops);
                }
            }
        }
    }

    /// Evaluate every voxel in one chunk. Only reached for chunks that might
    /// hold a surface.
    fn bake_chunk(&mut self, index: usize, cx: usize, cy: usize, cz: usize, ops: &[VoxelOp]) {
        let mut voxels = Box::new([127_i8; VOXELS]);
        let voxel_size = self.voxel_size;
        let mut any_solid = false;
        let mut any_air = false;

        // **Column-major, and Y innermost.** A heightfield's surface height
        // depends on X and Z only, so evaluating it per voxel computes the
        // same fBm 32 times over. Hoisting it to the column turned a
        // billion-voxel bake from 17 s into something usable — it was most of
        // the time at that scale, and invisible at the sizes tested before.
        //
        // Writes stride by 32 bytes instead of running sequentially, which
        // costs nothing measurable: a chunk is 32 KB and stays in cache for
        // the whole of this loop either way.
        let mut column = vec![0.0_f32; ops.len()];

        for z in 0..CHUNK {
            for x in 0..CHUNK {
                #[allow(clippy::cast_precision_loss)]
                let (px, pz) = (
                    ((cx * CHUNK + x) as f32 + 0.5) * voxel_size,
                    ((cz * CHUNK + z) as f32 + 0.5) * voxel_size,
                );
                for (slot, op) in column.iter_mut().zip(ops) {
                    // `None` for every op whose distance needs all three axes;
                    // the value is then unused and the op is asked directly.
                    *slot = op.height_at(px, pz).unwrap_or(f32::NAN);
                }

                for y in 0..CHUNK {
                    #[allow(clippy::cast_precision_loss)]
                    let py = ((cy * CHUNK + y) as f32 + 0.5) * voxel_size;
                    let p = [px, py, pz];
                    let mut d = f32::MAX;
                    for (height, op) in column.iter().zip(ops) {
                        // A cached height is the whole of a heightfield's
                        // distance; anything else re-evaluates in full.
                        let raw = if height.is_nan() {
                            op.distance(p)
                        } else {
                            py - height
                        };
                        d = accumulate(d, op.mode(), raw);
                    }
                    // Distance is stored in VOXELS, not world units: the i8
                    // range is ±1 voxel, so scaling by world size would
                    // saturate everything for any voxel_size below 1.
                    let q = quantize(d / voxel_size);
                    if q < 0 {
                        any_solid = true;
                    } else {
                        any_air = true;
                    }
                    voxels[index_local(x, y, z)] = q;
                }
            }
        }

        // Collapse immediately rather than allocating and then freeing: the
        // early-out is conservative, so some chunks it admits turn out uniform.
        self.chunks[index] = match (any_solid, any_air) {
            (true, false) => Chunk::Solid,
            (false, true) => Chunk::Air,
            _ => Chunk::Detailed(voxels),
        };
    }

    /// Carve or add at runtime, recording only the chunks actually touched.
    ///
    /// This is the destruction path, and it is why the edit layer exists: a
    /// crater costs the chunks it intersects, not a rewritten volume.
    ///
    /// Returns the chunk coordinates that changed, so a caller remeshes only
    /// those — **and their neighbours**, which is the caller's job and the
    /// single most common bug in these systems (§7.9).
    pub fn edit(&mut self, op: &VoxelOp) -> Vec<[usize; 3]> {
        #[allow(clippy::cast_precision_loss)]
        let chunk_radius = (CHUNK as f32 * 0.5 * self.voxel_size) * 1.7320508;
        let mut touched = Vec::new();

        // Only the chunks the op's bounds reach. This is what makes an edit
        // cost proportional to the EDIT rather than to the world: a crater in
        // a 512³ volume visits a handful of chunks instead of all 4096.
        let (lo, hi) = op.bounds();
        let span = self.chunk_span(lo, hi);

        for cz in span[2].0..span[2].1 {
            for cy in span[1].0..span[1].1 {
                for cx in span[0].0..span[0].1 {
                    let centre = self.world_of(
                        cx * CHUNK + CHUNK / 2,
                        cy * CHUNK + CHUNK / 2,
                        cz * CHUNK + CHUNK / 2,
                    );
                    // Second, tighter filter: a chunk inside the AABB but
                    // outside the shape still costs one distance evaluation,
                    // not 32,768.
                    //
                    // **Widened by `lipschitz`, exactly as `bake` does.** This
                    // tested the raw chunk radius, which is only sound for a
                    // 1-Lipschitz field: for `|grad f| <= L`, a chunk of radius
                    // R is provably surface-free only when `|f(centre)| > L*R`.
                    // It went unnoticed while every runtime carve was a plain
                    // sphere — for which L is 1 and the two are the same
                    // expression — and a displaced op is what makes it
                    // reachable. The symptom is a chunk at a crater's edge that
                    // holds surface and is never remeshed, so a slab hangs in
                    // the air.
                    if op.distance(centre).abs() > chunk_radius * op.lipschitz() {
                        continue;
                    }

                    let index = self.chunk_index(cx, cy, cz);
                    // Whether this chunk already carried damage. Taking the
                    // entry out and only putting it back `if changed` meant an
                    // op that altered nothing here — a second identical carve,
                    // or one that merely clips an already-hollow chunk —
                    // dropped every earlier edit and the terrain healed itself.
                    let had_edits = self.edits.contains_key(&index);
                    let mut voxels = self.edits.remove(&index).unwrap_or_else(|| {
                        let mut v = Box::new([127_i8; VOXELS]);
                        for z in 0..CHUNK {
                            for y in 0..CHUNK {
                                for x in 0..CHUNK {
                                    v[index_local(x, y, z)] = self.chunks[index].get(x, y, z);
                                }
                            }
                        }
                        v
                    });

                    // Only the voxels inside the op's bounds. A chunk the
                    // edit merely clips costs the clipped corner, not 32,768
                    // distance evaluations.
                    let local = |axis: usize, base: usize| -> (usize, usize) {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let first = (lo[axis] / self.voxel_size).floor().max(0.0) as usize;
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let last = (hi[axis] / self.voxel_size).ceil().max(0.0) as usize + 1;
                        (
                            first.saturating_sub(base).min(CHUNK),
                            last.saturating_sub(base).min(CHUNK),
                        )
                    };
                    let (x0, x1) = local(0, cx * CHUNK);
                    let (y0, y1) = local(1, cy * CHUNK);
                    let (z0, z1) = local(2, cz * CHUNK);

                    let mut changed = false;
                    for z in z0..z1 {
                        for y in y0..y1 {
                            for x in x0..x1 {
                                let p = self.world_of(
                                    cx * CHUNK + x,
                                    cy * CHUNK + y,
                                    cz * CHUNK + z,
                                );
                                let existing = dequantize(voxels[index_local(x, y, z)]);
                                let d = combine(
                                    existing * self.voxel_size,
                                    op,
                                    p,
                                );
                                let q = quantize(d / self.voxel_size);
                                if q != voxels[index_local(x, y, z)] {
                                    voxels[index_local(x, y, z)] = q;
                                    changed = true;
                                }
                            }
                        }
                    }

                    if changed || had_edits {
                        self.edits.insert(index, voxels);
                    }
                    // Only a real change dirties the mesh. Re-meshing a chunk
                    // nothing happened to would be pure cost.
                    if changed {
                        touched.push([cx, cy, cz]);
                    }
                }
            }
        }
        touched
    }

    /// Chunk index range covering a world-space AABB, clamped to the volume.
    fn chunk_span(&self, lo: [f32; 3], hi: [f32; 3]) -> [(usize, usize); 3] {
        let mut span = [(0, 0); 3];
        for axis in 0..3 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let first = (lo[axis] / self.voxel_size / CHUNK as f32).floor().max(0.0) as usize;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let last = ((hi[axis] / self.voxel_size / CHUNK as f32).ceil().max(0.0) as usize + 1)
                .min(self.dims[axis]);
            span[axis] = (first.min(self.dims[axis]), last);
        }
        span
    }

    /// Chunks that must be remeshed after `touched` changed.
    ///
    /// **This is §7.9's other half.** Surface extraction reads one voxel past
    /// the boundary, so an edit dirties the touched chunk *and its neighbours*
    /// — miss that and you get cracks at the seams of every edit, which is the
    /// single most common bug in these systems.
    #[must_use]
    pub fn dirty_with_neighbours(&self, touched: &[[usize; 3]]) -> Vec<[usize; 3]> {
        let mut out: std::collections::BTreeSet<[usize; 3]> = std::collections::BTreeSet::new();
        for [cx, cy, cz] in touched {
            for dz in -1_isize..=1 {
                for dy in -1_isize..=1 {
                    for dx in -1_isize..=1 {
                        let (Some(nx), Some(ny), Some(nz)) = (
                            cx.checked_add_signed(dx),
                            cy.checked_add_signed(dy),
                            cz.checked_add_signed(dz),
                        ) else {
                            continue;
                        };
                        if nx < self.dims[0] && ny < self.dims[1] && nz < self.dims[2] {
                            out.insert([nx, ny, nz]);
                        }
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// World position of a voxel's centre.
    #[must_use]
    pub fn world_of(&self, x: usize, y: usize, z: usize) -> [f32; 3] {
        #[allow(clippy::cast_precision_loss)]
        [
            (x as f32 + 0.5) * self.voxel_size,
            (y as f32 + 0.5) * self.voxel_size,
            (z as f32 + 0.5) * self.voxel_size,
        ]
    }

    /// Distance at a voxel. **The edit layer always wins over the base.**
    #[must_use]
    pub fn voxel(&self, x: usize, y: usize, z: usize) -> i8 {
        let [rx, ry, rz] = self.resolution();
        if x >= rx || y >= ry || z >= rz {
            // Outside is air. Solid would seal every volume in a shell.
            return 127;
        }
        let index = self.chunk_index(x / CHUNK, y / CHUNK, z / CHUNK);
        let (lx, ly, lz) = (x % CHUNK, y % CHUNK, z % CHUNK);
        self.edits
            .get(&index)
            .map_or_else(|| self.chunks[index].get(lx, ly, lz), |e| e[index_local(lx, ly, lz)])
    }

    fn chunk_index(&self, cx: usize, cy: usize, cz: usize) -> usize {
        cx + cy * self.dims[0] + cz * self.dims[0] * self.dims[1]
    }

    /// Chunks holding a surface. Uniform chunks are skipped in O(1).
    #[must_use]
    pub fn surface_chunks(&self) -> Vec<[usize; 3]> {
        let mut out = Vec::new();
        for cz in 0..self.dims[2] {
            for cy in 0..self.dims[1] {
                for cx in 0..self.dims[0] {
                    let index = self.chunk_index(cx, cy, cz);
                    // A uniform chunk still borders a surface when a neighbour
                    // is uniformly the other thing: the face between them is
                    // the surface. Testing only `has_surface` skipped both
                    // sides, so a wall that happened to land exactly on a
                    // 32-voxel line came out invisible.
                    let borders_a_seam = self.chunks[index].has_surface()
                        || self.neighbour_differs(cx, cy, cz);
                    if borders_a_seam || self.edits.contains_key(&index) {
                        out.push([cx, cy, cz]);
                    }
                }
            }
        }
        out
    }

    /// Heap bytes actually used, versus a dense `i8` field.
    ///
    /// Reported rather than estimated, so the sparsity claim is measurable —
    /// and it is asserted in a test.
    #[must_use]
    pub fn memory(&self) -> (usize, usize) {
        let used: usize = self.chunks.iter().map(Chunk::heap_bytes).sum::<usize>()
            + self.edits.len() * VOXELS;
        (used, self.chunks.len() * VOXELS)
    }
}

fn index_local(x: usize, y: usize, z: usize) -> usize {
    index(x, y, z)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Rotate a point *into* a shape yawed by `degrees` about the Y axis.
///
/// The shape turns the way `glam::Quat::from_rotation_y(degrees.to_radians())`
/// turns a node, which is the only reason to prefer one sign over the other:
/// an agent that yaws a voxel box and a mesh box by the same number gets the
/// same heading from both. Sampling applies the *inverse*, hence the transpose.
///
/// The early return is not only for cost — it makes an unrotated box
/// bit-identical to the one authored before `yaw_degrees` existed, rather than
/// identical up to `sin(0.0)`.
fn yawed(d: [f32; 3], degrees: f32) -> [f32; 3] {
    if degrees == 0.0 {
        return d;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    [c.mul_add(d[0], -s * d[2]), d[1], s.mul_add(d[0], c * d[2])]
}

/// Elongation: `p - clamp(p, -h, h)`, which stretches a shape along each axis
/// by `h` **without distorting it**. The interval `[-h, h]` collapses to the
/// origin, so every point maps to the closest point of the un-elongated shape's
/// own frame, and distance is preserved exactly rather than approximately.
fn elongated(d: [f32; 3], h: [f32; 3]) -> [f32; 3] {
    if h == [0.0; 3] {
        return d;
    }
    let (hx, hy, hz) = (h[0].abs(), h[1].abs(), h[2].abs());
    [
        d[0] - d[0].clamp(-hx, hx),
        d[1] - d[1].clamp(-hy, hy),
        d[2] - d[2].clamp(-hz, hz),
    ]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    /// A seeded xorshift returning `[-1, 1)`, for the property tests below.
    ///
    /// Seeded rather than `thread_rng` (never-do #7) so a failure is one that
    /// can be re-run, and hand-rolled rather than a dependency because the
    /// whole of it is three shifts.
    fn unit_random(seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            #[allow(clippy::cast_precision_loss)]
            {
                ((state >> 40) as f32) / 8_388_608.0 - 1.0
            }
        }
    }

    /// A surface lying exactly on a chunk boundary is still a surface. Both
    /// sides collapse to uniform — one all solid, one all air — so neither
    /// reported `has_surface`, `surface_chunks` skipped both, and the shared
    /// face was never meshed. A flat wall that happens to land on a 32-voxel
    /// line came out invisible, and invisible geometry is the hardest kind of
    /// bug to attribute.
    #[test]
    fn a_surface_on_a_chunk_boundary_is_meshed() {
        let voxel_size = 0.25;
        let boundary = super::CHUNK as f32 * voxel_size;

        // Fills chunk 0 exactly and stops on the seam with chunk 1.
        let mut volume = super::Volume::new([2, 1, 1], voxel_size);
        volume.bake(&[super::VoxelOp::Box {
            center: [boundary * 0.5, boundary * 0.5, boundary * 0.5],
            half_extents: [boundary * 0.5, boundary * 0.5, boundary * 0.5],
            mode: super::CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        }]);

        let chunks = volume.surface_chunks();
        assert!(
            !chunks.is_empty(),
            "the seam between a solid chunk and an empty one is a surface"
        );

        let mesh = super::mesh::mesh_volume(&volume, &super::SurfaceNets);
        assert!(
            !mesh.indices.is_empty(),
            "a wall on a chunk boundary must produce geometry"
        );
    }

    /// **A second edit must not undo the first.** `edit` took a chunk's stored
    /// edits out of the map up front and only put them back when *this* op
    /// changed something — so an op that clipped an already-carved chunk
    /// without altering a voxel dropped the earlier damage and the terrain
    /// healed itself. Applying the same carve twice is the simplest way to
    /// reach it: the second one is a no-op by definition.
    #[test]
    fn a_no_op_edit_does_not_erase_earlier_damage() {
        let carve = super::VoxelOp::Sphere {
            center: [4.0, 4.0, 4.0],
            radius: 1.5,
            mode: super::CsgMode::Subtract,
            displace: None,
            elongate: [0.0; 3],
        };
        let fill = super::VoxelOp::Box {
            center: [4.0, 4.0, 4.0],
            half_extents: [4.0, 4.0, 4.0],
            mode: super::CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        };

        let mut volume = super::Volume::new([1, 1, 1], 0.25);
        volume.bake(&[fill]);
        volume.edit(&carve);
        let after_first = volume.solid_cells().len();

        // Exactly the same carve again: nothing left to remove.
        volume.edit(&carve);
        let after_second = volume.solid_cells().len();

        assert_eq!(
            after_first, after_second,
            "the crater healed: {after_first} solid cells became {after_second}"
        );
    }

    /// parry places voxel cell `k` at `(k + 0.5) * voxel_size`. So do we. If
    /// either convention drifts the collider sits half a cell from the mesh —
    /// a gap or an overlap that looks like a physics bug and is not one.
    #[test]
    fn voxel_grid_matches_parry() {
        let volume = super::Volume::new([1, 1, 1], 0.25);
        for cell in [[0_usize, 0, 0], [3, 5, 7], [31, 31, 31]] {
            let ours = volume.world_of(cell[0], cell[1], cell[2]);
            #[allow(clippy::cast_precision_loss)]
            let parry = [
                (cell[0] as f32 + 0.5) * 0.25,
                (cell[1] as f32 + 0.5) * 0.25,
                (cell[2] as f32 + 0.5) * 0.25,
            ];
            assert_eq!(ours, parry, "cell {cell:?}");
        }
    }

    use super::*;

    fn sphere(center: [f32; 3], radius: f32, mode: CsgMode) -> VoxelOp {
        VoxelOp::Sphere {
            center,
            radius,
            mode,
            displace: None,
            elongate: [0.0; 3],
        }
    }

    #[test]
    fn quantisation_round_trips_within_one_step() {
        for step in -100_i8..=100 {
            let d = f32::from(step) * 0.01;
            let back = dequantize(quantize(d));
            assert!((back - d).abs() <= SDF_SCALE, "{d} -> {back}");
        }
    }

    #[test]
    fn a_uniform_chunk_carries_no_allocation() {
        assert_eq!(Chunk::Air.heap_bytes(), 0);
        assert_eq!(Chunk::Solid.heap_bytes(), 0);
        assert_eq!(Chunk::Air.get(5, 5, 5), 127);
        assert_eq!(Chunk::Solid.get(0, 0, 0), -127);
    }

    /// Writing the value a uniform chunk already holds must not allocate, or
    /// baking promotes every chunk it merely visits.
    #[test]
    fn writing_the_existing_fill_does_not_promote() {
        let mut chunk = Chunk::Air;
        chunk.set(1, 2, 3, 127);
        assert!(!chunk.has_surface());

        chunk.set(1, 2, 3, -10);
        assert!(chunk.has_surface(), "a different value must promote");
    }

    #[test]
    fn a_carved_then_refilled_chunk_collapses_again() {
        let mut chunk = Chunk::Solid;
        chunk.set(4, 4, 4, 100);
        assert!(chunk.has_surface());

        chunk.set(4, 4, 4, -127);
        chunk.collapse();
        assert_eq!(chunk, Chunk::Solid);
    }

    /// **The efficiency claim, measured.** A sphere in a 256³ volume touches
    /// only the shell of chunks its surface crosses; everything inside and
    /// outside collapses to a discriminant.
    #[test]
    fn a_sphere_uses_a_small_fraction_of_dense_storage() {
        let mut volume = Volume::new([8, 8, 8], 0.25);
        volume.bake(&[sphere([32.0, 32.0, 32.0], 20.0, CsgMode::Union)]);

        let (used, dense) = volume.memory();
        #[allow(clippy::cast_precision_loss)]
        let ratio = used as f32 / dense as f32;
        assert!(
            ratio < 0.5,
            "sphere used {:.1}% of dense storage ({used} of {dense} bytes)", ratio * 100.0
        );
        assert!(used > 0, "the surface must be stored somewhere");
    }

    /// An empty volume must cost nothing at all — the case a streamed world
    /// hits constantly.
    #[test]
    fn an_untouched_volume_allocates_nothing() {
        let volume = Volume::new([8, 8, 8], 0.25);
        assert_eq!(volume.memory().0, 0);
    }

    /// **The two-layer trick.** Runtime destruction costs the chunks it
    /// touches, not a rewritten volume (voxel doc §1.1).
    #[test]
    fn a_crater_costs_only_the_chunks_it_touches() {
        let mut volume = Volume::new([8, 8, 8], 0.25);
        volume.bake(&[sphere([32.0, 32.0, 32.0], 20.0, CsgMode::Union)]);
        let before = volume.memory().0;

        let touched = volume.edit(&sphere([32.0, 52.0, 32.0], 3.0, CsgMode::Subtract));

        assert!(!touched.is_empty(), "the crater must change something");
        assert!(
            touched.len() <= 8,
            "a 3-unit crater should touch a handful of chunks, touched {}",
            touched.len()
        );
        let after = volume.memory().0;
        assert!(
            after - before <= touched.len() * CHUNK * CHUNK * CHUNK,
            "an edit must cost only its own chunks"
        );
    }

    /// The early-out must be conservative: skipping a chunk that *does* hold a
    /// surface would punch holes in the mesh.
    #[test]
    fn the_early_out_never_skips_a_chunk_holding_a_surface() {
        let mut volume = Volume::new([4, 4, 4], 0.5);
        volume.bake(&[sphere([32.0, 32.0, 32.0], 14.0, CsgMode::Union)]);

        // Walk every voxel; any sign change between neighbours must fall
        // inside a chunk marked as holding a surface.
        let [rx, ry, rz] = volume.resolution();
        let surface: std::collections::BTreeSet<[usize; 3]> =
            volume.surface_chunks().into_iter().collect();
        for z in 0..rz - 1 {
            for y in 0..ry - 1 {
                for x in 0..rx - 1 {
                    let here = volume.voxel(x, y, z) < 0;
                    if here != (volume.voxel(x + 1, y, z) < 0)
                        || here != (volume.voxel(x, y + 1, z) < 0)
                        || here != (volume.voxel(x, y, z + 1) < 0)
                    {
                        let c = [x / CHUNK, y / CHUNK, z / CHUNK];
                        assert!(
                            surface.contains(&c),
                            "sign change at ({x},{y},{z}) is in chunk {c:?}, which was skipped"
                        );
                    }
                }
            }
        }
    }

    /// **Op order is not commutative**, and an agent will assume it is.
    #[test]
    fn op_order_changes_the_result() {
        let ball = sphere([8.0, 8.0, 8.0], 4.0, CsgMode::Union);
        let bite = sphere([10.0, 8.0, 8.0], 2.5, CsgMode::Subtract);

        let mut carved = Volume::new([1, 1, 1], 0.5);
        carved.bake(&[ball.clone(), bite.clone()]);
        let mut refilled = Volume::new([1, 1, 1], 0.5);
        refilled.bake(&[bite, ball]);

        let probe = 20;
        assert_ne!(
            carved.voxel(probe, 16, 16) < 0,
            refilled.voxel(probe, 16, 16) < 0,
            "subtract-then-union must differ from union-then-subtract"
        );
    }

    /// §7.9's dirty-neighbour rule: an edit dirties its chunk AND the
    /// neighbours, because extraction reads one voxel past the boundary.
    /// Missing this puts a crack at the seam of every edit.
    #[test]
    fn a_dirty_chunk_drags_its_neighbours_with_it() {
        let volume = Volume::new([4, 4, 4], 0.25);

        let dirty = volume.dirty_with_neighbours(&[[1, 1, 1]]);

        assert_eq!(dirty.len(), 27, "a 3x3x3 block around the edit");
        assert!(dirty.contains(&[0, 0, 0]) && dirty.contains(&[2, 2, 2]));
    }

    /// At a corner the neighbourhood is clipped, not wrapped — wrapping would
    /// dirty chunks on the far side of the volume.
    #[test]
    fn the_neighbourhood_clips_at_the_volume_edge() {
        let volume = Volume::new([4, 4, 4], 0.25);

        let dirty = volume.dirty_with_neighbours(&[[0, 0, 0]]);

        assert_eq!(dirty.len(), 8, "a corner has 8, not 27");
        assert!(!dirty.iter().any(|c| c[0] >= 4 || c[1] >= 4 || c[2] >= 4));
    }

    #[test]
    fn an_edit_wins_over_the_baked_base() {
        let mut volume = Volume::new([2, 2, 2], 0.5);
        volume.bake(&[sphere([16.0, 16.0, 16.0], 10.0, CsgMode::Union)]);
        assert!(volume.voxel(32, 32, 32) < 0, "centre starts solid");

        volume.edit(&sphere([16.0, 16.0, 16.0], 6.0, CsgMode::Subtract));

        assert!(volume.voxel(32, 32, 32) > 0, "the edit must show through");
    }

    #[test]
    fn outside_the_volume_reads_as_air_not_solid() {
        let volume = Volume::new([1, 1, 1], 0.5);
        assert!(volume.voxel(999, 0, 0) > 0, "solid would seal it in a shell");
    }

    /// The baked field must agree with the op it was baked from.
    ///
    /// This is the test that catches an unnormalised heightfield. `bake` fills
    /// a chunk in O(1) when the distance at its centre exceeds the chunk's own
    /// radius — sound only for a field that cannot fall faster than one unit
    /// per unit travelled. `y - h(x, z)` falls faster than that on a slope, so
    /// the early-out decides a chunk is far from any surface and fills it
    /// solid or empty when it is neither. The result is chunk-sized bubbles of
    /// wrong material buried in the terrain.
    ///
    /// **Two earlier versions of this test passed with the fix removed.** One
    /// used a volume too small for the early-out to fire at all; the other
    /// asserted that every column has a surface somewhere, which a bubble does
    /// not disturb — the ground above and below it is still there. Sign
    /// agreement against the source op is the property that actually holds,
    /// and these parameters were measured to break it 44 times without the fix.
    /// **The same property the heightfield test guards, for the other field
    /// that is not 1-Lipschitz.**
    ///
    /// A displaced primitive's gradient is the shape's plus the noise's, so
    /// the bake's O(1) early-out — fill this chunk solid or empty because the
    /// distance at its centre exceeds its radius — is unsound unless the
    /// threshold is widened by `lipschitz`. Unwidened, it decides a chunk near
    /// a deep cleft is far from any surface and fills it, and the rock comes
    /// out with chunk-sized bites taken from it.
    ///
    /// **The widening is precautionary and I could not prove it necessary.**
    /// Stubbing `gradient_bound` to zero leaves this test passing, at these
    /// parameters and at three others tried, including a geometry constructed
    /// so several chunk centres land in the window where the early-out could
    /// be wrong. The reason is probably that a displaced primitive's error is
    /// bounded by its *amplitude* — the surface never moves further than that
    /// — so the tight bound is additive (`chunk_radius + 2 * amplitude`)
    /// rather than the multiplicative one `lipschitz` expresses.
    ///
    /// It is kept because it errs in the safe direction and measures free:
    /// 0.8 s against 0.7 s to bake a 590k-voxel rock. Said plainly rather than
    /// dressed up as a verified guard, because a comment claiming a mutation
    /// check that does not fire is worse than no comment.
    #[test]
    fn a_displaced_sphere_agrees_with_the_op_it_came_from() {
        let op = VoxelOp::Sphere {
            center: [3.2, 3.2, 3.2],
            radius: 1.2,
            mode: CsgMode::Union,
            displace: Some(Displace {
                amplitude: 0.5,
                frequency: 2.0,
                octaves: 4,
                seed: 0x51D3,
                ridged: true,
            }),
            elongate: [0.0; 3],
        };
        let mut volume = Volume::new([4, 4, 4], 0.05);
        volume.bake(std::slice::from_ref(&op));

        let [rx, ry, rz] = volume.resolution();
        let mut disagreements = Vec::new();
        for z in (0..rz).step_by(3) {
            for y in (0..ry).step_by(3) {
                for x in (0..rx).step_by(3) {
                    let world = volume.world_of(x, y, z);
                    let truth = op.distance(world);
                    // As the heightfield test: the i8 quantisation makes a
                    // voxel within a cell of the boundary legitimately either
                    // sign.
                    if truth.abs() < volume.voxel_size * 2.0 {
                        continue;
                    }
                    if (volume.voxel(x, y, z) < 0) != (truth < 0.0) {
                        disagreements.push((x, y, z, truth));
                    }
                }
            }
        }
        assert!(
            disagreements.is_empty(),
            "{} sampled voxels disagree with the displaced op, e.g. {:?}",
            disagreements.len(),
            &disagreements[..disagreements.len().min(4)]
        );
    }

    /// A displacement must actually move the surface, and the shape must still
    /// be a shape.
    ///
    /// The second half is the one worth having: a displacement larger than the
    /// radius turns a sphere inside out, and the useful range is bounded by
    /// that. This asserts the surface moved by something comparable to the
    /// amplitude rather than merely being different.
    #[test]
    fn displacement_moves_the_surface_by_about_its_amplitude() {
        let plain = VoxelOp::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        };
        let bumpy = VoxelOp::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
            mode: CsgMode::Union,
            displace: Some(Displace {
                amplitude: 0.4,
                frequency: 0.8,
                octaves: 4,
                seed: 7,
                ridged: false,
            }),
            elongate: [0.0; 3],
        };

        let mut worst: f32 = 0.0;
        let mut any_moved = false;
        for i in 0_i16..40 {
            let t = f32::from(i) * 0.157;
            let p = [t.cos() * 2.0, (t * 0.7).sin() * 2.0, t.sin() * 2.0];
            let delta = (bumpy.distance(p) - plain.distance(p)).abs();
            worst = worst.max(delta);
            if delta > 1e-4 {
                any_moved = true;
            }
        }
        assert!(any_moved, "displacement changed nothing");
        assert!(
            worst <= 0.4 + 1e-3,
            "displacement exceeded its own amplitude: {worst}"
        );
        assert!(worst > 0.05, "displacement barely moved anything: {worst}");
    }

    /// **The bounds have to grow or the bake clips the rock at a chunk edge.**
    ///
    /// An op costs the chunks it touches; a displaced surface touches more of
    /// them than the analytic shape does.
    #[test]
    fn displaced_bounds_grow_by_the_amplitude() {
        let plain = VoxelOp::Sphere {
            center: [1.0, 2.0, 3.0],
            radius: 1.0,
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        };
        let bumpy = VoxelOp::Sphere {
            center: [1.0, 2.0, 3.0],
            radius: 1.0,
            mode: CsgMode::Union,
            displace: Some(Displace {
                amplitude: 0.5,
                frequency: 1.0,
                octaves: 3,
                seed: 1,
                ridged: true,
            }),
            elongate: [0.0; 3],
        };
        let (plo, phi) = plain.bounds();
        let (blo, bhi) = bumpy.bounds();
        for axis in 0..3 {
            assert!((plo[axis] - blo[axis] - 0.5).abs() < 1e-6);
            assert!((bhi[axis] - phi[axis] - 0.5).abs() < 1e-6);
        }
    }

    /// An absent displacement must be exactly the identity.
    ///
    /// **This is what keeps every scene authored before the field existed
    /// producing bit-identical voxels**, and therefore keeps the pinned sim
    /// hash where it is. If this fails, every terrain scene in the library
    /// moved.
    #[test]
    fn no_displacement_is_bit_identical_to_before() {
        let plain = VoxelOp::Sphere {
            center: [0.5, 1.5, 2.5],
            radius: 1.75,
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        };
        for i in 0_i16..64 {
            let t = f32::from(i) * 0.31;
            let p = [t.cos() * 3.0, t * 0.05, t.sin() * 3.0];
            let analytic = length(sub(p, [0.5, 1.5, 2.5])) - 1.75;
            assert_eq!(plain.distance(p).to_bits(), analytic.to_bits());
        }
        assert!((plain.lipschitz() - 1.0).abs() < 1e-9);
    }

    /// **The number `Displace` is judged by, and the test that keeps it
    /// honest.** `loom_asset::shape` measures concave surface-area fraction —
    /// the statistic that separates fractured stone from a smooth blob, since a
    /// union of analytic primitives bulges outward everywhere and creases
    /// inward only on a curve. A plain sphere is convex by definition, so it is
    /// the exact zero this asserts against.
    ///
    /// **This fires.** Stubbing `Displace::at` to return `0.0` takes the
    /// displaced rock from 27.3% concave to the plain sphere's 0.0% and the
    /// assertion fails — run, not assumed. It is the only test in the
    /// workspace that would notice `Displace` quietly becoming the identity on
    /// the *surface* rather than on the field.
    ///
    /// Deliberately not a threshold on a *target*: see
    /// `crates/loom_voxel/examples/rockcalib.rs` and `loom measure --shape` for
    /// the calibration, and the rule that the number is resolution-dependent
    /// and must never become a gate.
    #[test]
    fn displacement_makes_a_surface_concave_and_a_plain_sphere_never_is() {
        let rock = |displace| {
            let mut volume = Volume::new([2, 2, 2], 0.05);
            volume.bake(&[VoxelOp::Sphere {
                center: [1.6, 1.6, 1.6],
                radius: 1.0,
                mode: CsgMode::Union,
                displace,
                elongate: [0.0; 3],
            }]);
            loom_asset::shape::stats(&mesh::mesh_volume(&volume, &SurfaceNets))
        };

        let plain = rock(None);
        let bumpy = rock(Some(Displace {
            // The calibrated recipe at R = 1: A = 0.20R, f = 0.6/R, and the
            // four octaves a 0.05 voxel can resolve. Coarse on purpose — this
            // is a unit test, and R/voxel = 20 against the 50 an authored rock
            // gets, which is why the threshold below is well under the 43% the
            // same recipe reaches at authoring resolution.
            amplitude: 0.2,
            frequency: 0.6,
            octaves: 4,
            seed: 0xB0D1E,
            ridged: true,
        }));

        assert_eq!(plain.boundary, 0, "the rock must fit inside its volume");
        assert!(plain.concave < 0.01, "a sphere is convex: {}", plain.concave);
        assert!(
            bumpy.concave > 0.15,
            "a ridged displacement must produce concave area: {} vs the sphere's {}",
            bumpy.concave,
            plain.concave
        );
        assert!(
            bumpy.spread > plain.spread * 2.0,
            "detail at several scales is a wider curvature spread: {} vs {}",
            bumpy.spread,
            plain.spread
        );
    }

    /// The same guarantee for the three shape transforms of §2.2.
    ///
    /// **Zero yaw, zero fillet and zero elongation must be the identity on the
    /// raw bits**, not merely close: every `.loom` in the library was authored
    /// without these fields, `#[serde(default)]` gives them zero, and a change
    /// of one ULP at a voxel that straddles the surface flips a sign in the
    /// baked `i8` and moves a golden image.
    #[test]
    fn the_shape_transforms_are_bit_identical_at_zero() {
        let center = [0.5, 1.5, 2.5];
        let half = [1.25, 0.75, 2.0];
        let boxed = VoxelOp::Box {
            center,
            half_extents: half,
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        };
        let sphere = VoxelOp::Sphere {
            center,
            radius: 1.75,
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        };
        for i in 0_i16..64 {
            let t = f32::from(i) * 0.31;
            let p = [t.cos() * 3.0, t * 0.05 - 1.0, t.sin() * 3.0];
            // The box arm exactly as it read before yaw and rounding existed.
            let d = sub(p, center);
            let q = [
                d[0].abs() - half[0],
                d[1].abs() - half[1],
                d[2].abs() - half[2],
            ];
            let was = length([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)])
                + q[0].max(q[1]).max(q[2]).min(0.0);
            assert_eq!(boxed.distance(p).to_bits(), was.to_bits(), "box at {p:?}");
            let was = length(sub(p, center)) - 1.75;
            assert_eq!(sphere.distance(p).to_bits(), was.to_bits(), "sphere at {p:?}");
        }
        // And the bounds, which is the other half of what a scene depends on.
        assert_eq!(
            boxed.bounds(),
            ([-0.75, 0.75, 0.5], [1.75, 2.25, 4.5]),
            "an unrotated box's AABB moved"
        );
        assert!((boxed.lipschitz() - 1.0).abs() < 1e-9);
    }

    /// **A yaw is an isometry, a fillet is a constant, an elongation is a
    /// clamp — so none of them changes the Lipschitz constant.** The task said
    /// to confirm that rather than assume it, because `bake`'s early-out fills
    /// a chunk in O(1) on the strength of it and a field that falls faster than
    /// one unit per unit travelled punches holes in the surface.
    ///
    /// Sampled over random point pairs rather than reasoned about: the ratio
    /// `|d(a) - d(b)| / |a - b|` is what the early-out actually needs bounded.
    #[test]
    fn the_shape_transforms_stay_one_lipschitz() {
        let mut next = unit_random(0x2545_F491_4F6C_DD1D);
        let mut worst = 0.0_f32;
        for case in 0..64 {
            let op = if case % 2 == 0 {
                VoxelOp::Box {
                    center: [next() * 2.0, next() * 2.0, next() * 2.0],
                    half_extents: [1.0 + next(), 1.5 + next(), 2.0 + next()],
                    mode: CsgMode::Union,
                    displace: None,
                    yaw_degrees: next() * 180.0,
                    round: 0.5 + next() * 0.5,
                }
            } else {
                VoxelOp::Sphere {
                    center: [next() * 2.0, next() * 2.0, next() * 2.0],
                    radius: 1.5 + next(),
                    mode: CsgMode::Union,
                    displace: None,
                    elongate: [next() * 2.0, next() * 2.0, next() * 2.0],
                }
            };
            for _ in 0..128 {
                let a = [next() * 5.0, next() * 5.0, next() * 5.0];
                let b = [next() * 5.0, next() * 5.0, next() * 5.0];
                let travelled = length(sub(a, b));
                if travelled < 1e-3 {
                    continue;
                }
                worst = worst.max((op.distance(a) - op.distance(b)).abs() / travelled);
            }
        }
        assert!(worst > 0.5, "the sampling never got near the bound: {worst}");
        assert!(
            worst <= 1.0 + 1e-4,
            "a shape transform is not 1-Lipschitz: {worst}"
        );
    }

    /// **`lipschitz()` must bound a *displaced* op too, and it did not.**
    ///
    /// The transform test above sets `displace: None`, so nothing checked the
    /// one op whose field is not a true distance — which is the only op whose
    /// bound has to be derived rather than known. It was derived for fBm and
    /// applied to ridged noise, which is four times steeper at the same
    /// parameters (see [`Displace::gradient_bound`]); the early-out then filled
    /// chunks that held surface, at any `voxel_size` fine enough that a chunk
    /// was small against the noise's own wavelength.
    ///
    /// Pairs are close together on purpose. The ratio over a long segment is an
    /// average and cannot reach the bound, so a test that sampled the whole
    /// volume would pass on a bound half the true one — which is how this got
    /// through. **The mutation is `Displace::gradient_bound`'s `shape` factor
    /// back to 1.0**: run, not assumed, and it fails at 2.18 against 1.48.
    #[test]
    fn a_displaced_op_never_falls_faster_than_its_own_bound() {
        let mut next = unit_random(0xD1B5_4A32_D192_ED03);
        for &ridged in &[true, false] {
            for &octaves in &[1_u32, 2, 3, 5] {
                let mut worst = 0.0_f32;
                let op = VoxelOp::Sphere {
                    center: [0.0; 3],
                    radius: 1.5,
                    mode: CsgMode::Union,
                    displace: Some(Displace {
                        amplitude: 0.30,
                        frequency: 0.4,
                        octaves,
                        seed: 9,
                        ridged,
                    }),
                    elongate: [0.0; 3],
                };
                let bound = op.lipschitz();
                for _ in 0..20_000 {
                    let a = [next() * 3.0, next() * 3.0, next() * 3.0];
                    let step = 2e-3;
                    let b = [
                        next().mul_add(step, a[0]),
                        next().mul_add(step, a[1]),
                        next().mul_add(step, a[2]),
                    ];
                    let travelled = length(sub(a, b));
                    if travelled < 1e-5 {
                        continue;
                    }
                    worst = worst.max((op.distance(a) - op.distance(b)).abs() / travelled);
                }
                assert!(
                    worst > 0.5,
                    "ridged={ridged} octaves={octaves}: the sampling never got near \
                     anything, so a pass here would mean nothing: {worst}"
                );
                assert!(
                    worst <= bound,
                    "ridged={ridged} octaves={octaves}: the field falls at {worst} per unit \
                     against a claimed bound of {bound} — the bake's early-out will skip \
                     chunks that hold surface"
                );
            }
        }
    }

    /// **`bounds()` is the one thing here that breaks quietly.** `Volume::edit`
    /// culls chunk spans by it, so an AABB that does not cover the rotated or
    /// elongated shape makes a runtime crater miss chunks entirely and leaves a
    /// seam or a floating slab at its edge — geometry that looks like a
    /// modelling mistake rather than a bookkeeping one.
    ///
    /// Randomised over yaw and extents on purpose: hand-listing eight corners
    /// at 0 and 45 degrees — the two angles anyone writes by hand — passes
    /// trivially, since both are symmetric.
    #[test]
    fn rotated_and_elongated_bounds_contain_the_solid() {
        let mut next = unit_random(0x9E37_79B9_7F4A_7C15);
        for case in 0..48 {
            let center = [next() * 3.0, next() * 3.0, next() * 3.0];
            let op = if case % 2 == 0 {
                VoxelOp::Box {
                    center,
                    half_extents: [
                        0.5 + next().abs() * 2.0,
                        0.5 + next().abs() * 2.0,
                        0.5 + next().abs() * 2.0,
                    ],
                    mode: CsgMode::Union,
                    displace: None,
                    yaw_degrees: next() * 360.0,
                    round: next().abs() * 0.4,
                }
            } else {
                VoxelOp::Sphere {
                    center,
                    radius: 0.5 + next().abs() * 1.5,
                    mode: CsgMode::Union,
                    displace: None,
                    elongate: [next() * 2.0, next() * 2.0, next() * 2.0],
                }
            };
            let (lo, hi) = op.bounds();
            // Every point of the solid must lie inside the AABB. Marched over a
            // region comfortably larger than the bounds, so a *missing* corner
            // is what fails rather than a lucky sample.
            let mut escaped = Vec::new();
            for iz in 0..26 {
                for iy in 0..26 {
                    for ix in 0..26 {
                        let f = |i: usize, axis: usize| {
                            let (l, h) = (lo[axis] - 1.5, hi[axis] + 1.5);
                            #[allow(clippy::cast_precision_loss)]
                            (l + (h - l) * (i as f32) / 25.0)
                        };
                        let p = [f(ix, 0), f(iy, 1), f(iz, 2)];
                        if op.distance(p) <= 0.0
                            && (0..3).any(|a| p[a] < lo[a] - 1e-4 || p[a] > hi[a] + 1e-4)
                        {
                            escaped.push(p);
                        }
                    }
                }
            }
            assert!(
                escaped.is_empty(),
                "case {case}: {} solid samples outside bounds {lo:?}..{hi:?}, e.g. {:?}",
                escaped.len(),
                &escaped[..escaped.len().min(3)]
            );
        }
    }

    /// A yaw turns the box the same way a node's `Quat::from_rotation_y` turns
    /// a mesh, and 90 degrees is the case that says so unambiguously: a plank
    /// long in X becomes a plank long in Z.
    #[test]
    fn a_quarter_turn_swaps_the_axes() {
        let plank = |yaw: f32| VoxelOp::Box {
            center: [0.0; 3],
            half_extents: [4.0, 0.5, 1.0],
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: yaw,
            round: 0.0,
        };
        let (lo, hi) = plank(90.0).bounds();
        assert!((hi[0] - 1.0).abs() < 1e-5 && (hi[2] - 4.0).abs() < 1e-5, "{lo:?}..{hi:?}");
        // A point 3 m out along +Z is inside the turned plank and outside the
        // straight one, which is the fact the AABB above only implies.
        assert!(plank(90.0).distance([0.0, 0.0, 3.0]) < 0.0);
        assert!(plank(0.0).distance([0.0, 0.0, 3.0]) > 0.0);
        // The sign convention: +yaw carries +X toward -Z, as `from_rotation_y`
        // does. At 90 degrees the plank's +X end is at -Z.
        assert!(plank(90.0).distance([0.0, 0.0, -3.9]) < 0.0);
    }

    /// Elongation is **exactly** distance-preserving, and the proof available
    /// here is that a sphere stretched along a segment is the capsule op — two
    /// independent formulas that must agree to the float.
    #[test]
    fn an_elongated_sphere_is_a_capsule() {
        let center = [1.0, 2.0, -0.5];
        let h = 2.25_f32;
        let stretched = VoxelOp::Sphere {
            center,
            radius: 0.8,
            mode: CsgMode::Union,
            displace: None,
            elongate: [h, 0.0, 0.0],
        };
        let capsule = VoxelOp::Capsule {
            a: [center[0] - h, center[1], center[2]],
            b: [center[0] + h, center[1], center[2]],
            radius: 0.8,
            mode: CsgMode::Union,
            displace: None,
        };
        let mut worst = 0.0_f32;
        for i in 0_i16..96 {
            let t = f32::from(i) * 0.41;
            let p = [t.cos() * 4.0, t * 0.06, t.sin() * 3.0];
            worst = worst.max((stretched.distance(p) - capsule.distance(p)).abs());
        }
        assert!(worst < 1e-5, "elongation disagrees with the capsule by {worst}");
    }

    /// Rounding takes the fillet **out of** the box rather than adding it on,
    /// so `half_extents` keeps meaning the outer extent and `bounds()` needs no
    /// term for it. A face centre stays exactly on the surface; the corner
    /// moves inward, which is the fillet.
    #[test]
    fn rounding_keeps_the_authored_extents() {
        let half = [2.0_f32, 1.5, 1.0];
        let sharp = VoxelOp::Box {
            center: [0.0; 3],
            half_extents: half,
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        };
        let filleted = VoxelOp::Box {
            center: [0.0; 3],
            half_extents: half,
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.4,
        };
        assert_eq!(sharp.bounds(), filleted.bounds());
        for axis in 0..3 {
            let mut p = [0.0; 3];
            p[axis] = half[axis];
            assert!(
                filleted.distance(p).abs() < 1e-6,
                "the face centre moved on axis {axis}"
            );
        }
        // The corner of the authored box now stands `r*(sqrt(3)-1)` clear of
        // the filleted surface — 0.293 at r = 0.4 — which is the fillet.
        let corner = half;
        assert!(
            (filleted.distance(corner) - 0.4 * (3.0_f32.sqrt() - 1.0)).abs() < 1e-5,
            "the corner was not cut back: {}",
            filleted.distance(corner)
        );
        assert!(sharp.distance(corner).abs() < 1e-6);
        // Over-large radii clamp to the thinnest half-extent instead of growing
        // the box past its own bounds.
        let over = VoxelOp::Box {
            center: [0.0; 3],
            half_extents: half,
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 99.0,
        };
        assert!(over.distance([half[0], 0.0, 0.0]).abs() < 1e-6);
    }

    /// Only the heightfield hoists a per-column height, and the transforms must
    /// not quietly acquire one — `bake` uses `Some` here to skip two thirds of
    /// the work, and a rotated box has no such thing.
    #[test]
    fn only_the_heightfield_has_a_height() {
        let ops = [
            VoxelOp::Box {
                center: [0.0; 3],
                half_extents: [1.0; 3],
                mode: CsgMode::Union,
                displace: None,
                yaw_degrees: 33.0,
                round: 0.2,
            },
            VoxelOp::Sphere {
                center: [0.0; 3],
                radius: 1.0,
                mode: CsgMode::Union,
                displace: None,
                elongate: [1.0, 0.0, 2.0],
            },
        ];
        for op in ops {
            assert!(op.height_at(0.25, 0.5).is_none());
        }
    }

    #[test]
    fn a_baked_heightfield_agrees_with_the_op_it_came_from() {
        let op = VoxelOp::Heightfield {
            base: 16.0,
            amplitude: 10.0,
            frequency: 0.125,
            octaves: 1,
            seed: 0x7E44A1,
            mode: CsgMode::Union,
        };
        let mut volume = Volume::new([4, 8, 4], 0.25);
        volume.bake(std::slice::from_ref(&op));

        let [rx, ry, rz] = volume.resolution();
        let mut disagreements = Vec::new();
        // Strided: the point is coverage of every chunk, not of every voxel,
        // and 4 million distance evaluations do not belong in a unit test.
        for z in (0..rz).step_by(3) {
            for y in (0..ry).step_by(3) {
                for x in (0..rx).step_by(3) {
                    let world = volume.world_of(x, y, z);
                    let truth = op.distance(world);
                    // Skip the surface itself: the field is quantised to an i8,
                    // so a voxel within a cell of the boundary can legitimately
                    // land on either side of zero.
                    if truth.abs() < volume.voxel_size * 2.0 {
                        continue;
                    }
                    if (volume.voxel(x, y, z) < 0) != (truth < 0.0) {
                        disagreements.push((x, y, z, truth));
                    }
                }
            }
        }

        assert!(
            disagreements.is_empty(),
            "{} sampled voxels disagree with the op, e.g. {:?}",
            disagreements.len(),
            &disagreements[..disagreements.len().min(3)]
        );
    }

    /// The horizontal bound is meaningless; the vertical one is what makes an
    /// edit cheap, and it has to actually contain the surface.
    #[test]
    fn a_heightfield_bounds_its_own_vertical_range() {
        let op = VoxelOp::Heightfield {
            base: 10.0,
            amplitude: 4.0,
            frequency: 0.2,
            octaves: 3,
            seed: 99,
            mode: CsgMode::Union,
        };

        let (min, max) = op.bounds();

        assert!(min[1] <= 6.0 && max[1] >= 14.0, "got {min:?}..{max:?}");
        assert!(op.lipschitz() > 1.0, "a sloped field is never 1-Lipschitz");
    }

    /// A recipe with a carve steep enough that the bound matters.
    ///
    /// Written as the text an author would write, so the test exercises the
    /// same parse the scene does rather than a struct literal that cannot
    /// disagree with one.
    const CARVED: &str = r#"
        size = [64, 64]
        world_scale = [32.0, 32.0]
        height_range = [0.0, 12.0]
        seed = 23070

        [[layer]]
        kind = "fbm"
        amplitude = 1.0
        frequency = 0.05
        octaves = 4

        [[layer]]
        kind = "spline_carve"
        points = [[10.0, 4.0], [30.0, 50.0]]
        width = 6.0
        depth = 9.0
    "#;

    fn carved_recipe() -> loom_terrain::Recipe {
        loom_terrain::Recipe::from_toml(CARVED).expect("the test recipe parses")
    }

    fn carved_terrain() -> VoxelOp {
        VoxelOp::Terrain {
            recipe: "carved.toml".to_owned(),
            rect: [0.0, 0.0, 32.0, 32.0],
            base_y: 2.0,
            mode: CsgMode::Union,
            map: Some(std::sync::Arc::new(TerrainMap::new(
                carved_recipe().bake(),
            ))),
        }
    }

    /// **The early-out has to be sound over a baked map**, and this is the test
    /// that says so: a chunk skipped because its centre looked far from the
    /// surface, when the surface runs through it, is a hole in the terrain.
    ///
    /// It fires. Pinning `Terrain`'s `lipschitz` back to 1.0 — the bound that
    /// is right for a true distance field and wrong for `y - h(x, z)` — fails
    /// it with 283 of the sampled voxels on the wrong side of the surface.
    #[test]
    fn a_baked_terrain_agrees_with_the_op_it_came_from() {
        let op = carved_terrain();
        let mut volume = Volume::new([4, 8, 4], 0.25);
        volume.bake(std::slice::from_ref(&op));

        let [rx, ry, rz] = volume.resolution();
        let mut disagreements = Vec::new();
        for z in (0..rz).step_by(3) {
            for y in (0..ry).step_by(3) {
                for x in (0..rx).step_by(3) {
                    let world = volume.world_of(x, y, z);
                    let truth = op.distance(world);
                    // As the heightfield's own test: the i8 quantisation makes
                    // a voxel within a cell of the boundary legitimately either
                    // sign.
                    if truth.abs() < volume.voxel_size * 2.0 {
                        continue;
                    }
                    if (volume.voxel(x, y, z) < 0) != (truth < 0.0) {
                        disagreements.push((x, y, z, truth));
                    }
                }
            }
        }
        assert!(
            disagreements.is_empty(),
            "{} sampled voxels disagree with the baked recipe, e.g. {:?}",
            disagreements.len(),
            &disagreements[..disagreements.len().min(4)]
        );
        assert!(
            op.lipschitz() > 1.5,
            "a carved landscape is much steeper than 1-Lipschitz: {}",
            op.lipschitz()
        );
    }

    /// The op is bounded on every axis, which is what a heightfield is not, and
    /// the vertical bound has to contain the whole map or an edit at the
    /// crater's edge misses chunks.
    #[test]
    fn a_terrain_bounds_the_rect_it_was_placed_over() {
        let op = carved_terrain();
        let (min, max) = op.bounds();
        assert_eq!([min[0], min[2]], [0.0, 0.0]);
        assert_eq!([max[0], max[2]], [32.0, 32.0]);

        let map = carved_recipe().bake();
        let (lo, hi) = map.range();
        assert!(min[1] <= 2.0 + lo && max[1] >= 2.0 + hi, "got {min:?}..{max:?}");
    }

    /// **An op with no map answers "surface" at every point**, which bakes a
    /// full volume of plausible nonsense and validates clean. It is refused
    /// instead, loudly, because every quieter treatment of it is the dropped-op
    /// defect again.
    #[test]
    #[should_panic(expected = "reached bake unresolved")]
    fn an_unresolved_terrain_op_stops_the_bake() {
        let mut volume = Volume::new([1, 1, 1], 1.0);
        volume.bake(&[VoxelOp::Terrain {
            recipe: "carved.toml".to_owned(),
            rect: [0.0, 0.0, 32.0, 32.0],
            base_y: 0.0,
            mode: CsgMode::Union,
            map: None,
        }]);
    }

    /// The rect places the recipe; it does not rescale it. Disagreeing about
    /// that renders a plausible landscape at the wrong scale, which is the one
    /// failure here that looks like an art decision rather than a bug.
    #[test]
    fn a_rect_that_is_not_the_recipes_own_size_is_refused() {
        let dir = std::env::temp_dir().join("loom_terrain_op_test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("carved.toml"), CARVED).expect("write recipe");

        let mut wrong = [VoxelOp::Terrain {
            recipe: "carved.toml".to_owned(),
            rect: [0.0, 0.0, 64.0, 64.0],
            base_y: 0.0,
            mode: CsgMode::Union,
            map: None,
        }];
        let error = resolve(&mut wrong, &dir).expect_err("a 64 m rect over a 32 m recipe");
        assert!(error.contains("world_scale"), "{error}");

        let mut right = [VoxelOp::Terrain {
            recipe: "carved.toml".to_owned(),
            rect: [10.0, -4.0, 42.0, 28.0],
            base_y: 0.0,
            mode: CsgMode::Union,
            map: None,
        }];
        resolve(&mut right, &dir).expect("a rect the size of the recipe, anywhere");
        assert!(right[0].is_resolved());
        // Placed, not rescaled: the map's own corner is at the rect's corner.
        assert!(
            (right[0].height_at(10.0, -4.0).expect("a terrain has a height")
                - carved_recipe().bake().get(0, 0))
            .abs()
                < 1e-4
        );
    }
}
