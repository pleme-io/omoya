//! The pointer — an arrow, rasterized once, drawn as a memory element.
//!
//! ── ★ WHY NOT A SOLID RECTANGLE ─────────────────────────────────────────
//! It was one: a 12x12 `draw_solid` square. That is not a small cursor, it is
//! a different object. The operator reported it twice — first as "a white
//! square in the top left hand corner" and later as "the mouse isn't on the
//! screen" — and both readings are correct. A square carries none of the
//! shape cues a pointer has: no tip to aim with, no orientation, nothing that
//! says "this is me" rather than "this is a rendering artifact".
//!
//! ── ★ AND WHY AN OUTLINE ────────────────────────────────────────────────
//! A single-colour cursor is invisible against a surface of that colour. The
//! arrow is `snow_storm[2]` filled with a `polar_night[0]` border, so it
//! reads on a dark background AND on a light one — a terminal's white text,
//! a bright image, a light-themed client. Every real cursor theme does this,
//! and the reason only becomes obvious the first time one disappears.

use irodori::NORD;

/// The arrow, as a mask. `#` fills, `X` outlines, `.` is transparent.
///
/// Written as art rather than coordinates because that is the form a human
/// can check at a glance — a table of run-lengths encodes the same shape and
/// hides a typo perfectly.
const ART: &[&str] = &[
    "X.........",
    "XX........",
    "X#X.......",
    "X##X......",
    "X###X.....",
    "X####X....",
    "X#####X...",
    "X######X..",
    "X#######X.",
    "X########X",
    "X#####XXXX",
    "X##X##X...",
    "X#X.X##X..",
    "XX..X##X..",
    "X....X##X.",
    "......X##X",
    ".......XXX",
];

/// How many screen pixels per mask cell.
///
/// ★ 2, NOT 1. The mask is 10x17; at 1:1 that is a cursor smaller than a
/// character cell on a 1920x1080 panel, which is how the previous one managed
/// to be on screen and unfindable at the same time. Doubling puts it at
/// 20x34 — the size a pointer is on every other desktop.
pub const SCALE: i32 = crate::ukeire::DEFAULT_CURSOR_SCALE;

/// Mask cells across, DERIVED from the art.
///
/// ★ NOT THE LITERAL `10`. It was, alongside a `17` for the rows, which made
/// "the mask is 10x17" a fact written twice — once as art a human checks at a
/// glance, once as numbers nobody would think to update. A row added to `ART`
/// left `height()` short, and a cursor whose declared height is less than its
/// pixels does not fail loudly: the plane clips its bottom rows, which reads
/// as a rendering bug. Censused 2026-09-03 while typing the intake
/// vocabulary; the art is now the only source.
///
/// The mask is ASCII by construction (`#`, `X`, `.`), so byte length is cell
/// count — asserted in tests rather than assumed.
const CELLS_W: i32 = ART[0].len() as i32;

/// Mask cells down, derived from the art.
const CELLS_H: i32 = ART.len() as i32;

/// Width in screen pixels at the default scale.
#[must_use]
pub const fn width() -> i32 {
    width_at(SCALE)
}

/// Height in screen pixels at the default scale.
#[must_use]
pub const fn height() -> i32 {
    height_at(SCALE)
}

/// Width in screen pixels at `scale`.
#[must_use]
pub const fn width_at(scale: i32) -> i32 {
    CELLS_W * scale
}

/// Height in screen pixels at `scale`.
#[must_use]
pub const fn height_at(scale: i32) -> i32 {
    CELLS_H * scale
}

/// Rasterize the arrow at the default scale.
///
/// Kept as the zero-argument name existing callers use; `rasterize_at` takes
/// the operator's `ukeire.pointer.cursor_scale`.
#[must_use]
pub fn rasterize() -> Vec<u8> {
    rasterize_at(SCALE)
}

