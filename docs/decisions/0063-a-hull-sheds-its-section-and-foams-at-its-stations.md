# ADR 0063 — A hull sheds its section, and foams at its stations

- **Date:** 2026-08-20
- **Status:** **proposed** — needs human approval before this branch merges.
- **Governed by:** ADR 0045 (clause 1: everything here that reaches a force is
  CPU, deterministic and inside the fixed step; clause 2: nothing here reads
  back).
- **Extends:** ADR 0056 (the interactive surface, whose shed construction this
  corrects) and ADR 0055 (the CPU foam field, whose hull source this
  re-grains).
- **Decision touched:** none of CLAUDE.md's locked decisions. Deterministic-tier
  water throughout; `float_cinematic` is deliberately untouched.

## Context — nothing had ever moved a floating body

Every scene in this repository that floats something either drops it, drifts it,
or stands people on it. `wake` and `pool` drop a body vertically. `river` and
`water_crate` let one be carried. `jib_vi_float`, `jib_vi_drift`,
`jib_vi_walk`, `jib_vi_stations` and `fishing` all hold a nineteen-metre boat
still and ask questions about stability, carry and paint.

So the two systems that exist to make a boat *look* like a boat under way — the
shed wake of ADR 0056 and the hull foam of ADR 0055 — had never been asked for
a real answer. Both were written against, and tuned on, a 0.7 m crate. Both are
wrong on a 19 m hull, in ways that only a scene driving one can show.

`assets/test/jib_vi_underway.loom` is that scene, and it exists because these
two defects cannot be seen without it.

## Decision 1 — the shed source is a section, not a waterplane

`loom_cli::play` computed a shed packet as `swept_volume(waterplane_radius,
|v|)`, and `swept_volume` is `π r² · |v| · Δt`. Its own docstring called this
"a waterplane sweeping sideways", and that sentence is where the defect lives.

On `jib_vi_float.loom` the waterplane radius — the furthest pontoon's distance
from the body origin, plus its radius — is **8.8145 m**. Squared and swept at
6 m/s, that is **97.63 m³ of displaced water per packet**, against a hull whose
total displacement is **43.78 m³**. Sampled at the boat's own twelve pontoons,
the exact points `buoyancy::solve` reads to decide which way is up, the field it
emits reaches **−8.34 m at 6 m/s and −11.55 m at 8 m/s**. The boat digs a
trench and then floats in it.

**Havelock's source for a body moving horizontally is its immersed section
sweeping forward**, `V_disp / L_wl`, once per length travelled. The waterplane
belongs to a body heaving *vertically*. `loom_water::wavelet::shed_source`
replaces the call site and reads three quantities off the pontoons the buoyancy
solver has already filled in — nothing new is authored, nothing is passed twice:

- **The mean immersed section**, `submerged · Σ(4/3)πρ³ / L_wl`. For this hull
  1.02 m³ per packet at 6 m/s, and **0.19 m at its own pontoons**.
- **The speed through the water**, not over the ground. `flow` was already on
  every pontoon and this path threw it away, so a crate drifting perfectly with
  a river shed a full wake — and the faster the river, the bigger the wake it
  made by doing nothing.
- **`σ` is the half-beam across travel**, not the reach along it. `W_s =
  exp(−k*²σ²/4)` means a source cannot radiate waves much shorter than itself,
  so `λ_min ≈ πσ`. The shipped `σ = 8.81 m` puts the floor at **27.7 m**, while
  this hull's transverse wake at 6 m/s is `2πU²/g` = 23 m and its diverging
  waves are far shorter. The shipped source deleted the wake band by
  construction. The beam gives `σ = 3.04 m`, `λ_min = 9.6 m`.

`swept_volume` survives for the **impact** path, where it is right: a body
entering the water vertically really does present its waterplane. Its docstring
now says so, and says what it cost.

**Two of the four pinned water hashes moved** — `river` and `water_crate`, both
of which travel horizontally through water. `wake` and `pool` did not, because
they drop a body vertically and have never reached `SHED_MIN_SPEED` sideways.
Since the volume moved by two orders of magnitude, an unchanged hash is proof
the path never fired rather than a lucky escape, and that is why only two moved
where four were predicted.

## Decision 2 — granularity is chosen per consumer, and justified by the consumer

The shipped code applied **one** granularity to three consumers that want
different ones. This ADR sets them separately, each with its reason:

| consumer | granularity | why |
| --- | --- | --- |
| buoyancy | every pontoon | already so; a crest under one end and not the other is a torque, and that is the whole feature |
| the shed wake | the **body** | a waterline is one waterline. Twelve packets from twelve points is twelve wakes behind one boat, and would evict a 128-slot ring in ten ticks |
| foam | every **station** | see below |
| the splash crown and impact ring | the **body** | a body enters vertically and presents its waterplane; a 19 m hull slamming down must not throw a 1.09 m ring |

