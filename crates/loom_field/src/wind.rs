//! Sampling the wind, with shelter.
//!
//! [`crate::wind`] is the raw field: what the air would do over open ground.
//! This is what a caller actually wants — that field, reduced where terrain
//! stands in its way.
//!
//! **Sheltering comes from S3 and nowhere else.** `loom_voxel::exposure`
//! answers "how open is this direction" for rain, for audio and for gameplay;
//! wind asking the same function is what stops a hollow that blocks rain from
//! being breezy, with nothing in the code to blame.

use crate::{Field, Params};

/// A wind field plus the parameters it reads, ready to sample.
///
/// Holds the built tree because assembling it is not free and a simulation
/// samples the wind many times per tick.
#[derive(Debug)]
pub struct Wind {
    field: Field,
    params: Params,
}

impl Wind {
    /// Build from authored values.
    ///
    /// `direction_degrees` is clockwise from +X, the direction the wind blows
    /// *toward*. Converted to a unit vector here, once, at the boundary — the
    /// field takes the vector because an angle wraps at ±180° and a wrap is a
    /// snap, which is precisely what P1's slew-rate criterion forbids.
    #[must_use]
    pub fn new(
        direction_degrees: f32,
        speed: f32,
        gustiness: f32,
        turbulence: f32,
        ground_drag: f32,
    ) -> Self {
        let radians = direction_degrees.to_radians();
        let mut params = crate::wind_defaults();
        params.set("dir_x", radians.cos());
        params.set("dir_z", radians.sin());
        params.set("speed", speed);
        params.set("gustiness", gustiness);
        params.set("turbulence", turbulence);
        params.set("ground_drag", ground_drag);
        Self { field: crate::wind(), params }
    }

    /// The open-ground wind at a point and time.
    #[must_use]
    pub fn at(&self, p: [f32; 3], t: f32) -> [f32; 3] {
        self.field.eval_with(p, t, &self.params)
    }

    /// The wind at a point, reduced by how sheltered that point is.
    ///
    /// `exposure` is S3's number: 1 in the open, 0 sealed. Applied as a plain
    /// multiplier rather than something cleverer because the honest claim is
    /// "less wind gets in", and any curve on top would be a guess that the
    /// visuals would then be tuned around.
    ///
    /// **Not clamped to the open-ground direction.** A sheltered point gets
    /// less wind, not differently-directed wind — modelling flow *around* an
    /// obstacle needs a solver, which is a different project.
    #[must_use]
    pub fn sheltered(&self, p: [f32; 3], t: f32, exposure: f32) -> [f32; 3] {
        let shelter = exposure.clamp(0.0, 1.0);
        let open = self.at(p, t);
        [open[0] * shelter, open[1] * shelter, open[2] * shelter]
    }

    /// The parameters, for the GPU side to upload.
    #[must_use]
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// The parameters flattened in the order the generated shader reads them.
    #[must_use]
    pub fn params_array(&self) -> Vec<f32> {
        self.field.params_array(&self.params)
    }
}

impl Default for Wind {
    /// Beaufort 4 from the west — the lowest wind that visibly moves foliage.
    fn default() -> Self {
        Self::new(0.0, 5.5, 1.0, 0.8, 0.45)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authored_angle_points_the_wind() {
        // 90° clockwise from +X is +Z.
        let wind = Wind::new(90.0, 6.0, 1.0, 0.0, 1.0);

        let v = wind.at([4.0, 2.0, 4.0], 1.0);

        assert!(v[2].abs() > v[0].abs() * 4.0, "{v:?}");
        assert!(v[2] > 0.0, "the wind blows toward +Z, not away: {v:?}");
    }

    /// **The S3 tie-in.** A sheltered point gets less wind, and a sealed one
    /// gets none.
    #[test]
    fn shelter_reduces_the_wind() {
        let wind = Wind::default();
        let p = [3.0, 1.0, 2.0];

        let open = wind.sheltered(p, 2.0, 1.0);
        let half = wind.sheltered(p, 2.0, 0.5);
        let sealed = wind.sheltered(p, 2.0, 0.0);

        assert_eq!(open, wind.at(p, 2.0), "full exposure is the open field");
        assert!((half[0] - open[0] * 0.5).abs() < 1e-6, "{half:?} vs {open:?}");
        assert_eq!(sealed, [0.0, 0.0, 0.0]);
    }

    /// An exposure outside `[0, 1]` cannot amplify the wind. The value comes
    /// from another crate, and a field that could be scaled past its authored
    /// speed by a caller's arithmetic error is a bug with no obvious symptom.
    #[test]
    fn a_nonsense_exposure_cannot_amplify() {
        let wind = Wind::default();
        let p = [1.0, 1.0, 1.0];

        let open = wind.at(p, 0.5);
        let over = wind.sheltered(p, 0.5, 4.0);
        let under = wind.sheltered(p, 0.5, -2.0);

        assert_eq!(over, open);
        assert_eq!(under, [0.0, 0.0, 0.0]);
    }

    /// The flattened array is what the shader reads, so it has to match the
    /// field's own parameter order rather than the order they were set in.
    #[test]
    fn the_flattened_parameters_follow_the_fields_order() {
        let wind = Wind::new(0.0, 7.0, 1.0, 0.5, 0.6);
        let field = crate::wind();

        let flat = wind.params_array();

        assert_eq!(flat.len(), field.params().len());
        for (index, name) in field.params().iter().enumerate() {
            assert_eq!(flat[index], wind.params().get(name), "slot {index} is `{name}`");
        }
    }
}
