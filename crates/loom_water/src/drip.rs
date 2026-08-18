//! Drips: water leaving a lip one drop at a time — reference image 63.
//!
//! **Pure `f(where, speed, age, seed)`, with no history at all.** A drip source
//! releases drop `n` at `n / rate`; where that drop is, how big it is, whether
//! it has landed and what it threw up when it did are all algebra in `n` and
//! the clock. Nothing is stepped and nothing is stored, which is why a still at
//! `--sim 300` and a window that has been open for five seconds draw the same
//! drips, and why there is no warm-up.
//!
//! # Drop size is not a knob
//!
//! Tate's law with the Harkins–Brown correction: a drop detaches when its
//! weight overcomes the surface tension holding it to the lip's circumference,
//!
//! ```text
//! V = 2π·r·σ·ψ / (ρ·g),    σ = 0.0728 N/m,  ψ = 0.6,  ρ = 1000 kg/m³
//! ```
//!
//! so a 2.5 mm lip gives 0.070 mL and a **5.1 mm** drop, which is the size real
//! drips are. It is not exposed as a field, and the reason is the cube root:
//! `d ∝ r^⅓`, so **quadrupling the lip radius moves the drop by 58%**. A
//! diameter knob would be a knob whose whole useful range is a factor of two,
//! sitting next to a physical quantity that produces it for free.
//!
//! # Falling drops are not ballistic and the difference is visible
//!
//! A 5 mm drop reaches terminal velocity in a couple of metres. With
//! `v_t = 9.09 m/s` (Gunn & Kinzer):
//!
//! ```text
//! v(t) = v_t·tanh(g·t/v_t)          y(t) = (v_t²/g)·ln(cosh(g·t/v_t))
//! t_land(h) = (v_t/g)·arccosh(e^(g·h/v_t²))
//! ```
//!
//! Over 2.5 m that is **0.750 s arriving at 6.08 m/s**, against the ballistic
//! answer of 0.714 s at 7.00 m/s — five per cent of the time and fifteen of the
//! speed, and the speed is what sets the length of the streak the shutter
//! draws. [`tests`] asserts the tick, and asserts that it is *not* the
//! ballistic one.
//!
//! # Every drip splashes
//!
//! There is no splash threshold here and there should not be. The
//! Mundo–Cossali splash parameter `K = We^0.5·Re^0.25` crosses its 57.7
//! threshold for a 5 mm drop after a **0.15 mm** fall, so any drip that fell far
//! enough to be drawn has already crossed it. A gate would be a branch that is
//! always taken, with a threshold nobody could ever tune.

use crate::GRAVITY;
use crate::spray::Droplet;

/// Surface tension of water against air at 20 °C, N/m.
pub const SURFACE_TENSION: f32 = 0.0728;
/// Density of water, kg/m³.
pub const DENSITY: f32 = 1000.0;
/// Harkins–Brown correction: the fraction of the ideal Tate volume that
/// actually detaches. The rest stays on the lip and starts the next drop.
pub const HARKINS_BROWN: f32 = 0.6;
/// Terminal velocity of a millimetre-scale raindrop, m/s (Gunn & Kinzer 1949).
///
/// **A constant rather than a function of diameter.** Gunn & Kinzer's curve is
/// nearly flat from 4 mm up — 8.83 m/s at 4 mm, 9.09 at 5, 9.17 at 5.8 — and
/// [`drop_diameter`]'s cube root keeps every plausible lip inside that band.
/// Fitting the curve would be three coefficients producing a three per cent
/// correction on a quantity that already has a `tanh` in it.
pub const TERMINAL_SPEED: f32 = 9.09;

/// Seconds a satellite trails the main drop by. Rayleigh's fastest-growing
/// mode on a water ligament is `λ ≈ 9R`, which at these speeds is about one
/// drop diameter behind — see [`satellite_lag`].
pub const SATELLITE_SCALE: f32 = 1.0 / 3.0;

