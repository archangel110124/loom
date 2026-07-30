//! Terrain analysis — the agent's feedback channel for terrain.
//!
//! `render_preview` verifies placement and `run_scene` verifies behaviour.
//! Terrain needs its own channel, and it is mostly **not visual**: most terrain
//! mistakes are invisible in a render and obvious in a slope map.
//!
//! Two outputs matter more than the pretty one. A gorgeous mountain range with
//! 3% buildable ground is a failed level, and nothing in a hillshade reveals
//! that — `buildable_area_pct` and `largest_flat_region` do. And PNGs rather
//! than arrays, because an agent reads an image competently and a 2048×2048
//! float array not at all.

use crate::Heightmap;

/// What analysis found.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub height_min: f32,
    pub height_max: f32,
    pub height_mean: f32,
    pub slope_mean: f32,
    /// Fraction of the map steeper than 45°.
    pub slope_over_45_pct: f32,
    /// Fraction gentle enough to build on.
    pub buildable_pct: f32,
    /// Centre and area (in pixels) of the largest contiguous buildable region.
    pub largest_flat: Option<([usize; 2], usize)>,
}

/// Slope below which ground counts as buildable.
pub const BUILDABLE_SLOPE: f32 = 15.0;

/// Measure a heightmap.
#[must_use]
pub fn analyze(map: &Heightmap) -> Analysis {
    let (lo, hi) = map.range();
    #[allow(clippy::cast_precision_loss)]
    let count = map.data.len() as f32;
    let mean = map.data.iter().sum::<f32>() / count;

    let mut slope_sum = 0.0;
    let mut steep = 0usize;
    let mut buildable = vec![false; map.data.len()];

    for y in 0..map.height {
        for x in 0..map.width {
            let s = map.slope_degrees(x, y);
            slope_sum += s;
            if s > 45.0 {
                steep += 1;
            }
            if s <= BUILDABLE_SLOPE {
                buildable[y * map.width + x] = true;
            }
        }
    }

    let flat_count = buildable.iter().filter(|b| **b).count();
    #[allow(clippy::cast_precision_loss)]
    Analysis {
        height_min: lo,
        height_max: hi,
        height_mean: mean,
        slope_mean: slope_sum / count,
        slope_over_45_pct: steep as f32 / count * 100.0,
        buildable_pct: flat_count as f32 / count * 100.0,
        largest_flat: largest_region(&buildable, map.width, map.height),
    }
}

/// Largest contiguous buildable region, by flood fill.
///
/// Contiguity is the point: ten thousand scattered flat pixels are not
/// somewhere to put a fort.
fn largest_region(mask: &[bool], width: usize, height: usize) -> Option<([usize; 2], usize)> {
    let mut seen = vec![false; mask.len()];
    let mut best: Option<([usize; 2], usize)> = None;

    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        // Iterative, not recursive: a large flat map would blow the stack.
        let mut stack = vec![start];
        seen[start] = true;
        let (mut sx, mut sy, mut area) = (0usize, 0usize, 0usize);

        while let Some(index) = stack.pop() {
            let (x, y) = (index % width, index / width);
            sx += x;
            sy += y;
            area += 1;

            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx >= width || ny >= height {
                    continue;
                }
                let n = ny * width + nx;
                if mask[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }

        if best.is_none_or(|(_, b)| area > b) {
            best = Some(([sx / area, sy / area], area));
        }
    }
    best
}

/// Is there a route between two points no steeper than `max_slope`?
///
/// This closes the loop on the `Corridor` layer: generate, verify
/// traversability, adjust. That is an agent working on gameplay rather than on
/// aesthetics, which is the entire point of the channel.
#[must_use]
pub fn is_reachable(map: &Heightmap, from: [usize; 2], to: [usize; 2], max_slope: f32) -> bool {
    let passable = |x: usize, y: usize| map.slope_degrees(x, y) <= max_slope;
    if !passable(from[0], from[1]) || !passable(to[0], to[1]) {
        return false;
    }

    let mut seen = vec![false; map.data.len()];
    let mut queue = std::collections::VecDeque::from([from]);
    seen[from[1] * map.width + from[0]] = true;

    while let Some([x, y]) = queue.pop_front() {
        if [x, y] == to {
            return true;
        }
        for (nx, ny) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if nx >= map.width || ny >= map.height {
                continue;
            }
            let index = ny * map.width + nx;
            if !seen[index] && passable(nx, ny) {
                seen[index] = true;
                queue.push_back([nx, ny]);
            }
        }
    }
    false
}

