# Survey — the engine surface the editor has to reach

*Design phase, editor rework. Nothing here proposes UI; it inventories what exists,
what exposes it, and what the new editor must grow to reach it. Every claim is cited
to `file:line` in this worktree at `62f9ebe`.*

---

## The four findings that shape everything after this

**1. Nine `SceneOp`s are the entire write vocabulary of this engine.**
`crates/loom_scene/src/ops.rs:47-98` — `SpawnNode`, `SetTransform`, `SetField`,
`RemoveNode`, `RenameNode`, `ReparentNode`, `RemoveComponent`, `RevertOverrides`,
`UnpackPrefab`. That is it. Never-do #16 says every editor action becomes SceneOps, so
**every tool the new editor grows — terrain brush, paint stroke, decal stamp, prefab
drop — must decompose into those nine or the vocabulary must grow with an ADR.** The
survey below marks which features already decompose cleanly and which do not.

**2. The inspector cannot edit a string, an object, or an array of objects.**
`crates/loom_cli/src/panels.rs:877-895`: `Value::String` renders as a weak monospace
label, and the `_` arm renders every nested object and heterogeneous array through
`summarise` (`panels.rs:110`) as read-only text. Numbers, booleans and numeric arrays
are editable; nothing else is. That single `match` is why **roughly a third of the
authored surface of this engine is display-only in the editor today** — script paths,
mesh aliases, texture aliases, HUD text, water kind, the voxel op list, the wave set,
the pontoon list, the ground layer, the scatter excludes. The gap is not scattered; it
is one function.

**3. The editor does not resolve prefabs, and CLAUDE.md predicted this exact bug.**
`crates/loom_cli/src/scene_view.rs:110` calls `Scene::parse` directly. Every other
reader in the codebase goes through `prefab_load::for_reading`
(`main.rs:565, 2535, 3111, 3222, 3308, 3903, …`), and `prefab_load.rs:7-12` states the
rule: *"a command that reads a scene and skips resolution reintroduces exactly that
bug"* — the instance arrives with no components, draws nothing, validates clean.
**`loom run --edit assets/test/prefab_room.loom` is that bug, live.** `loom explode`
(`main.rs:3440`) is the second offender. Neither has any prefab UI either: `UiAction`
(`panels.rs:24-56`) has no revert, no apply, no unpack, no override display.

**4. All four requested painting systems are net-new *engine* work, not UI work.**
There is no vertex colour channel — `Vertex` is position/normal/uv only
(`crates/loom_asset/src/mesh.rs:12-19`). A repo-wide search for `decal` finds two
comments about what a raycast normal would be *useful* for
(`loom_physics/src/lib.rs:36,1118`) and no implementation. A search for `splat` finds
only `glam::Vec3::splat`. `Material::layer` / `GroundLayer`
(`components.rs:227,238-267`) is a **slope-driven two-layer blend, not a painted mask**
— it takes over below a normal-Y cutoff and has no per-texel control. So splat
painting, UV texture painting, vertex-colour painting and decals each need a new
component, a new render path, and — critically — **an answer to how a paint stroke
becomes diffable text**, because never-do #11's reasoning ("never serialize raw voxel
arrays") applies with equal force to a 4K splat map.

---

## The complete surface

Legend for **Editor today**: ✅ reachable and editable · ◐ visible but not editable ·
❌ not present at all · 🔧 CLI-only.

### Scene graph and transactions

