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
    /// How much water this surface can hold in its pores — how much it darkens
    /// when it is wet.
    ///
    /// `0.0` is glass or polished stone: a wet one is glossier and no darker.
    /// `1.0` is dry sand, raw concrete, unsealed timber — the surfaces that go
    /// visibly black in the rain. Most built things sit near the default.
    ///
    /// **Darkening is a property of the material, not of the weather.** Water
    /// fills the pores, light that would have scattered back out of them is
    /// internally reflected instead, and how much of that happens is decided by
    /// the surface. A single global "wetness makes things darker" number is the
    /// version of this that makes a wet window and a wet pavement behave alike.
    ///
    /// Ignored to the extent a surface is metallic: metal has no pores.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub porosity: f32,
    /// A second material shown where the surface is too steep for the first.
    ///
    /// **This is what stops terrain being one colour.** Real ground is not one
    /// substance: soil and growth sit where they can rest, and where the slope
    /// gets away from them the bedrock underneath shows. Without it a voxel
    /// volume samples a single texture at every angle, which reads as a hill
    /// wrapped in wallpaper however good the wallpaper is.
    ///
    /// Absent by default, and absent means exactly one material — no second
    /// sample, no blend, no cost. Every scene authored before this renders
    /// byte-identically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<GroundLayer>,
}

/// The steep-slope material under a [`Material`], and where it takes over.
///
/// It carries its own textures and its own roughness rather than only a colour,
/// because two substances that differ in colour alone still read as one
/// substance painted twice. Rock is smoother than soil and holds less water,
/// and blending those is most of what makes the boundary convincing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GroundLayer {
    /// Linear RGB, multiplied with `albedo_map` exactly as [`Material::albedo`]
    /// is.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub albedo: [f32; 3],
    #[schemars(range(min = 0.0, max = 1.0))]
    pub roughness: f32,
    /// Colour texture. Leave the alias empty for none.
    pub albedo_map: AssetRef,
    /// Tangent-space normal map. Leave the alias empty for none.
    pub normal_map: AssetRef,
    /// Repeats across the mesh's `0..1` UV range, independent of the base
    /// material's — bedrock grain is a different physical size from soil grit.
    #[schemars(inner(range(min = 0.0001, max = 10000.0)))]
    pub uv_scale: [f32; 2],
    /// How much water this surface holds. See [`Material::porosity`].
    #[schemars(range(min = 0.0, max = 1.0))]
    pub porosity: f32,
    /// Surface normal Y below which this layer takes over — the cosine of the
    /// slope, so `1.0` is flat and `0.0` is vertical.
    ///
    /// **The same quantity `loom_grass::coverage` cuts grass on, and it should
    /// usually be the same number.** Grass stopping at one angle while the rock
    /// beneath it starts at another draws two concentric rings around a hill,
    /// which is worse than either boundary alone.
    ///
    /// The default matches `Grass::slope_cutoff`'s.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub slope: f32,
}

impl Default for GroundLayer {
    fn default() -> Self {
        Self {
            albedo: [0.8; 3],
            roughness: 0.8,
            albedo_map: AssetRef::default(),
            normal_map: AssetRef::default(),
            uv_scale: [1.0; 2],
            // Rock holds far less water than soil, which is why a cliff stays
            // pale in rain that turns the ground beneath it black.
            porosity: 0.15,
            slope: 0.7,
        }
    }
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
            // Most of a scene is stone, concrete, timber and dirt, all of which
            // darken noticeably in the rain. A default of zero would make the
            // whole effect opt-in per material, which is how a feature ends up
            // authored into one test scene and nowhere else.
            porosity: 0.4,
            // Absent, so a scene authored before ground layers existed renders
            // byte-identically and pays nothing for the feature.
            layer: None,
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
    /// How readily these particles are carried by the scene's `Wind`.
    ///
    /// Per second, so `4.0` matches the air in about a quarter of a second —
    /// smoke, steam, dust. `0.5` lags it noticeably, which is what embers and
    /// light debris do. **`0.0` ignores wind entirely and is the default**, so
    /// every scene authored before wind existed renders exactly as it did.
    #[schemars(range(min = 0.0, max = 20.0))]
    pub wind_response: f32,
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
    /// Particles released all at once when the emitter starts.
    ///
    /// **This is what makes an explosion expressible.** A rate is a tap; a
    /// blast is everything released at the same instant, and no number of
    /// particles per second is that. Set `rate = 0` for a pure one-shot.
    #[schemars(range(min = 0, max = 100000))]
    pub burst: u32,
    /// Seconds before this emitter does anything.
    ///
    /// How an explosion is staged out of ordinary emitters: the fire flash
    /// now, the smoke a beat later, both plain `ParticleEmitter` nodes under
    /// one parent. No timeline concept required.
    #[schemars(range(min = 0.0, max = 3600.0))]
    pub delay: f32,
    /// Seconds of continuous emission after `delay`. **Zero means forever.**
    ///
    /// Forever is right for a chimney and wrong for a blast, where the
    /// fireball feeds smoke for a moment and then stops.
    #[schemars(range(min = 0.0, max = 3600.0))]
    pub duration: f32,
    /// Add light instead of covering what is behind.
    ///
    /// **Fire needs this.** An alpha-blended flame is a grey-brown cloud tinted
    /// orange, because overlapping sprites darken each other; an additive one
    /// gets brighter where it piles up, which is what a fireball does. Wrong
    /// for smoke, which occludes.
    pub additive: bool,
    /// Draw a flame field across the quad instead of a sprite.
    ///
    /// **This is fire, and it is a different thing from a lot of orange
    /// sprites.** The colour target is 8-bit and additive blending clamps at
    /// 1.0 with no rolloff, so overlapping sprites pin red after about three
    /// layers and everything past that is white — which is why a sprite
    /// campfire is a fireball however its sprites are shaped. A flame field is
    /// one quad with an overdraw of exactly one, so nothing clips and the
    /// tongues, the black gaps between them and the edges are all level sets
    /// of a single noise field.
    ///
    /// Use with `additive`, one long-lived particle, and no motion: the flame
    /// moves because the field is dragged through it, not because the quad is.
    pub flame: bool,
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
            wind_response: 0.0,
            size: [0.6, 3.2],
            color_start: [0.32, 0.30, 0.29],
            color_end: [0.62, 0.62, 0.64],
            alpha: [0.55, 0.0],
            burst: 0,
            delay: 0.0,
            duration: 0.0,
            additive: false,
            flame: false,
            seed: 1,
        }
    }
}

