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
// Since P1 it also carries the noise primitive, which is the one thing written
// twice on purpose — an integer lattice hash cannot be expressed in the float
// expression tree. That half is compared for **exact** equality, because a
// hash that differs by a bit is not a rounding difference, it is a different
// number.
//
// Nothing in the runtime dispatches it. It is built by the same `build.rs`
// loop as every other shader so that it cannot rot: a change to a field that
// breaks the generated Slang fails the build here, not six weeks later.

// First, because the entry points below call into it.
#include "generated/fields.slang"

struct AgreePush {
    // xyz = position, w = time. **A float4 rather than a float3 plus a float
    // deliberately** — a float3 in a buffer still aligns to 16 bytes, so the
    // packed-looking pair would leave a hole the Rust mirror does not have,
    // and every sample after the first would read shifted. Same reason the
    // results are float4: the trap is real and `scene.slang`'s Push block
    // carries a long comment about the version of it that already bit.
    float4* samples;
    // xyz = the field at that sample, w unused. For the noise entry point,
    // x = the noise value and the rest is zero.
    float4* results;
    // The field's authored parameters, flat, in the order the generated
    // `wind_at_params` reader expects. Generated from one list, so the index
    // here and the struct field there cannot drift apart.
    float* params;
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
    WindParams params = wind_at_params(push.params);
    push.results[id.x] = float4(wind_at(sample.xyz, sample.w, params), 0.0);
}

// Cloud coverage, which is a *scalar* field — `clouds_at` returns a float3
// whose y and z are zero, and `.x` is the answer. Its own entry point rather
// than a second field evaluated in `agreeMain`, because the parameter block is
// flat and positional: two fields sharing one dispatch would have to share one
// parameter array, and the first field to gain a parameter would silently shift
// the second field's reads.
[shader("compute")]
[numthreads(64, 1, 1)]
void cloudsMain(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.count) {
        return;
    }

    float4 sample = push.samples[id.x];
    CloudsParams params = clouds_at_params(push.params);
    push.results[id.x] = float4(clouds_at(sample.xyz, sample.w, params), 0.0);
}

// The noise primitive alone, so a disagreement in it is reported as a
// disagreement in it rather than as a slightly-off wind field.
[shader("compute")]
[numthreads(64, 1, 1)]
void noiseMain(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.count) {
        return;
    }

    float4 sample = push.samples[id.x];
    push.results[id.x] = float4(loom_value_noise(sample.xyz), 0.0, 0.0, 0.0);
}