| Feature | Exposed by | Editor today | Affordance the new editor needs |
| --- | --- | --- | --- |
| Node tree, parent/child | `scene.rs:28-67` (`Node`), `ops.rs:49,77` | ✅ hierarchy with drag-reparent, `run.rs:1275` | Hierarchy panel; keep multi-select and drag-reparent |
| Spawn / rename / delete | `ops.rs:49,75,73`; `run.rs:1786,1759` | ✅ `AddChild`, `Rename`, `Delete` | Create menu, F2 rename, Delete key |
| Duplicate | `run.rs:1260` (`UiAction::Duplicate`) | ✅ | Ctrl+D; must stay one transaction |
| Local transform | `components.rs:26-33`; sugar desugared at `run.rs:1762-1772` | ✅ gizmos + inspector | Move/Rotate/Scale gizmos, numeric fields, **snap/grid (absent)** |
| Version token / stale-write rejection | `ops.rs:198-215`, `edit.rs:186-190` | ✅ conflict banner, `run.rs:644-661` | Keep. Banner offering both versions, merging neither |
| Undo/redo, gesture coalescing | `edit.rs:170-191`, `edit.rs:267-295` | ✅ Ctrl+Z/Ctrl+Y, `run.rs:1296-1311` | Keep verbatim — this is never-do #16's machinery |
| Transaction log | `edit.rs:182` (`history`), `panels.rs:579` console | ✅ | Log panel with labels; labels are also git history |
| `--dry-run` diff preview | `ops.rs:107-109`, `main.rs` USAGE:74 | 🔧 CLI-only | **Gap:** a "preview this change" affordance for large ops |
| Semantic placement (on-top-of, align, face, grid) | `place.rs:102-131` (`PlaceOp`), `loom place --op` | 🔧 CLI-only | **Gap:** snap-to-surface drop, align tool, array/grid tool |
| Measure bounds & overlaps | `loom measure`, USAGE:104 | 🔧 CLI-only | **Gap:** a measure/ruler tool, overlap warnings in the viewport |
| Shape analysis (concavity, curvature spread) | `loom measure --shape`, `loom_asset/src/shape.rs` | 🔧 CLI-only | Low priority; report-only by design, never a gate |

**`SetField` is the workhorse and it is also the trapdoor.** `run.rs:1774-1779` routes
every non-transform inspector edit through it with a dotted `Type.field` path. It
creates the component table it writes into (`ops.rs:78-80`), which is how
`AddComponent` works at all (`run.rs:1598-1620`: write every schema default). That
means adding a component is *N* `SetField` ops in one transaction — fine. But it also
means **any field whose value is a whole array is edited by replacing the whole array**,
which is exactly what terrain sculpting and painting would do.

### Prefabs and scene inheritance

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `[[prefab]]` declaration, `prefab = "<alias>"` | `scene.rs:48-56,69-79`, `prefab.rs:34-54` | ❌ **not even resolved** (`scene_view.rs:110`) | Fix the load path first. Then: prefab instances drawn distinctly in the hierarchy |
| Per-instance `[node.overrides]` | `scene.rs:60-66` (flat dotted map) | ❌ | **Inspector must show an overridden field as overridden** and offer per-field revert |
| Setting a field on an instance writes an *override* | CLAUDE.md rule; `prefab.rs:227` `expand_instance` | ❌ | The inspector needs no idea which kind of node it holds — but the *op layer* must route it |
| `revert-overrides` (all or named keys) | `SceneOp::RevertOverrides`, `ops.rs:85-89`; `loom prefab` | 🔧 CLI-only | Right-click → Revert; per-field revert arrow |
| `apply-overrides` (promote to prefab) | `prefab_cmd.rs:7-12` — **two files, two undo steps** | 🔧 CLI-only | Menu item that says so out loud |
| `unpack` (stop tracking the prefab) | `SceneOp::UnpackPrefab`, `ops.rs:97` | 🔧 CLI-only | Right-click → Unpack, with the one-way warning |
| `extends` (scene inheritance, root node only) | `scene.rs:57-59` | ❌ | New-project templates want this; needs at minimum a "this scene extends X" header |
| Library keyed by `id`, never alias | `prefab.rs:44-52`; CLAUDE.md | ❌ | Prefab browser must key on `id`; two files may spell one alias differently |

**Prefabs are the single largest editor gap by volume of engine behaviour already
built.** S4 delivered them "in full" per CLAUDE.md and the editor reaches none of it.

