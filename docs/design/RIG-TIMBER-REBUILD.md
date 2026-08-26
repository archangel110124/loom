# The rig, rebuilt — rotting timber on rusted steel

**What this is.** The demo's hub — `Rig` in `assets/games/deeper_demo.loom` — is
83 nodes of `box` and `cylinder` primitives. Not one mesh, not one voxel, not
one texture. This document specifies replacing every drawn surface of it with
Blender-generated geometry and baked PBR textures, **without moving a single
number the simulation reads.**

It is the first textured hero asset in this project. `jib_vi` and `deckhand`
both carry flat per-node colour and no maps at all, so there is no precedent to
inherit and every trap below was measured here rather than remembered.

---

## 1. The decision the whole thing rests on

**Collision and appearance are separated, and the collision layer does not
change.**

Today the primitives *are* the colliders: `Sim::new` builds a static collider
from the drawn bounds of anything renderable (`crates/loom_cli/src/play.rs:768`).
Swap a box for a detailed mesh and physics moves with it — and roughly 57
`loom sim --assert` rows across eleven `rig_*.loom` scenes are watching.

The escape is `crates/loom_cli/src/play.rs:556-573`: when a node carries an
explicit `BoxCollider`, the half-extent is `|half_extents × world_scale|`
instead of the drawn bounds. So **every rig node keeps its collider numbers as
they are today, transcribed into an explicit `BoxCollider`, and only the drawn
geometry is replaced.**

**But that is not one rule, it is three.** This section stated transcription as
a single general technique. The whole-branch review at the end of Phase 0
proved it false in **two of the three cases Phase 1 will actually hit**. Only
the first case is the clean one.

#### Case 1 — axis-aligned, drawn, statically-parented node: exact, bit for bit

Transcription is exact. Measured `Rig/Drop.y` = **2.318190336227417** on the
mesh-plus-`BoxCollider` and **2.318190336227417** on the primitive it replaces
— the same digits, not agreement to a tolerance. This is the case the evidence
table below measures, and it is most of the rig, including the deck field
Phase 0 built.

#### Case 2 — rotated node: physically sound, NOT bit-exact

`play.rs:517` builds the collider from the node's world matrix and decomposes
it with `to_scale_rotation_translation()`, which **normalises the matrix
columns before extracting the quaternion**. For `diag(12, 0.2, 7)` those
divisions are exact and nothing moves. For `R·diag(…)` they are not, and the
quaternion comes back a few ULPs off the authored rotation. Measured
divergence: **5.8e-5 m** in the simple case, **1.3e-4 m** under a scaled
parent. The physics is right; the last digits are not.

Six demo nodes are rotated and will land here:

| node | `deeper_demo.loom` |
| --- | --- |
| `Rig/Slipway` | `:1930` |
| `Rig/SlipwayKerbNorth` | `:1947` |
| `Rig/SlipwayKerbSouth` | `:1960` |
| `Rig/Ladder` | `:2009` |
| `Rig/LadderKerbWest` | `:2025` |
| `Rig/LadderKerbEast` | `:2038` |

**Phase 1 must PLAN the re-pin, not discover it.** `green.sh` carries three
exact-equality asserts that a 1e-4 m divergence will move —
`state.lcg32 == 16672` and `state.landed_tick == 1345` (`green.sh:79-80`), and
`state.fight_ticks == 1177` (`:103`). A re-pin decided in advance is
bookkeeping. A re-pin discovered by a red gate is indistinguishable from a
regression, and the person who finds it will spend a day proving it is not one.

#### Case 3 — node under a dynamic ancestor: the technique INVERTS

**Do not transcribe these.** Adding a `BoxCollider` to a node under a dynamic
body does not preserve the physics, it changes it in both directions at once:

- **It creates physics that was not there.** Today a *primitive* child of a
  dynamic body gets **no collider at all**. The static arm (`play.rs:768-770`)
  requires `dynamic_ancestor(...).is_none()`; the pending arm (`:746-747`)
  requires an explicitly authored `BoxCollider`. A drawn primitive under a hull
  satisfies neither and falls through both.
