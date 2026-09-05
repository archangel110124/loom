# ADR 0079 — A shower is a medium, and the drops are only its last sixty metres

- **Date:** 2026-09-05
- **Status:** **proposed.**
- **Decision touched:** none of CLAUDE.md's locked decisions. No new pass, no new resource, no
  new dependency, no GPU state, no post-process. **Governed by ADR 0045:** rendering-only,
  produces no force, unreadable by `loom sim --assert` and by `rhai`, adds no state — a frame
  stays a pure function of (scene, tick), so it adds no obligation to `cargo xtask repeat`.
- **Discharges ADR 0016's last open debt**, recorded in its own header: *"a distant curtain of
  rain crossing a bay would have to be drawn in the sky pass, and is a separate feature needing
  its own ADR."* This is that ADR, and the sky pass is now a volume march (ADR 0078), which is
  the thing that makes it cheap.
- **Refuses ADR 0016's step 5 as literally written** — *"Drops fall from the deck"* — and
  adopts the alternative 0016 itself named in the same breath. See §3.
- **Builds on ADR 0078**, whose cloud map is the surface this hangs from, and **ADR 0017**,
  whose stateful drops stay exactly as they are.

---

## 1. Context

The human's ask was *"real clouds that have real rain that come out of them."* ADR 0078 built
the clouds. This is the other half, and it is still missing in two different ways.

**Rain has no distance.** `RAIN_BOX` is `float3(64, 32, 64)` — 64 m across, 32 m tall, locked
to the camera, holding 131,072 drops at one per cubic metre (`assets/shaders/include/rain.slang:69`).
Beyond 32 m there is no rain in the world at all. You cannot see a shower approaching, you
cannot see one leave, and you cannot see one fall on the far side of a bay while you stand dry.
ADR 0016 made the *rate* vary across the world and `squall.loom` asserts raining-here /
dry-there — so **the simulation already knows about the squall the picture cannot show.**

**And the drops do not come from anywhere.** They wrap within their block. ADR 0016 step 5 was
to give them a real spawn ceiling at the deck.

---

## 2. A curtain is a medium, not a lot of drops

**Growing the drop volume is arithmetically dead, and the numbers are not close.** At the
shipped density of one drop per cubic metre:

| column | volume | against `RAIN_BOX` |
|---|---|---|
| 500 m radius to a 900 m deck | 9.0e8 m³ | **6,866x** |
| 2 km radius | 1.44e10 m³ | **109,863x** |
| 5 km radius | 9.0e10 m³ | **686,646x** |

Even at a thousandth of the density a 2 km curtain is 110 times the whole drop budget, and
every one of those drops would be sub-pixel — which is the exact case ADR 0017 measured as the
dominant source of temporal noise, and could not fix because the rain pass has no samples.

**So a distant shower is not drops. It is a participating medium** — a region of air with
water in it, extinguishing and scattering light. That is the same class of thing as the
sea-spray haze (`fogLayerDepth`, two exponential layers summed in optical depth) and the soot
volume (ADR 0020/0050), both of which ship.

### The density function, and it needs no new field

```
rain_density(p) = intensity · coverage(p.xz) · fall(p.y)
```

- **`coverage(p.xz)`** is `clouds_at(p, t).x` — the same weather map ADR 0078 marches the deck
  from and `loom_rain::cover_at` reads on the CPU. **A shower is under a cloud by
  construction**, and moves with it, because it is the same function.
- **`intensity`** is the `Rain` component's authored scalar, already in the environment buffer.
- **`fall(p.y)`** is the vertical profile between the sea and the cloud base: densest just
  under the deck, thinning downward as the shaft spreads and evaporates.

**`clouds_at` is not edited.** Its third output slot is `c(0.0)` and Nubis would put
precipitation there, but `intensity × coverage` is what this needs and the field stays frozen —
which keeps ADR 0016's "the clouds the eye sees and the rain the simulation feels are the same
function" true of the shower as well.

### It rides ADR 0078's march, and that is the whole implementation

The cloud map already marches a slab from `cloudBase` to `cloudTop` once per frame into a
direction-indexed image, and composites `(body, cover)`. **A curtain is a second slab under the
first**, from the water line to `cloudBase`, marched in the same loop, accumulated into the same
transmittance, written to the same texel.

No new pass. No new image. No new pipeline. The reflection path gets the shower for free,
exactly as it got the deck.

**And it lands where the map is currently wasted.** `horizonFade` zeroes the deck below
`dir.y = 0.30`, so the bottom third of the map's elevation range is presently blank — which is
precisely where a shower two kilometres away sits.

---

## 3. ADR 0016 step 5 is refused as written, and 0016 says why

Step 5 reads *"Drops spawn from the deck, modulating the existing volume rather than growing
it."* Its own §"Costs and risks" already saw the problem:

> **Cloud altitude introduces a scale question.** A deck at 800 m and a drop volume 32 m tall
> do not meet. Either drops spawn at the top of their own volume and are *modulated* by the deck
> above, or the volume grows. The first is almost certainly right; say so explicitly when
> building.

