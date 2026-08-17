# Design — material-layer (splat) painting and vertex-colour painting

*Editor rework, design phase. Sibling documents cover UV texture painting and decals; the
**brush model, the stroke schema and the transaction shape in §1–§3 are shared with them**
and are written here once. Everything cited as `file:line` was read in this worktree at
`62f9ebe`. Nothing was built or run — see §12.*

---

## 0. The shape of the answer, in one paragraph

**Both systems store a stroke as a polyline plus brush parameters, in the scene text, and
derive every texel and every vertex value from it on load.** That is not a compromise
forced by the diffability rule; it is the same move `VoxelVolume.ops` already makes
(`assets/test/cave.loom:30-57` is forty lines standing for two million voxels) and the
same move `Grass` and `Scatter` make. The two systems differ only in *where the derived
value lands*: splat painting bakes an `RGBA8` control texture that the fragment shader
samples to bias the existing slope blend; vertex painting bakes a parallel `u32`-per-vertex
array that the vertex shader multiplies into albedo. One brush, one stroke vocabulary, one
gesture key, one transaction shape, two bakers. **Neither needs a new `SceneOp`** — a
stroke list is an array field on a component, and `SetField` already replaces arrays
(`crates/loom_scene/src/ops.rs:70-74`; the voxel CLI does exactly this at
`crates/loom_cli/src/main.rs:369`).

---

## 1. The shared brush

```rust
// crates/loom_paint/src/lib.rs
pub struct Brush {
    /// Footprint radius in WORLD METRES.
    pub radius: f32,
    /// Where the falloff starts, as a fraction of `radius`. 1.0 is a hard
    /// edge, 0.0 is a cone with no flat centre.
    pub hardness: f32,
    /// The value a fully-covered texel converges to under repeated dabs.
    pub strength: f32,
    /// How much of the remaining distance to `strength` one dab closes.
    pub flow: f32,
    /// Distance between dabs along the stroke, as a fraction of `radius`.
    pub spacing: f32,
}
```

**Radius is in world metres and not in screen pixels.** A screen-space radius cannot be
serialised into a stroke that reproduces — the same text would paint a different footprint
from a different camera, which breaks the property that the agent and the human authoring
the same thing produce the same diff (brief §M12 exit criterion 1). The editor shows the
metre value and lets `[`/`]` scale it; the *cursor* is drawn at whatever that projects to.

**Falloff, coverage and accumulation, spelled out because three plausible formulas differ
visibly:**

```
w      = smoothstep(1.0, hardness, dist / radius)      // 0 at the rim, 1 inside
value := value + (strength - value) * flow * w         // per dab
```

Accumulating *toward* `strength` rather than adding to it is what makes a slow drag and a
fast one converge to the same place instead of the slow one blowing past it, and it is what
makes `strength = 0` a perfectly good eraser. **So there is no `mode = "erase"` field** —
erasing is painting toward zero, and a separate mode would be a second spelling of the same
arithmetic.

**Dabs are derived, never stored.** The stroke holds the polyline the pointer traced; the
baker walks it at `spacing * radius` intervals and stamps. Storing the dabs would multiply
the text by ten and would freeze `spacing`, so re-authoring a stroke with a wider brush
could not re-space it.

### The stroke, as text

```toml
  [node.components.SplatPaint]
  texels_per_meter = 4.0

    [[node.components.SplatPaint.strokes]]
    radius = 1.5
    hardness = 0.5
    strength = 1.0
    flow = 0.6
    spacing = 0.25
    points = [[12.34, 4.51], [12.71, 4.63], [13.09, 4.80]]
```

Typed, not `serde_json::Value`. `VoxelVolume.ops` is untyped because it is a *union* of five
shapes behind a `kind` discriminator (`crates/loom_scene/src/components.rs:358-480` — the
doc comment is the schema, and validation happens in exactly one place). A stroke list has
one shape, so it gets `#[derive(JsonSchema)]` and validates through the registry like
everything else. That is strictly better than the precedent and worth not copying blindly.

**Points are quantised to millimetres when the editor captures them.** `f32` round-trips
shortest-form (ADR 0008 `:109-112`), so an un-quantised drag writes `12.339999` and a
quantised one writes `12.34`. A 40-point stroke is one line either way; the difference is
whether a human can read the diff.

