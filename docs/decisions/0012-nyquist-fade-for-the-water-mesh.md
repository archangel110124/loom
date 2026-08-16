# ADR 0012 — The water mesh fades a wave it cannot sample, and that is where drawn stops equalling sampled

- **Date:** 2026-08-13
- **Status:** accepted
- **Decision touched:** qualifies Phase 3's central property — that the surface
  drawn is the surface `loom_water::sample_water` returns, bit-identically, per
  ADR 0006's one-implementation rule and the `slang_agreement` test. It does not
  amend any row of CLAUDE.md's locked table.

## Context

W4 built the sea as concentric LOD rings: level `L` has cells of
`WATER_CELL · 2^L`, seven levels, 0.5 m under the camera and 32 m at 512 m out.
Every vertex calls `loom_sample_water` — the generated twin of the Rust function
buoyancy reads — so there is no second wave implementation to drift.

**A cell cannot represent a wave shorter than about twice its width.** At the far
rings the vertices land wherever the fixed world lattice happens to cut the wave,
and what rasterises is not the wave: it is a flat quad, or a stepped shelf, whose
height is an artifact of the sampling phase. The user's report was "visible
square/terraced facets", and a grazing-angle render of `shore` shows exactly
that — hard-edged diagonal creases and terraces across the mid field.

W4 measured the size of it and deferred the fix, in its own commit message:
dropping `ocean`'s two shortest waves (7.3 m and 4.7 m, 0.17 m of 1.26 m total
amplitude) took its flicker from **1.904 to 1.079**, so roughly 43% of the number
was waves the coarse rings under-sample. It named the lever — attenuate a wave as
its wavelength approaches the cell sampling it — and declined to pull it, because
doing so makes the drawn surface differ from `sample_water` at distance.

That is the tension this ADR exists to settle, and it is worth being precise
about it, because the deferral was right for the wrong reason. **The divergence
is not created by the fade.** At three samples per wavelength the drawn surface
already differs from `sample_water` by most of the wave's amplitude — and differs
*incoherently*, changing sign and shape as the wave travels through the lattice,
which is precisely the flicker being measured. The choice is not "agree or
diverge". It is between a divergence that is unbounded, unpredictable and
twinkling, and one that is monotone, bounded, and computable from first
principles.

## Decision

**Each wave's amplitude is scaled by a smooth weight of its wavelength against
the cell sampling it, in `waterVertexMain` and nowhere else.**

```slang
static const float WATER_FADE_WHOLE = 8.0;   // λ/cell at which a wave is whole
static const float WATER_FADE_GONE  = 3.0;   // and at which it is gone

float waterSamplingCell(float2 xz, float2 centre) {
    float2 offset = abs(xz - centre);
    return max(WATER_CELL, max(offset.x, offset.y) / (float(WATER_RES) * 0.5));
}
// per wave:
amplitude *= smoothstep(WATER_FADE_GONE, WATER_FADE_WHOLE, wavelength / cell);
```

Four properties make this the shape it is.

**1. It lives in the shader, not in `loom_water`.** The fade is a property of
*this mesh's sampling rate* and of nothing else — a different mesh, a compute
scatter, a CLI query or a buoyancy solver has a different one or none. So
`sample_water`, `slang()` and the generated `water.slang` are untouched, and
`slang_agreement` still passes unchanged, comparing the same two implementations
of the same whole sea. Putting the fade inside the shared function would have
made the CPU and GPU agree *about a faded surface*, which means buoyancy floating
a boat on a sea that flattens with distance from a camera — a rendering concept
reaching into the simulation, and non-deterministic besides.

**2. It is a function of position, never of level.** A vertex on a ring boundary
is emitted by both levels; W4 made the seam watertight by having both sides
evaluate the same function at the same `xz`. A weight read off `level` would
disagree across that seam and re-open it, and would step the amplitude at every
ring — trading facets for a visible concentric ridge. `waterSamplingCell` reads
only `xz` and the snapped centre, so the two sides cannot disagree, and
`smoothstep` is C¹ at both ends so nothing kinks.

**3. `r / 16`, clamped at `WATER_CELL`, and the direction of its error is
deliberate.** Level `L` covers Chebyshev radius `8·cell_L` to `16·cell_L` from
the centre, so `r/16` is the *exact* cell size at each ring's outer edge and
under-reads by at most 2× inside it. Under-reading means the shader believes
there are more samples per wavelength than there are, so it fades **less** than
warranted — the conservative direction for a change whose risk is removing real
detail. Clamped at `WATER_CELL` it is exact everywhere level 0 draws (`r ≤ 8 m`),
which is what makes the near field bit-identical: **no wave longer than
`WATER_FADE_WHOLE · WATER_CELL` = 4 m is touched at all inside 8 m.**

Measured from the **snapped** centre, not the eye. Against the eye, every
vertex's amplitude would move continuously as the camera moves — the shimmer
generator the 32 m snap exists to avoid. Against the centre a vertex's weight is
constant until the window jumps, which is the same moment its LOD level changes
anyway.

**4. Amplitude is the only thing scaled, and that is what keeps the shading
honest.** Every term in `loom_sample_water` reads `amplitude`: the vertical
offset, the horizontal Gerstner pinch, `ka` in the analytic normal, the orbital
velocity. Scaling it once, before the call, means the normal describes the
surface actually drawn rather than the one that would have been — a fade that
flattened the geometry and kept the old normals would light a smooth sea with
crests that are not there, which is how this change usually goes wrong. It also
cannot break the fold limit the validator proves: `Σ Q·k·A ≤ 1` is linear in `A`
and every weight is in `[0, 1]` — the same argument `shoal` already rests on.

