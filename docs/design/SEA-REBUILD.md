# The sea rebuild — waves that shoal, break and crash, and a hull that floats

**Date:** 2026-08-28. **Status:** design, approved in conversation; needs the human's
review of this file before an implementation plan is written.
**Governed by:** ADR 0045 (the VFX determinism line — every clause of it survives this
work), ADR 0053 §5 (the cinematic tier, which this work does **not** use), ADR 0055
(the CPU foam field), ADR 0056 (wavelet events), ADR 0063 (hull section and stations,
still *proposed*), ADR 0069 (the sea gets worse the further out you go).
**Decisions touched:** none of `CLAUDE.md`'s locked list. Deterministic tier throughout.

---

## 1. What was asked for

Four things, in the human's words:

1. procedurally generated waves "so it doesn't look so uniform"
2. "perfect the sea foam"
3. "waves that actually build up kind of like how they do once when people go surfing…
   should happen when the wind is high" — escalated mid-design to **"giant crashing
   waves"**
4. "rework how the boat floats in the water"

Four decisions were taken against those, and they set the scope of everything below.

| question | answer |
|---|---|
| what is wrong with the float | **the waterline** — not bobbing, not wave response, not drift |
| where surf breaks | **both, in one pass** — one wave field from deep water to the beach |
| what is wrong with the foam | **a painted stripe**, and **too continuous** — not persistence, not the hull |
| how much calibration may move | **everything is fair game**, the berth included |
| how the crash is built | **the curl sheet everywhere**, on the deterministic tier — not the cinematic solver |

---

## 2. Evidence

Every number here was measured on `45b9e67`, release build, this machine. They are
recorded so that nobody has to derive them twice, and so that a later disagreement has
something to be a disagreement *with*.

### 2.1 The boat floats where its proxy says, not where its hull says

Volume below the designed waterline (`y = 0`), integrated from the shipped meshes by
clipping every triangle at the plane and summing tetrahedra:

    jib_vi_antifoul.obj    47.741 m^3
    jib_vi_hull_white.obj   9.689
    jib_vi_grime.obj        2.714
    jib_vi_metal.obj        0.196
    union                  60.340 m^3
    jib_vi_wide.obj        57.636 m^3   <- matches the figure in the scene's own header

**The two disagree by 2.70 m^3 and the generator has to settle which is right**, because
`mass` will be set from it. `jib_vi_wide.obj` is the older single hull and is watertight
on its own; the material set is what actually ships, and summing per-material shells
assumes their union is closed and non-overlapping — `grime` and `hull_white` are plausible
double-counts against `antifoul`. 60.34 is therefore an upper bound, not a measurement.
§6.1's generator computes this properly, on the shipped meshes, with a watertightness
check, and that number — not either of these — is the one `mass` is set from.

Against that, `mass = 43776` kg, which is **43.776 m^3** of displacement. The hull holds
somewhere between 57.64 and 60.34 m^3 at the line it is painted for; it is asked to weigh
43.78. **It has been 24% light since the beam changed.** `jib_vi_float.loom`'s header
records how: *"the wide hull
displaces 57.64 m^3 … a ratio of 1.152, so `mass` scales 38000 -> 43776"* — mass was
scaled by a volume **ratio** rather than set to the displacement.

Nothing errors, because the twelve pontoons are internally consistent:

    12 x (4/3)pi(1.093)^3 x 2/3  =  43.78 m^3

so the proxy holds the origin at `y ~ 0` exactly as designed, and the hull it stands in
is simply not consulted. Measured settling, `loom sim`:

    tick     0   y = +0.400   (authored drop)
    tick    60   y = +0.0196
    tick   300   y = +0.0324
    tick   600   y = +0.0241
    tick  1200   y = +0.0392

And the paint disagrees with both: `jib_vi_antifoul.obj` spans `y` **-1.560 to -0.060**,
so the antifouling — the visual waterline on any real hull — tops out **8.4 cm under the
water the boat floats at**. Boot-top belongs above the line, not below it.

### 2.2 The sea is small because the fetch says it is

