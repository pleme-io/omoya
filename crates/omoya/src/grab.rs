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

/// The `PointerGrab` methods every omoya grab forwards unchanged.
///
/// Written once because two grabs (move, resize) need the identical dozen
/// pass-throughs, and a hand-copied second set is free to drift — the day a
/// gesture is handled in one and forgotten in the other. Each grab keeps only
/// what makes it a grab: `motion` and `button`. Requires a `start_data` field.
macro_rules! pointer_grab_passthrough {
    () => {
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
    };
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
        // ── ★ DRAGGING A SNAPPED WINDOW FREES IT (Windows/macOS) ─────────
        // It returns to its free size at once, and the grab offset is scaled
        // so the pointer keeps its RELATIVE spot on the titlebar — grabbing a
        // half-screen window near its right edge must not leave the pointer
        // hanging off the right of a window that just got narrower.
        if crate::snap::tile_of(&self.window).is_some() {
            let before = data.space.element_geometry(&self.window);
            crate::snap::set(&self.window, None);
            data.apply_layout();
            if let (Some(old), Some(new)) = (before, data.space.element_geometry(&self.window)) {
                if old.size.w > 0 {
                    let frac = f64::from(p.x - old.loc.x) / f64::from(old.size.w);
                    self.offset.x = -(f64::from(new.size.w) * frac.clamp(0.0, 1.0)) as i32;
                }
            }
        }
        // ★ CLAMPED, SO THE TITLEBAR CANNOT LEAVE THE SCREEN. `p + offset` is
        // unbounded, and the titlebar is the ONE part of a window that must
        // stay reachable — drag it past the top or the side and the thing you
        // grab it by is the thing that has gone. Reuses `floatpos::clamped`
        // rather than a second rule: the layout already clamps a remembered
        // position on every pass, and two implementations of "keep it on
        // screen" with different reference frames is how a window ends up
        // snapping to one edge on drag and a different one on the next layout.
        let new_loc = match data
            .space
            .element_geometry(&self.window)
            .zip(usable_zone(data))
        {
            Some((geo, usable)) => {
                // The BAR must stay inside, not the content — so the zone the
                // frame is clamped into is grown upward by the bar's height,
                // exactly as the frame is.
                let frame = smithay::utils::Rectangle::new(
                    (geo.loc.x, geo.loc.y - crate::chrome::HEIGHT).into(),
                    (geo.size.w, geo.size.h + crate::chrome::HEIGHT).into(),
                );
                let want = p + self.offset;
                let clamped = crate::floatpos::clamped(
                    (want.x, want.y - crate::chrome::HEIGHT).into(),
                    frame.size,
                    usable,
                );
                Point::from((clamped.x, clamped.y + crate::chrome::HEIGHT))
            }
            None => p + self.offset,
        };
        data.space.map_element(self.window.clone(), new_loc, true);
        // ★ RECORD IT. Writing only into the Space is what made dragging look
        // broken: the next `apply_layout` re-derived the position from the
        // window's index and put it straight back. The Space is the layout's
        // OUTPUT; this is where the operator's intent is kept.
        //
        // ★ THE FRAME, NOT THE CONTENT. `floatpos` is read by the layout as
        // the FRAME origin (the titlebar's top-left); remembering the content
        // origin made every later layout pass push a dragged window down by
        // `chrome::HEIGHT` (2026-09-19).
        crate::floatpos::remember(
            &self.window,
            (new_loc.x, new_loc.y - crate::chrome::HEIGHT).into(),
        );
        // ── ★ SHOW WHERE A RELEASE WOULD LAND ────────────────────────────
        // The same `zone_for` the release uses, so the preview can never
        // promise a tile the drop will not give. Cleared on release.
        data.snap_preview = data
            .space
            .outputs()
            .next()
            .and_then(|o| data.space.output_geometry(o))
            .and_then(|screen| crate::snap::zone_for(p, screen))
            .zip(usable_zone(data))
            .map(|(tile, usable)| crate::snap::frame_for(tile, usable));
        data.introspect.mark(crate::owed::Owed::Windows);
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
            // ── ★ AN EDGE OR CORNER RELEASE SNAPS TO A TILE ──────────────
            // Decided from where the POINTER is, not the window: the pointer
            // is what the operator pushed into the edge, and the window is
            // clamped inside the zone so it can never reach it.
            data.snap_preview = None;
            let pointer = handle.current_location();
            let snappable = crate::role::policy_of(&self.window, &data.config.placement).snappable;
            // An overlay is never snapped — it is the seat's own panel, not a
            // window the operator arranges.
            let tile = snappable
                .then(|| {
                    data.space
                        .outputs()
                        .next()
                        .and_then(|o| data.space.output_geometry(o))
                        .and_then(|screen| {
                            crate::snap::zone_for(
                                (pointer.x as i32, pointer.y as i32).into(),
                                screen,
                            )
                        })
                })
                .flatten();
            if let Some(tile) = tile {
                crate::snap::set(&self.window, Some(tile));
                data.apply_layout();
            } else if let Some(geo) = data.space.element_geometry(&self.window) {
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

    pointer_grab_passthrough!();
}

/// Which edges of a frame a resize moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    /// No edge at all — not a resize.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !(self.left || self.right || self.top || self.bottom)
    }
}

