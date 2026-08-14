//! The field a raindrop hits, baked once into a grid the GPU can sample.
//!
//! **This is ADR 0014's structural claim made concrete.** Niagara's two GPU
//! collision modes are scene depth — screen-space, so a drop stops existing
//! behind the camera — and a distance field users have to get *generated* from
//! their meshes. This engine's terrain already is a signed distance field, so
//! the representation Unreal users fight to bake is the one already sitting in
//! `loom_voxel`. All that is missing is a form a compute shader can read, which
//! is a 3D texture.
//!
//! # What goes into it, and why it is not only the voxels
//!
//! ADR 0014's trigger 2 is the one thing the height field can never do: **rain
//! behaving correctly under geometry that is not in the voxel volume** — a
//! bridge, a gantry, a mesh roof. Baking only [`Volume`] here would carry the
//! drops onto a better representation of exactly the same world and leave that
//! trigger unfired.
//!
//! So the field is the **collision world**: every voxel volume, unioned with
//! every static box collider. That is a rule with one sentence — *rain stops
//! where a body would stop* — and it is the same rule audio's `openness`
//! follows when it casts against the collision world rather than the voxels.
//!
//! `ponytail:` box colliders only, because `BoxCollider` is the only static
//! collider component the scene schema has. A mesh with no collider stops no
//! rain, which is consistent (it stops no bullets either) and is the line to
//! move if a `MeshCollider` ever lands: add its shape to [`Shape`] and to the
//! `match` in [`Shape::distance`], and nothing else here changes.
//!
//! # Why a grid rather than a query
//!
//! The same argument [`loom_voxel::heightfield`] makes: a compute shader cannot
//! walk a `BTreeMap` of chunks, so the choice is not "grid or query" but "grid,
//! or a second implementation of the terrain". The grid is baked here and read
//! on the GPU with a trilinear fetch that the hardware does for free.

use loom_voxel::Volume;

/// Voxels along each axis of the baked field.
///
/// **192 x 64 x 192, which is ADR 0014's own sizing**: a 6x2x6-chunk volume,
/// 2.36 MB as `R8_SNORM`. At the default spacing that is a 48 x 16 x 48 m
/// region, which covers every test scene whole.
///
/// Y is a quarter of the horizontal extent because rain falls: what matters is
/// the vertical span between the ground and the tallest roof, not the sky above
/// it, and a drop above the field is simply falling toward it.
pub const DIMS: [usize; 3] = [192, 64, 192];

/// Metres per voxel of the baked field, at its finest.
///
/// A quarter metre resolves a one-voxel roof in every scene here (the volumes
/// are authored at 0.5 m) and it is half the finest voxel size anything in the
/// repository uses, which is the sampling-rate argument
/// [`loom_voxel::exposure`] already had to make from the other side.
pub const FINEST_SPACING: f32 = 0.25;

/// A static box in world space — one `BoxCollider`, already transformed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Solid {
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
}

/// A shape the bake can rasterise. One variant per static collider the schema
/// has, which is currently one.
enum Shape<'a> {
    /// A voxel field and the world position of its origin corner.
    Voxels(&'a Volume, [f32; 3]),
    Box(Solid),
}

impl Shape<'_> {
    /// Signed distance to the surface at a world point, **in metres**, negative
    /// inside.
    ///
    /// The voxel case is [`loom_voxel::exposure::sample`] — the same trilinear
    /// read S3's sky march uses, so a roof that shelters the wind shelters a
    /// raindrop at exactly the same place. It returns voxels, saturated at ±1,
    /// so it is scaled here and stays saturated; that is fine, because a
    /// collision test only needs to be accurate *near* the surface.
    fn distance(&self, p: [f32; 3]) -> f32 {
        match self {
            Self::Voxels(volume, offset) => {
                let local = [p[0] - offset[0], p[1] - offset[1], p[2] - offset[2]];
                loom_voxel::exposure::sample(volume, local) * volume.voxel_size
            }
            Self::Box(solid) => {
                let q = [
                    (p[0] - solid.center[0]).abs() - solid.half_extents[0],
                    (p[1] - solid.center[1]).abs() - solid.half_extents[1],
                    (p[2] - solid.center[2]).abs() - solid.half_extents[2],
                ];
                let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
                let inside = q[0].max(q[1]).max(q[2]).min(0.0);
                outside + inside
            }
        }
    }

    /// World-space bounds, used only to frame the grid.
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Voxels(volume, offset) => {
                let res = volume.resolution();
                #[allow(clippy::cast_precision_loss)]
                let span = |axis: usize| res[axis] as f32 * volume.voxel_size;
                (
                    *offset,
                    [offset[0] + span(0), offset[1] + span(1), offset[2] + span(2)],
                )
            }
            Self::Box(s) => (
                [
                    s.center[0] - s.half_extents[0],
                    s.center[1] - s.half_extents[1],
                    s.center[2] - s.half_extents[2],
                ],
                [
                    s.center[0] + s.half_extents[0],
                    s.center[1] + s.half_extents[1],
                    s.center[2] + s.half_extents[2],
                ],
            ),
        }
    }
}