- **It removes physics that was.** Authoring a `BoxCollider` puts the node in
  `pending`, and `pending` triggers `demote_to_mass_only` on the ancestor
  (`play.rs:806-816`), which turns **every collider that body already had**
  into a sensor (`loom_physics/src/lib.rs:331-340`). Measured: the hull stopped
  colliding and sank **1.1 m** through the ground.

Eight `Rig/Boat/*` nodes are drawn with no collider today and **must stay that
way**: `Float` (`:1827`), `Catch` (`:1862`), `Fins` (`:1880`), `FishHold`
(`:2261`), `FishHoldLid` (`:2286`), `HelmMat` (`:2299`),
`SternLadderRailPort` (`:2510`), `SternLadderRailStbd` (`:2523`).

#### The fourth trap: a collider needs a mesh

Not in the original claim at all, and it bites the moment someone tries to work
around case 3 with a collision-only box. The static arm requires
**`world.is_renderable(entity)`** (`play.rs:768`) as well as the absence of a
dynamic ancestor. So **a static node carrying a `BoxCollider` and no mesh gets
no collider whatsoever** — verified, the character falls straight through to
`y = -120.03`. Collision-only boxes work in `jib_vi_decks.loom` only because
those hang off a dynamic body and go down the pending arm; re-pointed at the
static rig they silently do nothing.

### Measured, not argued

The table below is **case 1**, which is the case it was taken in and the only
case it speaks for.

Probed 2026-08-25 on this machine. A character dropped 300 ticks onto a
4 × 0.4 × 4 m platform:

| platform | settled `Player.y` |
| --- | --- |
| `box` primitive, `scale [2, 0.2, 2]`, no collider | **2.1180999279022217** |
| imported OBJ, `scale [1,1,1]`, `BoxCollider [2.0, 0.2, 2.0]` | **2.1180999279022217** |

Bit-identical. And fault-injected three ways, because a check that has never
failed has never been tested:

| injected fault | result | reads as |
| --- | --- | --- |
| collider thickened to `[2.0, 0.4, 2.0]` | 2.3181 | **+0.2000 m exactly** — the check can fail |
| `BoxCollider` deleted entirely | 2.9591 | the 4 × 0.4 × 4 mesh collides as a **2 × 2 × 2 cube** |
| `BoxCollider` repeating the node's `scale`, on a unit-box primitive | 1.9699 | **0.1482 m below the drawn surface** |

The second row is the standing trap for every imported asset in this engine:
the fallback uses world scale, **not** the mesh's real bounds. `Mesh::bounds()`
exists (`crates/loom_asset/src/mesh.rs:50`) and the physics build path never
calls it. An imported mesh without an explicit `BoxCollider` collides as a 2 m
cube whatever its size.

The third row is a **defect already shipped in `deeper_demo`**, found by this
probe and deliberately not fixed here: `StepLow`, `StepMid` and `StepHigh`
(`deeper_demo.loom:2383`, `:2398`, `:2413`) each author `half_extents` equal to
their `scale`, so the boarding treads the player stands on sit ~0.15 m below the
treads they see.

The file argues against itself about this. The comment sitting immediately
below `StepHigh` states the rule correctly — *"`half_extents = [1, 1, 1]` and
the size is in `scale`. `play.rs` multiplies…"* — and `SternLadder`
(`:2495`) follows it, authoring `[1.0, 1.0, 1.0]`. The three treads do not.
`green.sh`'s `rig_ashore` row passes regardless, so something else carries the
climb. **Out of scope for this work. It needs its own change, its own gate row
and its own re-bless conversation.**

---

## 2. The export contract

Stated once, here, so it cannot drift. Every generator restates it in its own
docstring and prints it at build time — `build_boat.py:14-16` is the model, and
the reason it does so is that its axis comment said the wrong thing for eight
rounds and shipped the boat's sidelights reversed.

    Model in Blender native Z-up.
    Export  bpy.ops.wm.obj_export(forward_axis='NEGATIVE_Z', up_axis='Y',
                                  export_triangulated_mesh=True,
                                  export_normals=True, export_uv=True,
                                  export_materials=False,
                                  export_selected_objects=True)
    Permutation  build (x, y, z) -> obj (x, z, -y), determinant +1.
    Delivered    metres, Y up, -Z forward, +X right.