The old comment defending one foam hull per body — "a per-pontoon version would
lay the same foam three times and mean nothing different" — is **true of a 0.7 m
crate**, whose pontoons are closer together than one 0.5 m foam cell, and false
of a boat. Measured on `jib_vi_float.loom`:

    water@0,0.foam = 0.01881403848528862
    water@0,3.foam = 0.01881403848528862
    water@8,0.foam = 0.01881403848528862

identical to sixteen digits, eight metres apart. A hull-shaped thing laid a
244 m² circle. Twelve stations of 1.093 m are 45 m², so this deposits **less**,
and deposits it where the hull is.

**Strength is `u · n̂`, the station's speed along its own outward waterline
normal**, never negative. A bow station's normal points into the direction of
travel and opens water; an amidships flank is parallel to it and scores nearly
zero; a stern closes water and clamps to zero. The bow moustache, the dark hull
side and the trailing wake are one dot product. What appears astern is what
`foam::velocity_at` dragged back from the bow, which is how
`a_moving_hull_leaves_foam_behind_it` already worked.

`Hull::opening` is a new field rather than a reinterpretation of `velocity`,
because the two answer different questions: `velocity` is what the hull drags
the surface along with, and advection needs the whole vector.

**Stations are visited in authored pontoon order and never sorted.** A sort on
a float key is stable within a run, so `cargo xtask repeat` would pass while the
order silently depended on geometry.

**`float_cinematic` is not changed.** It already deposits per station with a
shear strength from the solver's own readback, so stations there would deposit
twice; its whole-body `Hull` exists for the advection drag and now spells
`opening` out to keep that behaviour byte-identical.

Measured on `jib_vi_underway.loom` at 1200 ticks — a trail astern and not abeam
of it, which one disc cannot express:

    water@3,0.foam = 0.0440      2.4 m astern, on the track
    water@3,8.foam = 2.1e-24     8 m abeam of the same hull

The old disc reaches 8.81 m and covers both points.

**No hash moves.** `Physics::state_hash` folds translation, rotation, linvel and
angvel and nothing else, so foam reaches no rapier body. Verified, not assumed.

Cost, `jib_vi_float.loom` at 1200 ticks, measured by temporarily restoring the
single hull: **0.420 s → 0.654 s** (machine load 3.02 and 0.61 respectively, so
if anything flattering to the old number). 1.56×, not the order of magnitude a
per-cell twelve-fold loop suggests, because `foam::velocity_at` already rejects
on squared distance in four flops. No bounding-box reject was added; it would be
optimising 0.2 ms a tick.

## Decision 3 — the CPU foam field and the shader's contact band are two things

`waterFragmentMain` now draws a foam band wherever a solid comes through the
surface, from the geometric gap between the water and whatever the depth buffer
says is behind it. That band and `water@x,z.foam` are **different quantities and
must never be unified**:

- the CPU field has memory, advects, decays, and is readable by `loom sim
  --assert` and by `rhai`;
- the band is a per-pixel shading term with no state, unreadable by anything,
  and camera-dependent by construction.

"Unify these, they're both foam" is exactly the tidying that lands six months
later and silently makes an assertion depend on where the camera is. It is
written down here so the answer is on the record before the question is asked.

**The band reads a new `gap`, not the existing `column`, and the reason is
worth keeping.** The obvious quantity is the optical path `waterBehind` already
returns for the sub-surface wrap lobe. It does not work: `column` is only ever
written when `bedDepth < WATER_DEPTH_SENTINEL`, so it stays at the sentinel on
every scene with no voxel volume. That is *correct* for an absorption path — it
is what stops `ocean`'s submerged posts telling the wrap lobe they stand in a
hundred metres of water — and it makes `column` unusable for a waterline,
because the open sea is where every boat is.

## Decision 4 — `Propulsion` is one field, and the second one is always wrong

A thrust in newtons in the body's own frame, applied once per fixed step. On the
force path and therefore deterministic, in-step, and readable through `Node.x`
like any other consequence of a force.

- **No `max_speed`.** Terminal speed already falls out of this force against
  `Buoyancy::damp_quadratic`, which every floating hull here authors. Two knobs
  for one number means one of them is a lie at any given moment.
- **No `heading`.** Steering is a script writing this vector.
- **Free by absence and by value.** `[0,0,0]` is dropped at load rather than
  applied, so `assets/prefabs/jib_vi.loom` can carry an inert one purely to give
  an instance something to override — a prefab instance may only deviate through
  `[node.overrides]`, and an override needs a target.

**A limitation this exposes, recorded rather than fixed.** `jib_vi_underway`
needs **500 kN** to reach 1.8 m/s. Damping in `loom_water::buoyancy` is
`displaced · (damp_quadratic·v|v| + damp_linear·v)` on all three axes with one
coefficient, so at 1 m/s this hull meets 219 kN fore-and-aft — the number chosen
to stop a pontoon gaining amplitude in *heave*, spent on *surge*, where a real
hull of this size slips through the water an order of magnitude more easily.
Anisotropic hull resistance is a change to the force path and belongs in its own
ADR. Until then the scene pays the honest price and says so in its header.

