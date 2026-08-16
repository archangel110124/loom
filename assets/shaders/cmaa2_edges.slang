// CMAA2, pass 1 of 2: local-contrast-adaptive edge detection.
//
// This half used to live inside `cmaa2.slang` and be recomputed per pixel, up
// to five times each, because nothing stored it. **The Z-shape path needs the
// mask stored**: tracing a long edge means walking dozens of pixels along it,
// and recomputing a 4x4 luma window at every step would be sixteen texture
// fetches per step instead of one.
//
// So the mask is written to an `R8_UINT` image and pass 2 reads it. That also
// makes the detection cheaper for the pixels that were already being filtered:
// one evaluation per edge rather than five.
//
// Everything here is Intel's, read from `GameTechDev/CMAA2`,
// `Projects/CMAA2/CMAA2/CMAA2.hlsl` — `ComputeEdge`, `ComputeLocalContrastH/V`,
// `GetActualEdgeThreshold` at the HIGH preset, and `PackEdges`' bit order.

// The resolved colour target of the frame that just rendered. NEAREST and
// CLAMP_TO_EDGE, so `sample` is a texel fetch and the border needs no bounds
// test — a pixel off the edge reads its nearest neighbour, which is what an
// edge-detection kernel wants anyway.
[[vk::binding(0, 0)]]
uniform Sampler2D source;

// Intel's HIGH preset (`CMAA2_STATIC_QUALITY_PRESET == 2`).
static const float EDGE_THRESHOLD = 0.07;
// `g_CMAA2_LocalContrastAdaptationAmount`, non-extra-sharpness branch.
static const float LOCAL_CONTRAST_ADAPTATION = 0.10;

// The luma window this shader reads: offsets -1..+2 on both axes.
//
// Not a round number and not a guess — it is exactly what the centre pixel's
// four binary edges transitively need. It was 6x6 when this shader also had to
// produce its four *neighbours'* edges; those now come from the mask image, so
// the window shrank with the split.
static const int WINDOW_MIN = -1;
static const int WINDOW_SIZE = 4;

/// A full-screen triangle from the vertex index, no vertex buffer.
///
/// Shared with pass 2 by being compiled into both modules; the two are separate
/// Slang files so that each declares exactly the bindings it uses, and a
/// pipeline layout can therefore be an exact description rather than a superset.
[shader("vertex")]
float4 edgeVertexMain(uint vertexID: SV_VertexID) : SV_Position {
    float2 uv = float2(float((vertexID << 1) & 2), float(vertexID & 2));
    return float4(uv * 2.0 - 1.0, 0.0, 1.0);
}

float lumaAt(float window[WINDOW_SIZE][WINDOW_SIZE], int2 offset) {
    return window[offset.x - WINDOW_MIN][offset.y - WINDOW_MIN];
}

/// The fractional edge to the right (x) and below (y) of pixel `offset`.
float2 fracEdge(float window[WINDOW_SIZE][WINDOW_SIZE], int2 offset) {
    float centre = lumaAt(window, offset);
    return float2(
        abs(centre - lumaAt(window, offset + int2(1, 0))),
        abs(centre - lumaAt(window, offset + int2(0, 1))));
}

/// Is there a real vertical edge on the right of pixel `offset`?
///
/// `ComputeLocalContrastV`: the threshold is lowered by the strongest
/// horizontal edge among the two pixels this edge separates and the two above
/// them. A vertical edge running through busy horizontal detail is detail.
bool edgeRight(float window[WINDOW_SIZE][WINDOW_SIZE], int2 offset) {
    float contrast = max(
        max(fracEdge(window, offset).y, fracEdge(window, offset + int2(0, -1)).y),
        max(fracEdge(window, offset + int2(1, 0)).y, fracEdge(window, offset + int2(1, -1)).y));
    return (fracEdge(window, offset).x - contrast * LOCAL_CONTRAST_ADAPTATION) > EDGE_THRESHOLD;
}

/// Is there a real horizontal edge below pixel `offset`? `ComputeLocalContrastH`.
bool edgeBelow(float window[WINDOW_SIZE][WINDOW_SIZE], int2 offset) {
    float contrast = max(
        max(fracEdge(window, offset).x, fracEdge(window, offset + int2(-1, 0)).x),
        max(fracEdge(window, offset + int2(0, 1)).x, fracEdge(window, offset + int2(-1, 1)).x));
    return (fracEdge(window, offset).y - contrast * LOCAL_CONTRAST_ADAPTATION) > EDGE_THRESHOLD;
}

/// `PackEdges`: **bit 0 right, bit 1 below, bit 2 left, bit 3 above.**
///
/// Pass 2 reads them by that name and by those bit values — `FindZLineLengths`
/// spells its trace masks as the literals `0x01`, `0x02`, `0x04`, `0x08` — so
/// the order is not a private convention and cannot be changed on one side.
[shader("fragment")]
uint edgeFragmentMain(float4 position: SV_Position) : SV_Target {
    uint width;
    uint height;
    source.GetDimensions(width, height);
    float2 texel = 1.0 / float2(float(width), float(height));
    int2 centre = int2(position.xy);

    // `sqrt` of linear is `ProcessColorForEdgeDetect`, and the weights are
    // `RGBToLumaForEdges`'s Rec. 601 — which are defined on gamma-compressed
    // components, which is why the sqrt comes first.
    float window[WINDOW_SIZE][WINDOW_SIZE];
    [unroll]
    for (int x = 0; x < WINDOW_SIZE; ++x) {
        [unroll]
        for (int y = 0; y < WINDOW_SIZE; ++y) {
            float2 uv = (float2(centre + int2(x + WINDOW_MIN, y + WINDOW_MIN)) + 0.5) * texel;
            float3 linearRgb = source.Sample(uv).rgb;
            window[x][y] = dot(sqrt(max(linearRgb, 0.0)), float3(0.299, 0.587, 0.114));
        }
    }

    uint packed = 0;
    if (edgeRight(window, int2(0, 0))) { packed |= 0x01; }
    if (edgeBelow(window, int2(0, 0))) { packed |= 0x02; }
    if (edgeRight(window, int2(-1, 0))) { packed |= 0x04; }
    if (edgeBelow(window, int2(0, -1))) { packed |= 0x08; }
    return packed;
}
