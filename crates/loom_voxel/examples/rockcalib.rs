//! Calibrate [`loom_voxel::Displace`] against a photogrammetry boulder.
//!
//! **The claim this makes reproducible.** `Displace` shipped with four numbers
//! chosen by eye, because the only way to judge them was to render and look.
//! `loom_asset::shape` turns "does that read as stone" into concave surface-area
//! fraction, and this example is the sweep that picks the numbers with it.
//!
//! Run it: `cargo run --release -p loom_voxel --example rockcalib`
//!
//! **Read the rule in the header before comparing anything to anything.** The
//! metric is resolution-dependent, so a row is only comparable to a row at the
//! same `voxel_size` — which is exactly what the last table demonstrates rather
//! than assumes.
#![allow(clippy::disallowed_methods, clippy::cast_precision_loss, clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]

use loom_asset::shape::{ShapeStats, stats};
use loom_voxel::{CsgMode, Displace, SurfaceNets, Volume, VoxelOp, mesh::mesh_volume};

/// The scanned rocks every row is aiming at. Three, not one, because a single
/// scan gives a number and three give a *band* — and a band is the only honest
/// thing to calibrate against when the target is "reads as stone".
const TARGETS: [&str; 3] = [
    "assets/meshes/rock_boulder_a.obj",
    "assets/meshes/rock_boulder_b.obj",
    "assets/meshes/rock_beach.obj",
];

/// Radius of the rock every sweep below builds, in metres. A 3 m boulder is
/// the research pass's subject and the size at which `voxel_size` choices are
/// interesting — small enough to bake in a second, large enough to have scales.
const R: f32 = 1.5;

/// A rock of radius `r`, centred so the volume `bake` builds contains it.
fn rock(r: f32, amp: f32, freq: f32, octaves: u32, ridged: bool) -> Vec<VoxelOp> {
    let c = span(r, amp) * 0.5;
    vec![VoxelOp::Sphere {
        center: [c, c, c],
        radius: r,
        mode: CsgMode::Union,
        displace: Some(Displace { amplitude: amp, frequency: freq, octaves, seed: 0xB0D1E, ridged }),
        elongate: [0.0; 3],
    }]
}

/// The R = 1.5 rock every sweep below builds, in a volume of fixed size so
/// that changing one parameter changes exactly one thing.
fn displaced_sphere(amp: f32, freq: f32, octaves: u32, ridged: bool) -> Vec<VoxelOp> {
    vec![VoxelOp::Sphere {
        center: [2.3, 2.3, 2.3],
        radius: R,
        mode: CsgMode::Union,
        displace: Some(Displace { amplitude: amp, frequency: freq, octaves, seed: 0xB0D1E, ridged }),
        elongate: [0.0; 3],
    }]
}

/// Metres the volume must cover: the displaced sphere plus room for the
/// displacement to reach outward, plus a little air so nothing clips the wall.
fn span(r: f32, amp: f32) -> f32 {
    2.0 * (r + amp) + 0.6 * r
}

/// The blob: a pile of primitives with a hard `min`, which is what a rock looks
/// like when you have no displacement and try to fake detail with op count.
fn blob(n_sphere: usize, n_cap: usize, n_cut: usize) -> Vec<VoxelOp> {
    let mut rng = loom_terrain::noise::Rng::new(0xB0D1E);
    let c = [2.0_f32, 2.0, 2.0];
    let mut ops = Vec::new();
    let jitter = |s: f32, rng: &mut loom_terrain::noise::Rng| {
        [
            (rng.next_f32() - 0.5) * s + c[0],
            (rng.next_f32() - 0.5) * s + c[1],
            (rng.next_f32() - 0.5) * s + c[2],
        ]
    };
    for _ in 0..n_sphere {
        ops.push(VoxelOp::Sphere {
            center: jitter(R * 0.9, &mut rng),
            radius: R * (0.45 + 0.35 * rng.next_f32()),
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        });
    }
    for _ in 0..n_cap {
        let a = jitter(R * 0.9, &mut rng);
        let b = jitter(R * 0.9, &mut rng);
        ops.push(VoxelOp::Capsule {
            a,
            b,
            radius: R * (0.15 + 0.15 * rng.next_f32()),
            mode: CsgMode::Union,
            displace: None,
        });
    }
    for _ in 0..n_cut {
        ops.push(VoxelOp::Sphere {
            center: jitter(R * 1.6, &mut rng),
            radius: R * (0.2 + 0.25 * rng.next_f32()),
            mode: CsgMode::Subtract,
            displace: None,
            elongate: [0.0; 3],
        });
    }
    ops
}

fn bake(ops: &[VoxelOp], voxel_size: f32) -> (ShapeStats, f64) {
    bake_span(ops, voxel_size, span(R, 0.5 * R))
}

/// The recipe this example exists to produce, as a function of radius alone.
const AMP_RATIO: f32 = 0.20;
const FREQ_RATIO: f32 = 0.6;

/// `o*`: the last octave the grid can resolve. See the module docs.
fn octave_cap(r: f32, voxel_size: f32) -> u32 {
    octaves_by_wavelength(FREQ_RATIO / r, voxel_size)
        .min(octaves_by_amplitude(AMP_RATIO * r, voxel_size))
}

fn bake_span(ops: &[VoxelOp], voxel_size: f32, span: f32) -> (ShapeStats, f64) {
    let n = ((span / (32.0 * voxel_size)).ceil() as usize).max(1);
    let mut v = Volume::new([n, n, n], voxel_size);
    let t0 = std::time::Instant::now();
    v.bake(ops);
    let mesh = mesh_volume(&v, &SurfaceNets);
    (stats(&mesh), t0.elapsed().as_secs_f64() * 1000.0)
}

