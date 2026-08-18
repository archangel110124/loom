//! Spray thrown off a crest that is breaking — W5's first source.
//!
//! **The same quantity the whitecaps are painted from.** `WaterSample::fold` is
//! `Σ Q·k·A·sin φ`, undivided, so 1.0 is a cusp and the validator caps the sea
//! below it (see [`crate::WaterSample::fold`]). W2 renders foam by
//! `smoothstep(WATER_FOAM_WET, WATER_FOAM_BREAK, fold)` in the water shader;
//! this throws droplets from the *same* threshold, so the spray leaves the
//! crests that are already going white rather than from a second opinion about
//! where a wave is breaking.
//!
//! **Closed form, no state, and never read by anything.** A droplet is a
//! function of `(body, t, region)` — the cell it came from, the slot of time it
//! was born in, and a ballistic arc from there. Nothing accumulates, nothing is
//! read back, nothing produces a force, and no assertion can see it. That is
//! ADR 0045 clause 1 satisfied by not having any state to argue about, and it
//! is the same shape as `loom_rain::splashes`, deliberately.
//!
//! **The region follows the eye, and that is allowed here specifically.**
//! Spray is drawn and nothing else, so ADR 0045's trap clause — a *force*
//! grid must anchor to sim state, never the camera — does not apply. The eye is
//! how the population stays bounded on an unbounded ocean, exactly as
//! `loom_rain::splashes` bounds itself.

use crate::sample_water;
use loom_field::noise::hash;
use loom_scene::components::WaterBody;

/// Fold at which a crest starts throwing droplets.
///
/// **`WATER_FOAM_BREAK` in `scene.slang`, spelled again.** The two numbers are
/// one decision — where a crest is breaking — and if they drift the spray comes
/// off water that is not white and the foam appears where nothing sprays. Set
/// from σ(fold) = 0.189 on the seven-wave `ocean`, which makes this 1.75σ: the
/// steepest few percent of the surface, not every crest.
pub const SPRAY_BREAK: f32 = 0.33;

/// Metres per candidate cell. One crown per cell per [`SPRAY_PERIOD`] at most.
pub const SPRAY_CELL: f32 = 1.5;

/// Seconds between a cell's chances to throw.
pub const SPRAY_PERIOD: f32 = 0.5;

/// How long a droplet lives, in seconds.
///
/// Under [`SPRAY_PERIOD`] × 2, which is what lets the search below look at only
/// two slots: a droplet born three slots ago is already gone.
pub const SPRAY_LIFETIME: f32 = 0.9;

/// How far from the eye spray is thrown, in metres.
///
/// Beyond this a droplet is well under a pixel and costs a sample to decide
/// not to draw. The same argument `loom_rain::SPLASH_RANGE` makes.
pub const SPRAY_RANGE: f32 = 34.0;

/// Droplets per crown.
///
/// §2.2e: "a crown is a ring emitted with outward+upward velocity". Seven
/// rather than eight so the ring does not read as a square from above.
pub const SPRAY_CROWN: usize = 7;

/// Metres per second outward and upward at a fold of exactly 1.0 — the cusp.
///
/// Scaled by how far past [`SPRAY_BREAK`] the crest actually is, so a swell
/// that barely breaks lifts a puff and a storm crest throws.
const SPRAY_OUT: f32 = 1.9;
const SPRAY_UP: f32 = 3.4;

/// Gravity on a droplet. Plain `g` — a water droplet this size is ballistic
/// over a metre, and air drag on it is a term nobody can see.
const SPRAY_GRAVITY: f32 = crate::GRAVITY;

/// One droplet in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Droplet {
    pub position: [f32; 3],
    /// How fast it is going, m/s, right now.
    ///
    /// **The derivative of [`ballistic`] and not a second integration** — the
    /// position is a closed form over `age`, so this is too, and the two cannot
    /// drift apart. It is drawn with rather than simulated with: a droplet
    /// moving at 6 m/s is smeared across a shutter's worth of frame, which is
    /// what stops a crown reading as a string of beads.
    ///
    /// **Zero for the rising sheet.** A sheet is a surface, not a droplet, and
    /// a surface that streaks is a surface with holes in it.
    pub velocity: [f32; 3],
    /// How far through its life it is, in `[0, 1)`. Drives the fade.
    pub fraction: f32,
    /// How big this one is, as a multiple of what the visual says.
    ///
    /// **A band of a crown is one `fraction`, so without this every droplet in
    /// it is the same size to the pixel** — which is why a still of the crown
    /// reads as a string of manufactured beads rather than as torn water. It is
    /// per droplet rather than per band because the beads within a band are
    /// what line up.
    ///
    /// 1.0 everywhere it is not deliberately varied, so it is an exact no-op
    /// for the crest spray and the sheet.
    pub scale: f32,
    /// How opaque this one is, as a multiple of what the visual says.
    ///
    /// **The other half of the fingering, and it cannot be done with
    /// [`Self::scale`].** A crown rim is not a ring of equal beads: the
    /// Rayleigh–Plateau instability drains the sheet into a few dozen
    /// *ligaments* with thin water between them, so the ring is alternately
    /// thick and nearly empty. Size alone makes small beads; size and opacity
    /// together make ligaments with gaps, which is what stops the rim reading
    /// as a manufactured necklace.
    ///
    /// 1.0 everywhere it is not deliberately varied — an exact no-op for the
    /// crest spray.
    pub alpha: f32,
}

/// The largest `fold` this wave set can ever reach, anywhere, ever.
///
/// `fold` is `Σ Q·k·A·sin φ` and the phases are independent, so the ceiling is
/// `Σ Q·k·A` — reached only at a point where every wave crests together, which
/// a real sea approaches and never quite hits. Under [`SPRAY_BREAK`] no point
/// of this sea can break at any place or instant, so [`spray`] returns nothing
/// for the whole run however large `WaterBody::spray` is.
///
/// **This exists because that no-op is invisible.** Authoring `spray = 2.0` on
/// a gentle swell produces no droplet, no warning and no error, and the author
/// concludes the feature is broken. It was: every sea in this repository was
/// under the threshold when spray shipped, which is how the threshold came to
/// be measured at all.
///
/// Computed at full depth, which is the ceiling: `shoal` only ever *reduces*
/// an amplitude, so a shelving sea folds less than this and never more.
#[must_use]
pub fn peak_fold(body: &WaterBody) -> f32 {
    body.waves
        .waves
        .iter()
        .take(loom_scene::components::MAX_WAVES)
        .filter(|w| w.wavelength > 0.0)
        .map(|w| w.steepness * (std::f32::consts::TAU / w.wavelength) * w.amplitude)
        .sum()
}

