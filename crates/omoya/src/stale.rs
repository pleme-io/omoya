//! Stale-pixel detection — what the display is SHOWING versus what the
//! compositor COMPOSED.
//!
//! ── ★ THE BUG CLASS THIS EXISTS TO SEE ───────────────────────────────────
//! Compositing happens in a RAM shadow and one damage-clipped copy moves the
//! result into the scanout mapping (`nuri_renderer::NuriFramebuffer`). The
//! shadow accumulates every frame and is therefore always complete. The
//! scanout SLOTS alternate, and each flush copies only *this* frame's damage
//! into *this* slot — so if the damage is under-reported by even one rect,
//! that slot keeps whatever it held two frames ago and the display shows it.
//!
//! The operator sees it as "lingering graphics that go away when I move the
//! mouse over them": the pointer's own damage repaints the region and
//! uncovers the truth. It is not a client bug and not a tearing bug, and
//! nothing in the seat could previously observe it.
//!
//! ── ★ WHY A SCREENSHOT COULD NOT SEE IT, MEASURED 2026-08-28 ─────────────
//! `drm::capture` reads the SHADOW via `copy_framebuffer`, and a capture
//! request additionally forces `age = 0`, i.e. a full repaint. So the
//! screenshot showed what the compositor BELIEVED, repaired the frame in the
//! act of asking, and came back byte-identical across a deliberate
//! stale-pixel hunt. An observer that repairs the defect it is measuring is
//! worse than no observer: it reports health.
//!
//! So this reads the scanout mapping directly. That mapping is documented as
//! *"written exactly once per frame and never read"* because a read from
//! write-combining memory costs ~1000x a RAM read — which is the right
//! default for the hot path and exactly the wrong one for a diagnostic. The
//! cost is paid once, on request.
//!
//! ── ★ WHY REGIONS ARE ATTRIBUTED AND NOT JUST COUNTED ───────────────────
//! "142,000 pixels differ" names no cause. The SHAPE does: a stale rect that
//! matches a window's geometry means that window moved without its old area
//! being damaged; one matching the cursor means the pointer is trailing; one
//! matching the bar means the strip's damage is wrong; scattered single rows
//! mean row-level damage. The compositor already knows every element's
//! rectangle, so the scan compares against them and reports the match. That
//! turns "the seat looks buggy" into a subsystem name.

/// One contiguous run of stale rows, merged into a rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleRegion {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Stale pixels inside this rectangle — never its area, because a region
    /// is a bounding box over rows that each have their own extent.
    pub pixels: u64,
    /// Which element this rectangle coincides with, if any. `None` is a
    /// finding rather than a gap: stale pixels belonging to no element are
    /// the *background* not being repainted.
    pub attribution: Option<String>,
}

/// The verdict, with its denominator.
///
/// ★ `compared` is carried so "clean" and "did not look" cannot render the
/// same. A scan that compared zero bytes reports zero stale pixels, and
/// without the denominator that is indistinguishable from a healthy seat —
/// the exact shape `truedamage::Verdict` exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleReport {
    pub stale_pixels: u64,
    pub compared_pixels: u64,
    pub regions: Vec<StaleRegion>,
}

impl StaleReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.compared_pixels > 0 && self.stale_pixels == 0
    }
}

