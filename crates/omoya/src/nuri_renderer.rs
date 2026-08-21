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

use smithay::backend::allocator::{Buffer as _, Fourcc};
use smithay::backend::allocator::dmabuf::Dmabuf;
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
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    stride: usize,
    format: Fourcc,
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
fn normalise_opaque(bytes: &mut [u8], format: Fourcc) {
    if matches!(format, Fourcc::Xrgb8888) {
        for px in bytes.chunks_exact_mut(4) {
            px[3] = 0xff;
        }
    }
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
/// at an unmapped page the moment the mapping dropped, and the write would go
/// to whatever the kernel put there next.
pub struct NuriFramebuffer<'a> {
    data: &'a mut [u8],
    width: i32,
    height: i32,
    stride: usize,
    format: Fourcc,
    /// Dropped last, after `data` is gone. Never read.
    _mapping: smithay::backend::allocator::dmabuf::DmabufMapping,
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
    #[must_use]
    pub fn new() -> Self {
        // `DebugFlags` has no Default impl — spelled out rather than derived.
        Self {
            debug: DebugFlags::empty(),
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
            surface: nuri::Surface::new(
                framebuffer.data,
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

    fn clear(&mut self, color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        for r in at {
            self.surface.fill(to_rect(*r), nuri::Rgba(color.components()));
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
        let src_ref = nuri::SurfaceRef::new(
            &texture.data,
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

        // Count which path nuri will take, so a profile that only says
        // "memmove" can be told apart from a fast path that is never
        // entered. Mirrors nuri's own precondition exactly — if these drift,
        // the counter lies, so they are written next to each other.
        let dst_r = to_rect(dst);
        let fast = matches!(map_transform(src_transform), nuri::Transform::Normal)
            && src_rect.w == dst_r.w
            && src_rect.h == dst_r.h
            && alpha >= 1.0;
        let rows = u64::try_from(dst_r.h.max(0)).unwrap_or(0);
        if let Some(c) = BLIT_COUNTS.get() {
            if fast {
                c.0.fetch_add(rows, std::sync::atomic::Ordering::Relaxed);
            } else {
                c.1.fetch_add(rows, std::sync::atomic::Ordering::Relaxed);
            }
        }

        self.surface.blit(
            &src_ref,
            src_rect,
            dst_r,
            map_transform(src_transform),
            alpha,
            &dmg,
        );
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
pub static BLIT_COUNTS: std::sync::OnceLock<(
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
        normalise_opaque(&mut owned, format);
        Ok(NuriTexture {
            data: Arc::new(owned),
            width: w,
            height: h,
            stride,
            format,
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
                .checked_add(stride.checked_mul(height).ok_or(Error::Unsupported(
                    "shm stride * height overflows",
                ))?)
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
            let key = buffer.id();
            let reused = self.shm_cache.get_mut(&key).and_then(|tex| {
                let same = tex.width == width_u32
                    && tex.height == height_u32
                    && tex.stride == stride
                    && tex.format == fourcc
                    && tex.data.len() == bytes.len();
                if !same || damage.is_empty() {
                    return None;
                }
                let buf = Arc::get_mut(&mut tex.data)?;
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
                        buf[a..b].copy_from_slice(&bytes[a..b]);
                        normalise_opaque(&mut buf[a..b], fourcc);
                    }
                }
                Some(tex.clone())
            });
            if let Some(tex) = reused {
                return Ok(tex);
            }

            let mut owned = bytes.to_vec();
            normalise_opaque(&mut owned, fourcc);
            let tex = NuriTexture {
                data: Arc::new(owned),
                width: width_u32,
                height: height_u32,
                stride,
                format: fourcc,
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
        let mut bytes =
            unsafe { std::slice::from_raw_parts(mapping.ptr().cast::<u8>(), expected) }.to_vec();
        dmabuf
            .sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::READ)
            .map_err(|e| Error::Map(format!("sync end: {e:?}")))?;

        normalise_opaque(&mut bytes, format.code);

        #[allow(clippy::cast_sign_loss)]
        Ok(NuriTexture {
            data: Arc::new(bytes),
            width: size.w as u32,
            height: size.h as u32,
            stride,
            format: format.code,
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
        let data = unsafe {
            std::slice::from_raw_parts_mut(mapping.ptr().cast::<u8>(), mapping.length())
        };

        Ok(NuriFramebuffer {
            data,
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
        let relative: Rectangle<i32, Physical> =
            Rectangle::new((0, 0).into(), (250, 250).into());

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
        copy_region(
            target.data,
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
        copy_region(
            &texture.data,
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
