//! The water surface: one function, written once and evaluated twice.
//!
//! **This is the authoritative water query.** Buoyancy, rendering, gameplay,
//! the CLI and the agent's measure tool all go through [`sample_water`] — there
//! is no second opinion about where the surface is. That matters more here than
//! almost anywhere else in the engine, because the failure mode of two
//! implementations is a boat floating visibly above the water it is drawn on,
//! with nothing to blame: neither side errors, neither side is wrong on its
//! own, and the only symptom is that it never quite lines up.
//!
//! **Never GPU readback.** The industry-standard trick is to render wave
//! heights into a displacement texture and read it back for physics, accepting
//! a frame of latency. Loom cannot: readback timing is not reproducible, so
//! `loom sim --assert` would go flaky, and the latency makes buoyancy lag the
//! surface visibly. Instead the CPU evaluates the waves analytically and the
//! GPU evaluates the same function independently. They agree because they are
//! one implementation transcribed once (see [`slang`]) and measured against
//! each other, not because one copies the other.
//!
//! # The formula
//!
//! Gerstner (trochoidal) waves, the standard form. For each wave, with
//! `k = 2π/λ`, `ω = sqrt(g·k)` for deep water, and `φ = k·(d·xz) − ω·t`:
//!
//! ```text
//! x += Q·A·d.x·cos(φ)      z += Q·A·d.z·cos(φ)      y += A·sin(φ)
//! ```
//!
//! It is not a fluid simulation and is not trying to be. The ocean is an
//! analytic function of position and time with no state, which is exactly why
//! it is cheap, exactly why it is deterministic, and exactly why a crater
//! blown in a lake bed does not drain the lake (a known v1 limitation, written
//! down in [`loom_scene::components::WaterBody`] rather than left to be
//! discovered).
//!
//! # Shallow water
//!
//! A wave that can feel the bottom flattens. The factor is
//! `tanh(k·d) / tanh(k·D)`, clamped to `[0, 1]`, where `d` is the still-water
//! depth and `D` is the authored [`WaveSet::attenuation_depth`] — see
//! [`shoal`], which explains what that buys and what it deliberately leaves
//! out.
//!
//! # Order of accumulation
//!
//! Waves are summed in index order and only in index order. Float addition is
//! not associative, so summing them in a different order on a different run is
//! a different number — the same class of bug `clippy.toml` guards against by
//! banning `HashMap` iteration in simulation code.

pub mod buoyancy;
pub mod flow;
pub mod spectrum;

use loom_scene::components::{MAX_WAVES, WaterBody};

/// Standard gravity, m/s². Sets every wave's phase speed through `ω = sqrt(gk)`.
///
/// Written out in both halves rather than shared, because the shader cannot
/// read a Rust constant. `9.81` is exactly representable the same way on both
/// sides, so this is one number spelled twice, not two numbers.
pub const GRAVITY: f32 = 9.81;

/// Everything a caller can ask about the surface at one point.
///
/// Mirrors Unreal's `GetLastWaterSurfaceInfo` field for field, which is a good
/// sign rather than a coincidence: it is the set of things a buoyancy solver,
/// a shader and a gameplay query each turn out to need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterSample {
    /// World Y of the surface, still-water level plus wave displacement.
    pub height: f32,
    /// Unit surface normal, **analytic** — see [`sample_water`].
    pub normal: [f32; 3],
    /// The full Gerstner offset from the sample point, for rendering.
    ///
    /// Horizontal as well as vertical: that horizontal pinch is what turns a
    /// sine wave into water, and it is also why the normal cannot be finite-
    /// differenced.
    pub displacement: [f32; 3],
    /// Velocity of the water here, m/s. Drag and buoyancy read it.
    ///
    /// The waves' orbital motion plus the river's current, summed — **one
    /// velocity, not two**. Drag is `drag · (v_water − v_body)`, and a second
    /// term for the current would be a second force path with its own
    /// coefficient, free to disagree with the first about what the water is
    /// doing. See [`flow`] for where the current comes from.
    pub velocity: [f32; 3],
    /// Still-water depth: surface level minus the bed, in metres.
    ///
    /// **Still-water, not wave-displaced.** A depth that oscillated with the
    /// waves would make shallow-water attenuation modulate itself, and the
    /// quantity every consumer actually wants is how deep the water is here,
    /// not how deep it is this instant. Negative means the bed is above the
    /// surface — dry land, which is information rather than an error.
    pub depth: f32,
}

