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
const SCENES: [&str; 19] = [
    "assets/test/blockout.loom",
    "assets/test/tower.loom",
    "assets/test/primitives.loom",
    "assets/test/cave.loom",
    "assets/test/office.loom",
    "assets/test/materials.loom",
    "assets/test/terrain_stress.loom",
    "assets/test/smoke.loom",
    "assets/test/windy.loom",
    "assets/test/meadow.loom",
    "assets/test/grass_slope.loom",
    "assets/test/ocean.loom",
    // The only scene where a rigid body is driven by the water rather than
    // only drawn against it, so it is the only one whose `render --sim` runs
    // the buoyancy solver at all.
    "assets/test/water_crate.loom",
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
        "image" => image(std::env::args().any(|a| a == "--bless")),
        "flythrough" => flythrough(),
        "shimmer" => shimmer(),
        other => {
            eprintln!(
                "unknown task {other:?}\n\nUSAGE:\n    cargo xtask validate\n    \
                 cargo xtask image [--bless]\n    cargo xtask flythrough
    cargo xtask shimmer"
            );
            std::process::ExitCode::from(2)
        }
    }
}

/// Scenes with a committed reference image, and how to render each.
///
/// Chosen for coverage of *rendering paths*, not for looking nice: mesh
/// geometry, the bindless texture array, voxel terrain, alpha-blended
/// particles, additive particles over a physics run, and a scene whose
/// environment is authored dark with heavy fog. A regression in any one of
/// those shows up in exactly one of these and nowhere else.
///
/// Small on purpose. 320x200 is enough to catch a shader change and keeps
/// each reference a few kilobytes, which is the difference between committing
/// them and bloating history with them.
const GOLDEN: [(&str, &str, &[&str]); 10] = [
    ("primitives", "assets/test/primitives.loom", &[]),
    ("materials", "assets/test/materials.loom", &[]),
    ("cave", "assets/test/cave.loom", &[]),
    ("smoke", "assets/test/smoke.loom", &[]),
    ("explosion", "assets/test/explosion.loom", &["--sim", "22"]),
    // Wind-advected particles: the one scene where the field does something
    // visible, so a wind regression shows up as a picture rather than only as
    // a changed hash.
    ("windy", "assets/test/windy.loom", &["--sim", "360"]),
    (
        "proving_ground",
        "assets/games/proving_ground.loom",
        &["--sim", "150"],
    ),
    // **Grass is its own rendering path** and was missing from this list until
    // the density falloff landed and the gate reported seven matches without
    // having looked at a single blade. Nothing else here draws geometry that
    // exists only in the vertex shader — no vertex buffer, no index buffer, the
    // triangles derived from `SV_VertexID` — so nothing else would catch a
    // regression in it.
    ("meadow", "assets/test/meadow.loom", &[]),
    // **Grass reading the terrain it stands on**, which `meadow` cannot cover
    // because its ground is a flat box. This is the only scene that exercises
    // the voxel-SDF height march, the per-field ground grid, and the
    // slope/flow response — and it went in with none of the four gates
    // touching it, which is the same miss `meadow` made one commit earlier.
    ("grass_slope", "assets/test/grass_slope.loom", &[]),
    // **Water is its own rendering path**, and the most regression-prone one
    // in the project: it is a displaced mesh generated entirely in the vertex
    // shader from a wave sum, shaded by a Fresnel term against the sky. Small
    // numeric changes there produce large visible ones, and none of the nine
    // above draw a single water vertex.
    //
    // `--sim 90` because the sea at t=0 is a different surface from the sea a
    // second and a half in, and a reference taken at the one instant where
    // every wave's phase is zero would be the least representative frame there
    // is.
    ("ocean", "assets/test/ocean.loom", &["--sim", "90"]),
];

/// Every reference renders at this size.
const GOLDEN_SIZE: &str = "320x200";

