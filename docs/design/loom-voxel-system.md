# Loom: Destructible Smooth Voxel System

**Third companion doc.** Constraints: destructible at runtime, smooth surfaces, contained levels
now with a streamed open world later, agent-authored.

That's the hardest of the available combinations. Blocky voxels have a simple representation and
cheap colliders; static voxels bake everything offline. You've asked for neither. The good news is
that two things landed recently that make it substantially easier than it would have been two years
ago — native voxel colliders in Rapier, and a mature Rust crate ecosystem for the meshing.

---

## 1. Representation: SDF chunks, not occupancy

A 0/1 occupancy grid polygonizes fine into blocky surfaces but cannot represent curves. Blurring or
averaging it is expensive and still wrong. Smooth surfaces require a **signed distance field**: for
any point, the distance to the nearest surface, negative when inside something, so the surface is
the set of points where the field is 0 (the "isolevel"). Godot's Voxel Tools documentation makes
this point explicitly, and it's the correct starting premise.

Practical storage decisions:

```rust
/// One chunk. 32³ is the sweet spot for destructible: small enough to remesh
/// cheaply on edit, large enough that per-chunk overhead stays amortized.
pub struct Chunk {
    /// Quantized SDF. i8 covers ±1 voxel of distance at 1/127 precision, which is
    /// plenty for surface extraction and 4× cheaper than f32.
    sdf: Box<[i8; 32 * 32 * 32]>,
    /// Material index per voxel — separate array, because it compresses differently
    /// and most chunks use 1-2 materials.
    material: MaterialStorage,
    state: ChunkState,   // Uniform(solid) | Uniform(air) | Detailed
}
```

**Store `i8`, not `f32`.** Surface extraction only needs distance near the surface; far-field
values are irrelevant because those cells never generate geometry. This is a 4× memory win for free.

**Collapse uniform chunks.** Most chunks in any real volume are entirely solid or entirely air.
Storing a discriminant instead of 32,768 identical bytes is the single largest memory saving
available, and it also lets the mesher skip them in O(1).

### 1.1 The two-layer trick — steal this exactly

`bevy_voxel_world` uses a pattern that is precisely right for destructible terrain: the world has
two layers of voxel information — one procedural, determined by a terrain lookup function, and one
controlled by explicit set/get calls and persisted in a hash map, with the **persistent layer always
overriding the procedural one**. The consequence is that the world can be infinitely large while you
only store voxels that were deliberately changed.

For a destructible game this is the whole storage strategy. An untouched world costs nothing. A
world where the player has blown up forty craters costs forty craters. Save files are tiny.

### 1.2 Per-object voxel volumes — the answer to "structures"

Don't put buildings in the global terrain field. Give each destructible structure **its own chunked
SDF volume with its own transform**, stored as a component:

```rust
#[derive(Component, Reflect)]
pub struct VoxelVolume {
    grid: ChunkMap,          // local coordinates
    voxel_size: f32,
    origin: NodePath,        // transform comes from the node (§2.2 of main doc)
}
```

Four things fall out of this, all of them good:

1. **Structures are authorable and reusable as prefabs.** A building is a volume asset the agent can
   place many times — which fits the prefab/override model already designed in §2.4 of the main doc.
2. **Destruction is local.** Blowing a hole in one wall dirties chunks in one small volume, not the
   world grid.
3. **Colliders stay small.** Per-volume physics instead of terrain-sized trimeshes.
4. **Volumes can move.** A destructible vehicle or a collapsing tower becomes possible later without
   re-architecting.

Terrain is then just the one special volume that's very large and axis-aligned.

---

## 2. Surface extraction: the real decision

Three viable algorithms, and the tradeoff is genuinely sharp. This is the choice that determines
what your game can look like, so it's worth understanding properly rather than picking the one with
the best blog posts.

### 2.1 Marching Cubes — rule it out for structures