fn row(label: &str, s: &ShapeStats, ms: f64) {
    println!(
        "{label:<34} {:>6.1}%  {:>5.2}  {:>6.2}  {:>7}  {:>8.1}  {:>5.1}%",
        s.concave * 100.0,
        s.spread,
        s.p50,
        s.triangles,
        ms,
        100.0 * s.boundary as f32 / s.vertices.max(1) as f32,
    );
}

fn header(what: &str) {
    println!("\n{what}");
    println!(
        "{:<34} {:>7}  {:>5}  {:>6}  {:>7}  {:>8}  {:>6}",
        "", "concave", "sprd", "p50", "tris", "ms", "open"
    );
}

/// Octaves whose *wavelength* survives the grid: `1/(f·2^k) >= 2·voxel_size`.
/// Below that an octave is finer than the sampling and aliases the mesher.
fn octaves_by_wavelength(freq: f32, voxel_size: f32) -> u32 {
    (1.0 / (2.0 * freq * voxel_size)).log2().floor().max(0.0) as u32 + 1
}

/// Octaves whose *amplitude* survives the grid: `A/2^k >= voxel_size/2`. An
/// octave that moves the surface less than half a voxel cannot place a vertex
/// anywhere the octave above it did not already.
fn octaves_by_amplitude(amp: f32, voxel_size: f32) -> u32 {
    (2.0 * amp / voxel_size).log2().floor().max(0.0) as u32 + 1
}

fn main() {
    println!(
        "RULE: concave-area fraction is resolution-dependent. Only ever compare rows\n\
         at equal voxel_size. This is report-only and must never become a gate."
    );

    let voxel = 0.03_f32;
    let (amp, freq) = (0.25 * R, 0.6 / R);

    header(&format!("references (baked rows at voxel_size {voxel})"));
    for target in TARGETS {
        match loom_asset::mesh::import_obj(std::path::Path::new(target)) {
            Ok(m) => {
                let t0 = std::time::Instant::now();
                let s = stats(&m);
                row(
                    &format!("scan: {}", target.rsplit('/').next().unwrap_or(target)),
                    &s,
                    t0.elapsed().as_secs_f64() * 1000.0,
                );
            }
            Err(e) => println!("{target}: {e} (run from the repo root)"),
        }
    }
    for (label, ops) in [
        ("plain sphere, no displacement", displaced_sphere(0.0, freq, 1, true)),
        ("49-op union only, hard min", blob(49, 0, 0)),
        ("49-op union + capsules + cuts", blob(27, 10, 12)),
        ("displaced sphere, fbm", displaced_sphere(amp, freq, 5, false)),
        ("displaced sphere, ridged", displaced_sphere(amp, freq, 5, true)),
    ] {
        let (s, ms) = bake(&ops, voxel);
        row(label, &s, ms);
    }

    // **The cap formula is tested at two resolutions, not fitted at one.** A
    // rule read off a single curve is a coincidence; the claim is that the knee
    // moves with `voxel_size` exactly where the arithmetic says it will.
    for v in [voxel, 0.06] {
        header(&format!("octaves, A = {amp} f = {freq:.2} ridged, voxel_size {v}"));
        println!(
            "  predicted cap: {} by wavelength, {} by amplitude",
            octaves_by_wavelength(freq, v),
            octaves_by_amplitude(amp, v)
        );
        for o in [1_u32, 2, 3, 4, 5, 6, 7, 8] {
            let (s, ms) = bake(&displaced_sphere(amp, freq, o, true), v);
            row(&format!("octaves {o}"), &s, ms);
        }
    }

    header(&format!("amplitude, f = {freq:.2} x5 ridged, voxel_size {voxel}"));
    for k in [0.05_f32, 0.10, 0.15, 0.20, 0.25, 0.35, 0.50] {
        let (s, ms) = bake(&displaced_sphere(k * R, freq, 5, true), voxel);
        row(&format!("A = {k:.2}R = {:.3} m", k * R), &s, ms);
    }

    header(&format!("frequency, A = {amp} x5 ridged, voxel_size {voxel}"));
    for k in [0.2_f32, 0.4, 0.6, 0.9, 1.3, 2.0] {
        let (s, ms) = bake(&displaced_sphere(amp, k / R, 5, true), voxel);
        row(&format!("f = {k:.2}/R = {:.2}", k / R), &s, ms);
    }

    // **The recipe is stated as ratios of R, so it owes a scale check.** Each
    // row is a different rock at a different voxel size chosen to give the same
    // `o*`; if the ratios are right, all three land on the same number.
    header("THE RECIPE: A = 0.20R, f = 0.6/R, ridged, o* octaves");
    for (r, v) in [(0.4_f32, 0.008_f32), (1.5, 0.03), (6.0, 0.12)] {
        let (a, f) = (AMP_RATIO * r, FREQ_RATIO / r);
        let o = octave_cap(r, v);
        let (s, ms) = bake_span(&rock(r, a, f, o, true), v, span(r, a));
        row(&format!("R = {r} m, voxel {v}, o* = {o}"), &s, ms);
    }

    header("voxel_size — THE ROWS BELOW ARE NOT COMPARABLE TO EACH OTHER");
    for v in [0.12_f32, 0.06, 0.03, 0.02] {
        let (s, ms) = bake(&displaced_sphere(amp, freq, 5, true), v);
        row(
            &format!(
                "voxel {v:.2} (cap {} / {})",
                octaves_by_wavelength(freq, v),
                octaves_by_amplitude(amp, v)
            ),
            &s,
            ms,
        );
    }
}
