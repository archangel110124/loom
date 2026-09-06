# ADR 0081 — The deck shades the sea, and the sun was already dimmed by the wrong number

- **Date:** 2026-09-05
- **Status:** **accepted** (2026-09-05, human — "promote the four ADRs to accepted"). Built and green on all six
  checks at the time of acceptance.
- **Decision touched:** none of CLAUDE.md's locked decisions. No new pass, no new resource, no
  new dependency, no GPU state, no post-process. **Governed by ADR 0045:** rendering-only.
  ADR 0015 verified that nothing in `loom_script`, `loom_ecs` or `play.rs` reads sun strength,
  so this cannot reach the determinism hash — that verification is re-checked below rather
  than inherited.
- **Amends ADR 0015's light coupling**, which is the substance of this ADR. 0015 wrote *"Cloud
  cover should drive the light"* and implemented it as one global scalar. That was right when
  the deck was a flat texture with no position. It is now a volume with a silhouette, and the
  global scalar is the reason a squall can cross the bay without the bay noticing.
- **Fires ADR 0078 §6's ground-shadow gap** — *"Cloud shadows on the world, and god rays. The
  march makes both possible… neither is built here."* God rays became ADR 0080; this is the
  other half.

---

## 1. Context: the sun is already dimmed, and by a number that has no position

`sunStrength` and `sunColor` in `scene.slang` both read `cloudCover()`, which is
`push.environment[0].cloud.x` — **the scene's single authored cover, identical at every point
in the world.** So under a broken deck at cover 0.45, every surface in the scene is lit at the
same 55%-ish of full sun whether it stands in a gap or under the thickest mass in the sky.

The comment above them is explicit that this is the one coupling every lighting path shares:

> **Every lighting path in this file goes through these three**, which is why the coupling
> lives here and not at each site: specular highlights, wrapped foliage lighting, the sky
> gradient and the particle terms all follow without knowing clouds exist.

That structure is exactly right and this ADR does not disturb it. **What changes is the
number those three read.**

**This also means a naive cloud shadow would double-count.** Multiplying a new per-pixel
shadow into `sunVisibility` would dim by cover twice — once globally in `sunStrength`, once
locally — and every scene with cloud would go dark. The available move is not to add a term;
it is to give the existing term a position.

## 2. Decision: the same coupling, sampled where the surface is

```
sunStrengthAt(p) = sun.w * lerp(1.0, 0.12, coverAboveGround(p))
```

where `coverAboveGround(p)` projects `p` **along the sun ray** onto the cloud base and samples
`clouds_at` there:

```
q = p + sunDir * ((base - p.y) / sunDir.y)
```

Not straight up. A shadow falls where the sun is blocked, and at `lanternhead`'s 10-degree sun
the cloud blocking a point is four kilometres away — the same geometry ADR 0080 §2 works out
for the shafts. Sampling overhead would put every shadow in the wrong place, and at a low sun,
kilometres wrong.

**The mean is preserved by construction.** `clouds_at`'s coverage averages to the authored
`cloud.x` across the scene, so a frame's overall exposure is what it was; what changes is that
the light now *varies* — full sun in the gaps, 12% under the masses, and the boundary sweeps
across the world as the deck drifts.

`sunStrength()` without a position stays exactly as it is, for the sky gradient and the
particle terms, which have no surface to stand on.

## 3. What it buys, and why it is the largest remaining item

A squall crossing the bay currently darkens the *sky* and leaves the *water* uniformly lit.
Everything ADRs 0078 to 0080 built happens above the horizon. This is the first term that puts
the weather on the world:

- Cloud shadows sweeping across the sea, moving with the deck because they read the same field.
- A scene lit in patches rather than flatly — the single strongest cue that a sky is real.
- `lanternhead`'s quay and `croft`'s hillside falling in and out of sun as masses pass.

## 4. Cost, and the reason it is not free

**One `clouds_at` per shaded pixel** — twelve noise evaluations, because the expression tree
has no common-subexpression elimination across octaves. That is the same figure ADR 0080 §4
found dominating the light march, and unlike the shafts this one is paid on *every opaque
pixel in the frame*, not on the sky.

**Measured, and the first measurement was wrong.**

Two min-of-3 samples first read `mountain_pass`'s forward pass at 1.65 ms ablated against
4.746 ms with shadows, and this ADR carried a warning built on that ~~3x~~. **It is
withdrawn.** Re-measured interleaved, five reps each, 1920x1080, forward pass in ms:

| rep | ablated | shadows |
|---|---|---|
| 1 | 2.898 | 1.999 |
| 2 | 2.569 | 2.638 |
| 3 | 2.130 | 1.666 |
| 4 | 2.131 | 1.848 |
| 5 | 2.553 | 1.677 |

**The shadowed build is consistently the faster one**, which is impossible for added work — so
the whole difference is noise, and the noise band on this box spans 1.67 to 2.90 ms. Four
scenes at min-of-3 agree: `meadow` 0.496 -> 0.494, `mountain_pass` 1.684 -> 1.682, `stoneyard`
0.759 -> 0.760, `cave` 0.420 -> 0.419. **1.00x on every one.**

