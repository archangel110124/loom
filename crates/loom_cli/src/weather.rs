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
    /// The FFT cascade, cloned off the runner at the tick the run ended on — ADR 0076.
    ///
    /// **Cloned rather than rebuilt, for the reason the wavelet pool and the foam field
    /// are cloned**: there is one ocean in a run, the simulation owns it and evolves it
    /// inside the fixed step, and a probe that built a second one would be a second
    /// opinion about where the surface is. It holds the tiles for `Weather::seconds` and
    /// [`WaterProbe::at`] is only ever asked for that instant.
    sea: Option<loom_water::ocean::Ocean>,
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
        let field = self.foam.as_ref().map_or(0.0, |f| f.at(xz[0], xz[1]));
        instant_foam(mu, xz, seconds).max(field)
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
        loom_water::sample_water(
            &self.body,
            self.sea.as_ref(),
            xz,
            seconds,
            ground,
            flow,
            wavelet.surface(),
        )
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
    sea: Option<&loom_water::ocean::Ocean>,
) -> Option<WaterProbe> {
    let body = water_of(world, wind)?;
    let bed = crate::scene_terrain_field(scene);
    let flow = bed.as_ref().and_then(|g| crate::river_flow(g, &body));
    Some(WaterProbe {
        body,
        sea: sea.cloned(),
        bed,
        flow,
        wavelets: wavelets.clone(),
        foam: foam.cloned(),
    })
}

/// The sea state the audio bed is a function of — **read off the water, never
/// re-derived from it**.
///
/// Every number here is one the simulation already computed this tick, for the
/// reason `loom_water`'s own header opens on: a second opinion about how rough
/// the sea is would be free to disagree with the sea the boat floats on, and
/// this project has shipped that defect before.
///
/// - **`hs`** is `Ocean::significant_height` — the cascade's own `4√m0` over
///   its tiles — when there is a cascade. For a `gerstner` body there is none,
///   and the answer is `spectrum::significant_height` of the wave set that body
///   actually carries. That is the same pair of arms `loom water --at` reports
///   `waves.significant_height` from, spelled once here and read there.
/// - **`breaking`** is the foam field's mean coverage. The field is built for
///   *any* water body and its crest deposit is driven by `mu_max` past
///   `FOAM_CREST_BREAK`, so it answers "what fraction of this surface is
///   breaking" for both wave models through one implementation — which is what
///   makes the gerstner case need no invention. It carries hull wake with it,
///   deliberately: a wake is white water and sounds like it.
/// - **`wind`** is U10, the same figure the wave set was built from. It is a
///   tone control in the bed and never a level.
///
/// A scene with no water never reaches this and gets [`SeaState::default`],
/// which renders exact silence.
pub(crate) fn sea_state(
    body: Option<&WaterBody>,
    sea: Option<&loom_water::ocean::Ocean>,
    foam: Option<&loom_water::foam::FoamField>,
    u10: f32,
) -> loom_audio::sea::SeaState {
    let Some(body) = body else {
        return loom_audio::sea::SeaState::default();
    };
    loom_audio::sea::SeaState {
        hs: sea.map_or_else(
            || loom_water::spectrum::significant_height(&body.waves),
            loom_water::ocean::Ocean::significant_height,
        ),
        breaking: foam.map_or(0.0, loom_water::foam::FoamField::mean),
        wind: u10,
    }
}

/// The FFT cascade a scene's water asks for, from the same wind the wave set is.
///
/// **The one place a scene's `spectrum` body becomes an ocean**, and it sits beside
/// [`water_of`] on purpose: the two must be built from the same `u10` and the same
/// heading, or the sea a scene *reports* and the sea it *floats on* are two seas.
/// `None` for a scene with no water and for every `gerstner` body, which is all of them
/// but `ocean_fft`.
///
/// **It does not evolve what it builds.** `Ocean::new` leaves every tile zero; who calls
/// [`loom_water::ocean::Ocean::evolve`], and when, is the ownership decision written down
/// on [`loom_water::ocean::Ocean::for_body`] — the simulation, once per fixed step.
#[must_use]
pub(crate) fn sea_of(world: &World, wind: &Wind) -> Option<loom_water::ocean::Ocean> {
    sea_of_body(&water_of(world, wind)?, wind)
}

