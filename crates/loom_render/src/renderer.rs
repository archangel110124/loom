//! Headless offscreen renderer: draw a scene to a `VkImage`, read it back, PNG.
//!
//! Brief §7.1 — this is the highest-leverage decision in the project. The agent
//! cannot look at a window, so PNG writeout lands *before* the swapchain and
//! becomes the primary render path rather than a feature bolted on later. That
//! also guarantees `render_preview` can never diverge from "the real renderer",
//! because it *is* the real renderer.
//!
//! Dynamic rendering only — no `VkRenderPass`, no `VkFramebuffer` (never-do #1).
//! All memory through `gpu-allocator` (never-do #3).

use ash::vk;

use crate::raytrace::Raytracer;
use glam::{Mat4, Vec3};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc,
};

use loom_render_graph::{Access, GpuTimers, RenderGraph, Transition};

use crate::debug_names::DebugNames;
use crate::{Device, Instance};

/// **sRGB, not UNORM, and that is the whole colour pipeline in one word.**
///
/// Every lighting term in `scene.slang` is computed in linear light, which is
/// the only space where adding two lights or multiplying an albedo by a
/// cosine means anything. Those linear values then have to be *encoded* before
/// they land in eight bits, because every viewer that opens the PNG will read
/// them back as sRGB.
///
/// With `UNORM` nothing encoded them, so the whole image was displayed about a
/// gamma too dark. It went unnoticed for a long time because every colour in
/// the engine was a constant somebody had tuned until it looked right on
/// screen — which quietly folded the missing encode into the constants. The
/// first real sRGB texture, correctly linearised on read and then never
/// re-encoded on write, is what made it obvious.
///
/// `_SRGB` makes the hardware do the encode on write, so the bytes read back
/// for the PNG are already in the space the PNG says they are in.
pub(crate) const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
pub(crate) const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// One object to draw.
#[derive(Debug, Clone, Copy)]
pub struct Object {
    /// World transform.
    pub model: Mat4,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Index into the mesh library the renderer was built with.
    pub mesh: u32,
    /// Index into the material table, or `u32::MAX` for none — in which case
    /// `color` is used directly, which is what keeps an untextured blockout
    /// readable.
    pub material: u32,
}

/// One particle, as the GPU draws it.
///
/// Mirrors `ParticleInstance` in `scene.slang`. Two `vec4`s and nothing else:
/// a plume is thousands of these uploaded every frame, so every byte here is
/// paid per particle per frame.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ParticleInstance {
    /// xyz world position, w radius in metres.
    pub position: [f32; 4],
    /// Linear RGB and opacity.
    pub color: [f32; 4],
}

/// Where one mesh lives inside the combined index buffer.
///
/// Indices are absolute into the shared vertex buffer, so `vertexOffset` is
/// always zero and the shader can read `vertices[SV_VertexID]` unchanged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeshRange {
    first_index: u32,
    index_count: u32,
}

impl MeshRange {
    pub(crate) fn first_index(self) -> u32 {
        self.first_index
    }

    pub(crate) fn index_count(self) -> u32 {
        self.index_count
    }
}

/// The sky, the sun and the haze, as a scene sets them.
///
/// These were shader constants: one blue afternoon, baked in, the same in
/// every scene. A scene could place lights but could not turn the sun down,
/// so "night" was not expressible at all — the ambient term alone lit
/// everything to 95%.
///
/// Packed into four vectors because the trailing scalar of each is free and
/// a push-constant block has 128 bytes total.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentData {
    /// xyz sun direction (normalised), w its strength.
    pub sun: [f32; 4],
    /// rgb sun colour, a ambient strength.
    pub sun_color: [f32; 4],
    /// rgb sky overhead, a fog density.
    pub zenith: [f32; 4],
    /// rgb sky at the horizon, a fog falloff with height.
    pub horizon: [f32; 4],
    /// Camera position. **Here rather than in the push block**, which was at
    /// exactly its 128-byte guarantee before grass needed a pointer.
    pub eye: [f32; 4],
    /// x viewport width in pixels, y height, zw unused.
    ///
    /// **The grass vertex shader reasons in pixels**, because the whole
    /// minimum-width trick is "no blade may be thinner than about one pixel"
    /// and a shader cannot know what a pixel is without this.
    pub viewport: [f32; 4],
    /// xy wind direction, z speed, w gustiness.
    ///
    /// **Wind lives here rather than in the push block**, which is already at
    /// 116 of its 128 bytes. It is per-scene data read by whatever wants it,
    /// which is what this buffer is for.
    pub wind: [f32; 4],
    /// x turbulence, y ground drag, z seconds simulated, w unused.
    ///
    /// Time is here because the vertex shader needs it to bend a blade, and
    /// it is the simulation's clock — the tick count times the fixed timestep,
    /// never a wall clock (never-do #8). The water surface reads the same
    /// clock, so grass and waves cannot drift apart.
    pub weather: [f32; 4],
    /// x still-water level, y 1 when the eye is submerged, z 1 when the scene
    /// has water, w unused.
    ///
    /// **Depth used to live in `y` as a constant.** It is a real per-vertex
    /// query now: `surface_height - terrain_height(x, z)`, read out of
    /// [`Self::terrain`] and the height buffer beside it.
    ///
    /// **`y` is the underwater flag, and it is a CPU answer on purpose.**
    /// "Is the camera under the water" is one bool per frame, out of
    /// `loom_water::buoyancy::submersion_at` — the same query the buoyancy
    /// solver and the audio listener ask. The shader reading it via
    /// `eyeUnderwater()` is what turns the fog into water, the sky into the
    /// water's colour, and the surface into its own underside; recomputing a
    /// wave height per pixel to decide that would be a second answer to a
    /// question W7 already answers.
    pub water: [f32; 4],
    /// The terrain height grid: xy world origin, z metres between samples,
    /// w samples per axis. **`w == 0` means the scene has no terrain**, which
    /// is how an open ocean says "bottomless" without a second flag.
    ///
    /// Mirrors `LoomHeightField` in the generated shader minus its pointer,
    /// which follows immediately below for alignment's sake.
    pub terrain: [f32; 4],
    /// The heights themselves, `w²` of them, row-major.
    ///
    /// **A pointer in the environment buffer rather than in the push block**,
    /// which is at 124 of the 128 bytes Vulkan guarantees. It is per-scene data
    /// re-uploaded only when the terrain changes, which is exactly what this
    /// buffer is for.
    pub terrain_heights: vk::DeviceAddress,
    /// The sea, as [`WaterWave`]s — the same sixteen `loom_water` derives from
    /// the wind, in the same order.
    ///
    /// **Here rather than behind another pointer.** The push block is at 116
    /// of the 128 bytes Vulkan guarantees, and this is per-scene data read once
    /// per vertex, which is what this buffer is for. It is last in the struct
    /// so every offset above it is unmoved.
    pub waves: [WaterWave; MAX_WAVES],
    /// How many of `waves` to sum. Zero is a mirror.
    pub wave_count: u32,
    /// Depth in metres below which waves flatten. Zero disables it.
    ///
    /// Inside the wave set rather than beside it because that is where
    /// `LoomWaveSet` keeps it, and the two are one memory layout described
    /// twice.
    pub attenuation_depth: f32,
}

/// The cap on summed waves, mirroring `loom_scene::components::MAX_WAVES` and
/// the generated shader's `LOOM_MAX_WAVES`.
///
/// Spelled again here rather than imported: `loom_render` does not depend on
/// `loom_scene`, and `loom_water`'s own test asserts the shader's bound equals
/// the scene layer's. A test below asserts this one matches the shader too.
pub const MAX_WAVES: usize = 16;

/// One Gerstner wave, as the vertex shader reads it.
///
/// Mirrors `LoomWave` in the generated shader field for field, **including the
/// order**: `direction` leads because it is the only member needing 8-byte
/// alignment, and a scalar in front of it would pad the two layouts
/// differently. 24 bytes, no holes on either side.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WaterWave {
    pub direction: [f32; 2],
    pub wavelength: f32,
    pub amplitude: f32,
    pub steepness: f32,
    pub speed_scale: f32,
}

impl Default for EnvironmentData {
    fn default() -> Self {
        // The afternoon the constants used to hard-code, so a scene that says
        // nothing looks exactly as it did.
        Self {
            sun: [0.3906, 0.8790, 0.2930, 0.62],
            sun_color: [1.0, 0.98, 0.94, 0.95],
            zenith: [0.0272, 0.0946, 0.3424, 0.0026],
            horizon: [0.3931, 0.5071, 0.6038, 0.03],
            // Beaufort 4 from the west, matching `loom_field::wind_defaults`.
            eye: [0.0; 4],
            viewport: [1.0, 1.0, 0.0, 0.0],
            wind: [1.0, 0.0, 5.5, 1.0],
            weather: [0.8, 0.45, 0.0, 0.0],
            // No water: `z` is the flag the draw is skipped on, so a scene
            // that authors no `WaterBody` renders exactly as it did.
            water: [0.0, 0.0, 0.0, 0.0],
            // No terrain: side 0, so every depth query is bottomless.
            terrain: [0.0, 0.0, 1.0, 0.0],
            terrain_heights: 0,
            waves: [WaterWave {
                direction: [1.0, 0.0],
                wavelength: 0.0,
                amplitude: 0.0,
                steepness: 0.0,
                speed_scale: 1.0,
            }; MAX_WAVES],
            wave_count: 0,
            attenuation_depth: 0.0,
        }
    }
}

/// How many samples the offscreen renderer rasterises with.
///
/// **Four, and the number is measured rather than assumed.** P2's central
/// question is whether grass can be made stable without temporal
/// accumulation, and MSAA is the only tool in the non-temporal kit that works
/// on geometry silhouettes directly. `cargo xtask shimmer` is what says
/// whether a given count earns its bandwidth; the phase notes carry the
/// readings.
pub(crate) const MSAA_SAMPLES: vk::SampleCountFlags = vk::SampleCountFlags::TYPE_4;

/// Heights the terrain buffer holds: `loom_voxel::heightfield::MAX_SIDE²`.
///
/// Spelled here rather than imported, because `loom_render` does not depend on
/// `loom_voxel` — nothing in the renderer knows what a voxel is, and the bake
/// that fills this lives at the CLI layer. The number is a ceiling: the CPU
/// coarsens its grid rather than exceeding it.
pub const MAX_TERRAIN_SAMPLES: usize = 256 * 256;

/// Vertices in one water draw: `WATER_RES² × WATER_LEVELS × 6`.
///
/// **One memory layout described twice again** — the three constants live in
/// `scene.slang` and the draw count lives here, and a mismatch shows up as a
/// missing outer ring or as a wasted level drawn on top of itself. Spelled out
/// rather than computed from named constants on this side, because the numbers
/// that matter are the shader's; a test asserts they still agree.
pub(crate) const WATER_VERTS: u32 = 32 * 32 * 7 * 6;

