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

mod gizmo;
mod log;
mod panels;
mod play;
mod scene_view;
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
        Some("scene") => match args.get(1) {
            Some(path) => scene_tx(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("terrain") => match args.get(1) {
            Some(path) => terrain(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("explode") => match args.get(1) {
            Some(path) => explode(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("place") => match args.get(1) {
            Some(path) => place(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("measure") => match args.get(1) {
            Some(path) => measure(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("run") => match args.get(1) {
            Some(path) => match run::open_scene(
                path,
                args.iter().any(|a| a == "--edit"),
                flag(args, "--frames").and_then(|n| n.parse::<u32>().ok()),
            ) {
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
        Ok(scene) => {
            // Physical sanity runs after schema validation, because a scene
            // that will not load cannot be reasoned about physically. These
            // are warnings by design (graphics doc §C.5): an unusual scene is
            // not an invalid one, but the agent should be told what is odd.
            let findings = loom_physics::check_scene(&scene);
            let blocking = findings
                .iter()
                .any(|f| f.severity == loom_physics::Severity::Error);
            (
                u8::from(blocking),
                json_line(&serde_json::json!({
                    "ok": !blocking,
                    "path": path,
                    "nodes": scene.nodes().len(),
                    "physics": findings,
                })),
            )
        }
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

    let mut world = World::from_scene(&scene);
    let base = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);

    // --sim steps physics before drawing, which is what makes a still image a
    // useful view of a simulation rather than only of its initial state.
    if let Some(ticks) = flag(args, "--sim").and_then(|v| v.parse::<u32>().ok()) {
        simulate_physics(&mut world, ticks);
    }

    let objects = world_to_objects(&world, &library);
    let yaw = flag(args, "--yaw").and_then(|v| v.parse::<f32>().ok()).unwrap_or(35.0);
    let pitch = flag(args, "--pitch").and_then(|v| v.parse::<f32>().ok()).unwrap_or(28.0);
    // Framed from REAL mesh bounds. Assuming a unit cube was fine while every
    // mesh was one; a voxel volume spans tens of units and put the camera
    // inside the terrain.
    let camera = frame_scene(&node_bounds(&world, &library), yaw, pitch);

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
pub(crate) fn world_to_objects(world: &World, library: &MeshLibrary) -> Vec<Object> {
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
                mesh: mesh_index_for(world, library, *entity),
            })
        })
        .collect()
}

/// Baked voxel meshes, keyed by the op list that produced them.
pub(crate) type VoxelCache = std::collections::BTreeMap<u64, loom_asset::Mesh>;

/// Every mesh a scene needs, plus the mapping from asset alias to draw index.
///
/// Built per scene rather than globally: an agent iterating on one level
/// should not pay to load every asset in the project.
pub(crate) struct MeshLibrary {
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
    pub(crate) fn for_scene(scene: &Scene, base: &std::path::Path) -> Self {
        Self::with_cache(scene, base, &mut VoxelCache::default())
    }

    /// As [`Self::for_scene`], reusing voxel meshes whose recipe has not
    /// changed.
    ///
    /// The editor rebuilds this on **every** edit so the viewport can follow
    /// the file. Re-baking a 128³ volume at that rate is the difference between
    /// an editor and a slideshow, and the op list is the recipe — equal recipe,
    /// equal geometry, by construction (never-do #11).
    pub(crate) fn with_cache(
        scene: &Scene,
        base: &std::path::Path,
        cache: &mut VoxelCache,
    ) -> Self {
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

        // Voxel volumes bake into a mesh at load, from the op list the scene
        // stores. never-do #11: the scene holds the recipe, never the voxels.
        for node in scene.nodes() {
            let Some(volume) = node.components.get("VoxelVolume") else {
                continue;
            };
            let key = format!("voxel:{}", node.path);
            if by_name.contains_key(&key) {
                continue;
            }
            // Keyed by the recipe, not the node path: renaming a node must not
            // force a re-bake, and two nodes with the same ops share one mesh.
            let recipe = fnv(0xcbf2_9ce4_8422_2325, volume.to_string().as_bytes());
            let baked = match cache.get(&recipe) {
                Some(mesh) => Some(mesh.clone()),
                None => {
                    let mesh = bake_voxel(volume);
                    if let Some(mesh) = mesh.clone() {
                        // `ponytail:` unbounded until it isn't. Each entry is
                        // one volume's geometry and an edit session touches a
                        // handful; drop the lot rather than track ages.
                        if cache.len() >= 8 {
                            cache.clear();
                        }
                        cache.insert(recipe, mesh);
                    }
                    mesh
                }
            };
            match baked {
                Some(mesh) => {
                    by_name.insert(key, u32::try_from(meshes.len()).unwrap_or(0));
                    meshes.push(mesh);
                }
                None => crate::log::warn(format!("{}: voxel volume produced no surface", node.path)),
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
                        crate::log::warn(format!("{name}: {e}; falling back to a box"));
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

    /// The mesh data, for a renderer to upload.
    pub(crate) fn meshes(&self) -> &[loom_asset::Mesh] {
        &self.meshes
    }


    /// Identity of the mesh **set**, for deciding whether the GPU buffers a
    /// viewer already uploaded are still the right ones.
    ///
    /// Names alone are not enough: a re-baked voxel volume keeps its alias and
    /// changes its geometry entirely, so the sizes and bounds go in too.
    pub(crate) fn key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (name, index) in &self.by_name {
            h = fnv(h, name.as_bytes());
            h = fnv(h, &index.to_le_bytes());
        }
        for mesh in &self.meshes {
            h = fnv(h, &mesh.vertices.len().to_le_bytes());
            h = fnv(h, &mesh.indices.len().to_le_bytes());
            let (lo, hi) = mesh.bounds();
            for f in lo.iter().chain(hi.iter()) {
                // Bit patterns, not values — the determinism rule from §7.5
                // applies to any hash the engine compares across runs.
                h = fnv(h, &f.to_bits().to_le_bytes());
            }
        }
        h
    }

    /// Asset aliases this scene resolved, for the asset browser.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// Draw index for an asset alias; 0 (the box) when unknown.
    pub(crate) fn index_for(&self, asset: Option<&str>) -> u32 {
        asset
            .and_then(|a| self.by_name.get(a))
            .copied()
            .unwrap_or(0)
    }
}

/// FNV-1a over bytes. The engine's one hash, so every fingerprint it compares
/// is stable across runs and machines.
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Bake a `VoxelVolume` component into a mesh.
fn bake_voxel(component: &serde_json::Value) -> Option<loom_asset::Mesh> {
    #[allow(clippy::cast_possible_truncation)]
    let voxel_size = component.get("voxel_size").and_then(serde_json::Value::as_f64)? as f32;
    let chunks = component.get("chunks").and_then(|c| c.as_array())?;
    let dims: Vec<usize> = chunks
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();
    if dims.len() != 3 {
        return None;
    }
    let ops: Vec<loom_voxel::VoxelOp> = component
        .get("ops")
        .and_then(|o| o.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    if ops.is_empty() {
        return None;
    }

    let mut volume = loom_voxel::Volume::new([dims[0], dims[1], dims[2]], voxel_size);
    volume.bake(&ops);
    let mesh = loom_voxel::mesh::mesh_volume(&volume, &loom_voxel::SurfaceNets);
    (!mesh.indices.is_empty()).then_some(mesh)
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
fn frame_scene(
    boxes: &std::collections::BTreeMap<String, loom_scene::place::Bounds>,
    yaw_degrees: f32,
    pitch_degrees: f32,
) -> Camera {
    if boxes.is_empty() {
        return Camera { eye: Vec3::new(4.0, 4.0, 8.0), target: Vec3::ZERO, fov_y_degrees: 45.0 };
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for b in boxes.values() {
        min = min.min(Vec3::from_array(b.min));
        max = max.max(Vec3::from_array(b.max));
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

    // Scripts attached via a `Script` component run every tick. This is the
    // second verification channel (brief §5): a render tells you a script
    // *looks* fine while it leaks entities on frame 900; only simulation
    // catches behaviour.
    let mut host = loom_script::ScriptHost::default();
    let mut scripted: Vec<(loom_ecs::Entity, String)> = Vec::new();
    let base = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    for entity in world.entities() {
        let Some(script) = world.script_path(*entity) else {
            continue;
        };
        let source = match std::fs::read_to_string(base.join(script)) {
            Ok(s) => s,
            Err(e) => {
                return (1, json_line(&serde_json::json!({
                    "error": "io_error", "script": script, "constraint": e.to_string(),
                })));
            }
        };
        if let Err(e) = host.compile(script, &source) {
            return (1, json_line(&e));
        }
        scripted.push((*entity, script.to_owned()));
    }

    // Elapsed time is fed in as an exact constant, never read from the wall
    // clock (never-do #8). That is what makes this reproducible, and it is why
    // `advance` takes the delta as an argument.
    // Physics runs here, through the **same** `play::Sim` the editor's Play
    // button drives. It used to not run at all: `simulate_physics` was called
    // only by `loom render --sim`, so the picture had gravity and the
    // assertions did not — `--assert` was quietly checking authored positions.
    //
    // Built from the world as authored, before the loop. A script that moves a
    // body after this point is not fed back into the solver; scripted
    // kinematic bodies are not wired yet, and that is a gap rather than a
    // silent approximation.
    let mut physics = play::Sim::new(&world);

    for _ in 0..ticks {
        clock.advance(clock.step_seconds());
        physics.step(1);
        physics.write_back(&mut world);
        for (entity, script) in &scripted {
            let Some(transform) = world.transform(*entity).cloned() else {
                continue;
            };
            let state = loom_script::NodeState {
                position: transform.pos,
                rotation: transform.rot_euler,
                scale: transform.scale,
            };
            match host.tick(script, clock.tick, &state) {
                Ok(next) => {
                    if let Some(t) = world.transform_mut(*entity) {
                        t.pos = next.position;
                        t.rot_euler = next.rotation;
                        t.scale = next.scale;
                    }
                }
                Err(e) => return (1, json_line(&e)),
            }
        }
        world.propagate_transforms();
    }

    // Assertions are checked after the run, against final world state. This is
    // what makes an agent's claim about behaviour checkable rather than
    // asserted (design doc §2.10).
    let mut failures = Vec::new();
    for spec in flags(args, "--assert") {
        match check_assertion(&world, &spec) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                let actual = assertion_actual(&world, &spec);
                failures.push(serde_json::json!({
                    "assert": spec,
                    "actual": actual,
                    "hint": "Format is `Node/Path.axis OP value`, axis one of x/y/z, \
                             OP one of > >= < <= == ~=",
                }));
            }
        }
    }
    if !failures.is_empty() {
        return (1, json_line(&serde_json::json!({
            "ok": false,
            "ticks": clock.tick,
            "failed_assertions": failures,
        })));
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
            "assertions": flags(args, "--assert").len(),
        })),
    )
}

/// Step physics and write the result back onto the world.
///
/// One line, because the substance moved to `play::Sim` when the editor grew a
/// Play button — the headless path and the interactive one must agree about
/// what a scene means physically, and the only way to guarantee that is for
/// there to be one of them.
fn simulate_physics(world: &mut World, ticks: u32) {
    let mut sim = play::Sim::new(world);
    sim.step(ticks);
    sim.write_back(world);
}

/// Every value given for a repeated flag.
fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, a)| *a == name)
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect()
}

/// The world-space value an assertion refers to.
fn assertion_value(world: &World, path: &str, axis: &str) -> Option<f32> {
    let entity = world
        .entities()
        .iter()
        .find(|e| world.path(**e) == Some(path))?;
    let global = world.global_transform(*entity)?;
    // Translation is the last column of a column-major matrix.
    let index = match axis {
        "x" => 12,
        "y" => 13,
        "z" => 14,
        _ => return None,
    };
    Some(global.matrix[index])
}

fn parse_assertion(spec: &str) -> Option<(String, String, String, f32)> {
    let mut parts = spec.split_whitespace();
    let target = parts.next()?;
    let op = parts.next()?.to_owned();
    let value: f32 = parts.next()?.parse().ok()?;
    let (path, axis) = target.rsplit_once('.')?;
    Some((path.to_owned(), axis.to_owned(), op, value))
}

/// Evaluate one assertion against final world state.
fn check_assertion(world: &World, spec: &str) -> Result<bool, ()> {
    let (path, axis, op, expected) = parse_assertion(spec).ok_or(())?;
    let actual = assertion_value(world, &path, &axis).ok_or(())?;
    Ok(match op.as_str() {
        ">" => actual > expected,
        ">=" => actual >= expected,
        "<" => actual < expected,
        "<=" => actual <= expected,
        "==" => (actual - expected).abs() < 1e-4,
        // Approximate, for values a simulation lands near rather than on.
        "~=" => (actual - expected).abs() < 0.05,
        _ => return Err(()),
    })
}

fn assertion_actual(world: &World, spec: &str) -> serde_json::Value {
    parse_assertion(spec)
        .and_then(|(path, axis, _, _)| assertion_value(world, &path, &axis))
        .map_or(serde_json::Value::Null, |v| serde_json::json!(v))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn parse_size(spec: &str) -> Option<(u32, u32)> {
    let (w, h) = spec.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Apply a transaction to a scene.
///
/// Reads the transaction as JSON so the same payload works from a shell, from
/// a test, and from the MCP adapter — CLI first, MCP second (§7.10).
fn scene_tx(path: &str, args: &[String]) -> (u8, String) {
    let Some(tx_path) = flag(args, "--tx") else {
        return (
            2,
            json_line(&serde_json::json!({
                "error": "missing_argument",
                "hint": "--tx <file.json> holds the transaction: { label, ops }",
            })),
        );
    };

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "io_error", "path": path, "constraint": e.to_string(),
        }))),
    };
    let tx_text = match std::fs::read_to_string(&tx_path) {
        Ok(s) => s,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "io_error", "path": tx_path, "constraint": e.to_string(),
        }))),
    };
    let mut transaction: loom_scene::Transaction = match serde_json::from_str(&tx_text) {
        Ok(t) => t,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "invalid_transaction",
            "constraint": e.to_string(),
            "hint": "Expected { \"label\": \"...\", \"ops\": [ { \"op\": \"spawn_node\", ... } ] }",
        }))),
    };
    if args.iter().any(|a| a == "--dry-run") {
        transaction.dry_run = true;
    }

    match loom_scene::apply(&src, &transaction) {
        Ok(applied) => {
            // --dry-run prints the diff and touches nothing. This is how the
            // human reviews a large change before it lands.
            if !transaction.dry_run
                && let Err(e) = std::fs::write(path, &applied.scene)
            {
                return (1, json_line(&serde_json::json!({
                    "error": "io_error", "path": path, "constraint": e.to_string(),
                })));
            }
            (
                0,
                json_line(&serde_json::json!({
                    "ok": true,
                    "label": applied.label,
                    "dry_run": transaction.dry_run,
                    "version": applied.version,
                    "diff": applied.diff,
                })),
            )
        }
        Err(e) => (1, json_line(&e)),
    }
}

