//! THE SEAM. Every `unsafe` in this crate is in this file, and a test in
//! `lib.rs` fails the build if one escapes.
//!
//! ── ★ ash TYPES HAVE NO `Debug` HERE — USE `.as_raw()` ───────────────────
//! `Cargo.toml` builds ash with `default-features = false`, which switches off
//! its `debug` feature along with `linked`. So `{:?}` on a `Format`,
//! `QueueFlags`, `ImageUsageFlags` or any other generated type is a COMPILE
//! ERROR, not a runtime surprise — print `x.as_raw()` as `{:#x}` instead.
//! (`vk::Result` is special-cased by ash and does implement `Debug`, which is
//! why the error arms below can use `{e:?}`.)
//!
//! Written down because it has now cost two build cycles. Turning the `debug`
//! feature on would fix the symptom and re-open the door to `linked` arriving
//! by default on a version bump, which is the thing this crate exists to
//! prevent.
//!
//! ── ★ WHY THE UNSAFE IS HERE AND NOWHERE ELSE ────────────────────────────
//! The justification for binding Vulkan directly rather than going through
//! EGL/GLES is that the C boundary is thin enough to contain. A boundary is
//! only contained if you can point at it, so: one file, one module, and a
//! grep-backed gate that says so. Everything above this file is ordinary safe
//! Rust and can be read without Vulkan knowledge.
//!
//! ── ★ WHAT M0 PROVES, AND WHY IT IS SHAPED THIS WAY ──────────────────────
//! Export a linear dmabuf out of Vulkan, import it back, read a pixel.
//!
//! The round trip is deliberate. The alternative — allocate a buffer with GBM
//! and import that — needs `libgbm`, a C library we will not link, and it
//! would make the test depend on a real DRM device. Exporting from Vulkan
//! exercises the identical machinery (`VK_KHR_external_memory_fd` +
//! `VK_EXT_external_memory_dma_buf`, a real kernel dmabuf fd, a real
//! `vkBindImageMemory` of imported memory) with nothing but the loader.
//!
//! And it runs on `llvmpipe`, which advertises the same extensions NVIDIA does
//! — measured, not assumed — so CI covers the import path on machines with no
//! GPU.
//!
//! ── ★ WHAT IT DOES NOT PROVE (M1) ────────────────────────────────────────
//! The buffer here is LINEAR and `HOST_VISIBLE`, because that is what lets the
//! test read a pixel back by mapping. A real client buffer from NVIDIA is
//! TILED and device-local: it needs `VK_EXT_image_drm_format_modifier` on the
//! import and a `vkCmdCopyImageToBuffer` to read back. Both are M1, and this
//! file says so rather than implying M0 covers them.

use crate::{Geometry, KasaneError, Unavailable};
use ash::vk;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

/// The pixel format everything here uses.
///
/// `B8G8R8A8_UNORM` matches DRM's `ARGB8888`, which is little-endian B,G,R,A in
/// memory — the same convention `nuri` blits and the bar rasterises. Choosing a
/// different one here would make the two pipes disagree about what a pixel is.
/// The one pixel format kasane handles.
///
/// `B8G8R8A8_UNORM` is what DRM calls `ARGB8888` — the format every Wayland
/// client and every scanout buffer on this fleet uses. Public so a consumer
/// can compile its pipelines against the same one rather than restating it.
pub const FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;

/// The handle type. dmabuf, always — this crate has no other reason to exist.
const HANDLE: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;

/// How many validation errors this process has seen.
///
/// ── ★ WHY A COUNTER AND NOT A PANIC ──────────────────────────────────────
/// The callback is called BY THE DRIVER across an FFI boundary. Unwinding out
/// of it is undefined behaviour, so it may only record. Tests then assert the
/// count is zero, which turns a printed warning nobody reads into a failure.
///
/// Process-global rather than per-`Gpu` because the messenger is registered on
/// the instance and tests run in parallel. A race can only make a test fail
/// that should have passed — never the reverse — which is the safe direction
/// for a gate.
static VALIDATION_ERRORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Validation errors seen so far, across every `Gpu` in this process.
///
/// Always 0 unless `KASANE_VALIDATION=1` was set and the layer was found —
/// see [`Gpu::open`]. A zero here is therefore evidence only when validation
/// was actually on, which [`validation_active`] answers.
#[must_use]
pub fn validation_errors() -> usize {
    VALIDATION_ERRORS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a validation messenger is actually installed.
///
/// ★ THE ANTI-VACUITY HALF. `validation_errors() == 0` is trivially true when
/// the layer was never loaded, so a gate that checks only the count passes on
/// every machine without the layer while claiming to prove something. A test
/// asserts THIS first.
#[must_use]
pub fn validation_active() -> bool {
    VALIDATION_INSTALLED.load(std::sync::atomic::Ordering::Relaxed)
}

static VALIDATION_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Validation errors matching a documented, MEASURED disagreement.
///
/// Counted separately and reported, never hidden — see [`EXEMPT_VUIDS`].
static VALIDATION_EXEMPT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many exempted validation errors have been seen.
///
/// ★ Reported by the gate on every run so a growing list is VISIBLE. An
/// exemption nobody sees is an exemption nobody re-examines.
#[must_use]
pub fn validation_exempt() -> usize {
    VALIDATION_EXEMPT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Validation errors this crate has MEASURED to be wrong about it.
///
/// ── ★ AN EXEMPTION IS THE DANGEROUS DIRECTION ────────────────────────────
/// Silencing a gate is how a gate stops being a gate, so each entry carries
/// its own reason, is matched on the **VUID alone** (never on message prose,
/// which would silence an entire family), and is COUNTED and printed on every
/// run. An exemption nobody sees is an exemption nobody re-examines.
///
/// ── ★ BOTH ENTRIES ARE ONE ROOT, NOT TWO ─────────────────────────────────
/// The validation layer cannot work out the format features of an image
/// tiled with `DRM_FORMAT_MODIFIER_EXT` on lavapipe, so it concludes there are
/// none — and then complains at every call site that needs one. Two VUIDs,
/// one cause; listing them separately would suggest two independent problems.
///
/// **The measurement that refutes the layer**, taken 2026-09-03:
///
///   * `vkGetImageDrmFormatModifierPropertiesEXT` on the actual image reports
///     modifier **`0x0`** — exactly the one requested, so the explicit create
///     info was honoured and the driver did not substitute another.
///   * `vkGetPhysicalDeviceFormatProperties2` reports modifier `0x0` with
///     features **`0xdd83`** — `SAMPLED_IMAGE`, `STORAGE_IMAGE`,
///     `COLOR_ATTACHMENT`, `COLOR_ATTACHMENT_BLEND`, `BLIT_SRC`, `BLIT_DST`,
///     `SAMPLED_IMAGE_FILTER_LINEAR`, `TRANSFER_SRC` and `TRANSFER_DST`. Very
///     far from "no supported format features", and it includes both of the
///     bits the two VUIDs below say are missing.
///   * The frame renders correctly, an INDEPENDENT import of the same dmabuf
///     reads back the drawn pixel, and the capture path returns those pixels.
///
/// So the image is right, its modifier is right, and its features are right.
///
/// ★ **THIS COST A REAL BUG ONCE.** Reading the two VUIDs as two causes led to
/// dropping `TRANSFER_SRC` from `ImportUse::RenderTarget`, which silenced one
/// message and broke `Target::capture` — the screenshot path `drm.rs` requires.
/// The bit was always supported; only the layer's lookup was not.
///
/// ★ **RE-MEASURE ON REAL HARDWARE.** Justified on lavapipe. The same VUIDs on
/// plo's RTX 3070 are a DIFFERENT observation and must be diagnosed, not
/// inherited. `pending-kasane: re-measure the modifier VUIDs on the 3070`
const EXEMPT_VUIDS: &[(&str, &str)] = &[
    (
        "VUID-VkImageViewCreateInfo-None-02273",
        "a view over a modifier-tiled image; the driver reports the modifier \
         with full features",
    ),
    (
        "VUID-vkCmdCopyImageToBuffer-srcImage-01998",
        "the capture copy from a modifier-tiled image; the driver reports \
         TRANSFER_SRC on that modifier",
    ),
];

/// The layer's callback. Records errors and prints everything it is given.
///
/// # Safety
/// Called by the Vulkan loader with a valid `data` pointer, per the
/// `VK_EXT_debug_utils` contract. It never unwinds and never returns TRUE
/// (which would abort the call that triggered it).
unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut std::ffi::c_void,
) -> vk::Bool32 {
    // SAFETY: the loader guarantees `data` points at a live struct for the
    // duration of the call, and `p_message` at a NUL-terminated string.
    let message = unsafe {
        data.as_ref()
            .and_then(|d| {
                if d.p_message.is_null() {
                    None
                } else {
                    std::ffi::CStr::from_ptr(d.p_message).to_str().ok()
                }
            })
            .unwrap_or("<no message>")
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        // Matched on the VUID identifier alone. Matching prose would make one
        // exemption silence every error that happens to share a phrase.
        if EXEMPT_VUIDS.iter().any(|(vuid, _)| message.contains(vuid)) {
            VALIDATION_EXEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            eprintln!("kasane validation (KNOWN, exempt): {message}");
            return vk::FALSE;
        }
        VALIDATION_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        eprintln!("kasane VALIDATION ERROR: {message}");
    } else {
        eprintln!("kasane validation: {message}");
    }
    vk::FALSE
}

/// A driver-reported extension name, as a `CStr`.
///
/// # Safety contract
/// `extension_name` is a NUL-terminated fixed array the driver filled; the
/// Vulkan spec guarantees the terminator, so this cannot run off the end.
fn ext_name(e: &vk::ExtensionProperties) -> &std::ffi::CStr {
    // SAFETY: see the note above — the terminator is spec-guaranteed.
    unsafe { std::ffi::CStr::from_ptr(e.extension_name.as_ptr()) }
}

fn driver(call: &'static str, e: vk::Result) -> KasaneError {
    KasaneError::Vulkan {
        call,
        result: format!("{e:?}"),
    }
}

/// An open Vulkan device that can import a client's dmabuf.
///
/// Owns the loader, instance and device, and destroys them in order on drop.
pub struct Gpu {
    // Held because dropping the loader unloads the library the instance and
    // device still point into. Never read directly; its lifetime IS its job.
    _entry: ash::Entry,
    pub(crate) instance: ash::Instance,
    pub(crate) physical: vk::PhysicalDevice,
    device: ash::Device,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    ext_fd: ash::khr::external_memory_fd::Device,
    /// `cmd_begin_rendering` / `cmd_end_rendering`.
    ///
    /// ★ Loaded from the KHR extension rather than core: the device is opened
    /// at API 1.2, where the core 1.3 entry points do not exist even though
    /// the extension does. Calling the core ones on a 1.2 device is a null
    /// function pointer, which crashes rather than erroring.
    dyn_render: ash::khr::dynamic_rendering::Device,
    /// The validation messenger, when `KASANE_VALIDATION=1` found the layer.
    /// Held so `Drop` can destroy it BEFORE the instance it belongs to.
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    /// What the driver calls itself. For reports, so "which GPU answered" is
    /// never a guess.
    pub device_name: String,
    /// True when this is a software rasteriser. Not a defect — it is how CI
    /// exercises the path — but a seat must be able to tell the difference.
    pub is_cpu: bool,
    /// The graphics queue work is submitted on.
    pub(crate) queue: vk::Queue,
    /// Its family index — needed for command pools and for the foreign-queue
    /// ownership transfers an imported dmabuf requires.
    ///
    /// ★ Not read yet: the `VK_EXT_queue_family_foreign` acquire/release pair
    /// is M5's, and it needs exactly this. Recorded at open time because that
    /// is where the choice is made, and re-deriving it later would risk
    /// disagreeing with the family the pool was created against.
    #[allow(dead_code, reason = "M5's foreign-queue transfer needs it; see above")]
    pub(crate) queue_family: u32,
    /// One command pool, reset per frame rather than freed per buffer.
    pub(crate) command_pool: vk::CommandPool,
    /// Whether `VK_EXT_physical_device_drm` was available and enabled.
    ///
    /// ★ Recorded rather than assumed: querying `PhysicalDeviceDrmPropertiesEXT`
    /// on a device that never enabled the extension leaves the struct as the
    /// zeroes it was initialised with, and `has_primary=0 major=0 minor=0` is
    /// indistinguishable from a real answer of "no primary node". Carrying the
    /// flag is what lets `drm_nodes` say NOT ASKED instead of guessing.
    pub has_drm: bool,
}

impl Gpu {
    /// Open the first device that can import a dmabuf.
    ///
    /// # Errors
    /// Every failure is an [`Unavailable`] arm, and every arm is a legitimate
    /// state rather than a bug: no loader, no devices, none with the
    /// extensions. The caller falls back to the CPU pipe.
    pub fn open() -> Result<Self, Unavailable> {
        // SAFETY: `Entry::load` dlopens the Vulkan loader. It is unsafe because
        // a hostile `libvulkan.so.1` on the search path could do anything; that
        // is the same trust we already extend to every other shared library the
        // process loads. Failure is a `Result`, not a panic — which is the
        // whole reason `loaded` is used instead of `linked`.
        let entry = unsafe { ash::Entry::load() }.map_err(|_| Unavailable::NoLoader)?;

        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_2);

        // ★★ VALIDATION IS OPT-IN, AND THE REASON IS A MEASUREMENT.
        //
        // Two red runs proved that a wrong push-constant range and a wrong
        // descriptor type BOTH compile clean and draw wrong — lavapipe does
        // not enforce the declarations, so nothing in this crate could see
        // them. Under `VK_LAYER_KHRONOS_validation` the first is named
        // exactly: VUID-VkGraphicsPipelineCreateInfo-layout-07987.
        //
        // So this is not developer comfort; it is the only mechanism that
        // catches an entire CLASS — wrong layouts, missing barriers, sync
        // hazards, objects destroyed while in use — none of which a driver is
        // obliged to report. It is off by default because the layer costs real
        // time per call and a compositor runs at 360 Hz.
        //
        // `KASANE_VALIDATION=1` plus `VK_LAYER_PATH` pointing at the layer.
        let want_validation = std::env::var_os("KASANE_VALIDATION").is_some_and(|v| v == "1");
        let layer_name = c"VK_LAYER_KHRONOS_validation";
        // SAFETY: enumerating layers takes no arguments that could dangle.
        let have_layer = want_validation
            && unsafe { entry.enumerate_instance_layer_properties() }.is_ok_and(|ls| {
                ls.iter().any(|l| {
                    // SAFETY: `layer_name` is a NUL-terminated fixed array
                    // the loader filled; the spec guarantees the terminator.
                    let n = unsafe { std::ffi::CStr::from_ptr(l.layer_name.as_ptr()) };
                    n == layer_name
                })
            });

        let layers: Vec<*const std::ffi::c_char> = if have_layer {
            vec![layer_name.as_ptr()]
        } else {
            Vec::new()
        };
        // `VK_EXT_debug_utils` is provided BY the validation layer, so it is
        // requested only alongside it — asking for it without the layer fails
        // instance creation on a plain driver.
        let inst_exts: Vec<*const std::ffi::c_char> = if have_layer {
            vec![ash::ext::debug_utils::NAME.as_ptr()]
        } else {
            Vec::new()
        };
        if want_validation && !have_layer {
            // ★ SAID OUT LOUD. A silent downgrade here would leave a run
            // reporting zero validation errors because validation never
            // loaded — the vacuous green this whole mechanism exists to
            // avoid. `validation_active()` is the programmatic form.
            eprintln!(
                "kasane: KASANE_VALIDATION=1 but VK_LAYER_KHRONOS_validation was \
                 not found — set VK_LAYER_PATH. Continuing WITHOUT validation."
            );
        }

        let ci = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&inst_exts);
        // SAFETY: `ci` outlives the call, and `app` outlives `ci` — both are
        // locals in this frame. No allocator callbacks are supplied.
        let instance = unsafe { entry.create_instance(&ci, None) }
            .map_err(|e| Unavailable::Driver(format!("create_instance: {e:?}")))?;

        // The messenger has to outlive the instance's use, so it is held on
        // `Gpu` and destroyed in `Drop` before the instance.
        let debug = if have_layer {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback));
            // SAFETY: `info` is a local; the callback is a `'static` fn.
            match unsafe { loader.create_debug_utils_messenger(&info, None) } {
                Ok(m) => {
                    VALIDATION_INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);
                    Some((loader, m))
                }
                Err(e) => {
                    eprintln!("kasane: could not install the validation messenger: {e:?}");
                    None
                }
            }
        } else {
            None
        };

        match Self::pick(&entry, &instance, debug) {
            Ok(gpu) => Ok(gpu),
            Err(e) => {
                // SAFETY: the instance was created above, nothing else holds it,
                // and no child objects were created on the failing paths.
                unsafe { instance.destroy_instance(None) };
                Err(e)
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear device-selection sequence whose ORDER is load-bearing: \
                  enumerate, filter by extension, choose a graphics queue, open. \
                  Splitting it would hide that order behind call sites."
    )]
    fn pick(
        entry: &ash::Entry,
        instance: &ash::Instance,
        // Created in `open` alongside the instance it belongs to, and moved
        // here so the `Gpu` that owns its destruction also owns the instance.
        debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    ) -> Result<Self, Unavailable> {
        // SAFETY: `instance` is live for the whole function.
        let physicals = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| Unavailable::Driver(format!("enumerate_physical_devices: {e:?}")))?;
        if physicals.is_empty() {
            return Err(Unavailable::NoPhysicalDevice);
        }

        let mut examined = 0usize;
        let mut chosen: Option<(vk::PhysicalDevice, u32)> = None;
        for pd in physicals {
            examined += 1;
            // SAFETY: `pd` came from this instance and is live.
            let Ok(exts) = (unsafe { instance.enumerate_device_extension_properties(pd) }) else {
                continue;
            };
            let has = |want: &std::ffi::CStr| {
                exts.iter().any(|e| {
                    // SAFETY: `extension_name` is a NUL-terminated fixed array
                    // the driver filled; `from_ptr` reads to the first NUL,
                    // which the spec guarantees is present.
                    let name = unsafe { std::ffi::CStr::from_ptr(e.extension_name.as_ptr()) };
                    name == want
                })
            };
            if !has(ash::khr::external_memory_fd::NAME)
                || !has(ash::ext::external_memory_dma_buf::NAME)
                || !has(ash::ext::image_drm_format_modifier::NAME)
            {
                continue;
            }
            // SAFETY: same — `pd` is a live handle from this instance.
            let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
            // ── ★ A GRAPHICS FAMILY, NOT FAMILY ZERO ─────────────────────
            //
            // This was `Some((pd, 0))` with a comment saying any family would
            // do because M0 never submits work. From stage 1 it does, and
            // index 0 is right BY LUCK: plo's RTX 3070 exposes six families
            // and only some carry GRAPHICS. On a device that orders them
            // differently this is a validation error at submit time, which is
            // the worst place to find it — long after the device looked fine.
            //
            // TRANSFER comes with it: the spec guarantees any family with
            // GRAPHICS or COMPUTE also supports transfer operations, so
            // asking for GRAPHICS asks for both.
            let Some(gfx) = families
                .iter()
                .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            else {
                // A device that can import a dmabuf but cannot draw is not a
                // compositor renderer. Keep looking rather than choose it.
                continue;
            };
            chosen = Some((pd, u32::try_from(gfx).unwrap_or(0)));
            // ★ PREFER A REAL GPU, but do not require one: llvmpipe is how
            // this path is covered in CI, and refusing it would make the test
            // unrunnable on exactly the machines that run tests.
            // SAFETY: live handle.
            let props = unsafe { instance.get_physical_device_properties(pd) };
            if props.device_type != vk::PhysicalDeviceType::CPU {
                break;
            }
        }

        let Some((physical, queue_family)) = chosen else {
            return Err(Unavailable::NoDeviceWithDmabuf { examined });
        };

        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        // ★ OPTIONAL, NOT REQUIRED — and the asymmetry that made this a bug is
        // worth stating. The FILTER above tests three extensions; this list
        // asked for four. A device with the three but not
        // `physical_device_drm` therefore PASSED the filter, was chosen, and
        // died here with ERROR_EXTENSION_NOT_PRESENT — bypassing the typed
        // `NoDeviceWithDmabuf { examined }` refusal and surfacing as an opaque
        // `Driver(String)`. lavapipe is exactly that device, which is why
        // kasane's tests skipped on the build machine and the GPU path went
        // untested in CI.
        //
        // Adding it to the filter would have been the wrong fix: it makes
        // lavapipe an HONEST refusal and leaves the import path with no
        // coverage anywhere a GPU is absent. Requesting it only when present,
        // and reporting no DRM node when it is not, keeps the software
        // rasteriser usable as a test device while M4 stays exact on hardware.
        let mut want = vec![
            ash::khr::external_memory_fd::NAME.as_ptr(),
            ash::ext::external_memory_dma_buf::NAME.as_ptr(),
            // ★ M1. A real client buffer from a GPU is TILED, and its layout
            // is named by a DRM format modifier. Without this extension the
            // only importable layout is linear — the CPU-readback path this
            // crate exists to remove.
            ash::ext::image_drm_format_modifier::NAME.as_ptr(),
        ];
        // ★ M4, requested only if the device has it. `has_drm` is carried on
        // the Gpu so `drm_nodes()` answers "not asked" rather than querying a
        // properties struct the driver never filled — an unasked query returns
        // zeroes, and zeroes are a valid-looking DRM node of 0:0.
        // Enumerated ONCE and shared. Two optional extensions are decided from
        // this list; asking the driver twice invites the two answers to be
        // taken from different enumerations.
        let device_exts: Vec<vk::ExtensionProperties> =
            unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
        let has_drm = device_exts
            .iter()
            .any(|e| ext_name(e) == ash::ext::physical_device_drm::NAME);
        if has_drm {
            want.push(ash::ext::physical_device_drm::NAME.as_ptr());
        }

        // ★ DYNAMIC RENDERING, AND WHY IT IS REQUIRED RATHER THAN OPTIONAL.
        //
        // Without it a compositor needs `VkRenderPass` + `VkFramebuffer`
        // objects, both of which are baked against a specific attachment set —
        // so every output size and format change means recreating them, and a
        // cache keyed on that tuple, and an invalidation rule. Dynamic
        // rendering deletes that entire category: `cmd_begin_rendering` names
        // its attachments inline, per frame.
        //
        // Supporting BOTH would double the draw path to serve hardware this
        // fleet does not have. So absence is a typed state with a fallback —
        // rule 4 of CONTAIN THE C — and the caller lands on nuri, which is a
        // correct compositor, not a degraded one.
        //
        // Requested as the KHR extension rather than by raising the instance
        // to API 1.3: a 1.2 driver that exposes the extension then still
        // works, which is strictly more machines than a version bump reaches.
        let has_dynamic_rendering = device_exts
            .iter()
            .any(|e| ext_name(e) == ash::khr::dynamic_rendering::NAME);
        if !has_dynamic_rendering {
            return Err(Unavailable::Driver(
                "the device does not offer VK_KHR_dynamic_rendering, which the \
                 compositor's draw path requires; falling back to the CPU pipe"
                    .to_owned(),
            ));
        }
        want.push(ash::khr::dynamic_rendering::NAME.as_ptr());

        // The extension is not enough — the FEATURE has to be switched on in
        // the device's pNext chain, and a driver that offers the extension
        // while the feature is off would otherwise fail later, at
        // `cmd_begin_rendering`, with a validation message about a command
        // rather than about device creation.
        let mut dyn_rendering =
            vk::PhysicalDeviceDynamicRenderingFeatures::default().dynamic_rendering(true);

        let dci = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&want)
            .push_next(&mut dyn_rendering);
        // SAFETY: `physical` is live, `dci` and everything it borrows are
        // locals outliving the call.
        let device = unsafe { instance.create_device(physical, &dci, None) }
            .map_err(|e| Unavailable::Driver(format!("create_device: {e:?}")))?;

        // SAFETY: live instance + physical device.
        let props = unsafe { instance.get_physical_device_properties(physical) };
        // SAFETY: `device_name` is a NUL-terminated fixed array from the driver.
        let name = unsafe { std::ffi::CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: live handles.
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical) };

        // SAFETY: `queue_family` was selected above from this device's own
        // family list, and index 0 exists because the device was created with
        // exactly one queue from it.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        // ★ RESET_COMMAND_BUFFER, so a buffer is re-recorded rather than
        // reallocated every frame. A compositor records the same shape of work
        // 360 times a second; allocating for that is churn the driver has to
        // absorb.
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family);
        // SAFETY: `pool_info` is a local outliving the call; the family index
        // is this device's own.
        let command_pool = match unsafe { device.create_command_pool(&pool_info, None) } {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: the device was created above and owns nothing yet.
                unsafe { device.destroy_device(None) };
                return Err(Unavailable::Driver(format!("create_command_pool: {e:?}")));
            }
        };

        Ok(Self {
            _entry: entry.clone(),
            queue,
            queue_family,
            command_pool,
            ext_fd: ash::khr::external_memory_fd::Device::new(instance, &device),
            dyn_render: ash::khr::dynamic_rendering::Device::new(instance, &device),
            debug,
            instance: instance.clone(),
            physical,
            device,
            mem_props,
            device_name: name,
            is_cpu: props.device_type == vk::PhysicalDeviceType::CPU,
            has_drm,
        })
    }

    /// Find a memory type satisfying `mask` and `flags`.
    fn memory_type(
        &self,
        mask: u32,
        flags: vk::MemoryPropertyFlags,
        wanted: &'static str,
    ) -> Result<u32, KasaneError> {
        (0..self.mem_props.memory_type_count)
            .find(|i| {
                let bit = mask & (1 << i) != 0;
                let t = self.mem_props.memory_types[*i as usize];
                bit && t.property_flags.contains(flags)
            })
            .ok_or(KasaneError::NoMemoryType { wanted, mask })
    }

    /// Create a linear, host-visible, dmabuf-exportable image and fill it.
    ///
    /// # Errors
    /// A Vulkan failure, or no memory type that is both host-visible and
    /// permitted by the image's requirements.
    pub fn export_linear(
        self: &std::sync::Arc<Self>,
        width: u32,
        height: u32,
        fill: [u8; 4],
    ) -> Result<Exported, KasaneError> {
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE);
        let ici = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            // LINEAR so the rows are addressable by a CPU — which is what
            // lets this write pixels by mapping instead of submitting a
            // command buffer.
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            // ★ UNDEFINED, NOT PREINITIALIZED — AND THIS WAS A REAL BUG, found
            // by the validation gate on the day it was added.
            //
            // PREINITIALIZED is the reflex for a linear image filled by the
            // CPU, and it is what this said for as long as M0 has existed. It
            // is ILLEGAL here: `VUID-VkImageCreateInfo-pNext-01443` requires
            // `UNDEFINED` whenever the pNext chain carries
            // `VkExternalMemoryImageCreateInfo` with a non-zero `handleTypes`,
            // which this one does. Both drivers we have accepted it anyway, so
            // no test could see it and it would have shipped.
            //
            // Correct because the memory is written through a HOST mapping and
            // read back through one — the image object's layout never governs
            // those accesses. Proven by the M0 round-trip still recovering its
            // pixel, which is a stronger check than the layout enum.
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `ici` and the struct it chains are locals outliving the call.
        let image = unsafe { self.device.create_image(&ici, None) }
            .map_err(|e| driver("create_image(export)", e))?;

        let make = || -> Result<(vk::DeviceMemory, Geometry), KasaneError> {
            // SAFETY: `image` was just created on this device.
            let reqs = unsafe { self.device.get_image_memory_requirements(image) };
            let idx = self.memory_type(
                reqs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                "HOST_VISIBLE | HOST_COHERENT",
            )?;
            let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(HANDLE);
            let ai = vk::MemoryAllocateInfo::default()
                .push_next(&mut export)
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: locals outlive the call.
            let memory = unsafe { self.device.allocate_memory(&ai, None) }
                .map_err(|e| driver("allocate_memory(export)", e))?;
            // SAFETY: image and memory are both live, and nothing else is bound
            // to either.
            unsafe { self.device.bind_image_memory(image, memory, 0) }
                .map_err(|e| driver("bind_image_memory(export)", e))?;

            // ★ THE STRIDE COMES FROM THE DRIVER, never from width * 4.
            let sub = vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::COLOR);
            // SAFETY: live image, linear tiling (required by the spec for this
            // query, and set above).
            let layout = unsafe { self.device.get_image_subresource_layout(image, sub) };
            let geometry = Geometry {
                width,
                height,
                stride: layout.row_pitch,
                offset: layout.offset,
            };

            // SAFETY: the memory is HOST_VISIBLE (selected above), the whole
            // range is mapped, and nothing else holds a mapping of it.
            let ptr = unsafe {
                self.device
                    .map_memory(memory, 0, reqs.size, vk::MemoryMapFlags::empty())
            }
            .map_err(|e| driver("map_memory(export)", e))?
            .cast::<u8>();
            for y in 0..height {
                for x in 0..width {
                    let Some(off) = geometry.byte_offset(x, y) else {
                        continue;
                    };
                    if off + 4 > reqs.size {
                        continue;
                    }
                    // SAFETY: `off + 4 <= reqs.size` is checked directly above,
                    // and `ptr` maps exactly `reqs.size` bytes.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            fill.as_ptr(),
                            ptr.add(usize::try_from(off).unwrap_or(0)),
                            4,
                        );
                    }
                }
            }
            // HOST_COHERENT, so no explicit flush is needed before unmapping.
            // SAFETY: the memory is currently mapped by the call above.
            unsafe { self.device.unmap_memory(memory) };
            Ok((memory, geometry))
        };

        let (memory, geometry) = match make() {
            Ok(v) => v,
            Err(e) => {
                // SAFETY: `image` is live and unbound-to-nothing-else.
                unsafe { self.device.destroy_image(image, None) };
                return Err(e);
            }
        };

        let info = vk::MemoryGetFdInfoKHR::default()
            .memory(memory)
            .handle_type(HANDLE);
        // SAFETY: `memory` was allocated with an export info naming this handle
        // type, which is what makes the query legal.
        let raw =
            unsafe { self.ext_fd.get_memory_fd(&info) }.map_err(|e| driver("get_memory_fd", e))?;
        // SAFETY: Vulkan transfers ownership of this fd to us; wrapping it in
        // `OwnedFd` is what makes the close happen exactly once.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        Ok(Exported {
            gpu: std::sync::Arc::clone(self),
            image,
            memory,
            fd: Some(fd),
            geometry,
        })
    }

    /// Import a dmabuf as a linear image and make its pixels readable.
    ///
    /// Consumes `fd`: Vulkan takes ownership on a successful import, and the
    /// type says so rather than leaving a double-close to discipline.
    ///
    /// # Errors
    /// A Vulkan failure, or no memory type permitted by BOTH the image's
    /// requirements and the driver's answer for this particular fd.
    pub fn import_linear(
        self: &std::sync::Arc<Self>,
        fd: OwnedFd,
        geometry: Geometry,
    ) -> Result<Imported, KasaneError> {
        // ★ ASK THE DRIVER WHICH MEMORY TYPES THIS FD MAY USE, before consuming
        // it. Intersecting with the image's own requirements is what makes the
        // import legal; using either alone is how an import "succeeds" and then
        // binds to memory the driver never approved.
        let raw = fd.as_raw_fd_compat();
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: `raw` is a live dmabuf fd we still own — this query does not
        // consume it — and `fd_props` is a local that outlives the call, which
        // is where the driver writes its answer.
        unsafe {
            self.ext_fd
                .get_memory_fd_properties(HANDLE, raw, &mut fd_props)
        }
        .map_err(|e| driver("get_memory_fd_properties", e))?;

        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE);
        let ici = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(vk::Extent3D {
                width: geometry.width,
                height: geometry.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::LINEAR)
            // ★ SAMPLED IS NOT OPTIONAL, and its absence was invisible.
            //
            // `import_linear` was written for M0, whose only job is to read
            // the buffer back with the CPU — so TRANSFER was all it needed.
            // The moment a client buffer is COMPOSITED it becomes a texture,
            // and an image without this bit may not be viewed as one, barriered
            // to SHADER_READ_ONLY_OPTIMAL, or written into a descriptor.
            //
            // lavapipe did all three anyway and returned the RIGHT PIXELS.
            // Three separate VUIDs fired under the validation layer and not one
            // test could see them — this is the class that gate exists for, and
            // `import_tiled` (M1a, written to be sampled) already had it.
            .usage(
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            // ★ Same VUID-VkImageCreateInfo-pNext-01443 as the export side —
            // this image also chains `VkExternalMemoryImageCreateInfo`. See
            // the note there.
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: locals outlive the call.
        let image = unsafe { self.device.create_image(&ici, None) }
            .map_err(|e| driver("create_image(import)", e))?;

        let make = || -> Result<vk::DeviceMemory, KasaneError> {
            // SAFETY: live image on this device.
            let reqs = unsafe { self.device.get_image_memory_requirements(image) };
            let mask = reqs.memory_type_bits & fd_props.memory_type_bits;
            let idx =
                self.memory_type(mask, vk::MemoryPropertyFlags::HOST_VISIBLE, "HOST_VISIBLE")?;
            // Vulkan takes the fd on success, so it is released from `OwnedFd`
            // here and not before — a failure above must still close it.
            let raw = fd.into_raw_fd();
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(HANDLE)
                .fd(raw);
            let ai = vk::MemoryAllocateInfo::default()
                .push_next(&mut import)
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: locals outlive the call; `raw` is a valid dmabuf fd whose
            // ownership passes to Vulkan on success.
            let memory = unsafe { self.device.allocate_memory(&ai, None) }.map_err(|e| {
                // On failure Vulkan did NOT take the fd, so we must close it or
                // leak one per failed import.
                // SAFETY: `raw` is still ours because the call failed.
                drop(unsafe { OwnedFd::from_raw_fd(raw) });
                driver("allocate_memory(import)", e)
            })?;
            // SAFETY: live image and memory, nothing else bound.
            unsafe { self.device.bind_image_memory(image, memory, 0) }
                .map_err(|e| driver("bind_image_memory(import)", e))?;
            Ok(memory)
        };

        match make() {
            Ok(memory) => {
                // The view must be made AFTER the memory is bound: a view of
                // an unbound image is invalid, and the driver need not say so.
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(FORMAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    );
                // SAFETY: `view_info` is a local naming an image with memory
                // bound on the line above.
                match unsafe { self.device.create_image_view(&view_info, None) } {
                    Ok(view) => Ok(Imported {
                        gpu: std::sync::Arc::clone(self),
                        image,
                        memory,
                        view,
                        geometry,
                    }),
                    Err(e) => {
                        // SAFETY: both are live and neither is referenced yet.
                        unsafe {
                            self.device.destroy_image(image, None);
                            self.device.free_memory(memory, None);
                        }
                        Err(driver("create_image_view(import)", e))
                    }
                }
            }
            Err(e) => {
                // SAFETY: live image; memory was never bound on this path.
                unsafe { self.device.destroy_image(image, None) };
                Err(e)
            }
        }
    }
}

