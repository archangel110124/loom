# Re-blessing the tone-map change — the procedure and the numbers

**Nothing here has been blessed.** `cargo xtask image --bless` has not been run
and must not be run by anyone who has not read this page. The write is atomic —
`image(bless)` has no row filter and `write_manifest` re-hashes the whole
directory — so **the review is per row and the write is one call.** That
asymmetry is the entire reason this page exists.

## What is outstanding

**All 54 existing `GOLDEN` rows move, and one new row has no reference at all.**

The 54 belong **entirely to ADR 0068's contrast line** in
`assets/shaders/tonemap.slang`. Confirmed independently rather than assumed:
setting `CONTRAST = 1.00` and re-rendering the whole library returns every one
of the 54 to within **worst-channel 23 and 0.27% of pixels** of the currently
blessed references — inside the gate's own `worst: 72`. Nothing else in any
commit since moves a pixel.

Verified per change, by building a binary differing in exactly one line and
diffing all 55 renders:

| change | rows it moved |
| --- | --- |
| the `mood_of` fallback fix | **0 of 55** |
| the `push_block` extraction | 0 — touches no shader input |
| the stage range refusals | 0 — validation only; all 70 `SCENES` still validate |
| the `tinted`/`desaturated` branch split | **0 of 55**, byte for byte |
| the mood ladder retune | 0 — `deeper_demo` is not in `GOLDEN` |

## Before you bless: four rows to open, and why only four

Rank by `worst`, not by `mean` or `fraction`. `fraction` carries no signal here
— a global tone curve moves every pixel of every row by construction, and 50 of
the 54 report a fraction above 0.99. `mean` is flat at 4–12 across the library.
**`worst` is the only column that separates "the curve did its job" from
"something else happened".**

Four rows exceed the gate's `worst: 72`:

| row | worst, CMAA2 on | worst, CMAA2 off | pixels past 72 |
| --- | --- | --- | --- |
| `campfire` | **103** | 25 | 1 of 64,000, at x 168 y 128 |
| `slosh` | **85** | 22 | 3, at x 77–79 y 102 |
| `gleamsprat_beat` | **81** | 22 | 1 |
| `plume_roof` | **76** | 25 | 1 |

**All four are CMAA2 anti-aliasing a hard silhouette correctly for the first
time, not regressions.** CMAA2 runs downstream of the tonemap and is
contrast-sensitive by construction: raise the contrast and it finds edges it
used to ignore. Each is one to three pixels on a single scanline.

**Do not "verify" this by comparing a `LOOM_CMAA2=0` render against
`tests/references/`.** The references are blessed with CMAA2 **on**, so a
CMAA2-off render is *further* from them, not closer — measured `ribbon` worst
47 and `ground` worst 95, against a library that otherwise matches to within 23.
`LOOM_CMAA2=0` is only valid for an A/B between two renders you make yourself.

## The procedure

1. **Confirm the tree is the one you mean to bless.** `git status --short`
   should be clean. Several agents worked in this worktree concurrently; their
   commits interleave.

2. **Run the gate and read its output rather than its exit code.**

       cargo xtask image

   Expect: 54 rows changed, and `mood_deep` reported as having no reference.

3. **Open the four rows above, before and after, side by side.** One sheet, not
   four openings:

       magick montage <before>/campfire.png <after>/campfire.png \
                      <before>/slosh.png <after>/slosh.png \
                      <before>/gleamsprat_beat.png <after>/gleamsprat_beat.png \
                      <before>/plume_roof.png <after>/plume_roof.png \
              -tile 2x4 -geometry +2+2 -label '%f' sheet.png

   The "before" images are `tests/references/` themselves — they *are* the
   pre-change library, which is why no build is needed to produce a before.

