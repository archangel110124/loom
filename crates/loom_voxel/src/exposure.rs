//! How much of a direction a point can see — sky, wind, or anything else.
//!
//! **One function, four callers.** Rain needs "is this point exposed to sky",
//! wind needs "how sheltered is this point", audio needs "interior or
//! exterior", and gameplay wants the same answer the visuals used. Written
//! four times those drift, and the drift has no symptom: rain falls through a
//! roof that muffles sound and blocks wind, and nothing errors.

use crate::{Volume, dequantize};

/// Samples along the ray. **A count, never a tolerance.**
///
/// The loop runs exactly this many times regardless of what it finds. A march
/// that terminated on a float comparison — "close enough to the surface",
/// "occlusion is basically 1 now" — could take a different number of steps on
/// a different platform, and this value is in the deterministic simulation and
/// therefore in the hash.
pub const EXPOSURE_STEPS: usize = 128;

/// Distance between samples, in voxels.
///
/// **Half a voxel, because the thinnest feature the field can hold is one.**
/// Sampling at the feature size is not enough — a sample can land half a step
/// off the deepest point of a one-voxel slab, read it as barely outside, and
/// report a roof as 84% blocked instead of sealed. That is the ordinary
/// sampling-rate argument: to never miss a feature of width w, sample at w/2.
/// It was found by a test, not by reasoning, and the symptom was a roof that
/// sheltered at some heights and leaked at others.
const STEP: f32 = 0.5;

/// The field at a world position, in voxels, trilinearly interpolated.
///
/// **Interpolated rather than nearest.** Nearest-neighbour makes exposure jump
/// as an entity walks a fraction of a voxel, which is the popping this whole
/// module exists to avoid — and it would make the "smooth fraction" the spec
/// asks for a staircase with 127 steps.
///
/// Outside the volume reads as air, matching [`Volume::voxel`]. A ray that
/// leaves the world sees sky, which is the only sensible answer and the reason
/// the finite reach below is safe.
#[must_use]
pub fn sample(volume: &Volume, world: [f32; 3]) -> f32 {
    // `world_of` places voxel k at `(k + 0.5) * voxel_size`, so the inverse
    // carries the same half-cell offset. Getting this wrong shifts the whole
    // field by half a voxel against the mesh and the collider, which is the
    // trap `solid_cells` documents from the other side.
    let grid = [
        world[0] / volume.voxel_size - 0.5,
        world[1] / volume.voxel_size - 0.5,
        world[2] / volume.voxel_size - 0.5,
    ];

    let base = [grid[0].floor(), grid[1].floor(), grid[2].floor()];
    let frac = [grid[0] - base[0], grid[1] - base[1], grid[2] - base[2]];

    // A negative coordinate floors below zero, which `usize` cannot hold. The
    // cast is guarded rather than clamped: clamping would smear the edge voxel
    // outward forever, so a point far outside would read as whatever the
    // boundary happens to be instead of as air.
    let corner = |dx: usize, dy: usize, dz: usize| -> f32 {
        let (x, y, z) = (base[0] + dx as f32, base[1] + dy as f32, base[2] + dz as f32);
        if x < 0.0 || y < 0.0 || z < 0.0 {
            // Outside is air, saturated.
            return 1.0;
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        dequantize(volume.voxel(x as usize, y as usize, z as usize))
    };

    // Trilinear: lerp along x, then y, then z.
    let lerp = |a: f32, b: f32, t: f32| (b - a).mul_add(t, a);
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), frac[0]);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), frac[0]);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), frac[0]);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), frac[0]);
    let y0 = lerp(x00, x10, frac[1]);
    let y1 = lerp(x01, x11, frac[1]);
    lerp(y0, y1, frac[2])
}

/// Width of the soft edge, in voxels.
///
/// The ramp runs from fully blocked at the surface to fully clear this far
/// outside it. **Half a voxel, and the narrowness is the honest limit of a
/// single ray**: `SDF_SCALE` spreads the whole `i8` range across ±1 voxel, so
/// the field saturates there and no wider gradient is representable. A genuine
/// wide penumbra — "how much of the sky can this point see" — needs a cone of
/// rays averaged, which costs N marches. Not built, because nothing has asked
/// for it; this is where to start if something does.
const SOFTNESS: f32 = 0.5;