impl Gpu {
    /// Record one command buffer, submit it, and wait for it.
    ///
    /// ── ★ ONE SUBMIT SHAPE FOR THE WHOLE CRATE ───────────────────────────
    /// Allocating a buffer, beginning it, ending it, making a fence, waiting
    /// on it and freeing everything — on every path, including the error
    /// paths — is six chances to leak a fence or a command buffer, repeated
    /// per caller. It appears three times already (a frame, an upload, a
    /// capture), so it is one function.
    ///
    /// ★ A FENCE, NOT `device_wait_idle`. Waiting on the whole device would
    /// also wait on any other work in flight, which in a compositor means one
    /// slow client stalls every output.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] naming the failing call, or whatever `record`
    /// returns — in which case nothing is submitted and the buffer is still
    /// freed.
    pub(crate) fn submit_once<F>(&self, record: F) -> Result<(), KasaneError>
    where
        F: FnOnce(vk::CommandBuffer) -> Result<(), KasaneError>,
    {
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live and owned by this device.
        let cmds = unsafe { self.device.allocate_command_buffers(&alloc) }
            .map_err(|e| driver("allocate_command_buffers", e))?;
        let cmd = cmds[0];

        let result = self.record_submit_wait(cmd, record);

        // Freed on every path. A test that leaks one buffer per call exhausts
        // the pool and then fails somewhere unrelated.
        // SAFETY: `cmd` came from this pool, and the wait inside has returned
        // so it is not in flight.
        unsafe { self.device.free_command_buffers(self.command_pool, &cmds) };
        result
    }

    fn record_submit_wait<F>(&self, cmd: vk::CommandBuffer, record: F) -> Result<(), KasaneError>
    where
        F: FnOnce(vk::CommandBuffer) -> Result<(), KasaneError>,
    {
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `cmd` is a fresh primary buffer from this device's pool.
        unsafe { self.device.begin_command_buffer(cmd, &begin) }
            .map_err(|e| driver("begin_command_buffer", e))?;

        record(cmd)?;

        // SAFETY: recording is complete and balanced.
        unsafe { self.device.end_command_buffer(cmd) }
            .map_err(|e| driver("end_command_buffer", e))?;

        let fence_info = vk::FenceCreateInfo::default();
        // SAFETY: `fence_info` is a local.
        let fence = unsafe { self.device.create_fence(&fence_info, None) }
            .map_err(|e| driver("create_fence", e))?;
        let bufs = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&bufs);
        // SAFETY: queue and fence are from this device; `submit` borrows
        // `bufs`, a local that outlives the wait below.
        let submitted = unsafe { self.device.queue_submit(self.queue, &[submit], fence) };
        let waited = submitted.and_then(|()| {
            // SAFETY: the fence was just submitted with.
            unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
        });
        // SAFETY: the wait returned, so the fence is no longer in use.
        // Destroyed on the error path too — a leaked fence per failed frame is
        // a slow exhaustion that presents far from its cause.
        unsafe { self.device.destroy_fence(fence, None) };
        waited.map_err(|e| driver("submit/wait", e))
    }

    /// Upload host bytes into a new device-local sampled texture.
    ///
    /// `data` is tightly-packed BGRA — `width * 4` bytes per row.
    ///
    /// ★ For `wl_shm` clients, which hand over shared memory rather than a
    /// dmabuf. See [`Uploaded`] for why the copy here is not the copy this
    /// crate exists to remove.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] naming the failing call, or
    /// [`KasaneError::NoMemoryType`] if the device offers no suitable memory.
    pub fn upload_texture(
        self: &std::sync::Arc<Self>,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<Uploaded, KasaneError> {
        let dev = &self.device;
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            // OPTIMAL because nothing external reads this — unlike an imported
            // dmabuf, whose layout is dictated by the exporter.
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `image_info` and everything it borrows are locals.
        let image = unsafe { dev.create_image(&image_info, None) }
            .map_err(|e| driver("create_image(upload)", e))?;

        // Built inside a closure so one error path cleans up everything made
        // so far, rather than five nested matches.
        let build = || -> Result<(vk::DeviceMemory, vk::ImageView, vk::Buffer, vk::DeviceMemory), KasaneError>
        {
            // SAFETY: the image was just created on this device.
            let reqs = unsafe { dev.get_image_memory_requirements(image) };
            let idx = self.memory_type(
                reqs.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                "DEVICE_LOCAL",
            )?;
            let alloc = vk::MemoryAllocateInfo::default()
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: `alloc` is a local; `idx` came from this device.
            let memory = unsafe { dev.allocate_memory(&alloc, None) }
                .map_err(|e| driver("allocate_memory(upload)", e))?;
            // SAFETY: image and memory both from this device, bound once.
            unsafe { dev.bind_image_memory(image, memory, 0) }
                .map_err(|e| driver("bind_image_memory(upload)", e))?;

            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(FORMAT)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            // SAFETY: names an image with memory bound above.
            let view = unsafe { dev.create_image_view(&view_info, None) }
                .map_err(|e| driver("create_image_view(upload)", e))?;

            // Sized for the WHOLE texture even though an update may be a small
            // region: a client that damages a different rectangle each frame
            // would otherwise reallocate constantly.
            let size = u64::from(width) * u64::from(height) * 4;
            let buf_info = vk::BufferCreateInfo::default()
                .size(size)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            // SAFETY: `buf_info` is a local.
            let staging = unsafe { dev.create_buffer(&buf_info, None) }
                .map_err(|e| driver("create_buffer(staging)", e))?;
            // SAFETY: the buffer was just created on this device.
            let breqs = unsafe { dev.get_buffer_memory_requirements(staging) };
            let bidx = self.memory_type(
                breqs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                "HOST_VISIBLE | HOST_COHERENT",
            )?;
            let balloc = vk::MemoryAllocateInfo::default()
                .allocation_size(breqs.size)
                .memory_type_index(bidx);
            // SAFETY: `balloc` is a local; `bidx` came from this device.
            let staging_memory = unsafe { dev.allocate_memory(&balloc, None) }
                .map_err(|e| driver("allocate_memory(staging)", e))?;
            // SAFETY: buffer and memory both from this device, bound once.
            unsafe { dev.bind_buffer_memory(staging, staging_memory, 0) }
                .map_err(|e| driver("bind_buffer_memory(staging)", e))?;
            Ok((memory, view, staging, staging_memory))
        };

        match build() {
            Ok((memory, view, staging, staging_memory)) => {
                let up = Uploaded {
                    gpu: std::sync::Arc::clone(self),
                    image,
                    view,
                    memory,
                    staging,
                    staging_memory,
                    geometry: Geometry {
                        width,
                        height,
                        // Tightly packed: this image is ours, so nobody else's
                        // stride applies.
                        stride: u64::from(width) * 4,
                        offset: 0,
                    },
                };
                // The first write puts the pixels in AND leaves the image in
                // SHADER_READ_ONLY_OPTIMAL, so a freshly uploaded texture is
                // immediately drawable.
                up.write(0, 0, width, height, data)?;
                Ok(up)
            }
            Err(e) => {
                // SAFETY: live image; nothing else references it.
                unsafe { dev.destroy_image(image, None) };
                Err(e)
            }
        }
    }

    /// Create the compositor's shader module on this device.
    ///
    /// One module holds all three entry points, so a pipeline names the stage
    /// it wants rather than each stage owning a module. The caller destroys it
    /// with [`Gpu::destroy_shader_module`] — the pipelines that reference it
    /// keep working after it is gone, which is why it is not held on `Gpu`.
    ///
    /// # Errors
    /// [`Unavailable::Device`] if the driver refuses the module. In practice
    /// that means the SPIR-V is malformed, which `build.rs`'s validation pass
    /// is there to make impossible before this is ever reached.
    pub(crate) fn shader_module(&self) -> Result<vk::ShaderModule, Unavailable> {
        // Vulkan takes SPIR-V as 32-bit words, and the pointer must be
        // 4-byte aligned. `include_bytes!` gives a `&[u8]` with only 1-byte
        // guaranteed alignment, so the bytes are copied into a `Vec<u32>`
        // rather than transmuted — a misaligned read here is UB that happens
        // to work on x86 and faults elsewhere.
        // ★ NOT REDUNDANT WITH `chunks_exact`. `chunks_exact(4)` SILENTLY
        // DISCARDS a trailing partial chunk, so a blob of 2814 bytes would
        // become 703 whole words and reach the driver as a complete module —
        // which either fails much later with a message about the shader body,
        // or is accepted and renders wrong. The refusal has to happen here,
        // while the length is still known.
        //
        // The message names OUR blob rather than the driver, because the
        // `Driver` arm otherwise reads as a hardware problem and sends the
        // reader to the wrong machine.
        if !crate::COMPOSITE_SPV.len().is_multiple_of(4) {
            return Err(Unavailable::Driver(format!(
                "the compiled shader is {} bytes, not a whole number of 32-bit \
                 SPIR-V words — build.rs emitted a short blob; this is not a \
                 driver fault",
                crate::COMPOSITE_SPV.len()
            )));
        }
        let words: Vec<u32> = crate::COMPOSITE_SPV
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let info = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: `words` outlives the call, its length is a whole number of
        // words, and `info` borrows it for exactly that scope.
        unsafe { self.device.create_shader_module(&info, None) }.map_err(|e| {
            Unavailable::Driver(format!(
                "vkCreateShaderModule refused the compositor shader: {e:?}"
            ))
        })
    }

    /// Destroy a module created by [`Gpu::shader_module`].
    ///
    /// # Safety
    /// The module must have come from this device and must not be destroyed
    /// twice. Pipelines created from it stay valid — Vulkan copies what it
    /// needs at pipeline-creation time.
    pub(crate) unsafe fn destroy_shader_module(&self, module: vk::ShaderModule) {
        unsafe { self.device.destroy_shader_module(module, None) }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: every child object is owned by an `Exported`/`Imported` whose
        // lifetime is tied to `&self`, so all of them are already dropped by the
        // time this runs — that is what the borrow checker is enforcing here.
        unsafe {
            // ★ ORDER IS LOAD-BEARING: the pool is a child of the device, so
            // destroying the device first leaves it dangling. Vulkan will not
            // tell you — it is undefined behaviour, and the validation layers
            // are not on in production.
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            // Before the instance: the messenger is the instance's child, and
            // it must also stop being called before the instance goes — a
            // callback firing during `destroy_instance` would read freed
            // state.
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// The one part of [`Params`] that needs the seam: its bytes.
///
/// The struct itself is safe and lives at the crate root so its geometry is
/// tested on every platform — see its header. Only this needs `unsafe`, so
/// only this is here.
impl crate::Params {
    /// The bytes, for `cmd_push_constants`.
    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `Self` is `repr(C)` and entirely `f32`, so every byte is
        // initialised and there is no padding to expose. The slice borrows
        // `self` and cannot outlive it.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(self).cast::<u8>(),
                std::mem::size_of::<Self>(),
            )
        }
    }
}

