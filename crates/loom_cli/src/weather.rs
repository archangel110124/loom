//! Reading a scene's weather into a sampleable field.
//!
//! **The one place the authored scalars become the field.** `loom_scene`
//! depends on nothing else in the workspace, so it cannot know about
//! `loom_field`, and `loom_field` has no business parsing a scene. The bridge
//! between them is here, once — written twice it would be the same
//! two-sources-of-truth problem the field itself exists to prevent, one level
//! up.

use loom_ecs::World;
use loom_field::wind::Wind;
use loom_scene::Scene;
use loom_scene::components::{Rain, WaterBody};

/// Everything a scene says about the weather, resolved once.
///
/// **Carried as one value because the parts are not independent.** Rain's rate
/// is the authored intensity scaled by the sky exposure, and the lean on a
/// streak is P1's wind scaled by that same number — three fields that only mean
/// anything together, threaded through the assertion path as one argument
/// rather than as a tuple that grows a slot per phase.
pub(crate) struct Weather {
    /// The scene's wind, or the default breeze.
    pub wind: Wind,
    /// The scene's rain, or `None` — which means dry, not a default drizzle.
    pub rain: Option<Rain>,
    /// The voxel volume that stands between a point and the sky, with the world
    /// position of its origin, or `None` for a scene with no terrain to shelter
    /// under.
    ///
    /// **Held as a volume rather than baked into a grid**, which is the whole
    /// difference from W6's height field: `loom_rain` marches this on every
    /// query, so an edit to it is visible on the next call with no rebake.
    ///
    /// **What is not yet true is that a run can edit it.** This volume is
    /// rebuilt from the scene's op list, and `play::Sim` keeps no `Volume` at
    /// all — it turns one into solid cells for parry at load and drops it. So
    /// nothing inside a `loom sim` run can carve, and until something can, the
    /// live march is a property of the query rather than of the simulation.
    /// Wire the carve to *this* value when a mid-run CSG op exists; do not
    /// cache what it returns.
    pub sky: Option<(loom_voxel::Volume, [f32; 3])>,
    /// Simulated seconds the reading is taken at — the fixed timestep times the
    /// tick, never a wall clock (never-do #8).
    pub seconds: f32,
    /// The cloud deck, which decides *where* it rains. See [`deck_of`].
    pub deck: loom_rain::Deck,
    /// The water surface, resolved, or `None` for a scene with none — and
    /// `None` too when nothing asked, because the bed is a pass over every
    /// voxel in the scene. See [`water_probe`].
    pub water: Option<WaterProbe>,
}

impl Weather {
    /// What the rain is doing at a world point.
    ///
    /// The one call everything downstream makes. It exists here rather than at
    /// each call site so that the sky's frame conversion and the "absent means
    /// dry" rule are applied once.
    pub fn rain_at(&self, at: [f32; 3]) -> loom_rain::RainSample {
        rain_at(
            self.rain.as_ref(),
            &self.wind,
            self.sky.as_ref(),
            Some(self.deck),
            at,
            self.seconds,
        )
    }

    /// How wet a surface at a world point is by now.
    ///
    /// The exposure is [`Self::rain_at`]'s — S3's march of the scene's own
    /// voxels, the same number that scaled the rate. Wetness grows no occlusion
    /// of its own, which is the rule the whole phase is arranged around.
    pub fn wetness_at(&self, at: [f32; 3]) -> loom_rain::Wetness {
        // **Cover averaged over the recent past, not the cover now.** See
        // `loom_rain::cover_recent`: wetness is an integral and the deck drifts
        // faster than the ground responds.
        loom_rain::wetness_under(
            self.rain.as_ref(),
            self.rain_at(at).exposure,
            loom_rain::cover_recent(self.deck, &self.wind, at, self.seconds),
            self.seconds,
        )
    }
}

/// The water surface, with everything it needs pre-sampled from, held together.
///
/// **The same four arguments `loom water --at` assembles**, and that is the
/// point: an assertion has to read the surface the renderer draws and the
/// buoyancy solver feels, which means the bed, the current and the wake grid,
/// not the bare Gerstner sum. Two spellings of that assembly is how a
/// `water@` assertion would come to disagree with the picture.
pub(crate) struct WaterProbe {
    body: WaterBody,
    bed: Option<loom_voxel::heightfield::HeightField>,
    flow: Option<loom_water::flow::FlowGrid>,
    /// The interactive events as they stood at the end of the run, cloned off
    /// the runner. State, so there is no way to recompute them here.
    wavelets: loom_water::wavelet::WaveletField,
    /// The advected foam field at the end of the run, cloned off the runner
    /// for the same reason: it is stepped state and cannot be recomputed from
    /// a position and a time. `water@x,z.foam` reads it.
    foam: Option<loom_water::foam::FoamField>,
}

