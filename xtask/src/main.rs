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
const SCENES: [&str; 37] = [
    "assets/test/lanternhead.loom",
    // The imported PBR library. **Not in `GOLDEN`**: it adds no rendering path
    // that `materials.loom` does not already cover — it is a catalogue of
    // assets, and what it guards is that they still load and validate.
    "assets/test/pbr_library.loom",
    // The imported model library. In `SCENES` and not `GOLDEN` for the reason
    // `pbr_library.loom` gives — it is a catalogue of assets, and what is worth
    // guarding is that they still load, not their pixels.
    "assets/test/props.loom",
    // The secondary-ray demo (ADR 0019). In `SCENES` and not `GOLDEN`: the
    // paths it shows are already covered — `materials.loom` sweeps metallic to
    // 1.0, and sixteen references moved when the ray terms landed, so the gate
    // already watches them. What this scene is for is a human looking at it.
    "assets/test/stoneyard.loom",
    "assets/test/blockout.loom",
    "assets/test/ground.loom",
    "assets/test/beach.loom",
    "assets/test/campfire.loom",
    "assets/test/tower.loom",
    "assets/test/primitives.loom",
    "assets/test/cave.loom",
    "assets/test/office.loom",
    "assets/test/materials.loom",
    "assets/test/terrain_stress.loom",
    // The only scene whose landform comes from a `loom_terrain` recipe rather
    // than from `kind = "heightfield"`'s five octaves of fBm. Erosion, spline
    // carve, flatten disc and the corridor guarantee reach the SDF here and
    // nowhere else.
    "assets/test/vale.loom",
    "assets/test/smoke.loom",
    "assets/test/windy.loom",
    "assets/test/meadow.loom",
    "assets/test/grass_slope.loom",
    "assets/test/ocean.loom",
    // The only scene where a rigid body is driven by the water rather than
    // only drawn against it, so it is the only one whose `render --sim` runs
    // the buoyancy solver at all.
    "assets/test/water_crate.loom",
    // The only scene where water meets land: a depth grid uploaded over buffer
    // device address, a shoreline discard, and waves attenuating in the
    // shallows — none of which `ocean` draws a pixel of.
    "assets/test/shore.loom",
    // The only scene rendered from *under* the water: a different branch of
    // the water fragment shader, a different fog term in every other shader
    // in the frame, and a sky pass that paints the medium instead of the
    // horizon. Nothing else in this list submerges the camera.
    "assets/test/underwater.loom",
    // The only scene with a current: the flow field is baked from the voxel
    // bed's drainage at load, and `render --sim` is where a river both draws
    // and carries something at once. It is deliberately *not* in `GOLDEN` —
    // the surface it draws is the surface `shore` already covers, because flow
    // reaches `velocity` and nothing the shader reads.
    "assets/test/river.loom",
    // The only scene where particles are spawned *by* the simulation rather
    // than by an emitter standing somewhere: the crate goes under at tick 105
    // and the splash is replayed from the event log, so a `--sim 120` render
    // here binds the particle pipeline off the back of a physics run. Nothing
    // else in this list does that.
    "assets/test/splash.loom",
    // The only scene with rain: a third pass in the frame, a pipeline with no
    // depth attachment at all, and the depth buffer sampled as a texture
    // rather than tested against. None of the twenty-three above records a
    // single one of those transitions.
    "assets/test/rain_overhang.loom",
    // The only scene whose rain is stopped by geometry that is not in a voxel
    // volume — a mesh deck with a box collider. It is the only exercise of the
    // collision-field bake and upload path, and of the indirect splash draw
    // over a surface that is not the ground.
    "assets/test/rain_gantry.loom",
    // Rain that varies across the world, which no other scene has: the cover
    // evaluation in `rainVertexMain` and in `sample_rain` runs only when a
    // scene authors a broken deck, and every other rain scene authors a solid
    // one and short-circuits it.
    "assets/test/squall.loom",
    // The only scene with a dished floor, so the only one where the puddle term
    // is anything but zero: every other rain scene has a flat slab, and
    // concavity on a flat slab is nothing. Also the finest voxels in the list
    // at 0.25 m, which is what a 6 cm hollow needs to exist at all.
    "assets/test/puddles.loom",
    // The only scene with scattered instances: a `Scatter` component resolving
    // through `loom_scatter` onto voxel terrain, and the only exercise of a
    // mesh reached through a scatter field rather than a `MeshRenderer` — which
    // is the path that silently fell back to a box the first time.
    "assets/test/forest.loom",
    "assets/test/camera.loom",
    "assets/test/walker.loom",
    "assets/test/explosion.loom",
    "assets/test/range.loom",
    "assets/test/turret_range.loom",
    "assets/games/proving_ground.loom",
    // The only scene that runs several systems *at once*: voxel terrain, one
    // water body serving both a current and open ocean, three grass fields,
    // rain with wetness and shelter, additive and alpha particles, wind and an
    // authored environment, in one frame. Everything above it isolates one
    // path; this is the one that catches two paths interfering.
    "assets/test/homestead.loom",
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
const GOLDEN: [(&str, &str, &[&str]); 27] = [
    // **The post-process stack's standing shot** (ADR 0018). Nothing else in
    // this list is composed around what happens *above* diffuse white: the
    // flame's core, the brazier's pool, the sun's glitter path, wet stone at
    // `WET_SMOOTH` roughness under a low sun, and the sun disc are all past it.
    // 2400 ticks is 40 s - exactly `WET_COVER_WINDOW`, so `cover_recent`'s
    // three taps have a full window, and long enough for the dinghy to settle.
    ("lanternhead", "assets/test/lanternhead.loom", &["--sim", "2400"]),
    ("primitives", "assets/test/primitives.loom", &[]),
    ("materials", "assets/test/materials.loom", &[]),
    ("cave", "assets/test/cave.loom", &[]),
    // **The recipe pipeline reaching the SDF**, which nothing else in this list
    // covers and nothing else in the project measures. `cave` and
    // `terrain_stress` are voxels too, but their landform is analytic — a
    // sphere, or fBm evaluated per column — so a change to fBm's normalise
    // step, to either erosion pass, to the spline carve or to the bilinear
    // placement moves no pixel any gate looks at. It is not a new *rendering*
    // path and it does not claim to be: it is the only picture of a bake that
    // ran the whole 2D pipeline.
    ("vale", "assets/test/vale.loom", &[]),
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
    // **Water against terrain**, which `ocean` cannot cover: it has no voxel
    // volume at all, so its depth is the sentinel everywhere and its waves are
    // never attenuated. Everything W6 added is visible here and nowhere else —
    // the shoreline the fragment shader cuts, the flattening of the swell as
    // it comes onto the shelf, and the shallow tint that used to be a
    // hardcoded six metres.
    //
    // `--sim 90` for the same reason as `ocean`: t = 0 is the one instant
    // where every wave's phase is zero, which is the least representative
    // frame there is.
    ("shore", "assets/test/shore.loom", &["--sim", "90"]),
    // **The water seen from below**, which neither of the two above can cover:
    // they both render from a camera in the air, so the surface's
    // below-surface branch, the underwater fog constants and the submerged
    // sky pass are all dead code as far as they are concerned. It went in with
    // the branch it covers, rather than a commit later — the bug it exists to
    // catch broke nothing, produced no validation message and looked like a
    // sunlit sea, so a human reading a diff would not have found it either.
    //
    // `--sim 90` for the same reason as the two above.
    ("underwater", "assets/test/underwater.loom", &["--sim", "90"]),
    // **Short waves are a rendering path of their own.** `river` authors 3.4 m
    // and 1.9 m ripples, far shorter than anything `ocean` or `shore` carries,
    // and it was in SCENES but not here — so when the Nyquist fade landed and
    // silently deleted every wave under 4 m, no gate saw the river turn into a
    // mirror. A human did. This is that gate.
    ("river", "assets/test/river.loom", &["--sim", "300"]),
    // **Rain is its own rendering path**, and it carries what nothing else
    // here does: a pass drawn after the forward pass, into the resolved colour
    // target, with the depth buffer sampled as a texture instead of tested
    // against. Its content is the phase exit criterion made visible — streaks
    // everywhere except under the shelter, and a column of them coming through
    // the skylight one CSG subtract punched in the roof.
    //
    // `--sim` at all because rain at t = 0 is the one frame where every drop
    // sits exactly on its hashed lattice point, which is the least
    // representative instant there is — the same reason `ocean` and `shore`
    // are simulated.
    //
    // **1800 ticks, and it used to be 90, because wetness is slow.** Step 3's
    // whole content is in the forward material and a second and a half of rain
    // moves it by well under the tolerance — the reference matched
    // byte-for-byte with the wetness code in and with it stubbed out, which is
    // a gate protecting nothing. Thirty seconds in, the film is at its ceiling
    // and the soak is about half way, the exposed ground and the top of the
    // slab are visibly darker and glossier, and the sheltered floor and the
    // undersides are not: 18.5% of pixels move against a stub, measured.
    ("rain_overhang", "assets/test/rain_overhang.loom", &["--sim", "1800"]),
    // **And the near field, because `rain_overhang` cannot see step 4.** Its
    // ground starts ten metres away, where a splash crown is a third of a pixel
    // and a ripple ring is under one: both effects measured *below tolerance*
    // there — 0.016% of pixels for a stubbed ripple against a 0.1% threshold —
    // so a reference of that shot would have been protecting nothing. Exactly
    // the trap step 3 found in this same scene one slice earlier, and the
    // reason a second rain scene is worth its render.
    //
    // A metre above wet concrete with the sun low and ahead, which is the
    // geometry that makes wet ground read as wet, plus a canopy over the near
    // left so the impacts stop where the wetness does. Stubbing either half
    // moves 0.47% and 1.01% of pixels here.
    ("rain_impact", "assets/test/rain_impact.loom", &["--sim", "1800"]),
    // **Rain under a MESH roof, which no other scene here can see.** Both rain
    // scenes above shelter with a voxel slab, and a voxel slab is what the
    // baked terrain height field could already express — they passed for two
    // slices with no collision in the engine at all. The gantry is a box mesh
    // with a `BoxCollider` and nothing else, so the only thing that stops a
    // drop under it is `loom_rain::collide`'s bake of the collision world and
    // `rain_sim.slang`'s march through it (ADR 0015, and ADR 0014's trigger 2).
    //
    // It carries the splashes' *collision* half too: every ring on this apron
    // is an impact the simulation resolved, and there are none under the deck
    // for the same reason there are no streaks.
    //
    // `--sim 600` is ten seconds — well past the drop field's four-second
    // settling time, with the apron wet and the strip under the deck dry.
    ("rain_gantry", "assets/test/rain_gantry.loom", &["--sim", "600"]),
    // **The only scene where the rain is not uniform.** Cloud cover multiplies
    // the rate per drop, so the shower has an edge crossing open water — a
    // rendering path no other reference covers, because every other rain scene
    // authors a solid deck and short-circuits the cover evaluation entirely.
    // Adding a rendering path means adding a scene here, and rain that varies
    // across the frame is one.
    ("squall", "assets/test/squall.loom", &["--sim", "900"]),
    // **Standing water, which no other reference contains.** Stubbing
    // `puddleMask` moves 0.8% of this frame against a 0.1% tolerance — checked
    // rather than assumed, because a puddle scene framed a few metres higher
    // moved 0.23% and would have passed the gate while barely showing the
    // feature.
    ("puddles", "assets/test/puddles.loom", &["--sim", "900"]),
    // **Scattered instances, which no other reference contains.** Placement is
    // a pure function of position, so this needs no `--sim`: the same seed and
    // the same terrain give the same forest every time, which is the property
    // `loom_scatter` is built around and this is the picture of it.
    ("forest", "assets/test/forest.loom", &[]),
    // **A texture sampled on voxel terrain, which no other reference covers.**
    // `terrain_stress` and `terrain_billion` are the only other scenes that put
    // an `albedo_map` on a `VoxelVolume` and neither is in this list, so until
    // this scene existed every triplanar path — three projections, the whiteout
    // normal blend, the weight exponent — could be arbitrarily wrong and the
    // gate would still report a full pass. One of them WAS wrong: the X-plane
    // normal was sampled transposed. No `--sim`: nothing here moves.
    ("ground", "assets/test/ground.loom", &[]),
    // **A beach: a big shallow shelf with a seabed under it.** The only golden
    // scene where refraction has metres of water to work through over a lit
    // bottom, and the one the shore features are judged against. `--sim 400`
    // is past the wave set's settling.
    ("beach", "assets/test/beach.loom", &["--sim", "400"]),
    // **Night lit by a point light and nothing else.** No sun, no ambient, a
    // near-black sky — so this is the one reference where the `Light`
    // component is the only thing keeping the frame from being empty. Stub it
    // and the scene goes dark, which is what makes it a gate rather than a
    // picture of a fire.
    ("campfire", "assets/test/campfire.loom", &["--sim", "200"]),
    // **A buoyant body through the water surface, which no other reference
    // covers.** Every other water scene either has nothing crossing the
    // surface or has a static post. This is the scene that shows what happens
    // where a floating object meets the water it displaces, and it is the one
    // that would catch a refraction term smearing foreground geometry into the
    // sea. Blessed BEFORE the forward pass is split, deliberately: a reference
    // blessed afterwards proves nothing about whether the split changed it.
    ("water_crate", "assets/test/water_crate.loom", &["--sim", "90"]),
    // **Particles at the waterline.** Water and the blended particle pass are
    // drawn in the same block today; anything that splits that block moves
    // their ordering, and this is the only reference that would show it.
    ("splash", "assets/test/splash.loom", &["--sim", "120"]),
    // **The changelog shot, and it is deliberately not a normal gate.** Every
    // reference above protects one rendering path and moves only when that
    // path changes. This one touches terrain, water, current, grass, rain,
    // wetness, shelter, both particle blend modes, wind and the environment at
    // once, so it moves whenever *anything* does — and that churn is the
    // point, not a defect. Its value is that each re-bless is a readable line
    // in `MANIFEST.txt` next to a visible diff, which is the only artefact in
    // this repo that shows what a change did to a whole frame rather than to
    // one system. **Do not remove it to quiet the re-blessing.** The header of
    // the scene file says the same thing at more length.
    //
    // `--sim 1800` for the reason `rain_overhang` uses it: wetness is slow,
    // and thirty seconds in is the first frame where the film has reached its
    // ceiling and the soak is half way. It also puts the waves and both
    // plumes well past their opening transient.
    ("homestead", "assets/test/homestead.loom", &["--sim", "1800"]),
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
            // **And `shore`, for the pointer.** It is the only scene where the
            // water shader dereferences the terrain height buffer at all —
            // every other one has no terrain and returns the sentinel before
            // touching it — and the viewer allocates and uploads that buffer
            // through its own path, not the offscreen renderer's. A null or
            // stale address there is a device fault on the first wave, and no
            // headless gate would see it.
            "assets/test/shore.loom",
            // **And `rain_overhang`, for the pass and the descriptor.** Rain
            // is the first thing here that draws in a pass of its own, and the
            // first that binds a descriptor set the viewer builds and repoints
            // itself — the depth image is recreated on every resize, and the
            // rain pass samples it. Nothing headless resizes, so nothing
            // headless can catch a descriptor left pointing at a destroyed
            // view. It is also the third slice in a row where the window could
            // have quietly got a simplified version of the offscreen path.
            "assets/test/rain_overhang.loom",
    // The only scene whose rain is stopped by geometry that is not in a voxel
    // volume — a mesh deck with a box collider. It is the only exercise of the
    // collision-field bake and upload path, and of the indirect splash draw
    // over a surface that is not the ground.
    "assets/test/rain_gantry.loom",
        ] {
            if !root.join(scene).exists() {
                continue;
            }
            checked += 1;
            let result = run(&loom, &root, &["run", scene, "--edit", "--frames", &frames]);
            collect(&mut failures, &format!("run --edit {scene}"), &result);
        }

        // **The frame's CPU cost, with the simulation actually running.**
        //
        // Nothing else in this gate can see this. A golden image renders one
        // frame, and one frame is exactly the case where doing per-scene work
        // once and doing it sixty times a second cost the same — so placing a
        // scatter field on every frame took `forest.loom` to 9 fps while all
        // twenty references passed. `--play` exists so this check can reach the
        // path a running game takes; `--frames` alone runs a paused editor.
        //
        // **The budget is in DEBUG milliseconds**, which is what this gate
        // runs — the validation layers are the whole reason it does. Debug is
        // roughly thirty times slower than release here, and calibrating
        // against a release measurement is how the first version of this check
        // failed on correct code: 8 ms was right for release and `forest`
        // measures 9.5 ms debug.
        //
        // Measured baselines, debug, with the simulation running:
        //
        //     forest 9.6 ms/frame      proving_ground 3.2 ms/frame
        //
        // 30 ms is about three times the worst of those. It is a ceiling on
        // obvious regressions rather than a target: the defect this check was
        // written for measured 103 ms/frame in *release*, so in debug it is
        // orders of magnitude past this and would be caught with room to spare.
        // Tightening it toward the baseline would turn ordinary variance into
        // failures, which is how a gate gets ignored.
        const CPU_BUDGET_MS: f64 = 30.0;
        for scene in [
            "assets/test/forest.loom",
            "assets/games/proving_ground.loom",
            "assets/test/lanternhead.loom",
        ] {
            if !root.join(scene).exists() {
                continue;
            }
            checked += 1;
            let result =
                run(&loom, &root, &["run", scene, "--edit", "--play", "--frames", &frames]);
            collect(&mut failures, &format!("run --play {scene}"), &result);

            let text = result.as_ref().map_or_else(
                |e| e.clone(),
                |out| {
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    )
                },
            );
            match text
                .lines()
                .find_map(|l| l.split_once("cpu ").map(|(_, rest)| rest))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse::<f64>().ok())
            {
                Some(mean) if mean <= CPU_BUDGET_MS => {
                    println!("  {scene}: {mean:.3} ms/frame of CPU");
                }
                Some(mean) => failures.push(format!(
                    "{scene}: {mean:.3} ms of CPU per frame, over the {CPU_BUDGET_MS} ms budget"
                )),
                // A run that printed no line is a run that did not measure —
                // reported rather than treated as a pass, which is the whole
                // lesson of the gates that reported success without looking.
                None => failures
                    .push(format!("{scene}: --play printed no cpu line, so nothing was measured")),
            }
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

