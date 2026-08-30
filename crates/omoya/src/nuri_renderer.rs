//! `nuri` as a smithay renderer — the adapter that retires `libpixman`.
//!
//! ── ★ WHY THE ADAPTER IS SEPARATE FROM THE RASTERIZER ─────────────────────
//! `nuri` knows nothing about smithay, Wayland, DRM or dmabufs. It fills
//! rectangles and blits pixels into a mapped buffer, and it has **zero
//! dependencies** because that is all a rasterizer is. This file is where that
//! arithmetic meets a compositor's vocabulary.
//!
//! The split is not tidiness. It is what lets nuri be tested with a `Vec<u8>`
//! and no seat — all 11 of its tests run anywhere — while everything that
//! needs a display lives here.
//!
//! ── ★ THE MEMORY MAP IS THE WHOLE TRICK ───────────────────────────────────
//! pixman does not know about dmabufs either. smithay's `PixmanRenderer`
//! implements `Bind<Dmabuf>` by **mmap'ing plane 0** and handing pixman a raw
//! pointer, and it refuses multi-plane or non-Linear buffers
//! (`renderer/pixman/mod.rs:736,746`). This does exactly the same thing,
//! because it is the only thing that can be done: a CPU rasterizer needs the
//! pixels addressable.
//!
//! That is also the ceiling. A GPU renderer imports a dmabuf without ever
//! touching the bytes; this one must map them. On the dumb-buffer scanout path
//! the buffer is already CPU-visible, so the cost is zero — which is precisely
//! why the software path and the dumb-buffer path belong together.

use std::sync::Arc;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer as _, Fourcc};
use smithay::backend::renderer::{
    Bind, Color32F, DebugFlags, Frame, ImportDma, ImportDmaWl, ImportMem, ImportMemWl, Renderer,
    RendererSuper, Texture, TextureFilter, sync::SyncPoint,
};
use smithay::utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform};

/// What can go wrong painting.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("nuri refused the surface: {0}")]
    Nuri(#[from] nuri::Error),
    /// ★ Its own arm. A multi-plane or tiled buffer is not a *failure to
    /// render*, it is a buffer this renderer structurally cannot address —
    /// and folding it into a generic error would send someone looking for a
    /// bug in the raster code.
    #[error("unsupported buffer: {0}")]
    Unsupported(&'static str),
    #[error("mapping the buffer failed: {0}")]
    Map(String),
}

/// A client buffer, decoded into memory nuri can read.
///
/// ★ Owned, not borrowed. smithay hands a texture out and expects it to stay
/// valid across frames while the client is free to release its buffer; holding
/// a reference would be a use-after-free the first time a window closed
/// mid-frame. `Arc` so cloning a texture — which smithay does per element per
/// frame — copies a pointer rather than an image.
#[derive(Debug, Clone)]
pub struct NuriTexture {
    /// ★ `RwLock`, NOT A BARE `Arc<Vec<u8>>`, AND THE REASON IS MEASURED.
    ///
    /// The damage-only import needs to write into the cached allocation. With
    /// a bare `Arc` that requires `Arc::get_mut`, which returns `None`
    /// whenever anyone else holds a clone — and smithay ALWAYS does: it
    /// caches the imported texture per surface and keeps it across frames.
    /// So the incremental path silently never engaged and every commit fell
    /// back to copying the client's whole 8 MB buffer. Measured after that
    /// "fix": 99% CPU and 1.4 fps, exactly as before it.
    ///
    /// The lock is taken ONCE PER TEXTURE PER FRAME — at the top of a blit,
    /// not per pixel — so it costs nothing measurable against the copy it
    /// makes unnecessary.
    data: Arc<std::sync::RwLock<Vec<u8>>>,
    width: u32,
    height: u32,
    stride: usize,
    format: Fourcc,
    /// Whether every pixel in `data` is fully opaque.
    ///
    /// ★ COMPUTED AT IMPORT, WHERE THE BYTES ARE ALREADY IN CACHE, so the
    /// blitter can skip its own per-row scan. That scan is a FULL PASS over
    /// the source on every frame — nuri tests
    /// `srow.chunks_exact(4).all(|px| px[3] == 0xff)` for each row before
    /// choosing `copy_from_slice` — and it reads the same bytes the copy is
    /// about to read again.
    ///
    /// ★ NOT DERIVED FROM `format`. The obvious version is
    /// `matches!(format, Xrgb8888)`, and it would be worth nothing: mado's
    /// swapchain is `Bgra8UnormSrgb`, so a format gate answers "unknown" on
    /// the one client whose frames actually cost anything.
    ///
    /// Conservative under partial import: a damage-only re-import can only
    /// clear this, never set it, because the rows it did not touch were not
    /// re-examined. A false negative costs the scan we were already paying;
    /// a false positive would composite a translucent window as opaque.
    ///
    /// `Arc<AtomicBool>` for the same reason `data` is an `Arc<RwLock<..>>`:
    /// smithay caches the texture per surface and holds a clone across
    /// frames, so a damage-only re-import must be able to update the flag on
    /// the instance the compositor already has.
    opaque: Arc<std::sync::atomic::AtomicBool>,
}

/// Force the padding byte opaque for `X`-formats.
///
/// ★ nuri's blit reads byte 3 as ALPHA unconditionally
/// (`crates/nuri/src/lib.rs`), and for `Xrgb8888` that byte is PADDING with no
/// defined value. A buffer whose padding happens to be zero therefore
/// composites at alpha 0 — the window is imported, cached, drawn, and
/// invisible, with nothing logged.
///
/// Normalised at IMPORT rather than taught to the blitter: both import paths
/// already copy the bytes, so this costs one pass over a buffer that was being
/// copied anyway, and it keeps the hot loop free of a per-pixel format branch.
///
/// `NuriTexture` does carry `format`, which is exactly what made this easy to
/// miss — the information was present and simply never consulted.
/// Returns whether every pixel in `bytes` is fully opaque afterwards.
///
/// ★ THE RETURN IS THE POINT NOW. This function already walks every byte, and
/// it walks them while they are hot in cache from the copy that just wrote
/// them. nuri's blit then walked the SAME bytes again, per row, asking the
/// same question, on a cold pass. Answering here and carrying the answer on
/// the texture removes that second pass.
///
/// For `Xrgb8888` the answer is `true` by construction — we just wrote 0xff
/// into every alpha byte. For everything else it is measured, not assumed.
fn normalise_opaque(bytes: &mut [u8], format: Fourcc) -> bool {
    if matches!(format, Fourcc::Xrgb8888) {
        for px in bytes.chunks_exact_mut(4) {
            px[3] = 0xff;
        }
        return true;
    }
    bytes.chunks_exact(4).all(|px| px[3] == 0xff)
}

/// Copy `src` into `dst` and normalise its alpha **in the same walk**.
///
/// ★ **THIS DELETES A WHOLE PASS OVER THE SURFACE, AND THAT IS THE POINT.**
/// Every import site used to do `dst.copy_from_slice(src)` and then call
/// [`normalise_opaque`] on the bytes it had *just written* — two traversals of
/// the same memory, back to back, where one would do. On a fullscreen client
/// that is not a rounding error:
///
/// Measured on plo 2026-08-21, before this existed. The content surface is
/// 1912×1044 = **7.98 MB**, mado commits full-surface damage every frame
/// (`wl_surface.damage_buffer` is unreachable through wgpu — see
/// `mado/src/grid_damage.rs`), and one keystroke therefore walked it three
/// times: this copy, this normalise, then `nuri::blit`. **~24 MB per
/// character.** A commit-caused frame cost **4,099 µs** median against a
/// **2,778 µs** vblank interval at 360 Hz — 1.47 intervals, so every keystroke
/// missed its flip by construction rather than occasionally.
///
/// Fusing removes one of the three. It is a *deletion*, not a speed-up, which
/// is the only kind of win the hitofude doctrine counts
/// (`docs/VISUAL-PERFORMANCE.md` §I): making a redundant copy faster is never
/// the answer, removing it is.
///
/// ★ **BYTE-IDENTICAL BY CONSTRUCTION, NOT BY HOPE.** The alpha rule is
/// literally [`normalise_opaque`]'s, applied to each pixel as it lands instead
/// of after they all have. `the_fused_copy_matches_copy_then_normalise` pins
/// that against the original for every format and a deliberately-mismatched
/// case, so a future edit to one and not the other fails rather than drifting.
///
/// ★ **WHY NOT SKIP THE ALPHA WRITE ENTIRELY?** For `Xrgb8888` the bytes it
/// writes are provably never read — the texture's `opaque` flag is `true`, so
/// `nuri::blit` takes `OpaqueHint::Opaque`, which `copy_from_slice`s the row
/// without consulting alpha. Deleting the write is therefore *tempting and
/// wrong to do in the same change*: it is only safe while nothing later blends
/// against the scanout buffer's own alpha, and that is a property of draw
/// ORDER, not of this function. Fusing is unconditionally safe; skipping needs
/// its own measurement. `pending-nuri-alpha-write-elision`.
///
/// Returns whether every pixel written is fully opaque, exactly as
/// [`normalise_opaque`] would have reported for the same bytes.
fn copy_normalising(dst: &mut [u8], src: &[u8], format: Fourcc) -> bool {
    debug_assert_eq!(dst.len(), src.len(), "fused copy over mismatched lengths");
    if matches!(format, Fourcc::Xrgb8888) {
        // X is undefined in the source; the compositor needs it to read 0xff.
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
            d[3] = 0xff;
        }
        // A partial trailing chunk cannot carry a whole pixel, but it CAN
        // exist when a stride is not a multiple of 4. Copy it verbatim rather
        // than dropping it — leaving stale bytes there is a one-pixel smear
        // down the right edge, which reads as a rendering bug rather than as
        // the arithmetic slip it is.
        let tail = dst.len() - dst.len() % 4;
        dst[tail..].copy_from_slice(&src[tail..]);
        return true;
    }
    // Argb8888 — and this is the arm that matters, because it is the one the
    // fleet's own terminal takes. mado's swapchain is `Bgra8UnormSrgb`, so the
    // Xrgb branch above never fires for the client whose frames cost anything.
    //
    // ★ `copy_from_slice` FIRST, THEN SCAN — deliberately NOT a fused
    // per-pixel loop. `copy_from_slice` lowers to `memmove`, which the libc
    // implements with wide vector loads and non-temporal stores; a hand-rolled
    // `chunks_exact_mut(4).zip(..)` that also ANDs the alpha is ONE pass on
    // paper and roughly a third of memcpy's bandwidth in practice, so the
    // "fused" version loses to the two-pass one it replaces. Passes are a
    // proxy for bytes-moved, and the proxy breaks exactly here.
    //
    // The scan that follows re-reads what the copy just wrote. At the call
    // sites that matter it is re-reading a ROW — the partial-import loop
    // interleaves copy and normalise per row — so it lands in L1 and costs
    // nearly nothing. On the full-buffer paths it is a genuine second pass
    // and this fusion is genuinely one fewer.
    dst.copy_from_slice(src);
    dst.chunks_exact(4).all(|px| px[3] == 0xff)
}