/// Makes a node a character: a capsule that walks, climbs and slides.
///
/// **This component says what the character *is*, not how it moves.** Shape,
/// reach and what counts as a walkable floor live here because they are level
/// geometry — a doorway either fits the capsule or does not. Speed,
/// acceleration, gravity, jump height and whether there is a double jump are
/// the *movement model*, and that is authored in the node's `Script`, which
/// receives the character's state each tick and answers with a velocity.
///
/// The split is deliberate. Collision is what a script would get subtly wrong
/// in ways that look like broken level geometry; the movement model is the
/// part that is supposed to differ per game, and freezing it into the engine
/// is what makes an engine feel like somebody else's engine.
///
/// A character with no script still falls, and does nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CharacterController {
    /// Total standing height in metres, cap to cap. 1.8–2.0 is human.
    #[schemars(range(min = 0.1, max = 100.0))]
    pub height: f32,
    /// Capsule radius in metres. Half the width that has to fit a doorway.
    #[schemars(range(min = 0.01, max = 50.0))]
    pub radius: f32,
    /// Steepest floor still walkable, in degrees. Steeper counts as a wall to
    /// slide down rather than a ramp to climb.
    #[schemars(range(min = 0.0, max = 89.0))]
    pub max_slope_degrees: f32,
    /// Tallest ledge stepped over without jumping — a stair riser. Zero turns
    /// stepping off, which makes any staircase impassable.
    #[schemars(range(min = 0.0, max = 10.0))]
    pub step_height: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            height: 1.9,
            radius: 0.35,
            max_slope_degrees: 50.0,
            step_height: 0.35,
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

/// A shove applied to everything dynamic near this node, once.
///
/// The *force* half of an explosion. The look of one is `ParticleEmitter`
/// nodes — a `burst` of additive fire and a delayed cloud of smoke — and the
/// two are deliberately separate components: a shockwave through a doorway has
/// no flame, and a gas flare has no force. Authoring an explosion means
/// putting both on one node, which is a scene decision rather than an engine
/// one.
///
/// **Cover works.** A body with something solid between it and this node is
/// not pushed at all. An explosion that ignores the level is both wrong and
/// unteachable — nobody learns to duck behind a wall that does nothing.
///
/// Static geometry never moves, and neither does a `CharacterController`: a
/// character's velocity belongs to its movement script, and knockback is that
/// script's decision rather than something applied behind its back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Blast {
    /// Metres. The push falls to nothing at this distance.
    #[schemars(range(min = 0.0, max = 10000.0))]
    pub radius: f32,
    /// Newton-seconds delivered to a body at the centre, falling off linearly
    /// to zero at `radius`. A 10 kg crate takes `impulse / 10` m/s.
    #[schemars(range(min = 0.0, max = 1000000.0))]
    pub impulse: f32,
    /// Seconds into the simulation before it goes off. Fires exactly once.
    ///
    /// Matches `ParticleEmitter.delay`, so the force and the flash can be put
    /// on the same instant — or deliberately not, for a charge that flashes
    /// before it hits.
    #[schemars(range(min = 0.0, max = 3600.0))]
    pub delay: f32,
    /// Whether this blast goes off on its own.
    ///
    /// **`false` makes the node an explosion nobody has set off yet.** Neither
    /// its force nor the particles under it happen until something triggers
    /// one — a script firing a weapon, typically. That is how a scene carries
    /// a *kind* of explosion rather than an event: author it once, dormant,
    /// and the runtime reproduces it wherever it is asked for.
    ///
    /// Armed is the default because a node that does nothing is a strange
    /// thing to get by accident.
    pub armed: bool,
}

impl Default for Blast {
    fn default() -> Self {
        Self {
            radius: 6.0,
            impulse: 400.0,
            delay: 0.0,
            armed: true,
        }
    }
}

/// Where a HUD element sits on the screen.
///
/// Anchors rather than coordinates because a window is resizable: ammo pinned
/// to the bottom right has to stay there at any size, and a scene that says
/// `[1900, 1040]` is a scene authored for one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// A line of text drawn over the scene, in screen space.
///
/// **The overlay is authored, not coded.** Unity hangs UI off a Canvas in the
/// scene, and the part worth keeping is that: a HUD is content, it belongs in
/// the file, and changing where the score sits should not be a rebuild. What
/// is not worth keeping is the Canvas itself — it maintains a retained mesh
/// and invalidates it whenever anything changes, which is a lot of machinery
/// for a dozen labels. This rebuilds the whole overlay every frame into one
/// vertex buffer, which at HUD scale is cheaper *and* has no invalidation
/// logic to get wrong.
///
/// `text` may name the game's state in braces: `{score}` takes a number the
/// rules script is keeping, and `{status}` and `{message}` take the outcome.
/// An unknown name is left standing in the text rather than blanked, so a typo
/// shows up on screen instead of quietly rendering nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Hud {
    /// Which corner or edge this is measured from.
    pub anchor: Anchor,
    /// Pixels inward from that anchor.
    pub offset: [f32; 2],
    /// The line to draw. `{name}` is replaced by the game state's `name`.
    pub text: String,
    /// Point size.
    #[schemars(range(min = 1.0, max = 512.0))]
    pub size: f32,
    /// Linear RGB, each channel normalized 0..=1. Not 0-255.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub color: [f32; 3],
    /// Draw only while a game is running.
    ///
    /// A score is meaningless in the editor, and an overlay that cannot be
    /// switched off obscures the thing being edited.
    pub only_in_play: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            anchor: Anchor::TopLeft,
            offset: [16.0, 16.0],
            text: String::new(),
            size: 22.0,
            color: [1.0; 3],
            only_in_play: false,
        }
    }
}

