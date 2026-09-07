# ADR 0097 — A viewport that can answer a different question

- **Date:** 2026-09-07
- **Status:** **accepted** and built. The last Tier-2 item from
  `docs/design/EDITOR-SCOPE.md` §2.
- **Adds:** no crate, no dependency, no pipeline, no shader variant. An enum, a
  packed field, one shader function, a combo box and a CLI flag.

## 1. The gap

The viewport drew one thing: the game, lit and textured. That is the right
default and a poor debug tool. "Is that face flipped", "is this normal
unsmoothed", "what does the form read like without the textures carrying it" —
none of those could be asked, and the answers were only ever inferred from a
shaded picture.

## 2. Decision

Four modes, applied in the fragment shader after everything else:

| Mode | What it draws | What it answers |
|---|---|---|
| Shaded | lit and textured | the game |
| Unlit | albedo, no lighting | is that the texture, or the light on it |
| Normals | world normal as colour | is that face flipped or unsmoothed |
| Plain | untextured grey | does the form read without the textures |

**Fragment-only, deliberately.** Wireframe and overdraw would each need a
pipeline variant — a different polygon mode, a different blend state — and with
them a second set of pipelines to create, keep in step and tear down. These four
are a branch on a value already in the push constant, so the cost is a uniform
read and nothing in the renderer's structure changes.

## 3. Where the mode lives

Packed into the same word as the ablation bits — `environment[0].viewport.w` —
at **bits 12-15**.

That shift is arithmetic, not taste. The word travels to the shader as an `f32`,
which represents integers exactly only to 2^24. Bits 0-8 are the ablations, so
the mode goes above them; at bit 12 the largest value the word can carry is
`256 + (15 << 12)` = 61,696, comfortably exact. Somewhere roomier — bit 24, say
— would put the largest value at 2^28 and the packing would start rounding.

## 4. Plain is hemisphere plus sun, not sun alone

The first version was a straight lambert against the sun. Every face turned away
from it sat at the floor value, and the boat's hull came out as a silhouette —
which is the one thing a blockout view must not do, because form is all it has
left to show.

Adding a sky term keyed to `normal.y` means a deck, a wall and a soffit stay
three different greys whichever way the sun happens to point. That is the whole
job of the mode.

## 5. Reachable without a mouse

`loom render --view <mode>` exists as well as the editor's combo box. A mode
only reachable by clicking is one no gate row and no agent can check, and this
one is a packed integer unpacked in a shader — three places to get a shift wrong,
where the failure mode is a viewport that quietly keeps drawing shaded.

So the gate renders the same scene twice and requires the pictures to **differ**.
`cargo xtask image` covers the other direction: that Shaded still looks like
itself.

### The gate caught its own row

The first version of that row failed. `loom compare` exits nonzero when the
pictures differ — which is the *pass* condition here — and `green.sh` runs under
`set -o pipefail`, so the success case failed the pipeline. It passed in a
hand-run shell without `pipefail` and failed in the gate, which is the same trap
this file was hardened against earlier and is worth writing down twice.

## 6. What this does not cover

- **Meshes only.** The water, sky, grass and particle pipelines have their own
  fragment shaders and still draw shaded. For "is that face flipped" that is
  fine — the question is about geometry the mesh pipeline draws. For a whole
  screenshot in Normals it is visibly inconsistent, and honest to say so.
- **No wireframe, no overdraw** — see §2.