## Decision 5 — rejected on evidence: slope variance does not become roughness

The plan this work was built from proposed transferring the slope variance that
the detail fade retires into the shading roughness — widening
`tracedEnvironment`'s lobe first and the sun's second — with an acceptance of
`loom salt` on `ocean.loom`'s mid-field band falling from 848 to under 550.

**Measured, and it is refused.** All on `ocean.loom`, 1280×800, `--sim 240`,
`loom salt --rect 0,340,1280,60` (water between about 22 m and 67 m):

    baseline                                       848
    WATER_REFLECT_ROUGH 0.06 -> 0.40 (7x wider)    847
    WATER_ROUGHNESS 0.08 -> 0.30 (the sun lobe)    883
    capillary detail normal removed entirely       475
    WATER_DETAIL_RANGE 70 -> 40                    564
    the footprint fade below                      1060

The first row kills the proposal outright: a nearly seven-fold widening of the
reflection lobe moves the metric by **one count**. The second says the sun lobe
makes it *worse*. The third finds the actual mechanism — the capillary detail
normal is **44% of the mid-field salt on its own**, which also explains why
flattening the sky and switching off the wind each recover a similar fraction,
since both break the same chain.

**The mechanism is a high-frequency shading normal steering each pixel's
reflection to a different part of a high-contrast sky.** That is why no lobe
width can help: widening blurs each sample without making neighbouring pixels
agree. A surface whose normal varies faster than the eye can resolve has to be
*retired*, not blurred.

**And the resolution-correct fade is also refused, for now.** Replacing the
metre-denominated `WATER_DETAIL_RANGE` with a screen-footprint fade is the
principled change — a distance in metres is the wrong instrument for a
screen-space problem, and `FIRE_NYQUIST_LOW`'s docstring already cites this very
constant as the next example of the defect. It was built, analytically rather
than with `fwidth` (the shoreline `discard` a few hundred lines above leaves
undefined derivatives in exactly the quads `shore` and `river` exist to
protect), and it **measures 1060 against 848**. It preserves detail from 22 m to
45 m where the old linear ramp had already attenuated it, and in this band salt
scales monotonically with how much capillary detail is present. It would also
move every water reference PNG in a gate that is already failing.

So: no roughness transfer, and no fade change in this branch.

**What would change this.** A commit that can bless references and is willing to
argue for a slightly glassier near field can take `WATER_DETAIL_RANGE = 40` for
a measured 848 → 564, one constant, and should read the `MANIFEST.txt` diff line
by line. Anyone attempting the footprint fade again should tune its two
thresholds against `loom salt` rather than deriving them from Nyquist: the
measured optimum is about three times stricter, because the artifact is
reflection steering rather than aliasing.

## Consequences

- `loom_water::wavelet::shed_source` is the one place a hull's wake source is
  computed, with `a_hull_does_not_dig_a_hole_under_itself` and
  `a_body_drifting_with_the_current_sheds_nothing` pinning both halves.
- `river` and `water_crate` are re-pinned; `wake` and `pool` demonstrably never
  fired the path.
- Every water reference PNG showing a solid at the waterline moves, from the
  contact band. `ocean.loom` moves 1006 pixels of 1,024,000, worst channel 63 —
  inside the image gate's own tolerance of 1024 and 72.
- `plough.loom`'s acceptance was stale and is corrected here, because whoever
  touches foam owns it: it asserted `water@0.5,0.foam > 0.4` and read 0.0, not
  for want of foam but because the hull now rests at x = −1.17, leaving the
  sampled point ahead of the whole trail.
- `jib_vi_underway.loom` is in `SCENES` and **not** in `GOLDEN`. It earns a
  reference — a hull-shaped foam trail and a waterline band are rendering paths
  nothing else covers — and the row should be added and blessed in one
  deliberate commit.

## What would change this

- **A hull whose resistance knows which way it is pointing.** It would drop
  `jib_vi_underway`'s thrust by an order of magnitude and is the single largest
  piece of unrealism this work exposed.
- **A second body moving apart from the first**, which is what forces the
  question of re-centring the 64 m foam domain. Re-centring on a *hull* is legal
  — a body is sim state, not the camera — but needs an answer for which body.
- **A wake that visibly truncates**, which is the trigger for `MAX_EVENTS`
  128 → 256, priced by ADR 0056 at +0.088 ms on `ocean.loom` at 1080p. Measure
  before, not after.
- **A bow/stern dipole shed.** Measured against the corrected monopole it buys
  nothing safety-wise and doubles the event rate; it is a picture claim, and the
  trigger is a flythrough showing a symmetric bulge where a bow wave belongs.