MC is the classic and the default. Its fatal limitation for you: **it cannot represent sharp edges
and corners.** A square approximated with marching squares has its corners sliced off, and adaptivity
does not help, because marching squares always produces straight lines on a cell's interior — which
is exactly where the corner lies. Same in 3D.

If your world has buildings, MC alone will make every one of them look melted.

### 2.2 Surface Nets — simple, smooth, no corners

Naive Surface Nets is the earliest dual method and by far the easiest to implement and to chunk. One
developer's well-documented journey through exactly your problem: he implemented MC first, found it
too blocky, moved to Naive Surface Nets and was pleased to have smooth terrain — then discovered
Surface Nets can't support sharp features like a 90-degree angle, which meant it would never be
possible to have both smooth terrain *and* a realistic-looking building.

That's the summary. Great terrain, no architecture.

**Rust support is the best of the three:** `fast-surface-nets-rs` (bonsairobo) is a fast,
chunk-friendly implementation on regular grids. And `bevy-sculpter` builds a whole destructible
system on top of it — SDF density fields with chunked storage, Surface Nets meshing with seamless
chunk boundaries, sculpting brushes (smooth, hard CSG, blur, flatten), sphere-tracing and DDA
raycasting into the field, and SDF redistancing via the Fast Sweeping Method. That last one matters
and is easy to overlook (§5.3).

### 2.3 Dual Contouring — sharp features, and free LOD seams

DC places **one vertex inside each cell**, positioned at the point most consistent with the surface
normals sampled around that cell, minimizing a least-squares penalty over those normals. Because the
vertex can sit anywhere in the cell rather than on cell edges, DC reproduces sharp edges and picks
out corners where they occur — and the resulting surfaces have a more natural flow than MC.

The property that makes DC especially attractive here: **it handles chunk LOD seams natively.**
Where MC needs Transvoxel to stitch differing resolutions, DC's algorithm just works with
differently-sized leaf nodes in the octree — you gather the seam nodes from neighboring chunks,
build a new octree from them, and generate a mesh that completely covers the crack without
generating overlapping polygons.

The costs, which are real:

- **It needs Hermite data** — gradients at the edge intersections, not just density values. That
  complicates your edit path: every CSG operation must write normals as well as distances.
- **Non-manifold edges and self-intersections.** This is DC's known failure mode. Manifold Dual
  Contouring improves manifold preservation; Dual Marching Cubes uses a dual grid aligned to the
  implicit function's features and can preserve sharp features without DC's excessive subdivision;
  work on simplicial partitions eliminates self-intersecting triangles. But the honest state of the
  literature is that **no method solves both the non-manifold problem and the self-intersection
  problem simultaneously.** You will ship with some degenerate triangles. Plan for a cleanup pass
  and don't feed raw DC output to a physics trimesh (§4 makes this moot).
- No mature Rust crate. You're implementing it, using Nick Gildea's blog series and Boris the
  Brave's tutorial as references.

### 2.4 Transvoxel — the LOD answer if you stay MC-based

Eric Lengyel's algorithm (2009, C4 Engine) seamlessly stitches meshes generated at differing
resolutions so LOD can be applied to large volumetric datasets. It works by filling **transition
cells** at LOD boundaries: 512 possible cases reduced to **73 equivalence classes**, each a triangle
pattern that perfectly fills the seams and cracks between meshes of different resolution.
Implementation notes worth having up front: transition meshes operate in a 2D "face space" along the
block face with the local Z axis pointing from low-res to high-res; Lengyel recommends the
transition region occupy **0.5** of the original voxel so shading stays normal and no harsh elevation
changes appear; his high-performance approach uses **16³ blocks with aggressive vertex reuse**; and
each LOD level halves the volume resolution in each dimension, which is required for the algorithm
to work.

There's a Rust crate (`transvoxel`) implementing it, with a Bevy `MeshBuilder` in its examples. One
caveat from its docs: the examples don't cache, so the density function gets called many times for
the same voxel position — do your own caching.

