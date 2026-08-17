//! Materials and their textures, bound bindlessly.
//!
//! # Why the textures live in one array
//!
//! The alternative is a descriptor set per material, bound before each draw.
//! That is the model this project's never-do #2 exists to forbid: it turns one
//! instanced draw call into one draw per material, and the CPU cost of a scene
//! then scales with how varied it is rather than with how big it is.
//!
//! Instead every texture in the scene goes into a single array, bound once for
//! the whole pass, and a material names its textures by *index*. Adding a
//! material costs 64 bytes in a buffer and no API calls at all.
//!
//! # Fixed size, not `VARIABLE_DESCRIPTOR_COUNT`
//!
//! A variable-count binding exists for the case where the texture set changes
//! after the layout is built. It does not here — the scene is loaded before
//! the renderer is created, exactly like the mesh library — so the array is
//! simply sized to the scene. `PARTIALLY_BOUND` still applies, because a scene
//! with no textures at all must still produce a legal one-element array, and
//! that element is never sampled.
//!
//! # Barriers
//!
//! The two here transition a texture from `UNDEFINED` to
//! `SHADER_READ_ONLY_OPTIMAL` once, at load, on a command buffer that is
//! submitted and waited on before any frame begins. never-do #4 gives the
//! render graph ownership of *frame* barriers, and this follows the precedent
//! `raytrace.rs` already set for one-time initialisation work.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::raytrace::Submit;
use crate::renderer::{RenderError, create_image, write_slice};

/// A material index meaning "this material has no texture in that slot".
///
/// A sentinel rather than a separate bitfield because the shader has to branch
/// on it anyway, and `0` is a perfectly valid texture index.
pub const NO_TEXTURE: u32 = u32::MAX;

/// How many texture slots the bindless array declares, regardless of how many
/// a scene actually uses.
///
/// **This is fixed so that editing a material cannot invalidate a pipeline.**
/// The binding used to be `descriptor_count(textures.len().max(1))`, which made
/// the descriptor set layout a function of the scene's texture count — and two
/// descriptor set layouts are pipeline-compatible only when *identically
/// defined*. Assigning an `albedo_map` from the inspector changed the count,
/// which changed the layout, which invalidated the shared `pipeline_layout` and
/// every one of the eight pipelines built against it. Binding the new set would
/// have been a validation error rather than a wrong pixel.
///
/// With a fixed count the layout is constant for the life of the device, so
/// replacing the entire texture set is a `vkUpdateDescriptorSets` on the *same*
/// descriptor set and nothing else moves. The unused slots cost nothing: the
/// binding is `PARTIALLY_BOUND`, so a descriptor that is never written is legal
/// as long as it is never accessed, and every material without a map names
/// [`NO_TEXTURE`].
///
/// **1024 is not measured against this device's limit in code** — `Materials`
/// holds an `ash::Device`, and `maxPerStageDescriptorSampledImages` is an
/// *instance* query. It does not need to be: exceeding the limit is a
/// validation error at layout creation, which is green check 2, on the real
/// device, every run. The scenes in this repository use at most a couple of
/// dozen.
pub const TEXTURE_CAPACITY: u32 = 1024;