/// Seconds the neck between the lip and the drop survives after detachment.
///
/// **10.8 ms — 0.65 of a tick — so it is a two-frame authored shape and never a
/// simulation.** Pinch-off is a capillary singularity: the neck's radius goes
/// to zero in finite time with an exponent nothing here is going to integrate.
/// What it buys is that a drop does not appear from nothing; it is drawn
/// stretched back toward the lip it just left, for as long as one frame.
pub const PINCH_SECONDS: f32 = 0.0108;

/// Seconds the landing crown lives.
pub const CROWN_SECONDS: f32 = 0.28;

/// Droplets in the micro-crown a landing throws up.
///
/// **24, and it is a count rather than a coverage.** A 5 mm drop landing at
/// 6 m/s throws a rim that Rayleigh–Plateau drains into a few dozen ligaments;
/// two dozen is inside that and is what the reference photograph shows at the
/// foot of each drip.
pub const CROWN_DROPLETS: usize = 24;

/// The diameter of a drop leaving a lip of radius `r`, in metres.
///
/// Tate's law with Harkins–Brown, then the sphere of that volume.
#[must_use]
pub fn drop_diameter(lip_radius: f32) -> f32 {
    let r = lip_radius.max(1.0e-4);
    let volume = 2.0 * std::f32::consts::PI * r * SURFACE_TENSION * HARKINS_BROWN
        / (DENSITY * GRAVITY);
    (6.0 * volume / std::f32::consts::PI).powf(1.0 / 3.0)
}

/// Speed `t` seconds into a fall from rest, m/s.
#[must_use]
pub fn fall_speed(t: f32) -> f32 {
    TERMINAL_SPEED * (GRAVITY * t.max(0.0) / TERMINAL_SPEED).tanh()
}

/// Distance fallen in `t` seconds, metres.
#[must_use]
pub fn fall_distance(t: f32) -> f32 {
    let x = GRAVITY * t.max(0.0) / TERMINAL_SPEED;
    // `ln(cosh x)` directly overflows for large `x`; this form is the same
    // number and is stable, and a drip from a gantry is a large `x`.
    (TERMINAL_SPEED * TERMINAL_SPEED / GRAVITY) * (x + (-2.0 * x).exp().ln_1p() - core::f32::consts::LN_2)
}

/// Seconds to fall `h` metres, the inverse of [`fall_distance`].
///
/// **This is the number the acceptance test pins**, because it is where the
/// drag either exists or does not: 2.5 m is tick 45 with it and tick 43
/// without.
#[must_use]
pub fn time_to_fall(h: f32) -> f32 {
    let e = (GRAVITY * h.max(0.0) / (TERMINAL_SPEED * TERMINAL_SPEED)).exp();
    // `arccosh(e) = ln(e + sqrt(e² − 1))`, and `e ≥ 1` for any non-negative
    // height so the root is real.
    (TERMINAL_SPEED / GRAVITY) * (e + (e * e - 1.0).max(0.0).sqrt()).ln()
}

/// How far behind the main drop its satellite trails, in metres.
///
/// One drop diameter, which is what the Rayleigh mode `λ ≈ 9R` works out to on
/// the ligament a detaching drop leaves. Written as a function so the two
/// numbers cannot drift apart.
#[must_use]
pub fn satellite_lag(diameter: f32) -> f32 {
    diameter
}