/// What an imported dmabuf is going to be USED for.
///
/// ── ★ WHY THIS IS ONE CHOICE AND NOT TWO FIELDS ──────────────────────────
/// Importing a dmabuf needs two things that must agree: the `VkImageUsageFlags`
/// the image is created with, and the `VkFormatFeatureFlags` the modifier must
/// support. Passing them separately makes the disagreement expressible — and
/// it is a disagreement with no error: the driver offers a modifier list
/// filtered for `SAMPLED_IMAGE`, the import checks against that list, and
/// then creates a `COLOR_ATTACHMENT` image whose modifier was never validated
/// for
/// rendering.
///
/// Deriving both from one enum makes that unrepresentable. A caller states the
/// PURPOSE; the flags follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportUse {
    /// A client surface the compositor will sample.
    Surface,
    /// A scanout buffer the compositor will render into.
    RenderTarget,
}

impl ImportUse {
    /// The image usage this purpose needs.
    ///
    /// ── ★ EXACTLY ONE BIT, AND THE FIRST DRAFT HAD TWO ───────────────────
    /// This originally added `TRANSFER_SRC` to both, reasoning that either
    /// might be read back. That was WRONG, and the validation layer caught it
    /// the first time a dmabuf was used as a render target:
    ///
    /// ```text
    /// vkCreateImageView(): format B8G8R8A8_UNORM with tiling
    ///   DRM_FORMAT_MODIFIER_EXT has no supported format features
    /// vkCmdCopyImageToBuffer(): srcImage ... must contain TRANSFER_SRC_BIT
    /// ```
    ///
    /// With DRM-modifier tiling the format features come from the MODIFIER,
    /// not from `optimalTilingFeatures` — and lavapipe's linear modifier
    /// offers `COLOR_ATTACHMENT` without `TRANSFER_SRC`. Asking for a usage the
    /// modifier does not support left an image whose view had no valid
    /// features at all, and lavapipe created it anyway. The pixels still came
    /// out right, which is precisely why nothing but the layer could see it.
    ///
    /// So the usage is now exactly the bit the purpose needs, and
    /// [`ImportUse::required_feature`] is its mirror — see the test that pins
    /// the correspondence.
    fn usage(self) -> vk::ImageUsageFlags {
        match self {
            Self::Surface => vk::ImageUsageFlags::SAMPLED,
            // ★ TRANSFER_SRC IS NOT OPTIONAL ON A SCANOUT TARGET. `drm.rs`'s
            // renderer bound demands `ExportMem` — a seat whose output cannot
            // be read back can only be debugged by walking to the machine —
            // and `Target::capture` is a `vkCmdCopyImageToBuffer`, which
            // requires it.
            //
            // ★ IT WAS REMOVED ONCE, ON A MISDIAGNOSIS. The layer reported
            // both a missing TRANSFER_SRC and a view with no format features,
            // and dropping the bit silenced the first. The raw modifier
            // features say otherwise: lavapipe reports modifier 0 as `0xdd83`,
            // which INCLUDES TRANSFER_SRC. The view complaint was a separate,
            // measured layer disagreement — see `EXEMPT_VUIDS` — and treating
            // the two as one cause broke capture.
            Self::RenderTarget => {
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
            }
        }
    }

    /// The format feature a modifier must support to serve this purpose.
    ///
    /// ★ THE MIRROR OF [`ImportUse::usage`]. A modifier the driver can SAMPLE
    /// is not necessarily one it can RENDER INTO, and nothing reports the
    /// difference — the image is created and the pixels come out wrong. The
    /// two must name the same capability, which is what the correspondence
    /// test asserts.
    fn required_feature(self) -> vk::FormatFeatureFlags {
        match self {
            Self::Surface => vk::FormatFeatureFlags::SAMPLED_IMAGE,
            // The mirror of `usage`: a modifier offered as a render target
            // must support BOTH, or `Target::capture` fails on a buffer the
            // filter said was fine.
            Self::RenderTarget => {
                vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::TRANSFER_SRC
            }
        }
    }
}

/// How a texture is sampled when the source and destination sizes differ.
///
/// A closed pair rather than a `bool`, so a call site reads as what it means
/// and a third policy (were one ever wanted) is a compile error at every
/// `match` rather than a silently-wrong branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Nearest-neighbour. The right choice at 1:1, where LINEAR would blur a
    /// pixel-exact surface by sampling between texels.
    Nearest,
    /// Bilinear, for genuine scaling.
    Linear,
}

/// Everything needed to draw, created once and reused for every frame.
///
/// ── ★ WHY THIS IS BUILT PER FORMAT ───────────────────────────────────────
/// A graphics pipeline is compiled against its colour attachment's format, so
/// this takes the format it will render into. A compositor has one output
/// format at a time, so that is one `Pipelines` rather than a cache — and if a
/// second format ever appears, it is a second `Pipelines`, not an invalidation
/// rule.
///
/// Borrows the [`Gpu`], so it cannot outlive the device that must destroy it.
/// The compiler enforces the ordering; no comment has to.
pub struct Pipelines {
    /// The device this belongs to.
    ///
    /// ★ AN `Arc`, NOT A BORROW — and the reason is structural, not stylistic.
    /// A smithay `Renderer` must own its device AND its compiled pipelines,
    /// because `render()` takes `&mut self` and hands back a `Frame` that
    /// draws. With `gpu: &'g Gpu` that renderer is a self-referential struct,
    /// which safe Rust cannot express at all.
    ///
    /// The guarantee is unchanged: the device outlives every object that must
    /// be destroyed on it, enforced by the refcount instead of by a lifetime.
    /// Drop order WITHIN this struct is still explicit and still load-bearing.
    gpu: std::sync::Arc<Gpu>,
    module: vk::ShaderModule,
    set_layout: vk::DescriptorSetLayout,
    /// The pipeline layout, public because recording a draw needs it to push
    /// constants and bind the descriptor set.
    pub(crate) layout: vk::PipelineLayout,
    /// Samples a client surface.
    pub(crate) textured: vk::Pipeline,
    /// Fills a rectangle with a flat colour.
    pub(crate) solid: vk::Pipeline,
    sampler_nearest: vk::Sampler,
    sampler_linear: vk::Sampler,
    /// Descriptor sets for textured draws, reset once per frame.
    ///
    /// ★ RESET, NOT FREED PER SET. A compositor allocates the same handful of
    /// descriptors every frame; freeing individually needs
    /// `FREE_DESCRIPTOR_SET` and leaves the pool fragmented. Resetting the
    /// whole pool at the top of a frame is one call and cannot fragment.
    ///
    /// ★ Safe only because `Target::draw` waits on its fence before returning,
    /// so the previous frame's sets are certainly not in use. When the
    /// command-buffer ring lands, this becomes one pool PER FRAME IN FLIGHT —
    /// resetting a pool whose sets a running frame still references is
    /// undefined behaviour, and it is the first thing the ring must fix.
    descriptor_pool: vk::DescriptorPool,
    /// The colour format these were compiled against, so a caller cannot bind
    /// them to a mismatched attachment without the check being possible.
    pub format: vk::Format,
}

impl Pipelines {
    /// How many textured draws one frame may contain.
    ///
    /// ★ A HARD CEILING, and it fails LOUDLY rather than silently dropping the
    /// surplus — a compositor that quietly stops drawing after the 256th
    /// surface produces a half-rendered screen with no error, which is
    /// indistinguishable from a client bug. 256 is far above any real seat
    /// (plo runs single digits); raising it is one constant.
    pub const MAX_TEXTURE_DRAWS: u32 = 256;

    /// Compile the compositor's two pipelines for `format`.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] if the driver refuses any object. Every failure
    /// here is a driver verdict rather than a state — the SPIR-V was validated
    /// at build time and the layouts are constants.
    pub fn new(gpu: &std::sync::Arc<Gpu>, format: vk::Format) -> Result<Self, KasaneError> {
        let module = gpu.shader_module()?;

        // Built incrementally so that a failure part-way through still
        // destroys what was already made. Without this, an error between the
        // sampler and the pipeline leaks a sampler on every retry — and a
        // compositor retries.
        let mut built = Self {
            gpu: std::sync::Arc::clone(gpu),
            module,
            set_layout: vk::DescriptorSetLayout::null(),
            layout: vk::PipelineLayout::null(),
            textured: vk::Pipeline::null(),
            solid: vk::Pipeline::null(),
            sampler_nearest: vk::Sampler::null(),
            sampler_linear: vk::Sampler::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            format,
        };
        match built.finish_building() {
            Ok(()) => Ok(built),
            // `built`'s `Drop` runs here and destroys the partial set — the
            // reason the struct is constructed before it is complete.
            Err(e) => Err(e),
        }
    }

    fn finish_building(&mut self) -> Result<(), KasaneError> {
        let dev = &self.gpu.device;

        // ★ SEPARATE IMAGE AND SAMPLER, not COMBINED_IMAGE_SAMPLER. This
        // mirrors what the WGSL declares — `var tex: texture_2d<f32>` at
        // binding 0 and `var samp: sampler` at binding 1 — and naga lowers
        // those to two descriptors. Declaring a combined one here compiles and
        // then fails at descriptor-write time with a type mismatch that reads
        // as a binding-number problem.
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: `bindings` outlives the call; the device is live.
        self.set_layout = unsafe { dev.create_descriptor_set_layout(&set_info, None) }
            .map_err(|e| driver("descriptor set layout", e))?;

        // ★ THE RANGE COVERS BOTH STAGES. The vertex shader reads `dst`/`src`
        // and the fragment shader reads `tint`, and naga emits ONE push
        // constant block shared by both. A range naming only FRAGMENT would
        // leave the vertex stage reading undefined memory — every surface
        // drawn somewhere arbitrary, with no error.
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(crate::Params::SIZE)];
        let set_layouts = [self.set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: both slices outlive the call.
        self.layout = unsafe { dev.create_pipeline_layout(&layout_info, None) }
            .map_err(|e| driver("pipeline layout", e))?;

        self.sampler_nearest = self.sampler(vk::Filter::NEAREST)?;
        self.sampler_linear = self.sampler(vk::Filter::LINEAR)?;

        self.textured = self.pipeline(crate::entry::FRAGMENT_TEXTURE)?;
        self.solid = self.pipeline(crate::entry::FRAGMENT_SOLID)?;

        // Two descriptors per set — the WGSL declares the image and the
        // sampler separately, so the pool must size both.
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(Self::MAX_TEXTURE_DRAWS),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(Self::MAX_TEXTURE_DRAWS),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(Self::MAX_TEXTURE_DRAWS)
            .pool_sizes(&sizes);
        // SAFETY: `sizes` outlives the call.
        self.descriptor_pool = unsafe { dev.create_descriptor_pool(&pool_info, None) }
            .map_err(|e| driver("create_descriptor_pool", e))?;
        Ok(())
    }

    fn sampler(&self, filter: vk::Filter) -> Result<vk::Sampler, KasaneError> {
        // ★ CLAMP_TO_EDGE on both axes. A compositor samples a surface's own
        // rectangle; REPEAT would wrap the opposite edge into any half-texel
        // the sampler reaches past the border, drawing a one-pixel line of the
        // far side of the window along each edge.
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: `info` borrows nothing that does not outlive the call.
        unsafe { self.gpu.device.create_sampler(&info, None) }.map_err(|e| driver("sampler", e))
    }

    fn pipeline(&self, fragment_entry: &[u8]) -> Result<vk::Pipeline, KasaneError> {
        let vs_name = std::ffi::CStr::from_bytes_with_nul(crate::entry::VERTEX)
            .expect("the entry-point constants carry their NUL");
        let fs_name = std::ffi::CStr::from_bytes_with_nul(fragment_entry)
            .expect("the entry-point constants carry their NUL");
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(self.module)
                .name(vs_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(self.module)
                .name(fs_name),
        ];

        // ★ NO VERTEX INPUT AT ALL. The quad comes from `vertex_index`, so
        // there is no buffer to bind, nothing to upload per frame, and no
        // vertex-attribute description to keep in step with the shader.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);

        // Viewport and scissor are DYNAMIC, so one pipeline serves every
        // output size. Baking them in would mean recompiling both pipelines on
        // a mode change — a visible stall, to avoid two `cmd_set_*` calls.
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            // ★ NO CULLING. The quad's winding depends on whether `dst` has a
            // negative extent, which is how a caller expresses a flip. Culling
            // would make a flipped surface silently vanish.
            .cull_mode(vk::CullModeFlags::NONE)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // ★★ PREMULTIPLIED ALPHA — the single most consequential constant here.
        // wl_shm and zwp_linux_dmabuf_v1 both deliver premultiplied buffers, so
        // the source factor is ONE. Using SRC_ALPHA (the reflex, and what every
        // tutorial shows) multiplies alpha a second time and darkens every
        // translucent edge on the seat — a defect that looks like a theme
        // problem rather than a blend-state one.
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);

        // Dynamic rendering: the attachment format is declared on the pipeline
        // instead of through a `VkRenderPass` object.
        let formats = [self.format];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);

        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(self.layout)
            .push_next(&mut rendering);

        // SAFETY: every borrowed structure is a local outliving the call.
        // `create_graphics_pipelines` returns the partial vector alongside the
        // error, which is why the error arm destructures a tuple.
        let pipelines = unsafe {
            self.gpu
                .device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
        }
        .map_err(|(_, e)| driver("graphics pipeline", e))?;

        pipelines
            .first()
            .copied()
            .ok_or_else(|| KasaneError::Vulkan {
                call: "create_graphics_pipelines",
                result: "reported success and returned no pipeline".to_owned(),
            })
    }

    /// The sampler for a filter choice.
    pub(crate) fn sampler_for(&self, filter: Filter) -> vk::Sampler {
        match filter {
            Filter::Nearest => self.sampler_nearest,
            Filter::Linear => self.sampler_linear,
        }
    }
}

impl Drop for Pipelines {
    fn drop(&mut self) {
        let dev = &self.gpu.device;
        // SAFETY: every handle came from this device, and each is destroyed
        // once. Null handles are explicitly legal to pass to `destroy_*`,
        // which is what makes the partially-built case safe.
        unsafe {
            dev.destroy_pipeline(self.textured, None);
            dev.destroy_pipeline(self.solid, None);
            dev.destroy_sampler(self.sampler_linear, None);
            dev.destroy_sampler(self.sampler_nearest, None);
            // Before the set layout it was built from.
            dev.destroy_descriptor_pool(self.descriptor_pool, None);
            dev.destroy_pipeline_layout(self.layout, None);
            dev.destroy_descriptor_set_layout(self.set_layout, None);
            // Destroyed LAST of the children but before the device: pipelines
            // reference it at creation time only, so this order is safe and
            // the reverse would be too — stated because the next reader will
            // wonder.
            //
            // Through `Gpu`'s own helper rather than a second `dev.` call, so
            // there is one spelling of "destroy this module" in the crate.
            self.gpu.destroy_shader_module(self.module);
        }
    }
}

/// A sampleable image, borrowed for the length of a draw list.
///
/// ★ Carries the IMAGE as well as the view because the recorder has to
/// transition the image's layout to `SHADER_READ_ONLY_OPTIMAL`, and a view
/// does not name its image. Passing only the view compiles and then samples an
/// image in the wrong layout — undefined, and on a real driver it reads as
/// garbage rather than an error.
#[derive(Clone, Copy)]
pub struct TextureRef {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    /// The layout this image is ALREADY in.
    ///
    /// ── ★★ THIS FIELD IS A BUG FIX, AND THE BUG WAS SILENT ───────────────
    /// The recorder used to transition every texture from `UNDEFINED`, which
    /// is convenient because it needs no tracking — and `UNDEFINED` is defined
    /// to DISCARD THE CONTENTS. For an imported dmabuf that is harmless: the
    /// pixels live in memory another process wrote and this image is only a
    /// view onto it.
    ///
    /// For an UPLOADED texture it throws away the pixels that were just
    /// uploaded, immediately before sampling them. Measured: a partial update
    /// of a `wl_shm` texture read back as the ORIGINAL contents, because the
    /// draw discarded the update. lavapipe kept enough state that the first,
    /// simpler upload test still passed — which is exactly why this needed a
    /// second test with two different values in one texture to surface.
    ///
    /// Carrying the real layout means the barrier transitions FROM the truth,
    /// and can be skipped entirely when the texture is already where it needs
    /// to be.
    pub(crate) layout: vk::ImageLayout,
}

impl std::fmt::Debug for TextureRef {
    // ★ HAND-WRITTEN because `#[derive(Debug)]` cannot cover a struct holding
    // an ash type: this crate builds ash with `default-features = false`, so
    // `ImageLayout` has no `Debug`. Fourth time this rule has cost a build —
    // see the note at the top of this file. Layouts are printed raw.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureRef")
            .field("layout", &self.layout.as_raw())
            .finish_non_exhaustive()
    }
}

