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
    reg.register::<Script>("Script");
    reg
}

