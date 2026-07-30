//! Procedurally generated primitives.
//!
//! Design doc Phase 1 is emphatic about this: without a primitive library,
//! 3D blockout is blocked on having art, and the agent has nothing to compose.
//! Blocking out with primitives is how human level designers work anyway.

use crate::mesh::{Mesh, Vertex};

/// Every primitive kind, by the name a `.loom` file uses.
pub const NAMES: [&str; 5] = ["box", "plane", "sphere", "cylinder", "capsule"];

/// Build a primitive by name, or `None` if the name is not one.
#[must_use]
pub fn build(name: &str) -> Option<Mesh> {
    match name {
        "box" => Some(box_mesh()),
        "plane" => Some(plane()),
        "sphere" => Some(sphere(24, 16)),
        "cylinder" => Some(cylinder(24)),
        "capsule" => Some(capsule(24, 8)),
        _ => None,
    }
}

/// A unit cube spanning -1..1, with flat per-face normals.
///
/// Faces do not share vertices: a shared corner would have to average three
/// perpendicular normals, which rounds off every edge.
#[must_use]
pub fn box_mesh() -> Mesh {
    const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ];

    let mut mesh = Mesh {
        name: "box".into(),
        ..Mesh::default()
    };
    for (normal, u, v) in FACES {
        let base = u32::try_from(mesh.vertices.len()).unwrap_or(0);
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let position = [
                normal[0] + u[0] * su + v[0] * sv,
                normal[1] + u[1] * su + v[1] * sv,
                normal[2] + u[2] * su + v[2] * sv,
            ];
            mesh.vertices.push(Vertex::new(position, normal));
        }
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

