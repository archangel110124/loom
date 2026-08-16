//! What is behind the water, made readable by the water shader.
//!
//! **One descriptor set, one pair of samplers, set 3 of the scene layout.**
//! The forward pass now ends after the grass and resolves into
//! `loom.scene_opaque` and `loom.depth_opaque`; the water block declares those
//! `ShaderRead`/`DepthSample` and reads them here. That is what turns the
//! through-water term from a two-endpoint tint into the actual seabed, seen
//! through the actual thickness of water above it.
//!
//! **Set 3, not set 2, and the reason is a layout conflict rather than
//! tidiness.** Set 2 binding 0 is `SceneDepth`, pointing at
//! `loom.depth_target` — and the water block declares that same image
//! `DepthResolve`, which puts it in `DEPTH_ATTACHMENT_OPTIMAL`. Hanging a new
//! binding off set 2 would be a descriptor claiming a layout the image is not
//! in, on an image that is an attachment of the very block doing the reading.
//! Separately the rain pass already binds set 2, on scenes where these two
//! images are never touched by the graph at all.
//!
//! A fourth set costs nothing that matters: `VkPipelineLayoutCreateInfo` takes
//! push-constant ranges and set layouts independently, and this device reports
//! `maxBoundDescriptorSets = 32`.
//!
//! **Colour is LINEAR, depth is NEAREST**, and the asymmetry is the same one
//! `scene_depth.rs` explains: a depth *value* interpolated across a silhouette
//! is a distance where there is no surface, while a colour interpolated across
//! one is just a colour. Both clamp to the edge, because a refraction offset
//! that leaves the frame must find the nearest real pixel rather than wrap to
//! the far side of the screen.

use ash::vk;

use crate::renderer::RenderError;

pub(crate) struct WaterTextures {
    device: ash::Device,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    color_sampler: vk::Sampler,
    depth_sampler: vk::Sampler,
}

impl WaterTextures {
    pub(crate) fn new(device: &ash::Device) -> Result<Self, RenderError> {
        let binding = |slot: u32| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(slot)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        };
        let bindings = [binding(0), binding(1)];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `bindings` outlives the call.
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(2);
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
        // SAFETY: `layouts` outlives the call and the pool holds one set.
        let set = unsafe { device.allocate_descriptor_sets(&allocate) }?[0];

        let sampler = |filter: vk::Filter| {
            vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        };
        // SAFETY: nothing is borrowed by the infos.
        let color_sampler = unsafe { device.create_sampler(&sampler(vk::Filter::LINEAR), None) }?;
        // SAFETY: as above.
        let depth_sampler = unsafe { device.create_sampler(&sampler(vk::Filter::NEAREST), None) }?;

        Ok(Self {
            device: device.clone(),
            layout,
            pool,
            set,
            color_sampler,
            depth_sampler,
        })
    }

    /// Point the set at the opaque pair. Called at startup and again on every
    /// resize, because both images are recreated with the multisampled pair
    /// they live beside and the old views are destroyed under the descriptor.
    pub(crate) fn bind(&self, color: vk::ImageView, depth: vk::ImageView) {
        let color_info = vk::DescriptorImageInfo::default()
            .sampler(self.color_sampler)
            .image_view(color)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_info = vk::DescriptorImageInfo::default()
            .sampler(self.depth_sampler)
            .image_view(depth)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let colors = [color_info];
        let depths = [depth_info];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(0)
                .dst_array_element(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&colors),
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(1)
                .dst_array_element(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&depths),
        ];
        // SAFETY: both views are live and the infos outlive the call.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }

    pub(crate) fn descriptor_layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    pub(crate) fn descriptor_set(&self) -> vk::DescriptorSet {
        self.set
    }
}

impl Drop for WaterTextures {
    fn drop(&mut self) {
        // SAFETY: the caller has already waited for the device to be idle.
        unsafe {
            self.device.destroy_sampler(self.color_sampler, None);
            self.device.destroy_sampler(self.depth_sampler, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}