/// Bake a terrain recipe and report what it produced.
///
/// **The terrain feedback channel** (terrain doc §7). `render_preview` verifies
/// placement and `run_scene` verifies behaviour; terrain needs its own, and it
/// is mostly not visual — most terrain mistakes are invisible in a render and
/// obvious in a slope map. The stats matter more than the pretty picture: a
/// gorgeous mountain range with 3% buildable ground is a failed level, and
/// nothing in a hillshade reveals that.
fn terrain(path: &str, args: &[String]) -> (u8, String) {
    let recipe = match loom_terrain::Recipe::load(std::path::Path::new(path)) {
        Ok(r) => r,
        Err(e) => {
            return (
                1,
                json_line(&serde_json::json!({
                    "error": "invalid_recipe", "path": path, "constraint": e.to_string(),
                })),
            );
        }
    };

    let map = recipe.bake();
    let stats = loom_terrain::analyze(&map);

    let mut written = Vec::new();
    if let Some(prefix) = flag(args, "--out") {
        for (kind, name) in [
            (loom_terrain::analyze::MapKind::Hillshade, "hillshade"),
            (loom_terrain::analyze::MapKind::Slope, "slope"),
            (loom_terrain::analyze::MapKind::Buildable, "buildable"),
            (loom_terrain::analyze::MapKind::Height, "height"),
        ] {
            let file = format!("{prefix}_{name}.png");
            if let Err(e) = loom_terrain::analyze::write_png(&map, kind, std::path::Path::new(&file))
            {
                return (1, json_line(&serde_json::json!({
                    "error": "io_error", "path": file, "constraint": e.to_string(),
                })));
            }
            written.push(file);
        }
    }

    // Traversability, when asked. This closes the loop on the Corridor layer:
    // generate, verify the route, adjust — an agent working on gameplay rather
    // than on aesthetics.
    let reachable = match (
        flag(args, "--from").and_then(|s| parse_vec2(&s)),
        flag(args, "--to").and_then(|s| parse_vec2(&s)),
    ) {
        (Some(from), Some(to)) => Some(loom_terrain::analyze::is_reachable(
            &map,
            from,
            to,
            flag(args, "--max-slope")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20.0),
        )),
        _ => None,
    };

    (
        0,
        json_line(&serde_json::json!({
            "ok": true,
            "recipe": path,
            "content_hash": recipe.content_hash(),
            "size": recipe.size,
            "height": {
                "min": stats.height_min, "max": stats.height_max, "mean": stats.height_mean,
            },
            "slope": {
                "mean": stats.slope_mean, "over_45_pct": stats.slope_over_45_pct,
            },
            "buildable_pct": stats.buildable_pct,
            "largest_flat": stats.largest_flat.map(|(c, area)| serde_json::json!({
                "center": c, "area_px": area,
            })),
            "reachable": reachable,
            // Legal but almost certainly unintended layer ordering. A rejection
            // is the agent's teacher; so is a warning it can act on.
            "warnings": recipe.order_warnings(),
            "maps": written,
        })),
    )
}

