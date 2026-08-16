# ADR 0020 — A reflected hit shades with the material's *mean* albedo

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** none locked. This narrows one limitation ADR 0019
  recorded — "the material's base colour, not its texture" — without taking the
  index-buffer step that ADR closes on.

## Context — the bug was in the interaction, not in either half

`tracedEnvironment` shades a reflected hit with

```slang
albedo = push.materials[hitObject.material.x].albedo.rgb;
```

and the comment underneath explained why it is not a texel: fetching one needs
the triangle's UVs, which needs the index buffer, which is one more pointer than
the push block has room for. That was a defensible trade on its own.

**What made it look broken is a convention on the other side of the engine.**
Every textured material in this project authors `albedo = [1.0, 1.0, 1.0]` and
lets the map carry the colour — `stoneyard`'s shelf, both boulders, the log, the
lantern and the voxel rocks all do, and so does every textured surface in
`materials.loom` and `lanternhead.loom`. Measured mean *linear* albedo of the
five stone textures involved:

    quay_stone      [0.201, 0.168, 0.140]
    rock_beach      [0.190, 0.159, 0.106]
    rock_boulder_a  [0.254, 0.238, 0.188]
    rock_boulder_b  [0.199, 0.160, 0.089]
    tree_log        [0.186, 0.126, 0.076]

So the reflection was shading with 1.0 where the surface reflects about 0.2: a
factor of five too bright, and with the tint thrown away as well, since 1.0 is
grey by construction. Every reflected object came back a flat blown-out cream
silhouette. The geometry was plainly right and the colour was plainly wrong,
which is how a human found it in `stoneyard` within seconds of opening it.

## Decision

**Store each material's average linear albedo and shade the reflected hit with
that.** `MaterialData` grows a fourth `float4`, `meanAlbedo`, holding the
authored `albedo` multiplied by the mean colour of its albedo texture;
`tracedEnvironment` reads it in place of `albedo.rgb`.

**The mean is the 1x1 tail of the mip chain, which already exists.**
`loom_asset::texture` builds the whole chain in linear light — that is not
incidental, it is what the module was written for and what
`srgb_levels_are_averaged_in_linear_light` pins — so its last level *is* the
linear mean, re-encoded once. `Texture::mean_linear` decodes three bytes. No
image is walked that was not walked anyway.

Taking the mean on the bytes instead would have reproduced the original bug at
half strength: the sRGB curve is convex, so a byte average read as a linear
value is two to four times the light the surface actually reflects — measured,
`quay_stone` 0.476 against 0.200, `tree_log` 0.460 against 0.186.

## Consequences

**Cost.** One `float4` per material: `MaterialData` goes 48 → 64 bytes, and a
scene's material table is tens of records. Nothing per pixel, no new descriptor,
no new binding, no extra ray — the shader reads sixteen different bytes of a
record it was already fetching. Measured with `LOOM_GPU_TIMING=1` on `stoneyard`
at 1920x1080, five runs each side, the forward pass is **0.979 ms before and
0.979 ms after** — the same median, with the two spreads (0.962–0.989 and
0.965–1.002) overlapping almost exactly.

**Untextured materials are bit-identical.** With no albedo texture the mean is
exactly `[1, 1, 1]`, and multiplying by 1.0 is exact — so every scene reflecting
a plain-coloured surface renders byte for byte as it did.
`an_untextured_material_reflects_exactly_its_albedo` asserts equality rather
than closeness, deliberately.

**The two layouts are pinned.** `MaterialData` was one memory layout described
twice with nothing checking it;
`the_material_record_is_laid_out_as_the_shader_reads_it` now does, in the same
style as the `EnvironmentData` test beside it. The numbers are Slang's, read out
of the compiled module: `spirv-dis target/debug/build/loom_render-*/out/scene.spv
| grep MaterialData_natural` gives offsets 0/16/32/48 and `ArrayStride 64`.

**What this does not fix, and it is a real ceiling.** An average is one colour
for a whole texture. A piebald rock reflects as a uniform one; a texture whose
content *is* high-contrast pattern — a checkerboard, brickwork read close up —
reflects as its mean grey with the pattern gone. That is invisible at the
roughnesses these reflections are usually seen at and obvious in a mirror, and
the honest fix is still the one ADR 0019 named: the index buffer reachable from
the fragment shader, and per-hit UVs. This makes that step less urgent rather
than unnecessary.

**There is a cheaper way out of that ceiling than the index buffer, and it was
rejected here rather than missed.** For a `FLAG_TRIPLANAR` material — which is
`stoneyard`'s flags and voxel rocks, `ground`, and every terrain in the project
— the shader needs no UVs at all: triplanar projection derives them from the
*world position*, and the reflected hit position is already in hand as
`origin + dir * CommittedRayT()`. One `SampleLevel` at a coarse mip along the
dominant axis of `-dir` would give real spatial variation for the majority of
reflected surfaces. It is not this change because it costs a texture fetch per
reflected pixel where this costs none, and because it covers only triplanar
materials, so the general case still ends at the index buffer. It is the
obvious next step if reflections need to carry pattern.

**Second-order light is also missing.** The mean is the texture's average
reflectance, not the average *radiance* leaving the surface, so a reflected
surface still carries no shadow detail, no AO and no normal-map response — the
hit is shaded with a single normal (`-dir`, ADR 0019) as before. Only the base
colour changed.

**Golden references moved, and two of them meaningfully.** `materials` (0.85% of
pixels, worst channel 75) and `lanternhead` (4.28%, worst 31) both have
reflective surfaces looking at textured ones: the metallic spheres' lower halves
lose the cream wash and pick up the terracotta of the slab they stand on, and
the dark plinth on the quay loses a milky sheen it should never have had. Four
more — `beach`, `ground`, `meadow`, `smoke` — moved below the gate's tolerance
(worst channel 1–9, at most 0.008% of pixels) because rough dielectrics still
carry a `0.04 × 0.3` slice of the term; `--bless` rewrites those too, so the
MANIFEST diff is six lines rather than two.

## Human approval

Not required: no locked decision in CLAUDE.md moves, and this narrows a
limitation ADR 0019 recorded as a known trade rather than as a locked choice.
