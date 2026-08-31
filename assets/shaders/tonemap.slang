// The one place the frame's range collapses.
//
// Everything upstream of this pass is linear `R16G16B16A16_SFLOAT` with no
// ceiling; the attachment here is `_SRGB`, so the hardware does the encode
// exactly as it always has. What moved is WHERE — from the first fragment
// write to the last.
//
// **Fragment rather than compute, and that is a property of this renderer
// rather than a preference:** no sRGB format supports STORAGE, which
// `cmaa2.slang` already had to work around.
//
// `Texture2D` and `Load`, no sampler: this is a one-to-one copy, there is
// nothing to filter, and a sampler would be an object to create, name and
// destroy for no pixel.

[[vk::binding(0, 0)]] Texture2D<float4> scene;

struct TonemapPush {
    /// Where in the destination this copy lands, in pixels.
    ///
    /// **`SV_Position` is in framebuffer coordinates, not viewport-relative
    /// ones**, so an offset destination viewport reports coordinates the
    /// source image does not have: the scene is rendered at the *origin* of a
    /// window-sized image and only the tonemap moves it. Subtracting this is
    /// what turns a destination pixel back into the source pixel it came from.
    ///
    /// `int2` first and `float` second is 12 bytes under either packing rule,
    /// so the Rust side and the shader cannot disagree about the offset of
    /// `exposure`.
    int2 origin;
    float exposure;

    /// The grade's contrast, as a **multiplier** on the operator's own.
    ///
    /// Two powers about one pivot compose exactly —
    /// `P*(P*(x/P)^a/P)^b == P*(x/P)^(ab)` — so there is one `pow` in this
    /// shader rather than two, and `1.0` is bit-exact rather than nearly so.
    float contrast;

    /// `rgb` is the ASC CDL slope, `w` is the saturation.
    ///
    /// They ride together because a `float4` at offset 16 is 16-aligned under
    /// every packing rule and a bare `float3` is not — the same lane trick
    /// `light_pad` already uses in the environment buffer.
    float4 gain;
};                     // 32 bytes of a guaranteed 128
[[vk::push_constant]] TonemapPush push;

/// Rec.709 luma. The saturation axis, and nothing else uses it.
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

/// Where the shoulder departs from identity. **Unchanged — ADR 0018.**
static const float KNEE = 0.76;

/// Where the contrast pivots, in scene-linear light — ADR 0068.
///
/// **0.30, and the 0.18 the pooled reference median argues for was measured
/// and rejected.** Pooled over every pixel of all 54 golden references the
/// median linear value is 0.19, which is a good number for a library nobody is
/// looking at; `deeper_demo` sits at 0.36, and at a pivot of 0.18 the game's
/// own opening frame gets BRIGHTER — immediately after the human said "not too
/// bright". The alternative was to cancel that lift inside `deeper_demo`'s own
/// grade gain, which is two terms fighting in a file, invisible to whoever
/// authors the third scene.
///
/// **This is the brightness knob if an opening frame reads hot.** Raise it; do
/// not patch it downstream.
static const float PIVOT = 0.30;

/// The slope, and the whole of the base-look change.
///
/// **The complaint was "no blacks", and the answer has to be exposure-
/// RELATIVE.** Khronos PBR Neutral's toe subtracts a fixed 0.04 of linear
/// light, which is a fifth of a bright frame's shadows and nearly all of a
/// dark one's — measured, it takes `campfire` to 56% of the frame below code
/// 16 off a cliff between 0.75 and 0.80 of full strength, and it *raises*
/// shadow saturation, because a constant subtracted from all three channels is
/// not a magnitude scale. A power about a pivot scales with how the scene was
/// lit and cannot do either.
///
/// Measured over the golden library at this value: shadows fall and highlights
/// do not move. `lanternhead`'s 1st percentile 20.1 -> 7.5, `ocean`'s 37.1 ->
/// 22.2, frame saturation up about a quarter, 99th percentiles within 11
/// codes. **1.45 was measured too and is where it starts to harden** —
/// `campfire`'s 1st percentile reaches 0 and the sky posterises.
static const float CONTRAST = 1.30;