/// The multisampled render targets, when there are any.
///
/// Held together because they live and die together: both are transient, both
/// are recreated on resize, and a half-recreated pair is a validation error
/// rather than a visible bug.
struct Msaa {
    image: vk::Image,
    alloc: Allocation,
    view: vk::ImageView,
    depth_image: vk::Image,
    depth_allocation: Allocation,
    depth_view: vk::ImageView,
}

/// One grass blade, as the vertex shader reads it.
///
/// **Packed into `float4`s rather than mirroring `loom_grass::Blade` field for
/// field.** A `float3` in a buffer still aligns to 16 bytes, so the natural
/// layout would leave holes the Rust side does not have and every blade after
/// the first would read shifted — the trap `scene.slang`'s push block already
/// documents. Three `float4`s have no holes to get wrong.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GrassBlade {
    /// xyz base position, w height.
    pub position: [f32; 4],
    /// xy facing, z width, w rest tilt.
    pub facing: [f32; 4],
    /// x bend, y shade, z clump hash as a float, w the clump's albedo packed
    /// 8:8:8 (`loom_cli::pack_rgb`, unpacked in `grassVertexMain`).
    pub shape: [f32; 4],
}

/// What the camera looks at.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y_degrees: f32,
}

/// Push-constant block: two device addresses, nothing else.
///
/// Must match `Push` in `scene.slang` exactly — a mismatch is garbage on screen
/// with no diagnostic (brief §7.7), so the sizes are asserted in a test.
/// Keeping only pointers here means per-object fields can grow without ever
/// running into the 128-byte push-constant limit.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Push {
    /// NDC to world, for the sky's per-pixel ray.
    ///
    /// **First, matching `scene.slang`.** A 4x4 needs 16-byte alignment, so
    /// putting it after the pointers needs padding — and a `uint3` pad in
    /// Slang aligns to 16 itself, which silently moves the matrix somewhere
    /// the Rust side is not writing it. Leading with it removes the padding
    /// and the mistake.
    pub(crate) inv_view_proj: [f32; 16],
    pub(crate) vertices: vk::DeviceAddress,
    pub(crate) objects: vk::DeviceAddress,
    /// The material table. Another pointer rather than another descriptor:
    /// materials are read by index from the fragment stage and never bound.
    pub(crate) materials: vk::DeviceAddress,
    /// The particle instances for this frame. Null when there are none.
    pub(crate) particles: vk::DeviceAddress,
    /// This frame's grass blades. Null when the scene has none.
    ///
    /// **The block is at 124 of its 128 bytes with this**, which is why the
    /// wind parameters the vertex shader also needs live in the environment
    /// buffer instead. There is room for nothing else here.
    pub(crate) grass: vk::DeviceAddress,
    /// The sky and sun. Placed before `object_offset`, not after: a device
    /// address needs eight-byte alignment, and putting it last would pad the
    /// block to exactly the 128 bytes Vulkan guarantees, with nothing spare.
    pub(crate) environment: vk::DeviceAddress,
    /// First object this draw should read. See the note in `scene.slang`.
    ///
    /// **The particle pass borrows this too.** Its vertex shader needs the
    /// view-projection matrix to place a billboard, and the push block has no
    /// room for a second `float4x4` — 64 more bytes would put it over the 128
    /// Vulkan guarantees. So the particle draw points this at a reserved
    /// `ObjectData` slot whose `mvp` is the view-projection and whose other
    /// fields are unused. The buffer is already bound; the matrix rides along
    /// for free.
    pub(crate) object_offset: u32,
}

impl Push {
    /// The bytes to push, as Vulkan wants them.
    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `#[repr(C)]`, all fields are plain data, and the slice
        // borrows from `self`.
        unsafe {
            std::slice::from_raw_parts(std::ptr::from_ref(self).cast::<u8>(), size_of::<Self>())
        }
    }
}

/// Per-object data, indexed by `SV_InstanceID`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ObjectData {
    mvp: [f32; 16],
    /// The model matrix, so the vertex stage can emit a world position for the
    /// fragment stage to trace shadow rays from. The MVP alone cannot: it has
    /// the view and projection baked in and is not cheaply invertible.
    model: [f32; 16],
    /// Decode parameters for this object's packed vertices: xyz origin, w step.
    unpack: [f32; 4],
    /// Decode parameters for this object's packed UVs: xy origin, z step.
    uv_unpack: [f32; 4],
    /// Rows of inverse-transpose(model)'s upper 3x3, padded to `vec4`.
    normal: [[f32; 4]; 3],
    color: [f32; 4],
    /// Material index in `x`; the rest pads to the 16-byte alignment a
    /// std430 block needs for the member that follows it.
    material: [u32; 4],
}

/// Anything that can go wrong rendering.
#[derive(Debug)]
pub enum RenderError {
    Vulkan(vk::Result),
    Allocator(String),
    /// Validation was not silent. Carries the messages.
    Validation(Vec<String>),
    Io(std::io::Error),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "vulkan error: {r:?}"),
            Self::Allocator(e) => write!(f, "allocation failed: {e}"),
            Self::Validation(m) => write!(f, "validation was not silent:\n  {}", m.join("\n  ")),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<vk::Result> for RenderError {
    fn from(r: vk::Result) -> Self {
        Self::Vulkan(r)
    }
}

/// An offscreen renderer sized to one output resolution.
pub struct Renderer {
    device: ash::Device,
    queue: vk::Queue,
    allocator: Option<Allocator>,

    width: u32,
    height: u32,

    color: vk::Image,
    color_view: vk::ImageView,
    color_alloc: Option<Allocation>,
    depth: vk::Image,
    depth_view: vk::ImageView,
    depth_alloc: Option<Allocation>,
    readback: vk::Buffer,
    readback_alloc: Option<Allocation>,
    vertices: vk::Buffer,
    vertices_alloc: Option<Allocation>,
    vertex_address: vk::DeviceAddress,
    indices: vk::Buffer,
    indices_alloc: Option<Allocation>,
    ranges: Vec<MeshRange>,
    unpack: UnpackParams,
    objects: vk::Buffer,
    objects_alloc: Option<Allocation>,
    object_address: vk::DeviceAddress,
    environment_buffer: vk::Buffer,
    environment_alloc: Option<Allocation>,
    environment_address: vk::DeviceAddress,
    /// What the next frame draws with. One struct, written per frame.
    pub environment: EnvironmentData,
    max_objects: usize,

    /// `None` when the device has no ray query; shadows are simply skipped.
    raytracer: Option<Raytracer>,
    materials: crate::material::Materials,
    /// Particle instances, rewritten every frame that has any.
    particle_buffer: vk::Buffer,
    particle_alloc: Option<Allocation>,
    particle_address: vk::DeviceAddress,
    /// `None` when rendering at one sample.
    msaa: Option<Msaa>,
    /// The anti-aliasing pass and the image it writes into, or `None` when
    /// `LOOM_CMAA2` is unset — which is the default. When it is present the
    /// readback reads *this* image rather than the colour target.
    aa: Option<(crate::cmaa2::Cmaa2, vk::Image, vk::ImageView, Allocation)>,
    grass_buffer: vk::Buffer,
    grass_alloc: Option<Allocation>,
    grass_address: vk::DeviceAddress,
    grass_pipeline: vk::Pipeline,
    /// The terrain height grid the water reads its depth out of. Written once
    /// per scene, not per frame — see [`Renderer::set_terrain`].
    terrain_buffer: vk::Buffer,
    terrain_alloc: Option<Allocation>,
    terrain_address: vk::DeviceAddress,
    /// xy origin, z spacing, w samples per axis — stamped into the environment
    /// buffer at render time so a caller's per-frame assignment cannot lose it.
    terrain_params: [f32; 4],
    /// The address handed to the shader: `terrain_address`, or null when the
    /// scene has no terrain.
    terrain_heights: vk::DeviceAddress,
    /// The water surface. Drawn only when the environment says the scene has
    /// water; the mesh itself is entirely in the vertex shader, so there is no
    /// buffer beside this.
    water_pipeline: vk::Pipeline,
    /// Blades uploaded this frame. Zero draws nothing at all.
    grass_count: u32,
    max_particles: usize,
    /// Alpha-blended, depth-tested but not depth-writing.
    particle_pipeline: vk::Pipeline,
    /// Unpacked positions the acceleration structures were built from.
    rt_positions: Option<(vk::Buffer, Allocation, vk::DeviceAddress)>,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Fills the frame before the scene draws over it.
    sky_pipeline: vk::Pipeline,
    pipeline_cache: vk::PipelineCache,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    cache_path: Option<std::path::PathBuf>,
    /// What the graph decided last frame. Exposed so a test can assert the
    /// barriers actually happened rather than trusting that they did.
    last_transitions: Vec<Transition>,
    /// `Some` only when `LOOM_GPU_TIMING` is set and the queue can write
    /// timestamps. Off by default: it costs two commands per pass and it
    /// serialises passes slightly, so it must not be paid for by every render.
    timers: Option<GpuTimers>,
    /// Set when `LOOM_GPU_TIMING` asked for a table.
    print_timing: bool,
}

/// Timestamps this renderer's query pool has room for. The offscreen path
/// declares two passes; the slack is so adding one does not silently go
/// unmeasured.
const TIMED_PASSES: u32 = 8;

/// Whether `LOOM_GPU_TIMING` asks for a per-frame table on stderr.
///
/// An environment variable rather than a flag so every command that renders —
/// `loom render`, `loom sim`, `cargo xtask image` — gains the instrument
/// without any of them knowing it exists.
fn timing_requested() -> bool {
    std::env::var_os("LOOM_GPU_TIMING").is_some_and(|v| v != "0")
}

