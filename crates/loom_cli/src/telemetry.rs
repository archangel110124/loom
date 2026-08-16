//! Per-frame numbers written alongside a captured frame sequence.
//!
//! **The contact sheet answers "does it look right"; this answers "what are the
//! numbers doing".** For a phase error, a sign flip or a lag, a column of
//! floats settles in one line what a grid of small images can only suggest —
//! and the two together let a finding be stated by frame index: *the velocity
//! goes positive at frame 12 and the streaks visibly reverse in cells 12-15*.
//!
//! # Only what the CPU already knows
//!
//! Every column here is read from something the simulation computed anyway.
//! Nothing is measured *for* the CSV, and nothing is read back from the GPU.
//!
//! That last part is a rule rather than a convenience. The rain layer, the
//! grass blades' bend and the splash ring live entirely on the device and are
//! never read back — ADR 0014 is explicit that a readback would destroy the
//! determinism the whole capture rests on. So there is no `rain_active_
//! particles` or `grass_bend_mean` here: the honest substitutes are the drop
//! *count the layer was told to draw* and the *rate* the CPU computed, both of
//! which are the inputs those GPU numbers are derived from.
//!
//! # Adding a column
//!
//! Implement [`Probe`] and add it to [`probes`]. The writer takes the header
//! from whatever the probes return, so a new system needs no change here.

use loom_ecs::World;
use loom_scene::Scene;

/// Everything a probe may read about one captured frame.
pub(crate) struct Frame<'a> {
    pub index: u32,
    pub sim_time: f32,
    pub weather: &'a crate::weather::Weather,
    pub eye: [f32; 3],
    /// Draw-call count: objects after batching by mesh.
    pub draws: usize,
    /// Scattered instances in the frame, for count and placement hash.
    pub scattered: &'a [loom_render::Object],
    /// Grass blades the CPU placed. Zero in a scene with none.
    pub blades: usize,
    /// Raindrops the GPU layer was told to draw.
    pub drops: u32,
    /// Wall-clock milliseconds this frame took to produce.
    pub frame_ms: f64,
}

/// One system's contribution to a row.
pub(crate) trait Probe {
    /// Named values for this frame. **The same names in the same order every
    /// frame** — the header is taken from the first row and the writer rejects
    /// a row that disagrees, because a shifted column is worse than none.
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)>;
}

/// Timing and draw counts, always present.
struct Always;

impl Probe for Always {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        #[allow(clippy::cast_precision_loss)]
        let draws = frame.draws as f64;
        vec![("frame_time_ms", frame.frame_ms), ("draw_calls", draws)]
    }
}

/// The wind at the camera, which is what everything else leans along.
struct WindProbe;

impl Probe for WindProbe {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        let w = frame.weather.wind.at(frame.eye, frame.sim_time);
        let magnitude = w[0].mul_add(w[0], w[1].mul_add(w[1], w[2] * w[2])).sqrt();
        vec![
            ("wind_x", f64::from(w[0])),
            ("wind_y", f64::from(w[1])),
            ("wind_z", f64::from(w[2])),
            ("wind_magnitude", f64::from(magnitude)),
        ]
    }
}

/// The water surface over a grid around the camera.
///
/// **A grid rather than one point**, because a wave's phase at a single sample
/// tells you nothing about whether the sea is flat, and min/max/mean is what
/// distinguishes "no waves" from "waves at the wrong scale".
struct WaterProbe {
    body: loom_scene::components::WaterBody,
}

impl Probe for WaterProbe {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        const SIDE: i32 = 8;
        const SPAN: f32 = 40.0;
        let (mut min, mut max, mut total, mut energy) = (f32::MAX, f32::MIN, 0.0_f64, 0.0_f64);
        let mut n = 0_u32;
        for iz in 0..SIDE {
            for ix in 0..SIDE {
                #[allow(clippy::cast_precision_loss)]
                let at = [
                    (f32::from(ix as i16) / f32::from(SIDE as i16) - 0.5).mul_add(SPAN, frame.eye[0]),
                    (f32::from(iz as i16) / f32::from(SIDE as i16) - 0.5).mul_add(SPAN, frame.eye[2]),
                ];
                let sample = loom_water::sample_water(
                    &self.body,
                    at,
                    frame.sim_time,
                    loom_voxel::heightfield::NO_GROUND,
                    [0.0; 3],
                );
                // Height above the still surface, which is the quantity that
                // reads as "how big are the waves".
                let h = sample.height - self.body.surface_height;
                min = min.min(h);
                max = max.max(h);
                total += f64::from(h);
                // Displacement squared: proportional to the wave energy, and
                // the number that shows a sea building or dying.
                energy += f64::from(h) * f64::from(h);
                n += 1;
            }
        }
        let count = f64::from(n.max(1));
        vec![
            ("water_height_min", f64::from(min)),
            ("water_height_max", f64::from(max)),
            ("water_height_mean", total / count),
            ("water_energy", energy / count),
        ]
    }
}