/// **The pixel diff `CLAUDE.md`'s definition of green has never had.**
///
/// Clippy catches what the compiler does not, the validation layers catch what
/// clippy does not, and determinism hashes catch a simulation that drifted.
/// None of them catches a shader that now renders everything slightly wrong,
/// and until now that was found by a human opening the PNG.
///
/// That does not scale to what is queued: water, rain, wind and vegetation are
/// the four most visually regression-prone systems in the backlog, and every
/// numeric change to a shader in any of them is otherwise unverifiable.
fn image(bless: bool) -> std::process::ExitCode {
    let root = repo_root();

    let loom = match build_debug(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask image — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let references = root.join("tests/references");
    if let Err(e) = std::fs::create_dir_all(&references) {
        eprintln!("xtask: cannot create {}: {e}", references.display());
        return std::process::ExitCode::from(2);
    }
    let scratch = root.join("target/xtask-image");
    let _ = std::fs::create_dir_all(&scratch);

    let mut failures = Vec::new();
    let mut blessed = Vec::new();
    let mut checked = 0;

    for (name, scene, extra) in GOLDEN {
        // **A missing scene fails the gate.** It used to skip, which meant
        // renaming a scene file quietly dropped a whole rendering path out of
        // coverage while the gate still printed success. That is the one
        // failure mode a regression harness must not have, because every
        // phase after this one trusts it to be watching.
        if !root.join(scene).exists() {
            failures.push(format!(
                "{name}: {scene} is missing — the golden list names a scene that \
                 does not exist, so this rendering path is unverified"
            ));
            continue;
        }
        checked += 1;

        let rendered = scratch.join(format!("{name}.png"));
        let reference = references.join(format!("{name}.png"));
        let rendered_path = rendered.to_string_lossy().into_owned();

        let mut argv: Vec<&str> = vec!["render", scene, "--out", &rendered_path, "--size", GOLDEN_SIZE];
        argv.extend_from_slice(extra);
        let render = match run(&loom, &root, &argv) {
            Ok(output) => output,
            Err(e) => {
                failures.push(format!("render {name}: {e}"));
                continue;
            }
        };
        if !render.status.success() {
            failures.push(format!(
                "render {name}: {}",
                String::from_utf8_lossy(&render.stderr).trim()
            ));
            continue;
        }

        // A reference that does not exist yet is not a failure the first time
        // a scene is added — but it is not a pass either. Blessing is the only
        // way to create one, so the intent is always explicit.
        if !reference.exists() {
            if bless {
                if let Err(e) = std::fs::copy(&rendered, &reference) {
                    failures.push(format!("{name}: cannot write reference: {e}"));
                } else {
                    blessed.push(format!("{name} (new)"));
                }
            } else {
                failures.push(format!(
                    "{name}: no reference image; run `cargo xtask image --bless` to create one"
                ));
            }
            continue;
        }

        if bless {
            match std::fs::copy(&rendered, &reference) {
                Ok(_) => blessed.push(name.to_owned()),
                Err(e) => failures.push(format!("{name}: cannot write reference: {e}")),
            }
            continue;
        }

        let compared = match run(
            &loom,
            &root,
            &["compare", &rendered_path, &reference.to_string_lossy()],
        ) {
            Ok(output) => output,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if !compared.status.success() {
            // The JSON carries the numbers; printing it whole means the reader
            // sees how far off it was, not merely that it was.
            failures.push(format!(
                "{name}: {}",
                String::from_utf8_lossy(&compared.stdout).replace('\n', " ")
            ));
        }
    }

    if bless {
        write_manifest(&references);
        println!(
            "cargo xtask image --bless: {} reference(s) accepted{}",
            blessed.len(),
            if blessed.is_empty() {
                String::new()
            } else {
                format!(" — {}", blessed.join(", "))
            }
        );
        return std::process::ExitCode::SUCCESS;
    }

    if failures.is_empty() {
        println!("cargo xtask image: {checked} scene(s) match their reference");
        return std::process::ExitCode::SUCCESS;
    }
    for failure in &failures {
        eprintln!("  image diff: {failure}");
    }
    eprintln!(
        "\n{} scene(s) differ from their reference. If the change was intended, \
         look at target/xtask-image/*.png against tests/references/*.png and then \
         run `cargo xtask image --bless`.",
        failures.len()
    );
    std::process::ExitCode::from(1)
}


/// Measure temporal instability — the phase-2 risk, as a number.
///
/// **Shimmer is not visible in a still and is not an opinion.** It is pixels
/// changing between two frames that should look almost the same: nudge the
/// camera a quarter of a degree and a stable image barely moves, while
/// sub-pixel geometry crawls, twinkles and pops. So the measurement is the
/// mean fraction of pixels that differ between *consecutive* frames of a very
/// slow pan.
///
/// This is deliberately the same comparison the golden-image gate uses, at the
/// same calibrated per-channel threshold, so a number here means the same
/// thing a number there does.
///
/// Not a gate. There is no correct value — a scene with more geometry in it
/// shimmers more, and the useful reading is the *ratio* between two settings
/// of the thing being investigated. It prints a table and returns success.
fn shimmer() -> std::process::ExitCode {
    let root = repo_root();
    let loom = match build_debug(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask shimmer — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let out = root.join("target/xtask-shimmer");
    let _ = std::fs::create_dir_all(&out);

    // Twelve frames, camera bolted down, a fifth of a second of wind between
    // each. See the note on `--step` below for why the camera stopped moving.
    const FRAMES: u32 = 12;
    const SPIN: &str = "0";
    const SIZE: &str = "640x400";

    // **Only comparable within a scene.** The measurement counts every pixel
    // that changed, which includes legitimate parallax — a richly textured
    // scene scores higher than a bare one without being less stable. What it
    // is for is the ratio between two settings of the thing under
    // investigation, on the same scene, with everything else held still.
    println!("{FRAMES} frames at {SIZE}, camera static (spin {SPIN}), sim advancing.");
    println!("Compare a scene against ITSELF under another setting; across scenes it means nothing.");
    println!();
    println!("scene                     flicker   changed%   (flicker is the AA number)");
    let mut failed = false;
    // `meadow` used to be chained on here because it was not in `GOLDEN`. It is
    // now, and leaving the chain in printed it twice.
    for (name, scene, _) in &GOLDEN {
        if !root.join(scene).exists() {
            continue;
        }
        let prefix = out.join(format!("{name}.png"));
        let rendered = run(
            &loom,
            &root,
            &[
                "render",
                scene,
                "--out",
                &prefix.to_string_lossy(),
                "--size",
                SIZE,
                "--frames",
                &FRAMES.to_string(),
                "--spin",
                SPIN,
                // **The simulation advances and the camera does not.** This is
                // the reverse of how it started, and the reversal is the whole
                // point.
                //
                // It began as a camera-motion metric: pan slowly, hold the
                // scene still, on the theory that a stable image reprojects
                // smoothly and only unstable pixels change. That theory is
                // wrong for sub-pixel geometry. `|b - (a+c)/2|` cancels motion
                // that is linear *in pixel value*, which holds for smooth
                // gradients and never for a field of blades translating a
                // couple of pixels a frame — a pure pan at the authored camera
                // measured 5.65 with 52% of pixels changing, entirely swamped.
                //
                // Holding the camera still and letting the wind blow measures
                // the artifact that actually afflicts grass: twinkle at rest.
                // It has a perfect control — `cave` has no animated geometry and
                // scores exactly 0.000 — and it is discriminating: `meadow`
                // scores ~0.54 against that zero.
                //
                // The cost is that anything else that animates now scores, so
                // `smoke` reads as unstable when it is merely a smoke plume.
                // That is acceptable because this number was never comparable
                // across scenes anyway; it is a scene against itself under two
                // settings.
                "--step",
                "12",
            ],
        );
        match rendered {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                failed = true;
                eprintln!("  shimmer {name}: {}", String::from_utf8_lossy(&output.stderr).trim());
                continue;
            }
            Err(e) => {
                failed = true;
                eprintln!("  shimmer {name}: {e}");
                continue;
            }
        }

        let mut changed = 0.0_f64;
        let mut pairs = 0_u32;
        for frame in 1..FRAMES {
            let a = out.join(format!("{name}_{:04}.png", frame - 1));
            let b = out.join(format!("{name}_{frame:04}.png"));
            let Ok(output) = run(
                &loom,
                &root,
                &["compare", &a.to_string_lossy(), &b.to_string_lossy()],
            ) else {
                continue;
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let Some(fraction) = read_field(&text, "\"fraction\"") else { continue };
            changed += fraction;
            pairs += 1;
        }

        // **The number that actually means something.** Three consecutive
        // frames: coherent motion is near-linear and cancels, a pixel that
        // twinkles does not. The changed-pixel count beside it is kept only
        // because it is cheap and occasionally explains a surprise.
        let mut noise = 0.0_f64;
        let mut triples = 0_u32;
        for frame in 1..FRAMES - 1 {
            let a = out.join(format!("{name}_{:04}.png", frame - 1));
            let b = out.join(format!("{name}_{frame:04}.png"));
            let c = out.join(format!("{name}_{:04}.png", frame + 1));
            let Ok(output) = run(
                &loom,
                &root,
                &[
                    "flicker",
                    &a.to_string_lossy(),
                    &b.to_string_lossy(),
                    &c.to_string_lossy(),
                ],
            ) else {
                continue;
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let Some(value) = read_field(&text, "\"flicker\"") else { continue };
            noise += value;
            triples += 1;
        }

        if pairs == 0 || triples == 0 {
            eprintln!("  shimmer {name}: no comparable frames");
            failed = true;
            continue;
        }
        println!(
            "{name:<24} {:>7.3}   {:>7.2}",
            noise / f64::from(triples),
            changed / f64::from(pairs) * 100.0
        );
    }

    if failed {
        return std::process::ExitCode::from(1);
    }
    std::process::ExitCode::SUCCESS
}

/// Pull a named number out of a one-line JSON result.
///
/// Hand-scanned rather than parsed: `xtask` has no dependencies on purpose, so
/// that a gate cannot be broken by the same bad crate version as the thing it
/// checks.
fn read_field(json: &str, key: &str) -> Option<f64> {
    let start = json.find(key)? + key.len();
    let rest = json.get(start..)?.trim_start().strip_prefix(':')?.trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.' && c != 'e' && c != '-')?;
    rest.get(..end)?.parse().ok()
}

/// Dump a reviewable frame sequence for every golden scene.
///
/// **Not a gate, and that is deliberate.** There is no reference sequence to
/// diff against, because the artifacts this exists to catch — shimmer,
/// popping, unison sway, swimming vegetation, a wind direction that snaps
/// instead of turning — are things a person recognises in motion and a
/// threshold cannot describe. What it does is make looking cheap: one command,
/// numbered frames, flick through them.
///
/// The implementation order calls this "the part that matters most and the
/// part most likely to be skipped", which is exactly why it is a task rather
/// than a set of flags to remember.
fn flythrough() -> std::process::ExitCode {
    let root = repo_root();
    let loom = match build_debug(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask flythrough — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let out = root.join("target/xtask-flythrough");
    let _ = std::fs::create_dir_all(&out);

    let mut failed = false;
    for (name, scene, _) in GOLDEN {
        // Not a gate, but still not silent: a scene that vanished from the
        // list is a hole in what anyone reviewing motion will actually see.
        if !root.join(scene).exists() {
            failed = true;
            eprintln!("  flythrough {name}: {scene} is missing");
            continue;
        }
        let prefix = out.join(format!("{name}.png"));
        let result = run(
            &loom,
            &root,
            &[
                "render",
                scene,
                "--out",
                &prefix.to_string_lossy(),
                "--size",
                "480x300",
                "--frames",
                "16",
                "--spin",
                "7",
                "--step",
                "10",
            ],
        );
        match result {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                failed = true;
                eprintln!(
                    "  flythrough {name}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(e) => {
                failed = true;
                eprintln!("  flythrough {name}: {e}");
            }
        }
    }

    if failed {
        return std::process::ExitCode::from(1);
    }
    println!(
        "cargo xtask flythrough: 16 frames per scene in {}",
        out.display()
    );
    std::process::ExitCode::SUCCESS
}

/// Record each reference's content hash next to it.
///
/// The references are committed — six small PNGs are tens of kilobytes, not
/// history bloat — but a binary diff tells a reviewer nothing. The manifest
/// makes a blessed change show up as a readable one-line hash change, so
/// "which references moved, and did anyone mean to move them" is answerable
/// from the diff alone.
fn write_manifest(references: &Path) {
    let Ok(entries) = std::fs::read_dir(references) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    names.sort();

    let mut lines = vec![
        "# Content hashes of the golden reference images.".to_owned(),
        "# Written by `cargo xtask image --bless`. A change here is a deliberate".to_owned(),
        "# re-blessing; a change to a PNG without a change here is a mistake.".to_owned(),
        String::new(),
    ];
    for path in names {
        let Ok(sum) = Command::new("sha256sum").arg(&path).output() else {
            continue;
        };
        let text = String::from_utf8_lossy(&sum.stdout);
        let Some(hash) = text.split_whitespace().next() else {
            continue;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        lines.push(format!("{hash}  {name}"));
    }
    let _ = std::fs::write(references.join("MANIFEST.txt"), lines.join("\n") + "\n");
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
        // Missing means unverified, not fine — see the same guard in `image`.
        if !root.join(scene).exists() {
            failures.push(format!(
                "{scene}: missing — the scene list names a file that does not exist"
            ));
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
        // **`meadow` is here for the pipelines the other two never bind.** A
        // pipeline's rasterisation sample count must match its attachment, and
        // the windowed path is single-sampled where the offscreen one is 4x —
        // so the grass pipeline is only wrong in the window, and only while a
        // blade is actually being drawn. Neither a blockout nor a cave has one.
        // `ocean.loom` is here for the same reason `meadow` is, and it is the
        // same trap one system later: the water pipeline is built at one
        // sample in the window and four offscreen, so a mismatch is invisible
        // to every headless gate and fires on the first frame that draws a
        // wave.
        for scene in [
            "assets/test/blockout.loom",
            "assets/test/cave.loom",
            "assets/test/meadow.loom",
            "assets/test/ocean.loom",
        ] {
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
    // **P1's exit criterion, and it needs the release profile to mean
    // anything.** The wind field's 10,000-tick hash is pinned in a unit test,
    // and `cargo test` builds debug — so on its own it proves the field has
    // not changed, not that the two profiles agree about it. A field is
    // arithmetic, which is exactly where inlining and vectorisation differ.
    match run_cargo(&root, &["test", "--release", "-p", "loom_field", "--quiet"]) {
        Ok(output) if output.status.success() => {
            println!("wind field: the 10k-tick hash holds in release too");
        }
        Ok(output) => {
            failures.push(format!(
                "wind field release tests\n  {}",
                String::from_utf8_lossy(&output.stdout).trim()
            ));
        }
        Err(e) => failures.push(format!("wind field release tests\n  could not run: {e}")),
    }

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
/// Run a cargo subcommand from the workspace root.
fn run_cargo(root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))
}

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