impl Renderer {
    /// Build every resource needed to render at `width` x `height`.
    ///
    /// # Errors
    /// [`RenderError`] if any Vulkan object or allocation fails.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        instance: &Instance,
        device: &Device,
        width: u32,
        height: u32,
        meshes: &[loom_asset::Mesh],
        textures: &[loom_asset::Texture],
        materials: &[crate::material::MaterialData],
    ) -> Result<Self, RenderError> {
        let raw = device.handle().clone();
        let raytracing = device.supports_raytracing();
        // Enabled exactly when the instance enabled the extension.
        let names = DebugNames::new(instance.handle(), &raw, cfg!(debug_assertions));

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.handle().clone(),
            device: raw.clone(),
            physical_device: device.physical(),
            debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
            // Enabled on the device, so the allocator must know.
            buffer_device_address: true,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;

        let (color, color_alloc) = create_image(
            &raw,
            &mut allocator,
            width,
            height,
            COLOR_FORMAT,
            // `SAMPLED` because the anti-aliasing pass reads this image as a
            // texture (ADR 0010). Declared unconditionally: a usage flag
            // changes no pixel, and making it conditional on an environment
            // variable would mean the gate silently changed image creation.
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
            1,
            vk::SampleCountFlags::TYPE_1,
            "loom.color_target",
        )?;
        let (depth, depth_alloc) = create_image(
            &raw,
            &mut allocator,
            width,
            height,
            DEPTH_FORMAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            1,
            vk::SampleCountFlags::TYPE_1,
            "loom.depth_target",
        )?;

        // **The multisampled pair.** Geometry is rasterised into these and
        // resolved into `color` at the end of the pass, so everything
        // downstream — the readback, the golden images — still sees one
        // sample per pixel and needs no changes at all.
        //
        // `TRANSIENT_ATTACHMENT` is the honest usage: nothing ever reads these
        // images, they exist only between the first draw and the resolve, and
        // saying so lets a tiled GPU keep them entirely on-chip.
        let samples = MSAA_SAMPLES;
        let multisampled = samples != vk::SampleCountFlags::TYPE_1;
        let msaa = if multisampled {
            let (image, alloc) = create_image(
                &raw,
                &mut allocator,
                width,
                height,
                COLOR_FORMAT,
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
                1,
                samples,
                "loom.msaa_color",
            )?;
            let (depth_image, depth_allocation) = create_image(
                &raw,
                &mut allocator,
                width,
                height,
                DEPTH_FORMAT,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
                1,
                samples,
                "loom.msaa_depth",
            )?;
            let view = create_view(&raw, image, COLOR_FORMAT, vk::ImageAspectFlags::COLOR)?;
            let depth_view =
                create_view(&raw, depth_image, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;
            Some(Msaa { image, alloc, view, depth_image, depth_allocation, depth_view })
        } else {
            None
        };

        let color_view = create_view(&raw, color, COLOR_FORMAT, vk::ImageAspectFlags::COLOR)?;
        let depth_view = create_view(&raw, depth, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;

        // Host-visible landing zone for the finished image.
        let size = u64::from(width) * u64::from(height) * 4;
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `buffer_info` is fully initialised and outlives the call.
        let readback = unsafe { raw.create_buffer(&buffer_info, None) }?;
        // SAFETY: `readback` was just created on this device.
        let requirements = unsafe { raw.get_buffer_memory_requirements(readback) };
        let readback_alloc = allocator
            .allocate(&AllocationCreateDesc {
                name: "loom.readback",
                requirements,
                location: MemoryLocation::GpuToCpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| RenderError::Allocator(e.to_string()))?;
        // SAFETY: the allocation matches the buffer's requirements.
        unsafe { raw.bind_buffer_memory(readback, readback_alloc.memory(), readback_alloc.offset()) }?;

        // The whole mesh library in one vertex buffer and one index buffer,
        // so switching mesh between draws costs an offset rather than a bind.
        //
        // `ponytail:` host-visible upload, no staging copy. Fine at blockout
        // scale; the dedicated transfer queue and a staging ring (Vulkan doc
        // §6) arrive when a scene streams more geometry than fits comfortably
        // in host-visible memory.
        let (combined_vertices, combined_indices, ranges, unpack) = combine(meshes);
        // Geometry the acceleration structures are built from must say so at
        // creation. Conditional because asking for this bit without the
        // extension enabled is itself a validation error.
        let as_input = if raytracing {
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
        } else {
            vk::BufferUsageFlags::empty()
        };
        let (vertices, vertices_alloc, vertex_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (std::mem::size_of_val(combined_vertices.as_slice()) as u64).max(4),
            "loom.mesh_vertices",
            as_input,
        )?;
        write_slice(&vertices_alloc, &combined_vertices)?;

        let (indices, indices_alloc, index_address) = create_index_buffer(
            &raw,
            &mut allocator,
            (std::mem::size_of_val(combined_indices.as_slice()) as u64).max(4),
            "loom.mesh_indices",
            as_input | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        )?;
        write_slice(&indices_alloc, &combined_indices)?;

        // Per-object array. Sized once; the draw is a single instanced call.
        // A plume is a few thousand; a scene with several is a few tens of
        // thousands. Sized once rather than grown, so a frame never allocates.
        const MAX_PARTICLES: usize = 65536;
        // **A ceiling, not a target.** Ghost of Tsushima drew ~83,000 blades
        // from ~1,000,000 candidates; this is the buffer a CPU-side placement
        // pass can fill before the compute path takes over, and it is what
        // caps the frame cost while there is no culling.
        const MAX_BLADES: usize = 262_144;
        const MAX_OBJECTS: usize = 4096;
        let (objects, objects_alloc, object_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_OBJECTS * size_of::<ObjectData>()) as u64,
            "loom.object_data",
            vk::BufferUsageFlags::empty(),
        )?;
        let (environment_buffer, environment_alloc, environment_address) = create_address_buffer(
            &raw,
            &mut allocator,
            size_of::<EnvironmentData>() as u64,
            "loom.environment",
            vk::BufferUsageFlags::empty(),
        )?;
        let (particle_buffer, particle_alloc, particle_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_PARTICLES * size_of::<ParticleInstance>()) as u64,
            "loom.particles",
            vk::BufferUsageFlags::empty(),
        )?;
        let (grass_buffer, grass_alloc, grass_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_BLADES * size_of::<GrassBlade>()) as u64,
            "loom.grass",
            vk::BufferUsageFlags::empty(),
        )?;
        // 256² floats, which is `loom_voxel::heightfield::MAX_SIDE` squared —
        // the cap the CPU bake coarsens to rather than exceeding. 256 KB, once,
        // for the whole scene.
        let (terrain_buffer, terrain_alloc, terrain_address) = create_address_buffer(
            &raw,
            &mut allocator,
            (MAX_TERRAIN_SAMPLES * size_of::<f32>()) as u64,
            "loom.terrain",
            vk::BufferUsageFlags::empty(),
        )?;

        // Built before the pipeline, because the pipeline layout needs its
        // descriptor set layout. `None` on a device without ray query, which
        // is the whole graceful-degradation path: same pipeline, no shadows.
        let mut raytracer = if raytracing {
            Some(Raytracer::new(instance.handle(), &raw, &mut allocator)?)
        } else {
            None
        };

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(device.queue_family())
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the family index came from this device.
        let command_pool = unsafe { raw.create_command_pool(&pool_info, None) }?;

        // Before the pipeline, because the pipeline layout needs its descriptor
        // set layout — and after the command pool, because getting the textures
        // into a sampleable layout is queue work.
        let materials = crate::material::Materials::new(
            &raw,
            &mut allocator,
            textures,
            materials,
            crate::raytrace::Submit { pool: command_pool, queue: device.queue() },
        )?;

        let cache_path = pipeline_cache_path(instance, device);
        let (pipeline_layout, pipeline, pipeline_cache) =
            create_pipeline(
                &raw,
                cache_path.as_deref(),
                COLOR_FORMAT,
                raytracer.as_ref().map(Raytracer::descriptor_layout),
                materials.descriptor_layout(),
                MSAA_SAMPLES,
            )?;
        let sky_pipeline =
            create_sky_pipeline(&raw, pipeline_layout, pipeline_cache, COLOR_FORMAT, MSAA_SAMPLES)?;
        let particle_pipeline =
            create_particle_pipeline(
                &raw,
                pipeline_layout,
                pipeline_cache,
                COLOR_FORMAT,
                MSAA_SAMPLES,
            )?;
        let grass_pipeline = create_geometry_pipeline(
            &raw,
            pipeline_layout,
            pipeline_cache,
            COLOR_FORMAT,
            MSAA_SAMPLES,
            c"grassVertexMain",
            c"grassFragmentMain",
        )?;
        let water_pipeline = create_geometry_pipeline(
            &raw,
            pipeline_layout,
            pipeline_cache,
            COLOR_FORMAT,
            MSAA_SAMPLES,
            c"waterVertexMain",
            c"waterFragmentMain",
        )?;

        // The anti-aliasing pass (ADR 0010), when asked for. A second colour
        // image, never the same one: the filter reads a 6x6 neighbourhood and
        // writes the centre, so reading and writing one image would make every
        // pixel's result depend on whether its neighbours had been rewritten.
        let aa = if crate::cmaa2::requested() {
            let (image, allocation) = create_image(
                &raw,
                &mut allocator,
                width,
                height,
                COLOR_FORMAT,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                1,
                vk::SampleCountFlags::TYPE_1,
                "loom.aa_target",
            )?;
            let view = create_view(&raw, image, COLOR_FORMAT, vk::ImageAspectFlags::COLOR)?;
            let pass = crate::cmaa2::Cmaa2::new(
                &raw,
                &mut allocator,
                &names,
                pipeline_cache,
                COLOR_FORMAT,
                color_view,
                width,
                height,
            )?;
            names.set(image, "loom.aa_target");
            names.set(view, "loom.aa_target.view");
            Some((pass, image, view, allocation))
        } else {
            None
        };

        names.set(color, "loom.color_target");
        names.set(depth, "loom.depth_target");
        names.set(color_view, "loom.color_target.view");
        names.set(depth_view, "loom.depth_target.view");
        names.set(readback, "loom.readback_buffer");
        names.set(vertices, "loom.mesh_vertices");
        names.set(indices, "loom.mesh_indices");
        names.set(objects, "loom.object_data");
        names.set(pipeline, "loom.scene_pipeline");
        names.set(pipeline_layout, "loom.scene_pipeline_layout");
        names.set(pipeline_cache, "loom.pipeline_cache");

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: pool is live.
        let command_buffer = unsafe { raw.allocate_command_buffers(&alloc_info) }?[0];

        // SAFETY: default create-info is valid.
        let fence = unsafe { raw.create_fence(&vk::FenceCreateInfo::default(), None) }?;

        names.set(command_pool, "loom.command_pool");
        names.set(command_buffer, "loom.command_buffer");
        names.set(fence, "loom.frame_fence");

        // **Ray tracing needs real positions.** The vertex buffer the raster
        // path reads holds `PackedVertex` — quantised, with a per-mesh origin
        // and step the shader undoes. An acceleration structure has no such
        // hook: it reads a plain vertex format at a fixed stride. Handing it
        // the packed buffer as R32G32B32_SFLOAT makes it trace whatever those
        // bits happen to look like as floats, which renders as shadows of a
        // shape that is not in the scene.
        //
        // So the builds get their own buffer of unpacked positions, in the same
        // order `combine` emitted vertices. Built once with the BLAS, never
        // touched per frame.
        let mut rt_positions = None;
        if raytracing {
            let positions: Vec<[f32; 3]> = meshes
                .iter()
                .flat_map(|m| m.vertices.iter())
                .map(|v| [v.position[0], v.position[1], v.position[2]])
                .collect();
            let (buffer, allocation, address) = create_address_buffer(
                &raw,
                &mut allocator,
                (std::mem::size_of_val(positions.as_slice()) as u64).max(4),
                "loom.rt_positions",
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            )?;
            write_slice(&allocation, &positions)?;
            rt_positions = Some((buffer, allocation, address));
        }

        // BLAS per mesh, once. The TLAS is rebuilt per frame in `render`,
        // because objects move and triangles do not.
        if let Some(rt) = raytracer.as_mut() {
            rt.build_meshes(
                &mut allocator,
                crate::raytrace::Submit { pool: command_pool, queue: device.queue() },
                rt_positions.as_ref().map_or(0, |(_, _, a)| *a),
                index_address,
                &ranges,
                u32::try_from(combined_vertices.len()).unwrap_or(0),
            )?;
        }

        // GPU timing, if it was asked for. Queried and branched on rather than
        // assumed (Vulkan doc §14): `timestampValidBits` is per queue family
        // and may be zero, and `timestampPeriod` is nanoseconds per tick — 1.0
        // on this box's driver, ~40 on other hardware.
        let print_timing = timing_requested();
        let timers = if print_timing {
            // SAFETY: `physical` came from this instance.
            let limits = unsafe {
                instance
                    .handle()
                    .get_physical_device_properties(device.physical())
            }
            .limits;
            // SAFETY: same.
            let families = unsafe {
                instance
                    .handle()
                    .get_physical_device_queue_family_properties(device.physical())
            };
            let valid_bits = families
                .get(device.queue_family() as usize)
                .map_or(0, |f| f.timestamp_valid_bits);
            let timers = GpuTimers::new(&raw, limits.timestamp_period, valid_bits, TIMED_PASSES)?;
            match &timers {
                Some(t) => names.set(t.pool(), "loom.timestamps"),
                None => eprintln!(
                    "[loom gpu] timestamps unavailable: timestampValidBits={valid_bits}, \
                     timestampPeriod={}, timestampComputeAndGraphics={}",
                    limits.timestamp_period,
                    limits.timestamp_compute_and_graphics != 0,
                ),
            }
            timers
        } else {
            None
        };

        Ok(Self {
            timers,
            print_timing,
            raytracer,
            materials,
            particle_buffer,
            particle_alloc: Some(particle_alloc),
            particle_address,
            msaa,
            aa,
            grass_buffer,
            grass_alloc: Some(grass_alloc),
            grass_address,
            grass_pipeline,
            terrain_buffer,
            terrain_alloc: Some(terrain_alloc),
            terrain_address,
            terrain_params: [0.0, 0.0, 1.0, 0.0],
            terrain_heights: 0,
            water_pipeline,
            grass_count: 0,
            max_particles: MAX_PARTICLES,
            particle_pipeline,
            rt_positions,
            device: raw,
            queue: device.queue(),
            allocator: Some(allocator),
            width,
            height,
            color,
            color_view,
            color_alloc: Some(color_alloc),
            depth,
            depth_view,
            depth_alloc: Some(depth_alloc),
            readback,
            readback_alloc: Some(readback_alloc),
            vertices,
            vertices_alloc: Some(vertices_alloc),
            vertex_address,
            indices,
            indices_alloc: Some(indices_alloc),
            ranges,
            unpack,
            objects,
            objects_alloc: Some(objects_alloc),
            object_address,
            environment_buffer,
            environment_alloc: Some(environment_alloc),
            environment_address,
            environment: EnvironmentData::default(),
            max_objects: MAX_OBJECTS,
            pipeline_layout,
            pipeline,
            sky_pipeline,
            pipeline_cache,
            command_pool,
            command_buffer,
            fence,
            cache_path,
            last_transitions: Vec::new(),
        })
    }

    /// Render `objects` and return the image as RGBA8 rows, top to bottom.
    ///
    /// # Errors
    /// [`RenderError`] on any Vulkan failure.
    #[allow(clippy::too_many_lines)]
    /// Make sure the object buffer can hold `wanted` objects, growing it if
    /// not. Doubling, so a growing scene reallocates a handful of times.
    ///
    /// # Errors
    /// [`RenderError`] if the device will not idle or the buffer cannot be
    /// reallocated.
    pub fn reserve_objects(&mut self, wanted: usize) -> Result<(), RenderError> {
        if wanted <= self.max_objects {
            return Ok(());
        }
        let mut capacity = self.max_objects.max(1);
        while capacity < wanted {
            capacity = capacity.saturating_mul(2);
        }
        let Some(allocator) = self.allocator.as_mut() else {
            return Ok(());
        };
        // SAFETY: nothing is in flight on this path — `render` submits and
        // waits within one call — but idling costs nothing and makes the
        // invariant local rather than remembered.
        unsafe { self.device.device_wait_idle() }?;
        // **Build before destroying.** Freeing first leaves `self.objects`
        // holding a destroyed handle if the re-create fails, which `Drop` then
        // destroys again — the exact double-free fixed in `set_meshes`, which
        // I reintroduced here while adding buffer growth. A transient
        // allocation spike is the cheaper mistake.
        let (objects, objects_alloc, object_address) = create_address_buffer(
            &self.device,
            allocator,
            (capacity * size_of::<ObjectData>()) as u64,
            "loom.object_data",
            vk::BufferUsageFlags::empty(),
        )?;
        // Everything fallible is done; retire the old buffer now.
        if let Some(old) = self.objects_alloc.take() {
            let _ = allocator.free(old);
        }
        // SAFETY: the device is idle and the handle is ours.
        unsafe { self.device.destroy_buffer(self.objects, None) };

        self.objects = objects;
        self.objects_alloc = Some(objects_alloc);
        self.object_address = object_address;
        self.max_objects = capacity;
        Ok(())
    }

    /// Hand the renderer this frame's grass.
    ///
    /// **Uploaded once, expanded every frame.** Placement is a pure function
    /// of position, so the blades themselves do not change; what is per-frame
    /// is the Bézier expansion and the wind bend, and both happen in the
    /// vertex shader. That is the whole reason this is a buffer of blades
    /// rather than a mesh of triangles.
    ///
    /// Past capacity the tail is dropped rather than the buffer growing: this
    /// is the ceiling before the compute placement path exists, and silently
    /// reallocating a quarter-million-element buffer mid-frame is worse than a
    /// limit somebody can see.
    ///
    /// # Errors
    /// If the buffer is gone, which means the renderer is being torn down.
    pub fn set_grass(&mut self, blades: &[GrassBlade]) -> Result<(), RenderError> {
        let capacity = self.grass_capacity();
        let drawn = &blades[..blades.len().min(capacity)];
        self.grass_count = u32::try_from(drawn.len()).unwrap_or(0);
        if drawn.is_empty() {
            return Ok(());
        }
        write_slice(
            self.grass_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("grass buffer is gone".into()))?,
            drawn,
        )
    }

    /// Hand the renderer the terrain height grid the water reads.
    ///
    /// **Not per frame.** The grid is a bake of the voxel SDF, so it changes
    /// when the terrain does and at no other time; the caller uploads it on
    /// load and on the transaction that carved something. `origin`, `spacing`
    /// and the sample count are held here and stamped into
    /// [`EnvironmentData::terrain`] at render time, so the two halves cannot be
    /// set apart and a caller replacing the environment cannot lose them.
    ///
    /// An empty slice means the scene has no terrain, and every depth query
    /// then answers "bottomless" — which is what an open ocean wants.
    ///
    /// # Errors
    /// If the buffer is gone, which means the renderer is being torn down.
    pub fn set_terrain(
        &mut self,
        heights: &[f32],
        origin: [f32; 2],
        spacing: f32,
        side: usize,
    ) -> Result<(), RenderError> {
        // A grid too big for the buffer is dropped whole rather than in part: a
        // partly-uploaded height field would draw a shoreline through the
        // middle of the scene, which is a much worse failure than no shoreline.
        // The CPU bake coarsens at `MAX_SIDE`, so this is a guard, not a path.
        if heights.is_empty() || side * side > MAX_TERRAIN_SAMPLES || heights.len() < side * side {
            self.terrain_params = [0.0, 0.0, 1.0, 0.0];
            self.terrain_heights = 0;
            return Ok(());
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.terrain_params = [origin[0], origin[1], spacing, side as f32];
        }
        self.terrain_heights = self.terrain_address;
        write_slice(
            self.terrain_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("terrain buffer is gone".into()))?,
            &heights[..side * side],
        )
    }

    /// How many blades the buffer holds.
    #[must_use]
    pub fn grass_capacity(&self) -> usize {
        self.grass_alloc
            .as_ref()
            .map_or(0, |a| a.size() as usize / size_of::<GrassBlade>())
    }

    pub fn render(
        &mut self,
        objects: &[Object],
        particles: &[ParticleInstance],
        camera: &Camera,
    ) -> Result<Vec<u8>, RenderError> {
        // Grow rather than refuse, matching the windowed path. A ceiling the
        // caller cannot see is not a useful answer to "draw my scene".
        // One slot past the objects, for the particle pass's view-projection.
        self.reserve_objects(objects.len() + 1)?;
        if objects.len() > self.max_objects {
            return Err(RenderError::Allocator(format!(
                "{} objects exceeds the {} the object buffer was sized for",
                objects.len(),
                self.max_objects
            )));
        }
        let cmd = self.command_buffer;

        #[allow(clippy::cast_precision_loss)]
        let aspect = self.width as f32 / self.height as f32;
        let view_proj = view_projection(camera, aspect);

        // Grouped by mesh so each mesh is one instanced draw. Sorting here
        // rather than asking callers to is deliberate: the agent authors
        // scenes in whatever order reads well, and draw batching is not its
        // problem.
        let mut sorted: Vec<Object> = objects.to_vec();
        sorted.sort_by_key(|o| o.mesh);
        let mut object_data = pack_objects(&sorted, view_proj, &self.unpack);
        // The reserved slot described on `Push::object_offset`: the particle
        // vertex shader reads `mvp` from here as the view-projection, because
        // the push block cannot hold a second matrix.
        let particle_slot = u32::try_from(object_data.len()).unwrap_or(0);
        object_data.push(view_projection_slot(view_proj));

        // **Sorted back to front, and that is not optional.** These blend, so
        // the result depends on the order they are drawn in: nearest-first
        // would have each particle blend under the ones behind it. Sorted on
        // the CPU because the count is thousands, not millions.
        //
        // The comparison falls back to the original index when two particles
        // are the same distance away, so the order is total and the same scene
        // sorts the same way every run.
        let mut ordered: Vec<(usize, ParticleInstance)> =
            particles.iter().copied().enumerate().collect();
        let eye = camera.eye;
        ordered.sort_by(|(ai, a), (bi, b)| {
            let d = |p: &ParticleInstance| {
                let (dx, dy, dz) = (p.position[0] - eye.x, p.position[1] - eye.y, p.position[2] - eye.z);
                dz.mul_add(dz, dx.mul_add(dx, dy * dy))
            };
            d(b).partial_cmp(&d(a)).unwrap_or(std::cmp::Ordering::Equal).then(ai.cmp(bi))
        });
        let drawn: Vec<ParticleInstance> = ordered
            .into_iter()
            .take(self.max_particles)
            .map(|(_, p)| p)
            .collect();
        let particle_count = u32::try_from(drawn.len()).unwrap_or(0);
        if !drawn.is_empty() {
            write_slice(
                self.particle_alloc
                    .as_ref()
                    .ok_or_else(|| RenderError::Allocator("particle buffer is gone".into()))?,
                &drawn,
            )?;
        }
        // **Before the upload, not after.** The camera moved into this buffer
        // when the push block ran out of room, so setting it below the write
        // would send last frame's eye — a specular highlight one frame stale,
        // which is invisible in a still and wrong in motion.
        self.environment.eye = camera.eye.extend(0.0).to_array();
        #[allow(clippy::cast_precision_loss)]
        {
            self.environment.viewport =
                [self.width as f32, self.height as f32, 0.0, 0.0];
        }
        // **Stamped here for the same reason the eye is.** `environment` is a
        // public field callers assign wholesale every frame, and the terrain
        // grid is not theirs to know about — it is uploaded once by
        // `set_terrain` and would otherwise be cleared by the next assignment,
        // which is a shoreline that vanishes on frame two.
        self.environment.terrain = self.terrain_params;
        self.environment.terrain_heights = self.terrain_heights;
        write_slice(
            self.environment_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("environment buffer missing".into()))?,
            std::slice::from_ref(&self.environment),
        )?;
        let batches = batch_by_mesh(&sorted);
        write_slice(
            self.objects_alloc
                .as_ref()
                .ok_or_else(|| RenderError::Allocator("object buffer missing".into()))?,
            &object_data,
        )?;

        // The TLAS describes where things are, so it is rebuilt whenever they
        // move. The BLAS holding the triangles is untouched, which is what
        // makes a per-frame rebuild affordable.
        if self.raytracer.is_some() {
            let pool = self.command_pool;
            let queue = self.queue;
            let mut allocator = self.allocator.take();
            let result = match (self.raytracer.as_mut(), allocator.as_mut()) {
                (Some(rt), Some(alloc)) => rt.build_instances(
                    alloc,
                    crate::raytrace::Submit { pool, queue },
                    &sorted,
                ),
                _ => Ok(()),
            };
            self.allocator = allocator;
            result?;
        }

        // Every barrier below is chosen by the graph, not written here.
        // never-do #4: no barrier lives outside it. What each pass *touches*
        // is declared; what that requires is derived.
        let mut graph = RenderGraph::new();
        let color = graph.import("loom.color_target", self.color);
        let depth = graph.import("loom.depth_target", self.depth);
        // **The multisampled pair goes through the graph like anything else.**
        // never-do #4: barriers are the graph's, and these images start
        // UNDEFINED every frame and must reach ATTACHMENT_OPTIMAL before the
        // pass reads them. Skipping this is exactly the class of mistake the
        // validation layers exist to catch, and they caught it.
        let msaa_ids = self.msaa.as_ref().map(|m| {
            (
                graph.import("loom.msaa_color", m.image),
                graph.import("loom.msaa_depth", m.depth_image),
            )
        });

        let (width, height) = (self.width, self.height);
        let (color_view, depth_view) = (self.color_view, self.depth_view);
        let msaa_views = self.msaa.as_ref().map(|m| (m.view, m.depth_view));
        let (pipeline, layout) = (self.pipeline, self.pipeline_layout);
        // `ready` is false until a TLAS exists — an empty scene traces nothing.
        let shadow_set = self
            .raytracer
            .as_ref()
            .filter(|rt| rt.ready())
            .map(crate::raytrace::Raytracer::descriptor_set);
        let material_set = self.materials.descriptor_set();
        let base_push = Push {
            vertices: self.vertex_address,
            objects: self.object_address,
            environment: self.environment_address,
            materials: self.materials.address(),
            particles: self.particle_address,
            grass: self.grass_address,
            object_offset: 0,
            inv_view_proj: view_proj.inverse().to_cols_array(),

        };
        let sky = self.sky_pipeline;
        let particle_pipeline = self.particle_pipeline;
        let grass_pipeline = self.grass_pipeline;
        let grass_count = self.grass_count;
        let water_pipeline = self.water_pipeline;
        // The whole water mesh is in the vertex shader, so "is there water"
        // is the only thing the CPU decides — and the environment already
        // carries the answer, which is why there is no `set_water`.
        let water_verts = if self.environment.water[2] > 0.0 { WATER_VERTS } else { 0 };
        // **The readback follows the last pass that wrote a pixel.** With the
        // AA pass on, the finished frame is in its target and copying the
        // colour target instead would silently read the un-anti-aliased image
        // — a bug that looks exactly like "the pass does nothing".
        let (readback, image) = (
            self.readback,
            self.aa.as_ref().map_or(self.color, |(_, image, _, _)| *image),
        );
        let index_buffer = self.indices;
        let draws: Vec<(MeshRange, u32, u32)> = batches
            .iter()
            .filter_map(|(mesh, first, count)| {
                self.ranges.get(*mesh as usize).map(|r| (*r, *first, *count))
            })
            .collect();

        // The colour target is written by the *resolve* when multisampling, so
        // it is a colour write either way — what changes is which image the
        // fragment shader rasterises into.
        let mut forward_uses = vec![(color, Access::ColorWrite), (depth, Access::DepthWrite)];
        if let Some((ms_color, ms_depth)) = msaa_ids {
            forward_uses.push((ms_color, Access::ColorWrite));
            forward_uses.push((ms_depth, Access::DepthWrite));
        }
        graph.pass(
            "forward",
            &forward_uses,
            move |d, cmd| {
                // SAFETY: the graph has already transitioned both attachments
                // into the layouts this recording requires.
                unsafe {
                    // Multisampled: rasterise into the MSAA pair and resolve into
                    // the colour target, so the readback and the golden images
                    // still see one sample per pixel.
                    match msaa_views {
                        Some((ms_color, ms_depth)) => begin_rendering(
                            d, cmd, ms_color, ms_depth, Some(color_view), width, height,
                        ),
                        None => begin_rendering(d, cmd, color_view, depth_view, None, width, height),
                    }
                    set_viewport(d, cmd, width, height);
                    draw_sky(d, cmd, sky, layout, &base_push);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    // The scene's one descriptor set: the TLAS the fragment
                    // shader traces shadow rays against. Bound once for the
                    // whole pass, never per draw (never-do #2).
                    if let Some(set) = shadow_set {
                        d.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            layout,
                            0,
                            &[set],
                            &[],
                        );
                    }
                    // Set 1: every texture in the scene, bound once for the
                    // whole pass and never per draw (never-do #2).
                    d.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        layout,
                        1,
                        &[material_set],
                        &[],
                    );
                    set_viewport(d, cmd, width, height);
                    d.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
                    for (range, first_instance, instances) in draws {
                        let push = Push {
                            object_offset: first_instance,
                            ..base_push
                        };
                        let bytes = std::slice::from_raw_parts(
                            std::ptr::from_ref(&push).cast::<u8>(),
                            size_of::<Push>(),
                        );
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytes,
                        );
                        // firstInstance stays 0: the offset is in the push
                        // block, so the shader's indexing is unambiguous.
                        d.cmd_draw_indexed(
                            cmd,
                            range.index_count(),
                            instances,
                            range.first_index(),
                            0,
                            0,
                        );
                    }

                    // **Grass before particles and after the meshes.** It is
                    // opaque and depth-written, so it belongs with the solid
                    // geometry rather than with the blended pass — drawing it
                    // after the particles would put smoke behind the blades it
                    // should be drifting in front of.
                    //
                    // 42 vertices per blade, no vertex buffer: the Bezier is
                    // expanded from SV_VertexID, which is what lets the wind
                    // bend it per frame.
                    if grass_count > 0 {
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            grass_pipeline,
                        );
                        let push = Push {
                            object_offset: particle_slot,
                            ..base_push
                        };
                        let bytes = std::slice::from_raw_parts(
                            std::ptr::from_ref(&push).cast::<u8>(),
                            size_of::<Push>(),
                        );
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytes,
                        );
                        d.cmd_draw(cmd, grass_count * 42, 1, 0, 0);
                    }

                    // **Water, with the grass and the meshes rather than with
                    // the particles.** It is opaque and depth-written — there
                    // is no refraction in W4 — so it belongs in the solid pass,
                    // and drawing it after the blended one would put smoke
                    // behind the sea it drifts over.
                    //
                    // One draw for the whole surface, no vertex buffer and no
                    // instancing: the concentric LOD rings are derived from
                    // SV_VertexID and the wave sum is evaluated per vertex.
                    if water_verts > 0 {
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            water_pipeline,
                        );
                        let push = Push {
                            object_offset: particle_slot,
                            ..base_push
                        };
                        let bytes = std::slice::from_raw_parts(
                            std::ptr::from_ref(&push).cast::<u8>(),
                            size_of::<Push>(),
                        );
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytes,
                        );
                        d.cmd_draw(cmd, water_verts, 1, 0, 0);
                    }

                    // Particles last, over finished opaque geometry, so the
                    // depth test they read is the final one. Six vertices per
                    // particle and no vertex buffer: the quad is built from
                    // SV_VertexID in the shader, which is the same
                    // no-vertex-input model the rest of this renderer uses.
                    if particle_count > 0 {
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            particle_pipeline,
                        );
                        let push = Push {
                            object_offset: particle_slot,
                            ..base_push
                        };
                        let bytes = std::slice::from_raw_parts(
                            std::ptr::from_ref(&push).cast::<u8>(),
                            size_of::<Push>(),
                        );
                        d.cmd_push_constants(
                            cmd,
                            layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytes,
                        );
                        d.cmd_draw(cmd, particle_count * 6, 1, 0, 0);
                    }
                    d.cmd_end_rendering(cmd);
                }
            },
        );

        // The anti-aliasing passes, between the forward pass and the readback.
        // Every image is the graph's (never-do #4): it moves the colour target
        // from COLOR_ATTACHMENT_OPTIMAL to SHADER_READ_ONLY_OPTIMAL, the edge
        // mask out of UNDEFINED and then to SHADER_READ_ONLY_OPTIMAL, and the
        // AA target out of UNDEFINED. Nothing here writes a barrier.
        let readback_source = match self.aa.as_ref() {
            Some((pass, aa_image, aa_view, _)) => {
                let edges_id = graph.import("loom.aa_edges", pass.edges_image());
                let aa_id = graph.import("loom.aa_target", *aa_image);
                let (aa_view, width, height) = (*aa_view, self.width, self.height);
                graph.pass(
                    "cmaa2_edges",
                    &[(color, Access::ShaderRead), (edges_id, Access::ColorWrite)],
                    move |d, cmd| {
                        // SAFETY: the graph has put both images in the layouts
                        // this recording requires, and `cmd` is recording
                        // outside any rendering block.
                        unsafe { pass.record_edges(d, cmd, width, height) };
                    },
                );
                graph.pass(
                    "cmaa2",
                    &[
                        (color, Access::ShaderRead),
                        (edges_id, Access::ShaderRead),
                        (aa_id, Access::ColorWrite),
                    ],
                    move |d, cmd| {
                        // SAFETY: as above.
                        unsafe { pass.record(d, cmd, aa_view, width, height) };
                    },
                );
                aa_id
            }
            None => color,
        };

        graph.pass("readback", &[(readback_source, Access::TransferSrc)], move |d, cmd| {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            // SAFETY: the graph put the image in TRANSFER_SRC_OPTIMAL.
            unsafe {
                d.cmd_copy_image_to_buffer(
                    cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback,
                    &[region],
                );
            }
        });

        if let Some(timers) = self.timers.as_mut() {
            graph.time(timers);
        }

        let d = &self.device;
        // SAFETY: the buffer is not in flight — the previous submit was waited on.
        unsafe {
            d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            d.begin_command_buffer(cmd, &begin)?;
        }

        self.last_transitions = graph.execute(d, cmd);

        // SAFETY: recording is complete; nothing else uses this buffer.
        unsafe {
            d.end_command_buffer(cmd)?;
            let buffers = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&buffers);
            d.reset_fences(&[self.fence])?;
            d.queue_submit(self.queue, &[submit], self.fence)?;
            // Headless and single-frame, so a full stall is correct rather than
            // lazy: nothing can proceed until the pixels exist.
            d.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }

        // After the fence, never before: the queries are only guaranteed
        // available once the work that wrote them has completed.
        if let Some(timers) = self.timers.as_mut() {
            timers.resolve(&self.device);
            if self.print_timing {
                let table = timers
                    .times()
                    .iter()
                    .map(|(name, ms)| format!("{name} {ms:.3} ms"))
                    .collect::<Vec<_>>()
                    .join("  ");
                let total: f64 = timers.times().iter().map(|(_, ms)| ms).sum();
                // **`graph` and not `total`, deliberately.** This is the sum of
                // the render graph's passes and nothing else. It is roughly 2%
                // of what a frame costs: the per-frame TLAS rebuild is a
                // separate submit outside the graph, the host-side read of the
                // readback buffer is CPU work, and the offscreen path then
                // encodes a PNG. Labelling this `total` would be the one number
                // here capable of misleading its reader, because it would get
                // quoted later as if it were the frame.
                //
                // Blades on the line because P2's exit criterion is a frame
                // time *at a plausible blade count*, and a millisecond with no
                // count beside it answers half the question.
                eprintln!(
                    "[loom gpu] {}x{}  {table}  graph {total:.3} ms  ({} blades, {} objects)",
                    self.width, self.height, self.grass_count, objects.len(),
                );
            }
        }

        let allocation = self
            .readback_alloc
            .as_ref()
            .ok_or_else(|| RenderError::Allocator("readback allocation missing".into()))?;
        let mapped = allocation
            .mapped_ptr()
            .ok_or_else(|| RenderError::Allocator("readback memory is not host-visible".into()))?;

        let len = (self.width as usize) * (self.height as usize) * 4;
        // SAFETY: the copy above wrote exactly `len` bytes, and the fence has
        // been waited on, so the write is visible to the host.
        let pixels = unsafe { std::slice::from_raw_parts(mapped.as_ptr().cast::<u8>(), len) };
        Ok(pixels.to_vec())
    }

    /// The layout transitions the graph emitted on the last [`Self::render`].
    #[must_use]
    pub fn last_transitions(&self) -> &[Transition] {
        &self.last_transitions
    }

    /// GPU milliseconds per pass on the last [`Self::render`], in pass order.
    ///
    /// Empty unless `LOOM_GPU_TIMING` was set when this renderer was built —
    /// the query pool has to exist before the first frame records into it.
    /// This is the API a caller consumes; the stderr table is a convenience
    /// for commands that have no idea timing exists.
    #[must_use]
    pub fn last_pass_times(&self) -> &[(String, f64)] {
        self.timers.as_ref().map_or(&[], GpuTimers::times)
    }

    /// Render and write a PNG.
    ///
    /// # Errors
    /// [`RenderError`] on render or IO failure.
    pub fn render_to_png(
        &mut self,
        objects: &[Object],
        particles: &[ParticleInstance],
        camera: &Camera,
        path: &std::path::Path,
    ) -> Result<(), RenderError> {
        let pixels = self.render(objects, particles, camera)?;
        let file = std::fs::File::create(path).map_err(RenderError::Io)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| {
            RenderError::Io(std::io::Error::other(e.to_string()))
        })?;
        writer.write_image_data(&pixels).map_err(|e| {
            RenderError::Io(std::io::Error::other(e.to_string()))
        })?;
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: idle first, then destroy in reverse creation order. Freeing
        // allocations before destroying their owning objects would be a
        // use-after-free the compiler cannot see.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.save_pipeline_cache();
            if let Some(timers) = self.timers.as_mut() {
                timers.destroy(&self.device);
            }
            // Before the allocator goes away: acceleration structures own both
            // Vulkan handles and allocations from it.
            if let (Some(rt), Some(allocator)) =
                (self.raytracer.as_mut(), self.allocator.as_mut())
            {
                rt.destroy(allocator);
            }
            // After the idle wait, for the same reason: these are images the
            // fragment shader was sampling one frame ago.
            if let Some(allocator) = self.allocator.as_mut() {
                self.materials.destroy(allocator);
            }
            if let (Some((buffer, allocation, _)), Some(allocator)) =
                (self.rt_positions.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
                self.device.destroy_buffer(buffer, None);
            }
            self.device.destroy_pipeline(self.particle_pipeline, None);
            if let (Some((mut pass, image, view, allocation)), Some(allocator)) =
                (self.aa.take(), self.allocator.as_mut())
            {
                pass.destroy(&self.device, allocator);
                self.device.destroy_image_view(view, None);
                self.device.destroy_image(image, None);
                let _ = allocator.free(allocation);
            }
            if let (Some(msaa), Some(allocator)) = (self.msaa.take(), self.allocator.as_mut()) {
                self.device.destroy_image_view(msaa.view, None);
                self.device.destroy_image_view(msaa.depth_view, None);
                self.device.destroy_image(msaa.image, None);
                self.device.destroy_image(msaa.depth_image, None);
                let _ = allocator.free(msaa.alloc);
                let _ = allocator.free(msaa.depth_allocation);
            }
            self.device.destroy_pipeline(self.grass_pipeline, None);
            self.device.destroy_pipeline(self.water_pipeline, None);
            self.device.destroy_buffer(self.grass_buffer, None);
            self.device.destroy_buffer(self.terrain_buffer, None);
            self.device.destroy_buffer(self.particle_buffer, None);
            if let (Some(allocation), Some(allocator)) =
                (self.grass_alloc.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
            }
            if let (Some(allocation), Some(allocator)) =
                (self.terrain_alloc.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
            }
            if let (Some(allocation), Some(allocator)) =
                (self.particle_alloc.take(), self.allocator.as_mut())
            {
                let _ = allocator.free(allocation);
            }
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline(self.sky_pipeline, None);
            self.device.destroy_pipeline_cache(self.pipeline_cache, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_image_view(self.color_view, None);
            self.device.destroy_image_view(self.depth_view, None);

            if let Some(allocator) = self.allocator.as_mut() {
                for allocation in [
                    self.color_alloc.take(),
                    self.depth_alloc.take(),
                    self.readback_alloc.take(),
                    self.vertices_alloc.take(),
                    self.indices_alloc.take(),
                    self.objects_alloc.take(),
                    self.environment_alloc.take(),
                ]
                .into_iter()
                .flatten()
                {
                    let _ = allocator.free(allocation);
                }
            }
            self.device.destroy_image(self.color, None);
            self.device.destroy_image(self.depth, None);
            self.device.destroy_buffer(self.readback, None);
            self.device.destroy_buffer(self.vertices, None);
            self.device.destroy_buffer(self.indices, None);
            self.device.destroy_buffer(self.objects, None);
            self.device.destroy_buffer(self.environment_buffer, None);
            // The allocator must go before the device it borrows.
            self.allocator = None;
        }
    }
}