/// One material, as the GPU reads it. Mirrors `MaterialData` in `scene.slang`
/// exactly; the two are one memory layout described twice and a mismatch is
/// silent.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaterialData {
    /// Linear RGB base colour; `w` is porosity — how much the surface darkens
    /// when it is wet (`loom_scene::components::Material::porosity`).
    pub albedo: [f32; 4],
    /// metallic, roughness, uv_scale.x, uv_scale.y.
    pub params: [f32; 4],
    /// albedo texture index, normal texture index, flags, **layer index**.
    ///
    /// `maps[3]` is an index into this same material table, naming a second
    /// record shown where the surface is too steep for this one — a full
    /// material rather than a colour, so the layer carries its own textures,
    /// roughness, uv_scale and flags. `NO_TEXTURE` means no layer.
    ///
    /// **The sentinel is `NO_TEXTURE`, not zero, and that is load-bearing:**
    /// zero is a perfectly valid material index, so a default of zero would
    /// give every untextured surface in the project the first material in the
    /// table as its cliff face.
    pub maps: [u32; 4],
    /// `rgb` is this material's **average** linear albedo: the authored
    /// `albedo` multiplied by the mean colour of its albedo texture. `w` is
    /// unused.
    ///
    /// It exists because a ray-traced reflection has no UVs — see
    /// `tracedEnvironment` in `scene.slang` — so it cannot fetch a texel and
    /// used to shade a reflected hit with `albedo` alone. Every textured
    /// material in this project authors `albedo = [1, 1, 1]` and lets the map
    /// carry the colour, so that made every reflection a blown-out cream
    /// silhouette at five times the right brightness.
    ///
    /// Untextured, the mean is exactly `[1, 1, 1]` and this is `albedo`
    /// bit-for-bit.
    ///
    /// **`w` is the alpha-test threshold**, not unused as this said until
    /// `alpha_cutoff` landed: discard a texel below it, `0` meaning opaque.
    pub mean_albedo: [f32; 4],
    /// `x` is **opacity** — `1` opaque, and anything less makes this material
    /// blend (`loom_scene::components::Material::opacity`). `yzw` are free.
    ///
    /// **A fifth `float4` rather than another borrowed lane.** The other four
    /// are full: `albedo.w` is porosity, `params` is metallic/roughness/uv,
    /// `maps` is two texture indices plus flags plus the layer index, and
    /// `mean_albedo.w` is the alpha cutoff. This table is per *material* and
    /// uploaded once at load — not per object and not per frame — so sixteen
    /// bytes here costs a scene with fifty materials eight hundred bytes,
    /// which is the cheapest lane in the engine to widen.
    pub misc: [f32; 4],
}

/// Set in `maps[2]`: project textures down the world axes instead of reading
/// the mesh's UVs.
pub const FLAG_TRIPLANAR: u32 = 1;

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            albedo: [0.8, 0.8, 0.8, 0.4],
            params: [0.0, 0.8, 1.0, 1.0],
            maps: [NO_TEXTURE, NO_TEXTURE, 0, NO_TEXTURE],
            mean_albedo: [0.8, 0.8, 0.8, 0.0],
            misc: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// A texture resident on the GPU.
struct Resident {
    image: vk::Image,
    allocation: Option<Allocation>,
    view: vk::ImageView,
}

/// Every texture and material a scene needs.
pub(crate) struct Materials {
    device: ash::Device,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    textures: Vec<Resident>,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    address: vk::DeviceAddress,
    /// Per material: does it blend?
    ///
    /// Distinct from [`Self::cutout`], which covers both kinds of see-through
    /// because the acceleration structure cannot represent either. **The draw
    /// order cares about only one of them**: an alpha-tested surface discards
    /// and is still an ordinary opaque draw that writes depth, while a blended
    /// one must come after everything opaque, sorted, writing no depth.
    blended: Vec<bool>,
    /// Per material: does it alpha-test?
    ///
    /// Kept on the CPU because the **acceleration structure** needs it, and
    /// the acceleration structure is built where the GPU material table is
    /// already unreachable. See [`Self::is_cutout`].
    cutout: Vec<bool>,
}

