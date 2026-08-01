//! AI-authored terrain: recipe in, heightmap out.
//!
//! # The one principle everything follows from
//!
//! **The heightmap is an output, never an input.** The agent never writes
//! pixels, never uploads an image, never edits a height array. It writes a
//! *recipe* — text, diffable, reviewable, tiny — and the heightmap is a
//! derived, cached artifact.
//!
//! ```text
//! recipe (.toml)      authored — text, diffable, ~40 lines
//!    ↓ evaluate + bake
//! heightmap           derived, cached by content hash, not in version control
//!    ↓ sdf = y - h(x,z)
//! SDF voxel field     destructible, supports caves the heightmap cannot express
//! ```
//!
//! # A layer stack, not a node graph
//!
//! A DAG is more expressive and is where an agent makes most of its errors:
//! node ids, port names, edge wiring, accidental cycles — bookkeeping to get
//! right before any terrain logic matters. A linear stack is nearly as
//! expressive for terrain specifically, because terrain generation is
//! overwhelmingly "add this, then mask it by that", and it is dramatically
//! easier to write correctly.
//!
//! # Art direction sits between noise and erosion
//!
//! That order is deliberate: place the landforms gameplay needs, *then* let
//! simulated erosion make them look real. Doing it the other way sands off
//! exactly the features you placed on purpose.

pub mod analyze;
pub mod erosion;
pub mod noise;

pub use analyze::{Analysis, analyze};

use serde::{Deserialize, Serialize};

/// A 2D field of heights.
#[derive(Debug, Clone, PartialEq)]
pub struct Heightmap {
    pub width: usize,
    pub height: usize,
    /// Row-major, world units.
    pub data: Vec<f32>,
    /// World units covered per axis, for slope to mean anything.
    pub world_scale: [f32; 2],
}

impl Heightmap {
    #[must_use]
    pub fn new(width: usize, height: usize, world_scale: [f32; 2]) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
            world_scale,
        }
    }

    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y.min(self.height - 1) * self.width + x.min(self.width - 1)]
    }

    pub fn set(&mut self, x: usize, y: usize, value: f32) {
        let w = self.width;
        self.data[y.min(self.height - 1) * w + x.min(w - 1)] = value;
    }

    /// Bilinear sample in pixel space — particles move continuously over a
    /// discrete grid, so nearest-neighbour would make erosion stair-step.
    #[must_use]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        // Clamped into the grid *before* the cast. A saturating float cast
        // yields usize::MAX for a huge coordinate, and `x0 + 1` then overflows
        // — a debug panic, and a wrap to 0 in release, which silently samples
        // the wrong corner of the map.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x0, y0) = (
            (x.floor().max(0.0) as usize).min(self.width.saturating_sub(1)),
            (y.floor().max(0.0) as usize).min(self.height.saturating_sub(1)),
        );
        let (x1, y1) = (
            (x0 + 1).min(self.width.saturating_sub(1)),
            (y0 + 1).min(self.height.saturating_sub(1)),
        );
        #[allow(clippy::cast_precision_loss)]
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);

        let a = self.get(x0, y0);
        let b = self.get(x1, y0);
        let c = self.get(x0, y1);
        let d = self.get(x1, y1);
        (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
    }

    /// Metres per pixel on each axis.
    #[must_use]
    pub fn metres_per_pixel(&self) -> [f32; 2] {
        #[allow(clippy::cast_precision_loss)]
        [
            self.world_scale[0] / self.width as f32,
            self.world_scale[1] / self.height as f32,
        ]
    }

    /// Slope in degrees, from a central difference.
    ///
    /// Scaled by world size, or slope is measured in "height per pixel" and
    /// changes meaning when the resolution changes.
    #[must_use]
    pub fn slope_degrees(&self, x: usize, y: usize) -> f32 {
        let [mx, my] = self.metres_per_pixel();
        let dx = (self.get((x + 1).min(self.width - 1), y) - self.get(x.saturating_sub(1), y))
            / (2.0 * mx);
        let dy = (self.get(x, (y + 1).min(self.height - 1)) - self.get(x, y.saturating_sub(1)))
            / (2.0 * my);
        (dx * dx + dy * dy).sqrt().atan().to_degrees()
    }

    #[must_use]
    pub fn range(&self) -> (f32, f32) {
        self.data.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        })
    }

    /// The SDF bridge: signed distance to the surface along Y.
    ///
    /// Approximate but correct in sign; redistancing fixes the magnitude.
    /// Three lines, and it is the entire join between the 2D and 3D halves.
    #[must_use]
    pub fn sdf_at(&self, world: [f32; 3]) -> f32 {
        let [mx, my] = self.metres_per_pixel();
        world[1] - self.sample(world[0] / mx, world[2] / my)
    }
}