**Saying so explicitly, when building: the first is right, and it is already what ships.** A
drop released at 900 m falls for about thirteen seconds before entering the 32 m block, during
which it is not drawn and cannot be, because it is outside the only volume that exists. Giving
it a "physically meaningful spawn plane" would buy a number in a comment and nothing in the
picture.

What was actually missing is the thing between the deck and the block — and that is the medium
above, not a spawn altitude. **The drops stay exactly as ADR 0017 built them.** This ADR does
not touch `rain_sim.slang`.

---

## 4. What this changes in the picture

- A shower is visible as a distinct shaft under a cloud, kilometres away, and **moves with the
  cloud that made it** because both read one field.
- Standing in sun and watching a squall cross the bay — the thing ADR 0016 named as the point
  of the whole cloud field and could never show.
- Approach reads correctly: the shaft grows, and when it arrives the 64 m drop block is already
  raining, because both are `intensity × cover` at the same place and time.
- The horizon under a heavy deck greys out the way a real one does, rather than staying clear
  under a black sky.

---

## 5. Costs, and the escape hatch

**One more density evaluation per march step, on the steps below the cloud base.** ADR 0078
Addendum 5 measured the finished deck at +1.8 to +3.0 ms at 1080p, and the map's cost is fixed
regardless of resolution. The shower's marginal cost is measured before this ADR is promoted,
not estimated here — Addendum 5's own lesson is that single-shot GPU readings on this box spread
up to 55%, so the number will be min-of-3.

**`rain_curtain` joins `ABLATIONS` as its own row**, separate from `cloud_volume`, because the
two fail apart: a curtain that stopped drawing would leave the deck scoring a healthy 65% and
nobody would know. That is the same reasoning `spray_droplets` was split from `spray_haze` for
in `aa0bd9e`.

### Load-time refusal

A scene authoring a `Rain` component whose `intensity` is above zero while its `Environment`
authors `cloud_cover` explicitly at zero is refused, naming both values. Today that combination
is silently overridden — `rain_at_eye` forces cover to 1.0 only when cover is **unauthored** —
so a scene that deliberately says "heavy rain, clear sky" gets a shower with no cloud over it
and now, with a curtain, a visible shaft hanging from nothing. ADR 0016 already called that
combination *"a real weather state and an unusual one — say so in an ADR if a scene ever needs
it rather than adding a flag on spec."* No scene needs it; it becomes an error.

---

## 6. What this does not settle

- **Rain shafts do not light.** No god rays, no sun shafts through gaps in the deck. The curtain
  extinguishes and scatters ambient; it does not sample the sun the way ADR 0078's C3 march
  does. Reopening trigger: **a scene where the sun is low and behind a shower**, which is
  exactly `lanternhead`'s camera, so this may not stay shut long.
- **No wind shear.** A real shaft leans and trails downwind, and this one falls straight. The
  deck already advects on the wind so the shaft moves with it; what is missing is the *lean*.
  Reopening trigger: it reads as a column rather than a veil at high wind speed.
- **The shower does not wet what it falls on at distance.** `loom_rain::wetness` is per-scene
  scalars gated per pixel by cover, which is right and unchanged. A distant island darkening as
  a squall crosses it is not delivered.
- **No hail, no snow, no virga.** Virga — rain that evaporates before landing — is the one that
  is nearly free, since it is `fall(p.y)` reaching zero above the water. Not built.
- **The map's resolution binds here too.** ADR 0078 Addendum 5 recorded that masses under a
  degree wide are filtered out at 0.352 deg/texel. A narrow shaft far away is exactly such a
  feature. Reopening trigger: a shower that should be visible and is not.

---

## 7. Rejected alternatives

**More drops.** §2's table: 110,000x the budget for a 2 km curtain, every drop sub-pixel, and
sub-pixel streaks are what ADR 0017 measured as 97% of the frame-to-frame temporal noise in
`rain_impact` under a walking camera. Rejected on both counts.

**A billboard or particle curtain.** This is ADR 0015's argument about clouds, applying
unchanged: a curtain is large, overlapping and **alpha rather than additive**, so it would need
the depth sort the whole particle path was built to avoid. ADR 0015 rejected billboard clouds
for exactly this and the reasoning does not weaken for a shaft of rain.

**A separate pass or target for the shower.** Rejected as unnecessary: the cloud map is already
marched every frame, already direction-indexed, already sampled by both the background and every
reflected ray, and already leaves its lowest elevations blank. A second pass would duplicate all
of that to compute a term that fits in the first one's loop.

**Putting precipitation in `clouds_at`'s third channel.** Nubis's weather map has one, the slot
is literally `c(0.0)`, and it is still rejected: `intensity × coverage` is the same number,
authored where a scene already authors rain, and changing the field means moving `field_agree`
and everything ADR 0016 rests on for a value nothing else would read.

---

## 8. Human approval

Not required by CLAUDE.md's locked table — no locked decision moves. Required by this project's
rule that a builder never promotes its own ADR.

Recorded verbatim, from 2026-09-05:

- The original ask: *"clouds that look realistic and good … Like, they're real clouds that have
  real rain that come out of them."*
- On order: *"do adr then code"*.

**ADR 0078 §8 explicitly disclaimed the rain half of that ask.** This ADR is where it is
answered, and it should be read as the second of the two.
