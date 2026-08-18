//! The falling sheet — a **closed-form** waterfall, from open-channel hydraulics.
//!
//! A nappe is the sheet of water that leaves a weir crest and falls free. It is
//! not simulated here and does not need to be: everything a renderer or a force
//! wants about it — where it is at depth `Z`, how fast, how thick, how much air
//! it has entrained, and where it stops being a sheet at all — is an algebraic
//! function of one authored number, the discharge per metre of lip `q` (m²/s).
//!
//! # The formulae, and where they come from
//!
//! Critical depth at the brink of a free overfall is the standard result for
//! critical flow, `y_c = (q²/g)^⅓`. The depth *at* the brink is lower than
//! critical because the pressure distribution is no longer hydrostatic; Rouse
//! (1936) measured **`y_b = 0.715 · y_c`** and that ratio has held since.
//! Continuity then fixes the exit velocity, `V_i = q / y_b`, and the Froude
//! number that comes out is `1.65` for every `q` — a *consequence* of Rouse's
//! coefficient, not a second knob. [`Nappe::froude`] asserts it.
//!
//! Free fall from there is ballistic, so the throw and the jet velocity are
//! `X(Z) = V_i√(2Z/g)` and `V_j(Z) = √(V_i² + 2gZ)`. Continuity again gives the
//! **water** thickness `D_j = y_b·V_i / V_j` — the sheet thins as it speeds up.
//!
//! The *outer* thickness does not thin, it grows: turbulence at the brink
//! throws droplets off both faces and the sheet spreads linearly,
//! `D_out(Z) = y_b + 2·δ_out·Z` (Ervine & Falvey). The gap between the two is
//! air, which is what [`NappeAt::aeration`] returns and what makes a waterfall
//! white rather than clear.
//!
//! **Break-up** is where the inner core runs out: solving `D_j(Z) = 2δ_in·√(2gZ)`
//! for `Z` gives [`Nappe::break_length`], `Z_b = (y_b·V_i / (2δ_in√(2g)))^⅔`.
//! Below it the fall is no longer a sheet at all and is drawn as a curtain of
//! droplets (Bollaert 2004).
//!
//! # The two δ's are authored knobs, and that is honest rather than lazy
//!
//! Bollaert relates the inner spread to the turbulence intensity at the brink,
//! `δ_in ≈ 0.38·Tu`, and separately quotes half-angles measured on prototypes.
//! **Those two statements do not reconcile**: the angles imply spreads several
//! times what `0.38·Tu` gives at any plausible `Tu`, and five independent
//! attempts to derive one from the other in the design pass all failed, by
//! different amounts. So they are exposed as `spread` and `breakup` on
//! `loom_scene::components::Cascade`, with the literature's *ranges* as their
//! schema bounds and the middle of those ranges as the defaults. A derivation
//! written down here would be a fabricated one.
//!
//! # Discrimination, which is the whole point of the closed form
//!
//! Two reference waterfalls, told apart by `Z_b` alone:
//!
//! | | `q` | `Z_b` | at the pool | reads as |
//! | --- | --- | --- | --- | --- |
//! | stone spout | 0.10 | 1.31 m | `Z ≈ 1.5 m > Z_b` | translucent, stringy last third |
//! | canyon fall | 2.0 | 9.7 m | `Z ≈ 6 m < Z_b` | white, coherent, unbroken |
//!
//! One authored number moves a scene between those two pictures, which is what
//! a knob per look would never do.

use crate::GRAVITY;

/// A nappe, resolved from its discharge.
///
/// Cheap to construct — three multiplies and a cube root — so it is built where
/// it is needed rather than cached, on both sides of the CPU/GPU line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nappe {
    /// Depth of the water at the brink, in metres. `0.715 · y_c`.
    pub brink_depth: f32,
    /// Velocity leaving the brink, m/s. Horizontal.
    pub exit_speed: f32,
    /// Linear half-spread of the outer surface, per metre fallen.
    pub spread: f32,
    /// Linear half-spread of the inner core, per metre fallen.
    pub breakup: f32,
}

/// What the sheet is doing at one depth below the lip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NappeAt {
    /// Horizontal throw from the lip, in metres.
    pub throw: f32,
    /// Jet velocity, m/s — the streaks scroll at this.
    pub speed: f32,
    /// Thickness of the **water**, in metres.
    pub water_thickness: f32,
    /// Thickness of the whole air-and-water sheet, in metres.
    pub outer_thickness: f32,
    /// Fraction of the sheet that is air, `0..1`. This is what makes it white.
    pub aeration: f32,
}

