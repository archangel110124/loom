# ADR 0071 — A ladder that only got darker was not getting worse

- **Date:** 2026-08-21
- **Status:** **accepted** and built. Refines ADR 0068 (the operator) and
  ADR 0069 (the mood ladder); replaces neither.
- **Sits under:** ADR 0045. Everything here is render-side. The determinism
  hash is physics-only and did not move; `Play::state_hash` is
  `physics.state_hash()` and `fn sim` never constructs a `Renderer`.
- **Adds:** no pass, image, descriptor, pipeline, barrier or trait. One free
  function, one loop in an existing validator, one changed fallback, and a
  retune of two stages in one scene that is not in `GOLDEN`.

## The decision

**The far end of a mood ladder spends its budget on colour, not on darkness.**
When the intent is "the sea has gone wrong", the thing that has to escalate is
how wrong the colour is; brightness escalates only far enough to keep the
player able to see what they are doing. A rung that turns four dimmers at once
reads as dusk, and dusk is not a horror beat.

Concretely: `grade.saturation` **rises** toward the far end of a dread ladder
where intuition says drain it, and `exposure` carries whatever darkening is
wanted, on its own.

## Why — and the ladder that shipped did the opposite

Measured on `deeper_demo` at nine points on its own axis, from the game's own
camera, in three bands: the sky, the sea around the horizon, and the deck she
stands on. A whole-frame mean cannot answer this, because the design's own
stated intent is a claim about the *relationship* between the bands — "the sea
has gone the colour of an aquarium nobody has cleaned **while the deck you are
standing on is still lit**."

**Green excess in the sea band** (`G − (R+B)/2`), which is the aquarium axis:

    dread   0.00   0.125  0.25   0.375  0.50   0.625  0.75   0.875  1.00
    before  −1.39  +0.69  +2.60  +9.59  +11.00 +6.50  +3.75  +2.18  +1.03
    after   −1.28  +0.79  +2.70  +9.69  +11.10 +9.34  +8.43  +8.55  +8.28

**The cast peaked at the middle rung and was gone by the end.** The deepest
water — the whole point of the mechanic — was the *least* colour-cast rung in
the ladder, at +1.03 against the near rung's ability to reach +11. Frame
saturation ran 0.196 → 0.063 over the same axis.

**Brightness fell 4–10× faster in the back half.** Step-to-step whole-frame
luma: −3.8, −4.6, −5.4, −1.9 in the first half, then −13.7, −14.6, −17.4,
−20.5. The player crosses half the distance and sees almost nothing, then the
frame collapses.

**And the deck went with it.** Deck-to-sky luma ratio 0.48 → 0.23, deck landing
at luma 30 — dark enough that the working surface of a fishing game stops being
legible, against a design whose own words are "still lit".

Four dimmers were being turned together at the far end: `sun_strength`,
`ambient`, `exposure`, and `grade.saturation`. Raising the last of those and
easing the other three gives, after: back half at 2.5× the front rather than
4–10×, deck at 43 rather than 30, and the cast held to +8.28.

### The knob sensitivities that decided it

Rendered at `dread = 1.0`, one line changed each time:

| knob | change | luma | sea green |
| --- | --- | --- | --- |
| `grade.saturation` | 0.50 → 0.75 | **−0.1** | **+4.38** |
| `grade.saturation` | 0.50 → 0.25 | +0.1 | −4.21 |
| `exposure` | 0.66 → 0.85 | +17.6 | +1.34 |
| `exposure` | 0.66 → 0.50 | −16.7 | −1.26 |
| `grade.contrast` | 1.10 → 1.35 | −5.8 | +1.26 |
| `fog_density` | 0.058 → 0.090 | +0.5 | +0.21 |

**`saturation` moves the colour and nothing else** — ±4.4 of cast for ±0.1 of
brightness. That orthogonality is why it is the right knob and why the two must
not be turned together.

**`fog_density` is nearly inert at the far end**, which was a surprise and is
worth writing down: at `cloud_cover = 1.0` the frame is already fog-limited, so
a 55% increase buys half a code. Fog does its work in the *middle* rungs. It
was the intuitive answer to "make it more eerie" and it is the wrong one.

