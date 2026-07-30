//! Vulkan 1.3 via `ash`. **The only crate permitted to import `ash`**
//! (`scripts/check-deps.sh` enforces this).
//!
//! No portability or RHI abstraction — isolation, not abstraction (Vulkan doc
//! §0). A future backend would be a rewrite of this crate rather than a
//! diffusion through the codebase.
//!
//! Style note: modern Vulkan is short. The reference is Sascha Willems'
//! `HowToVulkan2026` — a lit, textured, multi-object scene in a few hundred
//! lines using dynamic rendering, descriptor indexing, buffer device address,
//! and Slang. If a feature here grows much beyond that, it is the obsolete
//! pre-1.3 style and should be reconsidered.

mod instance;

pub use instance::{Instance, InstanceError, take_validation_messages};

/// The compiled triangle shader, embedded at build time by `build.rs`.
pub const TRIANGLE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/triangle.spv"));

#[cfg(test)]
mod tests {
    use super::*;

    /// SPIR-V starts with the magic number `0x0723_0203` and is a whole number
    /// of 32-bit words. Cheap proof that `build.rs` produced a real module
    /// rather than an empty or truncated file.
    #[test]
    fn build_script_emitted_real_spirv() {
        assert!(!TRIANGLE_SPV.is_empty(), "shader did not build");
        assert_eq!(TRIANGLE_SPV.len() % 4, 0, "SPIR-V must be 32-bit words");

        let magic = u32::from_le_bytes(TRIANGLE_SPV[0..4].try_into().unwrap());
        assert_eq!(magic, 0x0723_0203, "not a SPIR-V module");
    }

    /// Creating and destroying an instance must produce **zero** validation
    /// messages. The debug callback panics on any error or warning, so this
    /// test failing means validation caught something real.
    ///
    /// Skipped when there is no Vulkan loader, so the suite still runs on a
    /// box without a driver — but never silently skipped when a loader *is*
    /// present.
    #[test]
    fn instance_creation_is_validation_clean() {
        match Instance::new(c"loom-test") {
            Ok(instance) => {
                if let Err(messages) = instance.check_validation() {
                    panic!("validation was not silent:\n  {}", messages.join("\n  "));
                }
            }
            Err(InstanceError::LoaderMissing(why)) => {
                eprintln!("skipping: no Vulkan loader ({why})");
            }
            Err(e) => panic!("{e}"),
        }
    }

    /// The collector drains and clears.
    ///
    /// **What is and is not proven here.** That the callback fires at all is
    /// already proven end-to-end: the loader's "skipping libvulkan_dzn.so"
    /// message arrives through it on every run. This covers the drain/clear
    /// contract that `check_validation` depends on.
    ///
    /// What is *not* covered synthetically is a real VALIDATION-type message
    /// reaching the collector. Three triggers were tried and all failed
    /// badly — recorded so nobody burns the time again:
    ///
    /// - null `VkPhysicalDevice` → **SIGSEGV**; the layer looks handles up
    ///   rather than checking them
    /// - out-of-range `VkFormat` → not range-checked on
    ///   `vkGetPhysicalDeviceFormatProperties`; silently ignored
    /// - `queueFamilyIndex = 9999` → **SIGABRT inside the layer's own C++**
    ///   (`std::vector::operator[]` bounds assertion) — it indexes the queue
    ///   family array before validating the index
    ///
    /// So the layer is not robust against deliberately absurd input. The
    /// VALIDATION branch gets its proof from the first genuine mistake in the
    /// device and render work, where errors are ordinary rather than absurd —
    /// and that is the normal case this mechanism exists for.
    #[test]
    fn the_collector_drains_and_clears() {
        let _ = take_validation_messages();
        assert!(take_validation_messages().is_empty(), "drain leaves it empty");
    }
}