/// Every droplet a drip source has in the air or on the ground right now.
///
/// `origin` is the lip, `floor_y` where the raycast at birth said the ground
/// is, `rate` drips per second, `lip_radius` what sets the drop size, `now` the
/// simulation clock in seconds, and `seed` what makes two sources in one scene
/// out of step.
///
/// **The whole population is enumerated from `now`, not accumulated.** Drop `n`
/// leaves at `n / rate`; the ones that can still be visible are the last
/// `(t_land + CROWN_SECONDS) · rate` of them, which is a small integer, so this
/// is a short loop rather than a system.
#[must_use]
pub fn drops(
    origin: [f32; 3],
    floor_y: f32,
    rate: f32,
    lip_radius: f32,
    now: f32,
    seed: u32,
) -> Vec<Droplet> {
    let mut out = Vec::new();
    if rate <= 0.0 || now < 0.0 {
        return out;
    }
    let height = (origin[1] - floor_y).max(0.0);
    if height <= 0.0 {
        return out;
    }
    let diameter = drop_diameter(lip_radius);
    let landing = time_to_fall(height);
    let life = landing + CROWN_SECONDS;

    // The interval of drop ordinals that can still be showing something.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let newest = (now * rate).floor() as i64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let oldest = ((now - life) * rate).ceil().max(0.0) as i64;

    for n in oldest..=newest {
        #[allow(clippy::cast_precision_loss)]
        let born = n as f32 / rate;
        let age = now - born;
        if age < 0.0 || age > life {
            continue;
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let salt = hash(seed ^ (n as u32).wrapping_mul(0x9E37_79B9));
        if age < landing {
            in_flight(&mut out, origin, diameter, age, landing, salt);
        } else {
            crown(&mut out, origin, floor_y, diameter, age - landing, salt);
        }
    }
    out
}

/// The main drop, its neck while it is still pinching off, and its satellite.
fn in_flight(
    out: &mut Vec<Droplet>,
    origin: [f32; 3],
    diameter: f32,
    age: f32,
    landing: f32,
    salt: u32,
) {
    let fallen = fall_distance(age);
    let speed = fall_speed(age);
    let at = |drop: f32| [origin[0], origin[1] - drop, origin[2]];
    // **Faded in over the pinch-off, never out.** A drip that dimmed as it fell
    // would be a drip evaporating; what it does instead is disappear into its
    // own crown, and the crown is drawn by the other branch.
    let fade = (age / PINCH_SECONDS).min(1.0);
    // **`fraction` is how far through the fall this drop is**, and the renderer
    // reads it as life progress: `drawn_at` ramps a droplet's opacity over the
    // first eighth of its life, so a drop reporting zero forever is a drop drawn
    // at zero opacity forever. Found by rendering the scene and seeing nothing
    // at all. It also gives the drop its forming-at-the-lip fade, over about
    // four centimetres of fall, which is the pinch-off the neck below draws.
    let through = (age / landing).clamp(0.0, 0.999);

    out.push(Droplet {
        position: at(fallen),
        velocity: [0.0, -speed, 0.0],
        fraction: through,
        scale: diameter,
        alpha: 1.0,
    });

    // **The neck, for one frame.** Drawn as a thin drop halfway back to the
    // lip: the shutter smear in the renderer stretches it along its own
    // velocity, which is what a pinching ligament looks like photographed.
    if age < PINCH_SECONDS {
        out.push(Droplet {
            position: at(fallen * 0.45),
            velocity: [0.0, -speed * 0.5, 0.0],
            fraction: through,
            scale: diameter * 0.35,
            alpha: 1.0 - fade,
        });
    }

    // The satellite: a third the diameter, one diameter behind. It exists on
    // most drips and not all, which is what the salt decides.
    if salt & 3 != 0 {
        let lag = satellite_lag(diameter);
        if fallen > lag {
            out.push(Droplet {
                position: at(fallen - lag),
                velocity: [0.0, -speed, 0.0],
                fraction: through,
                scale: diameter * SATELLITE_SCALE,
                alpha: 0.9,
            });
        }
    }
}

/// The micro-crown a landed drop threw up.
///
/// Ballistic, because at these sizes and this age air drag has moved a droplet
/// by microns: the whole crown is over in 0.28 s.
fn crown(
    out: &mut Vec<Droplet>,
    origin: [f32; 3],
    floor_y: f32,
    diameter: f32,
    age: f32,
    salt: u32,
) {
    let fraction = (age / CROWN_SECONDS).min(1.0);
    // The impact speed sets how hard the rim is thrown. A third of it is the
    // usual measured ratio for the ejecta of a low-Weber impact, and it is
    // where 24 droplets from a 5 mm drop reach a few centimetres.
    let launch = fall_speed(time_to_fall((origin[1] - floor_y).max(0.0))) * 0.33;
    for i in 0..CROWN_DROPLETS {
        #[allow(clippy::cast_possible_truncation)]
        let jitter = hash(salt ^ (i as u32).wrapping_mul(0x85EB_CA6B));
        let wobble = f32::from((jitter >> 8) as u16) / f32::from(u16::MAX);
        #[allow(clippy::cast_precision_loss)]
        let angle = (i as f32 / CROWN_DROPLETS as f32 + wobble * 0.03)
            * std::f32::consts::TAU;
        // Between 55° and 75° off the ground, which is the rim angle a crown
        // stands at before it collapses.
        let elevation = 0.96 + wobble * 0.35;
        let speed = launch * (0.7 + wobble * 0.6);
        let (vx, vz) = (angle.cos() * elevation.cos(), angle.sin() * elevation.cos());
        let vy = elevation.sin();
        let v = [speed * vx, speed * vy, speed * vz];
        out.push(Droplet {
            position: [
                origin[0] + v[0] * age,
                floor_y + (v[1] * age - 0.5 * GRAVITY * age * age).max(0.0),
                origin[2] + v[2] * age,
            ],
            velocity: [v[0], v[1] - GRAVITY * age, v[2]],
            fraction,
            // A quarter to a half the parent, and the spread is what stops the
            // ring reading as a necklace — the same argument
            // `spray::Droplet::scale` makes at length.
            // **Three tenths to three quarters of the parent**, which is the
            // measured band for secondary droplets off a low-Weber impact with
            // its top end rounded up rather than down.
            //
            // **Be honest about what this is worth on screen.** A droplet off a
            // 5 mm drop is one to four millimetres, which subtends under a
            // pixel at every camera distance a scene would use: zooming into
            // `dripping.loom`'s landing zone at 960x600 finds two or three
            // specks. The crown is right in the data and nearly invisible in
            // the picture, and what would make a landing read is a wet mark on
            // the floor — a large, low-contrast feature — which is not built.
            // Inflating the droplets instead would be drawing marbles.
            //
            // The spread is what stops the ring reading as a necklace, which is
            // the argument `spray::Droplet::scale` makes at length.
            scale: diameter * (0.30 + wobble * 0.45),
            alpha: 1.0 - fraction,
        });
    }
}

/// The integer hash the whole module salts with.
///
/// Written out here rather than taken from `loom_field::noise`: that one is
/// **frozen ABI** with a Slang twin compared exactly, and nothing about drips
/// crosses to the GPU. Borrowing it would put a caller on a constant whose
/// documentation says it must never change for a reason that does not apply.
fn hash(x: u32) -> u32 {
    let mut x = x;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Tate's law gives a 5 mm drop from a 2.5 mm lip**, which is what a real
    /// drip is, and the whole reason the diameter is not a field.
    #[test]
    fn a_two_and_a_half_millimetre_lip_makes_a_five_millimetre_drop() {
        let d = drop_diameter(0.0025);
        assert!((d - 0.0051).abs() < 0.0002, "d = {d}");
    }

    /// The cube root is the argument against exposing the diameter: four times
    /// the lip is 59% more drop, not four times.
    #[test]
    fn quadrupling_the_lip_moves_the_drop_by_a_cube_root() {
        let ratio = drop_diameter(0.010) / drop_diameter(0.0025);
        assert!((ratio - 4.0_f32.powf(1.0 / 3.0)).abs() < 1.0e-3, "ratio = {ratio}");
    }

    /// **The acceptance number, and it distinguishes drag from no drag.** 2.5 m
    /// is tick 45 with terminal velocity in the model and tick 43 without, so a
    /// test that only checked "about 0.7 s" would pass on the wrong physics.
    #[test]
    fn two_and_a_half_metres_lands_on_tick_forty_five_not_forty_three() {
        let t = time_to_fall(2.5);
        assert!((t - 0.750).abs() < 0.002, "t_land = {t}");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tick = (t * 60.0).round() as u32;
        assert_eq!(tick, 45, "t_land = {t} s");

        let ballistic = (2.0 * 2.5 / GRAVITY).sqrt();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ballistic_tick = (ballistic * 60.0).round() as u32;
        assert_eq!(ballistic_tick, 43, "the ballistic answer moved: {ballistic}");

        let v = fall_speed(t);
        assert!((v - 6.08).abs() < 0.02, "impact speed {v}");
    }

    /// `fall_distance` and `time_to_fall` are each other's inverse, which is
    /// what makes the landing land where the raycast said the floor was.
    #[test]
    fn the_fall_integrates_and_inverts_consistently() {
        for h in [0.05_f32, 0.5, 2.5, 12.0, 60.0] {
            let back = fall_distance(time_to_fall(h));
            assert!((back - h).abs() < 1.0e-3 * h.max(1.0), "{h} m came back as {back}");
        }
    }

    /// **Landing within 2 cm of the raycast hit**, which is the acceptance
    /// criterion. It is exact by construction — the crown is anchored at
    /// `floor_y` — and the test exists because "by construction" is what the
    /// last three bugs in this project were also true of.
    #[test]
    fn a_drip_lands_on_the_floor_the_raycast_found() {
        let origin = [1.0, 3.0, -2.0];
        let floor = 0.5;
        let landing = time_to_fall(origin[1] - floor);
        let drops = drops(origin, floor, 4.0, 0.0025, landing + 0.001, 7);
        let landed: Vec<_> = drops.iter().filter(|d| d.velocity[0] != 0.0).collect();
        assert!(!landed.is_empty(), "nothing had landed at t = t_land");
        for d in landed {
            assert!(
                (d.position[1] - floor).abs() < 0.02,
                "a crown droplet is at y = {}, floor is {floor}",
                d.position[1]
            );
        }
    }

    /// **Nothing is stepped**, so the same clock gives the same drips however
    /// it was reached. This is what lets the headless still and the window
    /// agree without either warming anything up.
    #[test]
    fn the_population_is_a_function_of_the_clock_alone() {
        let a = drops([0.0, 2.5, 0.0], 0.0, 3.0, 0.0025, 4.25, 11);
        let b = drops([0.0, 2.5, 0.0], 0.0, 3.0, 0.0025, 4.25, 11);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.position, y.position);
        }
        assert!(!a.is_empty(), "a 3 Hz source over a 2.5 m drop is never empty");
    }

    /// The satellite trails by a diameter and is a third the size — Rayleigh's
    /// mode, and the thing that makes a drip read as a drip rather than a bead.
    #[test]
    fn a_drip_has_a_satellite_behind_it() {
        let origin = [0.0, 2.5, 0.0];
        // Late enough that the satellite has cleared the lip, early enough that
        // nothing has landed.
        let mid = drops(origin, 0.0, 2.0, 0.0025, 0.4, 3);
        let in_air: Vec<_> = mid.iter().filter(|d| d.velocity[0] == 0.0).collect();
        assert!(in_air.len() >= 2, "no satellite: {in_air:?}");
        let big = in_air.iter().map(|d| d.scale).fold(0.0_f32, f32::max);
        let small = in_air.iter().map(|d| d.scale).fold(f32::MAX, |a, b| a.min(b));
        assert!((small / big - SATELLITE_SCALE).abs() < 1.0e-4, "{small} vs {big}");
    }
}