/// The brink state for a discharge of `q` m²/s.
///
/// `q` is clamped away from zero: a cascade with no discharge is not a thin
/// cascade, it is no cascade, and the caller decides that by not drawing it.
/// Zero here would divide.
#[must_use]
pub fn brink(discharge: f32, spread: f32, breakup: f32) -> Nappe {
    let q = discharge.max(1.0e-4);
    // `y_c = (q²/g)^⅓`. `powf(1/3)` rather than `cbrt` because the Slang half
    // has no `cbrt` and this is the operation both sides must perform — the
    // agreement test compares the result, not the intent.
    let critical = (q * q / GRAVITY).powf(1.0 / 3.0);
    let brink_depth = 0.715 * critical;
    Nappe {
        brink_depth,
        exit_speed: q / brink_depth,
        spread: spread.max(0.0),
        breakup: breakup.max(1.0e-5),
    }
}

impl Nappe {
    /// The Froude number at the brink. `1.65` for every discharge, by
    /// construction — `V_i/√(g·y_b)` with `y_b = 0.715·(q²/g)^⅓` has no `q` left
    /// in it. Exposed because it falsifies a typo in [`brink`] without needing a
    /// reference table.
    #[must_use]
    pub fn froude(self) -> f32 {
        self.exit_speed / (GRAVITY * self.brink_depth).sqrt()
    }

    /// Depth below the lip at which the coherent core is used up, in metres.
    ///
    /// **The discrimination number.** Above it the fall is a sheet; below it, a
    /// curtain of drops. See the module table.
    #[must_use]
    pub fn break_length(self) -> f32 {
        let numerator = self.brink_depth * self.exit_speed;
        let denominator = 2.0 * self.breakup * (2.0 * GRAVITY).sqrt();
        (numerator / denominator).powf(2.0 / 3.0)
    }

    /// The sheet at `z` metres below the lip. Negative `z` is clamped to the lip
    /// rather than reflected, so a strip that overshoots by a vertex degenerates
    /// instead of turning the fall inside out.
    #[must_use]
    pub fn at(self, z: f32) -> NappeAt {
        let z = z.max(0.0);
        let speed = (self.exit_speed * self.exit_speed + 2.0 * GRAVITY * z).sqrt();
        let water = self.brink_depth * self.exit_speed / speed;
        let outer = self.brink_depth + 2.0 * self.spread * z;
        NappeAt {
            throw: self.exit_speed * (2.0 * z / GRAVITY).sqrt(),
            speed,
            water_thickness: water,
            outer_thickness: outer,
            // Clamped at zero: `D_out` is never below `D_j` for a non-negative
            // spread, but the subtraction is the one place a future knob could
            // make it so, and a negative aeration would read as a hole in the
            // sheet rather than as an error.
            aeration: (1.0 - water / outer).max(0.0),
        }
    }
}

/// The Slang half, emitted verbatim into the generated shader.
///
/// **This and the Rust above are one thing written twice**, on exactly the
/// precedent [`crate::slang`] sets and for exactly its reason: the nappe is
/// force-capable — a fall pushes what stands under it — so the number a force
/// would read and the number the vertex shader draws must be the same number.
/// `slang_agreement` compiles this string and compares them numerically.
///
/// Never hand-write it into `scene.slang` and never edit the generated file:
/// that changes the GPU alone, which is the divergence the generator exists to
/// make impossible (ADR 0006).
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The falling sheet. The Rust half is `loom_water::nappe`; they are one
// implementation written twice, and `slang_agreement` compares them.
//
// Never edit this by hand: it is emitted from the Rust.

struct LoomNappe {
    float brink_depth;
    float exit_speed;
    float spread;
    float breakup;
};

struct LoomNappeAt {
    float throwX;
    float speed;
    float water_thickness;
    float outer_thickness;
    float aeration;
};

LoomNappe loom_nappe_brink(float discharge, float spread, float breakup) {
    float q = max(discharge, 1.0e-4);
    float critical = pow(q * q / 9.81, 1.0 / 3.0);
    LoomNappe n;
    n.brink_depth = 0.715 * critical;
    n.exit_speed = q / n.brink_depth;
    n.spread = max(spread, 0.0);
    n.breakup = max(breakup, 1.0e-5);
    return n;
}

// Depth at which the coherent core is used up. Past it the sheet is drawn as
// droplets rather than as a surface.
float loom_nappe_break_length(LoomNappe n) {
    float numerator = n.brink_depth * n.exit_speed;
    float denominator = 2.0 * n.breakup * sqrt(2.0 * 9.81);
    return pow(numerator / denominator, 2.0 / 3.0);
}

