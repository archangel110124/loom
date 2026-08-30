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
const SCENES: [&str; 80] = [
    // The mood ladder — ADR 0069. In `GOLDEN` too, where the reasoning is.
    "assets/test/mood_deep.loom",
    // A sphere dropped into a still pool. **In GOLDEN now** (W9): the impact
    // crown it threw nothing of is a rendering path, and the reasoning is on
    // that row. It still earns this line for what it always did — it loads,
    // bakes and validates the whole water stack at once.
    "assets/test/pool.loom",
    // The same pool with a stone in it, so the Worthington jet has somewhere to
    // come out of. In `GOLDEN`, where the reasoning is.
    "assets/test/pool_jet.loom",
    "assets/test/lanternhead.loom",
    // One building, composed. **In `SCENES` and not `GOLDEN`, on the stated
    // rule**: every path it draws is already covered — `ground` and
    // `lanternhead` the two-material slope blend, `materials` the bindless
    // tiling maps, `meadow` and `grass_slope` the vertex-shader blades, `props`
    // the per-model atlases, `campfire` the point light and the flame,
    // `smoke` the alpha plume, `squall` the cloud deck. Three voxel volumes in
    // one scene is not a rendering path either; `proving_ground` already has
    // three and sits in this list for the same reason.
    //
    // What it is for is a human looking at it, and what this line guards is
    // that it still loads, bakes and validates clean.
    //
    // **It is deliberately NOT in the `--play` list below.** Measured at 26.0
    // ms/frame debug against that check's 30 ms ceiling — 13% of headroom,
    // which is variance rather than margin. Its header has the breakdown.
    "assets/test/croft.loom",
    // A landscape out of one `heightfield` op, with the imported props on it at
    // their real size. **Not in `GOLDEN`**: `ground.loom` already references the
    // two-material slope blend, `grass_slope.loom` the grass over marched
    // terrain and `materials.loom` the bindless maps, and the only thing here
    // that no golden scene has is the heightfield op itself — which is a CPU
    // bake, not a rendering path, and `terrain_stress.loom` sits in this list on
    // the same basis.
    //
    // **What it is really guarding is the CPU budget**, and this line alone
    // does not do that — it is also in the `--play` list further down, which is
    // where the milliseconds are measured. It is the largest world here, 153.6 m
    // across, and its first draft measured 220.7 ms/frame against that 30 ms.
    "assets/test/mountain_pass.loom",
    // The imported PBR library. **Not in `GOLDEN`**: it adds no rendering path
    // that `materials.loom` does not already cover — it is a catalogue of
    // assets, and what it guards is that they still load and validate.
    "assets/test/pbr_library.loom",
    // The imported model library. In `SCENES` and not `GOLDEN` for the reason
    // `pbr_library.loom` gives — it is a catalogue of assets, and what is worth
    // guarding is that they still load, not their pixels.
    "assets/test/props.loom",
    // The first generated asset (Hunyuan3D via the `hunyuan3d-assets` skill).
    // In `SCENES` and not `GOLDEN` for the reason `props.loom` gives: it is an
    // import check, and a textured OBJ is not a rendering path anything else
    // is missing. What is worth guarding is that the mesh, its 2048 atlas and
    // Loom's UV convention still agree — a flipped V here would land the
    // needle islands on the atlas gutters and read as black patches.
    "assets/test/spruce.loom",
    // The alpha test. Also in `GOLDEN`, where the reasoning is.
    "assets/test/alpha_cutout.loom",
    // Blended transparency. Also in `GOLDEN`, where the reasoning is.
    "assets/test/glass.loom",
    // The layered tree. In `SCENES` and not `GOLDEN`: its rendering path is
    // the alpha test, which `alpha_cutout.loom` already guards at 320x200 far
    // more legibly than 6,500 cards would. What is worth checking here is that
    // a canopy of that size still loads and draws without a validation message.
    "assets/test/tree_layered.loom",
    // The secondary-ray demo (ADR 0019). **Also in `GOLDEN` now** — the
    // reasoning is there.
    "assets/test/stoneyard.loom",
    "assets/test/blockout.loom",
    "assets/test/ground.loom",
    "assets/test/beach.loom",
    "assets/test/campfire.loom",
    // The marched soot volume (W4) and the water reflection ray (W3) — see the
    // two `GOLDEN` rows for why each is a path nothing else covers.
    "assets/test/plume.loom",
    "assets/test/plume_gale.loom",
    "assets/test/plume_roof.loom",
    "assets/test/plume_close.loom",
    "assets/test/mirrorpool.loom",
    // **Here for the validation layers and for nothing else**, so it is in this
    // list and not in `GOLDEN`. Water with no mesh in the frame means an empty
    // TLAS, and once the water shader statically uses `sceneTLAS` an empty TLAS
    // is an unbound descriptor set 0. Every other water scene owns a mesh, so
    // the whole repository had no coverage of it — verified by reverting
    // `raytrace.rs`'s zero-instance build, which fires
    // `VUID-vkCmdDraw-None-08600` on this scene and on no other.
    "assets/test/bare_sea.loom",
    "assets/test/tower.loom",
    "assets/test/primitives.loom",
    "assets/test/cave.loom",
    "assets/test/office.loom",
    "assets/test/materials.loom",
    "assets/test/terrain_stress.loom",
    // The two scenes whose landform comes from a `loom_terrain` recipe rather
    // than from `kind = "heightfield"`'s five octaves of fBm. Erosion, spline
    // carve, flatten disc and the corridor guarantee reach the SDF here and
    // nowhere else.
    "assets/test/vale.loom",
    // The composition scene for §5 of the voxel-shape research: a 256 m glacial
    // vale with a recipe landform, a rotated hollow building on the pad it
    // flattened, an arch bored through a displaced crag, imported erratics and
    // grass, all in one frame.
    //
    // **In `SCENES` and not `GOLDEN`, on the stated rule.** It adds no
    // rendering path: `vale` already covers the terrain op, `meadow` and
    // `grass_slope` the vertex-shader blades, `ground` and `lanternhead` the
    // steep-slope layer, `props` and `materials` the bindless mesh atlases,
    // `campfire` the point light. What is new here is the *composition* and the
    // authoring rules it pins, and a reference of it would be a golden image
    // that moves whenever grass, terrain, materials or fog move — the churn
    // `homestead` and `lanternhead` already carry for the library, twice over.
    // `Displace` is not a rendering path either: it is an SDF term, and what
    // guards it is `loom_voxel`'s tests and the determinism hash.
    //
    // **It is the most expensive scene in this list at ~11 s**, against 2.1 s
    // for `terrain_stress` with twice the chunks. Its header has the breakdown
    // measured; the short version is that one displaced primitive and one grass
    // field cost more than the 1024-chunk landform bake put together.
    "assets/test/moraine.loom",
    "assets/test/smoke.loom",
    // The GPU particle pool. Also in `GOLDEN`, where the reasoning is; here
    // because the compute dispatch, the pipeline layout with no descriptor set
    // at all, and the compute-write/vertex-read buffer dependency are three
    // things the validation layers have an opinion about and the compiler has
    // none.
    "assets/test/emberfall.loom",
    "assets/test/windy.loom",
    "assets/test/meadow.loom",
    "assets/test/grass_slope.loom",
    "assets/test/ocean.loom",
    // The FFT ocean — ADR 0076. **The only scene with `wave_model = "spectrum"`**, so it
    // is the only exercise of the cascade upload (1.5 MB a tick of host-visible tiles read
    // by the vertex shader through a device address), of the buffer dependency declared for
    // it on the `forward` and `water` passes, and of the branch in `waterVertexMain` that
    // picks a wave model at all. Nothing else in this list can raise a validation message
    // about any of them.
    //
    // **Both runs this pass makes are needed here, and they take different branches.** The
    // no-`--sim` render has no cascade — the tiles are built inside the fixed step, so a
    // scene that never steps uploads `count = 0` — and it is what proves a spectrum body
    // with no sea still parses, still uploads an *empty* wave set (see the `add_water` fix
    // in the task-3 report) and draws the flat plane rather than a sixteen-wave sea the
    // physics has never heard of. The `--sim 120` render is the cascade path. `wake` is in
    // this list for the same two-branch reason.
    "assets/test/ocean_fft.loom",
    // The two teal scenes, in `SCENES` but deliberately not in `GOLDEN`: they exercise the
    // `WaterBody.optics` upload — the only path where a scene's own attenuation and
    // backscatter reach the environment buffer — and a reference for either wants a human
    // to bless it. `ocean_tropical` is also the only water scene with a sand shelf under it.
    "assets/test/ocean_fft_boat.loom",
    "assets/test/ocean_tropical.loom",
    // **The showcase**: the one scene that has to hold the backlit subsurface glow, the bed
    // seen through the column, the whitecaps and the spray in a single frame, from a camera
    // above the water. It is also the `water_glow` ablation's scene, below — `ocean_tropical`
    // held that row and the term is nearly silent there, because its sun is 38° up and its
    // steepest resolved face is 9.6°, so no face on it can be backlit at all.
    //
    // **In `SCENES` and not `GOLDEN`.** Its rendering paths are all covered elsewhere
    // (`ocean_tropical` the optics upload, `whitecaps` the foam, `spindrift` the spray,
    // `shore` the shallow band) and a reference image of an aesthetic deliverable is the
    // human's to bless.
    "assets/test/lucent.loom",
    // **The submerged camera that also authors `optics`**, and it is
    // `ocean_tropical` with the lens moved 1.5 m under its own waterline.
    // `underwater` below is submerged too and has been since the below-surface
    // branch landed — what neither it nor anything else had was a scene where
    // the eye is under water the *scene* described, which is how the medium
    // kept painting itself from two hardcoded constants after
    // `WaterBody.optics` existed. A constant and a correctly-read default are
    // the same picture. Not in `GOLDEN` — a reference is the human's to bless.
    "assets/test/ocean_under.loom",
    // The whitecap trail (W2). Three extra `loom_sample_water` taps per water
    // vertex and a fifth varying out of `waterVertexMain`, which is the one
    // place a wrong `TEXCOORD` index or an overflowed output signature would
    // show up as a validation message rather than as a picture.
    "assets/test/whitecaps.loom",
    // The advected foam field — ADR 0055. A crate ploughing a pool is the one
    // foam source that cannot be evaluated backwards from a position, which is
    // exactly what the field exists for. Also in `GOLDEN`.
    "assets/test/plough.loom",
    "assets/test/slosh.loom",
    // The cinematic tier's other two — ADR 0057 addendum. `ribbon` is the only
    // scene with an inflow at all (a `Cascade` over cinematic water, recycled
    // by ordinal) and `plough_cinematic` is the only one where a driven hull
    // opens a hollow in a volume. Both in `GOLDEN`, where the reasoning is.
    "assets/test/ribbon.loom",
    "assets/test/plough_cinematic.loom",
    // The same tank from the water's own level. **Not a duplicate scene** — it
    // `extends` the one above and changes one node's transform, so the physics
    // cannot drift between the pair. It is here because the authored camera is
    // shot from 3.6 m up looking down, and neither of the two defects reported
    // against this tier is visible from there: the spray beads stack along a
    // grazing line of sight (6x the blob count, measured) and the traced
    // reflection's flat patches are Fresnel-gated and simply fade out from
    // above. Also in `GOLDEN`.
    "assets/test/plough_cinematic_low.loom",
    // The falling sheet — ADR 0054. Two of them, because the whole claim is
    // that one authored discharge moves the picture between them: `spout` is
    // past its break length for the last third of a 2 m drop and `cascade` is
    // nowhere near its own over 6 m. Both in `GOLDEN`.
    "assets/test/spout.loom",
    "assets/test/cascade.loom",
    // Drips — ADR 0054 §8. The only scene with a `DripSource`, and the only
    // one whose particles come from a raycast rather than from an emitter.
    "assets/test/dripping.loom",
    // The only scene where a rigid body is driven by the water rather than
    // only drawn against it, so it is the only one whose `render --sim` runs
    // the buoyancy solver at all.
    "assets/test/water_crate.loom",
    // The interactive ripple grid — ADR 0046. **Now in `GOLDEN` too**, on the
    // same rule that kept it out: it earns a reference on the commit that
    // uploads the grid to the surface, because that is the commit where it
    // becomes a rendering path. Until then the wake was felt by the buoyancy
    // solver and by `loom sim --assert` and drew nothing, so a reference would
    // have recorded a picture the feature does not touch.
    //
    // The `SCENES` row still earns its place, and for the opposite reason:
    // this pass renders with **no `--sim`**, so no grid is ever built and
    // nothing is uploaded. It is therefore the scene that proves a file
    // carrying a `[ripples]` table still parses, passes `check_ripples` and
    // draws its plain Gerstner surface — the null branch, which the `GOLDEN`
    // row at `--sim 200` never takes.
    "assets/test/wake.loom",
    // **The only scene in this repository that drives a floating hull**, and
    // the only one that can fire the shed path with a body large enough for
    // the source construction to matter — ADR 0063. A boat under power is the
    // one case where `wavelet::shed_source` is asked for a real answer rather
    // than for `None`, and until this file existed nothing had ever asked it.
    //
    // **Not in `GOLDEN` yet, and that is a deliberate gap with an owner.** It
    // earns a reference — a hull-shaped foam trail and a waterline band are
    // both rendering paths nothing else covers — but a `GOLDEN` row without a
    // blessed PNG makes an already-failing image gate fail differently, and
    // blessing one needs `cargo xtask image --bless`. Add the row and bless it
    // in the same commit, deliberately, reading the `MANIFEST.txt` diff.
    "assets/test/jib_vi_underway.loom",
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
    // The only scene whose sea throws spray: `WaterBody::spray` is zero
    // everywhere else, so `loom_water::spray` is never called and no gate would
    // ever look at a droplet. It is a CPU particle population derived from the
    // `fold` threshold rather than from an emitter, which nothing else here is.
    "assets/test/spindrift.loom",
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
    // And the only scene whose rain is stopped by *water*: `rainWaterGap` in
    // `rain_sim.slang` runs only where a scene has both a `WaterBody` and a
    // `Rain`, and this is the only one that puts the camera close enough to
    // see which surface a drop ended on.
    "assets/test/rain_pool.loom",
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
    // **The fishing demo's hub, and it was in no render gate at all.** It is
    // where the game is being built — a rig, a 43-tonne boat you board and
    // drive, a `WaterBody`, four supplies and a `Hud` — and until this line
    // `grep deeper_demo xtask/src/main.rs` returned nothing. Its load failures
    // were caught by the gameplay block in `scripts/green.sh`; a scene that had
    // stopped *drawing* was caught by nobody.
    //
    // **Here and not in `GOLDEN`, on the stated rule**: every rendering path it
    // uses already has a reference — `pool` and `wake` the water, `props` the
    // per-model atlases, `materials` the bindless maps, `lanternhead` the
    // secondary rays. What this line guards is that it still loads, bakes,
    // renders and validates clean, static and after 120 ticks of physics with a
    // dynamic hull floating on a water body, which is the combination nothing
    // else in this list has.
    "assets/games/deeper_demo.loom",
    // **The articulated player**, and it is a rendering path nothing else in
    // this list covers: 27 rigid mesh leaves on a 43-node chain whose
    // transforms are written every tick by fourteen node `Script`s off a gait
    // clock in a `GameRules`. `deeper_demo` above instances the same prefab, so
    // this line is not about the meshes — it is about the scripts running, the
    // prefab resolving from `assets/test/`, and `play.rs`'s `character_ancestor`
    // test keeping 27 `MeshRenderer`s out of the collision world.
    //
    // **In `SCENES` and not yet in `GOLDEN`**, which is a gap and not a
    // judgement: a `GOLDEN` row needs a blessed reference, and the round that
    // added this scene was not the one holding the `cargo xtask` lock. The
    // reasoning for a row is written and the row is owed:
    //     ("deckhand_walk", "assets/test/deckhand_walk.loom", &["--sim", "520"])
    "assets/test/deckhand_walk.loom",
    // The same character in a mirror — `metallic = 1.0, roughness = 0.0` on a
    // box, reflected by the one traced ray ADR 0019 already spends on every
    // opaque fragment. **It is the only scene in either list where a subject
    // and its own reflection are in one frame**, so it is what would catch a
    // regression that leaves the raster path right and the TLAS stale: an
    // animated puppet whose reflection stands still is exactly the failure
    // ADR 0062:85 says a `Deform`ed or skinned mesh would have by design.
    //
    // **Also owed a `GOLDEN` row and for the same reason** — no `cargo xtask`
    // lock this round:
    //     ("deckhand_mirror", "assets/test/deckhand_mirror.loom", &["--sim", "520"])
    "assets/test/deckhand_mirror.loom",
    // The only scene that runs several systems *at once*: voxel terrain, one
    // water body serving both a current and open ocean, three grass fields,
    // rain with wetness and shelter, additive and alpha particles, wind and an
    // authored environment, in one frame. Everything above it isolates one
    // path; this is the one that catches two paths interfering.
    // The chrome creature. **The only scene in either list with a deformed
    // mesh** (ADR 0062): its vertices are moved by a travelling wave in
    // `vertexMain`, which nothing else here exercises. It is also the only
    // mirror-metal subject in the repository, so it is what would catch a
    // reflection or a normal going wrong on a specular surface.
    "assets/test/gleamsprat.loom",
    // The same wave on an animal that is *going somewhere*: a plain node with
    // a `Script` writing its own transform, no `CharacterController` and no
    // rigid body. Shape and place are separate systems (ADR 0062) and this is
    // the only scene where both run on one node.
    "assets/test/gleamsprat_cruise.loom",
    "assets/test/homestead.loom",
    // The deck field: the first textured mesh in the project, and the first
    // node whose collider is transcribed rather than taken from drawn bounds.
    // If this row ever renders a flat grey plate, the OBJ did not load and the
    // renderer substituted a unit box — which looks exactly like success.
    "assets/test/rig_deck.loom",
    // The whole rig, assembled: deck, six piles, the tide seam, all fourteen
    // bulwark rail/cap runs, the shed and its roof, the mast, and both
    // bollards, in one 35-node scene — 32 of them renderable, none of them a
    // primitive. If any one of the 21 meshes or 12 textures this scene
    // references fails to load, `loom validate`'s `assets` array stops being
    // empty and this row is where that first shows up outside `rig_deck`'s
    // single mesh.
    "assets/test/rig_structure.loom",
    // The shed zone alone, close in: `rig_structure.loom`'s camera sits off
    // the north-west corner and the shed is a sliver at the frame's edge
    // there (see that row's comment), so this is the only scene where the
    // shed's board siding, the roof's corrugation and its two rusted-through
    // holes sit close enough to camera for a stray Vulkan validation message
    // about any of them to have somewhere to surface. Same five Task 4 nodes
    // as `rig_structure.loom` (`Shed`, `ShedRoof`, `Mast`, `BollardWest`,
    // `BollardEast`) with the same materials, plus `Deck` for footing so the
    // shed doesn't render as a diorama piece with no ground under it — its
    // own 8 nodes and 13 assets, with fresh UUIDs. Those UUIDs are the ONLY
    // thing not shared: all 13 `path` values are byte-identical to
    // `rig_structure.loom`'s, so this row is not independent evidence about
    // asset loading and must not be described as such — a corrupt
    // `rig_shed.obj` fails both rows the same way, for the same reason. What
    // it adds is a second CAMERA on the same files, close enough in that a
    // stray Vulkan validation message about the shed's siding or the roof's
    // holes has somewhere to surface. It is also in `GOLDEN` as `rig_shed`:
    // this array's
    // zero-validation-message render gate and that one's image-diff gate
    // both watch this scene and neither substitutes for the other —
    // `rig_structure` carries the establishing view of the whole assembly;
    // this scene carries both gates for the close view of the shed end.
    "assets/test/rig_shed.loom",
];