### Rendering, materials, assets

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `MeshRenderer` + `[[asset]]` alias | `components.rs:58-63`, `scene.rs:177-179` | ◐ assign by clicking the asset panel (`panels.rs:540-568`); the alias string itself is read-only | Asset picker dropdown on the field, not only a side panel |
| Primitives | `loom_asset/src/primitives.rs:10` — `box, plane, sphere, cylinder, capsule` | ◐ only if the scene already declares them as assets | **Create menu.** Note the naming: the request says "cube" (`box`), "quad" (`plane` *is* a 4-vert quad, `primitives.rs:82-94`); `cylinder` exists and was not requested |
| Mesh import (OBJ/glTF) | `loom_asset/src/mesh.rs`, `assets/test/gltf/` | ❌ no import UI, no import command | **Gap:** drag-a-file-in, write the `[[asset]]` entry |
| Asset ids / manifest / content hash | `loom_asset/src/meta.rs:21-160` | ❌ **dead code** — no caller outside the crate | A project/Hub needs this; it is a stub today |
| `Material` (albedo, metallic, roughness, maps, uv_scale, triplanar, porosity, alpha_cutoff) | `components.rs:139-235` | ◐ numbers/bools editable; `albedo_map`/`normal_map` are nested `AssetRef` objects → read-only | Colour swatch + picker, texture slot with thumbnail and picker |
| `Material.layer` / `GroundLayer` (slope blend) | `components.rs:227,238-267` | ◐ read-only (nested object) | Foldout with its own fields; note `GroundLayer.slope` should match `Grass::slope_cutoff` or a hill gets two rings |
| `Light` — **point only**, intensity + linear RGB | `components.rs:83-115` | ✅ | Colour picker; an intensity helper, because `intensity = d²/albedo` (`components.rs:92-103`) is the whole usability problem |
| Environment: sun dir/strength/colour, ambient, sky zenith/horizon, fog density/falloff, cloud cover/scale, exposure | `components.rs:855-916` | ✅ numbers editable | A dedicated Environment/lighting panel; **sun direction wants a drag-the-sun widget**, three unbounded floats is the worst possible UI for a direction |
| MSAA 4x, resolve, CMAA2 | `renderer.rs` `MSAA_SAMPLES`, `cmaa2.rs` | ❌ not authored, not exposed | Viewport quality setting; per CLAUDE.md the viewer and offscreen paths must agree |
| Ray-traced soft shadows / RTAO / reflections | `raytrace.rs`, ADR 0019 | ❌ always on, no author control | A viewport toggle is a *measurement* tool here, not a setting; flag as ADR territory |
| `ids` / `collision` debug view modes | **do not exist** — repo-wide search finds no `DebugMode`, no `--debug` | ❌ | CLAUDE.md §"When stuck" step 3 recommends them. **They are aspirational.** A wireframe/collision/normals overlay is a real gap |
| Golden-image gate, flythrough, shimmer, flicker, dolly | `cargo xtask image/flythrough/shimmer`, `loom compare`, `loom flicker`, `loom render --dolly` | 🔧 CLI-only | Not editor work, but the editor must not make them harder — the new viewport still has to be measurable |

### Voxel terrain — the hardest case

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `VoxelVolume` (voxel_size, chunks) | `components.rs:351-357` | ✅ two numeric fields | Fine as-is |
| The op list — `sphere/box/capsule/heightfield/terrain`, each with `mode` union/subtract/intersect | `components.rs:358-480` (the doc comment **is** the schema; `ops` is `Vec<serde_json::Value>`) | ◐ shown as *"N items"* by `summarise` (`panels.rs:110-114`) | **The whole sculpt story.** See below |
| Shape modifiers: `displace`, `elongate`, `yaw_degrees`, `round` | `components.rs:404-424`, `loom_voxel/src/lib.rs:211` (`Displace`) | ❌ | Per-op inspector rows; `displace` has a measured recipe (`components.rs:436-470`) worth surfacing as a preset |
| Destructive edit / re-mesh dirty chunks | `loom_voxel/src/lib.rs:1227` (`Volume::edit`), `:1359` `dirty_with_neighbours` | 🔧 `loom explode` only | Live brush preview can use this; the *persisted* result is still an appended op |
| `loom explode --at --radius` | `main.rs:3419-3470` | 🔧 CLI-only, and it **does not go through `prefab_load`** | A blast/carve tool |
| Terrain recipes: fbm, ridged, spline carve, flatten disc, peak, corridor, hydraulic, thermal | `loom_terrain/src/lib.rs:145-231` (`Layer`), `:274-288` (`Recipe`) | ❌ recipes are a **separate `.toml` file** the editor never opens | **Gap:** a recipe editor. Layer order is the design (`assets/test/moraine.toml:9-12`) and `order_warnings` (`lib.rs:361`) exists to catch getting it wrong |
| `loom terrain` metrics: buildable_pct, slope_mean, largest_flat, reachable | USAGE:118-126, `loom_terrain/src/analyze.rs` | 🔧 CLI-only | **This is the only assertable feedback a landscape has.** It belongs beside the recipe editor as a live readout |
| Exposure / shelter query (S3) | `loom_voxel/src/exposure.rs`, `loom sim --assert …exposure` | 🔧 CLI-only | A "how sheltered is this point" probe would make rain and wind authorable |
| Height field probe | `loom_voxel/src/heightfield.rs`, used at `run.rs:583-596` | internal | — |