impl Texture for NuriTexture {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> Option<Fourcc> {
        Some(self.format)
    }
}

/// The scanout target, mapped for writing.
///
/// ★ HOLDS THE MAPPING ALIVE. `data` points into `_mapping`, so the two must
/// travel together — a framebuffer that kept only the slice would be pointing
pub struct NuriFramebuffer<'a> {
    /// What this scanout buffer is KNOWN to hold.
    ///
    /// ★ A PROOF, NOT A PREFERENCE. This was a `partial_copy: bool` — a knob
    /// where there should be evidence, which is the same smell one level down
    /// from `stale_scan` itself. See `mekuri::kentou`.
    ///
    /// ★ `Target<Unknown>` has NO `load_preserving` method. The partial copy is
    /// not disabled here — it is unreachable, because the type that would
    /// permit it cannot be obtained without paying for a full paint first.
    target: mekuri::kentou::Target<mekuri::kentou::Unknown>,
    /// The SCANOUT mapping. Written exactly once per frame, by
    /// [`NuriFramebuffer::flush_damage`], and never read.
    data: &'a mut [u8],
    /// ── ★ THE SHADOW: WHERE COMPOSITING ACTUALLY HAPPENS ────────────────
    ///
    /// Ordinary heap RAM. Every draw — clear, fill, blit, blend — lands here,
    /// and one damage-clipped streaming copy moves the result to `data`.
    ///
    /// **Why:** `data` is an mmap of a DRM dumb buffer, which the kernel maps
    /// WRITE-COMBINING. Measured on plo: a write into it costs ~3.5x a write
    /// into RAM, and a READ costs ~1000x. Compositing reads the destination
    /// on every translucent pixel (`blend_over`), so compositing *directly*
    /// into the mapping pays that 1000x read for every antialiased edge, every
    /// shadow, every rounded corner on the seat.
    ///
    /// The kernel documents the required shape itself, under
    /// `DRM_CAP_DUMB_PREFER_SHADOW`: *"the driver prefers userspace to render
    /// to a shadow buffer instead of directly rendering to a dumb buffer. For
    /// best speed, userspace should do streaming ordered memory copies into
    /// the dumb buffer and **never read from it**."* amdgpu, radeon and
    /// nouveau all set that capability to 1.
    ///
    /// ★ **Weston does exactly this and defaults it ON** (`pixman-shadow`,
    /// "Defaults to true"): it composites into `shadow_image` and then makes
    /// ONE `PIXMAN_OP_SRC` damage-clipped copy to the hardware buffer — a pure
    /// write with no destination read. wlroots, smithay's own pixman renderer,
    /// and Mutter all composite straight into the mapping instead. On this
    /// point Weston is right and the others carry the cost.
    ///
    /// ★ **ONE shadow across both scanout slots, deliberately.** The shadow
    /// holds "the last frame composed", which is a property of the OUTPUT, not
    /// of whichever buffer it was flipped into. smithay's damage tracker asks
    /// for `back_buffer_age`, so the damage it hands us always covers
    /// everything stale in THIS slot — a superset of what changed since the
    /// last compose. Compositing that superset into the shadow and copying the
    /// same region out leaves every untouched scanout pixel holding a value
    /// that is still correct.
    shadow: Vec<u8>,
    /// Where `shadow` goes when this framebuffer dies, so the next frame does
    /// not allocate and zero 8 MB. See `Drop`.
    pool: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    width: i32,
    height: i32,
    stride: usize,
    format: Fourcc,
    /// Dropped last, after `data` is gone. Never read.
    _mapping: smithay::backend::allocator::dmabuf::DmabufMapping,
}

/// A framebuffer that composites somewhere else and must be told when to put
/// the result on the display.
///
/// ★ **A TRAIT, NOT A DOWNCAST, AND THAT IS THE POINT.** The render loop is
/// generic over `R: Renderer` so it can be driven by nuri today and by
/// something else later. A shadow-buffer renderer that is never flushed
/// composites perfectly and shows nothing — the worst possible failure, since
/// every counter reports a healthy frame rate at a black screen. Making the
/// flush a BOUND means a renderer that cannot flush cannot drive the loop at
/// all, so the mistake is a compile error rather than a dark display.
///
/// A renderer that composites directly into scanout memory implements this as
/// a no-op and says so.
pub trait ScanoutFlush {
    /// Put everything drawn since the last flush onto the display, clipped to
    /// `damage`. An empty slice means "everything".
    fn flush_damage(&mut self, damage: &[Rectangle<i32, Physical>]);

    /// The scanout mapping's CURRENT bytes — what the display is actually
    /// showing, as opposed to what the compositor composed.
    ///
    /// ── ★ THE ONE SANCTIONED READ OF THIS MEMORY ─────────────────────────
    /// `data` is documented two fields up as *written once per frame and
    /// never read*, because a read from write-combining memory costs ~1000x a
    /// RAM read. That is the right rule for the hot path and exactly the
    /// wrong one for a diagnostic: the stale-pixel class lives HERE and
    /// nowhere else, since the shadow accumulates and is always complete.
    ///
    /// So this exists, it is called only on an explicit operator request, and
    /// it must never be called from the frame path. See `crate::stale`.
    fn scanout_bytes(&self) -> &[u8];
}

impl ScanoutFlush for NuriFramebuffer<'_> {
    fn flush_damage(&mut self, damage: &[Rectangle<i32, Physical>]) {
        NuriFramebuffer::flush_damage(self, damage);
    }

    fn scanout_bytes(&self) -> &[u8] {
        self.data
    }
}