/// The same, for a caller that already holds the body.
///
/// **Split out so a test can measure one body against a variant of itself** —
/// the swell removed, the fetch put back — without a second spelling of the
/// wind-to-`U10` conversion. Two spellings of that is exactly how the sea a
/// scene reports and the sea it floats on stop being one sea.
#[must_use]
pub(crate) fn sea_of_body(body: &WaterBody, wind: &Wind) -> Option<loom_water::ocean::Ocean> {
    let params = wind.params();
    // U10, not `Wind::speed` — the same line `water_of` takes, for the same reason.
    loom_water::ocean::Ocean::for_body(
        body,
        wind.mean_speed_at(10.0),
        [params.get("dir_x"), params.get("dir_z")],
    )
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

/// **The breaking threshold's wander, the CPU half — SEA-REBUILD §4.3.**
///
/// `waterFoamBreakWander` in `assets/shaders/scene.slang`, spelled again here
/// because a shader cannot read a Rust constant and this is not a
/// [`loom_field`] expression tree — the same arrangement `groundLayerWeight`
/// and `WATER_FOAM_WET` itself already live under. **The constants below and
/// the shader's must move together**, and
/// `the_gentle_sea_still_foams_nowhere` is what fails when only one of them
/// does: `water@x,z.foam` answers this, `loom sim --assert` reads that, and a
/// wander on one side only is two opinions about the same sea.
///
/// The noise is `loom_field::noise::value` — the frozen integer-lattice hash
/// whose Slang half is compared to it **exactly** by the agreement test, which
/// is what makes writing this twice safe at all.
///
/// **Only ever positive.** Both foam thresholds move up by it together, so a
/// sea whose `mu_max` never reaches `WATER_FOAM_WET` still foams nowhere.
///
/// The cell octave is taken at full weight: the shader retires it on the
/// fragment's screen footprint and a point query has none, so this is the
/// near-field answer, which is what "the surface at this point" means.
fn break_wander(xz: [f32; 2], seconds: f32) -> f32 {
    /// `WATER_FOAM_PATCH_SCALE`.
    const PATCH_SCALE: f32 = 0.035;
    /// `WATER_FOAM_PATCH`.
    const PATCH: f32 = 0.060;
    /// `WATER_FOAM_PATCH_SLICE`.
    const PATCH_SLICE: f32 = 21.3;
    /// `WATER_FOAM_PATCH_DRIFT`.
    const PATCH_DRIFT: f32 = 0.05;
    /// `WATER_FOAM_CELL_SCALE`.
    const CELL_SCALE: f32 = 1.4;
    /// `WATER_FOAM_CELL`.
    const CELL: f32 = 0.055;
    /// `WATER_FOAM_CELL_SLICE`.
    const CELL_SLICE: f32 = 27.9;

    let patch = loom_field::noise::value([
        xz[0] * PATCH_SCALE,
        xz[1] * PATCH_SCALE,
        PATCH_DRIFT.mul_add(seconds, PATCH_SLICE),
    ]);
    // `waterFoamBreakPatch`'s rectification: the lower half of the octave is
    // exactly zero, so half the sea breaks on the documented threshold.
    let patch = ((patch - 0.5) * 2.0).clamp(0.0, 1.0);
    let cell = loom_field::noise::value([xz[0] * CELL_SCALE, xz[1] * CELL_SCALE, CELL_SLICE]);
    CELL.mul_add(cell, PATCH * patch)
}

/// `WATER_FOAM_WET` in `assets/shaders/scene.slang`: where the sea starts to
/// darken and wet, the lower end of the pair [`instant_foam`] opens.
///
/// **Named rather than spelled inline, because this number has four copies and
/// they have to move together.** The shader owns it; this is its CPU twin, and
/// `loom_water`'s `the_derived_sea_breaks_without_a_hand_authoring_it` and
/// `main`'s `lucent_breaks_and_throws` each hold a fourth in a doc comment
/// there is no import path for. `the_gentle_sea_still_foams_nowhere` reads
/// *this* one, so the test that guards the property cannot drift away from the
/// number it is guarding.
pub(crate) const FOAM_WET: f32 = 0.13;
/// `WATER_FOAM_BREAK`: where a crest is drawn white. See [`FOAM_WET`].
///
/// **No longer equal to [`loom_water::spray::SPRAY_BREAK`], which stayed at
/// 0.33.** They were one decision — "where a crest is breaking" — and this
/// commit split them deliberately: the foam pair moved to raise coverage, and
/// how many crests *throw droplets* is a separate look judgement with its own
/// cost. Whether spray follows is the human's call; see the foam-threshold
/// report.
pub(crate) const FOAM_BREAK: f32 = 0.24;

/// The whitecap coverage a crest of this steepness is drawn with right now.
///
/// `smoothstep(WATER_FOAM_WET + w, WATER_FOAM_BREAK + w, mu)` — the water
/// fragment shader's two constants and [`break_wander`]'s `w`, spelled once
/// here for every caller in this crate rather than once per caller.
pub(crate) fn instant_foam(mu_max: f32, xz: [f32; 2], seconds: f32) -> f32 {
    let wander = break_wander(xz, seconds);
    let lo = FOAM_WET + wander;
    let hi = FOAM_BREAK + wander;
    let t = ((mu_max - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * 2.0_f32.mul_add(-t, 3.0)
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

    /// **A sea too gentle to break still foams nowhere — SEA-REBUILD §4.3's
    /// guard rail, asserted.**
    ///
    /// `break_wander` raises both foam thresholds together and is never
    /// negative, so `WATER_FOAM_WET` keeps the meaning every `water@x,z.foam`
    /// assertion in this repository rests on: below it there is no foam, at any
    /// position, at any time. `loom_grass::coverage` makes the same promise the
    /// other way up and CLAUDE.md records that a symmetric version there was
    /// caught by an existing test within a minute; this is that test for the
    /// sea.
    ///
    /// **Seen to fail before it passed, and re-watched when the pair moved to
    /// 0.13 / 0.24.** Centring the wander on its own mean —
    /// `PATCH * (patch - 0.5) + CELL * (cell - 0.5)`, the symmetric version —
    /// makes it reach **-0.0563**, which trips the first assertion; with that
    /// one lifted it reports foam under the gate on **146,098 of 333,396**
    /// grid points. (At the old 0.22 / 0.33 the same break read -0.0303 and
    /// 79,803: the wander's peak is unchanged at 0.115, so lowering `FOAM_WET`
    /// made it a *larger fraction* of the gate and a symmetric version now
    /// leaks nearly twice as widely. The guard matters more than it did, not
    /// less.) A test that has not been watched fail is not a test.
    #[test]
    fn the_gentle_sea_still_foams_nowhere() {
        // A lattice deliberately coprime with nothing in particular but wide
        // enough to cross many periods of both octaves: the patch octave is
        // 28.6 m and the cell octave 0.71 m, so 3.1 m steps over 900 m walk
        // through 31 patches and land on a different phase of the cell octave
        // every time.
        let mut worst = f32::INFINITY;
        let mut foaming = 0_u32;
        let mut points = 0_u32;
        for it in 0_i16..21 {
            let seconds = f32::from(it) * 3.7;
            for iz in 0_i16..63 {
                for ix in 0_i16..63 {
                    let xz = [f32::from(ix) * 3.1 - 97.0, f32::from(iz) * 3.1 - 97.0];
                    worst = worst.min(break_wander(xz, seconds));
                    // Just under the gate, **derived from [`FOAM_WET`] rather
                    // than spelled**: the literals here were `0.219, 0.22`,
                    // which silently stopped straddling the gate the moment
                    // the pair moved down and left the test asserting nothing
                    // about the boundary it exists to guard. `FOAM_WET` itself
                    // is the smoothstep's own zero and would pass on a
                    // symmetric wander too at exactly that value; the rung
                    // below it is the one that catches a negative wander.
                    for mu in [0.0, FOAM_WET * 0.5, FOAM_WET - 0.001, FOAM_WET] {
                        points += 1;
                        if instant_foam(mu, xz, seconds) > 0.0 {
                            foaming += 1;
                        }
                    }
                }
            }
        }
        assert!(
            worst >= 0.0,
            "the wander went negative ({worst}), so it can LOWER the breaking \
             threshold and a sea too gentle to break would foam"
        );
        assert_eq!(
            foaming, 0,
            "{foaming} of {points} points under WATER_FOAM_WET drew foam"
        );
    }
}