Transvoxel inherits MC's inability to do sharp corners. It solves seams, not features.

### 2.5 Recommendation

**Phase A (now): Naive Surface Nets via `fast-surface-nets-rs`.** Chunked, fast, proven Rust, and
`bevy-sculpter` gives you a working reference for the whole destructible pipeline including
redistancing. Terrain looks good. Buildings look soft.

**Phase B: Dual Contouring, hand-implemented.** You need it for structures with sharp corners, and
it pays you back with free LOD seam handling when the open world arrives — which is precisely when
you'd otherwise have to implement Transvoxel. Two problems solved by one algorithm.

Do **not** take the third path of Transvoxel-plus-something-else for sharp features. You'd be
implementing two hard algorithms to get what DC gives you in one. The only reason to choose
Transvoxel is if you decide structures will be conventional meshes rather than voxels — see §6.

Keep the mesher behind a trait from day one:

```rust
pub trait Mesher: Send + Sync {
    fn mesh(&self, chunk: &ChunkView, lod: u8) -> MeshData;
    fn needs_hermite(&self) -> bool;   // DC yes, Surface Nets no
}
```

The `needs_hermite` flag is what lets you switch later without rewriting the edit path — write
gradients from the start even if Surface Nets ignores them.

---

## 3. The edit → remesh pipeline

This is where destructible voxel systems live or die.

```
agent/player edit
  → apply CSG op to SDF chunks         (fast, small, deterministic)
  → mark touched chunks + neighbors dirty
  → redistance affected region          (§5.3)
  → push dirty chunks to a priority queue (by distance to camera)
  → worker pool meshes them             (rayon, off the main thread)
  → worker pool builds colliders        (off-thread — see §4)
  → main thread swaps mesh + collider, budgeted per frame
```

**Dirty the neighbors, not just the edited chunk.** Surface extraction reads one voxel past the
chunk boundary. Miss this and you get cracks at chunk seams on every edit — the single most common
bug in these systems.

### 3.1 Budgeting, and an advantage you should deliberately exploit

Godot's Voxel Tools documentation is unusually candid about where this hurts, and their pain points
are instructive because **most of them don't apply to you.**

Their measurements and constraints:

- **Building a collider from a mesh costs roughly 3–5× the meshing itself**, because it involves
  constructing an acceleration structure (BVH, octree) to speed up collision queries. This is the
  dominant cost in the whole destructible loop, not the meshing.
- Godot offers no reliable way to safely build collision shapes from within their meshing threads,
  so they must defer collider creation to the main thread and spread it across frames — which
  slows terrain loading tremendously compared to disabling collisions.
- Their frame-budget heuristic gets wrecked by an unrelated quirk: the first OpenGL call in a frame
  can cost about 15ms on the CPU regardless of how trivial the call is, so their uploader concludes
  it has done too much work and stops — often after a single mesh per frame, which they describe as
  ridiculously low.
- Godot Jolt has the same collider issue, made worse by deferring shape setup to the last possible
  moment.

**In Rust with `rapier` and `wgpu`, both meshing and collider construction can happen entirely on
worker threads.** `SharedShape` is `Send + Sync`; you build it in the pool and hand the finished
shape to the main thread for a pointer swap. That removes the exact bottleneck that is the primary
performance complaint of the most mature open-source implementation of this feature. It's a real
structural advantage of your stack and it's worth designing around explicitly rather than
discovering by accident.

Budget anyway — cap swaps per frame, prioritize by camera distance, and keep the old mesh visible
until the new one is ready so edits never flash holes.

---

## 4. Physics: use Rapier's voxel colliders

**The most important finding in this research pass.** Rapier now ships native voxel shapes, built
from a 3D grid of occupied cells, with constructors including `ColliderBuilder::voxels_from_points`.
Rapier's own documentation states that unlike triangle meshes, voxel-based shapes can offer
**improved collision detection robustness and performance owing to their regular structure**.

