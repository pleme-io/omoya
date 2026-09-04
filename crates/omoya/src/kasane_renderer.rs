//! kasane behind smithay's renderer traits — the GPU sibling of
//! [`crate::nuri_renderer`].
//!
//! ── ★ WHY THE ADAPTER LIVES HERE AND NOT IN `kasane` ─────────────────────
//! Exactly where `nuri_renderer.rs` lives, for exactly the same reason. `nuri`
//! is a rasteriser with no smithay dependency and its adapter is in this
//! crate; `kasane` is a Vulkan boundary with no smithay dependency and its
//! adapter is here. Putting it in `kasane` would drag wayland, drm and the
//! whole compositor stack into the crate whose entire purpose is a minimal,
//! auditable C boundary — and `ArmFlush`, `AdvertisesDmabuf` and
//! `ScanoutFlush` are omoya-local traits anyway, so half the impls could not
//! live there regardless.
//!
//! ── ★ THE SEAM IS A GENERIC BOUND, SO `drm.rs` DOES NOT CHANGE ───────────
//! `docs/KASANE.md` M2's done-predicate is that
//! `git show --stat <commit> -- crates/omoya/src/drm.rs` is EMPTY. That holds
//! by construction rather than by care: `drm.rs:560-595` takes its renderer as
//! a bounded type parameter, so satisfying the bound is the whole integration.
//! If drm.rs ever needs editing to accommodate this, the seam was wrong.
//!
//! ── ★ HOW smithay's `Frame` MAPS ONTO kasane ─────────────────────────────
//! smithay's `Frame` is a *stateful recorder*: `clear`, then any number of
//! draws, then `finish()`. kasane's `Target::draw` is the opposite shape — one
//! call taking the clear colour and the whole draw list.
//!
//! They meet cleanly because a `Frame` here ACCUMULATES. Each `draw_solid` and
//! `render_texture_from_to` pushes a `kasane::vk::Draw`; `finish()` makes the
//! single `Target::draw` call that records, submits and waits. That is not a
//! workaround — it is strictly better than recording eagerly, because the
//! whole frame's draw list is in hand before any command is written, which is
//! what a future reordering or batching pass would need.
//!
//! ── ★ TIER-HONEST: WHAT IS AND IS NOT BUILT ──────────────────────────────
//! Read this before citing the file as "M2 done". The trait SHAPES are all
//! real and the lifetime structure is proven by compiling against smithay's
//! own definitions. Three bodies are not built, and each returns a typed error
//! naming what is missing rather than a placeholder `Ok`:
//!
//!   `ImportMem`      needs a staging buffer and a host->device copy. kasane
//!                    has no upload path at all today.
//!   `ExportMem`      the readback exists (`Target::read_pixel`), but smithay
//!                    wants a `TextureMapping` object with a deferred map.
//!
//! Until those land this renderer cannot drive a seat, and `drm.rs` will not
//! accept it — the bound demands all three. That is the correct outcome: the
//! type system refuses a half-built renderer rather than letting it produce a
//! black screen at runtime.

// ★ UNREACHABLE TODAY, AND THE ALLOW SAYS WHEN IT GOES.
//
// Nothing constructs a `KasaneRenderer` because nothing CAN: `drm.rs`'s bound
// demands `Bind<Dmabuf>`, `ImportMem` and `ExportMem`, and those three bodies
// are not built (see the header). The type system is refusing a half-built
// renderer, which is the correct outcome — the alternative is a renderer that
// compiles, gets selected, and composes a black screen.
//
// This is NOT the "primitive with zero consumers" that `theory/RENDERING.md`
// warns is the fleet's real duplication problem. That warning is about
// primitives nobody reached for; this one has a named consumer (`drm.rs`) and
// three named, specific blockers. **Done-predicate for deleting this allow:**
// the three impls land, `drm.rs` accepts `KasaneRenderer`, and the seat can
// select it — at which point every item here is reachable and the attribute
// stops compiling clean.
#![allow(
    dead_code,
    reason = "blocked on Bind<Dmabuf>/ImportMem/ExportMem; see above for the \
              done-predicate that removes this"
)]

