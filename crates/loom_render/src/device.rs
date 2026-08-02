//! Physical device selection and the logical device.
//!
//! **Headless only.** No surface, no swapchain, no `winit` — brief §7.1's
//! inversion: the agent gets eyes at M2, the window is a human convenience at
//! M5. The M5 seam is one extra predicate in [`select_physical_device`]
//! ("…and can present to this surface"); nothing else here changes.
//!
//! Vulkan doc §14: capabilities are **queried and branched on**, never assumed
//! from the 4090's answers. Only one branch is ever taken on this box, and that
//! is fine — the point is that a device lacking a required feature is rejected
//! with a message naming it, rather than producing corruption somewhere later.

use ash::vk;

use crate::Instance;

/// A logical device and the queue we submit everything on.
pub struct Device {
    pub(crate) raw: ash::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) queue_family: u32,
    physical: vk::PhysicalDevice,
    name: String,
    /// Whether ray query and acceleration structures were actually enabled.
    ///
    /// Queried and branched on, never assumed (Vulkan doc §14). A device
    /// without them still renders — it just renders without ray-traced
    /// shadows, which is a worse picture rather than a dead engine.
    pub(crate) raytracing: bool,
}

/// Why no usable device could be produced.
#[derive(Debug)]
pub enum DeviceError {
    /// The loader found no Vulkan devices at all.
    NoDevices,
    /// Devices exist, but none met the requirements. Carries the per-device
    /// reasons, because "no suitable GPU" alone is not actionable.
    NoSuitableDevice(Vec<String>),
    /// The driver rejected `vkCreateDevice`.
    Vulkan(vk::Result),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevices => f.write_str("no Vulkan physical devices found"),
            Self::NoSuitableDevice(why) => {
                write!(f, "no suitable Vulkan 1.3 device:\n  {}", why.join("\n  "))
            }
            Self::Vulkan(r) => write!(f, "vkCreateDevice failed: {r:?}"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// Whether `physical` advertises everything ray-queried shadows need.
///
/// All three or none: `ray_query` is the one used in the shader,
/// `acceleration_structure` builds the structures it traverses, and
/// `deferred_host_operations` is a hard dependency of the latter that the
/// loader will refuse device creation without.
fn raytracing_supported(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    // SAFETY: `physical` came from this instance.
    let Ok(properties) = (unsafe {
        instance
            .handle()
            .enumerate_device_extension_properties(physical)
    }) else {
        return false;
    };
    let has = |wanted: &std::ffi::CStr| {
        properties
            .iter()
            .any(|p| p.extension_name_as_c_str() == Ok(wanted))
    };
    has(ash::khr::acceleration_structure::NAME)
        && has(ash::khr::ray_query::NAME)
        && has(ash::khr::deferred_host_operations::NAME)
}

impl Device {
    /// Select a device and create the logical device plus its queue.
    ///
    /// # Errors
    /// [`DeviceError`] if nothing meets the locked requirements.
    pub fn new(instance: &Instance) -> Result<Self, DeviceError> {
        Self::select(instance, None)
    }

    /// Select a device that can also present to `surface`.
    ///
    /// This is the M5 seam the headless module promised: one extra predicate in
    /// device selection, and the swapchain extension enabled. Nothing else in
    /// this file changes.
    ///
    /// # Errors
    /// [`DeviceError`] if nothing meets the requirements.
    pub fn for_surface(
        instance: &Instance,
        surface: (&ash::khr::surface::Instance, vk::SurfaceKHR),
    ) -> Result<Self, DeviceError> {
        Self::select(instance, Some(surface))
    }

    fn select(
        instance: &Instance,
        surface: Option<(&ash::khr::surface::Instance, vk::SurfaceKHR)>,
    ) -> Result<Self, DeviceError> {
        let (physical, queue_family, name) = select_physical_device(instance, surface)?;

        let priorities = [1.0_f32];
        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities);
        let queue_infos = [queue_info];

        // The locked binding model, turned on explicitly (CLAUDE.md §2):
        // dynamic rendering, descriptor indexing, buffer device address.
        let mut features13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let mut features12 = vk::PhysicalDeviceVulkan12Features::default()
            .buffer_device_address(true)
            .descriptor_indexing(true)
            .runtime_descriptor_array(true)
            .descriptor_binding_partially_bound(true)
            // Instances inside one draw can carry different materials, so the
            // texture index varies across a wave. Without this the hardware may
            // sample whichever index the first lane held, and one object wears
            // another's texture — intermittently, depending on wave packing.
            .shader_sampled_image_array_non_uniform_indexing(true);
        // Slang emits the SPIR-V `DrawParameters` capability for shaders that
        // read SV_VertexID. Without this the module is rejected with
        // VUID-VkShaderModuleCreateInfo-pCode-08740 — found by validation, not
        // by the compiler, which is exactly brief §7.3's point.
        let mut features11 =
            vk::PhysicalDeviceVulkan11Features::default().shader_draw_parameters(true);

        // Only when presenting; a headless device must not request it.
        let mut device_extensions: Vec<*const i8> = if surface.is_some() {
            vec![ash::khr::swapchain::NAME.as_ptr()]
        } else {
            Vec::new()
        };

        // Ray query, for shadows traced from the fragment shader.
        //
        // **Ray query rather than a ray tracing pipeline.** The rays here are
        // secondary — "is this point lit" — and a ray query answers that
        // inline from the shader that is already shading the pixel. The
        // pipeline variant would add a shader binding table, three new shader
        // stages and a second pipeline to reach the same answer more slowly.
        // Hybrid raster-plus-ray-query is what the extension is for.
        //
        // Enabled only where supported: `raytracing_supported` reads the
        // device's own extension list rather than trusting that a 4090 is what
        // is running.
        let raytracing = raytracing_supported(instance, physical);
        if raytracing {
            device_extensions.push(ash::khr::acceleration_structure::NAME.as_ptr());
            device_extensions.push(ash::khr::ray_query::NAME.as_ptr());
            // A dependency of acceleration_structure, not used directly: builds
            // here are all device-side.
            device_extensions.push(ash::khr::deferred_host_operations::NAME.as_ptr());
        }
        let mut accel_features =
            vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default().acceleration_structure(true);
        let mut query_features = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);

        // A Vulkan 1.0 core feature, so it lives in the base struct rather than
        // in any of the versioned ones. Anisotropic filtering is not optional
        // here: every scene in this project is a ground plane viewed at a
        // grazing angle, which is exactly where trilinear filtering blurs out.
        let base_features = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(true);

        let mut create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_features(&base_features)
            .enabled_extension_names(&device_extensions)
            .push_next(&mut features13)
            .push_next(&mut features12)
            .push_next(&mut features11);
        if raytracing {
            create_info = create_info
                .push_next(&mut accel_features)
                .push_next(&mut query_features);
        }

        // SAFETY: `physical` came from this instance; all borrows outlive the call.
        let raw = unsafe { instance.handle().create_device(physical, &create_info, None) }
            .map_err(DeviceError::Vulkan)?;
        // SAFETY: the family was just enabled with one queue.
        let queue = unsafe { raw.get_device_queue(queue_family, 0) };

        Ok(Self {
            raw,
            queue,
            queue_family,
            physical,
            name,
            raytracing,
        })
    }