/// **A pure shoulder: identity below the knee, hue and saturation exactly
/// preserved above it.**
///
/// Measured against the published operators on `fireRamp`'s amber rung: ACES's
/// full fit and Khronos PBR Neutral both desaturate a fire FASTER than
/// clipping does — 0.213 against 0.400 at sixteen times overdrive — because
/// each lerps toward white once the peak passes its knee. That is exactly the
/// failure `fireRamp`'s capped top rung exists to prevent, so adopting either
/// would have spent an ADR to reintroduce the bug the art was shaped around.
/// This holds hue to 0.3 degrees across six stops.
///
/// **Identity below the knee is load-bearing, not incidental.** It is what
/// makes "which golden references move" computable from the existing PNGs
/// before the change is written: six of the twenty-five contain no pixel above
/// the knee at all and must come out bit-identical. An acceptance test that
/// can be stated in advance is worth more than one discovered afterwards.
///
/// A flame's white core comes from `fireRamp`'s own top rung, not from a curve
/// bleaching it. The art decides what is white.
float3 shoulder(float3 c) {
    float peak = max(c.r, max(c.g, c.b));
    if (peak < KNEE) {
        return c;
    }
    // Scales the whole triple by one factor, which is why hue and saturation
    // survive: only the magnitude is compressed, never the ratios.
    float d = 1.0 - KNEE;
    return c * ((1.0 - d * d / (peak + d - KNEE)) / peak);
}

/// The ordered 8x8 Bayer value at a pixel, in [0, 1) — ADR 0077.
///
/// Built by interleaving the bits of `x ^ y` and `y` rather than from a table,
/// which is the standard construction and costs no memory.
float bayer8(uint2 p) {
    uint a = (p.x ^ p.y) & 7u;
    uint b = p.y & 7u;
    uint v = (((a >> 2) & 1u) << 5) | (((b >> 2) & 1u) << 4)
           | (((a >> 1) & 1u) << 3) | (((b >> 1) & 1u) << 2)
           | ((a & 1u) << 1) | (b & 1u);
    return float(v) * (1.0 / 64.0);
}

/// Half an output code of dither, **expressed in linear light** — ADR 0077.
///
/// The attachment is `R8G8B8A8_SRGB` (`renderer.rs:43`), so the hardware
/// encodes and quantises after this shader returns and the value here is
/// linear. Half a code of *linear* is the wrong quantity: sRGB's slope varies
/// by two orders across the range, so a fixed linear offset is invisible in the
/// highlights and enormous in the shadows — exactly backwards, because the
/// shadows are where the banding is.
///
/// sRGB's power segment is `L = ((s + 0.055)/1.055)^2.4`, so
/// `dL/ds = (2.4/1.055) * L^(7/12)`. One `pow` per channel, and no
/// encode/decode pair — which `tonemap.rs` records as existing nowhere in
/// `assets/shaders/`. Below the transfer's linear knee the slope is `1/12.92`;
/// that region is codes 0 and 1, where there is nothing to dither between, and
/// it is handled for correctness rather than for appearance.
///
/// **A function of the integer pixel coordinate and nothing else**, the same
/// rule `rayDither` follows, which is what keeps the frame a pure function of
/// its tick and `cargo xtask repeat` byte-identical.
static const float SRGB_LINEAR_KNEE = 0.0031308;
static const float SRGB_SLOPE = 2.4 / 1.055;
float3 dithered(float3 c, uint2 pixel) {
    float3 slope = float3(
        c.r < SRGB_LINEAR_KNEE ? (1.0 / 12.92) : SRGB_SLOPE * pow(c.r, 7.0 / 12.0),
        c.g < SRGB_LINEAR_KNEE ? (1.0 / 12.92) : SRGB_SLOPE * pow(c.g, 7.0 / 12.0),
        c.b < SRGB_LINEAR_KNEE ? (1.0 / 12.92) : SRGB_SLOPE * pow(c.b, 7.0 / 12.0));
    // Centred on zero: the offset must not shift the mean, only break the step.
    float offset = (bayer8(pixel) - 0.5 + 0.5 / 64.0) * (1.0 / 255.0);
    return max(c + slope * offset, 0.0);
}

