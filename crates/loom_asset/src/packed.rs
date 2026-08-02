//! Compressed vertex format: 32 bytes down to 8.
//!
//! # What this is and is not
//!
//! Voxel renderers commonly pack a vertex into a single 32-bit integer by
//! exploiting that blocky voxel corners sit on integer lattice points and
//! normals are one of six axis directions. **Loom's voxels are smooth**, so
//! neither holds: Surface Nets places a vertex anywhere inside its cell, and
//! the normal is an arbitrary direction.
//!
//! So the same *idea* applies with different arithmetic — quantise against the
//! mesh's own bounds rather than a voxel lattice, and encode the normal
//! octahedrally rather than as a face index. That still gets 32 bytes to 8.
//!
//! # Precision, stated rather than hoped for
//!
//! Position is 10 bits per axis over the mesh bounds. For a 24-unit mesh that
//! is 24/1023 ≈ 0.023 units — well under a tenth of a voxel at the 0.25 voxel
//! size these scenes use. Octahedral normals at 16 bits per axis are accurate
//! to well under a degree. Both are round-trip tested rather than assumed.

use serde::{Deserialize, Serialize};

use crate::Vertex;

/// A vertex in 12 bytes: 10-10-10 position, octahedral normal, quantised UV.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedVertex {
    /// 10 bits per axis, normalised against the mesh bounds. 2 bits spare.
    pub position: u32,
    /// Octahedral normal, 16 bits per component.
    pub normal: u32,
    /// Texture coordinates, 16 bits per component, normalised against the
    /// mesh's own UV bounds.
    ///
    /// **Not a unit-range encoding.** UVs outside `0..1` are ordinary — that
    /// is how a texture is authored to tile — and clamping them into a unit
    /// range would collapse a wall tiled eight times into one stretched
    /// sample. Quantising against the range the mesh actually uses keeps all
    /// 16 bits useful whatever that range is, which is the same reasoning that
    /// governs `position`.
    pub uv: u32,
}

/// How to decode a [`PackedVertex`] back to world space.
///
/// One per mesh, not per vertex — that is what makes the compression free
/// rather than moving the cost around.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PackedBounds {
    /// World position of the quantisation origin.
    pub origin: [f32; 3],
    /// World units per quantisation step.
    pub step: f32,
    /// UV of the quantisation origin.
    pub uv_origin: [f32; 2],
    /// UV units per quantisation step.
    pub uv_step: f32,
}

const MAX: f32 = 1023.0;
/// UVs get the full 16 bits per component, unlike position's 10.
const UV_MAX: f32 = 65535.0;

/// Map a unit vector onto the octahedron and into the unit square.
///
/// Octahedral encoding beats storing two angles or three quantised components:
/// it is uniform over the sphere, so precision does not collapse at the poles.
fn octahedral_encode(n: [f32; 3]) -> (f32, f32) {
    let len = n[0].abs() + n[1].abs() + n[2].abs();
    if len < 1e-6 {
        return (0.0, 0.0);
    }
    let (x, y, z) = (n[0] / len, n[1] / len, n[2] / len);
    if z >= 0.0 {
        (x, y)
    } else {
        // Fold the lower hemisphere outward.
        (
            (1.0 - y.abs()) * if x >= 0.0 { 1.0 } else { -1.0 },
            (1.0 - x.abs()) * if y >= 0.0 { 1.0 } else { -1.0 },
        )
    }
}

fn octahedral_decode(u: f32, v: f32) -> [f32; 3] {
    let z = 1.0 - u.abs() - v.abs();
    let (x, y) = if z < 0.0 {
        (
            (1.0 - v.abs()) * if u >= 0.0 { 1.0 } else { -1.0 },
            (1.0 - u.abs()) * if v >= 0.0 { 1.0 } else { -1.0 },
        )
    } else {
        (u, v)
    };
    let len = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / len, y / len, z / len]
}

