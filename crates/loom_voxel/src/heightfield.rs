//! Where the ground is, on a grid, sampled once and read by everything.
//!
//! **This is the one terrain-height query.** Grass drapes blades over it, water
//! subtracts it from the still-water level to get depth, and the water *shader*
//! reads the same array over buffer device address. Three consumers, one march.
//!
//! # Why a baked grid rather than a query
//!
//! A fragment or vertex shader cannot march a voxel SDF: the field is a sparse
//! `BTreeMap` of chunks with an edit layer on top, which is a pointer chase the
//! GPU has no access to. So the choice is not "grid or query" but "grid, or two
//! different implementations of the terrain height" — and the second one is the
//! divergence this project keeps paying for (ADR 0006). The grid is baked on the
//! CPU, uploaded once, and **both sides read it with the same bilinear
//! formula**: [`HeightField::at`] and the Slang in [`slang`] are one function
//! written twice, and `loom_water`'s agreement test compiles the second and
//! compares it against the first numerically.
//!
//! It was also already how grass did it, for an unrelated reason that turns out
//! to be the same reason: a march per blade measured 27× the cost of a march per
//! grid cell plus a bilinear lookup per blade.
//!
//! # The sentinel, and why it is not a `NaN`
//!
//! A column with no surface — outside the volume, or a hole blown clean through
//! it — has no height. That used to be a `NaN`, which is fine in Rust and a
//! menace in a shader: half the interesting arithmetic is a comparison, `NaN`
//! fails every one of them silently, and a driver compiling with fast-math
//! assumptions may not propagate it at all. [`NO_GROUND`] is a large finite
//! negative instead, so the bilinear is plain arithmetic on both sides with no
//! branch to get differently wrong, and "no ground" reads downstream as "the
//! water here is unfathomably deep", which is what it means.

use crate::Volume;

/// Height reported where a column has no surface at all.
///
/// Large, negative and finite. Water over it is bottomless (which is the right
/// answer for a sea with no terrain authored under it) and grass on it is
/// rejected by [`HeightField::has_ground`] before anything tries to stand a
/// blade at −10⁹.
pub const NO_GROUND: f32 = -1.0e9;

/// Anything below this came from [`NO_GROUND`], possibly blended with a real
/// height by the bilinear. Half the sentinel, so a single absent corner in a
/// cell is enough to fail it however the other three vote.
pub const NO_GROUND_LIMIT: f32 = NO_GROUND * 0.25;

/// Most samples a height field takes along one axis.
///
/// The grid is sampled at the voxel size until a field is big enough that that
/// would be more than this, and then it coarsens. 117² marches down a 96-voxel
/// column measured 0.11 s, so 256² is under a second; a 500 m field at 0.25 m
/// would be four thousand squared and would not finish in a load.
///
/// **Also the GPU buffer's capacity**, which is why it is here rather than at
/// the caller: `loom_render` sizes its terrain buffer from this number.
pub const MAX_SIDE: usize = 256;

/// The ground under a region, in world space.
///
/// Heights are world Y — absolute, not relative to any node. A caller that
/// wants them relative to something subtracts it.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightField {
    /// World X and Z of sample `(0, 0)`.
    pub origin: [f32; 2],
    /// Metres between samples.
    pub spacing: f32,
    /// Samples per axis.
    pub side: usize,
    /// Surface height in world Y, or [`NO_GROUND`] where the column has none.
    pub height: Vec<f32>,
}

