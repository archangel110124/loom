//! The `.loom` scene format: components, parsing, serialization.
//!
//! Depends only on `loom_reflect` (see `scripts/check-deps.sh`).

pub mod components;
pub mod edit;
pub mod ops;
pub mod place;
mod scene;

pub use edit::{SaveRejected, Session};
pub use ops::{Applied, SceneOp, Transaction, TransactionError, VersionToken, apply};
pub use place::{Anchor, Axis, PlaceOp};
pub use scene::{Node, Scene, SceneError};

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICE: &str = include_str!("../../../assets/test/office.loom");
    const BAD_INTENSITY: &str = include_str!("../../../assets/test/bad_intensity.loom");

    /// Node paths include the root and are slash-separated (§3).
    #[test]
    fn parse_builds_node_paths_from_the_root() {
        let scene = Scene::parse(OFFICE).expect("fixture is valid");

        let paths: Vec<&str> = scene.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths,
            ["Office", "Office/Desk", "Office/Desk/DeskLamp", "Office/CeilingLight"]
        );
    }

    /// **The M1 exit criterion.** A canonical file round-trips byte-identically,
    /// comments and all.
    #[test]
    fn canonical_scene_round_trips_byte_identically() {
        let scene = Scene::parse(OFFICE).expect("fixture is valid");

        assert_eq!(scene.to_loom_string(), OFFICE);
    }

    /// **The other half of the M1 exit criterion.** The rejection names the
    /// node, the field, the value, and the constraint.
    #[test]
    fn out_of_range_field_is_reported_with_its_node_path() {
        let errs = Scene::parse(BAD_INTENSITY).expect_err("intensity is 40000");

        assert_eq!(errs.len(), 1, "one violation, got: {errs:#?}");
        let e = &errs[0];
        assert_eq!(e.error, "field_out_of_range");
        assert_eq!(e.node, "Office/CeilingLight");
        assert_eq!(e.field, "Light.intensity");
        assert_eq!(e.constraint, "0.0..=10000.0");
        assert!(e.hint.is_some(), "the doc comment should reach the agent");
    }

    /// Godot's one-root rule — it is what makes scenes composable as instances.
    #[test]
    fn two_roots_is_an_error() {
        let src = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"A\"

[[node]]
name = \"B\"
";
        let errs = Scene::parse(src).expect_err("two nodes omit `parent`");

        assert_eq!(errs[0].error, "multiple_roots");
    }

    #[test]
    fn duplicate_sibling_names_are_an_error() {
        let src = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Root\"

[[node]]
name = \"Dup\"
parent = \"Root\"

[[node]]
name = \"Dup\"
parent = \"Root\"
";
        let errs = Scene::parse(src).expect_err("two siblings named Dup");

        assert_eq!(errs[0].error, "duplicate_sibling_name");
    }

    /// Forward references are rejected so the file reads top-to-bottom and
    /// cycles are unrepresentable rather than merely detected (§3).
    #[test]
    fn forward_parent_reference_is_an_error() {
        let src = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Root\"

[[node]]
name = \"Child\"
parent = \"Root/Later\"

[[node]]
name = \"Later\"
parent = \"Root\"
";
        let errs = Scene::parse(src).expect_err("parent declared after child");

        assert_eq!(errs[0].error, "unknown_parent");
    }

    #[test]
    fn a_newer_format_version_is_refused_rather_than_guessed_at() {
        let src = "\
[scene]
format = 99
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Root\"
";
        let errs = Scene::parse(src).expect_err("format 99 is from the future");

        assert_eq!(errs[0].error, "format_version_unsupported");
    }

    /// M1 specified six; `RigidBody` joined them at M7 when physics needed a
    /// way to say "this one falls".
    #[test]
    fn every_component_is_registered() {
        let reg = components::registry();

        for name in [
            "Name",
            "Transform",
            "MeshRenderer",
            "BoxCollider",
            "Light",
            "Script",
            "RigidBody",
        ] {
            assert!(reg.describe(name).is_some(), "{name} is not registered");
        }
    }

    /// `[[component.list]]` is an array of tables, which is neither an array
    /// nor a table to `toml_edit`. Missing that case dropped a voxel volume's
    /// entire op list — the scene parsed, validated, and rendered nothing.
    #[test]
    fn a_component_keeps_its_array_of_tables() {
        let src = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Hill\"

  [node.components.VoxelVolume]
  voxel_size = 0.25

  [[node.components.VoxelVolume.ops]]
  kind = \"sphere\"
  radius = 8.5

  [[node.components.VoxelVolume.ops]]
  kind = \"capsule\"
  radius = 2.0
";
        let scene = Scene::parse(src).expect("valid");
        let ops = scene.nodes()[0].components["VoxelVolume"]["ops"]
            .as_array()
            .expect("ops must survive parsing");

        assert_eq!(ops.len(), 2, "both ops must be there");
        assert_eq!(ops[0]["kind"], "sphere");
        assert_eq!(ops[1]["kind"], "capsule");
    }

    /// Scale defaults to 1, not 0. A zero-scale default silently collapses
    /// every node that omits the field — the classic version of this bug.
    #[test]
    fn transform_defaults_to_identity() {
        let t = components::Transform::default();

        assert_eq!(t.pos, [0.0, 0.0, 0.0]);
        assert_eq!(t.rot_euler, [0.0, 0.0, 0.0]);
        assert_eq!(t.scale, [1.0, 1.0, 1.0]);
    }

    /// `docs/format/README.md` §6: the rejection names field, value, and
    /// constraint. Light is the component that carries a real range.
    #[test]
    fn light_intensity_is_range_checked() {
        let reg = components::registry();

        let errs = reg
            .validate("Light", &serde_json::json!({ "intensity": 40000.0 }))
            .expect_err("40000 exceeds the declared maximum");

        assert_eq!(errs[0].field, "Light.intensity");
        assert_eq!(errs[0].constraint, "0.0..=10000.0");
    }

    /// Colour channels are normalized 0..=1. An agent writing 255 is a real
    /// and very likely mistake, so it must be caught rather than clamped.
    #[test]
    fn light_colour_channels_are_range_checked() {
        let reg = components::registry();

        assert!(
            reg.validate("Light", &serde_json::json!({ "color": [255.0, 0.0, 0.0] }))
                .is_err(),
            "255 is out of the 0..=1 channel range"
        );
        assert!(
            reg.validate("Light", &serde_json::json!({ "color": [1.0, 0.92, 0.78] }))
                .is_ok()
        );
    }
}
