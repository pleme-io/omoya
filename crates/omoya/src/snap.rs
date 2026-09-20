//! snap — Mac/Windows-style tiling of a FLOATING window, by gesture or chord.
//!
//! ── ★ WHAT THE OPERATOR ASKED FOR (2026-09-19) ───────────────────────────
//! "A floating snapping windows experience configured like mac and windows."
//! Floating mode already dragged, cascaded and edge-snapped; what it lacked is
//! the half of that experience people actually reach for:
//!
//! * drag a window to the LEFT/RIGHT screen edge → it takes that half,
//!   to the TOP edge → it maximises, into a CORNER → it takes that quarter;
//! * drag a snapped window away → it gets its free size back;
//! * Logo+Arrows step through the same tiles (Windows' Win+Arrow table).
//!
//! Everything here is PURE geometry plus one per-window cell, so the rules are
//! tested without a compositor. `layout.rs` places a snapped window, `grab.rs`
//! decides the tile on release, `deed.rs` maps the chord.
//!
//! ── ★ STATE LIVES ON THE WINDOW ─────────────────────────────────────────
//! The tile rides in the window's user data, the way `floatpos` keeps a
//! remembered position — no lookup, no lifetime to manage, gone when the
//! window is.
//!
//! ★ CORRECTED 2026-09-19: this note used to say `winid::of` "is a per-CLIENT
//! protocol id — every mado on the seat is 16", which is exactly backwards.
//! `surface.id().protocol_id()` was that, and `winid::of` is the module
//! written to REPLACE it: a monotonic counter stored on the surface, unique
//! across clients by construction (read `winid.rs`'s header for the eight
//! sites it fixed). A `HashMap<u32, Tile>` keyed by `winid::of` would be
//! sound — `windowmode` is exactly that and is correct. The note would have
//! sent the next reader away from the one primitive that solved this.

use kukaku::Direction;
use smithay::desktop::Window;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Point, Rectangle};
use std::cell::Cell;

/// Where a snapped window sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    /// Left half of the usable zone.
    Left,
    /// Right half.
    Right,
    /// Top-left quarter.
    TopLeft,
    /// Top-right quarter.
    TopRight,
    /// Bottom-left quarter.
    BottomLeft,
    /// Bottom-right quarter.
    BottomRight,
    /// The whole usable zone.
    Full,
}

/// How close to a screen edge the pointer must be, in logical px, for a drag
/// release to snap. The pointer cannot leave the screen, so "at the edge" is a
/// band, and 8 px is wide enough to hit with a flick and narrow enough that a
/// window dropped near — not at — the edge stays free.
pub const EDGE: i32 = 8;

/// How far along an edge from a corner still counts as that corner. Windows
/// and macOS both give corners a generous run; 96 px is 24 grid units on the
/// seat's 4 px grid.
pub const CORNER: i32 = 96;

/// The tile a drag released at `p` asks for, or `None` for a free drop.
///
/// `screen` is the WHOLE output, not the usable zone: the bar sits at the top,
/// and "drag to the top of the screen to maximise" means the top of the glass,
/// where the pointer actually stops.
///
/// Corners win over edges, and the bottom edge alone snaps nothing — that is
/// Windows' table, and it keeps "drop it low" from surprising anyone.
#[must_use]
pub fn zone_for(p: Point<i32, Logical>, screen: Rectangle<i32, Logical>) -> Option<Tile> {
    if screen.size.w <= 0 || screen.size.h <= 0 {
        return None;
    }
    let (l, t) = (screen.loc.x, screen.loc.y);
    let (r, b) = (l + screen.size.w - 1, t + screen.size.h - 1);
    let near_l = p.x <= l + EDGE;
    let near_r = p.x >= r - EDGE;
    let near_t = p.y <= t + EDGE;
    let near_b = p.y >= b - EDGE;
    let top_run = p.y <= t + CORNER;
    let bottom_run = p.y >= b - CORNER;
    let left_run = p.x <= l + CORNER;
    let right_run = p.x >= r - CORNER;
    if (near_l && top_run) || (near_t && left_run) {
        Some(Tile::TopLeft)
    } else if (near_r && top_run) || (near_t && right_run) {
        Some(Tile::TopRight)
    } else if (near_l && bottom_run) || (near_b && left_run) {
        Some(Tile::BottomLeft)
    } else if (near_r && bottom_run) || (near_b && right_run) {
        Some(Tile::BottomRight)
    } else if near_l {
        Some(Tile::Left)
    } else if near_r {
        Some(Tile::Right)
    } else if near_t {
        Some(Tile::Full)
    } else {
        None
    }
}