impl From<smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge>
    for Edges
{
    fn from(
        e: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) -> Self {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as R;
        let (left, right, top, bottom) = match e {
            R::Left => (true, false, false, false),
            R::Right => (false, true, false, false),
            R::Top => (false, false, true, false),
            R::Bottom => (false, false, false, true),
            R::TopLeft => (true, false, true, false),
            R::TopRight => (false, true, true, false),
            R::BottomLeft => (true, false, false, true),
            R::BottomRight => (false, true, false, true),
            _ => (false, false, false, false),
        };
        Self {
            left,
            right,
            top,
            bottom,
        }
    }
}

/// The smallest FRAME a resize may produce: room for the titlebar's three
/// buttons and a few lines of content. Below it a window is a sliver the
/// operator cannot grab again.
pub const MIN_W: i32 = 160;
/// See [`MIN_W`].
pub const MIN_H: i32 = crate::chrome::HEIGHT + 72;

/// How far OUTSIDE a floating frame a press still grabs its edge. The content
/// surface takes clicks inside it, so the handle lives in the margin — 8 px,
/// the width of a deliberate aim at an edge.
pub const RESIZE_MARGIN: i32 = 8;

/// How far along an edge from a corner still grabs the corner, so a corner is
/// a comfortable target rather than an 8 x 8 px square.
pub const CORNER_RUN: i32 = 16;

/// `frame` resized by a pointer that moved `(dx, dy)` while holding `edges`.
///
/// ★ PURE. The dragged edges move and the opposite edges STAY PUT — dragging
/// the left edge leftward grows the window leftward, it does not slide it.
/// Clamped at [`MIN_W`]/[`MIN_H`] without letting the fixed edge move.
#[must_use]
pub fn resized(
    frame: smithay::utils::Rectangle<i32, Logical>,
    edges: Edges,
    dx: i32,
    dy: i32,
    (min_w, min_h): (i32, i32),
) -> smithay::utils::Rectangle<i32, Logical> {
    // The floor is the larger of the seat's minimum and the CLIENT's own
    // (`xdg_toplevel.set_min_size`, grown by the titlebar): configuring a
    // window below what it declared it can render is the compositor
    // overruling the protocol.
    let (min_w, min_h) = (min_w.max(MIN_W), min_h.max(MIN_H));
    let (mut x, mut y, mut w, mut h) = (frame.loc.x, frame.loc.y, frame.size.w, frame.size.h);
    if edges.right {
        w = (frame.size.w + dx).max(min_w);
    }
    if edges.bottom {
        h = (frame.size.h + dy).max(min_h);
    }
    if edges.left {
        w = (frame.size.w - dx).max(min_w);
        x = frame.loc.x + frame.size.w - w;
    }
    if edges.top {
        h = (frame.size.h - dy).max(min_h);
        y = frame.loc.y + frame.size.h - h;
    }
    smithay::utils::Rectangle::new((x, y).into(), (w, h).into())
}