### The constants

Both are **estimated** samples per wavelength, `λ / cell`; by property 3 the true
count is between half the estimate and the estimate.

- `WHOLE = 8`. A sine reconstructed piecewise-linearly at `N` samples per period
  misses its crest by `A·(1 − cos(π/N))` — 7.6% of amplitude at `N = 8`, 13% at
  6, 29% at 4. Eight is where facets stop reading as facets. In true samples this
  puts full amplitude at 4–8 per wavelength.
- `GONE = 3`. Nyquist is 2, and *at* 2 the sampled height depends entirely on
  where the lattice falls relative to the crest — the amplitude that survives is
  arbitrary and changes as the wave travels, which is the flicker. Three retires
  the wave just before it enters that region rather than inside it. In true
  samples, 1.5–3.

## Consequences

**The divergence, stated.** For `ocean` (five waves, `Σ A` = 1.26 m):

| distance | est. cell | true cell | max \|drawn − sampled\| |
| --- | --- | --- | --- |
| ≤ 9.4 m | 0.5 m | 0.5 m | **0.000 m** |
| 16 m | 1.0 m | 1.0 m | 0.050 m |
| 32 m | 2.0 m | 2.0 m | 0.265 m |
| 64 m | 4.0 m | 4.0 m | 0.776 m |
| 128 m | 8.0 m | 8.0 m | 1.256 m |
| ≥ 140 m | — | ≥ 16 m | 1.260 m (flat) |

It **first touches anything at 9.4 m** — where the 4.7 m wave begins to fade,
inside the second ring — and saturates at the sum of the amplitudes past about
140 m, where the sea is drawn dead flat. `shore` is the same shape: first touch
9.0 m, saturating at 1.23 m past 128 m. The saturation point is not arbitrary: at
140 m the true cell is 16 m, so even the 26 m swell is at 1.6 samples per
wavelength — below Nyquist, and therefore something the mesh was never drawing
correctly.

**Why that is acceptable.** Nothing reads the drawn surface. Buoyancy, drag,
submersion, `loom sim --assert`, `loom water --at` and the CLI all call the CPU
`sample_water`, which is unchanged and still returns the whole sea everywhere.
The cost is visual and one-directional: a floating body 140 m away rides the true
surface while the water under it is drawn flat, an error of up to 1.26 m
subtending 0.52° — nine pixels at 1600×900 through `ocean`'s 55° lens. The trade
is that the same
region stops crawling. **If that ever matters, the fix is more mesh, not less
fade**: the fade is keyed to the cell size, so halving `WATER_CELL` or adding a
ring pushes the whole curve outward with no constant changed.

**The far field loses real detail as well as fake detail, and the honest answer
to that is a normal map.** With everything shorter than the swell gone past
~60 m, the distant sea reads calmer than the near sea. That is the same gap
`loom_water::spectrum` already documents from the other end — a Pierson-Moskowitz
sea's chop lives in the spectral tail that sixteen equal-energy waves do not
carry, and a normal map is where that belongs. This ADR makes the case sharper
rather than changing it: **geometry carries the waves the mesh can sample, and
anything shorter has to arrive as shading.** Not built here.

**Measured effect.** `cargo xtask shimmer`, 12 frames, camera static, sim
advancing:

| scene | before | after |
| --- | --- | --- |
| ocean | 1.904 | **1.644** (−14%) |
| shore | 2.189 | **1.713** (−22%) |
| underwater | 2.530 | **2.484** (−2%) |

No non-water scene moved by a thousandth. `ocean` does not reach W4's 1.079
control, and should not: that control deleted the short waves *everywhere*,
including the near field where they are correctly sampled and are most of what
makes the surface read as water. This keeps them where they are real.
`underwater` barely moves because it looks up through the surface from below,
where almost everything in frame is near field.

**Three references move, and only the three water scenes.** `ocean`, `shore` and
`underwater`, re-blessed deliberately; every other golden image is bit-identical,
which is the check that this touched the water path and nothing else. The
determinism hash is unchanged at `b478ea4ac2622d32` — the fade is in a vertex
shader and the simulation cannot see it.

**Cost: it is faster, and the reason is worth knowing.** The fade adds a copy of
the wave set and up to sixteen `smoothstep`s per vertex over 43,008 vertices, and
`LOOM_GPU_TIMING=1` on `ocean` at 1920×1080 says the forward pass went from
**0.106 ms to 0.061 ms** (medians of eight frames, tight in both). The vertex
work is real but invisible next to what it removes: a far field of crinkled,
steeply-tilted quads is a far field of overdraw and near-grazing fragments, and
flattening it deletes that shading. This is not a reason to do it — but it does
mean the answer to "is the fade worth its cost" is that there is no cost to
weigh.

**The golden-image gate is the regression test.** There is deliberately no unit
test of the weight: writing one in Rust would mean a second implementation of a
shader constant, which is the exact failure this project spends `loom_field` and
`loom_water::slang()` avoiding. What a regression here looks like is the near
field changing, and `cargo xtask image` compares the whole authored-camera frame
of all three water scenes against a reference.

## Human approval

Not required by the letter of CLAUDE.md — no locked row changes, and
`sample_water` is untouched. Recorded as an ADR anyway because "the surface drawn
is the surface sampled" is the property the whole of Phase 3 was built to
protect, and this is the first thing to qualify it. If the qualification is not
wanted, deleting `waterSamplingCell` and the loop above the `loom_sample_water`
call restores the previous behaviour exactly and re-blesses three references.
