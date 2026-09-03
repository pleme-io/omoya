//! Where a floating window ACTUALLY is — a per-window fact, not a formula.
//!
//! ── ★ THE DEFECT THIS CLOSES ─────────────────────────────────────────────
//! In floating mode `apply_layout` derived every window's position from
//! scratch on every pass:
//!
//! ```text
//! snap_to_edges(cascaded(usable, width, height, idx, cascade_step), ..)
//! ```
//!
//! Position was a pure function of the window's INDEX. `MoveGrab` wrote the
//! dragged location into the `Space`, and the next `apply_layout` — there are
//! twelve call sites — recomputed the cascade and put the window back. The
//! operator reported it precisely: *"I can move it but if I click on it or
//! both windows they snap back to their original position."*
//!
//! Two more symptoms fall out of the same line. `idx` comes from
//! `space.elements().enumerate()`, which is Z-ORDER — so raising or closing a
//! window renumbers the ones above it and physically relocates them. And
//! because every window gets the same `float_width`/`float_height`, the
//! cascade is the only thing separating them; on plo that is 24 px against
//! 883x523, a 93% overlap that reads as a stack.
//!
//! ── ★ WHY THIS LIVES ON THE WINDOW AND NOT IN A MAP ──────────────────────
//! The obvious shape is `HashMap<WindowId, Point>`. It would be WRONG today:
//! `surface_id_of` returns `protocol_id`, which wayland-backend documents as
//! per-client — *"each client has its own ID space, so this should not be
//! used as a unique identifier"* — and every mado on the seat measures as
//! `wl_surface#16`. Two mados would share one entry and fight over it.
//!
//! `Window::user_data()` is keyed by the window itself, so it needs no id and
//! cannot collide. That makes this fix independent of the WindowId work
//! rather than blocked behind it — which matters, because the titlebar hoist
//! without this reads as a NEW bug ("dragging works and then undoes itself")
//! rather than as a fix.
//!
//! ── ★ THE RULE ───────────────────────────────────────────────────────────
//! A floating window is placed ONCE, when it first appears. After that its
//! position is remembered and the layout respects it. The compositor decides
//! where a window *starts*; the operator decides where it *is*.

use smithay::desktop::Window;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Point, Rectangle};
use std::cell::Cell;

/// The remembered top-left of a floating window's FRAME (bar included).
///
/// `Cell` rather than `RefCell`: a `Point` is `Copy`, so there is nothing to
/// borrow and no borrow to panic on. `user_data` is single-threaded by
/// smithay's own contract.
#[derive(Debug, Default)]
struct FloatPos(Cell<Option<Point<i32, Logical>>>);

/// What this window remembers, if anything.
///
/// `None` means "never placed" — the caller should compute a first position
/// and record it with [`remember`].
#[must_use]
pub fn recall(w: &Window) -> Option<Point<i32, Logical>> {
    recall_in(w.user_data())
}

/// [`recall`], against the bare map.
///
/// ★ Split out so the round trip is TESTABLE. A `Window` needs a live client
/// and display to construct, so a test written against `&Window` cannot run
/// — and a red run proved that: neutering `recall` left all four tests green,
/// because every one of them exercised `clamped` and none of them touched the
/// storage this module exists for. A red run that PASSES means the test is
/// blind, and the fix is a test, not a shrug.
#[must_use]
pub fn recall_in(map: &UserDataMap) -> Option<Point<i32, Logical>> {
    map.insert_if_missing(FloatPos::default);
    map.get::<FloatPos>()?.0.get()
}

/// Record where this window is now.
///
/// Called from two places and they mean different things: the layout records
/// the FIRST position it computes, and the move grab records every position
/// the operator drags it to. Both are "this is where the window is", which is
/// why they are one function.
pub fn remember(w: &Window, loc: Point<i32, Logical>) {
    remember_in(w.user_data(), loc);
}

/// [`remember`], against the bare map. See [`recall_in`] for why.
pub fn remember_in(map: &UserDataMap, loc: Point<i32, Logical>) {
    map.insert_if_missing(FloatPos::default);
    if let Some(p) = map.get::<FloatPos>() {
        p.0.set(Some(loc));
    }
}

/// Forget a window's position, so the next layout pass places it afresh.
///
/// For the case where a remembered position stops making sense — a window
/// leaving floating mode, or an output that changed size under it.
pub fn forget(w: &Window) {
    if let Some(p) = w.user_data().get::<FloatPos>() {
        p.0.set(None);
    }
}

