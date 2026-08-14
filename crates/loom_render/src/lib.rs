//! Vulkan 1.3 via `ash`. **The only crate permitted to import `ash`**
//! (`scripts/check-deps.sh` enforces this).
//!
//! No portability or RHI abstraction — isolation, not abstraction (Vulkan doc
//! §0). A future backend would be a rewrite of this crate rather than a
//! diffusion through the codebase.
//!
//! **`LOOM_GPU_TIMING=1` turns on per-pass GPU timestamps.** Every command
//! that renders offscreen — `loom render`, `loom sim`, `cargo xtask image` —
//! then prints one line per frame to stderr:
//!
//! ```text
//! [loom gpu] 1920x1080  forward 0.105 ms  readback 0.610 ms  total 0.715 ms  (45460 blades, 2 objects)
//! ```
//!
//! **`LOOM_CMAA2=0` turns off the anti-aliasing pass** (ADR 0010), which is on
//! by default. What it buys is measured rather than assumed — see `cmaa2.rs`
//! and the ADR's results table — and that variable is how the reference numbers
//! are taken.
//!
//! Off by default, because the queries are commands in the buffer and the
//! ALL_COMMANDS timestamps between passes cost some overlap. The same numbers
//! are available to a caller as [`Renderer::last_pass_times`]. The windowed
//! [`Viewer`] is **not** instrumented: it has frames in flight, which needs a
//! pool per frame and a one-frame-late resolve, and the measurement this was
//! built for is the offscreen path.
//!
//! Style note: modern Vulkan is short. The reference is Sascha Willems'
//! `HowToVulkan2026` — a lit, textured, multi-object scene in a few hundred
//! lines using dynamic rendering, descriptor indexing, buffer device address,
//! and Slang. If a feature here grows much beyond that, it is the obsolete
//! pre-1.3 style and should be reconsidered.

mod cmaa2;
mod debug_names;
mod device;
mod instance;
mod material;
mod rain;
mod raytrace;
mod renderer;
mod scene_depth;
mod ui;
mod viewer;

/// Re-exported because this crate's public API is expressed in `glam` types.
/// A consumer depending on its own copy would hit a silent version skew — two
/// `Mat4` types that look identical and do not unify.
pub use glam;

pub use debug_names::DebugNames;
pub use device::{Device, DeviceError};
pub use material::{FLAG_TRIPLANAR, MaterialData, NO_TEXTURE};
pub use renderer::{
    Camera, EnvironmentData, GrassBlade, MAX_WAVES, Object, ParticleInstance, RenderError,
    Renderer, WaterWave,
};
pub use ui::Ui;
pub use viewer::Viewer;

/// Re-exported so the CLI builds panels without its own egui dependency.
pub use egui;

/// Re-exported so `loom_cli` can create a surface without its own `ash` dep
/// (the dependency rule: nothing outside `loom_render*` imports ash).
pub use ash;
pub use ash_window;
pub use instance::{Instance, InstanceError, take_validation_messages};

/// The compiled triangle shader, embedded at build time by `build.rs`.
pub const TRIANGLE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/triangle.spv"));

/// The scene shader (one lit cube per object), embedded at build time.
pub const SCENE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.spv"));

/// The anti-aliasing pass's shape filter (ADR 0010), embedded at build time.
///
/// Only ever used when `LOOM_CMAA2` is not `0` — see `cmaa2::requested`.
pub const CMAA2_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cmaa2.spv"));

/// The anti-aliasing pass's edge detection, which runs first and writes the
/// mask the shape filter walks.
pub const CMAA2_EDGES_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cmaa2_edges.spv"));

/// The raindrop simulation (ADR 0015), embedded at build time.
///
/// Two compute entry points: `rainSimulateMain` advances every drop and
/// collides it against the baked world, `rainSplashArgsMain` writes the splash
/// draw's indirect arguments.
pub const RAIN_SIM_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rain_sim.spv"));

/// The compute shader that evaluates the generated fields, for the S2
/// agreement test. Nothing in the runtime dispatches it — see `field_agree`.
#[cfg(test)]
pub const FIELD_AGREE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/field_agree.spv"));

#[cfg(test)]
mod field_agree;