/// One rectangle to draw, as a closed set.
///
/// ★ A CLOSED ENUM rather than a method per shape, so adding a third kind is a
/// compile error at every `match` instead of a branch someone forgets. It is
/// also the routing vocabulary's unit: whatever decides GPU-vs-CPU decides it
/// per `Draw`, and a variant that has no GPU form simply has no arm here.
#[derive(Clone, Copy, Debug)]
pub enum Draw {
    /// A flat premultiplied colour — a bar background, a border, an overlay.
    Solid(crate::Params),
    /// A client surface, sampled from an imported buffer.
    Texture {
        /// Where it goes and how opaque it is.
        params: crate::Params,
        /// What to sample.
        texture: TextureRef,
        /// How to sample it when the sizes differ.
        filter: Filter,
    },
}

/// An off-screen render target, plus the path to read it back.
///
/// ── ★ OPTIMAL TILING AND A COPY, NOT A LINEAR ATTACHMENT ─────────────────
/// Rendering straight into a LINEAR host-visible image and mapping it would be
/// shorter, and it works on lavapipe. It does NOT work on the hardware this
/// exists for: NVIDIA does not advertise `COLOR_ATTACHMENT` in
/// `linearTilingFeatures`, so the image creation fails on plo's 3070 and
/// succeeds in CI — the worst possible split, since the test machine would
/// prove a path the real machine cannot take.
/// Where a [`Target`]'s pixels live, and therefore who destroys them.
///
/// ★ A CLOSED CHOICE rather than an `Option<DeviceMemory>` beside a raw image.
/// With separate fields, "an owned target whose memory handle is null" and "an
/// imported target that also tries to free memory it does not own" are both
/// constructible, and the second is a double free the driver will not warn
/// about. As two variants, neither has a representation.
enum Backing {
    /// This target allocated its own image — the off-screen case.
    Owned {
        image: vk::Image,
        view: vk::ImageView,
        memory: vk::DeviceMemory,
    },
    /// The image came from a dmabuf and the import owns every handle.
    ///
    /// Held rather than borrowed so the import cannot be dropped while the
    /// target still renders into it.
    Imported(Imported),
}

impl Backing {
    fn image(&self) -> vk::Image {
        match self {
            Self::Owned { image, .. } => *image,
            Self::Imported(i) => i.texture().image,
        }
    }

    fn view(&self) -> vk::ImageView {
        match self {
            Self::Owned { view, .. } => *view,
            Self::Imported(i) => i.texture().view,
        }
    }

    /// Destroy what this backing owns.
    ///
    /// ★ Only the `Owned` arm does anything: an imported backing's handles
    /// belong to the `Imported`, whose own `Drop` frees them. Freeing them
    /// here as well is the double free the enum exists to prevent, so the
    /// `Imported` arm is deliberately empty rather than accidentally omitted.
    ///
    /// # Safety
    /// Must be called at most once, and only while the device is alive.
    unsafe fn destroy(&self, dev: &ash::Device) {
        if let Self::Owned {
            image,
            view,
            memory,
        } = self
        {
            // SAFETY: the caller guarantees a live device and a single call.
            // Memory is freed AFTER the objects bound to it, which is the
            // ordering Vulkan requires.
            unsafe {
                dev.destroy_image_view(*view, None);
                dev.destroy_image(*image, None);
                dev.free_memory(*memory, None);
            }
        }
    }
}

pub struct Target {
    /// The device. See [`Pipelines`]'s field for why this is an `Arc`.
    gpu: std::sync::Arc<Gpu>,
    backing: Backing,
    /// Host-visible copy of the last frame — `None` for an imported target.
    ///
    /// ★★ AN IMPORTED TARGET MUST NOT BE COPIED EVERY FRAME. It is the
    /// scanout buffer; the display reads it directly, and copying it into host
    /// memory each frame is exactly the 12.0ms-of-a-12.1ms-frame cost this
    /// whole crate exists to remove. An OWNED target is off-screen, so a
    /// readback is the ONLY way to observe it and is always wanted.
    ///
    /// Tying this to the backing rather than to a flag means "a scanout target
    /// that silently pays for a readback" has no representation.
    readback: Option<(vk::Buffer, vk::DeviceMemory)>,
    /// Size in pixels.
    pub extent: vk::Extent2D,
    /// Colour format — must match the [`Pipelines`] used to draw into it.
    pub format: vk::Format,
}

impl Target {
    /// Create a `width` x `height` off-screen target in `format`.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] for any driver refusal; every one names the
    /// call that failed, because "could not create target" is not actionable.
    pub fn new(
        gpu: &std::sync::Arc<Gpu>,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<Self, KasaneError> {
        let backing = Self::owned_backing(gpu, width, height, format)?;
        Self::with_backing(gpu, backing, vk::Extent2D { width, height }, format)
    }

    /// Render into a dmabuf somebody else allocated — a scanout buffer.
    ///
    /// ── ★ THIS IS WHAT MAKES ZERO-COPY POSSIBLE ──────────────────────────
    /// [`Target::new`] allocates its own image, so anything drawn into it must
    /// be copied somewhere to be seen. This one renders DIRECTLY into the
    /// buffer the display scans out of: no shadow, no flush, no copy. It is
    /// the difference between nuri's measured 12.0 ms of a 12.1 ms frame and
    /// nothing at all.
    ///
    /// # Errors
    /// [`KasaneError::ModifierNotSupported`] if the device cannot RENDER INTO
    /// that layout — a different question from whether it can sample it, and
    /// checked against the right list by [`ImportUse`].
    pub fn from_dmabuf(
        gpu: &std::sync::Arc<Gpu>,
        fd: OwnedFd,
        geometry: Geometry,
        modifier: u64,
    ) -> Result<Self, KasaneError> {
        let imported = gpu.import_for(fd, geometry, modifier, ImportUse::RenderTarget)?;
        let extent = vk::Extent2D {
            width: geometry.width,
            height: geometry.height,
        };
        Self::with_backing(gpu, Backing::Imported(imported), extent, FORMAT)
    }

    /// Add the readback buffer to a backing and make a `Target`.
    ///
    /// ★ Shared by both constructors so the readback path cannot exist for one
    /// and not the other — an imported target that could not be screenshot
    /// would fail `ExportMem` only on the machine that has a real display.
    fn with_backing(
        gpu: &std::sync::Arc<Gpu>,
        backing: Backing,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> Result<Self, KasaneError> {
        // Only an owned target gets one — see the field's own note.
        let readback = match &backing {
            Backing::Owned { .. } => match Self::readback_buffer(gpu, extent) {
                Ok(pair) => Some(pair),
                Err(e) => {
                    // The backing is not owned by a `Target` yet, so nothing
                    // else will free it.
                    // SAFETY: built above, destroyed once, device alive.
                    unsafe { backing.destroy(&gpu.device) };
                    return Err(e);
                }
            },
            Backing::Imported(_) => None,
        };
        Ok(Self {
            gpu: std::sync::Arc::clone(gpu),
            backing,
            readback,
            extent,
            format,
        })
    }

    /// Allocate an image this target owns, plus its view.
    ///
    /// Self-cleaning: a failure part-way through destroys what it made, so a
    /// retrying compositor does not leak an image per attempt.
    fn owned_backing(
        gpu: &std::sync::Arc<Gpu>,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<Backing, KasaneError> {
        let dev = &gpu.device;
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            // ★ OPTIMAL TILING AND A COPY, NOT A LINEAR ATTACHMENT.
            // Rendering into a LINEAR host-visible image and mapping it would
            // be shorter, and it works on lavapipe. It does NOT work on the
            // hardware this exists for: NVIDIA does not advertise
            // COLOR_ATTACHMENT in `linearTilingFeatures`, so image creation
            // fails on plo's 3070 and succeeds in CI — the worst possible
            // split, where the test machine proves a path the real machine
            // cannot take.
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `image_info` and everything it borrows are locals.
        let image = unsafe { dev.create_image(&image_info, None) }
            .map_err(|e| driver("create_image(target)", e))?;

        let finish = || -> Result<(vk::DeviceMemory, vk::ImageView), KasaneError> {
            // SAFETY: the image was just created on this device.
            let reqs = unsafe { dev.get_image_memory_requirements(image) };
            let idx = gpu.memory_type(
                reqs.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                "DEVICE_LOCAL",
            )?;
            let alloc = vk::MemoryAllocateInfo::default()
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: `alloc` is a local; `idx` came from this device.
            let memory = unsafe { dev.allocate_memory(&alloc, None) }
                .map_err(|e| driver("allocate_memory(target)", e))?;
            // SAFETY: image and memory are both from this device, bound once.
            unsafe { dev.bind_image_memory(image, memory, 0) }
                .map_err(|e| driver("bind_image_memory(target)", e))?;

            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            // SAFETY: `view_info` names an image with memory bound above.
            let view = unsafe { dev.create_image_view(&view_info, None) }
                .map_err(|e| driver("create_image_view(target)", e))?;
            Ok((memory, view))
        };

        match finish() {
            Ok((memory, view)) => Ok(Backing::Owned {
                image,
                view,
                memory,
            }),
            Err(e) => {
                // SAFETY: live image; the memory either was never allocated or
                // is freed by the allocator's own failure path.
                unsafe { dev.destroy_image(image, None) };
                Err(e)
            }
        }
    }

    /// The host-visible buffer a finished frame is copied into.
    ///
    /// ★ TIGHTLY PACKED, deliberately. `cmd_copy_image_to_buffer` with
    /// `buffer_row_length: 0` means "rows are `width` texels", so the readback
    /// has no stride of its own to get wrong — unlike the imported side, where
    /// the DRIVER picks the stride and it must be asked for.
    fn readback_buffer(
        gpu: &std::sync::Arc<Gpu>,
        extent: vk::Extent2D,
    ) -> Result<(vk::Buffer, vk::DeviceMemory), KasaneError> {
        let dev = &gpu.device;
        let size = u64::from(extent.width) * u64::from(extent.height) * 4;
        let buf_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `buf_info` is a local.
        let buffer = unsafe { dev.create_buffer(&buf_info, None) }
            .map_err(|e| driver("create_buffer(readback)", e))?;

        let finish = || -> Result<vk::DeviceMemory, KasaneError> {
            // SAFETY: the buffer was just created on this device.
            let reqs = unsafe { dev.get_buffer_memory_requirements(buffer) };
            let idx = gpu.memory_type(
                reqs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                "HOST_VISIBLE | HOST_COHERENT",
            )?;
            let alloc = vk::MemoryAllocateInfo::default()
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: `alloc` is a local; `idx` came from this device.
            let memory = unsafe { dev.allocate_memory(&alloc, None) }
                .map_err(|e| driver("allocate_memory(readback)", e))?;
            // SAFETY: buffer and memory both from this device, bound once.
            unsafe { dev.bind_buffer_memory(buffer, memory, 0) }
                .map_err(|e| driver("bind_buffer_memory", e))?;
            Ok(memory)
        };

        match finish() {
            Ok(memory) => Ok((buffer, memory)),
            Err(e) => {
                // SAFETY: live buffer, nothing bound to it.
                unsafe { dev.destroy_buffer(buffer, None) };
                Err(e)
            }
        }
    }

    /// Clear to `clear`, run `draws`, copy the result back, and wait for it.
    ///
    /// ── ★ ONE SUBMIT, ONE FENCE, SYNCHRONOUS ─────────────────────────────
    /// A compositor wants a pipelined ring of command buffers and no CPU wait.
    /// This is not that, and deliberately: the ring is a correctness question
    /// about frame N-1's resources still being alive, and answering it before
    /// there is a single proven correct frame would mean debugging both at
    /// once. The ring is the next stage; the seam it plugs into is here.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] naming the failing call.
    pub fn draw(
        &self,
        pipes: &Pipelines,
        clear: [f32; 4],
        draws: &[Draw],
    ) -> Result<(), KasaneError> {
        // ★ The pipelines were COMPILED against a format. Drawing with a
        // mismatched one is undefined behaviour that a driver may or may not
        // report, so it is refused here where the two are both in scope.
        if pipes.format != self.format {
            return Err(KasaneError::Vulkan {
                call: "Target::draw",
                result: format!(
                    "pipelines were compiled for format {:#x} but the target \
                     is {:#x}; a mismatched attachment format is undefined \
                     behaviour the driver need not report",
                    pipes.format.as_raw(),
                    self.format.as_raw()
                ),
            });
        }

        // ★ REFUSED, NOT SILENTLY SHORTENED. Allocating past the pool's
        // capacity returns ERROR_OUT_OF_POOL_MEMORY mid-frame, which would
        // abandon a half-recorded command buffer; saying so here names the
        // real limit instead.
        let textures = draws
            .iter()
            .filter(|d| matches!(d, Draw::Texture { .. }))
            .count();
        if textures > Pipelines::MAX_TEXTURE_DRAWS as usize {
            return Err(KasaneError::Vulkan {
                call: "Target::draw",
                result: format!(
                    "{textures} textured draws exceeds MAX_TEXTURE_DRAWS \
                     ({}); raise the constant rather than dropping surfaces",
                    Pipelines::MAX_TEXTURE_DRAWS
                ),
            });
        }

        let dev = &self.gpu.device;
        // Reclaim last frame's descriptor sets. Safe because this function
        // waits on its fence before returning — see the field's own note for
        // what the command-buffer ring must change here.
        // SAFETY: the pool is live and no submitted work references its sets.
        unsafe {
            dev.reset_descriptor_pool(pipes.descriptor_pool, vk::DescriptorPoolResetFlags::empty())
        }
        .map_err(|e| driver("reset_descriptor_pool", e))?;

        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.gpu.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live and owned by this device.
        let cmds = unsafe { dev.allocate_command_buffers(&alloc) }
            .map_err(|e| driver("allocate_command_buffers", e))?;
        let cmd = cmds[0];

        let result = self.record_and_submit(cmd, pipes, clear, draws);
        // Freed whether or not recording worked — the pool is reset per frame
        // in the shipping path, but a test that leaks one buffer per call
        // exhausts the pool and fails somewhere unrelated.
        // SAFETY: `cmd` came from this pool and is not in flight (the submit
        // below waits on its fence before returning).
        unsafe { dev.free_command_buffers(self.gpu.command_pool, &cmds) };
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear command-recording sequence whose ORDER is the \
                  correctness: barrier in, begin rendering, draw, end, barrier \
                  out, copy. Splitting it puts that order behind call sites, \
                  which is exactly where a missing barrier hides. The one \
                  self-contained block (descriptor writes) is already extracted."
    )]
    fn record_and_submit(
        &self,
        cmd: vk::CommandBuffer,
        pipes: &Pipelines,
        clear: [f32; 4],
        draws: &[Draw],
    ) -> Result<(), KasaneError> {
        let dev = &self.gpu.device;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `cmd` is a fresh primary buffer from this device's pool.
        unsafe { dev.begin_command_buffer(cmd, &begin) }
            .map_err(|e| driver("begin_command_buffer", e))?;

        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);

        // ★ EVERY SAMPLED IMAGE REACHES SHADER_READ_ONLY_OPTIMAL FIRST, and
        // this must happen OUTSIDE the rendering scope — layout transitions
        // are illegal between `cmd_begin_rendering` and `cmd_end_rendering`.
        //
        // ★ FROM THE LAYOUT THE TEXTURE SAYS IT IS IN, not from `UNDEFINED`.
        // See `TextureRef::layout`: transitioning an uploaded texture from
        // UNDEFINED discards the pixels that were just uploaded, and lavapipe
        // hid it well enough that a single-colour upload test still passed.
        for d in draws {
            if let Draw::Texture { texture, .. } = *d {
                // Already where it needs to be — an uploaded texture, which
                // `write` leaves in SHADER_READ_ONLY_OPTIMAL. Issuing a
                // barrier from UNDEFINED here would DISCARD its pixels.
                if texture.layout == vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL {
                    continue;
                }
                let b = vk::ImageMemoryBarrier::default()
                    .old_layout(texture.layout)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(texture.image)
                    .subresource_range(whole);
                // SAFETY: recording into a live buffer; `b` is a local naming
                // an image the caller borrows for this call.
                unsafe {
                    dev.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::TOP_OF_PIPE,
                        vk::PipelineStageFlags::FRAGMENT_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &[b],
                    );
                }
            }
        }

        // UNDEFINED → COLOR_ATTACHMENT_OPTIMAL. The old contents are discarded,
        // which is correct: the render pass clears.
        self.barrier(
            cmd,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::AccessFlags::empty(),
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            whole,
        );

        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(self.backing.view())
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { float32: clear },
            });
        let attachments = [attachment];
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .layer_count(1)
            .color_attachments(&attachments);

        // SAFETY: every borrowed structure is a local; the view is live and in
        // the layout just transitioned to.
        unsafe { self.gpu.dyn_render.cmd_begin_rendering(cmd, &rendering) };

        // Viewport and scissor are dynamic so one pipeline serves any size.
        //
        // ★ The casts are lossless for every real output. `f32` holds integers
        // exactly to 2^24 = 16,777,216, and Vulkan's own
        // `maxViewportDimensions` is far below that on all known hardware
        // (65,536 at the extreme). A display wider than 16 million pixels
        // would lose precision here; it would also not fit in the driver's
        // limits, so the refusal comes first.
        #[allow(
            clippy::cast_precision_loss,
            reason = "exact below 2^24; maxViewportDimensions is orders of magnitude smaller"
        )]
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            #[allow(
                clippy::cast_precision_loss,
                reason = "exact below 2^24; Vulkan's own maxViewportDimensions \
                          is orders of magnitude smaller, so a display big \
                          enough to lose precision is refused by the driver first"
            )]
            width: self.extent.width as f32,
            #[allow(clippy::cast_precision_loss, reason = "see width")]
            height: self.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: self.extent,
        };
        // SAFETY: recording into a live buffer inside a rendering scope.
        unsafe {
            dev.cmd_set_viewport(cmd, 0, &[viewport]);
            dev.cmd_set_scissor(cmd, 0, &[scissor]);
        }

        for d in draws {
            match *d {
                Draw::Texture {
                    params,
                    texture,
                    filter,
                } => {
                    let set = Self::texture_descriptor(dev, pipes, texture, filter)?;
                    // SAFETY: the pipeline matches the attachment format
                    // (checked in `draw`), and the set matches the layout the
                    // pipeline was built with.
                    unsafe {
                        dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipes.textured);
                        dev.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipes.layout,
                            0,
                            &[set],
                            &[],
                        );
                        dev.cmd_push_constants(
                            cmd,
                            pipes.layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            params.bytes(),
                        );
                        dev.cmd_draw(cmd, 4, 1, 0, 0);
                    }
                }
                Draw::Solid(params) => {
                    // SAFETY: the pipeline was compiled for this attachment
                    // format (checked in `draw`), the push-constant range
                    // covers both stages, and the quad needs no vertex buffer.
                    unsafe {
                        dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipes.solid);
                        dev.cmd_push_constants(
                            cmd,
                            pipes.layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            params.bytes(),
                        );
                        // Four vertices, one instance: the triangle strip the
                        // vertex shader builds from `vertex_index`.
                        dev.cmd_draw(cmd, 4, 1, 0, 0);
                    }
                }
            }
        }

        // SAFETY: inside the scope opened above.
        unsafe { self.gpu.dyn_render.cmd_end_rendering(cmd) };

        if let Some((readback, _)) = self.readback {
            self.barrier(
                cmd,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::TRANSFER_READ,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                whole,
            );

            let region = vk::BufferImageCopy::default()
                // 0 means "tightly packed at the image's width" — see
                // `readback_buffer`.
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: self.extent.width,
                    height: self.extent.height,
                    depth: 1,
                });
            // SAFETY: image is in TRANSFER_SRC_OPTIMAL, the buffer is
            // width*height*4 bytes, and `region` is a local.
            unsafe {
                dev.cmd_copy_image_to_buffer(
                    cmd,
                    self.backing.image(),
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback,
                    &[region],
                );
            }
        } else {
            // ── ★ AN IMPORTED TARGET IS RELEASED, NOT COPIED ─────────────
            // Nothing is read back; the buffer's other user is the display (or
            // another process). GENERAL is the layout an external consumer of
            // a dmabuf expects, and the transition is what makes this frame's
            // writes visible to it.
            //
            // ★ NOT YET A FOREIGN-QUEUE RELEASE. Strictly this wants
            // `VK_EXT_queue_family_foreign` — an explicit ownership transfer to
            // `VK_QUEUE_FAMILY_FOREIGN_EXT` — which is M5's work and is why
            // `Gpu::queue_family` is carried but unread. Without it the
            // handover relies on the layout transition alone, which is enough
            // for a linear buffer on one device and is NOT a general
            // guarantee. `pending-kasane: foreign-queue release`.
            self.barrier(
                cmd,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::GENERAL,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::MEMORY_READ,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                whole,
            );
        }

        // SAFETY: recording is complete and balanced.
        unsafe { dev.end_command_buffer(cmd) }.map_err(|e| driver("end_command_buffer", e))?;

        // ★ A FENCE, NOT `device_wait_idle`. Waiting on the whole device would
        // also wait on any other work in flight, which in a compositor means
        // one slow client stalls every output.
        let fence_info = vk::FenceCreateInfo::default();
        // SAFETY: `fence_info` is a local.
        let fence = unsafe { dev.create_fence(&fence_info, None) }
            .map_err(|e| driver("create_fence", e))?;
        let cmd_bufs = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmd_bufs);

        // SAFETY: the queue and fence are from this device; `submit` borrows
        // `cmd_bufs`, a local that outlives the wait below.
        let submitted = unsafe { dev.queue_submit(self.gpu.queue, &[submit], fence) };
        let waited = submitted.and_then(|()| {
            // SAFETY: the fence was just submitted with.
            unsafe { dev.wait_for_fences(&[fence], true, u64::MAX) }
        });
        // SAFETY: the wait above returned, so the fence is no longer in use.
        // Destroyed on the error path too — a leaked fence per failed frame is
        // a slow exhaustion that presents far from its cause.
        unsafe { dev.destroy_fence(fence, None) };
        waited.map_err(|e| driver("submit/wait", e))
    }

    /// Allocate and write one descriptor set naming `texture` and a sampler.
    ///
    /// ★ ONE SET PER DRAW, from the pool reset at the top of this frame.
    /// Vulkan forbids updating a set that a submitted command buffer still
    /// references, so re-using a single set across the draws in a frame is
    /// undefined — and the symptom is invisible until two surfaces are on
    /// screen and one shows the other's contents.
    fn texture_descriptor(
        dev: &ash::Device,
        pipes: &Pipelines,
        texture: TextureRef,
        filter: Filter,
    ) -> Result<vk::DescriptorSet, KasaneError> {
        let layouts = [pipes.set_layout];
        let alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pipes.descriptor_pool)
            .set_layouts(&layouts);
        // SAFETY: `layouts` outlives the call; the pool is live.
        let sets = unsafe { dev.allocate_descriptor_sets(&alloc) }
            .map_err(|e| driver("allocate_descriptor_sets", e))?;
        let set = sets[0];

        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(texture.view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(pipes.sampler_for(filter))];
        // Binding 0 is the image and 1 is the sampler, matching what the WGSL
        // declares — naga lowers `texture_2d` and `sampler` to two
        // descriptors, not a combined one.
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        ];
        // SAFETY: every borrowed array is a local outliving the call, and the
        // set was allocated from this device.
        unsafe { dev.update_descriptor_sets(&writes, &[]) };
        Ok(set)
    }

    #[allow(clippy::too_many_arguments)]
    fn barrier(
        &self,
        cmd: vk::CommandBuffer,
        from: vk::ImageLayout,
        to: vk::ImageLayout,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
        src_stage: vk::PipelineStageFlags,
        dst_stage: vk::PipelineStageFlags,
        range: vk::ImageSubresourceRange,
    ) {
        let b = vk::ImageMemoryBarrier::default()
            .old_layout(from)
            .new_layout(to)
            .src_access_mask(src_access)
            .dst_access_mask(dst_access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.backing.image())
            .subresource_range(range);
        // SAFETY: recording into a live buffer; `b` is a local naming a live
        // image owned by this target.
        unsafe {
            self.gpu.device.cmd_pipeline_barrier(
                cmd,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[b],
            );
        }
    }

    /// Copy a rectangle of this target into host memory, on demand.
    ///
    /// ── ★ WHY THIS EXISTS SEPARATELY FROM THE READBACK BUFFER ────────────
    /// An OWNED target keeps a readback buffer filled by every `draw`, because
    /// it is off-screen and that is the only way to observe it. An IMPORTED
    /// target has none — copying a scanout buffer every frame is exactly the
    /// cost this crate removes. But a screenshot still has to work, so this
    /// pays for the copy EXPLICITLY, once, when somebody asks.
    ///
    /// Works on either backing, so a caller does not need to know which it
    /// has.
    ///
    /// # Errors
    /// [`KasaneError::OutOfBounds`] if the region leaves the target, or
    /// [`KasaneError::Vulkan`] naming a failing call.
    pub fn capture(&self, x: u32, y: u32, w: u32, h: u32) -> Result<Vec<u8>, KasaneError> {
        if x + w > self.extent.width || y + h > self.extent.height {
            return Err(KasaneError::OutOfBounds {
                x: x + w,
                y: y + h,
                width: self.extent.width,
                height: self.extent.height,
            });
        }
        let size = u64::from(w) * u64::from(h) * 4;
        let (buffer, memory) = Self::readback_buffer(
            &self.gpu,
            vk::Extent2D {
                width: w,
                height: h,
            },
        )?;

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);

        let recorded = self.gpu.submit_once(|cmd| {
            // ★ THE SOURCE LAYOUT IS `GENERAL`, not UNDEFINED. `Target::draw`
            // leaves an imported target in GENERAL and an owned one in
            // TRANSFER_SRC_OPTIMAL; UNDEFINED here would DISCARD the frame
            // that was just drawn, and the capture would come back as
            // whatever the driver felt like — which on a fast path is often
            // the right pixels, so the bug would look intermittent.
            let from = if self.readback.is_some() {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            } else {
                vk::ImageLayout::GENERAL
            };
            self.barrier(
                cmd,
                from,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::AccessFlags::MEMORY_WRITE,
                vk::AccessFlags::TRANSFER_READ,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                range,
            );

            let region = vk::BufferImageCopy::default()
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D {
                    x: x.cast_signed(),
                    y: y.cast_signed(),
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                });
            // SAFETY: the image is in TRANSFER_SRC_OPTIMAL, the buffer holds
            // w*h*4 bytes, and `region` is a local.
            unsafe {
                self.gpu.device.cmd_copy_image_to_buffer(
                    cmd,
                    self.backing.image(),
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer,
                    &[region],
                );
            }

            // Put an imported target back where the display expects it.
            if self.readback.is_none() {
                self.barrier(
                    cmd,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::TRANSFER_READ,
                    vk::AccessFlags::MEMORY_READ,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    range,
                );
            }
            Ok(())
        });

        let out = recorded.and_then(|()| {
            // SAFETY: HOST_VISIBLE by construction, the whole range, unmapped
            // before returning.
            let ptr = unsafe {
                self.gpu
                    .device
                    .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
            }
            .map_err(|e| driver("map_memory(capture)", e))?;
            let len = usize::try_from(size).unwrap_or(0);
            // SAFETY: `len` bytes were allocated and just written by the copy.
            let v = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) }.to_vec();
            // SAFETY: mapped above; `v` is a copy, so nothing points into it.
            unsafe { self.gpu.device.unmap_memory(memory) };
            Ok(v)
        });

        // Freed on every path — this buffer is per-capture, unlike the owned
        // target's permanent one.
        // SAFETY: the submit waited, so nothing references it.
        unsafe {
            self.gpu.device.destroy_buffer(buffer, None);
            self.gpu.device.free_memory(memory, None);
        }
        out
    }

    /// The pixel at `(x, y)` from the last [`Target::draw`], as the format's
    /// own byte order.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] if the memory cannot be mapped, or the
    /// coordinate is outside the target.
    pub fn read_pixel(&self, x: u32, y: u32) -> Result<[u8; 4], KasaneError> {
        if x >= self.extent.width || y >= self.extent.height {
            return Err(KasaneError::Vulkan {
                call: "Target::read_pixel",
                result: format!(
                    "({x}, {y}) is outside a {}x{} target",
                    self.extent.width, self.extent.height
                ),
            });
        }
        // ★ AN IMPORTED TARGET HAS NO READBACK, and saying so is better than
        // returning stale or zero pixels. Copying a scanout buffer every frame
        // is the cost this crate exists to remove, so the absence is by
        // design — see the `readback` field.
        let Some((_, readback_memory)) = self.readback else {
            return Err(KasaneError::Vulkan {
                call: "Target::read_pixel",
                result: "this target renders into an imported dmabuf and has no \
                         readback buffer, by design. Capturing one needs an \
                         explicit copy submit — `ExportMem`'s job, not built."
                    .to_owned(),
            });
        };
        let size = u64::from(self.extent.width) * u64::from(self.extent.height) * 4;
        // SAFETY: the memory is HOST_VISIBLE (selected in `readback_buffer`),
        // the range is the whole allocation, and it is unmapped before return.
        let ptr = unsafe {
            self.gpu
                .device
                .map_memory(readback_memory, 0, size, vk::MemoryMapFlags::empty())
        }
        .map_err(|e| driver("map_memory(readback)", e))?;

        let offset = (y as usize * self.extent.width as usize + x as usize) * 4;
        // SAFETY: `offset + 4 <= size` because x and y were bounds-checked
        // above and the buffer is tightly packed at 4 bytes per texel.
        let px = unsafe {
            let base = ptr.cast::<u8>().add(offset);
            [
                base.read(),
                base.add(1).read(),
                base.add(2).read(),
                base.add(3).read(),
            ]
        };
        // SAFETY: mapped on the line above, and nothing holds a reference into
        // it — `px` is a copy.
        unsafe { self.gpu.device.unmap_memory(readback_memory) };
        Ok(px)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        let dev = &self.gpu.device;
        // SAFETY: every handle came from this device and is destroyed once.
        // The backing frees only what it OWNS — an imported one frees nothing
        // here, because its `Imported` does it. Memory is freed after the
        // objects bound to it, which is the ordering Vulkan requires.
        unsafe {
            self.backing.destroy(dev);
            if let Some((buffer, memory)) = self.readback {
                dev.destroy_buffer(buffer, None);
                dev.free_memory(memory, None);
            }
        }
    }
}