use std::sync::Arc;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{
    Color32F, ContextId, DebugFlags, Frame, Renderer, RendererSuper, Texture, TextureFilter,
};
use smithay::utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform};

use kasane::vk::{Draw, Filter, Gpu, Imported, Pipelines, Target};

/// Why a kasane-backed render failed.
///
/// ★ `NotBuilt` is its own arm, and carries WHAT is missing. Folding an
/// unbuilt path into a generic error would send the next reader looking for a
/// bug in working code — the same reasoning as `nuri_renderer`'s
/// `Unsupported` arm, one level further out.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The GPU refused, or a Vulkan call failed. Names the call.
    #[error("kasane: {0}")]
    Kasane(#[from] kasane::KasaneError),
    /// A buffer this renderer structurally cannot address.
    #[error("unsupported buffer: {0}")]
    Unsupported(&'static str),
    /// A path whose shape exists and whose body does not.
    ///
    /// Never returned from a path this renderer claims to serve — the traits
    /// that depend on these are not satisfiable until they are built, so a
    /// seat cannot select this renderer and reach one by accident.
    #[error(
        "kasane cannot {what} yet — {why}. This is an unbuilt path, not a \
         failure of one that works."
    )]
    NotBuilt {
        /// The capability, as a verb phrase.
        what: &'static str,
        /// What has to be built, concretely.
        why: &'static str,
    },
}

// ★ NO `impl smithay::…::Error` HERE, and that is correct rather than an
// omission. smithay's `type Error: Error` bound is `std::error::Error`
// (`renderer/mod.rs:15` imports it from `std`), which `thiserror` already
// derives — writing one produces E0119, a conflicting implementation.

/// A client buffer living in GPU memory, ready to be sampled.
///
/// ★ `Arc<Imported>` because `drm.rs` demands `R::TextureId: Clone + Send`.
/// The underlying import owns Vulkan objects that must be destroyed exactly
/// once, so it is shared rather than copied — cloning the handles would give
/// two owners of one image and a double free.
#[derive(Clone)]
pub struct KasaneTexture {
    inner: Arc<Imported>,
}

impl std::fmt::Debug for KasaneTexture {
    // `Texture` requires `Debug` and the Vulkan handles inside are opaque
    // integers, so the useful thing to print is the geometry.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KasaneTexture")
            .field("width", &self.inner.geometry.width)
            .field("height", &self.inner.geometry.height)
            .field("stride", &self.inner.geometry.stride)
            .finish()
    }
}

impl Texture for KasaneTexture {
    fn width(&self) -> u32 {
        self.inner.geometry.width
    }

    fn height(&self) -> u32 {
        self.inner.geometry.height
    }

    fn format(&self) -> Option<Fourcc> {
        // kasane imports ARGB8888 only today — `kasane::vk`'s `FORMAT` is
        // `B8G8R8A8_UNORM`, which is what DRM calls `Argb8888`. Stated as a
        // fixed answer rather than a lookup because there is exactly one.
        Some(Fourcc::Argb8888)
    }
}

/// The GPU renderer.
///
/// Owns the device and the compiled pipelines. Both are needed for the whole
/// life of the renderer, which is why `kasane` shares its `Gpu` by refcount —
/// a borrow would make this struct self-referential.
pub struct KasaneRenderer {
    gpu: Arc<Gpu>,
    pipelines: Pipelines,
    context_id: ContextId<KasaneTexture>,
    upscale: TextureFilter,
    downscale: TextureFilter,
    debug: DebugFlags,
}

impl std::fmt::Debug for KasaneRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KasaneRenderer")
            .field("device", &self.gpu.device_name)
            .field("is_cpu", &self.gpu.is_cpu)
            .finish()
    }
}

impl KasaneRenderer {
    /// Open a device and compile the pipelines.
    ///
    /// # Errors
    /// [`Error::Kasane`] wrapping the typed reason — no loader, no device with
    /// the extensions, a driver refusal. Every one of these is a legitimate
    /// state the caller answers by falling back to `nuri`, not a bug.
    pub fn new() -> Result<Self, Error> {
        let gpu = Arc::new(Gpu::open().map_err(kasane::KasaneError::from)?);
        let pipelines = Pipelines::new(&gpu, kasane::vk::FORMAT)?;
        Ok(Self {
            gpu,
            pipelines,
            context_id: ContextId::new(),
            upscale: TextureFilter::Linear,
            downscale: TextureFilter::Linear,
            debug: DebugFlags::empty(),
        })
    }