#[cfg(test)]
mod tests {
    /// **The validation collector is process-global**, and `cargo test` runs
    /// these concurrently. Without serialising, one test's `check_validation`
    /// drains messages another test caused — so a real Vulkan error could be
    /// swallowed by whichever test happened to drain first, and the suite
    /// would go green on a broken renderer.
    ///
    /// Every test that creates an `Instance` or drains the collector takes
    /// this first. Poisoning is ignored deliberately: if one test panics, the
    /// rest should still run and report honestly rather than cascade.
    pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    use super::*;

    /// **`EnvironmentData` is one memory layout described twice**, and the
    /// second description is in a shader the compiler never sees. A wrong
    /// offset here is not a build error and not a validation message — it is
    /// the water reading the wave table as a pointer, which is a hang or a
    /// blank screen with nothing to blame.
    ///
    /// The numbers are Slang's, read out of the compiled module with
    /// `spirv-dis | grep 'OpMemberDecorate %EnvironmentData_natural'`. The
    /// pointer is the one worth pinning: it sits between two `float4`-aligned
    /// blocks and is the only 8-byte member in the struct.
    #[test]
    fn the_environment_buffer_is_laid_out_as_the_shader_reads_it() {
        use renderer::EnvironmentData;
        let base = EnvironmentData::default();
        let at = |field: *const u8| {
            field as usize - std::ptr::from_ref(&base).cast::<u8>() as usize
        };

        assert_eq!(at(std::ptr::from_ref(&base.rain).cast()), 128, "rain");
        assert_eq!(at(std::ptr::from_ref(&base.water).cast()), 144, "water");
        assert_eq!(at(std::ptr::from_ref(&base.terrain).cast()), 160, "terrain");
        assert_eq!(
            at(std::ptr::from_ref(&base.terrain_heights).cast()),
            176,
            "terrainHeights — the pointer the height field is read through"
        );
        assert_eq!(
            at(std::ptr::from_ref(&base.rain_drops).cast()),
            184,
            "rainDrops — the drop buffer the streak draw reads"
        );
        assert_eq!(at(std::ptr::from_ref(&base.rain_splashes).cast()), 192, "rainSplashes");
        assert_eq!(at(std::ptr::from_ref(&base.waves).cast()), 200, "waves");
        assert_eq!(at(std::ptr::from_ref(&base.wave_count).cast()), 584, "count");
        assert_eq!(
            at(std::ptr::from_ref(&base.attenuation_depth).cast()),
            588,
            "attenuation_depth"
        );
        // **Appended after the wave table, which is why every offset above is
        // unchanged.** A `float4` needs 16-byte alignment and 592 is a multiple
        // of 16, so it lands flush with no padding.
        assert_eq!(at(std::ptr::from_ref(&base.eye_step).cast()), 592, "eyeStep");
        assert_eq!(size_of::<EnvironmentData>(), 608, "the whole struct");
    }

