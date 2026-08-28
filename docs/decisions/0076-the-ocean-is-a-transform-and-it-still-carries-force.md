# ADR 0076 — The ocean is a transform, and it still carries force

- **Date:** 2026-08-28
- **Status:** **proposed** — needs human approval before this branch merges.
- **Governed by:** ADR 0045. This document argues the FFT ocean satisfies **clause 1**
  rather than needing an exception to it, and it **supersedes clause 4**, which permitted
  an FFT ocean only as a windowed force-free detail tier.
- **Overturns:** W8's refusal (`OVERNIGHT-DECISIONS.md` D16, `VFX-STATUS.md` §"W8 is
  refused and here is the evidence").
- **Decision touched:** none of `CLAUDE.md`'s locked list. Deterministic tier throughout;
  no readback; `WaterSimTier::Cinematic` untouched.
- **Spec:** `docs/design/SEA-REBUILD.md` §3, which this replaces the deep-water half of.

---

## Context

### W8 was refused, and the refusal was correct on its own terms

D16 refused the FFT ocean on evidence. The argument was about **tiling**: `ocean.loom`
visibly corrugates from 120 m up, but `ocean.loom` hand-authors seven waves, and the
sixteen-wave spectrum sea rendered at the identical camera does not tile at all. So the
artifact was one scene's art direction, not the technique, and the recorded condition for
reopening was narrow:

> **What would reopen it:** a game camera that actually looks at the sea from height,
> *and* a spectrum sea that still tiles at it. Neither is demonstrated.

**Neither has been demonstrated, and this document does not claim otherwise.** The
refusal is being overturned on a different ground, which the human supplied: not that the
sixteen-wave sea repeats, but that it is **not dense enough**. Sixteen Gerstner components
cannot produce the detail of a real sea surface at close range no matter how they are
tuned. That is a claim about richness, not about periodicity, and D16 never addressed it.

### The collision

ADR 0045 clause 2 forbids GPU readback absolutely, and says why: readback timing is not
reproducible, so `loom sim --assert` over water would go flaky, and it lags a frame, so a
boat would float visibly above the water it is drawn on. Clause 4 therefore permitted an
FFT ocean only as a **force-free** detail tier.

A Tessendorf ocean lives in a texture. Buoyancy needs the surface on the CPU, inside the
fixed step. On the face of it those cannot both be true.

---

## Decision

**The IFFT runs on the CPU inside the fixed step, and its output is uploaded to the GPU.**
The CPU tile is the authority. Buoyancy, `loom sim --assert`, `rhai` and the sim hash all
read it directly; the GPU renders what the CPU computed.

### Why this satisfies clause 1 rather than bending it

**An FFT ocean has no state.** The spectrum is fixed at load from wind, fetch and
direction. Time evolution is one multiplication by `e^{iωt}` with `ω = sqrt(g·k)` — the
dispersion relation, evaluated fresh at each tick from `t` alone. Nothing accumulates and
nothing carries over.

So the surface at tick `t` is a **pure function of (scene, t)**, which is the exact
property clause 1 demands:

> anything that produces a force on a rapier body or is readable by `loom sim --assert`
> or `rhai` is computed **on the CPU** as a deterministic function of (scene, tick)

It is not "closed-form at a point" — it is closed-form over the whole tile at once. Clause
1's other permitted shape, "state stepped inside the fixed step", is not needed and is not
claimed. **Rewinding is free**: asking for tick 900 costs the same as reaching it, which
is not true of the rain buffer or the ripple grid.

**There is no readback and there is no second implementation.** The CPU/GPU agreement
problem that `loom_water::slang()` and its numerical agreement test exist to solve does
not arise here, because there is only one computation. This is *stronger* than the
discipline it replaces, not weaker.

### What it costs, measured

Hand-rolled radix-2 Cooley–Tukey, `f32`, single-threaded, scalar, no dependency — the
shape this project would ship, because `loom_field`'s rule is that a crate must never be
able to change the sim hash. Measured on this machine (9800X3D), release, against a fixed
step of **60 Hz = 16.67 ms**:

    N        one 2D IFFT     3 cascades x 3 fields     share of a tick
    128        0.138 ms            1.238 ms                  7.4%
    256        0.795 ms            7.153 ms                 42.9%
    512        5.316 ms           47.841 ms                287%

