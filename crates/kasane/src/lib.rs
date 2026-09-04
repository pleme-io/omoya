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
}