/// Keep a remembered position usable after the screen changes underneath it.
///
/// ★ A REMEMBERED POSITION IS NOT SACRED. If the output shrinks, or a bar
/// appears, a window parked at the old bottom-right is now off-screen and
/// unreachable — and unlike a badly-placed new window, the operator cannot
/// drag it back, because the titlebar is the thing that went off-screen.
///
/// So the remembered point is clamped into `usable` while KEEPING the window's
/// size. Clamping the position rather than shrinking the window is deliberate:
/// resizing a window because the screen moved would be the compositor
/// overruling a size the client chose.
#[must_use]
pub fn clamped(
    remembered: Point<i32, Logical>,
    size: smithay::utils::Size<i32, Logical>,
    usable: Rectangle<i32, Logical>,
) -> Point<i32, Logical> {
    // A window wider than the zone pins to the left edge rather than to a
    // negative x — `max` after `min` is what makes that fall out instead of
    // needing its own branch.
    let x = remembered
        .x
        .min(usable.loc.x + usable.size.w - size.w)
        .max(usable.loc.x);
    let y = remembered
        .y
        .min(usable.loc.y + usable.size.h - size.h)
        .max(usable.loc.y);
    Point::from((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((0, 28)), (1920, 1052).into())
    }

    /// ★ A WINDOW INSIDE THE ZONE IS NOT MOVED. The clamp must be a no-op in
    /// the ordinary case, or every layout pass would nudge windows around —
    /// which is the class of bug this whole module exists to remove.
    #[test]
    fn a_window_already_inside_the_zone_is_left_exactly_where_it_is() {
        let p = Point::from((518, 304));
        assert_eq!(clamped(p, (883, 523).into(), zone()), p);
    }

    /// ★ AN OFF-SCREEN WINDOW IS PULLED BACK, because its titlebar is the
    /// thing that went off-screen and the operator therefore cannot drag it
    /// back by hand.
    #[test]
    fn a_window_past_the_edge_is_pulled_back_inside() {
        // Parked beyond the right and bottom edges.
        let got = clamped(Point::from((1900, 1040)), (883, 523).into(), zone());
        assert_eq!(got, Point::from((1920 - 883, 28 + 1052 - 523)));
        // And it is genuinely inside.
        assert!(got.x + 883 <= 1920 && got.y + 523 <= 28 + 1052);
    }

    /// ★ ABOVE THE ZONE IS ALSO OUT. The bar occupies the top 28 px, so y=0
    /// puts the titlebar under the status bar where it cannot be clicked —
    /// the same "drawn but unreachable" class as the hoist bug.
    #[test]
    fn a_window_above_the_usable_zone_is_pushed_below_the_bar() {
        let got = clamped(Point::from((100, 0)), (400, 300).into(), zone());
        assert_eq!(got.y, 28);
        assert_eq!(got.x, 100, "x was inside and must not be disturbed");
    }

    /// ★ A WINDOW LARGER THAN THE ZONE PINS TO THE TOP-LEFT rather than
    /// landing at a negative coordinate. `min` alone would send it off the
    /// other side; the `max` is what makes this fall out with no branch.
    #[test]
    fn a_window_bigger_than_the_zone_pins_to_the_origin() {
        let got = clamped(Point::from((500, 500)), (3000, 2000).into(), zone());
        assert_eq!(got, Point::from((0, 28)));
    }

    /// ★★ THE ROUND TRIP — the invariant the whole module exists for, and the
    /// one my first four tests did not cover.
    ///
    /// Red-run receipt: neutering `recall` to always return `None` (the
    /// pre-fix behaviour, where position is re-derived every pass) left all
    /// four original tests GREEN, because every one of them tested `clamped`
    /// arithmetic. A red run that passes is a blind test.
    #[test]
    fn what_is_remembered_is_what_is_recalled() {
        let map = UserDataMap::new();
        assert_eq!(
            recall_in(&map),
            None,
            "a window that has never been placed must report so, or the layout \
             would never compute a first position for it"
        );
        remember_in(&map, Point::from((518, 304)));
        assert_eq!(recall_in(&map), Some(Point::from((518, 304))));
        // A drag moves it again; the newest position wins.
        remember_in(&map, Point::from((742, 511)));
        assert_eq!(
            recall_in(&map),
            Some(Point::from((742, 511))),
            "a second drag must overwrite the first, not be ignored"
        );
    }

    /// ★ TWO WINDOWS DO NOT SHARE A POSITION.
    ///
    /// This is why storage lives on the window rather than in a map keyed by
    /// `surface_id_of`: that returns a per-client `protocol_id`, and every
    /// mado on the seat measures as `wl_surface#16`. A keyed map would give
    /// two mados one entry and they would fight over it.
    #[test]
    fn each_window_remembers_its_own_position() {
        let a = UserDataMap::new();
        let b = UserDataMap::new();
        remember_in(&a, Point::from((100, 100)));
        remember_in(&b, Point::from((900, 600)));
        assert_eq!(recall_in(&a), Some(Point::from((100, 100))));
        assert_eq!(
            recall_in(&b),
            Some(Point::from((900, 600))),
            "the second window took the first's position — storage is shared"
        );
    }
}