/// The sky, the sun and the haze the whole scene sits in.
///
/// **Night was not expressible before this.** The sun's direction and
/// strength, the sky's two colours and the fog were constants compiled into
/// the shader — one blue afternoon, in every scene ever authored. A scene
/// could place lights but could not turn the sun down, and the ambient term
/// alone lit everything to 95%.
///
/// At most one per scene; the first is used. Absent, the afternoon those
/// constants described is the default, so every existing scene looks the same
/// as it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Environment {
    /// Direction *toward* the sun. Normalised on load, so any length works.
    pub sun_direction: [f32; 3],
    /// How strong the direct light is. `0.0` is a moonless overcast night.
    #[schemars(range(min = 0.0, max = 20.0))]
    pub sun_strength: f32,
    /// Linear RGB of the direct light. Not 0-255.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub sun_color: [f32; 3],
    /// How much light arrives from everywhere at once.
    ///
    /// **The knob that makes a scene dark.** Point lights are local; this is
    /// the floor under everything, and at the old default of `0.95` no amount
    /// of turning the sun down produced night.
    #[schemars(range(min = 0.0, max = 4.0))]
    pub ambient: f32,
    /// Linear RGB of the sky overhead.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub sky_zenith: [f32; 3],
    /// Linear RGB of the sky at the horizon.
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub sky_horizon: [f32; 3],
    /// Haze per metre. Zero is perfectly clear air.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub fog_density: f32,
    /// How fast the haze thins with height. Larger keeps it near the ground.
    #[schemars(range(min = 0.0, max = 10.0))]
    pub fog_falloff: f32,
    /// How much of the sky is cloud: `0.0` clear, `1.0` a solid deck.
    ///
    /// **Rain raises a floor under this**, so a raining scene that authors
    /// nothing still gets an overcast sky. A sun blazing through a downpour is
    /// a bigger tell than having no clouds at all, and it is what every scene
    /// in this project did before the deck existed.
    ///
    /// Author it above that floor for cloud without rain. Below it does
    /// nothing: there is no way to ask for a clear sky over heavy rain, which
    /// is a real weather state and an unusual one — say so in an ADR if a scene
    /// ever needs it rather than adding a flag on spec.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub cloud_cover: f32,
    /// Metres across one cloud mass. Larger is a broader, slower deck.
    #[schemars(range(min = 50.0, max = 20000.0))]
    pub cloud_scale: f32,
}

impl Default for Environment {
    fn default() -> Self {
        // Exactly the constants the shader used to carry, so a scene that
        // says nothing about its sky looks precisely as it did before this
        // component existed.
        Self {
            sun_direction: [0.4, 0.9, 0.3],
            sun_strength: 0.62,
            sun_color: [1.0, 0.98, 0.94],
            ambient: 0.95,
            sky_zenith: [0.0272, 0.0946, 0.3424],
            sky_horizon: [0.3931, 0.5071, 0.6038],
            fog_density: 0.0026,
            fog_falloff: 0.03,
            // Clear. Every scene authored before the deck existed looks exactly
            // as it did — unless it rains, and then the floor applies, which is
            // the one deliberate change to existing scenes.
            cloud_cover: 0.0,
            cloud_scale: 1200.0,
        }
    }
}

/// A field of scattered meshes — trees, rocks, bushes — as the rules that
/// generate them.
///
/// **Never the instances.** A field is a million of them, and the scene tree
/// shows one node: the same represent-the-generator principle as a voxel op
/// list (never-do #11) and as `Grass` below. Instances are never ECS entities.
///
/// **Rendering only**, the same exemption grass and rain get: a sim hash must
/// be identical with a field on or off.
///
/// The placement is `loom_scatter`, which is where the interesting decisions
/// are written down — why it is not Bridson, why boundaries fade, and why a
/// patch regenerated after a CSG op is bit-identical to a full rebuild.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Scatter {
    /// What to place: an `[[asset]]` alias, or a primitive name like `box`.
    ///
    /// An unresolvable name falls back to a box, the same rule every other mesh
    /// reference follows — a missing asset should be *visible*, not silent.
    pub mesh: String,
    /// Half the field's size on X and Z, in metres, centred on the node.
    #[schemars(inner(range(min = 1.0, max = 2000.0)))]
    pub half_extent: [f32; 2],
    /// Minimum metres between instances. **Guaranteed, not average** — see
    /// `loom_scatter`, which also explains why the average comes out near 1.8x
    /// this.
    #[schemars(range(min = 0.25, max = 200.0))]
    pub spacing: f32,
    /// How far an instance may wander inside its cell, `0` to `1`. Zero is a
    /// plantation.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub jitter: f32,
    /// Smallest and largest uniform scale.
    #[schemars(inner(range(min = 0.01, max = 100.0)))]
    pub scale: [f32; 2],
    /// Which world this field is. Two fields over the same ground need
    /// different seeds or they place in the same spots.
    pub seed: u32,
    /// Steepest ground anything stands on, in degrees. Thins over the last few
    /// degrees rather than stopping at a line.
    #[schemars(range(min = 0.0, max = 90.0))]
    pub max_slope_degrees: f32,
    /// Overall thinning, `0` to `1`. A copse rather than a forest, without
    /// changing how far apart the trees are.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub density: f32,
    /// How much the ground's moisture matters, `0` to `1`.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub moisture: f32,
    /// How much these bend in the wind. `0` is rigid — a boulder.
    ///
    /// Roughly "metres of lean at the top of a two-metre instance in a moderate
    /// breeze". A tree is around `0.5`; a sapling more, a trunk less. The bend
    /// samples the same P1 wind field the grass and the rain read, so a tree, a
    /// blade and a raindrop agree about which way the wind is going.
    #[schemars(range(min = 0.0, max = 4.0))]
    pub sway: f32,
    /// Fields this one keeps away from.
    ///
    /// **Named by node path, and only fields declared earlier in the file.**
    /// Backwards references are the whole cycle check: with them a cycle cannot
    /// be written, so there is no cycle detection anywhere.
    pub exclude: Vec<ScatterExclude>,
}

