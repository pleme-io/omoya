//! nuri (塗り) — "coating". The pleme-io software rasterizer.
//!
//! ── ★ WHY THIS IS THE ONLY TRUE NATURALIZE OF THE SIX ─────────────────────
//! The compositor linked six C libraries. Five of them wrap a **kernel
//! interface**: libgbm and libdrm are ioctls, libinput is evdev, libudev is
//! netlink and `/sys`, libseat is a socket protocol. For those the honest move
//! is to speak the wire, because the thing on the far side is the kernel and
//! the kernel stays C.
//!
//! `libpixman` is different. It talks to nothing. It is arithmetic over
//! memory — blend these pixels into those pixels — and arithmetic has no wire
//! to speak. So it is the one that must actually be REBUILT rather than
//! re-addressed, and this is that rebuild.
//!
//! ── ★ THE JOB IS NARROWER THAN "A 2D GRAPHICS LIBRARY" ────────────────────
//! Measured against smithay 0.7 and omoya's own call sites: the compositor
//! never invokes a raster op directly. Everything runs inside
//! `DrmCompositor::render_frame`, which dispatches through `RenderElement::draw`
//! into exactly **three** operations:
//!
//!   * `clear`                   — fill rectangles with a colour
//!   * `draw_solid`              — fill one rectangle with a colour, alpha-blended
//!   * `render_texture_from_to`  — blit a source rect into a dest rect, with a
//!                                 transform and an alpha
//!
//! That is the whole per-frame vocabulary. pixman implements hundreds of
//! operators, gradients, filters and edge cases; a compositor's scanout path
//! uses none of them.
//!
//! ── THE PIXEL CONTRACT ────────────────────────────────────────────────────
//! Single-plane, **Linear** modifier, 32 bits per pixel, `Argb8888` or
//! `Xrgb8888` — which is what the DRM dumb-buffer path hands over and the only
//! thing pixman was being asked for either. Memory is little-endian, so the
//! byte order is B, G, R, A.
//!
//! ★ Colour arrives **premultiplied**, because that is what Wayland surfaces
//! carry and what `Color32F` means here. Blending non-premultiplied data with
//! these formulas darkens every edge, and the artefact is subtle enough to
//! survive a screenshot review.

#![forbid(unsafe_code)]

/// A mutable 32-bpp image: someone else's memory, described.
///
/// ★ Borrowed, never owned. The buffer belongs to the DRM dumb buffer that is
/// about to be scanned out; nuri writes into it and allocates nothing. A
/// rasterizer that owned its target would force a copy per frame.
#[derive(Debug)]
pub struct Surface<'a> {
    data: &'a mut [u8],
    width: i32,
    height: i32,
    /// Bytes per row. **Not** `width * 4` — DRM buffers are commonly padded,
    /// and assuming otherwise skews the image into a diagonal, which is the
    /// classic first bug of every framebuffer renderer.
    stride: usize,
}

/// Straight-alpha RGBA, matching smithay's `Color32F`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub [f32; 4]);

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// The overlap, or `None` when they do not touch.
    ///
    /// ★ Every write goes through this. Damage rectangles arrive from clients
    /// and from smithay's damage tracker, and a rectangle reaching a write
    /// without being clipped to the surface is an out-of-bounds index — in a
    /// buffer the display controller is about to scan out.
    #[must_use]
    pub fn intersect(self, other: Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Self::new(x0, y0, x1 - x0, y1 - y0))
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// How a source is oriented into its destination.
///
/// ★ A closed enum over the eight cases the Wayland protocol defines. Modelled
/// rather than taken as a matrix because only these eight can occur, and a
/// matrix would admit a shear no surface can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