LoomNappeAt loom_nappe_at(LoomNappe n, float z) {
    z = max(z, 0.0);
    LoomNappeAt s;
    s.speed = sqrt(n.exit_speed * n.exit_speed + 2.0 * 9.81 * z);
    s.water_thickness = n.brink_depth * n.exit_speed / s.speed;
    s.outer_thickness = n.brink_depth + 2.0 * n.spread * z;
    s.throwX = n.exit_speed * sqrt(2.0 * z / 9.81);
    s.aeration = max(1.0 - s.water_thickness / s.outer_thickness, 0.0);
    return s;
}
"#
}

#[cfg(test)]
mod tests {
    use super::{Nappe, brink};
    use loom_scene::components::Cascade;

    fn nappe(q: f32) -> Nappe {
        let d = Cascade::default();
        brink(q, d.spread, d.breakup)
    }

    /// **The discrimination table, which is the reason the closed form exists.**
    ///
    /// One authored number moves the picture between reference image 60 (a stone
    /// spout, stringy in its last third) and image 62 (a canyon fall, white and
    /// coherent all the way down) — and it does it by moving `Z_b` past the
    /// pool, not by any look knob.
    #[test]
    fn break_length_tells_the_two_reference_waterfalls_apart() {
        let spout = nappe(0.10);
        let canyon = nappe(2.0);
        assert!(
            (spout.break_length() - 1.31).abs() < 0.01,
            "Z_b(0.10) = {}",
            spout.break_length()
        );
        assert!(
            (canyon.break_length() - 9.7).abs() < 0.05,
            "Z_b(2.0) = {}",
            canyon.break_length()
        );
        // The spout's 1.5 m pool is BELOW its break-up; the canyon's 6 m fall is
        // above its own. That inequality is the table.
        assert!(spout.break_length() < 1.5);
        assert!(canyon.break_length() > 6.0);
    }

    /// The canyon fall is four fifths air by the time it reaches the pool. This
    /// is the number the acceptance names, and it lands on it.
    #[test]
    fn a_big_fall_is_four_fifths_air_by_the_time_it_lands() {
        let a = nappe(2.0).at(6.0);
        assert!((a.aeration - 0.80).abs() < 0.01, "aeration = {}", a.aeration);
    }

    /// **A thin sheet aerates FASTER, not slower**, and the design pass expected
    /// the opposite.
    ///
    /// It predicted `aeration(1.5, q = 0.1) ≈ 0.42`. That is unreachable:
    /// `1 − D_j/D_out` there is `0.890`, and 0.42 would need `D_out < y_b` — a
    /// sheet thinner than the brink it left, which a non-negative linear spread
    /// can never produce. The physics runs the other way: `D_out` grows by the
    /// same `2δ_out·Z` whatever the discharge, so it swamps a small `y_b`
    /// sooner. What tells the two waterfalls apart is `Z_b` above, not this.
    #[test]
    fn a_thin_spout_is_more_broken_up_than_a_big_fall_not_less() {
        let spout = nappe(0.10).at(1.5);
        assert!((spout.aeration - 0.890).abs() < 0.005, "aeration = {}", spout.aeration);
        assert!(spout.aeration > nappe(2.0).at(6.0).aeration);
    }

    /// Rouse's coefficient, checked through its consequence rather than by
    /// reading `0.715` back out of the code.
    #[test]
    fn the_brink_froude_number_is_one_point_six_five_for_every_discharge() {
        for q in [0.02_f32, 0.1, 0.5, 2.0, 12.0] {
            let f = nappe(q).froude();
            assert!((f - 1.65).abs() < 0.01, "Fr({q}) = {f}");
        }
    }

    /// Continuity: `V·D` is conserved down the fall, because that is all `D_j`
    /// is. A thickness that drifted from it would be a typo the aeration curve
    /// would hide.
    #[test]
    fn the_water_flux_is_conserved_down_the_fall() {
        let n = nappe(0.4);
        let q = n.brink_depth * n.exit_speed;
        for z in [0.0_f32, 0.5, 3.0, 20.0] {
            let a = n.at(z);
            assert!(
                (a.speed * a.water_thickness - q).abs() < 1.0e-4,
                "flux at {z} m is {}",
                a.speed * a.water_thickness
            );
        }
    }

    /// The Slang half has to carry the same constants. The numeric comparison
    /// lives in `tests/slang_agreement.rs` and needs `slangc`; this catches a
    /// deletion without one.
    #[test]
    fn the_slang_half_uses_the_same_constants() {
        let s = super::slang();
        for needle in ["0.715", "9.81", "loom_nappe_at", "loom_nappe_break_length"] {
            assert!(s.contains(needle), "the Slang half is missing `{needle}`");
        }
    }
}