impl Default for Scatter {
    fn default() -> Self {
        Self {
            mesh: "box".to_owned(),
            half_extent: [32.0, 32.0],
            spacing: 4.0,
            jitter: 1.0,
            scale: [0.8, 1.25],
            seed: 0,
            max_slope_degrees: 22.0,
            density: 1.0,
            moisture: 0.0,
            // Rigid by default: the component places rocks as readily as trees,
            // and a default that waved boulders would be wrong more often than
            // right.
            sway: 0.0,
            exclude: Vec::new(),
        }
    }
}

/// "Not within `radius` metres of anything in `field`."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ScatterExclude {
    /// The node path of an earlier `Scatter` field.
    pub field: String,
    #[schemars(range(min = 0.0, max = 500.0))]
    pub radius: f32,
}

impl Default for ScatterExclude {
    fn default() -> Self {
        Self { field: String::new(), radius: 5.0 }
    }
}

/// A field of grass, stored as the rules that generate it.
///
/// **Never the blades.** A tile is a few thousand of them and a field is
/// millions; the same represent-the-generator principle as a voxel op list
/// (never-do #11). The scene tree shows one node for the field, and blades are
/// never ECS entities — `loom_ecs` is `Vec<Option<T>>`, and a million blade
/// entities would be pathological.
///
/// **Rendering only.** Grass is outside the deterministic simulation, the same
/// exemption rain gets: sim hashes must be identical with it on or off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Grass {
    /// Half the field's size on X and Z, in metres, centred on the node.
    #[schemars(inner(range(min = 0.5, max = 500.0)))]
    pub half_extent: [f32; 2],
    /// Blades per square metre on flat, ideal ground.
    #[schemars(range(min = 0.1, max = 2000.0))]
    pub density: f32,
    /// Blade height in metres, before per-blade variation.
    #[schemars(range(min = 0.01, max = 5.0))]
    pub height: f32,
    /// Blade width at the base, in metres.
    #[schemars(range(min = 0.0005, max = 0.5))]
    pub width: f32,
    /// Surface normal Y at which grass stops. `0.7` is roughly 45°.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub slope_cutoff: f32,
    /// How strongly blades in a clump agree about facing, `0` to `1`.
    ///
    /// The single highest-value realism control here: uniformly random blades
    /// read as carpet, and blades that agree with their neighbours read as a
    /// field.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub clump_facing: f32,
    /// How much a clump's colour varies from its neighbours'.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub clump_colour: f32,
}

impl Default for Grass {
    fn default() -> Self {
        Self {
            half_extent: [6.0, 6.0],
            density: 100.0,
            height: 0.42,
            width: 0.018,
            slope_cutoff: 0.7,
            clump_facing: 0.7,
            clump_colour: 0.35,
        }
    }
}

/// The weather the whole scene stands in.
///
/// **Authored as scalars, evaluated as a field.** The numbers here fill
/// `loom_field`'s wind parameters, so the same values drive the CPU side
/// (physics, gameplay, the determinism hash) and the GPU side (every vertex
/// that leans). Changing one changes both, which is the whole reason the field
/// is generated rather than written twice.
///
/// At most one per scene; the first is used. Absent, the default is a
/// moderate breeze from the west — Beaufort 4, the lowest wind that visibly
/// moves foliage, so a scene that says nothing still looks like weather.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Wind {
    /// Compass direction the wind blows *toward*, in degrees clockwise from
    /// +X. Normalised into a unit vector on load, so any value works.
    ///
    /// Degrees rather than a vector because that is how weather is quoted and
    /// how a human thinks about it; the field takes the vector, and the
    /// conversion happens once at the boundary.
    pub direction_degrees: f32,
    /// Mean speed in metres per second.
    ///
    /// The Beaufort scale, for authoring: 2 is a light breeze that stirs
    /// leaves, 5.5 a moderate breeze that moves small branches, 11 a strong
    /// breeze, 20 a gale. Above about 25 the vegetation shaders stop looking
    /// like plants and start looking like errors.
    #[schemars(range(min = 0.0, max = 40.0))]
    pub speed: f32,
    /// How much the speed swells and drops. `0` is a steady flow, `1` the
    /// authored default, `2` a squally day.
    #[schemars(range(min = 0.0, max = 3.0))]
    pub gustiness: f32,
    /// Small-scale swirl on top, in metres per second.
    ///
    /// This is the term that stops everything moving in unison — the single
    /// most recognisable tell that a vegetation shot is fake.
    #[schemars(range(min = 0.0, max = 10.0))]
    pub turbulence: f32,
    /// Fraction of the wind still blowing at ground level.
    ///
    /// Ground drag: `0.45` matches a 1/7 power law over the first few metres,
    /// which is where grass and undergrowth live. Lower makes a sheltered
    /// hollow, `1.0` removes the profile entirely.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub ground_drag: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            direction_degrees: 0.0,
            speed: 5.5,
            gustiness: 1.0,
            turbulence: 0.8,
            ground_drag: 0.45,
        }
    }
}

