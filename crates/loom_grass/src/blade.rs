//! Turning a placed blade into triangles.
//!
//! **A cubic Bézier expanded into a tapering strip**, which is the Ghost of
//! Tsushima shape every per-blade system since has copied: control points from
//! height, tilt and bend; roughly 15 vertices near and 7 far; width narrowing
//! to nothing at the tip.
//!
//! # Why this is on the CPU first
//!
//! The shipping architecture generates this in a vertex shader from
//! `SV_VertexID` with no vertex streams at all, and that is where it is going.
//! It is here first because the phase's stated risk is **anti-aliasing without
//! temporal accumulation**, shimmer is a property of sub-pixel geometry under a
//! moving camera, and a generated mesh through the existing renderer answers
//! that question with no new pipeline, no compute pass and no indirect draw.
//!
//! Getting the visual answer before building the machinery is the cheaper
//! order. The machinery does not change the answer; the answer may well change
//! the machinery.
//!
//! # The lighting is deliberately wrong
//!
//! A flat blade lit by its true geometric normal looks harsh and papery — this
//! is a case where the physically correct answer is the wrong-looking one. The
//! normals here are tilted outward and blended toward the ground's, which is
//! the standard trick and the reason grass reads as soft.

use crate::Blade;

/// Vertices along one side of a blade at full detail.
///
/// Seven segments is fifteen vertices with the tip, matching the published
/// Tsushima numbers. The far LOD halves it.
pub const SEGMENTS_NEAR: usize = 7;

/// Segments at the low LOD.
pub const SEGMENTS_FAR: usize = 3;

/// A point on the blade's spine, and the direction it is heading.
#[derive(Debug, Clone, Copy)]
struct Spine {
    point: [f32; 3],
    tangent: [f32; 3],
}

/// The blade's three Bézier control points, relative to its base.
///
/// The curve starts vertical and arcs over in the facing direction. `tilt`
/// sets how far it leans at rest and `bend` how much of that lean is
/// curvature rather than a straight lean — a blade with no bend is a needle,
/// which is what a field of unbent blades reads as.
fn controls(blade: &Blade) -> [[f32; 3]; 3] {
    let lean = blade.tilt.sin() * blade.height;
    let rise = blade.tilt.cos() * blade.height;
    let (fx, fz) = (blade.facing[0], blade.facing[1]);

    // Mid control point: up the blade, pulled toward the facing by `bend`.
    let mid = [
        fx * lean * 0.33 * blade.bend,
        rise * 0.55,
        fz * lean * 0.33 * blade.bend,
    ];
    // The tip.
    let tip = [fx * lean, rise, fz * lean];
    // A shoulder between them, so the curve leaves the ground upright and
    // arcs over rather than bending from the root — grass grows from a sheath.
    let shoulder = [
        (mid[0] + tip[0]) * 0.5,
        (mid[1] + tip[1]) * 0.5 + rise * 0.06,
        (mid[2] + tip[2]) * 0.5,
    ];
    [mid, shoulder, tip]
}