/// How solid the field is at a sampled distance, from 0 (air) to 1 (rock).
///
/// **Asymmetric on purpose.** The obvious ramp is centred on the surface —
/// `0.5 - d` — and it is wrong: it only reaches fully-blocked deep inside
/// solid, so a roof one voxel thick never gets a sample deep enough and leaks
/// about 10% of the sky through it. Anything at or below the surface is
/// blocked; the softening applies only to near misses on the outside.
///
/// The saturation is also why this module marches a fixed step rather than
/// sphere tracing. There is no long-range distance here to take a big step on,
/// and a recalled "it is an SDF, so sphere trace it" would step through walls.
fn occupancy(distance_in_voxels: f32) -> f32 {
    (1.0 - distance_in_voxels / SOFTNESS).clamp(0.0, 1.0)
}

/// Fraction of the direction that is open, from 0 (sealed) to 1 (clear).
///
/// Marches [`EXPOSURE_STEPS`] samples [`STEP`] voxels apart and reports how
/// open the most obstructed of them was — so the reach is 64 voxels, and a
/// roof beyond that reads as sky.
///
/// **The most obstructed sample, not an accumulation along the ray.** Adding
/// up absorption would make a thick roof block more than a thin one, and for
/// every question this answers — is rain landing here, is this point sheltered
/// from wind, is this indoors — one roof is enough. Two roofs are still one
/// roof.
///
/// The whole loop runs every time. There is an obvious early exit once a
/// sample is fully solid — it cannot change the answer — and it is left out
/// deliberately: the cost is then a function of the geometry, and a fixed cost
/// is worth more here than a saved microsecond in a value the simulation hash
/// depends on.
#[must_use]
pub fn exposure(volume: &Volume, from: [f32; 3], direction: [f32; 3]) -> f32 {
    let length = direction[0]
        .mul_add(direction[0], direction[1].mul_add(direction[1], direction[2] * direction[2]))
        .sqrt();
    // A zero direction has no exposure to report. Returning 1 rather than 0 so
    // a caller that forgets to set one gets "unoccluded", which is inert,
    // rather than "sealed", which would silently stop rain everywhere.
    if length <= 0.0 || !length.is_finite() {
        return 1.0;
    }
    let scale = volume.voxel_size * STEP / length;
    let step = [direction[0] * scale, direction[1] * scale, direction[2] * scale];

    let mut blocked = 0.0_f32;
    for i in 0..EXPOSURE_STEPS {
        // Half a step in, so the first sample is just off the origin: starting
        // at zero would read the caller's own position, and an entity standing
        // with its feet in the ground would report itself sealed.
        #[allow(clippy::cast_precision_loss)]
        let t = i as f32 + 0.5;
        let at = [
            step[0].mul_add(t, from[0]),
            step[1].mul_add(t, from[1]),
            step[2].mul_add(t, from[2]),
        ];
        blocked = blocked.max(occupancy(sample(volume, at)));
    }

    1.0 - blocked
}