/// How hard it is raining on the whole scene.
///
/// **One number, because one number is what the rain actually is.** Everything
/// else rain does — which way a streak leans, how sheltered a doorway is, how
/// wet a wall gets — is derived rather than authored: the lean comes from
/// [`Wind`], the shelter from the voxel field the scene already has. A second
/// authored knob for any of those would be a place for the picture and the
/// simulation to disagree, which is the failure this whole phase is arranged
/// to avoid.
///
/// At most one per scene; the first is used, the same rule [`Wind`] and
/// `Environment` follow. **Absent means dry** — not a default drizzle — so
/// every scene authored before rain existed simulates exactly as it did.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Rain {
    /// Rainfall rate in millimetres per hour.
    ///
    /// The meteorological unit, for the same reason [`Wind::speed`] is in m/s:
    /// a dimensionless 0-to-1 "how rainy" leaves every consumer — streak
    /// count, wetness rate, audio level — to invent its own curve against a
    /// number that means nothing, and those curves then drift apart.
    ///
    /// For authoring: `0.5` is drizzle you would not open an umbrella for,
    /// `2` light rain, `4` steady moderate rain, `16` heavy, `50` a tropical
    /// downpour. Above about `100` nothing in nature sustains it for an hour,
    /// which is why that is the ceiling.
    #[schemars(range(min = 0.0, max = 100.0))]
    pub intensity: f32,
    /// Seconds the shower lasts, from the start of the run. Omit for rain that
    /// never stops.
    ///
    /// **The only reason this exists is that drying is not otherwise
    /// expressible.** Wetness accumulates while it rains and recovers after,
    /// and "after" needs a moment the rain ends at — without one, the second
    /// half of the effect is unreachable from a scene file and unassertable
    /// from `loom sim`. It is one number rather than a schedule because a
    /// schedule is a thing to author for a game that wants weather, and nothing
    /// wants that yet.
    ///
    /// Past it the rate is zero everywhere: the streaks stop and the audio
    /// stops. The surfaces stay wet for as long as `loom_rain::wetness` says
    /// they do, which is minutes, and that gap is the point.
    #[schemars(range(min = 0.0, max = 86400.0))]
    pub duration: Option<f32>,
}

impl Default for Rain {
    /// Steady moderate rain — the rate a scene means when it says "raining"
    /// and nothing more.
    fn default() -> Self {
        Self { intensity: 4.0, duration: None }
    }
}

/// The most waves one body may sum.
///
/// **Per-vertex cost is linear in this**, and an agent tuning "make the sea
/// rougher" has no intuition for that — it will add waves until the frame time
/// goes. Unreal defaults to 16 and documents fewer as more performant, so 16 is
/// the ceiling here too: enough that the pattern does not visibly repeat, few
/// enough that a water mesh stays affordable.
pub const MAX_WAVES: usize = 16;

/// What shape of water this is.
///
/// Three bodies, one component, following Unreal — the factoring is right and
/// the difference between them is *how the surface is bounded and driven*, not
/// what a wave is.
///
/// **The surface is still one Gerstner sum for all three**, at one still-water
/// level, and a lake is not yet clipped to a polygon. What a river now has that
/// the others do not is [`WaterBody::flow`] — a current, derived from the
/// terrain's drainage, that carries what floats on it. Authoring the kind means
/// the scene says what it means before the renderer can tell the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaterKind {
    /// Unbounded, and the only one whose surface is correct today.
    #[default]
    Ocean,
    /// Enclosed still water. Waves should be smaller and shorter than an
    /// ocean's — fetch is what builds a big wave, and a lake has none.
    Lake,
    /// Flowing water. Real rivers are carried by flow, not waves; leave the
    /// wave set nearly flat and expect the flow field to do the work.
    River,
}

/// One trochoidal (Gerstner) wave.
///
/// **The surface is a function, not a simulation.** Everything a wave does is
/// determined by these five numbers plus the time, which is what lets the CPU
/// and the GPU each evaluate it and agree, and what makes `loom sim --assert`
/// trustworthy over water.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GerstnerWave {
    /// Crest-to-crest distance in metres.
    ///
    /// Also sets the speed: deep-water waves travel at `sqrt(g·k)`, so a long
    /// wave is inherently a slow, rolling swell and a short one is chop. There
    /// is no separate speed control for that reason — see `speed_scale`.
    #[schemars(range(min = 0.05, max = 2000.0))]
    pub wavelength: f32,
    /// Crest height above the still-water line, in metres.
    ///
    /// Roughly a twentieth of the wavelength is an ordinary sea. Much more than
    /// that is a wave physics does not produce and the steepness limit will
    /// reject.
    #[schemars(range(min = 0.0, max = 100.0))]
    pub amplitude: f32,
    /// `Q` — how peaked the crests are and how flat the troughs, `0` to `1`.
    ///
    /// Zero is a plain sine. Raising it pinches the crests, which is what makes
    /// a sea look like water rather than like corrugated iron. Too high and the
    /// wave loops through itself and the surface self-intersects, so this is
    /// bounded by amplitude and wavelength together — the validator computes the
    /// limit and reports it.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub steepness: f32,
    /// Travel direction on the XZ plane. Normalised on use, so any length works.
    ///
    /// **Spread these out.** Waves all facing one way read as a rippling sheet;
    /// what breaks the repetition is a few directions fanned around the wind,
    /// with the longest wave most closely aligned to it.
    pub direction: [f32; 2],
    /// Multiplier on the physically correct phase speed. `1.0` is correct.
    ///
    /// A knob for taste, not for physics: a scene that wants a livelier sea
    /// without changing its scale reaches for this. Far from `1.0` and the
    /// waves stop looking like water, because speed and wavelength no longer
    /// agree.
    #[schemars(range(min = 0.05, max = 4.0))]
    pub speed_scale: f32,
}

