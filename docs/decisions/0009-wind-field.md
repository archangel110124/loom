# ADR 0009 — The wind field, and a frozen noise primitive

- **Date:** 2026-08-11
- **Status:** accepted
- **Decision touched:** new (simulation field); implements
  LOOM-IMPLEMENTATION-ORDER.md P1. Refines ADR 0006's noise rule.

## Context

Wind is upstream of water and rain in the data flow. Waves should be derived from wind speed and
fetch rather than hand-authored, and rain must lean along the wind vector and drift with it — so
building water first means authoring wave parameters and then retrofitting the derivation, which
re-tunes every authored ocean.

It is also the cheapest of the three field systems, because S2 already built the codegen and the
agreement test it rests on.

## Decision

`loom_field::wind()` is an expression tree — base direction, three sinusoidal gust terms, one
octave of fBm turbulence, a height profile — with the Slang generated from it. `Params` makes it
authorable: a `Wind` scene component fills them, `loom_field::wind::Wind` samples it, and
`loom_cli::weather::wind_of` is the single bridge between the two, because `loom_scene` depends on
nothing else in the workspace and `loom_field` has no business parsing a scene.

### Parameters, not baked constants

`Expr::Param(name)` reads an authored scalar. Without it the generated shader would carry one
scene's wind compiled into it. The parameter block's Slang **and** the reader that fills it from a
flat array are both emitted from `Field::params()`, so the array index and the struct field are one
list rather than two kept in step by hand — the layout-described-twice hazard `scene.slang`
documents at length.

An unknown parameter reads zero rather than panicking: a field that loses a term is visible
immediately, and killing the simulation over a name typo helps nobody. `Params::missing` is what
turns that into a message, checked at the boundary.

### The noise primitive is frozen ABI, and written twice on purpose

ADR 0006 said noise would live in `loom_field` as an `Expr` node rather than a crate, so no version
bump could silently invalidate a determinism hash. That still holds — and building it exposed a
detail that ADR could not have known.

**An integer hash cannot be expressed in a float expression tree.** So `loom_field::noise` is
written in Rust and in Slang, side by side in one file. That is exactly what S2 forbids for
*fields*, and it is safe here for a reason fields do not have: the agreement test compares the two
halves for **exact equality**, not within an epsilon. A divergence is a hard failure on the first
sample rather than a slow drift.

**The lattice is integer, and that is the whole design.** The standard shader one-liner —
`frac(sin(dot(p, k)) * 43758.5453)` — is unusable: `sin` of a large argument depends on the
argument reduction, which differs between libm and a GPU, and `frac` of a large product amplifies a
last-bit difference into a completely different number. `u32` multiply, xor and shift are exact
everywhere, and `(h >> 8) as f32 * 2^-24` is exact too. So every lattice corner is bit-identical,
and only the interpolation between them is floating point — where a last-bit difference stays a
last-bit difference.

Measured: **noise agrees bit-exactly across 512 samples**, and the full wind field's worst
difference is **2.8e-5** against a 1e-3 threshold — tighter than the 4.5e-5 the pure-sinusoid field
managed, because the exact lattice contributes no error at all. Mutation-checked: changing one
constant in the Slang half by a single bit fails on sample 0.

### Two numbers the tests chose

Both were wrong first, and neither was caught by reading the code.

**The height profile.** The first version was two linear segments and put roughly *half* the
correct wind at ankle height, which is precisely where grass lives. A 1/7 power law referenced to
10 m gives 0.57 of free-stream at 0.2 m and 0.78 at 1.8 m; the profile is now `h / (h + 2)` scaled
by `ground_drag`, fitted to minimise the worst error against those, and the test checks all three
heights against the **real** numbers rather than against whatever the code happens to produce.

**The gust amplitudes.** They summed to 1.0, which swings the wind from dead calm to double its
mean — a fan being switched on and off. Meteorology quotes a gust factor of 1.3–1.5 over open
ground, so they now sum to 0.33. **A CLI assertion found this, not a unit test**: `wind@0,2,0.speed
>= 3` reported 1.87 where the mean should have been about 4. There is now a unit test measuring the
gust factor over 20,000 ticks.

### Determinism is gated in both profiles

The exit criterion asks for identical results in debug and release across 10k ticks. A pinned
FNV-1a hash over 30,000 samples covers "the field has not changed" — but `cargo test` builds debug,
so on its own it does not cover "the profiles agree", and a field is arithmetic, which is exactly
where inlining and vectorisation differ. `cargo xtask validate` now runs the field tests in release
too.

**Changing a field means re-pinning that hash in the same commit.** It is the only thing between a
retuned field and a silently different simulation.

### Direction is a vector, not an angle

The field takes `dir_x`/`dir_z`, and the angle is converted once at the authoring boundary. An
angle wraps at ±180°, and a wrap is a snap — which is the artifact P1's exit criterion asks to
bound. The slew rate is asserted numerically at under 3°/tick over 6,000 ticks, alongside a
magnitude-continuity check, because a snap is invisible in a still and barely visible in motion.

### Sheltering is a plain multiplier

`Wind::sheltered` scales the open-ground field by S3's exposure, clamped to `[0, 1]` so a caller's
arithmetic error cannot amplify the wind past its authored speed. It does **not** redirect the
flow: a sheltered point gets less wind, not differently-directed wind. Modelling flow *around* an
obstacle needs a solver and is a different project.

## Deliberately not built

- **Curl noise.** The implementation order defers it until particle advection shows visible sinks.
  One octave of value noise is visually sufficient for a wind field.
- **Terrain-driven ridge acceleration.** Cosmetic, and deferred by the same document.
- **A GPU wind pass.** Nothing samples the field on the GPU yet; `field_agree` reads results back,
  which is a harness, not the beginnings of one. P2 grass will want a frame-graph pass writing a
  texture.

## Consequences

- P3 water derives wave parameters from `speed`; P4 rain leans along the field. Neither authors its
  own wind.
- The `Wind` component's defaults are Beaufort 4 from the west, so a scene that says nothing about
  weather still looks like weather rather than like a still.
- `loom sim --assert "wind@x,y,z.speed >= v"` makes the field checkable from the CLI, which is how
  the gust-factor error surfaced.

## Human approval

Not required — this adds a simulation field and a scene component rather than changing a locked
decision.