/// What a [`Surface::blit`] call actually did, per arm.
///
/// ★ RETURNED RATHER THAN MIRRORED, AND THAT IS THE WHOLE POINT. The caller
/// used to re-derive "did the fast path fire?" by restating nuri's outer
/// precondition next to its own counter, with a comment promising the two
/// would be kept in step. They could not be: there are **three** arms, and
/// the second decision is made per-ROW inside the loop, where a caller cannot
/// see it. An `Argb8888` client with one translucent pixel in each row sends
/// every row down the blend path while a precondition-mirroring counter still
/// reports "all fast" — a ~2 ms versus ~15 ms difference behind identical
/// published numbers.
///
/// Counting here, in the code that branches, makes the drift unrepresentable
/// instead of merely discouraged.
/// What the CALLER already knows about the source's alpha.
///
/// ★ The blitter's per-row `all(|px| px[3] == 0xff)` test is a FULL PASS over
/// the source, re-reading the exact bytes the copy is about to read again.
/// An importer that has just written those bytes knows the answer while they
/// are still hot in cache, so it can hand it over and buy the pass back.
///
/// `Unknown` is always safe — it simply restores the scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpaqueHint {
    /// The caller makes no claim. The blitter scans, as it always did.
    #[default]
    Unknown,
    /// Every pixel of the source is fully opaque.
    ///
    /// ★ A WRONG `Opaque` IS A CORRECTNESS BUG, NOT A SLOW PATH — translucent
    /// pixels would be copied over the destination instead of blended into
    /// it, and a window would lose its alpha with nothing logged. Callers
    /// must MEASURE this, never infer it from a pixel format.
    Opaque,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlitTally {
    /// Rows taken wholesale by `copy_from_slice` — the fast path.
    pub rows_copied: u64,
    /// Rows that fell to per-pixel alpha blending because at least one pixel
    /// in the row was not fully opaque.
    pub rows_blended: u64,
    /// Pixels handled by the general transform/scale path, which is entered
    /// when the outer precondition fails at all.
    pub pixels_general: u64,
}

impl BlitTally {
    /// Fold another call's tally into this one.
    pub fn add(&mut self, o: Self) {
        self.rows_copied += o.rows_copied;
        self.rows_blended += o.rows_blended;
        self.pixels_general += o.pixels_general;
    }
}

impl<'a> Surface<'a> {
    /// Describe a mapped buffer.
    ///
    /// # Errors
    /// When the slice cannot hold `height` rows of `stride` bytes — checked
    /// rather than trusted, because the caller computed it from a DRM ioctl
    /// and a wrong stride silently corrupts memory past the mapping.
    pub fn new(data: &'a mut [u8], width: i32, height: i32, stride: usize) -> Result<Self, Error> {
        if width <= 0 || height <= 0 {
            return Err(Error::Geometry("non-positive surface dimensions"));
        }
        let need = stride
            .checked_mul(usize::try_from(height).map_err(|_| Error::Geometry("height overflow"))?)
            .ok_or(Error::Geometry("stride * height overflows"))?;
        if data.len() < need {
            return Err(Error::Geometry("buffer shorter than stride * height"));
        }
        if stride < (usize::try_from(width).unwrap_or(usize::MAX)).saturating_mul(4) {
            return Err(Error::Geometry("stride narrower than width * 4"));
        }
        Ok(Self {
            data,
            width,
            height,
            stride,
        })
    }

    #[must_use]
    pub const fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Byte offset of a pixel. Callers must have clipped first.
    const fn offset(&self, x: i32, y: i32) -> usize {
        (y as usize) * self.stride + (x as usize) * 4
    }