impl Renderer {
    /// Write the pipeline cache back to disk (Vulkan doc §9 step 3).
    ///
    /// Best-effort: a cache that cannot be written costs a recompile next run,
    /// which is not worth failing over.
    fn save_pipeline_cache(&self) {
        let Some(path) = &self.cache_path else { return };
        // SAFETY: the cache is live and idle.
        // SAFETY: the cache is live and the device is idle.
        let Ok(data) = (unsafe { self.device.get_pipeline_cache_data(self.pipeline_cache) }) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, data);
    }
}

/// Where this device's pipeline cache lives.
///
/// Vulkan doc §9 step 4: **key the file by driver version + device ID**, plus a
/// hash of the shader set. A stale cache is silently ignored at best and a
/// correctness hazard at worst, so a driver update must not be able to load the
/// previous driver's blob.
pub(crate) fn pipeline_cache_path(instance: &Instance, device: &Device) -> Option<std::path::PathBuf> {
    // SAFETY: the physical device came from this instance.
    let props = unsafe {
        instance
            .handle()
            .get_physical_device_properties(device.physical())
    };

    // FNV-1a over the shader bytes: not cryptographic, just needs to change
    // when the shaders do.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in crate::SCENE_SPV.iter().chain(crate::TRIANGLE_SPV) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }

    let key = format!(
        "{:08x}-{:08x}-{:08x}-{:016x}",
        props.vendor_id, props.device_id, props.driver_version, hash
    );
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    Some(base.join("loom").join(format!("pipeline-{key}.bin")))
}

