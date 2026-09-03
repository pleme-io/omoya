//! Interactive move and resize — the grabs that make a floating seat usable.
//!
//! ── ★ WHY THIS FILE EXISTS ──────────────────────────────────────────────────
//!
//! `move_request`/`resize_request` were empty stubs, documented as "M2:
//! accepted and ignored" on the reasoning that grabs "prove nothing about
//! whether omoya composites". That reasoning is sound for a COMPOSITING
//! milestone and wrong for a seat an operator uses: on plo the result was a
//! desktop where windows could not be moved at all.
//!
//! The operator's report was: *"the windows have no borders for me to drag
//! around and such"* / *"or snap"*. Both halves are this file.
//!
//! ── ★ THE SYMPTOM HAD TWO INDEPENDENT CAUSES ────────────────────────────────
//!
//! Worth stating because fixing one alone would have looked like a failed fix:
//!
//!   1. these stubs — a client asking to be dragged was answered with silence;
//!   2. `layout.rs` unmaps EVERY window from the tiling tree in floating mode,
//!      so keyboard `Deed::Resize` was a silent no-op too (now reported by
//!      `DeedOutcome::Refused`).
//!
//! ── ★ WHY A GRAB AND NOT SERVER-SIDE TITLEBARS ──────────────────────────────
//!
//! Drawing a draggable titlebar would ALSO need this: a client-side titlebar
//! drag routes through `xdg_toplevel.move` → `move_request` exactly like a
//! server-side one. The grab is the load-bearing half either way, so it is what
//! ships first; the chrome is a rendering question that can follow.

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point},
};

use crate::state::Omoya;

/// Snap a coordinate to `edge` when it lands within `threshold`.
///
/// ★ PURE, and separated from the grab so it is testable without a pointer, a
/// seat or a compositor. The snap rule is the part with an off-by-one worth
/// checking; the grab plumbing is not.
///
/// `threshold == 0` disables snapping, which is why the config can express
/// "off" without a second field: a distance of zero is exactly "only when
/// already flush", a no-op.
#[must_use]
pub fn snap_to(value: i32, edge: i32, threshold: i32) -> i32 {
    if threshold > 0 && (value - edge).abs() <= threshold {
        edge
    } else {
        value
    }
}

/// Snap a window rect's edges to the usable zone.
///
/// ★ Snaps LEADING edges to leading edges and TRAILING to trailing — left to
/// left, right to right. Snapping a left edge to the zone's right edge would
/// fling the window off-screen, and is the shape of bug that only appears on a
/// second monitor.
#[must_use]
pub fn snap_rect(
    (x, y): (i32, i32),
    (w, h): (i32, i32),
    zone: (i32, i32, i32, i32),
    threshold: i32,
) -> (i32, i32) {
    let (zx, zy, zw, zh) = zone;
    let mut nx = snap_to(x, zx, threshold);
    let mut ny = snap_to(y, zy, threshold);
    // Trailing edges: snap the RIGHT edge to the zone's right, then convert
    // back to an origin.
    if threshold > 0 && ((x + w) - (zx + zw)).abs() <= threshold {
        nx = zx + zw - w;
    }
    if threshold > 0 && ((y + h) - (zy + zh)).abs() <= threshold {
        ny = zy + zh - h;
    }
    (nx, ny)
}

/// A window being dragged by the pointer.
pub struct MoveGrab {
    pub start_data: GrabStartData<Omoya>,
    pub window: Window,
    /// Window origin minus pointer position at grab start. Held as a delta so
    /// the window does not jump to centre itself under the cursor.
    pub offset: Point<i32, Logical>,
}