/// An element the compositor drew, for attribution.
#[derive(Debug, Clone)]
pub struct NamedRect {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Compare the composed truth against what scanout actually holds.
///
/// Both buffers are ARGB8888, `w * h * 4` bytes. Rows are compared with `==`
/// on slices, which lowers to `memcmp` and bails at the first differing byte
/// — the same reason `truedamage` can afford a full-surface scan.
///
/// The alpha byte is ignored: the scanout plane has no alpha and the shadow's
/// is not meaningful after compositing, so comparing it would report every
/// pixel stale on a seat that is perfectly correct.
#[must_use]
pub fn scan(shadow: &[u8], scanout: &[u8], w: usize, h: usize) -> StaleReport {
    let stride = w * 4;
    let usable = stride.saturating_mul(h);
    if shadow.len() < usable || scanout.len() < usable || w == 0 || h == 0 {
        return StaleReport {
            stale_pixels: 0,
            compared_pixels: 0,
            regions: Vec::new(),
        };
    }

    // Per-row extent of the difference, so rows can be merged into rects.
    let mut runs: Vec<(usize, usize, usize, u64)> = Vec::new(); // (y, x0, x1, n)
    for y in 0..h {
        let o = y * stride;
        if shadow[o..o + stride] == scanout[o..o + stride] {
            continue;
        }
        let (mut lo, mut hi, mut n) = (usize::MAX, 0usize, 0u64);
        for x in 0..w {
            let p = o + x * 4;
            // BGR only — see the header on why alpha is skipped.
            if shadow[p] != scanout[p]
                || shadow[p + 1] != scanout[p + 1]
                || shadow[p + 2] != scanout[p + 2]
            {
                if lo == usize::MAX {
                    lo = x;
                }
                hi = x;
                n += 1;
            }
        }
        if n > 0 {
            runs.push((y, lo, hi, n));
        }
    }

    let stale_pixels: u64 = runs.iter().map(|r| r.3).sum();

    // Merge vertically-adjacent rows whose extents overlap. Deliberately not
    // full connected-component labelling: the shapes this bug produces are
    // rectangles (a window, the bar, a cursor), and a row-run merge names
    // them exactly while staying linear.
    let mut regions: Vec<StaleRegion> = Vec::new();
    for (y, x0, x1, n) in runs {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let (yi, x0i, x1i) = (y as i32, x0 as i32, x1 as i32);
        if let Some(last) = regions.last_mut() {
            let contiguous = last.y + last.h == yi;
            let overlaps = x0i <= last.x + last.w && x1i >= last.x;
            if contiguous && overlaps {
                let nx = last.x.min(x0i);
                let nr = (last.x + last.w).max(x1i + 1);
                last.x = nx;
                last.w = nr - nx;
                last.h += 1;
                last.pixels += n;
                continue;
            }
        }
        regions.push(StaleRegion {
            x: x0i,
            y: yi,
            w: x1i - x0i + 1,
            h: 1,
            pixels: n,
            attribution: None,
        });
    }

    StaleReport {
        stale_pixels,
        compared_pixels: (w * h) as u64,
        regions,
    }
}

/// Name each stale region after the element it coincides with.
///
/// ★ "contains" and not "equals". A stale region is usually a SUBSET of the
/// element that failed to repaint — the part of it the damage missed — so
/// demanding an exact rectangle match would attribute almost nothing and
/// report every real finding as unattributed background.
pub fn attribute(report: &mut StaleReport, elements: &[NamedRect]) {
    for r in &mut report.regions {
        let (rl, rt, rr, rb) = (r.x, r.y, r.x + r.w, r.y + r.h);
        // The smallest element that contains the region wins: a window inside
        // the full-screen background should read as the window.
        let mut best: Option<(&NamedRect, i64)> = None;
        for e in elements {
            let (el, et, er, eb) = (e.x, e.y, e.x + e.w, e.y + e.h);
            if rl >= el && rt >= et && rr <= er && rb <= eb {
                let area = i64::from(e.w) * i64::from(e.h);
                if best.is_none_or(|(_, a)| area < a) {
                    best = Some((e, area));
                }
            }
        }
        r.attribution = best.map(|(e, _)| e.name.clone());
    }
}

/// Render the scan as an image a human can read at a glance.
///
/// ── ★ WHY THIS SHAPE AND NOT A BARE MASK ────────────────────────────────
/// A 1-bit mask says WHERE without saying *what*, and the whole diagnostic
/// value is recognising the shape — "that is the terminal's old rectangle",
/// "that is the bar". So the base is the real screen content, dimmed, with
/// stale pixels painted in the seat's own error colour at full strength. The
/// operator sees their desktop with the broken parts glowing, and the cause
/// is usually obvious from the outline alone.
///
/// Dimming rather than greyscale: the hue is what makes a region
/// recognisable, and a red overlay on a dimmed COLOUR image stays readable
/// while red on grey does not.
#[must_use]
pub fn render_mask(shadow: &[u8], scanout: &[u8], w: usize, h: usize) -> Vec<u8> {
    // nord11 — the palette's `error`. See omoya docs/SHITSURAI.md §2.5: Aurora
    // appears only when the system has something to say, and a stale frame is
    // exactly that.
    const ERR: [u8; 3] = [0xBF, 0x61, 0x6A];
    let stride = w * 4;
    let mut out = Vec::with_capacity(20 + w * h * 3);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    for y in 0..h {
        for x in 0..w {
            let p = y * stride + x * 4;
            if p + 3 >= scanout.len() || p + 3 >= shadow.len() {
                out.extend_from_slice(&[0, 0, 0]);
                continue;
            }
            let differs = shadow[p] != scanout[p]
                || shadow[p + 1] != scanout[p + 1]
                || shadow[p + 2] != scanout[p + 2];
            if differs {
                out.extend_from_slice(&ERR);
            } else {
                // ARGB8888 little-endian is B,G,R in memory; PPM wants R,G,B.
                // Getting this backwards yields a plausible image with red and
                // blue swapped, which on Nord reads as "the theme is broken".
                out.push(scanout[p + 2] / 4);
                out.push(scanout[p + 1] / 4);
                out.push(scanout[p] / 4);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 4]
    }

    #[test]
    fn identical_buffers_are_clean_and_say_what_they_compared() {
        let (w, h) = (16, 8);
        let r = scan(&buf(w, h, 7), &buf(w, h, 7), w, h);
        assert_eq!(r.stale_pixels, 0);
        assert_eq!(r.compared_pixels, 128, "the denominator must be reported");
        assert!(r.is_clean());
    }

    /// The anti-vacuity case: a scan that looked at nothing must NOT read as
    /// a healthy seat.
    #[test]
    fn a_scan_that_compared_nothing_is_not_clean() {
        let r = scan(&[], &[], 16, 8);
        assert_eq!(r.stale_pixels, 0);
        assert_eq!(r.compared_pixels, 0);
        assert!(!r.is_clean(), "zero stale over zero compared is not health");
    }

    #[test]
    fn a_stale_rectangle_is_found_and_merged_into_one_region() {
        let (w, h) = (32, 16);
        let mut sc = buf(w, h, 0);
        let sh = buf(w, h, 0);
        // A 6x4 block at (10,5) differs.
        for y in 5..9 {
            for x in 10..16 {
                sc[(y * w + x) * 4 + 1] = 0xFF;
            }
        }
        let r = scan(&sh, &sc, w, h);
        assert_eq!(r.stale_pixels, 24);
        assert_eq!(r.regions.len(), 1, "one block must merge to one region");
        let g = &r.regions[0];
        assert_eq!((g.x, g.y, g.w, g.h), (10, 5, 6, 4));
    }

    #[test]
    fn alpha_alone_is_not_staleness() {
        let (w, h) = (8, 4);
        let sh = buf(w, h, 0);
        let mut sc = buf(w, h, 0);
        for p in sc.chunks_exact_mut(4) {
            p[3] = 0xFF; // alpha only
        }
        assert_eq!(
            scan(&sh, &sc, w, h).stale_pixels,
            0,
            "the scanout plane has no meaningful alpha; comparing it would \
             report a correct seat as entirely stale"
        );
    }

    #[test]
    fn a_region_is_attributed_to_the_smallest_element_containing_it() {
        let mut r = StaleReport {
            stale_pixels: 4,
            compared_pixels: 100,
            regions: vec![StaleRegion { x: 12, y: 12, w: 4, h: 4, pixels: 4, attribution: None }],
        };
        attribute(
            &mut r,
            &[
                NamedRect { name: "background".into(), x: 0, y: 0, w: 100, h: 100 },
                NamedRect { name: "window[0]".into(), x: 10, y: 10, w: 20, h: 20 },
            ],
        );
        assert_eq!(r.regions[0].attribution.as_deref(), Some("window[0]"));
    }

    #[test]
    fn an_unattributed_region_is_reported_rather_than_dropped() {
        let mut r = StaleReport {
            stale_pixels: 1,
            compared_pixels: 100,
            regions: vec![StaleRegion { x: 90, y: 90, w: 2, h: 2, pixels: 1, attribution: None }],
        };
        attribute(&mut r, &[NamedRect { name: "w".into(), x: 0, y: 0, w: 10, h: 10 }]);
        assert!(r.regions[0].attribution.is_none(), "background staleness is a finding");
        assert_eq!(r.regions.len(), 1);
    }

    #[test]
    fn the_mask_marks_stale_pixels_and_dims_the_rest() {
        let (w, h) = (2, 1);
        let sh = vec![0x40, 0x40, 0x40, 0xFF, 0x40, 0x40, 0x40, 0xFF];
        let mut sc = sh.clone();
        sc[4] = 0x00; // second pixel differs
        let png = render_mask(&sh, &sc, w, h);
        let body = &png[png.len() - 6..];
        assert_eq!(&body[0..3], &[0x10, 0x10, 0x10], "clean pixel is dimmed 4x");
        assert_eq!(&body[3..6], &[0xBF, 0x61, 0x6A], "stale pixel is nord11");
    }
}
