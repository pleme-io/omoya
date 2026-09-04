//! THE SEAM. Every `unsafe` in this crate is in this file, and a test in
//! `lib.rs` fails the build if one escapes.
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
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    ext_fd: ash::khr::external_memory_fd::Device,
    /// What the driver calls itself. For reports, so "which GPU answered" is
    /// never a guess.
    pub device_name: String,
    /// True when this is a software rasteriser. Not a defect — it is how CI
    /// exercises the path — but a seat must be able to tell the difference.
    pub is_cpu: bool,
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
        let ci = vk::InstanceCreateInfo::default().application_info(&app);
        // SAFETY: `ci` outlives the call, and `app` outlives `ci` — both are
        // locals in this frame. No allocator callbacks are supplied.
        let instance = unsafe { entry.create_instance(&ci, None) }
            .map_err(|e| Unavailable::Driver(format!("create_instance: {e:?}")))?;

        match Self::pick(&entry, &instance) {
            Ok(gpu) => Ok(gpu),
            Err(e) => {
                // SAFETY: the instance was created above, nothing else holds it,
                // and no child objects were created on the failing paths.
                unsafe { instance.destroy_instance(None) };
                Err(e)
            }
        }
    }

    fn pick(entry: &ash::Entry, instance: &ash::Instance) -> Result<Self, Unavailable> {
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
            // Any family will do: M0 never submits work. M1's readback copy
            // needs TRANSFER, which every family implicitly supports per spec
            // when it supports GRAPHICS or COMPUTE — revisit there, not here.
            if families.is_empty() {
                continue;
            }
            chosen = Some((pd, 0));
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
        let want = [
            ash::khr::external_memory_fd::NAME.as_ptr(),
            ash::ext::external_memory_dma_buf::NAME.as_ptr(),
            // ★ M1. A real client buffer from a GPU is TILED, and its layout
            // is named by a DRM format modifier. Without this extension the
            // only importable layout is linear — the CPU-readback path this
            // crate exists to remove.
            ash::ext::image_drm_format_modifier::NAME.as_ptr(),
        ];
        let dci = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&want);
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

        Ok(Self {
            _entry: entry.clone(),
            ext_fd: ash::khr::external_memory_fd::Device::new(instance, &device),
            instance: instance.clone(),
            physical,
            device,
            mem_props,
            device_name: name,
            is_cpu: props.device_type == vk::PhysicalDeviceType::CPU,
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
            // LINEAR so the rows are addressable by a CPU, and so
            // PREINITIALIZED is legal — which is what lets this write pixels
            // by mapping instead of submitting a command buffer.
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::PREINITIALIZED);
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
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::PREINITIALIZED);
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
            Ok(memory) => Ok(Imported {
                gpu: self,
                image,
                memory,
                geometry,
            }),
            Err(e) => {
                // SAFETY: live image; memory was never bound on this path.
                unsafe { self.device.destroy_image(image, None) };
                Err(e)
            }
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: every child object is owned by an `Exported`/`Imported` whose
        // lifetime is tied to `&self`, so all of them are already dropped by the
        // time this runs — that is what the borrow checker is enforcing here.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
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
    pub geometry: Geometry,
}

impl Imported<'_> {
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
        // SAFETY: both handles came from `gpu.device` and are destroyed once.
        unsafe {
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
            Ok(memory) => Ok(Imported {
                gpu: self,
                image,
                memory,
                geometry,
            }),
            Err(e) => {
                // SAFETY: live image; memory was never bound on this path.
                unsafe { self.device.destroy_image(image, None) };
                Err(e)
            }
        }
    }
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
                eprintln!("SKIP: no GPU pipe on this machine — {e}");
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
                eprintln!("SKIP: no GPU pipe — {e}");
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
                eprintln!("SKIP: no GPU pipe — {e}");
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
}