**A terrain brush stroke has no honest `SceneOp` today.** The only route is
`SetField { field: "VoxelVolume.ops", value: <the entire array> }` — that is what the
test at `ops.rs:1371` and the CLI at `main.rs:369` do. Three consequences the design
phase has to answer:

- **A stroke rewrites the whole list.** Fine for correctness (transactions are whole-file
  snapshots anyway, `edit.rs:174-179`), ruinous for the diff — the point of op-list
  serialization is that a scene is *diffable text*, and a 400-op list re-emitted whole on
  every stroke makes every commit unreadable.
- **The list only ever grows.** Nothing coalesces two overlapping subtracts. A sculpting
  session is unbounded, and bake cost is linear in ops (`Volume::bake`, `lib.rs:1100`).
- **`ops` is untyped JSON** (`components.rs:481`), so `Scene::parse` never looks inside it
  and schema validation happens in exactly one place: `loom validate` (`main.rs:355-378`).
  An inspector generated from the type registry sees *"array"* and nothing else — which is
  precisely why it shows *"4 items"*.

The lazy fix is an **`AppendVoxelOps` op** — one new `SceneOp`, order-preserving, appending
to a named array. That is a locked-decision-adjacent change (it grows the op vocabulary)
and wants an ADR, but every alternative is worse.

### Physics

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `RigidBody` (dynamic, mass) | `components.rs:321-327` | ✅ | Fine |
| `BoxCollider` (half_extents) | `components.rs:66-70` | ✅ numeric array | **A collider gizmo** — dragging half-extents as numbers is the classic bad UI |
| Voxel colliders for terrain | `loom_physics/src/lib.rs`, `loom_voxel` | automatic | Collision overlay (see debug modes gap) |
| `CharacterController` (height, radius, max_slope, step_height) | `components.rs:652-667` | ✅ | Capsule gizmo |
| Raycast + blast force with cover | `loom_physics/src/lib.rs:29` (`RayHit`), `components.rs:733-777` (`Blast`) | ✅ Blast fields editable | Radius gizmo (a sphere), an "arm/disarm" toggle that reads as one |
| Buoyancy: `Pontoon` list, coefficient, damping | `components.rs:1481-1560` | ◐ pontoon **array of objects → read-only** | Pontoon list editor + viewport spheres. `buoyancy.rs:322` `default_pontoons` can seed it from `half_extents` |
| `Submersion` (enter/exit hysteresis) | `components.rs:1589-1603` | ✅ | Fine |
| Physical sanity findings | `loom_physics/src/sanity.rs:48` `check_scene`, run by `loom validate` | 🔧 CLI-only | **Gap:** these warnings belong in the console panel, live |
| Never-do #10 (no trimesh on a dynamic body) | enforced in `sanity.rs` | 🔧 | Same — surface it at authoring time |

