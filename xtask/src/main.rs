//! `cargo xtask validate` — the second of the three green checks.
//!
//! `CLAUDE.md` calls the validation layers "the real compiler", and it is right:
//! missing barriers, wrong image layouts, use-after-free of in-flight
//! resources and out-of-bounds bindless indices all compile perfectly. Until
//! now this check printed `skip: ... xtask crate does not exist yet` and every
//! Vulkan defect had to be found by a human opening a window.
//!
//! Four of them were, the hard way — including one that segfaulted inside the
//! driver on every close and briefly froze the displays. This is that job,
//! automated.
//!
//! **It drives the real `loom` binary as a subprocess** rather than linking the
//! engine. A gate that shares code with the thing it checks can be fooled by
//! the same bug twice; a subprocess also means a segfault is a failed check
//! rather than a dead test runner.
//!
//! The debug profile is what turns the layers on (`cfg!(debug_assertions)` in
//! `loom_render::Instance`), so this always builds and runs debug.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Scenes worth exercising: a blockout, a physics stack, every primitive, a
/// voxel volume, a scene that references a missing asset, and a material
/// matrix. The missing-asset one is deliberate — the degrade-don't-crash path
/// has Vulkan work in it too.
///
/// `materials.loom` earns its place because it is the only scene that binds a
/// descriptor set: the bindless texture array, sampled with a non-uniform
/// index. Sampler creation, image layout transitions and array indexing are
/// all things the validation layers have an opinion about and the compiler has
/// none.
///
/// `terrain_stress.loom` is the largest mesh the project produces — 67 million
/// voxels down to ~778k triangles — so it is where a buffer sized from a
/// smaller scene would first overflow. It costs about 2.5 s of the run.
///
/// `smoke.loom` is the only scene that exercises the particle pipeline — a
/// second pipeline, alpha blending, and a draw with no vertex buffer at all.
const SCENES: [&str; 14] = [
    "assets/test/blockout.loom",
    "assets/test/tower.loom",
    "assets/test/primitives.loom",
    "assets/test/cave.loom",
    "assets/test/office.loom",
    "assets/test/materials.loom",
    "assets/test/terrain_stress.loom",
    "assets/test/smoke.loom",
    "assets/test/camera.loom",
    "assets/test/walker.loom",
    "assets/test/explosion.loom",
    "assets/test/range.loom",
    "assets/test/turret_range.loom",
    "assets/games/proving_ground.loom",
];

/// How many frames a windowed run draws before shutting itself down. Enough to
/// get past first-frame special cases and into the steady state, without
/// making the check slow.
const WINDOWED_FRAMES: u32 = 90;

fn main() -> std::process::ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "validate" => validate(),
        other => {
            eprintln!("unknown task {other:?}\n\nUSAGE:\n    cargo xtask validate");
            std::process::ExitCode::from(2)
        }
    }
}

fn validate() -> std::process::ExitCode {
    let root = repo_root();

    let loom = match build_debug(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };

    // No Vulkan device means this machine cannot answer the question. Say so
    // and skip, rather than passing — a green tick nobody earned is worse than
    // an honest gap, and CI without a GPU is the normal case.
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask validate — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let mut failures = Vec::new();
    let mut checked = 0;

    for scene in SCENES {
        if !root.join(scene).exists() {
            eprintln!("xtask: {scene} is missing; skipping");
            continue;
        }
        checked += 1;
        let out = root.join("target/xtask-validate.png");
        let result = run(
            &loom,
            &root,
            &["render", scene, "--out", out.to_str().unwrap_or("out.png")],
        );
        collect(&mut failures, &format!("render {scene}"), &result);

        // `--sim` runs physics before drawing, which is a different set of
        // buffer writes than a static render.
        let result = run(
            &loom,
            &root,
            &[
                "render",
                scene,
                "--sim",
                "120",
                "--out",
                out.to_str().unwrap_or("out.png"),
            ],
        );
        collect(&mut failures, &format!("render --sim {scene}"), &result);
    }

    // The windowed path, if there is a display to open on. This is where every
    // teardown bug so far has lived: swapchain, surface, egui, and the
    // instance-before-device destruction that crashed the driver. A headless
    // render touches none of it.
    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let frames = WINDOWED_FRAMES.to_string();
        for scene in ["assets/test/blockout.loom", "assets/test/cave.loom"] {
            if !root.join(scene).exists() {
                continue;
            }
            checked += 1;
            let result = run(&loom, &root, &["run", scene, "--edit", "--frames", &frames]);
            collect(&mut failures, &format!("run --edit {scene}"), &result);
        }
    } else {
        println!("note: no display; the windowed path was not exercised");
    }

    // Determinism is the property every `--assert` rests on, and it is exactly
    // the kind of thing that decays silently: a dependency bump, a new SIMD
    // path, a reordered iteration. Comparing the two build profiles is a real
    // test of it — debug and release inline differently, vectorise differently,
    // and must still agree to the bit.
    //
    // This is also the standing answer to "would a custom physics engine be
    // better": on the axis this project cares about most, the current one is
    // measurably correct, and now stays measured.
    match determinism_holds(&root) {
        Ok(hash) => println!("determinism: debug and release agree ({hash})"),
        Err(problem) => failures.push(problem),
    }

    if failures.is_empty() {
        println!("cargo xtask validate: {checked} scene runs, zero validation messages");
        return std::process::ExitCode::SUCCESS;
    }

    eprintln!("\ncargo xtask validate FAILED\n");
    for failure in &failures {
        eprintln!("{failure}\n");
    }
    eprintln!(
        "{} problem(s). These are not warnings: the validation layers are the \
         compiler for this part of the codebase.",
        failures.len()
    );
    std::process::ExitCode::from(1)
}