/// The baked collision field, ready to become an `R8_SNORM` 3D image.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// World position of the **centre of voxel (0, 0, 0)**.
    ///
    /// The centre rather than the corner, because that is where a hardware
    /// trilinear fetch puts texel zero, and disagreeing with the sampler by
    /// half a voxel puts the collision surface half a voxel off the visible
    /// one — which reads as rain landing slightly inside the floor.
    pub origin: [f32; 3],
    /// Metres per voxel.
    pub spacing: f32,
    /// Voxels per axis. Always [`DIMS`]; carried so the shader does not have to
    /// hardcode it twice.
    pub dims: [usize; 3],
    /// Signed distance, X fastest, then Y, then Z. `i8` scaled so that -128 is
    /// one [`Field::range`] inside the surface and 127 is one outside.
    pub sdf: Vec<i8>,
}

impl Field {
    /// Metres the `i8` range spans either side of the surface.
    ///
    /// Two voxels, which is as far as a drop can travel in a tick at terminal
    /// velocity when the field is at its finest — so the value at the drop's
    /// position is always meaningful over the step it is about to take, and the
    /// collision test never has to reason about a saturated field.
    #[must_use]
    pub fn range(&self) -> f32 {
        self.spacing * 2.0
    }

    /// Distance in metres at a voxel, for tests and for the CPU side of the
    /// agreement the shader has to hold up.
    #[must_use]
    pub fn at(&self, x: usize, y: usize, z: usize) -> f32 {
        if x >= self.dims[0] || y >= self.dims[1] || z >= self.dims[2] {
            return self.range();
        }
        let index = x + self.dims[0] * (y + self.dims[1] * z);
        f32::from(self.sdf[index]) / 127.0 * self.range()
    }

    /// Nearest-voxel distance at a world point, in metres. Outside the field
    /// this is open air, which is what lets a drop over a big terrain fall
    /// through to the height field instead.
    #[must_use]
    pub fn at_world(&self, p: [f32; 3]) -> f32 {
        let index = |axis: usize| -> Option<usize> {
            let v = ((p[axis] - self.origin[axis]) / self.spacing).round();
            if v < 0.0 || v >= self.dims[axis] as f32 {
                return None;
            }
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            Some(v as usize)
        };
        match (index(0), index(1), index(2)) {
            (Some(x), Some(y), Some(z)) => self.at(x, y, z),
            _ => self.range(),
        }
    }
}