impl Materials {
    /// Upload a scene's textures and materials.
    ///
    /// `submit` is used once, here, to get the images into a layout the
    /// fragment shader can sample.
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        textures: &[loom_asset::Texture],
        materials: &[MaterialData],
        submit: Submit,
    ) -> Result<Self, RenderError> {
        // An empty array is not a legal descriptor binding, and an empty
        // buffer is not a legal allocation. Both floors are one element.
        //
        // **Fixed, not `textures.len()`.** See [`TEXTURE_CAPACITY`]: sizing the
        // binding to the scene made the descriptor set layout a function of the
        // texture count, and therefore made every pipeline in the engine a
        // function of it too.
        let slots = TEXTURE_CAPACITY;
        if textures.len() > slots as usize {
            return Err(RenderError::TooManyTextures {
                found: textures.len(),
                capacity: slots as usize,
            });
        }

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(slots)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let bindings = [binding];
        // Partially bound: a scene with no textures still declares one slot,
        // and nothing ever samples it because every material names
        // `NO_TEXTURE`.
        let flags = [vk::DescriptorBindingFlags::PARTIALLY_BOUND];
        let mut flag_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .bindings(&bindings)
            .push_next(&mut flag_info);
        // SAFETY: `bindings` and `flags` outlive the call.
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(slots);
        let sizes = [size];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&sizes)
            .max_sets(1);
        // SAFETY: `sizes` outlives the call.
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }?;

        let layouts = [layout];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: `layouts` outlives the call and the pool has room for one set.
        let set = unsafe { device.allocate_descriptor_sets(&allocate) }?[0];

        // One sampler for every texture. Anisotropic because the ground plane
        // in every one of these scenes is viewed at a grazing angle, which is
        // precisely where trilinear filtering goes to mush.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .max_lod(vk::LOD_CLAMP_NONE);
        // SAFETY: no borrowed data in the info.
        let sampler = unsafe { device.create_sampler(&sampler_info, None) }?;

        let mut resident = Vec::with_capacity(textures.len());
        for texture in textures {
            resident.push(upload(device, allocator, texture, submit)?);
        }

        // Descriptor writes, one per texture. Done once here rather than per
        // frame: nothing about this set changes after load.
        let infos: Vec<vk::DescriptorImageInfo> = resident
            .iter()
            .map(|r| {
                vk::DescriptorImageInfo::default()
                    .sampler(sampler)
                    .image_view(r.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            })
            .collect();
        if !infos.is_empty() {
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .dst_array_element(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&infos);
            // SAFETY: every view in `infos` is live and `infos` outlives the call.
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }

        // A scene with no materials still needs a legal buffer to point at.
        let fallback = [MaterialData::default()];
        let records = if materials.is_empty() { &fallback[..] } else { materials };
        let (buffer, allocation, address) = crate::renderer::create_address_buffer(
            device,
            allocator,
            std::mem::size_of_val(records) as u64,
            "loom.materials",
            vk::BufferUsageFlags::empty(),
        )?;
        write_slice(&allocation, records)?;

        Ok(Self {
            device: device.clone(),
            layout,
            pool,
            set,
            sampler,
            textures: resident,
            buffer,
            allocation: Some(allocation),
            address,
            // **Either kind of see-through keeps a surface out of the
            // acceleration structure**: `mean_albedo.w` is the alpha-test
            // threshold and `misc.x` is opacity. A ray query runs no fragment
            // shader, so a cut leaf occludes as its whole quad and a window
            // casts a solid black pane.
            cutout: materials
                .iter()
                .map(|m| m.mean_albedo[3] > 0.0 || m.misc[0] < 1.0)
                .collect(),
            blended: materials.iter().map(|m| m.misc[0] < 1.0).collect(),
        })
    }

    /// Replace every texture and every material, in place.
    ///
    /// **One method rather than a fast path and a slow path**, because
    /// [`TEXTURE_CAPACITY`] made them the same operation. The descriptor set
    /// layout, the pool, the set handle and the sampler are all constant for
    /// the life of this object, so changing a colour and assigning a new
    /// `albedo_map` differ only in how much work they do — neither can
    /// invalidate a pipeline, and the caller needs no idea which it is doing.
    ///
    /// The material buffer is reallocated because the material *count* can
    /// change. Its device address is pushed to the shader every frame rather
    /// than baked into anything, so a new address needs no other update.
    ///
    /// # Safety and ordering
    ///
    /// The caller **must** have idled the device and reset any command buffer
    /// that references the old images or the old buffer. Idling alone is not
    /// enough: a recorded command buffer still holds them, and
    /// `vkDestroyImage` on something a recorded buffer references is
    /// VUID-vkDestroyImage-image-01000. [`crate::Viewer::set_materials`] is the
    /// caller that does this correctly.
    ///
    /// # Errors
    /// [`RenderError`] if the textures exceed [`TEXTURE_CAPACITY`], or if the
    /// upload or reallocation fails. **On the error paths that can fail after
    /// work has begun, `self` is left holding live handles** — the new buffer
    /// is built before the old one is destroyed, matching `set_meshes`.
    pub(crate) fn replace(
        &mut self,
        allocator: &mut Allocator,
        textures: &[loom_asset::Texture],
        materials: &[MaterialData],
        submit: Submit,
    ) -> Result<(), RenderError> {
        if textures.len() > TEXTURE_CAPACITY as usize {
            return Err(RenderError::TooManyTextures {
                found: textures.len(),
                capacity: TEXTURE_CAPACITY as usize,
            });
        }

        // Upload the new images first. A failure here must leave the old set
        // intact and drawable, so nothing is destroyed until this succeeds.
        let mut resident = Vec::with_capacity(textures.len());
        for texture in textures {
            match upload(&self.device, allocator, texture, submit) {
                Ok(image) => resident.push(image),
                Err(e) => {
                    // Unwind what this call already uploaded, or the failure
                    // leaks every image before the one that broke.
                    for mut image in resident {
                        // SAFETY: these were created in this loop, are
                        // referenced by no command buffer, and the device is
                        // idle by the caller's contract.
                        unsafe {
                            self.device.destroy_image_view(image.view, None);
                            self.device.destroy_image(image.image, None);
                        }
                        if let Some(allocation) = image.allocation.take() {
                            let _ = allocator.free(allocation);
                        }
                    }
                    return Err(e);
                }
            }
        }

        // Same for the buffer: build, then destroy.
        let fallback = [MaterialData::default()];
        let records = if materials.is_empty() { &fallback[..] } else { materials };
        let (buffer, allocation, address) = crate::renderer::create_address_buffer(
            &self.device,
            allocator,
            std::mem::size_of_val(records) as u64,
            "loom.materials",
            vk::BufferUsageFlags::empty(),
        )?;
        write_slice(&allocation, records)?;

        // Past this point nothing can fail, so tearing down the old state is
        // safe. SAFETY: the device is idle and the command buffer has been
        // reset by the caller, so nothing references these.
        unsafe {
            for texture in &self.textures {
                self.device.destroy_image_view(texture.view, None);
                self.device.destroy_image(texture.image, None);
            }
            self.device.destroy_buffer(self.buffer, None);
        }
        for texture in &mut self.textures {
            if let Some(allocation) = texture.allocation.take() {
                let _ = allocator.free(allocation);
            }
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = allocator.free(allocation);
        }

        self.textures = resident;
        self.buffer = buffer;
        self.allocation = Some(allocation);
        self.address = address;
        self.cutout = materials
            .iter()
            .map(|m| m.mean_albedo[3] > 0.0 || m.misc[0] < 1.0)
            .collect();
        self.blended = materials.iter().map(|m| m.misc[0] < 1.0).collect();

        // Rewrite the descriptor array. **The same set, not a new one** — that
        // is the whole point of the fixed capacity, and it is why no pipeline
        // has to be rebuilt. Slots past `textures.len()` keep whatever they
        // held; they are `PARTIALLY_BOUND` and unreachable, because a material
        // that lost its map names `NO_TEXTURE` and never indexes them.
        let infos: Vec<vk::DescriptorImageInfo> = self
            .textures
            .iter()
            .map(|r| {
                vk::DescriptorImageInfo::default()
                    .sampler(self.sampler)
                    .image_view(r.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            })
            .collect();
        if !infos.is_empty() {
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(0)
                .dst_array_element(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&infos);
            // SAFETY: every view in `infos` is live and `infos` outlives the call.
            unsafe { self.device.update_descriptor_sets(&[write], &[]) };
        }
        Ok(())
    }

    /// Does this material discard texels, and therefore lie about its own
    /// shape to anything that cannot run a fragment shader?
    ///
    /// **A ray query is exactly that.** It walks the acceleration structure
    /// and reports triangles; it never samples the albedo map, so an
    /// alpha-tested leaf occludes as the whole quad it was cut from. A canopy
    /// of a thousand cards then lays a thousand hard rectangles on the ground.
    ///
    /// `u32::MAX` is "no material", which is opaque.
    pub(crate) fn is_cutout(&self, material: u32) -> bool {
        self.cutout.get(material as usize).copied().unwrap_or(false)
    }

    /// Does this material blend, and therefore have to be drawn after every
    /// opaque surface, sorted, with no depth write?
    ///
    /// `u32::MAX` is "no material", which is opaque.
    pub(crate) fn is_blended(&self, material: u32) -> bool {
        self.blended.get(material as usize).copied().unwrap_or(false)
    }

    pub(crate) fn descriptor_layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    pub(crate) fn descriptor_set(&self) -> vk::DescriptorSet {
        self.set
    }

    pub(crate) fn address(&self) -> vk::DeviceAddress {
        self.address
    }

    pub(crate) fn destroy(&mut self, allocator: &mut Allocator) {
        // SAFETY: the caller has waited for the device to go idle.
        unsafe {
            for texture in &self.textures {
                self.device.destroy_image_view(texture.view, None);
                self.device.destroy_image(texture.image, None);
            }
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
            self.device.destroy_buffer(self.buffer, None);
        }
        for texture in &mut self.textures {
            if let Some(allocation) = texture.allocation.take() {
                let _ = allocator.free(allocation);
            }
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = allocator.free(allocation);
        }
    }
}