impl Default for GerstnerWave {
    fn default() -> Self {
        Self {
            wavelength: 12.0,
            amplitude: 0.35,
            steepness: 0.5,
            direction: [1.0, 0.0],
            speed_scale: 1.0,
        }
    }
}

/// The waves on one body.
///
/// **Empty by default, and that is deliberate.** Hand-authored amplitudes are
/// about to be replaced by a spectrum derived from the scene's `Wind` — fetch
/// and wind speed give a sea state, and authoring numbers now would mean
/// authoring them twice. Until then a wave set says what it says, and an empty
/// one is a mirror-flat surface rather than an invented sea.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WaveSet {
    /// The summed waves, at most [`MAX_WAVES`] of them.
    ///
    /// Summing several wavelengths and directions is the entire trick: one wave
    /// is a corrugation, and four or five with no common factor between their
    /// wavelengths is a sea.
    #[schemars(length(max = 16))]
    pub waves: Vec<GerstnerWave>,
    /// Depth in metres below which waves start to flatten.
    ///
    /// Real waves feel the bottom as they approach a beach, and this is the
    /// depth at and beyond which they are still their whole selves: below it
    /// each wave's amplitude is scaled by `tanh(k·d) / tanh(k·D)`, reaching zero
    /// at the shoreline. The longest waves flatten first, which is what `k`
    /// being in there buys.
    ///
    /// **Zero — the default — turns attenuation off entirely**, so an ocean
    /// authored before the shallows existed is unchanged. Both halves of the
    /// engine read it: the shader attenuates per vertex, and buoyancy samples
    /// the same flattened surface, so a crate in a foot of water does not ride
    /// the open sea's swell. See ADR 0011 for what the model leaves out —
    /// no shoaling amplification, no refraction, no wavelength shortening.
    ///
    /// **Author the sea floor deeper than this at the edge of the terrain
    /// volume** — and deeper than the shader's shallow-water tint ramp too.
    /// Outside the volume there is no terrain and the water is bottomless, so
    /// anything reading depth that has not saturated by the volume's edge draws
    /// the edge: a step in the wave height, or a rectangle of differently
    /// tinted water around the terrain. `assets/test/shore.loom` is the worked
    /// example.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub attenuation_depth: f32,
    /// Hard clamp on displacement above the still-water line, in metres.
    ///
    /// This is what the water mesh's bounds are built from, so it must be at
    /// least the summed amplitudes — a clamp that is too low crops the crests,
    /// and one that is far too high wastes the bounds it exists to define.
    #[schemars(range(min = 0.0, max = 200.0))]
    pub max_height: f32,
}

/// A body of water: an analytic, stateless, deterministic surface.
///
/// **No fluid simulation and no GPU readback.** The surface is a sum of
/// travelling waves evaluated from position and time, so physics and rendering
/// each compute it independently and get the same answer — which is the only
/// arrangement in which a buoyancy assertion means anything. A boat floating
/// visibly above the water it is drawn on is what the alternative looks like.
///
/// **Water is a plane that terrain does not drain.** Blowing a crater in a lake
/// bed below the waterline does not empty the lake; the surface ignores it. A
/// known and deliberate v1 limitation, written down here rather than discovered.
///
/// What the terrain *does* decide is depth, and depth decides three things: how
/// big the waves are here ([`WaveSet::attenuation_depth`]), how the shallows are
/// tinted, and where the surface stops being drawn at all. The bed is read from
/// the voxel SDF on the CPU, baked to a grid, and read by the shader from that
/// same grid — one number, two lookups (ADR 0011).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WaterBody {
    /// Ocean, lake or river. See [`WaterKind`] for what is implemented.
    pub kind: WaterKind,
    /// Still-water level in world Y, before any wave displacement.
    pub surface_height: f32,
    /// The waves summed over that level.
    pub waves: WaveSet,
    /// Density in kg/m³. `1000` fresh, `1025` salt.
    ///
    /// Read by buoyancy: it is what makes a floating object sit where it does,
    /// so the difference between fresh and salt is a real difference in
    /// waterline.
    #[schemars(range(min = 1.0, max = 20000.0))]
    pub density: f32,
    /// Linear drag coefficient on a submerged body.
    ///
    /// **Also what a river pushes with.** Drag against the *relative* velocity
    /// of the water is how a current carries anything, and it is the only path
    /// by which [`Self::flow`] reaches a rigid body — a river with `drag = 0`
    /// is a current that flows past everything without touching it.
    #[schemars(range(min = 0.0, max = 100.0))]
    pub drag: f32,
    /// Makes this water a river: how fast it runs, and nothing about where.
    ///
    /// **Rivers only, and `None` for everything else.** Absent — the default —
    /// is water that goes nowhere, which is every scene authored before this
    /// existed and every ocean and lake authored after it.
    pub flow: Option<FlowField>,
    /// The surface material.
    pub material: AssetRef,
}

impl Default for WaterBody {
    fn default() -> Self {
        Self {
            kind: WaterKind::Ocean,
            surface_height: 0.0,
            waves: WaveSet::default(),
            // Salt water, because the default kind is an ocean and the two
            // defaults should describe one thing rather than half of each.
            density: 1025.0,
            drag: 1.0,
            flow: None,
            material: AssetRef::default(),
        }
    }
}

