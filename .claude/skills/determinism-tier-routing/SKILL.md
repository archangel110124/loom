---
name: determinism-tier-routing
description: Use before building anything in Loom that moves, holds state, produces a force, or becomes readable by loom sim --assert or rhai — a VFX effect, a solver, a compute pass, a probe, a scene component. Decides which side of the determinism line it goes on, and what gates follow from that.
---

# Which tier does this belong to?

CLAUDE.md carries ADR 0045's clauses. This is the routing decision, its gate
consequences, and how to make the answer stick. **Answer it before writing
code** — the expensive mistake is not writing nondeterministic code, it is
building the right mechanism on the wrong side of the line and discovering it at
review.

Read the current authority before routing: **ADR 0045**, then **ADR 0053**
(which *amends* 0045 clauses 1–2 and introduces the cinematic tier), then
**ADR 0059** (the tier's budget). Newest applicable wins.

## The routing question

Ask, in this order:

1. **Does it produce a force on a rapier body, or is it readable by
   `loom sim --assert` / a rhai script?**
   → **Deterministic tier. CPU.** Either closed-form at a point (the
   `sample_water` shape) or state stepped inside the fixed step. No GPU float
   crosses back.

2. **Neither?** → **Presentation.** GPU-stateful is admissible under ADR 0045
   clause 3, gated by `cargo xtask repeat`. It may not be read by an assertion
   or a script — by construction, not as a gap to close.

3. **It genuinely needs both a GPU solver and forces?** → **Cinematic tier**,
   ADR 0053. Per-effect, authored in the scene text
   (`simulation = "cinematic"`), defaults off, and costs a device→host sync per
   tick.

## Gate consequences of each answer

| | Deterministic | Presentation | Cinematic |
| --- | --- | --- | --- |
| Sim hash / pinned-hash tests | yes, re-pin deliberately in the same commit | no | **barred** |
| `DETERMINISM_SCENES` (debug vs release) | yes | n/a | **refused at the gate, by name** |
| `cargo xtask repeat` | yes | yes — this is its licence | yes, and it is the **only** reproducibility gate it gets |
| Readable by `--assert` / rhai | yes | no | fails loudly, naming the ADR |
| Reproducible across machines | yes | yes | **no** — same device, driver and dispatch order only |

`xtask/src/main.rs` refuses a cinematic scene in `DETERMINISM_SCENES` with a
message quoting ADR 0053 §3, because *"a cinematic body is a GPU dispatch
neither profile computes, so it would agree for a reason that has nothing to do
with the property, and the check would go on printing a pass while measuring
nothing."* That refusal is the shape of the whole failure class.

## Three standing traps

- **A force-producing grid anchors to sim state, never to the camera.** The VFX
  report's §2.2a prescribed a camera-centred domain; for a grid that produces
  forces that makes physics depend on where the viewer stands. Refused in ADR
  0045 and restated harder in ADR 0053 §5.
- **A readback is not a shortcut past the line.** The report's milestone 4
  prescribed reading a height field back for buoyancy, which was already built
  correctly on CPU pontoons. Refused as `OVERNIGHT-DECISIONS.md` D1 and by name
  in ADR 0045 §2.
- **The tier does not leak.** A deterministic body forced by a cinematic one is
  outside the guarantee. `GameRules` beside cinematic water is refused at load
  unless the scene sets `acknowledge_nondeterminism = true`.

## Make the answer a refusal, not a paragraph

This is the part that actually holds. `crates/loom_scene/src/scene.rs` (~lines
880–1005) is the worked example: `cinematic_water_needs_an_extent`,
`second_cinematic_water_body`, `cinematic_domain_over_budget`,
`cinematic_water_demotes_a_game` — each a load-time error, **with the required
number in the message**.

ADR 0059 §2 says why explicitly: the tier's scope *"stops being a paragraph in a
companion document that a scene author never reads."*

**If a constraint can be a load-time refusal, the ADR says so and the commit
ships it.** An ADR that ends in "authors should be careful" has not finished.

## Precedence, because routing questions arrive as briefs

CLAUDE.md > `docs/decisions/` (newest applicable) > the brief > companion docs
(ADR 0002). **A companion doc's recipe is a hypothesis about this engine, not
an instruction** — the two refusals above are both companion-doc recipes that
were wrong on contact.

For the byte-identity half once you have routed to presentation or cinematic,
see `gpu-state-byte-identity`.