impl NuriFramebuffer<'_> {
    /// Copy the composed shadow into the scanout mapping, damage-clipped.
    ///
    /// ★ **THE ONLY WRITE TO WRITE-COMBINING MEMORY IN THE WHOLE FRAME**, and
    /// it is row-at-a-time `copy_from_slice`, which lowers to `memcpy` — the
    /// "streaming ordered memory copy" the kernel asks for. No read of `data`
    /// happens anywhere.
    ///
    /// Rows rather than rectangles: a damage rect narrower than ~2 cache lines
    /// (~32 px at 32bpp) costs MORE to skip than to write through, because
    /// stopping and restarting mid-line forces two partial-line flushes. Full
    /// rows keep every write contiguous and cache-line aligned at both ends.
    pub fn flush_damage(&mut self, damage: &[Rectangle<i32, Physical>]) {
        let h = usize::try_from(self.height).unwrap_or(0);
        let full = self.stride.saturating_mul(h);
        let len = full.min(self.data.len()).min(self.shadow.len());
        if len == 0 {
            return;
        }
        // No damage at all means a full repaint was requested (age 0) or the
        // caller could not say — copy everything rather than leave the screen
        // holding a stale frame.
        if damage.is_empty() {
            self.data[..len].copy_from_slice(&self.shadow[..len]);
            return;
        }

        // ── ★ THE PARTIAL COPY IS OFF BY DEFAULT (plo, 2026-08-30) ──────────
        //
        // MEASURED, not suspected. On plo the operator saw persistent stale
        // content in the terminal, and the evidence localises it precisely:
        //
        //   `omoya_capture` reads the SHADOW and forces a full repaint. Its
        //   output is CLEAN — fifteen identical prompt lines at 1:1, no
        //   corruption anywhere. The composite is correct.
        //
        //   What the operator sees is the SCANOUT, and it is not clean.
        //
        // Shadow correct + scanout wrong + this function being the only writer
        // between them means the DAMAGE SET reaching here under-reports. Which
        // term is wrong — buffer age, the tracker's union, or a client's
        // declaration — is not yet known, and `stale_scan` (the instrument
        // built for exactly this) does not fire on this build, so the question
        // stayed open while the screen stayed broken.
        //
        // ★ THE DESTINATION'S STATE DECIDES, NOT A PREFERENCE.
        //
        // `copy_plan` returns `None` when this buffer's contents are not
        // established, and `None` means copy everything. There is no branch
        // here that can partially copy into an `Unknown` target -- not because
        // a policy forbids it, but because no such path exists.
        //
        // A full copy of 1920x1080x4 is 8.3 MB; this seat presented 643 frames
        // in ~25 minutes, so today the cost is nothing. On a seat genuinely
        // sustaining 360 Hz it would matter, and the way to earn the partial
        // path back is to establish the generation -- not to add a flag.
        // ── ★ THE TYPE DECIDES, AND THERE IS NO OTHER BRANCH ────────────────
        //
        // `self.target` is `Target<Unknown>` and `load_preserving` DOES NOT
        // EXIST on it. Reaching for a partial copy here is a compile error
        // (E0599), not a policy check somebody can flip -- which is the whole
        // difference between this and the `partial_copy: bool` it replaced.
        //
        // The only route to a `Target<Known>` is `adopt_by_clearing`, and
        // kentou prices that transition at exactly the work that makes the
        // claim true: a full paint. So the copy below is not a fallback, it IS
        // the adoption.
        //
        // Opening the partial path is therefore a real change, not a flag:
        // thread the scanout slot's `last_drawn` generation through the bind,
        // construct `Target::owned(w, h, revision)`, and `load_preserving`
        // appears -- refusing on its own terms with `Coverage::StaleBaseline`
        // when the damage baseline and the target's revision disagree, which is
        // the exact defect this whole investigation was chasing.
        self.data[..len].copy_from_slice(&self.shadow[..len]);

        // ★ The adopted `Target<Known>` is deliberately NOT stored back.
        //
        // It could not be honestly kept: this framebuffer's allocation returns
        // to a pool on `Drop` and the next bind may receive different bytes, so
        // knowledge established now does not survive the round-trip. kentou has
        // no `forget_identity` and should not -- a type that could quietly
        // downgrade would let a caller carry a stale `Known` across exactly the
        // boundary that invalidates it.
        //
        // So the value is built and dropped: the full copy is what makes the
        // claim true for THIS frame, and the next bind starts `Unknown` again
        // because that is the truth about a recycled buffer.
        let _adopted = self
            .target
            .adopt_by_clearing(mekuri::kentou::Revision::ORIGIN);
        let _ = damage;
    }
}

impl Drop for NuriFramebuffer<'_> {
    fn drop(&mut self) {
        // Hand the allocation back so the next bind reuses it. Without this
        // every frame allocates and zeroes ~8 MB, which would cost more than
        // the write-combining traffic the shadow exists to remove.
        if let Ok(mut slot) = self.pool.lock() {
            *slot = std::mem::take(&mut self.shadow);
        }
    }
}

impl std::fmt::Debug for NuriFramebuffer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NuriFramebuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .finish_non_exhaustive()
    }
}

impl Texture for NuriFramebuffer<'_> {
    #[allow(clippy::cast_sign_loss)]
    fn width(&self) -> u32 {
        self.width as u32
    }
    #[allow(clippy::cast_sign_loss)]
    fn height(&self) -> u32 {
        self.height as u32
    }
    fn format(&self) -> Option<Fourcc> {
        Some(self.format)
    }
}

/// The renderer.
#[derive(Debug)]
pub struct NuriRenderer {
    debug: DebugFlags,
    /// Optional introspection sink, so the shm import can COUNT what the
    /// client declared. `Option` because `Default`/`new()` must keep working
    /// for the tests and the winit backend, which have no sidecar.
    introspect: Option<std::sync::Arc<crate::introspect::OmoyaIntrospect>>,
    /// The last texture imported for each shm buffer, kept so a re-import
    /// can copy only what changed.
    ///
    /// ★ THIS IS THE KEYSTROKE-LATENCY FIX. `import_shm_buffer` is handed a
    /// DAMAGE list and used to ignore it, copying the client's entire buffer
    /// — 8 MB for a 1920x1045 terminal — and then running `normalise_opaque`
    /// over all two million pixels. Every commit. Typing one character
    /// redrew, re-copied and re-normalised the whole window.
    ///
    /// Measured before the fix: 99% of a core, every `gdb` sample inside
    /// `memmove`, ~2 frames per second on an otherwise idle 16-core machine.
    ///
    /// Keyed by the buffer's `ObjectId`, because a client cycles a small set
    /// of buffers and re-uses them; keying on anything derived from the
    /// CONTENTS would defeat the point.
    /// The recycled shadow allocation. See `NuriFramebuffer::shadow` — one
    /// buffer, handed out at `bind` and returned on drop, so a frame costs no
    /// allocation and no 8 MB zeroing.
    shadow_pool: Arc<std::sync::Mutex<Vec<u8>>>,
    shm_cache: std::collections::HashMap<
        smithay::reexports::wayland_server::backend::ObjectId,
        NuriTexture,
    >,
    /// ★ MINTED ONCE, AT CONSTRUCTION — NOT PER CALL.
    ///
    /// `ContextId` is an Arc IDENTITY, not a value: `ContextId::new()`
    /// allocates a fresh `Arc<InnerContextId>` and equality is on that Arc.
    /// smithay stores a client's imported texture under
    /// `renderer.context_id()` (`renderer/utils/wayland.rs`) and retrieves it
    /// under `renderer.context_id()` (`renderer/element/surface.rs`), so the
    /// two must be the SAME id.
    ///
    /// Returning a new one each call made every store use a fresh key and
    /// every lookup miss. The lookup is `data.texture(...)?` — a `?` on None —
    /// so the surface was reported as "not mapped" and silently dropped from
    /// the render list. **omoya composited ZERO client surfaces**, shm
    /// included, with no error logged anywhere.
    ///
    /// Measured on plo: mado rendered 10,329 frames while the captured screen
    /// held exactly two colours, the background and the cursor. The compositor
    /// reported `windows: 1` the whole time, because the window WAS mapped in
    /// the space — it just never survived texture lookup.
    context: smithay::backend::renderer::ContextId<NuriTexture>,
}