impl WaterProbe {
    /// Foam coverage at a world XZ, `[0, 1]` — **the same number the shader
    /// draws**: the advected field, floored by the instantaneous whitecap
    /// coverage, exactly as the water fragment shader's `wetc` is.
    ///
    /// The trail is deliberately not in it. The trail is a ten-tap unroll of
    /// the same closed form evaluated at ten past instants, and reproducing it
    /// here would be a third implementation of one thing; what an assertion
    /// wants to know is whether there is foam here, and between the field and
    /// the instantaneous term that question is answered.
    pub fn foam_at(&self, xz: [f32; 2], seconds: f32) -> f32 {
        let mu = self.at(xz, seconds).mu_max;
        // `smoothstep(WATER_FOAM_WET, WATER_FOAM_BREAK, mu)`, the shader's two
        // constants spelled again next to the branch that uses them.
        let t = ((mu - 0.22) / (0.33 - 0.22)).clamp(0.0, 1.0);
        let instant = t * t * 2.0_f32.mul_add(-t, 3.0);
        let field = self.foam.as_ref().map_or(0.0, |f| f.at(xz[0], xz[1]));
        instant.max(field)
    }

    /// The surface at a world XZ, at the tick the run ended on.
    pub fn at(&self, xz: [f32; 2], seconds: f32) -> loom_water::WaterSample {
        let ground = self
            .bed
            .as_ref()
            .map_or(loom_voxel::heightfield::NO_GROUND, |g| g.at(xz[0], xz[1]));
        let flow = self.flow.as_ref().map_or([0.0; 3], |g| g.at(xz[0], xz[1]));
        let wavelet = self.wavelets.at(xz[0], xz[1], seconds);
        // **The orbital velocity is summed into the current, not reported
        // beside it** — `sample_water` has one water-velocity argument, so
        // `water@x,z.speed` reads what a floating body there would feel.
        let flow = [flow[0] + wavelet.velocity[0], flow[1], flow[2] + wavelet.velocity[1]];
        loom_water::sample_water(&self.body, xz, seconds, ground, flow, wavelet.surface())
    }
}

/// Resolve a scene's water into something a `water@` assertion can read.
///
/// **Built only when an assertion asks**, for the reason [`Weather::sky`] is:
/// the bed is a march over every voxel in the scene, and a run that never
/// mentions water must not pay for it.
#[must_use]
pub(crate) fn water_probe(
    scene: &Scene,
    world: &World,
    wind: &Wind,
    wavelets: &loom_water::wavelet::WaveletField,
    foam: Option<&loom_water::foam::FoamField>,
) -> Option<WaterProbe> {
    let body = water_of(world, wind)?;
    let bed = crate::scene_terrain_field(scene);
    let flow = bed.as_ref().and_then(|g| crate::river_flow(g, &body));
    Some(WaterProbe { body, bed, flow, wavelets: wavelets.clone(), foam: foam.cloned() })
}

/// The same query, for a caller that holds the pieces rather than a [`Weather`].
///
/// **The renderer is that caller.** It has a borrowed `Wind` (which is not
/// `Clone` — it holds a built expression tree) and the sky volume it baked once
/// at load, so it cannot assemble a `Weather` per frame. It must still get the
/// identical answer: the streaks a scene draws and the rate `loom sim --assert`
/// reads are the same number, and two spellings of the frame conversion is
/// exactly how they would stop being.
pub(crate) fn rain_at(
    rain: Option<&Rain>,
    wind: &Wind,
    sky: Option<&(loom_voxel::Volume, [f32; 3])>,
    deck: Option<loom_rain::Deck>,
    at: [f32; 3],
    seconds: f32,
) -> loom_rain::RainSample {
    loom_rain::sample_rain_under(
        rain,
        wind,
        sky.map(|(volume, offset)| loom_rain::Sky { volume, offset: *offset }),
        deck,
        at,
        seconds,
    )
}

