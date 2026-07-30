// One lit cube per scene node.
//
// Geometry is generated from SV_VertexID against a corner table rather than
// read from a vertex buffer. That is deliberate for M2: it gets objects on
// screen with no buffer upload, no vertex input state, and no descriptor sets,
// which keeps the first real render small enough to debug. Real meshes arrive
// with the asset pipeline at M5, via buffer device address.

struct Push {
    float4x4 mvp;       // model-view-projection, built on the CPU
    // Upper 3x3 of inverse-transpose(model), one row per float4. Stored as
    // three float4s rather than a float3x3 so the 16-byte row padding is
    // explicit rather than a layout rule to get wrong. 64+48+16 = 128 bytes,
    // exactly Vulkan's guaranteed minimum push-constant size.
    float4 normalRow0;
    float4 normalRow1;
    float4 normalRow2;
    float4 color;
};
[[vk::push_constant]] Push push;

// Unit cube corners, -1..1.
static const float3 CORNERS[8] = {
    float3(-1, -1, -1), float3( 1, -1, -1), float3( 1,  1, -1), float3(-1,  1, -1),
    float3(-1, -1,  1), float3( 1, -1,  1), float3( 1,  1,  1), float3(-1,  1,  1),
};

// 6 faces x 2 triangles, counter-clockwise when viewed from outside.
static const uint INDICES[36] = {
    0, 2, 1,  0, 3, 2,   // -Z
    5, 7, 4,  5, 6, 7,   // +Z
    4, 3, 0,  4, 7, 3,   // -X
    1, 6, 5,  1, 2, 6,   // +X
    4, 1, 5,  4, 0, 1,   // -Y
    3, 6, 2,  3, 7, 6,   // +Y
};

static const float3 NORMALS[6] = {
    float3(0, 0, -1), float3(0, 0, 1),
    float3(-1, 0, 0), float3(1, 0, 0),
    float3(0, -1, 0), float3(0, 1, 0),
};

struct VSOutput {
    float4 position : SV_Position;
    float3 normal : NORMAL;
    float3 color : COLOR;
};

[shader("vertex")]
VSOutput vertexMain(uint vertexID: SV_VertexID) {
    VSOutput out;
    float3 local = CORNERS[INDICES[vertexID]];
    out.position = mul(push.mvp, float4(local, 1.0));

    // Non-uniform scale is everywhere in a blockout (a desk is a squashed
    // box), and scaling a normal by the model matrix skews it. The CPU sends
    // inverse-transpose(model) so this stays correct.
    float3x3 normalMatrix = float3x3(
        push.normalRow0.xyz, push.normalRow1.xyz, push.normalRow2.xyz);
    float3 n = NORMALS[vertexID / 6];
    out.normal = normalize(mul(normalMatrix, n));
    out.color = push.color.rgb;
    return out;
}

[shader("fragment")]
float4 fragmentMain(VSOutput in) : SV_Target {
    // One fixed directional light plus ambient. Enough to read shape and
    // depth in a PNG, which is the whole point of this render path.
    const float3 lightDir = normalize(float3(0.4, 0.9, 0.3));
    float diffuse = max(dot(normalize(in.normal), lightDir), 0.0);
    float3 lit = in.color * (0.25 + 0.75 * diffuse);
    return float4(lit, 1.0);
}