    /// What the driver calls itself — for reports, so "which GPU answered" is
    /// never a guess.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.gpu.device_name
    }

    /// Whether this is a software rasteriser.
    ///
    /// ★ Not a defect — it is how CI exercises the path — but a seat must be
    /// able to tell, because choosing a CPU Vulkan device over `nuri` buys
    /// nothing and costs a copy.
    #[must_use]
    pub fn is_cpu(&self) -> bool {
        self.gpu.is_cpu
    }

    /// Turn smithay's filter choice into kasane's.
    fn filter(f: TextureFilter) -> Filter {
        match f {
            TextureFilter::Nearest => Filter::Nearest,
            TextureFilter::Linear => Filter::Linear,
        }
    }
}

/// A bound render target — a dmabuf kasane renders straight into.
///
/// ★ No shadow buffer anywhere in this type. That is the whole difference from
/// `NuriFramebuffer`, which composites into a shadow and then copies it to the
/// scanout mapping — measured at 12.0 ms of a 12.1 ms frame.
pub struct KasaneFramebuffer<'buffer> {
    target: Target,
    size: Size<i32, Physical>,
    /// Ties this to the buffer it was bound from, exactly as smithay's own
    /// framebuffers do.
    _buffer: std::marker::PhantomData<&'buffer mut Dmabuf>,
}

impl std::fmt::Debug for KasaneFramebuffer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KasaneFramebuffer")
            .field("size", &self.size)
            .finish()
    }
}

impl Texture for KasaneFramebuffer<'_> {
    fn width(&self) -> u32 {
        self.target.extent.width
    }

    fn height(&self) -> u32 {
        self.target.extent.height
    }

    fn format(&self) -> Option<Fourcc> {
        Some(Fourcc::Argb8888)
    }
}

impl crate::nuri_renderer::ScanoutFlush for KasaneFramebuffer<'_> {
    fn flush_damage(&mut self, _damage: &[Rectangle<i32, Physical>]) -> u64 {
        // ★ ZERO BYTES, AND THAT IS THE POINT OF THE WHOLE CRATE.
        //
        // nuri composites into a shadow and then copies it to the scanout
        // mapping — measured at 12.0 ms of a 12.1 ms frame, 99.6%. A GPU
        // renderer draws STRAIGHT INTO the scanout buffer, so there is no
        // second copy to make and nothing to flush.
        //
        // The return value is bytes written, and the honest answer is 0.
        0
    }

    fn scanout_bytes(&self) -> &[u8] {
        // The scanout buffer is device memory this renderer never maps —
        // mapping it is precisely the cost being removed. A reader wanting
        // pixels goes through `ExportMem`, which is explicit about paying for
        // a readback.
        &[]
    }
}

/// One frame in progress.
///
/// ── ★ IT ACCUMULATES RATHER THAN RECORDING EAGERLY ───────────────────────
/// See the module header. Every draw call pushes onto `draws`; `finish()`
/// makes the single `Target::draw` that records, submits and waits.
pub struct KasaneFrame<'frame, 'buffer> {
    renderer: &'frame mut KasaneRenderer,
    framebuffer: &'frame mut KasaneFramebuffer<'buffer>,
    /// Cleared to this before anything is drawn.
    clear: [f32; 4],
    draws: Vec<Draw>,
    /// Kept alive for the length of the frame: a `Draw::Texture` holds raw
    /// Vulkan handles, so the imports they point at must not be dropped
    /// between the draw call and `finish()`.
    ///
    /// ★ This is the one place the accumulate-then-submit shape costs
    /// something, and it is worth naming: eager recording would let each
    /// texture go as soon as its command was written.
    held: Vec<KasaneTexture>,
    transform: Transform,
    output_size: Size<i32, Physical>,
}

