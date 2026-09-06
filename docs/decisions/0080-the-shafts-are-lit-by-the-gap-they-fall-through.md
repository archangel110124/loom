# ADR 0080 — The shafts are lit by the gap they fall through, and that is worth about six percent

- **Date:** 2026-09-05
- **Status:** **accepted** (2026-09-05, human — "promote the four ADRs to accepted"). Built and green on all six
  checks at the time of acceptance.
- **Decision touched:** none of CLAUDE.md's locked decisions. No new pass, no new resource, no
  new dependency, no GPU state, no post-process. **Governed by ADR 0045:** rendering-only, no
  force, unreadable by `loom sim --assert` and by `rhai`, no state.
- **Fires ADR 0079 §6's first reopening trigger**, which read: *"Rain shafts do not light…
  Reopening trigger: **a scene where the sun is low and behind a shower**, which is exactly
  `lanternhead`'s camera, so this may not stay shut long."* It did not.
- **A new ADR rather than an addendum to 0079**, on this project's precedent: ADR 0014 deferred
  stateful drops with triggers and ADR 0017 exists because they fired; ADR 0016 deferred the
  curtain and ADR 0079 exists for the same reason. Addenda are for corrections; a deferred half
  arriving gets a number.

> **Read §4 before §3.** The technique in §3 is built and works. §4 is why it does not look
> the way its name suggests, and it is the part worth keeping — it cost four hypotheses, three
> of which were wrong, and it will stop the next person repeating them.

---

## 1. Context

ADR 0079 shaded the shower with a constant, recording in that constant's own doc comment that
it was the known gap: *"Rain does scatter sunlight — that is what a sunlit shaft against a dark
sky is — and this does not march the sun."*

Both scenes the feature is registered on have a low sun: `squall` at
`[0.20, 0.35, -0.92]`, about 20 degrees, and `lanternhead` at `[-0.28, 0.17, -0.94]`, about 10.

## 2. The geometry, which decides the technique

A shaft point is lit if the sun ray leaving it clears the deck. At 10 degrees, a point at sea
level under a 700 m base travels **700 / 0.17 ≈ 4.1 km horizontally** before reaching cloud —
so what shadows a shaft is cloud four kilometres upwind.

**`cloudSunlight` cannot serve this.** C3 sized its steps to walk *inside* the slab: 6% of
thickness growing 1.8x over six steps, reaching about 2.5 thicknesses. From below the base most
of those land in empty air and the reach does not dependably arrive.

## 3. Decision: intersect the slab, do not march to it

The sun ray's crossing of the slab is closed form — `(base - p.y)/sunDir.y` to
`(top - p.y)/sunDir.y` — so four samples between those two, through the same `cloudDensity` the
deck uses, and Beer–Lambert. **Nothing is spent on the kilometres of clear air**, which is the
whole cost of a march and none of the information. `sunDir.y <= 0` returns no light rather
than dividing by zero.

The phase is `smokePhase` **divided by its own peak** (4.23 at `SMOKE_G = 0.42`), so the gain
sets the shaft's tonal range directly — 0.10 unlit to 1.00 lit and facing the sun — and cannot
saturate. §4 is why it is written that way rather than as a plain gain.

---

## 4. What it actually delivers, measured — and three hypotheses that were wrong

The effect is **real, correct, and much subtler than "god rays" implies.** Every number below
is the standard deviation of a horizontal band through the shaft region of
`squall`-at-cover-0.70, 640x400, `--sim 300`, in 0-255 luma. Standard deviation because a beam
*is* spatial contrast; a mean cannot see one.

### The probe, and fault-injecting it

The shadow term was painted raw into the shaft colour. **Then it was fault-injected** — the
term replaced by a hard 50/50 stripe, the most contrast any lighting signal can carry:

| painted into the shaft | stdev |
|---|---|
| a hard 50/50 stripe — the ceiling | **15.60** |
| the real shadow term, raw | 12.00 |

That single pair reframed everything. **The composite can express at most 15.60 of 255 — about
6% contrast — in that band, whatever the lighting does.** Every later number is a fraction of
something known rather than a number on its own.

### Falsified: the cover was too low

*"Beams need high cover with sharp gaps; `squall` at 0.45 is too open."* Sweeping cover with
the raw probe:

| cover | mean | stdev |
|---|---|---|
| 0.45 | 223.0 | 9.5 |
| 0.70 | 170.5 | 12.0 |
| 0.85 | 164.3 | 11.5 |

