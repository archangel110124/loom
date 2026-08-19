---
name: loom-vulkan-resource-safety
description: Use when creating or resizing a Vulkan buffer or image in Loom, adding a render-graph pass or changing what a pass records, writing to a host-visible mapping, changing the order objects are created or dropped, or on any validation message or a crash inside the driver.
---

# The Vulkan bugs no validation layer will tell you about

CLAUDE.md says the validation layers are the real compiler and lists the
never-dos. This is the procedure for the classes that **compile clean, validate
clean, and still corrupt or crash.**

## Class 1 — drop order (no message at all; the driver segfaults)

`e40a6b1`: closing the editor window segfaulted inside the NVIDIA driver *every
time*, freezing the displays until the kernel reaped the process. **Six**
lifetime bugs in one teardown:

- `Option<(Instance, Device)>` drops `.0` first, so `vkDestroyInstance` ran
  before `vkDestroyDevice`.
- The surface outlived its X11 window.
- One `rendered` semaphore shared by every swapchain image — aborted in the
  driver on roughly one close in three.
- Semaphores destroyed before the swapchain that still referenced them.
- egui objects destroyed with its draws still recorded.
- `set_meshes` freeing buffers a recorded command buffer referenced.

The headless path *"got this right by accident, holding them as two locals."*
**Every SAFETY note involved asserted the opposite of what happened.**

Rules: never let a tuple or a struct field order carry Vulkan ownership; one
semaphore per swapchain image; destroy the swapchain before its semaphores;
never free a resource a recorded command buffer references.

## Class 2 — a size that exists twice

`fbef119`: `fluid_zero` filled `Allocation::size()` — gpu-allocator's **padded
suballocation size** — instead of the size `vkCreateBuffer` was given. Debug
aborts on `VUID-vkCmdFillBuffer-size-00027`; **release writes 8 bytes past the
buffer, silently.** Latent in every buffer whose length is not a multiple of
the memory type's alignment.

Fix pattern: `vk::WHOLE_SIZE` — **delete the second copy of the length rather
than correct it.** Never `Allocation::size()` as a buffer length.

## Class 3 — a host write with no bound check

`21ba0a7`: the windowed path wrote the per-object array with no bound check.
Because `gpu-allocator` hands out slices of a shared host-visible block, the
overflow lands on a **neighbouring buffer**: *"geometry corrupts, the GPU reads
nonsense indices, and no validation layer says a word, because it is a plain
host memcpy."*

Fix pattern: one guard at the choke point every writer routes through, and grow
by doubling rather than refuse.

## Class 4 — declared access ≠ recorded command

`573cbca`: `fluid_zero` declared `BufferAccess::ComputeReadWrite` but records
`vkCmdFillBuffer`, which is a **transfer** write. The graph emitted the next
barrier with a source mask that did not cover what actually happened — latent
until a second transfer touched the same buffer, then a plain
`SYNC-HAZARD-WRITE-AFTER-WRITE`.

**The rule that generalises the whole file: what a pass declares to the graph
must match what it records.**

| Recorded | Declare |
| --- | --- |
| `vkCmdFillBuffer`, `vkCmdCopyBuffer` (dst) | `TransferDst` |
| `vkCmdCopyBuffer` (src), readback copies | `TransferSrc` |
| compute shader reads and writes | `ComputeReadWrite` |
| compute shader reads only | `ComputeRead` |
| vertex shader reads the buffer | `VertexRead` |
| `VkDrawIndirectCommand` source | `IndirectRead` |

(`crates/loom_render_graph/src/lib.rs`, `enum BufferAccess`. Barriers live in
the graph — never-do #4, and it covers buffers as well as images.)

Assert it: `RenderGraph::plan_full()` returns `(Vec<Transition>,
Vec<BufferTransition>)` and the barrier-list tests in that file name every
transition. **Mutation-check the test** — delete one `(id, Access)` pair and
confirm the test fails (`a82e34d`).

## Class 5 — the sample count of a pipeline

A pipeline's rasterisation sample count must match its attachment. Getting it
wrong is four validation errors, not a visual bug. Every pipeline builder here
takes its sample count as a parameter for that reason.

## Where a buffer lives

`MemoryLocation::GpuOnly` for anything a shader touches more than trivially.
`CpuToGpu`/`GpuToCpu` only for a **staging twin** with a `vkCmdCopyBuffer`
between. Host-visible memory is addressable from a shader and it *works*, which
is the trap: `573cbca` had half a million `InterlockedAdd`s going out as bus
transactions and it looked exactly like a slow kernel. Device-local + a host
twin + a copy took `fluid_draw` from 26.6 → 11.4 ms.

Never call `vkAllocateMemory` — `gpu-allocator` only (never-do #3).

## Procedure when something is wrong

1. **Turn sync validation on and *print* the messages**, not just collect them.
   A validation message only a test can see is no use while chasing a teardown
   crash in a live window — finding the six drop-order bugs took switching that
   on.
2. Run the **debug** binary over the affected scenes and require silence.
   Validation layers panic in debug by design; do not downgrade that to a log
   line.
3. `cargo xtask validate` does 30+ scene runs and demands zero messages — but
   it is the verifier's to run, not a builder's (`OVERNIGHT-DECISIONS.md` D4).
4. For a "no picture should move" fix, the verification pair this project uses:
   **validation-silent debug renders plus sha256-identical release renders**
   across the change, from two demonstrably different binaries (`fbef119`,
   `7f04a71`).

## Two standing reminders

- **Never write `ash` calls from memory** — read the vendored source
  (`ls ~/.cargo/registry/src/*/ash-*/src/`). Recalled shapes are confidently
  wrong.
- Name every object via `crates/loom_render/src/debug_names.rs`. An unnamed
  object makes a validation message unreadable, which is what makes it useless
  as agent feedback.