`deeper_demo.loom`'s five-rung ladder, as measured and recorded in that file:

    stage        dread   wind   rain   Hs
    inshore       0.00    3.0    0.0   0.17 m
    underway      0.25    4.5    1.5   0.26 m
    outofsight    0.50    7.0    6.0   0.40 m
    grounds       0.80   11.5   16.0   0.66 m
    theedge       1.00   16.0   30.0   0.91 m

**Hs = 0.91 m at 16 m/s is not a bug in the spectrum — it is correct.** A fetch-limited
sea at `F = 15000 m` and `U10 = 16` gives `Hs ~ 1.0 m`; the engine produces 0.91. The sea
is small because 15 km of fetch is a sheltered coastal ground.

The ladder escalates the one knob Pierson–Moskowitz is provably scale-invariant in.
`spectrum.rs`'s own module docs say so: *"`Sum Q·k·A` is 0.327 at every wind speed there
is. Raising the wind zooms the sea; it never roughens it"* — and `deeper_demo.loom`
measures it directly, realised `mu_max` running **0.112 / 0.105 / 0.090 / 0.047** at wind
3 / 8 / 12 / 16. The sea gets *smoother* as the gale builds.

`fetch` is the unused knob, and it is welded to the berth: the file states that 15 000 and
`inshore`'s `wind_speed = 3.0` are one number in two places, and that moving either
re-times the boarding walk and fails thirty rows of `green.sh` §6.

### 2.3 The sea has no height limit — only a steepness one

`Sum Q·k·A <= 1` is enforced at load (`wave_steepness_exceeds_limit`) and is scale-free.
A 200 m swell carrying 14 m of amplitude is `k·A = 0.44` and validates clean. **Giant
waves are available today**; what is missing is a reason for the sea to be giant, and a
way for it to break.

### 2.4 Foam, measured

`whitecaps.loom` at 18 m/s, tick 400, at the origin:

    coverage 0.954    mu_max 0.3158    field 8.4e-45

Coverage is high and the persistent field is dead — but the human did not ask for
persistence, so the field stays as it is. The two defects named are structural:

- **the painted stripe.** The lace has two octaves and the second is gated
  `saturate(1 - |toEye| / WATER_FOAM_FINE_RANGE)` with `WATER_FOAM_FINE_RANGE = 40.0`.
  That constant's own comment is honest about why: *"its cross-streak lattice period is
  7.3 cm, and `loom_value_noise` is evaluated analytically with no mip chain, so past
  ~30 m that octave is far under a pixel and becomes a twinkle generator."* So the fix is
  not to raise it.
- **too continuous.** The break decision is `smoothstep(WATER_FOAM_WET = 0.22,
  WATER_FOAM_BREAK = 0.33, mu_max)` and nothing else. `mu_max` is near-constant along a
  crest, so the whole crest whitens as one unit.

And the foam is applied as `lerp(lit, foamLit, foam)` — an albedo blend. Aerated water is
a material, not a colour.

### 2.5 Three defects found by moving the camera on `shore.loom`

All three reproduce headlessly, so none of them is the desktop's CRT shader. All three are
invisible at the scene's authored camera, which is why they survived.

**A hard-edged square of shallow water.** The bed is a box, `center [24,1,24]`,
`half_extents [26,1,26]` — 52 m square, x/z from -2 to 50 — and outside it there is no bed
at all. The floor is visible through the water and ends on a straight line. The scene's
header worried about exactly this class and reasoned it away by saturating the two terms
that *read* depth (shoaling at 4 m, tint at 5 m, floor at 6 m clears both). Floor
**visibility** is not one of those terms.

**Stipple across the whole sea, and it is not foam.** Measured:

    at -40,-40   depth=none    mu_max 0.103   foam 0
    at  24,-10   depth=none    mu_max 0.189   foam 0
    at  24, 24   depth=-1.00   mu_max 0       foam 0
    at  24, 34   depth=+0.72   mu_max 0.044   foam 0
    at  24, 44   depth=+6.00   mu_max 0.040   foam 0

Foam coverage is zero everywhere and peak `mu_max` is 0.189 against a 0.22 threshold, so
the foam system is correctly drawing nothing. The dotted streaks are the capillary
detail's **specular** aliasing: `loom salt` reports worst-channel **32,328** isolated
pixels on that frame. MSAA cannot reach it — it anti-aliases geometry edges, not shading
that varies inside one fragment.