Four rules that are each a scar:

1. **Flip V on export.** Measured 2026-08-25: two identical quads, identical
   texture, differing only in the V convention. The flipped one reads correctly;
   the raw one is upside down. The importer takes `vt` exactly as written and
   flips nothing (`crates/loom_asset/src/mesh.rs:200-208`). Symptom of getting
   it wrong is not obviously "upside down" on a noise texture — it is UV islands
   landing on the atlas gutter, which reads as **black patches**.
2. **Strip every `o`, `g`, `s`, `usemtl` and `mtllib` tag**, and assert on
   read-back that none survived. A surviving tag makes `file.obj#Group` usable,
   and that path recentres the selection on its own bounding box
   (`crates/loom_asset/src/mesh.rs:268-276`) — which takes a model apart when
   the library *is* one model.
3. **`recalc_face_normals` per shell**, which is
   `normals_make_consistent(inside=False)` per connected region. A whole-mesh
   signed-volume check passes while most of a surface points inward, and Loom
   draws inward-facing surface **pure black** — both diffuse and
   `ambientVisibility` go to zero at once. Signed volume is the coarse net
   underneath, not the guarantee.
4. **One OBJ per material.** The engine never reads `.mtl` — no `mtllib` or
   `usemtl` handling exists anywhere in `crates/`. Colour, roughness, metallic
   and both maps come from the per-node `Material` component.

**Give every `[[asset]]` a real UUID `id`.** An absent `id` becomes `""`
(`crates/loom_scene/src/scene.rs:266`), and prefab asset merging keys identity
on exactly that (`crates/loom_scene/src/prefab.rs:606-624`), so every id-less
declaration is the same asset as every other one and the first adopted wins for
all of them. This once rewrote the boat's nine meshes to a `sphere` and drew
nine coincident unit spheres at the waterline, in a scene that validated clean.

---

## 3. Material zones, budgets and files

Each zone is one OBJ, one node, one `Material`, one albedo PNG and one normal
PNG. A zone is also a draw call, which is why there are ten and not thirty.

| zone | texture | tris (budget) | carries |
| --- | --- | --- | --- |
| `deck_timber` | 2048² | 12,000 | planks, cupping, gaps, rot patch, nosings |
| `steel_tidal` | 2048² | 6,000 | **the seam** — weed, barnacle, rust-to-rot |
| `steel_frame` | 2048² | 8,000 | piles, cross-bracing, bolts, brackets |
| `shed_timber` | 1024² | 6,000 | boards, battens, door |
| `roof_metal` | 1024² | 2,000 | corrugated, holed |
| `hardware` | 1024² | 4,000 | bollards, cleats, rings, fasteners |
| `cordage` | 1024² | 5,000 | rope coils, netting |
| `props_painted` | 1024² | 6,000 | crates, barrel, bench, lantern, spool |
| `lamp_glass` | none | 500 | mast lamp |
| `glass_mirror` | none | 12 | **untouched** — `metallic 1.0`, `roughness 0.0` |

**~50k triangles total.** `jib_vi` is 47,213 for a 19 m boat, so this is the
same order for a 24 × 14 m structure carrying more surface. There is no
engine-enforced triangle limit anywhere in `crates/`, and the project's own
measurements say the constraint is overdraw and AA, not triangle throughput
(`docs/design/loom-grass-system.md:79`). Budgets here are for iteration speed
and honesty, not for the GPU.