**`grade.contrast` costs deck legibility disproportionately** — −5.8 overall
but the deck alone goes 43.3 → 32.0. It is the last knob to reach for, not the
first.

## The five defects this ADR closes

Found by an independent verification pass over ADR 0068/0069's implementation.
Each was reproduced before it was fixed.

**1. A key no stage named stepped instead of ramping.** `mood_of`'s fallback
ladder had two rungs where its own doc comment promised three: a patch key that
neither bracketing stage names *and* the scene does not author made `mix1`
return `None`, dropping the key — and `environment_of_inner` then substituted
`Environment::default()` anyway. So the value was the default on one side of a
stage boundary and the authored number on the other. Live in the shipped game:
`inshore` authors no patch at all and the scene names no `cloud_cover`, so a
cloud deck appeared out of a clear sky in one tick as `dread` crossed 0.25.
Measured at 320×200, the same 0.0001 of `dread`: **mean 3.115 across the
boundary against 0.0032 just below it**, a thousandfold discontinuity. After:
0.0081, smaller than its own neighbour. **Rule: a patch ladder's fallback chain
must end where the consumer's fallback chain ends.**

**2. `mood_deep` could not see any of it.** It is the only gate on the mood
system, and both its stages named every key — so both sides of every `mix1`
always had a value and the dropped-key path was never taken. `exposure` is now
named by the far stage alone, which is that path; fault-injected, the row moves
100% of its pixels at mean 8.71. It also authored no `cloud_scale` while
claiming to pin "the scene's own value", so both answers to that test were
`Environment::default()`'s 1200 — **a fallback test whose two answers are the
same number is not a test.**

**3. The push-block test restated the layout instead of running it.** It
re-derived the byte offsets in its own body, so its assertions checked its own
copy against itself. Injecting a real swap into `record` — `saturation` at
offset 16, `gain[0]` at 28, so the shader would read the saturation as the red
gain — left it green. `push_block` is a free function now and the test calls
it.

**4. A stage's declared ranges were decoration.** `loom_reflect::validate`
walks a component's top-level keys, so `dread = 9.0` is refused while
`gain = [-9.0, …]`, `saturation = 50.0` and `at = 5.0` inside a stage all
validated clean. `at` is the one that failed *silently*: `dread` is a 0..1
scalar clamped to the authored span, so a ladder written `at = 0..5` uses only
its first fifth, every rung renders, nothing errors, and the sea never gets
worse. Now refused in `check_moods`, which already deserialises the component.

**5. One branch guarded two operations.** `graded` included
`push.contrast != 1.0`, which guards nothing — the contrast is applied
unconditionally between the two branches it gated — so a stage with neutral
gain, neutral saturation and moved contrast ran the inexact `lerp(l, c, 1.0)`
the branch exists to avoid. **Measured, it moved no pixel:** `l + 1.0*(c−l)` is
wrong by at most an ulp, ~1e-7 relative, three orders below one 8-bit code. The
fix buys provability, not pixels, and the comment now says so.

Two documentation defects are corrected in the same pass: two ADRs held the
number 0068 (the camera one moves to 0070, being cited by nothing outside
itself), and ADR 0068 undercounted its own re-bless — see its own header.

## What it does to the gate

**Nothing.** Every change in this ADR renders all 55 `GOLDEN` rows
byte-identical to the run before it, verified by building a binary differing in
exactly one line and diffing the whole set:

| change | rows moved |
| --- | --- |
| the fallback fix (defect 1) | **0 of 55** |
| the push-block extraction (3) | 0 — no shader input touched |
| the range refusals (4) | 0 — all 70 scenes in `SCENES` still validate |
| the branch split (5) | **0 of 55**, byte-for-byte |
| the ladder retune | 0 — `deeper_demo` is not in `GOLDEN` |
| `mood_deep`'s new key | that row only, and **it has no reference yet** |

**The 54-row re-bless that is still outstanding belongs entirely to ADR
0068's contrast line**, and this pass confirms that independently: setting
`CONTRAST = 1.00` and rendering the whole library returns all 54 rows to within
**worst-channel 23 and 0.27% of pixels** of the blessed references — inside the
gate's own `worst: 72`. Nothing else in either commit range moves a pixel.

