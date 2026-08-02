//! The six M1 components.
//!
//! Doc comments are not decoration here: `///` becomes the field's schema
//! `description`, which becomes the `hint` on a rejection. Writing a good doc
//! comment and teaching the agent are the same act (`docs/format/README.md` §6).

use loom_reflect::TypeRegistry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A node's name. Addressed in files as the `name` node key, which is sugar
/// for this component (`docs/format/README.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Name {
    /// Unique among siblings. May not contain `/` — node paths use it as a separator.
    pub value: String,
}

/// Position, rotation, and scale relative to the parent node.
///
/// Addressed in files as the `transform` node key, which is sugar for this
/// component. Rotation is euler degrees and is authoritative — the scene layer
/// never converts a quaternion back (`docs/format/README.md` §1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Transform {
    /// Metres, right-handed, Y-up, −Z forward.
    pub pos: [f32; 3],
    /// Degrees, applied intrinsic Y-X-Z, ordered `[pitch_x, yaw_y, roll_z]`.
    pub rot_euler: [f32; 3],
    /// Multiplier per axis. `1.0` is unscaled.
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            pos: [0.0; 3],
            rot_euler: [0.0; 3],
            // Not [0.0; 3] — a zero-scale default silently collapses every node
            // that omits the field, and "defaults are omitted" means most do.
            scale: [1.0; 3],
        }
    }
}

/// A reference to an imported asset, by the file-local alias declared in
/// `[[asset]]`. Never a raw UUID and never a path (`docs/format/README.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct AssetRef {
    /// The `key` of an `[[asset]]` entry in the same file.
    pub asset: String,
}

/// Draws a mesh at the node's transform.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MeshRenderer {
    /// The mesh to draw.
    pub mesh: AssetRef,
}

/// An axis-aligned box collider centred on the node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BoxCollider {
    /// Half the box's size on each axis, in metres. The full box is twice this.
    #[schemars(inner(range(min = 0.001, max = 1000.0)))]
    pub half_extents: [f32; 3],
}

impl Default for BoxCollider {
    fn default() -> Self {
        Self {
            half_extents: [0.5; 3],
        }
    }
}

/// A point light.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Light {
    /// Luminous intensity. Interior lights are typically 100-800.
    #[schemars(range(min = 0.0, max = 10000.0))]
    pub intensity: f32,
    /// Linear RGB, each channel normalized 0..=1. Not 0-255.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub color: [f32; 3],
}

impl Default for Light {
    fn default() -> Self {
        Self {
            intensity: 100.0,
            color: [1.0; 3],
        }
    }
}

/// How a surface looks: base colour, how metal it is, how rough, and the
/// textures that vary those across it.
///
/// **Absent means the node keeps its debug colour.** A blockout is authored
/// long before it is textured, and a default material would make every untextured
/// node turn the same flat grey — which loses exactly the "which box is which"
/// readability the palette exists to give.
///
/// The two maps are `[[asset]]` aliases like any other asset reference, so a
/// typo is caught by `loom validate` rather than showing up as an untextured
/// surface with no explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Material {
    /// Linear RGB, each channel normalized 0..=1. Not 0-255. Multiplied with
    /// `albedo_map` when one is set.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub albedo: [f32; 3],
    /// `0.0` for anything non-metal, `1.0` for bare metal. Values in between
    /// are physically meaningless for a single surface — use them only to
    /// blend across one.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub metallic: f32,
    /// `0.0` is a mirror, `1.0` is chalk. Fully smooth is rare in the real
    /// world; most surfaces sit between 0.3 and 0.9.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub roughness: f32,
    /// Colour texture. Leave the alias empty for none.
    pub albedo_map: AssetRef,
    /// Tangent-space normal map, in the usual convention where flat is
    /// `(0.5, 0.5, 1.0)`. Leave the alias empty for none.
    pub normal_map: AssetRef,
    /// How many times the texture repeats across the mesh's `0..1` UV range.
    #[schemars(inner(range(min = 0.0001, max = 10000.0)))]
    pub uv_scale: [f32; 2],
    /// Project textures down the three world axes instead of reading the
    /// mesh's UVs.
    ///
    /// **This is what voxel terrain needs.** Surface Nets places vertices
    /// anywhere in their cell and produces no UVs at all, so a voxel volume
    /// textured by UV samples whatever happens to be at `(0, 0)`. Triplanar
    /// costs three texture samples instead of one, which is why it is opt-in
    /// rather than the default.
    pub triplanar: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            albedo: [0.8; 3],
            metallic: 0.0,
            // Not 0.0: a default-smooth surface renders as a black mirror of a
            // scene with no reflections in it, which reads as a bug.
            roughness: 0.8,
            albedo_map: AssetRef::default(),
            normal_map: AssetRef::default(),
            uv_scale: [1.0; 2],
            triplanar: false,
        }
    }
}

/// Makes a node participate in physics.
///
/// Static is the default because most of a blockout is scenery: if every box
/// an agent placed started falling, the first render of any scene would be a
/// pile on the floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RigidBody {
    /// `true` to fall and collide; `false` to stay put and be collided with.
    pub dynamic: bool,
    /// Kilograms. Only meaningful when dynamic.
    #[schemars(range(min = 0.001, max = 100000.0))]
    pub mass: f32,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            dynamic: false,
            mass: 1.0,
        }
    }
}

/// A destructible voxel volume, stored as a **recipe** rather than voxels.
///
/// never-do #11: a 512³ volume is 134 million voxels and must never enter a
/// `.loom` file. The scene stores the ordered op list; the field is baked from
/// it on load. That keeps the scene diffable, keeps it small, and gives
/// determinism for free — the same ops with the same seed produce bit-identical
/// voxels, so `loom sim --assert` against a voxel world is stable.
///
/// **The list is ordered and NOT commutative**: subtract-then-union differs
/// from union-then-subtract. Said here because an agent will otherwise assume
/// it can reorder freely (voxel doc §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VoxelVolume {
    /// World units per voxel. Smaller is finer and costs cubically.
    #[schemars(range(min = 0.01, max = 4.0))]
    pub voxel_size: f32,
    /// Volume size in 32³ chunks per axis.
    #[schemars(inner(range(min = 1, max = 64)))]
    pub chunks: [u32; 3],
    /// Ordered CSG operations. Order matters.
    pub ops: Vec<serde_json::Value>,
}

impl Default for VoxelVolume {
    fn default() -> Self {
        Self {
            voxel_size: 0.25,
            chunks: [4, 4, 4],
            ops: Vec::new(),
        }
    }
}

/// Attaches a sandboxed Rhai script to the node.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Script {
    /// Project-relative path to a `.rhai` file.
    pub path: String,
}

/// A registry with the engine's component types registered.
///
/// `ponytail:` hand-maintained list. Six entries is not a drift risk; roughly
/// twenty is. Upgrade path is in ADR 0004 — a derive emitting only a
/// registration entry, which is additive because this function is the seam.
#[must_use]
pub fn registry() -> TypeRegistry {
    let mut reg = TypeRegistry::new();
    reg.register::<Name>("Name");
    reg.register::<Transform>("Transform");
    reg.register::<MeshRenderer>("MeshRenderer");
    reg.register::<BoxCollider>("BoxCollider");
    reg.register::<Light>("Light");
    reg.register::<RigidBody>("RigidBody");
    reg.register::<VoxelVolume>("VoxelVolume");
    reg.register::<Material>("Material");
    reg.register::<Script>("Script");
    reg
}