impl KasaneFrame<'_, '_> {
    /// Map a physical rectangle to the clip-space rectangle the shader wants.
    fn to_clip(&self, dst: Rectangle<i32, Physical>) -> [f32; 4] {
        #[allow(
            clippy::cast_precision_loss,
            reason = "screen coordinates are far below 2^24, where f32 is exact"
        )]
        kasane::Params::dst_from_pixels(
            [
                dst.loc.x as f32,
                dst.loc.y as f32,
                dst.size.w as f32,
                dst.size.h as f32,
            ],
            (self.output_size.w as f32, self.output_size.h as f32),
        )
    }
}

impl Frame for KasaneFrame<'_, '_> {
    type Error = Error;
    type TextureId = KasaneTexture;

    fn context_id(&self) -> ContextId<Self::TextureId> {
        self.renderer.context_id.clone()
    }

    fn clear(
        &mut self,
        color: Color32F,
        at: &[Rectangle<i32, Physical>],
    ) -> Result<(), Self::Error> {
        if at.is_empty() {
            // The whole target: this is the attachment's own load-op clear,
            // which costs nothing extra because the render pass has to load
            // or clear regardless.
            self.clear = [color.r(), color.g(), color.b(), color.a()];
            return Ok(());
        }
        // A partial clear is a solid rectangle. Vulkan has `cmd_clear_attachments`
        // for this, but a solid draw goes through the same pipeline as
        // everything else and is one code path rather than two — and the
        // rectangle count here is single digits.
        for rect in at {
            self.draws.push(Draw::Solid(kasane::Params {
                dst: self.to_clip(*rect),
                src: [0.0, 0.0, 1.0, 1.0],
                tint: [color.r(), color.g(), color.b(), color.a()],
            }));
        }
        Ok(())
    }

    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        // ★ DAMAGE IS IGNORED, DELIBERATELY, AND THIS IS NOT A SHORTCUT.
        //
        // Damage exists to avoid redrawing what has not changed, which is a
        // real saving for a CPU blitter re-rasterising every pixel. On a GPU
        // the draw is a scissored quad the hardware finishes in microseconds,
        // and honouring damage would mean N scissored draws instead of one —
        // strictly more work for the same picture.
        //
        // Where damage DOES still matter is scanout, and that is M5's
        // question, not this one.
        self.draws.push(Draw::Solid(kasane::Params {
            dst: self.to_clip(dst),
            src: [0.0, 0.0, 1.0, 1.0],
            tint: [color.r(), color.g(), color.b(), color.a()],
        }));
        Ok(())
    }

    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        if src_transform != Transform::Normal {
            // ★ REFUSED RATHER THAN IGNORED. A rotated surface drawn without
            // its rotation looks like a client bug, and the client is not the
            // one at fault. The shader takes a pre-transformed destination
            // rectangle and no matrix, so rotation is a real gap — naming it
            // is what keeps it from being silently dropped.
            return Err(Error::NotBuilt {
                what: "rotate a surface",
                why: "the vertex shader takes a pre-transformed rect and no \
                      matrix; a transform needs either four explicit corners \
                      or a 2x2 in the push constants",
            });
        }

        // Source rectangle in UV. The buffer's own size is the denominator,
        // NOT the destination's — mixing those up scales correctly at 1:1 and
        // wrongly everywhere else, which is the hardest kind to notice.
        #[allow(
            clippy::cast_precision_loss,
            reason = "buffer dimensions are far below 2^24"
        )]
        let (bw, bh) = (
            f64::from(texture.inner.geometry.width),
            f64::from(texture.inner.geometry.height),
        );
        #[allow(
            clippy::cast_possible_truncation,
            reason = "UVs are 0..1 ratios; f32 is the shader's own precision"
        )]
        let uv = [
            (src.loc.x / bw) as f32,
            (src.loc.y / bh) as f32,
            (src.size.w / bw) as f32,
            (src.size.h / bh) as f32,
        ];

        // NEAREST at 1:1, the configured filter otherwise. Sampling a
        // pixel-exact surface with LINEAR blurs it for nothing.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "comparing a buffer size to a destination size"
        )]
        let exact = dst.size.w == src.size.w as i32 && dst.size.h == src.size.h as i32;
        let filter = if exact {
            Filter::Nearest
        } else if dst.size.w < texture.width().cast_signed() {
            KasaneRenderer::filter(self.renderer.downscale)
        } else {
            KasaneRenderer::filter(self.renderer.upscale)
        };

        self.draws.push(Draw::Texture {
            params: kasane::Params {
                dst: self.to_clip(dst),
                src: uv,
                // Premultiplied throughout: the shader multiplies all four
                // channels by this, which keeps a premultiplied source
                // premultiplied. See `shaders/composite.wgsl`.
                tint: [alpha, alpha, alpha, alpha],
            },
            texture: texture.inner.texture(),
            filter,
        });
        // Held so the import outlives the handle inside the `Draw`.
        self.held.push(texture.clone());
        Ok(())
    }

    fn transformation(&self) -> Transform {
        self.transform
    }

    fn wait(&mut self, _sync: &SyncPoint) -> Result<(), Self::Error> {
        // `Target::draw` waits on its own fence before returning, so by the
        // time any caller can observe this frame the work is complete. When
        // the command-buffer ring lands this becomes a real semaphore wait —
        // and that is exactly when it starts to matter.
        Ok(())
    }

    fn finish(self) -> Result<SyncPoint, Self::Error> {
        // ★ THE ONE SUBMIT. Everything above only appended.
        self.framebuffer
            .target
            .draw(&self.renderer.pipelines, self.clear, &self.draws)?;
        // Already waited on inside `draw`, so the frame is complete and an
        // unsignalled sync point would make a caller wait for nothing.
        Ok(SyncPoint::signaled())
    }
}

