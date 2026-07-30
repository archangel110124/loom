//! `VK_EXT_debug_utils` object naming.
//!
//! Brief §7.3: naming costs a string per resource and turns validation output
//! and Nsight captures from hex-handle soup into `image "loom.color_target"`.
//!
//! That readability is not a human convenience here — it is what makes
//! validation output usable as **agent** feedback. "Pipeline `voxel_mesh_pass`
//! read descriptor index 4096, array size 4096" is something an agent can act
//! on; a 64-bit handle is not.

use std::ffi::CString;

use ash::vk;
use ash::vk::Handle;

/// Names Vulkan objects, or does nothing when debug utils is unavailable.
///
/// `None` in release builds, where the extension is not enabled. Callers name
/// objects unconditionally and this absorbs it, so there are no `cfg` branches
/// scattered through resource creation.
pub struct DebugNames {
    device: Option<ash::ext::debug_utils::Device>,
}

impl DebugNames {
    /// Build a namer. Pass `enabled = false` to make every call a no-op.
    #[must_use]
    pub fn new(instance: &ash::Instance, device: &ash::Device, enabled: bool) -> Self {
        Self {
            device: enabled.then(|| ash::ext::debug_utils::Device::new(instance, device)),
        }
    }

    /// Attach a name to a Vulkan object.
    ///
    /// Failures are ignored on purpose: a debug label that will not attach is
    /// never worth failing a render over, and the only cost is a less readable
    /// message later.
    pub fn set<T: Handle>(&self, object: T, name: &str) {
        let Some(device) = &self.device else { return };
        let Ok(cname) = CString::new(name) else { return };

        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(object)
            .object_name(&cname);
        // SAFETY: `info` borrows `cname`, which outlives the call, and the
        // handle belongs to this device.
        unsafe {
            let _ = device.set_debug_utils_object_name(&info);
        }
    }
}
