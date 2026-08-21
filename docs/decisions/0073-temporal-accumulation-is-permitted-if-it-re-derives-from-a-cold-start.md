# ADR 0073 — Temporal accumulation is permitted, if it re-derives from a cold start

- **Date:** 2026-08-21
- **Status:** **accepted** (2026-08-21, human — "yes let's add temporal").
- **Decision touched:** amends the one-paragraph rejection of TAA/TSR in
  **ADR 0010 §"Why not the alternatives"**, and widens the licence ADR 0017 and
  ADR 0053 already opened. It does **not** touch the locked graphics decisions:
  Vulkan 1.3, dynamic rendering, descriptor indexing, `gpu-allocator`, barriers
  owned by the render graph all stand unchanged.

## What ADR 0010 actually said, and why it needed re-reading

The rejection has been quoted around this project as "no temporal, ever". It is
one paragraph and its reason is narrower than that:

> **Temporal accumulation (TAA/TSR).** Rejected by the locked decisions, and
> rejecting it is load-bearing: determinism, agent-verifiable renders and
> single-frame golden images all assume a frame is a pure function of its state.
> TAA would make every golden image a function of the preceding frames.

So the objection is **not** aesthetic and not about temporal filters as such. It
is that the project's verification story — the golden gate, `cargo xtask repeat`,
and the agent's ability to check its own work — rests on a frame being
reproducible. That property is what is being defended, and it is worth defending.

## Why the door was already open

Two ADRs have since admitted GPU state on exactly that reasoning:

- **ADR 0017** made rain drops stateful in a device-local buffer, and says so in
  its own words: *"what state costs is that a frame is no longer a pure function
  of its tick, which is the same objection ADR 0010 used to reject TAA."* It
  survives because a headless still **seeds deterministically and advances to
  `--sim N` in one dispatch**, verified byte-identical across three fresh
  processes. The state is *replayed*, not *accumulated from what happened to be
  on screen*.
- **ADR 0053** went further, making the cinematic water tier explicitly
  GPU-stateful and machine-local, with `--assert` refusing loudly rather than
  answering.

The distinction both of them draw is the one that matters, and it was never
written down as a rule.

## The rule

**Temporal accumulation is permitted where the accumulated state is a
deterministic function of (scene, tick) that re-derives from a cold start.**

Concretely, a temporal technique may be built if all of these hold:

1. **Cold-start reproducible.** Rendering scene S at tick N in a fresh process
   produces the same image as any other fresh process rendering S at tick N.
   `cargo xtask repeat`'s three-process byte comparison is the test, and it is
   not negotiable.
2. **The history is replayed, not observed.** The accumulation must be advanced
   from the seed by the same rule every time — the way rain advances to `--sim N`
   in one dispatch — rather than depending on which frames a viewer happened to
   draw, at what frame rate, or in what order.
3. **It never crosses ADR 0045's line.** No accumulated GPU value may produce a
   force on a rapier body, or be readable by `loom sim --assert` or by a rhai
   script. Temporal accumulation is a *rendering* technique here and stays one.
4. **The golden row is honest.** If a scene in `GOLDEN` uses it, the reference is
   rendered through the same cold-start path the gate uses, and the row must
   still pass `repeat`. A row that only reproduces after a warm-up is not a row.
5. **It degrades, rather than breaks, without history.** The first frame after a
   cold start, a camera cut, or a resolution change has no history. Whatever it
   shows must be a usable image, not a black frame or a smear.

**Machine-local is acceptable**, as ADR 0053 established: reproducibility is
required within one machine, not across two. A technique that depends on driver
scheduling or on floating-point behaviour that differs between GPUs is still
admissible, and `--assert` must refuse to answer about it.

## What this permits, and what it still refuses

**Permitted, subject to the five conditions:** TAA and TSR; temporal
reprojection of secondary-ray results; ReSTIR and its spatiotemporal reservoirs;
SVGF and A-SVGF with their temporal halves intact; temporally-amortised
sampling, where a frame traces a fraction of the rays and reuses the rest.

**Still refused, and these are not conditions to be argued around:**

- Anything whose output feeds the simulation. ADR 0045 clause 1 is untouched.
- Any accumulation seeded by wall-clock time, by an unseeded random, or by frame
  arrival order — none of which re-derive.
- Any technique that makes a golden reference dependent on the frames a
  *viewer* drew. The gate renders from cold; if the viewer and the gate disagree,
  the technique is wrong rather than the gate.

## What this costs, stated plainly

**The single-frame golden image stops being the whole story.** A temporal
technique that satisfies condition 1 is reproducible, but its *first* frame and
its converged frame are different images, and the gate photographs one of them.
`cargo xtask flythrough` and the shimmer harness become more load-bearing than
they were, because they are the only instruments that see the sequence.

**And the failure mode is quiet.** A temporal filter that is subtly
non-reproducible passes the image gate — which compares at a tolerance — and
fails `repeat`, which does not. **`repeat` is the gate that matters for anything
built under this ADR**, and it should be run on any scene that uses one.

## What would reopen this

- A temporal technique that cannot be made cold-start reproducible without an
  unacceptable cost, where the honest answer is to refuse it rather than to
  weaken condition 1.
- Evidence that `repeat` is not actually catching non-reproducibility — the
  property it exists to check is the entire foundation of this decision.

## Consequences for work in flight

The ray-tracing effort was briefed that temporal accumulation is closed and that
denoising must be purely spatial. **That brief was wrong** and is corrected by
this ADR. The spatial research remains valuable — a spatial filter that needs no
history is still the cheaper and safer answer where it suffices, and ADR 0019's
quad-share is still unbuilt — but temporal options are now on the table and must
be priced against condition 1 rather than dismissed.