/// Pack a mesh's vertices, returning the decode parameters alongside.
#[must_use]
pub fn pack(vertices: &[Vertex]) -> (Vec<PackedVertex>, PackedBounds) {
    if vertices.is_empty() {
        return (
            Vec::new(),
            PackedBounds {
                origin: [0.0; 3],
                step: 1.0,
                uv_origin: [0.0; 2],
                uv_step: 1.0,
            },
        );
    }

    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    let mut uv_min = [f32::MAX; 2];
    let mut uv_max = [f32::MIN; 2];
    for v in vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(v.position[axis]);
            max[axis] = max[axis].max(v.position[axis]);
        }
        for axis in 0..2 {
            uv_min[axis] = uv_min[axis].min(v.uv[axis]);
            uv_max[axis] = uv_max[axis].max(v.uv[axis]);
        }
    }
    // One step for all three axes, so decoding is a single multiply-add rather
    // than three, and a cube stays a cube.
    let extent = (0..3).fold(0.0_f32, |acc, a| acc.max(max[a] - min[a]));
    let step = (extent / MAX).max(1e-6);
    // Likewise one step for both UV axes, so a square texture stays square.
    // A mesh with no UVs has zero extent here, and the `max` keeps the step
    // positive so decoding gives back the origin rather than a division by
    // zero.
    let uv_extent = (0..2).fold(0.0_f32, |acc, a| acc.max(uv_max[a] - uv_min[a]));
    let uv_step = (uv_extent / UV_MAX).max(1e-9);

    let packed = vertices
        .iter()
        .map(|v| {
            let q = |axis: usize| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (((v.position[axis] - min[axis]) / step).round().clamp(0.0, MAX)) as u32
                }
            };
            let (u, w) = octahedral_encode([v.normal[0], v.normal[1], v.normal[2]]);
            // -1..1 into 0..65535.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let enc = |value: f32| (((value * 0.5 + 0.5) * 65535.0).round().clamp(0.0, 65535.0)) as u32;

            let uv = |axis: usize| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (((v.uv[axis] - uv_min[axis]) / uv_step).round().clamp(0.0, UV_MAX)) as u32
                }
            };

            PackedVertex {
                position: q(0) | (q(1) << 10) | (q(2) << 20),
                normal: enc(u) | (enc(w) << 16),
                uv: uv(0) | (uv(1) << 16),
            }
        })
        .collect();

    (
        packed,
        PackedBounds {
            origin: min,
            step,
            uv_origin: uv_min,
            uv_step,
        },
    )
}

