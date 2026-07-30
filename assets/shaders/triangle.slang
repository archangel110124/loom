// M2's proof-of-life shader: one triangle, vertex colours, no inputs.
//
// Positions are generated from SV_VertexID rather than read from a buffer, so
// the first render needs no vertex buffer, no descriptor set, and no upload —
// which keeps the first Vulkan bring-up to the smallest thing that can put
// pixels in a PNG. Real geometry arrives with buffer device address (M2 later).

struct VSOutput {
    float4 position : SV_Position;
    float3 color : COLOR;
};

[shader("vertex")]
VSOutput vertexMain(uint vertexID: SV_VertexID) {
    // Clip space directly. Y is down in Vulkan NDC, so this is wound to be
    // front-facing under the default counter-clockwise convention.
    const float2 positions[3] = {
        float2(0.0, -0.6),
        float2(0.6, 0.6),
        float2(-0.6, 0.6),
    };
    const float3 colors[3] = {
        float3(1.0, 0.2, 0.2),
        float3(0.2, 1.0, 0.2),
        float3(0.2, 0.4, 1.0),
    };

    VSOutput out;
    out.position = float4(positions[vertexID], 0.0, 1.0);
    out.color = colors[vertexID];
    return out;
}

[shader("fragment")]
float4 fragmentMain(VSOutput in) : SV_Target {
    return float4(in.color, 1.0);
}
