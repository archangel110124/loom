//! The `.loom` scene format: components, parsing, serialization.
//!
//! Depends only on `loom_reflect` (see `scripts/check-deps.sh`).

pub mod components;
pub mod edit;
pub mod ops;
pub mod place;
mod scene;

pub use edit::{FileApplyError, SaveRejected, Session, apply_to_file, write_atomically};
pub use ops::{Applied, SceneOp, Transaction, TransactionError, VersionToken, apply};
pub use place::{Anchor, Axis, PlaceOp};
pub use scene::{Node, Scene, SceneError};

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICE: &str = include_str!("../../../assets/test/office.loom");
    const BAD_INTENSITY: &str = include_str!("../../../assets/test/bad_intensity.loom");

    /// A one-node scene with the `transform` line left for the test to append.
    const TRANSFORM_SCENE: &str = "\
[scene]
format = 1
id = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f\"

[[node]]
name = \"Root\"
";

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

    /// The `transform` key took a different route from every other component:
    /// straight through serde with `.ok().unwrap_or_default()`. So a guessed
    /// key name was ignored, a malformed element threw the *whole* transform
    /// away including its valid fields, and both produced a node at the origin
    /// with `ok: true`. `position` for `pos` is the single most likely guess an
    /// agent makes, and nothing in the pipeline corrected it.
    #[test]
    fn a_misspelled_transform_key_is_rejected_not_silently_ignored() {
        let scene = format!("{TRANSFORM_SCENE}transform = {{ position = [1.0, 2.0, 3.0] }}\n");

        let errs = Scene::parse(&scene).expect_err("`position` is not a transform field");

        assert_eq!(errs[0].error, "unknown_field");
        assert_eq!(errs[0].field, "Transform.position");
        assert_eq!(errs[0].node, "Root");
        let hint = errs[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("pos"), "should name the real field: {hint}");
    }

    /// A two-element position is not a position — and the old code discarded
    /// the node's rotation and scale along with it.
    #[test]
    fn a_malformed_transform_does_not_silently_become_the_identity() {
        let scene =
            format!("{TRANSFORM_SCENE}transform = {{ pos = [1.0, 2.0], scale = [2.0, 2.0, 2.0] }}\n");

        let errs = Scene::parse(&scene).expect_err("a position has three components");

        assert_eq!(errs[0].error, "field_type_mismatch");
        assert_eq!(errs[0].field, "Transform.pos");
    }

    /// §1: non-finite floats "poison the determinism hashes M3 depends on", and
    /// §6 makes `non_finite_float` a normative code. TOML happily parses `nan`
    /// and `inf`; JSON cannot represent either, so they became `null` and were
    /// skipped — `mass = nan` was accepted where `mass = 0.0` is correctly
    /// rejected. The check has to happen while the value is still a TOML float.
    #[test]
    fn nan_and_infinity_are_rejected() {
        for bad in ["nan", "inf", "-inf"] {
            let scene = format!("{TRANSFORM_SCENE}transform = {{ pos = [1.0, {bad}, 3.0] }}\n");

            let Err(errs) = Scene::parse(&scene) else {
                panic!("{bad} must be rejected, not silently dropped")
            };
            assert_eq!(errs[0].error, "non_finite_float", "for {bad}: {errs:?}");
            assert_eq!(errs[0].field, "Transform.pos[1]", "for {bad}");
        }
    }

    /// Valid scenes must keep parsing — rejecting more is only correct if it
    /// rejects nothing that was always legal.
    #[test]
    fn well_formed_transforms_still_parse() {
        let scene = format!(
            "{TRANSFORM_SCENE}transform = {{ pos = [1.0, 2.0, 3.0], rot_euler = [0.0, 90.0, 0.0] }}\n"
        );

        let parsed = Scene::parse(&scene).expect("this is a valid transform");
        assert_eq!(parsed.nodes()[0].transform.pos, [1.0, 2.0, 3.0]);
        // Integers coerce to floats where a float is expected (§7).
        let ints = format!("{TRANSFORM_SCENE}transform = {{ pos = [1, 2, 3] }}\n");
        assert!(Scene::parse(&ints).is_ok(), "§7: integers coerce to floats");
    }

    /// `[node.components.Transform]` is the spelling an agent reaches for after
    /// seeing every other component written that way, and it used to validate
    /// clean while being invisible to the ECS, the renderer, physics and
    /// `measure` — the node simply stayed at the origin. The node key
    /// `transform` is the one that means anything, so say so rather than
    /// accepting a second spelling that silently does nothing.
    #[test]
    fn the_transform_component_spelling_is_refused_and_points_at_the_node_key() {
        let scene = format!("{TRANSFORM_SCENE}\n  [node.components.Transform]\n  pos = [5.0, 0.0, 0.0]\n");

        let Err(errs) = Scene::parse(&scene) else {
            panic!("this spelling silently does nothing, so it must be refused")
        };

        assert_eq!(errs[0].error, "unknown_component_type");
        let hint = errs[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("transform ="), "point at the real key: {hint}");
    }

    /// §5 of the format spec describes prefab instances, `[node.overrides]`
    /// and `extends`. **None of it is implemented**, and the parser did not
    /// know the words — it ignored them. So a prefab instance node became a
    /// node with no components at all: it drew nothing and lit nothing, and
    /// the scene validated clean. The spec's own flagship fixture depended on
    /// it, which is how it went unnoticed.
    ///
    /// Refusing them is not a decision to never build prefabs; it is a refusal
    /// to accept text that means nothing today.
    #[test]
    fn an_unimplemented_prefab_key_is_refused_rather_than_ignored() {
        for line in ["prefab = \"lamp\"", "extends = \"base\""] {
            let scene = format!("{TRANSFORM_SCENE}{line}\n");

            let Err(errs) = Scene::parse(&scene) else {
                panic!("`{line}` does nothing today, so it must not be accepted")
            };
            assert_eq!(errs[0].error, "not_implemented", "for {line}: {errs:?}");
            let hint = errs[0].hint.as_deref().unwrap_or_default();
            assert!(hint.contains("§5"), "point at the spec section: {hint}");
        }
    }

    #[test]
    fn an_overrides_table_is_refused() {
        let scene =
            format!("{TRANSFORM_SCENE}\n  [node.overrides]\n  \"Light.intensity\" = 420.0\n");

        let Err(errs) = Scene::parse(&scene) else {
            panic!("overrides do nothing today, so they must not be accepted")
        };
        assert_eq!(errs[0].error, "not_implemented");
    }

    /// §7: the three Vec3 spellings are "accepted interchangeably", and the
    /// spec's own rationale is that this "removes a whole category of agent
    /// retry loops for the cost of a few `From` impls".
    ///
    /// Only the array form was ever built. Before the validator was tightened
    /// the other two deserialized to nothing and hit `unwrap_or_default()`, so
    /// the node silently sat at the origin with `ok: true`; afterwards they
    /// were a loud type error. Neither is what §7 promises.
    #[test]
    fn the_three_vec3_spellings_all_mean_the_same_thing() {
        let forms = [
            "[0.0, 1.0, 0.0]",
            "{ x = 0.0, y = 1.0, z = 0.0 }",
            "\"Vec3(0, 1, 0)\"",
        ];

        for form in forms {
            let scene = format!("{TRANSFORM_SCENE}transform = {{ pos = {form} }}\n");

            let parsed = Scene::parse(&scene)
                .unwrap_or_else(|e| panic!("§7 accepts `{form}` for a Vec3: {e:?}"));

            assert_eq!(parsed.nodes()[0].transform.pos, [0.0, 1.0, 0.0], "for {form}");
        }
    }

    /// The coercion must not swallow a genuinely malformed Vec3, or it has
    /// traded a loud error back for the silent default this all started with.
    #[test]
    fn a_vec3_lookalike_that_is_not_one_is_still_rejected() {
        for bad in [
            "{ x = 0.0, y = 1.0 }",           // missing z
            "{ x = 0.0, y = 1.0, w = 2.0 }",  // wrong key
            "\"Vec3(0, 1)\"",                 // two components
            "\"Vec3(a, b, c)\"",              // not numbers
        ] {
            let scene = format!("{TRANSFORM_SCENE}transform = {{ pos = {bad} }}\n");

            assert!(
                Scene::parse(&scene).is_err(),
                "`{bad}` is not a Vec3 and must not quietly become the default"
            );
        }
    }

    /// A voxel volume's op list is `[[node.components.VoxelVolume.ops]]` — an
    /// array of *tables*, which is neither `as_array` nor `as_table_like`, so
    /// the finiteness walk did not descend into it. A `radius = nan` in there
    /// validated clean and was then dropped, silently changing the recipe.
    ///
    /// never-do #11 is that the scene stores the recipe and never the voxels,
    /// which only works if replaying the recipe is reproducible. A NaN that
    /// disappears at load means the file no longer describes the geometry.
    #[test]
    fn a_non_finite_inside_a_voxel_op_list_is_rejected() {
        let cave = std::fs::read_to_string("../../assets/test/cave.loom").expect("fixture");
        let poisoned = cave.replacen("radius = 2.0", "radius = nan", 1);
        assert_ne!(poisoned, cave, "the fixture should contain that op");

        let Err(errs) = Scene::parse(&poisoned) else {
            panic!("a NaN in the op list must not validate")
        };
        assert_eq!(errs[0].error, "non_finite_float");
        assert!(errs[0].field.contains("ops"), "name the op list: {}", errs[0].field);
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
    /// way to say "this one falls", and `Material` when shading stopped being
    /// a per-object debug colour.
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
            "Material",
            "ParticleEmitter",
        ] {
            assert!(reg.describe(name).is_some(), "{name} is not registered");
        }
    }

    /// Roughness outside 0..=1 is the mistake that makes a surface render
    /// black with no other symptom, so the schema has to catch it rather than
    /// the shader clamping it silently.
    #[test]
    fn a_material_outside_its_range_is_rejected() {
        let reg = components::registry();

        let errs = reg
            .validate("Material", &serde_json::json!({ "roughness": 4.0 }))
            .expect_err("roughness is a 0..=1 fraction");

        assert_eq!(errs[0].error, "field_out_of_range");
    }

    /// Albedo is a linear factor, not 0-255. Same trap as `Light::color`,
    /// which is why it carries the same range.
    #[test]
    fn an_albedo_in_bytes_is_rejected() {
        let reg = components::registry();

        assert!(
            reg.validate("Material", &serde_json::json!({ "albedo": [255.0, 0.0, 0.0] }))
                .is_err(),
            "255 is the 0-255 spelling, not a linear factor"
        );
        assert!(
            reg.validate("Material", &serde_json::json!({ "albedo": [0.8, 0.2, 0.2] }))
                .is_ok()
        );
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