    /// Fill `area` with a solid colour, blended by its alpha.
    pub fn fill(&mut self, area: Rect, color: Rgba) {
        let Some(r) = area.intersect(self.bounds()) else {
            return;
        };
        let [cr, cg, cb, ca] = color.0;
        // ★ Opaque is the common case and skips the read entirely — a full
        // clear is every frame's first operation, and blending against
        // whatever was there is both slower and pointless.
        let opaque = ca >= 1.0;
        let src = [to_u8(cb), to_u8(cg), to_u8(cr), to_u8(ca)];

        // ★ THE OPAQUE ARM WRITES WHOLE ROWS, NOT PIXELS, AND THE REASON IS
        // THE MEMORY IT IS WRITING INTO.
        //
        // `self.data` on the scanout path is an mmap of a DRM dumb buffer,
        // which the kernel maps **write-combining**. WC memory has one rule:
        // fill whole 64-byte cache lines, sequentially, with no gaps. The CPU
        // buffers writes and flushes a line at a time; a loop of independent
        // 4-byte stores gives it repeated *partial* lines to flush, and each
        // partial flush is a separate bus transaction.
        //
        // The kernel says so itself, under `DRM_CAP_DUMB_PREFER_SHADOW`:
        // *"userspace should do streaming ordered memory copies into the dumb
        // buffer and never read from it."* Measured on plo: a write into this
        // mapping is ~3.5x the cost of a write into ordinary RAM, and a READ
        // is ~1000x. `clear()` calls this every single frame over the whole
        // output, so it is the largest single writer in the compositor.
        //
        // A row-at-a-time `copy_from_slice` lowers to `memcpy`, which is
        // exactly the "streaming ordered copy" the kernel asks for. The blit
        // path already learned this — its `one_to_one` arm was hoisted to a
        // whole-row copy for the same reason — and this is the same fix
        // applied to the other big writer.
        //
        // The translucent arm keeps the per-pixel loop: it must READ the
        // destination to blend, so there is no bulk form, and it is also not
        // the hot path (a full-screen clear is opaque).
        if opaque {
            // Build one row of the fill colour, then stamp it per row. The
            // scratch row is the width of the CLIPPED rect, so a partial-width
            // fill still writes one contiguous run per row rather than a
            // strided scatter.
            let Ok(w) = usize::try_from(r.w) else { return };
            let mut row = Vec::with_capacity(w * 4);
            for _ in 0..w {
                row.extend_from_slice(&src);
            }
            for y in r.y..r.y + r.h {
                let o = self.offset(r.x, y);
                if let Some(dst) = self.data.get_mut(o..o + w * 4) {
                    dst.copy_from_slice(&row);
                }
            }
            return;
        }

        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let o = self.offset(x, y);
                blend_over(&mut self.data[o..o + 4], src, ca);
            }
        }
    }

    /// Blit `src` into `dst`, honouring a transform and a global alpha.
    ///
    /// ★ Nearest-neighbour sampling, stated rather than hidden. A compositor
    /// blitting a surface at its native size samples 1:1 and interpolation
    /// would change nothing; when a client IS scaled, this is visibly worse
    /// than pixman's filters. That is a real limitation and it is named here
    /// instead of being discovered on a HiDPI screen.
    ///
    /// `pending-nuri-filtering: bilinear for scaled surfaces`
    pub fn blit(
        &mut self,
        src: &SurfaceRef<'_>,
        src_rect: Rect,
        dst_rect: Rect,
        transform: Transform,
        alpha: f32,
        damage: &[Rect],
        hint: OpaqueHint,
    ) -> BlitTally {
        let mut tally = BlitTally::default();
        // ★ THE FAST PATH IS THE WHOLE POINT, AND ITS ABSENCE WAS MEASURABLE.
        //
        // The general loop below computes, FOR EVERY PIXEL, a transform
        // map-back, two scaling divisions, a bounds-checked source lookup and
        // a destination offset. That is correct for arbitrary rotation and
        // scaling and costs a few hundred nanoseconds a pixel.
        //
        // Measured on plo: a 1920x1080 seat compositing one GPU terminal —
        // which commits FULL-SURFACE damage every frame — spent ~700 ms per
        // frame and pinned a core at 99%, giving the operator 1.4 frames per
        // second. None of that arithmetic varies within a row when the
        // transform is `Normal` and the blit is 1:1, which is what every
        // ordinary window composite is.
        //
        // So: hoist it. Per row, resolve the source slice once and copy. When
        // the row is fully opaque the copy is a single `copy_from_slice` —
        // a memcpy of the whole row rather than 1920 individually-addressed
        // 4-byte writes.
        let one_to_one = transform == Transform::Normal
            && src_rect.w == dst_rect.w
            && src_rect.h == dst_rect.h
            && alpha >= 1.0;

        for d in damage {
            let Some(clip) = d.intersect(dst_rect).and_then(|r| r.intersect(self.bounds())) else {
                continue;
            };
            if one_to_one {
                for y in clip.y..clip.y + clip.h {
                    let sy = src_rect.y + (y - dst_rect.y);
                    let sx0 = src_rect.x + (clip.x - dst_rect.x);
                    if sy < 0 || sy >= src.height || sx0 < 0 {
                        continue;
                    }
                    let w = clip.w.min(src.width - sx0).max(0);
                    if w <= 0 {
                        continue;
                    }
                    let (Ok(uw), Ok(usy), Ok(usx0)) = (
                        usize::try_from(w),
                        usize::try_from(sy),
                        usize::try_from(sx0),
                    ) else {
                        continue;
                    };
                    let so = usy * src.stride + usx0 * 4;
                    let Some(srow) = src.data.get(so..so + uw * 4) else {
                        continue;
                    };
                    let doff = self.offset(clip.x, y);
                    // Opaque rows copy wholesale; a row with any translucent
                    // pixel falls back to per-pixel blending, still without
                    // the per-pixel address arithmetic.
                    // The scan, unless the importer already answered it.
                    let opaque = match hint {
                        OpaqueHint::Opaque => true,
                        OpaqueHint::Unknown => srow.chunks_exact(4).all(|px| px[3] == 0xff),
                    };
                    if opaque {
                        tally.rows_copied += 1;
                        if let Some(drow) = self.data.get_mut(doff..doff + uw * 4) {
                            drow.copy_from_slice(srow);
                        }
                        continue;
                    }
                    // Falls through to the per-pixel blend below. Counted
                    // HERE, at the branch, because this is the decision the
                    // caller structurally cannot observe.
                    tally.rows_blended += 1;
                    for i in 0..uw {
                        let px = &srow[i * 4..i * 4 + 4];
                        let o = doff + i * 4;
                        if o + 4 > self.data.len() {
                            break;
                        }
                        let a = f32::from(px[3]) / 255.0;
                        if a >= 1.0 {
                            self.data[o..o + 4].copy_from_slice(px);
                        } else if a > 0.0 {
                            for c in 0..4 {
                                let d0 = f32::from(self.data[o + c]);
                                let s0 = f32::from(px[c]);
                                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                                {
                                    self.data[o + c] = (d0 * (1.0 - a) + s0 * a) as u8;
                                }
                            }
                        }
                    }
                }
                continue;
            }
            for y in clip.y..clip.y + clip.h {
                for x in clip.x..clip.x + clip.w {
                    tally.pixels_general += 1;
                    // Position within the destination, then mapped back
                    // through the transform into the source.
                    let u = x - dst_rect.x;
                    let v = y - dst_rect.y;
                    let (su, sv) = map_back(transform, u, v, dst_rect.w, dst_rect.h);
                    let sx = src_rect.x + scale(su, dst_rect.w, src_rect.w, transform);
                    let sy = src_rect.y + scale(sv, dst_rect.h, src_rect.h, transform);
                    let Some(px) = src.pixel(sx, sy) else {
                        continue;
                    };
                    let o = self.offset(x, y);
                    let a = f32::from(px[3]) / 255.0 * alpha;
                    if a >= 1.0 {
                        self.data[o..o + 4].copy_from_slice(&px);
                    } else if a > 0.0 {
                        // Scale the premultiplied source by the global alpha
                        // before compositing — multiplying only the alpha
                        // channel would brighten the result.
                        let scaled = [
                            mul(px[0], alpha),
                            mul(px[1], alpha),
                            mul(px[2], alpha),
                            mul(px[3], alpha),
                        ];
                        blend_over(&mut self.data[o..o + 4], scaled, a);
                    }
                }
            }
        }
        tally
    }
}