/// Every droplet in the air around `eye` at time `t`.
///
/// `ground` answers the bed height under a point, for the same reason
/// [`sample_water`] takes one: this crate does not know what a voxel is. A
/// closure that returns [`loom_voxel::heightfield::NO_GROUND`]'s value — any
/// large negative — is an open sea.
///
/// Returned in cell order and never sorted: the order is what the renderer
/// draws in, and a different order is a different additive sum.
#[must_use]
pub fn spray(
    body: &WaterBody,
    eye: [f32; 3],
    t: f32,
    ground: &dyn Fn(f32, f32) -> f32,
) -> Vec<Droplet> {
    let mut out = Vec::new();
    // A mirror throws nothing, and a sea that authors no spray throws nothing —
    // which is what keeps every reference image of every sea unmoved. Asking
    // costs a grid of `sample_water` calls, so both are checked before the loop.
    if body.spray <= 0.0 || body.waves.waves.is_empty() || t < 0.0 {
        return out;
    }

    #[allow(clippy::cast_possible_truncation)]
    let cell_of = |v: f32| (v / SPRAY_CELL).floor() as i32;
    let (x0, x1) = (cell_of(eye[0] - SPRAY_RANGE), cell_of(eye[0] + SPRAY_RANGE));
    let (z0, z1) = (cell_of(eye[2] - SPRAY_RANGE), cell_of(eye[2] + SPRAY_RANGE));
    #[allow(clippy::cast_possible_truncation)]
    let slot_now = (t / SPRAY_PERIOD).floor() as i32;

    for iz in z0..=z1 {
        for ix in x0..=x1 {
            // Two slots is the whole history: a droplet outlives its slot by
            // less than one more.
            for slot in (slot_now - 1)..=slot_now {
                crown_in(body, [ix, iz], slot, eye, t, ground, &mut out);
            }
        }
    }
    out
}