/// A texture uploaded from host memory — an `wl_shm` client surface.
///
/// ── ★ WHY THIS EXISTS ALONGSIDE `Imported` ───────────────────────────────
/// Not every client hands over a dmabuf. `wl_shm` clients — which is most
/// simple toolkits, and every client at all before it negotiates dmabuf —
/// deliver a shared-memory buffer, and the compositor must get those bytes
/// onto the GPU itself. That is a genuinely different operation from importing
/// a dmabuf: the memory is DEVICE_LOCAL and written through a staging buffer,
/// rather than shared and never copied.
///
/// ★ THE COPY HERE IS NOT THE COPY kasane EXISTS TO REMOVE. That one is the
/// per-frame scanout flush, paid for every pixel of the output on every frame.
/// This is paid once per client BUFFER UPDATE, for that client's damage only,
/// and there is no way around it — the bytes start in host memory.
pub struct Uploaded {
    gpu: std::sync::Arc<Gpu>,
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    /// Reused across updates rather than reallocated per frame — a client
    /// that redraws at 60 Hz would otherwise allocate and free a staging
    /// buffer 60 times a second.
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    /// Size in pixels.
    pub geometry: Geometry,
}

impl Uploaded {
    /// This texture as something a [`Draw::Texture`] can sample.
    #[must_use]
    pub fn texture(&self) -> TextureRef {
        TextureRef {
            image: self.image,
            view: self.view,
            // `write` leaves the image here, so the recorder needs no barrier
            // at all — and must not issue one from `UNDEFINED`, which would
            // discard the pixels it just uploaded.
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    }

    /// Replace a rectangle of this texture with `data`.
    ///
    /// `data` is the rectangle's own tightly-packed BGRA rows — `region.w * 4`
    /// bytes per row — not a window into a larger buffer.
    ///
    /// # Errors
    /// [`KasaneError::OutOfBounds`] if the region leaves the texture,
    /// [`KasaneError::Vulkan`] naming a failing call, or a size mismatch
    /// reported as its own message.
    pub fn write(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) -> Result<(), KasaneError> {
        if x + w > self.geometry.width || y + h > self.geometry.height {
            return Err(KasaneError::OutOfBounds {
                x: x + w,
                y: y + h,
                width: self.geometry.width,
                height: self.geometry.height,
            });
        }
        let needed = w as usize * h as usize * 4;
        if data.len() < needed {
            // ★ REFUSED, NOT PADDED. Uploading a short buffer would put
            // whatever the staging memory held last into the client's window —
            // the previous frame, or another client's pixels.
            return Err(KasaneError::Vulkan {
                call: "Uploaded::write",
                result: format!(
                    "{w}x{h} needs {needed} bytes and only {} were given; \
                     uploading anyway would show stale staging memory",
                    data.len()
                ),
            });
        }

        let dev = &self.gpu.device;
        // SAFETY: the staging memory is HOST_VISIBLE | HOST_COHERENT (chosen
        // in `Gpu::upload_texture`), the range is within it, and it is
        // unmapped before returning.
        let ptr = unsafe {
            dev.map_memory(
                self.staging_memory,
                0,
                needed as u64,
                vk::MemoryMapFlags::empty(),
            )
        }
        .map_err(|e| driver("map_memory(staging)", e))?
        .cast::<u8>();
        // SAFETY: `needed` bytes were allocated for the whole texture, which
        // is at least this region, and `data` holds at least `needed`.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, needed) };
        // HOST_COHERENT, so no explicit flush before unmapping.
        // SAFETY: mapped on the line above; nothing holds a reference into it.
        unsafe { dev.unmap_memory(self.staging_memory) };

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);

        self.gpu.submit_once(|cmd| {
            // ★ UNDEFINED → TRANSFER_DST. The old contents of the REGION are
            // about to be overwritten, so discarding them is correct — and
            // naming the previous layout would mean tracking it across every
            // frame for no gain, since the whole region is written.
            Self::layout(
                &self.gpu,
                cmd,
                self.image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                range,
            );

            let region = vk::BufferImageCopy::default()
                // 0 means "rows are `imageExtent.width` texels", which is what
                // the caller's contract says `data` is.
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D {
                    x: x.cast_signed(),
                    y: y.cast_signed(),
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                });
            // SAFETY: the image is in TRANSFER_DST_OPTIMAL, the staging buffer
            // holds `needed` bytes, and `region` is a local.
            unsafe {
                dev.cmd_copy_buffer_to_image(
                    cmd,
                    self.staging,
                    self.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }

            // ★ AND BACK TO SHADER_READ_ONLY, here rather than at draw time.
            // A texture left in TRANSFER_DST would be sampled in the wrong
            // layout — undefined, and on real hardware it reads as garbage
            // rather than an error.
            Self::layout(
                &self.gpu,
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::SHADER_READ,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                range,
            );
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn layout(
        gpu: &Gpu,
        cmd: vk::CommandBuffer,
        image: vk::Image,
        from: vk::ImageLayout,
        to: vk::ImageLayout,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
        src_stage: vk::PipelineStageFlags,
        dst_stage: vk::PipelineStageFlags,
        range: vk::ImageSubresourceRange,
    ) {
        let b = vk::ImageMemoryBarrier::default()
            .old_layout(from)
            .new_layout(to)
            .src_access_mask(src_access)
            .dst_access_mask(dst_access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        // SAFETY: recording into a live buffer; `b` is a local naming a live
        // image this type owns.
        unsafe {
            gpu.device.cmd_pipeline_barrier(
                cmd,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[b],
            );
        }
    }
}

impl Drop for Uploaded {
    fn drop(&mut self) {
        let dev = &self.gpu.device;
        // SAFETY: every handle came from this device and is destroyed once.
        // View before image, memory after what was bound to it.
        unsafe {
            dev.destroy_image_view(self.view, None);
            dev.destroy_image(self.image, None);
            dev.free_memory(self.memory, None);
            dev.destroy_buffer(self.staging, None);
            dev.free_memory(self.staging_memory, None);
        }
    }
}

/// A dmabuf this process exported, plus the Vulkan objects backing it.
///
/// Borrows the [`Gpu`], so it cannot outlive the device that must destroy it —
/// the ordering is enforced by the compiler rather than by a comment.
pub struct Exported {
    /// The device. See [`Pipelines`]'s field for why this is an `Arc`.
    gpu: std::sync::Arc<Gpu>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    /// The kernel dmabuf. `Option` because a type with a `Drop` impl cannot be
    /// destructured — the fd has to be TAKEN, and taking it must leave
    /// something well-defined behind. `None` simply means the caller already
    /// owns it, and `Drop` then closes nothing.
    fd: Option<OwnedFd>,
    pub geometry: Geometry,
}

impl Exported {
    /// Take the dmabuf out, leaving the Vulkan objects to be dropped normally.
    ///
    /// ★ This ordering is the interesting part, not a formality: once the fd is
    /// out, the exporting image and memory can be destroyed and the buffer
    /// stays alive, because a dmabuf is refcounted by the KERNEL. That is
    /// exactly the property a compositor depends on — a client's buffer must
    /// outlive the client's own frame bookkeeping.
    /// Repaint a rectangle of this buffer, through the driver's own stride.
    ///
    /// ★ Exists so a test can build a buffer with DISTINGUISHABLE REGIONS. A
    /// uniformly filled buffer cannot prove that a source rectangle is
    /// honoured — every crop of it looks the same — so "the UVs are ignored"
    /// and "the UVs work" produce identical pixels.
    ///
    /// Uses `Geometry::byte_offset`, so the row pitch is the one the driver
    /// reported rather than `width * 4`. That distinction is the whole reason
    /// `Geometry` carries a stride.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] if the memory cannot be mapped, or
    /// [`KasaneError::OutOfBounds`] if the rectangle leaves the buffer.
    pub fn fill_rect(
        &mut self,
        x0: u32,
        y0: u32,
        w: u32,
        h: u32,
        colour: [u8; 4],
    ) -> Result<(), KasaneError> {
        if x0 + w > self.geometry.width || y0 + h > self.geometry.height {
            return Err(KasaneError::OutOfBounds {
                x: x0 + w,
                y: y0 + h,
                width: self.geometry.width,
                height: self.geometry.height,
            });
        }
        let size = self.geometry.min_size();
        // SAFETY: the memory was allocated HOST_VISIBLE | HOST_COHERENT by
        // `export_linear`, and it is unmapped before returning.
        let ptr = unsafe {
            self.gpu
                .device
                .map_memory(self.memory, 0, size, vk::MemoryMapFlags::empty())
        }
        .map_err(|e| driver("map_memory(fill_rect)", e))?
        .cast::<u8>();

        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let Some(off) = self.geometry.byte_offset(x, y) else {
                    continue;
                };
                if off + 4 > size {
                    continue;
                }
                // SAFETY: `off + 4 <= size` is checked directly above, and
                // `ptr` maps exactly `size` bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        colour.as_ptr(),
                        ptr.add(usize::try_from(off).unwrap_or(0)),
                        4,
                    );
                }
            }
        }
        // HOST_COHERENT, so no explicit flush before unmapping.
        // SAFETY: mapped on the line above and not referenced after.
        unsafe { self.gpu.device.unmap_memory(self.memory) };
        Ok(())
    }

    pub fn take_fd(&mut self) -> Option<OwnedFd> {
        self.fd.take()
    }
}

impl Drop for Exported {
    fn drop(&mut self) {
        // SAFETY: both handles came from `gpu.device` and are destroyed once.
        // The dmabuf fd keeps the underlying buffer alive independently, which
        // is what makes it legal to import from it after this is dropped.
        unsafe {
            self.gpu.device.destroy_image(self.image, None);
            self.gpu.device.free_memory(self.memory, None);
        }
    }
}

/// A dmabuf imported as an image, with its pixels readable.
pub struct Imported {
    /// The device. See [`Pipelines`]'s field for why this is an `Arc`.
    gpu: std::sync::Arc<Gpu>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    /// A view of the whole image, so it can be SAMPLED.
    ///
    /// ★ Created at import time rather than on demand. A view is cheap and
    /// immutable, and creating one per frame would put a driver call on the
    /// hot path to produce a value that never changes. It also means
    /// [`Imported::texture`] cannot fail, so a draw list can be built without
    /// error handling per surface.
    view: vk::ImageView,
    pub geometry: Geometry,
}

impl Imported {
    /// The DRM modifier the driver says this image actually has.
    ///
    /// ★ A DIAGNOSTIC FOR ONE SPECIFIC DOUBT. The validation layer reported
    /// that a view on this image "has no supported format features", while our
    /// own query says the requested modifier has plenty. Exactly one of three
    /// things is true: the image has a different modifier than we asked for
    /// (our bug), the layer cannot determine it (a layer limitation), or our
    /// query reads a different list than the layer does. This answers the
    /// first, which is the only one that would be a defect in kasane.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] if the driver refuses the query.
    pub fn actual_modifier(&self) -> Result<u64, KasaneError> {
        let loader =
            ash::ext::image_drm_format_modifier::Device::new(&self.gpu.instance, &self.gpu.device);
        let mut props = vk::ImageDrmFormatModifierPropertiesEXT::default();
        // SAFETY: live device and image; `props` is a local outliving the call.
        unsafe { loader.get_image_drm_format_modifier_properties(self.image, &mut props) }
            .map_err(|e| driver("get_image_drm_format_modifier_properties", e))?;
        Ok(props.drm_format_modifier)
    }

