# The look — every knob, in one place

**This is the handover page.** The tone mapping and the mood grade are the only
two things in this engine whose settings are pure taste, and taste is the one
thing no gate in this project can measure. So every number either of them owns
is listed here, with which direction is *more*, what one turn of it is worth
measured, and what it costs to change.

Read `docs/decisions/0068-a-slope-below-the-knee.md` for why the operator is the
shape it is, `0069` for the mood ladder, and `0071` for the defects found in
both and what the ladder measured.

---

## If you only read three lines

| you want | turn this | which way |
| --- | --- | --- |
| **brighter** | `PIVOT` in `assets/shaders/tonemap.slang` | **down**. 0.30 → 0.24 lifts the whole game. |
| **more eerie** | `saturation` in the last two stages of `assets/games/deeper_demo.loom` | **up**. 0.50 → 0.75 at `theedge`. |
| **less of everything** | `CONTRAST` in `assets/shaders/tonemap.slang` | **down toward 1.00**, which is off. |

**"More eerie" is up, and that is the one counterintuitive line on this page.**
The ladder used to desaturate to 0.26 at its far end and the measured result was
a frame that was *dark* rather than *wrong* — see §5. What makes deep water
frightening is colour that should not be there, and draining the colour deletes
it. Darkness is `exposure`, and darkness alone is not the effect.

---

## 1. The operator — `assets/shaders/tonemap.slang`

Global. **Every scene, and all 54 golden references.** Touching anything here
means a full re-bless; see §6.

| knob | now | more is | what a turn is worth |
| --- | --- | --- | --- |
| `PIVOT` | `0.30` | **lower = brighter** | It is the value that does not move. Everything below it darkens, everything above brightens. At 0.18 the game's own opening frame gets brighter, which was rejected — "not too bright" was the brief. |
| `CONTRAST` | `1.30` | **higher = deeper shadows and brighter highlights** | An S about the pivot. `1.00` is the off switch. `1.45` is where it hardens — `campfire`'s p01 reaches 0 and the sky posterises. |
| `KNEE` | `0.76` | **higher = highlights hold their colour longer before rolling off** | ADR 0018 chose it; nothing since has argued with it. Below the knee the pass is the identity, which is what makes "which references move" computable in advance. |

**Do not add a toe here.** A constant subtracted in linear light is a fifth of a
bright frame's shadows and nearly all of a dark one's, and it *raises* shadow
saturation because a constant is not a magnitude scale. Measured on
Khronos PBR Neutral; ADR 0068 has the numbers.

## 2. The scene's own exposure — `Environment.exposure`

Per scene, so it re-blesses **one row**, not 54. `1.0` is the identity leg of
the shoulder.

Use it when a scene's lights are right and its frame is not. Do **not** use it
to cancel something the operator did — that is two terms fighting in a file, and
the third person to author a scene cannot see either of them.

## 3. The mood ladder — `assets/games/deeper_demo.loom`

Five named rungs on a `0..1` axis, blended between neighbours. **`deeper_demo`
is not in `GOLDEN`, so every number here is free to retune** — nothing to
re-bless, ever.

Each stage carries an `[..environment]` patch (any key it does not name holds
the scene's own value, or the component default) and a `[..grade]`.

| per-stage knob | more is | measured at `theedge`, dread = 1.0 |
| --- | --- | --- |
| `grade.saturation` | **up = more colour = more wrong** | `0.50 → 0.75` moves the sea's green cast **+4.38** and the frame's brightness **−0.1**. The cleanest eerie knob there is: it changes the colour without touching the light. |
| `exposure` | **up = brighter** | `0.66 → 0.85` is **+17.6** luma and takes the deck from 43 to 53. This is the far end's brightness. |
| `grade.contrast` | **up = grimmer, and it costs deck legibility** | `1.10 → 1.35` is −5.8 luma overall but takes the *deck* from 43.3 to **32.0**. Reach for it last. |
| `fog_density` | **up = the world closes in** | **Nearly inert at the far end** — `0.058 → 0.090` is +0.5 luma and +0.21 green. At `cloud_cover = 1.0` the frame is already fog-limited. It does its work in the *middle* rungs. |
| `grade.gain` | per-channel, in linear light | Equal channels is an exposure, unequal is a white balance. The green lean lives here and in `sky_zenith`. |
| `at` | where the rung sits on the axis | Must be `0..1`, sorted, distinct — refused at load since ADR 0071. |

## 4. The pacing — `assets/scripts/deeper_rules.rhai`

How fast `dread` moves, which is a different question from what each rung looks
like.

| knob | now | more is |
| --- | --- | --- |
| near radius | `25.0` m | **up = the calm zone extends further from the rig** |
| span | `125.0` m (so full dread at 150 m) | **up = a longer journey to the far end** |
| rise rate | `0.020`/tick | **up = the mood catches up faster as she heads out** |
| fall rate | `0.004`/tick | **up = turning for home brings relief faster** |

**Measured, holding W from the berth** — this is what the pacing actually is,
and it is not what the comment in that file used to claim:

| seconds | 5 | 10 | 15 | 20 | 25 | 30 | 35 | 40 | 50 | 60 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `dread` | 0.000 | 0.000 | 0.003 | 0.060 | 0.173 | 0.325 | 0.498 | 0.671 | 0.940 | 0.998 |

**Ten seconds of nothing, then the whole ladder in forty.** She passes the 25 m
radius at about five seconds, but the smoothstep is flat there and the 0.020
ease lags it, so the first change a player can *see* lands around twenty. If
that opening dead zone is too long, lower the near radius; if the escalation is
too quick, raise the span.

## 5. What the ladder measures — and the one thing it got wrong

Rendered at nine points from the game's own camera, in three bands (sky, sea,
and the deck she is standing on), **before** the retune in ADR 0071:

