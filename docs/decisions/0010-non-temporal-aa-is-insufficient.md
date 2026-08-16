# ADR 0010 — The non-temporal AA toolkit is insufficient for grass

- **Date:** 2026-08-12
- **Status:** **accepted** (2026-08-12, human). A CMAA2-class full-screen pass is authorised.
  The build brief's "no post-process stack before Phase 8" boundary is hereby **moved, not
  eroded** — this pass is its first and, until Phase 8, only inhabitant.

  **The risk this decision carries, stated at the point of acceptance:** CMAA2 is a *spatial*
  filter and the artifact it is being bought to fix is *temporal*. The expectation is that
  smoothing a sub-pixel edge reduces how violently it varies frame to frame, which is an
  inference and not a measurement. `cargo xtask shimmer` is now trustworthy enough to settle
  it, and the acceptance test is a measured reduction against the baselines below — not the
  pass merely existing and looking smoother in a still.
- **Decision touched:** potentially adds a post-process pass, which the build brief's locked
  decisions do not currently include. Implements the escape hatch named in
  `LOOM-IMPLEMENTATION-ORDER.md` Phase 2: *"If that combination proves insufficient, the escape hatch
  is a single non-temporal full-screen AA pass (SMAA or CMAA2 class). That is technically a
  post-process, so adding it is a scope decision to make deliberately rather than by drift — record
  it as an ADR if you take it."*

## Context

Grass was pulled forward to Phase 2 for one reason: it is **the forcing function on the no-TAA
decision**. Sub-pixel blades are the worst case of a problem that also affects water specular
highlights and rain streaks, and nearly every shipped grass system leans on TAA, DLSS or
checkerboard reconstruction to hide it. This engine has none of those. The phase exists to find out
in month two whether thin geometry can be made stable without temporal accumulation.

It cannot, with the toolkit the phase specified.

### The measurement, and why the earlier one was worthless

`cargo xtask shimmer` reports temporal flicker, `|b - (a+c)/2|` over three frames, with the camera
held at the scene's authored eye and the simulation advancing between frames. Three scenes with no
animated geometry score **exactly 0.000**, which is the control the metric needs to be trustworthy.

Everything measured before 2026-08-12 01:16 is void. Until then the tool framed whole-scene bounds
and ignored authored cameras, so for `meadow` it photographed a bare green slab from 38 m with the
density falloff having deleted every blade — and the wind clock never advanced, so nothing in frame
was moving anyway. The AA table was tuned against an empty, frozen field, and it duly reported that
whatever removed grass fastest was the best anti-aliasing. See `0938053`.

### What the toolkit actually delivers

All at 640x400, `meadow`, against a baseline of **2.712** at 4x MSAA:

| tool | result |
| --- | --- |
| **MSAA** | **works.** 1x 3.888 → 2x 3.000 → 4x 2.712 → 8x 2.502. Monotonic, ~36% total. |
| Density falloff | **nothing.** 2.712 on, 2.715 off. |
| Minimum screen-space width clamp | **nothing usable.** At the specified ~1 px floor: −0.9% on `meadow`, +1.2% on `grass_slope`. |
| Alpha-to-coverage | **nothing.** 0.002, under the metric's resolution. |
| True geometry over alpha cards | already chosen, and correct — it removes alpha-test aliasing entirely. |

That is the complete list from the phase's own specification. **Only MSAA helps, and 4x MSAA leaves
the field at 2.712 against a 0.000 control.**

Two structural findings explain why the rest fail, and both are worth keeping:

- **The width clamp's selectivity buys nothing.** A 2 px floor scores 2.631; simply authoring
  `width = 0.035` scores 2.635 — the same number, reached by widening everything 75% rather than
  making the far field up to 3x too wide. And the clamp's benefit inverts between scenes: it helps a
  camera standing *in* the field and hurts one looking *down a slope*, where nearly every blade is
  sub-pixel and it therefore widens the whole field.
- **Widening works, but only as brute force.** Flicker falls monotonically with blade width — 2.712,
  2.635, 2.338, 1.973 at 0.020/0.035/0.060/0.100 m. A 27% win costs blades that read as leaves.

## The decision being asked for

Add **one** non-temporal full-screen AA pass, CMAA2 or SMAA 1x class, run after the forward pass and
before readback.

This is a real scope change. It introduces the first post-process pass in the renderer, and the
build brief defers the post stack to Phase 8. It is not a slippery slope by itself, but it is the
first step onto one, which is why it is here rather than in a commit.

### Why not the alternatives

- **Temporal accumulation (TAA/TSR).** Rejected by the locked decisions, and rejecting it is
  load-bearing: determinism, agent-verifiable renders and single-frame golden images all assume a
  frame is a pure function of its state. TAA would make every golden image a function of the
  preceding frames.
