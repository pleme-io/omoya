//! The rolling wash — a bounded backstop for staleness we have not diagnosed.
//!
//! ── ★ WHAT THIS IS, AND WHAT IT IS NOT ──────────────────────────────────────
//!
//! Every other seal in this codebase tries to make a bad state impossible. This
//! one does not. It **bounds how long a bad state can survive**, which is a
//! strictly weaker promise, and it is here precisely because the residual
//! stale-pixel mechanism is still unidentified.
//!
//! Tier: **only-mitigated**, and the ceiling is stated rather than implied — a
//! wash cannot prevent a stale pixel, it can only guarantee one is repainted
//! within `slices` frames. If the cause is a repaint that never happens, the
//! wash repairs the symptom on a timer and the defect remains.
//!
//! ── ★ WHY A ROLLING SLICE AND NOT A PERIODIC FULL REPAINT ───────────────────
//!
//! This is intra-refresh, which video codecs adopted for the same reason: a
//! periodic keyframe is a bandwidth SPIKE, and a spike on a compositor is a
//! dropped frame the operator sees as a stutter. Refreshing 1/N of the screen
//! every frame costs a flat 1/N instead, with the same convergence guarantee.
//!
//! At the fleet default of 8 slices on plo's 360 Hz panel, any stale pixel is
//! repainted within 8 frames — 22 ms — for a flat 12.5% of extra paint.
//!
//! ── ★ THE INVARIANT THAT MAKES IT WORTH HAVING ──────────────────────────────
//!
//! **The slices must TILE the output**: cover every row exactly once, with no
//! gap and no overlap. A wash with a one-row gap looks like it works — the
//! screen refreshes, the counters move, most artifacts vanish — while the band
//! it never touches keeps its stale pixels forever. That failure is invisible
//! by construction, so it is the one thing tested exhaustively below.

use smithay::utils::{Physical, Rectangle};

/// A rolling refresh cursor over an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wash {
    /// How many frames one full sweep takes.
    slices: u8,
    /// Which slice the next frame refreshes.
    next: u8,
}

impl Wash {
    /// Build a wash of `slices` frames per sweep.
    ///
    /// Returns `None` for `slices == 0`, which is how the feature is disabled:
    /// an absent `Wash` rather than a `Wash` that yields empty rectangles. A
    /// disabled feature should have no object, not an object that does nothing
    /// — the second shape is how a disabled thing gets accidentally re-enabled
    /// by a caller that assumes a value means "on".
    #[must_use]
    pub const fn new(slices: u8) -> Option<Self> {
        if slices == 0 {
            None
        } else {
            Some(Self { slices, next: 0 })
        }
    }

    /// Frames per full sweep — the staleness bound, in frames.
    #[must_use]
    pub const fn bound_frames(&self) -> u8 {
        self.slices
    }

    /// Advance one frame and return the region to add to this frame's damage.
    ///
    /// Rows are distributed so the slices TILE `height`: each slice gets
    /// `height / slices`, and the remainder is spread one row at a time across
    /// the first `height % slices` slices. Handing the remainder to the last
    /// slice instead would also tile, but makes the final slice arbitrarily
    /// larger — a visible hitch once per sweep on an awkward resolution.
    pub fn advance(&mut self, width: i32, height: i32) -> Rectangle<i32, Physical> {
        let i = i32::from(self.next);
        let n = i32::from(self.slices);
        self.next = (self.next + 1) % self.slices;

        if height <= 0 || width <= 0 {
            return Rectangle::from_size((0, 0).into());
        }

        let base = height / n;
        let extra = height % n;
        // Every earlier slice contributed `base`, plus one row each for the
        // first `extra` of them.
        let y = i * base + i.min(extra);
        let h = base + i32::from(i < extra);
        Rectangle::new((0, y).into(), (width, h).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Disabled means ABSENT, not "present and inert".
    #[test]
    fn zero_slices_is_no_wash_at_all() {
        assert!(Wash::new(0).is_none());
        assert!(Wash::new(1).is_some());
    }

    /// ★ THE LOAD-BEARING TEST. A wash with a gap refreshes the screen, moves
    /// its counters, and never repairs one band — invisibly. So: for a spread
    /// of awkward heights, assert the sweep covers every row EXACTLY once.
    #[test]
    fn a_sweep_tiles_the_output_with_no_gap_and_no_overlap() {
        for slices in 1u8..=16 {
            for height in [1i32, 7, 8, 9, 100, 1045, 1080, 1081, 2160] {
                let mut w = Wash::new(slices).expect("non-zero");
                let mut covered = vec![0u32; usize::try_from(height).unwrap()];
                for _ in 0..slices {
                    let r = w.advance(1920, height);
                    for y in r.loc.y..r.loc.y + r.size.h {
                        covered[usize::try_from(y).unwrap()] += 1;
                    }
                }
                assert!(
                    covered.iter().all(|&c| c == 1),
                    "slices={slices} height={height}: rows covered {:?} times, not exactly once",
                    covered.iter().collect::<std::collections::BTreeSet<_>>()
                );
            }
        }
    }

    /// The sweep repeats, so the bound holds forever rather than for one pass.
    #[test]
    fn the_sweep_repeats_so_the_bound_is_permanent() {
        let mut w = Wash::new(4).expect("non-zero");
        let first: Vec<_> = (0..4).map(|_| w.advance(800, 600)).collect();
        let second: Vec<_> = (0..4).map(|_| w.advance(800, 600)).collect();
        assert_eq!(first, second);
    }

    /// The full width is always included — a wash that tiled rows but clipped
    /// columns would leave two stale margins with the same invisibility.
    #[test]
    fn every_slice_spans_the_full_width() {
        let mut w = Wash::new(5).expect("non-zero");
        for _ in 0..5 {
            let r = w.advance(1920, 1080);
            assert_eq!(r.loc.x, 0);
            assert_eq!(r.size.w, 1920);
        }
    }

    /// A degenerate output must not panic or produce a negative rectangle.
    #[test]
    fn a_zero_sized_output_yields_an_empty_rect() {
        let mut w = Wash::new(3).expect("non-zero");
        assert_eq!(w.advance(0, 0).size.h, 0);
        assert_eq!(w.advance(1920, 0).size.h, 0);
    }

    /// ANTI-VACUITY: the tiling test must actually be able to fail. A wash
    /// that always returned the whole screen would pass "no gap" but overlap
    /// 8x, so the test asserts EXACTLY once — proven here by construction.
    #[test]
    fn covering_every_row_more_than_once_would_fail_the_tiling_test() {
        let height = 64i32;
        let mut covered = vec![0u32; usize::try_from(height).unwrap()];
        for _ in 0..8 {
            for c in &mut covered {
                *c += 1; // a "wash" that repaints everything every frame
            }
        }
        assert!(
            !covered.iter().all(|&c| c == 1),
            "the tiling assertion must reject a full-screen repaint"
        );
    }
}
