//! Does the understated ridged bound let the early-out skip a chunk that holds
//! a surface?
//!
//! Tests the early-out's own predicate directly instead of baking a volume: the
//! bake fills a chunk uniformly when `|d(centre)| > lipschitz * chunk_radius`.
//! That is wrong exactly when some voxel inside the chunk has the opposite
//! sign. So walk candidate chunk centres in the shell where the predicate is
//! marginal, keep the ones the early-out would skip, and sample the chunk.
use loom_voxel::{CsgMode, Displace, VoxelOp, CHUNK};

#[allow(clippy::cast_precision_loss)]
fn probe(name: &str, op: &VoxelOp, voxel_size: f32) -> usize {
    let h = CHUNK as f32 * 0.5 * voxel_size;
    let radius = h * 1.732_050_8;
    let l = op.lipschitz();
    let reach = l * radius;
    let mut wrong = 0_usize;
    let mut skipped = 0_usize;
    // Random chunk centres in the shell where the predicate is marginal:
    // |d| just past the reach is where a wrongly-skipped chunk must live.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut rand = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 40) as f32) / 8_388_608.0 - 1.0
    };
    for _ in 0..200_000 {
        let dir = {
            let v = [rand(), rand(), rand()];
            let n = v[2].mul_add(v[2], v[0].mul_add(v[0], v[1] * v[1])).sqrt().max(1e-6);
            [v[0] / n, v[1] / n, v[2] / n]
        };
        // Radial band around the surface covering exactly the marginal shell.
        let r0 = op_radius(op);
        let t = rand().mul_add(0.5, 0.5);
        let s = (r0 - 3.0 * reach).mul_add(1.0 - t, (r0 + 3.0 * reach) * t);
        let c = [dir[0] * s, dir[1] * s, dir[2] * s];
        let d = op.distance(c);
        if d.abs() <= reach {
            continue;
        }
        skipped += 1;
        let m = 6;
        for a in 0..=m {
            for b in 0..=m {
                for e in 0..=m {
                    let p = [
                        (a as f32 / m as f32).mul_add(2.0 * h, c[0] - h),
                        (b as f32 / m as f32).mul_add(2.0 * h, c[1] - h),
                        (e as f32 / m as f32).mul_add(2.0 * h, c[2] - h),
                    ];
                    if (op.distance(p) < 0.0) != (d < 0.0) {
                        wrong += 1;
                    }
                }
            }
        }
    }
    println!("{name:<44} L = {l:>6.3}  skipped {skipped:>6}  wrong {wrong}");
    wrong
}

fn op_radius(op: &VoxelOp) -> f32 {
    match op {
        VoxelOp::Sphere { radius, .. } => *radius,
        _ => 1.0,
    }
}

fn rock(r: f32, amp_r: f32, freq_r: f32, oct: u32, seed: u64) -> VoxelOp {
    VoxelOp::Sphere {
        center: [0.0; 3],
        radius: r,
        mode: CsgMode::Union,
        displace: Some(Displace {
            amplitude: amp_r * r,
            frequency: freq_r / r,
            octaves: oct,
            seed,
            ridged: true,
        }),
        elongate: [0.0; 3],
    }
}

fn main() {
    let r = 1.5_f32;
    let mut total = 0;
    for &oct in &[1_u32, 2, 3, 5] {
        for &div in &[20.0_f32, 60.0, 200.0] {
            total += probe(
                &format!("recipe R=1.5 o={oct} vs=R/{div}"),
                &rock(r, 0.20, 0.6, oct, 9),
                r / div,
            );
        }
    }
    for &(amp, freq, oct) in &[(0.5_f32, 1.0_f32, 2_u32), (0.35, 3.0, 1), (0.6, 1.5, 3)] {
        for &div in &[60.0_f32, 200.0] {
            total += probe(
                &format!("A={amp}R f={freq}/R o={oct} vs=R/{div}"),
                &rock(r, amp, freq, oct, 5),
                r / div,
            );
        }
    }
    println!("\ntotal wrongly-skipped voxels: {total}");
}
