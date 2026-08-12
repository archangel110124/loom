// CMAA2, pass 2 of 2: the shape filter. Simple (L-corner) shapes **and**
// Z-shapes / long edges.
//
// ADR 0010 authorises **one** non-temporal full-screen AA pass, because the
// toolkit P2 specified (MSAA, width clamping, density falloff,
// alpha-to-coverage, geometry over cards) left `meadow` twinkling at rest
// against a 0.000 control. This is that pass. `cmaa2_edges.slang` is pass 1 and
// produces the mask this reads.
//
// # What this is, precisely
//
// A port of Intel's CMAA2, read from the published MIT-licensed source
// (`GameTechDev/CMAA2`, `Projects/CMAA2/CMAA2/CMAA2.hlsl`, revision fetched
// 2026-08-12). Ported rather than approximated, with Intel's constants:
//
//   * `ComputeSimpleShapeBlendValues` in full, including the 0.75 L-shape
//     factor, the `k = 0.9` shape-completion terms, and the
//     `saturate(1.30 - numberOfEdgesAllAround / 10)` dampening that preserves
//     text and texture.
//   * The candidate test from `EdgesColor2x2CS`: a pixel gets a *simple-shape*
//     blend **only** if its four binary edges contain at least one two-edge
//     corner. This is the conservatism the algorithm is named for.
//   * `DetectZsHorizontal`, `FindZLineLengths` and `BlendZs`, with
//     `c_maxLineLength`, `c_symmetryCorrectionOffset` = 0.22 and
//     `c_dampeningEffect` = 0.15.
//   * `DeferredColorApply2x2CS`'s weighting: a simple-shape contribution counts
//     0.8 and a Z contribution 1.8, and a pixel that receives several is their
//     weighted mean. That ratio is why a long edge wins over the L-corner
//     nudge that also fires on it.
//
// # The one structural difference, and it is the whole design
//
// **Intel's Z blend scatters; this one gathers.** `BlendZs` runs once per
// *candidate* pixel and writes a run of up to ~86 pixels along the line, which
// needs random write access — a compute pass with a storage image. This
// renderer cannot have one here: `COLOR_FORMAT` is `R8G8B8A8_SRGB`, the
// swapchain is `B8G8R8A8_SRGB`, and **no sRGB format supports `STORAGE`**
// (`cmaa2.rs` documents the consequences).
//
// So the loop is inverted. A pixel asks *which Z-shape covers me*, by tracing
// back along its own edge to the shape's step pixel and then re-deriving that
// shape exactly as `ProcessCandidatesCS` would. The arithmetic downstream of
// finding the step pixel is Intel's, unchanged; only the direction of the
// search is new. It costs more — up to eight searches per pixel instead of one
// blend per candidate — and it produces the same answer, with one honest
// exception: the search assumes the traced run *ends* at the step pixel, so a
// shape whose neighbour happens to carry the same edge bit one pixel past the
// step is missed rather than mis-blended. Intel never has to guess where the
// step is, because it starts there.
//
// # What is still NOT implemented
//
//   * **The compute architecture.** No 2x2 quad kernel, no groupshared edge
//     cache, no candidate append-buffer, no indirect dispatch. Every pixel pays
//     for the shape search instead of only the ~2-5% CMAA2 finds interesting.
//   * **Per-sample MSAA input** (`CMAA_MSAA_SAMPLE_COUNT > 1`). This runs on
//     the already-resolved image.
//   * `CMAA2_EXTRA_SHARPNESS`, `CMAA2_EXTRA_CONSERVATIVENESS2`, the HDR range
//     handling, and the debug paths.

// The resolved colour target of the frame that just rendered.
[[vk::binding(0, 0)]]
uniform Sampler2D source;

// The packed edge mask from pass 1. A `Texture2D<uint>` and not a sampler:
// every read is an integer texel fetch at an exact coordinate, and filtering a
// bitmask would be meaningless.
[[vk::binding(1, 0)]]
[[vk::image_format("unknown")]]
uniform Texture2D<uint> edgeMask;