Why this changes the design:

1. **It sidesteps the 3–5× collider-construction cost above.** No BVH build from arbitrary
   triangles — the grid *is* the acceleration structure.
2. **It's more robust.** Rapier separately advises against trimesh colliders on dynamic rigid
   bodies, and trimeshes have well-known ghost-collision problems at internal edges (mitigated but
   not eliminated by `TrimeshFlags::FIX_INTERNAL_EDGES`).
3. **No tunneling pathology from thin shells.** A surface-extracted mesh is an infinitely thin
   membrane with nothing behind it; a voxel grid is solid volume.

**So: decouple visual and physical representation.** Mesh the SDF with Surface Nets or DC for
rendering; build the collider from the *occupancy* derived from the same field (`sdf < 0`). They
don't need to match exactly — the visual surface is smooth, the collision surface is voxel-stepped
at your voxel size, and at a reasonable voxel size nobody notices. This is a much better trade than
it sounds, and it makes the destructible loop cheap.

Debris from destruction is separate: spawn dynamic rigid bodies with **convex** colliders (boxes or
convex hulls), never trimeshes, and pool them aggressively with a hard cap. Uncapped debris is the
classic way a destructible game dies.

Character controller note: Voxel Tools also offers a Minecraft-style AABB mover as an alternative to
mesh physics precisely because it's extremely fast and immune to tunneling. If your player
controller misbehaves on voxel terrain, a purpose-built swept-AABB mover against the voxel grid is a
legitimate answer rather than a hack.

---

## 5. Agent authoring **[the part that's genuinely novel]**

Voxels are a *better* fit for AI authoring than meshes, for a reason unrelated to destruction:
filling and modifying a field is a task models are good at, while placing meshes at computed
world coordinates is a task they're bad at (§2.8 of the main doc exists entirely to work around
that weakness).

### 5.1 The agent gets CSG operations, not voxel arrays

`bevy-sculpter`'s brush API is almost exactly the right tool surface: hard CSG sphere add/remove,
smooth continuous brushes with a rate and falloff, blur for surface smoothing, and flatten toward a
target height. Wrap that shape:

```rust
pub enum VoxelOp {
    Sphere   { center: Vec3, radius: f32, mode: CsgMode },
    Box      { center: Vec3, half_extents: Vec3, rot: Quat, mode: CsgMode },
    Capsule  { a: Vec3, b: Vec3, radius: f32, mode: CsgMode },
    Extrude  { profile: Vec<Vec2>, height: f32, at: Transform, mode: CsgMode },
    Noise    { region: Aabb, layer: NoiseLayer },   // terrain generation
    Smooth   { center: Vec3, radius: f32, strength: f32 },
    Flatten  { center: Vec3, radius: f32, height: f32, strength: f32 },
}
pub enum CsgMode { Union, Subtract, Intersect }
```

"Carve a cave system through this hillside" is then a handful of capsule subtractions — a request
the agent can express correctly on the first try.

### 5.2 Serialize the op list, never the voxels

**This is the most important design decision in this document.** A 512³ volume is 134 million
voxels. Never write that into a `.loom` scene file.

Instead the scene stores the **recipe**:

```toml
[[node]]
name = "Hillside"
parent = "Level"

  [node.components.VoxelVolume]
  voxel_size = 0.25
  bounds = [128, 64, 128]
  seed = 8891

  [[node.components.VoxelVolume.ops]]
  kind = "noise"; layer = "ridged_perlin"; octaves = 4; scale = 0.02

  [[node.components.VoxelVolume.ops]]
  kind = "capsule"; a = [12, 8, 30]; b = [40, 6, 44]; radius = 3.5; mode = "subtract"
```

Every property from §2.3 of the main doc is preserved: it's text, it diffs meaningfully, a human can
review what the agent did, git handles it, and it's compact. The volume is baked from the op list at
load. Runtime player destruction accumulates into a *separate* saved-game delta layer (§1.1), never
into the authored scene.