4. **Then look at the whole library at once**, for anything the ranking missed:

       magick montage tests/references/*.png -tile 8x7 -geometry +2+2 -label '%f' before.png

   and the same over the fresh renders. What you are looking for is a row that
   changed *character* — a black that crushed, a sky that posterised, a flame
   that bleached — not a row that got contrastier. Measured, none did: zero
   pure-black pixels anywhere in the library before or after, pure white 1 → 3
   pixels, channels at 255 167 → 443 of 10,368,000.

5. **`mood_deep` is a new row and needs its first reference in the same call.**
   It is the only gate on the mood system. Look at it: it should read as a
   sickly green-grey sea under a cloud deck with two lit boxes on a jetty,
   composed at `dread = 0.65` — between its two stages, not at either endpoint.

6. **Bless.**

       cargo xtask image --bless

7. **Read the manifest diff as the record of what you did.**

       git diff tests/references/MANIFEST.txt

   55 hashes should change or appear. That diff is the readable half of an
   otherwise opaque binary commit — it is why the manifest exists.

8. **Re-run the full gate**, including the two checks `image` cannot express:

       cargo xtask image      # now clean
       cargo xtask repeat     # three fresh processes, byte for byte
       cargo xtask validate   # zero Vulkan validation messages
       scripts/green.sh

   **`cargo xtask validate` is the one check nobody in this chain has run**
   since the tonemap's push range went from 12 to 32 bytes. It is the only
   green check with no substitute, and it should be run *first* if anything
   here is going to fail.

## The full per-row table

Rendered at `320x200` with each row's own extras taken programmatically from
`GOLDEN` in `xtask/src/main.rs` — never a hand-typed `--sim`, because a wrong
one produces a residual indistinguishable from a regression. Sorted by `worst`.

| row | fraction | mean | worst | note |
| --- | --- | --- | --- | --- |
| `campfire` | 1.0000 | 9.78 | **103** | CMAA2, 1 px |
| `slosh` | 0.9978 | 8.73 | **85** | CMAA2, 3 px |
| `gleamsprat_beat` | 1.0000 | 8.45 | **81** | CMAA2, 1 px |
| `plume_roof` | 1.0000 | 5.68 | **76** | CMAA2, 1 px |
| `vale` | 1.0000 | 9.97 | 66 | |
| `whitecaps` | 0.9999 | 8.98 | 60 | |
| `lanternhead` | 0.9994 | 9.09 | 56 | |
| `plume` | 1.0000 | 4.80 | 52 | |
| `proving_ground` | 1.0000 | 8.10 | 49 | |
| `homestead` | 0.9991 | 8.08 | 49 | |
| `forest` | 1.0000 | 6.67 | 47 | |
| `gleamsprat_cruise` | 1.0000 | 8.43 | 44 | |
| `gleamsprat` | 1.0000 | 8.89 | 43 | |
| `primitives` | 1.0000 | 5.54 | 42 | |
| `grass_slope` | 1.0000 | 9.77 | 41 | |
| `ribbon` | 0.9979 | 5.71 | 41 | |
| `plough_cinematic` | 0.9996 | 7.40 | 41 | |
| `emberfall` | 1.0000 | 11.52 | 40 | |
| `pool` | 1.0000 | 8.89 | 40 | |
| `stoneyard` | 1.0000 | 7.07 | 38 | |
| `spindrift` | 1.0000 | 9.21 | 38 | |
| `ocean` | 0.9999 | 8.94 | 37 | |
| `meadow` | 1.0000 | 8.94 | 36 | |
| `squall` | 1.0000 | 9.21 | 36 | |
| `plough_cinematic_low` | 0.9998 | 8.57 | 36 | |
| `mirrorpool` | 1.0000 | 8.45 | 35 | |
| `water_crate` | 0.9999 | 9.08 | 35 | |
| `plough` | 1.0000 | 9.30 | 34 | |
| `spout` | 0.9998 | 4.07 | 33 | |
| `shore` | 0.9999 | 8.53 | 33 | |
| `cave` | 1.0000 | 5.52 | 32 | |
| `ground` | 1.0000 | 10.56 | 32 | |
| `alpha_cutout` | 1.0000 | 7.28 | 31 | |
| `pool_jet` | 1.0000 | 8.54 | 31 | |
| `viewport_rect` | **0.3750** | 1.57 | 30 | the scissor holds — see below |
| `plume_gale` | 1.0000 | 5.41 | 30 | |
| `splash` | 1.0000 | 8.81 | 30 | |
| `dripping` | 1.0000 | 10.88 | 29 | |
| `beach` | 0.9999 | 5.94 | 29 | |
| `underwater` | 1.0000 | 10.57 | 28 | |
| `materials` | 1.0000 | 5.81 | 27 | |
| `puddles` | 1.0000 | 11.28 | 27 | |
| `explosion` | 1.0000 | 5.01 | 26 | |
| `windy` | 0.9998 | 6.77 | 26 | |
| `smoke` | 1.0000 | 5.93 | 25 | |
| `cascade` | 1.0000 | 7.09 | 25 | |
| `rain_pool` | 1.0000 | 9.38 | 24 | |
| `river` | 1.0000 | 5.30 | 23 | |
| `glass` | 1.0000 | 7.71 | 22 | |
| `plume_close` | 1.0000 | 4.58 | 22 | |
| `rain_overhang` | 0.9985 | 5.65 | 22 | |
| `rain_impact` | 0.9994 | 10.02 | 22 | |
| `rain_gantry` | 0.9998 | 9.22 | 22 | |
| `wake` | 0.9999 | 9.30 | 22 | |
| `mood_deep` | — | — | — | **new row, no reference** |

**`viewport_rect` at 0.3750 is a result, not a curiosity.** Its ceiling is
24,000 of 64,000 pixels = 0.375, because the tonemap is scissored to the
placement rect and the editor chrome outside it is never graded. A fraction of
1.0 on that row would mean the grade had escaped into the UI. `loom_render`'s
pinned chrome-corner test (`[0x0E, 0x10, 0x13]`) is the other half of that
guard.

## If you decide not to bless

The off switch is one number: `CONTRAST = 1.00` in
`assets/shaders/tonemap.slang`. Measured, that returns all 54 rows to inside
the gate's tolerance against the *current* references, so the gate goes green
with nothing re-blessed. See `docs/design/THE-LOOK.md` §6.
