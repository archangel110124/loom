//! Erosion. The step that makes terrain look real.
//!
//! fBm variants look iteratively more convincing and still look nothing like
//! real terrain, because real fractal landscapes are shaped by **erosion** —
//! principally water moving material downhill. Which is why this is not
//! optional.

use crate::Heightmap;
use crate::noise::Rng;

/// Particle (droplet) hydraulic erosion.
///
/// Spawn particles at random positions; each follows the slope downhill,
/// picking up material where it is steep and fast and dropping it where it
/// flattens out. Produces gullies, sediment-filled valleys, and alluvial fans.
///
/// **Cost scales with droplet count, not map size**, because only the paths
/// actually walked get touched. That is what makes it the right default.
///
/// Its honest limitation: droplets do not interact, so this erodes but is not
/// a fluid — it cannot form lakes or true rivers. Grid-based shallow-water is
/// the answer when those are wanted, and it has more parameters to get wrong.
pub fn hydraulic(map: &mut Heightmap, droplets: u32, erode_rate: f32, deposit_rate: f32, rng: &mut Rng) {
    const MAX_STEPS: u32 = 64;
    const INERTIA: f32 = 0.05;
    const CAPACITY: f32 = 4.0;
    const EVAPORATION: f32 = 0.02;
    const MIN_SLOPE: f32 = 0.01;

    #[allow(clippy::cast_precision_loss)]
    let (w, h) = (map.width as f32 - 2.0, map.height as f32 - 2.0);

    for _ in 0..droplets {
        let mut px = rng.next_f32() * w + 1.0;
        let mut py = rng.next_f32() * h + 1.0;
        let (mut dx, mut dy) = (0.0_f32, 0.0_f32);
        let mut speed = 1.0_f32;
        let mut water = 1.0_f32;
        let mut sediment = 0.0_f32;

        for _ in 0..MAX_STEPS {
            // Gradient by central difference at the droplet's continuous
            // position — bilinear, because the droplet is between samples.
            let gx = map.sample(px + 1.0, py) - map.sample(px - 1.0, py);
            let gy = map.sample(px, py + 1.0) - map.sample(px, py - 1.0);

            // Inertia carries momentum across small bumps, which is what stops
            // droplets from getting stuck oscillating in one pixel.
            dx = dx * INERTIA - gx * (1.0 - INERTIA);
            dy = dy * INERTIA - gy * (1.0 - INERTIA);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                break;
            }
            dx /= len;
            dy /= len;

            let (nx, ny) = (px + dx, py + dy);
            if nx < 1.0 || ny < 1.0 || nx > w || ny > h {
                break;
            }

            let old = map.sample(px, py);
            let new = map.sample(nx, ny);
            let drop = old - new;

            // Carrying capacity rises with slope, speed, and water. When the
            // droplet slows or flattens out, capacity falls and it deposits —
            // which is what builds alluvial fans where a valley opens up.
            let capacity = (drop.max(MIN_SLOPE) * speed * water * CAPACITY).max(0.0);

            if sediment > capacity || drop < 0.0 {
                // Uphill: never deposit more than the step, or the droplet
                // fills the hole it is climbing out of and terraces the map.
                let amount = if drop < 0.0 {
                    (sediment).min(-drop)
                } else {
                    (sediment - capacity) * deposit_rate
                };
                sediment -= amount;
                deposit(map, px, py, amount);
            } else {
                let amount = ((capacity - sediment) * erode_rate).min(drop.max(0.0));
                sediment += amount;
                deposit(map, px, py, -amount);
            }

            speed = (speed * speed + drop.abs() * 9.81).max(0.0).sqrt();
            water *= 1.0 - EVAPORATION;
            px = nx;
            py = ny;
            if water < 0.01 {
                break;
            }
        }
    }
}

/// Spread a change over the four surrounding samples, bilinearly.
///
/// Depositing into one pixel makes single-pixel spikes; spreading is what
/// keeps eroded terrain smooth.
fn deposit(map: &mut Heightmap, x: f32, y: f32, amount: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x0, y0) = (x.floor().max(0.0) as usize, y.floor().max(0.0) as usize);
    let (x1, y1) = ((x0 + 1).min(map.width - 1), (y0 + 1).min(map.height - 1));
    #[allow(clippy::cast_precision_loss)]
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);

    for (px, py, weight) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)),
        (x0, y1, (1.0 - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        map.set(px, py, map.get(px, py) + amount * weight);
    }
}

