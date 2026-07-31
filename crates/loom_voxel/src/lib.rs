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

pub mod mesh;

pub use mesh::{Mesher, SurfaceNets};

use std::collections::BTreeMap;

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

/// One authored edit.
///
/// **This is what a scene stores**, never the resulting voxels. "Carve a cave
/// through this hillside" is a handful of capsule subtractions — a request the
/// agent can express correctly on the first try (voxel doc §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoxelOp {
    Sphere {
        center: [f32; 3],
        radius: f32,
        mode: CsgMode,
    },
    Box {
        center: [f32; 3],
        half_extents: [f32; 3],
        mode: CsgMode,
    },
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
        mode: CsgMode,
    },
}

impl VoxelOp {
    #[must_use]
    pub fn mode(&self) -> CsgMode {
        match self {
            Self::Sphere { mode, .. } | Self::Box { mode, .. } | Self::Capsule { mode, .. } => *mode,
        }
    }

    /// World-space bounds this op can possibly affect.
    ///
    /// An edit costs the chunks it touches, not the chunks that exist. Without
    /// this, carving one crater walks every chunk in the volume — 4096 of them
    /// in a 512³ world, to change 34.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Sphere { center, radius, .. } => (
                [center[0] - radius, center[1] - radius, center[2] - radius],
                [center[0] + radius, center[1] + radius, center[2] + radius],
            ),
            Self::Box {
                center,
                half_extents,
                ..
            } => (
                [
                    center[0] - half_extents[0],
                    center[1] - half_extents[1],
                    center[2] - half_extents[2],
                ],
                [
                    center[0] + half_extents[0],
                    center[1] + half_extents[1],
                    center[2] + half_extents[2],
                ],
            ),
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
        }
    }

    /// Signed distance from `p` to this shape, in world units.
    #[must_use]
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        match self {
            Self::Sphere { center, radius, .. } => length(sub(p, *center)) - radius,
            Self::Box {
                center,
                half_extents,
                ..
            } => {
                let d = sub(p, *center);
                let q = [
                    d[0].abs() - half_extents[0],
                    d[1].abs() - half_extents[1],
                    d[2].abs() - half_extents[2],
                ];
                let outside = length([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
                let inside = q[0].max(q[1]).max(q[2]).min(0.0);
                outside + inside
            }
            Self::Capsule { a, b, radius, .. } => {
                let pa = sub(p, *a);
                let ba = sub(*b, *a);
                let h = (dot(pa, ba) / dot(ba, ba).max(1e-6)).clamp(0.0, 1.0);
                length(sub(pa, scale(ba, h))) - radius
            }
        }
    }
}

/// Combine one op's distance into an accumulated field value.
fn combine(accumulated: f32, op: &VoxelOp, p: [f32; 3]) -> f32 {
    let d = op.distance(p);
    match op.mode() {
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
    pub fn bake(&mut self, ops: &[VoxelOp]) {
        #[allow(clippy::cast_precision_loss)]
        // Half-diagonal of a chunk in world units — the furthest any voxel in
        // it can be from its centre.
        let chunk_radius = (CHUNK as f32 * 0.5 * self.voxel_size) * 1.7320508;

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
                    if d.abs() > chunk_radius {
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

        for z in 0..CHUNK {
            for y in 0..CHUNK {
                for x in 0..CHUNK {
                    #[allow(clippy::cast_precision_loss)]
                    let p = [
                        ((cx * CHUNK + x) as f32 + 0.5) * voxel_size,
                        ((cy * CHUNK + y) as f32 + 0.5) * voxel_size,
                        ((cz * CHUNK + z) as f32 + 0.5) * voxel_size,
                    ];
                    let mut d = f32::MAX;
                    for op in ops {
                        d = combine(d, op, p);
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
                    if op.distance(centre).abs() > chunk_radius {
                        continue;
                    }

                    let index = self.chunk_index(cx, cy, cz);
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

                    if changed {
                        self.edits.insert(index, voxels);
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
                    if self.chunks[index].has_surface() || self.edits.contains_key(&index) {
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
}
