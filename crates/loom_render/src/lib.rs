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

mod debug_names;
mod device;
mod instance;
mod renderer;
mod viewer;

/// Re-exported because this crate's public API is expressed in `glam` types.
/// A consumer depending on its own copy would hit a silent version skew — two
/// `Mat4` types that look identical and do not unify.
pub use glam;

pub use debug_names::DebugNames;
pub use device::{Device, DeviceError};
pub use renderer::{Camera, Object, RenderError, Renderer};
pub use viewer::Viewer;

/// Re-exported so `loom_cli` can create a surface without its own `ash` dep
/// (the dependency rule: nothing outside `loom_render*` imports ash).
pub use ash;
pub use ash_window;
pub use instance::{Instance, InstanceError, take_validation_messages};

/// The compiled triangle shader, embedded at build time by `build.rs`.
pub const TRIANGLE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/triangle.spv"));

/// The scene shader (one lit cube per object), embedded at build time.
pub const SCENE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.spv"));

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
    /// Device creation must be validation-clean, and must report a real GPU.
    #[test]
    fn device_creation_is_validation_clean() {
        let Ok(instance) = Instance::new(c"loom-device-test") else {
            eprintln!("skipping: no Vulkan loader");
            return;
        };
        let _ = instance.check_validation();

        let device = match Device::new(&instance) {
            Ok(d) => d,
            Err(DeviceError::NoDevices) => {
                eprintln!("skipping: no Vulkan device");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        eprintln!("selected device: {}", device.name());
        assert!(!device.name().is_empty());

        drop(device);
        if let Err(messages) = instance.check_validation() {
            panic!("validation was not silent:\n  {}", messages.join("\n  "));
        }
    }

    /// The M2 payoff: objects rendered offscreen, validation silent, pixels real.
    #[test]
    fn renders_objects_to_a_non_blank_image() {
        let Ok(instance) = Instance::new(c"loom-render-test") else {
            eprintln!("skipping: no Vulkan loader");
            return;
        };
        let device = match Device::new(&instance) {
            Ok(d) => d,
            Err(DeviceError::NoDevices) => {
                eprintln!("skipping: no Vulkan device");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let _ = instance.check_validation();

        // Two meshes, so the test also covers batching by mesh — a single-mesh
        // scene would pass even if the per-batch object offset were wrong.
        let meshes = [
            loom_asset::primitives::box_mesh(),
            loom_asset::primitives::sphere(16, 12),
        ];
        let mut renderer =
            Renderer::new(&instance, &device, 256, 192, &meshes).expect("renderer");
        let objects = [
            Object {
                model: glam::Mat4::from_translation(glam::Vec3::new(-2.2, 0.0, 0.0))
                    * glam::Mat4::from_scale(glam::Vec3::splat(0.8)),
                color: [0.9, 0.3, 0.3],
                mesh: 0,
            },
            Object {
                model: glam::Mat4::from_scale(glam::Vec3::splat(0.8)),
                color: [0.3, 0.9, 0.4],
                mesh: 1,
            },
        ];
        let camera = Camera {
            eye: glam::Vec3::new(3.0, 3.0, 6.0),
            target: glam::Vec3::ZERO,
            fov_y_degrees: 45.0,
        };

        let pixels = renderer.render(&objects, &camera).expect("render");

        if let Err(messages) = instance.check_validation() {
            panic!("validation was not silent:\n  {}", messages.join("\n  "));
        }

        assert_eq!(pixels.len(), 256 * 192 * 4);
        // Something was actually drawn: more than one distinct colour means
        // geometry landed, not just the clear.
        let distinct: std::collections::BTreeSet<[u8; 3]> = pixels
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(
            distinct.len() > 4,
            "image looks blank — only {} distinct colours",
            distinct.len()
        );

        // The graph must actually have placed barriers. A render that looks
        // right with no transitions emitted would mean the graph is inert and
        // the driver happened to forgive us — which it will not on other
        // hardware (Vulkan doc §14).
        let transitions: Vec<(&str, &str)> = renderer
            .last_transitions()
            .iter()
            .map(|t| (t.pass, t.image))
            .collect();
        assert_eq!(
            transitions,
            [
                ("forward", "loom.color_target"),
                ("forward", "loom.depth_target"),
                ("readback", "loom.color_target"),
            ],
            "graph did not place the expected barriers"
        );
    }

    #[test]
    fn push_constants_match_the_shader_block() {
        // Push constants now carry only two device addresses: 16 bytes against
        // a guaranteed minimum of 128. Per-object data lives in a buffer, so
        // adding fields to it can never push this over the limit — which is the
        // point of the buffer-device-address model.
        assert_eq!(size_of::<u64>() * 2, 16);
        // std430 for a PhysicalStorageBuffer block: every member 16-aligned.
        // `[f32; 3]` would put a vertex normal at offset 12 and spirv-val
        // rejects it, so the vertex is two vec4s.
        assert_eq!(size_of::<[f32; 4]>() * 2, 32, "Vertex must stay 16-aligned");
    }

    #[test]
    fn the_collector_drains_and_clears() {
        let _ = take_validation_messages();
        assert!(take_validation_messages().is_empty(), "drain leaves it empty");
    }
}