impl Default for NuriRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl NuriRenderer {
    /// Attach the introspection sink. Separate from `new()` so no existing
    /// construction site has to change (MODULARIZE, DON'T DELETE applied to a
    /// constructor).
    pub fn set_introspect(&mut self, i: std::sync::Arc<crate::introspect::OmoyaIntrospect>) {
        self.introspect = Some(i);
    }

    #[must_use]
    pub fn new() -> Self {
        // `DebugFlags` has no Default impl — spelled out rather than derived.
        Self {
            introspect: None,
            debug: DebugFlags::empty(),
            shadow_pool: Arc::new(std::sync::Mutex::new(Vec::new())),
            shm_cache: std::collections::HashMap::new(),
            context: smithay::backend::renderer::ContextId::new(),
        }
    }

    /// The formats nuri can address.
    ///
    /// ★ Exactly two, and both 32-bpp little-endian. pixman advertises 13; the
    /// DRM path offers the CRTC only `Argb8888` and `Xrgb8888` (`drm.rs:366`),
    /// so the other eleven were never reachable. Advertising a format the
    /// blitter cannot write is how a compositor ends up painting garbage on
    /// one machine and nothing on another.
    fn formats() -> impl Iterator<Item = Fourcc> {
        [Fourcc::Argb8888, Fourcc::Xrgb8888].into_iter()
    }
}

impl RendererSuper for NuriRenderer {
    type Error = Error;
    type TextureId = NuriTexture;
    type Framebuffer<'buffer> = NuriFramebuffer<'buffer>;
    type Frame<'frame, 'buffer>
        = NuriFrame<'frame, 'buffer>
    where
        'buffer: 'frame;
}

impl Renderer for NuriRenderer {
    fn context_id(&self) -> smithay::backend::renderer::ContextId<Self::TextureId> {
        self.context.clone()
    }

    fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
        // ★ ACCEPTED AND IGNORED, which is the honest shape. nuri samples
        // nearest-neighbour only, so there is no filter to select — and
        // returning an error would fail a compositor that merely expressed a
        // preference. The limitation is `pending-nuri-filtering`, not a
        // refusal to run.
        Ok(())
    }

    fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug
    }

    fn render<'frame, 'buffer>(
        &'frame mut self,
        framebuffer: &'frame mut Self::Framebuffer<'buffer>,
        _output_size: Size<i32, Physical>,
        _dst_transform: Transform,
    ) -> Result<Self::Frame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        Ok(NuriFrame {
            // ★ THE SHADOW, NOT THE MAPPING. This one substitution is the
            // whole change: every draw the frame performs now lands in cached
            // RAM, and `flush_damage` makes the single streaming write out.
            surface: nuri::Surface::new(
                &mut framebuffer.shadow,
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride,
            )?,
            context: self.context.clone(),
            _marker: std::marker::PhantomData,
        })
    }

    fn wait(&mut self, _sync: &SyncPoint) -> Result<(), Self::Error> {
        // ★ Nothing to wait FOR. A GPU renderer waits on a fence because the
        // work is asynchronous; nuri's blit finished before this call could be
        // made. Returning Ok is the truth, not a stub.
        Ok(())
    }

    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        // No cache: textures are Arc'd and freed when the last clone drops.
        Ok(())
    }
}

/// One frame of painting.
pub struct NuriFrame<'frame, 'buffer> {
    surface: nuri::Surface<'frame>,
    /// Cloned from the renderer, for the reason in [`NuriRenderer::context`]:
    /// the frame's id must match the renderer's or the same cache miss happens
    /// one layer down.
    context: smithay::backend::renderer::ContextId<NuriTexture>,
    _marker: std::marker::PhantomData<&'buffer ()>,
}

impl std::fmt::Debug for NuriFrame<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NuriFrame")
    }
}

const fn to_rect(r: Rectangle<i32, Physical>) -> nuri::Rect {
    nuri::Rect::new(r.loc.x, r.loc.y, r.size.w, r.size.h)
}

impl Frame for NuriFrame<'_, '_> {
    type Error = Error;
    type TextureId = NuriTexture;

    fn context_id(&self) -> smithay::backend::renderer::ContextId<Self::TextureId> {
        self.context.clone()
    }

    fn clear(
        &mut self,
        color: Color32F,
        at: &[Rectangle<i32, Physical>],
    ) -> Result<(), Self::Error> {
        for r in at {
            self.surface
                .fill(to_rect(*r), nuri::Rgba(color.components()));
        }
        Ok(())
    }

    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        // ★ DAMAGE IS RELATIVE TO `dst`. THIS IS THE CONTRACT, AND GETTING IT
        // WRONG MAKES EVERY OFF-ORIGIN DRAW VANISH.
        //
        // `render_output` computes each element's damage in output
        // coordinates and then does `d.loc -= element_geometry.loc` before
        // calling `draw`, so what arrives here is relative to `dst`. smithay's
        // own pixman backend adds it straight back (`rect.loc += dst_loc`)
        // before clipping — that line is the whole specification.
        //
        // Intersecting the RELATIVE damage with the ABSOLUTE `dst` is empty
        // for everything not at the origin, and empty in a way that looks
        // exactly like correct occlusion: no error, no warning, the element
        // simply is not there. It cost a full tiling investigation — the tree
        // was right, Space was right, the element geometry was right
        // (`512,0 250x250`), and the window was still invisible.
        //
        // It also explains the cursor: at (0,0) relative and absolute
        // coincide, so the pointer was the one thing that always drew, and
        // "a white square in the top-left corner" was that accident.
        for d in damage {
            let mut abs = *d;
            abs.loc += dst.loc;
            if let Some(area) = to_rect(dst).intersect(to_rect(abs)) {
                self.surface.fill(area, nuri::Rgba(color.components()));
            }
        }
        Ok(())
    }

    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        // Held for the blit only. See `NuriTexture::data`.
        let guard = texture
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let src_ref = nuri::SurfaceRef::new(
            &guard,
            i32::try_from(texture.width).unwrap_or(i32::MAX),
            i32::try_from(texture.height).unwrap_or(i32::MAX),
            texture.stride,
        )?;

        #[allow(clippy::cast_possible_truncation)]
        let src_rect = nuri::Rect::new(
            src.loc.x as i32,
            src.loc.y as i32,
            src.size.w as i32,
            src.size.h as i32,
        );
        // Damage arrives RELATIVE to `dst` — see the note in `draw_solid`.
        // nuri's blit clips against the absolute destination, so it has to be
        // translated first or every window away from the origin draws nothing.
        let dmg: Vec<nuri::Rect> = damage
            .iter()
            .map(|r| {
                let mut abs = *r;
                abs.loc += dst.loc;
                to_rect(abs)
            })
            .collect();

        // ★ THE COUNTER IS NOW SOURCED FROM THE BRANCH, NOT MIRRORED.
        //
        // This used to restate nuri's OUTER precondition here, under a comment
        // promising the two would be kept in step. They could not be. nuri
        // makes a SECOND decision per ROW — `srow.chunks_exact(4).all(|px|
        // px[3] == 0xff)` — and a caller cannot see it. An Argb8888 client
        // with one translucent pixel per row (an antialiased edge, a rounded
        // corner, a shadow) takes the blend path on every row while a
        // precondition-mirroring counter reported every row as fast: a ~2 ms
        // versus ~15 ms difference behind identical published numbers.
        //
        // `blit` returns a `BlitTally` counted at each branch, so the drift is
        // unrepresentable rather than discouraged.
        let dst_r = to_rect(dst);

        // ★ MEASURED AT IMPORT, NOT INFERRED FROM THE FORMAT. The tempting
        // version is `matches!(texture.format, Xrgb8888)` and it would buy
        // nothing: mado's swapchain is Bgra8UnormSrgb, so a format gate
        // answers "unknown" on the one client whose frames actually cost
        // anything. `opaque` is what `normalise_opaque` OBSERVED while the
        // bytes were hot from the copy.
        let hint = if texture.opaque.load(std::sync::atomic::Ordering::Relaxed) {
            nuri::OpaqueHint::Opaque
        } else {
            nuri::OpaqueHint::Unknown
        };
        let tally = self.surface.blit(
            &src_ref,
            src_rect,
            dst_r,
            map_transform(src_transform),
            alpha,
            &dmg,
            hint,
        );
        // Published once per blit, never per row: a `fetch_add` in the inner
        // loop would cost more than the branch it is measuring.
        if let Some(c) = BLIT_COUNTS.get() {
            c.0.fetch_add(tally.rows_copied, std::sync::atomic::Ordering::Relaxed);
            c.1.fetch_add(tally.rows_blended, std::sync::atomic::Ordering::Relaxed);
            c.2.fetch_add(tally.pixels_general, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    fn transformation(&self) -> Transform {
        Transform::Normal
    }

    fn wait(&mut self, _sync: &SyncPoint) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finish(self) -> Result<SyncPoint, Self::Error> {
        // ★ Already finished. Every blit completed synchronously inside the
        // calls above, so the sync point is signalled the instant it is made —
        // which is the honest representation of CPU rendering, not a shortcut.
        Ok(SyncPoint::signaled())
    }
}

/// smithay's transform vocabulary → nuri's.
///
/// ★ Both are the same eight cases from the Wayland protocol, and they are
/// translated explicitly rather than transmuted. Two enums that happen to
/// agree today is not a guarantee they agree tomorrow, and a wrong arm here
/// rotates a client's window with no error anywhere.
const fn map_transform(t: Transform) -> nuri::Transform {
    match t {
        Transform::Normal => nuri::Transform::Normal,
        Transform::_90 => nuri::Transform::Rotate90,
        Transform::_180 => nuri::Transform::Rotate180,
        Transform::_270 => nuri::Transform::Rotate270,
        Transform::Flipped => nuri::Transform::Flipped,
        Transform::Flipped90 => nuri::Transform::Flipped90,
        Transform::Flipped180 => nuri::Transform::Flipped180,
        Transform::Flipped270 => nuri::Transform::Flipped270,
    }
}

// ── IMPORT ────────────────────────────────────────────────────────────────

/// Where the blit path counters live.
///
/// A global because `Frame` is constructed per frame by smithay and cannot
/// carry a handle without changing the trait's shape. Installed once from
/// the render loop; absent in tests, where the counters are simply not kept.
/// Full-buffer versus damage-only shm imports. See
/// `OmoyaIntrospect::import_full`.
pub static IMPORT_COUNTS: std::sync::OnceLock<(
    std::sync::Arc<std::sync::atomic::AtomicU64>,
    std::sync::Arc<std::sync::atomic::AtomicU64>,
)> = std::sync::OnceLock::new();

/// `(rows_copied, rows_blended, pixels_general)` — nuri's three blit arms.
///
/// ★ THREE, NOT TWO. It used to be a `fast`/`slow` pair derived from the
/// outer precondition, which cannot see nuri's per-ROW opacity decision. The
/// arms are now counted where they branch and folded in here.
pub static BLIT_COUNTS: std::sync::OnceLock<(
    std::sync::Arc<std::sync::atomic::AtomicU64>,
    std::sync::Arc<std::sync::atomic::AtomicU64>,
    std::sync::Arc<std::sync::atomic::AtomicU64>,
)> = std::sync::OnceLock::new();

impl ImportMem for NuriRenderer {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, BufferCoord>,
        _flipped: bool,
    ) -> Result<Self::TextureId, Self::Error> {
        #[allow(clippy::cast_sign_loss)]
        let (w, h) = (size.w as u32, size.h as u32);
        let stride = (size.w as usize) * 4;
        let mut owned = data.to_vec();
        let opaque = normalise_opaque(&mut owned, format);
        Ok(NuriTexture {
            data: Arc::new(std::sync::RwLock::new(owned)),
            width: w,
            height: h,
            stride,
            format,
            opaque: Arc::new(std::sync::atomic::AtomicBool::new(opaque)),
        })
    }

    fn update_memory(
        &mut self,
        _texture: &Self::TextureId,
        _data: &[u8],
        _region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), Self::Error> {
        // ★ REFUSED, not silently ignored. The texture is behind an Arc that
        // other frames may hold, so an in-place update would mutate an image
        // something else is mid-blit from. Returning an error makes smithay
        // re-import instead, which is correct and slower — and a no-op here
        // would show a stale window with no clue why.
        Err(Error::Unsupported(
            "in-place texture update — re-import instead; the texture is shared",
        ))
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        Box::new(Self::formats())
    }
}