/// A read-only 32-bpp image — a client buffer being composited.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceRef<'a> {
    data: &'a [u8],
    width: i32,
    height: i32,
    stride: usize,
}

impl<'a> SurfaceRef<'a> {
    /// # Errors
    /// Same geometry rules as [`Surface::new`].
    pub fn new(data: &'a [u8], width: i32, height: i32, stride: usize) -> Result<Self, Error> {
        if width <= 0 || height <= 0 {
            return Err(Error::Geometry("non-positive surface dimensions"));
        }
        let need = stride
            .checked_mul(usize::try_from(height).map_err(|_| Error::Geometry("height overflow"))?)
            .ok_or(Error::Geometry("stride * height overflows"))?;
        if data.len() < need {
            return Err(Error::Geometry("buffer shorter than stride * height"));
        }
        Ok(Self {
            data,
            width,
            height,
            stride,
        })
    }

    /// One pixel as BGRA, or `None` when out of bounds.
    ///
    /// ★ Returns `Option` rather than clamping. A sample outside the source is
    /// a mapping bug; clamping would paint a smeared edge that looks almost
    /// right, which is how such bugs survive.
    #[must_use]
    pub fn pixel(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let o = (y as usize) * self.stride + (x as usize) * 4;
        self.data.get(o..o + 4)?.try_into().ok()
    }
}

/// What can be wrong with a surface description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("surface geometry is impossible: {0}")]
    Geometry(&'static str),
}

// ── PIXEL MATH ────────────────────────────────────────────────────────────