    /// Whether this device can trace rays.
    ///
    /// Branch on this rather than on the GPU's name: the same binary runs on
    /// the software rasteriser in CI, which reports no ray query at all.
    #[must_use]
    pub fn supports_raytracing(&self) -> bool {
        self.raytracing
    }

    /// The device as the driver reports it, for logs and PNG metadata.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn handle(&self) -> &ash::Device {
        &self.raw
    }

    #[must_use]
    pub fn physical(&self) -> vk::PhysicalDevice {
        self.physical
    }

    #[must_use]
    pub fn queue_family(&self) -> u32 {
        self.queue_family
    }

    /// The single queue everything is submitted on.
    ///
    /// One queue for M2: the whole path is draw-offscreen then copy-to-host,
    /// and a graphics family covers both. Async compute and the dedicated
    /// transfer queue arrive with the voxel work (Vulkan doc §6), where
    /// streaming genuinely competes with rendering.
    #[must_use]
    pub fn queue(&self) -> vk::Queue {
        self.queue
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: wait for idle first — destroying a device with work in flight
        // is the use-after-free class brief §7.3 warns compiles perfectly.
        unsafe {
            let _ = self.raw.device_wait_idle();
            self.raw.destroy_device(None);
        }
    }
}

/// Pick the first device meeting every locked requirement.
///
/// Returns the reason each rejected device failed, so "no suitable GPU" is
/// never the whole story.
fn select_physical_device(
    instance: &Instance,
    surface: Option<(&ash::khr::surface::Instance, vk::SurfaceKHR)>,
) -> Result<(vk::PhysicalDevice, u32, String), DeviceError> {
    // SAFETY: enumeration takes no user handles.
    let devices = unsafe { instance.handle().enumerate_physical_devices() }
        .map_err(DeviceError::Vulkan)?;
    if devices.is_empty() {
        return Err(DeviceError::NoDevices);
    }

    let mut rejections = Vec::new();
    let mut best: Option<(vk::PhysicalDevice, u32, String, bool)> = None;

    for &physical in &devices {
        // SAFETY: `physical` came from this instance.
        let props = unsafe { instance.handle().get_physical_device_properties(physical) };
        let name = props
            .device_name_as_c_str()
            .unwrap_or(c"<unnamed>")
            .to_string_lossy()
            .into_owned();

        if props.api_version < vk::API_VERSION_1_3 {
            rejections.push(format!(
                "{name}: reports Vulkan {}.{}, need 1.3",
                vk::api_version_major(props.api_version),
                vk::api_version_minor(props.api_version),
            ));
            continue;
        }

        let mut features13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut features12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut features11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut features13)
            .push_next(&mut features12)
            .push_next(&mut features11);
        // SAFETY: the chain above outlives this call.
        unsafe {
            instance
                .handle()
                .get_physical_device_features2(physical, &mut features2);
        }

        // Copied out before the chain is borrowed again: `features2` still
        // holds mutable borrows of the pushed structs.
        let core = features2.features;

        let mut missing = Vec::new();
        if features13.dynamic_rendering == vk::FALSE {
            missing.push("dynamicRendering");
        }
        if features13.synchronization2 == vk::FALSE {
            missing.push("synchronization2");
        }
        if features12.buffer_device_address == vk::FALSE {
            missing.push("bufferDeviceAddress");
        }
        if features12.descriptor_indexing == vk::FALSE {
            missing.push("descriptorIndexing");
        }
        if features12.runtime_descriptor_array == vk::FALSE {
            missing.push("runtimeDescriptorArray");
        }
        if features11.shader_draw_parameters == vk::FALSE {
            missing.push("shaderDrawParameters");
        }
        // Both are needed by the bindless material path. Requesting either
        // without checking makes `vkCreateDevice` fail with a bare
        // `ERROR_FEATURE_NOT_PRESENT` that names nothing; checking here reports
        // which one, which is the difference between a fixable message and a
        // bisect.
        if core.sampler_anisotropy == vk::FALSE {
            missing.push("samplerAnisotropy");
        }
        if features12.shader_sampled_image_array_non_uniform_indexing == vk::FALSE {
            missing.push("shaderSampledImageArrayNonUniformIndexing");
        }
        if !missing.is_empty() {
            rejections.push(format!("{name}: missing {}", missing.join(", ")));
            continue;
        }

        // SAFETY: `physical` is valid.
        let families = unsafe {
            instance
                .handle()
                .get_physical_device_queue_family_properties(physical)
        };
        // Graphics implies transfer, so one family covers M2's whole path:
        // draw offscreen, then copy to a host-visible buffer.
        let Some(family) = families.iter().enumerate().position(|(index, f)| {
            if !f.queue_flags.contains(vk::QueueFlags::GRAPHICS) || f.queue_count == 0 {
                return false;
            }
            // The M5 predicate. Queried per family rather than assumed: on some
            // drivers not every graphics family can present.
            match surface {
                None => true,
                Some((loader, surface)) => {
                    let index = u32::try_from(index).unwrap_or(u32::MAX);
                    // SAFETY: `physical` and `surface` are both live.
                    unsafe { loader.get_physical_device_surface_support(physical, index, surface) }
                        .unwrap_or(false)
                }
            }
        }) else {
            rejections.push(format!("{name}: no graphics queue family that can present"));
            continue;
        };

        let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
        let family = u32::try_from(family).unwrap_or(0);
        // Prefer a discrete GPU, but accept anything conforming — the software
        // ICDs on this box are legitimate fallbacks, just slower.
        if discrete {
            return Ok((physical, family, name));
        }
        if best.is_none() {
            best = Some((physical, family, name, discrete));
        }
    }

    match best {
        Some((physical, family, name, _)) => Ok((physical, family, name)),
        None => Err(DeviceError::NoSuitableDevice(rejections)),
    }
}
