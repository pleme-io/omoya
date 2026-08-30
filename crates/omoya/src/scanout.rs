//! Direct scanout — `DrmSurface` page flips, no `DrmCompositor`, no libgbm.
//!
//! ── ★ WHY THIS EXISTS ─────────────────────────────────────────────────────
//! `libgbm.so.1` is linked by smithay's `backend_gbm` feature, and that feature
//! exists in omoya's list for exactly one reason: `DrmCompositor` is gated
//! behind it (`backend/drm/mod.rs:73`). **No `GbmDevice` is ever constructed** —
//! the runtime path is dumb buffers and the gbm slot is passed `None`. So the
//! library is linked to satisfy a compile-time gate on a type we use, not to do
//! any work.
//!
//! Removing it means not using `DrmCompositor`. `DrmSurface` is **not gated**
//! (`mod.rs:101`), and neither is the dumb allocator (`backend_drm` only), so
//! the pieces to drive scanout directly are all still there.
//!
//! ── ★ WHAT IS AND IS NOT BEING RE-DERIVED ─────────────────────────────────
//! `DrmCompositor` is ~4400 lines, and it would be dishonest to imply this
//! replaces them. Almost all of that is **multi-plane assignment**, damage
//! tracking across overlay planes, format/modifier negotiation, and buffer-age
//! bookkeeping for partial repaint.
//!
//! A single-output, single-plane, dumb-buffer seat uses none of it. What is
//! needed is: two buffers, render into the back one, export it as a framebuffer,
//! flip, wait for vblank, swap. That is this file.
//!
//! **The cost is real and named:** no overlay planes, so no hardware cursor and
//! no zero-copy video.
//!
//! **Partial repaint is NO LONGER among the costs** — this header said "full
//! repaint every frame — the damage rectangles are computed and then ignored"
//! and that is now false. The buffer-age bookkeeping it called deliberately
//! not-re-derived IS re-derived: `back_buffer_age` below, feeding smithay's
//! `OutputDamageTracker` from `drm.rs`. Measured on vkms: **184 idle render
//! ticks produced 0 presentations.**
//!
//! ★ The re-derivation carried one defect worth remembering, because a
//! single-window seat could not expose it: smithay hands a renderer each
//! element's damage **relative to that element**, and nuri clipped it against
//! the absolute destination. Every window away from the origin drew nothing,
//! silently and indistinguishably from correct occlusion. See the note in
//! `nuri_renderer::draw_solid`.
//!
//! `pending-omoya-planes: overlay planes and a hardware cursor`

use smithay::backend::allocator::{Allocator, Buffer as _, Fourcc, Modifier, dumb::DumbBuffer};
use smithay::backend::drm::exporter::ExportFramebuffer;
use smithay::backend::drm::{DrmDeviceFd, DrmSurface, PlaneConfig, PlaneState};
use smithay::utils::{Physical, Rectangle, Transform};

/// One side of the flip chain.
///
/// ★ HOLDS THE FRAMEBUFFER OBJECT, NOT JUST ITS HANDLE. `add_framebuffer`
/// returns a `DumbFramebuffer` that OWNS the DRM framebuffer and removes it on
/// drop. Extracting the handle and letting the wrapper go would leave a handle
/// pointing at a framebuffer the kernel has already destroyed — and the flip
/// fails with an errno that says nothing about lifetimes.
struct Slot {
    buffer: DumbBuffer,
    // ★ Named by the ASSOCIATED TYPE, not by a module path. `DumbFramebuffer`
    // lives in `drm::dumb` and is reached through the exporter impl; spelling
    // the path invites guessing at a re-export that may not exist, while the
    // associated type is exactly what `add_framebuffer` returns by definition.
    framebuffer: <DrmDeviceFd as ExportFramebuffer<DumbBuffer>>::Framebuffer,
}

/// A double-buffered scanout over one CRTC.
///
/// ★ TWO BUFFERS, NOT ONE. Rendering into the buffer currently being scanned
/// out produces tearing that looks like a renderer bug — a torn frame and a
/// wrongly-composited frame are indistinguishable in a screenshot. Two is the
/// minimum that makes the class impossible rather than unlikely.
pub struct DirectScanout {
    surface: DrmSurface,
    slots: [Slot; 2],
    /// Which slot is safe to draw into.
    back: usize,
    /// Frames presented so far. Only ever used to derive buffer AGE.
    presented: u64,
    /// The value of `presented` when each slot was last drawn into.
    ///
    /// ★ BUFFER AGE IS WHAT MAKES PARTIAL REPAINT CORRECT. With two
    /// alternating buffers, the region that must be redrawn is not this
    /// frame's damage — it is everything that changed since THIS buffer was
    /// last the one on screen. Repainting only the current frame's damage
    /// leaves the other buffer's stale pixels visible on alternate frames,
    /// which does not look like a damage bug: it looks like tearing or a
    /// flickering ghost, and sends the reader to the wrong subsystem.
    ///
    /// `0` means "never drawn into", which smithay's damage tracker reads as
    /// "no history — repaint everything". That is the correct answer for a
    /// fresh buffer and the reason age is an Option-shaped 0 rather than a
    /// guess.
    last_drawn: [Option<u64>; 2],
    /// ★ HAS THE CRTC EVER BEEN MODESET?
    ///
    /// `page_flip` documents itself as *"This will not cause the crtc to
    /// modeset"*, and a flip against a CRTC that was never modeset is rejected
    /// with `EINVAL`. `commit` is the call that sets the mode.
    ///
    /// Without this the very first frame — and therefore every frame — failed,
    /// and it failed **quietly**: the error went to the log while the vkms gate
    /// asserted only that the process was alive and holding the display, which
    /// a compositor presenting nothing at all still satisfies. Measured on
    /// vkms: `Page flip commit failed … (Invalid argument (os error 22))`, once
    /// per frame, on a test that passed.
    modeset: bool,
}