/// One cell's crown for one slot of time, if it threw one.
fn crown_in(
    body: &WaterBody,
    cell: [i32; 2],
    slot: i32,
    eye: [f32; 3],
    t: f32,
    ground: &dyn Fn(f32, f32) -> f32,
    out: &mut Vec<Droplet>,
) {
    #[allow(clippy::cast_sign_loss)]
    let seed = hash(
        hash(hash(cell[0] as u32).wrapping_add(cell[1] as u32)).wrapping_add(slot as u32),
    );
    #[allow(clippy::cast_precision_loss)]
    let unit = |shift: u32| ((hash(seed ^ shift) >> 8) as f32) * (1.0 / 16_777_216.0);

    // Born somewhere inside its cell and somewhere inside its slot, so crowns
    // do not appear on a lattice on a metronome.
    #[allow(clippy::cast_precision_loss)]
    let born = (slot as f32 + unit(1)) * SPRAY_PERIOD;
    let age = t - born;
    if !(0.0..SPRAY_LIFETIME).contains(&age) {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let at = [
        (cell[0] as f32 + unit(2)) * SPRAY_CELL,
        (cell[1] as f32 + unit(3)) * SPRAY_CELL,
    ];
    // Round, not square: the cell grid is axis-aligned and a square region
    // around the eye puts spray 1.4× further away on the diagonals, which is
    // where it is thinnest and least worth the samples.
    if (at[0] - eye[0]).hypot(at[1] - eye[2]) > SPRAY_RANGE {
        return;
    }

    // **Sampled at the moment of birth, not now.** The crown is thrown by the
    // crest that was there when it left; evaluating the surface at `t` would
    // make a droplet's arc depend on water it is no longer touching.
    let surface = sample_water(body, at, born, ground(at[0], at[1]), [0.0; 3], [0.0; 3]);
    if surface.fold <= SPRAY_BREAK {
        return;
    }
    // Dry land throws nothing: `sample_water` flattens the waves to nothing at
    // the shoreline, but the fold of a wave set is not zero at zero amplitude
    // until `shoal` has taken every one of them, and a crown standing on a
    // beach is the artifact.
    if surface.depth <= 0.0 {
        return;
    }
    let strength = ((surface.fold - SPRAY_BREAK) / (1.0 - SPRAY_BREAK)).clamp(0.0, 1.0);
    // Not every breaking crest throws. Without this the whole steep half of a
    // storm sea sprays at once, which reads as a fog rather than as spray.
    //
    // **The author's multiplier is on the population, not on the throw.** A
    // droplet's arc is what the crest that threw it can do; how many crests
    // throw is a look. Scaling the velocity instead would put spray in the air
    // that the surface underneath it cannot account for.
    if unit(4) > strength * body.spray {
        return;
    }

    // The crest's own velocity carries the crown downwind — the orbital motion
    // `sample_water` already computed, not a second wind term.
    let drift = [surface.velocity[0], surface.velocity[2]];
    let base = [
        at[0] + surface.displacement[0],
        surface.height,
        at[1] + surface.displacement[2],
    ];
    let spin = unit(5) * std::f32::consts::TAU;
    for i in 0..SPRAY_CROWN {
        #[allow(clippy::cast_precision_loss)]
        let angle = spin + std::f32::consts::TAU * (i as f32) / (SPRAY_CROWN as f32);
        let outward = SPRAY_OUT * strength;
        let up = SPRAY_UP * strength;
        let v = [
            drift[0] + angle.cos() * outward,
            up,
            drift[1] + angle.sin() * outward,
        ];
        out.push(Droplet {
            position: ballistic(base, v, age),
            velocity: ballistic_velocity(v, age),
            fraction: age / SPRAY_LIFETIME,
            scale: 1.0,
            alpha: 1.0,
        });
    }
}

/// How fast a droplet launched at `v` is going after `age` seconds.
///
/// The exact derivative of [`ballistic`] — one closed form differentiated,
/// never a second integration that could drift away from the first.
fn ballistic_velocity(v: [f32; 3], age: f32) -> [f32; 3] {
    [v[0], SPRAY_GRAVITY.mul_add(-age, v[1]), v[2]]
}

/// Where a droplet launched at `v` from `base` is after `age` seconds.
///
/// The one line both crowns share. Plain `g` and no drag, for the reason
/// [`SPRAY_GRAVITY`] gives.
fn ballistic(base: [f32; 3], v: [f32; 3], age: f32) -> [f32; 3] {
    [
        base[0] + v[0] * age,
        v[1].mul_add(age, base[1]) - 0.5 * SPRAY_GRAVITY * age * age,
        base[2] + v[2] * age,
    ]
}

// ---------------------------------------------------------------------------
// W9: the impact crown — the *other* source §2.2e names.
// ---------------------------------------------------------------------------
//
// **W5 specified two sources and shipped one.** Everything above is driven by
// `WaterSample::fold`, the crest-steepness term, so a flat pool throws nothing
// however hard something falls into it — `assets/test/pool.loom` exists to make
// that visible and did. This half is driven by the *event*: a body broke the
// surface at a speed, and the impact sets the droplets' velocity.
//
// It is the same kind of object as the one above — a pure function of
// `(where, how hard, how old, a seed)`, no state, no readback, nothing an
// assertion can see. ADR 0045 clause 1 satisfied by having nothing to argue
// about, exactly as the crest crown satisfies it.

/// Downward speed, m/s, below which an entry is a settling rather than a splash.
///
/// **The gate that suppresses the phantom at tick zero as well.** A body
/// authored already floating has `fraction == 0.0` before its first solve, so
/// its first solve reads as an entry; it is also barely moving, so this is what
/// tells the two apart. Both jobs are one number on purpose — a second constant
/// would be a second thing to keep in step.
pub const SPLASH_MIN_SPEED: f32 = 1.0;

/// Impact speed at which the crown is at full size, m/s.
///
/// `pool.loom`'s sphere enters at about 7.1 m/s measured, so a three-metre drop
/// throws very nearly the largest crown there is and a half-metre drop throws a
/// visibly smaller one. That ratio is the acceptance test, not the absolute.
pub const SPLASH_FULL_SPEED: f32 = 8.0;

/// Fraction of the impact speed a droplet leaves with, upward and outward.
///
/// **Not tuned by eye: the ratio is what sets the crown's height**, and the
/// lifetime falls out of it ballistically rather than being a second authored
/// number that can disagree. At `SPLASH_FULL_SPEED` the upward fraction gives
/// 2.7 m/s, so the tallest droplet reaches 0.37 m and is gone in 0.55 s — a
/// crown, on the scale of the thing that made it, rather than a fountain.
pub const SPLASH_UP_FRAC: f32 = 0.34;
/// Outward fraction. Under the upward one: a crown is taller than it is wide,
/// which is what distinguishes it from a ring of spray.
pub const SPLASH_OUT_FRAC: f32 = 0.22;

/// Droplets per band of the crown.
///
/// **32 x 8, and the count was chosen by measurement rather than by budget.**
/// The brief for this slice assumed overdraw would be the binding constraint;
/// it is not, at anything like this scale. Probed on `pool.loom` at
/// 1920x1080, `--sim 52`, with `LOOM_GPU_TIMING=1`:
///
/// ```text
///     112 particles (16x4 rim + the old sheet)   forward 0.144 ms  water 0.268 ms
///     272 particles (32x8 rim + the old sheet)   forward 0.136 ms  water 0.267 ms
///    1840 particles (128x16 rim + the sheet)     forward 0.133 ms  water 0.280 ms
/// ```
///
/// A sixteen-fold rise in quads costs **0.012 ms** in the pass that draws them
/// and nothing at all in the forward pass — three runs each, whole-render wall
/// clock unmoved at 0.69 s against 0.74 s. So the counts here are set by what
/// the rim has to *look* like and not by what it costs: 32 around is what lets
/// [`SPLASH_FINGERS`] fingers be told apart at all, since a ring sampled at 16
/// points cannot express twenty ligaments.
pub const SPLASH_RING: usize = 32;
/// Bands at full strength. A marginal entry throws fewer — see [`crown`].
pub const SPLASH_BANDS: usize = 8;

/// Fingers a crown's rim tears into, at the lowest and the highest impact.
///
/// **This is the one cosine that stops a splash reading as a mushroom.** A
/// rising crown wall is a thin liquid sheet with a thickened rim, and a
/// thickened rim is Rayleigh–Plateau unstable: it drains into a set of
/// evenly-spaced ligaments, and the droplets come off *those* rather than off
/// the whole circumference. Every photograph of a milk-drop crown is a count
/// of them.
///
/// `n = round(6 + 14 · smoothstep(SPLASH_MIN_SPEED, SPLASH_FULL_SPEED, U))` —
/// **a fitted shape, not a derivation.** The real count goes as the square
/// root of the Weber number, which needs a surface tension and a sheet
/// thickness this engine has nowhere to put; the ramp reproduces the right
/// range (a gentle entry tears into a handful of lobes, a hard one into ~20)
/// off quantities the event already carries.
pub const SPLASH_FINGERS: (f32, f32) = (6.0, 14.0);

/// How deep the fingering cuts, as a fraction of the rim radius.
///
/// `r(θ) = R · (1 + 0.25·cos(nθ + φ))` on the birth radius **and on the
/// outward velocity**, so a ligament is both further out to start with and
/// travelling faster — which is what makes it a finger in flight rather than a
/// scalloped ring that stays a ring.
const SPLASH_FINGER_DEPTH: f32 = 0.25;

/// How much a droplet's size may vary from its band's, either way.
///
/// See [`Droplet::scale`]. 0.45 is wide enough to break the necklace and narrow
/// enough that the biggest droplet is not twice the smallest, which starts
/// reading as two different substances.
const SPLASH_SIZE_JITTER: f32 = 0.45;

/// The crown an impact throws, `age` seconds after it happened.
///
/// `at` is the point on the surface the body broke — the surface *including*
/// the wake, which the caller already computed for the submersion event.
/// `speed` is positive **downward**: how hard it hit. `radius` is the body's
/// waterplane radius, and it is the geometric point of this function —
///
/// **the ring starts at the rim of the cavity, never at the centre.** A crown
/// rises from the edge of the hole the body punched; droplets launched from the
/// centre would be launched from inside the body, and on anything wider than a
/// droplet the crown reads as a spout coming out of the object rather than as
/// water thrown aside by it.
///
/// Empty below [`SPLASH_MIN_SPEED`], and empty once every band has landed.
/// Returned in band-then-ring order and never sorted, like [`spray`].
#[must_use]
pub fn crown(at: [f32; 3], speed: f32, radius: f32, age: f32, seed: u32) -> Vec<Droplet> {
    let strength =
        ((speed - SPLASH_MIN_SPEED) / (SPLASH_FULL_SPEED - SPLASH_MIN_SPEED)).clamp(0.0, 1.0);
    if strength <= 0.0 || age < 0.0 {
        return Vec::new();
    }
    // **Count follows the impact, not only speed.** A marginal entry throws one
    // band and a hard one throws three, so how much water is in the air is a
    // reading of how hard the thing hit rather than a constant with a scale on
    // it. `ceil` rather than `round`: anything past the gate throws something.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bands = (strength * SPLASH_BANDS as f32).ceil().max(1.0) as usize;
    #[allow(clippy::cast_precision_loss)]
    let spin = ((hash(seed) >> 8) as f32) * (1.0 / 16_777_216.0) * std::f32::consts::TAU;
    // **Saturated, not extrapolated.** The velocity is a fraction of the impact
    // speed, so an unbounded speed is an unbounded crown: a body arriving at
    // 40 m/s would throw droplets thirteen metres up and leave them in the air
    // for nearly three seconds, which is a geyser rather than a splash and
    // would sit in a `--sim N` still long after the thing that caused it. The
    // ramp already stops counting at this speed; the arc stops with it.
    let impact = speed.min(SPLASH_FULL_SPEED);

    // The fingering. `n` is a count of ligaments and `phi` turns them, so two
    // impacts do not tear the same way — see [`SPLASH_FINGERS`].
    #[allow(clippy::cast_precision_loss)]
    let fingers = {
        let s = smoothstep(SPLASH_MIN_SPEED, SPLASH_FULL_SPEED, speed);
        SPLASH_FINGERS.1.mul_add(s, SPLASH_FINGERS.0).round()
    };
    #[allow(clippy::cast_precision_loss)]
    let phi = ((hash(seed ^ 0x00f1_9e12) >> 8) as f32) * (1.0 / 16_777_216.0)
        * std::f32::consts::TAU;

    let mut out = Vec::new();
    for band in 0..bands {
        // 0 for the innermost band, approaching 1 for the outermost. The outer
        // water is thrown out and the inner water is thrown up, which is the
        // shape of a real crown and also what makes it collapse from the
        // outside in: a flatter arc lands sooner.
        #[allow(clippy::cast_precision_loss)]
        let tilt = (band as f32 + 0.5) / bands as f32;
        let up = impact * SPLASH_UP_FRAC * 0.45f32.mul_add(-tilt, 1.0);
        let outward = impact * SPLASH_OUT_FRAC * 0.9f32.mul_add(tilt, 0.55);
        // Ballistic, so the lifetime is not a second authored number that can
        // disagree with the velocity: up, over, and back to the surface.
        let lifetime = 2.0 * up / SPRAY_GRAVITY;
        if age >= lifetime {
            continue;
        }
        for i in 0..SPLASH_RING {
            // Half-step per band so the bands interleave rather than stacking
            // into eight radial spokes.
            #[allow(clippy::cast_precision_loss)]
            let angle = std::f32::consts::TAU
                .mul_add((i as f32 + 0.5 * band as f32) / SPLASH_RING as f32, spin);
            let (dir_x, dir_z) = (angle.cos(), angle.sin());
            // **The ligament, one cosine of it.** `finger` is 0.75 between
            // ligaments and 1.25 on one; it swells the birth radius, throws
            // that droplet further out, and — through `alpha` below — leaves
            // the water between them thin. Doing only the radius makes a
            // scalloped ring; doing only the alpha makes a dotted one.
            let finger = SPLASH_FINGER_DEPTH.mul_add(fingers.mul_add(angle, phi).cos(), 1.0);
            // The rim. See the doc comment: this is the whole geometry.
            let base =
                [dir_x.mul_add(radius * finger, at[0]), at[1], dir_z.mul_add(radius * finger, at[2])];
            let v = [dir_x * outward * finger, up, dir_z * outward * finger];
            // **Per droplet, from the same hash the ring is turned by.** A band
            // shares one `fraction` and therefore one size, so without this the
            // crown is a string of identical beads — see [`Droplet::scale`].
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let unit = ((hash(seed.wrapping_add((band * SPLASH_RING + i) as u32 + 1)) >> 8) as f32)
                * (1.0 / 16_777_216.0);
            out.push(Droplet {
                position: ballistic(base, v, age),
                velocity: ballistic_velocity(v, age),
                fraction: age / lifetime,
                scale: SPLASH_SIZE_JITTER.mul_add(unit.mul_add(2.0, -1.0), 1.0),
                // `0.55 + 0.45·cos(nθ + φ)` — the same cosine, on the other
                // axis. Never zero, because a droplet that is exactly nothing
                // is a quad that costs what it always cost and draws no water.
                alpha: 0.45f32.mul_add(fingers.mul_add(angle, phi).cos(), 0.55),
            });
        }
    }
    out
}

/// The classic `smoothstep`, spelled out — Rust has no such thing in `std` and
/// this file is not adding a dependency for three multiplies.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (-2.0f32).mul_add(t, 3.0)
}

