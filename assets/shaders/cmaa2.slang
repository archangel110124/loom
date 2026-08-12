// Conservative Morphological Anti-Aliasing 2, the subset of it this engine runs.
//
// ADR 0010 authorises **one** non-temporal full-screen AA pass, because the
// toolkit P2 specified (MSAA, width clamping, density falloff,
// alpha-to-coverage, geometry over cards) left `meadow` twinkling at rest
// against a 0.000 control. This is that pass.
//
// # What this is, precisely
//
// A port of the *edge detection* and *simple-shape blend* halves of Intel's
// CMAA2, read from the published MIT-licensed source
// (`GameTechDev/CMAA2`, `Projects/CMAA2/CMAA2/CMAA2.hlsl`, revision fetched
// 2026-08-12). The following are ported rather than approximated, and the
// constants are Intel's, not invented here:
//
//   * `RGBToLumaForEdges` — Rec. 601 weights on `sqrt(linear)`, which is their
//     cheap stand-in for the sRGB curve. Our source image is a linear-valued
//     texture sampled from an `_SRGB` view, exactly the input CMAA2 expects.
//   * `ComputeEdge` / `ComputeEdgeLuma` — the fractional edge to the right and
//     below every pixel is the absolute luma difference across it.
//   * `ComputeLocalContrastH` / `ComputeLocalContrastV` — the four-connecting-
//     edges kernel, at `g_CMAA2_LocalContrastAdaptationAmount` = 0.10. This is
//     the "local-contrast-adaptive" half of the name: an edge in a busy
//     neighbourhood has to be stronger to count, which is what stops texture
//     detail being treated as silhouette.
//   * `GetActualEdgeThreshold` — 0.07, the HIGH quality preset.
//   * The candidate test from `EdgesColor2x2CS`: a pixel is processed **only**
//     if its four binary edges contain at least one two-edge corner. This is
//     the conservatism the algorithm is named for — a straight edge, a lone
//     edge, and a flat region are all left bit-for-bit alone.
//   * `ComputeSimpleShapeBlendValues` in full, including the 0.75 L-shape
//     factor, the `k = 0.9` shape-completion terms, and the
//     `saturate(1.30 - numberOfEdgesAllAround / 10)` dampening that preserves
//     text and texture.
//   * The four-tap blend in `ProcessCandidatesCS`, with `dontTestShapeValidity`
//     true — the candidate test above has already established the corner.
//
// # What is NOT implemented, stated plainly
//
//   * **The complex-shape / Z-shape path.** `DetectZsHorizontal`,
//     `FindZLineLengths`, `BlendZs` and the line tracing up to
//     `c_maxLineLength` = 86 pixels are all absent. That is CMAA2's main
//     quality advantage over a plain L-shape filter: long, near-horizontal or
//     near-vertical edges get a gradient across their length rather than a
//     local nudge. Adding it needs the edge bitmask stored in a texture so it
//     can be walked; here edges are recomputed per pixel and a walk would cost
//     16 texture fetches per step.
//   * **The compute architecture.** No 2x2 quad kernel, no groupshared edge
//     cache, no candidate append-buffer, no indirect dispatch, no deferred
//     blend-item list. This is one fragment shader per pixel, so it pays for
//     every pixel instead of only the ~2-5% CMAA2 finds interesting, and it
//     recomputes each edge up to five times. It is therefore *slower per pixel*
//     than the real thing while producing the same answer where both act.
//   * **Per-sample MSAA input** (`CMAA_MSAA_SAMPLE_COUNT > 1`). This runs on
//     the already-resolved image.
//   * `CMAA2_EXTRA_SHARPNESS`, the HDR range handling, and the debug paths.
//
// So: "CMAA2 edge detection plus simple-shape blend, no long-edge handling."
// Calling it CMAA2 without that sentence attached would be a lie.

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
// `g_CMAA2_SimpleShapeBlurinessAmount`, same branch.
static const float SIMPLE_SHAPE_BLURINESS = 0.10;

// The luma window this shader reads, as a radius: offsets -2..+3 on both axes.
// Not a round number and not a guess — it is exactly what the centre pixel's
// four neighbours' binary edges transitively need, and every index below is
// inside it.
static const int WINDOW_MIN = -2;
static const int WINDOW_SIZE = 6;