/// Evaluate the spine at `t` in `[0, 1]`.
fn spine(blade: &Blade, t: f32) -> Spine {
    let [p1, p2, p3] = controls(blade);
    let p0 = [0.0, 0.0, 0.0];

    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    let point = [
        b0 * p0[0] + b1 * p1[0] + b2 * p2[0] + b3 * p3[0],
        b0 * p0[1] + b1 * p1[1] + b2 * p2[1] + b3 * p3[1],
        b0 * p0[2] + b1 * p1[2] + b2 * p2[2] + b3 * p3[2],
    ];

    // Analytic derivative rather than a finite difference: the difference
    // degenerates at the tip, where the two samples coincide.
    let (d0, d1, d2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
    let mut tangent = [
        d0 * (p1[0] - p0[0]) + d1 * (p2[0] - p1[0]) + d2 * (p3[0] - p2[0]),
        d0 * (p1[1] - p0[1]) + d1 * (p2[1] - p1[1]) + d2 * (p3[1] - p2[1]),
        d0 * (p1[2] - p0[2]) + d1 * (p2[2] - p1[2]) + d2 * (p3[2] - p2[2]),
    ];
    let length = tangent[0]
        .mul_add(tangent[0], tangent[1].mul_add(tangent[1], tangent[2] * tangent[2]))
        .sqrt();
    if length > 1e-6 {
        for axis in &mut tangent {
            *axis /= length;
        }
    } else {
        tangent = [0.0, 1.0, 0.0];
    }
    Spine { point, tangent }
}

/// Half-width at `t`, tapering to a point.
///
/// Squared falloff rather than linear: a linearly tapering blade is a triangle
/// and reads as one. Grass is nearly parallel-sided for most of its length and
/// then narrows quickly.
fn half_width(blade: &Blade, t: f32) -> f32 {
    blade.width * 0.5 * (1.0 - t * t)
}

/// Append one blade's triangles to a mesh.
///
/// `ground_normal` is what the blade's shading normal is blended toward. It is
/// the caller's because the ground knows its own slope and the blade does not.
pub fn emit(
    blade: &Blade,
    ground_normal: [f32; 3],
    segments: usize,
    vertices: &mut Vec<loom_asset::Vertex>,
    indices: &mut Vec<u32>,
) {
    let segments = segments.max(1);
    let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let (fx, fz) = (blade.facing[0], blade.facing[1]);
    // Across the blade, perpendicular to its facing in the ground plane.
    let side = [-fz, 0.0, fx];

    for step in 0..=segments {
        #[allow(clippy::cast_precision_loss)]
        let t = step as f32 / segments as f32;
        let Spine { point, tangent } = spine(blade, t);
        let width = half_width(blade, t);

        // **Not the geometric normal.** The true normal of a flat blade makes
        // it read as paper: harsh, and black whenever it turns edge-on. The
        // shading normal is tilted outward from the spine and blended toward
        // the ground's, which is what makes a field read as a soft surface
        // rather than as a million little planes. The blend strengthens toward
        // the base, where a blade is most surrounded by its neighbours.
        let face = cross(side, tangent);
        let toward_ground = 1.0 - t * 0.65;
        let normal = normalise([
            face[0] + (ground_normal[0] - face[0]) * toward_ground * 0.55,
            face[1] + (ground_normal[1] - face[1]) * toward_ground * 0.55,
            face[2] + (ground_normal[2] - face[2]) * toward_ground * 0.55,
        ]);

        for edge in [-1.0_f32, 1.0] {
            let position = [
                blade.position[0] + point[0] + side[0] * width * edge,
                blade.position[1] + point[1] + side[1] * width * edge,
                blade.position[2] + point[2] + side[2] * width * edge,
            ];
            vertices.push(loom_asset::Vertex {
                position: [position[0], position[1], position[2], 1.0],
                normal: [normal[0], normal[1], normal[2], 0.0],
                // V runs base to tip. **The shader darkens toward the base
                // from this** — the cheapest high-value trick available, and
                // the stand-in for the ambient occlusion this renderer does
                // not have.
                uv: [edge.mul_add(0.5, 0.5), t],
            });
        }
    }

    // Two triangles per segment, wound consistently. Both faces are drawn —
    // grass has no back — so the winding only has to be self-consistent.
    for step in 0..segments {
        let i = base + u32::try_from(step).unwrap_or(0) * 2;
        indices.extend_from_slice(&[i, i + 1, i + 2, i + 2, i + 1, i + 3]);
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let length = v[0].mul_add(v[0], v[1].mul_add(v[1], v[2] * v[2])).sqrt();
    if length > 1e-6 { [v[0] / length, v[1] / length, v[2] / length] } else { [0.0, 1.0, 0.0] }
}

/// Build a mesh for every blade in a list.
#[must_use]
pub fn mesh(blades: &[Blade], ground_normal: [f32; 3], segments: usize) -> loom_asset::Mesh {
    let mut vertices = Vec::with_capacity(blades.len() * (segments + 1) * 2);
    let mut indices = Vec::with_capacity(blades.len() * segments * 6);
    for blade in blades {
        emit(blade, ground_normal, segments, &mut vertices, &mut indices);
    }
    loom_asset::Mesh { name: "grass".to_owned(), vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rules, TILE, Tile};

    fn one() -> Blade {
        Blade {
            position: [1.0, 2.0, 3.0],
            facing: [1.0, 0.0],
            height: 0.4,
            width: 0.02,
            tilt: 0.3,
            bend: 0.7,
            shade: 1.0,
            hue: 0.0,
            clump: 0,
        }
    }

    /// The strip is well-formed: two vertices per ring, six indices per
    /// segment, and every index inside the vertex list.
    #[test]
    fn a_blade_is_a_closed_strip_of_triangles() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        emit(&one(), [0.0, 1.0, 0.0], SEGMENTS_NEAR, &mut vertices, &mut indices);

        assert_eq!(vertices.len(), (SEGMENTS_NEAR + 1) * 2);
        assert_eq!(indices.len(), SEGMENTS_NEAR * 6);
        for index in &indices {
            assert!((*index as usize) < vertices.len(), "index {index} is past the end");
        }
    }

    /// **The published LOD numbers.** Fifteen vertices near, seven far.
    #[test]
    fn the_two_levels_of_detail_match_the_reference_numbers() {
        let near = mesh(&[one()], [0.0, 1.0, 0.0], SEGMENTS_NEAR);
        let far = mesh(&[one()], [0.0, 1.0, 0.0], SEGMENTS_FAR);

        assert_eq!(near.vertices.len(), 16, "one ring more than the 15 quoted");
        assert_eq!(far.vertices.len(), 8);
        assert!(far.indices.len() * 2 <= near.indices.len(), "the far LOD is not cheaper");
    }

    /// The blade starts at its base and ends a height above it, leaning the
    /// way it faces. A blade that does not reach its own height is a bug that
    /// looks like the grass being short.
    #[test]
    fn a_blade_starts_at_its_base_and_leans_the_way_it_faces() {
        let blade = one();
        let built = mesh(&[blade], [0.0, 1.0, 0.0], SEGMENTS_NEAR);

        let first = built.vertices[0].position;
        let tip = built.vertices[built.vertices.len() - 1].position;

        assert!((first[1] - blade.position[1]).abs() < 1e-4, "base is off the ground");
        assert!(tip[1] > first[1], "the tip is not above the base");
        assert!(
            tip[0] > first[0] + 0.01,
            "the blade does not lean toward its facing: {first:?} -> {tip:?}"
        );
        let reach = (tip[1] - blade.position[1]).hypot(tip[0] - blade.position[0]);
        assert!(
            (reach - blade.height).abs() < blade.height * 0.15,
            "a {}m blade reaches {reach}m",
            blade.height
        );
    }

    /// It tapers. A blade the same width at the tip as the base is a ribbon,
    /// and reads as one.
    #[test]
    fn the_blade_narrows_toward_the_tip() {
        let built = mesh(&[one()], [0.0, 1.0, 0.0], SEGMENTS_NEAR);

        let span = |ring: usize| {
            let a = built.vertices[ring * 2].position;
            let b = built.vertices[ring * 2 + 1].position;
            (a[0] - b[0]).hypot(a[2] - b[2])
        };

        assert!(span(0) > span(SEGMENTS_NEAR / 2), "no taper in the first half");
        assert!(span(SEGMENTS_NEAR) < span(0) * 0.1, "the tip is still wide");
    }

    /// **Shading normals are not geometric normals**, deliberately. Near the
    /// base they lean toward the ground; at the tip they are the blade's own.
    /// Lighting a flat blade honestly makes it read as paper.
    #[test]
    fn normals_blend_toward_the_ground_at_the_base() {
        let ground = [0.0, 1.0, 0.0];
        // A blade lying well over, so its true normal is far from the ground's.
        let blade = Blade { tilt: 1.2, ..one() };
        let built = mesh(&[blade], ground, SEGMENTS_NEAR);

        // **Measured by moving the ground, not by reading one normal.** The
        // geometric normal already varies along the blade — a vertical blade's
        // face points sideways, a flattened one's points up — so comparing the
        // base's normal to the tip's measures the curve, not the blend. Two
        // runs with different ground normals isolate it: whichever end is
        // blended harder moves further.
        let tilted = mesh(&[blade], [0.6, 0.8, 0.0], SEGMENTS_NEAR);
        let moved = |ring: usize| {
            let a = built.vertices[ring * 2].normal;
            let b = tilted.vertices[ring * 2].normal;
            (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
        };

        assert!(
            moved(0) > moved(SEGMENTS_NEAR) * 1.5,
            "the base ({}) follows the ground no more than the tip ({}) — \
             the blend is not stronger at the base",
            moved(0),
            moved(SEGMENTS_NEAR)
        );
        for v in &built.vertices {
            let n = v.normal;
            let length = n[0].mul_add(n[0], n[1].mul_add(n[1], n[2] * n[2])).sqrt();
            assert!((length - 1.0).abs() < 1e-3, "normal is not unit: {n:?}");
        }
    }

    /// V runs base to tip, which is what the shader darkens from.
    #[test]
    fn the_texture_coordinate_runs_from_base_to_tip() {
        let built = mesh(&[one()], [0.0, 1.0, 0.0], SEGMENTS_NEAR);

        assert!((built.vertices[0].uv[1] - 0.0).abs() < 1e-6);
        assert!((built.vertices[built.vertices.len() - 1].uv[1] - 1.0).abs() < 1e-6);
    }

    /// Nothing degenerate, at any tilt — including straight up, where the
    /// lean vanishes and a naive tangent would be undefined.
    #[test]
    fn every_vertex_is_finite_at_every_tilt() {
        for tilt in [0.0_f32, 0.001, 0.5, 1.4, std::f32::consts::FRAC_PI_2] {
            let blade = Blade { tilt, ..one() };
            let built = mesh(&[blade], [0.0, 1.0, 0.0], SEGMENTS_NEAR);
            for v in &built.vertices {
                assert!(v.position.iter().all(|f| f.is_finite()), "tilt {tilt}: {v:?}");
                assert!(v.normal.iter().all(|f| f.is_finite()), "tilt {tilt}: {v:?}");
            }
        }
    }

    /// A whole tile turns into a mesh of a believable size — the number that
    /// decides whether this approach is affordable at all.
    #[test]
    fn a_tile_of_grass_is_a_mesh_of_a_sane_size() {
        let rules = Rules::default();
        let blades = crate::tile(Tile { x: 0, z: 0 }, &rules, &|_, _| crate::Ground::default());
        let built = mesh(&blades, [0.0, 1.0, 0.0], SEGMENTS_NEAR);

        assert_eq!(built.vertices.len(), blades.len() * 16);
        assert_eq!(built.indices.len(), blades.len() * SEGMENTS_NEAR * 6);
        // 1,600 blades a tile at the default density.
        assert!(
            (1200..2200).contains(&blades.len()),
            "{} blades in a {TILE}m tile",
            blades.len()
        );
    }
}