It also gives you **determinism for free** (§A.3 of the graphics/physics doc): replaying the same op
list with the same seed produces bit-identical voxels, so `run_scene` assertions against voxel
worlds are stable.

### 5.3 Gotchas the agent will hit, so put them in the validator

- **Redistancing.** After CSG edits the field is no longer a valid signed distance field — values
  near the edit are wrong distances even though the sign is right, which degrades normals and
  surface quality. `bevy-sculpter` handles this with the **Fast Sweeping Method** to restore proper
  SDFs. Run it over the affected region after every batch of edits, not after every single op.
- **Op ordering matters.** Subtract-then-union is not union-then-subtract. Make the list explicitly
  ordered and say so in the schema docs, or the agent will assume commutativity.
- **Feature size vs voxel size.** An op with a radius smaller than about 2 voxels produces nothing
  or produces noise. Validate it and return the voxel size in the error, so the agent can either
  scale the op or request a finer volume.
- **Volume budget.** Reject volumes whose voxel count exceeds a configured cap. An agent asked for
  "a big terrain" will cheerfully request 1024³ at 0.1 units.
- **`voxel_measure` tool.** Give the agent DDA and sphere-tracing raycasts into the field, mirroring
  `scene_measure`. It needs to be able to ask "where is the ground here?" before placing anything.

---

## 6. Scaling to the open world

Deliberately deferred, but three decisions now keep it cheap later.

**Octree of chunks with distance-based LOD.** Each level halves resolution per dimension. If you
went DC, seams are handled by the algorithm. If you went Surface Nets, this is when you pay for
Transvoxel.

**The heightmap shortcut is legitimate.** Relic avoided applying LOD to volumetric terrain
altogether by using a heightmap representation for distant terrain. Volumetric detail only matters
where the player can see caves and overhangs — which is nearby. Consider a hybrid before building a
full LOD octree.

**Brickmaps for streaming, if you get there.** The pattern from a well-documented CUDA
implementation: 8³ voxel bricks indexed by integer indices into one linear allocation rather than
per-brick pointers, grouped into superchunks of 16³ bricks so an index fits in 12 bits, with the
storage doubling when it fills. Brickmaps carry LOD naturally and stream in and out, enabling huge
worlds — but the same source notes that their hierarchical nature makes them **relatively slow to
edit**, which is the tension with destructibility. Hence: chunked hash map for the playable region,
brickmap only for distant streamed data.

**Far-future frontier.** The Aokana paper (2025) describes a GPU-driven voxel rendering pipeline for
open-world games, using multiple shallow SVDAGs, then chunk selection, tile selection, DAG ray
marching, and Hi-Z build passes — all in compute shaders, writing depth, normals, chunk IDs and voxel
coordinates into a 64-bit visibility buffer via `InterlockedMax`. Note how neatly that composes with
the visibility-buffer and Hi-Z work already recommended in §B.2–B.3 of the graphics doc: the same
infrastructure serves both. Worth knowing the destination even if you never go there.

---

## 7. GPU meshing: yes, but not for chunks the player touches

You can run the whole thing on the GPU. `silk-clouds` is a Rust/wgpu proof: marching cubes on a
density function without leaving the GPU, around 2.8 million vertices from a 100³ volume at 60fps on
an M2 MacBook Air. Its key trick addresses the fundamental problem — marching-cubes output is
dynamically sized, which normally forces a CPU-side copy to set up the render pass, so instead it
allocates a vertex buffer of tunable amortized size and generates an indirect draw call buffer,
keeping everything GPU-side and avoiding the round trip. (A Unity implementation of the same idea
notes one wrinkle: the compute shader must append whole triangles, so the append-buffer count needs
multiplying by 3 with a tiny fixup shader before the indirect draw.)

The reason this isn't your default: **anything the player collides with needs its geometry
CPU-side anyway**, and a GPU→CPU readback is precisely the round trip you were avoiding.