/// How much of its deep-water amplitude a wave keeps in water `depth` deep.
///
/// **The model, and what it leaves out.** Linear wave theory puts the depth
/// dependence of a wave's surface amplitude in `tanh(k·d)` — the same factor
/// that appears in the dispersion relation `ω² = g·k·tanh(k·d)`, which is why
/// deep water (`tanh → 1`) is the form the rest of this file already uses.
/// Normalising by `tanh(k·D)` makes the authored depth `D` mean what the schema
/// says it means: at `D` and deeper the wave is at full height, and below it the
/// wave flattens, reaching zero exactly at the shoreline. Long swell feels the
/// bottom in deeper water than short chop does, which is the visible half of
/// shoaling and comes free from `k` being in there.
///
/// Three things are deliberately **not** modelled, and each is a visible thing a
/// real shore does:
///
/// - **No amplification.** Real waves grow before they break — Green's law says
///   amplitude rises as the water shallows, and only then collapses. This taper
///   is monotone into the shore and never exceeds 1. The reason is the fold
///   limit (§5.3): the validator proves `Σ Q·k·A ≤ 1` for the authored wave set,
///   and scaling every amplitude by a factor in `[0, 1]` cannot break a bound
///   that already holds, whereas amplification would need its own clamp and its
///   own proof. Surf that peaks belongs with breaking and foam, in W7.
/// - **No refraction.** Crests do not turn to run parallel to the beach. That
///   needs a per-wave direction that varies with position, which makes the phase
///   a path integral rather than a dot product, and the surface stops being an
///   analytic function of `(x, z, t)` — the property everything here rests on.
/// - **No wavelength shortening.** `ω` stays the deep-water `sqrt(g·k)`, so
///   crests do not bunch up as they slow down. Making `ω` depend on depth would
///   make the *phase* depend on the path a wave took to get here, with the same
///   consequence as refraction: no closed form, no agreement between CPU and GPU
///   without state.
///
/// `D ≤ 0` disables attenuation entirely, which is the schema default and what
/// keeps an open ocean identical to the day before this existed.
#[must_use]
pub fn shoal(k: f32, depth: f32, attenuation_depth: f32) -> f32 {
    if attenuation_depth <= 0.0 {
        return 1.0;
    }
    // `tanh` saturates rather than overflowing, so the sentinel depth a scene
    // with no terrain reports (a billion metres) is 1.0 and not an infinity.
    let full = (k * attenuation_depth).tanh();
    if full <= 0.0 {
        return 1.0;
    }
    ((k * depth).tanh() / full).clamp(0.0, 1.0)
}