/// One step of the recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Layer {
    /// Fractal Brownian motion — the base landform.
    Fbm {
        #[serde(default = "one")]
        amplitude: f32,
        #[serde(default = "default_frequency")]
        frequency: f32,
        #[serde(default = "default_octaves")]
        octaves: u32,
        #[serde(default = "two")]
        lacunarity: f32,
        #[serde(default = "half")]
        gain: f32,
        /// Domain warping: the cheapest large quality win available. Distorts
        /// the *input* coordinates, producing terrain that reads as eroded —
        /// river valleys and steep peaks — for the cost of extra samples.
        #[serde(default)]
        warp: f32,
    },
    /// Ridged multifractal — mountain ridges. Takes the absolute value of each
    /// octave and inverts, and weights each octave by what came before, so
    /// high ground gets more detail and valleys stay smooth. That weighting is
    /// what makes commercial terrain look better than naive fBm.
    Ridged {
        #[serde(default = "one")]
        amplitude: f32,
        #[serde(default = "default_frequency")]
        frequency: f32,
        #[serde(default = "default_octaves")]
        octaves: u32,
        #[serde(default)]
        warp: f32,
    },
    /// Carve or raise along a path. Rivers, valleys, roads, ridgelines.
    SplineCarve {
        points: Vec<[f32; 2]>,
        width: f32,
        depth: f32,
    },
    /// Flatten a disc toward a height. **Buildable ground** — the layer an
    /// agent reaches for when a fort needs somewhere to sit.
    FlattenDisc {
        center: [f32; 2],
        radius: f32,
        #[serde(default = "default_blend")]
        blend: f32,
        /// `None` samples the existing height at the centre, which is almost
        /// always what "flatten here" means.
        #[serde(default)]
        target: Option<f32>,
    },
    /// Raise a peak with a smooth profile. Named landmarks.
    Peak {
        at: [f32; 2],
        height: f32,
        radius: f32,
    },
    /// Guarantee a traversable route between two points.
    ///
    /// **This does not exist in commercial terrain tools and should exist
    /// here.** "There must be a walkable route from spawn to the fort" is a
    /// gameplay constraint; having the generator guarantee it beats having the
    /// agent iterate blindly until a slope check passes.
    Corridor {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
        #[serde(default = "default_max_slope")]
        max_slope: f32,
    },
    /// Particle hydraulic erosion. Gullies, sediment-filled valleys, alluvial
    /// fans. Cost scales with droplet count, not map size.
    Hydraulic {
        #[serde(default = "default_droplets")]
        droplets: u32,
        #[serde(default = "default_erode")]
        erode_rate: f32,
        #[serde(default = "default_deposit")]
        deposit_rate: f32,
    },
    /// Thermal erosion: material above the talus angle slides. Two parameters,
    /// trivial, and it fixes the implausibly steep slopes noise produces.
    Thermal {
        #[serde(default = "default_talus")]
        talus_degrees: f32,
        #[serde(default = "default_thermal_iters")]
        iterations: u32,
    },
}

