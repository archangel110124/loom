//! Surface extraction.
//!
//! # The chunk-boundary bug, handled up front
//!
//! Brief §7.9 calls this **the guaranteed bug**: surface extraction reads one
//! voxel *past* the chunk boundary. Forget it and you get cracks at every seam,
//! on every edit — and it is invisible in a single-chunk test.
//!
//! So the chunk is sampled with a one-voxel skirt on the far side, reading
//! across into the neighbour. Two adjacent chunks then agree about the field in
//! the overlap and produce vertices that meet. The test for it was written
//! before this module existed, which is the discipline §7.9 asks for.

use fast_surface_nets::ndshape::{ConstShape, ConstShape3u32};
use fast_surface_nets::{SurfaceNetsBuffer, surface_nets};

use crate::{CHUNK, Volume, dequantize};

/// Sampled region per chunk: the chunk plus a one-voxel skirt on every side.
///
/// The skirt is what makes seams meet. `fast-surface-nets` additionally
/// requires a one-voxel margin it does not emit geometry for, so the padding
/// is 2 and the emitted range is inset accordingly.
const PAD: u32 = 2;
const SAMPLED: u32 = CHUNK as u32 + PAD;
type ChunkShape = ConstShape3u32<SAMPLED, SAMPLED, SAMPLED>;

/// Extracted geometry for one chunk, in world space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl ChunkMesh {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// A surface extraction algorithm.
///
/// **Pre-authorised** by `CLAUDE.md` never-do #12, which forbids a trait with
/// one implementation and names this as an exception: Surface Nets and Dual
/// Contouring are both planned. Surface Nets gives smooth terrain and cannot
/// represent a sharp 90° corner; DC can, and brings free LOD seam handling with
/// it. The `needs_hermite` flag is what lets the edit path stay unchanged when
/// DC arrives — write gradients from the start even though Surface Nets
/// ignores them.
pub trait Mesher: Send + Sync {
    fn mesh_chunk(&self, volume: &Volume, chunk: [usize; 3]) -> ChunkMesh;

    /// Whether this mesher needs gradients at edge crossings, not just
    /// distances. Surface Nets: no. Dual Contouring: yes.
    fn needs_hermite(&self) -> bool {
        false
    }
}

/// Naive Surface Nets, via `fast-surface-nets`.
///
/// The voxel doc's Phase A choice: smooth, cheap, chunk-friendly, and proven
/// in Rust. It cannot produce a sharp corner, so buildings look soft — that is
/// a known ceiling, and Dual Contouring behind this same trait is the answer.
#[derive(Debug, Clone, Copy, Default)]
pub struct SurfaceNets;

impl Mesher for SurfaceNets {
    fn mesh_chunk(&self, volume: &Volume, chunk: [usize; 3]) -> ChunkMesh {
        let [cx, cy, cz] = chunk;
        let base = [cx * CHUNK, cy * CHUNK, cz * CHUNK];

        // Sample with a skirt, reading ACROSS the boundary into neighbours.
        // This is the whole fix for §7.9 — everything else here is bookkeeping.
        let mut field = vec![0.0_f32; ChunkShape::SIZE as usize];
        for i in 0..ChunkShape::SIZE {
            let [px, py, pz] = ChunkShape::delinearize(i);
            // Offset by one so index 1 is the chunk's own voxel 0, and index 0
            // is the neighbour's last voxel.
            let sample = |b: usize, p: u32| (b + p as usize).wrapping_sub(1);
            field[i as usize] = dequantize(volume.voxel(
                sample(base[0], px),
                sample(base[1], py),
                sample(base[2], pz),
            ));
        }

        let mut buffer = SurfaceNetsBuffer::default();
        surface_nets(
            &field,
            &ChunkShape {},
            [0; 3],
            [SAMPLED - 1; 3],
            &mut buffer,
        );

        // Positions come back in sampled-array space; shift by the skirt and
        // scale into world units.
        //
        // **The `0.5` is the half-cell offset, and it was missing.** Sampled
        // index `p` is voxel `p − 1`, and [`Volume::world_of`] places voxel `k`
        // at `(k + 0.5) · voxel_size` — the same convention `solid_cells` hands
        // parry and `exposure::sample` inverts, both of which say so in their
        // own comments. This mapping said `(p − 1) · size`, so **every voxel
        // mesh in the project was drawn half a voxel low on all three axes**
        // while its collider and its SDF sat where the author put them.
        //
        // Measured against an authored box top at y = 4.0: the height field
        // says 4.0002, the collider says 4.0000, the mesh said 3.8750 at a
        // 0.25 m voxel and 3.7500 at 0.5 m — exactly half a voxel, both times.
        //
        // It was invisible for as long as nothing drew two conventions in one
        // pixel. W6 does: the water's shoreline is cut where the SDF says the
        // ground breaks the surface, and against a mesh half a voxel low that
        // left a rim of beach the sea did not reach — 0.15 m of height on a
        // 1:4 slope is 0.57 m of dry sand.
        let size = volume.voxel_size;
        #[allow(clippy::cast_precision_loss)]
        let origin = [
            base[0] as f32 * size,
            base[1] as f32 * size,
            base[2] as f32 * size,
        ];
        let positions = buffer
            .positions
            .iter()
            .map(|p| {
                [
                    origin[0] + (p[0] - 0.5) * size,
                    origin[1] + (p[1] - 0.5) * size,
                    origin[2] + (p[2] - 0.5) * size,
                ]
            })
            .collect();

        ChunkMesh {
            positions,
            // Returned unnormalised by design — normalising is cheapest on the
            // GPU, and the shader already does it.
            normals: buffer.normals,
            indices: buffer.indices,
        }
    }
}