/// How many frames a windowed run draws before shutting itself down. Enough to
/// get past first-frame special cases and into the steady state, without
/// making the check slow.
const WINDOWED_FRAMES: u32 = 90;

fn main() -> std::process::ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();

    // Held for the whole run and released by `Drop` on the way out. Every task
    // below drives the engine over dozens of scenes and will use the whole
    // machine; see [`GateLock`] for what running two at once did.
    let _lock = match task.as_str() {
        "validate" | "image" | "flythrough" | "shimmer" | "repeat" | "ablate" => match GateLock::acquire(&task) {
            Ok(lock) => Some(lock),
            Err(e) => {
                eprintln!("{e}");
                return std::process::ExitCode::from(2);
            }
        },
        _ => None,
    };

    match task.as_str() {
        "validate" => validate(),
        "image" => image(std::env::args().any(|a| a == "--bless")),
        "flythrough" => flythrough(),
        "shimmer" => shimmer(),
        "repeat" => repeat(),
        "ablate" => ablate(),
        other => {
            eprintln!(
                "unknown task {other:?}\n\nUSAGE:\n    cargo xtask validate\n    \
                 cargo xtask image [--bless]\n    cargo xtask flythrough
    cargo xtask shimmer
    cargo xtask repeat
    cargo xtask ablate"
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
const GOLDEN: [(&str, &str, &[&str]); 60] = [
    // **The editor's sub-rectangle, which no other reference can see.** The
    // scene is `materials` deliberately — this entry is not about content, it
    // is about *where the content lands*: that the tonemap copies the scene to
    // the placement's origin rather than sampling outside what the forward pass
    // wrote, that the projection uses the rectangle's aspect and not the
    // image's, and that `chrome_clear` fills the rest with `ground` rather than
    // leaving uninitialised memory.
    //
    // It is the only gate on the editor's viewport at all. The window cannot be
    // photographed by anything here, so without this row the whole placement
    // path would ship on a human having looked at it once.
    ("viewport_rect", "assets/test/materials.loom", &["--viewport", "60,40,200,120"]),
    // **The scene the reflection bug was reported in, and it had no pixel gate
    // at all.** It sat in `SCENES` only, on the argument that `materials`
    // already sweeps metallic to 1.0 — which is true of the BRDF and false of
    // what a reflection *reads*: `stoneyard`'s ground is triplanar, so it is
    // the only reference covering a reflected hit that has no UVs and derives
    // its texture from world position. ADR 0021 shipped and ADR 0044 fixed a
    // visible defect here without one reference moving. Small at 320x200 —
    // the reflected band is a few dozen pixels — but a few dozen pixels is the
    // difference between a gate and no gate.
    ("stoneyard", "assets/test/stoneyard.loom", &[]),
    // **The post-process stack's standing shot** (ADR 0018). Nothing else in
    // this list is composed around what happens *above* diffuse white: the
    // flame's core, the brazier's pool, the sun's glitter path, wet stone at
    // `WET_SMOOTH` roughness under a low sun, and the sun disc are all past it.
    // 2400 ticks is 40 s - exactly `WET_COVER_WINDOW`, so `cover_recent`'s
    // three taps have a full window, and long enough for the dinghy to settle.
    ("lanternhead", "assets/test/lanternhead.loom", &["--sim", "2400"]),
    ("primitives", "assets/test/primitives.loom", &[]),
    // **The alpha test, which is a rendering path nothing else covers.** Every
    // other `discard` in this engine is in generated geometry — the grass
    // fade, the water shoreline, rain streaks — and none of them read a
    // texture's alpha channel. Without this the branch in `fragmentMain` could
    // be deleted and all 27 other references would still match.
    ("alpha_cutout", "assets/test/alpha_cutout.loom", &[]),
    // **Blended transparency, which is a second rendering path and a second
    // pipeline.** `alpha_cutout` covers discard; this covers the blend, the
    // no-depth-write, and the back-to-front sort that a blend cannot do for
    // itself. Three panes over a striped floor prove transmission, and two
    // overlapping angled panes prove the sort — swap the draw order and this
    // reference moves.
    ("glass", "assets/test/glass.loom", &[]),
    ("materials", "assets/test/materials.loom", &[]),
    ("cave", "assets/test/cave.loom", &[]),
    // **The only gate on `Environment.stages` there is or can be** — ADR 0069.
    // Every other scene in this list authors no stages, so all 54 of them pass
    // with the mood blend arbitrarily broken: a mid-stage `dread` reading as
    // its near endpoint, a patch key never reaching the shader, the grade never
    // leaving the identity. That is the "a gate that cannot see a feature
    // reports a full pass" failure this project has now had four times, and it
    // is why a new system arrives with a row rather than after one.
    //
    // **`dread` is authored at 0.65, and the value is the whole point.** It is
    // not a stage: it sits 0.30 of the way from `middle` to `far`, so this row
    // gates the *interpolation*. A row authored at either endpoint would pass
    // with the blend deleted outright.
    //
    // 1.80 s in a debug binary at 320x200 — measured, and cheaper than `cave`
    // beside it, because the scene is six nodes and the time is process
    // startup. `deeper_demo --sim 6000` is 69 s debug and four runs across
    // `image` and `repeat`, which is why the game is not the row.
    ("mood_deep", "assets/test/mood_deep.loom", &[]),
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
    // **The marched soot volume, which is a third rendering path for smoke and
    // the only scene that draws it.** `smoke` is the alpha sprite billboards,
    // `emberfall` is the GPU pool, `windy` is the wind coupling — and every one
    // of them would keep matching with `smokeColor` deleted, because a soot
    // volume is `flame = true` with `additive = false` and no other scene in
    // the repository authors that pair.
    //
    // It is also the only frame with an additive marched quad and an alpha
    // marched quad in it at once, so it is the only gate on the two being
    // sorted against each other.
    //
    // `--sim 200` for the same reason `campfire` uses it: the field rises with
    // the tick, and t = 0 is the one instant where the column has no history in
    // it.
    ("plume", "assets/test/plume.loom", &["--sim", "200"]),
    // **The containment gate on the marched region, and `plume` cannot be it.**
    // The volume the fragment marches is derived from `sootEnvelope`'s support
    // (`columnChord`); when the two drift apart the march stops at near-full
    // density and the silhouette becomes the primitive's analytic curve — which
    // is exactly what shipped, as an upright ellipsoid, for as long as the soot
    // volume has existed.
    //
    // At `plume`'s 3.2 m/s that failure is quiet: the ellipsoid still contained
    // the column's lower half, so it read as a slightly hard downwind edge with
    // ~89% of the authored lean clipped away, and every gate passed. Here the
    // wind is 9.0 m/s across the camera, the lean is ~2.3 plume radii and it is
    // entirely lateral on screen, so the same failure removes the top two thirds
    // of the column along a curve. That is the difference between a row that
    // catches the regression and a row that blesses it.
    ("plume_gale", "assets/test/plume_gale.loom", &["--sim", "200"]),
    // **A volume must not be depth-tested at a plane, and this is the only
    // frame in either gate that can tell.** The particle pipeline tests LESS
    // against the billboard's own plane, which runs through the marched hull's
    // centre; geometry nearer than that plane discarded every fragment of the
    // quad it covered, cutting the smoke dead straight along the geometry's
    // silhouette with no haze over its face at all.
    //
    // Every other GOLDEN row — `plume` and `plume_gale` included — renders
    // byte-identical with and without the depth write, because none of them has
    // anything crossing a plume's quad plane. A fix whose only gate cannot see
    // it is not gated, so the deck here is authored in front of the plane on
    // purpose.
    ("plume_roof", "assets/test/plume_roof.loom", &["--sim", "200"]),
    // **Which of the two draws the outline — the noise or the envelope?** No
    // other plume row can answer it. All three frame the column from 13 m or
    // more, where the rim that decides it is one or two pixels wide; here the
    // camera is 5.5 m from a 4.9 m column and the rim is ~40 px.
    //
    // It has been the wrong answer twice. The march clipped against an upright
    // ellipsoid until c335c2c, and after it the envelope still MULTIPLIED the
    // density, so the support ended exactly on `sootEnvelope`'s analytic cone and
    // the outer third of the radius was a uniform wash behind a dead-straight
    // line. Both times every plume row here was blessed on the broken picture and
    // the human found it by opening the viewer.
    //
    // Reverting `SMOKE_RIM` at the reference size, fraction of pixels moving more
    // than 4 levels:
    //
    //     plume 3.70%   plume_gale 1.76%   plume_roof 6.53%   plume_close 15.21%
    //
    // All four would fail, so this row is not the only one that can see THIS
    // change -- it is the only one where the rim is more than a pixel or two, so
    // it is the only one where a future change to the rim has anywhere to hide.
    // That is the same argument `plume_gale` makes about the lean.
    ("plume_close", "assets/test/plume_close.loom", &["--sim", "200"]),
    // **A traced reflection of scene geometry in water** (W3). Every other
    // water reference here looks at open sea or at a shoreline with nothing
    // standing beside it, so the reflection term is the analytic sky in all of
    // them and would keep being so with the ray deleted. This is a still pool
    // with a lit shed on its bank at a grazing angle, and the reflection is a
    // legible region rather than a few pixels.
    //
    // The lamp is on the bank deliberately: a reflection ray landing near a
    // point light is what ADR 0019 shipped `REFLECT_MAX_RADIANCE` for, and this
    // is the only scene that puts one where a water ray can find it.
    ("mirrorpool", "assets/test/mirrorpool.loom", &["--sim", "90"]),
    // **The GPU particle pool is its own rendering path** (ADR 0047), and it
    // is the only one whose instances the CPU never writes: a compute dispatch
    // fills a device-local buffer and the existing particle vertex shader
    // expands it. `smoke`, `explosion`, `windy`, `campfire` and the rest all
    // upload their instances from the host, so every one of them would keep
    // matching with the dispatch deleted.
    //
    // A 16,200-slot pool, about 12,000 of them live at once, against `smoke`'s
    // ~725 — the scale that justifies the path existing at all.
    //
    // `--sim 600` is ten seconds, past the four-second lifetime, so the
    // fountain is at its settled population. At tick zero the pool is empty,
    // which is the least representative frame there is.
    ("emberfall", "assets/test/emberfall.loom", &["--sim", "600"]),
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
    // **The whitecap trail, which no other reference can see** (W2, ADR 0049).
    // Foam that outlives the crest that made it is the difference between a
    // highlight welded to a wave and a sea with a memory, and every other
    // water scene here is blind to it: `ocean` looks at the horizon from 2.4 m
    // up, so a whitecap is a few pixels seen edge-on; `shore` and `beach` are
    // shallow, and shoaling flattens the swell, which is exactly where the
    // fold and every whitecap with it go to zero; `homestead` and `river` are
    // inland water in near-still air.
    //
    // Six metres up at 25 degrees over a storm sea running one way, so a patch
    // is legible sitting *behind* its crest rather than on it. **Stubbing the
    // trail (`FOAM_TRAIL_DECAY = 0.0`) moves 20.2% of this frame** against a
    // 0.1% tolerance — and reproduces every other water reference byte for
    // byte, which is the mutation proving the trail is what does the work.
    ("whitecaps", "assets/test/whitecaps.loom", &["--sim", "300"]),
    // **The FFT ocean, and no other reference draws one** — ADR 0076. Every other water
    // row here sums an authored or wind-derived Gerstner wave list; this is the only frame
    // in which `waterVertexMain` takes its cascade branch, samples the CPU's uploaded
    // tiles, and applies the per-cascade Nyquist fade. A numeric change in
    // `loom_ocean_at`, in `loom_sample_cascade`, in the tile order that
    // `Ocean::render_tiles` writes, or in `waterCascadeFade` moves this reference and
    // nothing else in this table.
    //
    // **`--sim 400` is the tick the whole slice was measured at** and it is not arbitrary:
    // the buoy is settled and riding (task 2 measured mean offset -0.091 m over ticks
    // 300-900, so anything before 300 is still the transient), `mu_max` at the buoy reads
    // 0.1024 there, and 0.93% of the surface is past `WATER_FOAM_WET` so there is visible
    // whitecap in frame. It is also the tick `ABLATE`'s row renders, so the two gates
    // photograph the same instant.
    //
    // **What this row is blind to.** The `ocean_fft` window draws a FLAT sea today —
    // `Viewer::set_ocean` has no caller, because its only possible one is
    // `crates/loom_cli/src/run.rs`, which is the human's uncommitted file (task-3 report
    // §7 has the block to paste). Every gate in this repository goes through the headless
    // path, so no reference and no diff here can see that; it shows up only when a human
    // opens a window.
    ("ocean_fft", "assets/test/ocean_fft.loom", &["--sim", "400"]),
    // **The foam field, and nothing else in this list can see it** — ADR 0055.
    // Every other water scene's foam is a closed form in `(x, t)`: the
    // instantaneous whitecap coverage and the ten-tap trail that unrolls it.
    // A hull's wake, an impact ring and what the current does to both are
    // stepped CPU state with memory, and `plough.loom` is the only scene that
    // deposits into it. At 240 ticks the crate has entered, ploughed, and the
    // patch it left has begun to break up into lace — which is the half of the
    // feature a fresh deposit cannot show.
    ("plough", "assets/test/plough.loom", &["--sim", "240"]),
    // **The falling sheet, and no other reference contains one** — ADR 0054.
    // The nappe is the only geometry in this engine generated from `SV_VertexID`
    // *past* another mesh in the same draw, and `nappeColor` is a branch no
    // other water scene ever takes. Adding a rendering path means adding a
    // scene here.
    //
    // **Two rows for one feature, because the feature is a discrimination.**
    // `spout` at `q = 0.10` breaks up at 1.31 m over a 2 m fall and opens into
    // strings; `cascade` at `q = 2.0` breaks up at 9.7 m over a 6 m fall and
    // never does. A single row would leave half the shader — `broken`, and
    // everything the break length feeds — uncovered, and the two pictures are
    // one authored number apart.
    //
    // `--sim 120` on both: the streaks are a function of the clock, so tick 0
    // is the one moment the noise field is unshifted, and a reference taken
    // there would agree with a broken scroll.
    //
    // **And they carry the mist bank**, which no other reference does either.
    // Stubbing it — `Cascade.mist = 0` — moves **4.93% of `cascade`** at
    // 1920x1080 against a 0.1% tolerance, checked rather than assumed on the
    // puddles precedent. The bank costs at most **+0.099 ms** of the water pass
    // over a dolly from 14 m in to inside it (14.3 m +0.030, 10.7 m +0.048,
    // 7.1 m +0.099, 3.5 m +0.055, inside +0.002), against a 0.8 ms budget.
    ("spout", "assets/test/spout.loom", &["--sim", "120"]),
    ("cascade", "assets/test/cascade.loom", &["--sim", "120"]),
    // **Drips, and no other reference has one** — ADR 0054 §8. Five sources at
    // different rates over a 1.1 m fall, so one frame catches drops forming,
    // falling and landing at once; a single source would make the row a lottery
    // on which tick the reference was taken at.
    //
    // `--sim 180` is three seconds, which is past the slowest source's first
    // drop and inside every source's steady cycle. There is nothing to warm up:
    // `loom_water::drip` is a pure function of the clock.
    ("dripping", "assets/test/dripping.loom", &["--sim", "180"]),
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
    // **Rain stopped by water rather than by a solid, which nothing else in
    // this list covers.** Every rain scene before this one collided its drops
    // against the baked collision world alone, so a drop over a pool fell
    // straight through the surface and splashed on the bed a metre down; this
    // is the only frame where that difference is legible, because it is the
    // only rain scene whose camera is close enough to the water to see where a
    // streak ends.
    //
    // Stubbing the water test moves 14% of this frame [measured at 960x600].
    // It is also the only reference that would catch `LoomWaveSet` drifting
    // out of step between `EnvironmentData` and `RainWater` — two descriptions
    // of one memory layout, which is the class of defect this repository has
    // paid for repeatedly.
    //
    // `--sim 300` is five seconds: past the drop field's four-second settling
    // time, and long enough for the splash ring to be full.
    ("rain_pool", "assets/test/rain_pool.loom", &["--sim", "300"]),
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
    // **The most silhouette in the repository, and it had no pixel gate.**
    // 8,386 needle clusters: the scene whose 2x2 fragment quads most often
    // straddle an edge, which is exactly what `ambientVisibility`'s quad share
    // is weakest at. Nothing else in this list can see a share that has
    // started costing more than it buys — `forest` above is scattered trunks
    // and `tree_layered` is a handful of cards.
    //
    // It earns the row on its own numbers, not on the hypothetical: against a
    // 64-ray render at 1920x1080 the unshared eight-ray build reads a
    // worst-channel error of **120**, past this gate's own 72, and the shared
    // four-ray one reads 68. The share is doing more for this scene than for
    // any other, and that is the reverse of what the objection predicted.
    //
    // **The acceptance test for the share is the resolution ladder, and it has
    // to be re-run per new scene rather than assumed.** Bleed is a perimeter
    // effect, so its share of the frame's error must FALL as resolution rises.
    // Mean error of the shared build minus the unshared one, vs a 64-ray
    // reference, one row per scene:
    //
    //             320x200   640x400   960x600   1920x1080
    //   cave      +.0108    -.0005    -.0025    -.0037
    //   materials +.0003    -.0019    -.0022    -.0022
    //   stoneyard +.0169    +.0028    -.0059    -.0146
    //   croft     +.0071    -.0026    -.0054    -.0090
    //   spruce    +.0034    -.0009    -.0016    -.0031
    //   forest    -.0039    -.0188    -.0232    -.0288
    //
    // Monotone on every row. **Note which end this gate sits at**: at
    // GOLDEN_SIZE the share is worse than the unshared build on six of eight
    // scenes and better on seven of eight at 1080p, which is where the human
    // judges and where the viewer runs. So from here this gate certifies
    // "nothing turned blocky", not "the picture got better" — and if a row's
    // ladder ever stops being monotone, the filter has become too wide for the
    // frame and that is the signal to act on.
    //
    // **That ladder is only half the acceptance test, and it is the half that
    // cannot see the failure it is named after.** It measures distance from
    // truth, which falls with resolution. Quad *structure* — the shading going
    // constant across a 2x2, which is what "turned blocky" actually means —
    // **rises** with resolution, and no instrument in the repository reports
    // it. `loom flicker` reads 0.00000 by construction, because the pattern is
    // screen-locked and perfectly still.
    //
    // The number that does see it is R = the mean absolute difference across a
    // quad boundary divided by the same inside a quad, per axis. An unquantised
    // image sits at 1.0; quad-constant shading pushes it up. `primitives` at
    // 1920x1080, Rx / Ry:
    //
    //   converged        1.022 / 0.996      <- the control
    //   8 rays / lane    1.243 / 1.148
    //   4 = the default  1.357 / 1.217
    //   1 ray  / lane    1.848 / 1.532      <- visible as speckle
    //
    // **And `spruce` — this row, added expressly to watch the share — cannot
    // see it at all**: 1.001 against the converged build's 0.999, identical to
    // three decimals, because a frame that is mostly silhouette has no flat
    // region for a 2x2 to go constant across. `primitives` and `forest` (1.234
    // against 1.005) are the rows that carry this half. Keep both scenes, and
    // read R on `primitives` when the share or the ray count next moves.
    ("spruce", "assets/test/spruce.loom", &[]),
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
    // **The interactive ripple grid displacing the surface — W6, ADR 0046.**
    // The only reference where the water's height comes from *stepped CPU
    // state* rather than from a closed form in `(x, z, t)`, so it is the only
    // one that can catch the upload being dropped. That is not hypothetical:
    // `Renderer::set_ripples` and the shader's `loom_ripple_at` both shipped a
    // commit before anything called them, and the wake was felt by buoyancy
    // and by `--assert` while the surface drew dead flat.
    //
    // `--sim 200` is chosen from the diff against a build with the upload
    // removed: 2.8% of pixels at tick 45 (the buoy alone, settling), **26.6%
    // at 200** with the crate's ring across the whole domain, 4.8% at 900 once
    // it has decayed. 200 is where the feature is largest on screen, which is
    // what a reference wants.
    ("wake", "assets/test/wake.loom", &["--sim", "200"]),
    // **Particles at the waterline.** Water and the blended particle pass are
    // drawn in the same block today; anything that splits that block moves
    // their ordering, and this is the only reference that would show it.
    ("splash", "assets/test/splash.loom", &["--sim", "120"]),
    // **The impact crown — W9, and the reason this scene's exclusion is now
    // obsolete.** `pool.loom` sat in `SCENES` and not here on the stated
    // grounds that it "adds no rendering path" and that what it is *for* is a
    // sequence a still cannot hold. The first half was true only because the
    // feature did not exist: `loom_water::spray` was driven entirely by
    // `WaterSample::fold`, so a dead-calm pool threw nothing and there was
    // nothing in the still to protect.
    //
    // There is now. The crown is the only closed-form particle population
    // driven by an *event* rather than by a field — `spindrift` above covers
    // the crest half and cannot cover this one — and it is what would catch
    // the trigger silently reverting to the hysteretic `submerged` flag, which
    // on this scene never fires at all.
    //
    // `--sim 50` is six ticks after the entry at 44, with all three bands up
    // and the sphere at the waterline: **224 droplets, against 0 at tick 41**
    // [re-measured 2026-08-18 off the renderer's own `particles` count].
    //
    // The two numbers this line used to carry were both wrong. It said "24
    // droplets", which is off by a factor of ten, and "0 by tick 80", which
    // was true when it was written and stopped being true the moment the jet
    // landed: the jet and its satellites are alive from tick 58 to ~101, so
    // tick 80 is 124. The crown is what tick 50 photographs and it is gone by
    // 76 — the population after that is the `pool_jet` row's subject, not this
    // one's.
    ("pool", "assets/test/pool.loom", &["--sim", "50"]),
    // **The cinematic tier, and it is the only scene outside the deterministic
    // one** — ADR 0053, solver ADR 0057. Volumetric APIC water in a bounded
    // tank, a pillar it breaks against, and a ball that pushes it and is pushed
    // back. It covers a rendering path nothing else does: the fluid's own
    // particles, written as `ParticleInstance`s by a compute pass and drawn as
    // velocity-stretched droplets.
    //
    // `--sim 150` is 1.4 s after the ball's entry, with the slosh piled against
    // the far side of the pillar and a trough behind it — a still at 60 has the
    // impact but a flat far end, and one at 300 has settled.
    //
    // **It must never enter `DETERMINISM_SCENES` or the pinned-hash tests.** It
    // is reproducible on this device, driver and dispatch order and nowhere
    // else, which is exactly what `cargo xtask repeat` checks and exactly what
    // a pinned hash would claim wrongly.
    ("slosh", "assets/test/slosh.loom", &["--sim", "150"]),
    // **The cinematic free surface, and the two scenes it exists for** — ADR
    // 0057 addendum. Both are marching-cubes meshes shaded by
    // `waterFragmentMain`, so they cover a rendering path nothing else does:
    // refraction, absorption and a traced reflection over geometry that came
    // out of a fluid solver rather than out of a closed form.
    //
    // `ribbon` at 180 is 3 s after the spout starts and 1.5 s after the stream
    // reaches the pool — the fall is fully established and the pool has not yet
    // finished filling in behind it. Both 60 and 400 show the same connected
    // sheet; 180 is chosen because it is where the impact churn is widest.
    //
    // **`plough_cinematic` at 110 and not at 300, and the tick is load
    // bearing.** The hull enters at about 60, ploughs to a stop by 120 and has
    // settled by 150; 110 is the one frame with all three of the reads the tier
    // exists for in it at once — water parted round the bow, a hollow behind it
    // with the tiled bed visible through the trough, and connected sheets
    // thrown off the entry. At 300 it is a still pool with foam on it, which
    // `plough.loom` can already draw, and it costs 13 seconds to render because
    // the projection has compressed by then (ADR 0057 failure 3, still open —
    // it was 83 seconds until the bucket sort stopped being O(k²)).
    //
    // **Neither may enter `DETERMINISM_SCENES` or the pinned-hash tests** — ADR
    // 0053 §3. `cargo xtask repeat` is the gate that applies, and both are
    // byte-identical across three fresh processes at these ticks.
    ("ribbon", "assets/test/ribbon.loom", &["--sim", "180"]),
    ("plough_cinematic", "assets/test/plough_cinematic.loom", &["--sim", "110"]),
    // **The low camera, and it is the one row in this list watching the water
    // from where a human stands.** Both of the defects the tier shipped with —
    // a carpet of pale blue beads and a flat straight-edged reflection patch —
    // are visible here and effectively invisible from the row above it, which
    // is why they survived five green gates. That is the shimmer harness's
    // failure a third time: an instrument framing the composite rather than the
    // subject reports a pass having never looked.
    //
    // At 250 rather than 110: the beads are what this row is for, and their
    // population *grows as the water calms*, so the settled frame is the
    // stress case rather than the impact.
    ("plough_cinematic_low", "assets/test/plough_cinematic_low.loom", &["--sim", "250"]),
    // **The Worthington jet, which does not exist at tick 50 and never could.**
    //
    // A splash is a sequence and Loom used to fire all of it at t = 0; the jet
    // is driven by the *cavity closing*, so it launches `sqrt(R/g)` after the
    // entry — 13.6 ticks at a 0.5 m sphere — and reaches its apex at 0.59 s,
    // half a second after the rim. The `pool` row above photographs the crown
    // at its widest; this one photographs the jet and the satellites pinched
    // off its tip, and **neither frame contains the other population**.
    //
    // **This row pointed at `pool.loom` for a slice and photographed nothing.**
    // The tick window was right and the subject was never in it: `pool.loom`'s
    // sphere floats, so it sits on the axis the jet comes up, and the jet's
    // 0.625 m apex is *below* the top of the 0.5 m ball standing on it at every
    // tick of the jet's life. Three judges swept 52-84 at up to 1920x1200 and
    // found two satellites clearing the ball's crown. That is the same failure
    // as grass — a gate reporting a full pass over an absent subject — and it
    // is why the row now has a scene of its own: `pool_jet.loom` is `pool.loom`
    // with a stone instead of a float, so the body is a metre down by the time
    // the jet leaves and there is nothing standing where it comes out. The
    // scene's header carries the measurements.
    //
    // `--sim 74` is 30 ticks after the entry at 44 and 16 after the jet
    // launched: the tip is 0.59 m up against the crown's 0.28 m ceiling and
    // 95% of its own apex, the crown has landed so the frame holds the jet
    // alone, and the satellites are fanning off the tip. Later is taller — the
    // apex is 78 — but by then the foam collar has gone hard-edged (defect 7)
    // and takes over the frame [measured, at `GOLDEN_SIZE`].
    ("pool_jet", "assets/test/pool_jet.loom", &["--sim", "74"]),
    // **The spray off a breaking crest — W5.** The population is a closed form
    // over `WaterSample::fold`, the same quantity the whitecaps are painted
    // from, so this reference is what would catch the two drifting apart:
    // droplets in the air over water that is not white, or foam with nothing
    // leaving it. `--sim 240` is four seconds in, well past the first crest.
    ("spindrift", "assets/test/spindrift.loom", &["--sim", "240"]),
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
    // **The only deformed mesh in either gate** — ADR 0062. `Deform` moves
    // vertices in `vertexMain` and nothing else in this list does, so with this
    // row absent the whole animation path renders unwatched, which is exactly
    // how grass shipped two slices before anyone noticed.
    //
    // **Two rows, 126 ticks apart, and the gap is the whole design.** A single
    // reference pins the *shape* of the wave and says nothing at all about its
    // *rate*: `weather.z` is `--sim N / 60` here and a free-running wall clock
    // in the viewer, so a frequency that drifts is invisible to the human's
    // window and invisible to any one still. 1% off 3 Hz is 1.3 degrees of
    // phase at tick 7 and 24 degrees at tick 133.
    //
    // Measured, at this size and against this tolerance — every frequency in
    // the scene multiplied by 1.01:
    //
    //     tick   7    fraction 0.0045   worst  85     (tolerance 0.001 / 72)
    //     tick 133    fraction 0.0105   worst 171
    //
    // So the far row is worth having and the near one is **not** blind: it
    // catches a 1% error four times over. This comment used to claim the
    // anchor "would not be caught", which was asserted rather than measured
    // and is wrong. The honest statement is that the far row is about twice as
    // sensitive to a rate error, and that a *smaller* drift than 1% is where
    // the gap starts to matter.
    //
    // Neither row is at tick 0, which is deliberate for the reason `ocean` and
    // `rain_overhang` give: t = 0 is the one instant where every phase term is
    // exactly zero and the wave is flat, which is the least representative
    // frame the effect has.
    ("gleamsprat", "assets/test/gleamsprat.loom", &["--sim", "7"]),
    ("gleamsprat_beat", "assets/test/gleamsprat.loom", &["--sim", "133"]),
    // **A deform on a node that is moving**, which the two rows above cannot
    // cover: the wave is authored in the node's own frame and the node's frame
    // is being rewritten every tick by a `rhai` script. It is also the only
    // frame in either gate carrying a plain-node transform script — every other
    // `Script` in the repository sits on a `CharacterController`.
    ("gleamsprat_cruise", "assets/test/gleamsprat_cruise.loom", &["--sim", "60"]),
    ("homestead", "assets/test/homestead.loom", &["--sim", "1800"]),
    // The only picture of a UV-mapped, normal-mapped imported mesh anywhere in
    // the library. Delete the albedo_map and this row still draws timber-shaped
    // geometry at the right height — so what it actually guards is the TEXTURE
    // reaching the surface, which nothing else in GOLDEN can see. Static scene,
    // measured flat across ticks 0/60/300, so --sim adds nothing.
    //
    // Fault-injected two ways, both at this size against this tolerance
    // (0.001 fraction / 72 worst):
    //
    //     albedo_map deleted        fraction 0.5141   worst 221   (mesh loads, surface goes flat)
    //     mesh UV V un-flipped      fraction 0.2857   worst  31   (albedo/normal sample the wrong texel)
    //
    // Both move roughly a third to a half of the frame, so the row is not
    // blind to either the texture path or the UV winding it depends on.
    ("rig_deck", "assets/test/rig_deck.loom", &[]),
    // **The only picture of all four zones in one frame.** Tasks 3b, 4 and 5
    // each proved their own piece in isolation (`rig_deck`, and the
    // substructure/bulwark scenes this task's brief points at); this is where
    // deck, piles, seam and bulwark share a camera for the first time, which
    // is the only place a materials mismatch between them could show up.
    // Camera is low off the north-west corner so the deck surface, the
    // bulwark above it, the piles below and the seam where they cross the
    // water are all in frame at once — see the task report for the judged
    // renders.
    //
    // **`--sim` is pinned to `0`, not omitted, and that took proving.** The
    // brief's assumption was that the structure is static so every tick
    // renders the same frame; the mesh geometry does (rows 0-81 of the
    // 320x200 frame — sky, deck, rail, cap — are bit-identical at `--sim`
    // 0/60/300, and direct pixel sampling on the pile columns below that
    // confirms the piles and seams are too). The *picture* is not: this
    // scene's `WaterBody`/`Wind` (authored so the seam has a tide to line)
    // puts a live sea across roughly the bottom 60% of frame, and comparing
    // whole PNGs across ticks reads fraction 0.4414 (t=0 vs 60) / 0.4613 (t=0
    // vs 300) purely from wave motion and its reflections — not from anything
    // this row is meant to catch. `--sim 0` pins one deterministic frame
    // rather than asserting an invariance the water breaks.
    //
    // Fault-injected two ways, both at this size against this tolerance
    // (0.001 fraction / 72 worst):
    //
    //     all 6 Seam nodes deleted        fraction 0.0031   worst 36   (small but past tolerance — caught)
    //     all 6 Pile normal_maps deleted  fraction 0.0000   worst  1   (invisible at this size — NOT caught)
    //
    // Say so plainly: at `GOLDEN_SIZE` and this camera's distance, a missing
    // seam is a real, if modest, signal — six colour patches a few pixels
    // each, but consistent enough across a static frame to clear the default
    // tolerance. A missing pile normal map is not; the piles are too small
    // and too far from camera for its lighting contribution to survive
    // compression to 320x200. This row therefore guards the seam's *presence*
    // and the whole assembly's *asset paths* (an empty `assets` array on
    // `validate`), not the substructure's normal-map path — that would need a
    // closer frame or a dedicated row.
    ("rig_structure", "assets/test/rig_structure.loom", &["--sim", "0"]),
    // **The only close picture of the shed and its roof.**
    // `rig_structure`'s camera is off the north-west corner and
    // the shed is a sliver at the frame's edge there — see that row's own
    // comment. `loom render` draws exactly one camera per scene
    // (`World::active_camera` takes the first `Camera` node and there is no
    // flag to name another), so a second view means a second scene file —
    // the same choice `rig_deck.loom` already made rather than reframing
    // `rig_structure.loom` for one zone. This one
    // (`assets/test/rig_shed.loom`) stands on the deck at y 3.70 and looks
    // SOUTH-WEST — forward (-0.812, -0.068, +0.579) — at the shed's north
    // wall and its EAST gable, the wall carrying the door. Not south-east,
    // and not the west gable: the camera is at x 6.8 and the shed spans
    // x -8.500..0.500.
    //
    // **The mast and the bollards are not in this frame.** At `GOLDEN_SIZE`
    // the horizontal half-angle is 37.97 deg (atan(tan(26 deg) * 1.6)) and
    // `Mast` sits 63.98 deg off the camera's facing, `BollardEast` 47.72,
    // `BollardWest` 44.76; projected, they land at pixel x -260, 381 and 361
    // of a 0..320 frame. No row in this table pictures them.
    //
    // Fault-injected two ways, both at this size against this tolerance
    // (0.001 fraction / 72 worst):
    //
    //     Shed node deleted              fraction 0.0906   worst 209   (the whole shed drops out)
    //     ShedRoof's albedo_map deleted  fraction 0.0139   worst 188   (roof goes flat grey)
    //     ShedBearers node deleted       fraction 0.0045   worst  71   (see below)
    //
    // Re-measured 2026-08-27 after Phase 2b rebuilt the shed as lapped
    // weatherboard on a corrugated roof and split the bearers into their own
    // node. The first two moved (0.0959 -> 0.0906, 0.0152 -> 0.0139) because
    // the shed is a different shape, not because the row got weaker; the
    // control — the same scene rendered twice — is still 0 differing pixels.
    //
    // The first two move far past tolerance, so this row is not blind to
    // either the shed's presence or the roof's texture path.
    //
    // **`ShedBearers` is caught by the FRACTION and not by `worst`.** Its
    // worst channel is 71 against this gate's 72 — under it by one. The
    // bearers are 150 mm of timber behind the shed's own sill line and only a
    // few pixels of them are lit at this size, so a `worst`-only gate would
    // be blind to them; 0.0045 against a 0.001 fraction is what catches it.
    // Do not raise the fraction tolerance on this row without re-measuring
    // this number. It says NOTHING about the
    // mast or the bollards — not "little", nothing: they are outside the
    // frustum by the angles above, so deleting either mesh cannot change one
    // pixel of this render and there is no tolerance at which it would. A
    // missing mast or bollard mesh trips `validate`'s `assets` array
    // instead, which is the only gate that covers them.
    ("rig_shed", "assets/test/rig_shed.loom", &[]),
];

/// Every reference renders at this size.
const GOLDEN_SIZE: &str = "320x200";

/// What each ablatable effect is measured on, and how much of the frame it must move.
///
/// `(label, scene path, LOOM_ABLATE value, extra render args, minimum fraction of pixels
/// that must change)` — the same shape `GOLDEN` uses for the scene path and its extra
/// args, so that adding an effect really is "a row" the way both tables' doc comments
/// claim: the scene need not live under `assets/test/` and the tick need not be `400`.
///
/// **This table is hand-kept and deliberately not shared with `loom_render`'s own
/// registry.** `xtask` links nothing from the engine — a gate that shares code with the
/// thing it checks can be fooled by the same bug twice — so adding an effect means adding
/// a row here, a row to `ABLATIONS` in `loom_render::ablate`, and a matching
/// `LOOM_ABLATE_<NAME>` constant in `assets/shaders/scene.slang` (see the warnings on both
/// of those). The floor is set from a measured run and written down with the row, never
/// guessed: too low and the check passes on a feature that has almost stopped drawing,
/// which is the exact failure it exists to catch.
///
/// The scene is chosen to be the one where the effect is *loudest*. `whitecaps.loom`
/// measures `foam.coverage = 0.954` at the origin, which is why foam is measured there
/// and not on `ocean`.
///
/// Measured 2026-08-28 at `GOLDEN_SIZE` (this is what `ablate()` actually renders — the
/// 0.2752 figure recorded when `LOOM_ABLATE_WATER_FOAM` first worked was taken at 640x400
/// and does not transfer): `loom render whitecaps.loom --sim 400 --size 320x200` with and
/// without `LOOM_ABLATE=water_foam`, then `loom compare`, gives `fraction = 0.29178125`,
/// reproducibly (byte-identical `with` renders across two runs). 0.15 is roughly half of
/// that — comfortably clear of run-to-run noise, and nowhere near the 0.0 a fully dead
/// effect would score.
///
/// **Re-measured 2026-08-29 at 0.19717**, still comfortably over the floor. It has drifted
/// because `whitecaps`' water has changed twice since — the foam commits, and the backlit
/// term's mask below. The floor is deliberately not re-tightened: this column measures
/// "is it drawing", not "is it drawing exactly what it drew in August".
///
/// **Re-measured 2026-08-30 at 0.25541**, the largest it has been. `foamAge` was reading a
/// 0.33-second-old trail as ten-second drained foam and painting nine tenths of a Gerstner
/// sea `WATER_FOAM_OLD_ALBEDO` at 0.55 opacity; with the age taken off the trail's own
/// decay envelope instead, foam is white where it is fresh and the row moved 0.19505 ->
/// 0.25541. The floor stays at 0.15 for the same reason it stayed after the drift down.
///
/// ## **Re-measured 2026-08-30 at 0.13898, and THE FLOOR MOVED 0.15 -> 0.07**
///
/// Read this before trusting any row above it: **every figure from 0.29178 down to
/// 0.25541 was measured while the vertex foam trail was seeded from `WATER_FOAM_WET`**
/// — it remembered a crest that merely got *wet* rather than one that *broke*, so
/// `in.foamHist` came back above 0.05 on 100% of `lucent`'s water at a mean of 0.344 and
/// the near-binary erosion threshold painted half the sea in 6x-stretched lace. The human
/// flew a viewer 1.4 m over the water and called it shredded; the ablated frame is a
/// clean sea. So this column has been reading "how much of the frame is lace" and the
/// floor derived from it inherited that.
///
/// Measured on this tree, `loom render assets/test/whitecaps.loom --sim 400 --size
/// 320x200` with and without `LOOM_ABLATE=water_foam`, then `loom compare`:
///
/// ```text
/// trail seeded from            row      note
/// WATER_FOAM_WET  / _BREAK    0.25541  the shipped defect
/// WATER_FOAM_BREAK/ _BROKEN   0.09511  the fix, at the old FOAM_TRAIL_DECAY = 0.858
/// WATER_FOAM_BREAK/ _BROKEN   0.13898  and at the derived 0.9268                 <- this
/// ```
///
/// **0.07 is roughly half of 0.13898**, which is the rule every other floor in this table
/// was set by, applied to the first measurement of this row taken on a trail that is not
/// smearing. The gate keeps its meaning — a severed `LOOM_ABLATE_WATER_FOAM` still scores
/// 0.000 and reports `DRAWING NOTHING`, which is the failure it exists for — and 13.9% of
/// a frame is emphatically an effect that draws. **Lowering a floor to pass a change is
/// the wrong move nine times in ten; the tenth is when the number the floor was derived
/// from was measuring the defect, and that is the claim being made here.**
///
/// **`ocean_fft` / `ocean_spectrum`.** Measured 2026-08-29 at `GOLDEN_SIZE`:
/// `loom render assets/test/ocean_fft.loom --sim 400 --size 320x200` with and without
/// `LOOM_ABLATE=ocean_spectrum`, then `loom compare`, gives `fraction = 0.587546875`
/// (`differing` 37603 of 64000, `mean` 8.5647, `worst` 220). Reproducible: both sides are
/// byte-identical across two runs. 0.30 is roughly half of that, on the same rule the row
/// above uses.
///
/// **It is over half the frame because ablating the cascade removes the sea, not a
/// highlight on it.** `ocean_fft` is the only scene that asks for `wave_model =
/// "spectrum"`, a spectrum body is refused an authored wave list at load, and the shader's
/// model branch is on `sea.count` — so zeroing the count drops it into the Gerstner sum
/// with nothing to sum and the surface goes flat. That is the honest measurement of "is
/// this rendering path drawing", which is the question this table asks.
///
/// **The bit was cross-checked against the other two sites rather than assumed**, because
/// nothing in the build does it: `LOOM_ABLATE=ocean_spectrum` on `whitecaps.loom` moves
/// **0 pixels of 64000** (a Gerstner scene has no cascade, so the correct answer is
/// nothing), and `LOOM_ABLATE=water_foam` still moves exactly the 0.29178125 recorded
/// above. A Slang constant that had collided with foam's bit would have shown up as a
/// large number in the first of those and a changed one in the second.
///
/// **`lucent` / `water_glow`.** The backlit subsurface term, measured at `GOLDEN_SIZE`:
/// `loom render assets/test/lucent.loom --sim 300 --size 320x200` with and without
/// `LOOM_ABLATE=water_glow`, then `loom compare`, gives `fraction = 0.106015625`
/// (`differing` 6785 of 64000, `mean` 0.7672, `worst` 76). Reproducible: both sides
/// byte-identical across two runs. 0.05 is roughly half of that, on the same rule the two
/// rows above use.
///
/// **Read the `fraction` this table uses at `compare`'s DEFAULT tolerance**, which is what
/// `ablate` below invokes — `channel = 2`, so a pixel that moved one or two levels is not
/// counted. At zero tolerance the same pair of renders differs across **15.49%** of the
/// frame. Only the first belongs in this column; a floor set from the second would have
/// failed a working effect, and did, once.
///
/// **This row was `ocean_tropical` and it moved here, because the term's gate is now a
/// fact about the scene's geometry.** The backlit path is Lambert's cosine on the face the
/// light enters, so it is non-zero only where a visible face is tilted further from the
/// vertical than the sun is above the horizon. `ocean_tropical`'s sun is 38.3° up and its
/// steepest resolved face is 9.6°, so almost nothing on it can be backlit at all: it
/// measured **0.578%** against a floor of 0.28%, a margin too thin to be a gate.
/// `lucent.loom` is authored for exactly this term — a 6.9° sun over a sea whose faces
/// reach 39.4° — which is what an ablation row is supposed to be pointed at: the scene
/// where the effect is loudest.
const ABLATE: [(&str, &str, &str, &[&str], f64); 3] = [
    (
        "whitecaps",
        "assets/test/whitecaps.loom",
        "water_foam",
        &["--sim", "400"],
        0.07,
    ),
    (
        "ocean_fft",
        "assets/test/ocean_fft.loom",
        "ocean_spectrum",
        &["--sim", "400"],
        0.30,
    ),
    (
        "lucent",
        "assets/test/lucent.loom",
        "water_glow",
        &["--sim", "300"],
        0.05,
    ),
];

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

    let loom = match build_release(&root) {
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

    let filter = only_filter();
    let mut skipped = 0_usize;
    let mut failures = Vec::new();
    let mut blessed = Vec::new();
    let mut checked = 0;

    for (name, scene, extra) in GOLDEN {
        if !selected(filter.as_ref(), name, scene) {
            skipped += 1;
            continue;
        }
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
    if skipped > 0 {
        println!("  (--only: {skipped} row(s) skipped — this run did not cover them)");
    }
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
    let loom = match build_release(&root) {
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
    let loom = match build_release(&root) {
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

    let filter = only_filter();
    let mut skipped = 0_usize;

    // **Every job writes its own PNG.** All eighty scenes used to render to one
    // `target/xtask-validate.png`; serially that was merely wasteful, and in
    // parallel it is a race. The index is the scene's position in `SCENES`, so
    // a leftover file names the scene that wrote it.
    let scratch = root.join("target/xtask-validate");
    let _ = std::fs::create_dir_all(&scratch);

    let wanted: Vec<(usize, &str)> = SCENES
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, scene)| {
            let keep = selected(filter.as_ref(), scene, scene);
            if !keep {
                skipped += 1;
            }
            keep
        })
        .collect();

    let jobs: Vec<_> = wanted
        .into_iter()
        .map(|(index, scene)| {
            let loom = loom.clone();
            let root = root.clone();
            let out = scratch.join(format!("{index:03}.png"));
            move || -> (u32, Vec<String>) {
                let mut failures = Vec::new();
                // Missing means unverified, not fine — see the same guard in `image`.
                if !root.join(scene).exists() {
                    failures.push(format!(
                        "{scene}: missing — the scene list names a file that does not exist"
                    ));
                    return (0, failures);
                }
                let out = out.to_str().unwrap_or("out.png");
                let result = run(&loom, &root, &["render", scene, "--out", out]);
                collect(&mut failures, &format!("render {scene}"), &result);

                // `--sim` runs physics before drawing, which is a different set of
                // buffer writes than a static render.
                let result =
                    run(&loom, &root, &["render", scene, "--sim", "120", "--out", out]);
                collect(&mut failures, &format!("render --sim {scene}"), &result);
                (1, failures)
            }
        })
        .collect();

    for (ran, mut found) in in_parallel(gate_jobs(), jobs) {
        checked += ran;
        failures.append(&mut found);
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
            // **The biggest world in the library — 153.6 m across — and the
            // only one whose cost is a voxel bake rather than a per-frame
            // system.** `lanternhead` above measures 32 chunks; this one is
            // also 32, at 1.2 m voxels instead of 0.4, and the first draft of
            // it was 384 chunks and 220.7 ms/frame. Nothing else here would
            // have noticed: it renders correctly, validates clean, and matches
            // no reference because it is deliberately not in `GOLDEN`. The
            // headroom is the point — a heightfield's early-out is disabled by
            // its own Lipschitz constant, so bake cost is linear in the chunk
            // count and a scene of this size is one `chunks` edit away from
            // being eight times over budget.
            "assets/test/mountain_pass.loom",
            // **`wake`, for the buffer the window allocates itself.** The
            // viewer keeps its own ripple buffer and its own `set_ripples`,
            // exactly as it does for the terrain grid `shore` is here to
            // cover — and the two are worse than the terrain case, because
            // this one is written *every frame* from stepped CPU state. A null
            // or stale device address there is a fault on the first wave and
            // no headless gate can reach it: `--frames` alone runs a paused
            // editor, which builds no grid at all, so only `--play` takes the
            // branch. Measured 0.735 ms/frame debug, well inside the budget.
            "assets/test/wake.loom",
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
    if skipped > 0 {
        println!("  (--only: {skipped} row(s) skipped — this run did not cover them)");
    }
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
    // **This list may never gain a cinematic scene** — ADR 0053 §3. Debug and
    // release disagreeing is the fault this check exists to find; a cinematic
    // body is a GPU dispatch neither profile computes, so it would agree for a
    // reason that has nothing to do with the property, and the check would go
    // on printing a pass while measuring nothing. One grep, which is what the
    // ADR says the containment costs.
    for scene in DETERMINISM_SCENES {
        let text = std::fs::read_to_string(root.join(scene)).unwrap_or_default();
        if text.contains("simulation = \"cinematic\"") {
            return Err(format!(
                "determinism\n  {scene} is in the cinematic tier and is in \
                 DETERMINISM_SCENES\n  ADR 0053 §3: it is reproducible on this \
                 device alone, so debug and release agreeing about it proves \
                 nothing. Remove it from the list, or take the scene back to \
                 `simulation = \"deterministic\"`."
            ));
        }
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

/// **Three fresh processes, byte-compared.** ADR 0045 clause 3.
///
/// ADR 0017 claimed the raindrop buffer was "verified byte-identical across
/// three processes", and it was — once, by hand, on one RTX 4090. That claim is
/// the entire licence for GPU-stateful rendering in this project, and it rests
/// on assumptions that are not guaranteed: that additive blending is
/// order-independent (float addition is not associative), that nothing in the
/// dispatch reads a counter another thread is writing, and that seeding is a
/// pure function of the scene. A hand check does not survive the next stateful
/// path, and the VFX overhaul queues several.
///
/// **Every scene in `GOLDEN`, not a list of the stateful ones.** A hand-kept
/// list of "the scenes with state in them" is the same artefact `SCENES` and
/// `GOLDEN` are, and it has gone stale three times: the gate reports a full
/// pass having never looked at the thing under test. Deriving it from `GOLDEN`
/// means adding a rendering path adds it here too, for free.
///
/// Not part of `image`, because they answer different questions. `image` asks
/// whether the render matches what was blessed, at a calibrated tolerance.
/// This asks whether the render is the *same* twice, exactly — a property with
/// no tolerance at all, which a tolerant comparison cannot see.
fn repeat() -> std::process::ExitCode {
    let root = repo_root();
    let loom = match build_release(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask repeat — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let scratch = root.join("target/xtask-repeat");
    let _ = std::fs::create_dir_all(&scratch);

    /// Renders of each scene. Three rather than two, so a difference says
    /// which run is the odd one out rather than only that two disagree.
    const RUNS: usize = 3;

    let mut failures = Vec::new();
    let mut checked = 0;
    println!("{RUNS} fresh processes per scene, compared byte for byte.");
    println!();

    let filter = only_filter();
    let mut skipped = 0_usize;
    for (name, scene, extra) in GOLDEN {
        if !selected(filter.as_ref(), name, scene) {
            skipped += 1;
            continue;
        }
        if !root.join(scene).exists() {
            failures.push(format!("{name}: {scene} is missing"));
            continue;
        }
        checked += 1;

        let mut renders: Vec<Option<Vec<u8>>> = Vec::new();
        for index in 0..RUNS {
            let path = scratch.join(format!("{name}_{index}.png"));
            let path_string = path.to_string_lossy().into_owned();
            let mut argv: Vec<&str> =
                vec!["render", scene, "--out", &path_string, "--size", GOLDEN_SIZE];
            argv.extend_from_slice(extra);
            match run(&loom, &root, &argv) {
                Ok(output) if output.status.success() => {
                    renders.push(std::fs::read(&path).ok());
                }
                Ok(output) => {
                    failures.push(format!(
                        "render {name} (run {index}): {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                    renders.push(None);
                }
                Err(e) => {
                    failures.push(format!("render {name} (run {index}): {e}"));
                    renders.push(None);
                }
            }
        }

        let digests: Vec<String> = renders
            .iter()
            .map(|bytes| bytes.as_deref().map_or_else(|| "-".to_owned(), digest))
            .collect();
        let identical = renders
            .iter()
            .all(|bytes| bytes.is_some() && *bytes == renders[0]);
        println!(
            "{name:<18} {}  {}",
            if identical { "same" } else { "DIFFER" },
            digests.join(" ")
        );
        if !identical {
            failures.push(format!(
                "{name}: three runs of one scene produced different bytes ({}) — \
                 a GPU-stateful path is depending on something that is not the \
                 scene and the tick",
                digests.join(" ")
            ));
        }
    }

    if failures.is_empty() {
        println!();
        println!("cargo xtask repeat: {checked} scene(s) reproduce byte for byte");
    if skipped > 0 {
        println!("  (--only: {skipped} row(s) skipped — this run did not cover them)");
    }
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!();
    for failure in &failures {
        eprintln!("  repeat: {failure}");
    }
    std::process::ExitCode::from(1)
}

/// A short, readable fingerprint of a rendered PNG.
///
/// FNV-1a rather than SHA-256: the *comparison* above is a full byte-for-byte
/// equality, so this is only ever printed, never trusted — and a hash used
/// purely as a log line does not justify a dependency.
fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

fn run(loom: &Path, root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new(loom)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())
}

/// `loom`, with environment variables set.
///
/// A sibling of [`run`] rather than a parameter on it, because every other caller passes
/// none and threading an empty slice through all of them would be noise.
fn run_env(loom: &Path, root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Output, String> {
    let mut command = Command::new(loom);
    command.args(args).current_dir(root);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().map_err(|e| e.to_string())
}

/// Render each scene with and without one effect, and fail if they are too alike.
///
/// **This is the only check in this repository that fails on SAMENESS**, and that is the
/// whole point of it. `image` and `repeat` both ask whether a render changed when it
/// should not have. Neither can see a feature that is registered, tested, closed-form
/// correct and drawing nothing — which has shipped four times here. Removing such a
/// feature changes no pixel, so it scores zero and this task fails it.
fn ablate() -> std::process::ExitCode {
    let root = repo_root();
    let loom = match build_release(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    // **Unlike `image`, `validate`, `flythrough`, `shimmer` and `repeat`, a missing GPU is
    // not success here.** Those gates all ask "did a render change when it should not
    // have" — no device, no render, nothing to compare, so withholding an answer is the
    // honest outcome and exit 0 is right. This gate asks the opposite question, "did a
    // render change *enough*", and its failure mode is a check that quietly answers
    // "yes, enough" without ever having looked — which is exactly the four-times-shipped
    // bug the whole harness exists to catch. So a run that could not obtain a device
    // must not report the same exit code as a run that measured every row and passed;
    // exit 2 (invocation/environment problem, per this CLI's own convention) it is, with
    // wording that a reader can tell apart from a genuine `DRAWING NOTHING` failure.
    if !has_vulkan_device(&loom, &root) {
        eprintln!(
            "xtask: cargo xtask ablate could not obtain a Vulkan device — nothing was \
             rendered or compared, so nothing was verified; this is not the same as every \
             effect passing"
        );
        return std::process::ExitCode::from(2);
    }

    let scratch = root.join("target/xtask-ablate");
    let _ = std::fs::create_dir_all(&scratch);

    println!("scene                effect          changed%   floor%   verdict");
    let mut failures = Vec::new();

    let filter = only_filter();
    let mut skipped = 0_usize;
    for (label, scene, effect, extra, floor) in ABLATE {
        if !selected(filter.as_ref(), label, scene) {
            skipped += 1;
            continue;
        }
        let with = scratch.join(format!("{label}_{effect}_with.png"));
        let without = scratch.join(format!("{label}_{effect}_without.png"));

        // **A stale PNG must not survive to be scored.** Everything below this line
        // detects a failed render by its exit status, but that check is only as strong
        // as this file keeps it — a future edit that loosens it would otherwise let
        // `compare` silently score yesterday's frame against today's, exactly the false
        // "ok" this gate was built to end. Removing the two paths first makes that
        // impossible by construction: a render that does not succeed leaves nothing
        // here for `compare` to read, regardless of how the failure is detected later.
        let _ = std::fs::remove_file(&with);
        let _ = std::fs::remove_file(&without);

        let render = |out: &Path, env: &[(&str, &str)]| {
            let out_str = out.to_string_lossy();
            let mut argv: Vec<&str> = vec!["render", scene];
            argv.extend_from_slice(extra);
            argv.extend_from_slice(&["--size", GOLDEN_SIZE, "--out", &out_str]);
            run_env(&loom, &root, &argv, env)
        };

        // **`LOOM_ABLATE` must be cleared for the "with" render, not merely left unset
        // here.** `run_env` inherits the calling process's environment — it never calls
        // `env_clear` — so a human who runs `LOOM_ABLATE=water_foam cargo xtask ablate`
        // would otherwise get it ablated in BOTH renders, a 0.000% difference, and a
        // false `DRAWING NOTHING` on the one instrument whose zero has to mean something.
        // Setting it to `""` explicitly overrides whatever was inherited; `parse_ablations`
        // already treats empty as "ablate nothing".
        let with_render = render(&with, &[("LOOM_ABLATE", "")]);
        let without_render = render(&without, &[("LOOM_ABLATE", effect)]);

        // `Command::output()` returning `Ok` only means the process launched — a
        // `loom render` that starts and then exits non-zero is `Ok` with a failed
        // `status`, which `.is_err()` alone never sees. Checked explicitly here, the
        // way `image()` checks its own `render.status.success()`. Each side is checked
        // independently and both are reported — a `match` that stops at the first arm
        // to fire previously let a failed "without" status hide a spawn error on "with"
        // whenever both sides went wrong at once.
        let mut render_failures = Vec::new();
        match &with_render {
            Err(e) => render_failures.push(format!("with: {e}")),
            Ok(o) if !o.status.success() => {
                render_failures.push(format!("with: {}", String::from_utf8_lossy(&o.stderr).trim()));
            }
            Ok(_) => {}
        }
        match &without_render {
            Err(e) => render_failures.push(format!("without: {e}")),
            Ok(o) if !o.status.success() => {
                render_failures.push(format!("without: {}", String::from_utf8_lossy(&o.stderr).trim()));
            }
            Ok(_) => {}
        }
        if !render_failures.is_empty() {
            failures.push(format!(
                "{label}/{effect}: render failed ({})",
                render_failures.join("; ")
            ));
            continue;
        }

        let Ok(output) = run(
            &loom,
            &root,
            &["compare", &with.to_string_lossy(), &without.to_string_lossy()],
        ) else {
            failures.push(format!("{label}/{effect}: compare failed to run"));
            continue;
        };
        // `compare` legitimately exits 1 for the case this loop expects — the two
        // renders differ by more than its default tolerance — so `status.success()`
        // cannot be the test here the way it is in `image()`, where a match is the
        // whole point. Exit 2 is `compare`'s own "the invocation was wrong" (a path
        // it could not load); that is the only status this loop treats as a failure
        // before looking at the output, and it is checked first so a bad path is
        // reported as "compare could not run" rather than the vaguer "printed no
        // fraction" below (which still stands as a backstop for anything else odd).
        if output.status.code() == Some(2) {
            failures.push(format!(
                "{label}/{effect}: compare could not run: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            ));
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(fraction) = read_field(&text, "\"fraction\"") else {
            failures.push(format!("{label}/{effect}: compare printed no fraction"));
            continue;
        };

        let ok = fraction >= floor;
        println!(
            "{label:<20} {effect:<15} {:>7.3}  {:>7.3}   {}",
            fraction * 100.0,
            floor * 100.0,
            if ok { "ok" } else { "DRAWING NOTHING" }
        );
        if !ok {
            failures.push(format!(
                "{label}/{effect}: removing it changed {:.3}% of the frame, floor is {:.3}% \
                 — the effect is not drawing",
                fraction * 100.0,
                floor * 100.0
            ));
        }
    }

    if failures.is_empty() {
        println!("\nok: {} ablation(s), every effect is drawing", ABLATE.len() - skipped);
        if skipped > 0 {
            println!("  (--only: {skipped} row(s) skipped — this run did not cover them)");
        }
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!();
    for failure in &failures {
        eprintln!("xtask: {failure}");
    }
    std::process::ExitCode::from(1)
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

/// The release binary, for the gates that compare *pixels* rather than listen
/// to the validation layers.
///
/// **Measured, not assumed: a debug render and a release render are the same
/// image.** `loom compare --channel 0 --fraction 0 --worst 0` reports 0 of
/// 64,000 differing pixels on `whitecaps`, `lucent`, and — the two that matter,
/// because they are the GPU-stateful paths — `emberfall` (the particle pool)
/// and `rain_gantry` (the drop buffer). The shaders are the same SPIR-V either
/// way, and the CPU half is already held to debug/release agreement by
/// `the_two_profiles_simulate_the_same_world`.
///
/// What it buys: **5.561 s per scene in debug against 0.618 s in release** at
/// `GOLDEN_SIZE` — nine times, on `image` (60 scenes), `repeat` (three runs of
/// each) and `flythrough` (sixteen frames of each).
///
/// **`validate` is deliberately NOT on this path.** Its whole job is to run the
/// validation layers, which panic in debug by design; a release binary would
/// report a clean sweep having listened to nothing.
fn build_release(root: &Path) -> Result<PathBuf, String> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "loom_cli"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !status.success() {
        return Err("the release build failed; nothing to render".to_owned());
    }
    let binary = root.join("target/release/loom");
    if binary.exists() {
        Ok(binary)
    } else {
        Err(format!("{} was not produced", binary.display()))
    }
}

/// How many scene processes to keep in flight.
///
/// **Each render is its own process with its own Vulkan device**, so this is
/// bounded by the GPU rather than by cores: four submitting queues keep a 4090
/// busy without any one of them waiting on another's fence. `LOOM_GATE_JOBS=1`
/// restores the serial behaviour, which is what to reach for when a gate fails
/// and the interleaved output is hard to read.
///
/// Nothing here shares a file: every job writes its own PNG. That is a
/// requirement rather than a happy accident — `validate` used to point all
/// eighty scenes at one `target/xtask-validate.png`, which is exactly the kind
/// of shared mutable path that turns a parallel run into a flaky one.
fn gate_jobs() -> usize {
    let asked = match std::env::var("LOOM_GATE_JOBS").ok().and_then(|v| v.parse::<usize>().ok()) {
        Some(n) if n >= 1 => n,
        _ => std::thread::available_parallelism().map_or(4, |n| n.get().min(4)),
    };
    let headroom = gpu_headroom();
    let jobs = asked.min(headroom);
    if jobs < asked {
        println!(
            "  (gpu busy: {jobs} job(s) instead of {asked} — something else is using the card)"
        );
    }
    jobs.max(1)
}

/// How many render processes this card can take **right now**, given whatever
/// else is already on it.
///
/// **The point is not to go faster, it is not to freeze the desktop.** A gate
/// is a background chore; a game or a ComfyUI batch is the thing the human is
/// actually doing, and four more Vulkan devices arriving on a saturated card is
/// how a compositor stops responding. So this yields to whatever is already
/// there rather than competing with it.
///
/// **VRAM is the reliable signal; utilisation is not, on this class of
/// machine.** Measured idle on the development box: **37–41% utilisation with
/// 4.0 GB resident**, which is the compositor and its shader, not work. A
/// threshold anywhere near that baseline would throttle every gate on an idle
/// machine, so utilisation is only consulted at 80% — high enough that only a
/// real workload reaches it — while the count of jobs comes from free memory.
///
/// `PER_JOB_MB` is deliberately generous: a `GOLDEN_SIZE` render is far under
/// it, but the same pool runs 1920x1080 windowed frames with MSAA and a TLAS,
/// and the failure mode of guessing low is the one this function exists to
/// prevent.
///
/// No `nvidia-smi`, no answer, no throttle — CI has no GPU and a missing tool
/// must not silently serialise a gate that would have been fine.
fn gpu_headroom() -> usize {
    /// Free megabytes assumed needed per concurrent render process.
    const PER_JOB_MB: u64 = 1536;
    /// Utilisation at or above which the card is doing someone else's work.
    const BUSY_PERCENT: u64 = 80;

    let Ok(out) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
    else {
        return usize::MAX;
    };
    if !out.status.success() {
        return usize::MAX;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().next() else {
        return usize::MAX;
    };
    let fields: Vec<u64> = line
        .split(',')
        .filter_map(|f| f.trim().parse::<u64>().ok())
        .collect();
    let [used, total, utilisation] = fields[..] else {
        return usize::MAX;
    };
    if utilisation >= BUSY_PERCENT {
        return 1;
    }
    usize::try_from(total.saturating_sub(used) / PER_JOB_MB).unwrap_or(1).max(1)
}

/// Run `jobs` with at most `width` in flight, returning results **in input
/// order**.
///
/// Order matters: a gate's output is read by a human comparing it against the
/// last run, and a report whose rows shuffle between runs is one nobody can
/// diff. The work is unordered; the reporting is not.
fn in_parallel<T, F>(width: usize, jobs: Vec<F>) -> Vec<T>
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    if width <= 1 || jobs.len() <= 1 {
        return jobs.into_iter().map(|j| j()).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<T>>> =
        (0..jobs.len()).map(|_| std::sync::Mutex::new(None)).collect();
    let jobs: Vec<std::sync::Mutex<Option<F>>> =
        jobs.into_iter().map(|j| std::sync::Mutex::new(Some(j))).collect();
    std::thread::scope(|scope| {
        for _ in 0..width.min(jobs.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(cell) = jobs.get(i) else { return };
                let Some(job) = cell.lock().ok().and_then(|mut g| g.take()) else {
                    return;
                };
                let value = job();
                if let Ok(mut slot) = slots[i].lock() {
                    *slot = Some(value);
                }
            });
        }
    });
    slots
        .into_iter()
        .filter_map(|m| m.into_inner().ok().flatten())
        .collect()
}

/// `--only <substring>`: run just the rows whose name or scene path contains it.
///
/// **A convenience, and it is never what CI runs.** A gate that has been
/// narrowed has not been passed, so every call site prints what it skipped —
/// silent truncation reading as "covered everything" is the failure this whole
/// gate suite exists to prevent.
fn only_filter() -> Option<String> {
    let mut args = std::env::args().skip_while(|a| a != "--only");
    args.next()?;
    args.next()
}

fn selected(filter: Option<&String>, name: &str, scene: &str) -> bool {
    // Comma-separated, because the thing this is most wanted for is blessing a
    // *set* of rows that share no substring — the water rows one commit, the
    // tonemap rows another — and one substring cannot name a set.
    filter.is_none_or(|f| {
        f.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .any(|p| name.contains(p) || scene.contains(p))
    })
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

/// Where every worktree of this repository agrees to look for the gate lock.
///
/// **`--git-common-dir`, not the worktree's own `.git`.** For a linked
/// worktree that resolves to the *main* repository's git directory, which is
/// precisely the scope the lock needs: several worktrees of one repo, on one
/// machine, each able to saturate every core. [`repo_root`] cannot be used —
/// it is `CARGO_MANIFEST_DIR` baked in at compile time, so each worktree
/// builds an xtask that points at itself and they would never collide.
fn gate_lock_path() -> PathBuf {
    let root = repo_root();
    let dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_owned()))
        .map_or_else(|| root.join(".git"), |p| if p.is_absolute() { p } else { root.join(p) });
    dir.join("loom-gate.lock")
}

/// Exclusive access to the machine while a gate runs.
///
/// **The gates are not parallel-safe against each other, and nothing used to
/// say so.** Each one drives the real `loom` binary as a subprocess over
/// dozens of scenes; six agents in six worktrees running `validate` and
/// `image` at once pegged every core on this machine, stuttered the desktop,
/// and — worse for a gate — made `validate`'s own CPU-per-frame budget report
/// 44.8 ms for a scene that measures 25.2 ms unloaded. A timing check that
/// fails because *another copy of itself* is running is not a check.
///
/// So the gates are a singleton. A second one **waits** rather than failing:
/// serialising them is the fix, and refusing would only turn a slow agent into
/// a broken one.
struct GateLock {
    path: PathBuf,
}

impl GateLock {
    fn acquire(task: &str) -> Result<Self, String> {
        let path = gate_lock_path();
        // Counted polls rather than a deadline: this crate is held to the same
        // "never read the wall clock" lint as the simulation, and 720 polls of
        // five seconds is an hour by construction without arguing about it.
        const POLL: std::time::Duration = std::time::Duration::from_secs(5);
        const MAX_POLLS: u32 = 720;
        let mut waited = 0_u32;
        let mut announced = false;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = writeln!(f, "{} {task}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("gate lock {}: {e}", path.display())),
            }

            let holder = std::fs::read_to_string(&path).unwrap_or_default();
            let pid = holder
                .split_whitespace()
                .next()
                .and_then(|p| p.parse::<u32>().ok());
            // **A lock whose owner is gone is not a lock.** A gate killed
            // mid-run — which is exactly what happens to an agent that is
            // cancelled — would otherwise block every later run forever.
            if pid.is_none_or(|p| !Path::new(&format!("/proc/{p}")).exists()) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !announced {
                println!(
                    "cargo xtask {task}: another gate is running ({}), waiting for it",
                    holder.trim()
                );
                announced = true;
            }
            waited += 1;
            if waited > MAX_POLLS {
                return Err(format!(
                    "waited an hour for the gate lock held by {}; \
                     if that process is dead, delete {}",
                    holder.trim(),
                    path.display()
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for GateLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