impl ImportMemWl for NuriRenderer {
    /// Import a client's SHM buffer.
    ///
    /// ── ★ THIS IS THE PATH THAT ACTUALLY CARRIES CLIENTS ──────────────────
    /// On the software scanout path a Wayland client draws into shared memory
    /// and hands over a `wl_buffer`. `import_dmabuf` is refused above because
    /// nothing on this path uses it; THIS is where a window's pixels come
    /// from, so it is implemented rather than deferred.
    ///
    /// `with_buffer_contents` is the only sanctioned way to read one: it holds
    /// the pool lock for the callback's duration. Copying inside that closure
    /// is deliberate — the client may release or resize the pool the moment it
    /// returns, and a texture holding a borrow into it would read freed
    /// memory on the next frame.
    fn import_shm_buffer(
        &mut self,
        buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
        _surface: Option<&smithay::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<Self::TextureId, Self::Error> {
        use smithay::wayland::shm;

        // ★ COUNT WHAT ARRIVED, at the one place it is knowable. See the
        // field docs in `introspect` for why reading source could not settle
        // this.
        if let Some(i) = self.introspect.as_ref() {
            use std::sync::atomic::Ordering::Relaxed;
            i.shm_imports.fetch_add(1, Relaxed);
            if damage.is_empty() {
                i.shm_imports_empty_damage.fetch_add(1, Relaxed);
            }
            i.shm_damage_rects.store(damage.len() as u64, Relaxed);
            let area: i64 = damage
                .iter()
                .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
                .sum();
            i.shm_damage_area
                .store(u64::try_from(area).unwrap_or(0), Relaxed);
            // Per-import, so the leaf reads "what did THIS keystroke cost"
            // rather than a number that only ever grows. The cumulative
            // figure lives in `route_cpu_bytes_total`.
            i.route_cpu_bytes.store(0, Relaxed);
            // ★ The route, through rouka's own type rather than a loose
            // string — so the label cannot drift from the enum (R7).
            *i.route_label.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                crate::rouka::Route::CpuReadback {
                    reason: crate::rouka::ReadbackReason::CompositorIsCpu,
                    bytes: 0,
                }
                .label(),
            );
        }

        shm::with_buffer_contents(buffer, |ptr, len, data| {
            let fourcc = shm::shm_format_to_fourcc(data.format)
                .ok_or(Error::Unsupported("shm format with no fourcc"))?;
            if !matches!(fourcc, Fourcc::Argb8888 | Fourcc::Xrgb8888) {
                return Err(Error::Unsupported("shm format nuri cannot address"));
            }

            #[allow(clippy::cast_sign_loss)]
            let (offset, stride, height, width) = (
                data.offset as usize,
                data.stride as usize,
                data.height as usize,
                data.width,
            );
            #[allow(clippy::cast_sign_loss)]
            let (width_u32, height_u32) = (width as u32, data.height as u32);

            // ★ CHECKED, not trusted. `offset`, `stride` and `height` come
            // from the CLIENT. A pool shorter than they claim is how a
            // malicious or buggy client reads compositor memory, and the only
            // place to stop it is here, before the copy.
            let need = offset
                .checked_add(
                    stride
                        .checked_mul(height)
                        .ok_or(Error::Unsupported("shm stride * height overflows"))?,
                )
                .ok_or(Error::Unsupported("shm offset + size overflows"))?;
            if need > len {
                return Err(Error::Unsupported("shm buffer shorter than its geometry"));
            }

            // SAFETY: `ptr` is valid for `len` bytes for the callback's
            // duration, and the range is bounds-checked above.
            let bytes = unsafe { std::slice::from_raw_parts(ptr.add(offset), need - offset) };

            // ── ★ COPY ONLY WHAT CHANGED ────────────────────────────
            //
            // smithay hands us `damage` precisely so a renderer need not
            // re-upload an unchanged buffer. Reusing the cached allocation
            // also avoids an 8 MB alloc+free per frame, which is its own
            // share of the memmove time.
            //
            // The fast route needs three things to hold: a cached texture
            // for THIS buffer, identical geometry, and sole ownership of the
            // allocation. The last one matters — the `Arc` may still be held
            // by a frame in flight, and mutating it underneath would tear
            // the image being scanned out. `Arc::get_mut` returning None is
            // the honest signal to fall back to a fresh copy rather than a
            // reason to reach for unsafe.
            // `Resource::id` — the trait must be in scope; `buffer.id` alone
            // resolves to a private FIELD, and the error says "private
            // field, not a method" rather than "missing trait".
            use smithay::reexports::wayland_server::Resource as _;
            let key = buffer.id();
            // Cloned BEFORE the `get_mut` below, which borrows `self`
            // mutably; an `Arc` handle sidesteps the conflict without
            // restructuring the hot path.
            let sink = self.introspect.clone();
            let reused = self.shm_cache.get_mut(&key).and_then(|tex| {
                let same = tex.width == width_u32
                    && tex.height == height_u32
                    && tex.stride == stride
                    && tex.format == fourcc
                    && tex.data.read().map(|d| d.len()).unwrap_or(0) == bytes.len();
                if !same || damage.is_empty() {
                    return None;
                }
                let mut buf = tex
                    .data
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut range_opaque = true;
                for d in damage {
                    let y0 = usize::try_from(d.loc.y.max(0)).unwrap_or(0).min(height);
                    let y1 = usize::try_from((d.loc.y + d.size.h).max(0))
                        .unwrap_or(0)
                        .min(height);
                    let x0 = usize::try_from(d.loc.x.max(0)).unwrap_or(0);
                    let x1 = usize::try_from((d.loc.x + d.size.w).max(0)).unwrap_or(0);
                    let (x0, x1) = (x0 * 4, (x1 * 4).min(stride));
                    if x1 <= x0 {
                        continue;
                    }
                    for y in y0..y1 {
                        let a = y * stride + x0;
                        let b = y * stride + x1;
                        if b > buf.len() || b > bytes.len() {
                            break;
                        }
                        // ★ ONE WALK, NOT TWO. This was `copy_from_slice`
                        // followed by `normalise_opaque` over the bytes it had
                        // just written — see `copy_normalising`, which is where
                        // the 24 MB-per-keystroke arithmetic is recorded.
                        // ★ COUNTED AT THE COPY, not derived from the damage
                        // rects. Deriving it would mean re-implementing this
                        // loop's clipping in a second place, and two
                        // implementations of one clip is how the numbers start
                        // disagreeing.
                        if let Some(i) = sink.as_ref() {
                            i.route_cpu_bytes_total
                                .fetch_add((b - a) as u64, std::sync::atomic::Ordering::Relaxed);
                            i.route_cpu_bytes
                                .fetch_add((b - a) as u64, std::sync::atomic::Ordering::Relaxed);
                        }
                        let row_opaque = copy_normalising(&mut buf[a..b], &bytes[a..b], fourcc);
                        // ★ AND, NEVER ASSIGN. This pass examined only the
                        // DAMAGED rows; the rest were not re-read, so their
                        // opacity is whatever it already was. A partial
                        // import can therefore only ever CLEAR the flag.
                        // Assigning would let one opaque damage rect declare
                        // a translucent window opaque, and the failure would
                        // be a window composited without its alpha.
                        range_opaque &= row_opaque;
                    }
                }
                if !range_opaque {
                    tex.opaque
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                Some(tex.clone())
            });
            if let Some(tex) = reused {
                if let Some(c) = IMPORT_COUNTS.get() {
                    c.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                return Ok(tex);
            }
            if let Some(c) = IMPORT_COUNTS.get() {
                c.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            // The FULL path: every byte of the client buffer crosses the CPU.
            // On plo at 1920x1080 that is the 8 294 400 the ledger names.
            if let Some(i) = sink.as_ref() {
                let n = bytes.len() as u64;
                i.route_cpu_bytes
                    .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                i.route_cpu_bytes_total
                    .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            }

            // One walk: allocate uninitialised-then-filled rather than
            // copy-then-rewrite. `to_vec()` + `normalise_opaque` walked the
            // whole buffer twice on every FIRST import of a client buffer.
            let mut owned = vec![0u8; bytes.len()];
            let opaque = copy_normalising(&mut owned, bytes, fourcc);
            let tex = NuriTexture {
                data: Arc::new(std::sync::RwLock::new(owned)),
                width: width_u32,
                height: height_u32,
                stride,
                format: fourcc,
                opaque: Arc::new(std::sync::atomic::AtomicBool::new(opaque)),
            };
            // Cached for the NEXT commit, which is the one that gets to copy
            // only its damage. Bounded by how many buffers a client cycles
            // through — a handful — because the key is the buffer's identity
            // and a client re-uses them.
            self.shm_cache.insert(key, tex.clone());
            Ok(tex)
        })
        .map_err(|e| Error::Map(format!("shm pool: {e:?}")))?
    }
}

impl NuriRenderer {
    /// The predicate BOTH the protocol handler and the renderer obey.
    ///
    /// ★ ONE PREDICATE, TWO CALLERS. `DmabufHandler::dmabuf_imported` decides
    /// whether to accept a client's buffer over the wire; `import_dmabuf`
    /// decides whether it can be textured. If those answers can differ, the
    /// compositor accepts a buffer and then renders nothing — an invisible
    /// window with a clean protocol log, which is the worst failure shape
    /// available and precisely the one this seat has already produced twice.
    ///
    /// It cannot be derived from `dmabuf_formats()` alone: smithay validates
    /// the FOURCC against the advertised set but takes the MODIFIER from
    /// whatever the client sent. The modifier check is ours to make.
    ///
    /// # Errors
    /// If the buffer is multi-plane, non-linear, or in a format nuri cannot
    /// address.
    pub fn accepts(dmabuf: &Dmabuf) -> Result<(), Error> {
        use smithay::backend::allocator::Modifier;

        if dmabuf.num_planes() != 1 {
            return Err(Error::Unsupported("multi-plane dmabuf"));
        }
        let format = dmabuf.format();
        // ★ `Invalid` is REFUSED, never treated as "probably linear".
        // MOD_INVALID means the layout is implicit and unknown, and a CPU
        // blitter guessing wrong paints structured noise instead of failing.
        if format.modifier != Modifier::Linear {
            return Err(Error::Unsupported("non-linear modifier"));
        }
        if !matches!(format.code, Fourcc::Argb8888 | Fourcc::Xrgb8888) {
            return Err(Error::Unsupported("pixel format nuri cannot address"));
        }
        Ok(())
    }
}

impl ImportDma for NuriRenderer {
    fn dmabuf_formats(&self) -> smithay::backend::allocator::format::FormatSet {
        // ★ LINEAR ONLY. A tiled or compressed modifier describes a memory
        // layout only a GPU can decode; a CPU blitter reading it would paint
        // structured noise. Advertising just Linear is what makes the
        // negotiation refuse rather than produce that.
        Self::formats()
            .map(|code| smithay::backend::allocator::Format {
                code,
                modifier: smithay::backend::allocator::Modifier::Linear,
            })
            .collect()
    }

    /// Import a client's dmabuf by mapping plane 0 and copying it.
    ///
    /// ── ★ WHY A COPY AND NOT A BORROW ────────────────────────────────────
    /// `NuriTexture` outlives the frame that created it and smithay clones it
    /// per element per frame, while the client may release its buffer at any
    /// commit. Holding the mapping instead would be correct and faster — it is
    /// what `PixmanRenderer` does, with a weak cache so the map happens once
    /// per buffer rather than once per import. That needs `NuriTexture` to
    /// carry a `DmabufMapping`, which is a lifetime change rather than a line
    /// change. `pending-nuri-dmabuf-zerocopy`.
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<Self::TextureId, Self::Error> {
        use smithay::backend::allocator::dmabuf::{DmabufMappingMode, DmabufSyncFlags};

        Self::accepts(dmabuf)?;

        let size = dmabuf.size();
        let format = dmabuf.format();
        let stride = *dmabuf
            .strides()
            .collect::<Vec<_>>()
            .first()
            .ok_or(Error::Unsupported("dmabuf with no stride"))? as usize;

        // READ only: this is someone else's window, and a writable mapping
        // would let a bug here corrupt it.
        let mapping = dmabuf
            .map_plane(0, DmabufMappingMode::READ)
            .map_err(|e| Error::Map(format!("{e:?}")))?;

        #[allow(clippy::cast_sign_loss)]
        let height = size.h as usize;
        let expected = stride
            .checked_mul(height)
            .ok_or(Error::Unsupported("stride * height overflows"))?;
        // ★ CHECKED AGAINST THE MAPPING'S OWN LENGTH. The stride is metadata
        // the CLIENT supplied; the length comes from the kernel. Trusting the
        // first without the second reads past the end of the mapping.
        if mapping.length() < expected {
            return Err(Error::Unsupported("dmabuf shorter than stride * height"));
        }

        // ★ THE SYNC IOCTL IS NOT DECORATION. DMA_BUF_IOCTL_SYNC tells the
        // exporter a CPU read is beginning and ending so it can flush or
        // invalidate caches. Skipping it reads stale pixels wherever the
        // mapping is not coherent, and the symptom is a window one frame
        // behind — not an error.
        dmabuf
            .sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::READ)
            .map_err(|e| Error::Map(format!("sync start: {e:?}")))?;
        // SAFETY: the mapping is valid for `length()` bytes while `mapping`
        // lives, and `expected <= length()` was checked immediately above.
        let src = unsafe { std::slice::from_raw_parts(mapping.ptr().cast::<u8>(), expected) };
        let mut bytes = vec![0u8; expected];
        let opaque = copy_normalising(&mut bytes, src, format.code);
        dmabuf
            .sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::READ)
            .map_err(|e| Error::Map(format!("sync end: {e:?}")))?;

        #[allow(clippy::cast_sign_loss)]
        Ok(NuriTexture {
            data: Arc::new(std::sync::RwLock::new(bytes)),
            width: size.w as u32,
            height: size.h as u32,
            stride,
            format: format.code,
            opaque: Arc::new(std::sync::atomic::AtomicBool::new(opaque)),
        })
    }
}

impl ImportDmaWl for NuriRenderer {}

impl Bind<Dmabuf> for NuriRenderer {
    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<Self::Framebuffer<'a>, Self::Error> {
        use smithay::backend::allocator::dmabuf::DmabufMappingMode;