/// What the rain is doing where the camera stands.
///
/// **Not a particle count.** The drops are GPU-resident and never read back
/// (ADR 0014); `rain_drops` is how many the layer was told to draw, and
/// `rain_rate` is the millimetres per hour the CPU computed — which is the
/// input every GPU number downstream is derived from.
struct RainProbe;

impl Probe for RainProbe {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        let sample = frame.weather.rain_at(frame.eye);
        vec![
            ("rain_rate", f64::from(sample.rate)),
            ("rain_exposure", f64::from(sample.exposure)),
            ("rain_drops", f64::from(frame.drops)),
        ]
    }
}

/// Grass the CPU placed. The bend itself is computed in the vertex shader and
/// is not readable without a readback, so it is not here.
struct GrassProbe;

impl Probe for GrassProbe {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        #[allow(clippy::cast_precision_loss)]
        let blades = frame.blades as f64;
        vec![("grass_blades", blades)]
    }
}

/// Scattered instances, and a hash of where they are.
///
/// **The hash is the determinism check.** Placement is a pure function of
/// position, so it must not change between two runs of the same scene — and a
/// changed hash says so in one column rather than requiring anyone to diff two
/// contact sheets by eye.
struct ScatterProbe;

impl Probe for ScatterProbe {
    fn columns(&self, frame: &Frame) -> Vec<(&'static str, f64)> {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for object in frame.scattered {
            // The translation column, which is where an instance is.
            for f in [object.model.w_axis.x, object.model.w_axis.y, object.model.w_axis.z] {
                for b in f.to_bits().to_le_bytes() {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let count = frame.scattered.len() as f64;
        vec![
            ("scatter_instance_count", count),
            // Truncated to 52 bits so an f64 carries it exactly — a CSV of
            // floats that silently rounded the hash would be a determinism
            // check that cannot fail.
            #[allow(clippy::cast_precision_loss)]
            ("scatter_placement_hash", (h >> 12) as f64),
        ]
    }
}

/// The probes a scene earns.
///
/// A column exists only when the system behind it does, so the header describes
/// the scene rather than the engine.
pub(crate) fn probes(scene: &Scene, world: &World) -> Vec<Box<dyn Probe>> {
    let mut out: Vec<Box<dyn Probe>> = vec![Box::new(Always), Box::new(WindProbe)];
    let has = |name: &str| scene.nodes().iter().any(|n| n.components.contains_key(name));

    if let Some(body) = world
        .water()
        .and_then(|v| serde_json::from_value::<loom_scene::components::WaterBody>(v.clone()).ok())
    {
        out.push(Box::new(WaterProbe { body }));
    }
    if has("Rain") {
        out.push(Box::new(RainProbe));
    }
    if has("Grass") {
        out.push(Box::new(GrassProbe));
    }
    if has("Scatter") {
        out.push(Box::new(ScatterProbe));
    }
    out
}

/// Rows collected across a capture, and the writer.
#[derive(Default)]
pub(crate) struct Telemetry {
    header: Vec<&'static str>,
    rows: Vec<Vec<f64>>,
}

impl Telemetry {
    pub(crate) fn push(&mut self, probes: &[Box<dyn Probe>], frame: &Frame) {
        let mut names: Vec<&'static str> = vec!["frame", "sim_time"];
        #[allow(clippy::cast_precision_loss)]
        let mut values: Vec<f64> = vec![f64::from(frame.index), f64::from(frame.sim_time)];
        for probe in probes {
            for (name, value) in probe.columns(frame) {
                names.push(name);
                values.push(value);
            }
        }
        if self.header.is_empty() {
            self.header = names;
        } else if self.header != names {
            // **Reported rather than silently written.** A row whose columns
            // moved would line numbers up under the wrong headings, which is a
            // worse failure than no telemetry: it reads as data.
            crate::log::warn(
                "telemetry: a frame produced different columns; the row was dropped".to_owned(),
            );
            return;
        }
        self.rows.push(values);
    }

    /// Write the CSV. No-op when nothing was collected.
    ///
    /// # Errors
    /// The io error, as a string.
    pub(crate) fn write(&self, path: &std::path::Path) -> Result<(), String> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let mut text = self.header.join(",");
        text.push('\n');
        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    // The frame index is an integer and reads as one; every
                    // other column is fixed to four places, which is what keeps
                    // the file diffable and free of scientific notation.
                    if i == 0 { format!("{}", *v as i64) } else { format!("{v:.4}") }
                })
                .collect();
            text.push_str(&cells.join(","));
            text.push('\n');
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}
