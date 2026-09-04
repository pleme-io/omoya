//! kasane (重ね) — the GPU pipe. Layering, to `nuri`'s coating.
//!
//! ── ★ WHAT THIS CRATE IS FOR ─────────────────────────────────────────────
//! omoya composites on the CPU, so it advertises **linear-modifier dmabuf
//! only** — a tiled modifier describes a layout only a GPU can decode, and a
//! CPU blitter reading one paints structured noise. NVIDIA does not present
//! linear. So on plo, a machine with a GeForce RTX 3070, **every GPU client
//! renders through `llvmpipe`** — `mado` included, which is a GPU terminal.
//!
//! kasane is the second pipe. It imports a client's dmabuf as a Vulkan image
//! so the compositor can texture what the client's GPU produced, without the
//! bytes crossing the CPU. `nuri` stays exactly as it is and remains the
//! fallback and the CPU pipe.
//!
//! Design, milestones and the tier-honest ledger: `omoya/docs/KASANE.md`.
//!
//! ── ★ THE C BOUNDARY, AND WHERE IT IS ────────────────────────────────────
//! Operator law: *"we go down to the lowest layer and surround the C with Safe
//! Rust and keep it contained."*
//!
//! The intuitive choice for a Wayland compositor is EGL/GLES, and it is the
//! wrong one — **it is a C abstraction stacked on top of the driver, so it is
//! MORE C surface, not less**, with unsafe spread across a large API. Raw
//! Vulkan is the thinnest boundary that exists: one ABI, reached through pure
//! Rust bindings that `dlopen` at runtime.
//!
//! So the containment is structural, not a matter of discipline:
//!
//! * every `unsafe` in this crate lives in [`vk`], and nowhere else;
//! * that is a **compile error** to violate, not a convention: the crate is
//!   `#![deny(unsafe_code)]` and exactly one module carries the matching
//!   `#[allow]`. An `unsafe` block anywhere else does not build;
//! * a grep-backed test stands behind it, catching the one thing the lint
//!   cannot — somebody adding a second `#[allow(unsafe_code)]`;
//! * the loader being absent is a **typed state** ([`Unavailable::NoLoader`]),
//!   never a panic — a machine with no Vulkan runs omoya on `nuri`.
//!
//! The irreducible remainder is stated rather than hidden: the vendor driver
//! behind the loader is C, and no Rust reaches a GPU without it. That is a
//! fact about the world, not one of our abstractions, so it is typed.
//!
//! ── ★ M0 — WHAT IS ACTUALLY BUILT HERE ───────────────────────────────────
//! Export a linear dmabuf from Vulkan, import it back as an image, and read a
//! pixel. That is the whole of M0 and it is deliberately narrow: it proves the
//! external-memory machinery end to end **without** needing GBM (a C library
//! we will not link) or a live Wayland client.
//!
//! It runs on `llvmpipe`, which advertises the same three extensions NVIDIA
//! does — so the import path is covered on machines with no GPU, which is most
//! of CI. That was measured, not assumed (`docs/KASANE.md` §3).
//!
//! **Not built:** tiled modifiers (M1), the `smithay::Renderer` impl (M2), the
//! typed fallback wiring (M3), routing (M4), scanout (M5).

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
// ★ CONTAINMENT, AT THE TIER THAT CANNOT BE FORGOTTEN.
//
// The doc above promised one unsafe seam. A grep test can only report a
// violation after someone writes it; this REFUSES TO COMPILE one. The single
// `#[allow]` below is the entire licensed surface, and it is one line, in one
// place, that a reviewer can find by searching for the lint name.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // THE seam. The only `#[allow(unsafe_code)]` in the crate.
pub mod vk;