/// The FRAME (titlebar included) a tile occupies in `usable`.
///
/// Halves and quarters keep `layout::GAP` between each other and the zone
/// edge, so two snapped windows read as two windows. `Full` fills the zone
/// exactly, matching maximise. Odd pixels go to the right/bottom tile, so the
/// two halves always cover the zone with no seam.
#[must_use]
pub fn frame_for(tile: Tile, usable: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    if tile == Tile::Full {
        return usable;
    }
    let g = crate::layout::GAP;
    let (ux, uy, uw, uh) = (usable.loc.x, usable.loc.y, usable.size.w, usable.size.h);
    let lw = ((uw - 3 * g) / 2).max(1);
    let rw = (uw - 3 * g - lw).max(1);
    let th = ((uh - 3 * g) / 2).max(1);
    let bh = (uh - 3 * g - th).max(1);
    let (lx, rx) = (ux + g, ux + 2 * g + lw);
    let (ty, by) = (uy + g, uy + 2 * g + th);
    let full_h = (uh - 2 * g).max(1);
    let (x, y, w, h) = match tile {
        Tile::Left => (lx, ty, lw, full_h),
        Tile::Right => (rx, ty, rw, full_h),
        Tile::TopLeft => (lx, ty, lw, th),
        Tile::TopRight => (rx, ty, rw, th),
        Tile::BottomLeft => (lx, by, lw, bh),
        Tile::BottomRight => (rx, by, rw, bh),
        Tile::Full => unreachable!("handled above"),
    };
    Rectangle::new((x, y).into(), (w, h).into())
}

/// What a Logo+Arrow chord does from `current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Snap to this tile, or `None` to restore the free geometry.
    To(Option<Tile>),
    /// Minimise — Windows' Win+Down on a window that is already free.
    Minimize,
}

/// Windows' Win+Arrow table.
///
/// Left/Right move sideways (a half, or the quarter on the same row), and
/// pressing toward the opposite side of a half restores it. Up grows (half →
/// top quarter → full, free → full); Down shrinks (full → free, top quarter →
/// half → bottom quarter → free, free → minimise).
#[must_use]
pub fn step(current: Option<Tile>, dir: Direction) -> Step {
    use Tile::{BottomLeft, BottomRight, Full, Left, Right, TopLeft, TopRight};
    let to = |t| Step::To(Some(t));
    match (dir, current) {
        (Direction::Left, Some(Right)) => Step::To(None),
        (Direction::Left, Some(TopRight)) => to(TopLeft),
        (Direction::Left, Some(BottomRight)) => to(BottomLeft),
        (Direction::Left, Some(t @ (TopLeft | BottomLeft))) => to(t),
        (Direction::Left, _) => to(Left),

        (Direction::Right, Some(Left)) => Step::To(None),
        (Direction::Right, Some(TopLeft)) => to(TopRight),
        (Direction::Right, Some(BottomLeft)) => to(BottomRight),
        (Direction::Right, Some(t @ (TopRight | BottomRight))) => to(t),
        (Direction::Right, _) => to(Right),

        (Direction::Above, Some(Left)) => to(TopLeft),
        (Direction::Above, Some(Right)) => to(TopRight),
        (Direction::Above, Some(BottomLeft)) => to(Left),
        (Direction::Above, Some(BottomRight)) => to(Right),
        (Direction::Above, _) => to(Full),

        (Direction::Below, Some(Full)) => Step::To(None),
        (Direction::Below, Some(TopLeft)) => to(Left),
        (Direction::Below, Some(TopRight)) => to(Right),
        (Direction::Below, Some(Left)) => to(BottomLeft),
        (Direction::Below, Some(Right)) => to(BottomRight),
        (Direction::Below, Some(BottomLeft | BottomRight)) => Step::To(None),
        (Direction::Below, None) => Step::Minimize,
    }
}

#[derive(Debug, Default)]
struct Snapped(Cell<Option<Tile>>);

/// The tile `w` is snapped to, if any.
#[must_use]
pub fn tile_of(w: &Window) -> Option<Tile> {
    tile_in(w.user_data())
}

/// [`tile_of`] over a bare map, so the rule is testable without a client.
#[must_use]
pub fn tile_in(map: &UserDataMap) -> Option<Tile> {
    map.get::<Snapped>().and_then(|s| s.0.get())
}

/// Snap `w` to `tile`, or free it with `None`.
pub fn set(w: &Window, tile: Option<Tile>) {
    set_in(w.user_data(), tile);
}

