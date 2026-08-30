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
//! **Closed form, and never read by anything.** A droplet is a function of
//! `(body, sea, t, region)` — the cell it came from, the slot of time it was
//! born in, and a ballistic arc from there. Nothing accumulates here, nothing is
//! read back, nothing produces a force, and no assertion can see it. That is
//! ADR 0045 clause 1, and it is the same shape as `loom_rain::splashes`,
//! deliberately.
//!
//! **The one piece of state is behind `sea`, and it is a recording rather than
//! an accumulator.** A Gerstner body answers any past instant for free, because
//! the sum is closed form at a point. An FFT body cannot: the cascade is a pure
//! function of `t` too, but evaluating one instant costs a whole tile stack, so
//! the simulation *keeps* every eighth tick's tiles and this reads the newest
//! one at or before a droplet's birth (`ocean::Ocean::keep`). That ring is
//! filled only from the fixed step, so its contents are a function of the tick
//! count and nothing else — `--sim N` in one jump and stepping to N leave the
//! same droplets in the air.
//!
//! **The region follows the eye, and that is allowed here specifically.**
//! Spray is drawn and nothing else, so ADR 0045's trap clause — a *force*
//! grid must anchor to sim state, never the camera — does not apply. The eye is
//! how the population stays bounded on an unbounded ocean, exactly as
//! `loom_rain::splashes` bounds itself.

use crate::ocean::Ocean;
use crate::sample_water_born;
use loom_field::noise::hash;
use loom_scene::components::{WaterBody, WaveModel};

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
/// **Twenty-one, and seven was a necklace.** §2.2e asks for "a ring emitted
/// with outward+upward velocity", and seven identical droplets on one ring at
/// one radius is exactly the artifact [`crown`] fifty lines down was built to
/// avoid — its own docs call it "a string of manufactured beads" and answer it
/// with [`Droplet::scale`] and [`Droplet::alpha`], which the crest crown then
/// left at 1.0. Photographed at 2x on `lucent --sim 300` the crest spray reads
/// as arcs of evenly-spaced pearls, because that is what it is.
///
/// **The count is not what costs.** [`SPLASH_RING`]'s own measurement is the
/// authority and it is on the same primitive: sixteen times the quads costs
/// 0.012 ms in the pass that draws them and nothing in the forward pass. So
/// this is set by what a puff has to look like. On `lucent` it is
/// 1,113 -> 3,339 droplets, and 0.15% -> 0.46% of the frame.
pub const SPRAY_CROWN: usize = 21;

/// How far a droplet's launch may sit off its crown's nominal arc, either way.
///
/// **A ring is the one shape torn water is not.** With every droplet at one
/// speed and one elevation the crown stays a perfect expanding circle for its
/// whole life, which is what makes twenty-one beads read as twenty-one beads
/// rather than as a puff. Spreading the launch turns the ring into a cone and
/// the crown into a cloud with a front and a back — and it is free, because
/// the arc is already a closed form of the velocity.
///
/// 0.55 keeps every droplet on a rising, outward path (the multiplier stays in
/// `0.45..1.55`); past 1.0 some of them launch inward or downward, which reads
/// as the crest sucking water back in.
const SPRAY_SPREAD: f32 = 0.55;

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
///
/// **Gerstner bodies only, and it is zero on a `spectrum` one.** A cascade has no wave
/// list to sum: its fold is differenced off tiles that do not exist until the first
/// evolve. So a caller must not read a zero here as "this sea cannot break" on a
/// spectrum body — it means the question was asked of the wrong model, and
/// [`Ocean::peak_fold`] is the one to ask instead.
///
/// **The two are not the same kind of ceiling and a caller has to know which it holds.**
/// This one is analytic and over all time: `Σ Q·k·A` is true before a tick has run and
/// stays true for the whole run, so a sea under [`SPRAY_BREAK`] here can be told it will
/// *never* break. [`Ocean::peak_fold`] is a max over the tiles that exist at one
/// instant, so a cascade under the threshold is a sea that is not breaking *now* and may
/// break in ten seconds. See its docs; the distinction is the reason it is a method
/// there rather than another arm of this function.
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
/// `sea` is the simulation's cascade, and it is what a `spectrum` body's crests are read
/// from — see [`sample_water_born`]. `None` on a Gerstner body, which is every sea that
/// has not opted in, and the closed form answers those. A spectrum body handed `None`
/// throws nothing rather than inventing a surface.
///
/// `ground` answers the bed height under a point, for the same reason
/// [`crate::sample_water`] takes one: this crate does not know what a voxel is. A
/// closure that returns [`loom_voxel::heightfield::NO_GROUND`]'s value — any
/// large negative — is an open sea.
///
/// Returned in cell order and never sorted: the order is what the renderer
/// draws in, and a different order is a different additive sum.
#[must_use]
pub fn spray(
    body: &WaterBody,
    sea: Option<&Ocean>,
    eye: [f32; 3],
    t: f32,
    ground: &dyn Fn(f32, f32) -> f32,
) -> Vec<Droplet> {
    let mut out = Vec::new();
    // **What "this body has a surface at all" means differs by model**: a Gerstner sea
    // is its wave list, and a spectrum sea is its cascade — which a spectrum body has
    // *instead* of a wave list, so the old `waves.is_empty()` test rejected every one of
    // them before the loop even started.
    let has_a_surface = if body.wave_model == WaveModel::Spectrum {
        sea.is_some()
    } else {
        !body.waves.waves.is_empty()
    };
    // A mirror throws nothing, and a sea that authors no spray throws nothing —
    // which is what keeps every reference image of every sea unmoved. Asking
    // costs a grid of surface samples, so both are checked before the loop.
    if body.spray <= 0.0 || !has_a_surface || t < 0.0 {
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
                crown_in(body, sea, [ix, iz], slot, eye, t, ground, &mut out);
            }
        }
    }
    out
}

