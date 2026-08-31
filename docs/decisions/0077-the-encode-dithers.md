# ADR 0077 — The encode dithers

- **Status:** ACCEPTED, 2026-08-30.
- **Decision touched:** reverses exactly one entry in ADR 0018's rejected list
  (dither) and nothing else. No locked decision in `CLAUDE.md` moves, and
  ADR 0010's post-process boundary is not approached — this adds no pass.
- **Applies to:** `assets/shaders/tonemap.slang`, and every golden reference.

## The decision, and the cost beside it

An ordered 8x8 Bayer offset of half an output code, added immediately before
the tonemap returns, as a function of the integer pixel coordinate alone.

**Cost: a handful of ALU in a full-screen fragment pass that already runs.**
Below the resolution of `LOOM_GPU_TIMING`. If it ever measures above 0.02 ms
something is wrong with the implementation rather than with the idea. It is the
cheapest item on the whole lighting list and it is first for that reason.

**Pixel-coordinate-only is load-bearing.** It is the same rule `rayDither`
(`scene.slang:1289`) already follows, and it is what keeps a frame a pure
function of its tick — no frame counter, no wall clock, no animation. So
`cargo xtask repeat` stays byte-identical across three fresh processes, and
ADR 0045's determinism line is untouched.

## Why

**Below `KNEE = 0.76` the operator is the identity** (ADR 0018), so in a dark
scene the whole visible range is decided by the 8-bit quantiser and nothing
else. `cave`'s entire span is sRGB 70 to 161 (ADR 0068). A smooth exponential
fog ramp across that span crosses roughly ninety codes over a screen, which is
textbook banding — and DEEPER is a dark, fog-limited game in which most of the
frame *is* that ramp.

This is the one item on the lighting list whose absence is visible in the game
being built rather than in a test scene.

## Half a code, in the right space

The attachment is `R8G8B8A8_SRGB` (`renderer.rs:43`), so the hardware encodes
and quantises after this shader returns. The shader writes **linear light**.

So the offset must be half a code *of the encoded value*, and half a code of
linear is not that: sRGB's slope varies by two orders of magnitude across the
range, so a fixed linear dither is invisible in the highlights and enormous in
the shadows — which is exactly backwards, because the shadows are where the
banding is.

sRGB's power segment is `L = ((s + 0.055)/1.055)^2.4`, so

    dL/ds = (2.4 / 1.055) * L^(7/12)

One `pow` per channel and no encode/decode pair, which `tonemap.rs:37-38`
records as existing nowhere in `assets/shaders/`. Below the transfer's linear
knee (`L < 0.0031308`) the slope is `1/12.92`; that region is codes 0 and 1,
where a dither has nothing to dither between, and the branch is taken there for
correctness rather than for appearance.

## What this costs, and it is a real property

**"Which golden references move is computable before the change is written"** is
named in both ADR 0018 and ADR 0068, and it is genuinely lost. Identity below
the knee was bit-exact; six of the twenty-five references contained no pixel
above the knee at all and were provably untouched by a shoulder change. After
this, every reference moves by up to one code everywhere.

That property was worth having and this spends it. The trade is that the
quantiser is currently the *dominant* source of error in exactly the scenes this
game is made of, and a property that makes a re-bless predictable is worth less
than a frame without visible bands in it.

## Overrule this in one line if

You would rather keep identity-below-the-knee bit-exact — or if the banding is
not visible to you on your own display, in which case this is spending a real
property to fix something nobody can see. Reverting is deleting one function
call.

## Rejected alternatives

- **Blue-noise dither from a texture.** Better than Bayer per unit of visual
  noise, and it needs a texture, a descriptor and an asset-routing answer this
  engine does not have yet (`Scene::asset_path` has no search path, ADR 0024).
  Bayer is analytic, costs no bandwidth, and the difference at half a code is
  not the thing standing between this game and looking good.
- **Animated dither.** Strictly better statistically and it makes the frame a
  function of something other than its tick, which `cargo xtask repeat` exists
  to forbid. Refused on ADR 0045's line, not on taste.
- **A wider output format.** `R10G10B10A2` would move the problem rather than
  solve it and costs bandwidth on every scene, including the ones with no
  banding.