/// A river's current: one number, because the terrain supplies the rest.
///
/// **There is deliberately no direction and no path in here.** Where the water
/// runs is a property of the ground, not of the author: `loom_water::flow`
/// routes water downhill across the same height grid the depth comes from,
/// accumulates what drains through each cell, and takes the direction from the
/// bed's own gradient. An author who could type a direction here could type one
/// that runs up a hill, and the agent — which cannot place spline control
/// points well anyway — would have to guess the terrain's shape to fill it in.
/// See the water doc §5.8: this is the one place Loom's architecture is
/// straightforwardly better than Unreal's, and it is better precisely by having
/// fewer fields.
///
/// So the only thing left to author is the scale, which the terrain genuinely
/// cannot supply: a drainage map says where the water is and how much of it
/// there is, not whether this is a mountain torrent or an estuary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FlowField {
    /// Speed in m/s where the drainage is heaviest — the river's own middle.
    ///
    /// Everywhere else is a fraction of it, falling off with the log of the
    /// catchment draining through that point, so a headwater trickles and the
    /// main channel runs. Zero is a river that does not move, which is what a
    /// mutation test sets it to.
    #[schemars(range(min = 0.0, max = 30.0))]
    pub speed: f32,
}

impl Default for FlowField {
    fn default() -> Self {
        // A brisk walking pace. Fast enough to visibly carry a crate within a
        // few seconds of simulated time, slow enough to be a river rather than
        // a flume.
        Self { speed: 2.0 }
    }
}

/// The most pontoons one floating body may carry.
///
/// Every pontoon is a full water sample plus a force accumulation, per body,
/// per fixed step — so this is a cost multiplier and an agent has no intuition
/// for it. Four is what a crate needs; sixteen is past the point where more
/// spheres describe the shape any better than a bigger radius would.
pub const MAX_PONTOONS: usize = 16;

/// One sphere standing in for part of a floating object.
///
/// Unreal's word, and Unreal's approximation: a floating body is a handful of
/// spheres rather than a real volume integral against its collider. It is
/// cheap, it is stable, and — the part that matters — the force from each one
/// lands at a *different place*, which is where the righting torque comes from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Pontoon {
    /// Centre in the node's local space, metres.
    ///
    /// Spread them: pontoons stacked at the origin sum to one force at the
    /// centre of mass and the object spins freely, which is [`Buoyancy`]'s
    /// documented trap.
    pub offset: [f32; 3],
    /// Sphere radius, metres.
    #[schemars(range(min = 0.001, max = 100.0))]
    pub radius: f32,
}

impl Default for Pontoon {
    fn default() -> Self {
        Self {
            offset: [0.0; 3],
            radius: 0.5,
        }
    }
}

/// Makes a rigid body float on the scene's water.
///
/// **This is simulation, not rendering.** Unlike the water surface and the
/// grass, buoyancy applies forces to rigid bodies, so it is inside the
/// determinism hash and every rule that protects it applies: the pontoons are
/// summed in index order into one force and one torque, applied once, from the
/// CPU wave function alone. Nothing here reads GPU memory, ever.
///
/// **Damping is not optional and is on by default.** An analytic wave field is
/// an infinite energy source with nothing to dissipate it, so an undamped
/// pontoon gains amplitude every wave until the object launches. It takes tens
/// of seconds to become obvious, which is why the check for it is a 1800-tick
/// `loom sim` assertion rather than a screenshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Buoyancy {
    /// The spheres, at most [`MAX_PONTOONS`] of them.
    ///
    /// **Empty means four, derived from the node's own size** — at the four
    /// horizontal corners, with radii summing to the body's volume. That is
    /// the documented default made real rather than written down: one pontoon
    /// gives no torque and the object spins, and hand-placed corner spheres
    /// with a radius each big enough to look right total several times the
    /// object's actual volume, so it floats like a cork.
    #[schemars(length(max = 16))]
    pub pontoons: Vec<Pontoon>,
    /// Multiplier on the buoyant force. `1.0` is Archimedes.
    ///
    /// Below one for something waterlogged, above one for a float that has to
    /// hold more than its own volume up. Taste, not physics.
    #[schemars(range(min = 0.0, max = 10.0))]
    pub coefficient: f32,
    /// First-order damping on a submerged pontoon's velocity, per second.
    ///
    /// **This is the one that stops the bobbing, and zero resonates.** It is a
    /// rate rather than a force: the damping is applied to the mass of water
    /// the pontoon has displaced, so the same number means the same thing on a
    /// buoy and on a barge. Read it as how fast the bob dies — `4` takes most
    /// of the speed out of it in a second.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub damp_linear: f32,
    /// Second-order damping, per metre. Only ever applied underwater.
    ///
    /// Scaled the same way, and it is what keeps a fast entry from punching
    /// through the surface — at entry speeds the linear term alone is far too
    /// weak, and at rest this one contributes nothing.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub damp_quadratic: f32,
}

impl Default for Buoyancy {
    fn default() -> Self {
        Self {
            pontoons: Vec::new(),
            coefficient: 1.0,
            // A pontoon float's natural frequency is a few radians per second
            // for anything crate-sized, so this is roughly a third of critical
            // damping: enough to kill an entry's bob within a few seconds,
            // little enough that the object still rides the waves it is on.
            damp_linear: 4.0,
            damp_quadratic: 1.0,
        }
    }
}

/// Tunes when a floating body counts as being *in* the water.
///
/// **Underwater is a gameplay state, not a shader effect** (water doc §5.7).
/// Scripts branch on it, entering the water is what makes a splash, and the
/// event log records both edges — so the question "is this body under" gets one
/// answer, computed once per step, rather than one per system that wanted to
/// know.
///
/// The answer itself is not authored here and cannot be: it is the submerged
/// fraction the buoyancy solver already computes, because it is what Archimedes
/// multiplies. Every node with [`Buoyancy`] has it whether or not it carries
/// this component; what this authors is where the two thresholds sit.
///
/// **Two thresholds, not one, and that is the entire component.** A body
/// floating at its waterline sits at some fixed fraction, and a wave passing
/// under it moves that fraction up and down through whatever single number you
/// compare against — so the state flips several times a second, every script
/// watching it fires on every flip, and the splash re-fires with each one. The
/// gap between `enter` and `exit` is what makes the state hold still. Set them
/// equal and it chatters; that is a choice, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Submersion {
    /// Fraction of the body that must be under before it counts as submerged.
    ///
    /// Above the fraction it floats at when settled, or it is submerged while
    /// bobbing on the surface. Default `0.6` — comfortably more than half under
    /// is a thing that has gone *in*, rather than a thing sitting on top.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub enter: f32,
    /// Fraction it must fall back below before it stops counting.
    ///
    /// Below the settled waterline of anything that is meant to surface again.
    /// Default `0.3`.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub exit: f32,
}