/// Exposure straight up. What rain asks.
#[must_use]
pub fn sky_exposure(volume: &Volume, from: [f32; 3]) -> f32 {
    exposure(volume, from, [0.0, 1.0, 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CsgMode, VoxelOp};

    /// A 64³ world with a flat slab roof over the middle.
    fn roofed() -> Volume {
        let mut volume = Volume::new([2, 2, 2], 1.0);
        volume.bake(&[VoxelOp::Box {
            center: [32.0, 20.0, 32.0],
            half_extents: [10.0, 1.0, 10.0],
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        }]);
        volume
    }

    #[test]
    fn open_sky_is_fully_exposed() {
        let volume = roofed();

        // Well outside the slab's footprint.
        let open = sky_exposure(&volume, [5.0, 10.0, 5.0]);

        assert!(open > 0.99, "open sky read {open}");
    }

    #[test]
    fn under_a_roof_is_not_exposed() {
        let volume = roofed();

        let sheltered = sky_exposure(&volume, [32.0, 10.0, 32.0]);

        assert!(sheltered < 0.01, "under a solid roof read {sheltered}");
    }

    /// **The S3 exit criterion.** Carving the overhang away raises exposure at
    /// the same point, in the same tick — the field is re-marched, not cached,
    /// so destruction is reflected by construction.
    #[test]
    fn carving_the_roof_away_raises_exposure() {
        let mut volume = roofed();
        let point = [32.0, 10.0, 32.0];
        let before = sky_exposure(&volume, point);

        volume.edit(&VoxelOp::Box {
            center: [32.0, 20.0, 32.0],
            half_extents: [4.0, 3.0, 4.0],
            mode: CsgMode::Subtract,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        });
        let after = sky_exposure(&volume, point);

        assert!(before < 0.01, "should have started sheltered, read {before}");
        assert!(after > 0.99, "should be open after carving, read {after}");
    }

    /// **Smooth, not boolean.** The whole reason S3 marches the SDF rather
    /// than asking a ray query: at the lip of an overhang the answer has to be
    /// a fraction, or wind and rain pop between two states as an entity walks
    /// under a ledge.
    #[test]
    fn the_edge_of_an_overhang_is_a_gradient() {
        let volume = roofed();

        // Walk out from under the slab, across its edge at x = 42.
        let readings: Vec<f32> = (0_u8..40)
            .map(|i| {
                let x = 40.0 + f32::from(i) * 0.1;
                sky_exposure(&volume, [x, 10.0, 32.0])
            })
            .collect();

        let partial = readings.iter().filter(|e| **e > 0.05 && **e < 0.95).count();
        assert!(
            partial >= 3,
            "crossing the edge gave no intermediate values: {readings:?}"
        );
    }

    /// **The tunnelling case, and the thinnest roof that exists at all.**
    ///
    /// A slab is in the field only where a voxel *centre* falls strictly
    /// inside it: `bake_chunk` counts a voxel solid at `q < 0`, so a slab
    /// spanning exactly 19.5 to 20.5 puts every centre exactly on the surface
    /// at `q == 0`, nothing is solid, and the chunk collapses to `Air` — the
    /// roof vanishes. This one is placed to catch exactly one centre, which is
    /// the thinnest representable overhang.
    ///
    /// It has to seal. A symmetric occupancy ramp leaves about 10% of the sky
    /// coming through here, because no sample gets deep enough inside to reach
    /// fully-blocked — which is why [`occupancy`] is asymmetric.
    #[test]
    fn a_one_voxel_roof_still_shelters() {
        let mut volume = Volume::new([2, 2, 2], 1.0);
        volume.bake(&[VoxelOp::Box {
            center: [32.0, 19.5, 32.0],
            half_extents: [10.0, 0.4, 10.0],
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        }]);

        // Several heights, because a single one could sit at a lucky phase of
        // the sampling and pass for the wrong reason.
        for offset in 0_u8..8 {
            let y = 8.0 + f32::from(offset) * 0.37;
            let sheltered = sky_exposure(&volume, [32.0, y, 32.0]);
            assert!(sheltered < 0.05, "a one-voxel roof at y={y} read {sheltered}");
        }
    }

    #[test]
    fn inside_solid_rock_is_sealed() {
        let volume = roofed();

        let buried = sky_exposure(&volume, [32.0, 20.0, 32.0]);

        assert!(buried < 0.01, "inside the slab read {buried}");
    }

    /// Not only up. Wind asks along its own direction, and audio asks along
    /// the path to a listener.
    #[test]
    fn exposure_follows_the_direction_it_is_given() {
        let volume = roofed();
        let point = [32.0, 10.0, 32.0];

        let up = exposure(&volume, point, [0.0, 1.0, 0.0]);
        let down = exposure(&volume, point, [0.0, -1.0, 0.0]);

        assert!(up < 0.01, "the roof is above, read {up}");
        assert!(down > 0.99, "nothing below, read {down}");
    }

    /// The same inputs must give bit-identical answers — this value goes into
    /// the simulation hash.
    #[test]
    fn the_answer_is_bit_identical_across_calls() {
        let volume = roofed();
        let point = [31.7, 9.3, 32.4];

        let a = sky_exposure(&volume, point);
        let b = sky_exposure(&volume, point);

        assert_eq!(a.to_bits(), b.to_bits());
    }

    /// The march has a finite reach, and that is a documented property rather
    /// than a bug: a roof beyond it reads as open sky.
    #[test]
    fn the_march_has_a_finite_reach() {
        let mut volume = Volume::new([4, 4, 4], 1.0);
        volume.bake(&[VoxelOp::Box {
            center: [64.0, 100.0, 64.0],
            half_extents: [20.0, 2.0, 20.0],
            mode: CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        }]);

        // 90 voxels below a roof, with a reach of EXPOSURE_STEPS = 64.
        let far = sky_exposure(&volume, [64.0, 10.0, 64.0]);

        assert!(far > 0.99, "a roof beyond reach should not shelter: {far}");
    }
}
