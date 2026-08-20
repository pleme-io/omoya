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

        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let o = self.offset(x, y);
                if opaque {
                    self.data[o..o + 4].copy_from_slice(&src);
                } else {
                    blend_over(&mut self.data[o..o + 4], src, ca);
                }
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
    ) {
        for d in damage {
            let Some(clip) = d.intersect(dst_rect).and_then(|r| r.intersect(self.bounds())) else {
                continue;
            };
            for y in clip.y..clip.y + clip.h {
                for x in clip.x..clip.x + clip.w {
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