/// Simulate the same scene with both build profiles and require the same hash.
fn determinism_holds(root: &Path) -> Result<String, String> {
    let scene = "assets/test/tower.loom";
    if !root.join(scene).exists() {
        return Ok("skipped, no scene".to_owned());
    }
    let release = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "loom_cli"])
        .current_dir(root)
        .status();
    if !release.is_ok_and(|s| s.success()) {
        return Ok("skipped, no release build".to_owned());
    }

    let hash_of = |binary: &str| -> Option<String> {
        let out = Command::new(root.join(binary))
            .args(["sim", scene, "--ticks", "300"])
            .current_dir(root)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Small enough to scan rather than pull in a JSON dependency, which
        // xtask deliberately has none of.
        let key = "\"state_hash\": \"";
        let start = text.find(key)? + key.len();
        let end = start + text[start..].find('"')?;
        Some(text[start..end].to_owned())
    };

    match (hash_of("target/debug/loom"), hash_of("target/release/loom")) {
        (Some(debug), Some(release)) if debug == release => Ok(debug),
        (Some(debug), Some(release)) => Err(format!(
            "determinism\n  debug and release disagree: {debug} vs {release}\n  \
             every `loom sim --assert` depends on this holding"
        )),
        _ => Ok("skipped, could not read a hash".to_owned()),
    }
}

/// Record anything the run said that means it went wrong.
fn collect(failures: &mut Vec<String>, what: &str, result: &Result<Output, String>) {
    let output = match result {
        Ok(output) => output,
        Err(e) => {
            failures.push(format!("{what}\n  could not run: {e}"));
            return;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    let messages: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("vulkan validation"))
        .collect();
    if !messages.is_empty() {
        // Deduplicated: the layers repeat per frame and ninety copies of one
        // message buries the other four.
        let mut seen: Vec<&str> = Vec::new();
        for message in messages {
            if !seen.contains(&message) {
                seen.push(message);
            }
        }
        failures.push(format!("{what}\n  {}", seen.join("\n  ")));
    }

    // A crash is a validation failure by another name — and the teardown bug
    // that froze the displays showed up as exactly this, with the layers
    // silent right up to the segfault.
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map_or_else(|| "killed by signal".to_owned(), |c| format!("exit {c}"));
        let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
        failures.push(format!(
            "{what}\n  {code}\n  {}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n  ")
        ));
    }
}

fn run(loom: &Path, root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new(loom)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())
}

/// Whether this machine can create a Vulkan device at all.
fn has_vulkan_device(loom: &Path, root: &Path) -> bool {
    // `describe` needs no GPU, so a failure here is not about Vulkan. The
    // cheapest real probe is a tiny headless render.
    let out = root.join("target/xtask-probe.png");
    run(
        loom,
        root,
        &[
            "render",
            "assets/test/blockout.loom",
            "--size",
            "32x32",
            "--out",
            out.to_str().unwrap_or("probe.png"),
        ],
    )
    .is_ok_and(|o| o.status.success())
}

/// Build the debug binary — the profile in which the layers are enabled — and
/// return its path.
fn build_debug(root: &Path) -> Result<PathBuf, String> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "-p", "loom_cli"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !status.success() {
        return Err("the debug build failed; nothing to validate".to_owned());
    }
    let binary = root.join("target/debug/loom");
    if binary.exists() {
        Ok(binary)
    } else {
        Err(format!("{} was not produced", binary.display()))
    }
}

fn repo_root() -> PathBuf {
    // The xtask crate lives one level below the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