**Textures are generated as numpy fields, NOT baked in Cycles.** This section
used to say "baked from Blender procedural shader networks", and Phase 0 did
not do that — deliberately. Cycles is available here (OPTIX and CUDA on the
4090) and would give richer nodes, but a Cycles bake is **not bit-reproducible
across machines or driver versions**, and this project's entire verification
story is that a result reproduces byte for byte. `tools/mesh/rig/textures.py`
is a periodic value-noise fBm evaluated in numpy: same bytes on any box, no GPU
while somebody is at the machine, and the asset stays a pure function of its
parameters — the property `build_boat.py` has and the reason it survived a
machine migration intact. Phase 1's builders should extend that module, not
reach for Cycles.

The upgrade path, if procedural numpy genuinely cannot reach a surface, is a
Cycles AO + curvature bake **composited on top of** the generated maps — an
additional layer whose non-reproducibility is contained and declared, not a
replacement for the reproducible base.

Either way: no hand-painting, no external packs, no generated-image step. The
rot is reproducible and tunable rather than a file someone once made.

**Albedo maps are written sRGB-encoded, height and normal maps linear.**
`crates/loom_cli/src/materials.rs:194` loads `albedo_map` as `ColorSpace::Srgb`
and `:195` loads `normal_map` as `ColorSpace::Linear`. Palettes are authored in
linear reflectance (the same space a flat `albedo = [0.19, 0.17, 0.15]` is read
in) and composed there, then encoded on the way to bytes. Phase 0 shipped this
wrong once — linear floats written straight out as bytes, the engine decoding
them again, the deck 4-4.9x too dark. The asymmetry is the rule: colour gets
the transfer function, vectors never do.

---

## 4. Part decomposition

Ten parallel builders. Each owns one zone, one OBJ, one texture pair, and runs
its own critique/refine loop against reference before integration.

| # | part | notes |
| --- | --- | --- |
| 1 | deck field | the surface the player's eye is on constantly |
| 2 | substructure | six piles, bracing; hangs from y=1.00 to y=-1.40, touches no seabed |
| 3 | tide zone | where rust becomes rot. The horror seam; gets the most attention |
| 4 | bulwark + cap rail | including **both gaps**, which are gameplay, not decoration |
| 5 | hardware | bollards are *"the only thing in the scene that says the berth is a berth"* |
| 6 | shed | carries the mirror — see §5 |
| 7 | mast + lamp | `MastLamp` at intensity 220 is the hub's warmth; do not add lights |
| 8 | props | crates, barrel, bench, lantern, spool, thermos, fish crate |
| 9 | cordage | rope, netting |
| 10 | ramps | slipway + swim ladder, both rotated ±34.8° |

---

## 5. Frozen numbers

Each of these breaks something specific if moved.

| number | value | what it breaks |
| --- | --- | --- |
| quay edge | `z = -7.000` | the boarding lane and the hull clearance |
| deck top | `y = 1.400` | every spawn and every assert row |
| boarding rail gap | `x -6.900 … -3.100` | the demo's only taught gesture; a static collider in it is fatal |
| hull clearance | boat at `z = -10.45` leaves **0.15 m** | nothing may project north of `z = -7.000` for `x ∈ [-6.6, +5.2]`. She is 43,776 kg of dynamic body on live water |
| `Wind.speed` / `fetch` | `3.0` / `15000.0` | one number in two places; ~30 gate rows |
| mirror front face | `z = 2.550`, 5 mm proud of the shed | documented as having cost an hour once |
| `RAMPS` | `deeper_rules.rhai:895` | hardcoded ramp feet; move a ramp node and this moves with it |

**The demo's header is not a reliable source for these.** Five of its stated
numbers are stale against the nodes — spawn x (says -3.70, is -4.30), eye height
(says 0.75 and 3.05, is 0.82 and 3.12), boat position (says `x = -5, z = -11.2`,
is `x = 3.0, z = -10.45`), hull clearance (says 0.90 m, is 0.15 m), and the
supply line's z. **Read the nodes.**

---

## 6. Verification plan

Defined before building, per `CLAUDE.md` §5.

**Per OBJ, at generation time:**
- signed volume > 0 (the coarse net; per-shell recalc is the guarantee)
- zero surviving `o`/`g`/`s`/`usemtl`/`mtllib` tags — asserted on read-back
- triangle count within the §3 budget, printed
- bounds match the contract, printed