    /// This buffer as something a [`Draw::Texture`] can sample.
    #[must_use]
    pub fn texture(&self) -> TextureRef {
        TextureRef {
            image: self.image,
            view: self.view,
            // ★ UNDEFINED IS CORRECT HERE, unlike for an uploaded texture.
            // The pixels live in memory the EXPORTER wrote; this image is a
            // view onto it and has never been rendered to by us, so there is
            // nothing of ours to discard. Naming the real previous layout is
            // impossible anyway — the buffer came from another process that
            // never told us.
            layout: vk::ImageLayout::UNDEFINED,
        }
    }

    /// Read one pixel as B,G,R,A — the DRM `ARGB8888` byte order.
    ///
    /// # Errors
    /// [`KasaneError::OutOfBounds`] outside the buffer, or a map failure.
    pub fn pixel(&self, x: u32, y: u32) -> Result<[u8; 4], KasaneError> {
        let off = self
            .geometry
            .byte_offset(x, y)
            .ok_or(KasaneError::OutOfBounds {
                x,
                y,
                width: self.geometry.width,
                height: self.geometry.height,
            })?;
        let size = self.geometry.min_size();
        // SAFETY: the memory was selected HOST_VISIBLE, the range is within the
        // allocation, and no other mapping is outstanding.
        let ptr = unsafe {
            self.gpu
                .device
                .map_memory(self.memory, 0, size, vk::MemoryMapFlags::empty())
        }
        .map_err(|e| driver("map_memory(import)", e))?
        .cast::<u8>();
        let mut out = [0u8; 4];
        // SAFETY: `off + 4 <= size` because `byte_offset` bounds x and y and
        // `min_size` covers the last row; `ptr` maps `size` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                ptr.add(usize::try_from(off).unwrap_or(0)),
                out.as_mut_ptr(),
                4,
            );
            self.gpu.device.unmap_memory(self.memory);
        }
        Ok(out)
    }
}

impl Drop for Imported {
    fn drop(&mut self) {
        // SAFETY: every handle came from `gpu.device` and is destroyed once.
        // The view goes FIRST — it is a child of the image.
        unsafe {
            self.gpu.device.destroy_image_view(self.view, None);
            self.gpu.device.destroy_image(self.image, None);
            self.gpu.device.free_memory(self.memory, None);
        }
    }
}

/// `OwnedFd` exposes the raw fd only through `AsRawFd`; this keeps the import
/// path from importing that trait into the safe half of the crate.
trait AsRawFdCompat {
    fn as_raw_fd_compat(&self) -> i32;
}

impl AsRawFdCompat for OwnedFd {
    fn as_raw_fd_compat(&self) -> i32 {
        use std::os::fd::AsRawFd as _;
        self.as_raw_fd()
    }
}

// ── ★ M1: THE TILED PATH ────────────────────────────────────────────────────
//
// M0 moved a LINEAR buffer, which a CPU can address. A real client buffer from
// a GPU is TILED: its bytes are arranged in a vendor-specific order that only
// that GPU can decode, and the arrangement is named by a DRM FORMAT MODIFIER.
//
// That is the whole difference between the two milestones, and it is why M0
// could not have been the end. `nuri` maps a linear buffer and copies it —
// measured at `gather_us 693 952` against `frame_us 3 825`. A tiled buffer
// cannot be mapped and copied at all; it has to be handed to the GPU as an
// image, which is exactly what makes the copy disappear rather than get faster.

impl Gpu {
    /// Which DRM format modifiers this device can IMPORT for [`FORMAT`].
    ///
    /// ★ Asked of the driver, never assumed. A hard-coded modifier list is how
    /// a compositor ends up advertising a layout the GPU cannot read, and the
    /// failure lands in the client as a black window rather than in us as an
    /// error.
    ///
    /// Filtered to modifiers usable as a SAMPLED image, because sampling is
    /// what compositing does — a modifier the device can only render to is of
    /// no use for importing somebody else's buffer.
    /// Modifiers this device can SAMPLE — the client-surface question.
    ///
    /// Kept as its own name because it is what the dmabuf global advertises,
    /// and that global is specifically about what clients may hand us.
    #[must_use]
    pub fn importable_modifiers(&self) -> Vec<u64> {
        self.modifiers_for(ImportUse::Surface)
    }

    /// Modifiers this device can RENDER INTO.
    ///
    /// ★ NOT THE SAME LIST as [`Gpu::importable_modifiers`], and assuming it
    /// is would be a silent error: a driver may sample a layout it cannot use
    /// as a colour attachment. Measured per device rather than reasoned about.
    #[must_use]
    pub fn renderable_modifiers(&self) -> Vec<u64> {
        self.modifiers_for(ImportUse::RenderTarget)
    }

    /// Every modifier this device reports, with its raw feature bits.
    ///
    /// ★ A DIAGNOSTIC, not a decision input. When the validation layer and our
    /// own filter disagree about what a modifier supports, the only way to
    /// tell which is wrong is to print what the driver actually said.
    #[must_use]
    pub fn modifier_features(&self) -> Vec<(u64, u32)> {
        self.query_modifiers()
            .into_iter()
            .map(|m| {
                (
                    m.drm_format_modifier,
                    m.drm_format_modifier_tiling_features.as_raw(),
                )
            })
            .collect()
    }

    /// The modifiers this device offers for one purpose.
    /// Every modifier this device reports for [`FORMAT`], raw.
    ///
    /// ★ ONE QUERY, so the filter and the diagnostic can never disagree about
    /// what the driver said — which matters precisely when they seem to.
    fn query_modifiers(&self) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut props = vk::FormatProperties2::default().push_next(&mut list);
        // SAFETY: live instance and physical device; `props` and the struct it
        // chains are locals outliving the call. This first call fills only the
        // COUNT, which is the required two-call shape.
        unsafe {
            self.instance
                .get_physical_device_format_properties2(self.physical, FORMAT, &mut props);
        }
        let n = list.drm_format_modifier_count as usize;
        if n == 0 {
            return Vec::new();
        }
        let mut store = vec![vk::DrmFormatModifierPropertiesEXT::default(); n];
        {
            // Scoped so both wrappers die here: `props` mutably borrows
            // `list`, `list` mutably borrows `store`, and `store` cannot be
            // read while either is alive.
            let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
                .drm_format_modifier_properties(&mut store);
            let mut props = vk::FormatProperties2::default().push_next(&mut list);
            // SAFETY: as above; `store` is large enough because it was sized
            // from the count the first call reported.
            unsafe {
                self.instance.get_physical_device_format_properties2(
                    self.physical,
                    FORMAT,
                    &mut props,
                );
            }
        }
        // `n` from the first query is the right bound: the second call fills
        // at most that many, and re-reading the count would need the wrappers
        // to still be alive.
        store.truncate(n);
        store
    }

    /// The modifiers this device offers for one purpose.
    fn modifiers_for(&self, purpose: ImportUse) -> Vec<u64> {
        self.query_modifiers()
            .iter()
            .filter(|m| {
                // Single-plane only: multi-plane formats are a YUV concern and
                // this crate composites BGRA. `accepts` refuses them on the
                // nuri side for the same reason.
                m.drm_format_modifier_plane_count == 1
                    && m.drm_format_modifier_tiling_features
                        .contains(purpose.required_feature())
            })
            .map(|m| m.drm_format_modifier)
            .collect()
    }

    /// Import a dmabuf whose layout is described by `modifier`, as a SAMPLED
    /// image the GPU reads in place.
    ///
    /// ★ THE POINT OF THE WHOLE CRATE. Nothing here maps, copies or touches a
    /// pixel: the fd becomes device memory, the memory backs an image, and the
    /// image is sampled by the GPU during compositing. `Cost::cpu_bytes_per_frame`
    /// for this route is zero by construction, not by measurement.
    ///
    /// Consumes `fd`: Vulkan takes ownership on success, and the signature says
    /// so rather than leaving a double-close to discipline.
    ///
    /// # Errors
    /// A Vulkan failure, or no memory type permitted by both the image's
    /// requirements and the driver's answer for this particular fd.
    pub fn import_tiled(
        self: &std::sync::Arc<Self>,
        fd: OwnedFd,
        geometry: Geometry,
        modifier: u64,
    ) -> Result<Imported, KasaneError> {
        self.import_for(fd, geometry, modifier, ImportUse::Surface)
    }

    /// Import a dmabuf for a stated PURPOSE.
    ///
    /// ★ ONE ENGINE, TWO FACES. A render target and a client surface differ in
    /// exactly two values — the image usage and the format feature a modifier
    /// must carry — and [`ImportUse`] derives both from one choice, so the
    /// pair cannot disagree. Copying this function to change two flags would
    /// have been the second copy of ~120 lines, and the copy that drifts.
    ///
    /// # Errors
    /// [`KasaneError::ModifierNotSupported`] if the modifier is not one this
    /// device offers FOR THAT PURPOSE, or [`KasaneError::Vulkan`] naming the
    /// failing call.
    pub fn import_for(
        self: &std::sync::Arc<Self>,
        fd: OwnedFd,
        geometry: Geometry,
        modifier: u64,
        purpose: ImportUse,
    ) -> Result<Imported, KasaneError> {
        // ── ★ VALIDATE THE MODIFIER OURSELVES ────────────────────────────
        //
        // Measured on plo's RTX 3070: `vkCreateImage` ACCEPTS
        // `DRM_FORMAT_MOD_INVALID`. The test that expected the driver to
        // refuse it FAILED on real hardware — so the driver will not catch an
        // exporter/importer disagreement, and an unchecked import samples a
        // layout nobody agreed on and paints structured noise.
        //
        // This is the same rule `nuri::accepts` applies on the CPU side:
        // refuse at the boundary rather than guess. It is the only place that
        // still knows which buffer it was.
        // ★ Against the list for THIS PURPOSE. Checking a render target
        // against the sampled list would pass on a modifier the device cannot
        // render into, and the driver would not say so.
        let offered = self.modifiers_for(purpose);
        if !offered.contains(&modifier) {
            return Err(KasaneError::ModifierNotSupported { modifier, offered });
        }

        let raw = fd.as_raw_fd_compat();
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: `raw` is a live dmabuf fd we still own; the query does not
        // consume it.
        unsafe {
            self.ext_fd
                .get_memory_fd_properties(HANDLE, raw, &mut fd_props)
        }
        .map_err(|e| driver("get_memory_fd_properties(tiled)", e))?;

        // The plane layout, as the EXPORTER reported it. Passing our own guess
        // here is how a correct buffer is read as diagonal noise: stride is the
        // driver's business and a tiled stride is not width * 4.
        let planes = [vk::SubresourceLayout {
            offset: geometry.offset,
            size: 0,
            row_pitch: geometry.stride,
            array_pitch: 0,
            depth_pitch: 0,
        }];
        let mut drm = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(modifier)
            .plane_layouts(&planes);
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE);
        let ici = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .push_next(&mut drm)
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(vk::Extent3D {
                width: geometry.width,
                height: geometry.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            // ★ DRM_FORMAT_MODIFIER_EXT, not LINEAR and not OPTIMAL. The
            // layout is the one the modifier names; claiming OPTIMAL would let
            // the driver assume its own arrangement and read somebody else's.
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            // SAMPLED, because compositing reads it. TRANSFER_SRC as well so a
            // screenshot can copy it out without a second import.
            .usage(purpose.usage())
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            // UNDEFINED, not PREINITIALIZED: the contents come from another
            // process and we make no claim about them until the layout
            // transition. PREINITIALIZED would assert host-visible contents we
            // never wrote.
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: locals outlive the call; both chained structs are live.
        let image = unsafe { self.device.create_image(&ici, None) }
            .map_err(|e| driver("create_image(tiled import)", e))?;

        let make = || -> Result<vk::DeviceMemory, KasaneError> {
            // SAFETY: live image on this device.
            let reqs = unsafe { self.device.get_image_memory_requirements(image) };
            let mask = reqs.memory_type_bits & fd_props.memory_type_bits;
            // ★ NO HOST_VISIBLE REQUIREMENT. A tiled buffer lives in device
            // memory and is not meant to be mapped — demanding HOST_VISIBLE
            // here is what would quietly force the import back onto a
            // CPU-readable heap and undo the whole milestone.
            let idx = self.memory_type(mask, vk::MemoryPropertyFlags::empty(), "any")?;
            let raw = fd.into_raw_fd();
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(HANDLE)
                .fd(raw);
            // Dedicated allocation: an imported dmabuf backs exactly one image,
            // and saying so lets the driver skip suballocation bookkeeping it
            // cannot honour for foreign memory anyway.
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let ai = vk::MemoryAllocateInfo::default()
                .push_next(&mut import)
                .push_next(&mut dedicated)
                .allocation_size(reqs.size)
                .memory_type_index(idx);
            // SAFETY: locals outlive the call; `raw` is a valid dmabuf fd whose
            // ownership passes to Vulkan on success.
            let memory = unsafe { self.device.allocate_memory(&ai, None) }.map_err(|e| {
                // On failure Vulkan did NOT take the fd, so we close it or leak
                // one per failed import.
                // SAFETY: `raw` is still ours because the call failed.
                drop(unsafe { OwnedFd::from_raw_fd(raw) });
                driver("allocate_memory(tiled import)", e)
            })?;
            // SAFETY: live image and memory, nothing else bound.
            unsafe { self.device.bind_image_memory(image, memory, 0) }
                .map_err(|e| driver("bind_image_memory(tiled import)", e))?;
            Ok(memory)
        };

        match make() {
            Ok(memory) => {
                // The view must be made AFTER the memory is bound: a view of
                // an unbound image is invalid, and the driver need not say so.
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(FORMAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    );
                // SAFETY: `view_info` is a local naming an image with memory
                // bound on the line above.
                match unsafe { self.device.create_image_view(&view_info, None) } {
                    Ok(view) => Ok(Imported {
                        gpu: std::sync::Arc::clone(self),
                        image,
                        memory,
                        view,
                        geometry,
                    }),
                    Err(e) => {
                        // SAFETY: both are live and neither is referenced yet.
                        unsafe {
                            self.device.destroy_image(image, None);
                            self.device.free_memory(memory, None);
                        }
                        Err(driver("create_image_view(import)", e))
                    }
                }
            }
            Err(e) => {
                // SAFETY: live image; memory was never bound on this path.
                unsafe { self.device.destroy_image(image, None) };
                Err(e)
            }
        }
    }
}

// ── ★ M4: SELECT THE GPU THAT DRIVES THE DISPLAY ────────────────────────────
//
// `Gpu::open` takes the first device that can import a dmabuf, preferring a
// non-CPU one. On a single-GPU machine that is right by luck. On a machine
// with two — a laptop with integrated plus discrete, or plo where llvmpipe
// enumerates alongside the RTX 3070 — "first" is not an answer: compositing
// must happen on the device that owns the KMS node the seat scans out through,
// or every frame crosses the PCIe bus twice for no reason.
//
// `VK_EXT_physical_device_drm` reports each device's DRM major:minor, which is
// exactly what `fstat` on the compositor's `DrmDeviceFd` gives. Matching the
// two is the only way to say "this GPU, the one the display is on" without
// guessing from a vendor string.

/// A DRM device node, as a major:minor pair.
///
/// ★ From `st_rdev`, not from a path. `/dev/dri/card1` is a name that can
/// change between boots; 226:1 is what the kernel actually keyed it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmNode {
    pub major: i64,
    pub minor: i64,
}

impl DrmNode {
    /// The node a raw device fd refers to.
    ///
    /// # Errors
    /// Returns `None` if the fd cannot be stat'd — a closed or invalid fd.
    #[must_use]
    pub fn of_fd(fd: std::os::fd::RawFd) -> Option<Self> {
        // SAFETY: `fstat` writes into a fully-owned zeroed struct and reads
        // only the fd. A bad fd is reported by the return value, not by
        // undefined behaviour.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &raw mut st) } != 0 {
            return None;
        }
        let rdev = st.st_rdev;
        Some(Self {
            major: i64::from(libc::major(rdev)),
            minor: i64::from(libc::minor(rdev)),
        })
    }
}

impl Gpu {
    /// This device's DRM nodes, if it reports any.
    ///
    /// Returns `(primary, render)`. A device with no DRM node — llvmpipe, or a
    /// GPU with no display attached — reports `None` for both, which is a
    /// legitimate answer and not an error.
    #[must_use]
    pub fn drm_nodes(&self) -> (Option<DrmNode>, Option<DrmNode>) {
        // ★ NOT ASKED is not NO NODE. Without the extension the properties
        // struct keeps its zeroes and would read as a device with no primary
        // node — a plausible answer that happens to be a lie.
        if !self.has_drm {
            return (None, None);
        }
        let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
        // SAFETY: live instance and physical device; `props` and the struct it
        // chains are locals outliving the call.
        unsafe {
            self.instance
                .get_physical_device_properties2(self.physical, &mut props);
        }
        let primary = drm.has_primary.eq(&vk::TRUE).then_some(DrmNode {
            major: drm.primary_major,
            minor: drm.primary_minor,
        });
        let render = drm.has_render.eq(&vk::TRUE).then_some(DrmNode {
            major: drm.render_major,
            minor: drm.render_minor,
        });
        (primary, render)
    }

    /// Does this device drive `node`?
    ///
    /// Either DRM node counts: a compositor may hold the primary node for KMS
    /// or the render node for offscreen work, and both name the same GPU.
    #[must_use]
    pub fn drives(&self, node: DrmNode) -> bool {
        let (primary, render) = self.drm_nodes();
        primary == Some(node) || render == Some(node)
    }
}

/// Refuse to pass by skipping, when the caller said a GPU must be there.
///
/// ── ★ A SKIPPED TEST REPORTS `ok` ────────────────────────────────────────
/// Every GPU test here ends `eprintln!("SKIP: …"); return;` when no device is
/// available, and `cargo test` then prints `ok` for it. That is right for a
/// developer laptop and WRONG for CI: the suite went green on the build
/// machine for weeks while exercising none of the Vulkan path, because
/// lavapipe was being rejected by a bug (see `Gpu::open`). A green suite that
/// ran nothing is worse than a red one.
///
/// With `OMOYA_REQUIRE_GPU=1` a skip becomes a panic naming the reason, so an
/// environment that is SUPPOSED to have a device says so when it does not.
#[cfg(test)]
fn skip_or_panic(what: &str, why: &dyn std::fmt::Display) {
    assert!(
        std::env::var_os("OMOYA_REQUIRE_GPU").is_none(),
        "{what}: no GPU pipe, but OMOYA_REQUIRE_GPU is set — this \
         environment is supposed to have a device. Reason: {why}"
    );
    eprintln!("SKIP: {what} — {why}");
}

#[cfg(test)]
#[allow(
    clippy::items_after_statements,
    reason = "a test's constants are declared where they are used, next to the \
              assertion that reads them — moving them to the top of the \
              function separates a value from its explanation"
)]
mod tests {
    use super::*;

    /// ★ M0's DONE-PREDICATE: export a dmabuf, import it back, read the pixel.
    ///
    /// This is the whole milestone. It proves the external-memory machinery
    /// end to end — a real kernel dmabuf fd, a real `vkBindImageMemory` of
    /// imported memory — without GBM and without a live Wayland client.
    ///
    /// It SKIPS rather than fails when there is no Vulkan, and says so: a
    /// machine with no loader is a legitimate state (that is the whole point of
    /// `Unavailable`), and a test that failed there would make the fallback
    /// look like a defect.
    #[test]
    fn a_dmabuf_round_trips_through_vulkan_and_the_pixel_survives() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        eprintln!(
            "kasane M0 running on {:?} (cpu={})",
            gpu.device_name, gpu.is_cpu
        );

        // A distinctive pattern: not 0, not 0xff, and asymmetric across the
        // four channels so a byte-order mistake cannot pass.
        const FILL: [u8; 4] = [0x11, 0x22, 0x33, 0xff];

        let exported = gpu
            .export_linear(64, 32, FILL)
            .expect("export a linear dmabuf");
        let geometry = exported.geometry;

        // ★ The driver's stride, asserted to be AT LEAST width*4 — and
        // deliberately not asserted EQUAL, because padding is legal and
        // assuming otherwise is the exact defect `Geometry` exists to prevent.
        assert!(
            geometry.stride >= u64::from(geometry.width) * 4,
            "stride {} cannot hold {} pixels",
            geometry.stride,
            geometry.width
        );

        // ★ Take the fd, then DESTROY the export-side Vulkan objects before
        // importing. The dmabuf keeps the buffer alive on its own — kernel
        // refcounting — which is the property that makes a client's buffer
        // usable after the client's own frame bookkeeping has moved on. If
        // this ordering were wrong the import below would read freed memory,
        // so the test is also a check on that assumption.
        let mut exported = exported;
        let fd = exported
            .take_fd()
            .expect("the export must yield its dmabuf");
        drop(exported);

        let imported = gpu
            .import_linear(fd, geometry)
            .expect("import the dmabuf back");

        for (x, y) in [(0, 0), (63, 31), (7, 3)] {
            assert_eq!(
                imported.pixel(x, y).expect("read a pixel"),
                FILL,
                "pixel ({x}, {y}) did not survive the dmabuf round trip"
            );
        }