/// Rasterize the arrow to premultiplied ARGB8888.
///
/// Transparent cells are written as fully-zero pixels, which in premultiplied
/// alpha is "contributes nothing" — so the blend leaves the desktop beneath
/// untouched rather than punching a black hole in it.
#[must_use]
pub fn rasterize_at(scale: i32) -> Vec<u8> {
    let fill = NORD.snow_storm[2];
    let line = NORD.polar_night[0];
    let (w, h) = (width_at(scale) as usize, height_at(scale) as usize);
    let mut buf = vec![0u8; w * h * 4];

    for (row, art) in ART.iter().enumerate() {
        for (col, ch) in art.chars().enumerate() {
            let colour = match ch {
                '#' => Some(fill),
                'X' => Some(line),
                _ => None,
            };
            let Some(c) = colour else { continue };
            for dy in 0..scale as usize {
                for dx in 0..scale as usize {
                    let x = col * scale as usize + dx;
                    let y = row * scale as usize + dy;
                    if x >= w || y >= h {
                        continue;
                    }
                    let o = (y * w + x) * 4;
                    // ARGB8888 little-endian is B,G,R,A in memory order.
                    buf[o] = c.b;
                    buf[o + 1] = c.g;
                    buf[o + 2] = c.r;
                    buf[o + 3] = 0xff;
                }
            }
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_is_rectangular() {
        // A ragged mask silently shifts every row below it, which looks like
        // a badly-drawn cursor rather than a malformed constant.
        let w = ART[0].len();
        for (i, row) in ART.iter().enumerate() {
            assert_eq!(row.len(), w, "row {i} is {} wide, expected {w}", row.len());
        }
    }

    #[test]
    fn the_tip_is_opaque_and_the_far_corner_is_not() {
        let buf = rasterize();
        let w = width() as usize;
        // (0,0) is the arrow's tip — it must be drawn, or the point the
        // operator aims with is the one pixel that is missing.
        assert_eq!(buf[3], 0xff, "the tip must be opaque");
        // Top-right is outside the arrow and must stay transparent, or the
        // cursor is a rectangle again.
        let far = ((w - 1) * 4) + 3;
        assert_eq!(buf[far], 0x00, "the top-right corner must be transparent");
    }

    #[test]
    fn it_has_both_a_fill_and_an_outline() {
        // A cursor drawn in one colour vanishes against that colour. Both
        // must actually appear in the raster, not merely be named.
        let buf = rasterize();
        let fill = NORD.snow_storm[2];
        let line = NORD.polar_night[0];
        let has = |c: irodori::Color| {
            buf.chunks_exact(4)
                .any(|px| px[3] == 0xff && px[0] == c.b && px[1] == c.g && px[2] == c.r)
        };
        assert!(has(fill), "no fill pixels");
        assert!(has(line), "no outline pixels");
    }

    // ── ukeire: the dimensions are DERIVED, and the scale is a knob ──────

    #[test]
    fn the_dimensions_come_from_the_art_and_not_from_a_literal() {
        // ★ THE DRIFT GATE, and it fires the way the defect actually
        // happens: `CELLS_W`/`CELLS_H` were the literals `10` and `17`, and
        // the failure mode is someone editing the art. Red-run by adding a
        // row while the literals stayed: `left: 17, right: 18`.
        assert_eq!(CELLS_H, ART.len() as i32);
        assert_eq!(CELLS_W, ART[0].len() as i32);
        for (row, art) in ART.iter().enumerate() {
            assert_eq!(
                art.len() as i32,
                CELLS_W,
                "art row {row} is a different width — the mask is not rectangular"
            );
            assert!(
                art.is_ascii(),
                "art row {row} is not ASCII, so byte length is not cell count"
            );
            assert!(
                art.chars().all(|c| matches!(c, '#' | 'X' | '.')),
                "art row {row} has a character the rasterizer does not know"
            );
        }
    }

    #[test]
    fn the_default_scale_pair_agrees_with_the_parameterized_one() {
        // The const face and the knob face must not drift apart. What keeps
        // `width()`/`height()` the documented default rather than a second
        // opinion.
        assert_eq!(width(), width_at(SCALE));
        assert_eq!(height(), height_at(SCALE));
    }

    #[test]
    fn a_larger_scale_produces_a_proportionally_larger_buffer() {
        // ★ ANTI-VACUITY for the whole cursor_scale knob, and not
        // hypothetical: the first draft of `rasterize_at` ignored its
        // parameter because the loop still read the `SCALE` const, so every
        // buffer was the same size and the config knob was decorative.
        for scale in [1, 2, 4] {
            let expected = (width_at(scale) * height_at(scale) * 4) as usize;
            assert_eq!(
                rasterize_at(scale).len(),
                expected,
                "scale {scale} did not reach the rasterizer"
            );
        }
        assert!(rasterize_at(4).len() > rasterize_at(2).len());
    }
}