**`N = 512` is out of reach on the CPU and therefore out of reach at all**, which settles
one thing up front: the four-million-component oceans in the reference material are
2048² GPU-only figures, and they are not available to a sea that carries force. That is
the price of the boat floating on the water you can see, and it is worth it.

**`N = 128`, three cascades, is affordable today** — 1.24 ms of a 16.67 ms tick, with a
deliberately naive implementation. That is **49,152 components against the present 16**.

**`N = 256` is the target**, via three exact optimisations, none of them speculative:

- the three fields are **real**, so two pack into one complex transform — 1.5x;
- a real field's spectrum is Hermitian, so an `N/2` transform plus fixup suffices — ~2x;
- rows are independent, so threading is **exactly** reproducible: there is no cross-thread
  reduction whose order could change, which is the only reason threading is admissible in
  simulation code at all.

Estimated ~2.4 ms for `N = 256` x 3 cascades — **196,608 components**. `N = 128` is the
proven fallback if the estimate does not survive measurement, and the fallback is a
constant, not a redesign.

Upload is three `RGBA16F` tiles per cascade per tick, ~3 MB/frame against a bus measured
at 13.5–14.0 GB/s in `CLAUDE.md`. Not a consideration.

### The cascades

Multiple independent tiles at different world scales, summed. This is what kills
periodicity, and it does so structurally rather than by tuning: the visible repeat of the
sum is the lowest common multiple of the tile sizes, which is made large by choosing them
incommensurate. It also removes the reason `WATER_DETAIL_STRENGTH`'s capillary noise
exists, because the smallest cascade covers that band with real geometry.

---

## Consequences

### What changes

- `loom_water::sample_water` stops being a sum over sixteen `LoomWave`s and becomes a
  bilinear lookup into the cascade tiles, summed in cascade order. Still a pure function;
  still summed in a fixed order, for the reason float addition is not associative.
- **Every water scene's sim hash moves.** They are re-pinned in the commit that moves
  them, deliberately, per the standing rule.
- **Every water golden reference moves.** They are the human's to bless, and they queue
  behind the tonemap re-bless that is already outstanding (`docs/rebless-the-tonemap.md`).
- `loom_water::spectrum`'s job changes from "sample the spectrum into sixteen waves" to
  "fill the initial complex amplitude field", which is the same physics with a different
  output. The Pierson–Moskowitz / fetch work and the U19.5-vs-U10 reference-height trap
  survive unchanged.
- `assets/shaders/generated/water.slang`'s wave-sum half is replaced by texture sampling.
  The generator and its agreement test remain for everything else.

### What does not change, and must not

- **The deterministic tier stays the thing physics reads.** No readback, ever.
- **Shoaling, refraction and breaking are not FFT's job.** A cascade is a periodic
  deep-water tile; it cannot feel a bed. `SEA-REBUILD.md` §3's bake, §4's breaking limiter
  and §5's curl sheet all stand exactly as designed, and the twenty-foot crashing waves
  come from them. The reference material's ocean has no shoreline in it.
- **`mu_max` stays the breaking criterion.** It is already the largest eigenvalue of the
  horizontal Jacobian's symmetric part — the same quantity the reference material calls
  "the Jacobian going negative" — so the foam source needs no new mathematics, only a
  richer field to read. ADR 0055's foam field with memory is already the accumulation
  texture with exponential decay that the technique asks for.
- **`Hs = 6.10 m` at `theedge` survives the change**, because significant wave height is a
  property of the spectrum and not of how the spectrum is evaluated.

### Risks

- **The estimate for `N = 256` is an estimate.** It is measured only at `N = 128`.
  Mitigation: the fallback is a constant.
- **A tile is periodic.** Cascades push the repeat out; they do not abolish it. The
  camera-from-height case D16 asked about is still the one to check, and now there is a
  reason to check it.