/// One cell's crown for one slot of time, if it threw one.
// Eight because the sea joined the seven that were already here, and every one of them
// is a distinct thing this cell needs: what the water is, where the eye is, when it is,
// and what the bed does. The same allow `foam.rs` takes one function along.
#[allow(clippy::too_many_arguments)]
fn crown_in(
    body: &WaterBody,
    sea: Option<&Ocean>,
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
    //
    // **Which is why the ocean has to be able to answer about the past**, and is the
    // whole of what stood between a `spectrum` body and any spray at all: the cascade is
    // a stack of tiles evolved to *one* instant, and `born` is never that instant. It
    // now answers off the ring `Ocean::keep` fills on the fixed step — see
    // `sample_water_born`, which is also where the Gerstner path's closed form lives.
    //
    // `None` is a birth older than the ring reaches, which is a run's first second. No
    // crown, rather than one launched off the wrong water.
    let Some(surface) = sample_water_born(body, sea, at, born, ground(at[0], at[1])) else {
        return;
    };
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
        // **Four independent draws per droplet, off the cell's own seed.** The
        // offset is `i` scaled past the shifts used above so no droplet can
        // collide with the crown's own place, time or spin draw. Still a pure
        // function of `(cell, slot, i)`, so `--sim N` in one jump and stepping
        // to N throw the same water — which is the property the whole file
        // rests on.
        #[allow(clippy::cast_possible_truncation)]
        let d = |k: u32| unit(0x1000 + (i as u32) * 8 + k);
        #[allow(clippy::cast_precision_loss)]
        let angle = spin + std::f32::consts::TAU * (i as f32) / (SPRAY_CROWN as f32);
        // The cone — see [`SPRAY_SPREAD`]. Two draws rather than one, so a
        // droplet thrown far is not also thrown high and the crown gains a
        // depth a scaled ring cannot have.
        let outward = SPRAY_OUT * strength * SPRAY_SPREAD.mul_add(d(0).mul_add(2.0, -1.0), 1.0);
        let up = SPRAY_UP * strength * SPRAY_SPREAD.mul_add(d(1).mul_add(2.0, -1.0), 1.0);
        let v = [
            drift[0] + angle.cos() * outward,
            up,
            drift[1] + angle.sin() * outward,
        ];
        out.push(Droplet {
            position: ballistic(base, v, age),
            velocity: ballistic_velocity(v, age),
            fraction: age / SPRAY_LIFETIME,
            // The other half of breaking the necklace, and it is the same pair
            // [`crown`] uses for the same stated reason — a band of a crown
            // shares one `fraction` and therefore one drawn size. Reusing
            // `SPLASH_SIZE_JITTER` rather than authoring a second width: it is
            // the same question about the same primitive.
            scale: SPLASH_SIZE_JITTER.mul_add(d(2).mul_add(2.0, -1.0), 1.0),
            // Never zero: a fully transparent droplet is a quad that costs
            // what it always cost and draws no water.
            alpha: 0.45f32.mul_add(d(3).mul_add(2.0, -1.0), 0.55),
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
/// The apex is `(0.5U)²/2g = 0.0127·U²`: at the 7.01 m/s entry both
/// `pool.loom` and `pool_jet.loom` produce, that is **0.625 m measured at tick
/// 78**, against the crown's tallest band at 0.30 m. **The jet must exceed the
/// crown**, or the anatomy is wrong and this constant is what says so.
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
/// unknown `h_cavity` is `R`, so this is `sqrt(R/g)`. At a 0.5 m sphere that is
/// 0.226 s — **13.6 ticks after the entry**, which is what puts the jet *after*
/// the crown in a `--sim` sweep rather than inside it: measured on
/// `pool_jet.loom`, entry 44, jet from 58, apex 78, gone by ~101.
///
/// **After is not the same as visible, and `pool.loom` is the counterexample.**
/// A jet rises on the impact's own axis, so a body that floats stands in it:
/// `pool.loom`'s sphere never gets a fifth of itself under and its crown is at
/// 0.69 m while the tip is at 0.625, which hid this population inside the ball
/// for a whole slice with a golden row pointed at it. `pool_jet.loom` exists
/// because of that — same pool, a stone instead of a float.
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
/// (`set_ripples`, ADR 0046 §7 — since deleted), and one entry point is what makes it
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
            assert!(spray(&flat, None, [0.0, 2.0, 0.0], t, &deep).is_empty());
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
            assert!(spray(&storm, None, [0.0, 2.0, 0.0], t, &deep).is_empty());
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
                .map(|tick| spray(body, None, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep).len())
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
        let a = spray(&storm, None, [3.0, 2.0, -4.0], 5.25, &deep);
        let b = spray(&storm, None, [3.0, 2.0, -4.0], 5.25, &deep);

        assert!(!a.is_empty(), "the sea threw nothing to compare");
        assert_eq!(a, b);
    }

    /// **A crest crown is torn water, not a necklace.** Every droplet used to
    /// leave at one speed, one elevation, one size and full opacity, which
    /// photographs as evenly-spaced pearls on a perfect expanding ring — the
    /// artifact [`crown`] answers with [`Droplet::scale`] and
    /// [`Droplet::alpha`] and that this path then left at 1.0.
    ///
    /// Watched to fail on the shipped constants: with `SPRAY_SPREAD = 0.0` and
    /// `scale`/`alpha` back at 1.0 every one of these four assertions trips.
    #[test]
    fn a_crest_crown_is_not_a_necklace() {
        let storm = sea(0.55, 0.85);
        // One crown, isolated: `spray` returns cell-then-slot order, so the
        // first SPRAY_CROWN droplets are one cell's throw.
        let all = spray(&storm, None, [0.0, 2.0, 0.0], 5.25, &deep);
        assert!(all.len() >= SPRAY_CROWN, "the sea threw no whole crown");
        let crown = &all[..SPRAY_CROWN];

        let spread = |f: &dyn Fn(&Droplet) -> f32| {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for d in crown {
                lo = lo.min(f(d));
                hi = hi.max(f(d));
            }
            hi - lo
        };
        assert!(spread(&|d| d.scale) > 0.3, "every droplet is the same size: a string of beads");
        assert!(spread(&|d| d.alpha) > 0.3, "every droplet is equally opaque: a ring with no gaps");
        // The cone. Horizontal speed apart is the ring's radius spreading; the
        // vertical is what gives the crown a front and a back.
        assert!(
            spread(&|d| d.velocity[0].hypot(d.velocity[2])) > 0.2,
            "every droplet leaves at one outward speed, so the crown stays a circle"
        );
        assert!(spread(&|d| d.velocity[1]) > 0.2, "every droplet leaves at one elevation");
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
            for d in spray(&storm, None, [0.0, 2.0, 0.0], tick as f32 / 60.0, &deep) {
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
        let out = spray(&storm, None, eye, 9.0, &deep);
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
                            .max(crate::sample_water(&body, None, at, t, -400.0, [0.0; 3], [0.0; 3]).fold);
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
                        spray(&body, None, [0.0, 2.0, 0.0], t, &deep).len()
                    })
                    .sum();
                assert_eq!(thrown, 0, "a sea under the ceiling threw {thrown} droplets");
            }
        }
    }

    /// **The cascade's ceiling is a ceiling too, and a spectrum sea too gentle to break
    /// now says so.** The twin of the test above, for the model that has no wave list:
    /// `Ocean::peak_fold` is the per-cascade max of `Sxx + Szz` summed over the stack,
    /// and the two halves are the same two claims — no sample of the field may exceed
    /// it, and a sea under `SPRAY_BREAK` must throw nothing.
    ///
    /// **The sampling is over one whole swell patch and its side is prime.** Every tile
    /// is periodic on its own patch — 2048, 256 and 32 m — and the two finer ones divide
    /// the coarsest, so `[0, 2048)²` is not a sample of the sea, it *is* the sea: no
    /// world point exists outside it. A smaller domain reads systematically low because
    /// it sees one flank of the 2048 m swell rather than a crest, and a side that shares
    /// a factor with 32 or 256 lands on the same handful of chop phases in every patch
    /// it crosses. 401 is prime, which is also why `play`'s crossing-sea measurement
    /// uses it, and the step it gives (5.107 m) walks 401 distinct phases through the
    /// chop rather than 64.
    ///
    /// It is a loose ceiling and that is honest rather than a defect: the three cascades'
    /// maxima are at three different places, so no single point can reach their sum.
    /// Measured on `ocean_fft_storm.loom` at tick 400 the worst sample is 0.923 against
    /// a 1.459 ceiling. What the test forbids is the direction that matters — a sample
    /// *over* the ceiling, which is what would send an author to raise a wind that was
    /// already high enough.
    #[test]
    fn the_cascade_ceiling_bounds_every_sample_and_predicts_a_dry_sea() {
        use loom_scene::components::{WaterBody, WaveModel};
        const SIDE: u16 = 401;
        // The largest cascade's patch: `shipping_stack`'s 2048 m, and therefore the
        // period of the whole summed field.
        let step = 2048.0 / f32::from(SIDE);
        // A sea that breaks and one that cannot be seen to. `u10 = 2` is the gentle
        // half and it is measured, not guessed: its ceiling wanders 0.212..0.289 over
        // forty quarter-seconds, under `SPRAY_BREAK` at every one of them.
        for (u10, breaks) in [(2.0_f32, false), (18.0, true)] {
            let body = WaterBody {
                wave_model: WaveModel::Spectrum,
                spray: 8.0,
                fetch: Some(50_000.0),
                ..WaterBody::default()
            };
            let mut sea = crate::ocean::Ocean::for_body(&body, u10, [1.0, 0.0]).expect("a cascade");
            assert!(sea.peak_fold().is_nan(), "an unevolved cascade answered a ceiling");

            let t = 400.0 / 60.0;
            sea.evolve(t);
            let ceiling = sea.peak_fold();
            assert_eq!(
                ceiling > SPRAY_BREAK,
                breaks,
                "u10 {u10} put the ceiling at {ceiling}, the wrong side of {SPRAY_BREAK}"
            );

            let mut worst = f32::NEG_INFINITY;
            for iz in 0..SIDE {
                for ix in 0..SIDE {
                    let at = [f32::from(ix) * step, f32::from(iz) * step];
                    let fold = crate::sample_water(
                        &body,
                        Some(&sea),
                        at,
                        t,
                        -1000.0,
                        [0.0; 3],
                        [0.0; 3],
                    )
                    .fold;
                    worst = worst.max(fold);
                }
            }
            assert!(
                worst <= ceiling,
                "fold reached {worst} over the whole tile, over the {ceiling} ceiling \
                 Ocean::peak_fold promised"
            );

            // And the claim the warning in `loom_cli::particles::spray` prints: a sea
            // under the threshold throws nothing. Every eighth tick kept, as
            // `Sim::evolve_sea` does, or the ring spray reads is empty and this would
            // pass on a sea that breaks.
            if !breaks {
                let mut kept =
                    crate::ocean::Ocean::for_body(&body, u10, [1.0, 0.0]).expect("a cascade");
                let mut thrown = 0;
                for tick in 0..120_u16 {
                    let t = f32::from(tick) / 60.0;
                    kept.evolve(t);
                    if tick % 8 == 0 {
                        kept.keep();
                    }
                    thrown += spray(&body, Some(&kept), [0.0, 2.0, 0.0], t, &deep).len();
                }
                assert_eq!(thrown, 0, "a cascade under the ceiling threw {thrown} droplets");
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
            let out = spray(&shore, None, [0.0, 2.0, 0.0], tick as f32 / 60.0, &land);
            assert!(out.is_empty(), "spray on dry land: {out:?}");
        }
    }

    /// **The gap this file was written around, closed: an FFT sea throws spray.**
    ///
    /// Before the ring existed, a `spectrum` body reached `sample_water` with no ocean,
    /// got the still surface back, and every crown failed the break test — the feature
    /// was a silent no-op on the one model a storm scene wants. The three halves here are
    /// the three ways that can come back:
    ///
    /// 1. **Without a sea it still throws nothing** — the mirror rule. A spectrum body
    ///    handed `None` must not fall back to a wave list it does not have.
    /// 2. **With a kept ring it throws**, which is the whole point.
    /// 3. **A sea that was evolved but never kept throws nothing**, because `at_kept`
    ///    reads the ring and not the current tiles. That is what would fail if
    ///    `Sim::evolve_sea` stopped calling `Ocean::keep` — the exact regression that
    ///    would otherwise be invisible until somebody rendered a storm.
    #[test]
    fn a_spectrum_sea_throws_spray_off_its_kept_tiles() {
        use loom_scene::components::{WaveModel, WaterBody};
        let body = WaterBody {
            wave_model: WaveModel::Spectrum,
            spray: 8.0,
            fetch: Some(50_000.0),
            ..WaterBody::default()
        };
        let eye = [0.0_f32, 4.0, 0.0];
        // Long enough that `born` reaches back past the oldest kept snapshot for some
        // droplets and not for others, which is the case the `None` arm exists for.
        let ticks = 96_u32;
        let stride = 8;

        let mut sea = crate::ocean::Ocean::for_body(&body, 25.0, [1.0, 0.0]).expect("a cascade");
        let at_tick = |tick: u32| f32::from(u16::try_from(tick).expect("small")) / 60.0;
        for tick in 0..=ticks {
            sea.evolve(at_tick(tick));
            if tick % stride == 0 {
                sea.keep();
            }
        }
        let t = at_tick(ticks);

        assert!(
            spray(&body, None, eye, t, &deep).is_empty(),
            "a spectrum body with no ocean invented a surface to spray off"
        );

        let thrown = spray(&body, Some(&sea), eye, t, &deep);
        assert!(!thrown.is_empty(), "a 25 m/s sea with spray = 8 threw nothing");
        assert_eq!(thrown.len() % SPRAY_CROWN, 0, "a partial crown");
        assert_eq!(thrown, spray(&body, Some(&sea), eye, t, &deep), "not reproducible");

        // The same sea, evolved identically and never kept: the ring is empty, every
        // birth is older than nothing, and no crown is thrown.
        let mut unkept = crate::ocean::Ocean::for_body(&body, 25.0, [1.0, 0.0]).expect("a cascade");
        for tick in 0..=ticks {
            unkept.evolve(at_tick(tick));
        }
        assert!(
            spray(&body, Some(&unkept), eye, t, &deep).is_empty(),
            "spray read tiles nobody kept"
        );
    }

    /// **The ring is a recording of the fixed step, so it cannot depend on how the
    /// caller got there.** `--sim N` in one jump loops the same `step`, but the property
    /// that matters is the one asserted here: filling the ring in two runs that visit the
    /// same ticks leaves the same droplets in the air.
    #[test]
    fn the_ring_does_not_care_how_the_ticks_were_dispatched() {
        use loom_scene::components::{WaveModel, WaterBody};
        let body = WaterBody {
            wave_model: WaveModel::Spectrum,
            spray: 8.0,
            fetch: Some(50_000.0),
            ..WaterBody::default()
        };
        let at_tick = |tick: u32| f32::from(u16::try_from(tick).expect("small")) / 60.0;
        let run = |chunk: u32| {
            let mut sea =
                crate::ocean::Ocean::for_body(&body, 25.0, [1.0, 0.0]).expect("a cascade");
            let mut tick = 0;
            while tick <= 96 {
                for _ in 0..chunk.min(97 - tick) {
                    sea.evolve(at_tick(tick));
                    if tick % 8 == 0 {
                        sea.keep();
                    }
                    tick += 1;
                }
            }
            spray(&body, Some(&sea), [0.0, 4.0, 0.0], at_tick(96), &deep)
        };

        let one_at_a_time = run(1);
        assert!(!one_at_a_time.is_empty(), "the sea threw nothing to compare");
        assert_eq!(one_at_a_time, run(97), "one jump left different spray than 97 steps");
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
