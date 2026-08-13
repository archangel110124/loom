//! Physical sanity checks, run at authoring time.
//!
//! The table is graphics doc §C.5, and the reasoning is worth restating:
//! Erin Catto names uncontrolled-content physics sandboxes as an unsolved
//! problem, and **an AI agent authoring scenes is exactly an uncontrolled
//! content generator**. It produces the configurations solvers handle worst
//! and has no way to notice, because nothing in a text file looks unusual.
//!
//! Each check costs an afternoon and prevents a class of unattributable "the
//! physics is broken" reports. Each one is also a message that teaches the
//! agent something a render cannot.

use loom_scene::Scene;
use serde::Serialize;

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Legal, but a known way to make a solver misbehave.
    Warning,
    /// Almost certainly a mistake; the scene will not behave.
    Error,
}

/// One physical problem with a scene.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub severity: Severity,
    /// Machine-readable code, matching the format spec's error style.
    pub error: String,
    pub node: String,
    pub constraint: String,
    /// What to do about it. A rejection is the agent's teacher (§6).
    pub hint: String,
}

/// Colliders outside this range wreck floating-point contact generation.
const MIN_EXTENT: f32 = 0.01;
const MAX_EXTENT: f32 = 100.0;

/// Check a scene's physical plausibility.
///
/// Returns findings rather than a pass/fail, because most are warnings: an
/// unusual scene is not an invalid one, and refusing to load it would be worse
/// than telling the author what is odd about it.
#[must_use]
pub fn check_scene(scene: &Scene) -> Vec<Finding> {
    let mut findings = Vec::new();

    for node in scene.nodes() {
        let collider = node.components.get("BoxCollider");
        let scale = node.transform.scale;

        if let Some(collider) = collider {
            let half = collider
                .get("half_extents")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_f64())
                        .map(|v| v as f32)
                        .collect::<Vec<f32>>()
                })
                .unwrap_or_default();

            for (axis, value) in half.iter().enumerate() {
                // The collider is scaled by the node, so the *effective* size
                // is what matters — a 0.5 collider on a 0.001 scale is still
                // degenerate, and checking the authored number alone misses it.
                let effective = value * scale.get(axis).copied().unwrap_or(1.0).abs();
                if effective < MIN_EXTENT {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        error: "degenerate_collider".to_owned(),
                        node: node.path.clone(),
                        constraint: format!("half extent {effective} on axis {axis} < {MIN_EXTENT}"),
                        hint: "Very thin colliders wreck contact generation and tunnel. \
                               Thicken it, or enable CCD if it must stay thin."
                            .to_owned(),
                    });
                } else if effective > MAX_EXTENT {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        error: "oversized_collider".to_owned(),
                        node: node.path.clone(),
                        constraint: format!("half extent {effective} on axis {axis} > {MAX_EXTENT}"),
                        hint: "Very large colliders lose precision. Split it into several."
                            .to_owned(),
                    });
                }
            }
        }

        // **The pontoon-count trap (water doc §5.6), both halves of it.** A
        // `Buoyancy` that lists its own pontoons can get either wrong, and
        // both failures look like the engine misbehaving rather than like the
        // scene being wrong. An empty list is not checked: it means "four,
        // sized to this body", which is correct by construction.
        if let Some(pontoons) = node
            .components
            .get("Buoyancy")
            .and_then(|b| b.get("pontoons"))
            .and_then(|v| v.as_array())
            .filter(|list| !list.is_empty())
        {
            // A sphere is the one shape one pontoon describes exactly, and a
            // body with no orientation has nothing to right — so warning about
            // it would be noise.
            let spherical = node
                .components
                .get("MeshRenderer")
                .and_then(|m| m.get("mesh"))
                .and_then(|m| m.get("asset"))
                .and_then(|a| a.as_str())
                == Some("sphere");
            if pontoons.len() == 1 && !spherical {
                findings.push(Finding {
                    severity: Severity::Warning,
                    error: "single_pontoon".to_owned(),
                    node: node.path.clone(),
                    constraint: "1 pontoon gives no righting torque".to_owned(),
                    hint: "One sphere puts the whole buoyant force through one \
                           point, so the body spins freely instead of sitting \
                           upright. Use four, or omit `pontoons` to get four \
                           sized to this node."
                        .to_owned(),
                });
            }

            // Radii that *look* right around a bounding box total several times
            // the object's volume, and then it floats like a cork with its deck
            // in the air. Compared against the box the node actually is.
            let displaced: f32 = pontoons
                .iter()
                .filter_map(|p| p.get("radius").and_then(|v| v.as_f64()))
                .map(|r| 4.0 / 3.0 * std::f32::consts::PI * (r as f32).powi(3))
                .sum();
            let half = collider.and_then(|c| c.get("half_extents")).map_or_else(
                || [scale[0].abs(), scale[1].abs(), scale[2].abs()],
                |v| {
                    let read = |i: usize| {
                        v.get(i).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32
                    };
                    [
                        read(0).abs() * scale[0].abs(),
                        read(1).abs() * scale[1].abs(),
                        read(2).abs() * scale[2].abs(),
                    ]
                },
            );
            let body_volume = 8.0 * half[0] * half[1] * half[2];
            if body_volume > 0.0 && displaced > body_volume * 2.5 {
                findings.push(Finding {
                    severity: Severity::Warning,
                    error: "pontoon_volume_mismatch".to_owned(),
                    node: node.path.clone(),
                    constraint: format!(
                        "pontoons displace {displaced:.2} m³ against a body of {body_volume:.2} m³"
                    ),
                    hint: "Pontoon radii should sum to roughly the object's own \
                           volume, not its bounding sphere's. Too much and it \
                           floats like a cork with its deck in the air. Omitting \
                           `pontoons` derives four that displace exactly this body."
                        .to_owned(),
                });
            }
        }

        // A node scaled to nothing has geometry that cannot be collided with
        // or seen, which is nearly always a mistake rather than intent.
        if scale.iter().any(|s| s.abs() < 1e-4) {
            findings.push(Finding {
                severity: Severity::Error,
                error: "degenerate_scale".to_owned(),
                node: node.path.clone(),
                constraint: format!("scale {scale:?} has a near-zero axis"),
                hint: "A zero-scaled axis flattens the node to nothing. \
                       Did you mean to omit `scale` and take the default of 1?"
                    .to_owned(),
            });
        }

        // Negative scale mirrors geometry, which inverts winding and turns
        // every face inside out. Visible as "the model is inside out".
        if scale.iter().any(|s| *s < 0.0) {
            findings.push(Finding {
                severity: Severity::Warning,
                error: "mirrored_scale".to_owned(),
                node: node.path.clone(),
                constraint: format!("scale {scale:?} has a negative axis"),
                hint: "Negative scale inverts triangle winding, so faces render \
                       inside out and normals point the wrong way. Rotate instead."
                    .to_owned(),
            });
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(body: &str) -> Scene {
        let src = format!(
            "[scene]\nformat = 1\nid = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"\n\n\
             [[node]]\nname = \"Root\"\n\n{body}"
        );
        Scene::parse(&src).expect("fixture should parse")
    }

    #[test]
    fn a_healthy_scene_reports_nothing() {
        let scene = scene(
            "[[node]]\nname = \"Crate\"\nparent = \"Root\"\n\
             transform = { scale = [1.0, 1.0, 1.0] }\n\n\
               [node.components.BoxCollider]\n  half_extents = [0.5, 0.5, 0.5]\n",
        );

        assert!(check_scene(&scene).is_empty());
    }

    /// The check must use the *effective* size. A reasonable collider on a
    /// tiny scale is still degenerate, and checking the authored number alone
    /// would miss exactly the case an agent produces by scaling a prefab down.
    #[test]
    fn a_collider_scaled_to_nothing_is_caught() {
        let scene = scene(
            "[[node]]\nname = \"Sliver\"\nparent = \"Root\"\n\
             transform = { scale = [0.001, 1.0, 1.0] }\n\n\
               [node.components.BoxCollider]\n  half_extents = [0.5, 0.5, 0.5]\n",
        );

        let findings = check_scene(&scene);
        assert!(
            findings.iter().any(|f| f.error == "degenerate_collider"),
            "effective extent 0.0005 should be caught, got {findings:#?}"
        );
    }

    #[test]
    fn a_zero_scaled_node_is_an_error() {
        let scene = scene(
            "[[node]]\nname = \"Flat\"\nparent = \"Root\"\n\
             transform = { scale = [1.0, 0.0, 1.0] }\n",
        );

        let findings = check_scene(&scene);
        let finding = findings
            .iter()
            .find(|f| f.error == "degenerate_scale")
            .expect("zero scale should be reported");
        assert_eq!(finding.severity, Severity::Error);
        assert!(finding.hint.contains("default of 1"));
    }

    #[test]
    fn a_mirrored_scale_is_warned_about() {
        let scene = scene(
            "[[node]]\nname = \"Mirror\"\nparent = \"Root\"\n\
             transform = { scale = [-1.0, 1.0, 1.0] }\n",
        );

        let findings = check_scene(&scene);
        assert!(findings.iter().any(|f| f.error == "mirrored_scale"));
    }

    /// §5.6, first half. One pontoon is legal and always wrong: the object
    /// spins because nothing gives it a righting torque.
    #[test]
    fn a_single_pontoon_is_warned_about() {
        let scene = scene(
            "[[node]]\nname = \"Raft\"\nparent = \"Root\"\n\
             transform = { scale = [1.0, 0.3, 1.0] }\n\n\
               [node.components.Buoyancy]\n  \
               pontoons = [{ offset = [0.0, 0.0, 0.0], radius = 0.6 }]\n",
        );

        let findings = check_scene(&scene);
        assert!(
            findings.iter().any(|f| f.error == "single_pontoon"),
            "{findings:#?}"
        );
    }

    /// §5.6, second half. Four spheres drawn around the corners of a bounding
    /// box displace far more than the box does, and the object then floats far
    /// too high — which reads as a physics bug rather than an authoring one.
    #[test]
    fn pontoons_that_displace_far_more_than_the_body_are_warned_about() {
        let scene = scene(
            "[[node]]\nname = \"Cork\"\nparent = \"Root\"\n\
             transform = { scale = [0.5, 0.5, 0.5] }\n\n\
               [node.components.Buoyancy]\n  pontoons = [\
               { offset = [-0.5, 0.0, -0.5], radius = 0.7 }, \
               { offset = [0.5, 0.0, -0.5], radius = 0.7 }, \
               { offset = [-0.5, 0.0, 0.5], radius = 0.7 }, \
               { offset = [0.5, 0.0, 0.5], radius = 0.7 }]\n",
        );

        let findings = check_scene(&scene);
        assert!(
            findings.iter().any(|f| f.error == "pontoon_volume_mismatch"),
            "{findings:#?}"
        );
    }

    /// And four sensible ones on a crate say nothing, or the check is noise.
    #[test]
    fn four_well_sized_pontoons_report_nothing() {
        let scene = scene(
            "[[node]]\nname = \"Crate\"\nparent = \"Root\"\n\
             transform = { scale = [0.5, 0.5, 0.5] }\n\n\
               [node.components.Buoyancy]\n  pontoons = [\
               { offset = [-0.25, 0.0, -0.25], radius = 0.39 }, \
               { offset = [0.25, 0.0, -0.25], radius = 0.39 }, \
               { offset = [-0.25, 0.0, 0.25], radius = 0.39 }, \
               { offset = [0.25, 0.0, 0.25], radius = 0.39 }]\n",
        );

        assert!(check_scene(&scene).is_empty());
    }

    #[test]
    fn an_oversized_collider_is_warned_about() {
        let scene = scene(
            "[[node]]\nname = \"Huge\"\nparent = \"Root\"\n\
             transform = { scale = [1.0, 1.0, 1.0] }\n\n\
               [node.components.BoxCollider]\n  half_extents = [500.0, 1.0, 1.0]\n",
        );

        let findings = check_scene(&scene);
        assert!(findings.iter().any(|f| f.error == "oversized_collider"));
    }
}
