// Evaluate the generated analytic fields on the GPU, for the agreement test.
//
// **This exists to be compared against Rust, not to be rendered.** S2's whole
// point is that a CPU field and a GPU field can each look internally correct
// and silently disagree, so physics and visuals drift apart with no error
// anywhere. `generated/fields.slang` removes the chance of a *structural*
// disagreement — both sides come from one expression tree in `loom_field`.
// What is left is floating point: a GPU `sin` is not libm's `sin`, and a
// driver may contract a multiply-add. This shader is how that residual gets
// measured instead of assumed.
//
// Nothing in the runtime dispatches it. It is built by the same `build.rs`
// loop as every other shader so that it cannot rot: a change to the field that
// breaks the generated Slang fails the build here, not six weeks later in
// Phase 1.

// First, because the entry point below calls into it.
#include "generated/fields.slang"

struct AgreePush {
    // xyz = position, w = time. **A float4 rather than a float3 plus a float
    // deliberately** — a float3 in a buffer still aligns to 16 bytes, so the
    // packed-looking pair would leave a hole the Rust mirror does not have,
    // and every sample after the first would read shifted. Same reason the
    // results are float4: the trap is real and `scene.slang`'s Push block
    // carries a long comment about the version of it that already bit.
    float4* samples;
    // xyz = the field at that sample, w unused.
    float4* results;
    uint count;
};

[[vk::push_constant]] AgreePush push;

[shader("compute")]
[numthreads(64, 1, 1)]
void agreeMain(uint3 id: SV_DispatchThreadID) {
    // The dispatch rounds up to whole workgroups, so the tail runs past the
    // end. Without this it writes outside the buffer.
    if (id.x >= push.count) {
        return;
    }

    float4 sample = push.samples[id.x];
    push.results[id.x] = float4(wind_at(sample.xyz, sample.w), 0.0);
}