### Scripts, rules, events, HUD

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `Script { path }` — rhai movement/behaviour model | `components.rs:1682-1685`; host `loom_script/src/lib.rs:444` | ◐ **path is a string → read-only** | Script slot with a file picker; open-in-editor; hot reload exists (`ScriptWatcher`, `lib.rs:878`) but has no UI |
| `GameRules { path }` — win/lose | `components.rs:1674-1678`; `GameState` `lib.rs:285-323` | ◐ read-only string | Same, plus a rules status readout |
| Sandbox limits (op/depth caps) | `loom_script/src/lib.rs:29` (`Limits`) | ❌ not authored | Probably should stay non-authored; note it |
| Script memory, `Motive`, `Detonation`, `Motion` | `lib.rs:86,177,185,236` | ❌ | Runtime inspection only — a "script state" debug panel |
| Deterministic event log | `lib.rs:345-377` (`Event`, `EventLog`), `:398` `on_tick`, `:404` `counts` | 🔧 only via `loom sim` | **Gap:** an event timeline panel. `EventLog` is *a replay* (`lib.rs:340-343`) — the most valuable debug surface in the engine and it has no viewer |
| `Hud` (anchor, offset, text, size, colour, only_in_play) | `components.rs:779-827`; drawn at `loom_cli/src/hud.rs` | ◐ **`text` is a string → read-only**; anchor is a string enum → read-only | Text field, anchor 3×3 picker, colour swatch, and a **HUD layout mode** — the whole point of `hud.rs:3-6` is that moving the score is an edit |
| Enemies / AI | **no component** — an enemy is `CharacterController` + `Script` pointing at `assets/scripts/enemy.rhai` (`assets/games/proving_ground.loom:293-310`) | ✅ by accident | Prefab-driven. This is the strongest argument for a prefab browser |
| Navigation grid + A* | `loom_physics/src/nav.rs:79` `bake`, `:143` `path` | ❌ **hardcoded** `[-64,-64]..[64,64]`, cell 0.5, ceiling 200 at `play.rs:676-683`, and `ponytail:` never rebuilt (`play.rs:673-675`) | **Gap:** nav bounds are not authorable. Needs either a component or scene-bounds derivation, plus a walkable-cell overlay |
| `NavAgent { step }` | `nav.rs:55-60` | ❌ default only | Per-character field |

### Weather, vegetation, water

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `Wind` (direction_degrees, speed, gustiness, turbulence, ground_drag) | `components.rs:1114-1150`; field tree `loom_field::wind` | ✅ numbers | **A compass widget** for direction; a live arrow in the viewport |
| `Rain` (intensity mm/h, duration) | `components.rs:1176-1210` | ✅ | Intensity presets (drizzle→downpour are documented at `:1184-1192`) |
| Cloud deck / cover / scale | `Environment` `components.rs:895-899`; `loom_rain/src/lib.rs:80` (`Deck`), `:159` `cover_at` | ✅ two numbers | A sky preview. **Rule from CLAUDE.md: a raining scene that authors no cover gets a solid deck** — the UI must not make that look like a bug |
| Wetness (film/soak), 40 s lagged cover | `loom_rain/src/lib.rs:261,329,345`, `:140` `cover_recent` | 🔧 `loom sim --assert wetness@…` | A probe readout |
| Rain collision world (voxels ∪ static box colliders → `R8_SNORM` 3D image) | `loom_rain/src/collide.rs` | automatic | Nothing to author; possibly a debug view |
| `Grass` (half_extent, density, height, width, slope_cutoff, clump_facing, clump_colour) | `components.rs:1059-1090`; placement `loom_grass/src/lib.rs:263,315` | ✅ numbers | Extent gizmo (a rectangle on the ground), density preview. **`slope_cutoff` should track `GroundLayer.slope`** — the UI should say so |
| Grass ground query (slope/rock/flow from the voxel SDF) | `GroundGrid`, `loom_grass/src/lib.rs:141` (`Ground`), seam is a `&dyn Fn` | automatic | A coverage-preview overlay would be the single most useful grass affordance |
| `Scatter` (mesh, half_extent, spacing, jitter, scale, seed, max_slope, density, moisture, sway) | `components.rs:959-1005`; `loom_scatter/src/lib.rs:583` | ✅ numbers | Extent gizmo; seed reroll button |
| `ScatterExclude { field, radius }` list | `components.rs:1007,1034-1039` | ◐ **array of objects → read-only** | List editor; exclusion circles in the viewport |
| `WaterBody` (kind, surface_height, waves, density, drag, flow, material) | `components.rs:1379-1410` | ◐ `kind` is a string enum → read-only; `waves` and `flow` are objects → read-only | **Enum dropdown**, wave-set editor, surface-height gizmo |
| `WaveSet` / `GerstnerWave` (wavelength, amplitude, steepness, direction, speed_scale), `MAX_WAVES = 16` | `components.rs:1258-1360` | ◐ read-only | A wave list with add/remove and a spectrum preset — `loom_water/src/spectrum.rs` already generates sets |
| `FlowField { speed }`, baked from drainage | `components.rs:1445-1455`; `loom_water/src/flow.rs:128` `bake`, `:318` `peak_speed` | ◐ read-only | Toggle + speed; flow-arrow overlay |
| `loom water --at` point probe (height, normal, depth, velocity) | USAGE:110-116, `loom_water/src/lib.rs:194` | 🔧 CLI-only | A click-to-probe tool |
| `ParticleEmitter` — 24 fields incl. `flame`, `additive`, `burst`, `delay`, `duration`, `wind_response`, `seed` | `components.rs:507-605` | ✅ all numeric/bool | It works, but 24 flat sliders is bad UI. Grouped foldouts + presets (fire / smoke / spark), because the defaults are *a smoke plume* by deliberate choice (`components.rs:498-501`) |
| Fire + smoke explosions = `Blast` + two `ParticleEmitter`s on one node | `components.rs:715-720` | ✅ | Prefab, not a feature. Ship it as a sample prefab |

