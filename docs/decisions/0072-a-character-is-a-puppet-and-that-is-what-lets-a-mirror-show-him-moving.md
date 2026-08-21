# ADR 0072 — A character is a puppet, and that is what lets a mirror show him moving

- **Date:** 2026-08-21
- **Status:** **accepted** and built. `assets/prefabs/deckhand.loom`,
  `assets/test/deckhand_walk.loom`, `assets/test/deckhand_mirror.loom`,
  eleven `assets/scripts/deckhand_*.rhai`, and the glass in
  `assets/games/deeper_demo.loom`.
- **Depends on:** ADR 0019 (one traced reflection ray, no roughness cutoff)
  and ADR 0062 (a deformed surface fires its rays from the rest pose).
- **Sits under:** ADR 0045. Everything here is render-side: node scripts write
  transforms, no mesh leaf of a character is in the collision world, and the
  determinism hash covers rapier bodies only.
- **Adds:** no engine code, no pass, no component, no trait, no schema field.
  Two asset conventions and a gate row.

## The decision

**A character in Loom is separate rigid parts on a node hierarchy, animated by
`rhai` scripts writing absolute rotations on the fixed step.** Not a skinned
mesh, not a `Deform`. And **a mirror is a `box` with `metallic = 1.0,
roughness = 0.0`** — two nodes, no planar pass, no second camera, no mirrored
projection matrix.

The two halves are one decision, and that is the part worth writing down.

## Why the puppet

The proximate reason is that Loom has no skeletal animation: no rig format, no
skinning, no bones, and `loom_asset` iterates `document.meshes()` and never
`skins()` or `animations()`. Adding it touches the asset loader, the scene
schema and the vertex shader.

**The real reason is ADR 0062:85.** Every secondary ray in this engine —
reflection, shadow, RTAO — intersects the acceleration structure, and the TLAS
is built from meshes at their authored transforms. A `Deform`ed or skinned
surface is *displaced in the vertex shader*, so every ray sees it at its rest
pose: its shadow does not move, its reflection does not move, and it does not
occlude where it appears to be.

So a skinned character in a mirror would stand still while his body walked.
**An articulated puppet is not the compromise available in the absence of
skinning; it is the only animation method in this engine that a reflection can
show moving.** `assets/test/deckhand_mirror.loom` is that claim as a picture:
the reflection walks because the parts really moved.

The cost is real and should be stated. Rigid parts cannot deform, so a shoulder
is a ball joint and not a shoulder; the rungs above a formula are morph targets,
not skinning (ADR 0062:107). The look this buys — a wooden artist's mannequin, a
Lego figure, Rain World, Fall Guys — is a legitimate stylistic target rather
than a fallback, and for a co-op horror game it is arguably the better one.

## Why the mirror needs nothing

`scene.slang` already calls `tracedEnvironment` for every opaque fragment — ADR
0019 measured a roughness cutoff on it and **deleted** it, because
picture-per-millisecond was flat across every band — and mixes the result in as
`environment * f0 * (1 - roughness * 0.7)`. At `metallic = 1.0` the `f0` **is**
the albedo, so a pale metal at roughness 0 returns about 95% of one traced ray's
radiance. A flat mirror's specular lobe is a delta function, which one ray
samples exactly. Measured on the demo's glass, high-frequency detail carried
against the direct view:

    roughness  0.00  0.05  0.10  0.20  0.35  0.55
    detail      97%   90%   85%   63%   43%   25%

There is nothing for a planar pass to add and it would cost a second camera, a
second scene traversal and a mirrored projection.

## The four rules that follow, and each one cost something

**1. It faces inward.** `build_instances` filters the TLAS to meshes. Water,
grass, rain, fire, smoke and every particle are generated from `SV_VertexID` and
are not in the acceleration structure at all, so a mirror pointed at the sea
reflects a hole where the sea is. Point it at built geometry.

**2. The frame goes BEHIND the glass.** A border drawn in front is not a border,
it is a lid: the panel reads as a flat dark rectangle with a ghost in it, and
every reasonable next move then measures the frame. In `deeper_demo` the frame's
front face is at z 2.570 against the glass at 2.550, and the border comes from
the frame being larger in x and y. Getting this backwards cost an hour and
invalidated a sweep of four lamp positions and four intensities.

