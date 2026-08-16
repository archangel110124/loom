//! Scratch measurement for the "convincing rock" research pass: what a 3 m
//! boulder costs to bake and mesh at each voxel size, for a 50-op recipe and a
//! 1-op one. Delete when the research note it feeds has landed.
#![allow(clippy::disallowed_methods, clippy::cast_precision_loss)]

use loom_voxel::{CsgMode, SurfaceNets, Volume, VoxelOp, mesh::mesh_volume};

fn blob(n_sphere: usize, n_cap: usize, n_cut: usize) -> Vec<VoxelOp> {
    // Deterministic jitter; the point is op COUNT and coverage, not the shape.
    let mut rng = loom_terrain::noise::Rng::new(0xB0D1E);
    let c = [2.0_f32, 2.0, 2.0];
    let r = 1.5_f32;
    let mut ops = Vec::new();
    let jitter = |s: f32, rng: &mut loom_terrain::noise::Rng| {
        [
            (rng.next_f32() - 0.5) * s + c[0],
            (rng.next_f32() - 0.5) * s + c[1],
            (rng.next_f32() - 0.5) * s + c[2],
        ]
    };
    for _ in 0..n_sphere {
        let center = jitter(r * 0.9, &mut rng);
        ops.push(VoxelOp::Sphere {
            center,
            radius: r * (0.45 + 0.35 * rng.next_f32()),
            mode: CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        });
    }
    for _ in 0..n_cap {
        let a = jitter(r * 0.9, &mut rng);
        let b = jitter(r * 0.9, &mut rng);
        ops.push(VoxelOp::Capsule {
            a,
            b,
            radius: r * (0.15 + 0.15 * rng.next_f32()),
            mode: CsgMode::Union,
            displace: None,
        });
    }
    for _ in 0..n_cut {
        let center = jitter(r * 1.6, &mut rng);
        ops.push(VoxelOp::Sphere {
            center,
            radius: r * (0.2 + 0.25 * rng.next_f32()),
            mode: CsgMode::Subtract,
            displace: None,
            elongate: [0.0; 3],
        });
    }
    ops
}

fn run(label: &str, ops: &[VoxelOp], voxel_size: f32) {
    let span = 4.0_f32; // metres the volume must cover
    let n = ((span / (32.0 * voxel_size)).ceil() as usize).max(1);
    let mut v = Volume::new([n, n, n], voxel_size);

    let t0 = std::time::Instant::now();
    v.bake(ops);
    let bake = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = std::time::Instant::now();
    let mesh = mesh_volume(&v, &SurfaceNets);
    let meshed = t1.elapsed().as_secs_f64() * 1000.0;

    let t2 = std::time::Instant::now();
    let touched = v.edit(&VoxelOp::Sphere {
        center: [2.0, 3.0, 2.0],
        radius: 0.4,
        mode: CsgMode::Subtract,
        displace: None,
        elongate: [0.0; 3],
    });
    let crater = t2.elapsed().as_secs_f64() * 1000.0;

    let (used, _) = v.memory();
    println!(
        "{label:<14} vs {voxel_size:>5.3}  chunks {n}^3={:<5} surf {:<5} bake {bake:>8.1} ms  mesh {meshed:>7.1} ms  tris {:>7}  mem {:>6.2} MB  crater {crater:>5.2} ms ({} chunks)",
        n * n * n,
        v.surface_chunks().len(),
        mesh.indices.len() / 3,
        used as f64 / 1048576.0,
        touched.len(),
    );
}

fn main() {
    let fifty = blob(27, 10, 12);
    let one = vec![VoxelOp::Sphere {
        center: [2.0, 2.0, 2.0],
        radius: 1.5,
        mode: CsgMode::Union,
        displace: None,
        elongate: [0.0; 3],
    }];
    let displaced = |octaves: u32| {
        vec![VoxelOp::Sphere {
            center: [2.0, 2.0, 2.0],
            radius: 1.2,
            mode: CsgMode::Union,
            displace: Some(loom_voxel::Displace {
                amplitude: 0.30,
                frequency: 0.7,
                octaves,
                seed: 0xB0CC,
                ridged: true,
            }),
            elongate: [0.0; 3],
        }]
    };
    println!("ops: {} / 1\n", fifty.len());
    for vs in [0.15_f32, 0.1, 0.06, 0.045, 0.03, 0.02] {
        run("50-op blob", &fifty, vs);
        run("1 sphere", &one, vs);
        run("displaced x4", &displaced(4), vs);
        run("displaced x7", &displaced(7), vs);
        println!();
    }
}