/// The compositor's shader module, compiled from WGSL at build time.
///
/// ── ★ WHERE THIS COMES FROM ──────────────────────────────────────────────
/// `build.rs` runs `naga` over `shaders/composite.wgsl` and writes the SPIR-V
/// here. naga is pure Rust and a BUILD dependency only, so the seat ships no
/// shader compiler and links no C — see `build.rs`'s header for why every
/// ordinary route to SPIR-V (shaderc, glslang) is a C library this crate
/// refuses.
///
/// `OUT_DIR` is used rather than a path under the manifest because
/// substrate's crate2nix builds set `CARGO_MANIFEST_DIR` to the WORKSPACE
/// ROOT; `OUT_DIR` is correct under every builder.
///
/// ── ★ WHY THIS IS AT THE CRATE ROOT AND NOT IN `vk` ──────────────────────
/// `mod vk` is `#[cfg(target_os = "linux")]`, and a constant placed there is
/// invisible everywhere else — including on the darwin workstation where most
/// of this crate is actually edited. The bytes are not part of the unsafe
/// seam: `build.rs` produces them on every platform and only
/// `vkCreateShaderModule` needs a driver. Keeping them here is what lets the
/// header gate below run where no GPU exists, which is the only place it adds
/// anything.
pub(crate) const COMPOSITE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/composite.spv"));

/// The entry point names inside [`COMPOSITE_SPV`], as the pipeline asks for them.
///
/// ★ These are `\0`-terminated because `VkPipelineShaderStageCreateInfo.pName`
/// is a C string and Vulkan reads past the Rust length. Writing them with the
/// terminator in the literal makes that impossible to forget at the call site,
/// where the mistake would be a driver reading arbitrary bytes rather than a
/// compile error.
pub(crate) mod entry {
    /// The shared vertex stage: builds a quad from `vertex_index`.
    pub const VERTEX: &[u8] = b"vs_quad\0";
    /// Samples a client surface, premultiplied.
    pub const FRAGMENT_TEXTURE: &[u8] = b"fs_texture\0";
    /// Fills a rectangle with a premultiplied colour.
    pub const FRAGMENT_SOLID: &[u8] = b"fs_solid\0";
}

/// The push constants every draw carries — one struct, both entry points.
///
/// ★ ONE STRUCT FOR BOTH PIPELINES is deliberate. Vulkan validates that a
/// pipeline's push-constant range matches its layout's, so two structs would
/// mean two layouts, two ranges, and a mismatch that reports as a validation
/// error about byte offsets rather than about the two shaders having drifted.
/// `fs_solid` simply ignores `src`.
///
/// ★ AT THE CRATE ROOT, NOT IN `mod vk`, for the reason the SPIR-V blob is:
/// `mod vk` is `#[cfg(target_os = "linux")]`, so a coordinate transform placed
/// there is untested on the darwin workstation where it is written. Nothing
/// about this struct needs a driver; only `bytes()` needs `unsafe`, and that
/// one method stays in the seam.
///
/// `repr(C)` because these bytes are handed to a driver: Rust's default layout
/// is explicitly allowed to reorder fields, and a reordered `dst`/`src` would
/// draw every surface at the wrong place with no error anywhere.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    /// Destination rectangle in CLIP SPACE: `[x, y, w, h]`, x and y in
    /// `[-1, 1]`. Pre-transformed on the CPU so the shader has no matrix.
    pub dst: [f32; 4],
    /// Source rectangle in texture UV: `[u, v, w, h]` in `[0, 1]`. Carrying it
    /// per-draw is what makes crop and scale one draw instead of a pre-pass.
    pub src: [f32; 4],
    /// Premultiplied colour for `fs_solid`; its `a` is the opacity multiplier
    /// `fs_texture` applies.
    pub tint: [f32; 4],
}

