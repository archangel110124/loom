//! How *rocky* a mesh is, as a number.
//!
//! **The problem this exists for.** Every primitive `loom_voxel` offers is
//! analytic and smooth and union is a hard `min`, so a shape assembled from
//! them has detail at exactly one scale and every joint is infinitely sharp.
//! [`crate::mesh::Mesh`] is where that shows up, and until now the only way to
//! judge it was to render it and look — the worst iteration loop an agent can
//! be given. `Displace` was shipped with four numbers chosen by eye for exactly
//! that reason.
//!
//! **The statistic that separates a rock from a blob is the sign of the
//! curvature, not its magnitude.** A union of spheres is convex nearly
//! everywhere: it bulges outward and creases inward on a measure-zero curve, so
//! its concave *area* is a few percent. Real stone is fractured, and a fracture
//! is a concave surface with area. Measured here, on this machine:
//!
//! ```text
//! photogrammetry boulder (rock_boulder_a.obj)   concave 51.9%
//! ridged-displaced sphere, voxel 0.03           concave 44.8%
//! 49-op union of spheres and capsules, hard min concave  4.5%
//! ```
//!
//! The radial power spectrum does not separate these; curvature sign does.
//!
//! # The rule, and it is not optional
//!
//! **Only ever compare a mesh against itself at equal `voxel_size`.** Discrete
//! curvature on a Surface Nets mesh is resolution-dependent — a finer bake
//! resolves quantisation ripple that a coarser one smooths away, and the finer
//! one therefore "wins" on concave area for no shape reason at all. This is
//! report-only for the same reason `loom flicker` is: a threshold on it would
//! move every time `voxel_size`, the mesher or Dual Contouring changed. It is
//! **not** a fifth green check, and `loom measure --shape` prints the rule and
//! the `voxel_size` in its own output so the comparison stays checkable.
//!
//! # What it computes
//!
//! Discrete mean curvature from the cotangent Laplacian (Meyer et al. 2003):
//! `H(v) = |L(v)| / (4·A(v))` where `L(v) = Σ_j (cot α + cot β)(x_j − x_i)` and
//! `A(v)` is a third of each incident triangle's area. `L` points inward on a
//! convex surface, so `L·n > 0` is concave. Magnitudes are reported as
//! `log₁₀(H · mean_radius)`, which puts a sphere at 0 whatever its size.

use crate::Mesh;

/// Curvature statistics of a triangle mesh. See the module docs for the rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeStats {
    pub triangles: usize,
    /// Distinct vertices after welding by position. A voxel mesh arrives as one
    /// island per chunk, so the raw count is meaningless for curvature.
    pub vertices: usize,
    /// Vertices on an open boundary, **excluded** from every figure below. The
    /// cotangent Laplacian is one-sided there and reads as enormous curvature;
    /// a terrain patch clipped at the volume edge is a ring of them. Watch this
    /// number: a large fraction means the weld failed, not that the shape is open.
    pub boundary: usize,
    pub area: f32,
    /// Fraction of surface **area** whose mean curvature is negative. This is
    /// the statistic; the rest is context.
    pub concave: f32,
    /// Percentiles of `log₁₀(H · mean_radius)`, and `p90 − p10` in decades.
    /// A shape with detail at one scale is narrow; a fractal one is wide.
    pub p10: f32,
    pub p50: f32,
    pub p90: f32,
    pub spread: f32,
    pub mean_radius: f32,
}

impl ShapeStats {
    /// The all-zero result, for a mesh with no triangles.
    const EMPTY: Self = Self {
        triangles: 0,
        vertices: 0,
        boundary: 0,
        area: 0.0,
        concave: 0.0,
        p10: 0.0,
        p50: 0.0,
        p90: 0.0,
        spread: 0.0,
        mean_radius: 0.0,
    };
}

