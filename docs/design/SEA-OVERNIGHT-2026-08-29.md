# The sea rebuild — one night's work

*29 August 2026, overnight, branch `sea/rebuild`. Written for the human who asked for it and
went to bed.*

**Read `.superpowers/sdd/*/progress.md` alongside this.** This file says what exists; those
ledgers say every decision taken without you, with the number behind it.

---

## What was asked for, and where it stands

| | asked | state |
|---|---|---|
| 1 | waves that don't look uniform | **done** — a 49,152-component FFT ocean |
| 2 | perfect the sea foam | **done** — the sea breaks on its own for the first time |
| 3 | waves that build and break when the wind is high | **done offshore** — shoaling surf is not started |
| 4 | rework how the boat floats | **done** — and it surfaced a controller defect that is not water |
| + | sea acoustics | **done** — did not exist at all before |

---

## The headline numbers

**The sea is twenty feet, measured three ways.** The engine's own full-tile figure is
`Hs = 6.100 m`. Point sampling reads 4.708 m over a 448 m domain and 5.613 m over 4 km —
both low, and progressively less so, which is the signature of domain-limited sampling
against a swell cascade carrying 2048 m wavelengths. The full-tile number is the true one.

**The derived sea breaks on its own**, which no sea in this repository had ever done.
Before, only `whitecaps` foamed, and only because a human hand-authored five waves at
18 m/s; `ocean`'s authored seven reach `mu_max` 0.175 and `shore` 0.059, both under the
0.22 threshold. The FFT sea reaches **0.3069 at a crest with foam coverage 0.886**.

**The dual spectrum is what made it break.** At *matched* `Hs`, with the threshold
untouched:

    swell alone       mu_max mean 0.0508   past 0.45: 0.000% (0 of 262,088)   past 0.22: 1.000%
    swell + wind sea  mu_max mean 0.0827   past 0.45: 0.133%                  past 0.22: 9.902%

The mechanism is that `mu_max` goes as `k·A`, so a wind sea a fifth the height contributes
**more** steepness than the swell it rides on. Height and steepness are different things,
and the twenty-foot target bought height.

**A crossing sea vindicated a decision made before there was a sea to test it.**
`WaterSample::mu_max` is the largest eigenvalue of the horizontal Jacobian rather than its
trace, argued years of commits ago. Measured now, with an aligned control row — essential,
because a `cos^p` spread makes the trace over-report on every row:

    crossing (swell 60 deg off wind)   eigenvalue 0.1007%   trace 0.8800%   8.7x apart
    aligned control                    eigenvalue 0.1766%   trace 0.9129%   5.2x apart

**89% of the whitecaps a trace-driven foam would paint sit where neither axis has folded.**

**The boat was a quarter light.** True displacement is 57.636 m³, proven apex-independent —
a closed surface's tetrahedron sum is invariant under the apex, and `jib_vi_wide.obj`
spreads 0.00000 m³ over apexes 10 km apart where the material-set union spreads 612.86. Mass
was 43,776 kg. It is now 57,636, and the sixteen re-solved pontoons reproduce the hull
station by station at 57.6360 m³ / 67.2306 m².

**The sea has a sound**, and a gale differs from a swell in colour rather than volume: at
fixed `Hs`, taking the breaking fraction 0 → 0.2 moves tilt **0.077 → 0.279** with the level
unchanged.

---

## What is not done

- **Shoaling and surf.** `SEA-REBUILD.md` §3's bake, §4's breaking limiter and §5's curl
  sheet are unbuilt. The ocean breaks offshore; it does not yet rear and pitch on a beach.
- **The boat's centre of mass.** `loom_physics::add_box_body` hangs the whole mass on one
  node-centred box, so her true stations trim her −8.78° and the pontoon set is shifted
  2.2994 m forward to cancel it. The honest fix is `RigidBody.center_of_mass`, which does
  not exist.
- **The antifoul stripe** sits 3.9 cm below the waterline she now floats at. A mesh edit,
  deferred because it touches two shipped art meshes and re-blesses every hull reference.
- **Spray on a spectrum body throws nothing** — it samples at particle birth time.
  `Sim::new` warns.
- **`rain_sim.slang` still collides drops against the Gerstner surface.** No scene rains on
  a spectrum body yet.

## What is broken, and it is not the water

**36 `scripts/green.sh` §6 rows fail**, where 0 failed before — `rig_drive` ×4, `rig_trip`
×1, `deeper_demo` ×31. **Every one is a character walking on a deck. Not one is a water
assertion.**

Measured, so this is not a guess: `Rig/Boat.bob` is 0.032 m, the same 3.2 cm the old hull
was documented at, so the sea did not get rougher. The `BoxCollider` is unchanged at
`[9.53, 1.66, 3.30]`, so the walking surface did not move. She floats 3.4 mm **lower**, not
higher. What moved is `Rig/Player.local_y` — 1.678 flat before, 1.7254 by tick 900. **The
player drifts 4.7 cm up in the boat's own frame and off his mat**, `at_helm` goes to 0, and
with no hand on the throttle she coasts.