/// World Y of the topmost surface in a column, or [`NO_GROUND`] where there is
/// none.
///
/// **Marched, then bisected.** The field is an `i8` quantized to ±1 voxel
/// (`SDF_SCALE`), so it saturates and the value carries no usable distance
/// beyond that — a sphere trace would step through the ground. Half-voxel steps
/// for the same reason [`crate::exposure`] uses them: the thinnest feature the
/// field can hold is one voxel, and a whole-voxel step can miss a lip.
#[must_use]
pub fn surface_height(volume: &Volume, offset: [f32; 3], x: f32, z: f32) -> f32 {
    let (lx, lz) = (x - offset[0], z - offset[2]);
    #[allow(clippy::cast_precision_loss)]
    let top = volume.resolution()[1] as f32 * volume.voxel_size;
    let step = volume.voxel_size * 0.5;
    // Positive is air, zero and below is solid — the sign convention `exposure`
    // documents from the other side.
    let solid = |y: f32| crate::exposure::sample(volume, [lx, y, lz]) <= 0.0;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (top / step).ceil() as usize;
    let mut air = top;
    for i in 1..=steps {
        #[allow(clippy::cast_precision_loss)]
        let y = step.mul_add(-(i as f32), top);
        if solid(y) {
            // Eight halvings of a half-voxel bracket puts the crossing well
            // inside a millimetre at any voxel size the engine uses. A blade
            // that sits a millimetre proud of the ground is invisible; one that
            // sits a centimetre under it is decapitated.
            let (mut lo, mut hi) = (y, air);
            for _ in 0..8 {
                let mid = 0.5 * (lo + hi);
                if solid(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return 0.5f32.mul_add(lo + hi, offset[1]);
        }
        air = y;
    }
    NO_GROUND
}

impl HeightField {
    /// Sample the ground over a square of half-extent `reach` around `centre`.
    ///
    /// `reach` is in metres and the caller owns the margin: a consumer that
    /// reads neighbours (grass's slope and flow stencils) must ask for more than
    /// it will read, or its border is a cliff.
    #[must_use]
    pub fn bake(volume: &Volume, offset: [f32; 3], centre: [f32; 2], reach: f32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let spacing = (reach * 2.0 / MAX_SIDE as f32).max(volume.voxel_size);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let side = ((reach * 2.0 / spacing).ceil() as usize + 1).min(MAX_SIDE);
        let origin = [centre[0] - reach, centre[1] - reach];

        let mut height = Vec::with_capacity(side * side);
        for j in 0..side {
            for i in 0..side {
                #[allow(clippy::cast_precision_loss)]
                let (x, z) = (
                    spacing.mul_add(i as f32, origin[0]),
                    spacing.mul_add(j as f32, origin[1]),
                );
                height.push(surface_height(volume, offset, x, z));
            }
        }
        Self { origin, spacing, side, height }
    }

    /// Height at a sample, or [`NO_GROUND`] outside the grid.
    #[must_use]
    pub fn node(&self, i: usize, j: usize) -> f32 {
        if i >= self.side || j >= self.side {
            return NO_GROUND;
        }
        self.height[j * self.side + i]
    }

    /// **The shared query: bilinear height at a world XZ.**
    ///
    /// The Slang half is in [`slang`], and the two are one function written
    /// twice — same order of operations, same edge test, same sentinel — because
    /// the number the buoyancy solver subtracts and the number the water shader
    /// draws have to be the same number.
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let gx = (x - self.origin[0]) / self.spacing;
        let gz = (z - self.origin[1]) / self.spacing;
        #[allow(clippy::cast_precision_loss)]
        let limit = (self.side - 1) as f32;
        if !(gx >= 0.0 && gz >= 0.0 && gx < limit && gz < limit) {
            return NO_GROUND;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (i, j) = (gx as usize, gz as usize);
        #[allow(clippy::cast_precision_loss)]
        let (fx, fz) = (gx - i as f32, gz - j as f32);

        // **Plain multiplies, not `mul_add`.** An fma rounds once where this
        // rounds twice, and the Slang half cannot be made to fuse on request —
        // the first draft of this used `mul_add` and the agreement test caught
        // it as a 4.8e-7 divergence, which is exactly what that test is for.
        let lerp = |a: f32, b: f32, t: f32| (b - a) * t + a;
        lerp(
            lerp(self.node(i, j), self.node(i + 1, j), fx),
            lerp(self.node(i, j + 1), self.node(i + 1, j + 1), fx),
            fz,
        )
    }

    /// Whether [`Self::at`] found real ground rather than the sentinel.
    #[must_use]
    pub fn has_ground(height: f32) -> bool {
        height > NO_GROUND_LIMIT
    }
}

/// The Slang half of [`HeightField::at`], emitted into the generated shader.
///
/// **Keep this and the Rust adjacent and change them together.** The agreement
/// test in `loom_water` compiles this string through `slangc -target cpp` and
/// compares it against `HeightField::at` at several hundred points; a formula
/// that drifts here draws a shoreline in a different place from the one the
/// buoyancy solver believes in.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The terrain height field. The Rust half is `loom_voxel::heightfield`; they
// are one implementation written twice, and `loom_water`'s agreement test
// compiles this and compares the two numerically.
//
// Never edit this by hand: it is emitted from the Rust, and editing it changes
// the GPU alone — which is a shoreline drawn somewhere the physics does not
// think it is.

// Where a column has no surface. Large, negative and finite, never a NaN: the
// bilinear below is plain arithmetic with no branch, so "no ground" has to
// survive being interpolated with a real height.
static const float LOOM_NO_GROUND = -1.0e9;

struct LoomHeightField {
    // World XZ of sample (0, 0).
    float2 origin;
    // Metres between samples.
    float spacing;
    // Samples per axis. Zero means the scene has no terrain at all.
    int side;
    // Row-major, `side * side` of them.
    float* height;
};

float loom_height_node(LoomHeightField field, int i, int j) {
    if (i >= field.side || j >= field.side) { return LOOM_NO_GROUND; }
    return field.height[j * field.side + i];
}

float loom_ground_height(LoomHeightField field, float2 xz) {
    if (field.side <= 0) { return LOOM_NO_GROUND; }
    float gx = (xz.x - field.origin.x) / field.spacing;
    float gz = (xz.y - field.origin.y) / field.spacing;
    float limit = float(field.side - 1);
    if (!(gx >= 0.0 && gz >= 0.0 && gx < limit && gz < limit)) {
        return LOOM_NO_GROUND;
    }
    int i = int(gx);
    int j = int(gz);
    float fx = gx - float(i);
    float fz = gz - float(j);

    float h00 = loom_height_node(field, i, j);
    float h10 = loom_height_node(field, i + 1, j);
    float h01 = loom_height_node(field, i, j + 1);
    float h11 = loom_height_node(field, i + 1, j + 1);
    // Written as `(b - a) * t + a` rather than `lerp`, because that is what the
    // Rust does and a library `lerp` is free to round the other way.
    float a = (h10 - h00) * fx + h00;
    float b = (h11 - h01) * fx + h01;
    return (b - a) * fz + a;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Volume, VoxelOp};

    /// A slab whose top is at `top`, in a volume that starts at the origin.
    fn slab(top: f32) -> Volume {
        let mut volume = Volume::new([2, 2, 2], 0.5);
        volume.bake(&[VoxelOp::Box {
            center: [16.0, top - 4.0, 16.0],
            half_extents: [20.0, 4.0, 20.0],
            mode: crate::CsgMode::Union,
            displace: None,
            yaw_degrees: 0.0,
            round: 0.0,
        }]);
        volume
    }

    /// The grid finds the slab it was baked over, everywhere inside it.
    #[test]
    fn a_flat_slab_reads_flat() {
        let volume = slab(6.0);
        let field = HeightField::bake(&volume, [0.0; 3], [16.0, 16.0], 6.0);

        for (x, z) in [(16.0, 16.0), (13.5, 18.25), (19.9, 12.1)] {
            let h = field.at(x, z);
            assert!((h - 6.0).abs() < 0.02, "height at ({x}, {z}) is {h}, not 6");
        }
    }

    /// **Outside the grid is the sentinel, not a clamp or a NaN.** A caller
    /// asking about a place the field was never baked over must get "no ground"
    /// rather than the nearest edge's height, or a sea 200 m from a small island
    /// would take the island's beach as its bed.
    #[test]
    fn outside_the_grid_has_no_ground() {
        let field = HeightField::bake(&slab(6.0), [0.0; 3], [16.0, 16.0], 6.0);

        for (x, z) in [(-100.0, 16.0), (16.0, 900.0), (5.0, 5.0)] {
            let h = field.at(x, z);
            assert!(!HeightField::has_ground(h), "({x}, {z}) reported ground at {h}");
            assert!(h.is_finite(), "the sentinel must stay finite: {h}");
        }
    }

    /// A column with nothing in it is the sentinel too — the volume is baked
    /// over air above the slab's edge, and an empty column must not read as
    /// ground at y = 0.
    #[test]
    fn an_empty_column_has_no_ground() {
        let empty = Volume::new([2, 2, 2], 0.5);
        let field = HeightField::bake(&empty, [0.0; 3], [16.0, 16.0], 6.0);

        assert!(!HeightField::has_ground(field.at(16.0, 16.0)));
    }

    /// The bilinear actually interpolates: a ramp read between two samples lands
    /// between the two heights rather than snapping to one.
    #[test]
    fn the_lookup_interpolates_between_samples() {
        let mut volume = Volume::new([2, 2, 2], 0.5);
        // A sphere makes a dome, so consecutive samples differ and a nearest-
        // neighbour lookup would show as a staircase.
        volume.bake(&[VoxelOp::Sphere {
            center: [16.0, 4.0, 16.0],
            radius: 9.0,
            mode: crate::CsgMode::Union,
            displace: None,
            elongate: [0.0; 3],
        }]);
        let field = HeightField::bake(&volume, [0.0; 3], [16.0, 16.0], 8.0);

        let s = field.spacing;
        let low = field.at(16.0 + 3.0 * s, 16.0);
        let mid = field.at(16.0 + 3.5 * s, 16.0);
        let high = field.at(16.0 + 4.0 * s, 16.0);
        assert!(
            (mid - 0.5 * (low + high)).abs() < 0.02,
            "midpoint {mid} is not between {low} and {high}"
        );
        assert!((low - high).abs() > 0.01, "the dome is flat here — pick another sample");
    }

    /// The Slang half carries the same sentinel and the same entry point. The
    /// numeric comparison lives in `loom_water`'s agreement test, which needs
    /// `slangc`; this catches a typo with no toolchain at all.
    #[test]
    fn the_slang_half_uses_the_same_constants() {
        let slang = slang();
        for needle in ["-1.0e9", "loom_ground_height", "LoomHeightField"] {
            assert!(slang.contains(needle), "the Slang half is missing `{needle}`");
        }
    }
}
