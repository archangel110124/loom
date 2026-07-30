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

impl Device {
    /// Select a device and create the logical device plus its queue.
    ///
    /// # Errors
    /// [`DeviceError`] if nothing meets the locked requirements.
    pub fn new(instance: &Instance) -> Result<Self, DeviceError> {
        let (physical, queue_family, name) = select_physical_device(instance)?;

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
            .descriptor_binding_partially_bound(true);

        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .push_next(&mut features13)
            .push_next(&mut features12);

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
        })
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
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut features13)
            .push_next(&mut features12);
        // SAFETY: the chain above outlives this call.
        unsafe {
            instance
                .handle()
                .get_physical_device_features2(physical, &mut features2);
        }

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
        let Some(family) = families.iter().position(|f| {
            f.queue_flags.contains(vk::QueueFlags::GRAPHICS) && f.queue_count > 0
        }) else {
            rejections.push(format!("{name}: no graphics queue family"));
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