impl Params {
    /// Byte size, as the pipeline layout's push-constant range declares it.
    ///
    /// Derived from the type rather than written as `48`, so adding a field to
    /// `Params` cannot leave the range describing the old struct — which
    /// Vulkan would accept, silently passing a shader fewer bytes than it
    /// reads.
    pub(crate) const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// Map a rectangle in PIXELS on an output of `size` to clip space.
    ///
    /// ★ Vulkan's Y axis points DOWN in framebuffer coordinates and clip space
    /// runs -1 (top) to +1 (bottom), which is the same direction a compositor
    /// already thinks in — so this is a scale and a bias, with no flip. Doing
    /// the flip here "for OpenGL reasons" is the classic way to get an
    /// upside-down desktop that looks correct in a screenshot.
    #[must_use]
    pub fn dst_from_pixels(rect: [f32; 4], size: (f32, f32)) -> [f32; 4] {
        let (w, h) = size;
        [
            rect[0] / w * 2.0 - 1.0,
            rect[1] / h * 2.0 - 1.0,
            rect[2] / w * 2.0,
            rect[3] / h * 2.0,
        ]
    }
}

use std::fmt;

/// Why the GPU pipe is not available.
///
/// ★ EVERY ARM IS A LEGITIMATE STATE, NOT AN ERROR PATH. A workstation with no
/// discrete GPU, a container with no `/dev/dri`, a CI runner, a Mac — all of
/// them take one of these arms and get `nuri`, which is a working compositor.
/// This is `kotae`'s rule applied to a capability: the answer says WHICH of
/// the things happened, because "no GPU pipe" and "the loader is missing" send
/// a reader to completely different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// `libvulkan.so.1` could not be dlopened. No Vulkan on this machine.
    NoLoader,
    /// This platform has no dmabuf at all — dmabuf is a Linux concept.
    UnsupportedPlatform,
    /// A loader is present but reported no physical devices.
    NoPhysicalDevice,
    /// Devices exist, but none offers the external-memory extensions this
    /// crate needs. Carries how many were examined, so a zero denominator is
    /// distinguishable from a real absence.
    NoDeviceWithDmabuf { examined: usize },
    /// The driver was reached and refused. Carries the Vulkan result as text
    /// rather than a code, because a bare `-3` sends the reader to a table.
    Driver(String),
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLoader => f.write_str(
                "no Vulkan loader on this machine (libvulkan.so.1 could not be \
                 dlopened) — the CPU pipe is the correct renderer here",
            ),
            Self::UnsupportedPlatform => {
                f.write_str("dmabuf is a Linux concept and this is not Linux")
            }
            Self::NoPhysicalDevice => {
                f.write_str("a Vulkan loader is present but reports no physical devices")
            }
            Self::NoDeviceWithDmabuf { examined } => write!(
                f,
                "examined {examined} Vulkan device(s); none offers \
                 VK_EXT_external_memory_dma_buf + VK_KHR_external_memory_fd, so \
                 a client dmabuf cannot be imported here"
            ),
            Self::Driver(e) => write!(f, "the Vulkan driver refused: {e}"),
        }
    }
}

impl std::error::Error for Unavailable {}

/// A four-byte-per-pixel linear buffer's geometry.
///
/// ★ `stride` is carried SEPARATELY from `width` and is never derived from it.
/// A driver is free to pad rows, and computing `stride = width * 4` is the
/// classic way to read a correct buffer as diagonal garbage — the same class
/// of defect as assuming a modifier is linear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// Bytes per row, as the DRIVER reported it.
    pub stride: u64,
    /// Byte offset of the first pixel.
    pub offset: u64,
}

impl Geometry {
    /// Byte offset of a pixel, or `None` when it is outside the buffer.
    ///
    /// Pure arithmetic, deliberately here in the safe half: the seam should
    /// contain FFI, not bounds logic that can be tested without a GPU.
    #[must_use]
    pub fn byte_offset(&self, x: u32, y: u32) -> Option<u64> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.offset + u64::from(y) * self.stride + u64::from(x) * 4)
    }

    /// The smallest allocation that can hold this geometry.
    #[must_use]
    pub fn min_size(&self) -> u64 {
        self.offset + u64::from(self.height) * self.stride
    }
}