/// Create one image, fill every mip level, and leave it sampleable.
///
/// **Every level is copied from the staging buffer**, rather than uploading
/// level 0 and generating the rest with `vkCmdBlitImage`. The chain was built
/// on the CPU in linear light (`loom_asset::texture`), which blitting cannot
/// do — a blit averages whatever bytes are in the image, so an sRGB texture
/// would darken with every level. It also means no dependency on the format
/// advertising `SAMPLED_IMAGE_FILTER_LINEAR` for blits.
fn upload(
    device: &ash::Device,
    allocator: &mut Allocator,
    texture: &loom_asset::Texture,
    submit: Submit,
) -> Result<Resident, RenderError> {
    let levels = u32::try_from(texture.levels.len()).unwrap_or(1).max(1);
    // sRGB for colour, UNORM for data. Getting this backwards is the classic
    // normal-map bug: the hardware would apply the sRGB curve to vectors, and
    // every surface would tilt toward the texture's brighter channels.
    let format = match texture.space {
        loom_asset::ColorSpace::Srgb => vk::Format::R8G8B8A8_SRGB,
        loom_asset::ColorSpace::Linear => vk::Format::R8G8B8A8_UNORM,
    };

    let (image, allocation) = create_image(
        device,
        allocator,
        texture.width,
        texture.height,
        format,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
        levels,
        vk::SampleCountFlags::TYPE_1,
        &format!("loom.texture.{}", texture.name),
    )?;

    // Staging: every level, back to back, each level's offset recorded so the
    // copy regions can address them.
    let mut staged: Vec<u8> = Vec::new();
    let mut offsets = Vec::with_capacity(texture.levels.len());
    for level in &texture.levels {
        offsets.push(staged.len() as u64);
        staged.extend_from_slice(level);
    }
    let (staging, staging_alloc, _) = crate::renderer::create_address_buffer(
        device,
        allocator,
        (staged.len() as u64).max(4),
        "loom.texture_staging",
        vk::BufferUsageFlags::TRANSFER_SRC,
    )?;
    write_slice(&staging_alloc, &staged)?;

    let regions: Vec<vk::BufferImageCopy> = (0..levels)
        .map(|level| {
            vk::BufferImageCopy::default()
                .buffer_offset(offsets[level as usize])
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(level)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: (texture.width >> level).max(1),
                    height: (texture.height >> level).max(1),
                    depth: 1,
                })
        })
        .collect();

    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(levels)
        .base_array_layer(0)
        .layer_count(1);

    let result = record(device, submit, |cmd| {
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
            .src_access_mask(vk::AccessFlags2::empty())
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(range);
        let barriers = [to_dst];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: `cmd` is recording and `image` is live.
        unsafe { device.cmd_pipeline_barrier2(cmd, &dependency) };

        // SAFETY: every region addresses bytes inside `staging`, and every mip
        // level of `image` exists.
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
        }

        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        let barriers = [to_read];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: `cmd` is recording and `image` is live.
        unsafe { device.cmd_pipeline_barrier2(cmd, &dependency) };
    });

    // SAFETY: the submit above has been waited on, so nothing is reading it.
    unsafe { device.destroy_buffer(staging, None) };
    let _ = allocator.free(staging_alloc);
    result?;

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(range);
    // SAFETY: `image` is live and the range is within it.
    let view = unsafe { device.create_image_view(&view_info, None) }?;

    Ok(Resident {
        image,
        allocation: Some(allocation),
        view,
    })
}