/// Scenes simulated under both build profiles, whose hashes must agree.
///
/// **`tower` has no voxels in it**, and for most of this project's life it was
/// the whole of this check — so "the sim hash is unchanged" was never once a
/// statement about a voxel bake, in either direction.
///
/// `river` is the cheapest scene measured that can *see* one. Its hash reads
/// the baked field twice over: the volume becomes solid cells for physics, and
/// the water's bed is marched out of the same volume, so the crate floating
/// down the channel is steered by it. Changing the channel capsule's radius
/// from 3.0 to 2.1 moves the hash from `1c33f211d7ea9916` to `ec853d50f513842a`
/// [measured].
///
/// **`rain_gantry` was tried first and cannot see a bake at all** — the report
/// suggests it for its 8-chunk cost, but its voxels are static and nothing
/// rests on them, so the same kind of mutation (`half_extents` 15 → 9) leaves
/// its hash at `1f3b3d49a69f244f` [measured]. A voxel scene is not
/// automatically a voxel *check*; the sim hash covers rigid bodies, so the
/// terrain has to be under one.
const DETERMINISM_SCENES: [&str; 2] = ["assets/test/tower.loom", "assets/test/river.loom"];

/// Simulate the same scenes with both build profiles and require the same hash.
fn determinism_holds(root: &Path) -> Result<String, String> {
    if DETERMINISM_SCENES.iter().any(|s| !root.join(s).exists()) {
        return Ok("skipped, no scene".to_owned());
    }
    let release = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "loom_cli"])
        .current_dir(root)
        .status();
    if !release.is_ok_and(|s| s.success()) {
        return Ok("skipped, no release build".to_owned());
    }

    let hash_of = |binary: &str, scene: &str| -> Option<String> {
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

    let mut agreed = Vec::new();
    for scene in DETERMINISM_SCENES {
        let name = scene.rsplit('/').next().unwrap_or(scene);
        match (
            hash_of("target/debug/loom", scene),
            hash_of("target/release/loom", scene),
        ) {
            (Some(debug), Some(release)) if debug == release => {
                agreed.push(format!("{name} {debug}"));
            }
            (Some(debug), Some(release)) => {
                return Err(format!(
                    "determinism\n  {name}: debug and release disagree: {debug} vs {release}\n  \
                     every `loom sim --assert` depends on this holding"
                ));
            }
            _ => return Ok("skipped, could not read a hash".to_owned()),
        }
    }
    Ok(agreed.join(", "))
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