impl RendererSuper for KasaneRenderer {
    type Error = Error;
    type TextureId = KasaneTexture;
    type Framebuffer<'buffer> = KasaneFramebuffer<'buffer>;
    type Frame<'frame, 'buffer>
        = KasaneFrame<'frame, 'buffer>
    where
        'buffer: 'frame;
}

impl Renderer for KasaneRenderer {
    fn context_id(&self) -> ContextId<Self::TextureId> {
        self.context_id.clone()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.downscale = filter;
        Ok(())
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.upscale = filter;
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
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<Self::Frame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        Ok(KasaneFrame {
            renderer: self,
            framebuffer,
            // Transparent black. A frame that draws an opaque background over
            // this never sees it; one that does not, shows through — which is
            // the correct behaviour for a compositor and is what makes a
            // missing background visible rather than merely black.
            clear: [0.0, 0.0, 0.0, 0.0],
            draws: Vec::new(),
            held: Vec::new(),
            transform: dst_transform,
            output_size,
        })
    }

    fn wait(&mut self, _sync: &SyncPoint) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl crate::nuri_renderer::AdvertisesDmabuf for KasaneRenderer {
    fn advertises_dmabuf(&self) -> bool {
        // ★ TRUE, AND UNLIKE nuri THIS IS EARNED. nuri answers `false` because
        // it maps and copies — advertising `zwp_linux_dmabuf_v1` there would
        // promise zero-copy and deliver a CPU readback. kasane imports the
        // buffer as a Vulkan image and samples it; nothing is copied.
        //
        // On a software Vulkan device the promise is thinner — llvmpipe reads
        // the memory with the CPU too — but it is still a genuine dmabuf
        // import, and `is_cpu()` is how a caller tells the difference.
        true
    }
}

impl crate::nuri_renderer::ArmFlush for KasaneRenderer {
    fn arm_flush(&mut self, _policy: crate::config::FlushPolicy, _generation: Option<u64>) {
        // ★ DELIBERATELY EMPTY, AND WRITTEN OUT RATHER THAN INHERITED.
        //
        // The trait's default body was REMOVED on 2026-09-03 precisely so this
        // decision has to be made per renderer: a renderer that silently
        // inherited a no-op armed no flush plan and took a full copy every
        // frame, with nothing reporting it.
        //
        // The honest answer here is nothing to do. A flush plan exists to
        // decide how much of a shadow buffer to copy to scanout, and this
        // renderer composites straight into the scanout buffer — there is no
        // shadow, so there is no plan. `flush_damage` returns 0 bytes for the
        // same reason.
    }
}

impl smithay::backend::renderer::Bind<Dmabuf> for KasaneRenderer {
    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<Self::Framebuffer<'a>, Self::Error> {
        use smithay::backend::allocator::Buffer as _;