/// Record a one-shot command buffer, submit it, and wait.
///
/// Same shape as `raytrace::Raytracer::submit_build` — initialisation work that
/// must finish before the first frame, not per-frame work the graph schedules.
pub(crate) fn record(
    device: &ash::Device,
    submit: Submit,
    record: impl FnOnce(vk::CommandBuffer),
) -> Result<(), RenderError> {
    let allocate = vk::CommandBufferAllocateInfo::default()
        .command_pool(submit.pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: the pool belongs to this device.
    let cmd = unsafe { device.allocate_command_buffers(&allocate) }?[0];

    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: `cmd` was just allocated and is not recording.
    unsafe { device.begin_command_buffer(cmd, &begin) }?;
    record(cmd);
    // SAFETY: `cmd` is recording.
    unsafe { device.end_command_buffer(cmd) }?;

    let buffers = [cmd];
    let info = vk::SubmitInfo::default().command_buffers(&buffers);
    // SAFETY: the fence and command buffer outlive the wait below.
    let result = unsafe {
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
        let submitted = device.queue_submit(submit.queue, &[info], fence);
        let waited =
            submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
        device.destroy_fence(fence, None);
        device.free_command_buffers(submit.pool, &buffers);
        waited
    };
    result.map_err(RenderError::Vulkan)
}