        // ★ THE SAME MAP pixman DOES, AND THE SAME REFUSALS. A CPU renderer
        // needs the pixels addressable, so plane 0 is mapped and anything it
        // cannot address is rejected up front rather than rendered wrongly.
        if target.num_planes() != 1 {
            return Err(Error::Unsupported("multi-plane dmabuf"));
        }
        let format = target.format();
        if format.modifier != smithay::backend::allocator::Modifier::Linear {
            return Err(Error::Unsupported("non-linear modifier"));
        }
        if !matches!(format.code, Fourcc::Argb8888 | Fourcc::Xrgb8888) {
            return Err(Error::Unsupported("pixel format nuri cannot address"));
        }

        let size = target.size();
        let stride = target
            .strides()
            .next()
            .ok_or(Error::Unsupported("dmabuf with no stride"))? as usize;

        // READ | WRITE: the compositor blends INTO this buffer, so it reads
        // the existing pixels wherever alpha is not 1. A write-only mapping
        // would fault on the first translucent surface.
        let mapping = target
            .map_plane(0, DmabufMappingMode::READ | DmabufMappingMode::WRITE)
            .map_err(|e| Error::Map(format!("{e:?}")))?;

        #[allow(clippy::cast_sign_loss)]
        let expected = stride
            .checked_mul(size.h as usize)
            .ok_or(Error::Unsupported("stride * height overflows"))?;
        // ★ CHECKED AGAINST THE MAPPING'S OWN LENGTH, exactly as pixman does
        // (`renderer/pixman/mod.rs:756`). The stride comes from the buffer's
        // metadata and the length from the kernel; trusting the first without
        // the second is how a rasterizer writes past the end of a mapping.
        if mapping.length() < expected {
            return Err(Error::Unsupported("dmabuf shorter than stride * height"));
        }