/// A full-screen triangle from the vertex index, no vertex buffer.
///
/// The same trick the sky pass uses. Position is read back in the fragment
/// stage as `SV_Position`, which is already in pixels, so nothing here has to
/// agree with anything about UV orientation.
[shader("vertex")]
float4 aaVertexMain(uint vertexID: SV_VertexID) : SV_Position {
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

/// One pixel's four binary edges, in CMAA2's packing order: **r right, g below,
/// b left, a above**. `ComputeSimpleShapeBlendValues` reads them by that name
/// and the order is easy to get silently wrong.
float4 edgesAt(float window[WINDOW_SIZE][WINDOW_SIZE], int2 offset) {
    return float4(
        edgeRight(window, offset) ? 1.0 : 0.0,
        edgeBelow(window, offset) ? 1.0 : 0.0,
        edgeRight(window, offset + int2(-1, 0)) ? 1.0 : 0.0,
        edgeBelow(window, offset + int2(0, -1)) ? 1.0 : 0.0);
}

/// `ComputeSimpleShapeBlendValues`, ported. Returns how much of the left,
/// above, right and below neighbours to mix in.
///
/// The validity test the original guards with `dontTestShapeValidity` is
/// skipped here for the same reason it is skipped there: the caller only gets
/// this far having already found a two-edge corner.
float4 simpleShapeBlend(
    float4 edges,
    float4 edgesLeft,
    float4 edgesRight,
    float4 edgesTop,
    float4 edgesBottom
) {
    float fromRight = edges.r;
    float fromBelow = edges.g;
    float fromLeft = edges.b;
    float fromAbove = edges.a;

    float blurCoeff = SIMPLE_SHAPE_BLURINESS;
    float numberOfEdges = dot(edges, float4(1.0, 1.0, 1.0, 1.0));
    float numberOfEdgesAllAround =
        dot(edgesLeft.bga + edgesRight.rga + edgesTop.rba + edgesBottom.rgb, float3(1.0, 1.0, 1.0));

    // An L-like step shape. The `k` terms complete the corner using the
    // neighbours' edges, so a staircase blends along its diagonal rather than
    // twice at right angles.
    if (numberOfEdges == 2.0) {
        blurCoeff *= 0.75;
        const float k = 0.9;
        fromRight += k * (edges.g * edgesTop.r * (1.0 - edgesLeft.g)
                        + edges.a * edgesBottom.r * (1.0 - edgesLeft.a));
        fromBelow += k * (edges.b * edgesRight.g * (1.0 - edgesTop.b)
                        + edges.r * edgesLeft.g * (1.0 - edgesTop.r));
        fromLeft += k * (edges.a * edgesBottom.b * (1.0 - edgesRight.a)
                       + edges.g * edgesTop.b * (1.0 - edgesRight.g));
        fromAbove += k * (edges.r * edgesLeft.a * (1.0 - edgesBottom.r)
                        + edges.b * edgesRight.a * (1.0 - edgesBottom.b));
    }

    // Lots of edges all around means texture, not silhouette. Back off.
    blurCoeff *= saturate(1.30 - numberOfEdgesAllAround / 10.0);
    return float4(fromLeft, fromAbove, fromRight, fromBelow) * blurCoeff;
}

[shader("fragment")]
float4 aaFragmentMain(float4 position: SV_Position) : SV_Target {
    uint width;
    uint height;
    source.GetDimensions(width, height);
    float2 texel = 1.0 / float2(float(width), float(height));
    int2 centre = int2(position.xy);

    float3 centreColor = source.Sample((float2(centre) + 0.5) * texel).rgb;

    // The luma window. `sqrt` of linear is `ProcessColorForEdgeDetect`, and
    // the weights are `RGBToLumaForEdges`'s Rec. 601 — which are defined on
    // gamma-compressed components, which is why the sqrt comes first.
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

    float4 edges = edgesAt(window, int2(0, 0));

    // **The conservative gate, and the whole reason this is not a blur.** No
    // two-edge corner, no shape, no touch: the pixel leaves as it arrived.
    bool candidate = (edges.x * edges.y + edges.y * edges.z
                    + edges.z * edges.w + edges.w * edges.x) != 0.0;
    if (!candidate) {
        return float4(centreColor, 1.0);
    }

    float4 edgesLeft = edgesAt(window, int2(-1, 0));
    float4 edgesRight = edgesAt(window, int2(1, 0));
    float4 edgesTop = edgesAt(window, int2(0, -1));
    float4 edgesBottom = edgesAt(window, int2(0, 1));

    float4 blend = simpleShapeBlend(edges, edgesLeft, edgesRight, edgesTop, edgesBottom);
    float centreWeight = 1.0 - dot(blend, float4(1.0, 1.0, 1.0, 1.0));

    float3 outColor = centreColor * centreWeight;
    if (blend.x > 0.0) {
        outColor += blend.x * source.Sample((float2(centre + int2(-1, 0)) + 0.5) * texel).rgb;
    }
    if (blend.y > 0.0) {
        outColor += blend.y * source.Sample((float2(centre + int2(0, -1)) + 0.5) * texel).rgb;
    }
    if (blend.z > 0.0) {
        outColor += blend.z * source.Sample((float2(centre + int2(1, 0)) + 0.5) * texel).rgb;
    }
    if (blend.w > 0.0) {
        outColor += blend.w * source.Sample((float2(centre + int2(0, 1)) + 0.5) * texel).rgb;
    }
    return float4(outColor, 1.0);
}