/// Thermal erosion: material steeper than the talus angle slides downhill.
///
/// Two parameters, trivial to implement, and it fixes the implausibly steep
/// slopes that noise produces. Both GPU erosion references pair it with
/// hydraulic for exactly that reason.
pub fn thermal(map: &mut Heightmap, talus_degrees: f32, iterations: u32) {
    let [mx, _] = map.metres_per_pixel();
    // Maximum height difference a neighbour may hold before material slides.
    let threshold = talus_degrees.to_radians().tan() * mx;

    for _ in 0..iterations {
        // **In place, not into a delta buffer.** Computing every cell's move
        // from the un-updated map lets one cell receive from several
        // neighbours in the same pass and spike above all of them. Measured:
        // mean slope fell 17.2° -> 16.4° while the MAX rose 55° -> 75° and the
        // count of cells over the talus angle went UP. Reading the map as it
        // is being updated is what keeps deposits from stacking.
        // **Every cell, borders included.** Skipping the border made it a
        // SINK: edge cells received material but were never processed as
        // sources, so everything flowed outward and piled against the frame,
        // and the slope beside it grew. That is why max slope kept rising —
        // 55° to 77° over 190 iterations — while mean slope fell.
        for y in 0..map.height {
            for x in 0..map.width {
                let here = map.get(x, y);

                // In-bounds neighbours only. A clamped neighbour would be the
                // cell itself, i.e. a zero difference, which is harmless but
                // wasteful.
                let mut neighbours = [(0usize, 0usize); 4];
                let mut count = 0;
                for (dx, dy) in [(-1_isize, 0_isize), (1, 0), (0, -1), (0, 1)] {
                    let (Some(nx), Some(ny)) =
                        (x.checked_add_signed(dx), y.checked_add_signed(dy))
                    else {
                        continue;
                    };
                    if nx < map.width && ny < map.height {
                        neighbours[count] = (nx, ny);
                        count += 1;
                    }
                }
                let neighbours = &neighbours[..count];
                let mut total = 0.0_f32;
                for (nx, ny) in neighbours {
                    let diff = here - map.get(*nx, *ny);
                    if diff > threshold {
                        total += diff - threshold;
                    }
                }
                if total <= 0.0 {
                    continue;
                }

                // Damped: move a quarter of the excess per pass. Moving all of
                // it overshoots past level and inverts the slope.
                let budget = total * 0.25;
                for (nx, ny) in neighbours {
                    let diff = here - map.get(*nx, *ny);
                    if diff <= threshold {
                        continue;
                    }
                    let share = (diff - threshold) / total;
                    let amount = budget * share;
                    map.set(*nx, *ny, map.get(*nx, *ny) + amount);
                    map.set(x, y, map.get(x, y) - amount);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Recipe;

    fn rough() -> Heightmap {
        Recipe::from_toml(
            "size = [64, 64]\nworld_scale = [256.0, 256.0]\n\
             height_range = [0.0, 120.0]\nseed = 3\n\n\
             [[layer]]\nkind = \"ridged\"\namplitude = 100.0\noctaves = 6\n",
        )
        .unwrap()
        .bake()
    }

    #[test]
    fn hydraulic_erosion_is_deterministic_for_a_seed() {
        let run = || {
            let mut m = rough();
            hydraulic(&mut m, 500, 0.3, 0.3, &mut Rng::new(11));
            m.data
        };

        assert_eq!(run(), run(), "erosion must replay identically");
    }

    /// Erosion moves material; it must not create or destroy much of it. A
    /// large drift means the deposit/erode accounting is wrong.
    #[test]
    fn erosion_roughly_conserves_material() {
        let mut map = rough();
        let before: f32 = map.data.iter().sum();

        hydraulic(&mut map, 2000, 0.3, 0.3, &mut Rng::new(5));

        let after: f32 = map.data.iter().sum();
        let drift = (after - before).abs() / before.abs().max(1.0);
        assert!(drift < 0.25, "material drifted {:.1}%", drift * 100.0);
    }

    #[test]
    fn thermal_erosion_reduces_the_steepest_slope() {
        let mut map = rough();
        let steepest = |m: &Heightmap| {
            (1..m.height - 1)
                .flat_map(|y| (1..m.width - 1).map(move |x| (x, y)))
                .map(|(x, y)| m.slope_degrees(x, y))
                .fold(0.0_f32, f32::max)
        };
        let before = steepest(&map);

        thermal(&mut map, 25.0, 50);

        assert!(steepest(&map) < before, "should soften from {before}°");
    }

    /// A droplet that walks off the edge must stop, not wrap or panic.
    #[test]
    fn erosion_does_not_escape_the_map() {
        let mut map = rough();
        hydraulic(&mut map, 3000, 0.5, 0.5, &mut Rng::new(1));

        assert!(map.data.iter().all(|v| v.is_finite()), "NaN or inf appeared");
    }
}