fn parse_vec2(spec: &str) -> Option<[usize; 2]> {
    let parts: Vec<usize> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (parts.len() == 2).then(|| [parts[0], parts[1]])
}

/// Blast a voxel surface and render the aftermath.
///
/// The destructible loop end to end: a CSG subtract carves the crater, the
/// removed material becomes debris with outward velocity, the remaining
/// surface becomes a static collider for it to land on, and a frame is
/// rendered per step.
///
/// Debris is boxes, never trimeshes (never-do #10), and capped — uncapped
/// debris is the classic way a destructible game dies.
#[allow(clippy::too_many_lines)]
fn explode(path: &str, args: &[String]) -> (u8, String) {
    const DEBRIS_CAP: usize = 160;

    let Some(at) = flag(args, "--at").and_then(|s| parse_vec3(&s)) else {
        return (2, json_line(&serde_json::json!({
            "error": "missing_argument", "hint": "--at X,Y,Z is the blast centre",
        })));
    };
    let radius = flag(args, "--radius").and_then(|v| v.parse::<f32>().ok()).unwrap_or(3.0);
    let out = flag(args, "--out").unwrap_or_else(|| "blast".to_owned());
    let frames = flag(args, "--frames").and_then(|v| v.parse::<u32>().ok()).unwrap_or(5);
    let (width, height) = flag(args, "--size")
        .and_then(|s| parse_size(&s))
        .unwrap_or((760, 520));

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "io_error", "path": path, "constraint": e.to_string(),
        }))),
    };
    let scene = match Scene::parse(&src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    // Find the voxel volume to blast.
    let Some((node, component)) = scene
        .nodes()
        .iter()
        .find_map(|n| n.components.get("VoxelVolume").map(|c| (n.path.clone(), c)))
    else {
        return (1, json_line(&serde_json::json!({
            "error": "no_voxel_volume", "path": path,
            "hint": "Add a VoxelVolume component to a node first.",
        })));
    };

    let Some((mut volume, _)) = build_volume(component) else {
        return (1, json_line(&serde_json::json!({
            "error": "voxel_bake_failed", "node": node,
        })));
    };

    // 1. Carve. The edit layer records only the chunks the blast touches.
    let blast = loom_voxel::VoxelOp::Sphere {
        center: at,
        radius,
        mode: loom_voxel::CsgMode::Subtract,
    };
    let touched = volume.edit(&blast);
    // §7.9: an edit dirties its chunks AND their neighbours, or the remesh
    // cracks at the seams.
    let dirty = volume.dirty_with_neighbours(&touched);

    // 2. Remesh and build a static collider from what survived.
    let surface = loom_voxel::mesh::mesh_volume(&volume, &loom_voxel::SurfaceNets);
    let mut physics = loom_physics::Physics::new(1.0 / 60.0);
    let positions: Vec<[f32; 3]> = surface
        .vertices
        .iter()
        .map(|v| [v.position[0], v.position[1], v.position[2]])
        .collect();
    physics.add_static_trimesh(&positions, &surface.indices);

    // 3. The removed material becomes debris, thrown outward from the centre.
    let mut rng = loom_terrain::noise::Rng::new(0xB1A57);
    let mut debris = Vec::new();
    let chunk_size = radius * 0.16;
    for _ in 0..DEBRIS_CAP {
        // Sample inside the blast sphere; reject-sample so the distribution is
        // even rather than clustered at the centre.
        let dir = [
            rng.next_f32() * 2.0 - 1.0,
            rng.next_f32() * 2.0 - 1.0,
            rng.next_f32() * 2.0 - 1.0,
        ];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if !(0.2..=1.0).contains(&len) {
            continue;
        }
        let unit = [dir[0] / len, dir[1] / len, dir[2] / len];
        let spawn = [
            at[0] + unit[0] * radius * 0.85,
            at[1] + unit[1] * radius * 0.85,
            at[2] + unit[2] * radius * 0.85,
        ];
        // Outward, biased upward — a blast that only pushes sideways looks
        // like a shove rather than an explosion.
        let speed = 6.0 + rng.next_f32() * 9.0;
        let velocity = [
            unit[0] * speed,
            unit[1].mul_add(speed, 6.0),
            unit[2] * speed,
        ];
        if let Some(handle) = physics.spawn_debris(spawn, chunk_size, velocity, DEBRIS_CAP + 1) {
            debris.push(handle);
        }
    }

    // 4. Step and render.
    let meshes = vec![surface, loom_asset::primitives::box_mesh()];
    let instance = match loom_render::Instance::new(c"loom") {
        Ok(i) => i,
        Err(e) => return (1, json_line(&serde_json::json!({ "error": "render_failed", "constraint": e.to_string() }))),
    };
    let device = match loom_render::Device::new(&instance) {
        Ok(d) => d,
        Err(e) => return (1, json_line(&serde_json::json!({ "error": "render_failed", "constraint": e.to_string() }))),
    };
    let mut renderer = match Renderer::new(&instance, &device, width, height, &meshes) {
        Ok(r) => r,
        Err(e) => return (1, json_line(&serde_json::json!({ "error": "render_failed", "constraint": e.to_string() }))),
    };

    // Framed once, from the terrain, and held still — a camera that reframes
    // every step would hide the debris flying by moving with it.
    let camera = Camera {
        eye: Vec3::new(at[0] + radius * 4.0, at[1] + radius * 2.6, at[2] + radius * 5.5),
        target: Vec3::new(at[0], at[1], at[2]),
        fov_y_degrees: 50.0,
    };

    let mut written = Vec::new();
    // Steps between frames. Enough that consecutive frames differ visibly —
    // a frame per tick would be sixty near-identical images.
    let steps_per_frame = flag(args, "--steps")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(6);
    for frame in 0..frames {
        if frame > 0 {
            for _ in 0..steps_per_frame {
                physics.step();
            }
        }
        let mut objects = vec![Object {
            model: Mat4::IDENTITY,
            color: [0.42, 0.46, 0.52],
            mesh: 0,
        }];
        for handle in &debris {
            let (Some(p), Some(r)) = (physics.position(*handle), physics.rotation_euler(*handle))
            else {
                continue;
            };
            objects.push(Object {
                model: Mat4::from_scale_rotation_translation(
                    Vec3::splat(chunk_size),
                    loom_render::glam::Quat::from_euler(
                        loom_render::glam::EulerRot::YXZ,
                        r[1].to_radians(),
                        r[0].to_radians(),
                        r[2].to_radians(),
                    ),
                    Vec3::from_array(p),
                ),
                color: [0.78, 0.42, 0.24],
                mesh: 1,
            });
        }

        let file = format!("{out}_{frame}.png");
        if let Err(e) = renderer.render_to_png(&objects, &camera, std::path::Path::new(&file)) {
            return (1, json_line(&serde_json::json!({
                "error": "render_failed", "constraint": e.to_string(),
            })));
        }
        written.push(file);
    }

    (
        0,
        json_line(&serde_json::json!({
            "ok": true,
            "node": node,
            "chunks_carved": touched.len(),
            "chunks_remeshed": dirty.len(),
            "debris": debris.len(),
            "frames": written,
        })),
    )
}