    /// **The water draw count is the shader's, and nothing but this says so.**
    /// `WATER_VERTS` on the Rust side and `WATER_RES`/`WATER_LEVELS` in
    /// `scene.slang` are one number described twice: too few and the outermost
    /// ring is never drawn, too many and a level is drawn on top of itself.
    /// Neither is a validation error and neither fails to compile — the
    /// symptom is a missing horizon or a doubled surface.
    ///
    /// Read out of the shader source rather than out of the compiled module
    /// because the constants fold away in SPIR-V, and reading the source is
    /// what a human would do to check.
    #[test]
    fn the_water_draw_matches_the_shader_s_grid() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/scene.slang"
        ))
        .expect("scene.slang is beside the crate that compiles it");

        let constant = |name: &str| -> u32 {
            let needle = format!("static const uint {name} = ");
            let start = source.find(&needle).unwrap_or_else(|| panic!("no {name}")) + needle.len();
            let rest = &source[start..];
            let end = rest.find(|c: char| !c.is_ascii_digit()).expect("a number");
            rest[..end].parse().expect("a number")
        };

        let res = constant("WATER_RES");
        let levels = constant("WATER_LEVELS");
        assert_eq!(res * res * levels * 6, renderer::WATER_VERTS);
        // Six vertices per quad is a triangle list, and the ring's hole is the
        // central quarter — which is only a whole number of cells if the
        // resolution divides by four.
        assert_eq!(res % 4, 0, "WATER_RES = {res} cannot have a quarter-sized hole");

        // **The snap quantum must fit inside the finest ring**, and nothing
        // else in the build says so.
        //
        // The whole mesh is snapped to the coarsest cell, `CELL·2^(LEVELS-1)`,
        // so the eye sits up to half of that from the centre. Level 0 only
        // reaches `(RES/2)·CELL`. When the first number exceeds the second the
        // camera stands *outside the finest ring* for most of its travel and
        // the water directly under it is drawn by level 1 or level 2 — the
        // sharpest possible version of "the LOD is too aggressive", and
        // invisible in a still from a camera that happens to be near a snap
        // point.
        //
        // The shipped `RES = 32, LEVELS = 7` failed this: 16 m of possible
        // offset against 8 m of level 0. It was found by measuring `river`,
        // where it turned the near surface into a mirror. `CELL` cancels, so
        // the condition is just `2^(LEVELS-1) <= RES`.
        assert!(
            1u32 << (levels - 1) <= res,
            "the {}-cell snap quantum is wider than level 0's {}-cell reach \
             (WATER_RES = {res}, WATER_LEVELS = {levels}): the camera would \
             stand outside the finest ring",
            1u32 << (levels - 1),
            res / 2,
        );
    }

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
        let _serialised = exclusive();
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
        let _serialised = exclusive();
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
        let _serialised = exclusive();
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
            Renderer::new(&instance, &device, 256, 192, &meshes, &[], &[]).expect("renderer");
        let objects = [
            Object {
                model: glam::Mat4::from_translation(glam::Vec3::new(-2.2, 0.0, 0.0))
                    * glam::Mat4::from_scale(glam::Vec3::splat(0.8)),
                color: [0.9, 0.3, 0.3],
                mesh: 0,
                material: crate::NO_TEXTURE,
            },
            Object {
                model: glam::Mat4::from_scale(glam::Vec3::splat(0.8)),
                color: [0.3, 0.9, 0.4],
                mesh: 1,
                material: crate::NO_TEXTURE,
            },
        ];
        let camera = Camera {
            eye: glam::Vec3::new(3.0, 3.0, 6.0),
            target: glam::Vec3::ZERO,
            fov_y_degrees: 45.0,
        };

        let pixels = renderer.render(&objects, &[], &camera).expect("render");

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
                // The multisampled pair, which the graph must move out of
                // UNDEFINED every frame. Rendering without these transitions
                // is a validation error, and it was — this list is how the
                // barrier ownership stays visible rather than assumed.
                ("forward", "loom.msaa_color"),
                ("forward", "loom.msaa_depth"),
                // **The depth target comes after them, not before**, because
                // multisampling makes it the *resolve destination* rather than
                // the attachment — `Access::DepthResolve`, declared in the
                // multisampled branch alongside the pair. The scene here draws
                // no rain, so nothing samples it; it is resolved anyway, for
                // the same reason the usage flags are unconditional.
                ("forward", "loom.depth_target"),
                // The anti-aliasing pass is **two** passes: edge detection
                // writes a mask image, and the shape filter reads the mask and
                // the colour target and writes a third image. The readback
                // therefore moves onto *that* one.
                //
                // Note what is missing: the shape filter reads the colour
                // target too, and there is no `("cmaa2", "loom.color_target")`
                // here, because read-after-read in the same layout needs no
                // barrier and the graph knows it.
                ("cmaa2_edges", "loom.color_target"),
                ("cmaa2_edges", "loom.aa_edges"),
                ("cmaa2", "loom.aa_edges"),
                ("cmaa2", "loom.aa_target"),
                ("readback", "loom.aa_target"),
            ],
            "graph did not place the expected barriers"
        );
    }

    /// **The anti-aliasing pass, which nothing else in the suite covers.**
    ///
    /// It is **on** by default now (ADR 0010 and `cmaa2::requested`), so this
    /// test runs the comparison the other way round: it renders once with
    /// `LOOM_CMAA2=0` and once with the default, and asserts they differ. The
    /// direction matters because a filter that silently does nothing passes
    /// every other check in the suite — the frame is still validation-clean,
    /// the graph still places its transitions, and the image still looks right.
    ///
    /// Three things are asserted, in the order they can fail: the pipeline
    /// builds and the frame is validation-clean; the render graph placed the
    /// transitions for both images and moved the readback onto the AA target;
    /// and the output actually differs from the same scene without the pass —
    /// because a filter that silently does nothing passes the first two.
    #[test]
    fn the_anti_aliasing_pass_runs_and_changes_the_image() {
        let _serialised = exclusive();
        let Ok(instance) = Instance::new(c"loom-aa-test") else {
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

        let meshes = [loom_asset::primitives::sphere(16, 12)];
        let objects = [Object {
            model: glam::Mat4::from_scale(glam::Vec3::splat(1.4)),
            color: [0.9, 0.3, 0.3],
            mesh: 0,
            material: crate::NO_TEXTURE,
        }];
        let camera = Camera {
            eye: glam::Vec3::new(3.0, 3.0, 6.0),
            target: glam::Vec3::ZERO,
            fov_y_degrees: 45.0,
        };

        // **The gate is an environment variable, so the test sets one.** This
        // is sound only because every test in this crate that touches Vulkan —
        // and therefore every other reader of the environment in this process
        // — takes `exclusive()` first, and this one holds it across the whole
        // pair.
        //
        // SAFETY: as above; no other thread in this process reads or writes
        // the environment while the guard is held.
        unsafe { std::env::set_var("LOOM_CMAA2", "0") };
        let mut plain =
            Renderer::new(&instance, &device, 128, 96, &meshes, &[], &[]).expect("renderer");
        let without = plain.render(&objects, &[], &camera).expect("render");
        drop(plain);
        // SAFETY: as above, and before anything else can observe it.
        unsafe { std::env::remove_var("LOOM_CMAA2") };

        let mut antialiased =
            Renderer::new(&instance, &device, 128, 96, &meshes, &[], &[]).expect("renderer");
        let with = antialiased.render(&objects, &[], &camera).expect("render");

        if let Err(messages) = instance.check_validation() {
            panic!("validation was not silent:\n  {}", messages.join("\n  "));
        }

        let transitions: Vec<(&str, &str)> = antialiased
            .last_transitions()
            .iter()
            .map(|t| (t.pass, t.image))
            .collect();
        assert!(
            transitions.contains(&("cmaa2_edges", "loom.color_target"))
                && transitions.contains(&("cmaa2_edges", "loom.aa_edges"))
                && transitions.contains(&("cmaa2", "loom.aa_edges"))
                && transitions.contains(&("cmaa2", "loom.aa_target")),
            "the graph did not transition all three AA images: {transitions:?}"
        );
        assert!(
            transitions.contains(&("readback", "loom.aa_target")),
            "the readback still reads the colour target, so the pass is being \
             computed and thrown away: {transitions:?}"
        );

        let changed = without
            .iter()
            .zip(&with)
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            changed > 100,
            "the pass changed only {changed} bytes — a filter that does nothing"
        );
        // ...but not *everything*: this is a conservative filter, and one that
        // touched every pixel would be a blur wearing its name.
        assert!(
            changed < without.len() / 2,
            "the pass changed {changed} of {} bytes, which is not conservative",
            without.len()
        );
    }

    #[test]
    fn push_constants_match_the_shader_block() {
        // A packed vertex is 12 bytes, not 40 — 10-10-10 position quantised
        // against the mesh bounds, an octahedral normal, and a UV quantised
        // against the mesh's UV bounds. Blocky-voxel engines pack into 4 bytes
        // by exploiting an integer lattice and six axis normals; smooth SDF
        // surfaces have neither, so this is the honest floor here.
        //
        // **The shader mirrors this as three `uint`s and indexes it as an
        // array**, so the stride has to be exactly 12 with no tail padding.
        // Nothing warns if it drifts: the vertex buffer is read through a raw
        // device address, so a mismatched stride is not an error, it is a mesh
        // that comes out sheared.
        assert_eq!(size_of::<loom_asset::PackedVertex>(), 12);
        assert_eq!(align_of::<loom_asset::PackedVertex>(), 4);

        // Push constants carry a matrix, two device addresses and an index,
        // against a guaranteed minimum of 128 bytes. Per-object data lives in
        // a buffer, so adding fields *there* can never push this over the
        // limit — which is the point of the buffer-device-address model.
        assert!(
            size_of::<crate::renderer::Push>() <= 128,
            "push block is {} bytes, over the guaranteed 128",
            size_of::<crate::renderer::Push>()
        );
    }

    #[test]
    fn the_collector_drains_and_clears() {
        let _serialised = exclusive();
        let _ = take_validation_messages();
        assert!(take_validation_messages().is_empty(), "drain leaves it empty");
    }
}