**Cover moves the mean and not the spread.** More cover is a darker shower, not a beamier one.

### Falsified: the shadow was undersampled along the view ray

*"16 steps over 8 km is 500 m apart against 260 m masses, so the shadow pattern aliases away."*

| curtain steps | spacing | stdev |
|---|---|---|
| 16 | 500 m | 12.02 |
| 48 | 166 m | 11.61 |
| 128 | 62 m | 11.60 |

Converged. Step count does not touch contrast. **This one had already been "tested" by eye at
48 steps and called null** — which is exactly the null result `loom-diagnosing-a-render-defect`
§6 says to distrust. The number is what settled it.

### Half-falsified: opacity was the limit

Raising `RAIN_CURTAIN_DEPTH` *does* raise the ceiling — and the real signal does not follow:

| depth | ceiling (fault-injected) | real signal | using |
|---|---|---|---|
| 1.2 | 15.60 | 6.75 | 43% |
| 2.5 | 21.21 | 6.79 | 32% |
| 4.0 | 25.20 | 6.85 | 27% |
| 7.0 | 30.44 | — | — |

So a more opaque shower *can* carry more contrast, and the thing stopping it was elsewhere.

### The actual cause: the gain

`RAIN_CURTAIN_SUN` was first 0.55, which pinned every lit sample to white because the phase
peaks at 4.23; then 0.12, which crushed the range. Both were guesses. Normalising the phase by
its peak and setting the range directly:

| shaft range | stdev | of ceiling |
|---|---|---|
| 0.34 + phase x 0.12 | 6.75 | 43% |
| 0.24 – 0.89, phase normalised | 8.26 | 53% |
| **0.10 – 1.00, phase normalised** | **10.53** | **68%** |

### The limit, stated plainly

**68% of 6% is still 6%.** Even flawless lighting cannot produce dramatic beams through a
shower this transparent, because the composite has nowhere to put them. Beams would need
optical depth around 7 — a ceiling of 30.44 — and a shower that opaque hides the horizon
behind it.

**That is the trade, and it is now a measurement rather than an argument:** in this engine a
rain shaft can be translucent or it can have beams, not both.

---

## 5. What this does not settle

- **The sun disc is not occluded by the shaft.** It is drawn additively in `skyColor` after the
  deck composites. Reopening trigger: a scene where the sun sits behind a heavy shaft and stays
  at full brightness.
- **No shafts on the ground.** A god ray that lands is a bright patch on the sea, and nothing
  here writes to water or terrain. That belongs with cloud shadows on the world (ADR 0078 §6,
  unbuilt).
- **`RAIN_CURTAIN_DEPTH` is an engine constant, not a scene field.** §4 says a scene *could*
  choose beams over translucency. Exposing that is a real option and is deliberately not taken
  here — it needs a load-time range refusal and an argument about which scenes want it.
  Reopening trigger: a scene that wants a genuinely opaque shower.
- **No inscattering along the view ray.** `fogAmount` handles atmosphere in closed form and
  does not know where the sun is.

## 6. Rejected alternatives

**A screen-space radial blur from the sun.** The classic cheap god ray, and it needs a
full-screen post-process — ADR 0010's boundary, *"moved, not eroded"*. It would also be wrong
here in a way that shows: a screen-space effect cannot appear in a **reflection**, and ADR 0078
Addendum 3's finding was that the water pass is where this engine's sky cost lives, because
water reflects the sky. Shafts that vanish in the water would be worse than no shafts.

**Marching to the deck with `cloudSunlight`.** §2 — its steps are sized for the slab's interior,
most would land in clear air, and its reach does not dependably arrive. Reuse for its own sake.

**A shadow map from the sun.** A second render of the deck from the sun's direction, a new
target, a new pass, a projection to keep in step. The slab is an analytic volume; its shadow is
closed form and does not need rasterising.

## 7. Human approval

Not required by CLAUDE.md's locked table. Required by this project's rule that a builder never
promotes its own ADR.

Recorded verbatim, 2026-09-05: the human chose *"God rays through the shafts"*, and then, when
the first result was subtle and three explanations for it had failed, *"Dig further into why
beams don't form"* — which is what produced §4. The decision to land it came after that
measurement, not before.

**Accepted 2026-09-05.** The human's words, verbatim: *"promote the four ADRs to accepted"* —
0078, 0079, 0080 and 0081 together, after the whole series was green on all six checks and
after the measurement corrections each of them carries were made. Recorded verbatim per this
project's rule, so the scope of what was accepted is not relitigable later.