/// What can go wrong driving scanout directly.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("allocating a scanout buffer: {0}")]
    Allocate(String),
    #[error("exporting a framebuffer: {0}")]
    Export(String),
    #[error("the DRM surface refused the flip: {0}")]
    Flip(String),
    #[error("no primary plane on this CRTC")]
    NoPrimaryPlane,
}

impl DirectScanout {
    /// Allocate the flip chain and take the first frame's slot.
    ///
    /// # Errors
    /// If a buffer cannot be allocated or exported as a framebuffer.
    pub fn new<A>(
        surface: DrmSurface,
        allocator: &mut A,
        fd: &DrmDeviceFd,
        size: (u32, u32),
        fourcc: Fourcc,
    ) -> Result<Self, Error>
    where
        A: Allocator<Buffer = DumbBuffer>,
        A::Error: std::fmt::Display,
    {
        // ★ Linear, always. A dumb buffer IS linear — smithay's own allocator
        // rejects any other modifier — and it is also the only layout a CPU
        // rasterizer can address. The two constraints agree, which is why the
        // software path and the dumb-buffer path belong together.
        let mut make = || -> Result<Slot, Error> {
            let buffer = allocator
                .create_buffer(size.0, size.1, fourcc, &[Modifier::Linear])
                .map_err(|e| Error::Allocate(e.to_string()))?;
            let framebuffer = fd
                .add_framebuffer(
                    fd,
                    smithay::backend::drm::exporter::ExportBuffer::Allocator(&buffer),
                    // `use_opaque = true`: the scanout target has no alpha to
                    // blend against — there is nothing behind the screen.
                    // Requesting an alpha format here makes some drivers refuse
                    // the framebuffer outright.
                    true,
                )
                .map_err(|e| Error::Export(format!("{e:?}")))?
                .ok_or_else(|| Error::Export("driver returned no framebuffer".into()))?;
            Ok(Slot {
                buffer,
                framebuffer,
            })
        };

        let slots = [make()?, make()?];
        Ok(Self {
            surface,
            slots,
            back: 0,
            presented: 0,
            last_drawn: [None; 2],
            modeset: false,
        })
    }

    /// The buffer to render into this frame.
    /// How many frames ago the back buffer was last drawn into.
    ///
    /// This is the `age` smithay's `OutputDamageTracker` takes: 0 means "no
    /// usable history, repaint everything", N means "this buffer's contents
    /// are N frames stale". Returning 0 when a slot has never been used is not
    /// a fallback — it is the protocol's own way of saying so.
    #[must_use]
    pub fn back_buffer_age(&self) -> usize {
        match self.last_drawn[self.back] {
            None => 0,
            Some(when) => usize::try_from(self.presented.saturating_sub(when) + 1).unwrap_or(0),
        }
    }

    pub fn back_buffer(&mut self) -> &mut DumbBuffer {
        &mut self.slots[self.back].buffer
    }

    /// Present the back buffer and swap.
    ///
    /// # Errors
    /// If the CRTC refuses the flip.
    pub fn flip(&mut self) -> Result<(), Error> {
        let slot = &self.slots[self.back];
        let (w, h) = (
            i32::from(slot.buffer.size().w as u16),
            i32::from(slot.buffer.size().h as u16),
        );

        let plane = self.surface.plane();
        let state = PlaneState {
            handle: plane,
            config: Some(PlaneConfig {
                src: Rectangle::from_size((f64::from(w), f64::from(h)).into()),
                dst: Rectangle::<i32, Physical>::from_size((w, h).into()),
                transform: Transform::Normal,
                alpha: 1.0,
                // ★ No damage clips: this is a full repaint. See the header —
                // partial repaint into an alternating back buffer needs the
                // PREVIOUS frame's damage as well, and inventing a clip list
                // from this frame's damage alone would leave stale pixels in
                // the other buffer.
                damage_clips: None,
                // `AsRef<framebuffer::Handle>` — the handle is borrowed from
                // the owning wrapper, which stays in `slot`.
                // ★ Through `AsRef`, with the target named. `Framebuffer` is a
                // TRAIT (`Framebuffer: AsRef<framebuffer::Handle>`), so calling
                // `Framebuffer::as_ref` asks for a trait object; the borrow we
                // want comes from the AsRef impl, and naming `Handle` keeps
                // inference from having to pick among several.
                fb: *AsRef::<smithay::reexports::drm::control::framebuffer::Handle>::as_ref(
                    &slot.framebuffer,
                ),
                fence: None,
            }),
        };

        // The first frame sets the mode; later frames flip. `commit_pending`
        // also goes true when a connector or mode changed under us, so a
        // hotplug re-modesets rather than flipping into a stale configuration.
        if self.modeset && !self.surface.commit_pending() {
            self.surface
                .page_flip([state], true)
                .map_err(|e| Error::Flip(e.to_string()))?;
        } else {
            self.surface
                .commit([state], true)
                .map_err(|e| Error::Flip(e.to_string()))?;
            self.modeset = true;
        }

        // ★ Swap AFTER the flip is accepted, not before. Swapping first and
        // then failing would leave the next frame drawing into the buffer the
        // display is actively scanning out.
        // Record that this slot now holds a presented frame, THEN swap. The
        // order matters: `back` still names the slot we just drew into.
        self.presented += 1;
        self.last_drawn[self.back] = Some(self.presented);
        self.back = 1 - self.back;
        Ok(())
    }
}
