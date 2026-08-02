//! Measured voxel cost. Not a test — it prints numbers rather than asserting
//! them, because "how long does a 512³ bake take" is a question with a
//! machine-specific answer.
//!
//! `clippy.toml` disallows `Instant::now` because SIMULATION must not read the
//! clock (never-do #8). A benchmark is the one place reading it is the entire
//! point, so the allow is scoped here and nowhere else.
#![allow(clippy::disallowed_methods)]

fn main() {
    terrain();
    carved_sphere();
}

/// A world filled with ground, which is the shape that actually scales badly.
///
/// A sphere in a big volume is a small surface in a lot of empty space, so
/// almost every chunk is uniform and the early-out does the work. **Terrain is
/// the opposite**: its surface spans the entire XZ extent, so the number of
/// chunks holding surface grows with the area of the world rather than sitting
/// still. That is the number that decides whether a world is affordable.
///
/// Frequency is held fixed in world units across the scales, so a bigger
/// volume means *more terrain* rather than the same terrain stretched. That is
/// the scaling question worth asking — the other one flatters the engine.
fn terrain() {
    use loom_voxel::{CsgMode, SurfaceNets, VoxelOp, Volume, mesh::mesh_volume};
    let t = |label: &str, f: &mut dyn FnMut()| {
        let start = std::time::Instant::now();
        f();
        println!("  {label:<34} {:>8.1} ms", start.elapsed().as_secs_f64() * 1000.0);
    };

    for dims in [[4usize, 4, 4], [8, 8, 8], [16, 8, 16], [16, 16, 16]] {
        let voxel_size = 0.25;
        let res = [dims[0] * 32, dims[1] * 32, dims[2] * 32];
        let voxels = res.iter().map(|r| *r as u64).product::<u64>();
        let world: Vec<f32> = res.iter().map(|r| *r as f32 * voxel_size).collect();
        println!(
            "\n=== terrain {}x{}x{} voxels ({:.1}M) over {:.0}x{:.0}x{:.0} m ===",
            res[0], res[1], res[2], voxels as f64 / 1e6, world[0], world[1], world[2]
        );

        let mut v = Volume::new(dims, voxel_size);
        let ops = vec![VoxelOp::Heightfield {
            base: world[1] * 0.5,
            amplitude: 6.0,
            frequency: 0.03,
            octaves: 4,
            seed: 0xB1A57,
            mode: CsgMode::Union,
        }];
        t("bake", &mut || v.bake(&ops));

        let (used, dense) = v.memory();
        let surface = v.surface_chunks().len();
        let total = dims[0] * dims[1] * dims[2];
        println!(
            "  {:<34} {:>8.2} MB  ({:.1}% of dense {:.1} MB)",
            "resident field",
            used as f64 / 1048576.0,
            used as f64 / dense as f64 * 100.0,
            dense as f64 / 1048576.0
        );
        println!(
            "  {:<34} {:>8} of {total}  ({:.1}%)",
            "chunks holding a surface",
            surface,
            surface as f64 / total as f64 * 100.0
        );

        let mut tris = 0;
        t("mesh", &mut || {
            tris = mesh_volume(&v, &SurfaceNets).indices.len() / 3;
        });
        println!("  {:<34} {:>8}", "triangles", tris);

        let mut touched = 0;
        t("runtime edit (crater on a hillside)", &mut || {
            touched = v
                .edit(&VoxelOp::Sphere {
                    center: [world[0] * 0.5, world[1] * 0.5, world[2] * 0.5],
                    radius: 3.0,
                    mode: CsgMode::Subtract,
                })
                .len();
        });
        println!("  {:<34} {:>8} chunks", "crater cost", touched);

        // **The odd one out, and the reason it is measured here.** Everything
        // above costs surface; this costs volume. The physics collider walks
        // every voxel in the world and returns one `[i32; 3]` per solid cell,
        // so it grows with the cube of the extent while the render path grows
        // with the square. Skipped past 16.8M voxels because the returned
        // vector alone would run to hundreds of megabytes — which is the
        // finding, not an omission.
        if voxels <= 20_000_000 {
            let mut cells = 0;
            t("physics collider (solid_cells)", &mut || {
                cells = v.solid_cells().len();
            });
            println!(
                "  {:<34} {:>8} cells, {:.1} MB",
                "collider size",
                cells,
                cells as f64 * 12.0 / 1048576.0
            );
        } else {
            println!("  {:<34} {:>8}", "physics collider (solid_cells)", "skipped");
        }
    }
}

fn carved_sphere() {
    use loom_voxel::{CsgMode, SurfaceNets, VoxelOp, Volume, mesh::mesh_volume};
    let t = |label: &str, f: &mut dyn FnMut()| {
        let start = std::time::Instant::now();
        f();
        println!("  {label:<34} {:>8.1} ms", start.elapsed().as_secs_f64() * 1000.0);
    };

    for (dims, label) in [([8usize,8,8], "256^3"), ([16,16,16], "512^3")] {
        let res = dims[0]*32;
        println!("\n=== {label} volume ({res}^3 = {} voxels) ===", (res as u64).pow(3));
        let mut v = Volume::new(dims, 0.25);
        let ops = vec![
            VoxelOp::Sphere { center: [res as f32*0.125, res as f32*0.125, res as f32*0.125], radius: res as f32*0.08, mode: CsgMode::Union },
            VoxelOp::Capsule { a: [0.0, res as f32*0.125, res as f32*0.125], b: [res as f32*0.25, res as f32*0.125, res as f32*0.125], radius: res as f32*0.02, mode: CsgMode::Subtract },
        ];
        t("bake (sphere + carved tunnel)", &mut || v.bake(&ops));
        let (used, dense) = v.memory();
        println!("  {:<34} {:>8.2} MB  ({:.1}% of dense {:.1} MB)", "resident field",
                 used as f64/1048576.0, used as f64/dense as f64*100.0, dense as f64/1048576.0);
        println!("  {:<34} {:>8} of {}", "chunks holding a surface", v.surface_chunks().len(), dims[0]*dims[1]*dims[2]);
        let mut tris = 0;
        t("mesh (surface chunks only)", &mut || { tris = mesh_volume(&v, &SurfaceNets).indices.len()/3; });
        println!("  {:<34} {:>8}", "triangles", tris);
        let mut touched = 0;
        t("runtime edit (blast a crater)", &mut || {
            touched = v.edit(&VoxelOp::Sphere { center: [res as f32*0.15, res as f32*0.15, res as f32*0.125], radius: res as f32*0.02, mode: CsgMode::Subtract }).len();
        });
        let (after, _) = v.memory();
        println!("  {:<34} {:>8} chunks, +{:.2} MB", "crater cost", touched, (after-used) as f64/1048576.0);
    }
}