### Audio

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `AudioSource` (clip, volume, range, looping, autoplay) | `components.rs:1624-1642` | ◐ `clip` is a nested `AssetRef` → read-only; the rest ✅ | Clip picker, **audition button**, range sphere gizmo |
| Ray-traced acoustics (`Acoustics::solve`, `Ears`) | `loom_audio/src/lib.rs:49,105,147` | automatic during play | A listener marker; an acoustics readout |
| Rain audio bed, synthesised fallback | `loom_audio/src/rain.rs` | automatic | — |
| `loom audio --seconds --openness --out` → rms, peak, tilt | USAGE:85-90 | 🔧 CLI-only | An audio meter panel during play |
| Underwater filtering | `loom_audio/src/lib.rs:91` `underwater` | automatic | — |

**Audio's `openness` is deliberately *not* unified with S3 exposure** (CLAUDE.md):
acoustics casts against the collision world, exposure marches the voxel volume only.
Any UI that shows both must label them separately or it will teach the wrong model.

### Camera, play, input

| Feature | Exposed by | Editor today | Affordance needed |
| --- | --- | --- | --- |
| `Camera { fov_y_degrees, active }` — first active in file order wins | `components.rs:695-704` | ✅ | **"Align camera to view"** is the missing verb — the single most-used camera affordance in every editor. It is two `SetTransform` ops |
| Camera preview / look-through | `main.rs` render path honours the scene camera | ❌ | Picture-in-picture preview when a camera is selected |
| Play / Pause / Step / Stop | `panels.rs:50-55`, `run.rs:1321` `start_play` | ✅ | Keep. Transport bar |
| First-person play with pointer capture | `run.rs:1329-1351,1361` | ✅ conditional on a `CharacterController` **and** a `Camera` | The failure message (`run.rs:1348-1350`) should be a UI state, not a console line |
| Input action map: `fly` / `edit` / `play` contexts | `loom_input/src/lib.rs:59` (`ActionMap`), `assets/input/default.toml` | ❌ no UI | **Gap, and a shipping blocker:** `run.rs:2242-2247` loads `assets/input/default.toml` **relative to the process cwd**. A shipped `exe + assets/` build works only if launched from the right directory, and a *project* has no way to carry its own bindings |
| Editor keybindings (Tab select, IJKL nudge, 1/2/3 gizmo modes, Ctrl+Z/Y/S/D) | `assets/input/default.toml:32-56` | ✅ text file, no rebuild | A rebinding UI; also note **1/2/3 rather than W/E/R because W and E fly the camera** — the new editor inherits that constraint or breaks the fly cam |
| Frame-cost readout, GPU timestamps | `LOOM_GPU_TIMING=1`, `Renderer::last_pass_times`; `loom run --frames n` | 🔧 env-var only | A stats overlay |
| Telemetry CSV per captured frame | `loom_cli/src/telemetry.rs:24` (`Probe`) | 🔧 CLI-only | — |

---

## The gap list — what is CLI-only, and therefore what the editor must close

Ranked by how much already-built engine behaviour is stranded behind each.

1. **Prefabs, entirely.** Not resolved on load (`scene_view.rs:110`), no override display,
   no revert/apply/unpack, no browser. Three `SceneOp`s and a whole `loom prefab` command
   exist and none is reachable. *Also fix `loom explode` (`main.rs:3440`) while you are there.*
