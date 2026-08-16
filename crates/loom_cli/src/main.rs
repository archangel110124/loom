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
mod hud;
mod imagediff;
mod log;
mod materials;
mod panels;
mod particles;
mod play;
mod prefab_cmd;
mod prefab_load;
mod scene_view;
mod telemetry;
mod weather;
mod sound;
mod run;

use std::process::ExitCode;

use loom_ecs::{FixedTimestep, World};
use loom_render::glam::{Mat4, Vec3};
use loom_render::{Camera, Device, Instance, Object, Renderer};
use loom_scene::{Scene, components};

const USAGE: &str = "\
loom — AI-native engine CLI

Every command prints one line of JSON. Exit 0 ok · 1 the thing was invalid ·
2 the invocation was wrong.

USAGE:
    loom validate <scene.loom>
        Validate a scene and report its version token. Exit 1 with JSON errors.

    loom describe <TypeName>
        Print a component's JSON Schema. No argument lists the known types.

    loom render <scene.loom> [--out <f.png>] [--size <WxH>] [--sim <ticks>]
                             [--yaw <deg>] [--pitch <deg>]
        Render the scene headless to a PNG. Uses the scene's `Camera` node if
        it has one; --yaw/--pitch overrides it and orbits the bounds instead.

    loom render <scene.loom> --frames <n> [--spin <deg>] [--step <ticks>]
                             [--dolly <m>]
        Dump a numbered frame sequence along a deterministic path, advancing
        the simulation between frames. Motion artifacts — shimmer, popping,
        unison sway — are invisible in a still. --spin turns the camera in
        place; --dolly walks it forward, which is the only one of the two that
        produces parallax.

    loom compare <a.png> <b.png> [--channel <0-255>] [--fraction <0-1>] [--worst <0-255>]
        Pixel-compare two renders. Exit 1 if they differ beyond tolerance.

    loom sim <scene.loom> [--ticks <n>] [--assert <expr>]
        Step physics deterministically and print the state hash. --assert also
        reads the weather where it is asked about: `wind@x,y,z.speed >= 3`, and
        `rain@x,y,z.rate < 0.05` for the rain reaching a point in mm/h after the
        scene's own voxels have sheltered it (`.exposure` is that shelter alone,
        0 sealed to 1 open sky). `wetness@x,y,z.film` is how wet a surface there
        has become by that tick — `.film` the sheen, `.soak` the darkening, and
        the two dry at different rates.

    loom scene <scene.loom> --tx <tx.json> [--dry-run]
        Apply a transaction. `expect_version` in the JSON makes the write
        conditional; --dry-run prints the diff and writes nothing.

    loom flicker <a.png> <b.png> <c.png>
        Temporal noise in the middle frame: |b - (a+c)/2|, averaged. Motion is
        smooth over three frames and cancels; a pixel that twinkles does not.
        This is the anti-aliasing measurement — `compare` cannot make it.

    loom audio <scene.loom> [--seconds <n>] [--openness <0-1>] [--out <x.wav>]
        Render the scene's weather bed offline and measure it: rms, peak, and
        tilt (high-band over low-band energy — the number that says `darker`).
        Rain is synthesised, so this is reproducible. --openness is the shelter
        to render at; the live path takes it from the acoustics solve.

    loom prefab <verb> <scene.loom> --node <path> [--key <Type.field> ...]
        The three prefab operations. `revert-overrides` puts an instance back
        to the prefab; `apply-overrides` promotes its deviations into the
        prefab so every instance gains them (two files, two undo steps);
        `unpack` replaces the instance with concrete nodes and stops it
        tracking the prefab. Naming no --key means all of them.

    loom place <scene.loom> --op <op.json> [--dry-run] [--expect-version <tok>]
        Geometry-aware placement: on top of, aligned to, facing, grid on.

    loom measure <scene.loom> [--node <path>]
        Bounds and overlaps, so a change can be checked without a render.

    loom measure <scene.loom|mesh.obj|mesh.gltf> --shape [--node <path>]
        How rocky a shape is, instead of rendering it and looking. Reports
        concave surface-area fraction and the spread of curvature magnitudes:
        a scanned rock is 42-44% concave over 1.2-1.5 decades, a ridged
        displaced sphere 47%/1.53, a fifty-primitive union 8.7%/0.53. It is
        REPORT-ONLY and must never become a gate — the number is
        resolution-dependent, so only ever compare shapes at equal voxel_size,
        which is why every row carries its own.

    loom water <scene.loom> --at <x,z|x,y,z> [--sim <ticks>]
        What the water is doing at one point: surface height, normal, depth to
        the bed, and velocity. --sim picks the instant, exactly as it does for
        render; without it the answer is t = 0, which is the one moment every
        wave is at the same phase. A third component in --at asks whether that
        height is under the surface.

    loom terrain <scene.loom|recipe.toml> [--out <prefix>] [--from <x,z>]
                              [--to <x,z>] [--max-slope <deg>]
        Measure a landform: buildable_pct, slope_mean, largest_flat, and
        whether a walkable route exists between two points. Given a scene, it
        reads the recipe that scene's `terrain` op names. --from/--to are
        recipe pixels, not world metres. This is the only assertable feedback
        a landscape has — a gorgeous range with 3% buildable ground is a
        failed level, and nothing in a hillshade reveals that.

    loom explode <scene.loom> --at <x,y,z> --radius <m> [--out <f.png>]
                              [--frames <n>] [--size <WxH>] [--steps <n>]
        Carve the voxel terrain and render the result.

    loom run <scene.loom> [--edit] [--frames <n>] [--play]
        Open the viewer. --edit gives the full editor; it reloads on change.
        --frames closes after n frames and then prints the frame's CPU cost;
        --play starts the simulation immediately, which is the only way to
        measure the per-frame work a running game actually does.
";

/// The flags each subcommand accepts, and whether each takes a value.
///
/// **An unknown flag is a failed invocation, not a no-op.** Ignoring them
/// meant `--frame 3` (singular) rendered the default frame and reported
/// success, and a misspelled `--dry-run` wrote the file for real. The agent
/// cannot tell from the output that it did something other than what it asked
/// for, which is the failure mode this whole CLI exists to avoid.
const FLAGS: &[(&str, &[(&str, bool)])] = &[
    ("validate", &[]),
    ("describe", &[]),
    (
        "render",
        &[
            ("--out", true), ("--size", true), ("--sim", true), ("--yaw", true),
            ("--pitch", true), ("--frames", true), ("--spin", true), ("--step", true),
            ("--dolly", true),
        ],
    ),
    (
        "compare",
        &[("--channel", true), ("--fraction", true), ("--worst", true)],
    ),
    ("sim", &[("--ticks", true), ("--assert", true)]),
    ("scene", &[("--tx", true), ("--dry-run", false)]),
    ("place", &[("--op", true), ("--dry-run", false), ("--expect-version", true)]),
    ("measure", &[("--node", true), ("--shape", false)]),
    ("water", &[("--at", true), ("--sim", true)]),
    (
        "terrain",
        &[("--out", true), ("--from", true), ("--to", true), ("--max-slope", true)],
    ),
    (
        "explode",
        &[
            ("--at", true), ("--radius", true), ("--out", true), ("--frames", true),
            ("--size", true), ("--steps", true),
        ],
    ),
    ("run", &[("--edit", false), ("--frames", true), ("--play", false)]),
];