/// [`set`] over a bare map.
pub fn set_in(map: &UserDataMap, tile: Option<Tile>) {
    map.insert_if_missing(Snapped::default);
    if let Some(s) = map.get::<Snapped>() {
        s.0.set(tile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    }
    fn usable() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 28).into(), (1920, 1052).into())
    }
    fn at(x: i32, y: i32) -> Point<i32, Logical> {
        (x, y).into()
    }

    #[test]
    fn edges_pick_halves_and_the_top_maximises() {
        assert_eq!(zone_for(at(0, 540), screen()), Some(Tile::Left));
        assert_eq!(zone_for(at(1919, 540), screen()), Some(Tile::Right));
        assert_eq!(zone_for(at(960, 0), screen()), Some(Tile::Full));
    }

    #[test]
    fn corners_beat_edges_along_their_whole_run() {
        assert_eq!(zone_for(at(0, 0), screen()), Some(Tile::TopLeft));
        assert_eq!(zone_for(at(0, 90), screen()), Some(Tile::TopLeft));
        assert_eq!(zone_for(at(90, 0), screen()), Some(Tile::TopLeft));
        assert_eq!(zone_for(at(1919, 0), screen()), Some(Tile::TopRight));
        assert_eq!(zone_for(at(0, 1079), screen()), Some(Tile::BottomLeft));
        assert_eq!(zone_for(at(1919, 1079), screen()), Some(Tile::BottomRight));
    }

    #[test]
    fn a_drop_short_of_the_edge_or_on_the_bottom_alone_stays_free() {
        assert_eq!(zone_for(at(EDGE + 1, 540), screen()), None);
        assert_eq!(zone_for(at(960, 540), screen()), None);
        assert_eq!(zone_for(at(960, 1079), screen()), None);
    }

    #[test]
    fn two_halves_cover_the_zone_with_one_gap_between_and_no_overlap() {
        let (l, r) = (
            frame_for(Tile::Left, usable()),
            frame_for(Tile::Right, usable()),
        );
        let g = crate::layout::GAP;
        assert_eq!(l.loc.x, g);
        assert_eq!(
            r.loc.x,
            l.loc.x + l.size.w + g,
            "one gap between the halves"
        );
        assert_eq!(
            r.loc.x + r.size.w,
            1920 - g,
            "the right half reaches the zone edge"
        );
        assert_eq!(l.loc.y, 28 + g);
        assert_eq!(l.size.h, 1052 - 2 * g);
    }

    #[test]
    fn four_quarters_tile_the_zone_exactly() {
        let q = [
            Tile::TopLeft,
            Tile::TopRight,
            Tile::BottomLeft,
            Tile::BottomRight,
        ]
        .map(|t| frame_for(t, usable()));
        let area: i32 = q.iter().map(|r| r.size.w * r.size.h).sum();
        let g = crate::layout::GAP;
        assert_eq!(area, (1920 - 3 * g) * (1052 - 3 * g));
        for (i, a) in q.iter().enumerate() {
            for b in &q[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn full_is_exactly_the_usable_zone_like_maximise() {
        assert_eq!(frame_for(Tile::Full, usable()), usable());
    }

    #[test]
    fn the_win_arrow_table() {
        use Direction::{Above, Below, Left, Right};
        assert_eq!(step(None, Left), Step::To(Some(Tile::Left)));
        assert_eq!(step(Some(Tile::Left), Right), Step::To(None));
        assert_eq!(step(Some(Tile::Left), Above), Step::To(Some(Tile::TopLeft)));
        assert_eq!(
            step(Some(Tile::TopLeft), Right),
            Step::To(Some(Tile::TopRight))
        );
        assert_eq!(
            step(Some(Tile::TopRight), Below),
            Step::To(Some(Tile::Right))
        );
        assert_eq!(
            step(Some(Tile::Right), Below),
            Step::To(Some(Tile::BottomRight))
        );
        assert_eq!(step(Some(Tile::BottomRight), Below), Step::To(None));
        assert_eq!(step(None, Above), Step::To(Some(Tile::Full)));
        assert_eq!(step(Some(Tile::Full), Below), Step::To(None));
        assert_eq!(step(None, Below), Step::Minimize);
    }

    #[test]
    fn the_tile_lives_on_the_window_not_on_an_id() {
        let (a, b) = (UserDataMap::new(), UserDataMap::new());
        assert_eq!(tile_in(&a), None);
        set_in(&a, Some(Tile::Left));
        assert_eq!(tile_in(&a), Some(Tile::Left));
        assert_eq!(
            tile_in(&b),
            None,
            "snapping one window must not snap another"
        );
        set_in(&a, None);
        assert_eq!(tile_in(&a), None);
    }
}
