//! The depth buffer, made readable by a shader.
//!
//! **One descriptor set, one sampler, set 2 of the scene layout.** The rain
//! pass wants the scene's depth per pixel and wants to *fade* against it rather
//! than be clipped by it, which a hardware depth test cannot express — so rain
//! draws with no depth attachment at all and reads this instead.
//!
//! Both render paths own one of these. The offscreen path resolves its
//! multisampled depth into the single-sample image this points at, so the
//! windowed and headless paths read exactly the same thing — the divergence
//! that has already cost this project three defects in one session.
//!
//! Nearest filtering and clamped addressing: this is a depth *value* per pixel,
//! and interpolating two of them across a silhouette produces a distance where
//! there is no surface.

use ash::vk;

use crate::renderer::RenderError;

pub(crate) struct SceneDepth {
    device: ash::Device,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
}

impl SceneDepth {
    pub(crate) fn new(device: &ash::Device) -> Result<Self, RenderError> {
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let bindings = [binding];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `bindings` outlives the call.
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }?;

        let size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1);
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

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: nothing is borrowed by the info.
        let sampler = unsafe { device.create_sampler(&sampler_info, None) }?;

        Ok(Self {
            device: device.clone(),
            layout,
            pool,
            set,
            sampler,
        })
    }

    /// Point the set at a depth view. Called at startup and again on every
    /// resize, because the depth image is recreated then and the old view is
    /// destroyed under the descriptor.
    pub(crate) fn bind(&self, view: vk::ImageView) {
        let info = vk::DescriptorImageInfo::default()
            .sampler(self.sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let infos = [info];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&infos);
        // SAFETY: `view` is live and `infos` outlives the call.
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
    }

    pub(crate) fn descriptor_layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    pub(crate) fn descriptor_set(&self) -> vk::DescriptorSet {
        self.set
    }
}

impl Drop for SceneDepth {
    fn drop(&mut self) {
        // SAFETY: the caller has already waited for the device to be idle.
        unsafe {
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}
