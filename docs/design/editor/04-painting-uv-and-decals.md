# Design — true UV texture painting, and decals

*Editor rework, design phase. Read `00-survey-existing.md`, `00-survey-engine-surface.md` and
`00-survey-constraints.md` first; this document assumes all three. Every `file:line` citation was
read in this worktree at `62f9ebe`. Nothing here was built or run — see §12.*

---

## The two sentences

**A painted texture is a stroke list in the scene text, rasterised on the CPU into an ordinary
bindless texture, and the live brush preview is deliberately not a transaction.** That last clause
is the whole design: the viewport does not derive the painted image from the scene text while the
stroke is in progress, so a stroke commits *once*, on mouse-up, as one `SetField` — one
transaction, one Ctrl+Z, no gesture coalescing needed at all.

**A decal is a box projector evaluated inside the existing forward fragment shader.** No pass, no
pipeline, no barrier, no G-buffer — the same shape ADR 0019 used for secondary rays. It costs one
loop in `fragmentMain`, it is MSAA'd and HDR because it happens before the resolve and before the
tonemap, and it is invisible in ray-traced reflections. That last limit is real and is stated
plainly in §10.

Neither feature adds a `SceneOp`. Both are `SpawnNode` + `SetField` against new components, which
is the outcome never-do #16 wants and the reason this design is short.

---

# Part I — UV texture painting

## 1. Where the paintable texture lives, and how it is created

### 1.1 The authored artifact is a component, not a file

```toml
[[node]]
name = "quay_wall"
[node.MeshRenderer]
mesh = { asset = "wall_10m" }
[node.Material]
albedo_map = { asset = "concrete" }
uv_scale = [8.0, 4.0]

[node.PaintLayer]
resolution = 1024
strokes = [
  { kind = "stroke", color = [0.42, 0.13, 0.11], radius = 0.031, hardness = 0.6, flow = 0.9,
    points = [[0.21, 0.44], [0.24, 0.46], [0.29, 0.45]] },
  { kind = "stamp", color = [0.02, 0.02, 0.02], radius = 0.012, hardness = 1.0, flow = 1.0,
    at = [0.55, 0.31] },
  { kind = "erase", radius = 0.02, hardness = 0.4, flow = 1.0, points = [[0.22, 0.45]] },
]
```

The new component, in `crates/loom_scene/src/components.rs` beside `Material`:

```rust
/// Paint applied to this node's mesh, in its own UV space.
pub struct PaintLayer {
    /// Texels per axis. Powers of two only. See the density rule in §7.
    #[schemars(range(min = 64, max = 4096))]
    pub resolution: u32,
    /// Ordered marks. Order matters: later strokes paint over earlier ones.
    ///
    /// **This doc string is the whole schema for a stroke**, exactly as
    /// `VoxelVolume::ops`' is — `strokes` is `Vec<serde_json::Value>`, so a
    /// generated JSON Schema says only "array".
    pub strokes: Vec<serde_json::Value>,
}
```

`strokes` is untyped JSON **on purpose and by precedent**: `VoxelVolume.ops` is
`Vec<serde_json::Value>` for the same reason (`components.rs:358-481`), and the format spec already
treats op lists as a vocabulary specified in the component's doc comment rather than in the schema
(`docs/format/README.md:411-422`). The cost of that precedent is also known and must be repeated
here: `invalid_voxel_op` exists because layer 1 never looks inside the array, so **the stroke
vocabulary is validated in exactly one funnel, `loom_asset::paint::parse_strokes`, called by
`loom validate` and by every rasterisation** (`docs/format/README.md:328-345` records the four
silent failures that funnel was created to stop). A `deny_unknown_fields` typed enum behind
`serde_json::from_value` gives the same enumerate-the-valid-fields error message that makes an
untyped array discoverable at all.

**Coordinates are the mesh's own `0..1` UV space, and `radius` is in UV units, not texels.** So a
scene's strokes survive a change of `resolution` — re-authoring a wall from 1024 to 2048 sharpens
the paint instead of moving it. This is the same choice `VoxelVolume` made in putting op
coordinates in the volume's own space rather than in world space (`components.rs:366-371`).

### 1.2 The texture is derived, never authored, and never on disk

There is no PNG. `loom_asset::paint::rasterise(&strokes, resolution) -> loom_asset::Texture`
produces the image, in linear light, with the full mip chain built the way `loom_asset::texture`
already builds one — which matters, because the chain is reduced in linear light and a blit would
darken every level (`crates/loom_render/src/material.rs:293-299` explains why the existing loader
does it that way).

`MaterialLibrary::for_scene` (`crates/loom_cli/src/materials.rs:52`) is where it is called. That
function is already the single place that knows both spellings — alias and bindless slot — and it
already loads and de-duplicates textures. A `PaintLayer` on a node pushes one more
`loom_asset::Texture` into `library.textures` and records its slot against the node's entity index.

**This is what makes `loom render` and `loom run` show the same pixels**, which
`00-survey-constraints.md` §2.8 names as a class of defect this project has paid for three times.
The offscreen path and the window call the same `for_scene`; there is no editor-only paint path.

### 1.3 The GPU slot is `ObjectData.material.y`, and `MaterialData` does not change

`ObjectData.material` is `[u32; 4]` with **only `.x` used** — the other three are explicit padding
for the std430 alignment of the member that follows (`crates/loom_render/src/renderer.rs:673-675`,
mirrored at `assets/shaders/scene.slang:56-57`). Paint goes in `.y`.

That is not just the lazy route, it is the semantically correct one. **A paint layer is per node,
not per material.** Two crates sharing one `Material` must be able to carry different paint, and a
paint index on `MaterialData` would force a material clone per painted node — which then has to be
cloned again the moment the human edits the shared material's roughness.

It also avoids a fight this design does not need: `MaterialData` is 64 bytes with **every field
occupied** (`material.rs:47-80` — `albedo.w` is porosity, `params` is all four, `maps` is all four,
`meanAlbedo.w` is the alpha-test threshold), so a paint slot there means a fifth `float4`, and ADR
0021 pinned that layout with a test for exactly this reason.

