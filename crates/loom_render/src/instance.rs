//! Vulkan instance, validation layers, and the debug messenger.
//!
//! Brief §7.3: **the validation layers are the real compiler here.** Rust
//! catches none of the bugs Vulkan introduces — missing barriers, wrong image
//! layouts, use-after-free of in-flight resources, out-of-bounds bindless
//! indices all compile fine.
//!
//! §7.3 says to route the callback to `panic!` rather than a log line. That is
//! **not implementable as written**: the callback is `extern "system"`, and
//! unwinding across that boundary aborts the process in Rust 2024 with
//! "panic in a function that cannot unwind" — losing the message it was trying
//! to report. Verified, not assumed, on 2026-07-30.
//!
//! The intent is kept and the mechanism changed: messages are **collected**,
//! and [`Instance::check_validation`] turns them into a failure at a safe point
//! on the Rust side. Nothing scrolls past — a message means the check fails.

use std::borrow::Cow;
use std::ffi::{CStr, c_void};
use std::sync::{Mutex, OnceLock};

use ash::{Entry, vk};

/// Validation messages seen since the last check.
///
/// Global because the callback is a bare `extern "system"` fn with only a
/// `*mut c_void` user-data slot, and threading a pointer to instance state
/// through it would mean the messenger outliving that state is a use-after-free.
/// A global is the boring, sound option.
fn collected() -> &'static Mutex<Vec<String>> {
    static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    MESSAGES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Never poison the collector: a panic elsewhere must not turn every later
/// validation check into a different, misleading failure.
fn with_collected<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = collected().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Vulkan 1.3 (Vulkan doc §1). The loader on this box is 1.4, which is fine —
/// 1.3 is what we require, 1.4 features are used only when present.
const API_VERSION: u32 = vk::API_VERSION_1_3;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// A Vulkan instance and its debug messenger, destroyed in the right order.
///
/// Vulkan doc §13: never hold a raw handle — wrap it in an RAII type
/// immediately. The `Drop` impl is the only place these are destroyed.
pub struct Instance {
    /// Kept alive: the loader owns the function pointers everything else uses.
    entry: Entry,
    pub(crate) raw: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
}

/// Why instance creation failed, in terms a caller can act on.
#[derive(Debug)]
pub enum InstanceError {
    /// No Vulkan loader — no driver, or running somewhere headless without one.
    LoaderMissing(String),
    /// The validation layer was asked for and is not installed.
    ///
    /// This is fatal by design in debug builds. Continuing without validation
    /// would make definition-of-green check #2 pass *vacuously*, and a green
    /// check that cannot fail is worse than one that fails.
    ValidationLayerMissing,
    /// The driver rejected the instance.
    Vulkan(vk::Result),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoaderMissing(e) => write!(f, "no Vulkan loader: {e}"),
            Self::ValidationLayerMissing => f.write_str(
                "VK_LAYER_KHRONOS_validation is not installed. Validation is the real \
                 compiler for this project (brief §7.3) — without it, `zero validation \
                 messages` is vacuously true. Install vulkan-validation-layers.",
            ),
            Self::Vulkan(r) => write!(f, "vkCreateInstance failed: {r:?}"),
        }
    }
}

impl std::error::Error for InstanceError {}

impl Instance {
    /// Create an instance, with validation enabled in debug builds.
    ///
    /// # Errors
    /// [`InstanceError`] if the loader is absent, the validation layer is
    /// missing in a debug build, or the driver rejects the instance.
    pub fn new(app_name: &CStr) -> Result<Self, InstanceError> {
        Self::with_extensions(app_name, &[])
    }

