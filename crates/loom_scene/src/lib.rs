//! The `.loom` scene format: components, parsing, serialization.
//!
//! Depends only on `loom_reflect` (see `scripts/check-deps.sh`).

pub mod components;
pub mod edit;
pub mod ops;
pub mod place;
pub mod prefab;
mod scene;

pub use edit::{FileApplyError, SaveRejected, Session, apply_to_file, write_atomically};
pub use ops::{Applied, SceneOp, Transaction, TransactionError, VersionToken, apply};
pub use place::{Anchor, Axis, PlaceOp};
pub use scene::{Node, PrefabDecl, Scene, SceneError};

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

    /// Inheritance is a property of the *scene*, so only the root extends. A
    /// mid-tree `extends` would be a prefab instance spelled differently, and
    /// two spellings for one thing is how a format grows a dialect.
    #[test]
    fn extends_on_a_child_node_is_refused() {
        let scene = format!(
            "[[prefab]]\nkey = \"base\"\nid = \"p-1\"\npath = \"base.loom\"\n\n\
             {TRANSFORM_SCENE}\n[[node]]\nname = \"Child\"\nparent = \"Root\"\n\
             extends = \"base\"\n"
        );

        let Err(errs) = Scene::parse(&scene) else {
            panic!("`extends` below the root must not parse")
        };

        assert_eq!(errs[0].error, "extends_on_a_child", "{errs:?}");
        let hint = errs[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("prefab"), "point at the alternative: {hint}");
    }

    /// An undeclared alias is named the same way a prefab's is.
    #[test]
    fn an_undeclared_extends_alias_is_refused() {
        let scene = format!("{TRANSFORM_SCENE}extends = \"nowhere\"\n");

        let Err(errs) = Scene::parse(&scene) else { panic!("nothing declares it") };

        assert_eq!(errs[0].error, "unresolved_prefab", "{errs:?}");
    }

    /// A prefab alias has to be declared, and the message lists what is —
    /// the §3 rule for asset aliases, applied to prefabs.
    #[test]
    fn an_undeclared_prefab_alias_names_the_declared_ones() {
        let scene = format!(
            "[[prefab]]\nkey = \"lamp\"\nid = \"p-1\"\npath = \"lamp.loom\"\n\n\
             {TRANSFORM_SCENE}prefab = \"lampp\"\n"
        );

        let Err(errs) = Scene::parse(&scene) else { panic!("a typo'd alias must not parse") };

        assert_eq!(errs[0].error, "unresolved_prefab", "{errs:?}");
        let hint = errs[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("lamp"), "list what is declared: {hint}");
    }

    /// **Two sources for one component, with no rule about which wins.** An
    /// instance takes its components from the prefab; deviations go in
    /// `overrides` and nowhere else.
    #[test]
    fn a_prefab_instance_may_not_also_declare_components() {
        let scene = format!(
            "[[prefab]]\nkey = \"lamp\"\nid = \"p-1\"\npath = \"lamp.loom\"\n\n\
             {TRANSFORM_SCENE}prefab = \"lamp\"\n\n  \
             [node.components.Light]\n  intensity = 5.0\n"
        );

        let Err(errs) = Scene::parse(&scene) else {
            panic!("components on an instance must not parse")
        };

        assert_eq!(errs[0].error, "prefab_instance_has_components", "{errs:?}");
    }

    /// Overrides deviate from a prefab. Without one there is nothing to
    /// deviate from, and the fields belong on the node directly.
    #[test]
    fn overrides_without_a_prefab_are_refused() {
        let scene =
            format!("{TRANSFORM_SCENE}\n  [node.overrides]\n  \"Light.intensity\" = 420.0\n");

        let Err(errs) = Scene::parse(&scene) else {
            panic!("overrides with no prefab must not parse")
        };

        assert_eq!(errs[0].error, "overrides_without_prefab", "{errs:?}");
    }

    /// The key grammar is `[Child/Path::]TypeName.field`. A malformed key is
    /// wrong in the file whatever prefab it points at, so it is caught at
    /// parse rather than left to resolution.
    #[test]
    fn a_malformed_override_key_is_named() {
        for key in ["Light", "Light.", ".intensity", "::Light.intensity", "a//b::L.f"] {
            let scene = format!(
                "[[prefab]]\nkey = \"lamp\"\nid = \"p-1\"\npath = \"lamp.loom\"\n\n\
                 {TRANSFORM_SCENE}prefab = \"lamp\"\n\n  \
                 [node.overrides]\n  \"{key}\" = 1.0\n"
            );

            let Err(errs) = Scene::parse(&scene) else { panic!("`{key}` must not parse") };

            assert_eq!(errs[0].error, "malformed_override_key", "for `{key}`: {errs:?}");
            assert_eq!(errs[0].field, key);
        }
    }

    /// The shape the spec's own §5 example uses, end to end through the
    /// parser: a declaration, an instance, a transform, and two overrides.
    #[test]
    fn the_spec_section_five_example_parses() {
        let scene = format!(
            "[[prefab]]\nkey = \"lamp\"\nid = \"p-1\"\npath = \"lamp.loom\"\n\n\
             {TRANSFORM_SCENE}prefab = \"lamp\"\n\n  \
             [node.overrides]\n  \"Light.intensity\" = 420.0\n  \
             \"Bulb/Glass::Material.color\" = [1.0, 0.92, 0.78]\n"
        );

        let parsed = Scene::parse(&scene).expect("the §5 shape must parse");

        let node = parsed.nodes().last().expect("a node");
        assert_eq!(node.prefab.as_deref(), Some("lamp"));
        assert_eq!(node.overrides.len(), 2);
        assert_eq!(parsed.prefab_id("lamp").as_deref(), Some("p-1"));
        // Round-trip is byte-identical, prefab keys included.
        assert_eq!(parsed.to_loom_string(), scene);
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
            "Camera",
            "CharacterController",
            "Blast",
            "GameRules",
            "Hud",
            "AudioSource",
            "Environment",
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

    /// A scene with one water body, whose wave table the test supplies.
    fn water_scene(waves: &str) -> String {
        format!(
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f\"\n\n\
             [[node]]\nname = \"Ocean\"\n\n\
             [node.components.WaterBody]\nsurface_height = 0.0\n{waves}"
        )
    }

    /// A sea an agent could plausibly author is accepted. The steepness limit
    /// has to leave room for real water or it is just a ban on waves.
    #[test]
    fn an_ordinary_sea_validates() {
        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 18.0\namplitude = 0.55\nsteepness = 0.7\ndirection = [1.0, 0.2]\n\n\
             [[node.components.WaterBody.waves.waves]]\n\
             wavelength = 7.0\namplitude = 0.18\nsteepness = 0.6\ndirection = [0.8, -0.6]\n",
        );

        Scene::parse(&src).expect("an ordinary two-wave sea must validate");
    }

    /// **§5.3, the trap that reads as a rendering bug.** Too much steepness and
    /// the wave loops through itself; the rejection has to carry the computed
    /// limit, because "too steep" without a number is not something an agent
    /// tuning choppiness can act on.
    #[test]
    fn an_over_steep_wave_is_rejected_with_its_computed_limit() {
        // A metre of amplitude on a four-metre wave: k = 2π/4 = 1.571, so
        // Q·k·A = 1.571 > 1 and the surface folds.
        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 4.0\namplitude = 1.0\nsteepness = 1.0\ndirection = [1.0, 0.0]\n",
        );

        let errors = Scene::parse(&src).expect_err("Q*k*A = 1.57 > 1 is a folded surface");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "wave_steepness_exceeds_limit");
        assert_eq!(errors[0].node, "Ocean");
        assert_eq!(errors[0].field, "WaterBody.waves.waves[0].steepness");
        assert_eq!(errors[0].value, serde_json::json!(1.0));
        // The limit is 1/(N·k·A) = 0.637, and the message has to say so.
        assert!(errors[0].constraint.starts_with("at most 0.637"), "{errors:?}");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("1/N")),
            "{errors:?}"
        );
    }

    /// **The limit is shared out, so adding waves tightens it.** Four copies of
    /// a wave that is fine alone fold four times as hard, which is the multi-
    /// wave half of §5.3 and the half an agent will hit by adding detail.
    #[test]
    fn the_steepness_limit_tightens_as_waves_are_added() {
        // Q·k·A = 0.9 · (2π/12) · 0.5 = 0.236: comfortable alone, over the
        // 1/N budget once there are five of them.
        let wave = "\n[[node.components.WaterBody.waves.waves]]\n\
                    wavelength = 12.0\namplitude = 0.5\nsteepness = 0.9\ndirection = [1.0, 0.0]\n";

        Scene::parse(&water_scene(wave)).expect("one such wave is fine");
        let errors = Scene::parse(&water_scene(&wave.repeat(5)))
            .expect_err("five of them exceed the shared budget");
        assert_eq!(errors.len(), 5, "every offending wave is named: {errors:?}");
        assert_eq!(errors[4].field, "WaterBody.waves.waves[4].steepness");
    }

    /// The cap is 16 (§5.3): per-vertex cost is linear in the count and an
    /// agent has no intuition for that.
    #[test]
    fn more_than_sixteen_waves_is_rejected() {
        let wave = "\n[[node.components.WaterBody.waves.waves]]\n\
                    wavelength = 30.0\namplitude = 0.05\nsteepness = 0.2\ndirection = [1.0, 0.0]\n";

        Scene::parse(&water_scene(&wave.repeat(components::MAX_WAVES))).expect("16 is the cap");
        let errors = Scene::parse(&water_scene(&wave.repeat(components::MAX_WAVES + 1)))
            .expect_err("17 is over it");
        assert_eq!(errors[0].error, "too_many_waves");
        assert_eq!(errors[0].constraint, "at most 16 waves");
    }

    /// A scene with one emitter, whose body the test supplies.
    fn emitter_scene(body: &str) -> String {
        format!(
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e50\"\n\n\
             [[node]]\nname = \"Embers\"\n\n\
             [node.components.ParticleEmitter]\n{body}"
        )
    }

    /// A scene with one cascade, whose body the test supplies, and a
    /// `WaterBody` unless `water` says otherwise.
    fn cascade_scene(body: &str, water: bool) -> String {
        let pool = if water {
            "\n[[node]]\nname = \"Pool\"\nparent = \"Scene\"\n\n\
             [node.components.WaterBody]\nsurface_height = 0.0\n"
        } else {
            ""
        };
        format!(
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e51\"\n\n\
             [[node]]\nname = \"Scene\"\n\n\
             [[node]]\nname = \"Fall\"\nparent = \"Scene\"\n\n\
             [node.components.Cascade]\n{body}{pool}"
        )
    }

    /// The component round-trips its schema with nothing but a discharge, which
    /// is the claim its documentation makes: one number, and the rest is
    /// hydraulics.
    #[test]
    fn a_cascade_needs_only_a_discharge() {
        Scene::parse(&cascade_scene("discharge = 0.10\n", true))
            .expect("a lip, a spread and a break-up all have documented defaults");
    }

    /// **A cascade with nothing to fall into is invisible, not subtle.** The
    /// sheet is drawn inside the water block and the water block is skipped
    /// entirely in a scene with no `WaterBody`, so the whole waterfall silently
    /// does not exist — the least informative failure a scene can produce.
    #[test]
    fn a_cascade_without_water_is_refused() {
        let errors = Scene::parse(&cascade_scene("discharge = 0.10\n", false))
            .expect_err("there is nothing for the fall to end at");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "cascade_needs_water_to_fall_into");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("WaterBody")),
            "the rejection has to name what to add: {errors:?}"
        );
    }

    /// **The environment buffer carries one lip.** A second cascade would
    /// validate and then not draw, which is the silent-no-op the registry and
    /// every rule beside this one exist to stop.
    #[test]
    fn a_second_cascade_is_refused() {
        let scene = format!(
            "{}\n[[node]]\nname = \"Fall2\"\nparent = \"Scene\"\n\n\
             [node.components.Cascade]\ndischarge = 0.5\n",
            cascade_scene("discharge = 0.10\n", true)
        );
        let errors = Scene::parse(&scene).expect_err("one lip fits in the buffer");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "one_cascade_per_scene");
    }

    /// A lip whose ends coincide has no direction for the water to leave along
    /// — `cross(lip, +Y)` is zero — and the strip collapses to a line.
    #[test]
    fn a_cascade_with_a_zero_length_lip_is_refused() {
        let errors = Scene::parse(&cascade_scene(
            "lip_a = [0.0, 0.0, 0.0]\nlip_b = [0.0, 0.0, 0.0]\n",
            true,
        ))
        .expect_err("a lip of no length has no normal");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "cascade_lip_has_no_length");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("Swap")),
            "the rejection has to explain the convention: {errors:?}"
        );
    }

    /// A scene with one deformed body: a parent carrying the `Deform` whose
    /// fields the test supplies, and a child mesh under it at identity.
    fn deform_scene(body: &str, child: &str) -> String {
        format!(
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e52\"\n\n\
             [[node]]\nname = \"Lagoon\"\n\n\
             [[node]]\nname = \"Hero\"\nparent = \"Lagoon\"\n\n\
             [node.components.Deform]\n{body}\n\
             [[node]]\nname = \"HeroBody\"\nparent = \"Lagoon/Hero\"\n{child}\n\
             [node.components.MeshRenderer]\nmesh = {{ asset = \"box\" }}\n"
        )
    }

    /// What `gleamsprat.loom` authors, as the control every rule below is one
    /// edit away from.
    const GOOD_DEFORM: &str = "nose = \"+z\"\nbeat = \"y\"\namplitude = 0.10\n\
                               wavelength = 0.5\nfrequency = 3.0\nphase = 0.0\n\
                               span_start = 0.58\n";

    #[test]
    fn the_authored_deform_validates() {
        Scene::parse(&deform_scene(GOOD_DEFORM, ""))
            .expect("this is what gleamsprat.loom carries");
    }

    /// **A knob wired to nothing is the S4 defect.** The shader early-outs at
    /// `amplitude <= 0`, so without this the node draws unchanged, every other
    /// field is dead text and `loom validate` reports clean.
    #[test]
    fn a_deform_with_no_amplitude_is_refused() {
        let errors = Scene::parse(&deform_scene(
            &GOOD_DEFORM.replace("amplitude = 0.10", "amplitude = 0.0"),
            "",
        ))
        .expect_err("zero amplitude displaces nothing");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_has_no_amplitude");
        assert_eq!(errors[0].field, "Deform.amplitude");
    }

    /// **The rank-1 Jacobian is the whole reason the normal is three ops**, and
    /// it holds only while the displacement points somewhere other than the
    /// coordinate it is a function of. A wrong normal is invisible in a still
    /// diff and obvious in motion, which is the worst combination there is.
    #[test]
    fn a_deform_that_beats_along_its_own_axis_is_refused() {
        let errors =
            Scene::parse(&deform_scene(&GOOD_DEFORM.replace("beat = \"y\"", "beat = \"z\""), ""))
                .expect_err("beat must differ from nose");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_beats_along_its_own_axis");
        assert!(
            errors[0].constraint.contains("+z"),
            "the rejection names the authored spelling, not the Rust variant: {errors:?}"
        );

        // The same beat is fine on a body that runs along a different axis —
        // the rule is about the pair, not about `z`.
        Scene::parse(&deform_scene(
            &GOOD_DEFORM.replace("nose = \"+z\"", "nose = \"+x\"").replace("beat = \"y\"", "beat = \"z\""),
            "",
        ))
        .expect("+x nose with a z beat is a perfectly good pair");
    }

    /// Zero wavelength is a divide by zero in a vertex shader: every vertex
    /// NaNs and the mesh simply vanishes, with nothing to breakpoint.
    #[test]
    fn a_deform_with_no_wavelength_is_refused() {
        let errors = Scene::parse(&deform_scene(
            &GOOD_DEFORM.replace("wavelength = 0.5", "wavelength = 0.0"),
            "",
        ))
        .expect_err("the wavenumber is 1/wavelength");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_wavelength_is_not_positive");
    }

    /// **Refused by the schema range rather than by a rule**, deliberately, so
    /// the fault is reported once. This pins that it is still reported at all.
    #[test]
    fn a_span_start_past_the_tail_is_refused_by_the_schema() {
        let errors = Scene::parse(&deform_scene(
            &GOOD_DEFORM.replace("span_start = 0.58", "span_start = 1.0"),
            "",
        ))
        .expect_err("1.0 leaves nothing to move");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "field_out_of_range");
        assert_eq!(errors[0].field, "Deform.span_start");
    }

    /// A `Deform` displaces vertices and nothing else. With no mesh in the
    /// subtree it validates, loads, and cannot possibly have an effect.
    #[test]
    fn a_deform_with_no_mesh_under_it_is_refused() {
        let scene = "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e53\"\n\n\
                     [[node]]\nname = \"Hero\"\n\n[node.components.Deform]\n"
            .to_owned()
            + GOOD_DEFORM;
        let errors = Scene::parse(&scene).expect_err("nothing to deform");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_has_no_mesh");
    }

    /// The inner wave would read rest positions the outer one has already
    /// moved, so the two disagree about where the body is. One wave per body.
    #[test]
    fn a_deform_under_a_deform_is_refused() {
        let child = format!("\n[node.components.Deform]\n{GOOD_DEFORM}");
        let errors =
            Scene::parse(&deform_scene(GOOD_DEFORM, &child)).expect_err("two body coordinates");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_inside_a_deform");
        assert_eq!(errors[0].node, "Lagoon/Hero/HeroBody");
    }

    /// **One shared `(lead, inv_len)` is only correct while the subtree shares
    /// the deform node's frame.** This refusal has a known expiry date — the
    /// arithmetic that lifts it is written out where the rule lives.
    #[test]
    fn a_deformed_child_moved_out_of_its_parents_frame_is_refused() {
        let errors =
            Scene::parse(&deform_scene(GOOD_DEFORM, "transform = { pos = [0.0, 0.05, 0.0] }\n"))
                .expect_err("the child would be deformed against the parent's body");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_child_is_not_at_identity");
        assert_eq!(errors[0].node, "Lagoon/Hero/HeroBody");

        // A scale of exactly 1 and a rotation of exactly 0 written out longhand
        // is still identity, which is what `gleamsprat.loom`'s children are.
        Scene::parse(&deform_scene(
            GOOD_DEFORM,
            "transform = { pos = [0.0, 0.0, 0.0], scale = [1.0, 1.0, 1.0] }\n",
        ))
        .expect("written-out identity is identity");
    }

    /// **A slope ceiling rather than an amplitude ceiling**, and this is the
    /// test that says why: the same amplitude passes at one wavelength and is
    /// refused at another, because shortening the wave steepens the shear
    /// exactly as much as raising the amplitude does.
    ///
    /// It is arithmetic only because `amplitude` is a body fraction. In metres
    /// it would need the mesh, and this crate depends on nothing.
    #[test]
    fn a_deform_steep_enough_to_fold_the_surface_is_refused() {
        // What gleamsprat.loom authors: 0.10 * (TAU/0.5 + 2/0.42) = 1.73.
        Scene::parse(&deform_scene(GOOD_DEFORM, "")).expect("1.73 is under the ceiling of 3.0");

        // Three times the amplitude, everything else the same.
        let errors = Scene::parse(&deform_scene(
            &GOOD_DEFORM.replace("amplitude = 0.10", "amplitude = 0.30"),
            "",
        ))
        .expect_err("0.30 puts the peak slope at 5.2");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "deform_surface_is_too_steep");
        assert!(
            errors[0].constraint.contains("0.17"),
            "the rejection has to carry the amplitude that WOULD pass: {errors:?}"
        );

        // The same amplitude at four times the wavelength is fine, which an
        // amplitude-only ceiling could not express.
        Scene::parse(&deform_scene(
            &GOOD_DEFORM
                .replace("amplitude = 0.10", "amplitude = 0.30")
                .replace("wavelength = 0.5", "wavelength = 2.0"),
            "",
        ))
        .expect("a long wave at a large amplitude is a gentle surface");
    }

    /// A node with no `Deform` is untouched by every rule above, which is what
    /// keeps every scene authored before this component byte-identical.
    #[test]
    fn a_scene_with_no_deform_is_unaffected() {
        Scene::parse(
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e54\"\n\n\
             [[node]]\nname = \"Hero\"\ntransform = { pos = [1.0, 2.0, 3.0] }\n\n\
             [node.components.MeshRenderer]\nmesh = { asset = \"box\" }\n",
        )
        .expect("an offset mesh with no Deform above it is ordinary");
    }

    /// The CPU path is untouched by any of the GPU rules, which is what makes
    /// `gpu = false` a safe default for the eight blessed particle scenes.
    #[test]
    fn an_ordinary_cpu_emitter_is_unaffected_by_the_gpu_rules() {
        Scene::parse(&emitter_scene("rate = 4000.0\nlifetime = 60.0\n"))
            .expect("a huge CPU plume is the CPU's problem, not the pool's");
    }

    /// **No GPU sort exists, so alpha is refused rather than scrambled.**
    /// Drawing a blended pool in slot order is not a subtle degradation — it is
    /// a visibly wrong picture with nothing in the frame to blame.
    #[test]
    fn a_gpu_emitter_must_be_additive() {
        let errors = Scene::parse(&emitter_scene("gpu = true\nadditive = false\n"))
            .expect_err("alpha on the GPU has no sort");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "gpu_emitter_needs_additive");
        assert_eq!(errors[0].field, "ParticleEmitter.additive");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("additive = true")),
            "the rejection has to say what to write: {errors:?}"
        );

        Scene::parse(&emitter_scene("gpu = true\nadditive = true\n"))
            .expect("additive is the supported mode");
    }

    /// **The pool-lapping rule, and the number has to be in the error.** "Too
    /// many particles" without the required slot count is not something an
    /// agent tuning a rate can act on.
    #[test]
    fn a_gpu_emitter_over_the_pool_is_rejected_with_the_count_it_needs() {
        // 4000/s for 30 s at the default 0.35 jitter is 162,000 slots.
        let errors = Scene::parse(&emitter_scene(
            "gpu = true\nadditive = true\nrate = 4000.0\nlifetime = 30.0\n",
        ))
        .expect_err("162,000 live particles do not fit a 65,536 pool");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "gpu_emitter_pool_too_small");
        assert!(
            errors[0].constraint.contains("162000")
                && errors[0].constraint.contains(&components::GPU_POOL_MAX.to_string()),
            "both numbers, or the author cannot tell how far over they are: {errors:?}"
        );

        // And the same emitter one third as long fits.
        Scene::parse(&emitter_scene(
            "gpu = true\nadditive = true\nrate = 4000.0\nlifetime = 10.0\n",
        ))
        .expect("54,000 slots fit");
    }

    /// The cinematic tier is one hero volume with a budget — ADR 0059.
    ///
    /// **Both halves matter and the second is the falsifier.** A cell budget
    /// that refuses everything is not a budget, so the scene that has to pass
    /// is `plough_cinematic`'s own 6.4 x 1.6 x 3.2 — the shape the ADR calls
    /// the right one — and the scene that has to fail is the cube of the same
    /// length, which is eight times the cells for the same hero.
    #[test]
    fn the_cinematic_tier_is_one_volume_with_a_cell_budget() {
        let head = "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e52\"\n";
        let body = |name: &str, parent: &str, extent: &str| {
            format!(
                "\n[[node]]\nname = \"{name}\"\n{parent}\n\
                 [node.components.WaterBody]\nkind = \"lake\"\n\
                 simulation = \"cinematic\"\nextent = {extent}\n"
            )
        };
        let tank = body("A", "", "[6.4, 1.6, 3.2]");

        Scene::parse(&format!("{head}{tank}")).expect("one tank is the tier");

        // A cube of the same longest axis: 64^3 = 262,144 cells.
        let errors = Scene::parse(&format!("{head}{}", body("A", "", "[6.4, 6.4, 6.4]")))
            .expect_err("a 64-cubed domain is eight times the budget");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "cinematic_domain_over_budget");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("262144"))
                && errors[0]
                    .hint
                    .as_deref()
                    .is_some_and(|h| h.contains(&scene::MAX_CINEMATIC_CELLS.to_string())),
            "both numbers, or the author cannot tell how far over they are: {errors:?}"
        );

        let second = body("B", "parent = \"A\"", "[6.4, 1.6, 3.2]");
        let errors =
            Scene::parse(&format!("{head}{tank}{second}")).expect_err("there is one solver");
        assert_eq!(errors.len(), 1, "the SECOND is named, not both: {errors:?}");
        assert_eq!(errors[0].error, "second_cinematic_water_body");
        assert_eq!(errors[0].node, "A/B");
    }

    /// One pool, so a second GPU emitter would silently not draw. Refusing is
    /// the whole point: an absent feature is the one thing no gate here can
    /// see.
    #[test]
    fn a_second_gpu_emitter_in_one_scene_is_refused() {
        let one = "\n[[node]]\nname = \"A\"\n\n\
                   [node.components.ParticleEmitter]\ngpu = true\nadditive = true\n";
        let two = "\n[[node]]\nname = \"B\"\nparent = \"A\"\n\n\
                   [node.components.ParticleEmitter]\ngpu = true\nadditive = true\n";
        let head = "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e51\"\n";

        Scene::parse(&format!("{head}{one}")).expect("one pool is what there is");
        let errors = Scene::parse(&format!("{head}{one}{two}")).expect_err("two pools do not exist");
        assert_eq!(errors.len(), 1, "the SECOND is named, not both: {errors:?}");
        assert_eq!(errors[0].error, "second_gpu_emitter");
        assert_eq!(errors[0].node, "A/B");
    }

    /// **A catch-up dispatch has one origin**, so an emitter on a body that
    /// physics moves lays its whole trail at the body's final position. That
    /// is a wrong picture that looks like a wrong scene.
    #[test]
    fn a_gpu_emitter_on_a_rigid_body_is_refused() {
        let src = "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e52\"\n\n\
                   [[node]]\nname = \"Rocket\"\n\n\
                   [node.components.RigidBody]\ndynamic = true\n\n\
                   [node.components.ParticleEmitter]\ngpu = true\nadditive = true\n";

        let errors = Scene::parse(src).expect_err("a moving GPU emitter is a wrong trail");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "gpu_emitter_on_a_moving_node");
        assert!(
            errors[0].hint.as_deref().is_some_and(|h| h.contains("ONE dispatch")),
            "{errors:?}"
        );
    }

    /// A zero wavelength divides by zero on the way to the wave number, and a
    /// non-finite anything poisons the determinism hashes (§1).
    #[test]
    fn a_zero_wavelength_is_rejected_before_it_becomes_infinity() {
        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 0.0\namplitude = 0.5\nsteepness = 0.5\ndirection = [1.0, 0.0]\n",
        );

        let errors = Scene::parse(&src).expect_err("k = 2π/0 is infinite");
        assert_eq!(errors[0].error, "wave_wavelength_not_positive");
    }

    /// **A `WaterBody` serde cannot read must not be a `WaterBody` nobody
    /// checks.** The steepness limit is the whole reason this validation
    /// exists, and it used to be switched off by any single field the schema
    /// check does not reach — a nested array of the wrong length being the
    /// cheapest example, because `direction` lives two tables down and the
    /// registry only walks a component's top level. The wave here is over the
    /// limit on purpose: without the rejection the file reports `ok: true`.
    #[test]
    fn a_water_body_that_cannot_be_read_is_rejected_rather_than_skipped() {
        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 4.0\namplitude = 1.0\nsteepness = 1.0\n\
             direction = [1.0, 0.0, 0.0]\n",
        );

        let errors = Scene::parse(&src).expect_err("a direction has two components");
        assert_eq!(errors[0].error, "component_unreadable", "{errors:?}");
        assert_eq!(errors[0].node, "Ocean");
        assert_eq!(errors[0].field, "WaterBody");
        // Serde's own message is the actionable half — it names what it could
        // not read, which is the one thing this layer cannot work out.
        assert!(
            errors[0]
                .hint
                .as_deref()
                .is_some_and(|h| h.contains("length 3")),
            "{errors:?}"
        );
    }

    /// The enum spelling of the same hole: `kind` is a name the schema does
    /// reach, and it stayed unchecked because schemars emits documented
    /// variants as `oneOf` rather than a flat `enum` array. Both layers hold
    /// it now — the schema check gets there first, which is why the error is
    /// the more specific one.
    #[test]
    fn an_invalid_water_kind_is_rejected() {
        let src = water_scene("kind = \"lava\"\n");

        let errors = Scene::parse(&src).expect_err("lava is not a kind of water");
        assert_eq!(errors[0].error, "field_not_in_enum", "{errors:?}");
        assert_eq!(errors[0].field, "WaterBody.kind");
        assert!(errors[0].constraint.contains("ocean"), "{errors:?}");
    }

    /// **A sign typo is its own mistake.** Folding it through `.abs()` blamed
    /// steepness for a negative amplitude and printed the value with the sign
    /// flipped, and reported `steepness = -9.0` as "value -9.0, constraint at
    /// most 3.183" — a rejection that contradicts itself is worse than none,
    /// because an agent acting on it lowers a number that is already low.
    #[test]
    fn a_negative_amplitude_or_steepness_is_rejected_as_itself() {
        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 4.0\namplitude = -5.0\nsteepness = 0.5\ndirection = [1.0, 0.0]\n",
        );
        let errors = Scene::parse(&src).expect_err("amplitude is a magnitude");
        assert_eq!(errors.len(), 1, "one fault, reported once: {errors:?}");
        assert_eq!(errors[0].error, "wave_amplitude_negative");
        assert_eq!(errors[0].field, "WaterBody.waves.waves[0].amplitude");
        assert_eq!(errors[0].value, serde_json::json!(-5.0));

        let src = water_scene(
            "\n[[node.components.WaterBody.waves.waves]]\n\
             wavelength = 4.0\namplitude = 1.0\nsteepness = -9.0\ndirection = [1.0, 0.0]\n",
        );
        let errors = Scene::parse(&src).expect_err("steepness is a magnitude");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].error, "wave_steepness_negative");
        assert_eq!(errors[0].field, "WaterBody.waves.waves[0].steepness");
        assert_eq!(errors[0].value, serde_json::json!(-9.0));
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

    /// **A scene still authoring the deleted `[ripples]` table is refused.**
    ///
    /// The three tests that were here checked the grid's own bounds — the
    /// Courant limit, the sponge width, the schema ranges — and every one of
    /// them is about a scheme ADR 0056 deleted. What replaces them is the
    /// question that outlives it: a key the parser does not understand is a key
    /// it *ignores*, which is how S4's prefab instances arrived with no
    /// components and validated clean. A file written against the old schema
    /// has to say so.
    #[test]
    fn the_deleted_ripple_table_is_refused_rather_than_ignored() {
        let scene = "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f\"\n\n\
             [[node]]\nname = \"Water\"\n\n  [node.components.WaterBody]\n  \
             kind = \"lake\"\n\n    [node.components.WaterBody.ripples]\n    \
             extent = 16.0\n    cell = 0.5\n    speed = 2.4\n    damping = 0.997\n    \
             strength = 0.9\n";
        let errors = match Scene::parse(scene) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.iter().map(|e| e.error.clone()).collect(),
        };
        assert!(
            !errors.is_empty(),
            "a scene authoring `[ripples]` parsed clean — the water it asks for \
             does not exist and nothing said so"
        );
    }
}
