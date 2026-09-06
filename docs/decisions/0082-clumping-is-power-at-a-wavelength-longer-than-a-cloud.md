# ADR 0082 — Clumping is power at a wavelength longer than a cloud

- **Date:** 2026-09-06
- **Status:** **accepted** (2026-09-06, human — *"Bless the nine golden rows and do ADR
  0082"*). Built, green on all six checks, and the nine rows blessed in the same breath.
- **Decision touched:** none of CLAUDE.md's locked decisions. No new pass, no new resource, no
  new dependency, no new `Param`. **But this is not rendering-only** — `clouds_at` is the field
  the rain reads (ADR 0016), so this crosses into simulation-visible territory and every
  contract in §4 has to hold.
- **Amends ADR 0078 Addendum 1's thickness coupling** — `thickness = cloud_scale ·
  lerp(0.35, 0.90, type)`, argued on "a cloud is roughly as tall as it is wide". This ADR makes
  the dominant mass about three times `cloud_scale` wide, so that sentence stops being true
  unless the coupling moves with it. §5.
- **Researched by a seven-agent workflow and reviewed by a Fable critic**, whose three
  objections are all adopted; §6 records them because two of them are traps that would have
  shipped.

---

## 1. The complaint, and that it is structural

The human's words: *"The clouds do not form into a big cloud. They're too scattered.
Realistically there are big clouds clumped together."*

Measured before any research, on `croft`'s sky at the dense cores: **about twenty blobs of
broadly similar size**, the largest holding only 22–39% of all cloud. Many comparable masses,
none dominant.

**The cause is in `fbm`, and it is structural rather than a tuning miss.**
`crates/loom_field/src/lib.rs:625` only ever *raises* frequency — amplitudes 1, ½, ¼ at
frequencies 1, 2, 4. So the coverage field has **no spectral power below one `cloud_scale`**,
and 57% of its variance sits at exactly that one wavelength. A field with a single correlation
length, thresholded at any level, gives about one blob per correlation cell with an
exponential-tailed size distribution. **There is no mechanism by which one mass can be an order
of magnitude larger than its neighbours**, because nothing in the field varies at a scale
larger than a cloud.

### The whole contrast family is a no-op, and that is provable

The obvious first reach — steepen the coverage curve, raise `n` to a power, narrow `BAND` — is
worthless here, and not as a matter of taste. Thresholding is invariant under any monotone
reshape:

```
{ g(n) > u }  ==  { n > g⁻¹(u) }     for monotone g
```

so `n³` and `exp(4n)` at matched area give **bit-identical connected components**. Verified in
the research rather than asserted. Any fix must change the field's *spectrum*, not its contrast.

## 2. Decision: respend the three octaves

Replace the `fbm(u, c(0.0), v, 3)` call with three explicit terms, the largest amplitude at the
**longest** wavelength:

```rust
let n = (c(1.5) * noise(u/3) + c(1.0) * noise(u) + c(0.5) * noise(2u)) * c(1.0 / 3.0);
```

`fbm` cannot express this — it multiplies frequency by two each octave and normalises by
Σamplitude — so the terms are written out. **`fbm` itself is not touched**: the 10k-tick wind
hash is pinned on `wind()`, which uses it.

**Three `Noise` nodes, exactly as today, so twelve emitted `loom_value_noise` calls** — the
generated `clouds_at` triples the `n` subtree through `smoothstep` and carries a fourth copy in
`body[1]`. **Zero added cost**, on a cloud-map pass measured at 0.71–0.75 ms.

### Measured effect

Matched at 16.6% area — below the 45–50% percolation transition, so components mean something:

| | current | after |
|---|---|---|
| largest blob's share | 22.5% | **50.8%** |
| blobs per frame | 34 | **11** |
| half-mass blob | 1.77 cells² | **11.97 cells²** |
| mass in blobs ≥ 8 cells² | 2.0% | **60%+** |

Eleven blobs rather than one: **big masses with small satellites**, which is what the
meteorology measures — Zhu et al. 1992 find large clouds near-randomly placed with small ones
clustered around them — rather than a smooth blanket.

## 3. Why this shape and not the physically fashionable one

The research offered multiplicative cascades (Lovejoy & Schertzer) as the physically correct
model of cloud fields. **It is the same mechanism**: `log(M·D) = log M + log D`, and thresholding
is invariant under `exp`, so a product cascade *is* a sum with a low-frequency-dominant
spectrum. The sum is chosen because it keeps `n ∈ [0,1)` by the Σamplitude argument the existing
code already relies on, which is what makes §4's endpoints exact rather than approximate.

Two candidates from the research are **not** taken: adding a fourth fine octave (1.33× the
noise cost for detail `cloudDensity`'s erosion octaves already provide) and a `saturate`-clipped
variant (its marginal distribution has a floor at 0.189, which weakens the cover-0 endpoint).

## 4. The contracts this must not break

- **cover = 1.0 must give coverage 1 everywhere** and cover = 0.0 exactly 0 —
  `crates/loom_field/tests/coverage_shape.rs`, and ADR 0016's reason: a solid overcast with dry
  pinholes breaks every rain assertion written before clouds existed. **Preserved by the same
  algebra as today**: the endpoints depend only on `n ∈ [0,1)`, which the Σamplitude normaliser
  guarantees. Measured on that test's own grid: 100.0% solid at 1.0, max coverage 0.000000 at 0.0.
- **`field_agree` C6** (`crates/loom_render/src/field_agree.rs:415`) requires coverage to reach
  below 0.2 *and* above 0.8 across the sample set — a field pinned at one end would agree with a
  CPU that is equally stuck. See §6.1; this is where the change is genuinely fragile.
- **`field_agree`'s numeric agreement** at 1.5e-6 against a 1e-3 threshold. Node count is flat,
  so there is room.
- **The wind hash** `0x413c_4a61_3c8c_eb4d` — untouched, because `fbm` and `noise` are untouched.
- **No new `Param`**, so none of the three hand-written `CloudsParams` fill sites in
  `scene.slang` need editing, and the silent-uninitialised-member hazard is avoided.

## 5. What it costs downstream, named before building

- **Every cloud-bearing golden row moves.** A full re-bless, which is the human's.
- **Check the three `ablate` floors before blessing**: `squall`/`cloud_volume` ≥ 0.30,
  `squall`/`rain_curtain` ≥ 0.25, `mountain_pass`/`cloud_shadow` ≥ 0.20. Concentrated cloud
  means *less* of the frame moves when the deck is ablated, and a floor failure here would read
  as "the effect stopped drawing" when it had not.
- **`squall` and `lanternhead` must be re-authored.** Their assertions live in scene-file
  comments rather than in any gate, so nothing will catch this: `squall`'s
  `rain@-120,4,0.rate > 6.5` passes today and goes to 0.00. Its tick-600 wetness pair rides
  `cover_recent`'s forty-second history of the mass layout and is at least as exposed.
- **`scripts/green.sh:314`** — `rain@3,3,-200.rate > 20.0` on `deeper_demo` — is the only
  *automated* cover-dependent rate assertion. It must be re-run.
- **The `cloud_scale` re-authoring rule**, because this is a class and not three accidents.
  Two thirds of the field's variance moves to 3× the wavelength, so every scene that authored a
  small `cloud_scale` to make rain vary *inside* a small scene loses within-scene contrast:
  `rain_pool` at 90 m, `squall` and `lanternhead` at 260 m. **Divide `cloud_scale` by three to
  preserve the prior gap geometry** — then read §5's next bullet, because that interacts.
- **ADR 0078 Addendum 1's thickness coupling has to move.** It sets
  `thickness = cloud_scale · lerp(0.35, 0.90, type)` on the argument that a cloud is about as
  tall as it is wide. The dominant mass is now ~3× `cloud_scale` across while the slab stays
  `cloud_scale` thick — **3:1 pancakes**, undoing the isotropy that addendum measured its way
  to. And a scene that divides `cloud_scale` by three to restore its gaps shrinks its deck's
  thickness threefold at the same time. Both directions are wrong; the coupling is
  re-derived in the same commit as the field, against the *mass* wavelength rather than the
  parameter.

## 6. The Fable review's three objections, all adopted

### 6.1 Do not widen the shared sample set — it would break the wind

The synthesis proposed fixing C6's shrinking margin by widening `field_agree`'s `samples()`
from ±120 m to ±3000 m in x and z. **That would have broken the wind half of the same test.**
`samples()` is shared by all three dispatches, and the wind field's gust terms take those
coordinates into `Sin`: at ±3000 m the arguments reach ~930 radians, where Vulkan guarantees
`sin` precision only on [−π, π] and argument reduction is implementation-defined. That is
exactly the divergence `loom_field::noise`'s own header warns about. Today's ±120 m keeps the
arguments under about 37 radians.

**The clouds field contains no `Sin`.** So the fix is a *clouds-only* sample set spanning the
field's new largest scale, leaving the shared set untouched.

### 6.2 The acceptance metric is gated on a measured delta, not on a model number

The 50.8% figure comes from flat noise-space windows; the metric will run on a perspective
projection of the equirectangular cloud map with `horizonFade` applied. The transfer is
supported at one point — the model's 22.5% against the 22–39% measured from a real render — but
the acceptance must not ride on it. **Run the metric on the pre-change build in the same
harness and gate on the delta**, keep a ≥40% floor on the largest-blob share and a ≤20
blob-count backstop, and define the sky-pixel mask in the commit rather than improvising one.

Connected-component analysis is meaningless above ~40–50% cover, where a thresholded field
percolates into one component. An earlier measurement of "largest blob holds 64.5%" was exactly
that and is withdrawn.

### 6.3 The scenes are a class, not a list

Folded into §5 above.

## 7. Rejected alternatives

**Contrast, gamma, a narrower `BAND`.** §1: provably a no-op on component structure. This is
the one worth writing down, because it is what anyone reaches for first.

**Worley / cellular noise**, the obvious clustering primitive. Every operator in `Expr` is
continuous; Worley needs `floor` to find its cell, and `floor` exists in this codebase only
inside the frozen noise primitive where it has a hand-written Slang twin. Not expressible.

**Domain warping**, `fbm(p + k·fbm(p))`. Expressible and cheap, and it makes edges filamentary
without moving the size distribution — it redistributes where the boundary runs, not how much
mass sits in the largest component. Wrong tool for this complaint.

**A painted or baked weather map**, which is what most shipped engines use. Refused by
construction: the field must be a closed-form function evaluated identically on CPU and GPU, or
ADR 0016's "the clouds the eye sees and the rain the simulation feels are the same function"
stops being true.

## 8. Human approval

Not required by CLAUDE.md's locked table — no locked decision moves. Required by this project's
rule that a builder never promotes its own ADR, and wanted here because the change is visible in
every scene with weather and moves a field the simulation reads.

Recorded verbatim, 2026-09-06: the human asked for *"research actual cloud formations, and make
our clouds into those formations maybe using some mathematical equations"*, for a workflow to do
it, and for *"a fable agent review it"* — then chose **"ADR then build it"** from the options
once the review came back, then **"Ship it, and re-author the small-scale scenes too"** once the
before-and-after renders were in front of them, and finally accepted it with *"Bless the nine
golden rows and do ADR 0082"*.

**Accepted with its corrections intact**, not a tidied version: §6.1's `Sin` trap, the
withdrawn percolation measurement, and the fact that the model's predicted 50.8% did not
transfer through the render pipeline all stand as written. The change was judged on the
pictures and on `squall`'s rain scan, not on the number the research predicted.