// `g_CMAA2_SimpleShapeBlurinessAmount`, non-extra-sharpness branch.
static const float SIMPLE_SHAPE_BLURINESS = 0.10;
// `c_symmetryCorrectionOffset` and `c_dampeningEffect`, "determined
// empirically :)" in Intel's own comment.
static const float SYMMETRY_CORRECTION_OFFSET = 0.22;
static const float DAMPENING_EFFECT = 0.15;

// `c_maxLineLength`. **Intel's value is 86 and this is 86.**
//
// It is the cap on how far a Z trace walks along an edge, and here it is paid
// twice — once to find the shape's step pixel and once inside
// `findZLineLengths`.
//
// **32 was measured against it and is bit-identical** on every scene in the
// gate at 1920x1080: no edge in them survives the 1.25x symmetry test past 32
// steps, so the cap never binds. It saves 0.009 ms on `meadow` (0.090 against
// 0.099) and changes not one pixel. 86 stays because it is Intel's number and
// because a longer edge — a higher resolution, a flatter horizon — would reach
// it, and a shorter cap does not merely clip very long edges: it changes
// `FindZLineLengths`' termination and therefore the blend gradient on *every*
// shape longer than the cap. Nine microseconds is not worth that.
static const int MAX_LINE_LENGTH = 86;

/// A full-screen triangle from the vertex index, no vertex buffer.
[shader("vertex")]
float4 aaVertexMain(uint vertexID: SV_VertexID) : SV_Position {
    float2 uv = float2(float((vertexID << 1) & 2), float(vertexID & 2));
    return float4(uv * 2.0 - 1.0, 0.0, 1.0);
}

/// The image size, cached once per pixel — every fetch below clamps against it.
static int2 g_size;

/// `LoadEdge`, with the clamping the trace needs.
///
/// A line trace walks until the edge stops, which routinely means walking off
/// the screen. Clamping reads the border pixel, matching the source sampler's
/// `CLAMP_TO_EDGE`; an unclamped `Load` past the edge is undefined in Vulkan,
/// not conveniently zero.
uint loadEdge(int2 p) {
    return edgeMask.Load(int3(clamp(p, int2(0, 0), g_size - 1), 0));
}

/// `UnpackEdgesFlt`: **r right, g below, b left, a above.**
float4 unpackEdges(uint value) {
    return float4(
        (value & 0x01) != 0 ? 1.0 : 0.0,
        (value & 0x02) != 0 ? 1.0 : 0.0,
        (value & 0x04) != 0 ? 1.0 : 0.0,
        (value & 0x08) != 0 ? 1.0 : 0.0);
}