/// The cloud deck a scene's `Environment` authors, for the term that decides
/// where it rains.
///
/// **`cloud_cover` of zero with rain in the scene is read as unauthored, not as
/// a clear sky**, and it is filled with a solid deck. Rain out of a clear sky is
/// not a weather state; every scene written before clouds existed says nothing
/// about cover and must keep raining exactly as it did; and a solid deck gives
/// coverage 1 everywhere, so it does. Authoring any value above zero takes full
/// control — that is how a scene asks for a squall.
///
/// **At the mood ladder's rung, not the file's floor.** A scene whose stages
/// ramp `cloud_cover` from a clear morning to a solid overcast had its rain
/// coverage decided by the *unramped* component — so the deck thickened in the
/// picture while the term that decides where it rains stayed at whatever the
/// file's own number said. Inert for every scene that authors no stages, which
/// is every scene in the library but two.
#[must_use]
pub(crate) fn deck_of(world: &loom_ecs::World, dread: Option<f32>) -> loom_rain::Deck {
    let defaults = loom_scene::components::Environment::default();
    let ramped = world
        .environment()
        .map(|c| crate::mood_of(c, crate::dread_of(world, dread)).0);
    let scalar = |name: &str, fallback: f32| {
        ramped
            .as_ref()
            .and_then(|c| c.get(name))
            .and_then(serde_json::Value::as_f64)
            .map_or(fallback, |v| {
                #[allow(clippy::cast_possible_truncation)]
                {
                    v as f32
                }
            })
    };
    let authored = scalar("cloud_cover", defaults.cloud_cover);
    loom_rain::Deck {
        cover: if authored > 0.0 { authored } else { 1.0 },
        scale: scalar("cloud_scale", defaults.cloud_scale),
        drift: 2.5,
    }
}

/// The scene's rain, or `None` when it authors none.
///
/// **`None` is dry.** Unlike wind, which every outdoor scene has whether or not
/// it says so, rain is the exception rather than the default — and every scene
/// authored before this phase must simulate byte-identically, which a component
/// that defaulted itself on would break.
///
/// At most one per scene; the first is used, the same rule `wind_of` follows.
#[must_use]
pub(crate) fn rain_of(scene: &Scene) -> Option<Rain> {
    let authored = scene
        .nodes()
        .iter()
        .find_map(|node| node.components.get("Rain"))?;
    serde_json::from_value::<Rain>(authored.clone()).ok()
}

/// The one mapping from the authored scalars to the field.
///
/// `speed` overrides the authored value when a mood stage names one — the
/// escalating half of a weather ramp. Everything else about the wind (where it
/// blows from, how gusty, how turbulent) stays the scene's, because a rising
/// wind is the same wind harder and not a different one.
fn wind_from(authored: Option<&serde_json::Value>, speed: Option<f32>) -> Wind {
    let Some(authored) = authored
        .and_then(|value| serde_json::from_value::<loom_scene::components::Wind>(value.clone()).ok())
    else {
        return speed.map_or_else(Wind::default, |speed| {
            let d = loom_scene::components::Wind::default();
            Wind::new(d.direction_degrees, speed, d.gustiness, d.turbulence, d.ground_drag)
        });
    };

    Wind::new(
        authored.direction_degrees,
        speed.unwrap_or(authored.speed),
        authored.gustiness,
        authored.turbulence,
        authored.ground_drag,
    )
}

/// The scene's wind, or a default breeze when it authors none.
///
/// **At most one per scene; the first is used** — the same rule `Environment`
/// follows, and for the same reason: two of them is a scene with two weathers
/// and no way to say which is meant. A second is not an error, because
/// rejecting a whole scene over a duplicate that has an obvious reading would
/// be worse than picking one.
#[must_use]
pub(crate) fn wind_of(scene: &Scene) -> Wind {
    wind_from(
        scene
            .nodes()
            .iter()
            .find_map(|node| node.components.get("Wind")),
        None,
    )
}

/// The same wind, read from a loaded world.
///
/// The simulation has a `World` and no `Scene` — and the sea a crate floats on
/// has to be the sea that is drawn, so both paths resolve the weather through
/// the same function rather than through two readings of the same fields.
#[must_use]
pub(crate) fn wind_of_world(world: &World) -> Wind {
    wind_from(world.wind(), None)
}

/// The same wind with a mood rung's speed substituted — the weather ramp.
///
/// **This is the fixed-tick path and the only one allowed to move the sea.**
/// `speed` comes from `mood_weather_of` at the tick's own `dread`, never from
/// the viewer's frame clock: a `WaterBody` with no authored waves derives them
/// from this, and those waves push rigid bodies (ADR 0045 clause 2).
#[must_use]
pub(crate) fn wind_of_world_at(world: &World, speed: Option<f32>) -> Wind {
    wind_from(world.wind(), speed)
}