/// Decode one vertex. Mirrors what the shader does, and is what the round-trip
/// test checks — a decoder that only exists in GLSL cannot be tested.
#[must_use]
pub fn unpack(packed: PackedVertex, bounds: PackedBounds) -> Vertex {
    let axis = |shift: u32| f32::from_bits(0).mul_add(0.0, ((packed.position >> shift) & 0x3FF) as f32);
    let position = [
        bounds.origin[0] + axis(0) * bounds.step,
        bounds.origin[1] + axis(10) * bounds.step,
        bounds.origin[2] + axis(20) * bounds.step,
    ];

    let dec = |shift: u32| ((packed.normal >> shift) & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0;
    let normal = octahedral_decode(dec(0), dec(16));

    let uv = |shift: u32, axis: usize| {
        bounds.uv_origin[axis] + ((packed.uv >> shift) & 0xFFFF) as f32 * bounds.uv_step
    };

    Vertex::with_uv(position, normal, [uv(0, 0), uv(16, 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Vec<Vertex> {
        // A sphere's worth of directions, spread over a 24-unit extent — the
        // size of the cave mesh these numbers were measured against.
        (0..500)
            .map(|i| {
                let t = f32::from(i as i16) * 0.0125;
                let n = [t.sin(), (t * 1.7).cos(), (t * 0.6).sin()];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                Vertex::new(
                    [t * 2.0, (t * 3.0) % 24.0, (t * 0.5) % 24.0],
                    [n[0] / len, n[1] / len, n[2] / len],
                )
            })
            .collect()
    }

    #[test]
    fn a_packed_vertex_is_twelve_bytes() {
        // 40 -> 12. Position and normal are 4 bytes each; the UV added a
        // third word rather than a pair of floats.
        assert_eq!(size_of::<PackedVertex>(), 12);
        assert_eq!(size_of::<Vertex>(), 40);
    }

    /// UVs are quantised against the mesh's own UV bounds, the same way
    /// positions are quantised against its position bounds. That is what lets
    /// a mesh whose UVs tile 0..8 keep its precision instead of being clamped
    /// into 0..1 — 16 bits spread over the range actually used.
    #[test]
    fn uvs_round_trip_within_a_quantisation_step() {
        let original: Vec<Vertex> = (0..200)
            .map(|i| {
                let t = f32::from(i as i16) * 0.04;
                // Deliberately outside 0..1: tiled UVs are ordinary, and a
                // unit-range encoding would silently clamp them.
                Vertex::with_uv([t, 0.0, 0.0], [0.0, 1.0, 0.0], [t * 0.5, 8.0 - t * 0.3])
            })
            .collect();
        let (packed, bounds) = pack(&original);

        for (a, b) in original.iter().zip(&packed) {
            let back = unpack(*b, bounds);
            for axis in 0..2 {
                let error = (a.uv[axis] - back.uv[axis]).abs();
                assert!(
                    error <= bounds.uv_step,
                    "uv axis {axis}: {} vs {} (step {})",
                    a.uv[axis],
                    back.uv[axis],
                    bounds.uv_step
                );
            }
        }
    }

    /// Voxel meshes have no UVs at all — Surface Nets produces none — so every
    /// vertex carries `[0, 0]`. A zero-extent range must not divide by zero or
    /// decode to garbage.
    #[test]
    fn a_mesh_with_no_uvs_decodes_to_zero() {
        let mesh = vec![
            Vertex::new([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Vertex::new([1.0, 2.0, 3.0], [1.0, 0.0, 0.0]),
        ];

        let (packed, bounds) = pack(&mesh);

        for p in &packed {
            assert_eq!(unpack(*p, bounds).uv, [0.0, 0.0]);
        }
    }

    /// Precision stated rather than hoped for: 10 bits per axis over the mesh
    /// extent. For a 24-unit mesh that is ~0.023 units, well under a tenth of
    /// a voxel at the 0.25 voxel size these scenes use.
    #[test]
    fn positions_round_trip_within_a_quantisation_step() {
        let original = mesh();
        let (packed, bounds) = pack(&original);

        for (a, b) in original.iter().zip(&packed) {
            let back = unpack(*b, bounds);
            for axis in 0..3 {
                let error = (a.position[axis] - back.position[axis]).abs();
                assert!(
                    error <= bounds.step,
                    "axis {axis}: {} vs {} (step {})",
                    a.position[axis],
                    back.position[axis],
                    bounds.step
                );
            }
        }
    }

    /// Octahedral encoding is uniform over the sphere, so precision does not
    /// collapse at the poles the way two-angle encodings do.
    #[test]
    fn normals_round_trip_to_under_a_degree() {
        let original = mesh();
        let (packed, bounds) = pack(&original);

        let mut worst = 0.0_f32;
        for (a, b) in original.iter().zip(&packed) {
            let back = unpack(*b, bounds);
            let dot = (0..3).map(|i| a.normal[i] * back.normal[i]).sum::<f32>();
            worst = worst.max(dot.clamp(-1.0, 1.0).acos().to_degrees());
        }
        assert!(worst < 1.0, "worst normal error was {worst}°");
    }

    /// Straight up and straight down are where naive encodings lose precision.
    #[test]
    fn axis_aligned_normals_survive_exactly_enough() {
        for n in [
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
        ] {
            let v = vec![Vertex::new([0.0; 3], n)];
            let (packed, bounds) = pack(&v);
            let back = unpack(packed[0], bounds);
            let dot: f32 = (0..3).map(|i| n[i] * back.normal[i]).sum();
            assert!(dot > 0.9999, "{n:?} decoded to {:?}", back.normal);
        }
    }

    #[test]
    fn an_empty_mesh_packs_to_nothing() {
        let (packed, _) = pack(&[]);
        assert!(packed.is_empty());
    }
}