/// Begin dynamic rendering with a colour and depth attachment, both cleared.
///
/// # Safety
/// Both views must already be in the layouts named here — which is the graph's
/// job, not this function's.
/// Fill the frame with the sky, before anything else is drawn.
///
/// Three vertices, no buffers, depth writes off. Drawn first rather than last
/// so nothing has to know whether a pixel was covered; the cost is one
/// fullscreen fill, which at blockout scale is not worth a depth-equal trick.
///
/// # Safety
/// `cmd` must be recording inside a rendering pass whose viewport is set.
pub(crate) unsafe fn draw_sky(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    sky: vk::Pipeline,
    layout: vk::PipelineLayout,
    push: &Push,
) {
    // SAFETY: the caller guarantees an active pass; the handles are ours.
    unsafe {
        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, sky);
        d.cmd_push_constants(
            cmd,
            layout,
            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
            0,
            push.bytes(),
        );
        d.cmd_draw(cmd, 3, 1, 0, 0);
    }
}

pub(crate) unsafe fn begin_rendering(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    color_view: vk::ImageView,
    depth_view: vk::ImageView,
    // When multisampling, the single-sample image the pass resolves into.
    // `color_view` is then the multisampled target rather than the result.
    resolve_view: Option<vk::ImageView>,
    width: u32,
    height: u32,
) {
    let mut color_attachment = vk::RenderingAttachmentInfo::default()
        .image_view(color_view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        // **DONT_CARE on the multisampled image.** Its samples are consumed by
        // the resolve and never read again, so storing them would be pure
        // bandwidth — and bandwidth is the whole cost of MSAA.
        .store_op(if resolve_view.is_some() {
            vk::AttachmentStoreOp::DONT_CARE
        } else {
            vk::AttachmentStoreOp::STORE
        })
        .clear_value(vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.05, 0.06, 0.08, 1.0],
            },
        });
    let depth_attachment = vk::RenderingAttachmentInfo::default()
        .image_view(depth_view)
        .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::DONT_CARE)
        .clear_value(vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        });
    if let Some(resolve) = resolve_view {
        color_attachment = color_attachment
            // AVERAGE is the only mode guaranteed for colour, and it is what
            // "anti-aliased" means here: the pixel is the mean of its samples.
            .resolve_mode(vk::ResolveModeFlags::AVERAGE)
            .resolve_image_view(resolve)
            .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    }
    let color_attachments = [color_attachment];
    let rendering = vk::RenderingInfo::default()
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width, height },
        })
        .layer_count(1)
        .color_attachments(&color_attachments)
        .depth_attachment(&depth_attachment);
    unsafe { d.cmd_begin_rendering(cmd, &rendering) };
}