/// Sample the water surface at a world XZ position and simulation time.
///
/// `flow` is the river's current at this point, in m/s, and is added to
/// [`WaterSample::velocity`]. It is a pre-sampled vector for exactly the reason
/// `ground_height` is a pre-sampled scalar: it comes off a grid derived from
/// the voxel terrain, and this function must not know what a voxel is or the
/// shader generated from it inherits the terrain system. `[0.0; 3]` is an ocean
/// or a lake, which is every scene but a river. See [`flow::FlowGrid::at`].
///
/// `ground_height` is the terrain surface under the sample. It sets
/// [`WaterSample::depth`], and through [`shoal`] it flattens the waves in the
/// shallows — so the same query has to answer for the shader as for the
/// buoyancy solver, which is what `loom_voxel::heightfield` is. The caller owns
/// it because it is a voxel-SDF lookup against terrain that changes at runtime,
/// and water must not cache it.
///
/// `t` is seconds since the simulation started — tick count times the fixed
/// timestep, **never a wall clock** (never-do #8). It is `f32` rather than
/// `f64` for one reason: the shader half has no `f64`, and a surface that
/// agrees with what is drawn matters more than phase precision after an hour of
/// simulated time.
///
/// # Analytic normals
///
/// The normal is the closed form of the Gerstner sum's partial derivatives, not
/// three nearby samples. Finite differencing is wrong here specifically, not
/// merely slower: Gerstner displaces horizontally, so three samples taken at
/// three nearby *base* points end up at three positions you did not choose, and
/// the resulting normal is subtly wrong everywhere and badly wrong at the
/// crests — which is precisely where a lighting error is visible. The closed
/// form costs nothing extra, because it reuses the `sin`/`cos` pair the
/// position evaluation already computed.
#[must_use]
pub fn sample_water(
    body: &WaterBody,
    world_xz: [f32; 2],
    t: f32,
    ground_height: f32,
    flow: [f32; 3],
) -> WaterSample {
    let mut displacement = [0.0_f32; 3];
    // **The current is the base the orbital motion is summed onto**, rather
    // than added at the end, so the two are one velocity from the first line.
    let mut velocity = flow;
    // The normal's three sums, from the standard tangent/binormal cross
    // product: the horizontal pair is Σ d·(kA)·cos(φ), the vertical is
    // Σ Q·(kA)·sin(φ) subtracted from one. Steepness enters only the vertical
    // term, which is why a steep wave flattens its own normal at the crest.
    let mut slope_x = 0.0_f32;
    let mut slope_z = 0.0_f32;
    let mut flatten = 0.0_f32;

    // Still-water depth, computed once: every wave's taper reads it, and a
    // depth that oscillated with the waves would make the attenuation modulate
    // itself.
    let depth = body.surface_height - ground_height;

    // `take` rather than trusting the caller: the validator rejects a longer
    // list at load, and the shader's loop is bounded at the same number, so a
    // hand-built body with more waves must not make the two disagree.
    for wave in body.waves.waves.iter().take(MAX_WAVES) {
        // Written as plain multiplies rather than `mul_add`: an fma rounds once
        // where this rounds twice, and the Slang half cannot be made to fuse on
        // request. Matching the other implementation is worth more here than a
        // last-bit-better length.
        let length = (wave.direction[0] * wave.direction[0]
            + wave.direction[1] * wave.direction[1])
            .sqrt();
        // A degenerate wave contributes nothing, on both sides, identically.
        // Skipping is what keeps a zero direction from becoming a NaN that
        // poisons the sample and every hash downstream of it.
        // Phrased as a positive test so a NaN fails it: `NaN > 0.0` is false,
        // and the wave is skipped rather than propagated.
        let usable = wave.wavelength > 0.0 && length > 0.0;
        if !usable {
            continue;
        }
        let d = [wave.direction[0] / length, wave.direction[1] / length];

        let k = std::f32::consts::TAU / wave.wavelength;
        let omega = (GRAVITY * k).sqrt() * wave.speed_scale;
        let phase = k * (d[0] * world_xz[0] + d[1] * world_xz[1]) - omega * t;
        let (sin_phase, cos_phase) = (phase.sin(), phase.cos());

        // **The shallows, entering as an amplitude and nothing else.** Every
        // term below reads `amplitude` rather than `wave.amplitude`, so the
        // normal and the orbital velocity flatten with the surface instead of
        // describing a wave that is no longer there.
        let amplitude = wave.amplitude * shoal(k, depth, body.waves.attenuation_depth);

        let qa = wave.steepness * amplitude;
        let ka = k * amplitude;

        displacement[0] += qa * d[0] * cos_phase;
        displacement[1] += amplitude * sin_phase;
        displacement[2] += qa * d[1] * cos_phase;

        slope_x += d[0] * ka * cos_phase;
        slope_z += d[1] * ka * cos_phase;
        flatten += wave.steepness * ka * sin_phase;

        // d/dt of the displacement above. dφ/dt = −ω, which is where each
        // sign comes from — the vertical term is the one that trips people up.
        velocity[0] += qa * d[0] * omega * sin_phase;
        velocity[1] -= amplitude * omega * cos_phase;
        velocity[2] += qa * d[1] * omega * sin_phase;
    }

    let normal = [-slope_x, 1.0 - flatten, -slope_z];
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    // Written out rather than a `normalize` helper because the shader half is
    // written out too: a library `normalize` may use a reciprocal square root
    // with different rounding, and this is the one place where matching the
    // other implementation is worth more than brevity.
    let normal = if length > 0.0 {
        [normal[0] / length, normal[1] / length, normal[2] / length]
    } else {
        // Only reachable on a folded surface, which the validator rejects at
        // load. Straight up is the least wrong answer and never a NaN.
        [0.0, 1.0, 0.0]
    };

    WaterSample {
        height: body.surface_height + displacement[1],
        normal,
        displacement,
        velocity,
        depth,
    }
}

/// The Slang half, emitted verbatim into the generated shader.
///
/// **This and [`sample_water`] are one thing written twice.** Keep them
/// adjacent, change them together, and rely on the agreement test — which runs
/// this source through `slangc` and compares it against the Rust numerically —
/// to catch it when someone does not.
///
/// Why verbatim rather than through `loom_field`'s expression tree: that tree
/// is a *scalar field* language with no loops and no vector results, and a wave
/// set is a runtime-length loop producing a position, a normal and a velocity.
/// Growing the tree to express this would be building a shading language to
/// avoid transcribing thirty lines of arithmetic. The precedent followed here
/// instead is `loom_field::noise`, which is written twice for the same reason
/// and kept honest by the same kind of test.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The water surface. The Rust half is `loom_water::sample_water`; they are one
// implementation written twice, and `slang_agreement` compiles this and
// compares the two numerically.
//
// Never edit this by hand: it is emitted from the Rust, and editing it changes
// the GPU alone — which is the divergence that puts a boat above its own water.

static const int LOOM_MAX_WAVES = 16;

