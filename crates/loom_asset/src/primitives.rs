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
        // **Tessellation is a physics-visible number here, not a style
        // choice.** These collide as analytic balls, cylinders and capsules,
        // but are drawn as polyhedra inscribed in those shapes — every
        // triangle sags inward between its vertices. At 24 segments the sag
        // was 1.32% of the radius: 9.3mm on the tower scene's 0.7m wrecking
        // ball, so a crate resting against it stopped 9mm short of the drawn
        // surface and visibly sank into it. The simulation was right and the
        // picture was wrong, which is the one failure this engine cannot have
        // — the render is how the agent checks its own work.
        //
        // 64 segments puts the sag at 1.2mm, just under the solver's own
        // resting penetration at that radius, so it stops being the artifact
        // you notice. Costs 3185 vertices for a shape a scene uses a handful
        // of times.
        "sphere" => Some(sphere(64, 48)),
        "cylinder" => Some(cylinder(64)),
        "capsule" => Some(capsule(64, 16)),
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
            // Each face gets the whole unit square, so a texture reads at the
            // same scale on all six regardless of how the cube is scaled.
            mesh.vertices.push(Vertex::with_uv(
                position,
                normal,
                [su.mul_add(0.5, 0.5), sv.mul_add(0.5, 0.5)],
            ));
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
            Vertex::with_uv([-1.0, 0.0, 1.0], n, [0.0, 0.0]),
            Vertex::with_uv([1.0, 0.0, 1.0], n, [1.0, 0.0]),
            Vertex::with_uv([1.0, 0.0, -1.0], n, [1.0, 1.0]),
            Vertex::with_uv([-1.0, 0.0, -1.0], n, [0.0, 1.0]),
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
            // On a unit sphere the position *is* the normal. The UV grid is
            // the sphere's own parameterisation — longitude across, latitude
            // down — which is the mapping an equirectangular texture expects.
            mesh.vertices.push(Vertex::with_uv(position, position, [u, v]));
        }
    }
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let b = a + stride;
            // **Wound to face outward, which is not the obvious order here.**
            // This grid's rings run top to bottom while its segments run
            // counter-clockwise, so the naive `[a, b, a + 1]` crosses to the
            // *inward* normal and every triangle on the sphere gets backface
            // culled. The renderer then draws the inside of the far hemisphere
            // instead: the silhouette still looks like a sphere, so it reads as
            // a shading bug, and the visible surface depth-tests at the back of
            // the ball, which is why other objects appeared to sink into it.
            //
            // The capsule below uses the opposite order for the same reason —
            // its rings run bottom to top.
            mesh.indices
                .extend([a, a + 1, b, a + 1, b + 1, b]);
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
        let u = segment as f32 / segments as f32;
        let theta = u * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        let normal = [c, 0.0, s];
        mesh.vertices.push(Vertex::with_uv([c, -1.0, s], normal, [u, 0.0]));
        mesh.vertices.push(Vertex::with_uv([c, 1.0, s], normal, [u, 1.0]));
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
        // The cap is a disc, so its unwrap is the texture's inscribed circle
        // rather than a strip: centre at the middle, rim on the unit circle.
        mesh.vertices.push(Vertex::with_uv([0.0, y, 0.0], normal, [0.5, 0.5]));
        for segment in 0..=segments {
            #[allow(clippy::cast_precision_loss)]
            let theta = (segment as f32 / segments as f32) * std::f32::consts::TAU;
            let (s, c) = theta.sin_cos();
            mesh.vertices.push(Vertex::with_uv(
                [c, y, s],
                normal,
                [c.mul_add(0.5, 0.5), s.mul_add(0.5, 0.5)],
            ));
        }
        for segment in 0..segments {
            let a = centre + 1 + segment;
            // Wind the bottom cap the other way so both face outward.
            //
            // These two were swapped. Going round the rim in increasing theta
            // and fanning from the centre crosses to -Y, so it is the *top*
            // cap that needs reversing, not the bottom. Both caps therefore
            // faced inward and were culled, leaving a tube open at both ends
            // with the far inner wall showing through.
            if y < 0.0 {
                mesh.indices.extend([centre, a, a + 1]);
            } else {
                mesh.indices.extend([centre, a + 1, a]);
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

        #[allow(clippy::cast_precision_loss)]
        let v = ring as f32 / total_rings as f32;
        for segment in 0..=segments {
            #[allow(clippy::cast_precision_loss)]
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (s, c) = theta.sin_cos();
            let normal = [phi.cos() * c, phi.sin(), phi.cos() * s];
            let position = [normal[0], normal[1] + offset, normal[2]];
            // Ring index rather than arc length, so the two hemispheres and
            // the band share one continuous unwrap. The caps are slightly
            // compressed against the band, which is what a capsule texture
            // expects anyway.
            mesh.vertices.push(Vertex::with_uv(position, normal, [u, v]));
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

    /// Every primitive is unwrapped across the full `0..1` range on both axes.
    ///
    /// A primitive whose UVs were all zero would sample a single texel of its
    /// albedo map and render as one flat colour — which reads as a broken
    /// texture rather than as a missing unwrap, and is the kind of thing that
    /// gets debugged in the shader for an hour.
    #[test]
    fn every_primitive_is_unwrapped() {
        for name in NAMES {
            let mesh = build(name).expect("NAMES only holds real primitives");
            for axis in 0..2 {
                let min = mesh.vertices.iter().map(|v| v.uv[axis]).fold(f32::MAX, f32::min);
                let max = mesh.vertices.iter().map(|v| v.uv[axis]).fold(f32::MIN, f32::max);
                assert!(
                    (max - min) > 0.99,
                    "{name} spans only {min}..{max} on uv axis {axis}"
                );
                assert!(
                    (-0.001..=1.001).contains(&min) && (-0.001..=1.001).contains(&max),
                    "{name} leaves the unit square on uv axis {axis}: {min}..{max}"
                );
            }
        }
    }

    /// Every triangle must wind the same way its vertices claim to face.
    ///
    /// Winding decides which side the rasteriser keeps — the front face is
    /// counter-clockwise and the back is culled. A triangle wound against its
    /// own normals is therefore **invisible from the side it is lit on**, and
    /// the hole it leaves looks like missing geometry rather than a winding
    /// bug, which is a long way to walk from the symptom to the cause.
    ///
    /// Checked against the vertex normals rather than against a hand-written
    /// expectation, because the normals are what the shader lights with: if
    /// the two disagree, one of them is wrong whichever it is.
    #[test]
    fn every_triangle_winds_the_way_its_normals_face() {
        for name in NAMES {
            let mesh = build(name).expect("NAMES only holds real primitives");

            for (triangle, corner) in mesh.indices.chunks_exact(3).enumerate() {
                let v: Vec<&Vertex> = corner
                    .iter()
                    .map(|i| &mesh.vertices[*i as usize])
                    .collect();
                let edge = |a: &Vertex, b: &Vertex| {
                    [
                        b.position[0] - a.position[0],
                        b.position[1] - a.position[1],
                        b.position[2] - a.position[2],
                    ]
                };
                let (e1, e2) = (edge(v[0], v[1]), edge(v[0], v[2]));
                // Right-hand rule: this is the direction a counter-clockwise
                // winding actually faces.
                let wound = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                let claimed: Vec<f32> = (0..3)
                    .map(|axis| v.iter().map(|x| x.normal[axis]).sum::<f32>() / 3.0)
                    .collect();

                let agreement: f32 = (0..3).map(|a| wound[a] * claimed[a]).sum();
                let area: f32 = wound.iter().map(|c| c * c).sum::<f32>().sqrt();
                // Degenerate slivers — a sphere's pole fan — have no meaningful
                // winding direction and are skipped rather than guessed at.
                if area < 1e-9 {
                    continue;
                }
                assert!(
                    agreement > 0.0,
                    "{name} triangle {triangle} is wound {wound:?} but its \
                     normals face {claimed:?} — it will be backface culled"
                );
            }
        }
    }

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

    /// The nearest point of `mesh`'s surface to the origin, as a fraction of
    /// the radius it is meant to have.
    ///
    /// A tessellated round shape is a polyhedron *inscribed* in the shape it
    /// stands for: its vertices sit on the surface, and every triangle sags
    /// inward between them. This measures that sag.
    fn inscribed_fraction(mesh: &Mesh) -> f32 {
        let mut closest = f32::MAX;
        for tri in mesh.indices.chunks_exact(3) {
            let p = mesh.vertices[tri[0] as usize].position;
            let q = mesh.vertices[tri[1] as usize].position;
            let r = mesh.vertices[tri[2] as usize].position;
            let u = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
            let w = [r[0] - p[0], r[1] - p[1], r[2] - p[2]];
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if length < 1e-9 {
                continue; // degenerate triangle at a pole
            }
            let distance = (n[0] * p[0] + n[1] * p[1] + n[2] * p[2]).abs() / length;
            closest = closest.min(distance);
        }
        closest
    }

    /// **What we draw has to match what we collide.** A sphere collides as an
    /// analytic ball of its radius, but is *drawn* as a polyhedron inscribed in
    /// that ball. At 24x16 the drawn surface sagged 1.33% of the radius inside
    /// the collider — 9.3mm on the tower scene's 0.7m wrecking ball — so
    /// anything resting against it stopped 9mm short of the surface and visibly
    /// sank into it. The simulation was right; the picture was wrong.
    ///
    /// 0.2% keeps the sag below the solver's own resting penetration (~1.3mm
    /// at that radius), so it stops being the artifact you notice.
    #[test]
    fn a_drawn_sphere_matches_the_ball_it_collides_as() {
        let sag = 1.0 - inscribed_fraction(&sphere(64, 48));

        assert!(
            sag < 0.002,
            "drawn sphere sags {:.4}% inside its collider; \
             anything resting on it will appear to sink in",
            sag * 100.0
        );
    }

    /// The same mismatch, and the same fix, for the other round primitives —
    /// they collide as analytic cylinders and capsules too.
    #[test]
    fn the_other_round_primitives_match_their_colliders_too() {
        for (name, sag) in [
            ("cylinder", 1.0 - inscribed_fraction(&cylinder(64))),
            ("capsule", 1.0 - inscribed_fraction(&capsule(64, 16))),
        ] {
            assert!(sag < 0.002, "drawn {name} sags {:.4}% inside", sag * 100.0);
        }
    }
}