// ---------------------------------------------------------------------------
// The Worthington jet, and the satellites that pinch off its tip.
// ---------------------------------------------------------------------------
//
// **This replaces W12's rising sheet, which ADR 0053 §7 lists for tear-out.**
// That sheet was `spray::column`: twelve quads round a ring, four rings up,
// launched at t = 0 with the crown and hanging off one ballistic rim. Three
// judges rendered it independently and all three called it a grey mushroom,
// and the reason is not the quad count — it is the *clock*. Loom fired every
// part of a splash at t = 0, so the wall, the rim and the droplets all rose
// together and the eye read one solid dome.
//
// A real impact is a sequence. The crown goes up and starts to collapse; the
// cavity the body punched closes from the sides; the collapse focuses on the
// axis and drives a thin, fast **jet** straight up, well past where the crown
// ever reached; the jet's tip necks and pinches off into satellites. The jet is
// the tall spike in every photograph of a drop hitting a pool, and Loom has
// never drawn it.
//
// Same kind of object as everything else in this file: a pure function of
// `(where, how hard, how old, a seed)`. No state, no readback, nothing an
// assertion can see — ADR 0045 clause 1 satisfied by having nothing to argue
// about, exactly as the crest crown satisfies it.

/// Fraction of the impact speed the jet's tip leaves with.
///
/// **Anchored on the apex, not on the velocity**, and it is the coefficient
/// `COLUMN_UP_FRAC` was measured at — reapplied to the event it belongs to.
/// The apex is `(0.5U)²/2g = 0.0127·U²`: at `pool.loom`'s measured 7.1 m/s
/// entry that is 0.64 m, against the crown's tallest band at 0.30 m. **The jet
/// must exceed the crown**, or the anatomy is wrong and this constant is what
/// says so.
pub const JET_UP_FRAC: f32 = 0.50;

/// The jet's radius at its base and at its tip, as fractions of the cavity.
///
/// A jet is *thin* — that is most of what distinguishes it from a column. It
/// tapers upward because the tip is travelling fastest and the water stretches.
const JET_RADIUS: (f32, f32) = (0.25, 0.15);

/// Droplets around the jet and segments up it: 6 x 14 = 84 quads.
const JET_RING: usize = 6;
const JET_SEGMENTS: usize = 14;

/// Seconds after the jet launches at which its tip pinches off.
///
/// Rayleigh–Plateau again, on a cylinder this time rather than on a rim: a
/// stretching liquid thread necks and breaks, and the tip is where it is
/// thinnest. Fitted, for [`SPLASH_FINGERS`]'s reason — the real time depends on
/// a surface tension nothing in this engine holds.
const SATELLITE_DELAY: f32 = 0.08;

/// Droplets pinched off the jet tip.
const SATELLITE_COUNT: usize = 40;