        // ★ ANTI-VACUITY: out of bounds must still refuse, or `pixel` returning
        // a constant would satisfy every assertion above.
        assert!(matches!(
            imported.pixel(64, 0),
            Err(KasaneError::OutOfBounds { .. })
        ));
    }

    /// ★ M1: THE DEVICE REPORTS IMPORTABLE TILED LAYOUTS.
    ///
    /// The modifier list is asked of the driver rather than hard-coded — a
    /// fixed list is how a compositor advertises a layout the GPU cannot read,
    /// and that failure lands in the CLIENT as a black window rather than in
    /// us as an error.
    ///
    /// This asserts the query works and that what comes back is usable for
    /// SAMPLING, which is what compositing does. It does NOT assert a
    /// particular modifier is present: llvmpipe and NVIDIA legitimately
    /// disagree, and pinning either one would make this a hardware test.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_driver_reports_modifiers_it_can_sample() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        let mods = gpu.importable_modifiers();
        eprintln!(
            "kasane M1: {:?} reports {} importable modifier(s): {:x?}",
            gpu.device_name,
            mods.len(),
            mods.iter().take(8).collect::<Vec<_>>()
        );

        // ★ LINEAR must be among them on any device that can import a dmabuf
        // at all — it is the one layout every driver understands, and its
        // absence means the query is reading the wrong thing rather than that
        // the hardware is unusual.
        const DRM_FORMAT_MOD_LINEAR: u64 = 0;
        assert!(
            mods.contains(&DRM_FORMAT_MOD_LINEAR),
            "no LINEAR modifier in {mods:x?} — the query is wrong, not the \
             driver: linear is the universal fallback layout"
        );

        // ★ AND THE LIST IS NOT JUST LINEAR on real hardware. A GPU that
        // reports exactly one modifier is either llvmpipe (correct — software
        // rendering has no tiling) or a query that silently truncated.
        if !gpu.is_cpu {
            assert!(
                mods.len() > 1,
                "a hardware GPU reported only LINEAR ({mods:x?}). Real GPUs \
                 expose vendor tiling layouts; one entry means the second \
                 query filled nothing."
            );
        }
    }

    /// ★ THE TILED IMPORT REFUSES A MODIFIER THE DEVICE DID NOT OFFER.
    ///
    /// Rather than succeeding and sampling garbage. An invalid modifier is a
    /// capability answer — the exporter and importer disagreed — and it must
    /// arrive as a typed error at the import boundary, which is the only place
    /// that can still say which buffer it was.
    #[test]
    #[cfg(target_os = "linux")]
    fn an_unoffered_modifier_is_refused_at_the_boundary() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        // Export a linear buffer just to obtain a real dmabuf fd; the point is
        // the modifier, not the contents.
        let Ok(mut exported) = gpu.export_linear(16, 16, [0, 0, 0, 0xff]) else {
            eprintln!("SKIP: export unavailable");
            return;
        };
        let geometry = exported.geometry;
        let Some(fd) = exported.take_fd() else {
            return;
        };
        drop(exported);

        // A modifier no vendor defines. `DRM_FORMAT_MOD_INVALID` is
        // 0x00ff_ffff_ffff_ffff and means "unknown layout" — precisely the
        // value a compositor must never accept, because guessing would paint
        // structured noise.
        const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
        let got = gpu.import_tiled(fd, geometry, DRM_FORMAT_MOD_INVALID);
        // ★ THE REFUSAL IS OURS, NOT THE DRIVER'S. This test failed on plo's
        // RTX 3070 before `import_tiled` gained its own check: NVIDIA accepts
        // DRM_FORMAT_MOD_INVALID at vkCreateImage without complaint. Finding
        // that out is the reason to run these on hardware rather than only on
        // llvmpipe.
        assert!(
            matches!(got, Err(KasaneError::ModifierNotSupported { .. })),
            "an unknown modifier was ACCEPTED. The import would sample a \
             layout nobody agreed on, and the result is noise on screen rather \
             than an error anyone can act on."
        );
    }

    /// ★ M4: THE DEVICE REPORTS THE DRM NODE IT DRIVES.
    ///
    /// On plo that is 226:1 (`/dev/dri/card1`) for the RTX 3070 and nothing at
    /// all for llvmpipe — software rendering owns no display. This asserts the
    /// query works and that a hardware GPU names a node; it does NOT assert a
    /// particular number, which would make it a test of one machine.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_hardware_device_names_the_drm_node_it_drives() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        let (primary, render) = gpu.drm_nodes();
        eprintln!(
            "kasane M4: {:?} primary={primary:?} render={render:?}",
            gpu.device_name
        );

        if gpu.is_cpu {
            // llvmpipe drives no display, and saying so is the correct answer
            // rather than a missing one.
            assert!(
                primary.is_none() && render.is_none(),
                "a software rasteriser claimed a DRM node: {primary:?}/{render:?}"
            );
            return;
        }

        assert!(
            primary.is_some() || render.is_some(),
            "a hardware GPU reported no DRM node at all. Without one there is \
             no way to tell which device drives the display, and compositing \
             would land on whichever enumerated first."
        );
        // ★ AND `drives` AGREES WITH ITSELF — a matcher that disagreed with
        // the value it matches on would send compositing to the wrong GPU
        // while reporting the right one.
        if let Some(n) = primary.or(render) {
            assert!(
                gpu.drives(n),
                "the device does not match its own node {n:?}"
            );
        }
        // A node it does not drive must be refused, or `drives` is a
        // constant-true that would accept any GPU.
        assert!(
            !gpu.drives(DrmNode {
                major: 226,
                minor: 999
            }),
            "drives() accepted a node this device does not have"
        );
    }

    /// ★ STAGE 1: THE QUEUE CAN ACTUALLY DRAW.
    ///
    /// `Gpu::open` used to take family 0 unconditionally, with a comment
    /// saying any family would do because M0 never submitted work. From stage
    /// 1 it does. Index 0 is right BY LUCK on plo — the RTX 3070 exposes six
    /// families and only some carry GRAPHICS — and on a device that orders
    /// them differently the symptom is a validation error at submit time,
    /// long after the device looked healthy.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_selected_queue_family_supports_graphics() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        // SAFETY: live instance and physical device.
        let families = unsafe {
            gpu.instance
                .get_physical_device_queue_family_properties(gpu.physical)
        };
        let idx = gpu.queue_family as usize;
        assert!(
            idx < families.len(),
            "queue_family {idx} is out of range for a device with {} families",
            families.len()
        );
        assert!(
            families[idx].queue_flags.contains(vk::QueueFlags::GRAPHICS),
            "family {idx} has flags {:#x} and cannot draw. A compositor \
             renderer submitted here would fail validation at draw time, not \
             at device creation. (Raw bits: ash is built without its `debug` \
             feature, so these flags have no Debug impl.)",
            families[idx].queue_flags.as_raw()
        );
        eprintln!(
            "kasane S1: {:?} queue_family={idx} of {} flags={:#x}",
            gpu.device_name,
            families.len(),
            families[idx].queue_flags.as_raw()
        );
    }

    /// ★ AND THE COMMAND POOL EXISTS, which is what proves the device was
    /// created with a family that can host one. A null pool would mean the
    /// constructor returned a Gpu whose submission path cannot work.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_device_has_a_command_pool() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("GPU test", &e);
                return;
            }
        };
        assert_ne!(
            gpu.command_pool,
            vk::CommandPool::null(),
            "no command pool — nothing can be recorded, so no frame can be drawn"
        );
    }

    /// ★ A DRIVER ACCEPTS IT — the half the header test cannot answer.
    ///
    /// A structurally valid module can still be refused: an unsupported
    /// capability, a SPIR-V version above what the driver implements, a
    /// malformed body past the header. `vkCreateShaderModule` is where that
    /// verdict comes from, and nothing before this line asks for it.
    #[test]
    fn the_driver_accepts_the_compiled_shader_module() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("shader module", &e);
                return;
            }
        };
        let module = gpu
            .shader_module()
            .expect("the driver refused a module build.rs already validated");
        assert_ne!(
            module,
            vk::ShaderModule::null(),
            "a null handle reported as success"
        );
        eprintln!(
            "kasane S2a: {:?} accepted {} bytes of SPIR-V",
            gpu.device_name,
            crate::COMPOSITE_SPV.len()
        );
        // SAFETY: created by this device on the line above, destroyed once.
        unsafe { gpu.destroy_shader_module(module) };
    }

    /// ★ THE PIPELINES COMPILE ON A REAL DRIVER — and that is ALL this proves.
    ///
    /// ── ★ CORRECTED BY ITS OWN RED RUN ───────────────────────────────────
    /// The first version of this comment claimed the interface mistakes are
    /// "refused HERE, at pipeline creation". TWO RED RUNS REFUTED IT, both
    /// measured on lavapipe:
    ///
    ///   A. descriptor type `SAMPLED_IMAGE` → `COMBINED_IMAGE_SAMPLER`
    ///      (disagreeing with what naga lowered the WGSL to) — **PASSED**
    ///   B. push-constant range `VERTEX | FRAGMENT` → `FRAGMENT` only
    ///      (leaving the vertex stage reading nothing) — **PASSED**
    ///
    /// Vulkan does not validate a pipeline's layout against its shader's
    /// interface without the validation layers, so a wrong layout compiles
    /// and then draws wrong. Both of those would have shipped.
    ///
    /// So: this test is worth keeping — it catches a malformed shader, an
    /// unsupported blend state, a format the driver cannot render to — but it
    /// is NOT an interface check, and a reader must not take it for one. The
    /// interface is checked by drawing and looking at the result, which is
    /// what `a_solid_draw_lands_the_colour_it_was_given` does.
    #[test]
    fn the_compositor_pipelines_compile_on_this_driver() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("pipelines", &e);
                return;
            }
        };

        // B8G8R8A8_UNORM is what ARGB8888 — the format every Wayland client
        // and every scanout buffer on this fleet uses — is called in Vulkan.
        let pipes = Pipelines::new(&gpu, vk::Format::B8G8R8A8_UNORM)
            .expect("the driver refused the compositor pipelines");

        assert_ne!(pipes.textured, vk::Pipeline::null(), "textured pipeline");
        assert_ne!(pipes.solid, vk::Pipeline::null(), "solid pipeline");
        assert_ne!(pipes.layout, vk::PipelineLayout::null(), "pipeline layout");
        assert_ne!(
            pipes.sampler_for(Filter::Linear),
            pipes.sampler_for(Filter::Nearest),
            "the two filters must be two samplers — one sampler reused would \
             silently blur pixel-exact surfaces or alias scaled ones, \
             depending on which survived"
        );

        eprintln!(
            "kasane S2b: {:?} compiled both pipelines for format {:#x}",
            gpu.device_name,
            pipes.format.as_raw()
        );
    }

    /// ★ A PARTIAL BUILD DESTROYS WHAT IT MADE.
    ///
    /// `Pipelines::new` constructs the struct BEFORE it is complete precisely
    /// so that `Drop` can clean up a failure part-way through. That is unusual
    /// enough to be worth pinning: the null handles a partial build leaves
    /// must be safe to pass to `destroy_*`, which Vulkan guarantees and this
    /// asserts by actually dropping one.
    #[test]
    fn dropping_a_partially_built_pipeline_set_is_safe() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("partial pipelines", &e);
                return;
            }
        };
        // A format no driver can use as a colour attachment, so `finish_building`
        // fails after the layout and samplers exist. If a driver ever accepts
        // it the test still passes — it is the DROP being exercised, and an
        // Ok here drops a complete set instead of a partial one.
        let _ = Pipelines::new(&gpu, vk::Format::UNDEFINED);
        // Reaching here without a double-free or a leak-report is the result.
    }

    /// ★★ THE FIRST PIXEL kasane HAS EVER DRAWN — and the gate that catches
    /// what pipeline creation cannot.
    ///
    /// Two red runs proved `the_compositor_pipelines_compile_on_this_driver`
    /// blind to a wrong descriptor type AND a wrong push-constant range: both
    /// compiled clean. This is the test that sees them, because a vertex stage
    /// reading nothing does not put the rectangle where it was asked to, and
    /// that shows up as the wrong pixel.
    ///
    /// Red is drawn over the LEFT HALF of a blue clear, so the test pins
    /// position as well as colour — a draw that covered everything, or
    /// nothing, would pass a colour-only assertion.
    #[test]
    fn a_solid_draw_lands_the_colour_and_the_place_it_was_given() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("solid draw", &e);
                return;
            }
        };
        // As f32 directly: these feed a clip-space transform, and routing a
        // `u32` through `as u16` to reach `f32::from` is a narrowing cast
        // wearing a lossless one's clothes.
        const W: f32 = 64.0;
        const H: f32 = 32.0;
        const FMT: vk::Format = vk::Format::B8G8R8A8_UNORM;

        // The target takes pixel counts; the transform takes f32. Written once
        // here rather than casting at four call sites.
        let (wpx, hpx) = (64_u32, 32_u32);
        let pipes = Pipelines::new(&gpu, FMT).expect("pipelines");
        let target = Target::new(&gpu, wpx, hpx, FMT).expect("target");

        let red = crate::Params {
            dst: crate::Params::dst_from_pixels([0.0, 0.0, W / 2.0, H], (W, H)),
            src: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 0.0, 0.0, 1.0],
        };
        // Clear blue, draw opaque red over the left half.
        target
            .draw(&pipes, [0.0, 0.0, 1.0, 1.0], &[Draw::Solid(red)])
            .expect("draw");

        // ★ B8G8R8A8_UNORM is BLUE FIRST in memory. Writing the expectation
        // as RGBA here is the classic way to get a test that passes on a
        // channel swap.
        let left = target.read_pixel(wpx / 4, hpx / 2).expect("read left");
        let right = target.read_pixel(3 * wpx / 4, hpx / 2).expect("read right");

        assert_eq!(
            left,
            [0, 0, 255, 255],
            "the left half must be the RED that was drawn (BGRA); got {left:?}"
        );
        assert_eq!(
            right,
            [255, 0, 0, 255],
            "the right half must still be the BLUE clear (BGRA) — a draw that \
             covered the whole target would pass a colour-only check; got {right:?}"
        );
        eprintln!(
            "kasane S2c: {:?} drew {W}x{H}, left={left:?} right={right:?}",
            gpu.device_name
        );
    }

    /// ★★ THE BLEND EQUATION IS PREMULTIPLIED — the single constant most
    /// likely to be wrong, and invisible until someone looks at a translucent
    /// window.
    ///
    /// A half-transparent white over OPAQUE RED gives a different answer
    /// under the two candidate equations, which is what makes this a test
    /// rather than a restatement of the code. The destination has to be opaque
    /// in the measured channel or the equations agree and the test is
    /// vacuous — see the note at the clear below, where that mistake was
    /// made and caught:
    ///
    ///   premultiplied (ONE, `ONE_MINUS_SRC_ALPHA)`:  0.5 + 1.0*0.5 = 1.00 → 255
    ///   straight      (`SRC_ALPHA`, `ONE_MINUS_SRC)`:  0.25 + 0.5    = 0.75 → 191
    ///
    /// So a red channel of 255 proves the premultiplied path and 191 proves
    /// the reflex mistake. Every Wayland buffer is premultiplied, so 191 would
    /// darken every translucent edge on the seat — a defect that reads as a
    /// theme problem, not a blend-state one.
    #[test]
    fn blending_treats_the_source_as_premultiplied() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("blend", &e);
                return;
            }
        };
        const N: u32 = 16;
        const FMT: vk::Format = vk::Format::B8G8R8A8_UNORM;
        let pipes = Pipelines::new(&gpu, FMT).expect("pipelines");
        let target = Target::new(&gpu, N, N, FMT).expect("target");

        // Premultiplied 50% white: every channel already scaled by alpha.
        let half_white = crate::Params {
            dst: [-1.0, -1.0, 2.0, 2.0],
            src: [0.0, 0.0, 1.0, 1.0],
            tint: [0.5, 0.5, 0.5, 0.5],
        };
        // ★ CLEARED TO RED, and the choice is the whole test. Over BLUE the
        // two equations both give 0.5 in the red channel and the test proves
        // nothing — which is exactly what the first draft did: it cleared blue
        // while its comment reasoned about red, and the driver's correct
        // answer (128) read as a failure. The destination must be OPAQUE in
        // the channel being measured for the two equations to differ.
        target
            .draw(&pipes, [1.0, 0.0, 0.0, 1.0], &[Draw::Solid(half_white)])
            .expect("draw");

        let px = target.read_pixel(N / 2, N / 2).expect("read");
        // BGRA: index 2 is red — the channel the two equations disagree on.
        let red = px[2];
        assert!(
            red >= 253,
            "expected the premultiplied result (255) in the red channel; got \
             {red}. 191 means the blend is SRC_ALPHA * ONE_MINUS_SRC_ALPHA, \
             which multiplies alpha twice. Full pixel (BGRA): {px:?}"
        );
        eprintln!("kasane S2c: blend over red gave BGRA {px:?}");
    }

    /// ★★ VALIDATION ERRORS FAIL THE BUILD — the gate that closes the class
    /// two red runs proved nothing else could see.
    ///
    /// Measured: a push-constant range naming only FRAGMENT compiles clean,
    /// draws clean, and produces the right pixel on lavapipe — so
    /// `the_compositor_pipelines_compile_on_this_driver` AND
    /// `a_solid_draw_lands_the_colour_and_the_place_it_was_given` both stay
    /// green. Under validation it is named exactly:
    /// `VUID-VkGraphicsPipelineCreateInfo-layout-07987`. A driver is not
    /// obliged to report any of this; the layer is.
    ///
    /// ── ★ THE ANTI-VACUITY HALF ──────────────────────────────────────────
    /// `validation_errors() == 0` is trivially true on a machine where the
    /// layer was never loaded, so this asserts `validation_active()` FIRST.
    /// Without that, the gate reports success everywhere while checking
    /// nothing — which is the exact failure shape it exists to catch.
    #[test]
    fn a_full_frame_provokes_no_validation_error() {
        if std::env::var_os("KASANE_VALIDATION").is_none() {
            eprintln!(
                "kasane: skipping the validation gate — set KASANE_VALIDATION=1 \
                 and VK_LAYER_PATH to run it"
            );
            return;
        }
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("validation", &e);
                return;
            }
        };
        assert!(
            validation_active(),
            "KASANE_VALIDATION=1 was set but no messenger is installed, so a \
             zero error count would prove nothing. Point VK_LAYER_PATH at the \
             validation layer's explicit_layer.d."
        );

        const N: u32 = 32;
        const FMT: vk::Format = vk::Format::B8G8R8A8_UNORM;
        let pipes = Pipelines::new(&gpu, FMT).expect("pipelines");
        let target = Target::new(&gpu, N, N, FMT).expect("target");
        let p = crate::Params {
            dst: crate::Params::dst_from_pixels([4.0, 4.0, 16.0, 16.0], (32.0, 32.0)),
            src: [0.0, 0.0, 1.0, 1.0],
            tint: [0.0, 1.0, 0.0, 1.0],
        };
        target
            .draw(&pipes, [0.0, 0.0, 0.0, 1.0], &[Draw::Solid(p)])
            .expect("draw");
        let _ = target.read_pixel(8, 8).expect("read");
        drop(target);
        drop(pipes);

        // The count is process-global, so this fails if ANY test in this run
        // provoked an error. That is deliberate: a race can only turn a
        // passing test red, never let a real error through.
        // ★ REPORTED, ALWAYS. An exemption that nobody sees is an exemption
        // nobody re-examines, and this number growing is the signal that the
        // list needs another look.
        eprintln!(
            "kasane: {} exempted validation error(s) from {} known VUID(s)",
            validation_exempt(),
            EXEMPT_VUIDS.len()
        );
        assert_eq!(
            validation_errors(),
            0,
            "the Vulkan validation layer reported errors — see the \
             `kasane VALIDATION ERROR:` lines above for the VUIDs"
        );
    }

    /// ★★★ A CLIENT BUFFER, COMPOSITED BY THE GPU — the whole point of kasane.
    ///
    /// A real kernel dmabuf is exported, imported as a SAMPLED image, and
    /// drawn into a render target by the fragment shader. The pixel that comes
    /// back has been through: host write → dmabuf → `vkBindImageMemory` →
    /// layout transition → descriptor set → `textureSample` → blend →
    /// attachment → copy. Every stage in pure Rust over `ash`.
    ///
    /// ── ★ WHAT THIS PROVES THAT M0 DOES NOT ──────────────────────────────
    /// M0 imports a dmabuf and reads it back with the CPU. That proves the
    /// external-memory machinery and nothing about compositing — it is, in
    /// fact, exactly the readback path that put the desktop on llvmpipe. This
    /// is the first test in which the GPU *looks at* a client buffer.
    ///
    /// ── ★ SCALED 2:1 ON PURPOSE ──────────────────────────────────────────
    /// The source is 8x8 and the destination 16x16, so the sampler, the UV
    /// rectangle and `Filter::Nearest` are all exercised. A 1:1 blit would
    /// pass even if the UVs were ignored entirely.
    #[test]
    fn a_client_buffer_is_sampled_and_composited_by_the_gpu() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("texture draw", &e);
                return;
            }
        };
        // Asymmetric across all four channels, so a channel swap or a
        // byte-order mistake cannot pass. Opaque, so the blend is the identity
        // and this test measures SAMPLING, not blending — that is the other
        // test's job.
        const FILL: [u8; 4] = [0x40, 0x80, 0xc0, 0xff];

        let mut exported = gpu.export_linear(8, 8, FILL).expect("export");
        let geometry = exported.geometry;
        let fd = exported.take_fd().expect("the exporter owns the fd");
        let imported = gpu.import_linear(fd, geometry).expect("import");

        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");
        let target = Target::new(&gpu, 16, 16, FORMAT).expect("target");

        let draw = Draw::Texture {
            params: crate::Params {
                // The whole target.
                dst: [-1.0, -1.0, 2.0, 2.0],
                // The whole source.
                src: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            },
            texture: imported.texture(),
            // NEAREST so the expected value is exact. LINEAR would blend
            // texels at the edges and the assertion would need a tolerance
            // that could hide a real error.
            filter: Filter::Nearest,
        };
        // Cleared to a colour that is NOT the fill, so a draw that did nothing
        // at all fails rather than coincidentally matching.
        target
            .draw(&pipes, [0.0, 1.0, 0.0, 1.0], &[draw])
            .expect("draw");

        let px = target.read_pixel(8, 8).expect("read");
        assert_eq!(
            px, FILL,
            "the composited pixel must be the client buffer's own — got \
             {px:?}, wanted {FILL:?}. A value of [0,255,0,255] means the draw \
             did not happen at all and this is the green clear."
        );
        eprintln!(
            "kasane S2d: {:?} sampled a dmabuf and composited it: {px:?}",
            gpu.device_name
        );
    }

    /// ★ THE SOURCE RECTANGLE IS HONOURED — cropping is one draw, not a pass.
    ///
    /// Half of an 8x8 source is drawn across the whole target. If `src` were
    /// ignored the result would be identical to the test above, which is
    /// exactly why that one cannot prove this: both halves of a uniformly
    /// filled buffer are the same colour.
    ///
    /// So the source is filled by hand with two different halves, through the
    /// SAME mapping the exporter used, and each half is asked for separately.
    #[test]
    fn the_source_rectangle_selects_which_part_of_the_buffer_is_drawn() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("uv crop", &e);
                return;
            }
        };
        const LEFT: [u8; 4] = [0x11, 0x22, 0x33, 0xff];
        const RIGHT: [u8; 4] = [0xaa, 0xbb, 0xcc, 0xff];

        let mut exported = gpu.export_linear(8, 8, LEFT).expect("export");
        // Repaint the right half through the exporter's own mapping, so the
        // stride comes from the driver rather than from an assumption.
        exported.fill_rect(4, 0, 4, 8, RIGHT).expect("repaint");
        let geometry = exported.geometry;
        let fd = exported.take_fd().expect("fd");
        let imported = gpu.import_linear(fd, geometry).expect("import");

        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");
        let target = Target::new(&gpu, 16, 16, FORMAT).expect("target");

        let sample_half = |u: f32| Draw::Texture {
            params: crate::Params {
                dst: [-1.0, -1.0, 2.0, 2.0],
                // Half the width of the source, starting at `u`.
                src: [u, 0.0, 0.5, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            },
            texture: imported.texture(),
            filter: Filter::Nearest,
        };

        target
            .draw(&pipes, [0.0, 0.0, 0.0, 1.0], &[sample_half(0.0)])
            .expect("draw left");
        let got_left = target.read_pixel(8, 8).expect("read");

        target
            .draw(&pipes, [0.0, 0.0, 0.0, 1.0], &[sample_half(0.5)])
            .expect("draw right");
        let got_right = target.read_pixel(8, 8).expect("read");

        assert_eq!(got_left, LEFT, "src.x = 0.0 must sample the left half");
        assert_eq!(
            got_right, RIGHT,
            "src.x = 0.5 must sample the RIGHT half; getting the left one \
             back means the UV rectangle is ignored and every surface would \
             be drawn uncropped"
        );
        assert_ne!(
            got_left, got_right,
            "the two halves must differ or this test proves nothing"
        );
    }

    /// ★★★ RENDERING STRAIGHT INTO A SHARED DMABUF — the thing that makes
    /// zero-copy possible, and the shape M5's scanout needs.
    ///
    /// `Target::new` allocates its own image, so anything drawn into it must
    /// be copied to be seen — which is the 12.0 ms of a 12.1 ms frame that
    /// nuri pays. This renders into a buffer somebody else allocated.
    ///
    /// ── ★ READ BACK THROUGH A SECOND, INDEPENDENT IMPORT ─────────────────
    /// Reading through the target's own readback buffer would prove nothing:
    /// it would pass even if the GPU had drawn into a private image that
    /// merely shares a handle. So the dmabuf fd is DUPLICATED before the
    /// target takes it, and the duplicate is imported separately and read.
    /// A pixel that arrives there was genuinely written into the shared
    /// buffer.
    #[test]
    fn a_frame_can_be_rendered_directly_into_a_shared_dmabuf() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("dmabuf target", &e);
                return;
            }
        };

        // ★ MEASURED, NOT ASSUMED. Whether a device can RENDER INTO a layout
        // is a different question from whether it can SAMPLE one, and the
        // answer differs by driver. `ImportUse` is what keeps the two lists
        // from being confused; this prints both so a failure here is
        // diagnosable rather than a shrug.
        let renderable = gpu.renderable_modifiers();
        let samplable = gpu.importable_modifiers();
        eprintln!(
            "kasane S2e: {:?} renderable={renderable:x?} samplable={samplable:x?} \
             raw_features={:#x?}",
            gpu.device_name,
            gpu.modifier_features()
        );

        /// `DRM_FORMAT_MOD_LINEAR`, the one layout every exporter can produce.
        const LINEAR: u64 = 0;
        if !renderable.contains(&LINEAR) {
            // A legitimate device answer, not a failure: this device cannot
            // render into a linear buffer. Saying so beats asserting a
            // capability the hardware never offered.
            eprintln!(
                "kasane S2e: this device offers no LINEAR render target; \
                 skipping (renderable={renderable:x?})"
            );
            return;
        }

        const SIZE: u32 = 16;
        // Exported black, so the drawn colour cannot be mistaken for the
        // buffer's initial contents.
        let mut exported = gpu
            .export_linear(SIZE, SIZE, [0, 0, 0, 0xff])
            .expect("export");
        let geometry = exported.geometry;
        let fd = exported.take_fd().expect("fd");
        // The second handle, taken BEFORE the target consumes the first.
        let read_fd = fd.try_clone().expect("dup the dmabuf fd");

        // ★ Import it FIRST as a bare object so the driver can be asked what
        // modifier the image really has — the one measurement that separates
        // "kasane asked for the wrong thing" from "the layer cannot tell".
        let probe = gpu
            .import_for(
                fd.try_clone().expect("dup for the probe"),
                geometry,
                LINEAR,
                ImportUse::RenderTarget,
            )
            .expect("probe import");
        match probe.actual_modifier() {
            Ok(m) => {
                eprintln!("kasane S2e: the driver says the image's modifier is {m:#x}");
                assert_eq!(
                    m, LINEAR,
                    "the image must carry the modifier that was asked for; a \
                     different one means the explicit-modifier create info was \
                     ignored, and every feature check was against the wrong \
                     layout"
                );
            }
            Err(e) => eprintln!("kasane S2e: the driver would not say ({e})"),
        }
        drop(probe);

        let target =
            Target::from_dmabuf(&gpu, fd, geometry, LINEAR).expect("import as a render target");
        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");

        // Opaque green over the whole target, via the clear — the simplest
        // thing that cannot be confused with black.
        target
            .draw(&pipes, [0.0, 1.0, 0.0, 1.0], &[])
            .expect("draw");

        // ★ THE INDEPENDENT READ.
        let witness = gpu.import_linear(read_fd, geometry).expect("second import");
        let px = witness.pixel(SIZE / 2, SIZE / 2).expect("read");
        assert_eq!(
            px,
            [0, 255, 0, 255],
            "a second import of the SAME dmabuf must see the green this frame \
             drew (BGRA). Black means the GPU rendered into a private image \
             and the buffer was never shared; got {px:?}"
        );
        eprintln!("kasane S2e: the shared buffer holds {px:?} — rendered, not copied");
    }

    /// ★ USAGE AND FEATURE NAME THE SAME CAPABILITY.
    ///
    /// [`ImportUse`] exists so these two cannot disagree, and this pins that
    /// they actually don't. The first draft DID disagree — usage asked for
    /// `COLOR_ATTACHMENT | TRANSFER_SRC` while the feature check tested only
    /// `COLOR_ATTACHMENT` — so a modifier was validated for one capability and
    /// the image created demanding two. The validation layer caught it; this
    /// makes the next such drift a test failure instead.
    #[test]
    fn every_import_purpose_checks_the_feature_its_usage_needs() {
        // The pairs that must correspond, written out rather than derived, so
        // this test fails when someone changes one side.
        let pairs = [
            (
                ImportUse::Surface,
                vk::ImageUsageFlags::SAMPLED,
                vk::FormatFeatureFlags::SAMPLED_IMAGE,
            ),
            (
                ImportUse::RenderTarget,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::TRANSFER_SRC,
            ),
        ];
        for (purpose, usage, feature) in pairs {
            // Raw bits, per the rule at the top of this file: ash is built
            // without its `debug` feature, so these flags have no `Debug`.
            assert_eq!(
                purpose.usage().as_raw(),
                usage.as_raw(),
                "{purpose:?} asks for a usage its feature check does not cover"
            );
            assert_eq!(
                purpose.required_feature().as_raw(),
                feature.as_raw(),
                "{purpose:?} checks a feature its usage does not need"
            );
        }
    }

    /// ★ THE TWO MODIFIER LISTS ARE ASKED SEPARATELY.
    ///
    /// A device may sample a layout it cannot render into. Reusing the sampled
    /// list to validate a render target would pass on a modifier the device
    /// cannot use as a colour attachment, and the driver need not say so —
    /// the image is created and the pixels come out wrong.
    ///
    /// On lavapipe both lists happen to be `[0]`, so this cannot assert they
    /// DIFFER. What it can assert — and what would actually regress — is that
    /// they are computed from different feature bits rather than one aliasing
    /// the other.
    #[test]
    fn renderable_and_samplable_modifiers_are_different_questions() {
        assert_ne!(
            ImportUse::Surface.required_feature().as_raw(),
            ImportUse::RenderTarget.required_feature().as_raw(),
            "if both purposes checked the same feature, `renderable_modifiers` \
             would be an alias for `importable_modifiers` and the distinction \
             this whole enum exists for would be gone"
        );
    }

    /// ★★ AN `wl_shm` CLIENT'S PIXELS REACH THE SCREEN — the other half of
    /// compositing, and the one most clients actually use.
    ///
    /// Not every client negotiates dmabuf; most simple toolkits hand over
    /// shared memory, and every client does before it negotiates anything. So
    /// `upload_texture` is not a fallback, it is the common path.
    ///
    /// Uploaded, sampled, composited, read back — the same journey as the
    /// dmabuf test, from the other kind of buffer.
    #[test]
    fn host_memory_becomes_a_texture_the_gpu_can_composite() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("upload", &e);
                return;
            }
        };
        // Asymmetric across all four channels so a channel swap cannot pass.
        const PIXEL: [u8; 4] = [0x12, 0x34, 0x56, 0xff];
        const N: u32 = 8;

        let data: Vec<u8> = PIXEL
            .iter()
            .copied()
            .cycle()
            .take((N * N * 4) as usize)
            .collect();
        let uploaded = gpu.upload_texture(N, N, &data).expect("upload");

        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");
        let target = Target::new(&gpu, 16, 16, FORMAT).expect("target");
        target
            .draw(
                &pipes,
                // Cleared to a colour the upload is not, so a draw that did
                // nothing fails rather than coincidentally matching.
                [1.0, 0.0, 1.0, 1.0],
                &[Draw::Texture {
                    params: crate::Params {
                        dst: [-1.0, -1.0, 2.0, 2.0],
                        src: [0.0, 0.0, 1.0, 1.0],
                        tint: [1.0, 1.0, 1.0, 1.0],
                    },
                    texture: uploaded.texture(),
                    filter: Filter::Nearest,
                }],
            )
            .expect("draw");

        let px = target.read_pixel(8, 8).expect("read");
        assert_eq!(
            px, PIXEL,
            "the composited pixel must be the uploaded one; [255,0,255,255] \
             means the draw did not happen and this is the clear"
        );
        eprintln!("kasane S2f: an uploaded shm texture composited as {px:?}");
    }

    /// ★ A PARTIAL UPDATE TOUCHES ONLY ITS OWN RECTANGLE.
    ///
    /// `update_memory` is how a client's damage reaches the GPU, and a version
    /// that rewrote the whole texture would be correct-looking and slow, while
    /// one that wrote to the wrong offset would corrupt a neighbouring region.
    /// Both are invisible to a test that only checks the updated pixel, so
    /// this checks a pixel OUTSIDE the region too.
    #[test]
    fn a_partial_upload_leaves_the_rest_of_the_texture_alone() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("partial upload", &e);
                return;
            }
        };
        const BASE: [u8; 4] = [0x11, 0x22, 0x33, 0xff];
        const PATCH: [u8; 4] = [0xaa, 0xbb, 0xcc, 0xff];
        const N: u32 = 8;

        let base: Vec<u8> = BASE
            .iter()
            .copied()
            .cycle()
            .take((N * N * 4) as usize)
            .collect();
        let uploaded = gpu.upload_texture(N, N, &base).expect("upload");

        // Repaint only the top-left 4x4.
        let patch: Vec<u8> = PATCH.iter().copied().cycle().take(4 * 4 * 4).collect();
        uploaded.write(0, 0, 4, 4, &patch).expect("partial write");

        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");
        let target = Target::new(&gpu, N, N, FORMAT).expect("target");
        target
            .draw(
                &pipes,
                [0.0, 0.0, 0.0, 1.0],
                &[Draw::Texture {
                    params: crate::Params {
                        dst: [-1.0, -1.0, 2.0, 2.0],
                        src: [0.0, 0.0, 1.0, 1.0],
                        tint: [1.0, 1.0, 1.0, 1.0],
                    },
                    texture: uploaded.texture(),
                    filter: Filter::Nearest,
                }],
            )
            .expect("draw");

        let inside = target.read_pixel(1, 1).expect("read inside");
        let outside = target.read_pixel(6, 6).expect("read outside");
        // ★ Print the four corners. If the patch appears at the BOTTOM-left
        // instead of the top-left, the UV mapping has a vertical flip — which
        // no earlier test could have caught: the solid-draw test checks `dst`
        // positioning, and the source-rectangle test only varies `src.x`.
        let tl = target.read_pixel(1, 1).expect("tl");
        let tr = target.read_pixel(6, 1).expect("tr");
        let bl = target.read_pixel(1, 6).expect("bl");
        let br = target.read_pixel(6, 6).expect("br");
        let tag = |p: [u8; 4]| if p == PATCH { "PATCH" } else { "base " };
        eprintln!(
            "kasane S2f: corners  tl={} tr={}\n             bl={} br={}",
            tag(tl),
            tag(tr),
            tag(bl),
            tag(br)
        );
        assert_eq!(inside, PATCH, "the updated region must show the new pixels");
        assert_eq!(
            outside, BASE,
            "a pixel OUTSIDE the updated rectangle must be untouched — getting \
             the patch here means the update ignored its offset and rewrote \
             the whole texture"
        );
    }

    /// ★ A SHORT UPLOAD IS REFUSED, not padded.
    ///
    /// Uploading fewer bytes than the region needs would put whatever the
    /// staging memory held last into the client's window — the previous
    /// frame, or another client's pixels. That is a disclosure bug, not a
    /// rendering one, so it is a refusal rather than a best effort.
    #[test]
    fn an_undersized_upload_is_refused_rather_than_showing_stale_memory() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("short upload", &e);
                return;
            }
        };
        let full = vec![0u8; 8 * 8 * 4];
        let uploaded = gpu.upload_texture(8, 8, &full).expect("upload");
        // Half the bytes an 8x8 region needs.
        let short = vec![0u8; 8 * 8 * 2];
        let err = uploaded
            .write(0, 0, 8, 8, &short)
            .expect_err("a short buffer must be refused");
        let text = err.to_string();
        assert!(
            text.contains("stale staging memory"),
            "the refusal must say WHY, or someone will pad it: {text}"
        );
    }

    /// ★★ A SCANOUT TARGET CAN STILL BE SCREENSHOT — on demand, not per frame.
    ///
    /// An imported target has no readback buffer, deliberately: copying it
    /// every frame is the cost kasane exists to remove. But "the screen is
    /// blank" has to remain answerable from another machine, which is the
    /// whole reason `ExportMem` is in `drm.rs`'s renderer bound.
    ///
    /// So this asserts the explicit path works on BOTH backings — the caller
    /// must not need to know which it has.
    #[test]
    fn a_finished_frame_can_be_captured_from_either_backing() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("capture", &e);
                return;
            }
        };
        const N: u32 = 8;
        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");

        // (a) an owned target
        let owned = Target::new(&gpu, N, N, FORMAT).expect("owned target");
        owned.draw(&pipes, [0.0, 0.0, 1.0, 1.0], &[]).expect("draw");
        let shot = owned.capture(0, 0, N, N).expect("capture owned");
        assert_eq!(
            shot.len(),
            (N * N * 4) as usize,
            "capture must be w*h*4 bytes"
        );
        assert_eq!(
            &shot[0..4],
            &[255, 0, 0, 255],
            "the captured pixel must be the blue that was cleared (BGRA)"
        );

        // (b) an imported target, which has NO readback buffer at all
        if !gpu.renderable_modifiers().contains(&0) {
            eprintln!("kasane S2f: no LINEAR render target on this device; owned arm only");
            return;
        }
        let mut exported = gpu.export_linear(N, N, [0, 0, 0, 0xff]).expect("export");
        let geometry = exported.geometry;
        let fd = exported.take_fd().expect("fd");
        let imported = Target::from_dmabuf(&gpu, fd, geometry, 0).expect("dmabuf target");
        imported
            .draw(&pipes, [0.0, 1.0, 0.0, 1.0], &[])
            .expect("draw");
        assert!(
            imported.read_pixel(0, 0).is_err(),
            "an imported target must have NO per-frame readback — if this \
             succeeds, every scanout frame is paying for a host copy"
        );
        let shot = imported.capture(0, 0, N, N).expect("capture imported");
        assert_eq!(
            &shot[0..4],
            &[0, 255, 0, 255],
            "the explicit capture must see the green this frame drew (BGRA)"
        );
        eprintln!("kasane S2f: captured both backings on demand");
    }

    /// ★ WHICH END OF THE SCREEN DOES PIXEL y=0 LAND ON?
    ///
    /// `a_fullscreen_rect_covers_exactly_the_clip_volume` pins the ARITHMETIC
    /// of `dst_from_pixels`, and `a_solid_draw_lands_the_colour_and_the_place
    /// _it_was_given` pins horizontal placement. Neither pins VERTICAL
    /// placement on real hardware — a full-height quad looks identical either
    /// way, which is exactly how a vertical flip survives a test suite.
    ///
    /// This draws the TOP HALF in pixels and asks where it came out.
    #[test]
    fn a_rect_at_pixel_y_zero_is_drawn_at_the_top_of_the_framebuffer() {
        let gpu = match Gpu::open() {
            Ok(g) => std::sync::Arc::new(g),
            Err(e) => {
                skip_or_panic("y direction", &e);
                return;
            }
        };
        const N: u32 = 16;
        let pipes = Pipelines::new(&gpu, FORMAT).expect("pipelines");
        let target = Target::new(&gpu, N, N, FORMAT).expect("target");

        // The top half in PIXEL coordinates: y from 0 to 8 of 16.
        let top_half = crate::Params {
            dst: crate::Params::dst_from_pixels([0.0, 0.0, 16.0, 8.0], (16.0, 16.0)),
            src: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 0.0, 0.0, 1.0],
        };
        target
            .draw(&pipes, [0.0, 0.0, 1.0, 1.0], &[Draw::Solid(top_half)])
            .expect("draw");

        let near_top = target.read_pixel(8, 2).expect("top");
        let near_bottom = target.read_pixel(8, 13).expect("bottom");
        const RED: [u8; 4] = [0, 0, 255, 255];
        const BLUE: [u8; 4] = [255, 0, 0, 255];
        assert_eq!(
            near_top, RED,
            "a rect at pixel y=0 must be drawn at the TOP; finding the clear \
             here and red at the bottom means clip y=-1 lands at the bottom \
             and every surface is drawn upside down"
        );
        assert_eq!(near_bottom, BLUE, "the bottom must still be the clear");
    }
}
