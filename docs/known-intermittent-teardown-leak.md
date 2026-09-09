# A teardown leak that fails `cargo xtask validate` about one run in four

**Status: open, not caused by any change in the editor work. Recorded 2026-09-09.**

## What is seen

`cargo xtask validate` fails with a Vulkan validation error at device
destruction:

```
render --sim assets/test/mood_deep.loom
  vulkan validation [VALIDATION ERROR] VUID-vkDestroyDevice-device-05137:
  vkDestroyDevice(): VkDevice 0x... has 42 leaked objects that have not been
  destroyed.
  VkBuffer 0x150000000015, VkBuffer 0x170000000017, ... (first 10 of 42)
```

## Why it is not new

The **identical** failure — same VUID, same object count of **42**, all
`VkBuffer` in the sample — was recorded on **2026-09-08** against a *different*
scene, `assets/test/gleamsprat.loom`, in a gate run that predates every change
in ADRs 0104–0108. Two observations, two different scenes, one signature.

It is not the scene. It is whichever scene happens to be running when it
happens.

## What is known

- **Intermittent, and only under the row's parallelism.** `cargo xtask validate
  --only mood_deep` passes; two subsequent full `cargo xtask validate` runs
  passed with "93 scene runs, zero validation messages". The failing run was the
  full `green.sh --all`, where other GPU rows share the machine.
- **The count is fixed at 42.** A random corruption would not leak the same
  number twice, on two scenes, a day apart. That points at one subsystem whose
  buffers are conditionally created and conditionally destroyed, with the two
  conditions able to disagree — rather than at a general ordering mistake in
  `Viewer::drop`, which is careful and documented.
- Both observations are `render --sim`, so the suspect set is what a *simulated*
  run allocates and a still one does not.

## Why it matters

The validation layers are the compiler for this part of the codebase — the gate
says so in as many words. A row that fails one run in four teaches everyone to
re-run it, and a gate nobody trusts is not a gate. This is worth chasing on its
own rather than as a footnote to a feature.

## How to chase it

1. Reproduce deliberately: run `cargo xtask validate` in a loop while another
   GPU row (`cargo xtask image`) runs beside it, and record which scene trips.
2. Raise `VK_LAYER_DUPLICATE_MESSAGE_LIMIT` so all 42 handles print rather than
   the first 10 — 42 buffers with a shared allocation pattern will name the
   subsystem outright.
3. Tag allocations with `VK_EXT_debug_utils` object names at creation, so the
   leak report says what leaked instead of a handle.