/// Which edges a press at `p` grabs on `frame`, or `None` when it is not in
/// the resize margin around it.
#[must_use]
pub fn border_hit(
    frame: smithay::utils::Rectangle<i32, Logical>,
    p: Point<f64, Logical>,
) -> Option<Edges> {
    #[allow(clippy::cast_possible_truncation)]
    let (px, py) = (p.x.floor() as i32, p.y.floor() as i32);
    let (l, t) = (frame.loc.x, frame.loc.y);
    let (r, b) = (l + frame.size.w, t + frame.size.h);
    let m = RESIZE_MARGIN;
    let inside_outer = px >= l - m && px < r + m && py >= t - m && py < b + m;
    let inside_frame = px >= l && px < r && py >= t && py < b;
    if !inside_outer || inside_frame {
        return None;
    }
    let near = |v: i32, edge: i32| (v - edge).abs() <= CORNER_RUN;
    let left = px < l || (py < t || py >= b) && near(px, l);
    let right = px >= r || (py < t || py >= b) && near(px, r);
    let top = py < t || (px < l || px >= r) && near(py, t);
    let bottom = py >= b || (px < l || px >= r) && near(py, b);
    let e = Edges {
        left,
        right,
        top,
        bottom,
    };
    (!e.is_empty()).then_some(e)
}

/// A floating window being resized by the pointer.
///
/// Works in FRAME space (titlebar included), because that is the unit
/// `floatpos` remembers and the layout places; the client is configured with
/// the content size `chrome::content_for` derives from it.
pub struct ResizeGrab {
    pub start_data: GrabStartData<Omoya>,
    pub window: Window,
    pub edges: Edges,
    /// The frame at grab start. Every motion resizes from THIS, never from
    /// the last motion, so rounding cannot accumulate into drift.
    pub initial: smithay::utils::Rectangle<i32, Logical>,
}

impl ResizeGrab {
    /// Start resizing `window` by `edges`. A snapped window is freed first and
    /// resized from the tile it was in, as on Windows and macOS.
    pub fn begin(
        data: &Omoya,
        window: Window,
        edges: Edges,
        start_data: GrabStartData<Omoya>,
    ) -> Option<Self> {
        let geo = data.space.element_geometry(&window)?;
        let initial = smithay::utils::Rectangle::new(
            (geo.loc.x, geo.loc.y - crate::chrome::HEIGHT).into(),
            (geo.size.w, geo.size.h + crate::chrome::HEIGHT).into(),
        );
        crate::snap::set(&window, None);
        Some(Self {
            start_data,
            window,
            edges,
            initial,
        })
    }
}

impl PointerGrab<Omoya> for ResizeGrab {
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
        handle.motion(data, None, event);
        #[allow(clippy::cast_possible_truncation)]
        let (dx, dy) = (
            (event.location.x - self.start_data.location.x) as i32,
            (event.location.y - self.start_data.location.y) as i32,
        );
        let min = crate::layout::client_min_frame(&self.window);
        let frame = resized(self.initial, self.edges, dx, dy, min);
        crate::floatpos::remember(&self.window, frame.loc);
        crate::floatpos::remember_size(&self.window, frame.size);
        data.place_frame(&self.window, frame);
        data.introspect.mark(crate::owed::Owed::Windows);
    }

    fn button(
        &mut self,
        data: &mut Omoya,
        handle: &mut PointerInnerHandle<'_, Omoya>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            data.active_resize = None;
            data.introspect.mark(crate::owed::Owed::Windows);
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    pointer_grab_passthrough!();
}

impl Omoya {
    /// The floating window whose resize margin `p` is in, and which edges —
    /// front-most first, so where two margins meet the visible window wins.
    /// Overlays (the launcher) are not resizable: they size themselves.
    ///
    /// ONE hit-test for both the click that starts a resize and the cursor
    /// that advertises it, so the arrow can never promise an edge the click
    /// will not grab.
    #[must_use]
    pub fn border_under(&self, p: Point<f64, Logical>) -> Option<(Window, Edges)> {
        if self.config.layout.mode != crate::config::LayoutMode::Floating {
            return None;
        }
        self.space.elements().rev().find_map(|w| {
            if !crate::role::policy_of(w, &self.config.placement).resizable {
                return None;
            }
            let geo = self.space.element_geometry(w)?;
            let frame = smithay::utils::Rectangle::new(
                (geo.loc.x, geo.loc.y - crate::chrome::HEIGHT).into(),
                (geo.size.w, geo.size.h + crate::chrome::HEIGHT).into(),
            );
            border_hit(frame, p).map(|e| (w.clone(), e))
        })
    }