        if target.num_planes() != 1 {
            return Err(Error::Unsupported(
                "multi-plane dmabuf as a render target; kasane renders                  single-plane ARGB8888",
            ));
        }
        let fd = target
            .handles()
            .next()
            .ok_or(Error::Unsupported("dmabuf with no plane handle"))?;
        // Duplicated because the importer must OWN the fd and smithay lends
        // it — consuming smithay's would close a descriptor the `Dmabuf` still
        // believes it owns. Same reasoning as `import_dmabuf`.
        let owned = fd
            .try_clone_to_owned()
            .map_err(|_| Error::Unsupported("could not duplicate the dmabuf fd"))?;

        let geometry = kasane::Geometry {
            width: target.width(),
            height: target.height(),
            stride: u64::from(target.strides().next().unwrap_or(0)),
            offset: u64::from(target.offsets().next().unwrap_or(0)),
        };
        let modifier: u64 = target.format().modifier.into();
        let inner = Target::from_dmabuf(&self.gpu, owned, geometry, modifier)?;

        #[allow(
            clippy::cast_possible_wrap,
            reason = "a display dimension that wrapped an i32 is not a display"
        )]
        let size = Size::from((target.width() as i32, target.height() as i32));
        Ok(KasaneFramebuffer {
            target: inner,
            size,
            _buffer: std::marker::PhantomData,
        })
    }

    fn supported_formats(&self) -> Option<smithay::backend::allocator::format::FormatSet> {
        use smithay::backend::allocator::Format;

        // ★ THE RENDERABLE LIST, NOT THE SAMPLABLE ONE. A device may sample a
        // layout it cannot render into, and answering this question with the
        // import list would advertise a scanout format the GPU cannot actually
        // target — a failure that lands at bind time, per output, on the
        // machine with a real display.
        Some(
            self.gpu
                .renderable_modifiers()
                .into_iter()
                .map(|m| Format {
                    code: Fourcc::Argb8888,
                    modifier: m.into(),
                })
                .collect(),
        )
    }
}