impl PointerGrab<Omoya> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut Omoya,
        handle: &mut PointerInnerHandle<'_, Omoya>,
        _focus: Option<(
            <Omoya as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &MotionEvent,
    ) {
        // ★ FOCUS IS FORCED TO None while a grab is active. Letting the pointer
        // re-focus mid-drag hands motion to whatever is underneath, and the
        // window stops following the cursor halfway across the screen.
        handle.motion(data, None, event);

        let p: Point<i32, Logical> = (event.location.x as i32, event.location.y as i32).into();
        let new_loc = p + self.offset;
        data.space.map_element(self.window.clone(), new_loc, true);
        // ★ RECORD IT. Writing only into the Space is what made dragging look
        // broken: the next `apply_layout` re-derived the position from the
        // window's index and put it straight back. The Space is the layout's
        // OUTPUT; this is where the operator's intent is kept.
        crate::floatpos::remember(&self.window, new_loc);
        data.introspect.mark(crate::owed::Owed::Windows);
    }

    fn relative_motion(
        &mut self,
        data: &mut Omoya,
        handle: &mut PointerInnerHandle<'_, Omoya>,
        focus: Option<(
            <Omoya as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Omoya,
        handle: &mut PointerInnerHandle<'_, Omoya>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        // Any button release ends the drag. `handle.current_pressed()` is the
        // set still held; empty means the operator let go.
        if handle.current_pressed().is_empty() {
            // ★ SNAP ON RELEASE, not during motion. Snapping continuously makes
            // the window fight the pointer near an edge — it jumps flush, the
            // operator pulls away, it jumps back. On release it reads as
            // alignment, which is what `snap_threshold`'s own doc-comment says
            // it is for.
            if let Some(geo) = data.space.element_geometry(&self.window) {
                let zone = data
                    .space
                    .outputs()
                    .next()
                    .and_then(|o| data.space.output_geometry(o))
                    .map_or((0, 0, 0, 0), |g| (g.loc.x, g.loc.y, g.size.w, g.size.h));
                let threshold = data.config.layout.snap_threshold;
                let (nx, ny) = snap_rect(
                    (geo.loc.x, geo.loc.y),
                    (geo.size.w, geo.size.h),
                    zone,
                    threshold,
                );
                if (nx, ny) != (geo.loc.x, geo.loc.y) {
                    data.space.map_element(self.window.clone(), (nx, ny), true);
                }
            }
            data.introspect.mark(crate::owed::Owed::Windows);
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Omoya,
        handle: &mut PointerInnerHandle<'_, Omoya>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }
    fn frame(&mut self, data: &mut Omoya, handle: &mut PointerInnerHandle<'_, Omoya>) {
        handle.frame(data);
    }
    fn gesture_swipe_begin(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GestureSwipeBeginEvent,
    ) {
        h.gesture_swipe_begin(d, e);
    }
    fn gesture_swipe_update(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GestureSwipeUpdateEvent,
    ) {
        h.gesture_swipe_update(d, e);
    }
    fn gesture_swipe_end(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GestureSwipeEndEvent,
    ) {
        h.gesture_swipe_end(d, e);
    }
    fn gesture_pinch_begin(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GesturePinchBeginEvent,
    ) {
        h.gesture_pinch_begin(d, e);
    }
    fn gesture_pinch_update(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GesturePinchUpdateEvent,
    ) {
        h.gesture_pinch_update(d, e);
    }
    fn gesture_pinch_end(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GesturePinchEndEvent,
    ) {
        h.gesture_pinch_end(d, e);
    }
    fn gesture_hold_begin(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GestureHoldBeginEvent,
    ) {
        h.gesture_hold_begin(d, e);
    }
    fn gesture_hold_end(
        &mut self,
        d: &mut Omoya,
        h: &mut PointerInnerHandle<'_, Omoya>,
        e: &GestureHoldEndEvent,
    ) {
        h.gesture_hold_end(d, e);
    }

    fn start_data(&self) -> &GrabStartData<Omoya> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Omoya) {}
}

#[cfg(test)]
mod snap_tests {
    use super::{snap_rect, snap_to};

    #[test]
    fn within_the_threshold_snaps_flush() {
        assert_eq!(
            snap_to(14, 0, 16),
            0,
            "14px from the edge is a deliberate nudge"
        );
        assert_eq!(snap_to(-9, 0, 16), 0, "snapping works from outside too");
    }

    #[test]
    fn beyond_the_threshold_is_left_alone() {
        // A window parked 20px away stays there -- the doc-comment's own
        // example, and the difference between alignment and the compositor
        // arguing with the operator.
        assert_eq!(snap_to(20, 0, 16), 20);
    }

    /// ★ `threshold == 0` is how the config expresses "off" WITHOUT a second
    /// field. A distance of zero means "only when already flush", a no-op.
    #[test]
    fn a_zero_threshold_disables_snapping() {
        assert_eq!(snap_to(0, 0, 0), 0, "already flush stays flush");
        assert_eq!(snap_to(3, 0, 0), 3, "nothing is pulled");
    }

    /// ★ TRAILING EDGES SNAP TO TRAILING EDGES. Snapping a left edge to the
    /// zone's RIGHT edge would fling the window off-screen -- the shape of bug
    /// that only shows up on a second monitor.
    #[test]
    fn the_right_edge_snaps_to_the_right_edge() {
        // zone 0,0 1920x1080; window 800x600 whose right edge is 10px short.
        let (x, y) = snap_rect((1110, 500), (800, 600), (0, 0, 1920, 1080), 16);
        assert_eq!(x, 1120, "right edge flush => origin 1920-800");
        assert_eq!(y, 500, "y was nowhere near an edge and must not move");
    }

    #[test]
    fn a_window_in_open_space_is_not_moved() {
        // Anti-vacuity: a snap that always fired would pass every test above.
        let start = (700, 400);
        assert_eq!(snap_rect(start, (300, 200), (0, 0, 1920, 1080), 16), start);
    }
}