fn one() -> f32 {
    1.0
}
fn two() -> f32 {
    2.0
}
fn half() -> f32 {
    0.5
}
fn default_frequency() -> f32 {
    0.004
}
fn default_octaves() -> u32 {
    5
}
fn default_blend() -> f32 {
    0.5
}
fn default_max_slope() -> f32 {
    20.0
}
fn default_droplets() -> u32 {
    50_000
}
fn default_erode() -> f32 {
    0.3
}
fn default_deposit() -> f32 {
    0.3
}
fn default_talus() -> f32 {
    38.0
}
fn default_thermal_iters() -> u32 {
    30
}

/// A complete terrain recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Heightmap resolution.
    pub size: [usize; 2],
    /// World units the map covers.
    pub world_scale: [f32; 2],
    /// Output height range.
    pub height_range: [f32; 2],
    /// Seeds the RNG. Stored in the recipe, never thread-local, so a scene
    /// with random features replays identically (§A.3).
    pub seed: u64,
    #[serde(default)]
    pub layer: Vec<Layer>,
}

/// Why a recipe could not be used.
#[derive(Debug)]
pub enum TerrainError {
    Io(std::io::Error),
    Parse(String),
    Invalid(String),
}

impl std::fmt::Display for TerrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Parse(e) => write!(f, "invalid recipe: {e}"),
            Self::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TerrainError {}

impl Recipe {
    /// Parse a recipe.
    ///
    /// # Errors
    /// [`TerrainError`] if the TOML is malformed or the recipe is unusable.
    pub fn from_toml(text: &str) -> Result<Self, TerrainError> {
        let recipe: Self = toml::from_str(text).map_err(|e| TerrainError::Parse(e.to_string()))?;
        if recipe.size[0] == 0 || recipe.size[1] == 0 {
            return Err(TerrainError::Invalid("size must be non-zero".to_owned()));
        }
        // An agent asked for "a big terrain" will cheerfully request something
        // that does not fit in memory. Rejected with the number, so it can
        // scale the request rather than guess.
        const MAX: usize = 8192;
        if recipe.size[0] > MAX || recipe.size[1] > MAX {
            return Err(TerrainError::Invalid(format!(
                "size {:?} exceeds {MAX}x{MAX}; that is {} MB of f32",
                recipe.size,
                recipe.size[0] * recipe.size[1] * 4 / 1_048_576
            )));
        }
        if recipe.height_range[1] <= recipe.height_range[0] {
            return Err(TerrainError::Invalid(format!(
                "height_range {:?} must increase",
                recipe.height_range
            )));
        }
        Ok(recipe)
    }

    /// Load from disk.
    ///
    /// # Errors
    /// [`TerrainError`] if unreadable or invalid.
    pub fn load(path: &std::path::Path) -> Result<Self, TerrainError> {
        let text = std::fs::read_to_string(path).map_err(TerrainError::Io)?;
        Self::from_toml(&text)
    }

    /// Layer-order problems that are legal but almost certainly unintended.
    ///
    /// The terrain doc's ordering — noise, then art direction, then erosion —
    /// is right for *shape* layers: place the landforms gameplay needs, then
    /// let erosion make them look real. It is **wrong for constraint layers**.
    ///
    /// A [`Layer::Corridor`] is a guarantee, not a shape. Measured: a corridor
    /// carved before 30 iterations of thermal erosion stops being traversable,
    /// because erosion happily slides material back into it. The guarantee
    /// holds at the moment it is applied and not afterwards.
    ///
    /// An agent hits this exactly once and has no way to see why, so the
    /// recipe says so rather than leaving it to be discovered.
    #[must_use]
    pub fn order_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let erosion_at = self
            .layer
            .iter()
            .position(|l| matches!(l, Layer::Hydraulic { .. } | Layer::Thermal { .. }));

