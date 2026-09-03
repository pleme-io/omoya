//! A window id that is actually unique.
//!
//! ── ★ THE DEFECT THIS CLOSES ─────────────────────────────────────────────
//! Eight sites across `deed.rs`, `handlers.rs`, `layout.rs` and
//! `truedamage.rs` identified a window by `surface.id().protocol_id()`.
//! wayland-backend documents that value verbatim:
//!
//! > each client has its own ID space, so this should not be used as a unique
//! > identifier
//!
//! Measured with `WAYLAND_DEBUG=1` against two live mado clients on plo:
//! byte-identical allocation, `wl_surface#16` for both. **Every mado on the
//! seat was id 16.**
//!
//! What that cost, replicated live with three windows:
//!
//! * one `toggle-maximize` took all three to fullscreen — a literal perfect
//!   stack, from a single deed;
//! * one `minimize` unmapped the whole seat while `minimized_count` read 1;
//! * `close_focused` and `focus_surface_id` use `find`, so they resolve to
//!   the FIRST match — a close aimed at one window lands on another;
//! * `truedamage::Shadows` held 2 keys for 4 windows, so damage tracking was
//!   comparing one window's shadow against another's frame;
//! * `previous_focus` is set only when `outgoing != focused_of(window)`,
//!   which between two mados is `Some(16) != Some(16)` — always false — so
//!   it was NEVER set and `Deed::TabJoin` could only ever refuse.
//!
//! ── ★ WHY A MINTED COUNTER, NOT A `{client, protocol}` PAIR ──────────────
//! A pair is the obvious fix and needs the client id, which means dragging a
//! `wayland_server` type into `windowmode.rs`. That module's header makes its
//! seatless purity load-bearing — *"exercised by unit tests with no seat, no
//! client and no GPU"*, citing the receipt that the first tiling defect had
//! to be chased through a VM screenshot. Keying on a client-bearing type
//! would kill ~15 of those tests.
//!
//! A monotonic counter stored on the SURFACE keeps the public type `u32`, so
//! every consumer and every seatless test is untouched, while the value is
//! unique across clients by construction. `Tiling::map_id` already does
//! exactly this for its own tree, so the shape is not new to this codebase.
//!
//! ── ★ STORED ON THE SURFACE, NOT THE WINDOW ──────────────────────────────
//! Five of the eight call sites hold only a `&WlSurface` — a destroyed
//! toplevel has no `Window` any more. `SurfaceData::data_map` is reachable
//! from both, so one function serves every site and there is no second
//! spelling to drift.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use std::sync::atomic::{AtomicU32, Ordering};

/// The next id to hand out.
///
/// Starts at 1 so 0 is available as the "could not resolve" answer without
/// colliding with a real window.
static NEXT: AtomicU32 = AtomicU32::new(1);

/// A window's id, as stored on its surface.
struct WinId(u32);

/// The id of the window owning `surface`, minting one on first sight.
///
/// Stable for the life of the surface: minted once, then read. Two surfaces
/// never share a value, including across clients — which is the whole point.
#[must_use]
pub fn of(surface: &WlSurface) -> u32 {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(|| WinId(NEXT.fetch_add(1, Ordering::Relaxed)));
        states.data_map.get::<WinId>().map_or(0, |w| w.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ THE COUNTER HANDS OUT DISTINCT VALUES.
    ///
    /// The storage half needs a live surface and is exercised by the seat;
    /// the part that can be unit-tested is the one that was actually wrong —
    /// `protocol_id` returned the same number twice, and this must not.
    #[test]
    fn successive_mints_never_repeat() {
        let a = NEXT.fetch_add(1, Ordering::Relaxed);
        let b = NEXT.fetch_add(1, Ordering::Relaxed);
        let c = NEXT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert!(
            b > a && c > b,
            "ids must increase, so a later window can never be mistaken for an \
             earlier one that has gone"
        );
    }

    /// ★ ZERO IS RESERVED, so "could not resolve" is distinguishable from a
    /// real window. `protocol_id` had no such reservation.
    #[test]
    fn no_window_is_ever_id_zero() {
        assert!(
            NEXT.load(Ordering::Relaxed) >= 1,
            "the counter must start above the reserved 0"
        );
    }
}