/// The first unrecognised flag in `args`, if any.
///
/// Values are skipped rather than inspected, so `--node --edit` reports
/// nothing: `--edit` is that node's name, however odd, not a flag.
fn unknown_flag(command: &str, args: &[String]) -> Option<(String, Vec<&'static str>)> {
    let allowed = FLAGS.iter().find(|(name, _)| *name == command)?.1;

    // args[0] is the path or type name; flags follow.
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some((_, takes_value)) = allowed.iter().find(|(name, _)| *name == arg) {
            i += if *takes_value { 2 } else { 1 };
        } else if arg.starts_with("--") {
            return Some((arg.to_owned(), allowed.iter().map(|(n, _)| *n).collect()));
        } else {
            i += 1;
        }
    }
    None
}

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
    if let Some(command) = args.first()
        && let Some((flag, allowed)) = unknown_flag(command, &args[1..])
    {
        return (
            2,
            json_line(&serde_json::json!({
                "error": "unknown_flag",
                "value": flag,
                "constraint": format!("a flag of `loom {command}`"),
                "hint": if allowed.is_empty() {
                    format!("`loom {command}` takes no flags")
                } else {
                    format!("`loom {command}` takes: {}", allowed.join(", "))
                },
            })),
        );
    }

    // Every subcommand takes its subject as the first argument, so this is the
    // one place that knows which directory a scene's `[[asset]]` paths and
    // terrain recipes are written relative to. See `SCENE_BASE`.
    if let Some(path) = args.get(1) {
        set_scene_base(path);
    }

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
        Some("compare") => match (args.get(1), args.get(2)) {
            (Some(a), Some(b)) => compare(a, b, args),
            _ => (2, USAGE.to_owned()),
        },
        Some("sim") => match args.get(1) {
            Some(path) => sim(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("scene") => match args.get(1) {
            Some(path) => scene_tx(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("flicker") => match (args.get(1), args.get(2), args.get(3)) {
            (Some(a), Some(b), Some(c)) => flicker(a, b, c),
            _ => (2, USAGE.to_owned()),
        },
        Some("audio") => match args.get(1) {
            Some(path) => audio_cmd(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("prefab") => prefab_cmd::run(args),
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
        Some("water") => match args.get(1) {
            Some(path) => water(path, args),
            None => (2, USAGE.to_owned()),
        },
        Some("run") => match args.get(1) {
            Some(path) => match run::open_scene(
                path,
                args.iter().any(|a| a == "--edit"),
                flag(args, "--frames").and_then(|n| n.parse::<u32>().ok()),
                args.iter().any(|a| a == "--play"),
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
            // **The one place override warnings surface.** A prefab that
            // renamed a child leaves an override pointing at nothing; §5 says
            // that is a loud warning with the value preserved, never a silent
            // drop, and `validate` is where an author goes looking.
            let (scene, prefab_warnings) =
                match prefab_load::for_reading_with_warnings(&scene, std::path::Path::new(path)) {
                    Ok(pair) => pair,
                    Err(errors) => {
                        return (1, json_line(&serde_json::json!({ "errors": errors })));
                    }
                };

            // An alias that resolves to nothing is a `docs/format/README.md` §6
            // error code, but the renderer deliberately substitutes a box and
            // carries on (design doc §2.6: degrade, do not crash). Both are
            // right — a broken asset should not stop a render, and it must not
            // be invisible to the agent that wrote the alias either. So the
            // report lives here rather than in `MeshLibrary`.
            let base = std::path::Path::new(path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            let (unresolved, missing) = alias_report(&scene, base);
            if !unresolved.is_empty() {
                return (1, json_line(&serde_json::json!({ "errors": unresolved })));
            }

            // **The op list is schema-checked here and nowhere else.** It rides
            // on the component as free-form JSON, so `Scene::parse` never looks
            // inside it: a misspelled `kind` reached the bake, which used to
            // drop it silently. No volume is baked here — though a `terrain`
            // op's recipe is, which is what makes `validate` catch a missing
            // or mis-scaled recipe rather than leaving it to a render.
            let voxel_errors: Vec<serde_json::Value> = scene
                .nodes()
                .iter()
                .filter_map(|node| {
                    let e = parse_ops(node.components.get("VoxelVolume")?).err()?;
                    Some(serde_json::json!({
                        "error": "invalid_voxel_op",
                        "node": node.path,
                        "field": "VoxelVolume.ops",
                        "constraint": e,
                        "hint": "`kind` is one of sphere, box, capsule, heightfield, terrain, \
                                 each with its own fields. The volume is refused whole rather \
                                 than baked without this op: a volume short one op still \
                                 renders, which is why this cannot be a warning.",
                    }))
                })
                .collect();
            if !voxel_errors.is_empty() {
                return (1, json_line(&serde_json::json!({ "errors": voxel_errors })));
            }

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
                    // The read side of read-modify-write. Without this no
                    // command reported a token, so `expect_version` could not
                    // be filled in through the intended interface and every
                    // agent write ran with the staleness check disabled.
                    "version": loom_scene::VersionToken::of(&src),
                    "physics": findings,
                    "assets": missing,
                    "overrides": prefab_warnings,
                })),
            )
        }
        // Every violation, not just the first — one round-trip per fix is the
        // retry loop `docs/format/README.md` §6 exists to avoid.
        Err(errors) => (1, json_line(&serde_json::json!({ "errors": errors }))),
    }
}

/// Aliases that do not resolve, split by whose problem they are.
///
/// **An alias nothing declares is an error** — a typo in the scene, and the
/// scene is what the agent controls. **A declared alias whose file is not
/// there is a warning**: the text is right and the workspace is incomplete,
/// which is an ordinary state during import and not something to reject a
/// scene over.
///
/// Either way it has to be *said*. The renderer substitutes mesh 0, a unit
/// box, and carries on (design doc §2.6: degrade, do not crash) — which is
/// right for a render and useless as feedback, because a scene full of
/// stand-in boxes looks exactly like a scene that loaded.
fn alias_report(
    scene: &Scene,
    base: &std::path::Path,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    // Every field that names an asset, not just the mesh. A mistyped texture
    // alias used to be invisible: the material simply rendered untextured,
    // which looks exactly like a material that was never given a map.
    const REFERENCES: [(&str, &str); 3] = [
        ("MeshRenderer", "mesh"),
        ("Material", "albedo_map"),
        ("Material", "normal_map"),
    ];

    for node in scene.nodes() {
        for (component, field) in REFERENCES {
            let Some(alias) = node
                .components
                .get(component)
                .and_then(|c| c.get(field))
                .and_then(|m| m.get("asset"))
                .and_then(serde_json::Value::as_str)
                // An empty alias is how a material says "no texture here",
                // which is the default and not a broken reference.
                .filter(|a| !a.is_empty())
            else {
                continue;
            };

            // Primitives resolve procedurally and need no `[[asset]]` entry.
            // There is no equivalent library for textures, so this escape is
            // for meshes only.
            let mesh = component == "MeshRenderer";
            if mesh && loom_asset::primitives::build(alias).is_some() {
                continue;
            }

            match scene.asset_path(alias) {
                None => errors.push(serde_json::json!({
                    "error": "unresolved_alias",
                    "node": node.path,
                    "field": format!("{component}.{field}.asset"),
                    "value": alias,
                    "constraint": if mesh {
                        "an alias declared in [[asset]], or a primitive name"
                    } else {
                        "an alias declared in [[asset]]"
                    },
                    "hint": if mesh {
                        format!(
                            "no `[[asset]]` declares `{alias}`. Either add one, or use a \
                             primitive: box, plane, sphere, cylinder, capsule."
                        )
                    } else {
                        format!("no `[[asset]]` declares `{alias}`. Add one pointing at the texture file.")
                    },
                })),
                Some(p) if !base.join(p).exists() => warnings.push(serde_json::json!({
                    "warning": "asset_file_missing",
                    "node": node.path,
                    "value": alias,
                    "path": p,
                    "hint": if mesh {
                        "the scene declares this asset but the file is not there; \
                         a unit box is drawn in its place."
                    } else {
                        "the scene declares this texture but the file is not there; \
                         the material falls back to its untextured albedo."
                    },
                })),
                Some(_) => {}
            }
        }
    }
    (errors, warnings)
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
    let weather = weather::wind_of(&scene);
    // Prefab instances become the nodes they stand for before anything looks
    // at the tree. Without this a `prefab = "..."` node reaches the renderer
    // with no components and draws nothing, silently.
    let scene = match prefab_load::for_reading(&scene, std::path::Path::new(path)) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let mut world = World::from_scene(&scene);
    let base = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);
    let material_library = materials::MaterialLibrary::for_scene(&world, &scene, base);

    // --sim steps physics before drawing, which is what makes a still image a
    // useful view of a simulation rather than only of its initial state.
    //
    // Explosions a script set off during that run come back here, so the
    // fireball is drawn where the blast happened rather than only its effect
    // on the crates being visible.
    let mut fired = Vec::new();
    let mut splashed = Vec::new();
    if let Some(ticks) = flag(args, "--sim").and_then(|v| v.parse::<u32>().ok()) {
        (fired, splashed) = simulate_physics(&mut world, base, ticks);
    }

    let mut objects = world_to_objects(&world, &library, &material_library);
    // Scattered instances are objects like any other, appended after the
    // scene's own nodes — the renderer batches consecutive same-mesh objects
    // into one instanced draw, so a field of ten thousand trees is one call.
    objects.extend(scatter_objects(&scene, &library));
    let particles = particles::simulate(
        &world,
        &weather,
        flag(args, "--sim").and_then(|v| v.parse::<u32>().ok()),
        &fired,
        &splashed,
    );
    // `--sim` as a tick count, which is what the rain simulation's clock is.
    let sim_ticks = flag(args, "--sim").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let yaw = flag(args, "--yaw").and_then(|v| v.parse::<f32>().ok());
    let pitch = flag(args, "--pitch").and_then(|v| v.parse::<f32>().ok());
    let frames = flag(args, "--frames").and_then(|v| v.parse::<u32>().ok()).filter(|n| *n > 0);
    // Degrees of orbit per frame, and simulation ticks between them. Defaults
    // chosen so a dozen frames sweep a visible arc and advance a fifth of a
    // second each — enough for a shimmer or a pop to show up between two.
    let spin = flag(args, "--spin").and_then(|v| v.parse::<f32>().ok()).unwrap_or(6.0);
    let step = flag(args, "--step").and_then(|v| v.parse::<u32>().ok()).unwrap_or(12);
    // Metres the camera WALKS forward between frames, along its own heading and
    // level with the ground.
    //
    // **Translation, which `--spin` deliberately is not.** A pan is a pure
    // rotation about a fixed eye, chosen because it produces no parallax and so
    // lets `loom flicker` see twinkle rather than motion. That makes it the
    // wrong instrument for anything whose artifact IS parallax — a
    // camera-locked volume, a density front travelling with the eye, a field
    // that reads as a flat sheet sliding. Rain is all three, and the human's
    // report was specifically "when you move".
    //
    // Level rather than along the view vector, because that is what walking is:
    // a camera tilted down at the ground must not burrow into it over sixteen
    // frames.
    let dolly = flag(args, "--dolly").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    // An authored `Camera` is the view, unless the caller asked for an angle.
    //
    // Both halves matter. A scene that places a camera means it, and rendering
    // it from somewhere else makes the component useless for checking a shot.
    // But `--yaw`/`--pitch` is how the agent looks at a scene from a second
    // angle (design doc §2.10, "one render is a lie"), and that has to keep
    // working on a scene that has a camera in it.
    let orbiting = yaw.is_some() || pitch.is_some();
    let camera = match world.active_camera().filter(|_| !orbiting) {
        Some(view) => Camera {
            eye: Vec3::from_array(view.eye),
            target: Vec3::from_array(view.target),
            fov_y_degrees: view.fov_y_degrees,
        },
        // Framed from REAL mesh bounds. Assuming a unit cube was fine while
        // every mesh was one; a voxel volume spans tens of units and put the
        // camera inside the terrain.
        None => frame_scene(
            &node_bounds(&world, &library),
            yaw.unwrap_or(35.0),
            pitch.unwrap_or(28.0),
        ),
    };

    // The clock the wind is sampled at. `--sim` advances it, so a still of a
    // simulated scene shows the grass bent the way that moment's wind bends it.
    #[allow(clippy::cast_precision_loss)]
    let wind_seconds = flag(args, "--sim")
        .and_then(|v| v.parse::<u32>().ok())
        .map_or(0.0, |t| t as f32 / 60.0);
    // The bed, baked once: the water reads its depth from it on the GPU and
    // the submersion test reads it on the CPU, and they must be the same grid.
    let terrain = scene_terrain_field(&scene);
    // The scene's rain, resolved once. `None` for a scene that authors none,
    // which is dry rather than drizzly. **No sky volume is built for it any
    // more**: the layer is handed the unsheltered rate and the height field in
    // the shader decides which drops the world stops, so nothing in the render
    // path marches the SDF — see `rain_at_eye`.
    let rain = weather::rain_of(&scene);
    let mut environment = environment_with_wind(&world, &weather, wind_seconds);
    stamp_flipbook(&mut environment, &material_library);
    submerge_eye(&mut environment, &world, &weather, terrain.as_ref(), camera.eye, wind_seconds);
    let rain_drops =
        rain_at_eye(&mut environment, rain.as_ref(), &weather, camera.eye, wind_seconds);
    // Where the rain is landing is no longer decided here. Splashes are
    // collisions the drop simulation resolved on the GPU, appended to a ring
    // and drawn indirectly — ADR 0015, and ADR 0014's trigger 3.
    let result = (|| -> Result<(String, bool), String> {
        let instance = Instance::new(c"loom").map_err(|e| e.to_string())?;
        let device = Device::new(&instance).map_err(|e| e.to_string())?;
        let name = format!(
            "{}{}",
            device.name(),
            if device.supports_raytracing() { "" } else { " (no ray query)" }
        );
        let raytracing = device.supports_raytracing();
        let mut renderer = Renderer::new(
            &instance,
            &device,
            width,
            height,
            &library.meshes,
            &material_library.textures,
            &material_library.materials,
        )
        .map_err(|e| e.to_string())?;
        renderer.environment = environment;
        // Placement is a pure function of position, so the blades go up once
        // and the vertex shader re-expands and re-bends them every frame.
        let blades = grass_blades(&scene);
        if !blades.is_empty() {
            warn_if_grass_truncated(blades.len(), renderer.grass_capacity());
            renderer.set_grass(&blades).map_err(|e| e.to_string())?;
        }
        // The bed the water reads its depth from, and the surface rain stops
        // at — uploaded once, because a bake of the voxel SDF changes only when
        // the terrain does.
        if let Some(field) = terrain.as_ref() {
            renderer
                .set_terrain(&field.height, field.origin, field.spacing, field.side)
                .map_err(|e| e.to_string())?;
        }
        renderer.set_rain(rain_drops);
        // **The drop simulation's clock.** A fresh renderer seeds at tick zero
        // and advances to here in one dispatch, so a still at `--sim N` is
        // always seed-then-advance-N and reproduces run to run (ADR 0015).
        renderer.set_rain_tick(u64::from(sim_ticks));
        // And the world those drops collide with, baked once. This is what
        // fires ADR 0014's trigger 2: the field is the collision world — voxel
        // volumes unioned with static box colliders — so a mesh roof stops rain
        // where the baked height field never could.
        if let Some(field) = rain_collision_field(&scene, &world) {
            renderer
                .set_rain_field(&field.sdf, field.dims, field.origin, field.spacing)
                .map_err(|e| e.to_string())?;
        }

        match frames {
            // The still. One image, the scene as `--sim` left it.
            None => renderer
                .render_to_png(&objects, &particles, &camera, std::path::Path::new(&out))
                .map_err(|e| e.to_string())?,

            // **The fly-through, which is the part that matters.** A still
            // cannot show unison sway, swimming vegetation, an instant
            // wind-direction snap, impostor popping, wave-direction snapping
            // or grass shimmer — every one of those is a *motion* artifact,
            // and motion artifacts are the dominant failure mode of every
            // system in the backlog.
            //
            // Both the camera and the simulation move between frames. Moving
            // only the camera misses anything that animates in place; moving
            // only the world misses anything that depends on view angle, which
            // is most aliasing.
            Some(count) => {
                let mut runner = play::Runner::new(&world, base)?;
                let mut elapsed = 0_u64;
                // **Stale frames removed before the first write.** A capture
                // that renders 9 frames into a directory holding 20 leaves 11
                // from the previous run, and a contact sheet built from the
                // directory then mixes two experiments — which reads as a
                // discontinuity that is not there.
                // Created rather than assumed. A capture into a fresh
                // `target/agent/frames` is the normal case, and failing on it
                // with "No such file or directory" is a poor first experience
                // of a tool whose whole job is to be run casually.
                if let Some(dir) = std::path::Path::new(&out).parent()
                    && !dir.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                let stem = std::path::Path::new(&out)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("frame")
                    .to_owned();
                if let Some(dir) = std::path::Path::new(&out).parent()
                    && let Ok(entries) = std::fs::read_dir(dir)
                {
                    {
                        for entry in entries.flatten() {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            if name.starts_with(&format!("{stem}_")) && name.ends_with(".png") {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                }
                let probes = telemetry::probes(&scene, &world);
                let mut rows = telemetry::Telemetry::default();

                for index in 0..count {
                    #[allow(clippy::disallowed_methods)]
                    let frame_started = std::time::Instant::now();
                    if index > 0 {
                        for _ in 0..step {
                            elapsed += 1;
                            if let Err(e) = runner.tick(&mut world, elapsed) {
                                return Err(format!("{}: {}", e.script, e.message));
                            }
                        }
                    }
                    let mut objects = world_to_objects(&world, &library, &material_library);
                    objects.extend(scatter_objects(&scene, &library));
                    #[allow(clippy::cast_possible_truncation)]
                    let particles = particles::simulate(
                        &world,
                        &weather,
                        Some(elapsed as u32),
                        &runner.fired(),
                        &runner.splashed(),
                    );

                    // Orbit from wherever the still would have looked, so a
                    // fly-through and a still of the same scene are the same
                    // shot at frame zero.
                    //
                    // **An authored camera is honoured here, and it was not,
                    // which invalidated the entire anti-aliasing investigation
                    // this tool exists to run.** This path used to frame whole
                    // scene bounds unconditionally. For `meadow` that is about
                    // 38 m back — past the 55 m density falloff's reach, at a
                    // resolution where a blade is a third of a pixel — so every
                    // fly-through and shimmer frame of the project's flagship
                    // grass scene was *a bare green slab with no grass in it*.
                    // The flicker metric then dutifully reported that removing
                    // grass reduced flicker, and the AA table was tuned against
                    // it. A metric that frames its own shot will eventually
                    // stop containing the subject, and then it rewards whatever
                    // deletes the subject fastest.
                    //
                    // So: an authored camera **pans**. The eye stays exactly
                    // where the scene put it and the view direction rotates
                    // about it.
                    //
                    // Rotating the *eye* about the target instead was the first
                    // attempt and it is wrong for the same underlying reason the
                    // old code was: at an authored first-person camera the
                    // target is metres away, so orbiting the eye around it
                    // translates the camera a third of a metre per frame, and
                    // translation through a dense near field is the largest
                    // parallax signal available. It measured 5.589 with 52% of
                    // pixels changing — swamped. A pure rotation about a fixed
                    // eye produces no parallax at all: a stable image simply
                    // reprojects, which `|b - (a+c)/2|` cancels, and only pixels
                    // that genuinely twinkle survive. That is also what a hand
                    // on a mouse actually does.
                    //
                    // Scenes with no camera keep the whole-scene framing, which
                    // is right for them.
                    // **The wind clock has to advance, and it did not.** The
                    // environment was built once from `--sim` and never touched
                    // again, so every frame of a fly-through sampled the wind at
                    // the same instant: the blades were frozen while the camera
                    // moved around them. `cargo xtask flythrough` exists
                    // specifically to catch motion artifacts in vegetation, and
                    // it had never once shown the vegetation moving. With a
                    // static camera the whole sixteen-frame sequence was
                    // byte-identical.
                    #[allow(clippy::cast_precision_loss)]
                    let moment = wind_seconds + elapsed as f32 / 60.0;
                    renderer.environment = environment_with_wind(&world, &weather, moment);
                    stamp_flipbook(&mut renderer.environment, &material_library);

                    #[allow(clippy::cast_precision_loss)]
                    let turn = spin * index as f32;
                    let camera = match world.active_camera().filter(|_| !orbiting) {
                        Some(view) => {
                            let start = Vec3::from_array(view.eye);
                            let ahead = Vec3::from_array(view.target) - start;
                            let (sin, cos) = turn.to_radians().sin_cos();
                            let facing = Vec3::new(
                                ahead.x.mul_add(cos, ahead.z * sin),
                                ahead.y,
                                ahead.z.mul_add(cos, -(ahead.x * sin)),
                            );
                            // The walk, along the heading and level with the
                            // ground. Zero by default, so every existing
                            // fly-through is the pure pan it was.
                            let level = Vec3::new(facing.x, 0.0, facing.z);
                            let step = if level.length_squared() > 1e-6 {
                                level.normalize() * (dolly * index as f32)
                            } else {
                                Vec3::ZERO
                            };
                            let eye = start + step;
                            Camera {
                                eye,
                                target: eye + facing,
                                fov_y_degrees: view.fov_y_degrees,
                            }
                        }
                        None => frame_scene(
                            &node_bounds(&world, &library),
                            yaw.unwrap_or(35.0) + turn,
                            pitch.unwrap_or(28.0),
                        ),
                    };

                    // After the camera, because it is a fact about the camera:
                    // a fly-through that starts under a wave crest and ends
                    // above it must change its answer between frames.
                    submerge_eye(
                        &mut renderer.environment,
                        &world,
                        &weather,
                        terrain.as_ref(),
                        camera.eye,
                        moment,
                    );
                    // For the same reason: the lean advances with the clock,
                    // and a shower with a `duration` has to stop partway
                    // through the sweep. **What must NOT change across the
                    // frames is whether there is rain at all** — a flythrough
                    // that walks under the overhang keeps raining outside it,
                    // and the frame where that stopped was the bug.
                    let drops = rain_at_eye(
                        &mut renderer.environment,
                        rain.as_ref(),
                        &weather,
                        camera.eye,
                        moment,
                    );
                    renderer.set_rain(drops);
                    // **And the drop simulation's clock, which is what makes a
                    // fly-through show rain that is falling rather than rain
                    // that has been re-hashed.** It advances by the difference
                    // since the last frame, so the sixteen frames of a
                    // fly-through are sixteen consecutive states of one
                    // simulation.
                    // `--sim` first, then the frames: the same clock
                    // `moment` above is built from, so the rain and the wind
                    // cannot be looking at different instants.
                    renderer.set_rain_tick(u64::from(sim_ticks) + elapsed);

                    renderer
                        .render_to_png(
                            &objects,
                            &particles,
                            &camera,
                            std::path::Path::new(&frame_path(&out, index)),
                        )
                        .map_err(|e| e.to_string())?;

                    // **After the render, so `frame_time_ms` is the whole cost
                    // of producing the frame** rather than of preparing it.
                    #[allow(clippy::disallowed_methods)]
                    let frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
                    let scattered = scatter_objects(&scene, &library);
                    // The same three pieces `rain_at_eye` was handed, at this
                    // frame's instant — so the CSV and the picture cannot be
                    // reading different weather.
                    let frame_weather = weather::Weather {
                        wind: weather::wind_of(&scene),
                        rain,
                        sky: scene_volume(&scene),
                        seconds: moment,
                        deck: weather::deck_of(&world),
                    };
                    rows.push(
                        &probes,
                        &telemetry::Frame {
                            index,
                            sim_time: moment,
                            weather: &frame_weather,
                            eye: camera.eye.to_array(),
                            draws: objects.len(),
                            scattered: &scattered,
                            blades: blades.len(),
                            drops,
                            frame_ms,
                        },
                    );
                }

                // Beside the frames, not inside them: `target/agent/frames/`
                // holds the sequence and `target/agent/telemetry.csv` sits next
                // to it, so clearing the frames does not take the numbers with
                // them.
                if let Some(dir) = std::path::Path::new(&out).parent() {
                    let csv = dir.parent().unwrap_or(dir).join("telemetry.csv");
                    rows.write(&csv)?;
                }
            }
        }
        // Zero validation messages is half the definition of green (brief §7.3).
        instance
            .check_validation()
            .map_err(|m| format!("validation was not silent: {}", m.join("; ")))?;
        Ok((name, raytracing))
    })();

    match result {
        Ok((gpu, raytracing)) => (
            0,
            json_line(&serde_json::json!({
                "ok": true, "out": out, "frames": frames.unwrap_or(1),
                "objects": objects.len(), "particles": particles.len(),
                "size": [width, height], "gpu": gpu,
                // Reported so the agent can tell a scene rendered without
                // shadows from a scene that has none.
                "raytracing": raytracing,
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
pub(crate) fn world_to_objects(
    world: &World,
    library: &MeshLibrary,
    materials: &materials::MaterialLibrary,
) -> Vec<Object> {
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
                material: materials.index_for(index),
                // Scenery does not sway. Only a scatter field asks for it.
                sway: 0.0,
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
            // **And what a scatter field places.** A `Scatter` names its mesh
            // directly rather than through a `MeshRenderer`, so without this
            // the library never resolves it and `index_for` falls back to the
            // box — which is a forest of grey cubes that renders, validates and
            // is wrong. That is exactly what the first render of `forest.loom`
            // showed.
            if let Some(mesh) = node
                .components
                .get("Scatter")
                .and_then(|s| s.get("mesh"))
                .and_then(serde_json::Value::as_str)
            {
                wanted.insert(mesh.to_owned());
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
                Some(mesh) => Ok(Some(mesh.clone())),
                None => bake_voxel(volume).inspect(|baked| {
                    if let Some(mesh) = baked.clone() {
                        // `ponytail:` unbounded until it isn't. Each entry is
                        // one volume's geometry and an edit session touches a
                        // handful; drop the lot rather than track ages.
                        if cache.len() >= 8 {
                            cache.clear();
                        }
                        cache.insert(recipe, mesh);
                    }
                }),
            };
            match baked {
                Ok(Some(mesh)) => {
                    by_name.insert(key, u32::try_from(meshes.len()).unwrap_or(0));
                    meshes.push(mesh);
                }
                Ok(None) => crate::log::warn(format!("{}: voxel produced no surface", node.path)),
                // Loud and whole rather than quiet and short: the node draws
                // nothing, which is visible, instead of a plausible surface
                // missing whichever ops failed to parse.
                Err(e) => crate::log::warn(format!("{}: {e}", node.path)),
            }
        }

        for name in wanted {
            if by_name.contains_key(&name) {
                continue;
            }
            let mesh = loom_asset::primitives::build(&name).or_else(|| {
                let raw = scene_asset_path(scene, &name)?;
                // **`file.obj#Object` selects one object out of a library.** A
                // multi-object OBJ is a pack rather than a model — the tree
                // file is eight species each with its own texture atlas — and
                // one mesh can carry one material, so without this seven of
                // the eight are wrong however they are shaded. Split before
                // joining, or the `#` becomes part of a filename that does not
                // exist.
                let (raw, object) = match raw.split_once('#') {
                    Some((file, obj)) => (file.to_owned(), Some(obj.to_owned())),
                    None => (raw, None),
                };
                let path = base.join(&raw);
                // Chosen by extension, because that is the one thing a path
                // reliably says about its contents. OBJ is text and glTF is a
                // binary container; sniffing would work and would be a second
                // way to be wrong about the same file.
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let imported = if ext == "obj" {
                    loom_asset::mesh::import_obj_object(&path, object.as_deref())
                } else {
                    loom_asset::mesh::import_gltf(&path)
                };
                match imported {
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

/// A node's world translation, by summing the parent chain.
///
/// `ponytail:` translation only — a rotated or scaled parent is ignored. Scene
/// nodes carry a *local* transform on purpose (`loom_scene` has no matrix
/// math), and nothing that carries terrain or a grass field is authored
/// rotated. Compose the full matrices here when one is.
fn world_translation(scene: &Scene, node: &loom_scene::Node) -> [f32; 3] {
    let mut sum = node.transform.pos;
    let mut parent = node.parent.as_deref();
    while let Some(path) = parent {
        let Some(above) = scene.nodes().iter().find(|n| n.path == path) else {
            break;
        };
        for (axis, value) in sum.iter_mut().enumerate() {
            *value += above.transform.pos[axis];
        }
        parent = above.parent.as_deref();
    }
    sum
}

/// The scene's terrain volume and where it sits, or `None` if it has none.
///
/// **The first one wins.** A scene with two voxel volumes under one grass field
/// is not a thing anything authors yet; when it is, this becomes a lookup per
/// sample and gets slower.
pub(crate) fn scene_volume(scene: &Scene) -> Option<(loom_voxel::Volume, [f32; 3])> {
    scene.nodes().iter().find_map(|node| {
        let volume = build_volume(node.components.get("VoxelVolume")?)
            .map_err(|e| crate::log::warn(format!("{}: {e}", node.path)))
            .ok()?;
        Some((volume, world_translation(scene, node)))
    })
}

/// The world raindrops collide with, or `None` for a scene with no rain and
/// nothing to hit.
///
/// **The bridge for ADR 0015, and the same shape as `scene_terrain_field`.**
/// The renderer never learns what a voxel or a collider is; it is handed a
/// block of `i8` and a frame. What crosses over is the *collision world* —
/// every voxel volume unioned with every static box collider — because ADR
/// 0014's trigger 2 is geometry that is not in the voxel volume, and a bake of
/// the voxels alone would carry drops onto a better representation of exactly
/// the same world.
///
/// The rule is one sentence: **rain stops where a body would stop.** It is the
/// same rule audio's `openness` follows when it casts against the collision
/// world rather than the voxels, and it means a mesh roof shelters rain exactly
/// when it also blocks a bullet.
pub(crate) fn rain_collision_field(
    scene: &Scene,
    world: &World,
) -> Option<loom_rain::collide::Field> {
    weather::rain_of(scene)?;
    let volume = scene_volume(scene);
    let volumes: Vec<(&loom_voxel::Volume, [f32; 3])> = volume
        .as_ref()
        .map(|(v, offset)| (v, *offset))
        .into_iter()
        .collect();

    let boxes: Vec<loom_rain::collide::Solid> = world
        .entities()
        .iter()
        .filter_map(|entity| {
            let half = world.collider_half_extents(*entity)?;
            let global = world.global_transform(*entity)?;
            let matrix = Mat4::from_cols_array(&global.matrix);
            let (scale, _, translation) = matrix.to_scale_rotation_translation();
            // **Rotation is dropped, and the box is grown to cover it.**
            // `loom_rain::collide` rasterises axis-aligned boxes; a rotated one
            // would need its own shape. A rotated collider is rare and the
            // conservative cover errs toward *more* shelter, which is the safe
            // direction — a drop stopped a little early is invisible, a drop
            // falling through a roof is the bug this exists to fix.
            //
            // `ponytail:` exact when the collider is axis-aligned, which every
            // one in the repository is. Add an oriented box to
            // `loom_rain::collide::Shape` if a scene ever needs one.
            Some(loom_rain::collide::Solid {
                center: translation.to_array(),
                half_extents: [
                    half[0] * scale.x.abs(),
                    half[1] * scale.y.abs(),
                    half[2] * scale.z.abs(),
                ],
            })
        })
        .collect();

    loom_rain::collide::bake(&volumes, &boxes)
}

/// The ground under a whole voxel volume, for water to take its depth from.
///
/// **The window is the volume's own footprint**, which is the honest answer to
/// "where might there be terrain": outside it there is none, and the height
/// field says so with its sentinel rather than by extrapolating the edge. The
/// water mesh reaches 512 m and the grid does not; a sea 200 m from a small
/// island is bottomless, which is correct.
///
/// **Rotation and scale on the volume's node are ignored**, exactly as the
/// grass path ignores them — only the translation is applied. A rotated terrain
/// volume would need the whole query in the volume's local frame, and nothing
/// authors one.
pub(crate) fn terrain_field(
    volume: &loom_voxel::Volume,
    offset: [f32; 3],
) -> loom_voxel::heightfield::HeightField {
    let [rx, _, rz] = volume.resolution();
    #[allow(clippy::cast_precision_loss)]
    let (wx, wz) = (rx as f32 * volume.voxel_size, rz as f32 * volume.voxel_size);
    let centre = [offset[0] + wx * 0.5, offset[2] + wz * 0.5];
    loom_voxel::heightfield::HeightField::bake(volume, offset, centre, wx.max(wz) * 0.5)
}

/// The river's current over a baked bed, or `None` when the water is not one.
///
/// **The bridge, and the only place the two systems meet.** `loom_water` must
/// not know what a voxel is — the Slang half of the surface is generated from
/// it — and `loom_voxel` has no business knowing what a `WaterBody` is. So the
/// height grid crosses over as a plain array of metres, exactly as the wind
/// parameters cross into `loom_field` in `weather.rs`, and for the same reason.
///
/// **The sentinel becomes a `NaN` on the way through.** A column with no ground
/// is `heightfield::NO_GROUND` — a large finite negative, so the *shader's*
/// bilinear can stay branchless — and routing rain into it would make one
/// imaginary cell a billion metres down the outlet for the entire map. The
/// grass path already converts it the same way, for the same reason.
pub(crate) fn river_flow(
    field: &loom_voxel::heightfield::HeightField,
    water: &loom_scene::components::WaterBody,
) -> Option<loom_water::flow::FlowGrid> {
    let flow = water.flow?;
    let heights: Vec<f32> = field
        .height
        .iter()
        .map(|h| {
            if loom_voxel::heightfield::HeightField::has_ground(*h) {
                *h
            } else {
                f32::NAN
            }
        })
        .collect();
    Some(loom_water::flow::FlowGrid::bake(
        field.origin,
        field.spacing,
        field.side,
        &heights,
        flow.speed,
    ))
}

/// The scene's terrain height grid, or `None` when nothing would read it.
///
/// **Only baked for a scene with water or rain**, and that early-out is not
/// politeness: the bake is a march down the SDF per sample, and
/// `terrain_stress.loom` is 67 million voxels. Grass bakes its own window
/// through the same code, which is one march each rather than one shared —
/// the two want different extents, and sharing them would mean baking the
/// coarser of the two for both.
///
/// **Rain is the second reader, and it uses the grid differently.** Water asks
/// how deep it is over the bed; rain asks what the first thing above a falling
/// drop is, which for a top-down height field is the same query and the right
/// answer — the topmost surface in a column is exactly what a drop hits. It is
/// the same grid either way, so a scene that has both bakes once.
pub(crate) fn scene_terrain_field(
    scene: &Scene,
) -> Option<loom_voxel::heightfield::HeightField> {
    let wanted = scene
        .nodes()
        .iter()
        .any(|n| n.components.contains_key("WaterBody"))
        || weather::rain_of(scene).is_some();
    if !wanted {
        return None;
    }
    let (volume, offset) = scene_volume(scene)?;
    Some(terrain_field(&volume, offset))
}

/// Radius, in metres, of the stencil that stands in for flow accumulation.
///
/// A metre, because a gully is a metre-scale feature and a voxel-scale stencil
/// measures quantization noise instead. This is a calibration knob: widen it
/// and only broad valleys read as lush, narrow it and every dimple does.
const FLOW_RADIUS: f32 = 1.0;

/// Rise, in metres over [`FLOW_RADIUS`], that reads as a fully lush gully.
const FLOW_FULL: f32 = 0.08;

/// Where the ground is under a grass field, sampled from the voxel SDF.
///
/// **A grid, not a query per blade, and the ratio was measured.** Height needs
/// a march down the SDF and the flow proxy needs four more around each point:
/// on `grass_slope.loom` that is 352k marches and **2.98 s**. The same field
/// sampled once onto a 117² grid is 13.7k marches and **0.11 s**, with every
/// blade a bilinear lookup and the neighbourhood terms array reads. Twenty-seven
/// times, for an answer that is smoother rather than worse — a blade cannot
/// resolve terrain finer than the voxel the grid is sampled at anyway.
///
/// **The grid itself is [`loom_voxel::heightfield::HeightField`]**, shared with
/// water — which needs exactly the same "where is the ground at (x, z)" and
/// must get exactly the same answer, because the shoreline it draws and the
/// depth the buoyancy solver reads are the same number. What lives here is only
/// what grass adds on top: the slope, the flow proxy, and the node-relative
/// height [`loom_grass::Ground`] is written in terms of.
struct GroundGrid {
    field: loom_voxel::heightfield::HeightField,
    /// World Y of the grass node, subtracted from every height handed out.
    base_y: f32,
    /// Flow stencil radius, in samples.
    stencil: usize,
}

impl GroundGrid {
    /// Sample the ground under a field of half-extent `half` centred on `base`.
    fn bake(
        volume: &loom_voxel::Volume,
        offset: [f32; 3],
        base: [f32; 3],
        half: [f32; 2],
    ) -> Self {
        // Margin so the flow stencil and the slope differences never read off
        // the edge of the grid, which would make the field's border a cliff.
        let reach = half[0].max(half[1]) + FLOW_RADIUS + volume.voxel_size * 2.0;
        let field = loom_voxel::heightfield::HeightField::bake(
            volume,
            offset,
            [base[0], base[2]],
            reach,
        );
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let stencil = ((FLOW_RADIUS / field.spacing).round() as usize).max(1);
        Self { field, base_y: base[1], stencil }
    }

    /// Height at a sample relative to the grass node, or `NaN` where there is
    /// no ground.
    ///
    /// **`NaN` here rather than the height field's sentinel**, because every
    /// caller below is a Rust arithmetic path that already treats "not finite"
    /// as "no ground", and a −10⁹ blended into a slope would read as a cliff
    /// instead. The sentinel exists for the shader's sake, which has no branch
    /// to spare.
    fn node(&self, i: usize, j: usize) -> f32 {
        let h = self.field.node(i, j);
        if loom_voxel::heightfield::HeightField::has_ground(h) {
            h - self.base_y
        } else {
            f32::NAN
        }
    }

    /// What the ground is doing at a world position.
    ///
    /// **`rock = 1` is also how "there is no ground here" is said.** It is the
    /// one term in [`loom_grass::coverage`] that zeroes the answer whatever the
    /// normal says, so a column with no surface — outside the volume, or a hole
    /// blown clean through it — grows nothing. That is the no-floating-blades
    /// half of P2's exit criteria, and it costs no new channel.
    fn at(&self, x: f32, z: f32) -> loom_grass::Ground {
        let bare = loom_grass::Ground { rock: 1.0, ..loom_grass::Ground::default() };
        let spacing = self.field.spacing;
        let (gx, gz) = (
            (x - self.field.origin[0]) / spacing,
            (z - self.field.origin[1]) / spacing,
        );
        // The stencil reaches `stencil` samples out, so anything nearer the
        // edge than that has no neighbourhood to measure.
        #[allow(clippy::cast_precision_loss)]
        let limit = (self.field.side - self.stencil - 1) as f32;
        #[allow(clippy::cast_precision_loss)]
        let low = self.stencil as f32;
        if !(gx >= low && gz >= low && gx < limit && gz < limit) {
            return bare;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (i, j) = (gx as usize, gz as usize);
        let (fx, fz) = (gx - gx.floor(), gz - gz.floor());

        // Bilinear height. One absent corner makes the whole cell absent — an
        // interpolation against NaN is NaN, and a blade at NaN is a triangle at
        // infinity.
        let lerp = |a: f32, b: f32, t: f32| (b - a).mul_add(t, a);
        let h = lerp(
            lerp(self.node(i, j), self.node(i + 1, j), fx),
            lerp(self.node(i, j + 1), self.node(i + 1, j + 1), fx),
            fz,
        );
        if !h.is_finite() {
            return bare;
        }

        // Slope from central differences of the height field rather than the
        // SDF gradient: the field is quantized to 127 steps across a voxel, so
        // its gradient is noisy at exactly the scale a blade cares about, and
        // the surface is what grass grows on anyway.
        let (dx, dz) = (
            (self.node(i + 1, j) - self.node(i - 1, j)) / (2.0 * spacing),
            (self.node(i, j + 1) - self.node(i, j - 1)) / (2.0 * spacing),
        );
        let length = dx.mul_add(dx, dz.mul_add(dz, 1.0)).sqrt();
        let normal = if length.is_finite() && length > 0.0 {
            [-dx / length, 1.0 / length, -dz / length]
        } else {
            [0.0, 1.0, 0.0]
        };

        // **A concavity proxy, not flow accumulation.** Real flow means routing
        // water downhill across the whole field and counting what drains
        // through each cell (D8 or D-infinity over the heightfield); there is
        // no such data for a voxel volume, and `loom_terrain` is a separate
        // heightmap system these scenes do not use. Concave ground is where
        // water would collect, which is the part grass responds to, so: the
        // centre against the mean of four neighbours a metre out. Positive is a
        // hollow, negative — a ridge — reads as zero. Replace this with real
        // accumulation when erosion lands and there is a drainage field worth
        // reading.
        let s = self.stencil;
        let ring = [
            self.node(i + s, j),
            self.node(i - s, j),
            self.node(i, j + s),
            self.node(i, j - s),
        ];
        let flow = if ring.iter().all(|v| v.is_finite()) {
            ((ring.iter().sum::<f32>() / 4.0 - h) / FLOW_FULL).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // `rock` stays 0 for real ground. Nothing in the scene schema says
        // which parts of a volume are stone rather than soil — deriving it from
        // slope would only double-count the slope term coverage already has.
        // A per-op material on `VoxelVolume`, or the erosion field Phase 8
        // wants, is what would drive it.
        loom_grass::Ground { height: h, normal, rock: 0.0, flow }
    }
}

/// Everything [`grass_blades`] reads out of the scene, as a comparable key.
///
/// Placing grass marches the voxel SDF — seconds on `grass_slope` — and the
/// editor re-derives its view on **every frame of a gizmo drag**. This is the
/// same guard `SceneView::mesh_key` is for geometry: regenerate only when a
/// grass field, the terrain under it, or where either sits actually moved.
/// Everything [`scene_terrain_field`] reads out of the scene, as a comparable
/// key.
///
/// **This is the answer to §5.2 in the viewer.** Carve the lake bed with a
/// transaction and the ops under `VoxelVolume` change, so this string changes,
/// so the height field is rebaked and re-uploaded and the shoreline moves. It
/// costs the bake — a march per grid sample, tenths of a second on the scenes
/// here — paid on the edit rather than on the frame, which is the same bargain
/// grass makes. What it does *not* do is rebake inside a running simulation:
/// `Sim` bakes once at load, and a crater blown mid-run is seen by the water
/// only after a reload.
pub(crate) fn terrain_key(scene: &Scene) -> String {
    // The same two readers `scene_terrain_field` bakes for. Rain is here for
    // the sharper version of the same criterion: carve the overhang open with
    // a transaction and the streaks come through on the next re-derive,
    // because the ops changed, so this key changed, so the grid was rebaked.
    let wanted = scene
        .nodes()
        .iter()
        .any(|n| n.components.contains_key("WaterBody"))
        || weather::rain_of(scene).is_some();
    if !wanted {
        return String::new();
    }
    let mut parts = Vec::new();
    for node in scene.nodes() {
        if let Some(component) = node.components.get("VoxelVolume") {
            let at = world_translation(scene, node);
            parts.push(format!("{}|{at:?}|{component}", node.path));
        }
    }
    parts.join("\n")
}

pub(crate) fn grass_key(scene: &Scene) -> String {
    // Same early-out as `grass_blades`, and for the same reason: a scene with
    // no grass must not pay to describe a 67-million-voxel volume it will
    // never grow anything on. The empty string then means "no blades".
    if !scene.nodes().iter().any(|n| n.components.contains_key("Grass")) {
        return String::new();
    }
    let mut parts = Vec::new();
    for node in scene.nodes() {
        for name in ["Grass", "VoxelVolume"] {
            if let Some(component) = node.components.get(name) {
                let at = world_translation(scene, node);
                parts.push(format!("{}|{name}|{at:?}|{component}", node.path));
            }
        }
        // The field's colour comes from the `Material` beside its `Grass`, so
        // editing that albedo has to regenerate — only for a grass node, or
        // every material edit in the scene would pay the voxel march.
        if node.components.contains_key("Grass")
            && let Some(material) = node.components.get("Material")
        {
            parts.push(format!("{}|Material|{material}", node.path));
        }
    }
    parts.join("\n")
}

/// Bake a `VoxelVolume` component into a mesh.
/// Every blade in every grass field in the scene, ready to upload.
///
/// **Placement stays on the CPU; expansion moved to the GPU.** A blade is a
/// pure function of its coordinates, so this is not per-frame work — it is
/// uploaded once. What happens every frame is the Bezier expansion and the
/// wind bend, in the vertex shader, which is what a baked mesh could not do.
/// Say so when the blade buffer could not hold the field.
///
/// **The failure this prevents is the most visible one a scene author can hit,
/// and it used to be silent.** Past capacity the renderer drops the tail, and
/// tiles are generated in z-major order, so what gets dropped is a contiguous
/// spatial slab rather than a thin scatter — with a camera in the field that is
/// the *near* half, and the render comes back with a straight horizontal edge
/// across the middle of the image, `"ok": true`, and exit code 0.
///
/// `Grass::half_extent` is schema-legal to 500 m and `density` to 2000 blades
/// per square metre, so a field well inside what the schema invites overruns a
/// quarter-million blades easily: 60x60 m at the default density does it.
///
/// This does not raise the ceiling — growing a quarter-million-element buffer
/// mid-frame is worse than a limit somebody can see — it just stops the ceiling
/// being invisible.
fn warn_if_grass_truncated(wanted: usize, capacity: usize) {
    if wanted > capacity && capacity > 0 {
        eprintln!(
            "warning: grass field needs {wanted} blades and the buffer holds {capacity}; \
             {} were dropped. They are dropped in generation order, not by distance, so \
             expect a hard edge across the field rather than a thinner one. Reduce \
             `density` or `half_extent`.",
            wanted - capacity
        );
    }
}

/// What a grass field with no `Material` beside it is coloured.
///
/// The hue `scene.slang` used to hardcode, so a field that authors nothing
/// renders exactly as it did.
const GRASS_ALBEDO: [f32; 3] = [0.24, 0.40, 0.13];

/// One clump's albedo: the field's authored colour, pushed along the single
/// axis grass actually varies on.
///
/// **Dry is yellow-green and lush is blue-green** — more red and less blue, or
/// the reverse — so the whole of hue variation here is two multiplies against
/// `hue` from [`loom_grass`]. A free hue rotation would need an HSV round trip
/// and could reach colours no field contains; this cannot leave the range
/// between straw and blue-green whatever it is handed.
fn hue_shift(albedo: [f32; 3], hue: f32) -> [f32; 3] {
    // Deliberately small. Doubling these reads as patches of different plants
    // rather than as one field, which is the failure mode of the whole idea.
    const RED: f32 = 0.45;
    const BLUE: f32 = 0.60;
    [albedo[0] * (1.0 - hue * RED), albedo[1], albedo[2] * (1.0 + hue * BLUE)]
}

/// Three colour channels into the one `float4` slot `GrassBlade` has left.
///
/// Eight bits each: 1/255 is finer than the variation being carried, and the
/// packed integer stays under 2^24 where an `f32` is exact, so the shader
/// unpacks precisely what was packed. **It unpacks in the *vertex* stage** —
/// an interpolated 16-million-magnitude float is not guaranteed bit-identical
/// across a triangle, and one ULP there is a blue channel that wrapped.
fn pack_rgb(colour: [f32; 3]) -> f32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round();
    q(colour[0]).mul_add(65536.0, q(colour[1]).mul_add(256.0, q(colour[2])))
}

/// Every scattered instance in a scene, as objects the renderer draws.
///
/// **The wiring, and it is deliberately thin.** `loom_scatter` decides where
/// things go and knows nothing about scenes, voxels or meshes; this reads the
/// components, hands it a closure over the terrain, and turns what comes back
/// into transforms. Every interesting decision is in the library, next to its
/// tests.
///
/// Instances are appended to the object list and never become entities — the
/// phase's hierarchy rule, and the same one grass and voxels follow. The tree
/// shows one node for the field.
pub(crate) fn scatter_objects(scene: &Scene, library: &MeshLibrary) -> Vec<Object> {
    let mut out = Vec::new();
    // Baking the volume costs seconds on a large scene, so one with no scatter
    // in it must not pay for one. The same guard `grass_blades` opens with.
    if !scene.nodes().iter().any(|n| n.components.contains_key("Scatter")) {
        return out;
    }
    let terrain = scene_volume(scene);

    // Fields in declaration order, because exclusion references point
    // backwards — see `loom_scatter::scatter`.
    let mut fields: Vec<(String, loom_scene::components::Scatter, [f32; 3])> = Vec::new();
    for node in scene.nodes() {
        let Some(component) = node.components.get("Scatter") else {
            continue;
        };
        let Ok(field) =
            serde_json::from_value::<loom_scene::components::Scatter>(component.clone())
        else {
            continue;
        };
        fields.push((node.path.clone(), field, world_translation(scene, node)));
    }

    // One `loom_scatter::Layer` per field. The whole set is resolved together
    // so exclusions can see each other, and each layer's own bounds are its
    // node's — a field only places inside its own half-extent.
    let excludes: Vec<Vec<loom_scatter::Exclude>> = fields
        .iter()
        .map(|(_, f, _)| {
            f.exclude
                .iter()
                .map(|e| loom_scatter::Exclude { layer: e.field.as_str(), radius: e.radius })
                .collect()
        })
        .collect();
    let layers: Vec<loom_scatter::Layer> = fields
        .iter()
        .zip(&excludes)
        .map(|((path, f, _), ex)| loom_scatter::Layer {
            name: path.as_str(),
            rules: loom_scatter::Rules {
                spacing: f.spacing,
                jitter: f.jitter,
                scale: f.scale,
                seed: f.seed,
                max_slope_degrees: f.max_slope_degrees,
                slope_fade: 6.0,
                altitude: [f32::NEG_INFINITY, f32::INFINITY],
                moisture: f.moisture,
                density: f.density,
            },
            exclude: ex.as_slice(),
            biome: None,
        })
        .collect();

    // The region every field together covers, so one resolve serves them all.
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for (_, f, origin) in &fields {
        min[0] = min[0].min(origin[0] - f.half_extent[0]);
        min[1] = min[1].min(origin[2] - f.half_extent[1]);
        max[0] = max[0].max(origin[0] + f.half_extent[0]);
        max[1] = max[1].max(origin[2] + f.half_extent[1]);
    }

    // **The seam**, exactly as grass has it: `loom_scatter` asks a closure what
    // the ground is doing and this is where the scene's terrain answers. A
    // scene with no voxel volume keeps flat ground.
    let half = [(max[0] - min[0]) * 0.5, (max[1] - min[1]) * 0.5];
    let centre = [(min[0] + max[0]) * 0.5, 0.0, (min[1] + max[1]) * 0.5];
    let grid = terrain
        .as_ref()
        .map(|(volume, offset)| GroundGrid::bake(volume, *offset, centre, half));
    let ground = |x: f32, z: f32| {
        grid.as_ref().map_or_else(loom_scatter::Ground::default, |g| {
            let g = g.at(x, z);
            loom_scatter::Ground {
                height: g.height,
                normal: g.normal,
                rock: g.rock,
                flow: g.flow,
            }
        })
    };

    let resolved = match loom_scatter::scatter(min, max, &layers, &ground) {
        Ok(resolved) => resolved,
        Err(e) => {
            // **Said out loud, not swallowed.** `loom validate` checks schemas
            // and asset aliases; it does not yet resolve `Scatter.exclude`
            // references, so a typo reaches here. An empty forest with no
            // explanation is the failure this project keeps writing tests
            // against — a rule that placed nothing looks exactly like a rule
            // that matched nothing.
            crate::log::warn(format!("scatter: {e}"));
            return out;
        }
    };

    for ((path, field, origin), (_, instances)) in fields.iter().zip(resolved) {
        let mesh = library.index_for(Some(field.mesh.as_str()));
        let albedo = scene
            .nodes()
            .iter()
            .find(|n| &n.path == path)
            .and_then(|n| n.components.get("Material"))
            .and_then(|m| {
                serde_json::from_value::<loom_scene::components::Material>(m.clone()).ok()
            })
            .map_or([0.29, 0.34, 0.20], |m| m.albedo);

        for instance in instances {
            // A field only places inside its own half-extent, even though the
            // resolve covered every field's union.
            let (dx, dz) = (instance.at[0] - origin[0], instance.at[1] - origin[2]);
            if dx.abs() > field.half_extent[0] || dz.abs() > field.half_extent[1] {
                continue;
            }
            let g = ground(instance.at[0], instance.at[1]);
            let model = Mat4::from_translation(Vec3::new(
                instance.at[0],
                origin[1] + g.height,
                instance.at[1],
            )) * Mat4::from_rotation_y(instance.yaw)
                * Mat4::from_scale(Vec3::splat(instance.scale));
            out.push(Object {
                model,
                color: albedo,
                mesh,
                // No material table entry: an instance is not a scene node, so
                // it has no index into the per-node material list. `color`
                // carries the field's authored albedo instead.
                material: u32::MAX,
                sway: field.sway,
            });
        }
    }
    out
}

pub(crate) fn grass_blades(scene: &Scene) -> Vec<loom_render::GrassBlade> {
    let mut out = Vec::new();
    // Baking the volume is not free — `terrain_stress` is 67 million voxels —
    // so a scene with no grass in it must not pay for one.
    if !scene.nodes().iter().any(|n| n.components.contains_key("Grass")) {
        return out;
    }
    let terrain = scene_volume(scene);
    for node in scene.nodes() {
        let Some(component) = node.components.get("Grass") else {
            continue;
        };
        let Ok(field) = serde_json::from_value::<loom_scene::components::Grass>(component.clone())
        else {
            continue;
        };
        let rules = loom_grass::Rules {
            density: field.density,
            height: field.height,
            height_jitter: 0.35,
            width: field.width,
            slope_cutoff: field.slope_cutoff,
            clump_facing: field.clump_facing,
            clump_colour: field.clump_colour,
        };
        // **The authored colour, read from the `Material` beside the `Grass`.**
        // Both shipped grass scenes have always carried one and nothing read
        // it — the shader hardcoded a green — which is precisely the failure an
        // engine whose premise is "the agent can verify its own work" cannot
        // have: the scene said one thing and the render showed another, and
        // every gate passed.
        let albedo = node
            .components
            .get("Material")
            .and_then(|m| serde_json::from_value::<loom_scene::components::Material>(m.clone()).ok())
            .map_or(GRASS_ALBEDO, |m| m.albedo);

        // The field is centred on its node.
        let origin = world_translation(scene, node);
        let (hx, hz) = (field.half_extent[0], field.half_extent[1]);
        // **The seam.** `loom_grass` knows nothing about voxels — it asks a
        // closure what the ground is doing, and this is where the scene's
        // actual terrain answers. A scene with no voxel volume keeps the flat
        // plane, so `meadow` renders exactly as it did.
        let grid = terrain
            .as_ref()
            .map(|(volume, offset)| GroundGrid::bake(volume, *offset, origin, [hx, hz]));
        let ground = |x: f32, z: f32| {
            grid.as_ref().map_or_else(loom_grass::Ground::default, |g| {
                g.at(x + origin[0], z + origin[2])
            })
        };
        let low = loom_grass::Tile::at(-hx, -hz);
        let high = loom_grass::Tile::at(hx, hz);
        for z in low.z..=high.z {
            for x in low.x..=high.x {
                let tile = loom_grass::Tile { x, z };
                for blade in loom_grass::tile(tile, &rules, &ground) {
                    if blade.position[0].abs() > hx || blade.position[2].abs() > hz {
                        continue;
                    }
                    out.push(loom_render::GrassBlade {
                        position: [
                            blade.position[0] + origin[0],
                            blade.position[1] + origin[1],
                            blade.position[2] + origin[2],
                            blade.height,
                        ],
                        facing: [blade.facing[0], blade.facing[1], blade.width, blade.tilt],
                        // The clump hash reaches the shader as a float only to
                        // phase-shift the sway. Its fractional part is what is
                        // used, so precision past 24 bits does not matter.
                        #[allow(clippy::cast_precision_loss)]
                        shape: [
                            blade.bend,
                            blade.shade,
                            (blade.clump % 65536) as f32 / 65536.0,
                            pack_rgb(hue_shift(albedo, blade.hue)),
                        ],
                    });
                }
            }
        }
    }
    out
}

/// A voxel volume's drawable surface, or `Ok(None)` if the recipe carves
/// nothing solid.
///
/// One bake, shared with physics: this used to read `voxel_size`, `chunks` and
/// `ops` a second time, which is two chances to disagree about what a scene
/// means and is how the drawn surface and the collided one drift apart.
fn bake_voxel(component: &serde_json::Value) -> Result<Option<loom_asset::Mesh>, String> {
    let volume = build_volume(component)?;
    let mesh = loom_voxel::mesh::mesh_volume(&volume, &loom_voxel::SurfaceNets);
    Ok((!mesh.indices.is_empty()).then_some(mesh))
}

/// The advisory `path` an `[[asset]]` entry carries, for importing.
fn scene_asset_path(scene: &Scene, key: &str) -> Option<String> {
    scene.asset_path(key).map(str::to_owned)
}

/// `shot.png` and frame 7 becomes `shot_0007.png`.
///
/// Zero-padded so the shell orders them the way they were rendered — a
/// sequence that sorts `frame_10` before `frame_2` is a sequence nobody can
/// flick through, which defeats the point of dumping one.
fn frame_path(out: &str, index: u32) -> String {
    let path = std::path::Path::new(out);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    let numbered = format!("{stem}_{index:04}.{extension}");
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join(numbered).to_string_lossy().into_owned(),
        None => numbered,
    }
}

/// Pixel-compare two renders.
/// How much of the middle frame is temporal noise rather than motion.
///
/// **The measurement `compare` cannot make.** Counting changed pixels
/// conflates flicker with parallax, and the anti-aliasing work needs to tell
/// them apart: widening distant grass so it stops twinkling covers more
/// screen, changes more pixels, and reads as a regression to a pixel-count
/// metric. See `imagediff::flicker`.
fn flicker(a: &str, b: &str, c: &str) -> (u8, String) {
    let images: Result<Vec<imagediff::Image>, String> =
        [a, b, c].iter().map(|p| imagediff::load(std::path::Path::new(p))).collect();
    let images = match images {
        Ok(images) => images,
        Err(e) => {
            return (2, json_line(&serde_json::json!({ "error": "io_error", "constraint": e })));
        }
    };
    match imagediff::flicker(&images[0], &images[1], &images[2]) {
        Err(e) => (
            2,
            json_line(&serde_json::json!({ "error": "size_mismatch", "constraint": e })),
        ),
        Ok(measured) => (
            0,
            json_line(&serde_json::json!({ "ok": true, "flicker": measured, "b": b })),
        ),
    }
}

fn compare(a: &str, b: &str, args: &[String]) -> (u8, String) {
    let mut tolerance = imagediff::Tolerance::default();
    if let Some(v) = flag(args, "--channel").and_then(|v| v.parse::<u8>().ok()) {
        tolerance.channel = v;
    }
    if let Some(v) = flag(args, "--fraction").and_then(|v| v.parse::<f64>().ok()) {
        tolerance.fraction = v;
    }
    if let Some(v) = flag(args, "--worst").and_then(|v| v.parse::<u8>().ok()) {
        tolerance.worst = v;
    }

    let (left, right) = match (
        imagediff::load(std::path::Path::new(a)),
        imagediff::load(std::path::Path::new(b)),
    ) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(e), _) | (_, Err(e)) => {
            return (2, json_line(&serde_json::json!({ "error": "io_error", "constraint": e })));
        }
    };

    match imagediff::compare(&left, &right, tolerance) {
        Err(e) => (
            1,
            json_line(&serde_json::json!({
                "ok": false, "error": "size_mismatch", "constraint": e, "a": a, "b": b,
            })),
        ),
        Ok(diff) => {
            let passed = diff.passes(tolerance);
            (
                u8::from(!passed),
                json_line(&serde_json::json!({
                    "ok": passed,
                    "a": a,
                    "b": b,
                    "pixels": diff.pixels,
                    "differing": diff.differing,
                    "fraction": diff.fraction(),
                    "worst": diff.worst,
                    "mean": diff.mean,
                    "tolerance": {
                        "channel": tolerance.channel,
                        "fraction": tolerance.fraction,
                        "worst": tolerance.worst,
                    },
                })),
            )
        }
    }
}

// `environment_of(world)` used to live here: `environment_with_wind` with
// `Wind::default()` and a clock of zero. It was deleted rather than fixed,
// because its whole shape was a trap. It looked like the convenient overload
// and it silently substituted the wrong weather at a frozen instant, which is
// exactly what it did to the viewer — the authored `Wind` reached the particles
// and never reached the grass, and the blades stood still in the window while
// swaying correctly in every headless render.
//
// There is now one way to build an environment and it takes the wind and the
// time, so neither can be forgotten.

/// The scene's environment, with its weather and the simulation's clock.
///
/// **The wind parameters travel in the environment buffer**, not the push
/// block, which is at 124 of its 128 bytes. The vertex shader reads them to
/// bend a blade, so they have to reach the GPU somehow and this is the buffer
/// for per-scene data.
/// Point the environment at the scene's fire flipbook, if it declares one.
///
/// **By a reserved alias rather than a component field**, and the choice is
/// worth stating. The flipbook is per-scene, not per-emitter: every additive
/// particle in a frame samples the same sheet, so hanging it off
/// `ParticleEmitter` would mean carrying a texture index per particle to the
/// fragment shader for a value that never differs between them. A reserved
/// name is the smaller lie — a scene declares `[[asset]] key = "fire_flipbook"`
/// and the fire uses it.
fn stamp_flipbook(
    env: &mut loom_render::EnvironmentData,
    materials: &materials::MaterialLibrary,
) {
    if let Some(slot) = materials.by_alias.get("fire_flipbook") {
        env.fire_flipbook = *slot;
    }
}

pub(crate) fn environment_with_wind(
    world: &World,
    wind: &loom_field::wind::Wind,
    seconds: f32,
) -> loom_render::EnvironmentData {
    let mut env = environment_of_inner(world);
    gather_lights(world, &mut env);
    let params = wind.params();
    env.wind = [
        params.get("dir_x"),
        params.get("dir_z"),
        params.get("speed"),
        params.get("gustiness"),
    ];
    env.weather = [params.get("turbulence"), params.get("ground_drag"), seconds, 0.0];
    add_water(&mut env, world, wind);
    env
}

/// Put the scene's sea into the environment the shader reads.
///
/// **The waves are derived from the wind unless the file lists its own.** A sea
/// has one honest input — how hard it is blowing — and `loom_water::spectrum`
/// turns that into sixteen waves whose significant height matches the published
/// relation. A scene that authors waves anyway keeps them, because a stylised
/// or scripted sea is a legitimate thing to want; a scene that does not gets a
/// sea that matches the grass bending beside it, for free.
///
/// The wave set is the equilibrium one, not `SeaState`'s slewed version: a
/// still and a fly-through both want the sea the wind has already built, and
/// the inertia only matters once something turns the wind mid-run.
fn add_water(
    env: &mut loom_render::EnvironmentData,
    world: &World,
    wind: &loom_field::wind::Wind,
) {
    let Some(body) = weather::water_of(world, wind) else {
        return;
    };
    let waves = &body.waves.waves;

    // Depth is no longer in here: it is a per-vertex query against the terrain
    // height grid `set_terrain` uploads, which is the same grid the buoyancy
    // solver reads (W6). `y` is the underwater flag, and it is left off here
    // because this function does not know where the camera is —
    // `submerge_eye` stamps it once the shot is framed.
    env.water = [body.surface_height, 0.0, 1.0, 0.0];
    env.attenuation_depth = body.waves.attenuation_depth;
    // Truncated at the cap the shader's loop is bounded by, which is also the
    // schema's `maxItems`, so this only bites on a hand-built body.
    env.wave_count = u32::try_from(waves.len().min(loom_render::MAX_WAVES)).unwrap_or(0);
    for (slot, wave) in env.waves.iter_mut().zip(waves) {
        *slot = loom_render::WaterWave {
            direction: wave.direction,
            wavelength: wave.wavelength,
            amplitude: wave.amplitude,
            steepness: wave.steepness,
            speed_scale: wave.speed_scale,
        };
    }
}

/// Tell the shader whether the camera is under the water.
///
/// **One bool per frame, and it is W7's bool.**
/// [`loom_water::buoyancy::submersion_at`] is the same function the buoyancy
/// solver calls per pontoon and `Physics::submerged_at` calls for the audio
/// listener — same surface, same clock, same bed. A swimmer whose ears go
/// muffled one tick and whose view goes green on another would be the exact
/// divergence this project keeps paying to avoid, so there is no second
/// height test here and none in the shader either: `scene.slang` reads this
/// flag out of the environment buffer and never asks again.
///
/// A radius of zero because an eye has no volume — the answer is "above" or
/// "below" with nothing in between, which is also why there is no hysteresis:
/// a camera bobbing exactly at the waterline flips, and that is what water at
/// your eyeline actually does.
pub(crate) fn submerge_eye(
    env: &mut loom_render::EnvironmentData,
    world: &World,
    wind: &loom_field::wind::Wind,
    terrain: Option<&loom_voxel::heightfield::HeightField>,
    eye: Vec3,
    seconds: f32,
) {
    let Some(body) = weather::water_of(world, wind) else {
        return;
    };
    // The same bed the water shader reads its depth from, because the shoaling
    // taper flattens the waves over it — off the grid it is bottomless, which
    // is what an open ocean wants.
    let ground = terrain.map_or(loom_voxel::heightfield::NO_GROUND, |g| g.at(eye.x, eye.z));
    let under = loom_water::buoyancy::submersion_at(&body, eye.to_array(), 0.0, seconds, ground);
    env.water[1] = f32::from(under > 0.5);
}

/// Drops the layer draws, which is one per cell of `scene.slang`'s rain grid.
///
/// **`RAIN_DROPS` in `include/rain.slang`, and it is not free to be any
/// number**: it is the length of the buffer `rain_sim.slang` simulates and the
/// vertex shader indexes with no bounds test. A short draw would leave a slab
/// of the buffer undrawn rather than thin the field; a long one reads past the
/// end through a device address, which is a hang. The rate thins the layer
/// inside the shader instead, where a drop keeps or drops *itself*.
///
/// The extent it covers is a calibration knob — real rain at 8 mm/h has
/// millions of drops per block and no renderer draws them. What this buys is
/// the *look* of weather at the cost of its overdraw. At light rates most of
/// these collapse to a degenerate quad in the vertex stage and rasterise
/// nothing, so the overdraw still scales with the rate; only the vertex work,
/// measured at well under a tenth of a millisecond for the whole layer, does
/// not.
const MAX_RAIN_DROPS: u32 = 64 * 32 * 64;

/// Tell the shader what the rain is doing at the camera, and whether that is
/// worth a draw at all.
///
/// **The rate no longer scales the count here, and `RAIN_FULL_RATE` is gone
/// with it.** Both halves of "light rain is sparser *and* fainter" now live in
/// `scene.slang`, next to the grid whose cells they thin — one number in one
/// place instead of the same 20 mm/h spelled on both sides of the boundary.
///
/// **The UNSHELTERED rate, and no sky march at all.** This used to pass the
/// scene's voxel volume and hand the GPU `sample_rain(...).rate` at the eye —
/// the authored intensity already scaled by S3's exposure *where the camera is
/// standing*. The shader multiplies drop density and opacity by it, so walking
/// under a roof scaled the whole layer toward zero and the rain falling in the
/// open five metres away vanished with it. **Occlusion belongs to the place, not
/// to the camera**: the per-drop test against W6's height field in
/// `rainVertexMain` already kills exactly the drops that would fall inside the
/// shelter, and that answer is right from any viewpoint.
///
/// So what goes to the GPU is what the sky is doing, and the shader decides
/// which drops the world stops. `loom sim --assert "rain@x,y,z.rate"` is
/// unaffected: it asks `sample_rain` about a *point*, which is the question
/// exposure answers, and this was never that call's reader.
///
/// The wind is unsheltered for the same reason. It has to be one vector for the
/// whole layer — a velocity that varied with position would stop a drop's
/// position being a closed form of its index and the time — and sheltering it at
/// the eye made every streak in the scene fall vertically the moment the camera
/// stepped under the roof.
///
/// What the GPU gets is four floats and a draw count; it writes nothing back,
/// which is what keeps this whole layer out of the determinism hash.
pub(crate) fn rain_at_eye(
    env: &mut loom_render::EnvironmentData,
    rain: Option<&loom_scene::components::Rain>,
    wind: &loom_field::wind::Wind,
    eye: Vec3,
    seconds: f32,
) -> u32 {
    if rain.is_none() {
        return 0;
    }
    // **No deck here, deliberately.** This is the rate handed to the whole GPU
    // layer, and cover — like shelter before it — is a property of *where a
    // drop is*, not of where the camera stands. Passing the deck would make the
    // cloud over the camera decide whether it rains five metres away, which is
    // precisely the bug ADR 0014's amendment records for exposure. The shader
    // applies `clouds_at` per drop.
    let sample = weather::rain_at(rain, wind, None, None, eye.to_array(), seconds);
    env.rain = [sample.wind[0], sample.wind[1], sample.wind[2], sample.rate];

    // **Rain out of a clear sky is not a weather state**, and this is the only
    // place that knows both the sky and the rain. A scene that says it rains
    // and says nothing about cover gets a solid deck.
    //
    // Solid, not "mostly": at cover 1.0 the coverage field is 1 everywhere
    // (measured in `loom_field`'s `coverage_shape` test), so rain stays uniform
    // and every rate assertion written before clouds existed holds unchanged.
    // Anything less would put dry pinholes in a downpour and make those
    // assertions depend on where their point happened to land.
    //
    // Authoring any cover above zero takes full control — that is how a scene
    // asks for a squall crossing a bay, and the rain then follows it because
    // the shader reads these same three numbers.
    if rain.is_some_and(|r| r.intensity > 0.0) && env.cloud[0] <= 0.0 {
        env.cloud[0] = 1.0;
    }

    // **Wetness, at full exposure, for the whole scene.** Two scalars and no
    // per-surface state: the shader decides how much of them reaches a given
    // pixel, out of the terrain height field it already reads. Asking for the
    // exposure at the *eye* here would be wrong in a way the streaks can get
    // away with and this cannot — walking under a roof would dry the field
    // outside it.
    //
    // `weather.w` and `water.w` were the two free trailing scalars in the
    // environment buffer, which is exactly what that buffer's packing is for.
    // Growing it by a `float4` would move `terrainHeights` and every offset
    // pinned in `loom_render`'s layout test, for two floats.
    let wet = loom_rain::wetness(rain, 1.0, seconds);
    env.weather[3] = wet.film;
    env.water[3] = wet.soak;

    // Every cell, or none. A shower that has ended still skips the draw
    // entirely; a sheltered camera no longer does, which is the fix.
    if sample.rate > 0.0 { MAX_RAIN_DROPS } else { 0 }
}

/// Collect every `Light` a scene declares into the environment buffer.
///
/// **Gathered outside `environment_of_inner` because lights are not part of
/// the `Environment` component** — that function returns early when a scene
/// declares no environment at all, and a scene lit only by a campfire is
/// exactly the scene most likely to declare none.
///
/// World position comes from the propagated global transform, so a light
/// parented to a moving node moves with it. The translation is the last column
/// of a column-major matrix.
fn gather_lights(world: &World, env: &mut loom_render::EnvironmentData) {
    let mut count = 0usize;
    for entity in world.entities() {
        if count >= loom_render::MAX_LIGHTS {
            break;
        }
        let Some(component) = world.light(*entity) else {
            continue;
        };
        let Some(global) = world.global_transform(*entity) else {
            continue;
        };
        #[allow(clippy::cast_possible_truncation)]
        let scalar = |name: &str, fallback: f32| {
            component
                .get(name)
                .and_then(serde_json::Value::as_f64)
                .map_or(fallback, |v| v as f32)
        };
        let intensity = scalar("intensity", 100.0);
        if intensity <= 0.0 {
            continue;
        }
        let mut color = [1.0_f32; 3];
        if let Some(values) = component.get("color").and_then(serde_json::Value::as_array) {
            for (slot, value) in color.iter_mut().zip(values) {
                if let Some(v) = value.as_f64() {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        *slot = v as f32;
                    }
                }
            }
        }
        let m = global.matrix;
        // **Radius from intensity rather than authored.** A cutoff is a
        // performance bound, not a property of a light, and asking an author
        // for one invites a number that disagrees with the brightness. This is
        // where an inverse square falls to roughly a hundredth of a unit,
        // which is below what any display shows.
        let radius = (intensity * 100.0).sqrt().clamp(1.0, 400.0);
        env.lights[count] = loom_render::PointLight {
            position: [m[12], m[13], m[14], intensity],
            color: [color[0], color[1], color[2], radius],
        };
        count += 1;
    }
    env.light_count = u32::try_from(count).unwrap_or(0);
}

fn environment_of_inner(world: &World) -> loom_render::EnvironmentData {
    let defaults = loom_scene::components::Environment::default();
    let Some(component) = world.environment() else {
        return loom_render::EnvironmentData::default();
    };
    #[allow(clippy::cast_possible_truncation)]
    let scalar = |name: &str, fallback: f32| {
        component
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .map_or(fallback, |v| v as f32)
    };
    let vector = |name: &str, fallback: [f32; 3]| {
        let Some(values) = component.get(name).and_then(serde_json::Value::as_array) else {
            return fallback;
        };
        let mut out = fallback;
        for (slot, value) in out.iter_mut().zip(values) {
            if let Some(v) = value.as_f64() {
                #[allow(clippy::cast_possible_truncation)]
                {
                    *slot = v as f32;
                }
            }
        }
        out
    };

    // Normalised here rather than in the shader, so a scene can write
    // `[0, 1, 0]` or `[0, 100, 0]` and mean the same thing.
    let sun = vector("sun_direction", defaults.sun_direction);
    let length = sun.iter().map(|c| c * c).sum::<f32>().sqrt();
    let sun = if length < 1e-6 {
        defaults.sun_direction
    } else {
        [sun[0] / length, sun[1] / length, sun[2] / length]
    };
    let sun_color = vector("sun_color", defaults.sun_color);
    let zenith = vector("sky_zenith", defaults.sky_zenith);
    let horizon = vector("sky_horizon", defaults.sky_horizon);

    loom_render::EnvironmentData {
        sun: [sun[0], sun[1], sun[2], scalar("sun_strength", defaults.sun_strength)],
        sun_color: [
            sun_color[0],
            sun_color[1],
            sun_color[2],
            scalar("ambient", defaults.ambient),
        ],
        zenith: [zenith[0], zenith[1], zenith[2], scalar("fog_density", defaults.fog_density)],
        horizon: [
            horizon[0],
            horizon[1],
            horizon[2],
            scalar("fog_falloff", defaults.fog_falloff),
        ],
        // Cover is the authored value only. **The rain floor is applied in
        // `environment_with_wind`**, which is the caller that knows whether the
        // scene rains; putting it here would need this function to reach for a
        // component it otherwise has no business reading.
        cloud: [
            scalar("cloud_cover", defaults.cloud_cover),
            scalar("cloud_scale", defaults.cloud_scale),
            2.5,
            0.0,
        ],
        exposure: scalar("exposure", defaults.exposure),
        // The sky is this function's business; the weather is filled in by
        // `environment_with_wind`, which is the only caller that knows the
        // scene's `Wind` and the simulation's clock.
        ..loom_render::EnvironmentData::default()
    }
}

/// Distinct per-object colours until materials exist (M5).
fn palette(index: usize) -> [f32; 3] {
    // Linear, converted from the sRGB values these used to be. The renderer
    // encodes on write now, so a colour written here is a physical quantity
    // rather than a screen value — and the two differ by a gamma.
    const COLORS: [[f32; 3]; 6] = [
        [0.6921, 0.1005, 0.1005],
        [0.1005, 0.5225, 0.6921],
        [0.7874, 0.5225, 0.1005],
        [0.2633, 0.6038, 0.1706],
        [0.4480, 0.2140, 0.6921],
        [0.6038, 0.2633, 0.1329],
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
    // Expanded before the simulation sees it — see `render`.
    let scene = match prefab_load::for_reading(&scene, std::path::Path::new(path)) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let mut world = World::from_scene(&scene);
    let mut clock = FixedTimestep::new(60.0);

    let base = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let mut runner = match play::Runner::new(&world, base) {
        Ok(r) => r,
        Err(json) => return (1, json),
    };

    // How far each node moved vertically over the closing stretch of the run,
    // for the `.bob` assertion. Recorded rather than derived afterwards
    // because a final position cannot tell a settled body from one passing
    // through the middle of a swing.
    let mut travel: Vec<(String, f32, f32)> = Vec::new();
    let window_opens = ticks.saturating_sub(RESONANCE_WINDOW);

    // Elapsed time is fed in as an exact constant, never read from the wall
    // clock (never-do #8). That is what makes this reproducible, and it is why
    // `advance` takes the delta as an argument.
    for _ in 0..ticks {
        clock.advance(clock.step_seconds());
        if let Err(e) = runner.tick(&mut world, clock.tick) {
            return (1, json_line(&e));
        }
        if clock.tick > window_opens {
            record_travel(&world, &mut travel);
        }
        // A finished game stops. Running past the end would let the world
        // drift on after the result was decided, so a `--ticks` that happened
        // to be generous would report a different final state than one that
        // was exact — and both would claim to be the same run.
        if runner.state().status().is_over() {
            break;
        }
    }

    // Assertions are checked after the run, against final world state. This is
    // what makes an agent's claim about behaviour checkable rather than
    // asserted (design doc §2.10).
    let log = runner.events().clone();
    let state = runner.state();
    let specs = flags(args, "--assert");
    // The scene's weather, sampled at the tick the run ended on — so a wind or
    // rain assertion is checked against the same clock the simulation used
    // rather than against a wall clock (never-do #8).
    let weather = weather::Weather {
        wind: weather::wind_of(&scene),
        rain: weather::rain_of(&scene),
        // **Rebuilt only when something asks about rain.** The bake is a pass
        // over every voxel in the scene and `terrain_stress.loom` has 67
        // million of them; a run that never mentions rain must not pay for it.
        sky: specs
            .iter()
            .any(|spec| {
                let spec = spec.trim_start();
                spec.starts_with("rain") || spec.starts_with("wetness")
            })
            .then(|| scene_volume(&scene))
            .flatten(),
        #[allow(clippy::cast_precision_loss)]
        seconds: ticks as f32 / 60.0,
        deck: weather::deck_of(&world),
    };
    let mut failures = Vec::new();
    for spec in &specs {
        match check_assertion(&world, state, &log, &weather, &travel, spec) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                let actual = assertion_actual(&world, state, &log, &weather, &travel, spec);
                failures.push(serde_json::json!({
                    "assert": spec,
                    "actual": actual,
                    "hint": "Format is `Node/Path.axis OP value`, axis one of x/y/z \
                             or `bob` (peak-to-peak vertical travel over the last \
                             300 ticks), OP one of > >= < <= == ~=. \
                             `status == won|lost|playing`, \
                             `state.<name> OP value` and `events.<kind> OP n` \
                             check the game's rules and what happened. \
                             `wind@x,y,z.<x|y|z|speed>`, \
                             `rain@x,y,z.<rate|exposure>` and \
                             `wetness@x,y,z.<film|soak>` read the weather where \
                             it is asked about.",
                }));
            }
        }
    }
    // The game's own result, reported whether or not anything asserted on it.
    // An agent that ran a scene and got `"status": "lost"` learns something
    // from the line it already had to read.
    let mut game = serde_json::Map::new();
    game.insert("status".into(), serde_json::json!(state.status().as_str()));
    if !state.message().is_empty() {
        game.insert("message".into(), serde_json::json!(state.message()));
    }
    for (name, value) in state.numbers() {
        game.insert(name, serde_json::json!(value));
    }
    // What happened, by kind. Reported whether or not anything asserted on it:
    // an agent that ran a scene and sees `"damage": 0` learns why nobody died
    // from the line it already had to read.
    if !log.counts().is_empty() {
        game.insert("events".into(), serde_json::json!(log.counts()));
    }

    if !failures.is_empty() {
        return (1, json_line(&serde_json::json!({
            "ok": false,
            "ticks": clock.tick,
            "game": game,
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
            "game": game,
            "assertions": specs.len(),
        })),
    )
}

/// Step physics and write the result back onto the world.
///
/// One line, because the substance moved to `play::Sim` when the editor grew a
/// Play button — the headless path and the interactive one must agree about
/// what a scene means physically, and the only way to guarantee that is for
/// there to be one of them.
/// Run a scene forward for `ticks`, for a render that wants a simulated view.
///
/// A failing script is reported and the run stops there rather than aborting
/// the render: a picture of the scene up to the failure is more use to whoever
/// has to fix the script than no picture at all, which is the same reasoning
/// that makes a missing texture a warning.
/// When and where something the particles have to replay happened.
type Happenings = Vec<(u64, [f32; 3])>;

fn simulate_physics(
    world: &mut World,
    base: &std::path::Path,
    ticks: u32,
) -> (Happenings, Happenings) {
    let mut runner = match play::Runner::new(world, base) {
        Ok(r) => r,
        Err(json) => {
            log::warn(format!("scripts did not load, running physics only: {json}"));
            let mut sim = play::Sim::new(world);
            sim.step(ticks);
            sim.write_back(world);
            return (Vec::new(), Vec::new());
        }
    };
    for tick in 1..=u64::from(ticks) {
        if let Err(e) = runner.tick(world, tick) {
            log::warn(format!("{}: {}", e.script, e.message));
            break;
        }
    }
    (runner.fired(), runner.splashed())
}

/// Every value given for a repeated flag.
fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, a)| *a == name)
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect()
}

/// How many closing ticks `.bob` measures over — five seconds.
///
/// **Long enough to contain several cycles of anything a body floats on.** A
/// shorter window would catch a body at the top of one swing and call it
/// settled; a longer one would blur the entry transient back into the answer,
/// which is the thing the measurement is trying to see the far side of.
const RESONANCE_WINDOW: u64 = 300;

/// Fold this tick's positions into the per-node vertical travel.
///
/// A linear scan per node, which is O(n²) over a tick and irrelevant at the
/// scale of a scene — `ponytail:` sort or index it if a scene with thousands of
/// nodes ever makes `loom sim` slow.
fn record_travel(world: &World, travel: &mut Vec<(String, f32, f32)>) {
    for (path, at) in world.positions() {
        match travel.iter_mut().find(|(name, ..)| *name == path) {
            Some((_, low, high)) => {
                *low = low.min(at[1]);
                *high = high.max(at[1]);
            }
            None => travel.push((path, at[1], at[1])),
        }
    }
}

/// The world-space value an assertion refers to.
fn assertion_value(
    world: &World,
    state: &loom_script::GameState,
    log: &loom_script::EventLog,
    weather: &weather::Weather,
    travel: &[(String, f32, f32)],
    path: &str,
    axis: &str,
) -> Option<f32> {
    // `wind@12,2,-4.speed >= 3` — the field itself, at a fixed position and
    // the tick the run ended on. P1'''s exit criterion asks for exactly this:
    // an assertion on `wind_at()` that runs identically in debug and release,
    // because a wind field is the kind of thing that can drift without any
    // visible symptom until vegetation four phases later leans wrong.
    if let Some(rest) = path.strip_prefix("wind") {
        let at = rest.strip_prefix('@').map_or(Some([0.0, 0.0, 0.0]), parse_vec3)?;
        let v = weather.wind.at(at, weather.seconds);
        return match axis {
            "x" => Some(v[0]),
            "y" => Some(v[1]),
            "z" => Some(v[2]),
            "speed" => Some(v[0].mul_add(v[0], v[2] * v[2]).sqrt()),
            _ => None,
        };
    }

    // `wetness@2,1,-4.film` — **the third exit criterion, and it is temporal,
    // which is why it is here and not in a screenshot.** "Wetness accumulates
    // and dries" is a claim about how a surface changes over a run: a still
    // cannot distinguish a scene that dried from one that was never wet. The
    // two axes are the two rates — `film` is the sheen, `soak` is the
    // darkening, and the whole point of the model is that they do not decay
    // together. Checked before `rain`, because "wetness" would otherwise never
    // be reached: `strip_prefix("rain")` does not match it, but the reader who
    // adds the next axis should not have to know that.
    if let Some(rest) = path.strip_prefix("wetness") {
        let at = rest.strip_prefix('@').map_or(Some([0.0, 0.0, 0.0]), parse_vec3)?;
        let wet = weather.wetness_at(at);
        return match axis {
            "film" => Some(wet.film),
            "soak" => Some(wet.soak),
            _ => None,
        };
    }

    // `rain@2,1,-4.rate < 0.05` — **the only way Phase 4's exit criteria are
    // checkable at all.** "Rain visibly stops under an overhang" is a claim
    // about a still picture that a still picture cannot settle: the streaks are
    // stateless GPU noise deliberately outside the simulation, so what has to
    // be right is the rate the CPU says is reaching the point, and that is what
    // this reads. Sheltering comes from S3's march of the scene's own voxels —
    // see `loom_rain`.
    if let Some(rest) = path.strip_prefix("rain") {
        let at = rest.strip_prefix('@').map_or(Some([0.0, 0.0, 0.0]), parse_vec3)?;
        let sample = weather.rain_at(at);
        return match axis {
            "rate" => Some(sample.rate),
            "exposure" => Some(sample.exposure),
            _ => None,
        };
    }

    // `events.damage >= 1` — what happened, rather than where things ended up.
    // A game can be wrong in ways no position shows: nobody was ever hit, the
    // explosion never fired, the pickup was never taken.
    if path == "events" {
        #[allow(clippy::cast_precision_loss)]
        return Some(log.count_of(axis) as f32);
    }

    // `state.score > 10` — the rules' own numbers, checkable the same way a
    // position is. Without this a game loop can only be asserted on through
    // whatever it happened to move.
    if path == "state" {
        #[allow(clippy::cast_possible_truncation)]
        return state.number(axis).map(|v| v as f32);
    }

    // `Crate.bob < 1.3` — **how far a body moved up and down over the last five
    // seconds, not where it ended up.** A final position cannot tell a settled
    // body from one caught in the middle of a swing, which is exactly the
    // question a buoyancy assertion has to answer: an undamped float passes
    // through its resting height twice a cycle forever (water doc §5.5).
    if axis == "bob" {
        return travel
            .iter()
            .find(|(name, ..)| name == path)
            .map(|(_, low, high)| high - low);
    }

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
fn check_assertion(
    world: &World,
    state: &loom_script::GameState,
    log: &loom_script::EventLog,
    weather: &weather::Weather,
    travel: &[(String, f32, f32)],
    spec: &str,
) -> Result<bool, ()> {
    // `status == won` is the assertion a game loop exists to make, and it is
    // not a number on an axis — so it is matched before the geometry form
    // rather than bent into it.
    if let Some(expected) = spec.strip_prefix("status") {
        let expected = expected.trim().strip_prefix("==").ok_or(())?.trim();
        return Ok(state.status().as_str() == expected);
    }

    let (path, axis, op, expected) = parse_assertion(spec).ok_or(())?;
    let actual =
        assertion_value(world, state, log, weather, travel, &path, &axis).ok_or(())?;
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

fn assertion_actual(
    world: &World,
    state: &loom_script::GameState,
    log: &loom_script::EventLog,
    weather: &weather::Weather,
    travel: &[(String, f32, f32)],
    spec: &str,
) -> serde_json::Value {
    if spec.trim_start().starts_with("status") {
        return serde_json::json!(state.status().as_str());
    }
    parse_assertion(spec)
        .and_then(|(path, axis, _, _)| {
            assertion_value(world, state, log, weather, travel, &path, &axis)
        })
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

    // No read of the scene here on purpose: `apply_to_file` reads it inside the
    // lock. Reading it out here as well would reintroduce the window the lock
    // exists to close, and the value would be stale by the time it was used.
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

    // --dry-run prints the diff and touches nothing. This is how the human
    // reviews a large change before it lands. `apply_to_file` honours it and
    // holds the scene lock across read-apply-write, so the version check is
    // compared against a scene that cannot change before the write lands.
    match loom_scene::apply_to_file(std::path::Path::new(path), &transaction) {
        Ok(applied) => {
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
        Err(e) => file_apply_error(path, &e),
    }
}

/// Render a failed file-apply as the CLI's two existing error shapes: an io
/// error names the path, a rejected transaction carries its own JSON.
fn file_apply_error(path: &str, error: &loom_scene::FileApplyError) -> (u8, String) {
    match error {
        loom_scene::FileApplyError::Io(e) => (
            1,
            json_line(&serde_json::json!({
                "error": "io_error", "path": path, "constraint": e.to_string(),
            })),
        ),
        loom_scene::FileApplyError::Rejected(e) => (1, json_line(e)),
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
    // **A scene, or the bare recipe it names.** The recipe is where the numbers
    // come from either way; asking through the scene is what makes them
    // answers about the thing that will actually be rendered, and it is the
    // difference between a landform an agent can iterate on and a `.toml` it
    // has to remember the name of.
    let recipe_path = match recipe_of(path) {
        Ok(p) => p,
        Err(e) => return (1, e),
    };
    let recipe = match loom_terrain::Recipe::load(&recipe_path) {
        Ok(r) => r,
        Err(e) => {
            return (
                1,
                json_line(&serde_json::json!({
                    "error": "invalid_recipe", "path": recipe_path, "constraint": e.to_string(),
                })),
            );
        }
    };
    let path = recipe_path.to_string_lossy().into_owned();
    let path = path.as_str();

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
            // **`size` is pixels and `world_scale` is metres**, and the two are
            // only equal by convention in this library. A `terrain` op's `rect`
            // must be exactly `world_scale` across — that is the one number an
            // agent needed from here and had to open the `.toml` to get. With
            // it, `rect` and the volume's Y extent are both arithmetic on this
            // output: `rect = [x0, z0, x0 + world_scale[0], z0 + world_scale[1]]`
            // and `base_y + height.max` must fit inside `chunks[1] * 32 * voxel_size`.
            "world_scale": recipe.world_scale,
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

/// The recipe `loom terrain` should read: the argument itself, or the one the
/// scene's first `terrain` op names.
///
/// The scene is loaded through [`prefab_load::for_reading`] like every other
/// command that reads one — a `VoxelVolume` can arrive from a prefab, and a
/// reader that skipped resolution would report on a scene that has no ops at
/// all and call it fine.
fn recipe_of(path: &str) -> Result<std::path::PathBuf, String> {
    if !path.ends_with(".loom") {
        return Ok(std::path::PathBuf::from(path));
    }
    let src = std::fs::read_to_string(path).map_err(|e| {
        json_line(&serde_json::json!({
            "error": "io_error", "path": path, "constraint": e.to_string(),
        }))
    })?;
    let scene = Scene::parse(&src).map_err(|errors| json_line(&serde_json::json!({ "errors": errors })))?;
    let scene = prefab_load::for_reading(&scene, std::path::Path::new(path))
        .map_err(|errors| json_line(&serde_json::json!({ "errors": errors })))?;

    let named = scene.nodes().iter().find_map(|node| {
        node.components
            .get("VoxelVolume")?
            .get("ops")?
            .as_array()?
            .iter()
            .find(|op| op.get("kind").and_then(serde_json::Value::as_str) == Some("terrain"))?
            .get("recipe")?
            .as_str()
            .map(str::to_owned)
    });
    match named {
        Some(recipe) => Ok(scene_base().join(recipe)),
        None => Err(json_line(&serde_json::json!({
            "error": "no_terrain_op", "path": path,
            "hint": "This scene has no `kind = \"terrain\"` voxel op, so there is no recipe to \
                     report on. Add one, or pass the .toml recipe directly.",
        }))),
    }
}

fn parse_vec2(spec: &str) -> Option<[usize; 2]> {
    let parts: Vec<usize> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (parts.len() == 2).then(|| [parts[0], parts[1]])
}

/// `x,z`, or `x,y,z` when the caller also wants the submersion test.
///
/// Strict, unlike [`parse_vec2`]: every component must parse. Dropping an
/// unparseable one would turn `--at 4,oops,9` into the point `(4, 9)` and
/// answer confidently about somewhere else, which is the failure this whole
/// CLI is arranged to prevent.
fn parse_at(spec: &str) -> Option<([f32; 2], Option<f32>)> {
    let parts: Vec<f32> = spec
        .split(',')
        .map(|p| p.trim().parse())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts[..] {
        [x, z] => Some(([x, z], None)),
        [x, y, z] => Some(([x, z], Some(y))),
        _ => None,
    }
}

/// What the water is doing at one point, at one instant.
///
/// **This computes nothing.** Every number below comes out of the function the
/// engine itself calls for it: the wave set from [`weather::water_of`], which
/// is what the renderer and the buoyancy solver both read; the surface from
/// [`loom_water::sample_water`]; the bed from [`scene_terrain_field`], the same
/// grid uploaded to the shader; the current from [`river_flow`]; the
/// submersion test from [`loom_water::buoyancy::submersion_at`], which is what
/// decides a swimmer's muffled ears and the underwater tint. A query that
/// recomputed any of them would agree with the engine right up until the day
/// it mattered.
///
/// **Time is `--sim <ticks>` at the fixed 60 Hz step, and it defaults to 0.**
/// Waves move, so an instant is part of the question rather than a detail of
/// it — and t = 0 is the worst possible default to leave implicit, because it
/// is the one moment every wave in the set is at the same phase. Both the tick
/// count and the seconds it means are in the output, so an answer can never be
/// read without the instant it belongs to.
/// Render the scene's weather bed offline and measure it.
///
/// **The instrument, and it is the same argument S1 made for golden images.**
/// Without it, "the rain sounds right" is a human with a speaker and nothing in
/// the gate can see it at all — and audio is worse than pixels here, because a
/// silent bug produces no output to look at rather than a wrong picture.
///
/// It reports measured quantities rather than a hash, for the reason
/// `rain::Measurement` gives: a hash pins the arithmetic, and what is worth
/// asserting is claims about the sound.
fn audio_cmd(path: &str, args: &[String]) -> (u8, String) {
    let seconds = match flag(args, "--seconds") {
        None => 4.0_f32,
        Some(spec) => match spec.parse::<f32>() {
            Ok(s) if s.is_finite() && s > 0.0 && s <= 600.0 => s,
            _ => return (2, json_line(&serde_json::json!({
                "error": "invalid_argument", "value": spec,
                "constraint": "--seconds takes a positive number up to 600",
            }))),
        },
    };
    let openness = match flag(args, "--openness") {
        None => 1.0_f32,
        Some(spec) => match spec.parse::<f32>() {
            Ok(v) if (0.0..=1.0).contains(&v) => v,
            _ => return (2, json_line(&serde_json::json!({
                "error": "invalid_argument", "value": spec,
                "constraint": "--openness takes 0 (sealed) to 1 (open sky)",
            }))),
        },
    };

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
    // A `Rain` on a prefab instance is a component like any other, and an
    // unexpanded instance carries none — the scene would render silence and
    // look like a working dry scene.
    let scene = match prefab_load::for_reading(&scene, std::path::Path::new(path)) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let intensity = weather::rain_of(&scene).map_or(0.0, |r| r.intensity);
    const RATE: u32 = 48_000;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (RATE as f32 * seconds) as usize;
    let mut samples = vec![0.0_f32; frames * 2];
    let mut bed = loom_audio::rain::RainBed::new(RATE);
    // Relative to the scene, like every other asset reference. Missing falls
    // back to the synthesiser rather than to silence.
    let recording = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("../audio/rain.wav");
    let source = if bed.use_recording_at(&recording) { "recording" } else { "synthesised" };
    bed.render(
        loom_audio::rain::RainAudio { intensity, openness, volume: 1.0 },
        &mut samples,
    );
    let measured = loom_audio::rain::measure(&samples, RATE);

    let mut written = serde_json::Value::Null;
    if let Some(out) = flag(args, "--out") {
        let bytes = loom_audio::Clip::encode(&samples, RATE, 2);
        if let Err(e) = std::fs::write(&out, bytes) {
            return (2, json_line(&serde_json::json!({
                "error": "io_error", "path": out, "constraint": e.to_string(),
            })));
        }
        written = serde_json::Value::String(out);
    }

    (0, json_line(&serde_json::json!({
        "ok": true,
        "path": path,
        "source": source,
        "intensity": intensity,
        "openness": openness,
        "seconds": seconds,
        "rms": measured.rms,
        "peak": measured.peak,
        "tilt": measured.tilt,
        "out": written,
    })))
}

fn water(path: &str, args: &[String]) -> (u8, String) {
    let Some((at, y)) = flag(args, "--at").and_then(|s| parse_at(&s)) else {
        return (2, json_line(&serde_json::json!({
            "error": "missing_argument",
            "constraint": "--at X,Z or --at X,Y,Z, all finite numbers",
            "hint": "--at X,Z is the point to sample. A third component asks \
                     whether that height is under the surface.",
        })));
    };
    // Rejected rather than defaulted, because a `--sim` this could not read
    // would silently report t = 0 while the caller believed it asked for t = 15.
    let ticks = match flag(args, "--sim") {
        None => 0_u32,
        Some(spec) => match spec.parse::<u32>() {
            Ok(t) => t,
            Err(_) => return (2, json_line(&serde_json::json!({
                "error": "invalid_argument", "value": spec,
                "constraint": "--sim takes a whole number of ticks",
                "hint": "Ticks are the fixed 60 Hz simulation step; `--sim 90` is 1.5 s.",
            }))),
        },
    };
    #[allow(clippy::cast_precision_loss)]
    let seconds = ticks as f32 / 60.0;

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
    // A `WaterBody` on a prefab instance is a component like any other, and an
    // unexpanded instance carries none — the scene would read as dry.
    let scene = match prefab_load::for_reading(&scene, std::path::Path::new(path)) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let world = World::from_scene(&scene);
    let wind = weather::wind_of_world(&world);
    // **Provenance has to be read before the resolution, not after.** `water_of`
    // fills an empty wave set from the spectrum, and once it has, a derived sea
    // and an authored one are the same sixteen numbers. This is the last moment
    // the difference exists.
    let authored = world
        .water()
        .and_then(|v| serde_json::from_value::<components::WaterBody>(v.clone()).ok())
        .is_some_and(|b| !b.waves.waves.is_empty());
    let Some(body) = weather::water_of(&world, &wind) else {
        return (1, json_line(&serde_json::json!({
            "error": "no_water", "path": path,
            "constraint": "the scene has no WaterBody",
            "hint": "Add a WaterBody component to a node. `loom describe WaterBody` \
                     is its schema.",
        })));
    };

    // The bed, and the current that is routed off it — the same two bakes the
    // renderer and the simulation do, through the same two functions.
    let bed = scene_terrain_field(&scene);
    let ground = bed
        .as_ref()
        .map_or(loom_voxel::heightfield::NO_GROUND, |g| g.at(at[0], at[1]));
    // **Off the grid is not an error.** There is no terrain there, so the water
    // is bottomless — which is the right answer for an open sea and exactly what
    // the shader draws. Reported as nulls rather than as the sentinel, because
    // a depth of a billion metres is a number a caller would act on.
    let grounded = loom_voxel::heightfield::HeightField::has_ground(ground);
    let flow = bed
        .as_ref()
        .and_then(|g| river_flow(g, &body))
        .map_or([0.0; 3], |grid| grid.at(at[0], at[1]));

    let sample = loom_water::sample_water(&body, at, seconds, ground, flow);
    // `sample_water` sums the orbital motion onto the current, so the wave half
    // is the difference. Subtracted rather than sampled a second time with no
    // flow: two samples is two chances to disagree.
    let orbital = [
        sample.velocity[0] - flow[0],
        sample.velocity[1] - flow[1],
        sample.velocity[2] - flow[2],
    ];

    (
        0,
        json_line(&serde_json::json!({
            "ok": true,
            "path": path,
            "at": at,
            "y": y,
            // Both, always: seconds is what the waves were evaluated at, ticks
            // is what a caller passes to `render --sim` or `sim --ticks` to see
            // this same instant.
            "ticks": ticks,
            "seconds": seconds,
            "kind": body.kind,
            "surface": {
                "height": sample.height,
                "still_height": body.surface_height,
                "normal": sample.normal,
                "displacement": sample.displacement,
            },
            "bed": {
                "height": grounded.then_some(ground),
                "depth": grounded.then_some(sample.depth),
            },
            "velocity": {
                "total": sample.velocity,
                "orbital": orbital,
                "flow": flow,
            },
            // **What an agent that just authored this body needs to see.** Not
            // the raw sample: "did I get the sea I asked for" is answered by
            // where the waves came from and how big they are.
            "waves": {
                "source": if authored { "authored" } else { "wind" },
                "count": body.waves.waves.len(),
                "significant_height": loom_water::spectrum::significant_height(&body.waves),
                "attenuation_depth": body.waves.attenuation_depth,
                "max_height": body.waves.max_height,
                // Only when it is what the sea was built from. Reporting the
                // wind of a scene whose waves are authored would read as a
                // cause when it is a bystander.
                "wind_speed_10m": (!authored).then(|| wind.mean_speed_at(10.0)),
            },
            // Null when `--at` named no height: not "no", but "not asked".
            "submerged": y.map(|y| {
                loom_water::buoyancy::submersion_at(&body, [at[0], y, at[1]], 0.0, seconds, ground)
                    > 0.5
            }),
        })),
    )
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

    let mut volume = match build_volume(component) {
        Ok(volume) => volume,
        Err(e) => {
            return (1, json_line(&serde_json::json!({
                "error": "voxel_bake_failed", "node": node, "constraint": e,
            })));
        }
    };

    // 1. Carve. The edit layer records only the chunks the blast touches.
    let blast = loom_voxel::VoxelOp::Sphere {
        center: at,
        radius,
        mode: loom_voxel::CsgMode::Subtract,
        displace: None,
        elongate: [0.0; 3],
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
    // `explode` synthesises its own debris rather than reading a scene, so it
    // has no authored materials — the palette colour is the whole surface.
    let mut renderer = match Renderer::new(&instance, &device, width, height, &meshes, &[], &[]) {
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
            material: loom_render::NO_TEXTURE,
            sway: 0.0,
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
                material: loom_render::NO_TEXTURE,
            sway: 0.0,
            });
        }

        let file = format!("{out}_{frame}.png");
        if let Err(e) = renderer.render_to_png(&objects, &[], &camera, std::path::Path::new(&file)) {
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

/// The op list a `VoxelVolume` authors, or the first one that does not parse.
///
/// **An op the schema does not recognise is an error, never a skipped line.**
/// This read used to be `filter_map(…ok())`: a misspelled `kind`, or a `sphere`
/// missing its `radius`, was dropped on the floor. The volume then baked *short*
/// — a plausible surface, a clean `loom validate`, and a bake that got
/// **faster**, so a cost measurement reported an improvement on the commit that
/// broke the scene. It is the prefab defect verbatim (CLAUDE.md: "a key it does
/// not understand is a key it ignores"), and the fix is the same one: refuse.
/// **A `terrain` op is baked here**, which is the one place in the op list
/// where parsing is not free: the recipe is loaded relative to the scene file
/// and run, and that is 0.05–0.5 s depending on how much erosion it asks for.
/// It happens at parse rather than at bake because an op with no map answers
/// "surface" everywhere, and one shared funnel is what stops that reaching a
/// volume. `loom_voxel` caches the result by recipe content hash, so the three
/// `build_volume` calls a render makes pay for one.
pub(crate) fn parse_ops(component: &serde_json::Value) -> Result<Vec<loom_voxel::VoxelOp>, String> {
    let Some(ops) = component.get("ops") else {
        return Ok(Vec::new());
    };
    let Some(ops) = ops.as_array() else {
        return Err("ops must be a list of ops".to_owned());
    };
    let mut parsed: Vec<loom_voxel::VoxelOp> = ops
        .iter()
        .enumerate()
        .map(|(index, value)| {
            serde_json::from_value(value.clone()).map_err(|e| {
                let kind = value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<missing>");
                format!("op {index} (kind = \"{kind}\"): {e}")
            })
        })
        .collect::<Result<_, _>>()?;
    loom_voxel::resolve(&mut parsed, &scene_base())?;

    // **`intersect` first is a silent empty volume.** The bake seeds its
    // accumulator at `f32::MAX` and intersect is `max`, so the first op keeps
    // the seed and every voxel stays outside; a later `union` then fills in,
    // which is why this reads as "the landform disappeared and the props are
    // still there" rather than as an empty scene. Verified: one intersect
    // sphere alone bakes to nothing while `loom validate` returned `ok: true`.
    if parsed.first().is_some_and(|op| op.mode() == loom_voxel::CsgMode::Intersect) {
        return Err(
            "op 0 has mode = \"intersect\", which intersects with nothing: the first op is \
             what there is to intersect *with*, so the volume bakes empty. Make op 0 a \
             union and intersect against it."
                .to_owned(),
        );
    }

    terrain_covers_the_volume(component, &parsed)?;
    Ok(parsed)
}

/// A `terrain` op must cover the volume it fills.
///
/// **This lives here, in the shared funnel, because it is the half of the
/// coordinate-system trap `loom validate` used to miss.** `resolve` catches a
/// rect that is not the size the recipe claims; this catches a rect that is the
/// right size in the wrong place. Outside its rect a `TerrainMap` clamps, which
/// extrudes the boundary row across everything past it — parallel ridges
/// running to the edge of the world, and they look authored.
///
/// It was checked in `build_volume` alone, so `loom validate` returned
/// `"ok": true` on a scene whose terrain then failed to bake: `loom sim` printed
/// one line to stderr, exited **0**, and the ground the player stands on was
/// simply not in the collision world. Named with both numbers, because the fix
/// is arithmetic the agent can do the moment it can see them.
fn terrain_covers_the_volume(
    component: &serde_json::Value,
    ops: &[loom_voxel::VoxelOp],
) -> Result<(), String> {
    if !ops.iter().any(|op| matches!(op, loom_voxel::VoxelOp::Terrain { .. })) {
        return Ok(());
    }
    let (Some(voxel_size), Some(chunks)) = (
        component.get("voxel_size").and_then(serde_json::Value::as_f64),
        component.get("chunks").and_then(|c| c.as_array()),
    ) else {
        // Missing or malformed dimensions are `build_volume`'s error to name.
        return Ok(());
    };
    let dims: Vec<f64> = chunks.iter().filter_map(serde_json::Value::as_f64).collect();
    if dims.len() != 3 {
        return Ok(());
    }
    #[allow(clippy::cast_possible_truncation)]
    let footprint = [
        (dims[0] * 32.0 * voxel_size) as f32,
        (dims[2] * 32.0 * voxel_size) as f32,
    ];
    for (index, op) in ops.iter().enumerate() {
        let loom_voxel::VoxelOp::Terrain { rect, .. } = op else {
            continue;
        };
        if rect[0] > 0.0 || rect[1] > 0.0 || rect[2] < footprint[0] || rect[3] < footprint[1] {
            return Err(format!(
                "op {index} (kind = \"terrain\"): rect {rect:?} does not cover the volume, \
                 which is [0.0, 0.0, {}, {}]. Op coordinates are the volume's own space: \
                 its near corner is [0, 0, 0] and it runs in +X, +Y and +Z for \
                 chunks * 32 * voxel_size, and the node's transform places that box in the \
                 world. So the rect must start at or before 0 and end at or after that. \
                 Outside its rect a recipe repeats its edge row forever.",
                footprint[0], footprint[1]
            ));
        }
    }
    Ok(())
}

// The directory scene-relative paths resolve against.
//
// `ponytail:` a global. One `loom` invocation works on one scene, and the
// alternative is threading a base through `build_volume`, `scene_volume`,
// `bake_voxel`, the physics build in `play.rs` and every helper between them —
// forty signatures so that one op can find one file. A command that ever loads
// two scenes from different directories is the trigger to thread it properly.
//
// **Thread-local rather than a `Mutex`, and that is not a style preference.**
// The test binary runs its cases in parallel *in one process*, so a process
// global made every recipe-carrying scene's base whichever test set it last:
// two tests validating scenes from two temp directories, both with the recipe
// beside them, and one of them failing on `No such file or directory`. It was
// not the assertion that was flaky — it was the state. Each thread getting its
// own base is the same behaviour for the real single-threaded CLI (verified:
// nothing in `loom_cli` or `loom_render` spawns a thread) and removes the class.
thread_local! {
    static SCENE_BASE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point scene-relative paths at the directory of `path`.
fn set_scene_base(path: &str) {
    let base = std::path::Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    SCENE_BASE.with_borrow_mut(|b| *b = Some(base));
}

fn scene_base() -> std::path::PathBuf {
    SCENE_BASE.with_borrow(|b| b.clone().unwrap_or_else(|| std::path::PathBuf::from(".")))
}

/// Build and bake a voxel volume from a component.
pub(crate) fn build_volume(component: &serde_json::Value) -> Result<loom_voxel::Volume, String> {
    #[allow(clippy::cast_possible_truncation)]
    let voxel_size = component
        .get("voxel_size")
        .and_then(serde_json::Value::as_f64)
        .ok_or("voxel_size is missing or not a number")? as f32;
    let dims: Vec<usize> = component
        .get("chunks")
        .and_then(|c| c.as_array())
        .ok_or("chunks is missing or not a list")?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();
    if dims.len() != 3 {
        return Err("chunks must be three whole numbers".to_owned());
    }
    let ops = parse_ops(component)?;
    if ops.is_empty() {
        return Err("the volume authors no ops, so it has no shape".to_owned());
    }
    // The terrain rect is checked in `parse_ops`, so `loom validate` catches it
    // too — a coverage failure used to reach this point and be reported as a
    // stderr line on an exit-0 run.
    let mut volume = loom_voxel::Volume::new([dims[0], dims[1], dims[2]], voxel_size);
    volume.bake(&ops);
    Ok(volume)
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
        // Was hardcoded `None`, with no flag to set it, so a semantic
        // placement could never be rejected as stale — it always overwrote
        // whatever had landed since it read. `scene --tx` could carry a token
        // in its JSON; this path had no way to express one at all.
        // Defaults to the version this command actually read. Unlike
        // `scene --tx`, placement resolves geometry — bounds, world
        // transforms, what is "on top of" what — from a read taken *before*
        // the lock, so the ops it computed only make sense against that
        // content. Falling back to `None` here would apply geometry reasoned
        // from one scene to a different one.
        expect_version: flag(args, "--expect-version")
            .map(loom_scene::VersionToken)
            .or_else(|| Some(loom_scene::VersionToken::of(&src))),
    };
    match loom_scene::apply_to_file(std::path::Path::new(path), &transaction) {
        Ok(applied) => (0, json_line(&serde_json::json!({
            "ok": true, "placed": ops.len(),
            "dry_run": transaction.dry_run,
            "version": applied.version,
        }))),
        Err(e) => file_apply_error(path, &e),
    }
}

/// Bounds and overlaps, so the agent can check itself numerically.
///
/// Cheaper than a render and catches what one camera angle hides — an object
/// inside another object (design doc §2.8).
fn measure(path: &str, args: &[String]) -> (u8, String) {
    if args.iter().any(|a| a == "--shape") {
        return measure_shape(path, args);
    }
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

    // Measuring an unexpanded instance would report a node with no geometry —
    // true of the file, false of the scene.
    let scene = match prefab_load::for_reading(&scene, std::path::Path::new(path)) {
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

/// The rule that makes this measurement comparable, carried in its own output.
///
/// Exactly the discipline `loom flicker` carries: discrete curvature on a
/// Surface Nets mesh is resolution-dependent, so a finer bake scores higher for
/// no shape reason at all. Emitting `voxel_size` beside every row is what makes
/// "compare like with like" checkable by the agent rather than remembered.
const SHAPE_RULE: &str = "report-only, never a gate: concave-area fraction is \
     resolution-dependent, so only compare shapes measured at equal voxel_size";

/// How *rocky* a shape is: `loom measure <scene|mesh> --shape`.
///
/// **Because the alternative is render-and-look.** `Displace` has four numbers
/// and no way to judge them; this is the number that judges them. See
/// `loom_asset::shape` for what it computes and
/// `cargo run --release -p loom_voxel --example rockcalib` for the calibration
/// it was chosen by.
fn measure_shape(path: &str, args: &[String]) -> (u8, String) {
    let p = std::path::Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();

    // A mesh file measures directly. That is what makes the scanned rock in
    // `assets/meshes` usable as the target rather than as a remembered number.
    if ext != "loom" {
        let imported = if ext == "obj" {
            loom_asset::mesh::import_obj(p)
        } else {
            loom_asset::mesh::import_gltf(p)
        };
        return match imported {
            Ok(mesh) => (
                0,
                json_line(&serde_json::json!({
                    "ok": true, "rule": SHAPE_RULE,
                    "shapes": [shape_json(&mesh.name, None, &loom_asset::shape::stats(&mesh))],
                })),
            ),
            Err(e) => (
                2,
                json_line(&serde_json::json!({
                    "error": "import_failed", "path": path, "constraint": e.to_string(),
                })),
            ),
        };
    }

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
    // never-do: a scene read anywhere must go through the prefab load path, or
    // an instance arrives with no components and measures as nothing.
    let scene = match prefab_load::for_reading(&scene, p) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };
    let base = p.parent().unwrap_or(std::path::Path::new("."));
    let library = MeshLibrary::for_scene(&scene, base);
    let only = flag(args, "--node");

    let mut shapes = Vec::new();
    for node in scene.nodes() {
        if only.as_ref().is_some_and(|n| *n != node.path) {
            continue;
        }
        let voxel = node.components.get("VoxelVolume");
        // The voxel bake is keyed by node path; a `MeshRenderer` names its asset.
        let key = if voxel.is_some() {
            format!("voxel:{}", node.path)
        } else {
            match node
                .components
                .get("MeshRenderer")
                .and_then(|m| m.get("mesh"))
                .and_then(|m| m.get("asset"))
                .and_then(serde_json::Value::as_str)
            {
                Some(asset) => asset.to_owned(),
                None => continue,
            }
        };
        let Some(mesh) = library.by_name.get(&key).and_then(|i| library.meshes.get(*i as usize))
        else {
            continue;
        };
        let voxel_size =
            voxel.and_then(|v| v.get("voxel_size")).and_then(serde_json::Value::as_f64);
        shapes.push(shape_json(&node.path, voxel_size, &loom_asset::shape::stats(mesh)));
    }

    if shapes.is_empty() {
        return (
            1,
            json_line(&serde_json::json!({
                "error": "no_geometry", "path": path, "node": only,
                "hint": "The node must carry a VoxelVolume or a MeshRenderer whose mesh resolved.",
            })),
        );
    }
    (
        0,
        json_line(&serde_json::json!({ "ok": true, "rule": SHAPE_RULE, "shapes": shapes })),
    )
}

fn shape_json(
    name: &str,
    voxel_size: Option<f64>,
    s: &loom_asset::shape::ShapeStats,
) -> serde_json::Value {
    serde_json::json!({
        "node": name,
        "voxel_size": voxel_size,
        "concave": round3(s.concave),
        "spread": round3(s.spread),
        "p10": round3(s.p10), "p50": round3(s.p50), "p90": round3(s.p90),
        "triangles": s.triangles,
        "area": round3(s.area),
        "mean_radius": round3(s.mean_radius),
        // Not decoration: a large open fraction means the weld failed or the
        // shape is clipped by the volume wall, and then `concave` is measuring
        // a subset of the surface without saying so.
        "open": round3(s.boundary as f32 / s.vertices.max(1) as f32),
    })
}

/// Three decimals, in `f64` — an `f32` serialises as its exact binary value and
/// `0.442` prints as `0.44200000166893005`, which reads as false precision.
fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
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

    /// The environment a scene's text produces, wind and water and all.
    fn environment_of_text(text: &str) -> loom_render::EnvironmentData {
        let scene = loom_scene::Scene::parse(text).expect("valid scene");
        let world = World::from_scene(&scene);
        environment_with_wind(&world, &crate::weather::wind_of(&scene), 0.0)
    }

    /// **The underwater flag is a fact about the camera, and it is W7's fact.**
    ///
    /// A flat sea at y = 0 and three eyes: well under, well over, and exactly
    /// at the surface. The middle one is the one that matters — every scene
    /// that shipped before this existed renders from above, and a flag that
    /// came on for them would turn eleven references green-black at once.
    ///
    /// No terrain here, which is a scene with no volume: the bed is
    /// bottomless, the waves are unattenuated, and the answer comes from
    /// `sample_water` alone — the same call `Physics::submerged_at` makes for
    /// the audio listener.
    #[test]
    fn the_underwater_flag_follows_the_eye() {
        let text = "[scene]\nformat = 1\n\n[[node]]\nname = \"Sea\"\n\n\
                    [node.components.Wind]\nspeed = 8.0\ndirection_degrees = 0.0\n\n\
                    [node.components.WaterBody]\nsurface_height = 0.0\n";
        let scene = loom_scene::Scene::parse(text).expect("valid scene");
        let world = World::from_scene(&scene);
        let wind = crate::weather::wind_of(&scene);

        let flag = |y: f32| {
            let mut env = environment_with_wind(&world, &wind, 0.0);
            assert_eq!(env.water[1], 0.0, "the flag must start off");
            submerge_eye(&mut env, &world, &wind, None, Vec3::new(3.0, y, -4.0), 0.0);
            env.water[1]
        };

        assert_eq!(flag(-6.0), 1.0, "an eye six metres down is not submerged");
        assert_eq!(flag(6.0), 0.0, "an eye six metres up is submerged");
        // Well above the swell this wind builds, which is what every scene
        // authored before the underwater path renders from.
        assert_eq!(flag(40.0), 0.0);
    }

    /// **A scene with no `WaterBody` draws no water at all**, and the flag the
    /// draw is skipped on is the only thing that says so — there is no vertex
    /// buffer to be empty. Every scene that shipped before W4 depends on this.
    #[test]
    fn a_scene_without_water_leaves_the_water_flag_off() {
        let env = environment_of_text("[scene]\nformat = 1\n\n[[node]]\nname = \"Dry\"\n");

        assert_eq!(env.water[2], 0.0);
        assert_eq!(env.wave_count, 0);
    }

    /// **The sea is derived from the wind, and it is the right size.** The
    /// published fully-developed relation is `Hs = 0.22·U10²/g`, and the whole
    /// point of deriving rather than authoring is that a scene which says
    /// "blowing 8 m/s" gets the sea that goes with it. Checked here rather than
    /// only in `loom_water` because this is the seam: the wind component, the
    /// reference-height conversion and the buffer the GPU reads all meet in
    /// `add_water`, and each of them is a place to lose a factor.
    #[test]
    fn the_waves_are_derived_from_the_scene_s_wind() {
        let env = environment_of_text(
            "[scene]\nformat = 1\n\n[[node]]\nname = \"Sea\"\n\n\
             [node.components.Wind]\nspeed = 8.0\ndirection_degrees = 0.0\n\n\
             [node.components.WaterBody]\nsurface_height = -1.5\n",
        );

        assert_eq!(env.water[0], -1.5, "the still-water level did not reach the shader");
        assert_eq!(env.water[2], 1.0, "the water flag is off on a scene with water");
        assert_eq!(env.wave_count, u32::try_from(loom_render::MAX_WAVES).unwrap());

        // Hs = 4√m0 with m0 = ΣA²/2, summed in index order like everything
        // else. U10 is the mean at 10 m, which is not the authored `speed`.
        let u10 = crate::weather::wind_of(
            &loom_scene::Scene::parse(
                "[scene]\nformat = 1\n\n[[node]]\nname = \"Sea\"\n\n\
                 [node.components.Wind]\nspeed = 8.0\ndirection_degrees = 0.0\n",
            )
            .expect("valid scene"),
        )
        .mean_speed_at(10.0);
        let m0: f32 = env.waves.iter().map(|w| w.amplitude * w.amplitude / 2.0).sum();
        let derived = 4.0 * m0.sqrt();
        let published = 0.22 * u10 * u10 / 9.81;
        assert!(
            (derived - published).abs() / published < 0.01,
            "the sea reaching the GPU is Hs = {derived} m, the published value at \
             U10 = {u10} m/s is {published} m"
        );
        // And the waves travel: a zero speed_scale is a frozen sea, and a zero
        // wavelength is a wave the shader skips.
        for wave in &env.waves {
            assert!(wave.wavelength > 0.0 && wave.speed_scale > 0.0, "{wave:?}");
        }
    }

    /// An authored wave list wins, because a stylised or scripted sea is a
    /// legitimate thing to want and the derivation would silently overwrite it.
    #[test]
    fn an_authored_wave_list_is_not_replaced_by_the_derivation() {
        let env = environment_of_text(
            "[scene]\nformat = 1\n\n[[node]]\nname = \"Sea\"\n\n\
             [node.components.Wind]\nspeed = 8.0\n\n\
             [node.components.WaterBody]\nsurface_height = 0.0\n\n\
             [[node.components.WaterBody.waves.waves]]\n\
             wavelength = 7.0\namplitude = 0.35\nsteepness = 0.7\n\
             direction = [1.0, 0.0]\nspeed_scale = 1.0\n",
        );

        assert_eq!(env.wave_count, 1);
        assert_eq!(env.waves[0].wavelength, 7.0);
        assert_eq!(env.waves[0].amplitude, 0.35);
    }

    /// The packed colour survives the trip, unpacked exactly as `unpackRGB`
    /// does it in `scene.slang`.
    ///
    /// This is the layout-described-twice hazard in miniature: the pack is
    /// Rust and the unpack is Slang, and nothing but this test says they are
    /// the same arithmetic. Quantisation to 1/255 is the only error allowed.
    #[test]
    fn a_packed_grass_colour_unpacks_to_itself() {
        // Both shipped fields, both endpoints of the hue shift, and the
        // extremes that would overflow the 24 bits if the clamp went missing.
        for colour in [[0.29, 0.44, 0.14], [0.24, 0.40, 0.13], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]] {
            for hue in [-1.0, -0.4, 0.0, 0.4, 1.0] {
                let want = hue_shift(colour, hue);
                let packed = pack_rgb(want);
                assert!(packed <= 16_777_215.0, "{packed} is past what an f32 holds exactly");
                let unpacked = [
                    (packed / 65536.0).floor() / 255.0,
                    ((packed % 65536.0) / 256.0).floor() / 255.0,
                    (packed % 256.0) / 255.0,
                ];
                for i in 0..3 {
                    let expected = want[i].clamp(0.0, 1.0);
                    assert!(
                        (unpacked[i] - expected).abs() <= 1.0 / 255.0,
                        "channel {i} of {want:?} came back as {:?}",
                        unpacked
                    );
                }
            }
        }
    }

    /// Neighbouring clumps differ in hue, and not by much. Both halves matter:
    /// no variation is the carpet this exists to break up, and too much reads
    /// as patches of different plants.
    #[test]
    fn clumps_vary_in_hue_without_becoming_a_patchwork() {
        let rules = loom_grass::Rules::default();
        let ground = |_: f32, _: f32| loom_grass::Ground::default();
        let blades = loom_grass::tile(loom_grass::Tile { x: 0, z: 0 }, &rules, &ground);

        let hues: Vec<f32> = blades.iter().map(|b| b.hue).collect();
        let spread = hues.iter().fold(f32::MIN, |a, b| a.max(*b))
            - hues.iter().fold(f32::MAX, |a, b| a.min(*b));
        assert!(spread > 0.2, "every clump is the same hue: spread {spread}");

        // What the eye judges is the resulting colour, not the parameter, and
        // it judges it *relative* to how dark the field already is. The band
        // is deliberately wide: it is a guard against a later retune landing
        // an order of magnitude out, not a fit to today's constants. What
        // settles the strength is looking at `meadow` at 1920x1080.
        let base = [0.29, 0.44, 0.14];
        let colours: Vec<[f32; 3]> = hues.iter().map(|h| hue_shift(base, *h)).collect();
        for channel in [0, 2] {
            let values: Vec<f32> = colours.iter().map(|c| c[channel]).collect();
            let range = (values.iter().fold(f32::MIN, |a, b| a.max(*b))
                - values.iter().fold(f32::MAX, |a, b| a.min(*b)))
                / base[channel];
            assert!(
                (0.1..0.6).contains(&range),
                "channel {channel} varies by {range} of itself across one tile"
            );
        }
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

    /// **W5's exit criterion, run as the implementation order writes it.**
    ///
    ///     loom sim water_crate.loom --ticks 1800
    ///
    /// Thirty seconds, because resonance is not visible in five. The crate has
    /// to still be in the sea *and* to be moving no more than the sea is: the
    /// bounds alone are the assertion the water doc suggests and they are too
    /// weak on their own, because an undamped float passes through its resting
    /// height twice a cycle and a check at one instant catches it there half
    /// the time. `.bob` is what makes it discriminating — measured 1.01 m
    /// damped against 2.73 m with the damping removed, on a sea whose own
    /// travel there is 0.97 m.
    #[test]
    fn a_crate_floats_for_thirty_seconds_without_resonating() {
        let scene = "../../assets/test/water_crate.loom";
        let (code, out) = run(&args(&[
            "sim",
            scene,
            "--ticks",
            "1800",
            "--assert",
            "Sea/Crate.bob < 1.3",
            "--assert",
            "Sea/Crate.y < 1.5",
            "--assert",
            "Sea/Crate.y > -1.5",
        ]));

        assert_eq!(code, 0, "the crate should float and settle: {out}");
    }

    /// **The W7 exit criterion, end to end.** A crate falls in, the engine
    /// says so once, the crate floats back out, the engine says that once too,
    /// and a script reading the state ends the game on it.
    ///
    /// `== 1` rather than `>= 1` is the whole test. A state that chattered as
    /// the swell went past would satisfy `>= 1` on its first tick and go on
    /// firing for the next twenty-five seconds — a splash per flip, a script
    /// callback per flip, and nothing in a still image or a final position to
    /// show for it.
    #[test]
    fn a_crate_enters_the_water_once_and_leaves_it_once() {
        let scene = "../../assets/test/splash.loom";
        let (code, out) = run(&args(&[
            "sim",
            scene,
            "--ticks",
            "1800",
            "--assert",
            "events.submerged == 1",
            "--assert",
            "events.surfaced == 1",
            "--assert",
            "status == won",
            // The settled band never reaches `enter`, so the single entry is a
            // property of this sea and this crate rather than a lucky run.
            "--assert",
            "state.wet_max < 0.6",
            "--assert",
            "Sea/Crate.y < 1.5",
            "--assert",
            "Sea/Crate.y > -1.5",
        ]));

        assert_eq!(code, 0, "the crate should go in once and come back out: {out}");
    }

    /// **Phase 4's first two exit criteria, as the implementation order writes
    /// them**: rain stops under an overhang, and a CSG carve lets it back in.
    ///
    /// Both are claims about a *picture* that no picture can settle — the
    /// streaks are stateless GPU noise deliberately outside the simulation, so
    /// what has to be right is the rate the CPU reports. Measured on this
    /// scene, which authors 8 mm/h: **8.0 in the open, 0.0 under the roof, and
    /// 8.0 again under the skylight** — and the skylight is inside the roof's
    /// own footprint, so the only thing letting rain through is one `subtract`
    /// op in the recipe.
    ///
    /// One tick, because rain has no transient to wait out. Its rate is a
    /// function of position and the field, not of anything that accumulates.
    #[test]
    fn rain_stops_under_an_overhang_and_a_carve_lets_it_in() {
        let scene = "../../assets/test/rain_overhang.loom";
        let (code, out) = run(&args(&[
            "sim",
            scene,
            "--ticks",
            "1",
            // Out from under it entirely: the whole authored rate.
            "--assert",
            "rain@-13,3,-13.rate > 7.9",
            // Under the slab: none of it.
            "--assert",
            "rain@0,3,0.rate < 0.05",
            // Under the hole cut through that same slab: all of it back.
            "--assert",
            "rain@0,3,-6.rate > 7.9",
            // And the shelter is the voxel field's own answer, not a rate that
            // happens to come out right — S3's exposure, read directly.
            "--assert",
            "rain@0,3,0.exposure < 0.01",
            "--assert",
            "rain@-13,3,-13.exposure > 0.99",
        ]));

        assert_eq!(code, 0, "{out}");
    }

    /// **Phase 4's third exit criterion: wetness accumulates and dries.**
    ///
    /// Temporal, so it is a simulation assertion and not a screenshot — a still
    /// of a dried surface and a still of one that was never wet are the same
    /// picture. `rain_overhang` rains for sixty seconds and then stops, which is
    /// what makes "afterwards" a moment there is a tick number for.
    ///
    /// The three readings at 150 s are the whole model in one line: **the sheen
    /// is gone, the darkening is still more than half there, and the sheltered
    /// floor never got either.** If the two dried at one rate the second of
    /// those would fail, and drying would be a crossfade back to the dry
    /// material rather than something that looks like drying.
    #[test]
    fn wetness_accumulates_and_dries() {
        let scene = "../../assets/test/rain_overhang.loom";

        // 3600 ticks — sixty seconds, the last instant of the shower.
        let (code, out) = run(&args(&[
            "sim", scene, "--ticks", "3600",
            "--assert", "wetness@-13,3,-13.film > 0.99",
            "--assert", "wetness@-13,3,-13.soak > 0.7",
            // **Gated by exposure, and by S3's exposure alone.** Under the
            // slab nothing wets; under the hole cut through it, everything does.
            "--assert", "wetness@0,3,0.film < 0.01",
            "--assert", "wetness@0,3,-6.film > 0.99",
        ]));
        assert_eq!(code, 0, "it should be soaked after a minute of rain: {out}");

        // 9000 ticks — a hundred and fifty seconds, ninety of them dry.
        let (code, out) = run(&args(&[
            "sim", scene, "--ticks", "9000",
            "--assert", "rain@-13,3,-13.rate == 0",
            "--assert", "wetness@-13,3,-13.film < 0.05",
            "--assert", "wetness@-13,3,-13.soak > 0.4",
        ]));
        assert_eq!(code, 0, "the sheen should go long before the stain: {out}");
    }

    /// **The draw count and the simulation's drop count are one number, and
    /// this is the only place they can be compared.** The vertex shader indexes
    /// `environment.rainDrops[vertexID / 6]` with no bounds test — a device
    /// address read past the end is a hang, not a validation message — and the
    /// compute shader sizes its own bounds check from its own copy. Neither
    /// side sees the other, and the failure of a mismatch looks like a driver
    /// fault rather than an off-by-one.
    ///
    /// The same argument `loom_field::noise` makes for grepping its own Slang
    /// half, and the shape this test had before ADR 0015 for the closed-form
    /// grid it replaced.
    #[test]
    fn the_draw_count_matches_the_shaders_drop_count() {
        let slang = std::fs::read_to_string("../../assets/shaders/include/rain.slang")
            .expect("the shared rain header is readable");
        let decl = slang
            .split("static const uint RAIN_DROPS = ")
            .nth(1)
            .and_then(|rest| rest.split('u').next())
            .expect("include/rain.slang declares RAIN_DROPS");
        let drops: u32 = decl.trim().parse().expect("RAIN_DROPS is an integer");

        assert_eq!(MAX_RAIN_DROPS, drops, "MAX_RAIN_DROPS does not match RAIN_DROPS `{decl}`");
        // The dispatch is 64 wide, and a count that is not a multiple of it is
        // not wrong — the shader discards the tail — but it is a smell worth
        // pinning, because the tail is the one part no scene exercises.
        assert_eq!(drops % 64, 0, "RAIN_DROPS `{decl}` is not a whole number of workgroups");
    }

    /// **The streak layer is told what the SKY is doing, not what the camera
    /// can see.** An eye sealed under the overhang gets the same rate, the same
    /// lean and the same full draw as one out in the open — occlusion is the
    /// per-drop height-field test in `rainVertexMain`, which is right from
    /// anywhere. Scaling the layer by the exposure at the eye is what made the
    /// rain five metres away disappear when you stepped under a roof, and it is
    /// the obvious thing to reach for again.
    #[test]
    fn a_sheltered_camera_does_not_stop_the_rain_outside() {
        let rain = loom_scene::components::Rain { intensity: 8.0, duration: None };
        let wind = loom_field::wind::Wind::new(20.0, 6.0, 1.2, 0.9, 0.45);
        let stamp = |eye: Vec3| {
            let mut env = loom_render::EnvironmentData::default();
            let drops = rain_at_eye(&mut env, Some(&rain), &wind, eye, 1.0);
            (env.rain, drops)
        };

        // `rain_overhang`'s roof is 20 m square at y = 8, over the world origin.
        let (under, under_drops) = stamp(Vec3::new(0.0, 3.0, 0.0));
        let (open, open_drops) = stamp(Vec3::new(-13.0, 3.0, -13.0));

        assert_eq!(under_drops, MAX_RAIN_DROPS, "a sheltered eye skipped the draw");
        assert_eq!(open_drops, MAX_RAIN_DROPS);
        assert!((under[3] - 8.0).abs() < 1e-4, "the rate was sheltered at the eye: {under:?}");
        assert!((open[3] - 8.0).abs() < 1e-4, "{open:?}");
        // And the lean, which was sheltered by the same number: a scene whose
        // streaks stand straight up under cover is the same artifact.
        assert!(
            under[0].abs() + under[2].abs() > 0.1,
            "the lean died under the roof: {under:?}"
        );
    }

    /// **A scene that authors no rain must be exactly as dry as it was.** The
    /// component is new and every scene in the repository predates it; if
    /// `sample_rain` treated a missing component as its `Default` — moderate
    /// rain — then every one of them would quietly start raining.
    #[test]
    fn a_scene_without_rain_reports_no_rain() {
        // `cave` rather than something flat, because exposure is a separate
        // fact from the rate and has to keep working in fair weather: wetness
        // drying and rain audio both read it. Deep in the tunnel it is zero and
        // out on the open ground it is one — in a scene that never rains.
        let scene = "../../assets/test/cave.loom";
        let (code, out) = run(&args(&[
            "sim",
            scene,
            "--ticks",
            "1",
            "--assert",
            "rain@16,7,16.rate == 0",
            "--assert",
            "rain@2,4,2.rate == 0",
            "--assert",
            "rain@16,7,16.exposure < 0.01",
            "--assert",
            "rain@2,4,2.exposure > 0.99",
        ]));

        assert_eq!(code, 0, "{out}");
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
        // **The rule, not a phrase.** This used to match "Interior lights",
        // which was the sentence claiming the useful range is 100-800 — wrong
        // by about 40x, and the reason every interior scene in the library was
        // blown out (ADR 0018). Anchoring on the formula an agent has to apply
        // means the test fails if the doc is emptied *or* if the actionable
        // part of it is dropped again.
        assert!(
            v["properties"]["intensity"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("d^2 / albedo")),
            "doc comment should reach the agent"
        );
    }

    /// **The `Displace` recipe has to reach an agent, and `ops` is the only
    /// door.** `VoxelVolume.ops` is `Vec<serde_json::Value>`, so nothing in
    /// `loom_voxel::Displace`'s own doc comment — where the measured tables
    /// live — is reflected anywhere. Without the recipe restated on `ops`, an
    /// agent authoring a rock has exactly what it had before this work: four
    /// numbers and a render.
    ///
    /// Anchored on the formula rather than a phrase, for the reason the `Light`
    /// test above records: the test must fail if the *actionable* part is
    /// dropped, not only if the paragraph is deleted.
    #[test]
    fn describe_carries_the_displacement_recipe() {
        let (code, out) = run(&args(&["describe", "VoxelVolume"]));

        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let ops = v["properties"]["ops"]["description"].as_str().unwrap_or_default();
        assert!(ops.contains("0.20 * R"), "the amplitude rule should reach the agent: {ops}");
        assert!(ops.contains("0.6 / R"), "the frequency rule should reach the agent: {ops}");
        assert!(
            ops.contains("floor(log2(2 * amplitude / voxel_size)) + 1"),
            "the octave cap is the expensive one to get wrong: {ops}"
        );
    }

    /// **`loom describe VoxelVolume` is the whole schema for an op**, because
    /// `ops` is `Vec<serde_json::Value>` and JSON Schema says only "array".
    ///
    /// Authoring a volume from the schema alone was impossible until this
    /// table existed: nothing named the `kind` values, nothing said which
    /// fields each takes, and nothing said whether the volume's near corner is
    /// at the origin or centred on it. Every voxel scene in the library was
    /// authored by copying another one.
    ///
    /// Anchored on the field names rather than the prose, so the test fails if
    /// the actionable part is dropped and not merely if the paragraph is
    /// reworded. `elongate` is the one worth naming twice: it is on `sphere`
    /// alone, and it adds to `radius` rather than replacing it.
    #[test]
    fn describe_carries_the_op_schema() {
        let (code, out) = run(&args(&["describe", "VoxelVolume"]));

        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let ops = v["properties"]["ops"]["description"].as_str().unwrap_or_default();
        for needle in [
            "half_extents",
            "yaw_degrees",
            "elongate",
            "base_y",
            "world_scale",
            "chunks * 32 * voxel_size",
            "radius + elongate[i]",
        ] {
            assert!(ops.contains(needle), "an agent cannot author `{needle}` blind: {ops}");
        }
    }

    /// `--shape` reports the number `Displace` is judged by, and it must carry
    /// its own comparison rule — the metric is resolution-dependent, so a row
    /// without a `voxel_size` beside it is not comparable to anything.
    #[test]
    fn measure_shape_reports_curvature_and_its_own_rule() {
        let (code, out) = run(&args(&["measure", "../../assets/meshes/rock_boulder_a.obj", "--shape"]));

        assert_eq!(code, 0, "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert!(
            v["rule"].as_str().is_some_and(|r| r.contains("equal voxel_size")),
            "the rule travels with the number, as it does for flicker: {out}"
        );
        // A scanned rock is fractured stone: concave over a wide spread. The
        // thresholds are deliberately loose — this checks the wiring, and
        // `rockcalib` is where the calibration lives.
        let rock = &v["shapes"][0];
        assert!(rock["concave"].as_f64().is_some_and(|c| c > 0.3), "{out}");
        assert!(rock["spread"].as_f64().is_some_and(|s| s > 1.0), "{out}");
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

    /// **A mistyped flag must not look like a success.** Unknown flags were
    /// ignored, so `--frame 3` (singular) rendered the default frame and
    /// reported ok, and `--dry-run` misspelled wrote the file for real. An
    /// agent has no way to notice: the output says it did what was asked.
    #[test]
    fn an_unknown_flag_is_refused() {
        let (code, out) = run(&args(&[
            "validate",
            "../../assets/test/office.loom",
            "--frame",
            "3",
        ]));

        assert_eq!(code, 2, "wrong invocation is exit 2: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "unknown_flag");
        assert_eq!(v["value"], "--frame");
    }

    /// A near-miss on a flag that *does* exist for another subcommand is the
    /// likeliest mistake, so the rejection lists what this one takes.
    #[test]
    fn the_rejection_lists_the_flags_that_subcommand_accepts() {
        let (code, out) = run(&args(&["sim", "../../assets/test/tower.loom", "--out", "x.png"]));

        assert_eq!(code, 2);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("--ticks"), "should list sim's flags: {hint}");
    }

    /// Every flag the code actually reads must be accepted, or the allowlist
    /// has turned into a second bug.
    #[test]
    fn every_documented_flag_is_accepted() {
        let (code, out) = run(&args(&[
            "measure",
            "../../assets/test/blockout.loom",
            "--node",
            "Room/Floor",
        ]));

        assert_eq!(code, 0, "--node is a real measure flag: {out}");
    }

    /// The water query at one instant, as JSON.
    fn water_at(scene: &str, spec: &[&str]) -> (u8, serde_json::Value) {
        let mut argv = vec!["water", scene];
        argv.extend_from_slice(spec);
        let (code, out) = run(&args(&argv));
        (code, serde_json::from_str(&out).unwrap_or_else(|_| serde_json::json!({ "raw": out })))
    }

    /// **The query has to agree with the engine, or it is worth nothing as a
    /// check on it.** Not "the numbers look plausible" — the same call, made
    /// the same way, has to give the same float. This is the test that fails
    /// the day someone gives `loom water` its own sampling path.
    #[test]
    fn the_reported_surface_is_the_surface_the_engine_samples() {
        let path = "../../assets/test/ocean.loom";
        let (code, v) = water_at(path, &["--at", "3.5,-9.0", "--sim", "90"]);
        assert_eq!(code, 0, "ocean has water: {v}");

        // The same route `add_water` and `submerge_eye` take.
        let scene = Scene::parse(&std::fs::read_to_string(path).unwrap()).expect("valid scene");
        let world = World::from_scene(&scene);
        let wind = crate::weather::wind_of_world(&world);
        let body = crate::weather::water_of(&world, &wind).expect("ocean has water");
        let expected = loom_water::sample_water(&body, [3.5, -9.0], 90.0 / 60.0, -1.0e9, [0.0; 3]);

        assert_eq!(v["surface"]["height"].as_f64().unwrap() as f32, expected.height);
        assert_eq!(v["surface"]["normal"][1].as_f64().unwrap() as f32, expected.normal[1]);
        assert_eq!(v["seconds"].as_f64().unwrap() as f32, 1.5);
    }

    /// **Waves move, so an instant is part of the question.** A query that
    /// reported one number whatever the clock said would be a trap rather than
    /// a measurement — and t = 0 is the one instant where every wave in the set
    /// shares a phase, which is why it is never implicit in the output.
    #[test]
    fn the_instant_is_part_of_the_question() {
        let ocean = "../../assets/test/ocean.loom";
        let (_, still) = water_at(ocean, &["--at", "3.5,-9.0"]);
        let (_, later) = water_at(ocean, &["--at", "3.5,-9.0", "--sim", "90"]);

        assert_eq!(still["ticks"], 0, "t = 0 is stated, not implied: {still}");
        assert!(
            (still["surface"]["height"].as_f64().unwrap()
                - later["surface"]["height"].as_f64().unwrap())
            .abs()
                > 1e-3,
            "the sea did not move between t = 0 and t = 1.5 s: {still} / {later}"
        );

        // And a `--sim` that could not be read is refused rather than silently
        // answering for t = 0, which is the trap itself wearing a disguise.
        let (code, v) = water_at(ocean, &["--at", "0,0", "--sim", "later"]);
        assert_eq!(code, 2, "an unreadable --sim is a failed invocation: {v}");
    }

    /// **Provenance is the question an author actually has.** `ocean` writes
    /// its seven waves down; a scene with a `WaterBody` and no wave list gets
    /// sixteen from the wind spectrum, and the difference is invisible in every
    /// other output the engine produces.
    ///
    /// The count is asserted because it is what separates the two sources, not
    /// because seven is interesting — it moved from five when the scene grew a
    /// 3 m and a 2 m wave, and it must keep being the scene's real count or the
    /// assertion stops distinguishing anything.
    #[test]
    fn the_wave_set_says_where_it_came_from() {
        let (_, authored) = water_at("../../assets/test/ocean.loom", &["--at", "0,0"]);
        assert_eq!(authored["waves"]["source"], "authored");
        assert_eq!(authored["waves"]["count"], 7);
        assert!(authored["waves"]["wind_speed_10m"].is_null(), "{authored}");

        let dir = std::env::temp_dir().join("loom_water_derived");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("sea.loom");
        std::fs::write(
            &scene,
            "[scene]\nformat = 1\nid = \"7d1e4c0a-2b6f-4a58-9c31-5e8b0f2a7d64\"\n\n\
             [[node]]\nname = \"Sea\"\n\n  [node.components.Wind]\n  speed = 11.0\n\n  \
             [node.components.WaterBody]\n  surface_height = 0.0\n",
        )
        .expect("write scene");
        let (code, derived) = water_at(scene.to_str().unwrap(), &["--at", "0,0"]);

        assert_eq!(code, 0, "{derived}");
        assert_eq!(derived["waves"]["source"], "wind");
        assert_eq!(derived["waves"]["count"], 16);
        assert!(
            derived["waves"]["wind_speed_10m"].as_f64().unwrap() > 1.0,
            "a derived sea has to name the wind it came from: {derived}"
        );
        assert!(
            derived["waves"]["significant_height"].as_f64().unwrap() > 0.5,
            "an 11 m/s sea is not flat: {derived}"
        );
    }

    /// The river is the only scene with a bed under the water and a current
    /// over it, and both come from the ground rather than from the `WaterBody`.
    #[test]
    fn the_river_reports_its_bed_and_its_current() {
        let (code, v) = water_at("../../assets/test/river.loom", &["--at", "24.0,24.0"]);

        assert_eq!(code, 0, "{v}");
        assert_eq!(v["kind"], "river");
        // The channel bed is well below the 6.5 m surface, and in it.
        let depth = v["bed"]["depth"].as_f64().expect("in the channel, over ground");
        assert!(depth > 1.0, "the middle of the channel is not a puddle: {v}");
        // Downstream is +X, and the current is what carries the crate.
        let flow_x = v["velocity"]["flow"][0].as_f64().unwrap();
        assert!(flow_x > 0.1, "the river does not run downstream: {v}");
        // Total = current + orbital, which is one velocity and not two.
        let total_x = v["velocity"]["total"][0].as_f64().unwrap();
        let orbital_x = v["velocity"]["orbital"][0].as_f64().unwrap();
        assert!((total_x - (flow_x + orbital_x)).abs() < 1e-6, "{v}");

        // Off the terrain grid the water is bottomless, which is information
        // rather than an error — and reported as null rather than as −10⁹.
        let (code, far) = water_at("../../assets/test/river.loom", &["--at", "9000,9000"]);
        assert_eq!(code, 0, "outside the grid is a legitimate answer: {far}");
        assert!(far["bed"]["depth"].is_null(), "{far}");
        assert!(far["velocity"]["flow"][0].as_f64().unwrap().abs() < 1e-9, "{far}");
    }

    /// **A dry scene is a failed query, not a measurement.** It exits 1 with a
    /// named error for the same reason `measure --node` does on a node with no
    /// geometry: an agent that asks what the water is doing and gets exit 0 has
    /// been told there is an answer.
    #[test]
    fn a_scene_with_no_water_is_an_error_that_names_itself() {
        let (code, v) = water_at("../../assets/test/tower.loom", &["--at", "0,0"]);

        assert_eq!(code, 1, "{v}");
        assert_eq!(v["error"], "no_water");
        assert!(v["hint"].as_str().unwrap().contains("WaterBody"), "{v}");
    }

    /// The submersion answer is `submersion_at`'s — the same call that decides
    /// a swimmer's muffled ears and the underwater tint — and it is null rather
    /// than false when `--at` named no height, because it was not asked.
    #[test]
    fn a_height_turns_the_query_into_a_submersion_test() {
        let ocean = "../../assets/test/ocean.loom";
        let (_, flat) = water_at(ocean, &["--at", "0,0"]);
        assert!(flat["submerged"].is_null(), "not asked is not `false`: {flat}");

        let (_, under) = water_at(ocean, &["--at", "0,-4,0"]);
        let (_, over) = water_at(ocean, &["--at", "0,4,0"]);
        assert_eq!(under["submerged"], true, "{under}");
        assert_eq!(over["submerged"], false, "{over}");
        assert_eq!(under["y"], -4.0, "the height is echoed back: {under}");
    }

    /// A component that cannot be read is a point somewhere else answered
    /// confidently, so `--at` is strict where `parse_vec2` is not.
    #[test]
    fn a_malformed_point_is_refused_rather_than_repaired() {
        let (code, v) = water_at("../../assets/test/ocean.loom", &["--at", "4,oops,9"]);

        assert_eq!(code, 2, "should not read as the point (4, 9): {v}");
        assert_eq!(v["error"], "missing_argument");
    }

    /// An agent restricted to the tool's own output has to be able to find the
    /// subcommands. The usage text listed two of ten and no flags at all.
    #[test]
    fn usage_lists_every_subcommand() {
        for command in [
            "validate", "describe", "render", "sim", "scene", "place", "measure", "terrain",
            "explode", "run", "water",
        ] {
            assert!(USAGE.contains(command), "usage should mention `{command}`");
        }
    }

    /// **A missing asset must be reported, even though the render degrades.**
    /// `docs/format/README.md` §6 makes `unresolved_alias` a normative M1 error
    /// code; the renderer substitutes mesh 0 — a unit box — and says nothing.
    /// Both behaviours are right for their own context: a broken asset should
    /// not stop a render (design doc §2.6, degrade rather than crash), but it
    /// must not be invisible to the agent that wrote the alias either. So
    /// `validate` reports it and `render` keeps drawing a box.
    #[test]
    fn validate_reports_an_alias_that_resolves_to_nothing() {
        let dir = std::env::temp_dir().join("loom_alias_check");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("a.loom");
        std::fs::write(
            &scene,
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f\"\n\n\
             [[node]]\nname = \"Root\"\n\n  [node.components.MeshRenderer]\n  \
             mesh = { asset = \"crate_wooden\" }\n",
        )
        .expect("write");

        let (code, out) = run(&args(&["validate", scene.to_str().unwrap()]));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(code, 1, "an unresolvable alias is invalid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let e = &v["errors"][0];
        assert_eq!(e["error"], "unresolved_alias");
        assert_eq!(e["value"], "crate_wooden");
        assert_eq!(e["node"], "Root");
    }

    /// **The prefab defect in the voxel path.** An op whose `kind` the schema
    /// does not know used to be dropped by a `filter_map(…ok())`: the volume
    /// baked short, drew a plausible surface, validated clean, and baked
    /// *faster* for it. The mutation is one word in a real scene.
    #[test]
    fn validate_rejects_a_voxel_op_it_does_not_recognise() {
        let good = std::fs::read_to_string("../../assets/test/rain_gantry.loom").unwrap();
        let dir = std::env::temp_dir().join("loom_voxel_op_check");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("a.loom");
        // The mesh aliases are `asset = "box"`; only the op line matches this.
        let bad = good.replace("kind = \"box\"", "kind = \"swirl\"");
        assert_ne!(bad, good, "the mutation did not apply, so this proves nothing");
        std::fs::write(&scene, &bad).expect("write");

        let (code, out) = run(&args(&["validate", scene.to_str().unwrap()]));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(code, 1, "an op nothing can parse is invalid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let e = &v["errors"][0];
        assert_eq!(e["error"], "invalid_voxel_op");
        assert_eq!(e["node"], "Weather/Apron");
        assert!(
            e["constraint"].as_str().unwrap_or_default().contains("swirl"),
            "the offending kind has to be named or the agent cannot fix it: {out}"
        );
    }

    /// **The same defect one level down: a typo'd *field* inside a recognised
    /// kind.** `000daf8` made an unknown `kind` an error and left this open —
    /// `yaw = 30` for `yaw_degrees` on a `box` loaded clean and drew an
    /// unrotated wall, and so did `elongate` on a box, `round` on a sphere and
    /// `displace` on a terrain, which are the three most likely mistakes given
    /// that each of the new transforms is on exactly one kind.
    ///
    /// `deny_unknown_fields` on `VoxelOp` and on `Displace` closes it, and the
    /// error serde produces **enumerates the fields that kind does take** —
    /// which is what makes the untyped `ops` array discoverable at all.
    #[test]
    fn validate_rejects_a_field_the_op_does_not_have() {
        let good = std::fs::read_to_string("../../assets/test/vale.loom").unwrap();
        let dir = std::env::temp_dir().join("loom_voxel_field_check");
        std::fs::create_dir_all(&dir).expect("temp dir");
        // The recipe is named relative to the scene, so it has to come along.
        std::fs::copy("../../assets/test/vale.toml", dir.join("vale.toml")).expect("recipe");
        let scene = dir.join("a.loom");

        // Each is a field that exists on some other kind, which is the mistake
        // the schema invites: `yaw_degrees` and `round` are box-only, and
        // `elongate` is sphere-only. The expected substring is a *sibling*
        // field of the op being mutated, so it can only come from the
        // enumerated list serde emits.
        for (mutation, replacement, expected) in [
            ("  yaw_degrees = 24.0", "  yaw = 24.0", "yaw_degrees"),
            ("  radius = 4.5", "  radius = 4.5\n  round = 1.0", "elongate"),
            ("  round = 0.4", "  elongate = [1.0, 0.0, 0.0]", "yaw_degrees"),
        ] {
            let bad = good.replacen(mutation, replacement, 1);
            assert_ne!(bad, good, "the mutation did not apply, so this proves nothing");
            std::fs::write(&scene, &bad).expect("write");

            let (code, out) = run(&args(&["validate", scene.to_str().unwrap()]));
            assert_eq!(code, 1, "`{replacement}` is not a field of that op: {out}");
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let e = v["errors"][0]["constraint"].as_str().unwrap_or_default().to_owned();
            assert_eq!(v["errors"][0]["error"], "invalid_voxel_op");
            assert!(
                e.contains(expected),
                "the message must list the fields that kind does take, or the agent is \
                 guessing again: {e}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`intersect` as op 0 bakes an empty volume and said nothing.** The
    /// accumulator seeds at `f32::MAX` and intersect is `max`, so the first op
    /// keeps the seed; a later union then fills in, which is why the symptom is
    /// "the landform vanished and the props are still floating there" rather
    /// than an obviously empty scene. Measured before the fix: a single
    /// intersect sphere printed `voxel produced no surface` on stderr, exited
    /// **0**, and `loom validate` said `"ok": true`.
    #[test]
    fn validate_rejects_an_intersect_with_nothing_to_intersect() {
        let dir = std::env::temp_dir().join("loom_voxel_intersect_check");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("a.loom");
        std::fs::write(
            &scene,
            "[scene]\nformat = 1\nid = \"0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f\"\n\n\
             [[node]]\nname = \"Root\"\n\n  [node.components.VoxelVolume]\n  \
             voxel_size = 0.5\n  chunks = [2, 2, 2]\n\n  \
             [[node.components.VoxelVolume.ops]]\n  kind = \"sphere\"\n  \
             center = [16.0, 16.0, 16.0]\n  radius = 8.0\n  mode = \"intersect\"\n",
        )
        .expect("write");

        let (code, out) = run(&args(&["validate", scene.to_str().unwrap()]));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(code, 1, "an intersect with nothing to intersect is invalid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["errors"][0]["error"], "invalid_voxel_op");
        assert!(
            v["errors"][0]["constraint"]
                .as_str()
                .unwrap_or_default()
                .contains("op 0"),
            "which op is wrong is the whole message: {out}"
        );
    }

    /// **The coverage half of the terrain coordinate trap reached `validate`
    /// as `"ok": true`.** It was checked in `build_volume` alone, so a rect the
    /// right size in the wrong place passed the hard gate; `loom sim` then
    /// printed one line to stderr, exited **0**, and left the ground the player
    /// stands on out of the collision world entirely. Both halves belong in
    /// `parse_ops`, which is the funnel `validate` and every bake share.
    #[test]
    fn validate_rejects_a_terrain_rect_that_misses_the_volume() {
        let good = std::fs::read_to_string("../../assets/test/vale.loom").unwrap();
        let dir = std::env::temp_dir().join("loom_voxel_rect_check");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::copy("../../assets/test/vale.toml", dir.join("vale.toml")).expect("recipe");
        let scene = dir.join("a.loom");
        // Still 128 m across, so the size check passes and only coverage fails.
        let bad = good.replacen(
            "rect = [0.0, 0.0, 128.0, 128.0]",
            "rect = [20.0, 20.0, 148.0, 148.0]",
            1,
        );
        assert_ne!(bad, good, "the mutation did not apply, so this proves nothing");
        std::fs::write(&scene, &bad).expect("write");

        let (code, out) = run(&args(&["validate", scene.to_str().unwrap()]));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(code, 1, "a rect that misses the volume is invalid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["errors"][0]["error"], "invalid_voxel_op");
        assert!(
            v["errors"][0]["constraint"]
                .as_str()
                .unwrap_or_default()
                .contains("does not cover"),
            "{out}"
        );
    }

    /// Primitives resolve procedurally and need no `[[asset]]` entry, and the
    /// real fixtures declare their assets. Neither may start failing.
    #[test]
    fn validate_still_accepts_primitives_and_declared_assets() {
        for fixture in ["office.loom", "blockout.loom", "tower.loom", "primitives.loom"] {
            let (code, out) = run(&args(&["validate", &format!("../../assets/test/{fixture}")]));
            assert_eq!(code, 0, "{fixture} should be valid: {out}");
        }
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

    /// A slab with a dome on it and a bowl scooped out of it — the shape
    /// `assets/test/grass_slope.loom` authors, in four ops.
    fn hillside() -> loom_voxel::Volume {
        let mut volume = loom_voxel::Volume::new([4, 3, 4], 0.25);
        volume.bake(&[
            loom_voxel::VoxelOp::Box {
                center: [16.0, 3.0, 16.0],
                half_extents: [14.0, 3.0, 14.0],
                mode: loom_voxel::CsgMode::Union,
                displace: None,
                yaw_degrees: 0.0,
                round: 0.0,
            },
            loom_voxel::VoxelOp::Sphere {
                center: [9.0, 3.5, 16.0],
                radius: 6.5,
                mode: loom_voxel::CsgMode::Union,
                displace: None,
                elongate: [0.0; 3],
            },
            loom_voxel::VoxelOp::Sphere {
                center: [19.5, 11.0, 22.0],
                radius: 6.5,
                mode: loom_voxel::CsgMode::Subtract,
                displace: None,
                elongate: [0.0; 3],
            },
        ]);
        volume
    }

    /// **The ground closure actually reads the terrain.** P2's exit criterion
    /// is that grass thins on steep ground and thickens in gullies *without an
    /// authored mask*, and `loom_grass` has implemented that rule from the
    /// start — against a flat constant `Ground`, so none of it ever fired.
    ///
    /// **What this test does and does not cover.** It builds a `GroundGrid`
    /// directly, so it proves the grid reads the terrain; it does *not* prove
    /// the grid is wired into `grass_blades`. An earlier version of this
    /// comment claimed these numbers collapse to the flat case if the closure
    /// regresses to `Ground::default()`, and that was simply untrue — the
    /// mutation was run and this test passed. `grass_on_a_voxel_scene_follows_
    /// the_terrain` is the one that fails, and it is the only test guarding the
    /// seam. Said plainly here because a test that documents a guarantee it
    /// does not provide is worse than no comment: it stops the next person
    /// looking for the missing one.
    #[test]
    fn coverage_follows_the_voxel_terrain() {
        let volume = hillside();
        let grid = GroundGrid::bake(&volume, [0.0; 3], [16.0, 0.0, 16.0], [13.0, 13.0]);
        let rules = loom_grass::Rules::default();
        let cover = |x: f32, z: f32| loom_grass::coverage(&rules, &grid.at(x, z), [x, z]);

        // The slab top, well away from both features.
        let flat = cover(26.0, 10.0);
        // Half way up the dome's flank, past 35° — inside the fade band.
        let flank = cover(13.0, 16.0);
        // The floor of the hollow: level, and concave for a metre around.
        let hollow = cover(19.5, 22.0);

        assert!(flat > 0.5, "the flat slab grows nothing: {flat}");
        assert!(
            flank < flat * 0.75,
            "the dome's flank ({flank}) is as lush as level ground ({flat}) — \
             the ground closure is not reading the slope"
        );
        assert!(
            hollow > flat * 1.25,
            "the hollow ({hollow}) is no lusher than level ground ({flat}) — \
             the ground closure is not reading the concavity"
        );
    }

    /// Blades sit **on** the surface, at three heights that are all different.
    /// A blade whose Y comes from its node's transform is a blade floating over
    /// a hill or buried in it.
    #[test]
    fn blade_height_comes_from_the_surface_under_it() {
        let volume = hillside();
        let grid = GroundGrid::bake(&volume, [0.0; 3], [16.0, 0.0, 16.0], [13.0, 13.0]);

        // The slab top is y = 6, the dome peaks near y = 10, the hollow bottoms
        // out around y = 4.5 — measured from the ops, not from the code.
        assert!((grid.at(26.0, 10.0).height - 6.0).abs() < 0.05);
        assert!((grid.at(9.0, 16.0).height - 10.0).abs() < 0.1);
        assert!((grid.at(19.5, 22.0).height - 4.5).abs() < 0.1);
    }

    /// **No floating blades where the ground was destroyed.** A shaft punched
    /// clean through the slab leaves a column with no surface at all, and that
    /// has to grow nothing rather than growing grass at the old height. The
    /// bake reads the volume as it is now, so a carve is reflected by
    /// re-baking — the same property `loom_voxel::exposure` has.
    #[test]
    fn a_hole_through_the_terrain_grows_no_grass() {
        let mut volume = hillside();
        let grid = GroundGrid::bake(&volume, [0.0; 3], [16.0, 0.0, 16.0], [13.0, 13.0]);
        let rules = loom_grass::Rules::default();
        let before = loom_grass::coverage(&rules, &grid.at(26.0, 10.0), [26.0, 10.0]);
        assert!(before > 0.5, "nothing grew there to begin with: {before}");

        volume.edit(&loom_voxel::VoxelOp::Box {
            center: [26.0, 3.0, 10.0],
            half_extents: [2.0, 5.0, 2.0],
            mode: loom_voxel::CsgMode::Subtract,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        });
        let grid = GroundGrid::bake(&volume, [0.0; 3], [16.0, 0.0, 16.0], [13.0, 13.0]);
        let after = loom_grass::coverage(&rules, &grid.at(26.0, 10.0), [26.0, 10.0]);

        assert_eq!(after, 0.0, "grass is floating over the hole");
    }

    /// **The wiring, end to end**, on the scene authored for it. The tests
    /// above check the grid; this one checks that `grass_blades` actually hands
    /// it to `loom_grass`. Every assertion here fails if the ground closure goes
    /// back to a flat constant: the heights collapse to one value and the
    /// dome's flank grows as much grass as the level slab.
    #[test]
    fn grass_on_a_voxel_scene_follows_the_terrain() {
        let src = std::fs::read_to_string("../../assets/test/grass_slope.loom").expect("scene");
        let scene = Scene::parse(&src).expect("grass_slope parses");

        let blades = grass_blades(&scene);
        assert!(!blades.is_empty(), "no blades, so this proves nothing");

        // The slab top is y = 6 and the dome peaks near y = 10: blades reach
        // both, because their Y comes from the surface and not from the node.
        let high = blades.iter().map(|b| b.position[1]).fold(f32::MIN, f32::max);
        let low = blades.iter().map(|b| b.position[1]).fold(f32::MAX, f32::min);
        assert!(high > 9.0, "nothing grew on the dome: highest blade is {high}");
        assert!(low < 5.5, "nothing grew in the hollow: lowest blade is {low}");

        // **A ladder up the dome, not one point on it.** The dome is a sphere
        // of radius 6.5 centred at (9, 16) with its equator buried, so at a
        // horizontal distance `r` from its axis the surface normal's Y is
        // `sqrt(6.5² - r²) / 6.5` — every slope from flat to vertical, indexed
        // by a number the test can compute. That makes the assertion the real
        // invariant, "steeper grows less", rather than a claim about one
        // sample's absolute value.
        //
        // The single-point version this replaces compared r = 4 (n.y = 0.79,
        // about 38°) against level ground and demanded it be a quarter barer.
        // That is not a property of the mechanism, it is a property of where
        // `slope_cutoff` happens to sit — and it broke the moment the scene
        // authored a cutoff that keeps a 38° hillside grassy, which is what a
        // real hillside does. A ladder holds for any cutoff.
        let density = |cx: f32, cz: f32| {
            blades
                .iter()
                .filter(|b| (b.position[0] - cx).abs() < 0.6 && (b.position[2] - cz).abs() < 0.6)
                .count()
        };
        // Along +X from the dome's axis, so the offset from x = 9 is `r`. The
        // samples stay inside r < 6, which is where the dome meets the slab —
        // a box straddling that rim would mix bare flank with lush level
        // ground and read as *more* grass on the steeper sample, which is
        // exactly the false negative an earlier version of this test produced.
        //
        //   r = 2.0   n.y 0.97   18°   well inside
        //   r = 4.0   n.y 0.79   38°   inside the fade band
        //   r = 5.2   n.y 0.60   53°   past the cutoff
        let gentle = density(11.0, 16.0);
        let mid = density(13.0, 16.0);
        let steep = density(14.2, 16.0);
        let flat = density(26.0, 10.0);

        assert!(flat > 60, "the level slab is bare: {flat} blades in 1.44 m²");
        assert!(gentle > flat / 2, "nothing grew on the dome's gentle cap: {gentle}");
        assert!(
            gentle >= mid && mid >= steep,
            "grass did not thin monotonically up the dome's flank: \
             {gentle} (18°), {mid} (38°), {steep} (53°)"
        );
        assert!(
            steep * 4 < gentle,
            "past the slope cutoff the dome is still lush: {steep} at 53° \
             against {gentle} at 18° — the slope term is not firing"
        );
    }

    /// Grass over a scene with no voxel volume is unchanged — `meadow.loom`
    /// keeps its reference image, and a flat field stays flat.
    #[test]
    fn a_scene_without_terrain_keeps_the_flat_ground() {
        let src = std::fs::read_to_string("../../assets/test/meadow.loom").expect("meadow");
        let scene = Scene::parse(&src).expect("meadow parses");

        let blades = grass_blades(&scene);

        assert!(!blades.is_empty(), "no blades, so this proves nothing");
        assert!(
            blades.iter().all(|b| b.position[1] == 0.0),
            "a scene with no terrain moved its blades off the plane"
        );
    }

    /// The guard that keeps the viewer from re-marching the voxel SDF on every
    /// frame of a gizmo drag. Both halves matter: too eager and the editor
    /// stalls for seconds, too lazy and edits to a grass field never show up.
    #[test]
    fn the_grass_key_tracks_grass_and_nothing_else() {
        let src = std::fs::read_to_string("../../assets/test/meadow.loom").expect("meadow");
        let scene = Scene::parse(&src).expect("meadow parses");
        let key = grass_key(&scene);
        assert!(!key.is_empty(), "meadow has a grass field");

        // A scene with no grass never places any, so it needs no key.
        let bare = Scene::parse("[scene]\nformat = 1\n\n[[node]]\nname = \"Root\"\n")
            .expect("bare scene parses");
        assert_eq!(grass_key(&bare), "");

        // Dragging the stone — which is what a gizmo drag looks like — must not
        // re-place the field.
        let dragged = src.replace("pos = [2.4, 0.22, -1.6]", "pos = [3.9, 0.22, -1.6]");
        assert_ne!(dragged, src, "the stone's transform moved in the file");
        let dragged = Scene::parse(&dragged).expect("the dragged scene parses");
        assert_eq!(
            grass_key(&dragged),
            key,
            "moving an unrelated node re-places the grass, so a drag stalls"
        );

        // Editing the field itself must.
        let denser = src.replace("density = 140.0", "density = 260.0");
        let denser = Scene::parse(&denser).expect("the denser scene parses");
        assert_ne!(
            grass_key(&denser),
            key,
            "an edited grass field kept its key, so the edit never shows"
        );
    }
}
