# ADR 0044 — A reflected hit reads its own triangle

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Supersedes:** ADR 0021, whose decision (shade a reflected hit with the
  material's *mean* albedo) is now the fallback for a material with no map
  rather than the rule. ADR 0021's analysis of *why* the reflection looked
  blown out stands unaltered and is worth keeping.
- **Amends:** ADR 0019, which recorded "the material's base colour, not its
  texture" and "the hit normal is `-dir`" as the two known limitations of
  inline ray queries. Both are now gone.
- **Decision touched:** none locked. No new buffer, descriptor, barrier, pass
  or `ash` call.

## The number that misrouted two ADRs

**ADR 0021 deferred this feature on a fact that was not true.** Its blocker,
quoted verbatim: fetching a texel "would need the triangle's UVs, which needs
the index buffer, which is one more pointer than the push block has room for."
CLAUDE.md says the same in stronger words — "the push block is at its 128-byte
guarantee".

`renderer.rs` said it twice as "124 of its 128 bytes" and twice as "116". The
scene `Push` is a `float4x4`, six `DeviceAddress`es and a `u32`: **64 + 48 + 4
= 116 occupied**, padded to 120. A seventh pointer makes 124, padded to exactly
128 — legal, with nothing spare after it. The only assertion on a `Push`
anywhere in the crate was `rain.rs:717`'s, on a *different* struct of the same
name, which is how the wrong number survived. `the_scene_push_block_is_not_full`
now pins it, and the two wrong doc comments are corrected.

**The push block was never the constraint, and it turns out not to be needed
either.** `ObjectData` already carried a `uint4 material` using one lane of
four. Two of the spare lanes hold a `uint2` and — because `uint2` and a pointer
are both 8-aligned — the pointer that follows lands where the third and fourth
lanes were. `224 + 8 + 8` is the same 240 bytes the `uint4` occupied. The
record did not grow, and nothing above it moved.

## Decision

**Give each object its mesh's `first_index` and the shared index buffer's
device address, and let `tracedEnvironment` decode the triangle it hit.**

    uint slot = hitObject.material.y + query.CommittedPrimitiveIndex() * 3u;

Three vertices out of `push.vertices`, blended by
`CommittedTriangleBarycentrics()`. That is a real UV and a real interpolated
normal, and from them:

- **Albedo is a texel**, at a mip `reflectLod` chooses. Triplanar materials —
  `stoneyard`'s ground, every voxel rock, every terrain — project the *world*
  hit position exactly as the raster path does, including the macro-variation
  term. Everything else uses the real UV.
- **The normal is the interpolated vertex normal** through the object's
  existing inverse-transpose rows, flipped against the ray for a back-face hit.
  It replaces `-dir`, which was exact only head-on.
- **The steep-slope layer is blended in**, through the same `groundLayerWeight`
  the direct view calls. `lanternhead`'s banks are soil over stone; without it
  a reflected bank comes back stone with full conviction.

**The interpolated vertex normal, not the triangle's cross product.** Both are
reachable now. The interpolated one is what the rasteriser uses, so a sphere
reflects as a sphere rather than as a facetted one, and the reflection agrees
with the direct view of the same surface.

## Why `SampleLevel`, and never `Sample`

Implicit-derivative sampling asks the neighbouring lanes of the quad what UV
*they* got. For a reflection those lanes may have hit a different triangle of a
different object metres away, or the sky. The derivative is noise, the chosen
mip is whatever that noise implies, it compiles, and it looks nearly right in a
still. `sampleMapLevel` and `triplanarLevel` are twins of the raster path's
functions for exactly this reason — the raster path *wants* its derivatives.

`reflectLod` is `0.5 + log2(1 + t) + 4·roughness`. Distance because the
reflected image compresses into fewer pixels the further the ray travelled;
roughness because the direction handed in is already the widened lobe. The
roughness coefficient was chosen against the *direct view of the same surface
in the same frame*, which is the only reference available:

    coefficient        4.0     2.0     direct view
    materials floor    4.73    5.11    5.00
    stoneyard ground  10.43   11.32   18.61

Halving it buys 8–9% of high-frequency energy and takes `materials` past its
own direct view. More high-frequency energy per pixel than the thing being
reflected is undersampling, so the sharper-looking number is the one that
aliases. `ponytail:` the base term is a constant, not a footprint; the honest
version cones the lobe onto the hit surface and needs `GetDimensions`.

## What was measured

`hf` is mean |Laplacian| of luminance. `stoneyard` and `materials` at
3200x2000, identical boxes on both sides, baseline rendered from the parent
commit's binary in the same session:

| box | baseline | now |
| --- | --- | --- |
| stoneyard reflected ground `hf` | 1.05 | **9.73** |
| stoneyard reflected ground mean | 65.35 | **71.81** |
| stoneyard **sky control** mean / `hf` | 157.30 / 0.59 | **157.30 / 0.59** |
| materials reflected floor `hf` | 3.14 | **4.73** |
| materials reflected floor mean | 68.90 | **72.98** |

The direct view of that same `stoneyard` flagstone in the same frame reads
mean 76.06, `hf` 16.17. So the reflection went from 85% of the surface's
brightness to 94% of it, carrying 60% of its high-frequency energy — which is
what a reflection compressed into fewer pixels and read at a coarser mip should
do. **It moved brighter, not darker**, and it moved *toward* the thing it
reflects.

**The macro-variation term is why it agrees.** The raster path modulates all
triplanar albedo by a wide top-down sample; without it the reflected ground
reads 80.6 against the surface's 73.1, systematically brighter than the terrain
it reflects, and the two disagree where they meet. It is applied to the base
material and **not** to the steep-slope layer, because that is where the raster
path applies it.

**Cost, forward pass at 1920x1080:** `stoneyard` 1.011 → 1.014 ms, `materials`
0.402 → 0.407, `lanternhead` 0.894 → 0.909 — under 2% and inside run-to-run
range. **It cannot approach the occupancy cliff ADR 0019 found** (0.024 ms/ray
to eight rays, 0.101 ms after): this casts no new rays, it adds texture fetches
to the hit shading of a ray that was already being traced.

## The indexing was verified, not inferred

`CommittedPrimitiveIndex`'s origin is a convention — counted from the BLAS
build range, whose `primitiveOffset` is `first_index * 4` bytes — and indices
are absolute into the shared vertex buffer because `combine` rewrites them that
way. A wrong convention reads *a different real triangle*: plausible texture,
no error, nothing to report it.

`REFLECT_INDEX_DEBUG` (a `#define` at the top of `scene.slang`) interpolates the
three decoded vertex *positions* by the same barycentrics and returns
`|reconstructed − hit| × 80`. On `stoneyard` and `materials` at 1600x1000 it is
**black**. The control — `slot + 1u`, deliberately off by one vertex — is vivid
magenta and green across every reflective surface in the frame. The check is
falsifiable and it was falsified on purpose before it was believed.

It is left in the shader behind the `#define` rather than deleted: it costs
nothing when off, and it is either true for every triangle or false for every
triangle, so it is not worth a gate.

## Consequences

**`stoneyard` joins `GOLDEN`.** It was in `SCENES` only. The scene the bug was
reported in had no pixel gate, so ADR 0021 shipped and this fixed a visible
defect there without one reference moving. `materials` sweeps metallic to 1.0
but its floor has UVs; `stoneyard`'s ground is triplanar, which is a different
code path in `reflectedAlbedo` and the common one.

**Five references moved and all five were looked at**, ×8-amplified against
their references:

| scene | fraction | worst channel | why |
| --- | --- | --- | --- |
| proving_ground | 1.48% | 35 | **no textures at all** — moved from the normal alone |
| lanternhead | 0.56% | 41 | wet quay and cobbles gain crack detail; banks now soil |
| materials | 0.37% | 45 | the metallic spheres' floor band gains the tile lattice |
| homestead | 0.30% | 31 | wet rock along the shore |
| stoneyard | new | — | new reference, no prior |

The gate's ceiling is a worst channel of 72; every mover is well under it and
fails on changed *fraction*, which is what a diffuse correctness fix looks like.
`proving_ground` and `homestead` are dielectrics carrying the 4% Fresnel slice
— they were not predicted and they are right.

**Determinism is untouched.** Reflections are rendering-only; the sim hash is
still `b478ea4ac2622d32`. `xtask shimmer` scores `materials` and `cave` at
exactly **0.000** — every sample direction is a function of the pixel's integer
coordinate, so nothing here swims.

## What is still not fixed

1. **A reflected hit has no normal map, no AO, no soft shadow and no reflection
   of its own.** It gets one hard shadow ray, hemisphere ambient and a texel.
2. **`reflectLod` is calibrated, not derived**, and `xtask shimmer` cannot judge
   it: it holds the camera still, and a dolly's own parallax swamps the
   difference (three LOD settings inside 0.1% of each other on `materials`,
   including a deliberate `lod = 0` control). The measurement above is against
   the direct view, which is the best reference available and not a motion test.
3. **`meanAlbedo.rgb` is now read only on the no-map fallback path.** Its `.w`
   is still the alpha-test threshold, so the field stays. Trimming it is a
   separate subtractive commit against a pinned layout.
4. **`set_meshes` still never rebuilds the BLAS or `rt_positions`**, so a
   hot-reloaded mesh raytraces as its previous geometry. Pre-existing; this
   change fixed only the half that had become a *dangling* device address, and
   made the rest more visible by reading geometry through it.
5. **`raytrace.rs` still excludes alpha-tested objects from the TLAS** on the
   stated grounds that "a hit has no UVs". That premise is now false, so every
   leaf card casting no shadow is newly unblocked. **Its own ADR** — it changes
   what is in the acceleration structure, which is a cost decision.
6. **Grass, water, rain, fire and smoke remain unreflectable.** They are
   generated from `SV_VertexID` and are not in the TLAS at all. Anything that
   wants to be reflected has to become an `Object`.

## Human approval

Not required: no locked decision in CLAUDE.md moves. **CLAUDE.md's "the push
block is at its 128-byte guarantee" should be corrected to 116 of 128** — a
stale premise there misrouted this feature once already.