**Per node, at integration:**
- `loom validate` clean, no `asset_file_missing`
- `loom measure --node` bounds match the primitive being replaced
- `loom sim --assert` proves the player stands at **the same height as today,
  to the digit**, for every rig scene

**Per gate:**
- all ~57 existing `green.sh` assert rows pass unchanged
- new `GOLDEN` rows added — **both `SCENES` and `GOLDEN` bake their length into
  their type** (`[&str; 72]`, `[(…); 56]`), so the count must be bumped in the
  same edit or it will not compile
- `--sim` for each new row chosen by diff sweep, not by taste
- **every new row fault-injected** — stub the feature, measure what moves,
  record it. A row that does not move when the feature is deleted is not a gate
- `tools/scene_index.py` re-run to regenerate `assets/SCENES.md`, which is
  currently stale at "49 scenes" against an actual 72

**In motion:** `tools/watch.sh` contact sheets for anything that moves, read
row-major by the burned-in frame index, with `telemetry.csv` beside it. Two
signals agreeing is a finding; one alone is a hypothesis.

**Blessing is the verifier's act, not the builder's.** `--bless` has no per-row
filter — one invocation overwrites every reference.

---

## 7. Phasing

**Phase 0 — the tool and one proving part.** `split_parts.py`, the per-material
splitter cited at `build_boat.py:77`, **does not exist** anywhere on disk or in
the migration bundle. The mechanical heart of the pipeline is gone and must be
written again. Phase 0 rebuilds it, plus the generator skeleton and the bake
path, then takes **the deck field alone** end to end: model → unwrap → bake →
V-flip → export → `.loom` → render → gate.

Nothing else starts until one part is green end to end. Phase 0 is the honest
gate on whether the rest is worth doing.

**Phase 1** structure — deck, substructure, tide zone, bulwark.
**Phase 2** shed, mast, hardware.
**Phase 3** props, cordage, ramps.
**Phase 4** integration into `deeper_demo`, new gate rows, re-bless negotiation.

---

## 8. Risks and open questions

- **`deeper_demo` is the demo.** The rig deck is the only ground in the scene. A
  mistake here does not degrade the demo, it makes it unplayable. Every phase
  must leave the scene runnable.
- **`World::state_hash` eats node names and transforms**, so adding nodes moves
  it. `deeper_demo` is in `SCENES` but **not** `GOLDEN`, and
  `DETERMINISM_SCENES` is `["tower", "river"]` and nothing else
  (`xtask/src/main.rs:1784`). So no gate pins the demo's hash. The reason it is
  not a `GOLDEN` row is cost, stated in the array's own comment: *"`deeper_demo
  --sim 6000` is 69 s debug and four runs across `image` and `repeat`, which is
  why the game is not the row."* That reasoning holds for the rebuilt rig too —
  **gate the rig through dedicated `assets/test/` scenes, never by adding the
  demo to `GOLDEN`.**
- **The 56-row tone-map re-bless is still pending** and unrelated to this work.
  Two unrelated reasons for a red `image` gate is the situation the migration
  notes warned about. Do not let this build become the reason someone blesses.
- **Texture memory has no precedent here.** Ten zones at the §3 sizes is roughly
  100 MB of PNG. No budget exists to check it against.
- **Transparent surfaces are excluded from the acceleration structure** entirely
  — no ray-traced shadow, no appearance in reflections. Anything glazed must be
  authored knowing that, and the mirror is 1.7 m from where the player spawns.
- **Alpha-cut surfaces cast their whole triangle's shadow**, because ray queries
  never run a fragment shader. Netting authored as alpha cards will cast solid
  rectangles.
- **`--play` CPU budget is 30 ms/frame** (`xtask/src/main.rs:1647`) and the rig
  is in the scene every frame of the demo.

---

*Probes and measurements in this document were taken 2026-08-25 on the Omarchy
box, driver 610.57.04. The `Player.y` figures are from
`loom sim --ticks 300`, release binary.*