/// Build a voxel volume from a component, returning it and its mesh.
pub(crate) fn build_volume(component: &serde_json::Value) -> Option<(loom_voxel::Volume, ())> {
    #[allow(clippy::cast_possible_truncation)]
    let voxel_size = component.get("voxel_size").and_then(serde_json::Value::as_f64)? as f32;
    let dims: Vec<usize> = component
        .get("chunks")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();
    if dims.len() != 3 {
        return None;
    }
    let ops: Vec<loom_voxel::VoxelOp> = component
        .get("ops")?
        .as_array()?
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    if ops.is_empty() {
        return None;
    }
    let mut volume = loom_voxel::Volume::new([dims[0], dims[1], dims[2]], voxel_size);
    volume.bake(&ops);
    Some((volume, ()))
}

fn parse_vec3(spec: &str) -> Option<[f32; 3]> {
    let parts: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]])
}

/// Resolve a semantic placement into ops and apply them.
///
/// Design doc §2.8: the agent says "put the monitor on the desk", and engine
/// code does the arithmetic. It never computes a world coordinate, which is
/// the failure that leaves a monitor floating above a desk — or, as the
/// unplaced state here shows, buried inside one.
fn place(path: &str, args: &[String]) -> (u8, String) {
    let Some(op_path) = flag(args, "--op") else {
        return (2, json_line(&serde_json::json!({
            "error": "missing_argument",
            "hint": "--op <file.json>, e.g. { \"place\": \"place_on\", \"node\": ..., \"surface\": ... }",
        })));
    };
    let (src, op_text) = match (std::fs::read_to_string(path), std::fs::read_to_string(&op_path)) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => {
            let e = a.err().or(b.err()).map(|e| e.to_string()).unwrap_or_default();
            return (2, json_line(&serde_json::json!({ "error": "io_error", "constraint": e })));
        }
    };
    let ops: Vec<loom_scene::PlaceOp> = match serde_json::from_str::<serde_json::Value>(&op_text)
        .and_then(|v| if v.is_array() { serde_json::from_value(v) } else { serde_json::from_value(v).map(|o| vec![o]) })
    {
        Ok(o) => o,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "invalid_placement", "constraint": e.to_string(),
        }))),
    };

    let scene = match Scene::parse(&src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };
    let base = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);
    let world = World::from_scene(&scene);
    let boxes = node_bounds(&world, &library);

    let lookup = |name: &str| boxes.get(name).copied();
    let geometry = loom_scene::place::Geometry {
        bounds_of: &lookup,
        parent: scene.nodes().first().map(|n| n.path.clone()).unwrap_or_default(),
    };

    let mut scene_ops = Vec::new();
    for op in &ops {
        match loom_scene::place::resolve(op, &geometry) {
            Ok(mut resolved) => scene_ops.append(&mut resolved),
            Err(e) => return (1, json_line(&e)),
        }
    }
    // `resolve` reasons in world space, because that is where "on top of" and
    // "facing" mean anything — but `SetTransform` writes a node's **local**
    // transform. The two are the same only when the parent is identity, so a
    // desk placed on a floor inside a moved room landed at the room's offset
    // twice over. The conversion lives here rather than in `loom_scene`,
    // which has no business knowing about the ECS hierarchy.
    to_parent_space(&mut scene_ops, &world);

    let transaction = loom_scene::Transaction {
        label: format!("Place: {} op(s)", ops.len()),
        ops: scene_ops,
        dry_run: args.iter().any(|a| a == "--dry-run"),
        expect_version: None,
    };
    match loom_scene::apply(&src, &transaction) {
        Ok(applied) => {
            if !transaction.dry_run
                && let Err(e) = std::fs::write(path, &applied.scene)
            {
                return (1, json_line(&serde_json::json!({
                    "error": "io_error", "constraint": e.to_string(),
                })));
            }
            (0, json_line(&serde_json::json!({
                "ok": true, "placed": ops.len(),
                "dry_run": transaction.dry_run,
                "version": applied.version,
            })))
        }
        Err(e) => (1, json_line(&e)),
    }
}