- **Supersample and downsample.** Honest and trivial, and it is what 8x MSAA already approximates
  for 2.502. Buying another factor costs quadratically and the engine is not fill-bound today — the
  whole forward pass is 0.05–0.11 ms — but it does not scale to a real frame budget.
- **Accept the twinkle.** Defensible *if* the target is a still-image and CLI-verified workflow. It
  is not defensible for `loom run --edit`, which is a human watching a live window, and the phase
  was explicitly set up to reject this answer by measurement rather than taste.
- **Do nothing and revisit at Phase 8.** This is the option I would push back on. Water and rain are
  next and have the same failure mode — specular highlights and thin streaks — so building both on
  an unanswered AA question repeats the mistake this phase exists to prevent.

## Consequences if accepted

- One compute or fragment pass, owned by the render graph like everything else.
- Golden images all move once, deliberately, in the commit that lands it.
- `cargo xtask shimmer` becomes the acceptance test, and it now has a control that makes it
  trustworthy.
- Phase 8's post stack acquires a first inhabitant, and the boundary of "no post-process" is
  formally moved rather than eroded.

## Consequences if rejected

- Grass ships at 2.712 and the phase's exit criterion 2 is recorded as **not met**, not waived.
- Water and rain inherit an open question, and their own AA work should be budgeted accordingly.
- `MSAA_SAMPLES` should probably go to 8x (2.502 for double the bandwidth) since it is then the only
  tool available and the frame has room.

## What the pass actually delivered

Built 2026-08-12: `assets/shaders/cmaa2.slang` plus `crates/loom_render/src/cmaa2.rs`, wired into
both the offscreen renderer and the windowed viewer, behind **`LOOM_CMAA2=1`** — off by default.
It implements CMAA2's local-contrast-adaptive **edge detection** and its **simple-shape (L-corner)
blend**, with Intel's own constants read from the published source. It does **not** implement the
Z-shape / long-edge path, which is the half that puts a gradient along a long near-horizontal edge,
because that needs the edge bitmask kept in a texture so it can be walked. The shader's header says
so at length; the name is only honest with that sentence attached.

`cargo xtask shimmer`, 640x400, camera at the authored eye, sim advancing:

| | `meadow` | `grass_slope` |
| --- | --- | --- |
| 4x MSAA (the shipped setting) | 3.059 | 1.755 |
| 4x MSAA + CMAA2 | **2.791** (−8.8%) | **1.605** (−8.5%) |
| 1x MSAA | 4.375 | 2.630 |
| 1x MSAA + CMAA2 | 3.652 (−16.5%) | 2.288 (−13.0%) |

The three static control scenes stay at exactly 0.000 with the pass on, so it invents no flicker of
its own.

**Read the table in this order.** The reduction is real, reproducible and in the right direction on
both scenes — but it is 9%, against MSAA's 36%, and it is *smaller* the more MSAA is already doing.
CMAA2 alone at one sample (3.652) is well behind 4x MSAA alone (3.059), so **it does not make MSAA
redundant** and the two are complementary with sharply diminishing returns. The prediction this ADR
was accepted on — that a spatial filter would steady a temporal artifact — is confirmed in sign and
disappointing in size: it does not get `meadow` near its 0.000 control, and on that basis the phase's
exit criterion 2 is still not met.

It costs **0.042 ms at 1920x1080** on the 4090 (`LOOM_GPU_TIMING=1`), stable across scenes because it
is a fixed per-pixel cost — about 20% on top of a `meadow` forward pass and 5% of the render graph.
It does not cost visible sharpness: at 1080p it changes 0.18% of `materials`, a scene that is nothing
but texture detail, against 4.7% of `meadow`, which is nothing but silhouettes. That ratio is the
conservatism working.

**Left gated off, deliberately.** Turning it on moves seven of the nine golden references, and a 9%
reduction is a judgement call about whether that is worth a permanent post-process pass and a
re-blessing — which is a human's call, not the implementation's.

### The Z-shape half, added 2026-08-12, and what it was worth

The omission above is now filled: `DetectZsHorizontal`, `FindZLineLengths` and `BlendZs` are
implemented, with Intel's `c_maxLineLength` = 86, `c_symmetryCorrectionOffset` = 0.22 and
`c_dampeningEffect` = 0.15, and with `DeferredColorApply2x2CS`'s 0.8 / 1.8 contribution weighting.
Doing so required splitting the pass in two — `cmaa2_edges.slang` writes a per-pixel edge bitmask
to an `R8_UINT` image and `cmaa2.slang` walks it — because tracing a line needs the mask stored.
One structural difference remains and is documented in the shader: **Intel's Z blend scatters and
this one gathers**, because scattering needs a storage image and no sRGB format supports `STORAGE`.