/// Something went wrong while moving a real buffer around.
#[derive(Debug, thiserror::Error)]
pub enum KasaneError {
    /// The GPU pipe is not available at all.
    #[error("the GPU pipe is unavailable: {0}")]
    Unavailable(#[from] Unavailable),
    /// A Vulkan call failed. Named, so the message says which.
    #[error("{call} failed: {result}")]
    Vulkan { call: &'static str, result: String },
    /// No memory type satisfied the requirements. Carries the mask so the
    /// report is diagnosable rather than a shrug.
    #[error(
        "no memory type satisfies {wanted:?} within the driver's mask {mask:#x} \
         — this is a capability answer, not a bug"
    )]
    NoMemoryType { wanted: &'static str, mask: u32 },
    /// A dmabuf arrived describing a layout this device did not offer.
    ///
    /// ★ MEASURED, NOT ASSUMED. The RTX 3070 **accepts**
    /// `DRM_FORMAT_MOD_INVALID` at `vkCreateImage` — the test that expected a
    /// refusal failed on real hardware. So the driver will not catch an
    /// exporter/importer disagreement for us; it will sample whatever layout
    /// it was told and paint structured noise. The check has to be ours, at
    /// the import boundary, which is the only place that still knows which
    /// buffer it was.
    #[error(
        "modifier {modifier:#x} is not one this device can sample; it offers          {offered:x?}. Importing anyway would read a layout nobody agreed on."
    )]
    ModifierNotSupported { modifier: u64, offered: Vec<u64> },
    /// A pixel was asked for outside the buffer.
    #[error("pixel ({x}, {y}) is outside a {width}x{height} buffer")]
    OutOfBounds {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ THE CONTAINMENT INVARIANT, ENFORCED RATHER THAN PROMISED.
    ///
    /// The whole justification for reaching Vulkan directly is that the unsafe
    /// is contained to ONE auditable seam. That claim is worth exactly as much
    /// as a check on it, so: every file in this crate except `vk.rs` must be
    /// free of `unsafe`.
    ///
    /// Scans code, not prose — a doc comment discussing unsafe is not unsafe,
    /// and both of omoya's source-scanning gates caught their own commentary
    /// on the first run before they learned to cut at `//`.
    #[test]
    fn unsafe_lives_only_in_the_seam() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut scanned = 0usize;
        // ★ SKIP OFF-TREE RATHER THAN FAIL. This binary is deliberately run
        // on plo — the machine with the GPU — where the source it scans does
        // not exist. A source test that fails for want of source would make
        // every hardware run report a defect that is not there, and the real
        // GPU failures would be lost in it.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // ★ SKIPS OFF-TREE, but REFUSES to skip where a GPU run is
            // required — the same rule the Vulkan tests obey. A source seal
            // that prints SKIP and reports `ok` is a seal that stopped
            // sealing, and the SKIP line interleaves with another test's
            // result, attributing it to the wrong test.
            assert!(
                std::env::var_os("OMOYA_REQUIRE_GPU").is_none(),
                "the source seal cannot run: {} is absent, and \
                 OMOYA_REQUIRE_GPU is set — this run was supposed to be \
                 on-tree",
                dir.display()
            );
            eprintln!(
                "SKIP: {} is not present — source scan needs the source tree",
                dir.display()
            );
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            if path.file_name().is_some_and(|n| n == "vk.rs") {
                continue; // THE seam, and the only one.
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            scanned += 1;
            // ★ Cut at the first `#[cfg(test)]` AND strip comments. Both
            // halves are needed and I learned that the hard way twice today:
            // omoya's two source scanners each caught their own doc comment on
            // the first run, and THIS one then caught its own test body — the
            // word `unsafe` appears in the matcher and in the failure message
            // below. A source-scanning test matches itself by default.
            let body = text.split("#[cfg(test)]").next().unwrap_or("");
            let code: String = body
                .lines()
                .map(|l| l.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            // ★ Match unsafe CONSTRUCTS, not the substring. The lint's own
            // name contains "unsafe", so a naive `contains` accuses the very
            // attributes that enforce the rule — which is what it did on the
            // first run of this version.
            for pat in ["unsafe {", "unsafe fn", "unsafe impl", "unsafe trait"] {
                if code.contains(pat) {
                    offenders.push(format!("{}: {pat}", path.display()));
                }
            }
        }
        // The denominator, inside the assertion: a broken walk would otherwise
        // find no offenders and pass while checking nothing.
        assert!(scanned >= 1, "scanned {scanned} files — the walk broke");
        assert!(
            offenders.is_empty(),
            "unsafe escaped the seam into {offenders:?}. The entire argument \
             for binding Vulkan directly is that unsafe is contained to vk.rs; \
             move it back or the argument is gone."
        );

        // ★ THE HALF THE LINT CANNOT CHECK: `#![deny(unsafe_code)]` refuses a
        // stray `unsafe` block at compile time, but it is powerless against
        // somebody widening the licence with a second `#[allow(unsafe_code)]`.
        // Exactly one may exist, and it is the one on `mod vk`.
        let mut licences = 0usize;
        for e in std::fs::read_dir(&dir).expect("read src/").flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // ★ Comments stripped HERE TOO. Counting raw text found three:
            // the real attribute, plus two doc comments that merely NAME it
            // while explaining this very rule. Third time today a
            // source-scanning check matched its own commentary — it is the
            // default behaviour of the technique, not an unlucky coincidence.
            let body: String = text
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .lines()
                .map(|l| l.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            licences += body.matches("allow(unsafe_code)").count();
        }
        assert_eq!(
            licences, 1,
            "the crate carries {licences} `allow(unsafe_code)` licences; \
             exactly one may exist (on `mod vk`), or the compile-time \
             containment has been quietly widened"
        );
    }

    /// ★ STRIDE IS NOT WIDTH * 4, and the type must not let anyone forget.
    ///
    /// Drivers pad rows. Deriving stride from width reads a correct buffer as
    /// diagonal garbage, which looks like a corrupt client rather than a
    /// arithmetic bug in the compositor.
    #[test]
    fn a_padded_row_is_addressed_by_the_drivers_stride() {
        let g = Geometry {
            width: 3,
            height: 2,
            stride: 64, // padded far beyond 3 * 4 = 12
            offset: 0,
        };
        assert_eq!(g.byte_offset(0, 0), Some(0));
        assert_eq!(g.byte_offset(2, 0), Some(8));
        // Row 1 starts at the STRIDE, not at width * 4.
        assert_eq!(g.byte_offset(0, 1), Some(64));
        assert_ne!(
            g.byte_offset(0, 1),
            Some(12),
            "row 1 was addressed as width*4 — every row after the first would \
             be read from the wrong place"
        );
        assert_eq!(g.min_size(), 128);
    }

    /// ★ AND OUT OF BOUNDS IS `None`, NOT A WRAPPED OFFSET.
    #[test]
    fn a_pixel_outside_the_buffer_has_no_offset() {
        let g = Geometry {
            width: 4,
            height: 4,
            stride: 16,
            offset: 0,
        };
        assert_eq!(g.byte_offset(4, 0), None);
        assert_eq!(g.byte_offset(0, 4), None);
        assert!(g.byte_offset(3, 3).is_some());
    }

    /// ★ AN OFFSET IS HONOURED — a dmabuf plane need not start at byte zero,
    /// and ignoring the offset reads the wrong plane entirely.
    #[test]
    fn a_plane_offset_shifts_every_pixel() {
        let g = Geometry {
            width: 2,
            height: 2,
            stride: 8,
            offset: 4096,
        };
        assert_eq!(g.byte_offset(0, 0), Some(4096));
        assert_eq!(g.byte_offset(1, 1), Some(4096 + 8 + 4));
    }

    /// ★ EVERY UNAVAILABLE ARM SAYS SOMETHING DIFFERENT.
    ///
    /// The arms exist to send a reader to different places; two arms rendering
    /// the same words would defeat that silently. This is the same rule
    /// `garasu::CpuFallback` needed after "no hardware GPU adapter on this
    /// machine" was printed on a box holding an RTX 3070.
    #[test]
    fn no_two_unavailable_arms_read_alike() {
        let arms = [
            Unavailable::NoLoader,
            Unavailable::UnsupportedPlatform,
            Unavailable::NoPhysicalDevice,
            Unavailable::NoDeviceWithDmabuf { examined: 2 },
            Unavailable::Driver("ERROR_DEVICE_LOST".into()),
        ];
        let mut seen: Vec<String> = Vec::new();
        for a in &arms {
            let s = a.to_string();
            assert!(
                !s.is_empty(),
                "an arm rendered empty, which reads as no answer at all"
            );
            assert!(
                !seen.contains(&s),
                "two Unavailable arms render identically: {s:?}"
            );
            seen.push(s);
        }
        // The denominator: the arm count is asserted so adding one without a
        // row here is visible rather than silently unexercised.
        assert_eq!(seen.len(), 5);
    }

    /// ★ `NoDeviceWithDmabuf` CARRIES ITS DENOMINATOR.
    ///
    /// "no device supports it" after examining zero devices is a different
    /// fact from the same sentence after examining four, and the difference is
    /// exactly the vacuity this fleet keeps rediscovering.
    #[test]
    fn the_capability_refusal_reports_how_many_it_looked_at() {
        let none = Unavailable::NoDeviceWithDmabuf { examined: 0 }.to_string();
        let some = Unavailable::NoDeviceWithDmabuf { examined: 4 }.to_string();
        assert!(none.contains('0'), "{none}");
        assert!(some.contains('4'), "{some}");
        assert_ne!(none, some);
    }
    /// ★ THE SPIR-V IS REAL, checked without a GPU.
    ///
    /// `build.rs` could write an empty file, or a file in the wrong byte
    /// order, and every GPU test would then skip on a machine with no loader
    /// and report green. This reads the blob's own header, so it fails on
    /// CI, on darwin, and anywhere else — the half of the shader path that
    /// does not need a driver is checked where a driver is not available.
    #[test]
    fn the_compiled_shader_is_a_spirv_module_with_the_three_entry_points() {
        assert!(
            COMPOSITE_SPV.len() >= 20 && COMPOSITE_SPV.len() % 4 == 0,
            "SPIR-V is a stream of 32-bit words with a 5-word header; got {} bytes",
            COMPOSITE_SPV.len()
        );

        let word = |i: usize| {
            u32::from_le_bytes([
                COMPOSITE_SPV[i * 4],
                COMPOSITE_SPV[i * 4 + 1],
                COMPOSITE_SPV[i * 4 + 2],
                COMPOSITE_SPV[i * 4 + 3],
            ])
        };

        // The magic number is also the ENDIANNESS check: a big-endian file
        // reads as 0x03022307 here, and a driver handed one rejects the whole
        // module for a reason that looks nothing like byte order.
        assert_eq!(
            word(0),
            0x0723_0203,
            "not SPIR-V, or written big-endian (0x03022307 is the byte-swapped magic)"
        );

        // ★ THE ENTRY POINTS ARE THE PART THAT CAN SILENTLY DRIFT. Renaming a
        // function in the WGSL still compiles and still emits a valid module;
        // the failure lands much later, at pipeline creation, as a driver
        // error naming a string. Tying the names here means a rename is
        // caught in the crate that depends on them.
        let names = spirv_entry_point_names(COMPOSITE_SPV);
        for expected in [
            entry::VERTEX,
            entry::FRAGMENT_TEXTURE,
            entry::FRAGMENT_SOLID,
        ] {
            // The constants carry their NUL because Vulkan needs it; the
            // SPIR-V name does not, so compare the Rust-visible part.
            let want = std::str::from_utf8(&expected[..expected.len() - 1]).unwrap();
            assert!(
                names.iter().any(|n| n == want),
                "composite.wgsl no longer exports `{want}` — found {names:?}"
            );
        }
    }

    /// Walk a SPIR-V module's instruction stream and collect `OpEntryPoint`
    /// names. Small enough to be worth having in-tree rather than taking a
    /// SPIR-V parsing dependency for one assertion.
    fn spirv_entry_point_names(blob: &[u8]) -> Vec<String> {
        const OP_ENTRY_POINT: u32 = 15;
        let words: Vec<u32> = blob
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let mut names = Vec::new();
        // Word 5 is the first instruction; 0..4 are the header.
        let mut i = 5;
        while i < words.len() {
            let len = (words[i] >> 16) as usize;
            let op = words[i] & 0xffff;
            if len == 0 {
                break; // malformed; the header test above is the real guard
            }
            if op == OP_ENTRY_POINT && i + 3 < words.len() {
                // Layout: [op] [execution model] [entry id] [name...]
                let bytes: Vec<u8> = words[i + 3..(i + len).min(words.len())]
                    .iter()
                    .flat_map(|w| w.to_le_bytes())
                    .collect();
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                if let Ok(s) = std::str::from_utf8(&bytes[..end]) {
                    names.push(s.to_owned());
                }
            }
            i += len;
        }
        names
    }

    /// ★ THE CLIP-SPACE TRANSFORM, which is pure arithmetic and the easiest
    /// thing here to get subtly wrong — a sign error draws the whole desktop
    /// upside down, which looks like a driver bug and is not one.
    #[test]
    fn a_fullscreen_rect_covers_exactly_the_clip_volume() {
        let got = Params::dst_from_pixels([0.0, 0.0, 1920.0, 1080.0], (1920.0, 1080.0));
        assert_eq!(
            got,
            [-1.0, -1.0, 2.0, 2.0],
            "a full-output rect must map to the whole clip volume: origin at \
             (-1,-1) and extent 2x2"
        );
    }

    /// ★ Y IS NOT FLIPPED. Vulkan's framebuffer Y points down and clip -1 is
    /// the TOP, which is already the direction a compositor thinks in. The
    /// reflex — flipping Y "because OpenGL" — produces an upside-down desktop
    /// that a screenshot renders correctly, so it is diagnosed on hardware
    /// rather than in a test. This pins the direction.
    #[test]
    fn the_top_of_the_screen_maps_to_the_top_of_clip_space() {
        let top = Params::dst_from_pixels([0.0, 0.0, 100.0, 100.0], (1000.0, 1000.0));
        let bottom = Params::dst_from_pixels([0.0, 900.0, 100.0, 100.0], (1000.0, 1000.0));
        assert!(
            top[1] < bottom[1],
            "a rect at pixel y=0 must land above one at y=900; got {} vs {}",
            top[1],
            bottom[1]
        );
        assert_eq!(top[1], -1.0, "pixel y=0 is clip y=-1");
    }

    /// ★ A HALF-SIZED CENTRED RECT, so the scale and the bias are both pinned
    /// rather than only their sum — the fullscreen case above passes for a
    /// transform that is wrong in a way the two errors cancel.
    #[test]
    fn a_centred_quarter_rect_maps_to_the_middle_of_clip_space() {
        let got = Params::dst_from_pixels([480.0, 270.0, 960.0, 540.0], (1920.0, 1080.0));
        assert_eq!(got, [-0.5, -0.5, 1.0, 1.0]);
    }

    /// ★ THE PUSH-CONSTANT RANGE MATCHES THE STRUCT. `SIZE` is derived from
    /// the type so this cannot drift, and the test says why that matters: a
    /// range smaller than the struct passes Vulkan validation and hands the
    /// shader fewer bytes than it reads.
    #[test]
    fn the_push_constant_size_is_the_three_vectors_the_shader_declares() {
        assert_eq!(Params::SIZE, 48, "three vec4<f32> is 48 bytes");
        assert_eq!(
            Params::SIZE as usize,
            core::mem::size_of::<Params>(),
            "the declared range and the struct must not be able to disagree"
        );
    }
}