---

## 2. A continuous stroke becomes one transaction

Nothing new is built here. `Session::apply_coalescing` (`crates/loom_scene/src/edit.rs:282-297`)
already collapses a run of transactions sharing a gesture key into one undo entry, and
`gesture_epoch` already bumps on every mouse-button release (`crates/loom_cli/src/run.rs:898`),
which is what makes letting go and re-painting a second undo step.

- **Press.** Raycast (§8), start a new `SplatStroke` with the current brush, push its first
  point. Issue `SetField { node, "SplatPaint.strokes", <whole array> }` through
  `apply_coalescing` with key `paint:{node}:splat:{gesture_epoch}`.
- **Drag.** Append a point **only when the pointer has travelled `spacing * radius` in
  world space since the last one.** Re-issue the same `SetField`.
- **Release.** Nothing. The epoch bump ends the run.
- **Label.** `format!("Paint {node}: stroke of {n} points")` — it lands in the log panel
  and in git history, so it says what and where.

**Decimation is what makes the transaction rate sane, and it falls out of the brush model
rather than being a special case.** At the default `spacing = 0.25` and a 1.5 m brush, a
point lands every 0.375 m of world travel — a brisk drag produces on the order of ten
transactions a second, not sixty. A gizmo drag already issues one per frame and is fine
(§4 of the existing-editor survey), so this is *less* write pressure than shipped
behaviour, not more.

**Each of those transactions still re-emits the whole document.** That is unavoidable —
`Applied::undo` is the entire previous scene text (`ops.rs:126-128`) and there is no partial
write anywhere in this project. The measurement to take before the design is called
finished is `Session::apply` wall time on `assets/games/proving_ground.loom` (the largest
scene here) with a 200-point stroke sitting in the array. If it is over ~5 ms the answer is
to raise `spacing`, not to invent a second write path.

**Three behaviours that must be got right and are easy to miss:**