So split it:

| Chunk class | Meshing | Collider |
| --- | --- | --- |
| Near, editable | CPU on `rayon` pool | Voxel collider (§4) |
| Far, visual only | GPU compute + indirect draw | None |
| Very far | Heightmap or impostor | Heightfield |

---

## 8. Build order

Slot after Phase 3 of the main doc — you want the agent loop working before adding a whole new
representation.

| Step | Work | Exit criterion |
| --- | --- | --- |
| V0 | `Chunk` + `ChunkMap` + i8 SDF + uniform collapse; op-list bake | A hand-written op list produces a field you can query |
| V1 | Surface Nets via `fast-surface-nets-rs`; chunk-seam correctness | A noise terrain renders with no cracks |
| V2 | Rapier voxel colliders; player walks on it | No tunneling, no ghost collisions |
| V3 | Edit path: CSG ops, dirty queue, off-thread remesh + recollide, redistancing | Carve a tunnel at 60fps, no hitches |
| V4 | MCP tools: `voxel_edit`, `voxel_measure`; validator checks from §5.3 | *"Carve a cave into the hillside and put a bunker at the entrance"* — verified by render **and** by a headless assertion that the player can walk in |
| V5 | Per-object volumes; destructible structures as prefabs | Blow a hole in a wall; the rest of the building stands |
| V6 | Debris pool with convex colliders and a hard cap | Sustained destruction doesn't degrade framerate |
| V7 | Dual Contouring behind the `Mesher` trait | A voxel building has sharp corners |
| V8 | LOD octree; DC seam meshes (or Transvoxel if still on Surface Nets) | Visible distance 1km+ with no LOD cracks |

**V4 is the gate**, same as Phase 3 was. If the agent can't author voxel terrain and verify it, the
rest is a graphics project rather than the thing you're actually building.

---

## 9. Crate additions

```
loom_voxel/           # chunks, SDF storage, CSG ops, op-list bake, redistancing
loom_voxel_mesh/      # Mesher trait; surface_nets + dual_contouring impls
loom_voxel_physics/   # occupancy extraction → rapier voxel colliders
```

Dependencies to pull: `fast-surface-nets` (meshing), `ilattice` + `ndshape` (lattice math — the
underlying types the ecosystem shares), `rapier3d` (voxel colliders), `rayon` (worker pool),
`transvoxel` (only if you stay MC-based). Read `bevy-sculpter` as a reference implementation before
writing V3 — it has already solved the brush/redistance/raycast loop, and it credits
`fast-surface-nets-rs` as its meshing inspiration.

---

## Sources

Godot Voxel Tools documentation — smooth terrains, blocky terrains, performance (voxel-tools
.readthedocs.io) and the Zylann/godot_voxel Transvoxel implementation notes · Eric Lengyel,
transvoxel.org and *Voxel-Based Terrain for Real-Time Virtual Simulations* · `transvoxel` Rust crate
docs · Ju et al., *Dual Contouring of Hermite Data* (2002) · Schaefer & Warren, *Dual Marching
Cubes: Primal Contouring of Dual Grids* · Nick Gildea's Voxel Blog, dual contouring seams/LOD and
generation performance · Boris the Brave, Dual Contouring Tutorial · swiftcoder's isosurface
extraction index · *Multi-Resolution Dual Contouring from Volumetric Data* · Rapier colliders
documentation (voxel shapes, trimesh flags) · `bevy_voxel_world`, `bevy-sculpter`,
`fast-surface-nets-rs` crate documentation · stijnherfst/BrickMap · Laine & Karras, *Efficient
Sparse Voxel Octrees* (NVIDIA, 2010) · *Aokana: A GPU-Driven Voxel Rendering Framework for Open
World Games* (arXiv 2505.02017) · rgerd/silk-clouds · bink.eu.org, Fast Voxel Data Structures.
