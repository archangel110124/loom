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
};
[[vk::push_constant]] TonemapPush push;

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

    // **The contrast, on scene-referred light, before the shoulder.** Before,
    // because a curve applied after the shoulder moves shadows and nothing
    // else — the shoulder has already spent the top of the range. The floor
    // inside the `max` is not the one above: `pow(0, s)` is
    // `exp2(log2(0) * s)` on this hardware, and `log2(0)` is a NaN even when
    // the input was an honest black.
    c = PIVOT * pow(max(c / PIVOT, 1e-8), CONTRAST);

    return float4(shoulder(c), 1.0);
}