`cargo xtask shimmer`, same conditions as the table above:

| | `meadow` | `grass_slope` |
| --- | --- | --- |
| 4x MSAA | 3.059 | 1.755 |
| 4x + simple shapes | 2.791 | 1.605 |
| **4x + Z-shapes** | **2.788** | **1.605** |
| 8x MSAA | 2.824 | 1.310 |
| 8x + simple shapes | 2.601 | 1.244 |
| **8x + Z-shapes** | **2.598** | **1.244** |

**On grass it is worth nothing.** 0.003 on `meadow`, 0.000 on `grass_slope`, at both sample counts.
That was the prediction and it is now the measurement: Z-shape handling smooths long, shallow-angled
edges, and a field of thin near-vertical blades has none. The residual is sub-pixel *coverage*
flicker — information absent from the frame, not information in the wrong shape — and this ADR's
conclusion is unchanged and if anything firmer.

It is not inert, and the distinction matters. At 1920x1080 it changes 0.037% of `primitives` and
0.044% of `materials`, and on the ground plane's long shallow silhouette in `primitives` the
staircase the simple-shape blend leaves — steps still individually countable, merely softened at
their corners — becomes a continuous ramp. Texture is untouched: the tile grid in `materials` is
pixel-identical with the path on and off.

The refactor is verifiable rather than asserted: with the Z loop disabled, the two-pass build
reproduces the one-pass build's numbers **exactly** on all nine scenes (`meadow` 2.791,
`grass_slope` 1.605), so the whole delta above is the new path and none of it is the restructuring.

Cost at 1920x1080, `LOOM_GPU_TIMING=1`, against 0.042 ms for the one-pass version:

| scene | edge pass | shape pass | total |
| --- | --- | --- | --- |
| `meadow` | 0.022 | 0.099 | **0.121** |
| `primitives` | 0.023 | 0.045 | 0.068 |
| `materials` | 0.022 | 0.035 | 0.057 |

Roughly 3x, and still under 15% of a `meadow` render graph. A cap of 32 instead of 86 saves 0.009 ms
on `meadow` and is bit-identical on every gate scene at this resolution; 86 is kept because it is
Intel's and because a flatter horizon at a higher resolution would reach it.

## Status of the evidence

Every number above is reproducible with `cargo xtask shimmer` at `8c2bcb6`. The gated-off clamp
implementation and its full table are in `assets/shaders/scene.slang` so the experiment is not run a
fourth time.

### The baseline moved after this ADR was written, and the reason is a flaw in the metric

At `1062550` grass takes its colour from the authored `Material` instead of a hardcoded green, and
`meadow`'s baseline moved **2.712 → 3.059** with no change to geometry or to any AA setting. It was
attributed rather than assumed: with the new hue variation disabled it is 3.026, and with hue
variation on but the old darker albedo restored it is 2.753.

So roughly 1% is the hue variation and **the rest is simply that the field is now painted brighter.**
`cargo xtask shimmer` measures absolute pixel differences, so the same geometry in a lighter colour
scores higher without being one bit less stable.

**The metric is therefore not invariant to brightness, and comparing any two AA numbers across a
colour change is invalid.** Current baselines are `meadow` **3.059** and `grass_slope` **1.755**.
Every number in the table above was taken on the darker field and they remain valid *relative to each
other*, which is what the argument rests on.

I recommended normalising flicker by mean brightness as the fix, and called it load-bearing. **Then I
built it and measured it, and it does not work.** Dividing by the frame's mean channel value and
re-rendering `meadow` with the grass albedo darkened and brightened:

    dark   [0.15, 0.22, 0.07]    11.627
    normal [0.29, 0.44, 0.14]    17.331
    bright [0.55, 0.80, 0.28]    26.723

Still scaling with brightness, and more steeply than the raw metric did. The reason is obvious in
hindsight: the numerator comes almost entirely from grass, while the denominator is the whole frame
— sky and soil included — and those do not scale when only the grass albedo moves. It is reverted.

**And the premise deserves more doubt than I gave it.** A brighter blade against the same dark soil
is genuinely *higher contrast*, and contrast is what makes flicker visible. So a metric in absolute
units reporting more flicker for brighter grass may be perceptually right, and normalising it away
would remove a real effect rather than an artifact. Per-pixel local normalisation would be invariant
in the way I wanted, but it would also divide out exactly that contrast.

What stands is the narrower, certain claim: **do not compare two AA numbers across a change in
colour or lighting.** Whether the right long-term answer is a contrast-aware perceptual metric or
simply a discipline of holding colour fixed is unresolved, and I no longer think it is obvious.