        if let Some(erosion) = erosion_at {
            for (index, layer) in self.layer.iter().enumerate() {
                if index < erosion && matches!(layer, Layer::Corridor { .. }) {
                    warnings.push(
                        "a `corridor` layer runs BEFORE erosion, which will erode the route \
                         it guarantees. A corridor is a constraint, not a shape — move it \
                         after the erosion layers."
                            .to_owned(),
                    );
                }
            }
        }
        warnings
    }

    /// Content hash of the recipe.
    ///
    /// **Erosion is baked once and keyed by this** (§4.4). GPU erosion with
    /// atomic accumulation is order-dependent and not bit-reproducible, which
    /// collides with the determinism requirement. Resolved structurally rather
    /// than fought: the recipe is authoritative for *authoring*, and the baked
    /// artifact is authoritative for *determinism*. Runtime never re-simulates.
    #[must_use]
    pub fn content_hash(&self) -> String {
        let canonical = toml::to_string(self).unwrap_or_default();
        blake3::hash(canonical.as_bytes()).to_hex().to_string()
    }

    /// Evaluate every layer in order.
    #[must_use]
    pub fn bake(&self) -> Heightmap {
        let mut map = Heightmap::new(self.size[0], self.size[1], self.world_scale);
        let mut rng = noise::Rng::new(self.seed);

        // Noise output is -1..1 and must be scaled into world units before
        // anything measures a slope. Scaling at the END instead would rescale
        // the map after erosion and UNDO it: thermal erosion reduces the height
        // range, and normalising back to `height_range` stretches the softened
        // slopes straight back to where they were. Found by the test for it.
        //
        // So the boundary is after the last noise layer — which is exactly
        // where the terrain doc puts art direction: place the landforms
        // gameplay needs, then let erosion make them look real.
        let last_noise = self
            .layer
            .iter()
            .rposition(|l| matches!(l, Layer::Fbm { .. } | Layer::Ridged { .. }));

        for (index, layer) in self.layer.iter().enumerate() {
            apply_layer(&mut map, layer, self.seed, &mut rng);
            if Some(index) == last_noise {
                self.normalise(&mut map);
            }
        }
        // A recipe with no noise at all still gets a predictable range.
        if last_noise.is_none() {
            self.normalise(&mut map);
        }
        map
    }

    /// Rescale into `height_range`.
    fn normalise(&self, map: &mut Heightmap) {
        let (lo, hi) = map.range();
        if (hi - lo).abs() < 1e-6 {
            return;
        }
        let [out_lo, out_hi] = self.height_range;
        for v in &mut map.data {
            *v = out_lo + (*v - lo) / (hi - lo) * (out_hi - out_lo);
        }
    }
}

