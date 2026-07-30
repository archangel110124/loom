// One lit box per scene object, drawn in a single instanced draw call.
//
// Both the geometry and the per-object data are reached through **buffer
// device address** — the scene is a pointer graph the GPU walks, with no
// descriptor sets and no vertex input state (CLAUDE.md locked decisions).
//
// Push constants carry only two pointers, so adding per-object fields never
// runs into the 128-byte push limit: the object array grows instead. CPU work
// per frame is one draw call regardless of object count, which is the property
// §B.1 of the graphics doc is after.

// 16-byte aligned members. `float3` would place `normal` at offset 12 and
// violate std430 for a PhysicalStorageBuffer block — spirv-val rejects it.
struct Vertex {
    float4 position;
    float4 normal;
};

struct ObjectData {
    float4x4 mvp;
    // Rows of inverse-transpose(model)'s upper 3x3. Non-uniform scale is
    // everywhere in a blockout, and it skews normals unless corrected.
    float4 normalRow0;
    float4 normalRow1;
    float4 normalRow2;
    float4 color;
};

struct Push {
    Vertex* vertices;
    ObjectData* objects;
};
[[vk::push_constant]] Push push;

struct VSOutput {
    float4 position : SV_Position;
    float3 normal : NORMAL;
    float3 color : COLOR;
};

[shader("vertex")]
VSOutput vertexMain(uint vertexID: SV_VertexID, uint instanceID: SV_InstanceID) {
    Vertex v = push.vertices[vertexID];
    ObjectData o = push.objects[instanceID];

    float3x3 normalMatrix = float3x3(
        o.normalRow0.xyz, o.normalRow1.xyz, o.normalRow2.xyz);

    VSOutput out;
    out.position = mul(o.mvp, float4(v.position.xyz, 1.0));
    out.normal = normalize(mul(normalMatrix, v.normal.xyz));
    out.color = o.color.rgb;
    return out;
}

[shader("fragment")]
float4 fragmentMain(VSOutput in) : SV_Target {
    // One fixed directional light plus ambient. Enough to read shape and depth
    // in a PNG, which is the whole point of this render path.
    const float3 lightDir = normalize(float3(0.4, 0.9, 0.3));
    float diffuse = max(dot(normalize(in.normal), lightDir), 0.0);
    float3 lit = in.color * (0.25 + 0.75 * diffuse);
    return float4(lit, 1.0);
}