/// A flat 2x2 quad in the XZ plane, facing up.
#[must_use]
pub fn plane() -> Mesh {
    let n = [0.0, 1.0, 0.0];
    Mesh {
        name: "plane".into(),
        vertices: vec![
            Vertex::new([-1.0, 0.0, 1.0], n),
            Vertex::new([1.0, 0.0, 1.0], n),
            Vertex::new([1.0, 0.0, -1.0], n),
            Vertex::new([-1.0, 0.0, -1.0], n),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

/// A UV sphere of radius 1.
#[must_use]
pub fn sphere(segments: u32, rings: u32) -> Mesh {
    let mut mesh = Mesh {
        name: "sphere".into(),
        ..Mesh::default()
    };
    for ring in 0..=rings {
        #[allow(clippy::cast_precision_loss)]
        let v = ring as f32 / rings as f32;
        let phi = v * std::f32::consts::PI;
        for segment in 0..=segments {
            #[allow(clippy::cast_precision_loss)]
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let position = [
                phi.sin() * theta.cos(),
                phi.cos(),
                phi.sin() * theta.sin(),
            ];
            // On a unit sphere the position *is* the normal.
            mesh.vertices.push(Vertex::new(position, position));
        }
    }
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let b = a + stride;
            mesh.indices
                .extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

/// A capped cylinder, radius 1, spanning y -1..1.
#[must_use]
pub fn cylinder(segments: u32) -> Mesh {
    let mut mesh = Mesh {
        name: "cylinder".into(),
        ..Mesh::default()
    };

    // Side wall: duplicated ring so the seam normal is not averaged.
    for segment in 0..=segments {
        #[allow(clippy::cast_precision_loss)]
        let theta = (segment as f32 / segments as f32) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        let normal = [c, 0.0, s];
        mesh.vertices.push(Vertex::new([c, -1.0, s], normal));
        mesh.vertices.push(Vertex::new([c, 1.0, s], normal));
    }
    for segment in 0..segments {
        let a = segment * 2;
        mesh.indices
            .extend([a, a + 1, a + 2, a + 1, a + 3, a + 2]);
    }

    // Caps, each with its own fan centre so the flat normal is not shared
    // with the curved wall.
    for (y, normal) in [(-1.0_f32, [0.0, -1.0, 0.0]), (1.0, [0.0, 1.0, 0.0])] {
        let centre = u32::try_from(mesh.vertices.len()).unwrap_or(0);
        mesh.vertices.push(Vertex::new([0.0, y, 0.0], normal));
        for segment in 0..=segments {
            #[allow(clippy::cast_precision_loss)]
            let theta = (segment as f32 / segments as f32) * std::f32::consts::TAU;
            let (s, c) = theta.sin_cos();
            mesh.vertices.push(Vertex::new([c, y, s], normal));
        }
        for segment in 0..segments {
            let a = centre + 1 + segment;
            // Wind the bottom cap the other way so both face outward.
            if y < 0.0 {
                mesh.indices.extend([centre, a + 1, a]);
            } else {
                mesh.indices.extend([centre, a, a + 1]);
            }
        }
    }
    mesh
}

/// A capsule: a cylinder of half-height 1 with hemispherical caps of radius 1.
#[must_use]
pub fn capsule(segments: u32, rings: u32) -> Mesh {
    let mut mesh = Mesh {
        name: "capsule".into(),
        ..Mesh::default()
    };

    // Two hemispheres offset along Y, joined by a cylindrical band. Built as
    // one vertex grid so the seams share vertices and shade continuously.
    let total_rings = rings * 2 + 1;
    for ring in 0..=total_rings {
        let top = ring > rings;
        #[allow(clippy::cast_precision_loss)]
        let t = if top {
            (ring - rings - 1) as f32 / rings as f32
        } else {
            ring as f32 / rings as f32
        };
        let phi = if top {
            t * std::f32::consts::FRAC_PI_2
        } else {
            (t - 1.0) * std::f32::consts::FRAC_PI_2
        };
        let offset = if top { 1.0 } else { -1.0 };

        for segment in 0..=segments {
            #[allow(clippy::cast_precision_loss)]
            let theta = (segment as f32 / segments as f32) * std::f32::consts::TAU;
            let (s, c) = theta.sin_cos();
            let normal = [phi.cos() * c, phi.sin(), phi.cos() * s];
            let position = [normal[0], normal[1] + offset, normal[2]];
            mesh.vertices.push(Vertex::new(position, normal));
        }
    }
    let stride = segments + 1;
    for ring in 0..total_rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let b = a + stride;
            mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_primitive_builds_and_validates() {
        for name in NAMES {
            let mesh = build(name).unwrap_or_else(|| panic!("{name} should build"));
            mesh.validate()
                .unwrap_or_else(|e| panic!("{name} is malformed: {e}"));
            assert!(!mesh.vertices.is_empty(), "{name} has no vertices");
            assert_eq!(mesh.indices.len() % 3, 0, "{name} has partial triangles");
        }
    }

    #[test]
    fn an_unknown_primitive_is_none() {
        assert!(build("dodecahedron").is_none());
    }

    #[test]
    fn the_box_spans_minus_one_to_one() {
        let (min, max) = box_mesh().bounds();

        assert_eq!(min, [-1.0, -1.0, -1.0]);
        assert_eq!(max, [1.0, 1.0, 1.0]);
    }

    /// A shared corner would average three perpendicular normals and round
    /// every edge off. 6 faces x 4 corners = 24, not 8.
    #[test]
    fn box_faces_do_not_share_vertices() {
        assert_eq!(box_mesh().vertices.len(), 24);
    }

    /// Every normal must be unit length, or lighting is subtly wrong in a way
    /// that reads as "the shading looks off" and is hard to trace.
    #[test]
    fn primitive_normals_are_unit_length() {
        for name in NAMES {
            let mesh = build(name).unwrap();
            for (i, v) in mesh.vertices.iter().enumerate() {
                let n = v.normal;
                let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!(
                    (length - 1.0).abs() < 1e-3,
                    "{name} vertex {i} normal length {length}"
                );
            }
        }
    }

    /// A capsule is taller than it is wide — half-height 1 plus two radius-1
    /// caps is 2 either side of centre.
    #[test]
    fn the_capsule_is_two_units_taller_than_the_sphere() {
        let (min, max) = capsule(16, 6).bounds();

        assert!((min[1] - -2.0).abs() < 1e-3, "bottom at {}", min[1]);
        assert!((max[1] - 2.0).abs() < 1e-3, "top at {}", max[1]);
        assert!((max[0] - 1.0).abs() < 1e-3, "radius {}", max[0]);
    }
}
