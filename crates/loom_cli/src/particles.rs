//! Turning `ParticleEmitter` components into particles the renderer can draw.
//!
//! **Stepped, not sampled.** A particle system has no meaningful state at tick
//! zero — every particle is newly born at the emitter, which draws as a dot.
//! So the plume is simulated forward before it is rendered, either for the
//! ticks `--sim` asked for or, failing that, for long enough to reach the
//! steady population it will hold from then on. A still render of a smoke
//! plume should look like smoke, not like the instant somebody lit it.

use loom_ecs::World;
use loom_render::ParticleInstance;

/// The simulation's fixed timestep. Matches physics, and is a constant rather
/// than a measured frame time for the reason never-do #8 exists.
const DT: f32 = 1.0 / 60.0;

/// Read an emitter component into the simulation's parameter struct.
fn parse(component: &serde_json::Value) -> (loom_particles::Emitter, Visual) {
    let f = |name: &str, fallback: f32| {
        #[allow(clippy::cast_possible_truncation)]
        component
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .map_or(fallback, |v| v as f32)
    };
    let v = |name: &str, fallback: [f32; 3]| {
        let Some(list) = component.get(name).and_then(serde_json::Value::as_array) else {
            return fallback;
        };
        let mut out = fallback;
        for (slot, value) in out.iter_mut().zip(list) {
            if let Some(x) = value.as_f64() {
                #[allow(clippy::cast_possible_truncation)]
                {
                    *slot = x as f32;
                }
            }
        }
        out
    };
    let pair = |name: &str, fallback: [f32; 2]| {
        let got = v(name, [fallback[0], fallback[1], 0.0]);
        [got[0], got[1]]
    };

    let defaults = loom_particles::Emitter::default();
    (
        loom_particles::Emitter {
            rate: f("rate", defaults.rate),
            lifetime: f("lifetime", defaults.lifetime),
            lifetime_jitter: f("lifetime_jitter", defaults.lifetime_jitter),
            speed: f("speed", defaults.speed),
            spread_degrees: f("spread_degrees", defaults.spread_degrees),
            radius: f("radius", defaults.radius),
            gravity: f("gravity", defaults.gravity),
            drag: f("drag", defaults.drag),
            turbulence: f("turbulence", defaults.turbulence),
            turbulence_scale: f("turbulence_scale", defaults.turbulence_scale),
            seed: component
                .get("seed")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1),
        },
        Visual {
            size: pair("size", [0.6, 3.2]),
            color_start: v("color_start", [0.32, 0.30, 0.29]),
            color_end: v("color_end", [0.62, 0.62, 0.64]),
            alpha: pair("alpha", [0.55, 0.0]),
        },
    )
}

/// The part of an emitter that only the renderer cares about.
struct Visual {
    size: [f32; 2],
    color_start: [f32; 3],
    color_end: [f32; 3],
    alpha: [f32; 2],
}

/// Simulate every emitter in the world and return what to draw.
///
/// `ticks` of `None` means "long enough to look settled": a plume reaches its
/// steady population after one particle lifetime, so twice that is comfortably
/// past the transient.
#[must_use]
pub(crate) fn simulate(world: &World, ticks: Option<u32>) -> Vec<ParticleInstance> {
    let mut out = Vec::new();

    for entity in world.entities() {
        let Some(component) = world.emitter(*entity) else {
            continue;
        };
        let Some(global) = world.global_transform(*entity) else {
            continue;
        };
        let (emitter, visual) = parse(component);
        // Column 3 of the model matrix is its translation.
        let origin = [global.matrix[12], global.matrix[13], global.matrix[14]];

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let steps = ticks.unwrap_or_else(|| ((emitter.lifetime * 2.0) / DT).ceil() as u32);
        let mut system = loom_particles::System::new(emitter.seed);
        for _ in 0..steps {
            system.step(DT, &emitter, origin);
        }

        for p in system.particles() {
            let t = p.fraction();
            // Size and colour interpolate over the particle's life. Smoke
            // expands and pales as it cools and mixes with air; a plume whose
            // particles keep their birth size and colour reads as a stream of
            // identical blobs.
            let size = visual.size[0] + (visual.size[1] - visual.size[0]) * t;
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            // Fade in as well as out. Particles that appear at full opacity
            // pop, and the pop is at the emitter where the eye already is.
            let fade = (t * 8.0).min(1.0);
            out.push(ParticleInstance {
                position: [p.position[0], p.position[1], p.position[2], size * 0.5],
                color: [
                    lerp(visual.color_start[0], visual.color_end[0]),
                    lerp(visual.color_start[1], visual.color_end[1]),
                    lerp(visual.color_start[2], visual.color_end[2]),
                    lerp(visual.alpha[0], visual.alpha[1]) * fade,
                ],
            });
        }
    }

    out
}
