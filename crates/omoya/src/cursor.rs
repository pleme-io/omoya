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
pub const SCALE: i32 = 2;

/// Width in screen pixels.
#[must_use]
pub const fn width() -> i32 {
    10 * SCALE
}

/// Height in screen pixels.
#[must_use]
pub const fn height() -> i32 {
    17 * SCALE
}

/// Rasterize the arrow to premultiplied ARGB8888.
///
/// Transparent cells are written as fully-zero pixels, which in premultiplied
/// alpha is "contributes nothing" — so the blend leaves the desktop beneath
/// untouched rather than punching a black hole in it.
#[must_use]
pub fn rasterize() -> Vec<u8> {
    let fill = NORD.snow_storm[2];
    let line = NORD.polar_night[0];
    let (w, h) = (width() as usize, height() as usize);
    let mut buf = vec![0u8; w * h * 4];

    for (row, art) in ART.iter().enumerate() {
        for (col, ch) in art.chars().enumerate() {
            let colour = match ch {
                '#' => Some(fill),
                'X' => Some(line),
                _ => None,
            };
            let Some(c) = colour else { continue };
            for dy in 0..SCALE as usize {
                for dx in 0..SCALE as usize {
                    let x = col * SCALE as usize + dx;
                    let y = row * SCALE as usize + dy;
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
}
