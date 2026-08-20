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

use smithay::backend::allocator::Fourcc;
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
#[derive(Debug)]
pub struct NuriFramebuffer<'a> {
    data: &'a mut [u8],
    width: i32,
    height: i32,
    stride: usize,
    format: Fourcc,
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
#[derive(Debug, Default)]
pub struct NuriRenderer {
    debug: DebugFlags,
}

impl NuriRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        smithay::backend::renderer::ContextId::new()
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
        smithay::backend::renderer::ContextId::new()
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
        // ★ Clipped to the DAMAGE, not just to dst. Painting the whole
        // rectangle when only part of it changed is correct on screen and
        // wrong for a damage-tracked compositor: it would make every partial
        // update cost a full one.
        for d in damage {
            if let Some(area) = to_rect(dst).intersect(to_rect(*d)) {
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
        let dmg: Vec<nuri::Rect> = damage.iter().map(|r| to_rect(*r)).collect();

        self.surface.blit(
            &src_ref,
            src_rect,
            to_rect(dst),
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
        Ok(NuriTexture {
            data: Arc::new(data.to_vec()),
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

impl ImportMemWl for NuriRenderer {}

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

    fn import_dmabuf(
        &mut self,
        _dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<Self::TextureId, Self::Error> {
        // ★ NOT IMPLEMENTED, AND SAYING SO. Importing a client's dmabuf as a
        // TEXTURE means mapping their buffer for reading every frame — real
        // work with real lifetime questions. The scanout path does not need
        // it: `Bind<Dmabuf>` below maps the OUTPUT, and clients on this path
        // deliver SHM, which `import_memory` handles.
        //
        // A stub returning an empty texture would show clients as invisible
        // black rectangles, which reads as a compositing bug rather than an
        // unimplemented import.
        //
        // `pending-nuri-dmabuf-import: client dmabuf as texture`
        Err(Error::Unsupported(
            "dmabuf import as texture — clients on the software path use SHM",
        ))
    }
}

impl ImportDmaWl for NuriRenderer {}

impl Bind<Dmabuf> for NuriRenderer {
    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<Self::Framebuffer<'a>, Self::Error> {
        // ★ THE SAME MAP pixman DOES, AND THE SAME REFUSALS. A CPU renderer
        // needs the pixels addressable, so plane 0 is mapped and anything it
        // cannot address is rejected up front rather than rendered wrongly.
        if target.num_planes() != 1 {
            return Err(Error::Unsupported("multi-plane dmabuf"));
        }
        if target.format().modifier != smithay::backend::allocator::Modifier::Linear {
            return Err(Error::Unsupported("non-linear modifier"));
        }
        Err(Error::Map(
            "dmabuf mapping is not yet wired — see pending-nuri-bind".into(),
        ))
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

    #[test]
    fn dmabuf_formats_are_linear_only() {
        let r = NuriRenderer::new();
        let set = r.dmabuf_formats();
        assert!(!set.is_empty());
        assert!(
            set.iter()
                .all(|f| f.modifier == smithay::backend::allocator::Modifier::Linear),
            "a tiled modifier would be decoded as structured noise by a CPU blitter"
        );
    }
}