/// Bake every voxel volume and static box in a scene into one field.
///
/// Returns `None` when there is nothing to collide with at all — a scene with
/// no terrain and no boxes, where every drop should simply fall past.
///
/// The grid is framed on the union of the shapes' bounds and then **clamped to
/// [`DIMS`] by coarsening**, exactly as [`loom_voxel::heightfield`] coarsens
/// rather than truncating: a 500 m terrain gets a coarse field rather than a
/// fine field covering a fifteenth of itself.
#[must_use]
pub fn bake(volumes: &[(&Volume, [f32; 3])], boxes: &[Solid]) -> Option<Field> {
    let shapes: Vec<Shape> = volumes
        .iter()
        .map(|(v, o)| Shape::Voxels(v, *o))
        .chain(boxes.iter().copied().map(Shape::Box))
        .collect();
    if shapes.is_empty() {
        return None;
    }

    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    for shape in &shapes {
        let (l, h) = shape.bounds();
        for axis in 0..3 {
            low[axis] = low[axis].min(l[axis]);
            high[axis] = high[axis].max(h[axis]);
        }
    }
    if !low.iter().chain(&high).all(|v| v.is_finite()) {
        return None;
    }

    // One spacing for all three axes: an anisotropic grid would make the
    // shader's gradient — which is what a drop bounces off — wrong by the
    // aspect ratio, and a wrong normal is a drop sliding sideways along a
    // flat floor.
    #[allow(clippy::cast_precision_loss)]
    let spacing = (0..3)
        .map(|axis| (high[axis] - low[axis]) / (DIMS[axis] - 1) as f32)
        .fold(FINEST_SPACING, f32::max);

    // Centre the grid on the shapes rather than pinning it at their low
    // corner: coarsening only happens when the scene is bigger than the field,
    // and then the middle is what matters.
    #[allow(clippy::cast_precision_loss)]
    let origin = [0, 1, 2].map(|axis| {
        let mid = f32::midpoint(low[axis], high[axis]);
        mid - (DIMS[axis] - 1) as f32 * spacing * 0.5
    });

    let mut field = Field {
        origin,
        spacing,
        dims: DIMS,
        sdf: vec![0; DIMS[0] * DIMS[1] * DIMS[2]],
    };
    let range = field.range();

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    for z in 0..DIMS[2] {
        for y in 0..DIMS[1] {
            for x in 0..DIMS[0] {
                let p = [
                    origin[0] + x as f32 * spacing,
                    origin[1] + y as f32 * spacing,
                    origin[2] + z as f32 * spacing,
                ];
                // Union: the nearest surface wins. A drop is stopped by
                // whichever solid it meets first, and `min` is what "or" means
                // for signed distances.
                let d = shapes
                    .iter()
                    .map(|s| s.distance(p))
                    .fold(f32::INFINITY, f32::min);
                let index = x + DIMS[0] * (y + DIMS[1] * z);
                field.sdf[index] = (d / range * 127.0).clamp(-127.0, 127.0) as i8;
            }
        }
    }

    Some(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_voxel::{CsgMode, VoxelOp};

    fn slab() -> Volume {
        let mut volume = Volume::new([2, 1, 2], 0.5);
        volume.bake(&[VoxelOp::Box {
            center: [16.0, 0.5, 16.0],
            half_extents: [15.0, 0.5, 15.0],
            mode: CsgMode::Union,
        }]);
        volume
    }

    /// The sign is the whole contract: below the floor is solid, above it is
    /// air. Everything the compute shader does keys off that one bit.
    #[test]
    fn a_voxel_floor_reads_solid_below_and_air_above() {
        let volume = slab();
        let field = bake(&[(&volume, [-16.0, 0.0, -16.0])], &[]).expect("a field");

        assert!(field.at_world([0.0, 0.2, 0.0]) < 0.0, "inside the slab reads air");
        assert!(field.at_world([0.0, 3.0, 0.0]) > 0.0, "above the slab reads solid");
    }

    /// **Trigger 2.** A box collider with no voxel behind it must stop a drop,
    /// because that is the case the baked height field can never express and
    /// the only reason this module is not simply an upload of `Volume`.
    #[test]
    fn a_box_collider_is_solid_even_with_no_voxels_anywhere() {
        let roof = Solid {
            center: [0.0, 5.0, 0.0],
            half_extents: [4.0, 0.25, 4.0],
        };
        let field = bake(&[], &[roof]).expect("a field");

        assert!(field.at_world([0.0, 5.0, 0.0]) < 0.0, "the roof is not solid");
        assert!(field.at_world([0.0, 7.0, 0.0]) > 0.0, "above the roof is not air");
        assert!(field.at_world([0.0, 3.0, 0.0]) > 0.0, "under the roof is not air");
    }

    /// A scene with nothing to hit bakes nothing, rather than a field of air
    /// that costs 2.4 MB and answers every query the same way.
    #[test]
    fn a_scene_with_no_solids_bakes_no_field() {
        assert!(bake(&[], &[]).is_none());
    }

    /// The grid coarsens to fit rather than covering a fraction of the scene:
    /// a drop over the far end of a big terrain must still collide with it.
    #[test]
    fn a_scene_larger_than_the_grid_coarsens_instead_of_being_clipped() {
        let wide = Solid {
            center: [0.0, 0.0, 0.0],
            half_extents: [200.0, 4.0, 200.0],
        };
        let field = bake(&[], &[wide]).expect("a field");

        assert!(field.spacing > FINEST_SPACING, "spacing {} did not coarsen", field.spacing);
        assert!(
            field.at_world([190.0, 0.0, 190.0]) < 0.0,
            "the far corner fell outside the grid"
        );
    }

    /// Two solids union. A roof over a floor must leave the space between them
    /// as air — `min` rather than anything that averages, which would fill the
    /// gap with a soft nothing and stop rain in mid-air.
    #[test]
    fn a_roof_over_a_floor_leaves_air_between_them() {
        let volume = slab();
        let roof = Solid {
            center: [0.0, 6.0, 0.0],
            half_extents: [4.0, 0.5, 4.0],
        };
        let field = bake(&[(&volume, [-16.0, 0.0, -16.0])], &[roof]).expect("a field");

        assert!(field.at_world([0.0, 6.0, 0.0]) < 0.0, "the roof");
        assert!(field.at_world([0.0, 3.0, 0.0]) > 0.0, "the air under it");
        assert!(field.at_world([0.0, 0.2, 0.0]) < 0.0, "the floor");
    }
}