struct LoomWave {
    // Direction first: it is the only member needing 8-byte alignment, and
    // leading with a scalar would pad the struct differently here than in the
    // Rust that fills the buffer.
    float2 direction;
    float wavelength;
    float amplitude;
    float steepness;
    float speed_scale;
};

struct LoomWaveSet {
    LoomWave waves[LOOM_MAX_WAVES];
    int count;
    // Depth in metres below which waves start to flatten. Zero disables it,
    // which is the schema default and what keeps an open ocean unchanged.
    float attenuation_depth;
};

// How much of its deep-water amplitude a wave keeps in water `depth` deep.
// The Rust half is `loom_water::shoal`, which carries the reasoning: linear
// theory's tanh(k*d), normalised so the authored depth is where the wave is
// whole again. Never above 1, which is what keeps the steepness limit the
// validator enforces from being broken by the shallows.
float loom_shoal(float k, float depth, float attenuation_depth) {
    if (attenuation_depth <= 0.0) { return 1.0; }
    float full = tanh(k * attenuation_depth);
    if (full <= 0.0) { return 1.0; }
    return clamp(tanh(k * depth) / full, 0.0, 1.0);
}

struct LoomWaterSample {
    float height;
    float3 normal;
    float3 displacement;
    float3 velocity;
    float depth;
};

