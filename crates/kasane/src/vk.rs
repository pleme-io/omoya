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
//! The buffer here is LINEAR and HOST_VISIBLE, because that is what lets the
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
const FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;

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
        VALIDATION_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        eprintln!("kasane VALIDATION ERROR: {message}");
    } else {
        eprintln!("kasane validation: {message}");
    }
    vk::FALSE
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
            && unsafe { entry.enumerate_instance_layer_properties() }
                .map(|ls| {
                    ls.iter().any(|l| {
                        // SAFETY: `layer_name` is a NUL-terminated fixed array
                        // the loader filled; the spec guarantees the terminator.
                        let n = unsafe { std::ffi::CStr::from_ptr(l.layer_name.as_ptr()) };
                        n == layer_name
                    })
                })
                .unwrap_or(false);

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
            let exts = match unsafe { instance.enumerate_device_extension_properties(pd) } {
                Ok(e) => e,
                Err(_) => continue,
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
        let device_exts = match unsafe { instance.enumerate_device_extension_properties(physical) }
        {
            Ok(e) => e,
            Err(_) => Vec::new(),
        };
        // SAFETY: `extension_name` is a NUL-terminated fixed array the driver
        // filled; the spec guarantees the terminator.
        fn ext_name(e: &vk::ExtensionProperties) -> &std::ffi::CStr {
            unsafe { std::ffi::CStr::from_ptr(e.extension_name.as_ptr()) }
        }
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
        &self,
        width: u32,
        height: u32,
        fill: [u8; 4],
    ) -> Result<Exported<'_>, KasaneError> {
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
            gpu: self,
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
        &self,
        fd: OwnedFd,
        geometry: Geometry,
    ) -> Result<Imported<'_>, KasaneError> {
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
                        gpu: self,
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
        if crate::COMPOSITE_SPV.len() % 4 != 0 {
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
pub struct Pipelines<'g> {
    gpu: &'g Gpu,
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

impl<'g> Pipelines<'g> {
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
    pub fn new(gpu: &'g Gpu, format: vk::Format) -> Result<Self, KasaneError> {
        let module = gpu.shader_module()?;

        // Built incrementally so that a failure part-way through still
        // destroys what was already made. Without this, an error between the
        // sampler and the pipeline leaks a sampler on every retry — and a
        // compositor retries.
        let mut built = Self {
            gpu,
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

impl Drop for Pipelines<'_> {
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
            dev.destroy_shader_module(self.module, None);
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
#[derive(Clone, Copy, Debug)]
pub struct TextureRef {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
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
pub struct Target<'g> {
    gpu: &'g Gpu,
    image: vk::Image,
    view: vk::ImageView,
    image_memory: vk::DeviceMemory,
    readback: vk::Buffer,
    readback_memory: vk::DeviceMemory,
    /// Size in pixels.
    pub extent: vk::Extent2D,
    /// Colour format — must match the [`Pipelines`] used to draw into it.
    pub format: vk::Format,
}

impl<'g> Target<'g> {
    /// Create a `width` x `height` target in `format`.
    ///
    /// # Errors
    /// [`KasaneError::Vulkan`] for any driver refusal; every one names the
    /// call that failed, because "could not create target" is not actionable.
    pub fn new(
        gpu: &'g Gpu,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<Self, KasaneError> {
        let extent = vk::Extent2D { width, height };
        let mut t = Self {
            gpu,
            image: vk::Image::null(),
            view: vk::ImageView::null(),
            image_memory: vk::DeviceMemory::null(),
            readback: vk::Buffer::null(),
            readback_memory: vk::DeviceMemory::null(),
            extent,
            format,
        };
        // Same incremental shape as `Pipelines`: `Drop` cleans up a failure
        // part-way through, which matters because a compositor retries.
        t.build(width, height, format)?;
        Ok(t)
    }

    fn build(&mut self, width: u32, height: u32, format: vk::Format) -> Result<(), KasaneError> {
        let dev = &self.gpu.device;

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
            .tiling(vk::ImageTiling::OPTIMAL)
            // TRANSFER_SRC is what makes the result readable at all — see the
            // struct header for why the copy is not optional.
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `image_info` and everything it borrows are locals.
        self.image = unsafe { dev.create_image(&image_info, None) }
            .map_err(|e| driver("create_image(target)", e))?;

        // SAFETY: the image was just created on this device.
        let reqs = unsafe { dev.get_image_memory_requirements(self.image) };
        let idx = self.gpu.memory_type(
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            "DEVICE_LOCAL",
        )?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(reqs.size)
            .memory_type_index(idx);
        // SAFETY: `alloc` is a local; `idx` came from this device's properties.
        self.image_memory = unsafe { dev.allocate_memory(&alloc, None) }
            .map_err(|e| driver("allocate_memory(target)", e))?;
        // SAFETY: image and memory are both from this device, bound once.
        unsafe { dev.bind_image_memory(self.image, self.image_memory, 0) }
            .map_err(|e| driver("bind_image_memory(target)", e))?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        // SAFETY: `view_info` is a local naming a live image.
        self.view = unsafe { dev.create_image_view(&view_info, None) }
            .map_err(|e| driver("create_image_view(target)", e))?;

        // ★ TIGHTLY PACKED, deliberately. `cmd_copy_image_to_buffer` with
        // `buffer_row_length: 0` means "rows are `width` texels", so the
        // readback has no stride of its own to get wrong — unlike the imported
        // side, where the DRIVER picks the stride and it must be asked for.
        let size = u64::from(width) * u64::from(height) * 4;
        let buf_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `buf_info` is a local.
        self.readback = unsafe { dev.create_buffer(&buf_info, None) }
            .map_err(|e| driver("create_buffer(readback)", e))?;
        // SAFETY: the buffer was just created on this device.
        let breqs = unsafe { dev.get_buffer_memory_requirements(self.readback) };
        let bidx = self.gpu.memory_type(
            breqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            "HOST_VISIBLE | HOST_COHERENT",
        )?;
        let balloc = vk::MemoryAllocateInfo::default()
            .allocation_size(breqs.size)
            .memory_type_index(bidx);
        // SAFETY: `balloc` is a local; `bidx` came from this device.
        self.readback_memory = unsafe { dev.allocate_memory(&balloc, None) }
            .map_err(|e| driver("allocate_memory(readback)", e))?;
        // SAFETY: buffer and memory both from this device, bound once.
        unsafe { dev.bind_buffer_memory(self.readback, self.readback_memory, 0) }
            .map_err(|e| driver("bind_buffer_memory", e))?;
        Ok(())
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
        pipes: &Pipelines<'_>,
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

    fn record_and_submit(
        &self,
        cmd: vk::CommandBuffer,
        pipes: &Pipelines<'_>,
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

        // ★ EVERY SAMPLED IMAGE MOVES TO SHADER_READ_ONLY_OPTIMAL FIRST, and
        // this must happen OUTSIDE the rendering scope — layout transitions
        // are illegal between `cmd_begin_rendering` and `cmd_end_rendering`.
        //
        // The source layout is UNDEFINED, which discards the image's previous
        // CONTENTS in general — but not here: an imported dmabuf's pixels live
        // in memory the exporter wrote and this image is only a view onto it.
        // Naming the real previous layout is impossible anyway, because the
        // buffer came from another process that never told us.
        for d in draws {
            if let Draw::Texture { texture, .. } = *d {
                let b = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
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
            .image_view(self.view)
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
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
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
                    // One set per draw, from the pool reset at the top of this
                    // frame. Vulkan forbids updating a set that a submitted
                    // command buffer still references, so re-using one set
                    // across draws in a frame would be undefined — the mistake
                    // is invisible until two surfaces are on screen and one
                    // shows the other's contents.
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
                    let sampler_info =
                        [vk::DescriptorImageInfo::default().sampler(pipes.sampler_for(filter))];
                    // Binding 0 is the image and 1 is the sampler, matching
                    // what the WGSL declares — naga lowers `texture_2d` and
                    // `sampler` to two descriptors, not a combined one.
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
                    // SAFETY: every borrowed array is a local outliving the
                    // call, and the set was allocated from this device.
                    unsafe { dev.update_descriptor_sets(&writes, &[]) };

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
            // 0 means "tightly packed at the image's width" — see `build`.
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
        // SAFETY: image is in TRANSFER_SRC_OPTIMAL, buffer is large enough
        // (allocated as width*height*4 in `build`), region is a local.
        unsafe {
            dev.cmd_copy_image_to_buffer(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.readback,
                &[region],
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
            .image(self.image)
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
        let size = u64::from(self.extent.width) * u64::from(self.extent.height) * 4;
        // SAFETY: the memory is HOST_VISIBLE (selected in `build`), the range
        // is the whole allocation, and it is unmapped before returning.
        let ptr = unsafe {
            self.gpu
                .device
                .map_memory(self.readback_memory, 0, size, vk::MemoryMapFlags::empty())
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
        unsafe { self.gpu.device.unmap_memory(self.readback_memory) };
        Ok(px)
    }
}

impl Drop for Target<'_> {
    fn drop(&mut self) {
        let dev = &self.gpu.device;
        // SAFETY: every handle came from this device and is destroyed once.
        // Null handles are legal to pass, which is what makes a partially
        // built target safe to drop. Memory is freed AFTER the object bound to
        // it, which is the ordering Vulkan requires.
        unsafe {
            dev.destroy_image_view(self.view, None);
            dev.destroy_image(self.image, None);
            dev.free_memory(self.image_memory, None);
            dev.destroy_buffer(self.readback, None);
            dev.free_memory(self.readback_memory, None);
        }
    }
}

/// A dmabuf this process exported, plus the Vulkan objects backing it.
///
/// Borrows the [`Gpu`], so it cannot outlive the device that must destroy it —
/// the ordering is enforced by the compiler rather than by a comment.
pub struct Exported<'g> {
    gpu: &'g Gpu,
    image: vk::Image,
    memory: vk::DeviceMemory,
    /// The kernel dmabuf. `Option` because a type with a `Drop` impl cannot be
    /// destructured — the fd has to be TAKEN, and taking it must leave
    /// something well-defined behind. `None` simply means the caller already
    /// owns it, and `Drop` then closes nothing.
    fd: Option<OwnedFd>,
    pub geometry: Geometry,
}

impl Exported<'_> {
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

impl Drop for Exported<'_> {
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
pub struct Imported<'g> {
    gpu: &'g Gpu,
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

impl Imported<'_> {
    /// This buffer as something a [`Draw::Texture`] can sample.
    #[must_use]
    pub fn texture(&self) -> TextureRef {
        TextureRef {
            image: self.image,
            view: self.view,
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

impl Drop for Imported<'_> {
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
    #[must_use]
    pub fn importable_modifiers(&self) -> Vec<u64> {
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
        store
            .iter()
            .take(n)
            .filter(|m| {
                // Single-plane only: multi-plane formats are a YUV concern and
                // this crate composites BGRA. `accepts` refuses them on the
                // nuri side for the same reason.
                m.drm_format_modifier_plane_count == 1
                    && m.drm_format_modifier_tiling_features
                        .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
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
        &self,
        fd: OwnedFd,
        geometry: Geometry,
        modifier: u64,
    ) -> Result<Imported<'_>, KasaneError> {
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
        let offered = self.importable_modifiers();
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
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC)
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
                        gpu: self,
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
        let primary = drm.has_primary.eq(&vk::TRUE).then(|| DrmNode {
            major: drm.primary_major,
            minor: drm.primary_minor,
        });
        let render = drm.has_render.eq(&vk::TRUE).then(|| DrmNode {
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
    if std::env::var_os("OMOYA_REQUIRE_GPU").is_some() {
        panic!(
            "{what}: no GPU pipe, but OMOYA_REQUIRE_GPU is set — this \
             environment is supposed to have a device. Reason: {why}"
        );
    }
    eprintln!("SKIP: {what} — {why}");
}

#[cfg(test)]
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
            Err(e) => {
                skip_or_panic("solid draw", &e);
                return;
            }
        };
        const W: u32 = 64;
        const H: u32 = 32;
        const FMT: vk::Format = vk::Format::B8G8R8A8_UNORM;

        let pipes = Pipelines::new(&gpu, FMT).expect("pipelines");
        let target = Target::new(&gpu, W, H, FMT).expect("target");

        let red = crate::Params {
            dst: crate::Params::dst_from_pixels(
                [0.0, 0.0, f32::from(W as u16) / 2.0, f32::from(H as u16)],
                (f32::from(W as u16), f32::from(H as u16)),
            ),
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
        let left = target.read_pixel(W / 4, H / 2).expect("read left");
        let right = target.read_pixel(3 * W / 4, H / 2).expect("read right");

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
    ///   premultiplied (ONE, ONE_MINUS_SRC_ALPHA):  0.5 + 1.0*0.5 = 1.00 → 255
    ///   straight      (SRC_ALPHA, ONE_MINUS_SRC):  0.25 + 0.5    = 0.75 → 191
    ///
    /// So a red channel of 255 proves the premultiplied path and 191 proves
    /// the reflex mistake. Every Wayland buffer is premultiplied, so 191 would
    /// darken every translucent edge on the seat — a defect that reads as a
    /// theme problem, not a blend-state one.
    #[test]
    fn blending_treats_the_source_as_premultiplied() {
        let gpu = match Gpu::open() {
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
            Ok(g) => g,
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
}