/// Bounds and overlaps, so the agent can check itself numerically.
///
/// Cheaper than a render and catches what one camera angle hides — an object
/// inside another object (design doc §2.8).
fn measure(path: &str, args: &[String]) -> (u8, String) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (2, json_line(&serde_json::json!({
            "error": "io_error", "path": path, "constraint": e.to_string(),
        }))),
    };
    let scene = match Scene::parse(&src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let base = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);
    let world = World::from_scene(&scene);
    let boxes = node_bounds(&world, &library);

    if let Some(node) = flag(args, "--node") {
        return match boxes.get(&node) {
            Some(b) => (0, json_line(&serde_json::json!({
                "node": node, "min": b.min, "max": b.max,
                "center": b.center(), "size": b.size(),
            }))),
            None => (1, json_line(&serde_json::json!({
                "error": "no_geometry", "node": node,
                "hint": "The node must exist and carry a MeshRenderer.",
            }))),
        };
    }

    // Every interpenetrating pair. This is the check that catches a monitor
    // buried in a desk, which a single camera angle would hide.
    let names: Vec<&String> = boxes.keys().collect();
    let mut overlaps = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            if boxes[*a].overlaps(&boxes[*b]) {
                overlaps.push(serde_json::json!({ "a": a, "b": b }));
            }
        }
    }

    (
        0,
        json_line(&serde_json::json!({
            "ok": true,
            "nodes": names.len(),
            "bounds": boxes.iter().map(|(k, v)| serde_json::json!({
                "node": k, "center": v.center(), "size": v.size(),
            })).collect::<Vec<_>>(),
            "overlaps": overlaps,
        })),
    )
}

