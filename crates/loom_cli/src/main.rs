//! The agent-facing entrypoint. CLI first, MCP second (brief §7.10).
//!
//! Subcommands land with the milestone that makes them real:
//!   validate | describe  M1 · render  M2 · run [--watch]  M5.5 · scene  M9 · voxel  M10
//!
//! Every subcommand emits structured JSON on stdout and uses the exit code as
//! the coarse signal, so both a shell and an agent can consume it.
//!
//! `ponytail:` hand-rolled argument matching. Two subcommands do not justify a
//! parser dependency. Switch to `clap` when M9 lands `scene place/measure/...`
//! and the count goes past four — `run` is the seam, so it is a local change.

use std::process::ExitCode;

use loom_render::glam::{EulerRot, Mat4, Quat, Vec3};
use loom_render::{Camera, Device, Instance, Object, Renderer};
use loom_scene::{Scene, components};

const USAGE: &str = "\
loom — AI-native engine CLI

USAGE:
    loom validate <scene.loom>    Validate a scene; exit 1 with JSON errors if invalid
    loom describe <TypeName>      Print a component's JSON Schema
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (code, output) = run(&args);
    if !output.is_empty() {
        println!("{output}");
    }
    ExitCode::from(code)
}

/// Exit code plus whatever should go to stdout.
///
/// Split out from `main` so it is testable without spawning a process.
/// Codes: 0 ok · 1 the thing was invalid · 2 the invocation was wrong.
fn run(args: &[String]) -> (u8, String) {
    match args.first().map(String::as_str) {
        Some("validate") => match args.get(1) {
            Some(path) => validate(path),
            None => (2, USAGE.to_owned()),
        },
        Some("describe") => match args.get(1) {
            Some(name) => describe(name),
            None => (2, USAGE.to_owned()),
        },
        Some("render") => match args.get(1) {
            Some(path) => render(path, args),
            None => (2, USAGE.to_owned()),
        },
        _ => (2, USAGE.to_owned()),
    }
}

fn validate(path: &str) -> (u8, String) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return (
                2,
                json_line(&serde_json::json!({
                    "error": "io_error",
                    "path": path,
                    "constraint": e.to_string(),
                })),
            );
        }
    };

    match Scene::parse(&src) {
        Ok(scene) => (
            0,
            json_line(&serde_json::json!({
                "ok": true,
                "path": path,
                "nodes": scene.nodes().len(),
            })),
        ),
        // Every violation, not just the first — one round-trip per fix is the
        // retry loop `docs/format/README.md` §6 exists to avoid.
        Err(errors) => (1, json_line(&serde_json::json!({ "errors": errors }))),
    }
}

fn describe(name: &str) -> (u8, String) {
    let registry = components::registry();
    match registry.describe(name) {
        Some(schema) => (0, json_line(schema)),
        None => {
            let known: Vec<&str> = registry.type_names().collect();
            (
                2,
                json_line(&serde_json::json!({
                    "error": "unknown_component_type",
                    "value": name,
                    // Listing the alternatives turns a failed lookup into one
                    // correction instead of a guessing loop (§6).
                    "hint": format!("known types: {}", known.join(", ")),
                })),
            )
        }
    }
}

/// Render a scene headless to a PNG.
///
/// Brief §7.1: this is the agent's eyes, and it runs the *same* path a windowed
/// renderer would — there is no separate preview renderer to drift.
fn render(path: &str, args: &[String]) -> (u8, String) {
    let out = flag(args, "--out").unwrap_or_else(|| "render.png".to_owned());
    let (width, height) = match flag(args, "--size") {
        Some(spec) => match parse_size(&spec) {
            Some(wh) => wh,
            None => {
                return (
                    2,
                    json_line(&serde_json::json!({
                        "error": "bad_argument",
                        "value": spec,
                        "hint": "--size takes WxH, e.g. 1280x720",
                    })),
                );
            }
        },
        None => (960, 640),
    };

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return (
                2,
                json_line(&serde_json::json!({
                    "error": "io_error", "path": path, "constraint": e.to_string(),
                })),
            );
        }
    };
    let scene = match Scene::parse(&src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let objects = scene_to_objects(&scene);
    let yaw = flag(args, "--yaw").and_then(|v| v.parse::<f32>().ok()).unwrap_or(35.0);
    let pitch = flag(args, "--pitch").and_then(|v| v.parse::<f32>().ok()).unwrap_or(28.0);
    let camera = frame_scene(&objects, yaw, pitch);

    let result = (|| -> Result<String, String> {
        let instance = Instance::new(c"loom").map_err(|e| e.to_string())?;
        let device = Device::new(&instance).map_err(|e| e.to_string())?;
        let name = device.name().to_owned();
        let mut renderer =
            Renderer::new(&instance, &device, width, height).map_err(|e| e.to_string())?;
        renderer
            .render_to_png(&objects, &camera, std::path::Path::new(&out))
            .map_err(|e| e.to_string())?;
        // Zero validation messages is half the definition of green (brief §7.3).
        instance
            .check_validation()
            .map_err(|m| format!("validation was not silent: {}", m.join("; ")))?;
        Ok(name)
    })();

    match result {
        Ok(gpu) => (
            0,
            json_line(&serde_json::json!({
                "ok": true, "out": out, "objects": objects.len(),
                "size": [width, height], "gpu": gpu,
            })),
        ),
        Err(e) => (
            1,
            json_line(&serde_json::json!({ "error": "render_failed", "constraint": e })),
        ),
    }
}