/// The scene's water with its wave set resolved, or `None` if it has no water.
///
/// **The resolution is the point.** A `WaterBody` that lists no waves gets
/// sixteen derived from the wind, and the surface a body floats on must be the
/// surface that is drawn — so rendering and buoyancy both come through here.
/// Two readings of "authored waves, else the spectrum" is exactly the divergence
/// that puts a boat above its own water.
#[must_use]
pub(crate) fn water_of(world: &World, wind: &Wind) -> Option<WaterBody> {
    let mut body =
        serde_json::from_value::<WaterBody>(world.water()?.clone()).ok()?;
    if body.waves.waves.is_empty() {
        let params = wind.params();
        // U10, which is what the spectrum is written against — never
        // `Wind::speed`, which is a free-stream value about 10% above it.
        let u10 = wind.mean_speed_at(10.0);
        let direction = [params.get("dir_x"), params.get("dir_z")];
        // **A stated fetch is the difference between wind that zooms the sea
        // and wind that roughens it.** Absent is unlimited, which is the
        // fully-developed sea this has always derived.
        body.waves = match body.fetch {
            Some(fetch) => loom_water::spectrum::wave_set_fetch(u10, direction, fetch),
            None => loom_water::spectrum::wave_set(u10, direction),
        };
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(wind: &str) -> Scene {
        Scene::parse(&format!(
            "[scene]\nformat = 1\n\n[[node]]\nname = \"World\"\n{wind}"
        ))
        .expect("valid scene")
    }

    /// A scene that says nothing about weather still gets weather — the
    /// lowest wind that visibly moves foliage, so nothing looks like a still.
    #[test]
    fn a_scene_without_wind_gets_the_default_breeze() {
        let wind = wind_of(&scene(""));

        let v = wind.at([0.0, 2.0, 0.0], 1.0);
        let speed = v[0].mul_add(v[0], v[2] * v[2]).sqrt();
        assert!(speed > 1.0, "the default is dead calm: {v:?}");
    }

    /// **Every authored field reaches the sampler.** A component whose values
    /// are read into the wrong parameters is invisible in a still and wrong in
    /// every frame, so each one is checked by the effect it has.
    #[test]
    fn the_authored_values_reach_the_field() {
        let calm = wind_of(&scene(
            "\n  [node.components.Wind]\n  speed = 0.0\n  turbulence = 0.0\n",
        ));
        assert_eq!(calm.at([1.0, 1.0, 1.0], 3.0), [0.0, 0.0, 0.0], "speed did not reach it");

        // 90° clockwise from +X is +Z.
        let northward = wind_of(&scene(
            "\n  [node.components.Wind]\n  direction_degrees = 90.0\n  turbulence = 0.0\n",
        ));
        let v = northward.at([4.0, 2.0, 4.0], 1.0);
        assert!(v[2].abs() > v[0].abs() * 4.0, "direction did not reach it: {v:?}");

        // Ground drag is the fraction still blowing at the surface, so raising
        // it to 1 removes the height profile.
        let undragged = wind_of(&scene(
            "\n  [node.components.Wind]\n  ground_drag = 1.0\n  turbulence = 0.0\n",
        ));
        let low = undragged.at([2.0, 0.0, 2.0], 1.0)[0];
        let high = undragged.at([2.0, 30.0, 2.0], 1.0)[0];
        assert!(
            (low - high).abs() < 1e-3,
            "ground_drag did not reach it: {low} at the surface, {high} above"
        );

        // Gustiness is the swell around the mean, so zero makes the speed
        // constant in time.
        let steady = wind_of(&scene(
            "\n  [node.components.Wind]\n  gustiness = 0.0\n  turbulence = 0.0\n",
        ));
        let a = steady.at([5.0, 2.0, 5.0], 0.0)[0];
        let b = steady.at([5.0, 2.0, 5.0], 9.0)[0];
        assert!((a - b).abs() < 1e-4, "gustiness did not reach it: {a} then {b}");
    }

    /// Turbulence is the term that breaks unison. With it off the field is
    /// smooth in space at a fixed time; with it on, neighbouring points differ.
    #[test]
    fn turbulence_breaks_the_unison() {
        let still = wind_of(&scene(
            "\n  [node.components.Wind]\n  turbulence = 0.0\n  gustiness = 0.0\n",
        ));
        let swirling = wind_of(&scene(
            "\n  [node.components.Wind]\n  turbulence = 3.0\n  gustiness = 0.0\n",
        ));

        let spread = |wind: &Wind| {
            let a = wind.at([10.0, 2.0, 10.0], 4.0);
            let b = wind.at([13.0, 2.0, 10.0], 4.0);
            (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
        };

        assert!(spread(&still) < 1e-3, "the calm field is not uniform");
        assert!(spread(&swirling) > 0.1, "turbulence does not vary in space");
    }
}