/// Which mesh an entity draws.
///
/// **One lookup, used by both the renderer and the measurer.** They disagreed
/// once — bounds resolved a voxel node to the default unit box while the draw
/// used the real mesh — and the result was a camera framed four units from a
/// twenty-four-unit hill, i.e. inside it. Two copies of a lookup is two
/// answers.
fn mesh_index_for(world: &World, library: &MeshLibrary, entity: loom_ecs::Entity) -> u32 {
    if let Some(path) = world.path(entity) {
        let voxel = format!("voxel:{path}");
        if library.by_name.contains_key(&voxel) {
            return library.index_for(Some(&voxel));
        }
    }
    library.index_for(world.mesh_asset(entity))
}

/// Centre and radius of everything in the scene, from real mesh bounds.
pub(crate) fn scene_bounds(
    boxes: &std::collections::BTreeMap<String, loom_scene::place::Bounds>,
) -> (Vec3, f32) {
    if boxes.is_empty() {
        return (Vec3::ZERO, 4.0);
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for b in boxes.values() {
        min = min.min(Vec3::from_array(b.min));
        max = max.max(Vec3::from_array(b.max));
    }
    ((min + max) * 0.5, (max - min).length() * 0.5)
}

/// World bounds per renderable node, from its mesh and its global transform.
pub(crate) fn node_bounds(
    world: &World,
    library: &MeshLibrary,
) -> std::collections::BTreeMap<String, loom_scene::place::Bounds> {
    let mut out = std::collections::BTreeMap::new();
    for entity in world.entities() {
        if !world.is_renderable(*entity) {
            continue;
        }
        let (Some(global), Some(path)) = (world.global_transform(*entity), world.path(*entity))
        else {
            continue;
        };
        let index = mesh_index_for(world, library, *entity) as usize;
        let Some(mesh) = library.meshes.get(index) else {
            continue;
        };
        let (lo, hi) = mesh.bounds();
        let model = Mat4::from_cols_array(&global.matrix);

        // Transform all eight corners: a rotated box's world AABB is not its
        // local AABB rotated.
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            );
            let p = model.transform_point3(corner);
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        out.insert(path.to_owned(), loom_scene::place::Bounds { min, max });
    }
    out
}