**And the diagnostic that settles what it is:** scaling the helm's thrust by the mass ratio
made it *worse*, not better — it broke a workspace test that was green, by unseating the
helmsman sooner. That commit is reverted. So he is not being shaken off by heave; **he is
being unseated by surge acceleration**, and pushing harder unseats him sooner.

That reduces the whole cluster to one question, and it is ADR 0060's — *"a character stands
in the frame of what it stands on"* — under linear acceleration. A correct buoyancy fix
surfaced it because a heavier boat accelerates differently for a given thrust, but **the
fragility is in the controller, not the hull.** It wants a scene built for it and a human to
scope it, not a guess at four in the morning.

## Owed to you, none of it doable by an agent

1. **Three hunks to paste**, all into `crates/loom_cli/src/run.rs` and one into `play.rs` —
   two for the viewer's ocean upload (§7 of `.superpowers/sdd/SEA-FFT-WIRE-PLAN/task-3-report.md`),
   one for the sea audio (§5 of `.superpowers/sdd/SEA-SOUND-PLAN/task-2-report.md`). The
   ocean pair must land in one commit or clippy fails on dead code. **Until they land, a
   window draws a flat sea while the physics runs the real one, and no gate can see that** —
   every gate goes through the headless path, which is fully wired.
2. **`scripts/green.sh` wants its check-6 line**, `cargo xtask ablate`, in the file's own
   bare-command style inside the xtask `if` block.
3. **Every bless.** `cargo xtask image` reports 60 differing — 54 are the tonemap backlog
   that predates all of this and that `docs/rebless-the-tonemap.md` says must not be blessed
   by anyone who has not read it. `ocean_fft` and `jib_vi_underway` are among the new ones.
4. **The 36 rows above**, and whether the answer is new tapes or a controller that tolerates
   an accelerating deck. It is your call which.
5. **The audio constants.** `SWELL_HZ`, `SURGE_DEPTH` and `SURGE_SECONDS` were chosen by ear
   by something that cannot hear, and are flagged in-file as exactly that. Wave files are in
   `target/sea-audio/`.

## Defects found in code nobody was auditing

Eleven, none of them the thing being worked on at the time:

1. `add_water` uploaded sixteen wind-derived Gerstner waves for a spectrum body — a `--sim 0`
   render would have drawn a sea the physics had never heard of.
2. A pointer to a function-local array: compiles for Slang's C++ target, invalid SPIR-V.
3. The Nyquist row and column broke the antisymmetry the code documented as guaranteed —
   2n−1 cells, a 5.9% imaginary residue silently discarded by `.re`.
4. Cascades double-counted the spectrum: 6.07 / 8.60 / 10.56 m of `Hs` for one, two and
   three cascades of the same sea.
5. A fftshift-centred spectrum left the field modulated by Nyquist — a per-cell sign flip
   that **passed all six of its plan's tests**, because a sign flip does not change variance.
6. `rain::measure`'s tilt is not interleaving-safe and its doc says it is: the same rumble
   reads 0.922 interleaved against 0.077 mono, so the recorded rain bed's printed tilt is
   inflated.
7. `TypeRegistry::validate` is **one level deep** — nested tables get no type, enum or range
   check at all, and `flow.speed`'s range has never been enforced.
8. `cargo xtask ablate` returned success when it could not obtain a GPU, i.e. asserted "every
   effect is drawing" having rendered nothing.
9. …and again by a different route: a failed render left the previous run's PNG in place for
   `compare` to score.
10. `scripts/green.sh` runs `clippy --all-targets` where `CLAUDE.md` documented the weaker
    form, which is how a lint violation survived a commit and two reviews.
11. `rigkit.verify_obj` checks positive volume and never closure, so no mesh export in this
    repository has ever asserted watertightness.

## The lesson this branch keeps teaching

**Nine false comments were found, three of them introduced by fixes to other false comments**,
and several were numbers quoted from a configuration that had been replaced. Four were mine.

The tests were worse. The FFT core's own plan found seven bugs and **its tests caught none of
them** — every one was caught by a reviewer or by a test an implementer added on its own
initiative. A whole-plan review put it plainly: *every test in these three files is a
self-comparison, an inequality, or a statistical band; not one pins a number the arithmetic
would move.* Golden pins were added, and under a one-ULP injection **all 130 other tests
passed**.

So the rule this branch earned, three times over: **a test that has not been seen to fail is
not yet a test**, and when the question is what this engine does, run this engine — a formula
that shares a name with the code is a hypothesis about the code, not a description of it.
