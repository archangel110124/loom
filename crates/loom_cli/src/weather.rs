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
use loom_scene::components::WaterBody;

/// The one mapping from the authored scalars to the field.
fn wind_from(authored: Option<&serde_json::Value>) -> Wind {
    let Some(authored) = authored
        .and_then(|value| serde_json::from_value::<loom_scene::components::Wind>(value.clone()).ok())
    else {
        return Wind::default();
    };

    Wind::new(
        authored.direction_degrees,
        authored.speed,
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
    )
}

/// The same wind, read from a loaded world.
///
/// The simulation has a `World` and no `Scene` — and the sea a crate floats on
/// has to be the sea that is drawn, so both paths resolve the weather through
/// the same function rather than through two readings of the same fields.
#[must_use]
pub(crate) fn wind_of_world(world: &World) -> Wind {
    wind_from(world.wind())
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
        body.waves = loom_water::spectrum::wave_set(
            // U10, which is what the spectrum is written against — never
            // `Wind::speed`, which is a free-stream value about 10% above it.
            wind.mean_speed_at(10.0),
            [params.get("dir_x"), params.get("dir_z")],
        );
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