/// How wide the satellites spread, in metres per second, sideways.
const SATELLITE_SPREAD: f32 = 1.1;

/// When the jet leaves, in seconds after the impact.
///
/// `t_jet = R / sqrt(g · max(h_cavity, R))` — the cavity's collapse time, which
/// is how long the sides take to fall back in under gravity across a gap of
/// their own size.
///
/// **`h_cavity` is not available at the event and the formula already says what
/// to do about it.** The submersion event carries where, how hard and how wide,
/// and nothing measures how deep the hole got; `max(h_cavity, R)` with an
/// unknown `h_cavity` is `R`, so this is `sqrt(R/g)`. At `pool.loom`'s 0.5 m
/// sphere that is 0.226 s — **13.6 ticks after the entry**, which is what puts
/// the jet visibly *after* the crown in a `--sim` sweep rather than inside it.
#[must_use]
pub fn jet_delay(radius: f32) -> f32 {
    (radius.max(1e-3) / SPRAY_GRAVITY).sqrt()
}

/// The jet an impact drives up out of the closing cavity, `age` seconds after
/// the impact — empty before [`jet_delay`] and empty once it has fallen back.
///
/// Same arguments as [`crown`] and deliberately so: one event, handed to both,
/// so the two can never disagree about where the impact was or how hard it hit.
///
/// **One ballistic tip, and the column hangs under it.** Every segment sits at
/// a fixed fraction of the tip's current height, so its velocity is that
/// fraction of the tip's — the exact derivative, not a second integration.
/// Giving each segment its own launch was tried on the sheet this replaces and
/// is what tore it into a detached puff.
#[must_use]
pub fn jet(at: [f32; 3], speed: f32, radius: f32, age: f32, seed: u32) -> Vec<Droplet> {
    let Some((up, jet_age)) = jet_state(speed, radius, age) else {
        return Vec::new();
    };
    let tip = up.mul_add(jet_age, -0.5 * SPRAY_GRAVITY * jet_age * jet_age);
    if tip <= 0.0 {
        return Vec::new();
    }
    let lifetime = 2.0 * up / SPRAY_GRAVITY;
    // A turn of its own, so the jet's seams and the crown's ligaments do not
    // line up into spokes.
    #[allow(clippy::cast_precision_loss)]
    let spin =
        ((hash(seed ^ 0x000d_7e70) >> 8) as f32) * (1.0 / 16_777_216.0) * std::f32::consts::TAU;

    let mut out = Vec::new();
    for segment in 0..JET_SEGMENTS {
        #[allow(clippy::cast_precision_loss)]
        let f = (segment as f32 + 1.0) / JET_SEGMENTS as f32;
        let y = tip * f;
        let r = radius * (JET_RADIUS.1 - JET_RADIUS.0).mul_add(f, JET_RADIUS.0);
        // The exact derivative of `y = tip(jet_age) · f`.
        let rise = f * SPRAY_GRAVITY.mul_add(-jet_age, up);
        for i in 0..JET_RING {
            #[allow(clippy::cast_precision_loss)]
            let angle = std::f32::consts::TAU
                .mul_add((i as f32 + 0.5 * segment as f32) / JET_RING as f32, spin);
            out.push(Droplet {
                position: [angle.cos().mul_add(r, at[0]), at[1] + y, angle.sin().mul_add(r, at[2])],
                // **Smeared, unlike the sheet it replaces.** A sheet's quads
                // had to overlap into a surface and a streak tore holes in it;
                // a jet is a fast thread of water, and the vertical smear is
                // what makes six quads round a 0.1 m circle read as one spout.
                velocity: [0.0, rise, 0.0],
                fraction: age / lifetime,
                // Fat at the base where the water is still a column, thinning
                // toward the tip that is about to break up.
                scale: 0.8f32.mul_add(-f, 1.5),
                alpha: 1.0,
            });
        }
    }
    out
}

/// The droplets that pinch off the jet's tip, [`SATELLITE_DELAY`] after it left.
///
/// Launched from where the tip actually was at that instant and carrying the
/// tip's own velocity plus a spread, so they lead the jet up and then rain back
/// past it. Small — a satellite is what is left when a thread necks, not a
/// second crown.
#[must_use]
pub fn satellites(at: [f32; 3], speed: f32, radius: f32, age: f32, seed: u32) -> Vec<Droplet> {
    let Some((up, jet_age)) = jet_state(speed, radius, age) else {
        return Vec::new();
    };
    let flight = jet_age - SATELLITE_DELAY;
    if flight < 0.0 {
        return Vec::new();
    }
    // Where the tip was, and how fast it was going, when the thread broke.
    let y0 = up.mul_add(SATELLITE_DELAY, -0.5 * SPRAY_GRAVITY * SATELLITE_DELAY * SATELLITE_DELAY);
    let rise = SPRAY_GRAVITY.mul_add(-SATELLITE_DELAY, up);
    let base = [at[0], at[1] + y0, at[2]];

    let mut out = Vec::new();
    for i in 0..SATELLITE_COUNT {
        #[allow(clippy::cast_precision_loss)]
        let unit = |shift: u32| {
            ((hash((seed ^ 0x0057_a721).wrapping_add(i as u32 * 3 + shift)) >> 8) as f32)
                * (1.0 / 16_777_216.0)
        };
        // A cone: mostly up, spread sideways by the neck's own instability.
        let angle = unit(0) * std::f32::consts::TAU;
        let out_speed = SATELLITE_SPREAD * unit(1).sqrt();
        // The tip's velocity, varied either way — the ones that keep more of it
        // go highest and land last, which is what makes a spray rather than a
        // shell.
        let v = [
            angle.cos() * out_speed,
            rise * 0.35f32.mul_add(unit(2), 0.75),
            angle.sin() * out_speed,
        ];
        // Ballistic from the break, and gone when it is back at the surface.
        let lifetime = 2.0 * v[1] / SPRAY_GRAVITY + (2.0 * y0 / SPRAY_GRAVITY).sqrt();
        if flight >= lifetime {
            continue;
        }
        let position = ballistic(base, v, flight);
        if position[1] <= at[1] {
            continue;
        }
        out.push(Droplet {
            position,
            velocity: ballistic_velocity(v, flight),
            fraction: flight / lifetime,
            scale: 0.3f32.mul_add(unit(2), 0.35),
            alpha: 1.0,
        });
    }
    out
}

/// The jet's launch speed and how long it has been flying, or `None` before it
/// launches and once the impact is too gentle to drive one.
///
/// One place, so [`jet`] and [`satellites`] cannot disagree about when the jet
/// left — which is the failure the sheet's per-ring launch already demonstrated
/// once.
fn jet_state(speed: f32, radius: f32, age: f32) -> Option<(f32, f32)> {
    if speed <= SPLASH_MIN_SPEED || age < 0.0 {
        return None;
    }
    let jet_age = age - jet_delay(radius);
    if jet_age < 0.0 {
        return None;
    }
    // Saturated for [`crown`]'s reason: an unbounded speed would otherwise
    // stand a jet in the air for seconds after the thing that caused it.
    Some((speed.min(SPLASH_FULL_SPEED) * JET_UP_FRAC, jet_age))
}