/// Curvature statistics for one mesh.
///
/// Cost is linear in triangles and dominated by the weld's sort: 2.4 ms on the
/// 62,660-triangle `rock_boulder_a.obj` [measured].
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn stats(mesh: &Mesh) -> ShapeStats {
    if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
        return ShapeStats::EMPTY;
    }
    let pos: Vec<[f64; 3]> = mesh
        .vertices
        .iter()
        .map(|v| [f64::from(v.position[0]), f64::from(v.position[1]), f64::from(v.position[2])])
        .collect();

    let (lo, hi) = mesh.bounds();
    let diag = f64::from(
        ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt(),
    );
    // **Weld first, or nothing below means anything.** `mesh_volume` appends
    // each chunk's vertices, so a voxel mesh is a pile of disconnected islands
    // whose every vertex is a boundary vertex. The tolerance is coarse relative
    // to float noise and fine relative to any voxel size: distinct Surface Nets
    // vertices are a voxel apart.
    let eps = (diag * 1e-4).max(1e-9);
    let key = |p: &[f64; 3]| {
        [(p[0] / eps).round() as i64, (p[1] / eps).round() as i64, (p[2] / eps).round() as i64]
    };
    // Sorted, not hashed: deterministic order, and no `HashMap` (clippy.toml).
    let mut order: Vec<u32> = (0..pos.len() as u32).collect();
    order.sort_unstable_by_key(|&i| key(&pos[i as usize]));
    let mut remap = vec![0_u32; pos.len()];
    let mut welded: Vec<[f64; 3]> = Vec::new();
    for (n, &i) in order.iter().enumerate() {
        if n == 0 || key(&pos[order[n - 1] as usize]) != key(&pos[i as usize]) {
            welded.push(pos[i as usize]);
        }
        remap[i as usize] = welded.len() as u32 - 1;
    }

    let n = welded.len();
    let mut lap = vec![[0.0_f64; 3]; n];
    let mut vert_area = vec![0.0_f64; n];
    let mut normal = vec![[0.0_f64; 3]; n];
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(mesh.indices.len());
    let mut triangles = 0_usize;

    for tri in mesh.indices.chunks_exact(3) {
        let v = [
            remap[tri[0] as usize] as usize,
            remap[tri[1] as usize] as usize,
            remap[tri[2] as usize] as usize,
        ];
        if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
            continue; // Collapsed by the weld, or degenerate in the source.
        }
        let p = [welded[v[0]], welded[v[1]], welded[v[2]]];
        let cr = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let twice_area = len(cr);
        if twice_area < 1e-18 {
            continue;
        }
        triangles += 1;
        for k in 0..3 {
            vert_area[v[k]] += twice_area / 6.0;
            // Area-weighted, which is what an unnormalised face normal already is.
            for axis in 0..3 {
                normal[v[k]][axis] += cr[axis];
            }
            // Cotangent of the angle at corner `k`, contributed to the opposite
            // edge. Summing over triangles gives each interior edge both of its
            // opposite angles, which is the whole cot formula.
            let (i, j) = (v[(k + 1) % 3], v[(k + 2) % 3]);
            let e1 = sub(p[(k + 1) % 3], p[k]);
            let e2 = sub(p[(k + 2) % 3], p[k]);
            let s = len(cross(e1, e2));
            // Clamped: a sliver triangle's cotangent goes to infinity and one
            // of them would otherwise decide the whole statistic.
            let w = if s < 1e-18 { 0.0 } else { (dot(e1, e2) / s).clamp(-1e4, 1e4) };
            let d = sub(welded[j], welded[i]);
            for axis in 0..3 {
                lap[i][axis] += w * d[axis];
                lap[j][axis] -= w * d[axis];
            }
            edges.push((i.min(j) as u32, i.max(j) as u32));
        }
    }
    if triangles == 0 {
        return ShapeStats::EMPTY;
    }

    // An edge used by one triangle is an open boundary.
    edges.sort_unstable();
    let mut on_boundary = vec![false; n];
    let mut e = 0;
    while e < edges.len() {
        let mut run = 1;
        while e + run < edges.len() && edges[e + run] == edges[e] {
            run += 1;
        }
        if run == 1 {
            on_boundary[edges[e].0 as usize] = true;
            on_boundary[edges[e].1 as usize] = true;
        }
        e += run;
    }

    let total_area: f64 = vert_area.iter().sum();
    let mut centroid = [0.0_f64; 3];
    for (i, p) in welded.iter().enumerate() {
        for axis in 0..3 {
            centroid[axis] += p[axis] * vert_area[i];
        }
    }
    for c in &mut centroid {
        *c /= total_area.max(1e-18);
    }
    let mut mean_radius = 0.0_f64;
    for (i, p) in welded.iter().enumerate() {
        mean_radius += len(sub(*p, centroid)) * vert_area[i];
    }
    mean_radius /= total_area.max(1e-18);

    let mut concave_area = 0.0_f64;
    let mut measured_area = 0.0_f64;
    let mut logs: Vec<f64> = Vec::with_capacity(n);
    let mut boundary = 0_usize;
    for i in 0..n {
        if on_boundary[i] {
            boundary += 1;
            continue;
        }
        let a = vert_area[i];
        if a < 1e-18 {
            continue;
        }
        measured_area += a;
        // |L| / 4A is |H|; L points inward where the surface bulges out.
        let h = len(lap[i]) / (4.0 * a);
        if dot(lap[i], normal[i]) > 0.0 {
            concave_area += a;
        }
        // Floored so a perfectly flat vertex does not take the log to -inf and
        // the spread with it. Six decades below a sphere of the same size is
        // flat by any standard this metric is used at.
        logs.push((h * mean_radius).max(1e-6).log10());
    }
    logs.sort_unstable_by(f64::total_cmp);
    let pct = |q: f64| -> f32 {
        if logs.is_empty() {
            0.0
        } else {
            logs[((logs.len() - 1) as f64 * q).round() as usize] as f32
        }
    };
    let (p10, p50, p90) = (pct(0.10), pct(0.50), pct(0.90));

    ShapeStats {
        triangles,
        vertices: n,
        boundary,
        area: total_area as f32,
        concave: (concave_area / measured_area.max(1e-18)) as f32,
        p10,
        p50,
        p90,
        spread: p90 - p10,
        mean_radius: mean_radius as f32,
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere is convex everywhere and its mean curvature is `1/R`
    /// everywhere, which is the one case with an exact answer. Both halves
    /// matter: `concave` pins the *sign* convention and `p50` pins the
    /// *magnitude* and the mean-radius normalisation together.
    #[test]
    fn a_sphere_is_convex_and_scores_one_over_r() {
        let s = stats(&crate::primitives::sphere(48, 24));
        assert_eq!(s.boundary, 0, "a UV sphere must weld closed, got {} open", s.boundary);
        assert!(s.concave < 0.02, "a sphere is convex: {}", s.concave);
        // log10(H * mean_radius) = log10(1) = 0 for any sphere.
        assert!(s.p50.abs() < 0.05, "p50 should be 0 for a sphere, got {}", s.p50);
        assert!(s.spread < 0.15, "one scale of detail is a narrow spread: {}", s.spread);
    }

    /// **The sign convention, tested by breaking it.** Reversing the winding
    /// flips every face normal and nothing else, so a shape that read 100%
    /// convex must read 100% concave. Without this, `concave` could be reading
    /// the wrong side of the surface and every number in this module's docs
    /// would be its complement.
    #[test]
    fn reversed_winding_reads_inside_out() {
        let mut mesh = crate::primitives::sphere(48, 24);
        for tri in mesh.indices.chunks_exact_mut(3) {
            tri.swap(0, 2);
        }
        let s = stats(&mesh);
        assert!(s.concave > 0.98, "an inside-out sphere is concave: {}", s.concave);
    }

    /// A plane is one open boundary ring and flat inside. It checks that the
    /// boundary exclusion fires — an unexcluded ring reads as huge curvature
    /// and would swamp any terrain patch.
    #[test]
    fn a_plane_is_flat_inside_its_boundary() {
        let s = stats(&crate::primitives::plane());
        assert!(s.boundary > 0, "a plane has an open edge");
        assert!(s.concave < 0.01, "flat is not concave: {}", s.concave);
    }

    #[test]
    fn an_empty_mesh_measures_zero() {
        assert_eq!(stats(&Mesh::default()), ShapeStats::EMPTY);
    }
}