/// One oversized triangle. The same three vertices, and the same reason, as
/// the CMAA2 passes: a full-screen quad is two triangles with a seam down the
/// diagonal where the rasteriser evaluates the same pixels twice.
[shader("vertex")]
float4 tonemapVertexMain(uint id: SV_VertexID) : SV_Position {
    float2 uv = float2(float((id << 1) & 2), float(id & 2));
    return float4(uv * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 tonemapFragmentMain(float4 pos: SV_Position) : SV_Target {
    // `max` before everything: a negative can only arrive from an authored
    // colour or a numerical accident, and feeding one to the shoulder's divide
    // — or to the power below — turns it into a NaN the golden gate reports as
    // a corrupt pixel rather than as the authoring mistake it is.
    float3 c = max(scene.Load(int3(int2(pos.xy) - push.origin, 0)).rgb, 0.0) * push.exposure;

    // **Two branches, each guarding exactly its own operation, and that is the
    // correction.** `lerp(l, c, 1.0)` is `l + 1.0*(c-l)`, which is not
    // bit-exact in fp32; the gate's tolerance would swallow the difference,
    // and "a neutral grade re-blesses nothing" should be true by construction
    // rather than by luck.
    //
    // One shared `graded` flag broke that. It included `push.contrast != 1.0`
    // — which guards nothing, because the contrast below is applied
    // unconditionally — so a stage with a neutral gain, a neutral saturation
    // and a moved contrast took the branch and ran the inexact
    // `lerp(l, c, 1.0)` anyway.
    //
    // **Measured, and the honest result is that it moved no pixel.** A probe
    // scene with exactly that grade renders byte-identical under both forms:
    // `l + 1.0*(c-l)` is wrong by at most an ulp, which is ~1e-7 relative and
    // three orders below one 8-bit code, so no PNG this project compares could
    // ever have shown it. The branch's value is that the property is provable
    // rather than measured — which is the whole reason it is a branch and not
    // an epsilon — and a condition that does not describe the operation it
    // guards cannot be reasoned about at all. That is what is fixed here, not
    // a visible artifact.
    //
    // Both conditions are on push constants, so every lane in every wave
    // agrees and both branches are free.
    bool tinted = any(push.gain.rgb != float3(1.0, 1.0, 1.0));
    bool desaturated = push.gain.w != 1.0;

    // ASC CDL slope, in linear light. **Equal channels is an exposure and
    // unequal is a white balance**, which is why the grade has no exposure of
    // its own — one multiply does both jobs and a second would only be a place
    // for them to disagree.
    if (tinted) {
        c *= push.gain.rgb;
    }

    // **The contrast, on scene-referred light, before the shoulder.** Before,
    // because a curve applied after the shoulder moves shadows and nothing
    // else — the shoulder has already spent the top of the range. The floor
    // inside the `max` is not the one above: `pow(0, s)` is
    // `exp2(log2(0) * s)` on this hardware, and `log2(0)` is a NaN even when
    // the input was an honest black.
    c = PIVOT * pow(max(c / PIVOT, 1e-8), CONTRAST * push.contrast);

    // Toward grey at 0, unchanged at 1, exaggerated above. **Nothing in
    // `Environment` can do this** — every "eerie" lighting setting anyone
    // measured came out MORE colourful than a happy baseline, because taking a
    // warm sun away stops washing albedos toward a common white.
    if (desaturated) {
        c = max(lerp(dot(c, LUMA).xxx, c, push.gain.w), 0.0);
    }

    return float4(dithered(shoulder(c), uint2(pos.xy)), 1.0);
}