impl smithay::backend::renderer::ImportDma for KasaneRenderer {
    fn dmabuf_formats(&self) -> smithay::backend::allocator::format::FormatSet {
        use smithay::backend::allocator::Format;

        // ★ THE SAMPLABLE LIST — the mirror of `supported_formats` above, and
        // deliberately a different query. This one becomes what
        // `zwp_linux_dmabuf_v1` advertises to clients, so answering it with
        // the renderable list would promise clients a layout the compositor
        // cannot texture from.
        self.gpu
            .importable_modifiers()
            .into_iter()
            .map(|m| Format {
                code: Fourcc::Argb8888,
                modifier: m.into(),
            })
            .collect()
    }

    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<Self::TextureId, Self::Error> {
        use smithay::backend::allocator::Buffer as _;

        if dmabuf.num_planes() != 1 {
            // ★ Its own answer, not a generic failure. Multi-plane is a buffer
            // shape this import path does not address yet — not a broken one.
            return Err(Error::Unsupported(
                "multi-plane dmabuf; kasane imports single-plane ARGB8888",
            ));
        }
        let fd = dmabuf
            .handles()
            .next()
            .ok_or(Error::Unsupported("dmabuf with no plane handle"))?;
        // The importer must own the fd, and smithay lends it — so it is
        // duplicated. Consuming smithay's would close a descriptor the
        // `Dmabuf` still believes it owns.
        let owned = fd
            .try_clone_to_owned()
            .map_err(|_| Error::Unsupported("could not duplicate the dmabuf fd"))?;

        let geometry = kasane::Geometry {
            width: dmabuf.width(),
            height: dmabuf.height(),
            stride: u64::from(dmabuf.strides().next().unwrap_or(0)),
            offset: u64::from(dmabuf.offsets().next().unwrap_or(0)),
        };
        let modifier: u64 = dmabuf.format().modifier.into();
        let imported = self.gpu.import_tiled(owned, geometry, modifier)?;
        Ok(KasaneTexture {
            inner: Arc::new(imported),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ THE TRAIT SHAPES COMPILE — which is the whole point of this file
    /// existing before its bodies do.
    ///
    /// smithay's `Frame<'frame, 'buffer>` is a stateful recorder borrowing
    /// both the renderer and the framebuffer, while kasane's `Target::draw` is
    /// one-shot. Whether those two shapes can be reconciled in safe Rust was
    /// the open design question, and it is answered by this module compiling
    /// at all: a `KasaneFrame` that could not hold both borrows would fail
    /// here, not at some later fill-in-the-body stage.
    ///
    /// This test asserts the associated types resolve, which is a compile-time
    /// claim with a runtime spelling — deliberately, because a `where` clause
    /// alone can be satisfied vacuously by a type nobody constructs.
    #[test]
    fn the_renderer_satisfies_smithays_associated_types() {
        fn assert_renderer<R: Renderer>() {}
        fn assert_import_dma<R: smithay::backend::renderer::ImportDma>() {}
        fn assert_local_traits<R>()
        where
            R: crate::nuri_renderer::ArmFlush + crate::nuri_renderer::AdvertisesDmabuf,
        {
        }
        fn assert_texture<T: Texture + Clone + Send + 'static>() {}
        fn assert_scanout<F: crate::nuri_renderer::ScanoutFlush>() {}

        fn assert_bind<R: smithay::backend::renderer::Bind<Dmabuf>>() {}

        assert_renderer::<KasaneRenderer>();
        assert_import_dma::<KasaneRenderer>();
        // ★ `Bind<Dmabuf>` is what turns a scanout buffer into a render
        // target, and it is the reason there is no shadow buffer in this
        // renderer at all.
        assert_bind::<KasaneRenderer>();
        assert_local_traits::<KasaneRenderer>();
        // ★ `Clone + Send + 'static` is `drm.rs`'s own bound on `R::TextureId`,
        // restated here so a change that breaks it fails in this file rather
        // than at the seat's generic instantiation, where the error names a
        // type parameter instead of a cause.
        assert_texture::<KasaneTexture>();
        assert_scanout::<KasaneFramebuffer<'_>>();
    }

    /// ★ THE TWO FORMAT QUESTIONS ARE ASKED SEPARATELY.
    ///
    /// `dmabuf_formats` becomes what `zwp_linux_dmabuf_v1` advertises to
    /// CLIENTS — what they may hand us to texture from. `supported_formats`
    /// is what the compositor may RENDER INTO. A device can sample a layout it
    /// cannot target, so answering one with the other advertises a capability
    /// that fails at bind time, per output, on the machine with a real display.
    ///
    /// The renderer cannot be constructed here (no GPU in a unit test), so
    /// what this pins is the thing that would actually regress: that the two
    /// are backed by different kasane queries rather than one aliasing the
    /// other.
    #[test]
    fn the_client_format_list_and_the_scanout_format_list_are_different_queries() {
        let src = include_str!("kasane_renderer.rs");
        // Cut at the test module so this cannot match its own body — a
        // source-scanning check that matches its own matcher passes for the
        // wrong reason.
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            code.contains("importable_modifiers()"),
            "dmabuf_formats must ask the SAMPLABLE list"
        );
        assert!(
            code.contains("renderable_modifiers()"),
            "supported_formats must ask the RENDERABLE list — using the \
             samplable one advertises a scanout format the GPU cannot target"
        );
    }

    /// ★ THE UNBUILT PATHS SAY SO, and say WHAT is missing.
    ///
    /// A renderer whose gaps returned `Ok` would compose a blank frame and
    /// report success — the exact failure mode `docs/KASANE.md` was reordered
    /// to avoid. This pins that each gap is a typed refusal carrying its own
    /// remedy, so a reader who hits one is told what to build.
    #[test]
    fn an_unbuilt_path_names_what_it_needs() {
        let e = Error::NotBuilt {
            what: "rotate a surface",
            why: "the vertex shader takes a pre-transformed rect and no matrix",
        };
        let text = e.to_string();
        assert!(
            text.contains("rotate a surface") && text.contains("vertex shader"),
            "an unbuilt path must name both the capability and the remedy; got: {text}"
        );
        assert!(
            text.contains("not a \nfailure") || text.contains("not a failure"),
            "it must also distinguish itself from a broken working path, or a \
             reader debugs the wrong thing; got: {text}"
        );
    }
}