    /// Create an instance that can also present to a window.
    ///
    /// The surface extensions are platform-specific, so the caller supplies
    /// them (via `ash_window::enumerate_required_extensions`) rather than this
    /// crate guessing. Headless stays the default — brief §7.1's ordering is
    /// about which path is primary, and it still is.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_extensions(
        app_name: &CStr,
        extra_extensions: &[*const i8],
    ) -> Result<Self, InstanceError> {
        // SAFETY: loading the system Vulkan loader. Unsound only if the
        // installed loader is itself broken.
        let entry = unsafe { Entry::load() }
            .map_err(|e| InstanceError::LoaderMissing(e.to_string()))?;

        let want_validation = cfg!(debug_assertions);
        if want_validation && !validation_layer_available(&entry)? {
            return Err(InstanceError::ValidationLayerMissing);
        }

        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .application_version(0)
            .engine_name(c"loom")
            .engine_version(0)
            .api_version(API_VERSION);

        let mut layers: Vec<*const i8> = Vec::new();
        let mut extensions: Vec<*const i8> = Vec::new();
        if want_validation {
            layers.push(VALIDATION_LAYER.as_ptr());
            // Named objects turn validation output from hex-handle soup into
            // `buffer "voxel_chunk_staging"` — which is what makes it usable as
            // *agent* feedback rather than human-only noise (brief §7.3).
            extensions.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        extensions.extend_from_slice(extra_extensions);

        // Attaching the messenger create-info to the instance chain is what
        // catches errors *during* vkCreateInstance/vkDestroyInstance, which a
        // messenger created afterwards cannot see.
        let mut messenger_info = debug_messenger_info();
        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if want_validation {
            create_info = create_info.push_next(&mut messenger_info);
        }

        // SAFETY: all pointers above outlive this call, and the layer and
        // extension names are static.
        let raw = unsafe { entry.create_instance(&create_info, None) }
            .map_err(InstanceError::Vulkan)?;

        let debug = if want_validation {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &raw);
            // SAFETY: `raw` is a live instance created with the extension enabled.
            let messenger = unsafe { loader.create_debug_utils_messenger(&messenger_info, None) }
                .map_err(InstanceError::Vulkan)?;
            Some((loader, messenger))
        } else {
            None
        };

        Ok(Self {
            entry,
            raw,
            debug,
        })
    }

    /// The loader, for extensions that need it (surface creation).
    #[must_use]
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    /// The underlying handle, for the rest of `loom_render`.
    #[must_use]
    pub fn handle(&self) -> &ash::Instance {
        &self.raw
    }

    /// Take every validation message seen so far, clearing the buffer.
    ///
    /// This is the seam `cargo xtask validate` and every render test assert
    /// against. A non-empty result is a failure — brief §7.3's "zero validation
    /// messages", enforced where a panic legally can happen.
    ///
    /// # Errors
    /// The collected messages, if any.
    pub fn check_validation(&self) -> Result<(), Vec<String>> {
        let messages = take_validation_messages();
        if messages.is_empty() {
            Ok(())
        } else {
            Err(messages)
        }
    }
}

/// Drain collected validation messages.
///
/// Free-standing as well as on [`Instance`] because some messages only arrive
/// during `vkDestroyInstance` — a leaked child object is reported exactly then,
/// by which point the `Instance` is gone.
#[must_use]
pub fn take_validation_messages() -> Vec<String> {
    with_collected(std::mem::take)
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: destroyed exactly once, messenger before instance, and no
        // child objects outlive this — enforced by ownership, since nothing
        // else holds these handles.
        unsafe {
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.raw.destroy_instance(None);
        }
    }
}

fn validation_layer_available(entry: &Entry) -> Result<bool, InstanceError> {
    // SAFETY: the entry is loaded; this only reads loader-owned data.
    let layers =
        unsafe { entry.enumerate_instance_layer_properties() }.map_err(InstanceError::Vulkan)?;
    Ok(layers
        .iter()
        .any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER)))
}

fn debug_messenger_info<'a>() -> vk::DebugUtilsMessengerCreateInfoEXT<'a> {
    use vk::{DebugUtilsMessageSeverityFlagsEXT as Sev, DebugUtilsMessageTypeFlagsEXT as Type};
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        // No INFO: it is loader chatter by the hundred and would bury the
        // messages that matter.
        .message_severity(Sev::ERROR | Sev::WARNING)
        .message_type(Type::GENERAL | Type::VALIDATION | Type::PERFORMANCE)
        .pfn_user_callback(Some(debug_callback))
}

/// Collects validation output. **Must never panic** — see the module header.
///
/// Only `VALIDATION` and `PERFORMANCE` messages are collected as failures.
/// `GENERAL` is the loader talking about itself, and on this box it always
/// reports skipping `libvulkan_dzn.so` (Mesa's D3D12 translation ICD, which
/// cannot initialise here). Treating that as fatal would fail every single run
/// forever, which is how a team learns to ignore the check.
unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    // SAFETY: the loader guarantees `data` is valid for the duration of this call.
    let data = unsafe { &*data };

    let id = if data.p_message_id_name.is_null() {
        Cow::Borrowed("<no id>")
    } else {
        // SAFETY: non-null and NUL-terminated per the spec.
        unsafe { CStr::from_ptr(data.p_message_id_name) }.to_string_lossy()
    };
    let message = if data.p_message.is_null() {
        Cow::Borrowed("<no message>")
    } else {
        // SAFETY: as above.
        unsafe { CStr::from_ptr(data.p_message) }.to_string_lossy()
    };

    use vk::DebugUtilsMessageTypeFlagsEXT as Type;
    if kind.intersects(Type::VALIDATION | Type::PERFORMANCE) {
        with_collected(|m| m.push(format!("[{kind:?} {severity:?}] {id}: {message}")));
    } else {
        // Informational, and deliberately visible rather than swallowed.
        eprintln!("vulkan loader [{severity:?}] {id}: {message}");
    }

    vk::FALSE
}