/// Flatten the node tree into draw calls, composing world transforms.
///
/// Transform propagation properly belongs to the ECS at M3; this walks the
/// parent chain directly because M2 has no ECS yet. Nodes are declared
/// parents-first (the forward-reference rule), so one pass suffices.
fn scene_to_objects(scene: &Scene) -> Vec<Object> {
    let mut world: std::collections::BTreeMap<String, Mat4> = std::collections::BTreeMap::new();
    let mut objects = Vec::new();

    for (index, node) in scene.nodes().iter().enumerate() {
        let t = &node.transform;
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(t.scale),
            // Intrinsic Y-X-Z, array ordered [pitch_x, yaw_y, roll_z]
            // (docs/format/README.md §1.1).
            Quat::from_euler(
                EulerRot::YXZ,
                t.rot_euler[1].to_radians(),
                t.rot_euler[0].to_radians(),
                t.rot_euler[2].to_radians(),
            ),
            Vec3::from_array(t.pos),
        );
        let parent = node
            .parent
            .as_ref()
            .and_then(|p| world.get(p))
            .copied()
            .unwrap_or(Mat4::IDENTITY);
        let model = parent * local;
        world.insert(node.path.clone(), model);

        // Only nodes that declare a MeshRenderer are drawn — a Light or a bare
        // grouping node is not geometry.
        if node.components.contains_key("MeshRenderer") {
            objects.push(Object {
                model,
                color: palette(index),
            });
        }
    }
    objects
}

/// Distinct per-object colours until materials exist (M5).
fn palette(index: usize) -> [f32; 3] {
    const COLORS: [[f32; 3]; 6] = [
        [0.85, 0.35, 0.35],
        [0.35, 0.75, 0.85],
        [0.90, 0.75, 0.35],
        [0.55, 0.80, 0.45],
        [0.70, 0.50, 0.85],
        [0.80, 0.55, 0.40],
    ];
    COLORS[index % COLORS.len()]
}

/// Point the camera at the scene's bounds.
///
/// Auto-framing rather than a fixed camera because the agent's first render of
/// a scene it just authored should show the whole thing — a hardcoded camera
/// produces an empty image and a confused retry loop.
fn frame_scene(objects: &[Object], yaw_degrees: f32, pitch_degrees: f32) -> Camera {
    if objects.is_empty() {
        return Camera { eye: Vec3::new(4.0, 4.0, 8.0), target: Vec3::ZERO, fov_y_degrees: 45.0 };
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for object in objects {
        // Cube spans -1..1 locally, so transform all eight corners.
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            let p = object.model.transform_point3(corner);
            min = min.min(p);
            max = max.max(p);
        }
    }
    let center = (min + max) * 0.5;
    let radius = (max - min).length() * 0.5;
    let distance = (radius / (22.5_f32).to_radians().tan()).max(3.0);

    // Orbit the framed bounds. Design doc §2.10: one render is a lie — an
    // object inside another object is invisible from exactly one angle, so the
    // agent should always look from more than one.
    let (yaw, pitch) = (yaw_degrees.to_radians(), pitch_degrees.to_radians());
    let direction = Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    Camera {
        eye: center + direction * distance,
        target: center,
        fov_y_degrees: 45.0,
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn parse_size(spec: &str) -> Option<(u32, u32)> {
    let (w, h) = spec.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn json_line<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn describe_emits_the_schema_including_constraints() {
        let (code, out) = run(&args(&["describe", "Light"]));

        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["properties"]["intensity"]["maximum"], 10000.0);
        assert!(
            v["properties"]["intensity"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("Interior lights")),
            "doc comment should reach the agent"
        );
    }

    /// A failed lookup lists the alternatives, so it costs one correction
    /// rather than a guessing loop.
    #[test]
    fn describe_of_an_unknown_type_lists_the_known_ones() {
        let (code, out) = run(&args(&["describe", "Ligth"]));

        assert_eq!(code, 2);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "unknown_component_type");
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("Light"), "should suggest Light, got: {hint}");
    }

    #[test]
    fn validate_accepts_the_canonical_fixture() {
        let (code, out) = run(&args(&["validate", "../../assets/test/office.loom"]));

        assert_eq!(code, 0, "office.loom should be valid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["nodes"], 4);
    }

    #[test]
    fn validate_reports_the_out_of_range_field_with_its_node() {
        let (code, out) = run(&args(&["validate", "../../assets/test/bad_intensity.loom"]));

        assert_eq!(code, 1);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let e = &v["errors"][0];
        assert_eq!(e["error"], "field_out_of_range");
        assert_eq!(e["node"], "Office/CeilingLight");
        assert_eq!(e["field"], "Light.intensity");
        assert_eq!(e["constraint"], "0.0..=10000.0");
    }

    #[test]
    fn a_missing_file_is_an_invocation_error_not_an_invalid_scene() {
        let (code, out) = run(&args(&["validate", "nope.loom"]));

        assert_eq!(code, 2, "2 means 'you called it wrong', 1 means 'invalid'");
        assert!(out.contains("io_error"));
    }

    #[test]
    fn no_arguments_prints_usage() {
        let (code, out) = run(&[]);

        assert_eq!(code, 2);
        assert!(out.contains("loom validate"));
    }
}