**3. A reflected hit is shaded diffuse-only and has no reflection of its own.**
So a character meant to read in a mirror is built out of one bright feature and
one secondary against near-black — here a sou'wester and two mittens on black
oilskin. Two facing mirrors give one bounce, then flat albedo.

**4. A mirror is only a feature if the player finds it.** The demo's glass is on
the shed's north face, 1.7 m from the spawn point — and the player spawns facing
away from it, because the scene is turned so the berth is on −Z and neither an
authored eye yaw nor an authored character yaw can express "start facing this
way". No headless render and no gate row in this repository has ever contained
it without a patched copy. One line of the HUD tape he is already reading fixes
that for nothing.

## The animation convention

**A `GameRules` script owns the clock and every knob; node scripts own one joint
each.** A node `Script` has no memory (`ScriptHost::tick` hands it a *copy* of
`state` and takes back three vectors) and its `position` is a local offset, so a
joint can see neither the phase nor how fast the character is moving. Both live
in the one script that has state and world positions.

**Phase advances with distance, and the constant is derived, not chosen.** This
is where the first version of the deckhand went wrong under a comment saying it
could not: distance-driven phase makes the skate *speed-independent*, not zero.
Zero requires the sole to travel backwards under the body at exactly the body's
speed, which pins

    stride = pi / reach

where `reach` is the fore-aft distance the foot can cover. A 0.720 m leg
swinging ±30° reaches 0.720 m, so 4.36 rad/m. At 2.30 the clock demanded a
1.37 m step from a 0.72 m leg and 46% of the body's travel left as slip.

**And the swing is not a sine.** A sine is fastest in mid-stance and stationary
at each end, so it matches the body's speed at one instant per step. The angle
falls linearly through stance and returns on a cosine through the swing. With
the stance knee straight — a bent knee swings the shin forward under a planted
sole — that is 46.3% skate down to 5.0%.

**Every rotation channel is authored at zero.** `play.rs` reads a node's live
transform, ticks the script and writes the result back, so a shared script must
write absolute values and there is no rest pose to add them to. Joint identity
comes from `sign(position[0])`, the one channel nothing writes.

**Amplitudes and rates live in one block, in plain language.** `loom_script`
runs with `set_max_modules(0)`, so a rhai file cannot include another, and a
scene gets one `GameRules` — the block is duplicated verbatim and a test
(`the_two_copies_of_the_deckhand_knobs_agree`) fails when the copies drift.

## What can see any of this

**Almost nothing, and that is a property of the design rather than a gap.** The
determinism hash covers rapier bodies; a character's mesh leaves are kept out of
the collision world by `character_ancestor`, so zeroing a joint law leaves
`Root/Walker` bit-identical at every tick. `loom sim --assert` and `rhai` read
node world positions, which is the one seam that works — and `scripts/green.sh`
§6 now uses it to pin the stance foot's world x across a stride. The band was
chosen by putting each defect back and measuring what it read.

Everything else is a picture. **Adding a character means adding a scene to
`SCENES` and a `GOLDEN` row**, or the gate reports a full pass without ever
having looked at it.

## What was rejected

- **Skinning.** Large, and ADR 0062 says the rays would still see the rest pose,
  so it would buy a worse mirror than the puppet has.
- **Parenting the held rod to the hand.** Five reviews asked for it. It takes
  the rod out of the first-person frame — a grip at chest height is 76° below
  the view axis against a half-FOV of 37.5 — which is the thing the rod-in-hand
  round existed to fix. The grip pose and the rod's view-model offset were moved
  toward each other instead, to 99 mm apart.
- **A planar reflection pass.** See above. One ray already carries 97%.
- **A lamp on the mirror.** Anything bright enough to lift a man 1.7 m away also
  lights the deck he is standing on and the glass itself, so it raises the
  panel's mean and not its contrast.
- **A modelled face.** The void under the brim is the tone. What the hat needed
  was to stop being an empty bell, which is a dark plug inside it, not a
  portrait.