    /// The pointer shape to draw right now: the resize arrow for the edge
    /// being dragged, else for the margin the pointer hovers, else the arrow.
    #[must_use]
    pub fn cursor_shape(&self) -> crate::cursor::Shape {
        let edges = self
            .active_resize
            .or_else(|| self.border_under(self.pointer_location).map(|(_, e)| e));
        crate::cursor::Shape::for_edges(edges)
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;
    use smithay::utils::Rectangle;

    fn frame() -> Rectangle<i32, Logical> {
        Rectangle::new((100, 100).into(), (400, 300).into())
    }
    const R: Edges = Edges {
        left: false,
        right: true,
        top: false,
        bottom: false,
    };
    const L: Edges = Edges {
        left: true,
        right: false,
        top: false,
        bottom: false,
    };
    const TL: Edges = Edges {
        left: true,
        right: false,
        top: true,
        bottom: false,
    };
    const BR: Edges = Edges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };

    #[test]
    fn the_right_edge_grows_rightward_and_the_left_edge_stays() {
        let r = resized(frame(), R, 50, 999, (0, 0));
        assert_eq!(r, Rectangle::new((100, 100).into(), (450, 300).into()));
    }

    #[test]
    fn the_left_edge_grows_leftward_and_the_right_edge_stays() {
        let r = resized(frame(), L, -50, 0, (0, 0));
        assert_eq!(r.loc.x, 50);
        assert_eq!(r.loc.x + r.size.w, 500, "the right edge must not move");
    }

    #[test]
    fn a_corner_moves_both_of_its_edges() {
        let r = resized(frame(), TL, -10, -20, (0, 0));
        assert_eq!(r, Rectangle::new((90, 80).into(), (410, 320).into()));
    }

    #[test]
    fn shrinking_stops_at_the_minimum_without_moving_the_fixed_edge() {
        let r = resized(frame(), TL, 10_000, 10_000, (0, 0));
        assert_eq!((r.size.w, r.size.h), (MIN_W, MIN_H));
        assert_eq!((r.loc.x + r.size.w, r.loc.y + r.size.h), (500, 400));
        let r = resized(frame(), BR, -10_000, -10_000, (0, 0));
        assert_eq!(
            r.loc,
            frame().loc,
            "shrinking from bottom-right keeps the origin"
        );
    }

    #[test]
    fn a_clients_own_minimum_beats_the_seats() {
        let r = resized(frame(), BR, -10_000, -10_000, (300, 250));
        assert_eq!((r.size.w, r.size.h), (300, 250));
        // …and the seat floor still holds when the client declares less.
        let r = resized(frame(), BR, -10_000, -10_000, (10, 10));
        assert_eq!((r.size.w, r.size.h), (MIN_W, MIN_H));
    }

    #[test]
    fn the_margin_outside_an_edge_grabs_it_and_the_inside_does_not() {
        let p = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));
        assert_eq!(border_hit(frame(), p(96.0, 250.0)), Some(L));
        assert_eq!(border_hit(frame(), p(503.0, 250.0)), Some(R));
        assert_eq!(
            border_hit(frame(), p(300.0, 250.0)),
            None,
            "inside is the client's"
        );
        assert_eq!(
            border_hit(frame(), p(80.0, 250.0)),
            None,
            "beyond the margin"
        );
    }

    #[test]
    fn near_a_corner_the_margin_grabs_the_corner() {
        let p = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));
        assert_eq!(border_hit(frame(), p(96.0, 96.0)), Some(TL));
        assert_eq!(
            border_hit(frame(), p(96.0, 108.0)),
            Some(TL),
            "along the left edge near the top"
        );
        assert_eq!(border_hit(frame(), p(503.0, 398.0)), Some(BR));
    }

    #[test]
    fn every_protocol_edge_maps_and_none_is_empty() {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as E;
        assert_eq!(Edges::from(E::BottomRight), BR);
        assert_eq!(Edges::from(E::TopLeft), TL);
        assert!(Edges::from(E::None).is_empty());
    }
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

/// The zone a dragged window must stay inside — the output minus the bar.
///
/// ★ THE SAME ZONE THE LAYOUT USES. The release snap took its zone from
/// `space.output_geometry` (the whole output) while the layout snaps to the
/// non-exclusive zone, so the two disagreed by the bar's height and a window
/// snapped to one edge on drag and a different one on the next layout pass.
fn usable_zone(data: &Omoya) -> Option<smithay::utils::Rectangle<i32, Logical>> {
    let output = data.space.outputs().next()?;
    let geo = data.space.output_geometry(output)?;
    let bar = data.config.bar.height;
    Some(smithay::utils::Rectangle::new(
        (geo.loc.x, geo.loc.y + bar).into(),
        (geo.size.w, (geo.size.h - bar).max(0)).into(),
    ))
}