/// Mesh every chunk that holds a surface, and merge into one mesh.
///
/// Uniform chunks are skipped in O(1), which is most of them — the reason
/// meshing a large mostly-empty volume is cheap.
#[must_use]
pub fn mesh_volume(volume: &Volume, mesher: &dyn Mesher) -> loom_asset::Mesh {
    let mut out = loom_asset::Mesh {
        name: "voxel".to_owned(),
        ..loom_asset::Mesh::default()
    };

    for chunk in volume.surface_chunks() {
        let part = mesher.mesh_chunk(volume, chunk);
        if part.is_empty() {
            continue;
        }
        let base = u32::try_from(out.vertices.len()).unwrap_or(0);
        for (i, p) in part.positions.iter().enumerate() {
            let n = part.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            out.vertices.push(loom_asset::Vertex::new(*p, normalize(n)));
        }
        out.indices.extend(part.indices.iter().map(|i| i + base));
    }
    out
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CsgMode, VoxelOp};

    fn sphere(center: [f32; 3], radius: f32) -> VoxelOp {
        VoxelOp::Sphere {
            center,
            radius,
            mode: CsgMode::Union,
        }
    }

    /// **THE test from brief §7.9, and it was written before the mesher.**
    ///
    /// Two adjacent chunks, a sphere spanning the boundary. Every vertex the
    /// two chunks produce near the shared plane must have a partner in the
    /// other chunk at the same position — otherwise there is a crack, and a
    /// crack is invisible in any single-chunk test.
    #[test]
    fn adjacent_chunks_meet_at_the_seam() {
        let mut volume = Volume::new([2, 1, 1], 0.5);
        // Centre the sphere ON the boundary between chunk 0 and chunk 1.
        let boundary = CHUNK as f32 * 0.5;
        volume.bake(&[sphere([boundary, 8.0, 8.0], 5.0)]);

        let mesher = SurfaceNets;
        let left = mesher.mesh_chunk(&volume, [0, 0, 0]);
        let right = mesher.mesh_chunk(&volume, [1, 0, 0]);

        assert!(!left.is_empty() && !right.is_empty(), "both must have geometry");

        // Vertices within half a voxel of the shared plane, from each side.
        let near_seam = |m: &ChunkMesh| -> Vec<[f32; 3]> {
            m.positions
                .iter()
                .filter(|p| (p[0] - boundary).abs() < 0.26)
                .copied()
                .collect()
        };
        let a = near_seam(&left);
        let b = near_seam(&right);
        assert!(!a.is_empty(), "left chunk must reach the seam");
        assert!(!b.is_empty(), "right chunk must reach the seam");

        // Every seam vertex on the left must have a partner on the right.
        for p in &a {
            let matched = b.iter().any(|q| {
                (p[0] - q[0]).abs() < 1e-3
                    && (p[1] - q[1]).abs() < 1e-3
                    && (p[2] - q[2]).abs() < 1e-3
            });
            assert!(
                matched,
                "seam vertex {p:?} has no partner across the boundary — this is a crack"
            );
        }
    }

    /// Without the skirt the mesher would see air past the boundary and close
    /// the surface off. A sphere spanning two chunks must not produce a wall
    /// at the seam, so the seam region has geometry on both sides.
    #[test]
    fn a_sphere_spanning_two_chunks_is_not_capped_at_the_boundary() {
        let mut volume = Volume::new([2, 1, 1], 0.5);
        let boundary = CHUNK as f32 * 0.5;
        volume.bake(&[sphere([boundary, 8.0, 8.0], 5.0)]);

        let mesh = mesh_volume(&volume, &SurfaceNets);
        let (min, max) = mesh.bounds();

        // The sphere spans boundary ± 5. If the seam capped it, the mesh would
        // stop at the boundary on one side.
        assert!(min[0] < boundary - 3.0, "left side truncated: min {min:?}");
        assert!(max[0] > boundary + 3.0, "right side truncated: max {max:?}");
    }

    #[test]
    fn an_empty_volume_meshes_to_nothing() {
        let volume = Volume::new([2, 2, 2], 0.5);
        let mesh = mesh_volume(&volume, &SurfaceNets);

        assert!(mesh.indices.is_empty(), "no surface, no triangles");
    }

    #[test]
    fn a_baked_sphere_produces_a_watertight_looking_mesh() {
        let mut volume = Volume::new([2, 2, 2], 0.5);
        volume.bake(&[sphere([16.0, 16.0, 16.0], 6.0)]);

        let mesh = mesh_volume(&volume, &SurfaceNets);
        mesh.validate().expect("indices must address real vertices");
        assert!(mesh.indices.len().is_multiple_of(3), "whole triangles only");
        assert!(mesh.vertices.len() > 100, "a sphere should be well-tessellated");
    }

    /// Surface Nets does not need gradients; Dual Contouring will. The flag is
    /// what keeps the edit path unchanged when DC lands.
    #[test]
    fn surface_nets_does_not_need_hermite_data() {
        assert!(!SurfaceNets.needs_hermite());
    }

    /// **The drawn surface is where the author put it, and where the collider
    /// and the SDF agree it is.**
    ///
    /// Three subsystems place a voxel in world space — this mesher,
    /// `Volume::solid_cells` for parry, and `exposure::sample` for every SDF
    /// query — and until this test they did not all use the same convention.
    /// The mesher was half a voxel low on all three axes, so the geometry you
    /// saw was not the geometry you shot at, grass stood half a voxel above the
    /// ground it was draped on, and W6's shoreline cut the sea short of a beach
    /// that was drawn too low to meet it.
    ///
    /// A box top is the right probe because the answer is exact: no curvature,
    /// no interpolation, one number the scene file states outright. Both voxel
    /// sizes, because a bug measured in voxels hides in a single one.
    #[test]
    fn the_mesh_the_collider_and_the_field_place_a_surface_in_the_same_place() {
        for voxel in [0.25_f32, 0.5] {
            let mut volume = Volume::new([2, 2, 2], voxel);
            volume.bake(&[crate::VoxelOp::Box {
                center: [8.0, 2.0, 8.0],
                half_extents: [6.0, 2.0, 6.0],
                mode: crate::CsgMode::Union,
            }]);

            let field = crate::heightfield::surface_height(&volume, [0.0; 3], 8.0, 8.0);
            let mesh = mesh_volume(&volume, &SurfaceNets);
            let top = mesh
                .vertices
                .iter()
                .map(|v| v.position)
                .filter(|p| (p[0] - 8.0).abs() < 1.0 && (p[2] - 8.0).abs() < 1.0)
                .fold(f32::MIN, |a, p| a.max(p[1]));

            // Half the voxel of tolerance: Surface Nets puts its vertex at the
            // centroid of a cell's crossings, so a flat top lands within half a
            // cell of the plane and never on it exactly. The bug this catches
            // was a *whole* half-voxel of systematic offset on top of that.
            assert!(
                (field - 4.0).abs() < voxel * 0.5,
                "the SDF march puts the authored y = 4.0 top at {field} ({voxel} m voxels)"
            );
            assert!(
                (top - 4.0).abs() < voxel * 0.5,
                "the drawn top is at {top}, not the authored 4.0 ({voxel} m voxels)"
            );
        }
    }
}
