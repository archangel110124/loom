# Ground textures

The `.png` files in `assets/textures/ground_*` were generated here. This
directory exists so they can be regenerated rather than being binary blobs whose
producer was lost.

```bash
/home/k-dorui/.claude/skills/comfyui-headless/comfy-start.sh
python3 tools/texture/generate.py soil "top-down photograph of dark brown \
crumbly forest soil, loose earth, small twigs and grit, flat even overcast \
lighting, no shadows, orthographic, photoreal, seamless texture, 4k" 4471
python3 tools/texture/seamless.py assets/textures/ground_soil_albedo.png
```

## Seamless means seamless, and it is checked

`seamless.py` scores the wrap boundary against the texture's own interior
gradient statistics as a z-score, and fails over 3.0. That threshold is not a
guess — it was calibrated against both ends:

| texture | worst z | verdict |
| --- | --- | --- |
| `grass_albedo.png`, authored as tiling | +1.86 | PASS |
| a random crop of a photograph | +24.93 | FAIL |

A genuinely tiling texture scores under 2 and a non-tiling one scores in the
twenties, so 3.0 sits in a wide gap rather than on a boundary. Textures with
strong periodic structure (the `tiles_albedo.png` checkerboard) score near 4
because their interior gradients are bimodal; that is a known limit and does
not affect stochastic ground.

**Run it on every generated texture.** The first soil texture produced here
looked perfect and scored +24 — statistically indistinguishable from a random
photo crop — because only the UNet had been patched for circular padding. The
VAE decoder's own convolutions re-introduce the seam. Both must be patched, and
without an objective check the broken one would have shipped.

## Two traps, both hit

**Patch the VAE as well as the UNet.** `Model Patch Seamless (mtb)` returns
`(passthrough, patched)` — the patched model is output **slot 1**. Taking slot 0
silently produces an ordinary image. Then `Vae Decode (mtb)` needs
`seamless_model = true` and `use_tiling_decoder = false`; the stock `VAEDecode`
undoes the UNet's work.

**Exclusions go in the negative prompt.** CLIP has no negation, so "absolutely
no cracks" in the positive prompt only ever adds cracks — it produced cracked
mud twice before the terms moved to the negative side.

## Normals

`Deep Bump (mtb)` infers a normal map from the colour map with a trained model
whose convolutions wrap, so a seamless albedo yields a seamless normal. This is
deliberately not height-from-luminance: differentiating brightness turns every
shadow in the albedo into permanent relief, which is why every prompt here asks
for flat overcast light with no shadows.

## Physical scale is not in the file

A generated texture carries no dimensions, so metres-per-repeat cannot be
derived and must be set by eye once against a known length. `assets/test/
ground.loom` contains a 1 m cube for exactly this, and records its chosen
`uv_scale` in a comment beside the value. Wrong texel scale is a more reliable
"this is a game" tell than tiling or material count, and no check here can
catch it.