- **Determinism of the transform itself.** Radix-2 with a fixed twiddle table and a fixed
  traversal order is reproducible; a threaded version must partition by rows only. Both
  are pinned by the same 10k-tick hash discipline `loom_field` already uses.
- **This is a large, deliberate rewrite of the thing every water scene rests on**, taken
  after four tasks of a different plan had already landed. The ablation harness from S0
  exists precisely so that the result can be shown to be drawing something.

## Two things the grid makes newly available, from the reference implementation

The human's reference is `github.com/GarrettGunnell/Water` (Garrett Gunnell / Acerola,
Unity, MIT), which implements **"Dual JONSWAP, 4 layered frequency bands"**. Its README
also states plainly that **buoyancy, wakes and interactive water are not implemented** —
which is why it can live entirely on the GPU, and is precisely the constraint that forced
this ADR's CPU-authoritative design. It is a look to learn from, not an architecture to
copy.

Two of its choices are worth taking, and one of them is only possible now.

**JONSWAP's peak enhancement `γ` should be reconsidered, because the recorded reason for
refusing it has expired.** `loom_water::spectrum`'s module docs refuse it on two grounds:
that it "moves neither `Hs` nor `ω_p` — *the only two numbers sixteen equal-energy bands
can carry*", and that it "has no closed-form cumulative, which is exactly what makes
[`bands`] exact rather than approximate". **Both are scoped to the sixteen-band
representation.** A 128² × 3 grid carries 49,152 components and therefore carries spectral
*shape*; and `amplitude_field` evaluates the density per cell and never needs a cumulative
at all. So the objection is not that `γ` is wrong — it is that the old representation
could not express it.

`γ` concentrates energy near the peak, which reads as longer, more coherent, more
organised crests — visibly what the reference footage has and what a Pierson–Moskowitz
sea lacks. **It must be applied with the `Hs` target held**, not merely switched on:
measured analytically, `γ = 3.3` at `U10 = 18, F = 440 km` gives 8.22 m against PM's
6.10 m, so turning it on without renormalising would silently make the top rung a
twenty-seven-foot sea. It is also **not free of consequence for the sixteen-wave path**,
which must keep its present shape — the two are held together by
`amplitude_field_agrees_with_wave_set_fetch`, so `γ` on the grid alone would break that
test, and that test is the thing keeping the two honest. Adding `γ` therefore means
deciding what that agreement test now asserts, and is its own slice.

**"Dual" means two spectra summed — a swell and a wind sea**, typically running in
different directions. That is worth having for its own sake: it is what makes a real sea
look like weather rather than like one wind, and it is exactly the case this engine
already prepared for. `WaterSample::mu_max`'s docs record that on a crossing sea the trace
"calls 12.32% of the surface past breaking where the largest eigenvalue calls 0.00%",
which is why the breaking criterion is an eigenvalue and not a trace. That machinery has
never had a crossing sea to prove itself on. A swell from the old wind plus a wind sea
from the new one would give it one, and would give `SEA-REBUILD.md` §3.5's weather ladder
somewhere better to go than one wind speed.

**Four bands against this ADR's three cascades** is a parameter, not a decision — but note
the measured interaction: cascades stack variance, so a fourth changes the realised `Hs`
and the `Hs` test is single-cascade by construction. A cascade count is chosen with its
target re-derived, never inherited.

## Alternatives rejected

- **GPU-only, force-free detail tier** (what clause 4 already permits). Cheapest and needs
  no ADR — rejected because the boat would then float on a surface subtly different from
  the one on screen, which is the exact defect `loom_water`'s module docs open by warning
  about: *"a boat floating visibly above the water it is drawn on, with nothing to blame."*
- **GPU-only with readback for buoyancy.** Closest to the reference material and simplest
  to render — rejected because it ends `loom sim --assert` over water and makes the sim
  hash untrustworthy. That is a retreat from the project's first principle, and nothing
  here is worth it.
- **Keeping sixteen Gerstner waves and tuning harder.** Rejected on the human's evidence:
  the target look is a density sixteen components cannot reach at any amplitude.