2. **Terrain.** Voxel ops are display-only, recipes are a file the editor never opens,
   `loom terrain`'s four metrics — the only assertable feedback a landscape has — are
   invisible, and `loom explode` is the only destructive path.
3. **Every string and every nested value in the inspector** (`panels.rs:877-895`): script
   paths, rules paths, HUD text, mesh and texture aliases, water kind, ground layer,
   wave sets, pontoons, scatter excludes. One `match` arm's worth of gap covering roughly
   a third of the schema.
4. **Semantic placement** — `loom place --op` (`place.rs:102-131`) has snap-on-top,
   align, face-toward and grid, and the editor offers none of them.
5. **The event log.** `EventLog` is a replay (`loom_script/src/lib.rs:340-343`) and has no
   viewer anywhere.
6. **Navigation.** Bounds hardcoded at `play.rs:676-683`, never rebaked, no overlay,
   no component.
7. **Physical sanity warnings** (`sanity.rs:48`) run in `loom validate` and never reach the
   console panel where a human would act on them.
8. **The probes**: `loom sim --assert` (wind, rain, exposure, wetness), `loom water --at`,
   `loom measure`, `loom audio`. Each answers a question the viewport cannot, and none
   has a click-in-the-world affordance.
9. **Debug view modes do not exist at all.** CLAUDE.md recommends `ids` and `collision`;
   the renderer has neither.
10. **Asset import** — no command, no UI, and `loom_asset::meta`'s id/manifest machinery
    (`meta.rs:21-160`) is dead code that a Hub and a project system will need.
11. **Input bindings** load from a cwd-relative path (`run.rs:2243`) — breaks the
    `exe + assets/` ship target and gives a project no way to own its bindings.

## What has no engine behind it at all

Distinct from the list above: these are **new engine features**, and each needs an answer
to "how does this become diffable text" before any UI is drawn.

- **Material-layer / splat painting** — `GroundLayer` is a slope rule, not a mask.
- **UV texture painting** — meshes have UVs (`mesh.rs:18`), nothing writes to a texture.
- **Vertex-colour painting** — `Vertex` has no colour channel (`mesh.rs:12-19`), so this
  is a vertex-layout change, a shader change, and a serialization question all at once.
- **Decals** — nothing, anywhere. `RayHit.normal` (`loom_physics/src/lib.rs:36`) is the
  only thing in the codebase that anticipates them.
- **Project Hub / project creation / `loom new`** — no subcommand exists (USAGE, `main.rs:36-140`).
- **Runtime/editor split for shipping** — the editor is `loom_cli` modules
  (`panels.rs`, `run.rs`, `gizmo.rs`, `hud.rs`, `scene_view.rs`, `materials.rs`), not a
  separable crate; `run.rs` mixes editor UI, fly camera, play mode and HUD drawing in
  2,312 lines. Stripping the editor from a runtime build is a crate-boundary problem
  before it is a build-flag problem.
- **Snapping and grid** — no snap anywhere in `gizmo.rs` (280 lines, no grid constant).

## Two constraints the new editor inherits whether it likes them or not

**Every write is a whole-file re-emit through a format-preserving DOM**
(`ops.rs:217-232`, `scene.rs:1-6`). Comments and key order survive; that is the
mechanism behind never-do #15. It also means there is no partial write, no streaming,
and no cheap way to make a 60 Hz brush stroke anything other than a gesture-coalesced
sequence of whole-document rewrites (`edit.rs:267-295` already does exactly that for
gizmo drags, so the pattern holds — but a paint stroke carries far more payload than
three floats).

**The type registry is the inspector, and it only walks a component's top level**
(`scene.rs:638` notes this explicitly; `panels.rs:786-811` reads `properties`,
`description` for tooltips, and `minimum`/`maximum` for slider bounds from
`#[schemars(range(...))]`). That is a genuinely good property — *"writing a good doc
comment, teaching the agent, and labelling the editor are all one act"*
(`panels.rs:797-799`) — and the new editor should keep it rather than hand-writing
per-component UI. The fix for nested values is to make the *walker* recursive, not to
abandon generation.