Determinism, at the 300 ticks `cargo xtask validate` itself uses: `tower`
**b478ea4ac2622d32** and `river` **913a639c78956696**, both unchanged, and the
workspace's pinned-hash tests pass. `deeper_demo`'s hash is deliberately not
quoted — another agent was editing `deeper_rules.rhai` and `deeper_demo.loom`
throughout this work, so that scene's physics is not a stable reference for
anything in this ADR. `tower` and `river` are untouched by anyone.

## The instrument, and why the earlier numbers were wrong

Two of ADR 0068's own measurements did not survive re-measurement, and both
failures are the same shape — a number quoted from a partial view.

**"Shadows fall and highlights do not move; 99th percentiles within 11 codes"
is wrong in both halves.** Measured across all 54 references against current
renders — which costs no build at all, because *the blessed references are the
pre-change library* — p99 moves by more than 11 codes on **32 of the 54**, and
it moves **up**: `homestead` +20.4, `rain_overhang` +20.1, `rain_gantry` +19.2.
That is what the operator does. `PIVOT * pow(c/PIVOT, 1.30)` is an S about the
pivot, so everything above 0.30 linear brightens by construction, and "a slope
below the knee" names half the curve. p99 falls only on the three darkest rows,
where the frame lives below the pivot and the same expression is a uniform
dimming: `dripping` 69.7 → 54.7, `emberfall` 87.8 → 75.0, `campfire` 161.5 →
155.4.

**And "no blacks" deserves a number rather than a word:** zero pure-black
pixels anywhere in the library before or after; pure white 1 → 3 pixels;
channels at 255 167 → 443, of 10,368,000.

**The reference library is blessed with CMAA2 ON.** This is the second half of
ADR 0068's trap and it was not written down. `LOOM_CMAA2=0` answers "is the
operator what I published" only for an A/B between two renders you made
yourself; against `tests/references/` a CMAA2-off render is *further* away, not
closer — measured `ribbon` worst 47 and `ground` worst 95, against a library
that otherwise matches to within 23.

**The pacing is not what the script says it is.** `deeper_rules.rhai` claims
"the first change lands about five seconds out". Measured, holding W from the
berth: `dread` is **0.000 for the first ten seconds** and 0.060 at twenty, and
the whole ladder then runs in forty. She does pass the 25 m radius at about
five seconds — but the smoothstep is flat there and the 0.020 ease lags it, so
what a player can *see* begins around twenty. Distance is not the same question
as visible change, and the comment was answering the first while claiming the
second.

## Consequences

- **`grade.saturation` rising toward the far end will read as a mistake to the
  next person who sees it.** It is the counterintuitive half of this decision
  and the reason it is an ADR rather than a commit. `docs/design/THE-LOOK.md`
  says it in the first table on the page.
- **No gate can judge any of this and none should be built to.** Every number
  above is a measurement of taste, offered so the human can aim. `deeper_demo`
  stays out of `GOLDEN`: it is the scene whose whole job is to be retuned.
- **The ladder is still only ever judged as a still.** `flythrough` does not
  move `dread` and `shimmer` holds it fixed by design. Whether the escalation
  works in motion is unmeasured and unmeasurable here.
- **The four rows past `worst: 72` are CMAA2, not the operator**, and there are
  four of them, not three: `campfire` 103, `slosh` 85, `gleamsprat_beat` 81,
  `plume_roof` 76 — one to three pixels each, all on a single scanline, all
  dropping to 22–25 with CMAA2 off.

## Rejected

- **Making the far end darker still.** It is what the ladder already did and it
  is the defect.
- **Fog as the eerie knob.** Measured inert at the far end: +0.5 luma for a 55%
  increase.
- **Validating every nested number generically.** `check_moods` already
  deserialises the component for another reason; a loop over its stages costs
  nothing and needs no new machinery in `loom_reflect`. Teaching the reflection
  layer to walk nested types is a real change with a real blast radius, and one
  loop bought the whole benefit.
- **Blessing anything.** The coordinator owns `cargo xtask` and the re-bless is
  reviewed per row. A workflow that blesses 54 references is a workflow that
  has destroyed the project's ability to detect a regression.