/// Render an analysis map to a small PNG.
///
/// Small on purpose: a 256×256 slope map answers "is this terrain playable" in
/// one glance, and costs the agent almost nothing to look at.
///
/// # Errors
/// [`std::io::Error`] if the file cannot be written.
pub fn write_png(
    map: &Heightmap,
    kind: MapKind,
    path: &std::path::Path,
) -> Result<(), std::io::Error> {
    let (w, h) = (map.width, map.height);
    let mut pixels = Vec::with_capacity(w * h * 3);
    let (lo, hi) = map.range();
    let span = (hi - lo).max(1e-6);

    for y in 0..h {
        for x in 0..w {
            let rgb = match kind {
                MapKind::Height => {
                    let t = (map.get(x, y) - lo) / span;
                    let v = (t * 255.0) as u8;
                    [v, v, v]
                }
                MapKind::Slope => {
                    // Green flat → red steep. Buildable ground reads instantly.
                    let s = (map.slope_degrees(x, y) / 60.0).clamp(0.0, 1.0);
                    [(s * 255.0) as u8, ((1.0 - s) * 200.0) as u8, 40]
                }
                MapKind::Buildable => {
                    let ok = map.slope_degrees(x, y) <= BUILDABLE_SLOPE;
                    if ok { [80, 220, 120] } else { [30, 32, 40] }
                }
                MapKind::Hillshade => {
                    // Lambertian from a north-west sun, the convention every
                    // topographic map uses.
                    let [mx, my] = map.metres_per_pixel();
                    let dx = (map.get((x + 1).min(w - 1), y) - map.get(x.saturating_sub(1), y))
                        / (2.0 * mx);
                    let dy = (map.get(x, (y + 1).min(h - 1)) - map.get(x, y.saturating_sub(1)))
                        / (2.0 * my);
                    let len = (dx * dx + dy * dy + 1.0).sqrt();
                    let light = ((-dx * 0.6 - dy * 0.6 + 1.0) / len / 1.8).clamp(0.15, 1.0);
                    let v = (light * 235.0) as u8;
                    [v, v, v]
                }
            };
            pixels.extend_from_slice(&rgb);
        }
    }

    let file = std::fs::File::create(path)?;
    #[allow(clippy::cast_possible_truncation)]
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    writer
        .write_image_data(&pixels)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Which analysis map to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    Height,
    Slope,
    Buildable,
    Hillshade,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Recipe;

    fn baked(layers: &str) -> Heightmap {
        Recipe::from_toml(&format!(
            "size = [64, 64]\nworld_scale = [256.0, 256.0]\n\
             height_range = [0.0, 100.0]\nseed = 5\n\n{layers}"
        ))
        .unwrap()
        .bake()
    }

    #[test]
    fn a_flat_map_is_entirely_buildable() {
        let map = Heightmap::new(32, 32, [128.0, 128.0]);
        let a = analyze(&map);

        assert!((a.buildable_pct - 100.0).abs() < 0.01);
        assert_eq!(a.slope_over_45_pct, 0.0);
    }

    /// **The stat an agent actually iterates on.** A gorgeous mountain range
    /// with no buildable ground is a failed level, and a hillshade will not
    /// tell you that.
    ///
    /// Compared across *height range*, not across noise type. An earlier
    /// version asserted ridged is steeper than fBm and that is simply false —
    /// ridged has broad flat valleys between its ridges, and measured 54%
    /// buildable against fBm's 34%.
    #[test]
    fn taller_terrain_reports_less_buildable_ground() {
        let tall = Recipe::from_toml(
            "size = [64, 64]\nworld_scale = [256.0, 256.0]\n\
             height_range = [0.0, 400.0]\nseed = 5\n\n\
             [[layer]]\nkind = \"fbm\"\noctaves = 6\n",
        )
        .unwrap();
        let flat = Recipe {
            height_range: [0.0, 20.0],
            ..tall.clone()
        };

        let steep = analyze(&tall.bake());
        let gentle = analyze(&flat.bake());
        assert!(
            steep.buildable_pct < gentle.buildable_pct,
            "tall {:.1}% vs flat {:.1}%",
            steep.buildable_pct,
            gentle.buildable_pct
        );
    }

    /// Contiguity is the point: scattered flat pixels are not somewhere to put
    /// a fort.
    #[test]
    fn the_largest_flat_region_is_contiguous() {
        let map = baked(
            "[[layer]]\nkind = \"ridged\"\namplitude = 90.0\noctaves = 6\n\n\
             [[layer]]\nkind = \"flatten_disc\"\ncenter = [32.0, 32.0]\nradius = 14.0\n",
        );
        let a = analyze(&map);

        let ([cx, cy], area) = a.largest_flat.expect("a plateau was carved");
        assert!(area > 100, "plateau area was only {area} pixels");
        assert!(
            (cx as i32 - 32).abs() < 12 && (cy as i32 - 32).abs() < 12,
            "plateau centre {cx},{cy} should be near the flatten"
        );
    }

    /// Closes the loop on the `Corridor` layer: generate, verify, adjust.
    #[test]
    fn a_corridor_makes_a_route_reachable() {
        let without = baked("[[layer]]\nkind = \"ridged\"\namplitude = 120.0\noctaves = 7\n");
        let with = baked(
            "[[layer]]\nkind = \"ridged\"\namplitude = 120.0\noctaves = 7\n\n\
             [[layer]]\nkind = \"corridor\"\n\
             from = [4.0, 32.0]\nto = [59.0, 32.0]\nwidth = 4.0\nmax_slope = 12.0\n",
        );

        let a = is_reachable(&without, [4, 32], [59, 32], 15.0);
        let b = is_reachable(&with, [4, 32], [59, 32], 15.0);
        assert!(b, "the corridor must guarantee a route");
        assert!(!a || b, "corridor must not make things worse (was {a})");
    }

    #[test]
    fn every_map_kind_writes_a_png() {
        let map = baked("[[layer]]\nkind = \"fbm\"\namplitude = 40.0\n");
        let dir = std::env::temp_dir().join("loom_terrain_png");
        std::fs::create_dir_all(&dir).unwrap();

        for (kind, name) in [
            (MapKind::Height, "height"),
            (MapKind::Slope, "slope"),
            (MapKind::Buildable, "buildable"),
            (MapKind::Hillshade, "hillshade"),
        ] {
            let path = dir.join(format!("{name}.png"));
            write_png(&map, kind, &path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(std::fs::metadata(&path).unwrap().len() > 0);
        }
    }
}