LoomWaterSample loom_sample_water(
    LoomWaveSet set,
    float surface_height,
    float ground_height,
    float2 xz,
    float t,
    // The river's current here, m/s, sampled from the flow grid by the caller —
    // `float3(0, 0, 0)` for an ocean or a lake. Pre-sampled for the same reason
    // `ground_height` is: this function knows nothing about the terrain.
    float3 flow)
{
    float3 displacement = float3(0.0, 0.0, 0.0);
    // The current is what the orbital motion is summed onto, so the two are one
    // velocity rather than two forces.
    float3 velocity = flow;
    float slope_x = 0.0;
    float slope_z = 0.0;
    float flatten = 0.0;

    // Still-water depth, once: a depth that oscillated with the waves would
    // make the attenuation modulate itself.
    float depth = surface_height - ground_height;

    for (int i = 0; i < LOOM_MAX_WAVES; ++i) {
        if (i >= set.count) { break; }
        LoomWave wave = set.waves[i];

        float length = sqrt(wave.direction.x * wave.direction.x
                          + wave.direction.y * wave.direction.y);
        bool usable = wave.wavelength > 0.0 && length > 0.0;
        if (!usable) { continue; }
        float2 d = float2(wave.direction.x / length, wave.direction.y / length);

        // 2π and g, spelled to the same float the Rust holds.
        float k = 6.2831855 / wave.wavelength;
        float omega = sqrt(9.81 * k) * wave.speed_scale;
        float phase = k * (d.x * xz.x + d.y * xz.y) - omega * t;
        float sin_phase = sin(phase);
        float cos_phase = cos(phase);

        // The shallows, entering as an amplitude and nothing else.
        float amplitude = wave.amplitude * loom_shoal(k, depth, set.attenuation_depth);

        float qa = wave.steepness * amplitude;
        float ka = k * amplitude;

        displacement.x += qa * d.x * cos_phase;
        displacement.y += amplitude * sin_phase;
        displacement.z += qa * d.y * cos_phase;

        slope_x += d.x * ka * cos_phase;
        slope_z += d.y * ka * cos_phase;
        flatten += wave.steepness * ka * sin_phase;

        velocity.x += qa * d.x * omega * sin_phase;
        velocity.y -= amplitude * omega * cos_phase;
        velocity.z += qa * d.y * omega * sin_phase;
    }

    float3 normal = float3(-slope_x, 1.0 - flatten, -slope_z);
    float normal_length = sqrt(normal.x * normal.x
                             + normal.y * normal.y
                             + normal.z * normal.z);
    if (normal_length > 0.0) {
        normal = float3(normal.x / normal_length,
                        normal.y / normal_length,
                        normal.z / normal_length);
    } else {
        normal = float3(0.0, 1.0, 0.0);
    }

    LoomWaterSample result;
    result.height = surface_height + displacement.y;
    result.normal = normal;
    result.displacement = displacement;
    result.velocity = velocity;
    result.depth = depth;
    return result;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_scene::components::{GerstnerWave, WaterBody, WaveSet};

    /// A body with the given waves at still-water level zero.
    fn body(waves: Vec<GerstnerWave>) -> WaterBody {
        WaterBody {
            waves: WaveSet { waves, ..WaveSet::default() },
            ..WaterBody::default()
        }
    }

    fn wave(wavelength: f32, amplitude: f32, steepness: f32, direction: [f32; 2]) -> GerstnerWave {
        GerstnerWave { wavelength, amplitude, steepness, direction, speed_scale: 1.0 }
    }

    /// No waves is a mirror: flat at the authored level, normal straight up,
    /// nothing moving. The default `WaterBody` is this, so it is also the test
    /// that an unauthored body is inert rather than wrong.
    #[test]
    fn still_water_is_flat_and_still() {
        let mut still = body(Vec::new());
        still.surface_height = 3.5;

        let s = sample_water(&still, [12.0, -7.0], 4.25, -2.0, [0.0; 3]);

        assert_eq!(s.height, 3.5);
        assert_eq!(s.normal, [0.0, 1.0, 0.0]);
        assert_eq!(s.displacement, [0.0, 0.0, 0.0]);
        assert_eq!(s.velocity, [0.0, 0.0, 0.0]);
        assert_eq!(s.depth, 5.5);
    }

    /// A single wave has a closed form, so the sampler can be checked against
    /// arithmetic rather than against itself. At phase zero the crest is on its
    /// way up: height is zero, the horizontal displacement is at its maximum,
    /// and the surface is moving downward.
    #[test]
    fn one_wave_matches_the_closed_form_at_the_origin() {
        // λ = 2π so k = 1 exactly, which makes every number below checkable by
        // hand: ω = sqrt(9.81), φ = −ωt, and at t = 0, φ = 0.
        let w = wave(std::f32::consts::TAU, 0.5, 0.4, [1.0, 0.0]);
        let sea = body(vec![w]);

        let s = sample_water(&sea, [0.0, 0.0], 0.0, 0.0, [0.0; 3]);

        assert!(s.height.abs() < 1e-6, "sin(0) = 0, so the surface is at rest level: {s:?}");
        assert!((s.displacement[0] - 0.2).abs() < 1e-6, "Q·A·cos(0) = 0.2: {s:?}");
        assert_eq!(s.displacement[2], 0.0, "a wave along +X moves nothing on Z");
        // vy = −A·ω·cos(0) = −0.5·sqrt(9.81).
        let omega = GRAVITY.sqrt();
        assert!((s.velocity[1] + 0.5 * omega).abs() < 1e-5, "{s:?}");
    }

    /// **§5.4: the normal is the analytic one, and it differs from a finite
    /// difference by a visible margin.** The point of the test is not that the
    /// analytic normal is self-consistent — it is that naively sampling three
    /// nearby points gives a measurably different answer, so the shortcut this
    /// crate refuses to take is a real error rather than a style preference.
    #[test]
    fn the_analytic_normal_is_not_what_finite_differencing_would_give() {
        let sea = body(vec![wave(9.0, 1.2, 0.9, [1.0, 0.0])]);
        let t = 1.0;
        let height = |x: f32| sample_water(&sea, [x, 0.0], t, 0.0, [0.0; 3]).height;

        // Walk a wavelength: the two disagree everywhere, and the question is
        // how badly at the worst point — which is at the crests, where the
        // horizontal displacement is largest and a lighting error is most
        // visible.
        let mut worst = 0.0_f32;
        for i in 0..900 {
            let x = i as f32 * (9.0 / 900.0);
            let analytic = sample_water(&sea, [x, 0.0], t, 0.0, [0.0; 3]).normal;

            // What a finite difference sees: three base points, whose heights
            // are read as if the surface were still above those base points. It
            // is not — the wave slid it sideways.
            let h = 0.01;
            let slope = (height(x + h) - height(x - h)) / (2.0 * h);
            let naive_length = slope.mul_add(slope, 1.0).sqrt();
            worst = worst.max((analytic[0] - -slope / naive_length).abs());

            // And the analytic one is a unit vector, which the sampler
            // promises to every caller that lights with it.
            let n = analytic;
            let magnitude = n[0].mul_add(n[0], n[1].mul_add(n[1], n[2] * n[2])).sqrt();
            assert!((magnitude - 1.0).abs() < 1e-6, "{magnitude} at x={x}");
        }

        assert!(
            worst > 0.1,
            "the finite difference agrees to within {worst} everywhere, so this \
             test proves nothing — steepen the wave"
        );
    }

    /// The normal is consistent with the surface it describes: rotate the wave
    /// direction and the normal rotates with it, rather than leaning a fixed
    /// way. An axis mix-up between X and Z is invisible in a still image and
    /// lights the whole sea wrong.
    #[test]
    fn the_normal_follows_the_wave_direction() {
        let along_x = sample_water(&body(vec![wave(8.0, 0.4, 0.7, [1.0, 0.0])]), [1.0, 1.0], 0.0, 0.0, [0.0; 3]);
        let along_z = sample_water(&body(vec![wave(8.0, 0.4, 0.7, [0.0, 1.0])]), [1.0, 1.0], 0.0, 0.0, [0.0; 3]);

        assert!(along_x.normal[0].abs() > 0.1, "{along_x:?}");
        assert_eq!(along_x.normal[2], 0.0, "a wave along +X tilts nothing on Z");
        assert!(along_z.normal[2].abs() > 0.1, "{along_z:?}");
        assert_eq!(along_z.normal[0], 0.0);
    }

    /// **The steepness limit is where the surface folds, measured rather than
    /// asserted.** The horizontal map `x → x + ΣQ·A·d.x·cos(φ)` stops being
    /// monotonic exactly when `Q·k·A` passes 1: below it every base point lands
    /// left of its neighbour, above it they cross over and the surface
    /// self-intersects. That crossing is the "visibly broken, folded mesh" the
    /// validator exists to prevent, and this is the test that the number it
    /// enforces is the right number.
    #[test]
    fn the_surface_folds_exactly_at_the_steepness_limit() {
        // Q·k·A = product, by construction: λ = 2π gives k = 1, so with A = 1
        // the product is just Q.
        let folded = |steepness: f32| {
            let sea = body(vec![wave(std::f32::consts::TAU, 1.0, steepness, [1.0, 0.0])]);
            // Walk a full wavelength and watch the displaced X coordinate. A
            // fold is the moment it stops increasing.
            let mut previous = f32::NEG_INFINITY;
            let mut folds = false;
            for i in 0..2000 {
                let x = i as f32 * (std::f32::consts::TAU / 2000.0);
                let moved = x + sample_water(&sea, [x, 0.0], 0.0, 0.0, [0.0; 3]).displacement[0];
                if moved < previous {
                    folds = true;
                }
                previous = moved;
            }
            folds
        };

        assert!(!folded(0.9), "Q·k·A = 0.9 is under the limit and must not fold");
        assert!(!folded(0.99), "the limit is 1.0, and 0.99 is still a surface");
        assert!(folded(1.05), "Q·k·A = 1.05 is over the limit: the surface folds");
        assert!(folded(1.5), "and it only gets worse");
    }

    /// The same limit, divided among several waves — which is what makes the
    /// validator's `1/N` right. Four waves each at the single-wave limit fold;
    /// four waves each at a quarter of it do not.
    #[test]
    fn several_waves_share_one_fold_budget() {
        let four = |steepness: f32| {
            let sea = body(vec![wave(std::f32::consts::TAU, 1.0, steepness, [1.0, 0.0]); 4]);
            let mut previous = f32::NEG_INFINITY;
            let mut folds = false;
            for i in 0..2000 {
                let x = i as f32 * (std::f32::consts::TAU / 2000.0);
                let moved = x + sample_water(&sea, [x, 0.0], 0.0, 0.0, [0.0; 3]).displacement[0];
                if moved < previous {
                    folds = true;
                }
                previous = moved;
            }
            folds
        };

        assert!(!four(0.24), "four waves at 1/N of the budget must not fold");
        assert!(four(0.30), "four waves over 1/N each do");
    }

    /// The surface never produces a NaN or an infinity, including at the
    /// degenerate inputs a hand-built body can carry — a zero direction, a
    /// zero wavelength, a zero amplitude. One NaN poisons every determinism
    /// hash downstream of it.
    #[test]
    fn degenerate_waves_are_skipped_rather_than_poisoning_the_sample() {
        let sea = body(vec![
            wave(0.0, 1.0, 0.5, [1.0, 0.0]),
            wave(10.0, 0.5, 0.5, [0.0, 0.0]),
            wave(10.0, 0.0, 0.5, [1.0, 0.0]),
            wave(12.0, 0.4, 0.6, [0.7, -0.7]),
        ]);

        for tick in 0..500 {
            let t = tick as f32 / 60.0;
            let s = sample_water(&sea, [t * 3.0 - 400.0, 90.0 - t], t, -5.0, [0.0; 3]);
            for v in [s.height, s.depth] {
                assert!(v.is_finite(), "{s:?}");
            }
            for axis in 0..3 {
                assert!(s.normal[axis].is_finite(), "{s:?}");
                assert!(s.displacement[axis].is_finite(), "{s:?}");
                assert!(s.velocity[axis].is_finite(), "{s:?}");
            }
        }
    }

    /// **Waves beyond the cap are not sampled**, because the shader's loop is
    /// bounded at the same number. A body that got past the validator with a
    /// longer list must produce the same surface on both sides, not a taller
    /// one on the CPU.
    #[test]
    fn only_the_first_sixteen_waves_are_summed() {
        let small = wave(11.0, 0.1, 0.3, [1.0, 0.3]);
        let capped = sample_water(&body(vec![small; MAX_WAVES]), [3.0, 5.0], 2.0, 0.0, [0.0; 3]);
        let over = sample_water(&body(vec![small; MAX_WAVES + 4]), [3.0, 5.0], 2.0, 0.0, [0.0; 3]);

        assert_eq!(capped.height, over.height);
        assert_eq!(capped.displacement, over.displacement);
    }

    /// The waves travel, and they travel the way the direction says. A field
    /// that evaluated the same at every time would pass most of the tests above
    /// and animate nothing.
    #[test]
    fn the_waves_move_downwind_over_time() {
        let sea = body(vec![wave(10.0, 0.4, 0.5, [1.0, 0.0])]);
        let at = [0.0, 0.0];

        let now = sample_water(&sea, at, 0.0, 0.0, [0.0; 3]).height;
        let later = sample_water(&sea, at, 0.7, 0.0, [0.0; 3]).height;
        assert!((now - later).abs() > 0.05, "the surface is frozen: {now} vs {later}");

        // ω = sqrt(g·k), so the crest travels sqrt(g/k) metres per second. One
        // period later the same point is back where it started.
        let k = std::f32::consts::TAU / 10.0;
        let period = std::f32::consts::TAU / (GRAVITY * k).sqrt();
        let one_period = sample_water(&sea, at, period, 0.0, [0.0; 3]).height;
        assert!((now - one_period).abs() < 1e-3, "{now} vs {one_period} after one period");
    }

    /// A body with waves and an authored shoaling depth.
    fn shoaling(waves: Vec<GerstnerWave>, attenuation_depth: f32) -> WaterBody {
        WaterBody {
            waves: WaveSet { waves, attenuation_depth, ..WaveSet::default() },
            ..WaterBody::default()
        }
    }

    /// **Waves flatten as the bottom rises, and stop at the shoreline.** The
    /// exit criterion of the step, as a number rather than as a screenshot.
    #[test]
    fn the_waves_flatten_toward_the_shore() {
        let sea = shoaling(vec![wave(20.0, 0.8, 0.4, [1.0, 0.0])], 10.0);

        // The peak-to-trough range over one period, which is the honest measure
        // of "how big are the waves here" — a single instant could be sampled
        // at a zero crossing whatever the amplitude.
        let range = |ground: f32| {
            let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
            for i in 0..200 {
                let t = i as f32 * 0.05;
                let h = sample_water(&sea, [0.0, 0.0], t, ground, [0.0; 3]).height;
                low = low.min(h);
                high = high.max(h);
            }
            high - low
        };

        let deep = range(-40.0);
        let shelf = range(-10.0);
        let shallow = range(-2.0);
        let ankle = range(-0.2);
        let dry = range(0.5);

        assert!((deep - shelf).abs() < 1e-3, "at the authored depth the wave is whole: {deep} vs {shelf}");
        assert!(shallow < shelf * 0.75, "2 m of water barely flattened it: {shallow} of {shelf}");
        assert!(ankle < shelf * 0.2, "ankle-deep water still has a swell in it: {ankle}");
        assert!(dry == 0.0, "the surface is still moving over dry land: {dry}");
    }

    /// Long waves feel the bottom before short ones do — the one piece of real
    /// shoaling physics the taper keeps, and the reason it is `tanh(k·d)`
    /// rather than a depth ramp shared by every wave.
    #[test]
    fn long_waves_feel_the_bottom_first() {
        let depth = 3.0;
        let attenuation = 12.0;
        let swell = shoal(std::f32::consts::TAU / 60.0, depth, attenuation);
        let chop = shoal(std::f32::consts::TAU / 4.0, depth, attenuation);

        assert!(swell < 0.8, "a 60 m swell in 3 m of water is barely touched: {swell}");
        assert!(chop > 0.99, "a 4 m chop in 3 m of water should not care: {chop}");
    }

    /// **An unauthored `attenuation_depth` changes nothing at all.** Zero is the
    /// schema default, and every scene written before the shallows existed has
    /// it — so this is the test that they still render and hash as they did.
    #[test]
    fn no_authored_depth_is_no_attenuation() {
        let waves = vec![wave(20.0, 0.8, 0.4, [1.0, 0.0]), wave(7.0, 0.3, 0.5, [0.2, 1.0])];
        let open = body(waves.clone());

        // Including a bed a metre under the surface: with no authored depth,
        // even that must not change a single bit of the answer.
        for ground in [-1000.0, -6.0, -1.0, 0.5] {
            let a = sample_water(&open, [3.0, -4.0], 1.25, ground, [0.0; 3]);
            let b = sample_water(&open, [3.0, -4.0], 1.25, -1000.0, [0.0; 3]);
            assert_eq!(a.height, b.height, "ground {ground} moved the surface");
            assert_eq!(a.displacement, b.displacement);
            assert_eq!(a.velocity, b.velocity);
            assert_eq!(a.normal, b.normal);
        }
    }

    /// **The fold limit holds at every depth, over a flat bed.** Attenuation
    /// scales every amplitude by a factor in `[0, 1]`, and the fold condition
    /// `Σ Q·k·A ≤ 1` is linear in `A` — so a wave set the validator accepted in
    /// deep water cannot fold in shallow water. Measured rather than argued,
    /// with the set sitting right at the limit where the argument is tightest.
    #[test]
    fn shoaling_never_folds_the_surface_over_a_flat_bed() {
        // Q·k·A = 1 exactly at full amplitude: λ = 2π gives k = 1, A = 1, Q = 1.
        let sea = shoaling(vec![wave(std::f32::consts::TAU, 1.0, 1.0, [1.0, 0.0])], 6.0);

        for step in 0..=60 {
            let ground = -(step as f32) * 0.2;
            let mut previous = f32::NEG_INFINITY;
            for i in 0..2000 {
                let x = i as f32 * (std::f32::consts::TAU / 2000.0);
                let moved = x + sample_water(&sea, [x, 0.0], 0.0, ground, [0.0; 3]).displacement[0];
                assert!(
                    moved >= previous,
                    "the surface folds at depth {}: x = {x} moved to {moved} behind {previous}",
                    -ground
                );
                previous = moved;
            }
        }
    }

    /// **The one way shoaling can fold a surface, measured and bounded.** A bed
    /// that rises steeply makes the taper itself vary with x, and the horizontal
    /// map `x → x + Σ Q·A(x)·cos φ` picks up a `dA/dx` term the flat-bed
    /// argument above does not cover. So: how steep does the bed have to be?
    ///
    /// The answer, measured, is **between 1:1 and 1.1:1** — a 45° underwater
    /// slope, and only for a wave set sitting exactly on the fold limit; the
    /// budget is shared, so a gentler sea folds on a steeper bed or not at all.
    /// Real beaches are 1:20 to 1:100 and the scene in the gate is about 1:4 at
    /// its waterline, so this
    /// is a documented bound rather than a bug. The test exists so the number is
    /// known and so a future taper that makes it worse is caught here.
    #[test]
    fn a_steep_enough_bed_is_where_shoaling_could_fold() {
        // The same wave set as above, right at the limit.
        let sea = shoaling(vec![wave(std::f32::consts::TAU, 1.0, 1.0, [1.0, 0.0])], 6.0);
        // A bed climbing toward the shore: deep at x = 0, breaking the surface
        // at x = SHORE. The dangerous half is where the taper is *falling*,
        // which is why the bed has to rise with x rather than away from it.
        const SHORE: f32 = 8.0 * std::f32::consts::TAU;
        let folds = |rise: f32| {
            let mut previous = f32::NEG_INFINITY;
            let mut folded = false;
            for i in 0..8000 {
                let x = i as f32 * (SHORE / 8000.0);
                let ground = rise * (x - SHORE);
                let moved = x + sample_water(&sea, [x, 0.0], 0.0, ground, [0.0; 3]).displacement[0];
                if moved < previous {
                    folded = true;
                }
                previous = moved;
            }
            folded
        };

        assert!(!folds(0.125), "a 1:8 beach must not fold");
        assert!(!folds(0.25), "nor a 1:4 one — which is the gate scene's shore");
        assert!(!folds(1.0), "nor, measured, a 45° one");
        assert!(folds(1.1), "steeper than 45° is where it goes, at Q·k·A = 1");
    }

    /// The Slang half has to carry the same constants. A numeric comparison
    /// lives in `tests/slang_agreement.rs` and needs `slangc`; this catches a
    /// typo without any toolchain at all.
    #[test]
    fn the_slang_half_uses_the_same_constants() {
        let slang = slang();
        for needle in ["6.2831855", "9.81", "LOOM_MAX_WAVES = 16", "loom_sample_water"] {
            assert!(slang.contains(needle), "the Slang half is missing `{needle}`");
        }
        // The cap is declared in three places — `loom_scene::MAX_WAVES`, the
        // schema's `maxItems`, and the shader's loop bound. This is the one
        // pairing that would silently change the surface if it drifted: a
        // shader summing fewer waves than the CPU is a boat above its water.
        assert!(
            slang.contains(&format!("LOOM_MAX_WAVES = {MAX_WAVES}")),
            "the Slang loop bound is not `loom_scene`'s MAX_WAVES = {MAX_WAVES}"
        );
    }
}