Shader changes are three lines and one varying:

```slang
// VSOutput, beside `material`:
nointerpolation uint paint : PAINT;   // vertexMain: obj.material.y

// fragmentMain, immediately after the albedo-map block and BEFORE the ground layer:
if (in.paint != NO_TEXTURE) {
    float4 p = sampleMap(in.paint, in.uv);          // raw mesh UV — NOT scaled by uv_scale
    albedo = lerp(albedo, p.rgb, p.a);
}
```

**`in.uv`, not `in.uv * m.params.zw`.** Paint is authored against the mesh's `0..1` unwrap; the
material's `uv_scale` tiles the base texture and must not tile the paint. A wall whose concrete
repeats eight times still has exactly one painted layer across it. Getting this wrong is the single
most likely implementation bug in Part I and is worth an assertion in the golden scene: `paint_wall`
authors `uv_scale = [8, 4]` precisely so a regression tiles the paint eight times and the image gate
sees it.

**Before the ground layer, deliberately**: `Material.layer` is the steep-slope blend
(`components.rs:227`), and a painted mark should be overpainted by the rock that takes over on a
cliff, not survive on top of it. That is a judgement call; the alternative — paint last, over
everything — makes paint a decal-like overlay and is what §10's decals are actually for.

The branch is wave-uniform: `paint` is `nointerpolation`, so every lane in a wave reads the same
index and **a scene with no `PaintLayer` pays a compare and nothing else** — the same property
`m.maps.w` was given for the ground layer (`scene.slang:2038-2041`).

## 2. Objects without UVs: refuse, and say what to use instead

### 2.1 Voxel meshes have no UVs, and this is verified rather than assumed

`loom_asset::Vertex::new` writes `uv: [0.0, 0.0]`, and the field's own doc says so:
*"`[0, 0]` for geometry that has none — voxel meshes, which are textured triplanar because Surface
Nets has no surface to unwrap"* (`crates/loom_asset/src/mesh.rs:15-18`). `triplanar`'s doc in
`scene.slang:1429-1432` repeats it. `packed.rs:266` has a test named
`a_mesh_with_no_uvs_decodes_to_zero`. Painting such a mesh in UV space would put every stamp on
texel `(0,0)` and the surface would come out one flat colour.

### 2.2 The decision: refuse, do not auto-unwrap

**A `PaintLayer` on a mesh with degenerate UVs is a `loom validate` error, and the paint tool
refuses to arm on that node with a message naming the two alternatives.** The three reasons, in
order of weight:

**The mesh is regenerated whenever the op list changes, and an unwrap would change with it.** A
voxel volume's geometry is baked from `VoxelVolume.ops` (`loom_voxel::Volume::bake`), and the
editor re-bakes dirty chunks live — that is how carving a roof lets rain through on the next frame
(`00-survey-existing.md` §12). Any automatic parameterisation is a function of the triangles, so
every carve would re-atlas the surface and slide every painted texel somewhere else. There is no
version of auto-unwrap that survives destructible terrain, and destructible terrain is a locked
feature of this engine.

**The engine already has the right tool and documents it as such.** Triplanar exists precisely
because Surface Nets produces no unwrap, and it is a per-material opt-in with a stated three-sample
cost (`components.rs:163-169`). Bolting a UV atlas onto voxel meshes would be a second answer to a
question that is already answered.

**xatlas is a C++ dependency with no Rust-native equivalent of comparable quality**, which is a new
build-system surface on a project pinned to one target and cross-compiling to a second (§4.G of the
constraints survey). Not worth it for a case where the answer is "use a different tool".

**What the human is told to use instead**, both of which work on voxel meshes:
decals (Part II) project in world space and need no UVs at all — that is the bullet-hole,
scorch-mark and graffiti case; and the material-layer / splat system (design doc `03`) is the
large-area terrain-substance case. Between them they cover what a UV brush would have been used for
on terrain, and neither needs an atlas.

### 2.3 Detection is free

`loom_asset::packed::bounds` already computes the mesh's UV extent to quantise `packedUv` against
it — `uv_extent` and `uv_step`, with `uv_step = (uv_extent / UV_MAX).max(1e-9)`
(`crates/loom_asset/src/packed.rs:139-140`). A mesh whose `uv_extent` is below a small epsilon has
no unwrap. One accessor on `Mesh`, one check in `loom validate`, one guard in the tool.

### 2.4 The unwelcome surprise: the box primitive shares one unit square across all six faces

Verified, not inferred. `primitives::box`'s comment reads *"Each face gets the whole unit square, so
a texture reads at the same scale on all six regardless of how the cube is scaled"*
(`crates/loom_asset/src/primitives.rs:66-72`), and the test at `primitives.rs:273-281` asserts every
primitive spans exactly `0..1` on both axes — which is a *tiling* guarantee, not an atlas guarantee.

**So painting one face of a cube paints all six.** That is correct for tiled materials and wrong for
paint, and it is the most confusing thing a first-time user of this tool will hit. `sphere`,
`cylinder` and `capsule` have genuine unwraps (`primitives.rs:119, 160, 244`) and are fine; `plane`
is a single quad and is fine.

Two honest options, and the design picks the second:

- Re-unwrap `box` into a six-region atlas. Changes the mesh, therefore changes `mesh_key`, therefore
  re-blesses every golden image containing a box — which is most of them — and breaks every scene
  that relies on per-face tiling. Rejected as a large blast radius for a small win.
- **Ship a second primitive, `box_atlas`, whose six faces occupy a 3×2 grid of the unit square**, and
  have the paint tool offer "this mesh's faces overlap in UV — swap to `box_atlas`?" when it detects
  the overlap. Additive, breaks nothing, and it is one function in `primitives.rs` plus a name in
  `NAMES`. The user's request already adds `quad` there (`00-survey-constraints.md` §3), so the file
  is being touched anyway.

Overlap detection: a mesh whose triangles' UV areas sum to more than ~1.05× the union of their UV
bounding boxes has overlapping charts. Approximate and cheap; a warning, never an error, because
overlapping UVs are a legitimate authoring choice.

