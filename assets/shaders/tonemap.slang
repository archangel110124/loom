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
    float exposure;
};
[[vk::push_constant]] TonemapPush push;

/// Where the curve departs from identity.
static const float KNEE = 0.76;

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
    float3 hdr = scene.Load(int3(int2(pos.xy), 0)).rgb * push.exposure;
    // `max` before the curve: a negative can only arrive from an authored
    // colour or a numerical accident, and feeding one to the shoulder's divide
    // turns it into a NaN the golden gate reports as a corrupt pixel rather
    // than as the authoring mistake it is.
    return float4(shoulder(max(hdr, 0.0)), 1.0);
}
