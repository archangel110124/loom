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
