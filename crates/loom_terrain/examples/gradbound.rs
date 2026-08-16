//! Measure the largest gradient `value3` and `value` actually reach.
//!
//! The number matters because `loom_voxel::Displace::gradient_bound` and
//! `VoxelOp::lipschitz` widen the bake's early-out by it, and **understating it
//! punches holes in the surface**. Run with:
//!
//!     cargo run -q --release -p loom_terrain --example gradbound

fn main() {
    let n = 61;
    let mut max3: f32 = 0.0;
    let mut max2: f32 = 0.0;
    // Inside one lattice cell, where the interpolant's derivative peaks, over
    // several seeds so a single unlucky corner set cannot flatter the result.
    for seed in 0..24_u64 {
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    let (x, y, z) = (
                        i as f32 / (n - 1) as f32,
                        j as f32 / (n - 1) as f32,
                        k as f32 / (n - 1) as f32,
                    );
                    let h = 1e-3;
                    let g3 = [
                        (loom_terrain::noise::value3(x + h, y, z, seed)
                            - loom_terrain::noise::value3(x - h, y, z, seed))
                            / (2.0 * h),
                        (loom_terrain::noise::value3(x, y + h, z, seed)
                            - loom_terrain::noise::value3(x, y - h, z, seed))
                            / (2.0 * h),
                        (loom_terrain::noise::value3(x, y, z + h, seed)
                            - loom_terrain::noise::value3(x, y, z - h, seed))
                            / (2.0 * h),
                    ];
                    max3 = max3.max((g3[0] * g3[0] + g3[1] * g3[1] + g3[2] * g3[2]).sqrt());

                    let g2 = [
                        (loom_terrain::noise::value(x + h, y, seed)
                            - loom_terrain::noise::value(x - h, y, seed))
                            / (2.0 * h),
                        (loom_terrain::noise::value(x, y + h, seed)
                            - loom_terrain::noise::value(x, y - h, seed))
                            / (2.0 * h),
                    ];
                    max2 = max2.max((g2[0] * g2[0] + g2[1] * g2[1]).sqrt());
                }
            }
        }
    }
    println!("max |grad value3| = {max3:.3}   (Displace::gradient_bound uses 3.0)");
    println!("max |grad value|  = {max2:.3}   (Heightfield lipschitz uses 3.0)");
    println!("analytic per-axis bound for smootherstep over a [-1,1] range = 3.75");
}
