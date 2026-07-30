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

mod run;

use std::process::ExitCode;

use loom_ecs::{FixedTimestep, World};
use loom_render::glam::{Mat4, Vec3};
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
        Some("sim") => match args.get(1) {
            Some(path) => sim(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("run") => match args.get(1) {
            Some(path) => match run::open_scene(path) {
                Ok(()) => (0, String::new()),
                Err(e) => (1, json_line(&serde_json::json!({
                    "error": "run_failed", "constraint": e,
                }))),
            },
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

    let world = World::from_scene(&scene);
    let base = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);
    let objects = world_to_objects(&world, &library);
    let yaw = flag(args, "--yaw").and_then(|v| v.parse::<f32>().ok()).unwrap_or(35.0);
    let pitch = flag(args, "--pitch").and_then(|v| v.parse::<f32>().ok()).unwrap_or(28.0);
    let camera = frame_scene(&objects, yaw, pitch);

    let result = (|| -> Result<String, String> {
        let instance = Instance::new(c"loom").map_err(|e| e.to_string())?;
        let device = Device::new(&instance).map_err(|e| e.to_string())?;
        let name = device.name().to_owned();
        let mut renderer = Renderer::new(&instance, &device, width, height, &library.meshes)
            .map_err(|e| e.to_string())?;
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

/// Flatten the world into draw calls.
///
/// Transform propagation lives in `loom_ecs` as of M3; this only reads the
/// resolved `GlobalTransform`. The parent-chain walk that used to live here
/// was a stand-in until the ECS existed, and is gone.
fn world_to_objects(world: &World, library: &MeshLibrary) -> Vec<Object> {
    world
        .entities()
        .iter()
        .enumerate()
        .filter(|(_, e)| world.is_renderable(**e))
        .filter_map(|(index, entity)| {
            let global = world.global_transform(*entity)?;
            Some(Object {
                model: Mat4::from_cols_array(&global.matrix),
                color: palette(index),
                mesh: library.index_for(world.mesh_asset(*entity)),
            })
        })
        .collect()
}

/// Every mesh a scene needs, plus the mapping from asset alias to draw index.
///
/// Built per scene rather than globally: an agent iterating on one level
/// should not pay to load every asset in the project.
struct MeshLibrary {
    meshes: Vec<loom_asset::Mesh>,
    by_name: std::collections::BTreeMap<String, u32>,
}

impl MeshLibrary {
    /// Resolve every asset a scene references.
    ///
    /// A primitive name resolves procedurally; anything else is a path to
    /// import. An asset that cannot be resolved falls back to a box rather
    /// than failing the render — a missing mesh should be *visible*, not
    /// fatal (design doc §2.6: degrade, do not crash).
    fn for_scene(scene: &Scene, base: &std::path::Path) -> Self {
        let mut meshes = vec![loom_asset::primitives::box_mesh()];
        let mut by_name = std::collections::BTreeMap::new();
        by_name.insert("box".to_owned(), 0_u32);

        let mut wanted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for node in scene.nodes() {
            if let Some(asset) = node
                .components
                .get("MeshRenderer")
                .and_then(|m| m.get("mesh"))
                .and_then(|m| m.get("asset"))
                .and_then(serde_json::Value::as_str)
            {
                wanted.insert(asset.to_owned());
            }
        }

        for name in wanted {
            if by_name.contains_key(&name) {
                continue;
            }
            let mesh = loom_asset::primitives::build(&name).or_else(|| {
                let path = scene_asset_path(scene, &name).map(|p| base.join(p))?;
                match loom_asset::mesh::import_gltf(&path) {
                    Ok(mesh) => Some(mesh),
                    Err(e) => {
                        eprintln!("loom: {name}: {e}; falling back to a box");
                        None
                    }
                }
            });
            if let Some(mesh) = mesh {
                by_name.insert(name, u32::try_from(meshes.len()).unwrap_or(0));
                meshes.push(mesh);
            }
        }

        Self { meshes, by_name }
    }

    /// Hand the mesh data to a renderer.
    fn into_meshes(self) -> Vec<loom_asset::Mesh> {
        self.meshes
    }

    /// Draw index for an asset alias; 0 (the box) when unknown.
    fn index_for(&self, asset: Option<&str>) -> u32 {
        asset
            .and_then(|a| self.by_name.get(a))
            .copied()
            .unwrap_or(0)
    }
}

/// The advisory `path` an `[[asset]]` entry carries, for importing.
fn scene_asset_path(scene: &Scene, key: &str) -> Option<String> {
    scene.asset_path(key).map(str::to_owned)
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

/// Simulate headless and print a deterministic state hash.
///
/// The second of the agent's two verification channels (brief §5). A render
/// tells you a script *looks* fine while it leaks entities on frame 900; only
/// simulation catches that.
fn sim(path: &str, args: &[String]) -> (u8, String) {
    let ticks: u64 = flag(args, "--ticks")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

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

    let mut world = World::from_scene(&scene);
    let mut clock = FixedTimestep::new(60.0);

    // Elapsed time is fed in as an exact constant, never read from the wall
    // clock (never-do #8). That is what makes this reproducible, and it is why
    // `advance` takes the delta as an argument.
    for _ in 0..ticks {
        clock.advance(clock.step_seconds());
        world.propagate_transforms();
    }

    (
        0,
        json_line(&serde_json::json!({
            "ok": true,
            "path": path,
            "ticks": clock.tick,
            "entities": world.entities().len(),
            // Hex so two runs are trivially eyeball-comparable.
            "state_hash": format!("{:016x}", world.state_hash()),
        })),
    )
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

    /// **The M3 exit criterion, end to end.** The same scene simulated twice
    /// must produce the same hash — otherwise every `--assert` the agent writes
    /// is flaky, and flaky assertions train it to ignore failures (§7.5).
    #[test]
    fn simulating_the_same_scene_twice_gives_the_same_hash() {
        let run = || {
            let (code, out) = run(&args(&["sim", "../../assets/test/blockout.loom", "--ticks", "240"]));
            assert_eq!(code, 0, "{out}");
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            v["state_hash"].as_str().unwrap().to_owned()
        };

        assert_eq!(run(), run());
    }

    /// A different scene must hash differently, or the check proves nothing.
    /// The M5 exit criterion, end to end: a scene mixing an imported glTF
    /// mesh with procedural primitives renders every object.
    #[test]
    fn a_scene_mixing_gltf_and_primitives_resolves_every_mesh() {
        let src = std::fs::read_to_string("../../assets/test/workshop.loom").unwrap();
        let scene = Scene::parse(&src).expect("workshop.loom is valid");
        let library = MeshLibrary::for_scene(&scene, std::path::Path::new("../../assets/test"));

        // plane, pyramid (imported), box, sphere, cylinder — plus the default
        // box the library always carries at index 0.
        assert!(
            library.meshes.len() >= 5,
            "only resolved {} meshes",
            library.meshes.len()
        );
        // The imported one is not a primitive, so a name lookup proves the
        // glTF path ran rather than falling back.
        assert!(
            loom_asset::primitives::build("pyramid").is_none(),
            "pyramid must not be a primitive, or this proves nothing"
        );
        assert_ne!(
            library.index_for(Some("pyramid")),
            0,
            "pyramid fell back to the default box — the glTF import failed"
        );
    }

    #[test]
    fn a_different_scene_hashes_differently() {
        let hash_of = |path: &str| {
            let (_, out) = run(&args(&["sim", path, "--ticks", "60"]));
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            v["state_hash"].as_str().map(str::to_owned)
        };

        assert_ne!(
            hash_of("../../assets/test/blockout.loom"),
            hash_of("../../assets/test/office.loom"),
            "two different scenes must not collide"
        );
    }

    #[test]
    fn no_arguments_prints_usage() {
        let (code, out) = run(&[]);

        assert_eq!(code, 2);
        assert!(out.contains("loom validate"));
    }
}