/// # Safety
/// `cmd` must be recording, with a pipeline bound that uses dynamic state.
pub(crate) unsafe fn set_viewport(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    width: u32,
    height: u32,
) {
    #[allow(clippy::cast_precision_loss)]
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: width as f32,
        height: height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D { width, height },
    };
    unsafe {
        d.cmd_set_viewport(cmd, 0, &[viewport]);
        d.cmd_set_scissor(cmd, 0, &[scissor]);
    }
}

/// How to decode one mesh's packed vertices.
///
/// Both halves quantise against the mesh's own bounds, so a prop and a terrain
/// each get the full precision of their own range rather than sharing one.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Unpack {
    /// xyz quantisation origin, w step.
    pub(crate) position: [f32; 4],
    /// xy UV origin, z step, w unused.
    pub(crate) uv: [f32; 4],
}

impl Default for Unpack {
    fn default() -> Self {
        Self {
            // Step 1 rather than 0: a zero step collapses every vertex of a
            // mesh with no entry onto its origin.
            position: [0.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0, 1.0, 0.0],
        }
    }
}

/// Per-mesh decode parameters, produced alongside the packed vertices.
pub(crate) type UnpackParams = Vec<Unpack>;

/// Concatenate a mesh library into one vertex buffer and one index buffer.
///
/// Indices are rewritten to be absolute, so a draw needs only a first-index
/// and a count — no per-mesh vertex offset to keep in sync.
pub(crate) fn combine(
    meshes: &[loom_asset::Mesh],
) -> (
    Vec<loom_asset::PackedVertex>,
    Vec<u32>,
    Vec<MeshRange>,
    UnpackParams,
) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut ranges = Vec::new();
    let mut unpack = Vec::new();

    for mesh in meshes {
        let base = u32::try_from(vertices.len()).unwrap_or(0);
        let first_index = u32::try_from(indices.len()).unwrap_or(0);

        // Packed per MESH, so each gets quantisation steps sized to its own
        // extent. Packing the whole library against one global bound would
        // give a small prop the precision of the largest terrain.
        let (packed, bounds) = loom_asset::packed::pack(&mesh.vertices);
        vertices.extend_from_slice(&packed);
        unpack.push(Unpack {
            position: [bounds.origin[0], bounds.origin[1], bounds.origin[2], bounds.step],
            uv: [bounds.uv_origin[0], bounds.uv_origin[1], bounds.uv_step, 0.0],
        });

        indices.extend(mesh.indices.iter().map(|i| i + base));
        ranges.push(MeshRange {
            first_index,
            index_count: u32::try_from(mesh.indices.len()).unwrap_or(0),
        });
    }

    // An empty library would make a zero-sized buffer, which Vulkan rejects.
    if vertices.is_empty() {
        vertices.push(loom_asset::PackedVertex::default());
        indices.push(0);
        unpack.push(Unpack::default());
    }
    (vertices, indices, ranges, unpack)
}