float3 loadColor(int2 p) {
    return source.Sample((float2(clamp(p, int2(0, 0), g_size - 1)) + 0.5) / float2(g_size)).rgb;
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

/// `DetectZsHorizontal`, verbatim. The vertical case calls it with every input
/// swizzled `.argb`, which is a 90-degree rotation of the edge bits.
void detectZsHorizontal(
    float4 edges,
    float4 edgesM1P0,
    float4 edgesP1P0,
    float4 edgesP2P0,
    out float invertedZScore,
    out float normalZScore
) {
    // Inverted Z case:      Normal Z case:
    //    __                 __
    //   X|                   X|
    //  --                      --
    invertedZScore = edges.r * edges.g * edgesP1P0.a;
    invertedZScore *= 2.0 + ((edgesM1P0.g + edgesP2P0.a)) - (edges.a + edgesP1P0.g)
        - 0.7 * (edgesP2P0.g + edgesM1P0.a + edges.b + edgesP1P0.r);

    normalZScore = edges.r * edges.a * edgesP1P0.g;
    normalZScore *= 2.0 + ((edgesM1P0.a + edgesP2P0.g)) - (edges.g + edgesP1P0.a)
        - 0.7 * (edgesP2P0.a + edgesM1P0.g + edges.b + edgesP1P0.r);
}

/// The "complex shapes - detect" block of `ProcessCandidatesCS`, asked about a
/// specific pixel rather than about the pixel being shaded.
///
/// Returns false when `q` is the corner of no Z at all. When it returns true,
/// `horizontal` and `inverted` are the orientation that **won** — Intel scores
/// both and keeps only the better, and a gather has to agree with that choice
/// or it will blend a shape the scatter never would.
bool zShapeAt(int2 q, out bool horizontal, out bool inverted, out float shapeQualityScore) {
    float4 edges = unpackEdges(loadEdge(q));
    float4 edgesLeft = unpackEdges(loadEdge(q + int2(-1, 0)));
    float4 edgesRight = unpackEdges(loadEdge(q + int2(1, 0)));
    float4 edgesTop = unpackEdges(loadEdge(q + int2(0, -1)));
    float4 edgesBottom = unpackEdges(loadEdge(q + int2(0, 1)));

    float invertedZScore;
    float normalZScore;
    detectZsHorizontal(
        edges, edgesLeft, edgesRight, unpackEdges(loadEdge(q + int2(2, 0))),
        invertedZScore, normalZScore);
    float maxScore = max(invertedZScore, normalZScore);
    horizontal = true;
    inverted = false;
    if (maxScore > 0.0) {
        inverted = invertedZScore > normalZScore;
    }

    // Vertical: the same code on inputs rotated 90 degrees counter-clockwise —
    // left becomes bottom, top becomes left, right becomes top, bottom becomes
    // right — which is what the `.argb` swizzle does to a packed edge quad.
    detectZsHorizontal(
        edges.argb, edgesBottom.argb, edgesTop.argb, unpackEdges(loadEdge(q + int2(0, -2))).argb,
        invertedZScore, normalZScore);
    float verticalScore = max(invertedZScore, normalZScore);
    if (verticalScore > maxScore) {
        maxScore = verticalScore;
        horizontal = false;
        inverted = invertedZScore > normalZScore;
    }

    // 0 best quality, 1 some edges missing but ok, 2 and 3 dubious but better
    // than nothing.
    shapeQualityScore = floor(clamp(4.0 - maxScore, 0.0, 3.0));
    return maxScore > 0.0;
}

/// The trace masks `FindZLineLengths` builds. Returned as a pair so the search
/// that *finds* the step pixel uses exactly the masks the walk from it will.
void traceMasks(bool horizontal, bool inverted, out uint maskLeft, out uint maskRight) {
    // Horizontal traces the top edge leftward and the bottom edge rightward;
    // vertical (rotated) traces the left edge and the right edge. Inverting the
    // Z swaps which side gets which.
    uint left = horizontal ? 0x08 : 0x04;
    uint right = horizontal ? 0x02 : 0x01;
    maskLeft = inverted ? right : left;
    maskRight = inverted ? left : right;
}

/// `FindZLineLengths`, verbatim apart from the loop being explicitly bounded.
///
/// The two directions advance **together** and the search ends when one gets
/// more than 1.25x ahead of the other. That coupling is why the run lengths
/// cannot simply be read off the two edge runs: they are not the runs' lengths,
/// they are how far a symmetric search got.
void findZLineLengths(
    int2 screenPos,
    bool horizontal,
    bool inverted,
    int2 stepRight,
    out float lineLengthLeft,
    out float lineLengthRight
) {
    uint maskLeft;
    uint maskRight;
    traceMasks(horizontal, inverted, maskLeft, maskRight);

    bool continueLeft = true;
    bool continueRight = true;
    lineLengthLeft = 1.0;
    lineLengthRight = 1.0;
    for (int step = 0; step < MAX_LINE_LENGTH; ++step) {
        uint edgeLeft = loadEdge(screenPos - stepRight * int(lineLengthLeft));
        uint edgeRight = loadEdge(screenPos + stepRight * (int(lineLengthRight) + 1));

        continueLeft = continueLeft && ((edgeLeft & maskLeft) == maskLeft);
        continueRight = continueRight && ((edgeRight & maskRight) == maskRight);

        lineLengthLeft += continueLeft ? 1.0 : 0.0;
        lineLengthRight += continueRight ? 1.0 : 0.0;

        float maxLR = max(lineLengthRight, lineLengthLeft);
        if (!continueLeft && !continueRight) {
            maxLR = float(MAX_LINE_LENGTH);
        }
        if (maxLR >= min(float(MAX_LINE_LENGTH),
                         1.25 * min(lineLengthRight, lineLengthLeft) - 0.25)) {
            break;
        }
    }
}

/// Walk from `p` in `dir` while the pixels keep carrying `mask`, and return the
/// last one that does.
///
/// This is the inverted half of the algorithm: Intel starts at the step pixel
/// and walks outward, so it never needs this. A gather starts somewhere in the
/// middle of the line and has to find the step first.
int2 runEnd(int2 p, int2 dir, uint mask) {
    int2 current = p;
    for (int step = 0; step < MAX_LINE_LENGTH; ++step) {
        int2 next = current + dir;
        if ((loadEdge(next) & mask) != mask) {
            break;
        }
        current = next;
    }
    return current;
}

[shader("fragment")]
float4 aaFragmentMain(float4 position: SV_Position) : SV_Target {
    uint width;
    uint height;
    edgeMask.GetDimensions(width, height);
    g_size = int2(int(width), int(height));

    int2 centre = int2(position.xy);
    float3 centreColor = loadColor(centre);

    uint centreMask = loadEdge(centre);
    // **Nothing to do at all for a pixel with no edges**, which is most of
    // them. Every cost below — the eight shape searches especially — is paid
    // only by pixels the detector already flagged.
    if (centreMask == 0) {
        return float4(centreColor, 1.0);
    }

    float4 edges = unpackEdges(centreMask);
    float4 edgesLeft = unpackEdges(loadEdge(centre + int2(-1, 0)));
    float4 edgesRight = unpackEdges(loadEdge(centre + int2(1, 0)));
    float4 edgesTop = unpackEdges(loadEdge(centre + int2(0, -1)));
    float4 edgesBottom = unpackEdges(loadEdge(centre + int2(0, 1)));

    // `DeferredColorApply2x2CS`: contributions accumulate weighted and the
    // result is their mean. A pixel with none keeps its original colour.
    float3 accumulated = float3(0.0, 0.0, 0.0);
    float weight = 0.0;

    // **The conservative gate on the simple-shape half, and the whole reason
    // this is not a blur.** No two-edge corner, no L-shape.
    bool candidate = (edges.x * edges.y + edges.y * edges.z
                    + edges.z * edges.w + edges.w * edges.x) != 0.0;
    if (candidate) {
        float4 blend = simpleShapeBlend(edges, edgesLeft, edgesRight, edgesTop, edgesBottom);
        float centreWeight = 1.0 - dot(blend, float4(1.0, 1.0, 1.0, 1.0));
        float3 simple = centreColor * centreWeight;
        simple += blend.x * loadColor(centre + int2(-1, 0));
        simple += blend.y * loadColor(centre + int2(0, -1));
        simple += blend.z * loadColor(centre + int2(1, 0));
        simple += blend.w * loadColor(centre + int2(0, 1));
        accumulated += simple * 0.8;
        weight += 0.8;
    }

    // The Z-shape half. Eight searches: two orientations, normal and inverted,
    // and this pixel sitting on either side of the step. Each is skipped
    // immediately unless this pixel carries the edge bit that side of that
    // shape traces along, which is what keeps the count nearer one in practice.
    int2 found[8];
    int foundCount = 0;
    for (int combo = 0; combo < 8; ++combo) {
        bool horizontal = combo < 4;
        bool inverted = ((combo >> 1) & 1) != 0;
        bool rightSide = (combo & 1) != 0;
        // Vertical's "right" is up: the rotation that makes the vertical case
        // reuse the horizontal code maps +x to -y.
        int2 stepRight = horizontal ? int2(1, 0) : int2(0, -1);

        uint maskLeft;
        uint maskRight;
        traceMasks(horizontal, inverted, maskLeft, maskRight);
        uint mask = rightSide ? maskRight : maskLeft;
        if ((centreMask & mask) != mask) {
            continue;
        }

        // The left run is `q, q-1, q-2, ...` and the right run is
        // `q+1, q+2, ...` — `FindZLineLengths` starts its right-hand load at
        // `q + 2` precisely because `q + 1` is already known from the score.
        // So the step pixel is the far end of this pixel's run, one further
        // back on the right-hand side.
        int2 q = rightSide
            ? runEnd(centre, -stepRight, mask) - stepRight
            : runEnd(centre, stepRight, mask);

        // Two searches can land on the same step pixel where the runs meet.
        // Intel blends such a pixel once, because the scatter runs once per
        // step pixel; counting it twice here would double its weight.
        bool seen = false;
        for (int i = 0; i < foundCount; ++i) {
            seen = seen || all(found[i] == q);
        }
        if (seen) {
            continue;
        }
        found[foundCount] = q;
        ++foundCount;

        bool qHorizontal;
        bool qInverted;
        float shapeQualityScore;
        if (!zShapeAt(q, qHorizontal, qInverted, shapeQualityScore)) {
            continue;
        }
        // The shape that actually wins at `q` may be the other orientation, in
        // which case this search found a line that Intel would not blend along.
        if (qHorizontal != horizontal || qInverted != inverted) {
            continue;
        }

        float lineLengthLeft;
        float lineLengthRight;
        findZLineLengths(q, horizontal, inverted, stepRight, lineLengthLeft, lineLengthRight);
        lineLengthLeft -= shapeQualityScore;
        lineLengthRight -= shapeQualityScore;
        if ((lineLengthLeft + lineLengthRight) < 5.0) {
            continue;
        }

        // `BlendZs`, for the single `i` that is this pixel.
        int2 delta = centre - q;
        float i = float(delta.x * stepRight.x + delta.y * stepRight.y);
        float loopFrom = -floor((lineLengthLeft + 1.0) / 2.0) + 1.0;
        float loopTo = floor((lineLengthRight + 1.0) / 2.0);
        if (i < loopFrom || i > loopTo) {
            continue;
        }

        float leftOdd = SYMMETRY_CORRECTION_OFFSET * fmod(lineLengthLeft, 2.0);
        float rightOdd = SYMMETRY_CORRECTION_OFFSET * fmod(lineLengthRight, 2.0);
        float dampenEffect =
            saturate((lineLengthLeft + lineLengthRight - shapeQualityScore) * DAMPENING_EFFECT);

        float totalLength = (loopTo - loopFrom) + 1.0 - leftOdd - rightOdd;
        float lerpStep = 1.0 / totalLength;
        float lerpFromK = (0.5 - leftOdd - loopFrom) * lerpStep;

        float secondPart = i > 0.0 ? 1.0 : 0.0;
        float srcOffset = 1.0 - secondPart * 2.0;
        float lerpK = ((lerpStep * i + lerpFromK) * srcOffset + secondPart) * dampenEffect;

        int2 blendDir = horizontal ? int2(0, -1) : int2(-1, 0);
        if (inverted) {
            blendDir = -blendDir;
        }
        float3 colorFrom = loadColor(centre + blendDir * int(srcOffset));
        accumulated += lerp(centreColor, colorFrom, lerpK) * 1.8;
        weight += 1.8;
    }

    if (weight <= 0.0) {
        return float4(centreColor, 1.0);
    }
    return float4(accumulated / weight, 1.0);
}