## 3. The GPU path: neither a compute pass nor render-to-texture

### 3.1 The rasteriser is on the CPU, and there is only one of it

The brush is evaluated in `loom_asset::paint`, in Rust, and the GPU never rasterises a stroke. The
reason is the reason `loom_field` exists: **a second implementation of the same formula is a
divergence waiting to happen**, and ADR 0006 spent a whole slice making that impossible for scalar
fields. A GPU brush plus a CPU load-time rasteriser is exactly two implementations of one formula,
and unlike `loom_field` there is no expression tree here that could generate both — a brush is a
stamp accumulator with hardness falloff and per-stroke alpha compositing, which S2's `Expr` cannot
express (the same limit CLAUDE.md already records for grass placement: *"a scalar-field language
that cannot express neighbourhood search or struct output"*).

The cost is bounded and the numbers decide it. A stamp of UV radius `r` at resolution `N` dirties a
`(2rN + 2)²` texel rect. At `N = 2048`, a generous `r = 0.03` is a 124-texel radius → a 250² rect →
**62,500 texels**, a few arithmetic ops and one blend each. That is tens of microseconds, once per
frame, against a human hand. The failure case is a deliberately enormous brush: `r = 0.25` at 4096
is a 2048² rect — 4.2 M texels, low single-digit milliseconds — which is why **the brush radius is
clamped so the dirty rect never exceeds 512², and the clamp carries a `ponytail:` comment naming
the upgrade path**.

That upgrade path, written down now so it is not rediscovered: a compute shader writing a
`STORAGE_IMAGE`. It needs a new `Access` variant — **the render graph has no storage-image access
today; `Access` is `ColorWrite`, `DepthWrite`, `DepthRead`, `ShaderRead`, `DepthResolve`,
`DepthSample`, `TransferSrc`, `TransferDst`, `Present` and nothing else**
(`crates/loom_render_graph/src/lib.rs:94-124`), so a `ComputeWrite` at `ImageLayout::GENERAL` with
`COMPUTE_SHADER` / `SHADER_STORAGE_WRITE` would be added there, plus a line in the barrier-list
test. It also needs the brush formula in Slang, which is the divergence problem above. Do not do
this until a measurement demands it.

### 3.2 What reaches the GPU is a dirty-rect copy, inside the graph

The paint image is an ordinary `SAMPLED | TRANSFER_DST` image, created by the same
`renderer::create_image` every material texture uses, with mips. Per stroke-step the CPU writes the
changed rect (and its mip pyramid, each level a quarter the area) into a persistent host-visible
staging buffer, and one graph pass issues a single `vkCmdCopyBufferToImage` with one region per mip
level — the exact shape `material.rs::upload` already uses at load
(`crates/loom_render/src/material.rs:340-360`).

The pass, and its two barriers, both emitted by the graph and neither written by hand:

```
paint_upload   (paint_id, Access::TransferDst)      // SHADER_READ_ONLY -> TRANSFER_DST
forward        (paint_id, Access::ShaderRead) …     // TRANSFER_DST -> SHADER_READ_ONLY
```

`forward` must **declare the paint image**, which is a genuine change to `forward_uses` in
`viewer.rs:1200-1245`: today no material texture is in the graph at all, because none of them ever
changes layout after load. Add the id to `forward_uses` only when the scene has a `PaintLayer`; the
barrier-list test in `loom_render_graph`'s `lib.rs` gets the two new transitions named, which is how
that ownership stays visible rather than assumed (CLAUDE.md's note on the MSAA pair).

**The paint image is imported with a known layout, not as UNDEFINED**, and this is the trap worth
naming. `RenderGraph::import` starts an image at `UNDEFINED` — honest for a target cleared every
frame, and catastrophic for one whose contents are the whole point, because an `UNDEFINED` source
layout permits the driver to discard. `import_with_layout(name, image, SHADER_READ_ONLY_OPTIMAL)`
already exists for exactly this (`loom_render_graph/src/lib.rs:410-425`), and the paint image is
always in that layout at frame start: the load-time upload leaves it there, and every frame ends
with `forward` having put it back.

**When nothing was painted this frame, the pass is simply not added.** No copy, no barrier, and the
image stays in `SHADER_READ_ONLY_OPTIMAL` across the frame boundary, which is what
`import_with_layout` asserts.

### 3.3 Adding a paint layer mid-session: descriptor headroom, not a pipeline rebuild

`Materials::new` sizes its descriptor array to the scene's texture count and that layout is baked
into the pipeline layout at `Viewer::new` (`viewer.rs:314-322`), which every pipeline is created
from. **So growing the texture array means a new pipeline layout and a rebuild of every pipeline** —
unacceptable for "the human added a paint layer to a crate".

The fix is one line and uses a flag the code already sets. `Materials::new` already declares
`PARTIALLY_BOUND` so that a scene with no textures still presents a legal one-element array
(`material.rs:141-145`). Size the array to `textures.len() + PAINT_HEADROOM` instead, leave the
spare slots unwritten — legal under `PARTIALLY_BOUND` as long as nothing samples them, and nothing
does, because every material names `NO_TEXTURE` — and write a spare slot with
`update_descriptor_sets` when a paint layer appears. No layout change, no pipeline rebuild.

`PAINT_HEADROOM = 16` is a guess dressed as a constant; the honest form is
`headroom = max(8, painted_nodes_at_load)`, and exhausting it falls back to the full
`Viewer::set_materials` rebuild below.

Updating a descriptor set a command buffer in flight uses is illegal, so this path idles first —
`device_wait_idle` then `reset_command_buffer`, in that order, because idle alone is not enough
while a recorded command buffer still *references* the resources (VUID-vkDestroyBuffer-buffer-00922
is the same trap, and `set_meshes` documents it at `viewer.rs:841-857`). **`Viewer::set_materials`
is written as a sibling of `set_meshes` and copies its structure verbatim**, including "build the
new before destroying the old, so a failed re-upload leaves the viewer exactly as it was"
(`viewer.rs:870-876`). This is a per-transaction event, not a per-frame one.

## 4. How a stroke becomes undoable — **REQUIRED ADR**

### 4.1 The mechanism

**The live preview is not derived from the scene text, and that is what makes the undo model work.**

While the mouse is down, the tool paints into the CPU-side image and uploads the dirty rect. Nothing
is written to the scene. The viewport is showing state the scene file does not yet contain — which
is a thing this editor otherwise never does, and is the one deliberate exception.

On mouse-up, the accumulated polyline becomes **one** `SceneOp::SetField` on `PaintLayer.strokes`,
in **one** `Transaction`, with a label like `"Paint quay_wall: 1 stroke, 34 points"`. One stroke,
one entry in the transaction log, one Ctrl+Z.

The consequences are worth spelling out because they are all favourable:

**No gesture coalescing is needed.** `Session::apply_coalescing` exists to collapse a per-frame
stream of transactions (`edit.rs:282-297`), and a paint stroke produces no such stream. The survey
predicted a `paint:{node}:{layer}:{epoch}` gesture key would be required
(`00-survey-existing.md` §4); it is not, and the reason the survey gave for wanting it is the reason
it is unnecessary — *"a texture-paint stroke writing an op list per frame is a different volume and
deserves a measurement before the design is fixed"*. It never writes per frame, so the measurement
is moot.

**The whole-file re-emit happens once per stroke, not once per frame.** `SceneView::build_cached` is
re-run on every transaction (`run.rs:622-627`), and at a stroke a second that is a cost the existing
gizmo path already pays sixty times a second.

**The rebuilt view must produce the image the preview already showed.** After the commit,
`SceneView` re-derives and `MaterialLibrary::for_scene` re-rasterises the whole stroke list. If the
result differed from the preview by one texel, the surface would visibly twitch at every mouse-up.
The guard is a test, not a hope:

```rust
// loom_asset::paint
#[test]
fn incremental_painting_equals_a_full_rasterisation() {
    // Paint 40 strokes incrementally into one image; rasterise the same 40
    // from scratch; assert the two byte-identical, including every mip level.
}
```

That test is the correctness gate of Part I. It is cheap, it runs in `cargo test`, and it is the
only thing standing between this design and a preview that drifts.

**A rejected commit discards the stroke, and never merges.** If the agent wrote the file mid-stroke,
the mouse-up transaction fails `stale_version`. The editor reloads, re-rasterises from the reloaded
text, and **the stroke is lost with a console line saying so**. That is never-do #15 applied without
an exception: the alternative — replaying the stroke onto the reloaded text — is an auto-merge, and
an auto-merge of a strokes array is exactly the silent-destruction class the rule exists to forbid.
Losing at most one stroke is the correct price.

**Undo re-rasterises from scratch.** Ctrl+Z restores the previous text, `SceneView` rebuilds, and
the paint layer is rasterised from the shorter list. At 1,000 strokes averaging a 128² rect that is
~16 M texel-writes, on the order of 30 ms — a visible hitch on a held Ctrl+Z and acceptable for a
single press. The mitigation, when it is wanted, is a pure in-memory checkpoint every 64 strokes;
it is derived state, invisible to the format, and it belongs behind a `ponytail:` comment rather
than in v1. A `paint_key` — a hash of the strokes array, exactly like `mesh_key`
(`main.rs:1206-1224`) and `grass_key` — skips the re-raster when the list did not change, which is
every transaction that is not a paint transaction.

### 4.2 Alternatives rejected

**A PNG asset written per stroke, with an exemption from undo.** This is what every other editor
does and it fails three separate rules here. Ctrl+Z has nothing to restore, because
`Applied::undo` is the previous *scene text* and nothing else (`ops.rs:126-128`,
`edit.rs:314-323`). `loom scene --tx` cannot express a stroke, so the agent can neither author nor
review one, which breaks property 2 and M12's identical-diff exit criterion. And it collides
head-on with `LOOM-IMPLEMENTATION-ORDER.md:455-457`: *"Painted regions serialize as polygons or
splines, never bitmaps."*

**Tile deltas in a sidecar binary journal.** Undoable in principle, and still binary — so still not
diffable, still invisible to the agent, and it requires a second undo mechanism running alongside
the scene-text stack, which is never-do #16 read literally. It also has to answer what happens when
the journal and the scene text disagree, and there is no good answer.

**Checkpoint PNG plus a text journal of strokes since the checkpoint.** Strictly worse than the
chosen design: it keeps every property of the stroke list, adds a binary artifact, and creates two
sources of truth for the same image. It is, however, the right *escape hatch* — see `loom paint
bake` in §6.

**A GPU-authoritative texture with readback on commit.** Adds the CPU/GPU divergence of §3.1, adds
a readback stall, and still leaves the question of what the text contains.

**Vector regions — closed polygons and splines with fills, instead of brush stamps.** This is the
letter of the implementation order's instruction, and it is a genuinely different tool: it is
precise, resolution-independent and terrible at grime, weathering and hand-painted variation, which
is what texture painting is for. **The chosen representation satisfies the instruction anyway**: a
stroke *is* a polyline with a radius and a falloff, which is a spline with a stated width, and it is
resolution-independent because the coordinates are UV and the raster is derived. If the human review
of the ADR reads "polygons or splines" as excluding stamped polylines, the design does not survive
and Part I becomes a region-fill tool.

### 4.3 Draft ADR

> **ADR 00XX — A painted texture is a stroke list in the scene text**
>
> *Status: proposed. Next free number at the time of writing is 0022; the exact number depends on
> the sibling editor design docs, which also owe ADRs.*
>
> **Decision.** True UV texture painting authors a `PaintLayer` component carrying an ordered list
> of strokes in UV space, as diffable TOML/JSON on the node, in the same untyped-array form
> `VoxelVolume.ops` uses. The painted image is **derived**: rasterised on the CPU by
> `loom_asset::paint` at scene load and after every transaction that changes the list, and uploaded
> as an ordinary bindless texture indexed from `ObjectData.material.y`. **No painted bitmap is ever
> an authored artifact**, and no painted state exists outside the scene text.
>
> A stroke is committed as a single `SceneOp::SetField` on mouse-up, so it is one transaction and
> one Ctrl+Z. **The in-progress brush preview is explicitly permitted to diverge from the scene
> text**, and is the only editor state allowed to do so; the divergence is closed at mouse-up and is
> pinned by a test asserting an incremental paint is byte-identical to a full rasterisation of the
> same list.
>
> A stroke rejected for a stale version token is **discarded**, not replayed onto the reloaded text
> (never-do #15).
>
> A `PaintLayer` on a mesh with degenerate UVs is a validation error. Voxel meshes are never
> auto-unwrapped, because the mesh is regenerated from `VoxelVolume.ops` and any parameterisation
> would move under every carve. Decals and the material-layer system cover that case.
>
> **Consequences.** The strokes array only ever grows, and rasterisation and undo are both linear in
> its length; `loom paint bake` is the one-way escape hatch and it says so. The forward pass gains
> a render-graph declaration for the paint image, which is the first material texture the graph has
> ever tracked. `Materials`' descriptor array gains headroom so a layer can be added without
> rebuilding every pipeline. Golden scene `paint_wall` is added to `SCENES` and `GOLDEN`.
>
> **Rejected:** a PNG per stroke with an undo exemption; a binary tile-delta journal; a checkpoint
> image plus journal; GPU-authoritative painting with readback. Reasons in §4.2 of
> `docs/design/editor/04-painting-uv-and-decals.md`.

### 4.4 A second, shared ADR this design would benefit from but does not require

**`SetField` on a growing array re-emits the whole array, and that makes the diff unreadable.** A
fiftieth stroke rewrites all fifty in the file, so `git diff` shows the entire list changed. This is
the identical complaint `00-survey-engine-surface.md` raises for terrain sculpting, and it has one
shared answer: an **`AppendToArray { node, field, values }`** op — order-preserving, append-only,
and it makes both a sculpt stroke and a paint stroke a one-line diff.

**If the terrain design doc proposes `AppendVoxelOps`, these should be one op and not two.** Growing
the nine-op vocabulary is ADR territory and should be decided once, for both callers, in whichever
doc gets there first. Part I works without it — the diff is merely ugly — so it is not a blocker
here.

## 5. Saving, and how the .loom file refers to it

**Nothing new is saved.** `Session::save` writes the scene text through the existing atomic
write-tmp-then-rename path (`edit.rs:37-71`), the strokes ride along inside it, and there is no
second artifact to keep in step, no `.meta` hash to invalidate (`loom_asset::meta`), and no
question about what git sees. That is the whole point of choosing text.

The material's `albedo_map` alias is untouched and keeps meaning what it meant. Paint composites
over the material result at shade time (§1.3); it does not replace, wrap, or shadow the base map.

**Asset references stay aliases.** There is no path and no UUID anywhere in a `PaintLayer` —
`docs/format/README.md:158-169` is normative on that and a paint layer names nothing external at
all, which is the easiest way to comply.

`loom paint bake --node <path>` is the one-way flattening operation, and it is deliberately shaped
like `UnpackPrefab`: it rasterises the list, writes `assets/textures/<scene>_<node>_paint.png`,
adds an `[[asset]]` entry, points a new `Material` slot at it, and removes the `PaintLayer`. It is
one transaction and one undo step, it says out loud that the strokes are gone, and it exists for the
case where a session has accumulated thousands of strokes and load time has become the complaint.
**It is not the default and the editor never invokes it implicitly.**

## 6. The agent reaches all of this, because it is text

Property 2 is satisfied without a new command: `loom scene --tx` can already set
`PaintLayer.strokes`, and `loom describe` shows the component. Two additions earn their place:

- `loom validate` gains the stroke-vocabulary funnel and the degenerate-UV check.
- `loom paint bake` (above), because it is the one operation that is not a `SetField`.

A convenience `loom paint stroke --node … --points … --color …` mirroring `loom place --op` is
**deliberately not proposed**. It would be a second spelling of a `SetField` the agent can already
write, and the op-list precedent is that `loom place --op` exists because placement involves
*computation* (snap-to-surface, align, face-toward) that the agent should not redo. Appending a
stroke involves none.

## 7. Memory, at 2048 and 4096

Per painted object, RGBA8, mip chain included (`×4/3`). The CPU keeps **level 0 only** while
painting — the mip pyramid for a dirty rect is generated into a small scratch and copied straight
into staging, so `loom_asset::Texture`'s all-levels representation is built once at load and not
kept resident by the brush.

| `resolution` | texels | GPU level 0 | GPU with mips | CPU level 0 | **total per object** |
| --- | --- | --- | --- | --- | --- |
| 512 | 262,144 | 1.00 MiB | 1.33 MiB | 1.00 MiB | **2.33 MiB** |
| 1024 | 1,048,576 | 4.00 MiB | 5.33 MiB | 4.00 MiB | **9.33 MiB** |
| **2048** | 4,194,304 | 16.0 MiB | 21.3 MiB | 16.0 MiB | **37.3 MiB** |
| **4096** | 16,777,216 | 64.0 MiB | 85.3 MiB | 64.0 MiB | **149.3 MiB** |

Plus one process-wide staging buffer sized to the largest permitted dirty rect — 512² RGBA8 with
mips, **1.33 MiB**, allocated once.

**The numbers set the default at 1024, not 2048.** Ten painted objects at 4096 is 1.5 GiB and would
put the RTX 4090 into eviction on a scene that is otherwise trivial; ten at 1024 is 93 MiB and
nobody notices. The authoring rule that makes this a decision rather than a preference is
**texels per metre**: `resolution / (largest object dimension in metres × uv span)`. A 2 m crate at
1024 is 512 texels/m, which is more than a 4K monitor can resolve at arm's length. A 20 m wall at
1024 is 51 texels/m, which is visibly soft, and is the case that justifies 2048. **4096 is for a
single hero surface and the inspector should say so in the field's doc comment**, which is also the
tooltip (`panels.rs:798-803`).

Two scope decisions the table depends on, both stated rather than assumed:

**Albedo and coverage only in v1 — RGBA8, one image.** A painted *normal* or *roughness* layer
doubles or triples every row above and is a separate feature. `PaintLayer` is forward-compatible
with it (add a `channels` field defaulting to `"albedo"`), and adding it later is additive under
`docs/format/README.md` §9.

**No paint on instanced or generated geometry.** Grass, water, rain, fire, smoke and scattered
instances are all generated from `SV_VertexID` or shared meshes; a `PaintLayer` on such a node is a
validation error for the same reason a `PaintLayer` on a voxel mesh is.

## 8. Files touched, Part I

| File | Change |
| --- | --- |
| `crates/loom_scene/src/components.rs` | `PaintLayer`; register it in the by-hand list at `:1695-1723` |
| `crates/loom_asset/src/paint.rs` | **new**: `Stroke`, `parse_strokes`, `rasterise`, `rasterise_into`, `dirty_rect`; the incremental-equals-full test |
| `crates/loom_asset/src/mesh.rs` | `Mesh::uv_extent`, `Mesh::uvs_overlap` |
| `crates/loom_asset/src/primitives.rs` | `box_atlas` (and `quad`, already owed) |
| `crates/loom_cli/src/materials.rs` | rasterise a `PaintLayer` into `textures`; record its slot per entity |
| `crates/loom_cli/src/scene_view.rs` | `paint_key`, beside `mesh_key`; paint slot per object |
| `crates/loom_cli/src/main.rs` | `loom validate` stroke funnel + UV check; `loom paint bake` |
| `crates/loom_render/src/renderer.rs` | `Object::paint`; write it to `ObjectData.material[1]` |
| `crates/loom_render/src/material.rs` | `PAINT_HEADROOM`; `Materials::write_texture_slot`; expose an update path |
| `crates/loom_render/src/viewer.rs` | `set_materials`; the `paint_upload` pass; `(paint_id, ShaderRead)` in `forward_uses` |
| `crates/loom_render_graph/src/lib.rs` | barrier-list test names the two new transitions |
| `assets/shaders/scene.slang` | `paint` varying; three lines in `fragmentMain` |
| `xtask/src/main.rs` | `paint_wall` in `SCENES` and `GOLDEN` |
| `assets/test/paint_wall.loom` | **new** golden scene; authors `uv_scale = [8, 4]` on purpose |

Shader entry points: `vertexMain`, `fragmentMain`. No new entry point, no new pipeline.

---

# Part II — Decals

## 9. The decal as a scene entity

```toml
[[node]]
name = "scorch_01"
parent = "yard"
transform = { pos = [3.0, 0.02, -1.4], rot_euler = [0, 34, 0], scale = [1.2, 0.5, 1.2] }
[node.Decal]
albedo_map = { asset = "scorch" }
opacity = 0.85
angle_fade = 0.35
depth_fade = 0.4
```

```rust
/// A texture projected onto whatever geometry is inside the node's box.
pub struct Decal {
    /// Colour texture. Its alpha is the decal's coverage.
    pub albedo_map: AssetRef,
    /// Tangent-space normal map. Optional; costs a second sample.
    pub normal_map: AssetRef,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub opacity: f32,
    /// Surfaces facing more than this far from the projection axis are skipped.
    /// The cosine, so `0.0` accepts everything up to 90° and `0.7` is a 45° cone.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub angle_fade: f32,
    /// Fraction of the box's depth over which the decal fades out at each end.
    #[schemars(range(min = 0.0, max = 0.5))]
    pub depth_fade: f32,
}
```

**The node's transform is the projector.** A unit box centred on the node, scaled and rotated by the
node's own transform, projecting **down local −Y**. Nothing new is authored to describe the volume,
so the existing Move/Rotate/Scale gizmos manipulate a decal on day one with no new manipulator — the
same trick `BoxCollider` does not get and should (`00-survey-engine-surface.md` notes dragging
half-extents as numbers is the classic bad UI).

**−Y rather than −Z, and the choice is not free.** Unity projects along −Z; this engine is Y-up with
−Z forward, and the dominant authored case is stamping onto ground and terrain, where −Y means "drop
the box on the floor and it works". A wall decal then needs a 90° pitch, which the placement tool
applies automatically from the surface normal it hit — `loom_physics::RayHit::normal`
(`loom_physics/src/lib.rs:36`) is already in the codebase with a comment anticipating exactly this.
The alternative reads better for wall decals and worse for the common one.

**Zero new `SceneOp`s.** Creating a decal is `SpawnNode` + one `SetField` per authored field, which
is what Add Component already does (`run.rs:1598-1628`). Placing it is `SetTransform`, which is what
the gizmo already issues. Nudging opacity is a scrubbed slider through `apply_coalescing`, unchanged.

## 10. How it renders, and what it cannot do

### 10.1 A forward projector inside `fragmentMain`

There is no G-buffer, and the honest consequence is that a screen-space decal pass is not available.
The forward-compatible answer is that **the receiving surface evaluates the decals, rather than the
decals being drawn onto the receiving surface**:

```slang
// scene.slang, after the ground layer, before the wet block.
for (uint i = 0; i < decalCount(); ++i) {
    LoomDecal d = push.environment[0].decals[i];
    float3 local = mul(d.invModel, float4(in.worldPos, 1.0)).xyz;
    if (any(abs(local) > 0.5)) continue;
    float facing = dot(geometric, -d.axis);            // d.axis = world −Y of the projector
    if (facing < d.angleFade) continue;
    float2 duv = local.xz + 0.5;
    float4 t = sampleMap(d.albedoIndex, duv);
    float a = t.a * d.opacity
            * smoothstep(d.angleFade, 1.0, facing)
            * smoothstep(0.5, 0.5 - d.depthFade, abs(local.y));
    albedo = lerp(albedo, t.rgb, a);
    // normal_map, when present, perturbs through the same cotangentFrame the
    // material path uses, blended by `a`.
}
```

The decal list lives in the **environment buffer** and not the push block, because
`crates/loom_render/src/renderer.rs:626-628` records that the push block is at **124 of the 128
bytes Vulkan guarantees** and there is room for nothing else. That is exactly why wind, the camera
position, the terrain height pointer and the wave set all live in `EnvironmentData` already
(`renderer.rs:146-160, 216-219, 236-241`), and a `decals` device address plus a `decal_count` is the
fifth instance of the same pattern.

The loop bound follows the established precedent: `LOOM_MAX_LIGHTS = 8` and `pointLights()` loops
every light for every fragment (`scene.slang:105, 306`). Decals are capped at **16**, checked by
`loom validate` with a message rather than silently truncated.

### 10.2 What this buys, stated because it is the reason to prefer it

**The decal is anti-aliased.** It is shaded inside the forward pass, which rasterises into the 4×
multisampled pair and resolves afterwards (`viewer.rs:1250-1270`). Compare rain, which draws into
the resolved single-sample target and is *"the one thing in the frame with no anti-aliasing at
all"* (ADR 0017 and CLAUDE.md's P4 block) — a screen-space decal pass placed there would inherit
that defect exactly.

**The decal is in HDR and is lit.** It modifies `albedo` before the sun, ambient, point lights, wet
film and fog are applied, so a decal in shadow is dark and a decal in sun is bright. A pass after
the tonemap would paste display-referred pixels onto a lit image, and a scorch mark inside a dark
shed would glow.

**The decal conforms to anything the forward pass draws, including voxel terrain**, with no UVs and
no unwrap — the projection is in the decal's own box space and the receiver's UVs are never
consulted. This is the §2 answer to "how do you mark up a voxel mesh".

**Nothing enters the acceleration structure**, so a decal casts no shadow of its own quad and
adds no TLAS rebuild cost. A stuck-on geometry decal would do both.

### 10.3 The limits, all four of them

**A decal is invisible in ray-traced reflections.** `tracedEnvironment` shades a reflected hit from
the material's `mean_albedo` and never runs `fragmentMain` (`material.rs:66-78` and ADR 0021), so a
scorch mark on a quay does not appear in the puddle beside it. This is the same rule ADR 0019 states
for grass, water, rain and fire — *"anything that wants to be reflected has to become an
`Object`"* — and a decal deliberately is not one. Accepted, and the mitigation if it ever matters is
to fold decal colour into the receiver's `mean_albedo` at load, which is an averaging approximation
and should be argued on evidence rather than built now.

**A decal only appears on geometry drawn by `fragmentMain`.** Grass, water, rain, fire and smoke
have their own entry points and their own fragment shaders, and none of them will show a decal.
This is correct for all five.

**A decal projects through thin geometry.** The box does not know where the surface it hits ends, so
a decal on one side of a 10 cm wall appears on the other side too if the box is deeper than the
wall. `angle_fade` removes the back face in the common case — the back face points the other way —
but a curved or folded surface inside the box still receives twice. The cure is a thin box, and the
tool should author `scale.y` from the hit surface rather than defaulting to 1.0.

**Cost is O(decals) for every mesh fragment on screen, unculled.** Sixteen decals is sixteen matrix
transforms and a box test per pixel of every mesh, whether or not any decal is near. That is
defensible at the authored counts this feature targets and indefensible at gameplay counts. **The
upgrade path is already provisioned**: `ObjectData.material` has `.z` and `.w` free (§1.3), which is
enough for a per-object `[first, count]` range into a CPU-culled decal list — a decal's box against
each object's world AABB is the same test `pick_at_cursor` already does (`run.rs:2156-2178`). Write
it as a `ponytail:` comment naming that path; do not build it until a GPU timestamp asks.

### 10.4 Rejected alternatives

**Deferred screen-space decals.** The textbook approach and the one this renderer cannot have. It
needs a G-buffer of albedo, normal and roughness; adding one to a 4× MSAA forward renderer means
either resolving material attributes (which is meaningless — you cannot average two materials) or
carrying 4× G-buffer memory and shading per sample, which is a fundamental change to a renderer
whose whole frame is 0.05–0.9 ms. It also lands after the resolve, inheriting rain's no-AA defect,
and after the tonemap, inheriting the unlit-decal defect. Rejected on three independent grounds.

**Decals as geometry — a quad or a projected-and-clipped mesh stuck to the surface.** Z-fighting
against the receiver at any distance, does not conform to a curved or voxel surface without CPU
clipping against the receiver's triangles, and — the decisive one — it becomes an `Object`, enters
the TLAS, and casts the ray-traced shadow of its own rectangle. That is the same defect
`Material::alpha_cutoff`'s doc comment already records for foliage: *"a ray query never runs a
fragment shader — so an alpha-cut surface casts the shadow of its whole triangle"*
(`components.rs:212-217`).

**Decals baked into a `PaintLayer` at author time.** Tempting, since Part I already exists: stamp
the decal into the receiver's paint layer and there is no runtime cost at all. It fails on voxel
meshes (no UVs — §2), on any receiver spanning several nodes, and on any decal the human wants to
move afterwards. Worth offering as a "flatten into paint" *action*, not as the mechanism.

### 10.5 Draft ADR

> **ADR 00YY — Decals are box projectors evaluated in the forward fragment shader**
>
> *Status: proposed.*
>
> **Decision.** A `Decal` component makes its node a unit-box projector, oriented and sized by the
> node's own transform, projecting along its local −Y. Decals are evaluated **inside
> `fragmentMain`**, by the receiving surface, from a list in the environment buffer — no pass, no
> pipeline, no barrier, no G-buffer, and no change to ADR 0018's pass order. Decals modify `albedo`
> and optionally the normal before lighting, so they are lit, fogged, HDR, and anti-aliased by the
> existing 4× MSAA.
>
> The list is capped at 16 and is unculled; every mesh fragment tests every decal.
>
> **Decals are not in the acceleration structure and therefore do not appear in ray-traced
> reflections, and do not cast shadows.** This follows ADR 0019's rule rather than making an
> exception to it.
>
> **Consequences.** No new `SceneOp` — a decal is `SpawnNode` + `SetField`, placed by the existing
> transform gizmos. `EnvironmentData` gains a device address and a count, its fifth use of that
> pattern. Decals do not appear on grass, water, rain, fire or smoke. A decal box deeper than the
> surface it marks projects through to the far side; `angle_fade` covers the flat case and a thin
> box covers the rest. Golden scene `decals` is added to `SCENES` and `GOLDEN`, and it must cover a
> decal on a mesh, a decal on voxel terrain, a decal at a grazing angle, and two overlapping decals
> — the last because blend order is array order and a silent reorder is otherwise invisible.
>
> **Rejected:** a deferred screen-space pass (needs a G-buffer; lands after the resolve with no AA
> and after the tonemap unlit); geometry decals (z-fighting, no conformity, and they would cast the
> ray-traced shadow of their own quad).

## 11. Files touched, Part II

| File | Change |
| --- | --- |
| `crates/loom_scene/src/components.rs` | `Decal`; register it |
| `crates/loom_cli/src/materials.rs` | load decal textures into the bindless array (already the only place that does this) |
| `crates/loom_cli/src/scene_view.rs` | collect `Decal` nodes into a `Vec<DecalData>` with world→local inverses |
| `crates/loom_cli/src/main.rs` | `loom validate`: the 16-decal cap, alias resolution |
| `crates/loom_render/src/renderer.rs` | `DecalData` (`#[repr(C)]`, 96 B: `inv_model`, `axis`, `color_params`); `EnvironmentData::decals` + `decal_count` |
| `crates/loom_render/src/viewer.rs` | `set_decals`, uploading into a small device-address buffer beside the environment buffer |
| `assets/shaders/scene.slang` | `LoomDecal`, `decalCount()`, the loop in `fragmentMain` |
| `xtask/src/main.rs` | `decals` in `SCENES` and `GOLDEN` |
| `assets/test/decals.loom` | **new** golden scene, four cases per §10.5 |

Shader entry points: `fragmentMain` only. `vertexMain` is unchanged; a decal needs no varying,
because `in.worldPos` and `in.normal` are already there for the shadow rays.

**Both features owe `cargo xtask shimmer` a control reading of exactly 0.000** on their golden
scenes with the camera static. Paint and decals are textures on MSAA'd mesh surfaces with no
animated geometry, so anything above zero means something is sampling with a per-frame-varying
coordinate, which would be a real bug. `primitives`, `materials`, `cave` and `ground` already score
exactly 0.000 (CLAUDE.md, ADR 0019 block), so the instrument is known good.

---

## 12. What I could not verify

Written plainly, because an unmarked guess is worse than an admitted gap. **Nothing in this document
was compiled or run** — this phase is design-only and `cargo` was not invoked.

**The descriptor-headroom trick is reasoned, not tested.** `PARTIALLY_BOUND` is set
(`material.rs:141-145`) and the spec permits an unwritten partially-bound descriptor that is never
accessed, but I did not run the validation layers against an over-sized array with unwritten slots
on this driver. If it complains, the fallback is `VARIABLE_DESCRIPTOR_COUNT` or a full
`set_materials` rebuild, and both are strictly more work.

**I did not verify that changing the descriptor *count* forces a new pipeline layout in this
codebase's exact construction.** I verified that `materials.descriptor_layout()` is passed into the
pipeline-layout builder at `viewer.rs:314-322` and that every pipeline derives from it; I inferred
the rest from the Vulkan rule that a set layout's descriptor count is part of its identity. If the
inference is wrong, §3.3's headroom is unnecessary and the design gets simpler.

**The CPU raster timings are arithmetic, not measurements.** "Tens of microseconds for a 250² rect"
is texel counts times an assumed few-nanoseconds-per-texel, not a benchmark. The brush-radius clamp
is sized from the same arithmetic. The first thing to measure when this is built is a stamp at the
clamp limit; if it is over a millisecond, the clamp is wrong, not the design.

**The memory table is exact arithmetic on RGBA8 and the standard `×4/3` mip sum**, and it assumes
`gpu-allocator` adds no significant per-image overhead and that the driver does not pad a
power-of-two RGBA8 image. Both are safe assumptions and neither was checked with `nvidia-smi`.

**Whether the paint layer belongs before or after the ground layer in `fragmentMain` is a taste
call I could not settle without looking at a render.** §1.3 argues for before; if painted marks on
terrain read as being eaten by the rock layer, it moves.

**The overlapping-UV heuristic in §2.4 (sum of triangle UV areas versus the union of their bounding
boxes) is a plausible test I have not validated against the actual primitives.** The *fact* that
`box` gives all six faces the unit square is verified from source and the comment
(`primitives.rs:66-72`); the detector for it is not.

**Whether a stroke committed on mouse-up is fast enough at scene scale is unmeasured.**
`SceneView::build_cached` plus `MaterialLibrary::for_scene` runs on every transaction, and
`for_scene` will now re-rasterise the paint layer. `paint_key` is designed to skip that for
non-paint transactions, but the paint transaction itself pays a full re-raster of its own layer —
which is the same ~30 ms the undo path pays. A stroke a second at 30 ms is fine; a fast scribble may
not be. **If it is not, the fix is to hand the already-correct preview image into the rebuilt
`MaterialLibrary` instead of re-rasterising it**, keyed by `paint_key`, and the incremental-equals-
full test is what makes that substitution safe.

**Decal cost is entirely unmeasured.** Sixteen box tests per mesh fragment at 1920×1080 is roughly
33 M matrix-vector transforms in the worst case, which I would guess lands somewhere between 0.05
and 0.3 ms — comparable to the whole forward pass of most scenes in this project, and therefore not
negligible. `LOOM_GPU_TIMING=1` answers it in one run, and the answer decides whether the per-object
cull in §10.3 is an upgrade path or a requirement.

**Whether human review reads "polygons or splines, never bitmaps"
(`LOOM-IMPLEMENTATION-ORDER.md:455-457`) as admitting stamped polylines is a judgement I cannot
make.** §4.2 argues it does. If it does not, the ADR in §4.3 is rejected and Part I becomes a
region-fill tool, which is a different feature with a different UI.
