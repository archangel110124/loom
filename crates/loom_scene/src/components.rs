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

/// Emits particles from the node's position.
///
/// **Deterministic, like everything else that moves.** The `seed` is authored
/// here rather than taken from the clock, so the same scene stepped the same
/// number of ticks produces the same plume — which is what lets a particle
/// effect appear in a `loom sim --assert` hash instead of being the one visual
/// layer nobody can make an assertion about.
///
/// Defaults are a smoke plume, because that is the effect whose parameters are
/// least obvious: buoyant rather than falling, slow, wide-lived, and heavily
/// damped so it billows instead of shooting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ParticleEmitter {
    /// Particles born per second.
    #[schemars(range(min = 0.0, max = 10000.0))]
    pub rate: f32,
    /// Seconds a particle lives.
    #[schemars(range(min = 0.01, max = 120.0))]
    pub lifetime: f32,
    /// Fraction by which lifetime varies per particle. `0.0` makes a burst
    /// vanish all at once, which reads as a light being switched off.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub lifetime_jitter: f32,
    /// Initial speed, in metres per second.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub speed: f32,
    /// Half-angle of the emission cone about +Y, in degrees.
    #[schemars(range(min = 0.0, max = 180.0))]
    pub spread_degrees: f32,
    /// Radius of the disc particles are born on.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub radius: f32,
    /// Vertical acceleration. **Positive rises** — smoke is hotter than the air
    /// around it, and that buoyancy is the whole reason a plume goes up.
    #[schemars(range(min = -100.0, max = 100.0))]
    pub gravity: f32,
    /// Fraction of velocity shed per second. High drag billows; low drag jets.
    #[schemars(range(min = 0.0, max = 20.0))]
    pub drag: f32,
    /// Strength of the swirl, in metres per second squared.
    #[schemars(range(min = 0.0, max = 100.0))]
    pub turbulence: f32,
    /// Spatial scale of that swirl, in cycles per metre. Small values make
    /// broad slow curls; large ones make fine noise.
    #[schemars(range(min = 0.001, max = 10.0))]
    pub turbulence_scale: f32,
    /// Diameter in metres at birth and at death. Smoke expands as it cools.
    #[schemars(inner(range(min = 0.0, max = 1000.0)))]
    pub size: [f32; 2],
    /// Linear RGB at birth. Not 0-255.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub color_start: [f32; 3],
    /// Linear RGB at death.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub color_end: [f32; 3],
    /// Opacity at birth and at death. Ending above zero leaves a hard edge
    /// where particles pop out of existence.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub alpha: [f32; 2],
    /// Reproducibility, authored rather than sampled from the clock.
    pub seed: u32,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            rate: 40.0,
            lifetime: 3.0,
            lifetime_jitter: 0.35,
            speed: 2.0,
            spread_degrees: 18.0,
            radius: 0.25,
            gravity: 1.2,
            drag: 0.8,
            turbulence: 1.4,
            turbulence_scale: 0.35,
            size: [0.6, 3.2],
            color_start: [0.32, 0.30, 0.29],
            color_end: [0.62, 0.62, 0.64],
            alpha: [0.55, 0.0],
            seed: 1,
        }
    }
}

/// The viewpoint a render is taken from.
///
/// **The node's transform is the camera.** The eye is the node's world
/// position and the view direction is its local −Z, so a camera is moved,
/// parented and animated by exactly the same ops as anything else. A
/// first-person view is a camera parented to the player's head and nothing in
/// the engine has to know that; a security monitor is the same component on a
/// node bolted to a wall.
///
/// **Absent, the renderer frames the whole scene automatically.** That is the
/// behaviour worth keeping for an agent authoring a scene it has never seen —
/// a scene with no camera renders to something visible rather than to an empty
/// image (design doc §2.10). Adding this component is how you take that over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Camera {
    /// Vertical field of view in degrees. 60–90 reads as first person, 40–55
    /// as cinematic; past about 120 the edges of the image visibly stretch.
    #[schemars(range(min = 1.0, max = 179.0))]
    pub fov_y_degrees: f32,
    /// Whether this camera is eligible to be the view. When a scene holds
    /// several, the first active one in file order is used — so alternate
    /// viewpoints can be authored and left switched off.
    pub active: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_degrees: 60.0,
            active: true,
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
    reg.register::<ParticleEmitter>("ParticleEmitter");
    reg.register::<Camera>("Camera");
    reg.register::<Script>("Script");
    reg
}