#[allow(clippy::too_many_lines)]
fn apply_layer(map: &mut Heightmap, layer: &Layer, seed: u64, rng: &mut noise::Rng) {
    match layer {
        Layer::Fbm {
            amplitude,
            frequency,
            octaves,
            lacunarity,
            gain,
            warp,
        } => {
            for y in 0..map.height {
                for x in 0..map.width {
                    #[allow(clippy::cast_precision_loss)]
                    let (fx, fy) = (x as f32, y as f32);
                    let (wx, wy) = noise::warp(fx, fy, *warp, *frequency, seed);
                    let v = noise::fbm(wx, wy, *frequency, *octaves, *lacunarity, *gain, seed);
                    map.set(x, y, map.get(x, y) + v * amplitude);
                }
            }
        }
        Layer::Ridged {
            amplitude,
            frequency,
            octaves,
            warp,
        } => {
            for y in 0..map.height {
                for x in 0..map.width {
                    #[allow(clippy::cast_precision_loss)]
                    let (fx, fy) = (x as f32, y as f32);
                    let (wx, wy) = noise::warp(fx, fy, *warp, *frequency, seed);
                    let v = noise::ridged(wx, wy, *frequency, *octaves, seed);
                    map.set(x, y, map.get(x, y) + v * amplitude);
                }
            }
        }
        Layer::SplineCarve {
            points,
            width,
            depth,
        } => {
            for y in 0..map.height {
                for x in 0..map.width {
                    #[allow(clippy::cast_precision_loss)]
                    let p = [x as f32, y as f32];
                    let d = distance_to_polyline(p, points);
                    if d < *width {
                        // Smoothstep, so the carve blends instead of leaving a
                        // hard-edged trench.
                        let t = 1.0 - (d / width);
                        let falloff = t * t * (3.0 - 2.0 * t);
                        map.set(x, y, map.get(x, y) - depth * falloff);
                    }
                }
            }
        }
        Layer::FlattenDisc {
            center,
            radius,
            blend,
            target,
        } => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let target = target.unwrap_or_else(|| map.sample(center[0], center[1]));
            for y in 0..map.height {
                for x in 0..map.width {
                    #[allow(clippy::cast_precision_loss)]
                    let d = ((x as f32 - center[0]).powi(2) + (y as f32 - center[1]).powi(2)).sqrt();
                    if d > *radius {
                        continue;
                    }
                    let edge = radius * (1.0 - blend).max(0.0);
                    let t = if d <= edge {
                        1.0
                    } else {
                        let u = 1.0 - (d - edge) / (radius - edge).max(1e-3);
                        u * u * (3.0 - 2.0 * u)
                    };
                    let current = map.get(x, y);
                    map.set(x, y, current + (target - current) * t);
                }
            }
        }
        Layer::Peak { at, height, radius } => {
            for y in 0..map.height {
                for x in 0..map.width {
                    #[allow(clippy::cast_precision_loss)]
                    let d = ((x as f32 - at[0]).powi(2) + (y as f32 - at[1]).powi(2)).sqrt();
                    if d < *radius {
                        let t = 1.0 - d / radius;
                        map.set(x, y, map.get(x, y) + height * t * t * (3.0 - 2.0 * t));
                    }
                }
            }
        }
        Layer::Corridor {
            from,
            to,
            width,
            max_slope,
        } => carve_corridor(map, *from, *to, *width, *max_slope),
        Layer::Hydraulic {
            droplets,
            erode_rate,
            deposit_rate,
        } => erosion::hydraulic(map, *droplets, *erode_rate, *deposit_rate, rng),
        Layer::Thermal {
            talus_degrees,
            iterations,
        } => erosion::thermal(map, *talus_degrees, *iterations),
    }
}