- **The eerie cast peaked in the middle and died at the end.** Green excess in
  the sea band ran `−1.4 → +11.0` at dread 0.5 and then back down to **+1.03**
  at 1.0. Frame saturation `0.196 → 0.063`. The far end was the *least*
  colour-cast rung in the ladder.
- **Whole-frame brightness fell 4–10× faster in the back half** — steps of
  −3.8, −4.6, −5.4, −1.9 and then −13.7, −14.6, −17.4, −20.5.
- **The deck went with it.** Deck-to-sky luma ratio `0.48 → 0.23`, deck at luma
  30 — against the design's own stated intent, "the sea has gone the colour of
  an aquarium nobody has cleaned *while the deck you are standing on is still
  lit*".

Four dimmers were being turned at once at the far end: `sun_strength`,
`ambient`, `exposure` and `grade.saturation`. After the retune the cast holds
(`+11.1 → +8.28`), the back half runs at 2.5× the front rather than 4–10×, and
the deck lands at 43 instead of 30.

**Judge it in motion, not as a still.** Nothing in this project can: the mood
does not move during a `flythrough`, and `shimmer` holds `dread` still by
design. Take the boat out and hold W.

## 6. How to switch it off

| what | how | what it costs |
| --- | --- | --- |
| **the grade** | delete `stages` from the scene, or set every stage's grade to `gain = [1,1,1]`, `contrast = 1.0`, `saturation = 1.0` | Nothing. The shader takes a uniform branch on exactly that condition and the pass becomes the bare operator — not a lerp that happens to land on it. |
| **the operator** | `CONTRAST = 1.00` in `tonemap.slang` | **Measured: all 54 rows return to within worst-channel 23 and 0.27% of pixels of the blessed references** — inside the gate's own `worst: 72`, so the gate passes without re-blessing. Not bit-exact, because `PIVOT * pow(c/PIVOT, 1.0)` is a divide and a multiply rather than the identity. |
| **CMAA2** | `LOOM_CMAA2=0` | It defaults to **on** — it is read as opt-*out*. See the trap below. |

### The CMAA2 trap, in two halves

**It runs downstream of the tonemap and it is contrast-sensitive**, so raising
the contrast makes it anti-alias silhouettes it used to ignore. That is why four
rows report a `worst` above the gate's 72 while the operator's own residual is
2: `campfire` 103, `slosh` 85, `gleamsprat_beat` 81, `plume_roof` 76 — one to
three pixels each, all on one scanline, all dropping to 22–25 with it off.

**And the references are blessed with it ON.** So `LOOM_CMAA2=0` is only valid
for an A/B between two renders you made yourself. Compared against
`tests/references/`, a CMAA2-off render is *further* away, not closer — measured
`ribbon` worst 47 and `ground` worst 95 against a library that otherwise matches
to within 23. Two different questions, two different renders, and neither
answers the other.

## 7. What is deliberately not here

No bloom, chromatic aberration, depth of field, motion blur, vignette, dither,
film grain, or 3D LUT. Each was considered and refused — a LUT because 32,768
lines of floats defeats the project's first property, the rest because they are
effects looking for a reason. No AgX and no ACES: both desaturate a fire faster
than clipping does, which is the exact failure `fireRamp`'s capped top rung
exists to prevent.

**The sun disc is still wrong and is not a post-processing problem.**
`scene.slang:611` and `:641` both colour the disc and its glow with a constant
`float3(1.0, 0.94, 0.82)`, so neither `sun_strength` nor `sun_color` reaches
it: a scene can turn its sun down to 0.12 and the disc stays exactly as bright.
Only `cloud_cover` fades it, via `disc * (1.0 - cover)`.

That is why it is invisible at the far end of the ladder — `theedge` authors
`cloud_cover = 1.0` and the term goes to zero — and why it is most wrong at the
*near* end, which is the frame the player looks at first. One line, its own
re-bless, and it wants its own ADR.