/// Runs of consecutive objects sharing a mesh: `(mesh, first_instance, count)`.
///
/// Assumes `objects` is already sorted by mesh, which [`Renderer::render`]
/// guarantees.
pub(crate) fn batch_by_mesh(objects: &[Object]) -> Vec<(u32, u32, u32)> {
    let mut batches: Vec<(u32, u32, u32)> = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(0);
        match batches.last_mut() {
            Some((mesh, _, count)) if *mesh == object.mesh => *count += 1,
            _ => batches.push((object.mesh, index, 1)),
        }
    }
    batches
}

/// An index buffer, reached by binding rather than by address — index fetch is
/// fixed-function and cannot go through a pointer.
pub(crate) fn create_index_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    size: u64,
    name: &str,
    extra: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, Allocation, vk::DeviceAddress), RenderError> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::INDEX_BUFFER | extra)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `info` is fully initialised and outlives the call.
    let buffer = unsafe { device.create_buffer(&info, None) }?;
    // SAFETY: `buffer` was just created on this device.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    // From here on every `?` must destroy `buffer` first. These helpers used to
    // run only during init, where a failure took the whole process down; the
    // runtime growth path calls them on exactly the low-memory condition that
    // makes `allocate` fail, and a leaked handle then trips the object-tracking
    // check at teardown — reporting a leak instead of the out-of-memory that
    // caused it.
    let on_failure = |device: &ash::Device, e: String| {
        // SAFETY: the buffer was just created here and nothing else holds it.
        unsafe { device.destroy_buffer(buffer, None) };
        RenderError::Allocator(e)
    };
    let allocation = allocator
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| on_failure(device, e.to_string()))?;
    // SAFETY: the allocation matches the buffer's requirements.
    if let Err(e) = unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
    {
        // The allocation goes back too — gpu-allocator's `Allocation` has no
        // `Drop`, so dropping it here would leak the memory as well as the
        // handle.
        let _ = allocator.free(allocation);
        return Err(on_failure(device, e.to_string()));
    }
    // Zero unless the caller asked for an address. Acceleration structure
    // builds read indices by address, and a zero there is
    // VUID-vkCmdBuildAccelerationStructuresKHR-pInfos-03806.
    let address = if extra.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) {
        let info = vk::BufferDeviceAddressInfo::default().buffer(buffer);
        // SAFETY: the buffer was created with SHADER_DEVICE_ADDRESS.
        unsafe { device.get_buffer_device_address(&info) }
    } else {
        0
    };
    Ok((buffer, allocation, address))
}

/// Build the per-object array the shader indexes with `SV_InstanceID`.
pub(crate) fn pack_objects(
    objects: &[Object],
    view_proj: Mat4,
    unpack: &UnpackParams,
) -> Vec<ObjectData> {
    objects
        .iter()
        .map(|object| {
            // Inverse-transpose, because non-uniform scale skews normals.
            let normal_matrix = glam::Mat3::from_mat4(object.model).inverse().transpose();
            // Transposing gives columns that are the original's rows.
            let rows = normal_matrix.transpose();
            ObjectData {
                mvp: (view_proj * object.model).to_cols_array(),
                model: object.model.to_cols_array(),
                unpack: unpack
                    .get(object.mesh as usize)
                    .copied()
                    .unwrap_or_default()
                    .position,
                uv_unpack: unpack
                    .get(object.mesh as usize)
                    .copied()
                    .unwrap_or_default()
                    .uv,
                normal: [
                    rows.x_axis.extend(0.0).to_array(),
                    rows.y_axis.extend(0.0).to_array(),
                    rows.z_axis.extend(0.0).to_array(),
                ],
                color: [object.color[0], object.color[1], object.color[2], 1.0],
                material: [object.material, 0, 0, 0],
            }
        })
        .collect()
}

/// The camera's view-projection, with Vulkan's Y flip applied.
pub(crate) fn view_projection(camera: &Camera, aspect: f32) -> Mat4 {
    let view = Mat4::look_at_rh(camera.eye, camera.target, Vec3::Y);
    let mut proj = Mat4::perspective_rh(camera.fov_y_degrees.to_radians(), aspect, 0.1, 1000.0);
    // Vulkan NDC has +Y down; flipping here keeps world space Y-up, which is
    // the convention pinned in the format spec.
    proj.y_axis.y *= -1.0;
    proj * view
}

/// A host-visible buffer that the shader reaches by device address.
pub(crate) fn create_address_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    size: u64,
    name: &str,
    extra: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, Allocation, vk::DeviceAddress), RenderError> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        // SHADER_DEVICE_ADDRESS is what makes `vkGetBufferDeviceAddress` legal
        // on this buffer; without it the call is a validation error.
        //
        // `extra` carries the acceleration-structure bits when ray tracing is
        // on. Passed in rather than always set: requesting
        // ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR without the
        // extension enabled is itself a validation error.
        .usage(
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
                | extra,
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `info` is fully initialised and outlives the call.
    let buffer = unsafe { device.create_buffer(&info, None) }?;
    // SAFETY: `buffer` was just created on this device.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    // From here on every `?` must destroy `buffer` first. These helpers used to
    // run only during init, where a failure took the whole process down; the
    // runtime growth path calls them on exactly the low-memory condition that
    // makes `allocate` fail, and a leaked handle then trips the object-tracking
    // check at teardown — reporting a leak instead of the out-of-memory that
    // caused it.
    let on_failure = |device: &ash::Device, e: String| {
        // SAFETY: the buffer was just created here and nothing else holds it.
        unsafe { device.destroy_buffer(buffer, None) };
        RenderError::Allocator(e)
    };
    let allocation = allocator
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| on_failure(device, e.to_string()))?;
    // SAFETY: the allocation matches the buffer's requirements.
    if let Err(e) = unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
    {
        // The allocation goes back too — gpu-allocator's `Allocation` has no
        // `Drop`, so dropping it here would leak the memory as well as the
        // handle.
        let _ = allocator.free(allocation);
        return Err(on_failure(device, e.to_string()));
    }

    let address_info = vk::BufferDeviceAddressInfo::default().buffer(buffer);
    // SAFETY: the buffer is bound and was created with SHADER_DEVICE_ADDRESS.
    let address = unsafe { device.get_buffer_device_address(&address_info) };
    Ok((buffer, allocation, address))
}

