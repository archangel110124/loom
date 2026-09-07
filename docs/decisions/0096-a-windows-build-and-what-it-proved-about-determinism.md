# ADR 0096 — A Windows build, and what it proved about determinism

- **Date:** 2026-09-07
- **Status:** **accepted** and built. `cargo xtask dist --windows`.
- **Amends:** ADR 0091 §1. Its determinism premise is true **within a build**
  and false **across platforms**, which was asserted and never checked.

## 1. The build was the easy half

`x86_64-pc-windows-gnu` compiles the entire workspace **with no source change
at all**: 42 crates, zero errors. ash loads `vulkan-1.dll` at runtime rather
than linking it, winit and cpal have Windows backends, and `build.rs` had
already turned the Slang into SPIR-V on the host, which is platform-independent.

Only linking needed anything: `rustup target add x86_64-pc-windows-gnu` and
`mingw-w64-gcc`. `loom.exe` is a 71 MB PE32+ binary that linked first try, and
`cargo xtask dist --windows` produces a 22.1 MB archive with a `play.bat`.

Under wine it validates `deeper_demo` correctly — 260 nodes, same version hash —
and runs the simulation.

## 2. The hard half, which nobody had checked

ADR 0091 says two machines stepping the same scene with the same inputs have the
same world, and builds deterministic lockstep on it. That is proven by
`cargo xtask repeat` — **three fresh processes of the same binary on one
machine**.

The Windows build gave the first chance to test it across platforms. It does not
hold.

| ticks | Linux vs Windows |
|---|---|
| 1, 2 | identical |
| 5 | `state_hash` differs |
| 300 | **37 values of simulation state differ**, ~1e-8 relative |

Both builds are internally deterministic — three wine runs and two Linux runs
each gave a stable hash — so this is a real difference between them, not noise.

**It starts identical and drifts.** That is the signature of the platform math
library: `f32::sin`, `cos` and friends are not required to be identically
rounded, glibc and mingw's are not, and every euler-to-matrix composition and
every wave evaluation goes through them. The drift is tiny per tick and
compounds.

## 3. What that means for co-op

**Cross-platform lockstep will desync.** A Linux host and a Windows client
exchange identical inputs and compute slightly different worlds; ADR 0091's hash
exchange would correctly report it, within seconds.

Same-platform co-op is unaffected — Linux↔Linux and Windows↔Windows each hold,
since both builds are internally deterministic.

So the Windows build is real and useful for **single-player and Windows↔Windows
co-op**, and **cross-play is not currently possible**. That is a decision for
the human, and the options are genuinely different sizes:

- Ship per-platform lobbies. Free, and dishonest only if the UI implies
  otherwise.
- Replace every transcendental on the determinism path with one implementation
  compiled into the binary — a software `sin`/`cos`/`exp` — so both platforms
  run the same instructions. Contained, testable, and the usual fix.
- Move cross-play off lockstep onto host-authoritative state replication, which
  is a different networking design and a much larger piece of work.

## 4. The caveat this measurement carries

**This was measured under wine, not on Windows.** Wine implements the Windows
API on Linux; the math may come from mingw's own libm or from wine's msvcrt, and
real Windows may differ again — possibly from *both*.

What is established is the class of the problem: this build depends on the
platform's math library and is therefore not cross-platform deterministic.
Whether real Windows drifts by the same amount is unmeasured, and would need a
Windows machine to answer. It does not change the conclusion, because "drifts
differently" and "drifts" have the same consequence for lockstep.

## 5. What ADR 0091 should have said

> Two machines **running the same build** that step the same scene with the same
> inputs have the same world.

The original sentence was true of everything that had been tested and was
asserted more broadly than the evidence. `xtask repeat` proves same-binary,
same-machine reproduction; nothing had ever compared two platforms, because
until this ADR there was only one.
