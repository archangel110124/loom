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
const SCENES: [&str; 51] = [
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
    // The whitecap trail (W2). Three extra `loom_sample_water` taps per water
    // vertex and a fifth varying out of `waterVertexMain`, which is the one
    // place a wrong `TEXCOORD` index or an overflowed output signature would
    // show up as a validation message rather than as a picture.
    "assets/test/whitecaps.loom",
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

    // Held for the whole run and released by `Drop` on the way out. Every task
    // below drives the engine over dozens of scenes and will use the whole
    // machine; see [`GateLock`] for what running two at once did.
    let _lock = match task.as_str() {
        "validate" | "image" | "flythrough" | "shimmer" | "repeat" => match GateLock::acquire(&task) {
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
        other => {
            eprintln!(
                "unknown task {other:?}\n\nUSAGE:\n    cargo xtask validate\n    \
                 cargo xtask image [--bless]\n    cargo xtask flythrough
    cargo xtask shimmer
    cargo xtask repeat"
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
const GOLDEN: [(&str, &str, &[&str]); 37] = [
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
    let loom = match build_debug(&root) {
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

    for (name, scene, extra) in GOLDEN {
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