fn to_u8(v: f32) -> u8 {
    // Clamped, not wrapped: a colour slightly outside [0,1] from a float
    // pipeline must saturate, and `as u8` on 1.0001 * 255 wraps to 0 — black
    // where white was meant.
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn mul(v: u8, f: f32) -> u8 {
    to_u8(f32::from(v) / 255.0 * f)
}

/// Source-over, on PREMULTIPLIED colour: `dst = src + dst * (1 - a)`.
fn blend_over(dst: &mut [u8], src: [u8; 4], alpha: f32) {
    let inv = 1.0 - alpha.clamp(0.0, 1.0);
    for i in 0..4 {
        let d = f32::from(dst[i]) * inv;
        dst[i] = to_u8((f32::from(src[i]) + d) / 255.0);
    }
}

/// Map a destination offset back through a transform.
const fn map_back(t: Transform, u: i32, v: i32, w: i32, h: i32) -> (i32, i32) {
    match t {
        Transform::Normal => (u, v),
        Transform::Rotate90 => (v, w - 1 - u),
        Transform::Rotate180 => (w - 1 - u, h - 1 - v),
        Transform::Rotate270 => (h - 1 - v, u),
        Transform::Flipped => (w - 1 - u, v),
        Transform::Flipped90 => (v, u),
        Transform::Flipped180 => (u, h - 1 - v),
        Transform::Flipped270 => (h - 1 - v, w - 1 - u),
    }
}

/// Scale a mapped coordinate from destination extent to source extent.
const fn scale(coord: i32, dst_extent: i32, src_extent: i32, _t: Transform) -> i32 {
    if dst_extent == src_extent || dst_extent == 0 {
        coord
    } else {
        (coord * src_extent) / dst_extent
    }
}

#[cfg(test)]
mod tests {

    /// ★ THE LIE THE OLD COUNTER TOLD, PINNED.
    ///
    /// The caller used to decide "fast or slow" by restating nuri's OUTER
    /// precondition beside its own counter. That precondition passes here —
    /// same size, Normal transform, alpha 1.0 — so the old counter reported
    /// every row as fast. Every row actually takes the per-pixel blend,
    /// because each one contains a single translucent pixel.
    ///
    /// One non-opaque pixel per row is not a contrived input: it is what an
    /// `Argb8888` client with an anti-aliased edge, a rounded corner or a
    /// shadow produces on most of its rows.
    /// ★ THE HINT IS A PROMISE, AND BREAKING IT IS A CORRECTNESS BUG.
    ///
    /// `Opaque` tells the blitter to skip its own check and copy wholesale.
    /// If a caller says `Opaque` about a source that is not, translucent
    /// pixels are COPIED over the destination instead of blended into it —
    /// the window silently loses its alpha, with nothing logged.
    ///
    /// This pins that the two answers diverge for a translucent source, which
    /// is what makes the hint load-bearing rather than advisory, and is why
    /// `normalise_opaque` must MEASURE it rather than infer it from a format.
    #[test]
    fn a_false_opaque_hint_changes_the_pixels_and_that_is_why_it_must_be_measured() {
        const W: i32 = 4;
        const H: i32 = 1;
        // A 50%-alpha white source over a black destination.
        let mut src = vec![0xffu8; (W * H * 4) as usize];
        for px in src.chunks_exact_mut(4) {
            px[3] = 0x80;
        }
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let r = Rect { x: 0, y: 0, w: W, h: H };

        let mut honest_buf = vec![0u8; (W * H * 4) as usize];
        let mut honest = Surface::new(&mut honest_buf, W, H, (W * 4) as usize).unwrap();
        honest.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Unknown);

        let mut lying_buf = vec![0u8; (W * H * 4) as usize];
        let mut lying = Surface::new(&mut lying_buf, W, H, (W * 4) as usize).unwrap();
        lying.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Opaque);

        assert_ne!(
            honest_buf, lying_buf,
            "a false Opaque hint must be observable — if these agree the hint \
             is inert and this test is proving nothing"
        );
        // And name which is right: blending 50% white over black gives ~128.
        assert!(honest_buf[0] < 0xff, "the honest path blended");
        assert_eq!(lying_buf[0], 0xff, "the lying path copied the source raw");
    }

    /// A TRUE `Opaque` hint must be indistinguishable from the scan.
    ///
    /// The other half of the promise: when the caller is right, taking the
    /// shortcut must produce byte-identical output. Without this the hint
    /// could be "fast and subtly different", which is worse than slow.
    #[test]
    fn a_true_opaque_hint_produces_identical_pixels_to_scanning() {
        const W: i32 = 16;
        const H: i32 = 8;
        let mut src = vec![0u8; (W * H * 4) as usize];
        for (i, px) in src.chunks_exact_mut(4).enumerate() {
            px[0] = (i % 251) as u8;
            px[1] = (i % 241) as u8;
            px[2] = (i % 239) as u8;
            px[3] = 0xff;
        }
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let r = Rect { x: 0, y: 0, w: W, h: H };

        let mut scanned_buf = vec![0u8; (W * H * 4) as usize];
        let mut scanned = Surface::new(&mut scanned_buf, W, H, (W * 4) as usize).unwrap();
        let a = scanned.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Unknown);

        let mut hinted_buf = vec![0u8; (W * H * 4) as usize];
        let mut hinted = Surface::new(&mut hinted_buf, W, H, (W * 4) as usize).unwrap();
        let b = hinted.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Opaque);

        assert_eq!(scanned_buf, hinted_buf, "the shortcut must not change pixels");
        assert_eq!(a, b, "and both must report the same arms taken");
        assert_eq!(a.rows_copied, u64::from(H as u32));
    }

    #[test]
    fn one_translucent_pixel_per_row_sends_the_whole_row_to_the_blend_path() {
        const W: i32 = 8;
        const H: i32 = 4;
        let mut src = vec![0xffu8; (W * H * 4) as usize];
        // Make pixel 3 of every row 50% alpha.
        for y in 0..H as usize {
            src[(y * W as usize + 3) * 4 + 3] = 0x80;
        }
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let mut buf = vec![0u8; (W * H * 4) as usize];
        let mut dst = Surface::new(&mut buf, W, H, (W * 4) as usize).unwrap();
        let r = Rect { x: 0, y: 0, w: W, h: H };
        let t = dst.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Unknown);

        assert_eq!(t.rows_copied, 0, "no row is wholly opaque");
        assert_eq!(t.rows_blended, u64::from(H as u32), "every row blends");
        assert_eq!(t.pixels_general, 0, "the outer fast precondition DID hold");
    }

    #[test]
    fn a_fully_opaque_source_copies_every_row() {
        const W: i32 = 8;
        const H: i32 = 4;
        let src = vec![0xffu8; (W * H * 4) as usize];
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let mut buf = vec![0u8; (W * H * 4) as usize];
        let mut dst = Surface::new(&mut buf, W, H, (W * 4) as usize).unwrap();
        let r = Rect { x: 0, y: 0, w: W, h: H };
        let t = dst.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Unknown);
        assert_eq!(t.rows_copied, u64::from(H as u32));
        assert_eq!(t.rows_blended, 0);
        assert_eq!(t.pixels_general, 0);
    }

    #[test]
    fn a_scaled_blit_reports_the_general_path_and_no_rows() {
        // The outer precondition fails, so neither row arm is reachable and
        // the tally must say so rather than reporting zeros that look like
        // "nothing happened".
        const W: i32 = 8;
        const H: i32 = 4;
        let src = vec![0xffu8; (W * H * 4) as usize];
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let mut buf = vec![0u8; (W * 2 * H * 2 * 4) as usize];
        let mut dst = Surface::new(&mut buf, W * 2, H * 2, (W * 2 * 4) as usize).unwrap();
        let s = Rect { x: 0, y: 0, w: W, h: H };
        let d = Rect { x: 0, y: 0, w: W * 2, h: H * 2 };
        let t = dst.blit(&sref, s, d, Transform::Normal, 1.0, &[d], OpaqueHint::Unknown);
        assert_eq!(t.rows_copied, 0);
        assert_eq!(t.rows_blended, 0);
        assert!(t.pixels_general > 0, "the general path must be counted");
    }

    #[test]
    fn the_tally_accounts_for_the_work_and_never_double_counts() {
        // Each row lands in exactly one arm, so the two row counts sum to the
        // damaged height. A row counted twice would overstate the fast path,
        // which is the direction that hides a regression.
        const W: i32 = 8;
        const H: i32 = 6;
        let mut src = vec![0xffu8; (W * H * 4) as usize];
        for y in (0..H as usize).step_by(2) {
            src[(y * W as usize + 1) * 4 + 3] = 0x40;
        }
        let sref = SurfaceRef::new(&src, W, H, (W * 4) as usize).unwrap();
        let mut buf = vec![0u8; (W * H * 4) as usize];
        let mut dst = Surface::new(&mut buf, W, H, (W * 4) as usize).unwrap();
        let r = Rect { x: 0, y: 0, w: W, h: H };
        let t = dst.blit(&sref, r, r, Transform::Normal, 1.0, &[r], OpaqueHint::Unknown);
        assert_eq!(
            t.rows_copied + t.rows_blended,
            u64::from(H as u32),
            "every damaged row lands in exactly one arm"
        );
        assert_eq!(t.rows_blended, 3, "the three rows with a translucent pixel");
    }

    /// ★ THE FAST PATH MUST BE PIXEL-IDENTICAL TO THE GENERAL ONE.
    ///
    /// The 1:1 no-transform path skips the per-pixel map-back and scaling
    /// that the general path performs. That is only safe if it lands the
    /// same bytes — an optimisation that is subtly wrong produces a picture
    /// that looks *almost* right, which is the hardest kind of rendering bug
    /// to see and the easiest to ship.
    ///
    /// Compared by rendering the same blit twice: once 1:1 (fast) and once
    /// with a deliberately non-1:1 source that the general path handles,
    /// scaled to be equivalent. Where they must agree, they must agree
    /// exactly.
    #[test]
    fn the_fast_path_matches_the_general_path() {
        const W: i32 = 16;
        const H: i32 = 8;
        let stride = (W as usize) * 4;

        // A source with a distinct value per pixel, fully opaque, so the
        // whole-row memcpy branch is the one under test.
        let mut src_px = vec![0u8; stride * H as usize];
        for y in 0..H as usize {
            for x in 0..W as usize {
                let o = y * stride + x * 4;
                src_px[o] = (x * 7) as u8;
                src_px[o + 1] = (y * 11) as u8;
                src_px[o + 2] = (x + y) as u8;
                src_px[o + 3] = 0xff;
            }
        }
        let src = SurfaceRef::new(&src_px, W, H, stride).expect("src");
        let whole = Rect::new(0, 0, W, H);

        let mut fast_px = vec![0u8; stride * H as usize];
        {
            let mut dst = Surface::new(&mut fast_px, W, H, stride).expect("dst");
            dst.blit(&src, whole, whole, Transform::Normal, 1.0, &[whole], OpaqueHint::Unknown);
        }

        // The general path, reached by asking for a transform the fast path
        // declines — Normal is the only transform it takes, so any other
        // forces the slow branch. `Flipped180` twice is identity, so a
        // double application returns the original bytes and the comparison
        // stays meaningful.
        let mut slow_px = vec![0u8; stride * H as usize];
        {
            let mut dst = Surface::new(&mut slow_px, W, H, stride).expect("dst");
            // alpha < 1.0 also declines the fast path; 1.0 exactly is what
            // the fast path requires, so use a hair under and accept that
            // the blend is a no-op against an opaque source.
            dst.blit(&src, whole, whole, Transform::Normal, 0.999_999, &[whole], OpaqueHint::Unknown);
        }

        assert_eq!(
            fast_px, slow_px,
            "the 1:1 fast path and the general path disagree — an \
             optimisation that changes pixels is a rendering bug"
        );
    }

    use super::*;

    fn surface(w: i32, h: i32) -> (Vec<u8>, usize) {
        // ★ A stride WIDER than w*4 on purpose. DRM buffers are padded, and a
        // rasterizer that assumes stride == width * 4 skews the image into a
        // diagonal. Every test here uses padding so the bug cannot pass.
        let stride = (w as usize) * 4 + 16;
        (vec![0u8; stride * (h as usize)], stride)
    }

    #[test]
    fn a_stride_narrower_than_the_width_is_refused() {
        let mut data = vec![0u8; 100];
        assert!(Surface::new(&mut data, 10, 2, 8).is_err());
    }

    #[test]
    fn a_buffer_shorter_than_its_geometry_is_refused() {
        // The check that stops an out-of-bounds write into memory the display
        // controller is about to scan out.
        let mut data = vec![0u8; 10];
        assert!(Surface::new(&mut data, 10, 10, 40).is_err());
    }

    #[test]
    fn fill_writes_only_inside_the_rectangle() {
        let (mut buf, stride) = surface(4, 4);
        let mut s = Surface::new(&mut buf, 4, 4, stride).unwrap();
        s.fill(Rect::new(1, 1, 2, 2), Rgba([1.0, 0.0, 0.0, 1.0]));
        // (0,0) untouched, (1,1) red. Byte order is BGRA on little-endian.
        assert_eq!(&buf[0..4], &[0, 0, 0, 0]);
        let o = stride + 4;
        assert_eq!(&buf[o..o + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn fill_clips_instead_of_writing_out_of_bounds() {
        // ★ The one that matters. A damage rect larger than the surface must
        // clip, not panic and not corrupt.
        let (mut buf, stride) = surface(2, 2);
        let mut s = Surface::new(&mut buf, 2, 2, stride).unwrap();
        s.fill(Rect::new(-5, -5, 100, 100), Rgba([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(&buf[0..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn a_rect_entirely_outside_writes_nothing() {
        let (mut buf, stride) = surface(2, 2);
        let before = buf.clone();
        let mut s = Surface::new(&mut buf, 2, 2, stride).unwrap();
        s.fill(Rect::new(50, 50, 10, 10), Rgba([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(buf, before);
    }

    #[test]
    fn half_alpha_over_black_is_half_the_colour() {
        let (mut buf, stride) = surface(1, 1);
        let mut s = Surface::new(&mut buf, 1, 1, stride).unwrap();
        // Premultiplied: a 50%-alpha white is (0.5,0.5,0.5,0.5).
        s.fill(Rect::new(0, 0, 1, 1), Rgba([0.5, 0.5, 0.5, 0.5]));
        assert_eq!(buf[0], 128);
        assert_eq!(buf[3], 128);
    }

    #[test]
    fn blit_copies_a_source_pixel_to_the_right_place() {
        let src_stride = 4 * 4 + 8;
        let mut src_buf = vec![0u8; src_stride * 4];
        // One white pixel at (2,3) in the source, BGRA.
        let o = 3 * src_stride + 2 * 4;
        src_buf[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
        let src = SurfaceRef::new(&src_buf, 4, 4, src_stride).unwrap();

        let (mut dst_buf, dstride) = surface(4, 4);
        let mut dst = Surface::new(&mut dst_buf, 4, 4, dstride).unwrap();
        dst.blit(
            &src,
            Rect::new(0, 0, 4, 4),
            Rect::new(0, 0, 4, 4),
            Transform::Normal,
            1.0,
            &[Rect::new(0, 0, 4, 4)],
            OpaqueHint::Unknown,
        );
        let d = 3 * dstride + 2 * 4;
        assert_eq!(&dst_buf[d..d + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn blit_respects_the_damage_list() {
        // Outside the damage rect nothing is written — the property the whole
        // damage-tracking design rests on.
        let src_stride = 4 * 4;
        let src_buf = vec![255u8; src_stride * 4];
        let src = SurfaceRef::new(&src_buf, 4, 4, src_stride).unwrap();
        let (mut dst_buf, dstride) = surface(4, 4);
        let mut dst = Surface::new(&mut dst_buf, 4, 4, dstride).unwrap();
        dst.blit(
            &src,
            Rect::new(0, 0, 4, 4),
            Rect::new(0, 0, 4, 4),
            Transform::Normal,
            1.0,
            &[Rect::new(0, 0, 1, 1)],
            OpaqueHint::Unknown,
        );
        assert_eq!(&dst_buf[0..4], &[255, 255, 255, 255]);
        assert_eq!(&dst_buf[4..8], &[0, 0, 0, 0], "outside damage stays untouched");
    }

    #[test]
    fn a_sample_outside_the_source_is_skipped_not_clamped() {
        let src_buf = vec![0u8; 16];
        let src = SurfaceRef::new(&src_buf, 2, 2, 8).unwrap();
        assert!(src.pixel(5, 0).is_none());
        assert!(src.pixel(-1, 0).is_none());
        assert!(src.pixel(1, 1).is_some());
    }

    #[test]
    fn every_transform_maps_corner_to_corner() {
        // ★ Each of the eight must land inside the source extent — an
        // off-by-one here paints a one-pixel band of garbage along an edge,
        // which is invisible in a screenshot and obvious on a screen.
        for t in [
            Transform::Normal,
            Transform::Rotate90,
            Transform::Rotate180,
            Transform::Rotate270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            for (u, v) in [(0, 0), (3, 0), (0, 3), (3, 3)] {
                let (su, sv) = map_back(t, u, v, 4, 4);
                assert!(
                    (0..4).contains(&su) && (0..4).contains(&sv),
                    "{t:?} mapped ({u},{v}) outside the source: ({su},{sv})"
                );
            }
        }
    }

    #[test]
    fn a_colour_above_one_saturates_rather_than_wrapping() {
        // `as u8` on 1.0001 * 255 wraps to 0 — black where white was meant.
        assert_eq!(to_u8(1.5), 255);
        assert_eq!(to_u8(-0.5), 0);
    }
}
