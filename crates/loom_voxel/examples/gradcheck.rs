//! Measure max |grad Displace::at| against Displace::gradient_bound().
use loom_voxel::Displace;

fn grad(d: &Displace, p: [f32; 3], h: f32) -> f32 {
    let mut g = [0.0f32; 3];
    for a in 0..3 {
        let mut lo = p;
        let mut hi = p;
        lo[a] -= h;
        hi[a] += h;
        g[a] = (d.at(hi) - d.at(lo)) / (2.0 * h);
    }
    g[2].mul_add(g[2], g[0].mul_add(g[0], g[1] * g[1])).sqrt()
}

/// Worst gradient over a dense grid, refined by hill-climbing each top hit.
fn worst_grad(d: &Displace, span: f32, n: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f32;
    let cell = span / nf;
    let h = 1e-3 * span;
    let mut worst = 0.0f32;
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                #[allow(clippy::cast_precision_loss)]
                let mut p = [
                    (i as f32 / nf - 0.5) * span,
                    (j as f32 / nf - 0.5) * span,
                    (k as f32 / nf - 0.5) * span,
                ];
                let mut g = grad(d, p, h);
                if g < worst * 0.7 {
                    continue;
                }
                // Local refinement: coordinate descent toward a steeper point.
                let mut step = cell * 0.5;
                while step > h {
                    let mut moved = false;
                    for a in 0..3 {
                        for s in [-step, step] {
                            let mut q = p;
                            q[a] += s;
                            let gq = grad(d, q, h);
                            if gq > g {
                                g = gq;
                                p = q;
                                moved = true;
                            }
                        }
                    }
                    if !moved {
                        step *= 0.5;
                    }
                }
                worst = worst.max(g);
            }
        }
    }
    worst
}

fn sweep(name: &str, mk: impl Fn(u64) -> Displace, span: f32, n: usize) {
    let mut worst = 0.0f32;
    let mut bound = 0.0;
    for seed in 0..6_u64 {
        let d = mk(seed);
        bound = d.gradient_bound();
        worst = worst.max(worst_grad(&d, span, n));
    }
    println!(
        "{name:<32} bound {bound:>8.3}  measured {worst:>8.3}  ratio {:>6.2}  {}",
        worst / bound,
        if worst > bound { "VIOLATED" } else { "ok" }
    );
}

fn main() {
    let r = 1.5_f32;
    for oct in [1_u32, 2, 3, 4, 5, 6, 8] {
        sweep(
            &format!("ridged R=1.5 octaves={oct}"),
            |seed| Displace {
                amplitude: 0.20 * r,
                frequency: 0.6 / r,
                octaves: oct,
                seed,
                ridged: true,
            },
            6.0,
            40,
        );
    }
    for oct in [1_u32, 3, 5, 8] {
        sweep(
            &format!("fbm    R=1.5 octaves={oct}"),
            |seed| Displace {
                amplitude: 0.20 * r,
                frequency: 0.6 / r,
                octaves: oct,
                seed,
                ridged: false,
            },
            6.0,
            40,
        );
    }
}