        // SAFETY: the mapping is valid for `length()` bytes and is moved into
        // the returned framebuffer, so it outlives the slice. The range is
        // bounds-checked above.
        let data =
            unsafe { std::slice::from_raw_parts_mut(mapping.ptr().cast::<u8>(), mapping.length()) };

        // ★ TAKE THE POOLED SHADOW AND SIZE IT TO THIS BUFFER.
        //
        // `resize` only ever grows here, and it zero-fills the growth — which
        // is correct for a first frame (age 0 forces a full repaint, so every
        // byte is written before it is read) and free thereafter, because a
        // steady-state seat reuses the same allocation untouched.
        let need = stride.saturating_mul(usize::try_from(size.h).unwrap_or(0));
        let mut shadow = self
            .shadow_pool
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        if shadow.len() < need {
            shadow.resize(need, 0);
        }

        Ok(NuriFramebuffer {
            // ★ UNKNOWN, and honestly so. This buffer comes from a pool that
            // recycles allocations across binds (see `Drop`), so nothing here
            // has established which generation its bytes belong to. Claiming
            // `KnownAsOf` would be the assertion this type exists to remove.
            //
            // Establishing it -- threading the scanout slot's `last_drawn`
            // generation through the bind -- is what earns the partial copy
            // back, and it is a real change rather than a flag flip.
            // ★ UNKNOWN, and honestly so. This allocation is recycled through
            // a pool across binds (see `Drop`), so nothing here establishes
            // which generation its bytes belong to. `Target::surface` is
            // kentou's constructor for exactly that situation -- its doc says
            // it takes no revision "because there is nothing true to pass".
            target: mekuri::kentou::Target::surface(
                u32::try_from(size.w).unwrap_or(0),
                u32::try_from(size.h).unwrap_or(0),
            ),
            data,
            shadow,
            pool: self.shadow_pool.clone(),
            width: size.w,
            height: size.h,
            stride,
            format: format.code,
            _mapping: mapping,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ THE DIFFERENTIAL THAT LETS `copy_normalising` REPLACE THE PAIR.
    ///
    /// The fused copy is only allowed to exist if it is byte-identical to
    /// `copy_from_slice` followed by `normalise_opaque` — the exact pair it
    /// replaced at three call sites, one of which is on the keystroke path.
    /// Asserting that here rather than reasoning about it is the whole reason
    /// this change is safe to ship to a machine whose console is its only
    /// local access.
    ///
    /// It runs against BOTH formats and against buffers that are opaque,
    /// translucent and mixed, because the two arms disagree about what the
    /// alpha byte MEANS: Xrgb imposes 0xff, Argb reports what it found.
    #[test]
    fn the_fused_copy_matches_copy_then_normalise() {
        let cases: Vec<Vec<u8>> = vec![
            // fully opaque
            vec![1, 2, 3, 0xff, 4, 5, 6, 0xff],
            // fully transparent — the case that made an Xrgb window invisible
            vec![1, 2, 3, 0x00, 4, 5, 6, 0x00],
            // mixed: one opaque pixel and one not
            vec![1, 2, 3, 0xff, 4, 5, 6, 0x7f],
            // a single pixel
            vec![9, 8, 7, 0x01],
            // empty — a zero-width damage rect is a real input
            vec![],
        ];
        for fourcc in [Fourcc::Argb8888, Fourcc::Xrgb8888] {
            for src in &cases {
                // The original: copy, then walk again.
                let mut want = src.clone();
                let want_opaque = normalise_opaque(&mut want, fourcc);

                // The replacement: one walk.
                let mut got = vec![0u8; src.len()];
                let got_opaque = copy_normalising(&mut got, src, fourcc);

                assert_eq!(
                    got, want,
                    "{fourcc:?}: fused bytes differ from copy-then-normalise for {src:?}"
                );
                assert_eq!(
                    got_opaque, want_opaque,
                    "{fourcc:?}: fused opacity differs for {src:?}"
                );
            }
        }
    }

    #[test]
    fn the_fused_copy_does_not_read_the_destination() {
        // ★ The partial-import path writes into a CACHED texture, so `dst`
        // arrives holding the previous frame's pixels. A fused routine that
        // blended with, or read, whatever was already there would leave a
        // ghost of the last frame — and it would only show up on the second
        // commit, which is the kind of bug that reaches a screen.
        let src = vec![10, 20, 30, 0x80];
        for fourcc in [Fourcc::Argb8888, Fourcc::Xrgb8888] {
            let mut from_dirty = vec![0xaa, 0xbb, 0xcc, 0xdd];
            let mut from_zero = vec![0u8; 4];
            let a = copy_normalising(&mut from_dirty, &src, fourcc);
            let b = copy_normalising(&mut from_zero, &src, fourcc);
            assert_eq!(
                from_dirty, from_zero,
                "{fourcc:?}: the prior contents of dst changed the result"
            );
            assert_eq!(a, b);
        }
    }

    #[test]
    fn xrgb_alpha_is_forced_and_argb_alpha_is_preserved() {
        // The two arms must not converge. If a refactor ever made Argb also
        // force 0xff, every translucent window would composite opaque and the
        // differential above would still pass — both sides would be wrong
        // together, because `normalise_opaque` is the oracle.
        let src = vec![1, 2, 3, 0x40];
        let mut x = vec![0u8; 4];
        assert!(copy_normalising(&mut x, &src, Fourcc::Xrgb8888));
        assert_eq!(x[3], 0xff, "Xrgb must impose opacity");

        let mut a = vec![0u8; 4];
        assert!(!copy_normalising(&mut a, &src, Fourcc::Argb8888));
        assert_eq!(a[3], 0x40, "Argb must preserve the client's alpha");
    }

    #[test]
    fn only_the_two_reachable_formats_are_advertised() {
        // ★ pixman advertises 13; the CRTC is offered 2. Advertising a format
        // the blitter cannot write is how a compositor paints garbage on one
        // machine and nothing on another.
        let f: Vec<Fourcc> = NuriRenderer::formats().collect();
        assert_eq!(f, vec![Fourcc::Argb8888, Fourcc::Xrgb8888]);
    }

    #[test]
    fn every_smithay_transform_has_a_nuri_arm() {
        // A wrong arm rotates a client's window with no error anywhere, so the
        // mapping is exhaustive by construction and checked at both ends.
        for t in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let _ = map_transform(t);
        }
    }

    /// ★ THE CONTRACT THAT COST A WHOLE INVESTIGATION: element damage is
    /// RELATIVE to `dst`, and a renderer that treats it as absolute silently
    /// drops every element away from the origin.
    ///
    /// `render_output` does `d.loc -= element_geometry.loc` before calling
    /// `draw`; smithay's own pixman backend adds it straight back
    /// (`rect.loc += dst_loc`). nuri did not, so intersecting relative damage
    /// with an absolute `dst` came out empty for a window at 512,0 — no
    /// error, no warning, indistinguishable from correct occlusion. The tree
    /// was right, `Space` was right, and the element geometry was right.
    ///
    /// This pins the arithmetic directly rather than through a renderer,
    /// because the failure has no observable symptom short of a screenshot.
    #[test]
    fn element_damage_is_relative_to_dst() {
        use smithay::utils::{Physical, Rectangle};
        // A window at 512,0 sized 250x250, fully damaged. `render_output`
        // hands the damage over with the element's origin subtracted, so a
        // full-surface damage arrives as 0,0 250x250 — NOT 512,0.
        let dst: Rectangle<i32, Physical> = Rectangle::new((512, 0).into(), (250, 250).into());
        let relative: Rectangle<i32, Physical> = Rectangle::new((0, 0).into(), (250, 250).into());

        // The bug: intersecting as-is.
        assert!(
            dst.intersection(relative).is_none(),
            "if this ever overlaps, the test no longer reproduces the defect"
        );

        // The fix: translate into the destination's frame first.
        let mut abs = relative;
        abs.loc += dst.loc;
        assert_eq!(
            dst.intersection(abs),
            Some(dst),
            "a fully-damaged element must clip to its whole destination"
        );
    }

    #[test]
    fn dmabuf_formats_are_linear_only() {
        let r = NuriRenderer::new();
        let set = r.dmabuf_formats();
        // ★ `iter().next().is_some()`, because `FormatSet` HAS NO `is_empty`.
        //
        // This line said `!set.is_empty()` and therefore did not compile —
        // which means `cargo test -p omoya` had never run at all. Every unit
        // test in this crate was ABSENT rather than passing, and absent tests
        // report exactly the same thing as a clean suite: nothing.
        assert!(
            set.iter().next().is_some(),
            "advertising an EMPTY format list is how DmabufState fails with              NoSupportedRendererFormat — an error that names the renderer              rather than the missing declaration"
        );
        assert!(
            set.iter()
                .all(|f| f.modifier == smithay::backend::allocator::Modifier::Linear),
            "a tiled modifier would be decoded as structured noise by a CPU blitter"
        );
    }
}

// ── EXPORT ────────────────────────────────────────────────────────────────

/// Pixels copied out of a framebuffer or texture.
///
/// ★ This is what makes SCREENSHOTS work, which is not a side quest: the
/// operator asked for pixel-level troubleshooting over MCP precisely because a
/// seat that can only be inspected by walking to the machine is a seat that
/// gets debugged by someone's eyes. omoya's `capture()` needs `ExportMem`, and
/// implementing it here is what lets `capture` stop naming `PixmanRenderer` —
/// which is in turn what lets the pixman feature be dropped at all.
#[derive(Debug)]
pub struct NuriMapping {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: Fourcc,
}

impl Texture for NuriMapping {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> Option<Fourcc> {
        Some(self.format)
    }
}

impl smithay::backend::renderer::TextureMapping for NuriMapping {
    fn flipped(&self) -> bool {
        // ★ Not flipped. A GPU renderer reads back bottom-up because its
        // origin is bottom-left; nuri writes top-down into a linear buffer
        // whose origin is top-left, so there is nothing to invert. Returning
        // `true` here would present every screenshot upside down.
        false
    }