**The shoreline is a stair-stepped `discard` edge**, with no coverage blending, where
depth goes negative.

### 2.6 What the water mesh is

`waterVertexMain`, generated from `SV_VertexID` with no vertex or index buffer, a
camera-centred clipmap:

    WATER_RES     128        WATER_CELL   0.5 m
    WATER_LEVELS  6          WATER_HORIZON 50 km

Each vertex takes the **full Gerstner offset, horizontal as well as vertical**, which is
why the normal is analytic and cannot be finite-differenced. Waves fade as their
wavelength approaches the local cell — `WATER_FADE_WHOLE = 4.0`, `WATER_FADE_GONE = 2.0`
in cell units — so **the shortest wave that survives at level 0 is about 2 m**, and more
of the spectrum fades further out.

Two consequences carried into the design: shoaling *shortens* waves, so a shoaling wave
walks toward that floor exactly where it should be sharpest; and the clipmap is
**camera-centred**, which is correct for rendering and forbidden for anything on the force
path (ADR 0045: *"a force-producing CPU sim grid anchors to sim state, never to the
camera"*).

---

## 3. The wave field

**The fact that makes this tractable: `omega` is conserved as a wave shoals.** A wave
crossing onto a shelf keeps its period for ever; only `k`, its direction and its amplitude
change. So all time dependence stays one global constant per wave, and everything spatial
can be baked.

### 3.1 The table

`LoomWaveField` follows the shape `generated/water.slang` already ships for
`LoomHeightField`: `origin`, `spacing`, `side`, and a device-address pointer, read
bilinearly, written twice in Rust and Slang, compared numerically by the agreement test.

At load — and on a terrain dirty region, exactly as `GroundGrid` regenerates — the CPU
traces each of the <= 16 waves across the grid against the **static** bed:

- local `k` from `omega^2 = g·k·tanh(k·d)` — the wave shortens as it slows
- direction turned by the depth gradient — crests swing round to face the beach
- amplitude by Green's law times ray spreading — **the wave grows before it breaks**

Four floats per wave per cell: `dphi`, `dir.xz`, `amp`. At 16 waves on a 256^2 grid that
is **16.8 MB per water body with a bed**.

**Its barrier belongs to the render graph** (never-do #4, which covers buffers as well as
images since the rain drop buffer forced it). A CPU-written table read by `waterVertexMain`
needs the dependency declared through `BufferId`/`BufferAccess`/`pass_with`, and the
barrier-list test in `lib.rs` names it — the same discipline the drop buffer got, and for
the same reason: a missing dependency there draws *last frame's* table, which looks almost
right.

### 3.2 Why `dphi` and not `phi`

`dphi` is the deviation from the deep-water phase, so:

    phi = k0·(d0 · x) + dphi(x, z) - omega·t

It is small, smooth, interpolates cleanly, and is **exactly zero in deep water**, where
the expression collapses to today's, bit for bit.

### 3.3 No bed, no table, no change

`LoomHeightField.side == 0` means the scene has no terrain, and every open-ocean scene in
the repository is that case. The bake is bounded by the terrain's own extent and costs
nothing offshore. `ocean`, `whitecaps` and the fishing ground keep today's mathematics
exactly; the new physics appears only where there is a bed to shoal on.

### 3.4 The three clauses this buys back

`shoal()`'s docstring refuses amplification, refraction and wavelength shortening by name,
each for the same reason — the surface must stay an analytic function of `(x, z, t)`. The
bake keeps that property and moves the position dependence into a table that is itself a
pure function of the scene. **All three come back.** `shoal()` itself stays as the
deep-water fallback for scenes with no table.

### 3.5 The sea gets bigger by fetch, not by wind

ADR 0069 already makes the sea worse further out, through wind alone, on a constant fetch.
Fetch becomes a per-rung quantity on the same ladder: going offshore *is* more open water
upwind. At 16 m/s with 200 km of fetch, `Hs ~ 3.6 m` against today's 0.91.

The berth is unfrozen by decision, so `inshore` may move too — but every rung's `Hs` is to
be measured and recorded in the file the way the present five are, and `green.sh` §6's
boarding rows re-derived rather than preserved.

### 3.6 The target: twenty feet at `theedge`

**`Hs = 6.10 m` at the top rung.** That is the acceptance test the headline feature is
judged on, and it replaces "giant" as a word.

`theedge`'s present `wind_speed = 16` **cannot reach it at any fetch**: the
fully-developed ceiling at `U10 = 16` is `Hs = 0.22·U²/g = 5.74 m`. So the wind rises with
the fetch. The pair:

    U10 = 18 m/s,  fetch = 440 km   ->   Hs = 6.10 m

and it is *genuinely fetch-limited* — fully developed at 624 km — so the sea is **steeper
than Pierson–Moskowitz**, which is exactly the property that makes it break rather than
merely zoom. For reference, at `U10 = 18`: 200 km gives 4.11 m, 300 km gives 5.04 m,
624 km gives 7.26 m.

`Wind.speed` is **not** `U10`. `spectrum.rs` is explicit that the authored field is a
free-stream value roughly 10% above it; the rung is authored so that
`Wind::mean_speed_at(10.0) == 18.0`, converted properly rather than by the rule of thumb.

**Three consequences, stated so nobody is surprised by them:**

- **`Hmax ~ 1.86·Hs = 11.3 m`** over a long record. `Hs` is the mean of the highest third,
  so sets will throw waves getting on for twice the number that was asked for. If 11 m is
  too much, the lever is this rung's `Hs`, and it is one number.
- **`Hs` is 32% of the boat's length** — 6.10 m against 19.07 m LOA on a 1.65 m draft.
  That is survival weather, not heavy weather, and §6.4's no-capsize test becomes a
  genuinely hard test rather than a formality.
- **The steepness bound may bite.** A fetch-limited sea this size pushes `Sum Q·k·A`
  toward the validator's 1.0. That is *intended* — reaching the fold limit is what
  breaking is — but if `wave_steepness_exceeds_limit` refuses the derived set, the
  spectrum's band count or `Q` allocation is what gives, never the target.

`surf`'s bed follows from the same number: breaking a 6.10 m wave needs **7.8 m** of water
under it (`H <= 0.78·d`), which is why §7.2 asks for ~8 m and `shore.loom`'s 6 m will not
do.

---

## 4. Breaking, foam, and the specular

### 4.1 Breaking is a limiter

Green's-law amplification voids the validator's `Sum Q·k·A <= 1` guarantee, so amplitude
needs a physical cap rather than a clamp: the depth-limited breaker index **`H <= 0.78·d`**.
One number with a referent outside this file, the same species as `fetch`. Below it the
wave grows; at it, growth stops and the excess feeds foam and spray — which is what a
breaking wave physically does with that energy.

### 4.2 The crest leans, in closed form

A Gerstner wave is symmetric fore and aft; a shoaling one is not.

    phi' = phi + B·s·sin(phi)          B = local breaking parameter, 0 offshore

`dphi'/dx = (1 + B·s·cos phi)·dphi/dx`, so **the analytic normal survives** — which is
mandatory, because it cannot be finite-differenced here. `B = 0` in deep water leaves
offshore untouched.

### 4.3 Foam breaks in cells

The grass slope boundary had the identical defect and `CLAUDE.md` records the cure: *"a
cutoff on slope — at any threshold, softened by any amount — draws grass to a clean curve
… `coverage` now perturbs the **threshold** with low-frequency noise on world position."*

`WATER_FOAM_BREAK` gains the same wander — `loom_field::noise` on world position, drifting
with the wave, at tens of metres so a crest breaks in patches. And the same guard rail that
made it safe there: **the wander only ever raises the threshold**, so "a sea too gentle to
break foams nowhere" stays exactly true and the existing gate keeps its meaning.

### 4.4 Foam is a material with structure

Two changes, neither of which is "raise `WATER_FOAM_FINE_RANGE`":

- a **distance-aware octave ladder** — roll each octave off as it approaches a pixel and
  add a coarser one below, so structure survives at every range at whatever frequency that
  range can carry. Judged on `loom salt`, which can see stipple; not on flicker, which
  cannot tell "wider" from "less stable".
- foam drives **roughness and specular**, and the lace modulates **opacity**, not only
  albedo. The ageing path already knows this — *"a dispersing raft is holes: the water's
  own reflection comes back through it, which is the difference between foam ageing and
  foam dimming"* — and fresh foam gets the same treatment from the start.

### 4.5 Specular anti-aliasing

A normal-variance-to-roughness term, widening the specular lobe as the capillary detail
goes sub-pixel. Baseline to beat: **worst-channel salt 32,328** on `shore.loom` at
`--yaw 20 --pitch 60 --sim 300`, 1100x1100.

---

## 5. The curl sheet

**A second generated surface on the same terms as the first.** `SV_VertexID` only, no
vertex or index buffer, anchored to the **water body** and never to the camera.

Each invocation maps to a cell of a crest grid, evaluates the closed-form surface there,
and either:

- `B <= 1` — **emits a degenerate triangle**, discarded before rasterisation for free.
  The same trick the water clipmap's covered centre and the grass fade already use.
  Non-breaking water costs arithmetic and nothing else.
- `B > 1` — builds the lip: a sheet thrown forward along `break_dir`, rolling under
  through an arc that opens as `B` grows, sized by the local wave height and celerity.
  **Its base is the height field's own crest evaluated at the same cell**, so the seam is
  exact by construction rather than by tuning.

The height field underneath stays single-valued. Buoyancy, `--assert` and the sim hash are
untouched by the geometry.

**Whitewater and spray reuse what exists.** The lip's landing line deposits into the
ADR 0055 foam field, which has memory, so the impact leaves a raft that ages and drifts.
Its leading edge feeds `loom_water::spray`. No new foam system and no new particle system.

**Stated limitation.** The TLAS holds meshes only, so the lip will not appear in
reflections and cannot be hit by any ray — as is already true of water, grass and rain.

### 5.1 The CPU twin

The lip is a closed-form function of `(cell, t, wave set, table)`, so a CPU twin answers
"is this pontoon under a falling lip" as a deterministic function of scene and tick. That
is **ADR 0045 clause 1 satisfied, not bypassed**: CPU, inside the fixed step, no readback,
no GPU float crossing the line. A breaker puts a real downward impulse on the hull and
`loom sim --assert` can check that it happened.

Without the twin the boat would still be thrown, because the height field under a breaking
wave is genuinely steep and tall — but it would not feel the falling jet, and that is the
one place the sea would be lying to the physics.

---

## 6. The boat

### 6.1 One generator, three consumers

A tool in the idiom of `tools/mesh/rig/*` reads the `jib_vi_*.obj` set and computes the
hull's **sectional area curve**: at each station along X, immersed cross-section as a
function of waterline height. Out of that one table falls

- **displacement at any draft**, so `mass` is set *from* it and never scaled by a ratio,
- **per-station section**, so the immersion model tapers as the hull does instead of
  twelve identical `r = 1.093` spheres on a hull narrowing from ±1.951 to ±0.918,
- **the waterline where displacement equals mass**, which is where the antifoul belongs.

Three numbers that must agree, from one source, so they cannot drift apart again.

### 6.2 Stations, and the spheres stay

`Buoyancy.stations` becomes an alternative to `Buoyancy.pontoons`, generated rather than
authored. Crates, buoys and every other floating thing keep the sphere path, untouched and
bit-identical.

The sphere's virtue is deliberate and documented — the spherical cap's curvature is what
turns a bob into a resting position, where a linear ramp oscillates for ever. A real hull
section has that property **naturally**, because a V or U section widens as it rises, so
the nonlinearity survives the change on physical grounds rather than by imitation.

It also makes ADR 0063 exact rather than approximate: the wake already reads *"the mean
immersed section"* off the pontoons, and with real stations that stops being an estimate.

### 6.3 Two terms the breaking sea makes mandatory

**Added mass** — a hull heaving drags water with it, order 1x displacement vertically.
Without it the heave response is simply too fast, and `damp_linear = 4.0` is currently
absorbing that error by hand, tuned on flat water.

**Directional drag** — a hull resists sideways motion far more than fore-and-aft, and the
station table already knows the lateral and frontal areas. Both derived, neither authored.

### 6.4 Acceptance

1. the solver's displacement-vs-draft curve matches the mesh's own, computed independently
2. `|settled_y - boot_top| < 0.02 m` — the paint and the float agree
3. heave natural period matches `T = 2*pi*sqrt((m + m_add)/(rho·g·A_w))` from the table
4. a righting-arm sweep: `GZ` positive through the working range, and no capsize across a
   long run **in the new breaking sea** — the test `jib_vi_float` was built to be and has
   only ever been asked on flat water

---

## 7. Gates and verification

### 7.1 The failure mode is an invisible feature, not a broken one

Four times: `set_ripples` shipped with no caller and drew dead-flat water through a commit,
an ADR and a review; the W9 crown was registered and drawn nowhere; grass ran two slices
outside `GOLDEN`; `meadow` was missing while the gate reported seven matches.
`CLAUDE.md`: *"no gate in this project can detect an absent feature."*

**So every new visual is measured against a build with it removed**, the way the ripple grid
was (*"2.8% of pixels at tick 45, 26.6% at 200, 4.8% at 900"*). A curl sheet that renders
nothing scores zero against its own ablation and fails loudly, where a golden reference
records the absence and passes for ever.

### 7.2 New scenes, in both `SCENES` and `GOLDEN`

*"Adding a rendering path means adding a scene to `GOLDEN`, or the gate reports a full pass
without ever having looked at it."*

- `surf` — a bed deep enough to carry a real breaker. `H <= 0.78·d` puts a 6 m wave in
  ~8 m of water; `shore.loom`'s 6 m bed caps at 4.7 m.
- `refraction` — crests turning to face a beach, the clause `shoal()` refuses by name.
- `boarding_sea` — the boat taking one over the bow, where the CPU twin is checked.

### 7.3 The instrument for a break already exists; the ablation harness does not

**Corrected after checking.** An earlier draft of this section claimed the sequence
instrument was missing and had to be built. It is not: `loom render <scene> --frames 4
--spin 0 --step 15 --sim 300` was run on `whitecaps.loom` and emitted `w_0000..0003.png`
— camera fixed, simulation advancing, which is exactly the shape a break has to be judged
in. `--spin 0` is documented in the CLI's own help for precisely this. **Nothing to build;
it needs using, and S0 writes down how.**

What is genuinely missing is the **ablation harness**. Every ablation in this project's
history was done by hand, by making a throwaway build with the feature removed — which is
why it has only ever happened after somebody already suspected a problem. S0 makes it a
switch in the shipping binary and a task that sweeps it.

**And the gate inverts.** Every other check in this repository fails when a render
*differs*. The ablation check fails when it does not differ enough — a feature whose
removal changes nothing was never there. That inversion is the entire reason it can catch
the failure class four features have already shipped with.

The recipe, verified on `whitecaps.loom`:

    loom render <scene> --frames 8 --spin 0 --step 6 --sim <t> --size 960x600 --out <f>.png

`--spin 0` holds the camera and `--step` advances the simulation between frames, so the
subject moves and the viewer does not. Eight frames at six ticks is one second of a break
at 48 Hz. Judge the sequence; then judge the ablation number. Neither alone is enough —
the sequence says whether it looks right and the ablation says whether it is there.

### 7.4 Closed-form acceptance wherever it is available

Predictions, not opinions: heave period against `T = 2*pi*sqrt((m+m_add)/(rho·g·A_w))`;
displacement-vs-draft against the mesh; settled waterline against the boot top within
2 cm; specular salt against **32,328**; and each rung's `Hs` measured and written into the
scene file the way the present five are.

### 7.5 Three things that stay the human's

- **`cargo xtask image --bless` is a human act.** The diffs and the numbers get produced;
  the human reads them and accepts. Moving 50+ references at once makes that diff
  unreadable, which is the real argument for landing this in slices with a bless each.
- **Determinism hashes are re-pinned in the same commit that moves them**, deliberately.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's and uncommitted.**
  Neither is to be staged, stashed, moved or reverted. The berth is unfrozen, so §6's
  boarding rows will re-time; those numbers get re-derived and handed over, not edited in
  place.

---

## 8. Phasing

This is too large for one plan and it decomposes cleanly, because each phase leaves the
repository green and shows something on screen. Each gets its own implementation plan, its
own slice of `GOLDEN`, and its own bless.

| | phase | delivers | done when |
|---|---|---|---|
| **S0** | the ablation harness | `LOOM_ABLATE`, `cargo xtask ablate` — the sequence instrument already exists (§7.3) | removing a feature that is drawing scores a large difference; removing one that is not scores zero and **fails** |
| **S1** | fetch on the ladder | a genuinely big sea offshore, no new mechanism | each rung's `Hs` measured and written into the scene; §6 rows re-derived |
| **S2** | the boat | §6 in full — generator, stations, added mass, directional drag | §6.4's four acceptance tests, on today's sea |
| **S3** | the bake | §3 — shoaling, refraction, wavelength shortening | `refraction` scene: crests turn to face the beach; deep water bit-identical |
| **S4** | breaking | §4.1–4.2 — the limiter and the crest lean | `surf` scene: waves rear and pitch; `H <= 0.78·d` holds |
| **S5** | the curl sheet | §5 and the CPU twin | `boarding_sea`: a lip throws over, and the hull takes the impulse |
| **S6** | foam and specular | §4.3–4.5 | patchy crests; salt beaten against 32,328 |

**S2 before S3 deliberately.** The boat's acceptance tests are closed-form predictions and
they are far easier to trust on the sea that exists today than on one that is changing
underneath them. Fixing the hull first also means every later phase is judged with a boat
that floats where it is painted to.

**S0 first, for the reason §7.1 gives.** Building the ablation harness before there is
anything to ablate is the whole lesson of the four invisible features.

## 9. What is deliberately not built

- **The cinematic tier.** ADR 0053's solver would give genuinely chaotic overturning water
  and is refused here for four stated costs: machine-local reproducibility, `--assert`
  refusing inside the volume, one hero volume per scene on a budget (ADR 0059), and an open
  3–0 kick-back on its whitewater and spray quality dated 18 Aug. It stays available as a
  later phase behind its own ADR.
- **Foam persistence.** The field with memory measures 8.4e-45 in practice, but the human
  named the stripe and the continuity, not the memory. Not in scope.
- **Hull foam and the bow collar.** Named as a candidate defect and explicitly not chosen.
- **An FFT ocean.** Refused on evidence in OVERNIGHT-DECISIONS D16 and nothing here
  reopens it.

## 10. Risks

- **The bake's size and load time**, and its interaction with dirty-region terrain edits.
- **The wavelength floor.** Shoaling shortens waves toward `WATER_FADE_GONE`, which is 2 m
  at level 0 and larger further out. A 22 m swell coming in is fine; `shore.loom`'s 4.5 m
  wave is the one that would vanish. The bake must know about the fade.
- **The blast radius.** Fourteen references had water in shot at the last count and the
  berth is now unfrozen on top of that. Slices with individual blesses are the mitigation.
- **`shore.loom`'s bed ends on a cliff** (§2.5) and the bake will be traced across that
  discontinuity. The bed wants authoring deeper and wider regardless.

## 11. ADRs this work owes

Three, numbered from 0076, each written when its slice lands rather than up front:

1. the baked wave-ray field — a new mechanism on the force path, and the argument that it
   keeps ADR 0045's clauses rather than bending them
2. the curl sheet and its CPU twin — a second generated surface, and a force derived from
   a closed-form presentation object
3. station buoyancy — the sectional model, added mass and directional drag

## 12. Questions the human has answered

1. **Which branch.** `sea/rebuild`, cut from `rig/phase1-structure` at `45b9e67` — off
   `HEAD` rather than off the shared base, deliberately, because switching base would
   rewrite the working tree around the human's two uncommitted files. Rebasing onto
   `overnight/2026-08-11` later is a clean operation; doing it now was not.
2. **How giant is giant.** **Twenty feet — `Hs = 6.10 m` at `theedge`.** See §3.6 for the
   wind/fetch pair that reaches it and the three consequences of asking for it.