/// Rewrite world-space `SetTransform` positions into each node's parent space.
///
/// Rotation and scale are left alone: semantic placement only ever sets a
/// position, and silently reinterpreting an authored rotation would be a
/// second bug wearing this one's clothes.
fn to_parent_space(ops: &mut [loom_scene::SceneOp], world: &World) {
    for op in ops {
        let loom_scene::SceneOp::SetTransform { node, pos: Some(p), .. } = op else {
            continue;
        };
        let parent_inverse = world
            .entities()
            .iter()
            .find(|e| world.path(**e) == Some(node.as_str()))
            .and_then(|e| world.parent(*e))
            .and_then(|parent| world.global_transform(parent))
            .map_or(Mat4::IDENTITY, |g| {
                play::invertible_parent(Mat4::from_cols_array(&g.matrix))
            });

        // A position, so `transform_point3` — translation included. Using the
        // vector form here would drop the parent's offset and look almost
        // right, which is worse.
        let local = parent_inverse.transform_point3(Vec3::from_array(*p));
        *p = local.to_array();
    }
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

    /// Semantic placement reasons in world space — "on top of" means nothing
    /// otherwise — but `SetTransform` writes a node's local transform. Inside a
    /// room that has been moved, the two differ, and the placed node landed at
    /// the room's offset twice over.
    #[test]
    fn placing_inside_a_moved_parent_lands_on_the_surface() {
        let dir = std::env::temp_dir().join("loom_place_nested");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("nested.loom");
        std::fs::write(
            &scene,
            r#"
[scene]
format = 1
id = "9e8d7c6b-5a49-4382-91f0-2b3c4d5e6f70"

[[node]]
name = "World"

[[node]]
name = "Room"
parent = "World"
transform = { pos = [30.0, 0.0, -12.0] }

[[node]]
name = "Floor"
parent = "World/Room"
transform = { scale = [4.0, 0.2, 4.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }

[[node]]
name = "Lamp"
parent = "World/Room"
transform = { pos = [0.0, 9.0, 0.0], scale = [0.3, 0.3, 0.3] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }
"#,
        )
        .expect("write scene");
        let op = dir.join("op.json");
        std::fs::write(
            &op,
            r#"{"place":"place_on","node":"World/Room/Lamp","surface":"World/Room/Floor","anchor":"center"}"#,
        )
        .expect("write op");

        let (code, out) = run(&args(&[
            "place",
            scene.to_str().expect("path"),
            "--op",
            op.to_str().expect("path"),
        ]));
        assert_eq!(code, 0, "{out}");

        let text = std::fs::read_to_string(&scene).expect("read back");
        let parsed = Scene::parse(&text).expect("still valid");
        let lamp = parsed
            .nodes()
            .iter()
            .find(|n| n.path == "World/Room/Lamp")
            .expect("lamp");

        // Local, not world: the room's offset must not appear here.
        assert!(
            lamp.transform.pos[0].abs() < 0.01 && lamp.transform.pos[2].abs() < 0.01,
            "the parent offset leaked into the local transform: {:?}",
            lamp.transform.pos
        );
        // Floor top is 0.2, the lamp's half-height 0.3.
        assert!(
            (lamp.transform.pos[1] - 0.5).abs() < 0.01,
            "should sit on the floor: {:?}",
            lamp.transform.pos
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`loom sim` must actually simulate.** It stepped scripts and nothing
    /// else, so `--assert` on a physics scene checked the *authored*
    /// positions: a wrecking ball asserted to be on the floor after four
    /// seconds passed while still sitting at its starting height. The picture
    /// (`loom render --sim`) had physics and the assertions did not, which is
    /// the worse half of that pair — a render you can eyeball versus a claim
    /// you cannot.
    #[test]
    fn sim_steps_physics_so_assertions_mean_something() {
        let tower = "../../assets/test/tower.loom";
        // Authored at y = 7.0, above a stack it knocks over.
        let (code, out) = run(&args(&[
            "sim",
            tower,
            "--ticks",
            "300",
            "--assert",
            "Yard/Wrecker.y < 3.0",
        ]));

        assert_eq!(code, 0, "the ball should have fallen: {out}");
    }

    /// And the same run must still be reproducible.
    #[test]
    fn simulating_physics_twice_gives_the_same_hash() {
        let tower = "../../assets/test/tower.loom";
        let (_, a) = run(&args(&["sim", tower, "--ticks", "180"]));
        let (_, b) = run(&args(&["sim", tower, "--ticks", "180"]));

        assert_eq!(a, b, "physics must be deterministic across runs");
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