/// Force a traversable route by clamping heights along it.
///
/// Walks the line and holds the height within what `max_slope` permits per
/// step, so the route is walkable **by construction** rather than by iteration.
fn carve_corridor(map: &mut Heightmap, from: [f32; 2], to: [f32; 2], width: f32, max_slope: f32) {
    let steps = (((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt().ceil() as usize).max(1);
    let [mx, _] = map.metres_per_pixel();
    let max_rise = max_slope.to_radians().tan() * mx;

    let mut height = map.sample(from[0], from[1]);
    for step in 0..=steps {
        #[allow(clippy::cast_precision_loss)]
        let t = step as f32 / steps as f32;
        let px = from[0] + (to[0] - from[0]) * t;
        let py = from[1] + (to[1] - from[1]) * t;

        // Follow the terrain, but never faster than the slope limit.
        let want = map.sample(px, py);
        height += (want - height).clamp(-max_rise, max_rise);

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (cx, cy) = (px as isize, py as isize);
        let r = width.ceil() as isize;
        for dy in -r..=r {
            for dx in -r..=r {
                #[allow(clippy::cast_precision_loss)]
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d > width {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x as usize >= map.width || y as usize >= map.height {
                    continue;
                }
                let t = 1.0 - d / width;
                let blend = t * t * (3.0 - 2.0 * t);
                #[allow(clippy::cast_sign_loss)]
                let (x, y) = (x as usize, y as usize);
                let current = map.get(x, y);
                map.set(x, y, current + (height - current) * blend);
            }
        }
    }
}

fn distance_to_polyline(p: [f32; 2], points: &[[f32; 2]]) -> f32 {
    if points.is_empty() {
        return f32::MAX;
    }
    let mut best = f32::MAX;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [p[0] - a[0], p[1] - a[1]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let t = if len2 < 1e-6 {
            0.0
        } else {
            ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
        };
        let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t];
        let d = ((p[0] - closest[0]).powi(2) + (p[1] - closest[1]).powi(2)).sqrt();
        best = best.min(d);
    }
    best
}

#[cfg(test)]
mod tests {
    /// Both of these arrive from CLI flags or from erosion particles that have
    /// drifted, so "the caller will not do that" is not a defence.
    #[test]
    fn extreme_coordinates_do_not_panic() {
        let map = super::Heightmap::new(8, 8, [10.0, 10.0]);
        for (x, y) in [(0.0_f32, 0.0_f32), (7.9, 7.9), (1e20, 1e20), (-5.0, -5.0), (f32::MAX, 0.0)] {
            let value = map.sample(x, y);
            assert!(value.is_finite(), "sample({x}, {y}) = {value}");
        }
    }

    use super::*;

    fn recipe(layers: &str) -> Recipe {
        Recipe::from_toml(&format!(
            "size = [64, 64]\nworld_scale = [256.0, 256.0]\n\
             height_range = [0.0, 100.0]\nseed = 7\n\n{layers}"
        ))
        .expect("fixture should parse")
    }

    #[test]
    fn a_recipe_bakes_deterministically() {
        let r = recipe("[[layer]]\nkind = \"fbm\"\noctaves = 4\n");

        assert_eq!(r.bake().data, r.bake().data, "same recipe, same heightmap");
    }

    /// A different seed must give different terrain, or the seed is decorative.
    #[test]
    fn the_seed_changes_the_result() {
        let a = recipe("[[layer]]\nkind = \"fbm\"\n");
        let mut b = a.clone();
        b.seed = 99;

        assert_ne!(a.bake().data, b.bake().data);
    }

    #[test]
    fn the_content_hash_follows_the_recipe() {
        let a = recipe("[[layer]]\nkind = \"fbm\"\n");
        let mut b = a.clone();
        b.seed = 8;

        assert_eq!(a.content_hash(), a.clone().content_hash());
        assert_ne!(a.content_hash(), b.content_hash(), "the artifact key must move");
    }

    /// Noise is scaled into `height_range` before anything measures a slope.
    /// Later layers work in those world units and may exceed it — a peak is
    /// allowed to stick up, and clamping it would silently discard intent.
    #[test]
    fn noise_output_lands_inside_the_requested_range() {
        let r = recipe("[[layer]]\nkind = \"fbm\"\namplitude = 50.0\n");
        let (lo, hi) = r.bake().range();

        assert!(lo >= -0.01 && hi <= 100.01, "range was {lo}..{hi}");
    }

    /// **The layer an agent reaches for when a fort needs somewhere to sit.**
    #[test]
    fn flatten_disc_creates_buildable_ground() {
        let r = recipe(
            "[[layer]]\nkind = \"fbm\"\namplitude = 40.0\n\n\
             [[layer]]\nkind = \"flatten_disc\"\ncenter = [32.0, 32.0]\nradius = 12.0\n",
        );
        let map = r.bake();

        let centre_slope = map.slope_degrees(32, 32);
        assert!(centre_slope < 5.0, "plateau slope was {centre_slope}°");
    }

    /// Compared at the SAME point with and without the carve. Comparing two
    /// arbitrary points instead just measures the noise, and an earlier
    /// version of this test did exactly that and failed for no real reason.
    #[test]
    fn spline_carve_lowers_the_ground_along_its_path() {
        let plain = recipe("[[layer]]\nkind = \"fbm\"\namplitude = 30.0\n").bake();
        let carved = recipe(
            "[[layer]]\nkind = \"fbm\"\namplitude = 30.0\n\n\
             [[layer]]\nkind = \"spline_carve\"\n\
             points = [[10.0, 32.0], [54.0, 32.0]]\nwidth = 6.0\ndepth = 25.0\n",
        )
        .bake();

        assert!(
            carved.get(32, 32) < plain.get(32, 32),
            "on the path: {} should be below {}",
            carved.get(32, 32),
            plain.get(32, 32)
        );
        assert!(
            (carved.get(32, 58) - plain.get(32, 58)).abs() < 1.0,
            "far from the path the carve must change nothing"
        );
    }

    /// Thermal erosion exists to fix the implausibly steep slopes noise makes.
    #[test]
    fn thermal_erosion_reduces_the_steepest_slopes() {
        let base = recipe("[[layer]]\nkind = \"ridged\"\namplitude = 80.0\noctaves = 6\n");
        let eroded = recipe(
            "[[layer]]\nkind = \"ridged\"\namplitude = 80.0\noctaves = 6\n\n\
             [[layer]]\nkind = \"thermal\"\ntalus_degrees = 30.0\niterations = 40\n",
        );

        let steepest = |m: &Heightmap| {
            (0..m.height)
                .flat_map(|y| (0..m.width).map(move |x| (x, y)))
                .map(|(x, y)| m.slope_degrees(x, y))
                .fold(0.0_f32, f32::max)
        };
        let before = steepest(&base.bake());
        let after = steepest(&eroded.bake());

        assert!(after < before, "thermal should soften: {before}° -> {after}°");
    }

    /// A corridor before erosion is legal and almost certainly wrong: the
    /// route it guarantees does not survive. Measured before it was warned
    /// about — reachable went true to false purely by adding thermal.
    #[test]
    fn a_corridor_before_erosion_is_warned_about() {
        let bad = recipe(
            "[[layer]]\nkind = \"ridged\"\n\n\
             [[layer]]\nkind = \"corridor\"\nfrom = [4.0, 32.0]\nto = [59.0, 32.0]\nwidth = 4.0\n\n\
             [[layer]]\nkind = \"thermal\"\n",
        );
        let good = recipe(
            "[[layer]]\nkind = \"ridged\"\n\n\
             [[layer]]\nkind = \"thermal\"\n\n\
             [[layer]]\nkind = \"corridor\"\nfrom = [4.0, 32.0]\nto = [59.0, 32.0]\nwidth = 4.0\n",
        );

        assert_eq!(bad.order_warnings().len(), 1, "must warn");
        assert!(bad.order_warnings()[0].contains("after the erosion"));
        assert!(good.order_warnings().is_empty(), "correct order is silent");
    }

    /// The guarantee itself: a corridor applied last survives.
    #[test]
    fn a_corridor_after_erosion_stays_traversable() {
        let r = Recipe::from_toml(
            "size = [96, 96]\nworld_scale = [768.0, 768.0]\n\
             height_range = [0.0, 300.0]\nseed = 4\n\n\
             [[layer]]\nkind = \"ridged\"\noctaves = 6\n\n\
             [[layer]]\nkind = \"thermal\"\ntalus_degrees = 34.0\niterations = 25\n\n\
             [[layer]]\nkind = \"corridor\"\n\
             from = [8.0, 48.0]\nto = [87.0, 48.0]\nwidth = 7.0\nmax_slope = 10.0\n",
        )
        .unwrap();

        assert!(
            analyze::is_reachable(&r.bake(), [8, 48], [87, 48], 20.0),
            "a corridor applied after erosion must survive it"
        );
    }

    #[test]
    fn an_oversized_recipe_is_refused_with_the_number() {
        let err = Recipe::from_toml(
            "size = [16384, 16384]\nworld_scale = [1.0, 1.0]\n\
             height_range = [0.0, 1.0]\nseed = 1\n",
        )
        .expect_err("too big");

        assert!(format!("{err}").contains("MB"), "must say how big: {err}");
    }

    #[test]
    fn the_sdf_bridge_is_negative_below_the_surface() {
        let r = recipe("[[layer]]\nkind = \"fbm\"\namplitude = 50.0\n");
        let map = r.bake();
        let surface = map.sample(32.0, 32.0);
        let [mx, my] = map.metres_per_pixel();
        let world = [32.0 * mx, surface, 32.0 * my];

        assert!(map.sdf_at([world[0], world[1] - 10.0, world[2]]) < 0.0, "below is inside");
        assert!(map.sdf_at([world[0], world[1] + 10.0, world[2]]) > 0.0, "above is outside");
    }
}
