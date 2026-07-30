//! Measured voxel cost. Not a test — it prints numbers rather than asserting
//! them, because "how long does a 512³ bake take" is a question with a
//! machine-specific answer.
//!
//! `clippy.toml` disallows `Instant::now` because SIMULATION must not read the
//! clock (never-do #8). A benchmark is the one place reading it is the entire
//! point, so the allow is scoped here and nowhere else.
#![allow(clippy::disallowed_methods)]

fn main() {
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