The cost is real arithmetic and it is below what this box can measure. That is consistent
rather than surprising: one `clouds_at` is twelve noise evaluations against a forward pass
already firing sixteen AO rays, which ADR 0019 measured as two thirds of the cost of ray
tracing here.

**The lesson is the interleaving.** Two consecutive min-of-3 batches, taken minutes apart, are
two draws from a drifting distribution and not a comparison. Alternating the two builds within
one run is what made the answer obvious, and it is the protocol any figure in this series
should have been taken with.

**The escape hatch, if a slower machine ever needs one:** the sample is a 2D lookup at a
position that varies smoothly across a surface, so it is a candidate for evaluating per-vertex
and interpolating. Not built, and on this hardware there is nothing to recover.

`cloud_shadow` joins `ABLATIONS` as its own row: it fails apart from `cloud_volume`, and a
scene lit flatly under a moving deck is precisely the sort of absence a reference image
records and passes for ever.

## 5. What this does not settle

- ~~**No penumbra.**~~ **Built, measured, and reverted — see Addendum 1.** A physically correct
  penumbra is smaller than the softness the coverage curve already has, and changes 61 of 62
  golden rows by nothing at all.
- **The shadow does not darken the rain.** ADR 0080 lights the shafts from the deck; this
  lights the ground from the deck; a shaft standing in another shower's shadow is not modelled.
- **No shadow from the shower itself**, only from the cloud that made it. A heavy shaft really
  does darken the water under it.
- **`sunColor` keeps the global number.** Its cover term shifts the sun's hue toward overcast
  grey, which is an atmospheric property rather than a local occlusion, and making it local
  would tint patches of sea differently for no physical reason.

## 6. Rejected alternatives

**A shadow map from the sun.** A second render of the deck, a new target, a new pass, and a
projection to keep in step. The slab is analytic — its occlusion is a closed-form lookup, as
ADR 0080 §6 already argued for the shafts.

**Multiplying a new term into `sunVisibility`.** §1: it double-counts against the dimming
already in `sunStrength`, and every clouded scene would go dark. The bug would look like a
tuning problem and be a structural one.

**Sampling cover straight up.** Cheaper by one division and wrong by kilometres at any low sun
— which is both scenes this repository lights with one.

**Leaving it global, as ADR 0015 had it.** Defensible while the deck was a texture. With a
volume that has a silhouette, a shadow is the thing the silhouette is *for*.

## 7. Human approval

Not required by CLAUDE.md's locked table. Required by this project's rule that a builder never
promotes its own ADR — and doubly wanted here, because this changes how **every lit surface in
every scene with cloud** is shaded, which is the widest blast radius of anything in the 0078
series. The cost objection this section originally carried is withdrawn — see §4.

Recorded verbatim, 2026-09-05: chosen from the options after the wind shear landed, as *"Bless,
then cloud shadows on the world"*.

**Accepted 2026-09-05.** The human's words, verbatim: *"promote the four ADRs to accepted"* —
0078, 0079, 0080 and 0081 together, after the whole series was green on all six checks and
after the measurement corrections each of them carries were made. Recorded verbatim per this
project's rule, so the scope of what was accepted is not relitigable later.

---

## Addendum 1 — the penumbra is real, correct, and below this engine's own softness floor

**Date:** 2026-09-05, firing §5's first trigger.

The sun is not a point: its angular radius is 0.265 degrees, so a shadow edge blurs by the
distance the light has travelled since the occluder. That distance is long here — the cloud
shadowing a point is kilometres away at any low sun — so the penumbra was expected to matter.

**The arithmetic says otherwise, and the arithmetic is checkable before writing code:**

| scene | cloud to ground | penumbra radius | as a fraction of one cloud mass |
|---|---|---|---|
| `squall` | 2722 m | 12.6 m | 5% |
| `lanternhead` | 4296 m | 19.9 m | 8% |
| `mountain_pass` | 1900 m | 8.8 m | 1% |

One to eight percent of a mass — and `clouds_at`'s coverage curve already carries a fade band
of 0.18 of the noise range, which spans considerably more ground than that. **The penumbra was
predicted to be subsumed by softness the deck already has.**

### Built as a probe, measured across every row, reverted

Five taps across the sun's angular radius, then every one of the 62 `GOLDEN` rows rendered **at
its own gate arguments** and compared:

**One row moves.** `squall`, 4.42% of pixels at worst channel 21. The other sixty-one are
within tolerance, most at worst channel 0.

So a correct penumbra costs five cover taps where there was one, and buys a 4% change on a
single scene. It is reverted, and the revert is verified byte-identical rather than assumed.

**§5's trigger is answered rather than left open.** It read *"the boundary reads as a hard line
rather than an edge"* — it does not, because the coverage curve's own fade is already wider
than the sun's penumbra. If a future change sharpens that curve, this becomes worth revisiting;
as the deck stands, there is nothing to soften.

**Driving the sweep from the `GOLDEN` table rather than typing arguments by hand is what made
it trustworthy.** Three separate measurements in this ADR series were taken at ticks the gate
never renders — `--sim 300` against a row gated at 2400, 900 against one gated at 120 — each
time producing a large and meaningless number. Reading the arguments from the table removes
that error class entirely, and should be how any sweep here is run.