/// Everything one impact throws, at `age` seconds: the crown, then the jet,
/// then the satellites off its tip.
///
/// **One function because it is one event on one clock**, and because the two
/// call sites in `loom_cli::particles` — the headless replay and the window's
/// live plumes — have to draw the same population. Wiring a water effect into
/// one path only is a mistake this repository has made three times
/// (`set_ripples`, ADR 0046 §7), and a single entry point is what makes it
/// impossible rather than merely tested.
///
/// Ordered crown, jet, satellites: that is the blend order, and the jet stands
/// in front of the rim it came up through.
#[must_use]
pub fn impact(at: [f32; 3], speed: f32, radius: f32, age: f32, seed: u32) -> Vec<Droplet> {
    let mut out = crown(at, speed, radius, age, seed);
    out.extend(jet(at, speed, radius, age, seed));
    out.extend(satellites(at, speed, radius, age, seed));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_scene::components::{GerstnerWave, WaveSet};

    /// A sea steep enough to break, and one that is not.
    fn sea(amplitude: f32, steepness: f32) -> WaterBody {
        WaterBody {
            spray: 1.0,
            waves: WaveSet {
                waves: vec![
                    GerstnerWave {
                        wavelength: 9.0,
                        amplitude,
                        steepness,
                        direction: [1.0, 0.2],
                        speed_scale: 1.0,
                    },
                    GerstnerWave {
                        wavelength: 5.0,
                        amplitude: amplitude * 0.6,
                        steepness,
                        direction: [0.7, -0.7],
                        speed_scale: 1.0,
                    },
                ],
                ..WaveSet::default()
            },
            ..WaterBody::default()
        }
    }

    fn deep(_x: f32, _z: f32) -> f32 {
        -1000.0
    }

    /// **A mirror throws nothing**, which is also the test that every scene
    /// with flat water in it renders as it did before spray existed.
    #[test]
    fn still_water_throws_no_spray() {
        let flat = WaterBody { spray: 4.0, ..WaterBody::default() };
        for tick in 0..120 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 / 60.0;
            assert!(spray(&flat, [0.0, 2.0, 0.0], t, &deep).is_empty());
        }
    }

    /// **And the unauthored default throws nothing however hard it blows**,
    /// which is the whole of the compatibility story: every sea committed
    /// before W5 has `spray = 0` and renders bit for bit as it did.
    #[test]
    fn a_sea_that_authors_no_spray_throws_none() {
        let storm = WaterBody { spray: 0.0, ..sea(0.55, 0.85) };
        for tick in 0..600 {
            #[allow(clippy::cast_precision_loss)]
            let t = tick as f32 / 60.0;
            assert!(spray(&storm, [0.0, 2.0, 0.0], t, &deep).is_empty());
        }
    }

    /// A gentle swell never reaches the break threshold; a steep sea does. The
    /// point is that the threshold is a threshold and not a formality.
    #[test]
    fn only_a_breaking_sea_sprays() {
        let calm = sea(0.12, 0.2);
        let storm = sea(0.55, 0.85);
        let count = |body: &WaterBody| {
            (0..240)
                .map(|tick| spray(body, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep).len())
                .sum::<usize>()
        };

        assert_eq!(count(&calm), 0, "a swell that never breaks threw spray");
        assert!(count(&storm) > 0, "a breaking sea threw none");
    }

    /// The same instant twice is the same spray — the property everything in
    /// this file rests on, and the one a clock would destroy.
    #[test]
    fn the_same_second_gives_the_same_spray() {
        let storm = sea(0.55, 0.85);
        let a = spray(&storm, [3.0, 2.0, -4.0], 5.25, &deep);
        let b = spray(&storm, [3.0, 2.0, -4.0], 5.25, &deep);

        assert!(!a.is_empty(), "the sea threw nothing to compare");
        assert_eq!(a, b);
    }

    /// Droplets are ballistic: a crown thrown a moment ago is above the crest
    /// it left, and one thrown a second ago is on its way back down.
    #[test]
    fn a_crown_goes_up_and_comes_back() {
        let storm = sea(0.55, 0.85);
        // Every droplet's height above the still-water line, by age band.
        let mut early = f32::NEG_INFINITY;
        let mut late = f32::NEG_INFINITY;
        for tick in 0..600 {
            for d in spray(&storm, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep) {
                if d.fraction < 0.15 {
                    early = early.max(d.position[1]);
                } else if d.fraction > 0.85 {
                    late = late.max(d.position[1]);
                }
            }
        }

        assert!(early.is_finite() && late.is_finite(), "no droplets in one band");
        assert!(late < early, "a droplet at the end of its life is still rising: {late} vs {early}");
    }

    /// Nothing is thrown outside the region, which is what bounds the cost on
    /// an ocean with no edges.
    #[test]
    fn spray_stays_near_the_eye() {
        let storm = sea(0.55, 0.85);
        let eye = [40.0, 2.0, -20.0];
        let out = spray(&storm, eye, 9.0, &deep);
        assert!(!out.is_empty());
        for d in &out {
            let distance = (d.position[0] - eye[0]).hypot(d.position[2] - eye[2]);
            // The crown is born inside the range and travels a little further.
            assert!(distance < SPRAY_RANGE + 6.0, "{d:?} is {distance} m from {eye:?}");
        }
    }

    /// **`peak_fold` is a ceiling, and the warning built on it is sound.**
    ///
    /// Two halves, because a bound that is not a bound sends the author to fix
    /// waves that were fine: no sample anywhere in the field may exceed it, and
    /// a sea whose ceiling is under `SPRAY_BREAK` must throw nothing — which is
    /// exactly the claim `particles::spray` prints.
    #[test]
    fn peak_fold_bounds_every_sample_and_predicts_a_dry_sea() {
        for body in [sea(0.12, 0.2), sea(0.55, 0.85), sea(0.3, 0.5)] {
            let ceiling = peak_fold(&body);
            let mut worst = f32::NEG_INFINITY;
            for tick in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let t = tick as f32 / 60.0;
                for i in -20_i16..20 {
                    for j in -20_i16..20 {
                        let at = [f32::from(i) * 0.7, f32::from(j) * 0.7];
                        worst = worst
                            .max(sample_water(&body, at, t, -400.0, [0.0; 3], [0.0; 3]).fold);
                    }
                }
            }
            assert!(
                worst <= ceiling + 1e-4,
                "fold reached {worst}, over the {ceiling} ceiling peak_fold promised"
            );
            if ceiling < SPRAY_BREAK {
                let thrown: usize = (0..240)
                    .map(|tick| {
                        #[allow(clippy::cast_precision_loss)]
                        let t = tick as f32 / 60.0;
                        spray(&body, [0.0, 2.0, 0.0], t, &deep).len()
                    })
                    .sum();
                assert_eq!(thrown, 0, "a sea under the ceiling threw {thrown} droplets");
            }
        }
    }

    /// Dry land does not spray. The waves flatten toward a shoreline, so the
    /// fold falls off with them — but this asserts it rather than trusting it,
    /// because a crown standing on a beach is the artifact everyone sees.
    #[test]
    fn spray_stops_at_the_shoreline() {
        let mut shore = sea(0.55, 0.85);
        shore.waves.attenuation_depth = 8.0;
        // A bed above the surface everywhere: the whole region is dry.
        let land = |_x: f32, _z: f32| 1.0_f32;

        for tick in 0..240 {
            let out = spray(&shore, [0.0, 2.0, 0.0], tick as f32 / 60.0, &land);
            assert!(out.is_empty(), "spray on dry land: {out:?}");
        }
    }

    // -- W9, the impact crown -------------------------------------------------

    /// **A gentle entry throws nothing**, which is the gate that keeps a body
    /// authored already floating from splashing on its first solve: its
    /// fraction starts at zero, so the edge detector reads an entry, and this
    /// is what tells that apart from an impact.
    #[test]
    fn a_settling_body_throws_no_crown() {
        for speed in [0.0_f32, 0.2, 0.9, SPLASH_MIN_SPEED] {
            assert!(
                crown([0.0, 0.0, 0.0], speed, 0.5, 0.0, 1).is_empty(),
                "an entry at {speed} m/s threw a crown"
            );
        }
    }

    /// **The crown starts at the rim, not at the centre.** Every droplet is at
    /// the body's own waterplane radius at birth, which is the correction that
    /// keeps a crown from reading as a spout out of the object.
    #[test]
    fn the_ring_starts_at_the_cavity_rim() {
        let radius = 0.5;
        let out = crown([3.0, 0.25, -2.0], 7.1, radius, 0.0, 7);
        assert!(!out.is_empty());
        for d in &out {
            let r = (d.position[0] - 3.0).hypot(d.position[2] + 2.0);
            // The rim, scalloped by the fingering and no further — see
            // `SPLASH_FINGER_DEPTH`. The bound is the point: a ligament reaches
            // further out than the mean rim, and nothing reaches past it.
            assert!(
                ((1.0 - SPLASH_FINGER_DEPTH) * radius - 1e-4
                    ..=(1.0 + SPLASH_FINGER_DEPTH) * radius + 1e-4)
                    .contains(&r),
                "born {r} m out, off the {radius} m rim by more than the fingering"
            );
            assert!((d.position[1] - 0.25).abs() < 1e-6, "born off the surface: {d:?}");
        }
    }

    /// **Size follows the impact.** A three-metre drop and a half-metre drop
    /// differ in droplet count and in peak height by about the speed ratio —
    /// which is the whole reason the event carries a speed.
    #[test]
    fn a_harder_impact_throws_a_bigger_crown() {
        let peak = |speed: f32| {
            let mut best: f32 = 0.0;
            let mut count = 0;
            for step in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let age = step as f32 / 60.0;
                let out = crown([0.0, 0.0, 0.0], speed, 0.5, age, 3);
                count = count.max(out.len());
                for d in &out {
                    best = best.max(d.position[1]);
                }
            }
            (best, count)
        };
        // 3 m and 0.55 m of free fall: 7.1 m/s measured on `pool.loom`, 3.3 m/s.
        let (hard, hard_n) = peak(7.1);
        let (soft, soft_n) = peak(3.3);
        assert!(hard > soft * 2.0, "peak {hard} m against {soft} m is not a harder impact");
        assert!(hard_n > soft_n, "{hard_n} droplets against {soft_n}");
        assert!(soft_n > 0, "a 0.55 m drop threw nothing at all");
    }

    /// **It ends.** Every band is ballistic, so the crown empties itself rather
    /// than needing a lifetime nobody can see — and a still at `--sim 120` must
    /// not have last second's splash hanging in the air.
    #[test]
    fn the_crown_lands() {
        let full = crown([0.0, 0.0, 0.0], SPLASH_FULL_SPEED, 0.5, 0.0, 5);
        assert_eq!(full.len(), SPLASH_RING * SPLASH_BANDS, "a full crown is every band");
        // The tallest band is 2*up/g at the saturated impact speed: 0.55 s.
        assert!(crown([0.0, 0.0, 0.0], 40.0, 0.5, 1.0, 5).is_empty(), "the crown never landed");
    }

    // -- The jet and its satellites -------------------------------------------

    /// **The same gate as the crown.** A settling body drives no jet either,
    /// and the two must agree about that or a scene would get a spout with no
    /// droplets on it — which is the one shape a splash never takes.
    #[test]
    fn a_settling_body_drives_no_jet() {
        for speed in [0.0_f32, 0.2, 0.9, SPLASH_MIN_SPEED] {
            for step in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let age = step as f32 / 60.0;
                assert!(
                    impact([0.0, 0.0, 0.0], speed, 0.5, age, 1).is_empty(),
                    "an entry at {speed} m/s threw something at {age} s"
                );
            }
        }
    }

    /// **The jet comes AFTER the crown, and that ordering is the whole slice.**
    ///
    /// Loom fired every part of a splash at t = 0, which is why three judges
    /// independently called the result a mushroom. So: nothing above the rim
    /// early, a spike later, and the spike clears the tallest droplet the crown
    /// can ever reach.
    #[test]
    fn the_jet_arrives_after_the_crown_and_exceeds_it() {
        let (speed, radius) = (7.1_f32, 0.5_f32);
        let delay = jet_delay(radius);
        // `pool.loom`'s sphere, and the number the scene comment quotes.
        assert!((0.20..0.25).contains(&delay), "the jet fires at {delay} s");

        // Before it: the jet is empty, and so are the satellites.
        for step in 0..12 {
            #[allow(clippy::cast_precision_loss)]
            let age = step as f32 / 60.0;
            assert!(age >= delay || jet([0.0; 3], speed, radius, age, 3).is_empty());
            assert!(age >= delay || satellites([0.0; 3], speed, radius, age, 3).is_empty());
        }

        let ceiling = (0..120)
            .flat_map(|step| {
                #[allow(clippy::cast_precision_loss)]
                crown([0.0; 3], speed, radius, step as f32 / 60.0, 3)
            })
            .fold(0.0_f32, |best, d| best.max(d.position[1]));
        let apex = (0..180)
            .flat_map(|step| {
                #[allow(clippy::cast_precision_loss)]
                jet([0.0; 3], speed, radius, step as f32 / 60.0, 3)
            })
            .fold(0.0_f32, |best, d| best.max(d.position[1]));

        assert!(ceiling > 0.0, "the crown threw nothing to compare against");
        assert!(
            apex > ceiling * 1.5,
            "the jet reached {apex} m against the crown's {ceiling} m — if it does not \
             exceed the crown the constants are wrong, not the reference"
        );
        // `(0.5U)²/2g` at the saturated impact speed, which is what
        // `JET_UP_FRAC` claims.
        let predicted = (JET_UP_FRAC * speed).powi(2) / (2.0 * crate::GRAVITY);
        assert!(
            (apex - predicted).abs() < 0.03,
            "apex {apex} m against the predicted {predicted} m"
        );
    }

    /// **It is a thin thread on the axis**, never a flaring cone — that is what
    /// distinguishes a jet from the sheet it replaces, and the reason it takes
    /// a radius at all.
    #[test]
    fn the_jet_is_a_narrow_tapering_thread() {
        let radius = 0.5;
        let at = [3.0, 0.25, -2.0];
        let mut widest: f32 = 0.0;
        for step in 0..90 {
            #[allow(clippy::cast_precision_loss)]
            let age = step as f32 / 60.0;
            for d in jet(at, 7.1, radius, age, 7) {
                let r = (d.position[0] - at[0]).hypot(d.position[2] - at[2]);
                assert!(
                    r <= radius * JET_RADIUS.0 + 1e-4,
                    "the jet is {r} m wide, past the {} m its base allows",
                    radius * JET_RADIUS.0
                );
                assert!(d.position[1] > at[1], "the jet dipped under the surface: {d:?}");
                widest = widest.max(r);
            }
        }
        assert!(widest > 0.0, "the jet never fired");
    }

    /// **Everything lands.** Both new populations are ballistic, so a still at
    /// `--sim N` must not have last second's water hanging in the air.
    #[test]
    fn the_whole_impact_ends() {
        assert!(
            impact([0.0, 0.0, 0.0], 40.0, 0.5, 2.0, 5).is_empty(),
            "something was still in the air two seconds after the impact"
        );
    }

    /// **A harder impact drives a taller jet**, for the same reason the crown
    /// gets more bands: how much water comes up is a reading of how hard the
    /// thing hit.
    #[test]
    fn a_harder_impact_drives_a_taller_jet() {
        let peak = |speed: f32| {
            let mut best: f32 = 0.0;
            for step in 0..180 {
                #[allow(clippy::cast_precision_loss)]
                let age = step as f32 / 60.0;
                for d in jet([0.0, 0.0, 0.0], speed, 0.5, age, 3) {
                    best = best.max(d.position[1]);
                }
            }
            best
        };
        let (hard, soft) = (peak(7.1), peak(3.3));
        assert!(hard > soft * 2.0, "peak {hard} m against {soft} m is not a harder impact");
        assert!(soft > 0.0, "a 0.55 m drop drove nothing at all");
    }

    /// **The satellites leave the jet's tip and lead it.** They pinch off
    /// `SATELLITE_DELAY` after the jet fires, from where the tip was then — so
    /// they are above the jet's own top for most of the flight, which is what
    /// a photograph of a Worthington jet shows.
    #[test]
    fn the_satellites_pinch_off_the_tip_and_lead_it() {
        let (speed, radius) = (7.1_f32, 0.5_f32);
        let start = jet_delay(radius) + SATELLITE_DELAY;
        for step in 0..120 {
            #[allow(clippy::cast_precision_loss)]
            let age = step as f32 / 60.0;
            let sats = satellites([0.0; 3], speed, radius, age, 9);
            assert!(age >= start || sats.is_empty(), "satellites at {age} s, before {start} s");
        }
        let top = |f: fn([f32; 3], f32, f32, f32, u32) -> Vec<Droplet>| {
            (0..180)
                .flat_map(|step| {
                    #[allow(clippy::cast_precision_loss)]
                    f([0.0; 3], speed, radius, step as f32 / 60.0, 9)
                })
                .fold(0.0_f32, |best, d| best.max(d.position[1]))
        };
        assert!(
            top(satellites) > top(jet),
            "the satellites ({} m) never cleared the jet ({} m)",
            top(satellites),
            top(jet)
        );
    }

    /// **The rim tears into ligaments rather than into equal beads.**
    ///
    /// The count follows the impact — that is [`SPLASH_FINGERS`] — and the
    /// evidence that the cosine is doing anything is that a band's droplets no
    /// longer share one radius and one opacity. A ring of identical beads is
    /// exactly the mushroom this replaces.
    #[test]
    fn the_rim_is_fingered() {
        let out = crown([0.0, 0.0, 0.0], 7.1, 0.5, 0.0, 7);
        let radii: Vec<f32> =
            out.iter().take(SPLASH_RING).map(|d| d.position[0].hypot(d.position[2])).collect();
        let (lo, hi) = radii.iter().fold((f32::MAX, 0.0_f32), |(lo, hi), r| (lo.min(*r), hi.max(*r)));
        assert!(
            (hi / lo - (1.0 + SPLASH_FINGER_DEPTH) / (1.0 - SPLASH_FINGER_DEPTH)).abs() < 0.05,
            "the rim runs {lo} m to {hi} m — the fingering is not cutting to depth"
        );
        let alpha_lo = out.iter().fold(f32::MAX, |lo, d| lo.min(d.alpha));
        let alpha_hi = out.iter().fold(0.0_f32, |hi, d| hi.max(d.alpha));
        assert!(alpha_lo < 0.2 && alpha_hi > 0.9, "opacity ran {alpha_lo} to {alpha_hi}");

        // And a gentler impact tears into fewer ligaments than a harder one.
        let count = |speed: f32| {
            let s = smoothstep(SPLASH_MIN_SPEED, SPLASH_FULL_SPEED, speed);
            SPLASH_FINGERS.1.mul_add(s, SPLASH_FINGERS.0).round()
        };
        assert!(count(2.0) < count(7.1), "{} against {}", count(2.0), count(7.1));
        assert!((count(SPLASH_FULL_SPEED) - 20.0).abs() < 0.5);
    }

    /// **Two impacts on one tick are two different crowns.** Same shape, turned
    /// differently — the seed is the caller's `salt(tick, at)`, so two bodies
    /// going in together do not throw one splash drawn twice.
    #[test]
    fn the_seed_turns_the_ring() {
        let a = crown([0.0, 0.0, 0.0], 6.0, 0.4, 0.1, 11);
        let b = crown([0.0, 0.0, 0.0], 6.0, 0.4, 0.1, 12);
        assert_eq!(a.len(), b.len());
        assert!(a != b, "two seeds threw the same crown");
    }
}
