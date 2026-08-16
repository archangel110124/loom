# ADR 0006 — Rust→Slang codegen for analytic fields

- **Date:** 2026-08-11
- **Status:** accepted
- **Decision touched:** new (build + test mechanism); implements LOOM-IMPLEMENTATION-ORDER.md S2

## Context

Wind, water and rain each independently specified an analytic field evaluated on both sides: the
CPU needs it because physics and gameplay read it inside the deterministic simulation, the GPU
needs it because vertices are displaced by it every frame at a density no readback could feed.

Written twice, that is the failure the implementation order calls "the highest-severity failure
mode in the whole backlog", and the reason is that it has no symptom. Nothing errors, nothing
validates, no hash changes — both implementations are individually deterministic and individually
plausible. Wind pushes a rigid body one way while the grass it stands in leans another, and the
only evidence is that the scene looks subtly wrong in motion. That is the one class of defect a
still PNG cannot catch (ADR 0005) and a determinism hash does not cover.

Built three times — once for wind, once for water, once for rain — it is three chances at it.

## Decision

**One expression tree, two backends.** A field is authored once in `loom_field` as an `Expr`:
`Const`, `X`, `Y`, `Z`, `T`, arithmetic, `Sin`, `Cos`, `Abs`, `Min`, `Max`. That tree has exactly
two consumers — `Expr::eval` walks it on the CPU, and `Expr::to_slang` prints it as Slang. Neither
side can implement a different formula, because there is only one formula.

`crates/loom_render/build.rs` calls the emitter and writes `assets/shaders/generated/fields.slang`
before compiling any shader. `scene.slang` includes it.

**Rejected: a proc macro over hand-written Rust.** Parsing a subset of Rust expressions into
shader source is more machinery, and its failure mode is worse — a Rust expression that parses but
means something different in Slang (integer division, operator precedence, `f32` vs `float`
promotion) compiles on both sides and diverges silently. An explicit `Expr` tree cannot express
what it cannot emit.

**Rejected: writing the Slang by hand and testing agreement.** That is the agreement test without
the codegen, and it leaves the structural divergence — the actual risk — entirely to review.

### Three traps, pinned

**Constants emit through `{:?}`, never `{}`.** `format!("{}", 1.0_f32)` gives `1`, and `1 / 2` in
Slang is *integer* division yielding zero. The Rust side would be right, the shader would be
silently wrong, and no test of `eval` could see it. `{:?}` always emits `1.0`.

**Expressions are fully parenthesised.** Emitting precedence-aware output means implementing
Slang's precedence table correctly, and the only symptom of getting it slightly wrong is a subtly
different field. The output is uglier and it is generated, so nobody reads it.

**Entry-point names are pinned with `-fvk-use-entrypoint-name`.** Slang renames a lone entry point
to `main` and preserves real names only when a module has several — so `scene.slang` kept
`vertexMain`/`fragmentMain` purely because it has two, and the first single-entry-point module did
not. The symptom is a pipeline that will not compile with the reason buried in a driver warning.

### The agreement test

Codegen removes structural divergence. What is left is floating point: a GPU `sin` is not libm's
`sin`, and a driver may contract `a * b + c` into an fma with different rounding. That residual is
measured, not assumed.

`loom_render`'s `field_agree` module dispatches a compute shader over 512 fixed `(position, time)`
samples and compares against `Expr::eval`. It runs in `cargo test`, so it is inside green check 3
with no new gate.

- **Measured worst absolute difference: 4.5e-5**, on a field whose components reach ~8.
- **Threshold: 1e-3.** Mutation-checked: perturbing `to_slang`'s constants by 0.1% while leaving
  `eval` alone — precisely the divergence this exists to catch — produces a difference of 0.278,
  about 6,000× the noise floor.
- The threshold is **per-field and must be re-measured** when a field grows. More operations
  accumulate more error, and a field with noise in it will not have this residual.
- The test asserts the GPU field is non-flat and takes >400 distinct values before comparing.
  Without that it would pass for free if the dispatch silently did nothing.

**Samples are generated once in Rust and uploaded**, not derived from the invocation index on each
side. Deriving them separately would be one fewer buffer and would reintroduce the exact
write-it-twice hazard inside the test built to catch it.

**The barrier does not belong to the render graph.** never-do #4 gives the graph ownership of
*frame* barriers, and the graph models images; this is a one-shot buffer dispatch submitted and
waited on outside any frame, following the precedent `raytrace.rs` set and `material.rs`
documents. The `HOST_READ` barrier after the dispatch is required: a fence says the work finished,
not that its writes are visible to the host.

### Noise is implemented here, not imported

S2 asks to "pin the noise implementation used by both sides and treat its output as ABI." **No
field uses noise yet** — the wind field is deliberately pure sinusoids — so there is nothing to
pin today. The policy for when there is:

Noise goes into `loom_field` as an `Expr` node with the implementation written out, **not taken
from a noise crate**. Then there is no version to bump: the CPU eval and the emitted Slang are
both generated from the one implementation, and changing it is a visible edit to this repository
that fails the determinism hashes loudly rather than silently. A crate dependency would put the
sim hash at the mercy of someone else's patch release.

### The generated file is committed

`assets/shaders/generated/fields.slang` is tracked rather than ignored. It costs a few lines of
diff noise and buys the ability to see, in review, exactly what changed on the GPU side. The
header says not to edit it, and `build.rs` overwrites it — but only when the contents actually
differ, because an unconditional write touches the mtime and makes every shader recompile forever.

## Consequences

- Adding a field means adding it to `loom_field::all()`. Both sides follow.
- The `Expr` vocabulary is deliberately small. Extending it is a real change with a test, which is
  the intended friction — every node is something the emitter must get right in two languages.
- The agreement test needs a Vulkan device and skips without one, like the other device tests.
- Phase 1 will want the GPU to sample wind for real. That wants a frame-graph pass writing a
  texture, not a host readback, so `field_agree` is a harness rather than the start of it.

## Human approval

Not required — this adds a build step and a test rather than changing a locked decision. It does
touch `build.rs` and shader compilation flags, both listed above.