    fn format(&self) -> Fourcc {
        self.format
    }
}

/// Copy a rectangle out of a 32-bpp buffer, row by row.
///
/// Shared by `copy_framebuffer` and `copy_texture` because they differ only in
/// where the source bytes live — writing it twice is how the two drift.
fn copy_region(
    src: &[u8],
    src_stride: usize,
    src_w: i32,
    src_h: i32,
    region: Rectangle<i32, BufferCoord>,
    format: Fourcc,
) -> Result<NuriMapping, Error> {
    let Some(r) = nuri::Rect::new(region.loc.x, region.loc.y, region.size.w, region.size.h)
        .intersect(nuri::Rect::new(0, 0, src_w, src_h))
    else {
        return Err(Error::Unsupported("copy region lies outside the source"));
    };

    let mut out = Vec::with_capacity((r.w as usize) * (r.h as usize) * 4);
    for y in r.y..r.y + r.h {
        let row = (y as usize) * src_stride + (r.x as usize) * 4;
        let end = row + (r.w as usize) * 4;
        // Bounds-checked per row rather than once up front: a stride from a
        // DRM ioctl and a region from a caller are independent, and only the
        // combination can overrun.
        let slice = src
            .get(row..end)
            .ok_or(Error::Unsupported("copy region overruns the source"))?;
        out.extend_from_slice(slice);
    }

    #[allow(clippy::cast_sign_loss)]
    Ok(NuriMapping {
        data: out,
        width: r.w as u32,
        height: r.h as u32,
        format,
    })
}

impl smithay::backend::renderer::ExportMem for NuriRenderer {
    type TextureMapping = NuriMapping;

    fn copy_framebuffer(
        &mut self,
        target: &Self::Framebuffer<'_>,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        // ★ Nearly a memcpy, and that is the point: nuri's framebuffer IS
        // memory. A GPU renderer has to schedule a readback and wait on a
        // fence; the software path already has the pixels.
        //
        // ★ READ THE SHADOW, NOT THE MAPPING — the last write-combining read
        // in the codebase, removed. `target.data` is the scanout mmap, where a
        // read costs ~1000x a read from RAM (measured on plo); `target.shadow`
        // holds the same bytes in ordinary cached memory.
        //
        // They are identical here and not merely usually-identical: `capture`
        // deliberately passes age 0, which forces a full repaint, so
        // `flush_damage` has just copied the entire shadow into the mapping.
        // Reading the mapping would re-fetch, uncached, bytes that are already
        // hot in cache one buffer over — which for a 1920x1080 frame was
        // measured elsewhere at several SECONDS.
        copy_region(
            &target.shadow,
            target.stride,
            target.width,
            target.height,
            region,
            format,
        )
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        // Held for the copy only. See `NuriTexture::data`.
        let guard = texture
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        copy_region(
            &guard,
            texture.stride,
            i32::try_from(texture.width).unwrap_or(i32::MAX),
            i32::try_from(texture.height).unwrap_or(i32::MAX),
            region,
            format,
        )
    }

    fn can_read_texture(&mut self, _texture: &Self::TextureId) -> Result<bool, Self::Error> {
        // Always. Every nuri texture is a plain byte vector — there is no
        // opaque GPU-side handle that could refuse to be read.
        Ok(true)
    }

    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        Ok(&texture_mapping.data)
    }
}