/// Copy a `#[repr(C)]` slice into a mapped allocation.
pub(crate) fn write_slice<T: Copy>(allocation: &Allocation, data: &[T]) -> Result<(), RenderError> {
    let mapped = allocation
        .mapped_ptr()
        .ok_or_else(|| RenderError::Allocator("buffer is not host-visible".into()))?;

    // **The bound check belongs here**, not in the callers. The old SAFETY note
    // below asserted "the allocation was sized for at least `data`" and nothing
    // enforced it: the offscreen path checked its object count, the windowed
    // path did not, and a scene with more than 4096 renderable nodes wrote past
    // the end of a suballocation. Because gpu-allocator hands out slices of a
    // shared host-visible block, that overflow lands on a neighbouring buffer
    // rather than segfaulting — geometry corrupts, the GPU reads nonsense
    // indices, and no validation layer says a word, because it is a plain host
    // memcpy.
    //
    // One guard at the choke point every writer already routes through.
    let bytes = std::mem::size_of_val(data) as u64;
    if bytes > allocation.size() {
        return Err(RenderError::Allocator(format!(
            "tried to write {bytes} bytes into a {}-byte allocation",
            allocation.size()
        )));
    }

    // SAFETY: the write fits, checked immediately above; `T` is `Copy` and
    // `#[repr(C)]`; the destination is uninitialised bytes we fully write.
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.as_ptr().cast::<u8>(),
            mapped.as_ptr().cast::<u8>(),
            std::mem::size_of_val(data),
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_image(
    device: &ash::Device,
    allocator: &mut Allocator,
    width: u32,
    height: u32,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    // Render targets have one; a sampled texture has a full chain, built on
    // the CPU and copied in level by level.
    mip_levels: u32,
    // Multisampling. TYPE_1 for everything but a multisampled render target,
    // which cannot be sampled or copied — only resolved.
    samples: vk::SampleCountFlags,
    name: &str,
) -> Result<(vk::Image, Allocation), RenderError> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(mip_levels.max(1))
        .array_layers(1)
        .samples(samples)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);

    // SAFETY: `info` is fully initialised and outlives the call.
    let image = unsafe { device.create_image(&info, None) }?;
    // SAFETY: `image` was just created on this device.
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let allocation = allocator
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| RenderError::Allocator(e.to_string()))?;
    // SAFETY: the allocation matches the image's requirements.
    unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }?;
    Ok((image, allocation))
}

pub(crate) fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> Result<vk::ImageView, RenderError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    // SAFETY: `info` is fully initialised and `image` is live.
    Ok(unsafe { device.create_image_view(&info, None) }?)
}

/// Build the graphics pipeline. Dynamic rendering, so no render pass object.
pub(crate) fn create_pipeline(
    device: &ash::Device,
    cache_path: Option<&std::path::Path>,
    color_format: vk::Format,
    set_layout: Option<vk::DescriptorSetLayout>,
    material_layout: vk::DescriptorSetLayout,
    samples: vk::SampleCountFlags,
) -> Result<(vk::PipelineLayout, vk::Pipeline, vk::PipelineCache), RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
        .offset(0)
        .size(u32::try_from(size_of::<Push>()).unwrap_or(128));
    let ranges = [push_range];
    // Set 0 is the TLAS the fragment shader traces shadow rays against; set 1
    // is the bindless texture array. **Set 0 is present either way** — when the
    // device has no ray query it is an empty layout rather than absent, because
    // dropping it would renumber set 1 to set 0 and silently unbind every
    // texture on exactly the machines that already have the least to spare.
    let empty = if set_layout.is_none() {
        let info = vk::DescriptorSetLayoutCreateInfo::default();
        // SAFETY: an empty layout borrows nothing.
        Some(unsafe { device.create_descriptor_set_layout(&info, None) }?)
    } else {
        None
    };
    let sets: Vec<vk::DescriptorSetLayout> = vec![
        set_layout.or(empty).unwrap_or_default(),
        material_layout,
    ];
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .push_constant_ranges(&ranges)
        .set_layouts(&sets);
    // SAFETY: `ranges` and `sets` outlive the call.
    let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }?;

    // Vulkan doc §9: create the cache up front and pass it to every pipeline
    // creation. Retrofitting means auditing every creation site.
    let previous = cache_path.and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let cache_info = vk::PipelineCacheCreateInfo::default().initial_data(&previous);
    // SAFETY: `previous` outlives the call. A corrupt or foreign blob is
    // rejected by the driver and simply ignored, which is why the path is
    // keyed by driver and device (see `pipeline_cache_path`).
    let cache = unsafe { device.create_pipeline_cache(&cache_info, None) }?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"vertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"fragmentMain"),
    ];

    // No vertex input: geometry comes from SV_VertexID in the shader.
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::BACK)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(DEPTH_FORMAT);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering_info);

    // SAFETY: every borrowed struct above outlives this call.
    let pipeline = unsafe { device.create_graphics_pipelines(cache, &[info], None) }
        .map_err(|(_, r)| RenderError::Vulkan(r))?[0];

    // SAFETY: the module is baked into the pipeline and no longer needed.
    unsafe { device.destroy_shader_module(module, None) };

    Ok((layout, pipeline, cache))
}

/// A pipeline that fills the frame with the sky, sharing the scene's layout.
///
/// Differs from the scene pipeline in exactly three ways, and each matters:
/// no depth test or write (it is the backdrop, drawn first), no culling (the
/// fullscreen triangle's winding is whatever `SV_VertexID` produces), and the
/// sky entry points.
/// The reserved `ObjectData` entry that carries a pass's view-projection.
///
/// The particle vertex shader needs the view-projection to place a billboard,
/// and the push block has no room for a second `float4x4` — 64 more bytes
/// would put it past the 128 Vulkan guarantees. The object buffer is already
/// bound, so one spare slot carries the matrix instead. Shared between the two
/// render paths so they cannot disagree about what is in it.
pub(crate) fn view_projection_slot(view_proj: Mat4) -> ObjectData {
    ObjectData {
        mvp: view_proj.to_cols_array(),
        model: Mat4::IDENTITY.to_cols_array(),
        unpack: [0.0, 0.0, 0.0, 1.0],
        uv_unpack: [0.0, 0.0, 1.0, 0.0],
        normal: [[0.0; 4]; 3],
        color: [1.0; 4],
        material: [crate::material::NO_TEXTURE, 0, 0, 0],
    }
}

/// The particle pipeline: alpha blended, depth tested, depth *not* written.
///
/// Three settings carry the whole effect and each is a bug if wrong.
///
/// - **Blending on**, source-alpha over one-minus-source-alpha. Particles are
///   translucent; that is the entire point.
/// - **Depth test on** so smoke is correctly hidden behind terrain.
/// - **Depth write off.** This is the one that is easy to miss. A particle
///   that writes depth occludes the particles behind it, so a plume renders as
///   whichever billboard happened to be drawn first and the rest vanish — the
///   volume collapses into one flat card.
///
/// Culling is off because a billboard faces the camera by construction and a
/// back-facing one is a rounding error away, not an error.
/// A pipeline for geometry that exists only in the vertex shader: opaque,
/// depth-written, both faces drawn, no vertex input at all.
///
/// **Grass and water share this**, because they want exactly the same state
/// and differ only in which entry points they name. Each reason below is a bug
/// if it is wrong for either of them:
///
/// - **Opaque, not blended.** Alpha blending forces a sort and kills early-Z,
///   and neither a geometry blade nor a wave surface has an alpha edge to
///   antialias. Water without refraction has nothing to see through it either.
/// - **No back-face culling.** A blade has no back — it is a surface one
///   triangle thick, and culling half of it makes a field flicker as blades
///   turn. A Gerstner crest can likewise present its underside to the camera.
/// - **Depth written**, so both sit with the solid geometry rather than in a
///   blended pass.
pub(crate) fn create_geometry_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
    // **The viewer and the offscreen renderer disagree about this**, which is
    // why it is a parameter. A pipeline's rasterisation sample count must
    // match the attachment it draws into, and the windowed path draws straight
    // into a single-sample swapchain image.
    samples: vk::SampleCountFlags,
    vertex_entry: &std::ffi::CStr,
    fragment_entry: &std::ffi::CStr,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(vertex_entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(fragment_entry),
    ];

    // No vertex input at all: the geometry comes from SV_VertexID and the
    // buffers the push block points at.
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(DEPTH_FORMAT);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering_info);

    // SAFETY: every borrowed slice outlives the call.
    let pipeline = unsafe { device.create_graphics_pipelines(cache, &[info], None) }
        .map_err(|(_, e)| RenderError::Vulkan(e))?[0];
    // SAFETY: the pipeline holds what it needs from the module.
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipeline)
}

pub(crate) fn create_particle_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
    // **The viewer and the offscreen renderer disagree about this**, which is
    // why it is a parameter. A pipeline's rasterisation sample count must
    // match the attachment it draws into, and the windowed path draws straight
    // into a single-sample swapchain image.
    samples: vk::SampleCountFlags,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"particleVertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"particleFragmentMain"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(true)
        // **Premultiplied alpha**, which is what lets one pipeline draw both
        // smoke and fire. The shader multiplies colour by alpha itself, so
        // `src = ONE`; a particle that then reports alpha 0 contributes its
        // colour and occludes nothing, which is exactly additive blending.
        // Fire needs to add light rather than cover what is behind it, and the
        // alternative — a second pipeline and a second sorted draw — costs a
        // pass to express something the blend equation already can.
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(DEPTH_FORMAT);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering_info);

    // SAFETY: every borrowed slice outlives the call.
    let pipeline = unsafe { device.create_graphics_pipelines(cache, &[info], None) }
        .map_err(|(_, e)| RenderError::Vulkan(e))?[0];
    // SAFETY: the module is owned by this call and the pipeline holds no
    // reference to it after creation.
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipeline)
}

pub(crate) fn create_sky_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    color_format: vk::Format,
    samples: vk::SampleCountFlags,
) -> Result<vk::Pipeline, RenderError> {
    let module = create_shader_module(device, crate::SCENE_SPV)?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(c"skyVertexMain"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(c"skyFragmentMain"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    // The depth format still has to be declared even though depth is unused:
    // the pass has a depth attachment, and a pipeline that disagrees with its
    // pass is VUID-...-08914 on every draw.
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(DEPTH_FORMAT);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering_info);

    // SAFETY: every borrowed struct above outlives this call.
    let pipeline = unsafe { device.create_graphics_pipelines(cache, &[info], None) }
        .map_err(|(_, r)| RenderError::Vulkan(r))?[0];
    // SAFETY: baked into the pipeline, no longer needed.
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipeline)
}

pub(crate) fn create_shader_module(
    device: &ash::Device,
    spv: &[u8],
) -> Result<vk::ShaderModule, RenderError> {
    // SPIR-V is a stream of 32-bit words; `build.rs` guarantees alignment of
    // the embedded bytes only to 1, so copy into an aligned Vec.
    let words: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    // SAFETY: `words` outlives the call and is a valid SPIR-V module —
    // `build.rs` ran `spirv-val` on it.
    Ok(unsafe { device.create_shader_module(&info, None) }?)
}