*A version-token rejection mid-stroke abandons the stroke.* `transact_as` already reloads
rather than forcing (`run.rs:1743-1753`). The editor must also drop the live preview mask
and re-bake it from the reloaded text, or the viewport shows paint the file does not have.
Never force, never merge (never-do #15).

*Painting a prefab instance writes an override of the whole stroke array.* That is ADR
0008's routing working as designed (`SetField` on an instance becomes `[node.overrides]`),
and the inspector needs no branch for it. The consequence is that each painted instance
carries its own full copy of its strokes, which is correct — they *are* different — and
worth saying out loud in the docs so nobody reports it as bloat.

*Undo mid-stroke.* `Session::undo` clears `self.gesture` (`edit.rs:315`), so a Ctrl+Z with
the mouse still down ends the run and the next frame starts a new undo entry. That is the
right behaviour and needs no code; it needs a test.

### Live preview without re-baking from text

Re-deriving a 1024² mask from sixty strokes on every one of those transactions is the
difference between an editor and a slideshow — the phrase is `MeshLibrary::with_cache`'s
own (`crates/loom_cli/src/main.rs:1066-1069`), about exactly this. Two mechanisms, both
copied from things already here:

1. **A bake cache keyed on a hash of the stroke list**, alongside `VoxelCache`,
   `grass_key` and `terrain_key` (`run.rs:547-619`). Equal strokes, equal raster, by
   construction.
2. **An incremental fast path**: if the new stroke list is the old one with its *last*
   stroke extended, stamp only the new dabs into the cached mask. This is
   `Volume::edit`'s dirty-chunk shape (`crates/loom_voxel/src/lib.rs:1227`) — live edit is
   incremental, the persisted result is still the appended text.

Both are small. (2) is about fifteen lines and is what keeps the stroke interactive.

### CLI parity

**No new subcommand is needed for the agent to paint.** A stroke list is a JSON array on a
component, so `loom scene --tx '{"op":"set_field","node":"Hill","field":"SplatPaint.strokes","value":[…]}'`
already works, and property 2 is satisfied the day the component exists. A convenience
`loom paint --node … --radius … --points "x,z x,z"` that *appends* rather than replaces is
worth adding the first time an agent gets the replace-the-whole-array dance wrong, and not
before.

---

## 3. Splat: what is stored, and what is derived

**Stored:** `SplatPaint { texels_per_meter: f32, strokes: Vec<SplatStroke> }`, a new
component registered in `loom_scene::registry()` (`components.rs:1693-1723`).

**Derived, never serialised:** one `loom_asset::Texture` per painted node.

The derived texture is **`RGBA8`, `ColorSpace::Linear`**, and that choice costs nothing
because `loom_asset::texture` already normalises every texture to 8-bit RGBA on purpose —
*"one GPU format for every texture means one bindless array rather than one per format
combination"* (`crates/loom_asset/src/texture.rs:164-166`). So a mask is just another entry
pushed into `MaterialLibrary::textures`, it gets a mip chain from the existing
`mip_chain(…, Linear)`, and **no new Vulkan format, sampler or descriptor exists anywhere
in this design.** An `R8G8_UNORM` mask would halve its memory and require a second bindless
array; that trade is not close.

Channel assignment:

| | |
| --- | --- |
| **R** | the painted value — 0 is the base material, 1 is `Material.layer` |
| **G** | **authority** — how much this texel was painted at all |
| **B, A** | unused, reserved for layers 2 and 3 (§5) |

**Authority is a separate channel from value, and that is the load-bearing decision in this
document.** See §4.

### Projection: top-down, world XZ

The mask maps world XZ over the node's own bounds. `texels_per_meter` defaults to 4.0 and
the resulting dimension is clamped to `256..=2048`; a 60 m terrain at 4/m is 240², which is
sixteen texels per blade-spacing and far finer than the boundary needs.

**A top-down projection cannot control a vertical face, and that is deliberate rather than
a limitation being excused.** The thing splat painting is for is ground: a worn path, a
scree fan, a patch of soil on a ledge. Vertical faces are precisely where
`groundLayerWeight`'s slope rule is already unambiguous and already right. The alternative
is a triplanar mask — three samples instead of one, on a *control* texture, to gain
authority over surfaces the rule already handles correctly. Rejected.

The mapping travels per-object, not per-material, because two nodes sharing a material
paint differently. It rides in `ObjectData` (§6).

---

## 4. How this interacts with the existing `GroundLayer` slope rule

This is the question the brief asks most sharply, and the answer is one line of shader:

```slang
float w = groundLayerWeight(in.worldPos, geometric, lm.params.x);   // unchanged
if (splatMap != NO_TEXTURE) {
    float2 m = sampleMap(splatMap, splatUv).rg;
    w = lerp(w, m.r, m.g);                                          // value, authority
}
```

**Three properties follow, and each is the reason a simpler encoding was rejected:**

**1. A scene with no `SplatPaint` renders byte-identically.** `splatMap` is `NO_TEXTURE`,
the branch is per-object uniform, and there is no sample, no multiply, nothing — the same
discipline `Material.layer` itself follows (*"a scene with no layer pays nothing — not an
identity multiply, nothing"*, `assets/shaders/scene.slang:2044-2046`) and that
`alpha_cutoff` follows. `cargo xtask image` proves it: **no existing reference should move,
and if one does the branch is wrong.**

**2. Painting is local. The wandering boundary survives everywhere you did not paint.**
`groundLayerWeight` perturbs its *threshold* with low-frequency noise (`scene.slang:1936-1957`)
because a clean curve across a hill is the synthetic tell, and `loom_grass::coverage`
learned the same thing first. A mask that simply *was* the weight would have to be
pre-seeded with the rule's output everywhere, and painting anything would then bake the
noise into a raster — after which changing `GroundLayer.slope` would do nothing on painted
terrain. **That is a procedural rule frozen into a bitmap, which is the exact failure
never-do #11 names.** The authority channel is what avoids it: untouched texels have
authority 0 and are still computed, live, every frame.

**3. `lerp(w, x, 0.0)` is exactly `w` in IEEE arithmetic**, so even a node that carries a
`SplatPaint` with an all-zero mask — one stroke, then undone — is bit-identical to one
without. The branch buys the cost, not the correctness.

### The grass ring, which this would otherwise reintroduce

`GroundLayer.slope` and `Grass::slope_cutoff` are documented as needing to match, because
*"grass stopping at one angle while the rock beneath it starts at another draws two
concentric rings around a hill"* (`components.rs:262-266`; the shader repeats the constants
at `scene.slang:1934-1940`). **Painting rock under grass recreates that mismatch pointing
the other way** — rock in the shading, grass still growing on it.

The fix is one line and needs no new API, because the seam already exists.
`loom_grass::tile` takes ground as `&dyn Fn(f32, f32) -> Ground`
(`crates/loom_grass/src/lib.rs:315`) and `Ground.rock` is *"1 where the ground is bare
rock"* (`:149`), which `coverage` already folds into density. The CLI's `GroundGrid`
currently hands back a hard-coded `rock: 0.0` (`crates/loom_cli/src/main.rs:1589`, with a
comment saying nothing in the schema drives it). Make it:

```rust
loom_grass::Ground { height: h, normal, rock: painted_rock(x, z), flow }
```

where `painted_rock` samples the same CPU mask the baker just produced. **`loom_grass`
gains no dependency on `loom_paint`** — the closure is the seam, exactly as it is for
`loom_voxel`. Scenes with no paint return 0.0 and are unaffected, so `meadow` and
`grass_slope` do not move and nothing needs re-blessing.

---

## 5. Layer count: two now, four later, and why not four now

The engine has exactly one optional second material (`Material.layer: Option<GroundLayer>`,
`components.rs:227`). **Slice 1 paints that blend and nothing else**, because it is the
whole shader change and it covers the dominant use — paint the path, paint the outcrop,
erase the rock off the ledge you want to walk on.

Growing to four layers is additive and needs no rewrite of anything above: add
`Material.layers: Vec<GroundLayer>` (leaving `layer` alone — renaming it is a `format` bump
and a migration function, `docs/format/README.md:395-407`, and adding a field is free), add
`layer: u8` to `SplatStroke` with a default of 1, and use the mask's B and A channels. The
encoding question — four weights and no authority, versus two layers with authority each —
is real and unresolved, and it is why the four-layer version is not designed here on
speculation.

---

## 6. Splat: the GPU plumbing, exactly

**`ObjectData` grows by 16 bytes** (`crates/loom_render/src/renderer.rs:660-676`, mirrored at
`assets/shaders/scene.slang:34-59`). It is a storage buffer, not push constants, so this is
free; the discipline is that the two declarations are one layout described twice and a
mismatch is silent.

- `material: [u32; 4]` — `x` is the material index today and `y`/`z`/`w` are stated padding
  (`renderer.rs:673-675`). **Take `y` for the splat mask's bindless slot**, `NO_TEXTURE`
  when unpainted. Zero size change.
- **New: `splat: [f32; 4]`** — world XZ origin in `xy`, reciprocal extent in `zw`. Appended
  after `material`, taking `ObjectData` from 240 to 256 bytes.

**`VSOutput` gains `nointerpolation uint object : OBJECT`** (`scene.slang:738-751`), set in
`vertexMain` to `push.objectOffset + instanceID` (`:876-878`). The fragment stage currently
receives `material` but not the object index, and needs the object to reach `splat`. One
`nointerpolation uint` is the cheapest way to give the fragment stage everything on the
object rather than adding a varying per feature — and **UV painting and decals will both
want it**, which is the only reason it is worth adding rather than passing the two floats
directly.

The fragment change is the four lines in §4, inside the existing
`if (m.maps.w != NO_TEXTURE)` block at `scene.slang:2044-2071`.

**Entry points touched: `vertexMain` and `fragmentMain` only.** Not `grassVertexMain`,
`waterVertexMain`, `rainVertexMain`, `particleVertexMain` or `skyVertexMain`.

### The prerequisite this design does not create but does depend on

**`Viewer` has no way to update materials or textures after construction.** `Materials::new`
uploads every texture in the constructor with a one-shot submit
(`crates/loom_render/src/material.rs:128-135`, `record` at `:436`), everything in the module
is `pub(crate)`, and `Viewer`'s public surface has `set_meshes`, `set_grass`, `set_terrain`,
`set_rain*` and no `set_materials` (`crates/loom_render/src/viewer.rs:668-921`). As far as
reading shows, **an inspector edit to `Material.roughness` in `loom run --edit` does not
reach the GPU today** — that is a pre-existing gap, not one painting introduces, and
painting cannot ship without closing it.

The narrow fix: `Viewer::set_material_texture(slot, &loom_asset::Texture)`, reusing
`material::record`'s one-shot submit — which is explicitly framed as *"initialisation work
that must finish before the first frame, not per-frame work the graph schedules"*
(`material.rs:433-435`). A mask update happens on a stroke transaction, roughly ten times a
second, outside the render loop. **If it is ever recorded into the frame's own command
buffer it becomes a render-graph pass with a declared `Access::TransferWrite`, never a
hand-placed barrier (never-do #4).** A 240² RGBA8 mask with mips is ~300 KB against a
measured 13.5 GB/s bus, so whole-mask re-upload is the right first answer and dirty-rect
copies are the upgrade if masks ever exceed 2048².

---

## 7. Vertex colour: where the bytes actually go

Three facts decide this, and none of them is the one the constraints survey assumed.

**Fact 1: the GPU vertex is not `loom_asset::Vertex`.** It is `PackedVertex` — twelve
bytes, three `u32`s: 10-10-10 position, octahedral normal, 16-16 UV
(`crates/loom_asset/src/packed.rs:29-43`), with size and alignment pinned by a test
(`crates/loom_render/src/lib.rs:590-591`). The 40-byte `Vertex` is a CPU authoring type that
`combine` packs on upload (`renderer.rs:2941-2946`). So "add a colour channel to `Vertex`"
costs nothing on the GPU by itself, and the real question is what happens to `PackedVertex`.

**Fact 2: widening `PackedVertex` taxes every scene.** 12 → 16 bytes is +33% vertex-fetch
bandwidth on `primitives`, `cave`, `lanternhead` and everything else, forever, for a feature
most of them never use. Rejected.

**Fact 3: the push block has exactly one slot left.** `size_of::<Push>()` is pinned at
**120** bytes against Vulkan's guaranteed 128 (`crates/loom_render/src/rain.rs:717-718`) —
so one more 8-byte device address fits, and it is the last one. (The doc comment at
`renderer.rs:626` says "124 of its 128 bytes"; the test says 120. The test is the one that
runs.)

### The design

**A parallel `u32`-per-vertex array, reached by a new push-constant pointer, null when
nothing in the scene is painted.**

```
// scene.slang, Push
uint* vertexColors;   // RGBA8; null when no node in this scene paints
```

`combine` (`renderer.rs:2918-2962`) emits it alongside the vertex and index buffers, in the
same order and the same length, zero-filled for unpainted meshes. `vertexMain` reads
`push.vertexColors[vertexID]` when the pointer is non-null and emits `float3(1,1,1)`
otherwise; the fragment stage multiplies it into `albedo`. **Multiplying by exactly 1.0 is
exact** — the codebase already leans on this for `mean_of`'s white default
(`crates/loom_cli/src/materials.rs:33-42`) — so unpainted scenes stay byte-identical without
a branch in the hot path, and the null check keeps the *buffer* from existing at all.

Cost when present: 4 bytes per vertex across the whole combined library. A million-vertex
voxel scene pays 4 MB, once, on a card with 24 GB. One extra `float3` varying and one extra
multiply.

**Spending the last push slot is a real decision and the alternative should be recorded.**
The documented overflow is the environment buffer — *"wind and the camera position both live
in the environment buffer because the push block is at its 128-byte guarantee"*. Putting the
address there instead means storing a `uint64_t` in a buffer and casting it to a pointer in
Slang, which works but is more exotic than a push-constant pointer and is not something I
can verify without building. **Take the push slot; if a later feature needs one, this
pointer is the one that moves**, because it is read once per vertex and the cast cost is
amortised over a whole mesh.

### The per-node problem, and the precedent that solves it

`MeshLibrary` keys meshes by asset alias (`main.rs:1047-1050`), so forty nodes with
`mesh = "box"` share one entry — and one colour array indexed by absolute `vertexID`
therefore cannot give them different paint.

**A painted node gets a private mesh entry, exactly as a voxel volume already does.**
Voxel meshes are keyed `voxel:<node>` and the asset panel disables them with a tooltip
saying they are baked from that node's op list (`panels.rs:543-574`). `VertexPaint` gets
`paint:<node path>` on the same rule. Consequences, stated rather than discovered:

- Painting a shared prop **duplicates its vertices**. For a 500-triangle crate that is
  nothing; for a 200k-triangle imported mesh painted on twelve instances it is 2.4 M
  vertices. `ponytail: private mesh copy per painted node; if that ever hurts, the answer is
  a per-object colour pointer in ObjectData plus a first-vertex offset, not a smarter cache.`
- **Voxel terrain pays nothing extra** — its mesh is already private — and voxel terrain is
  the case with no UVs and therefore the strongest argument for vertex colour existing at
  all.
- The mesh library's `key()` (`main.rs:1206-1224`) hashes names, sizes and bounds. Paint
  changes none of those, so **`key()` must fold in the paint hash or the viewport will not
  follow a stroke.** This is the most likely way to ship vertex painting that silently does
  nothing.

### What vertex colour is, semantically

**A tint multiplied into albedo, and nothing else.** It cannot brighten past the authored
albedo, which is the standard limitation of multiplied vertex colour and is the correct
trade for a system whose job is dirt in crevices, damp at the base of a wall, and colour
variation across scattered rock. Driving the layer blend from vertex colour was considered
and rejected: that is what §4 is for, and two mechanisms answering one question is how a
scene ends up with two contradictory boundaries.

Storage mirrors splat exactly — `VertexPaint { strokes: Vec<VertexStroke> }`, where a
stroke carries `color: [f32; 3]` and `points: Vec<[f32; 3]>` in **world** space. Points are
3D because a mesh wraps in three dimensions and a top-down projection would paint a wall's
near and far faces identically. A dab is therefore a sphere, and **a sphere paints through a
thin wall.** `ponytail: sphere dab, bleeds through walls thinner than the brush; add a
per-stroke normal limit if it bites.`

---

## 8. Both systems on voxel meshes, which have no UVs

Surface Nets places a vertex anywhere in its cell and produces no UVs at all, which is why
voxel materials are triplanar (`components.rs:163-170`).

**Splat painting is unaffected, because its mask was never in UV space.** The world-XZ
projection is the same projection triplanar's dominant axis already uses on flat ground, so
a mask and a triplanar albedo agree by construction. This is the case splat painting is
*for*.

**Vertex painting is unaffected because it never touches UVs.** Colour rides on the vertex,
which Surface Nets produces plenty of. It is the only one of the four painting systems that
works on voxel terrain with no caveat at all, and that is the honest argument for building
it alongside splat rather than instead of it.

**UV texture painting is the one that cannot work here**, and the sibling design owes that
statement rather than this one.

### Picking a surface point to paint

Both need a world hit with a normal, and AABB picking (`run.rs:2002-2030`) cannot give one.
The lazy answer that is also the right one: **the brush paints the selected node, so the
raycast is against one mesh.** Brute-force ray-vs-triangle over the selected node's mesh is
~50k tests for a large mesh, well under a millisecond, and it needs no BVH, no ID buffer and
no GPU readback. It also works for nodes with no collider, which `loom_physics`'s raycast
would not. `ponytail: brute-force ray-vs-triangle over one mesh; add a BVH when a painted
mesh exceeds ~200k triangles.`

---

## 9. Brush preview in the viewport

Drawn with the existing machinery, into egui's background layer, exactly where the gizmo
handles go (`panels.rs:701-739`).

`gizmo::View` already projects world to screen and is tested as the inverse of its own ray
(`gizmo.rs:8-10`, `:211-224`) — **it is the shared projection and painting must not grow a
second one.** The cursor is the brush circle projected onto the surface: sample 32 points on
the world-space circle of radius `radius` around the hit, each dropped onto the surface by
the same query the raycast used, project each with `View::project`, and stroke the polyline.
That reads correctly on a slope, where a flat screen-space circle lies.

Two rings, not one: the outer at `radius`, the inner at `hardness * radius`, so the falloff
is visible before the stroke rather than after it. A live readout — `1.5 m · 60%` — beside
the cursor, because the radius is a world quantity and no other affordance says so.

**The preview must be suppressed while Play runs**, with the rest of the editing keys
(`run.rs:2042-2044`), and it must not be drawn when the divergence banner is up.

---

## 10. Files touched

**New crate `loom_paint`** — CPU only, depends on `loom_asset` and nothing else, imports no
`ash` and no egui. `Brush`, dab walking, `bake_splat(strokes, bounds, texels_per_meter) ->
loom_asset::Texture`, `bake_vertex(strokes, &Mesh) -> Vec<u32>`, `stamp_incremental` for the
live path, and a `sample(x, z) -> f32` used by the grass hook. Two round-trip tests: a
stroke replayed twice is byte-identical, and a stroke plus its inverse-strength twin returns
a texel to within one LSB of where it started.

| File | Change |
| --- | --- |
| `crates/loom_scene/src/components.rs` | `SplatPaint`, `SplatStroke`, `VertexPaint`, `VertexStroke`; two lines in `registry()` (`:1693`) |
| `crates/loom_paint/` | new crate, above |
| `crates/loom_cli/src/materials.rs` | bake each node's mask, push it into `textures`, record its slot and world extent |
| `crates/loom_cli/src/scene_view.rs` | carry per-object splat slot + extent; carry the paint hash |
| `crates/loom_cli/src/main.rs` | `MeshLibrary`: `paint:<node>` private copies; fold paint into `key()` (`:1206`); `GroundGrid` feeds painted rock into `Ground` (`:1589`) |
| `crates/loom_cli/src/run.rs` | the paint tool: press/drag/release, decimation, `apply_coalescing`, live stamp, cursor |
| `crates/loom_render/src/renderer.rs` | `ObjectData.splat`; `material.y`; `Push.vertex_colors`; `combine` emits the colour array |
| `crates/loom_render/src/viewer.rs` | `set_material_texture`, `set_vertex_colors` |
| `crates/loom_render/src/material.rs` | make the texture-update path reachable |
| `crates/loom_render/src/rain.rs` | re-pin `size_of::<Push>()` 120 → 128, with a comment that this is the last slot |
| `assets/shaders/scene.slang` | `ObjectData.splat`; `Push.vertexColors`; `VSOutput.object` + `.paint`; four lines in the layer block |
| `assets/test/painted.loom` | new golden scene |
| `xtask/src/main.rs` | `painted` in `SCENES` (`:41`) **and** `GOLDEN` (`:253`) |

**One golden scene, two nodes** — a painted voxel slope and a vertex-painted mesh — rather
than two scenes. The rule is coverage of rendering paths, not a scene per feature, and both
branches are in `fragmentMain`. If a regression ever proves hard to attribute between them,
split it then. `cargo xtask flythrough` needs nothing new: paint is static geometry with no
motion failure mode, which is the one pleasant thing about this phase.

---

## 11. Slices

1. `loom_paint` + the two components + `loom scene --tx` authoring, no rendering. Test: a
   stroke bakes to a mask with the expected texels.
2. Splat renders. `ObjectData`, the shader, the `Viewer` texture-update path, `painted.loom`,
   golden bless. **Every other reference must be unmoved** — that is the check that the
   branch is right.
3. The paint tool in the editor: cursor, decimation, coalescing, live stamp. Test: one drag
   is one Ctrl+Z; two drags are two.
4. The grass hook (`Ground.rock`), so the boundary stops contradicting itself.
5. Vertex colour: the parallel array, the push pointer, the private mesh copies, the paint
   hash in `key()`.

Slices 1–3 are shippable on their own and 4 is the smallest of them. Slice 5 is separable
and is the one to drop if the schedule bites, because §4 covers most of what it does.

---

## 12. ADRs this needs

**ADR: painted surface data is a stroke list, rasterised on load.** Covers §C of the
constraints survey and, by construction, §B as well.

> *Decision.* Material-layer and vertex-colour painting store the **stroke** — a world-space
> polyline plus brush radius, hardness, strength, flow and spacing — as a typed array field
> on a scene component, and derive the splat mask and the per-vertex colour array from it on
> load. No raster and no per-vertex colour is ever serialised. This satisfies
> `LOOM-IMPLEMENTATION-ORDER.md:455-457` (*"painted regions serialize as polygons or splines,
> never bitmaps"*) rather than seeking an exemption from it, and it makes a stroke undoable
> through `SetField` with no new `SceneOp`. **The vertex format does not change**: colour
> travels in a parallel `u32` array reached by the last free push-constant pointer, so
> `PackedVertex` stays 12 bytes and unpainted scenes pay nothing. Painting is
> rendering-only and outside the sim hash, the same exemption grass and rain hold — it
> changes no vertex position, so colliders, navigation and physics are untouched.
> *Rejected:* storing the raster with a stated exemption from undo (breaks Ctrl+Z, breaks
> agent authoring, invalidates the `.meta` content hash on every stroke); widening
> `PackedVertex` to 16 bytes (+33% vertex bandwidth on every scene for a feature most never
> use); a `SplatWeights` blob field (a bitmap in a text file is still a bitmap).

**ADR: the splat mask biases the slope rule; it does not replace it.**

> *Decision.* The mask carries a painted **value** and a painted **authority**, and the
> shader computes `w = lerp(groundLayerWeight(…), value, authority)`. Where nothing was
> painted, authority is zero and the existing slope rule — including the low-frequency
> wander that stops it drawing a shaved ring — is evaluated live, every frame, unchanged. A
> mask that simply *was* the blend weight would have to bake the rule's output into a raster,
> after which `GroundLayer.slope` would no longer do anything on painted terrain: a
> procedural rule frozen into a bitmap, which is never-do #11's failure with a different
> noun. `loom_grass`'s `Ground.rock` is fed from the same mask through the existing `&dyn Fn`
> closure, so painting rock also removes grass and the two boundaries cannot disagree.

The `Viewer` texture-update path (§6) is **implementation under never-do #4, not an ADR** —
it adds no pass and no hand-placed barrier as long as it stays a one-shot submit like
`material::record` already is. It becomes ADR territory the moment it is recorded into the
frame's command buffer.

The sibling documents own ADR §A (UV painting) and §D (decals). **Nothing here depends on
either landing**, and that is deliberate: splat plus vertex colour is a complete answer to
"vary the surface of terrain and large meshes" on its own.

---

## 13. What I could not verify

Design phase; no builds were run, per the brief. Everything below is a real gap, not a
hedge.

1. **Whether Slang accepts a `uint*` in the push block alongside the six pointers already
   there**, and whether `size_of::<Push>()` at 128 hits a driver limit rather than the
   guaranteed minimum. The RTX 4090 reports far more than 128 bytes, so this should be
   fine, but the test at `rain.rs:718` asserts `<= 128` for portability and the new value
   sits exactly on it.
2. **Whether an inspector edit to a `Material` field really does nothing in `loom run --edit`
   today.** I traced `Viewer::new` → `Materials::new` and found no update path and no
   `set_materials` in `Viewer`'s public surface, and `App::show` (`run.rs:507-539`) calls
   only `set_meshes`, `set_terrain` and `upload_grass`. I did not run the editor and change a
   roughness slider. If some other path re-creates the `Viewer`, §6's prerequisite is
   smaller than I have made it.
3. **The transaction cost of a whole-document rewrite with a 200-point stroke in the array.**
   §2 names the measurement and the threshold; I have not taken it. The existing gizmo drag
   is the evidence that the *rate* is affordable, not that the *payload* is.
4. **Whether 4 texels/metre is enough.** It is sixteen texels per blade-spacing at the
   default grass density, which reasons out, but the boundary between soil and rock is
   exactly where an under-resolved mask shows stair-stepping and nobody has looked at one.
5. **Whether `RGBA8` mip filtering on a control texture reads acceptably.** Averaging an
   authority channel down the chain is not obviously wrong, but a half-authority texel at
   distance blends toward the slope rule in a way that could pop. `cargo xtask shimmer` at
   the authored camera would answer it; I could not run it.
6. **Whether `smoothstep(1.0, hardness, r)` is the falloff a human wants.** It is the
   defensible default. Brush feel is judged by hand and this one has not been.
7. **Whether the `paint:<node>` mesh-copy scheme survives contact with `mesh_key`'s cache
   invalidation.** I have named the trap (§7) but not walked every caller of
   `MeshLibrary::key`.

---

## 14. What I deliberately did not design

**Layer counts above two** (§5), **a brush-preset library**, **pressure or tilt input**,
**symmetry/mirror painting**, **a paint-layer stack with visibility toggles**, **baking a
stroke list down to a flattened raster for shipping**, and **any painting of `Grass`,
`Scatter` or `Wind` fields**. Each is a real feature and none of them changes the shape of
what is above; every one of them is cheaper to add after a human has used slices 1–3 for an
afternoon than to guess at now.

**A `Brush` trait.** There are two bakers, and they share a struct and two free functions.
Never-do #12.