impl Default for Submersion {
    fn default() -> Self {
        Self {
            enter: 0.6,
            exit: 0.3,
        }
    }
}

/// A sound that plays from this node's position.
///
/// **What it sounds like from where you stand is measured, not authored.**
/// There are no reverb volumes to place and no occlusion flags to set: the
/// engine casts rays through the actual level to work out how much of this
/// reaches the listener, how much material it crossed, and how much the room
/// sends back. Move a wall and the sound changes, because the wall is what
/// was measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AudioSource {
    /// The `[[asset]]` alias of a 16-bit PCM WAV.
    pub clip: AssetRef,
    /// Loudness before anything the world does to it.
    #[schemars(range(min = 0.0, max = 8.0))]
    pub volume: f32,
    /// Metres at which it has faded to nothing.
    ///
    /// Linear, not inverse-square. Inverse-square is correct and unusable —
    /// deafening at a metre and inaudible at ten — so every game uses a curve
    /// a level designer can reason about, and this is that curve.
    #[schemars(range(min = 0.1, max = 10000.0))]
    pub range: f32,
    /// Start again when it ends. For a generator hum or a fire, not a gunshot.
    pub looping: bool,
    /// Play as soon as the scene runs. A one-shot triggered by an event
    /// leaves this off.
    pub autoplay: bool,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: AssetRef::default(),
            volume: 1.0,
            range: 25.0,
            looping: false,
            autoplay: true,
        }
    }
}

/// The game's rules: a script that runs once per tick for the whole scene.
///
/// **This is the game loop, and none of it is in the engine.** The engine
/// contributes the loop — run this every tick, after everything has moved,
/// and stop when it says the game is over. What winning means, what health
/// is, whether there is a score at all: rules, and rules are authored.
///
/// Distinct from `Script`, which belongs to a node. A `Script` on a character
/// is its movement model and on anything else moves its transform; both are
/// about one node. Rules are about the game, so they hang off no node in
/// particular and keep their own state, which outlives any character.
///
/// The script reads `positions` and `detonations`, and writes `state`,
/// `status` (`"playing"`, `"won"`, `"lost"`) and `message`. It cannot move
/// anything — a rule that could teleport a node would be a movement model
/// wearing a different hat, and that already has a seam.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GameRules {
    /// Project-relative path to a `.rhai` file.
    pub path: String,
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
    reg.register::<CharacterController>("CharacterController");
    reg.register::<Blast>("Blast");
    reg.register::<GameRules>("GameRules");
    reg.register::<Hud>("Hud");
    reg.register::<AudioSource>("AudioSource");
    reg.register::<Environment>("Environment");
    reg.register::<Wind>("Wind");
    reg.register::<Rain>("Rain");
    reg.register::<Grass>("Grass");
    reg.register::<Scatter>("Scatter");
    // `WaveSet`, `GerstnerWave` and `Pontoon` are deliberately absent: they are
    // fields of a `WaterBody` or a `Buoyancy`, not things a node can carry.
    // Registering them would make `components.WaveSet = { ... }` validate
    // cleanly and then be read by nothing, which is the silent-no-op failure
    // the registry exists to stop.
    reg.register::<WaterBody>("WaterBody");
    reg.register::<Buoyancy>("Buoyancy");
    reg.register::<Submersion>("Submersion");
    reg.register::<Script>("Script");
    reg
}


#[cfg(test)]
mod ground_layer_tests {
    use super::{GroundLayer, Grass, Material};

    /// **A material with no layer must serialize with no `layer` key at all.**
    ///
    /// `loom scene --tx` round-trips every material it touches. Without
    /// `skip_serializing_if`, the first transaction against any scene would
    /// write a `layer` table into every material in the project — a diff
    /// touching hundreds of lines nobody asked for, in a system whose first
    /// property is that everything authored is diffable text.
    ///
    /// Mutation: delete the `skip_serializing_if` attribute and this fails on
    /// the serialized string.
    #[test]
    fn a_material_without_a_layer_serializes_without_the_key() {
        let json = serde_json::to_string(&Material::default()).expect("serialize");
        assert!(!json.contains("layer"), "an absent layer wrote itself into the file: {json}");

        let back: Material = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, Material::default());
    }

    /// A layer that IS authored survives the round trip, including the field
    /// the shader thresholds on.
    #[test]
    fn an_authored_layer_round_trips() {
        let material = Material {
            layer: Some(GroundLayer { slope: 0.42, roughness: 0.31, ..GroundLayer::default() }),
            ..Material::default()
        };
        let json = serde_json::to_string(&material).expect("serialize");
        assert!(json.contains("layer"), "an authored layer vanished: {json}");
        let back: Material = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, material);
        assert!((back.layer.expect("layer").slope - 0.42).abs() < 1e-6);
    }

    /// **The layer's default slope matches `Grass::slope_cutoff`'s, and that is
    /// not decoration.** Grass stopping at one angle while the rock beneath it
    /// starts at another draws two concentric rings around the same hill, which
    /// is worse than either boundary on its own.
    #[test]
    fn the_layer_slope_default_matches_the_grass_cutoff() {
        assert!(
            (GroundLayer::default().slope - Grass::default().slope_cutoff).abs() < 1e-6,
            "the ground layer and grass would draw two different boundaries"
        );
    }
}
