# ADR 0015 — Clouds live in the sky shader, not in the particle system

- **Date:** 2026-08-14
- **Status:** **proposed**, and its *scope* is superseded by ADR 0016, which makes the clouds the
  source of the rain rather than a backdrop. **The technique decision below still stands** — sky
  shader, not particles, not volumetric — and 0016 treats this as a sub-decision of itself.
- **Decision touched:** adds a feature to the existing sky pass. Does **not** move a locked decision
  and does **not** add a post-process pass; that boundary stays where ADR 0010 put it.

## The ask

Clouds that read as real: greys and whites, varied shading, moving. Suggested implementation was the
particle system.

## Particles are the wrong tool, and this project has already written down why

ADR 0014 quotes Latta's *Building a Million-Particle System* on where particle cost actually lives:
**fill rate and overdraw dominate when particles are large and overlapping**, and the concern
*recedes* as particles get smaller. That is the sentence that made 131,072 raindrops cost 0.036 ms —
drops are tiny.

**Clouds are the opposite case in every respect.** They are large, they overlap heavily, and they
fill the sky. Using the citation that justified cheap rain to justify cloud billboards would be
reading it backwards.

There is a second, harder objection. `crates/loom_cli/src/particles.rs` says it plainly:

> additive blending needs no sort

The whole particle path is order-independent by construction. Clouds are **alpha**, not additive —
they occlude each other — so they would need depth sorting, which means per-frame CPU work and
popping whenever the order flips as the camera turns. That is precisely the cost the architecture was
built to avoid, spent on the one effect that pays it worst.

## Volumetric raymarching is the best-looking answer and is deferred

Nubis-class volumetric clouds — raymarching 3D noise per pixel — are what a modern engine ships and
they are genuinely better: parallax, clouds you can fly through, god rays, self-shadowing that is
real rather than approximated.

Deferred for two reasons. It is expensive in exactly the way this engine has no budget model for
yet, and it is a full-screen effect, so it would sit against ADR 0010's post-process boundary. If it
is ever wanted, it deserves its own ADR rather than arriving as a cloud detail.

## Decision: procedural clouds in the existing sky pass

`skyColor(dir)` already runs per pixel across the whole sky. Clouds become part of it.

Everything it needs already exists:

- `loom_field::noise` is **frozen ABI with a Slang twin already generated**, and `loom_field::fbm`
  exists. No new hash family, no new codegen path.
- **P1's wind gives the motion for free.** Advect the noise along the wind vector and the clouds
  drift the same way the grass leans and the rain falls. That coherence is most of what sells
  weather; clouds moving on their own schedule would read as a backdrop.
- No sorting, no overdraw, no new pass, no new pipeline, no post-process.
- Rendering-only, so outside the determinism hash by construction.

### The technique, concretely

1. For a view ray `dir` with `dir.y > 0`, project onto a horizontal cloud slab — the classic sky-plane
   projection, `dir.xz / dir.y`, which naturally compresses toward the horizon the way real cloud
   decks do.
2. Sample `fbm` at that point offset by `wind * t`. Two or three octaves is enough; the fourth is
   rarely visible against a sky gradient.
3. Map density through an authored **coverage** curve, so 0 is clear and 1 is overcast.
4. **Shade with one extra sample toward the sun.** If the noise is denser in the sun's direction, this
   patch is behind cloud and goes grey; if not, it is lit and goes white. That single second tap is
   what produces the greys-and-whites variation asked for — it is cheap fake self-shadowing and it is
   the highest realism-per-instruction trick available here.
5. Composite over the existing sky gradient, and let the existing fog do the horizon.

### The part that makes it a system rather than a backdrop

**Cloud cover should drive the light.** `Environment` already carries sun strength and colour, and
rain already carries an intensity. Couple them:

- coverage rises → sun strength falls, sun colour cools, ambient rises. Overcast light is dimmer,
  bluer and flatter, and getting that wrong is more noticeable than the clouds themselves.
- rain intensity → a floor under coverage. **Rain currently falls from a clear sky**, which is a
  bigger tell than having no clouds at all.

Verified for this ADR: **nothing in `loom_script`, `loom_ecs` or `play.rs` reads `sun_strength` or
`sun_direction`**, so this coupling is purely visual and cannot reach the determinism hash. If that
ever changes, this coupling has to be re-examined.

Authored coverage should remain overridable — a scene that wants a clear sky over heavy rain is
unusual but is the author's call.

## What this cannot do, stated so nobody assumes otherwise

- **No parallax.** Sky-dome clouds are infinitely far away. Moving the camera does not move them
  relative to each other.
- **You cannot fly through them**, or up past them.
- **No god rays or volumetric shadows.** They shade themselves and darken the sun; they cast nothing
  into the scene.
- **No cloud shadows on the ground** unless added separately, which is a different feature (a
  projected noise term in the lighting, and cheap — worth considering alongside).

For weather overhead, none of that is visible. For anything a camera approaches, all of it is.

## Cost

One extra `fbm` evaluation plus one sun-direction tap per sky pixel, in a pass that already runs.
Sky pixels are a fraction of the frame and the pass is currently trivial. Expect it to stay well
inside the 0.05–0.11 ms the whole forward pass costs today — but **measure it with
`LOOM_GPU_TIMING=1` rather than assuming**, because unlike the rain estimate this one has no
measurement behind it yet.

## How it gets gated

A scene in `SCENES` and `GOLDEN`, framed so the sky is a large fraction of the frame — and, per this
session's three separate failures of exactly this kind, **verify the golden actually fails when the
cloud term is stubbed**, and report the pixel percentage. A sky scene whose camera points at the
ground would pass forever.

## If it is rejected

The sky stays a gradient. Rain keeps falling from a clear sky, which is the current state and is
visibly wrong in any weather scene. Nothing else is blocked.
