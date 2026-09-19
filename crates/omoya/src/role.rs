//! What KIND of window this is — and therefore what may be done to it.
//!
//! ── ★ WHY A ROLE AND NOT ANOTHER PREDICATE (2026-09-19) ──────────────────
//! The operator: *"tobira should not have a top bar and actually should not be
//! movable, it's special in that way, perhaps we need to architect for that."*
//!
//! tobira is the launcher: a transient centred panel summoned by Ctrl+Space.
//! It is not a window an operator manages, yet it was getting the full
//! treatment — a server-side titlebar with close/minimise/maximise (drawn
//! DETACHED above its panel, because the launcher's surface is smaller than
//! the frame), draggable by that bar and by Logo+drag.
//!
//! The notion already existed, three times over and under three names:
//! `placement::Placement::Floating` (the app is an overlay BY NATURE),
//! `layout.rs`'s local `is_overlay`, and `grab::border_under`'s
//! `is_floating()` exclusion. Three readings of one fact, each re-derived at
//! its call site — so chrome and move, which never asked, got it wrong.
//!
//! This module is the single answer. A window has ONE role; a role has ONE
//! policy; every site reads the policy instead of re-deriving a predicate.
//!
//! ── ★ THE TIER, HONESTLY ────────────────────────────────────────────────
//! Most of the policy is *forced-at-the-call-site*, not *unrepresentable*:
//! a site that wants to move a window must ask [`RolePolicy::movable`].
//! ONE guarantee is stronger — chrome geometry is gated by [`Decorated`], a
//! token with no public constructor outside this module, obtainable only from
//! a policy that permits decoration. `chrome::bar_rect` and `chrome::hit`
//! take it, so "draw a titlebar for the launcher" does not COMPILE, rather
//! than being a rule someone remembers. The rest is test-caught, by
//! `policy_table` and `overlay_is_not_managed` below.

use smithay::desktop::Window;

/// The kind of window.
///
/// Closed and two-armed on purpose: a third arm would be a third policy, and
/// the fleet rule is one place per fact. Add an arm only with its row in
/// [`WindowRole::policy`] — the match is exhaustive, so the compiler asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRole {
    /// An ordinary application window: the operator owns its geometry.
    Managed,
    /// A transient panel the SEAT owns — the launcher. Centred, undecorated,
    /// immovable, unresizable, and never snapped or tiled.
    Overlay,
}

/// What may be done to a window of a given role.
///
/// Every field answers a question some site used to answer for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolePolicy {
    /// Gets a server-side titlebar. See [`RolePolicy::decorated`].
    decorated: bool,
    /// May be dragged — by its bar, by Logo+drag, or by its own
    /// `xdg_toplevel.move` request.
    pub movable: bool,
    /// May be resized — by its edge margin, by Logo+right-drag, by the
    /// keyboard, or by its own `xdg_toplevel.resize` request.
    pub resizable: bool,
    /// May be snapped to a tile, by drag or by chord.
    pub snappable: bool,
    /// The compositor chooses its size. `false` means the window sizes itself
    /// and the seat only places it — sending a size would overrule the client
    /// about its own content.
    pub sized_by_compositor: bool,
    /// Placed in the centre of the usable zone every time it maps, rather
    /// than cascaded and then remembered.
    pub centred: bool,
}

/// Proof that a window may be decorated.
///
/// ★ THE ONE COMPILE-TIME GUARANTEE HERE. The field is private and this
/// module has the only constructor ([`RolePolicy::decorated`]), so a caller
/// cannot conjure one: `chrome::bar_rect(…)` and `chrome::hit(…)` require it,
/// and therefore cannot be called for an overlay at all. That is the
/// difference between "we remembered to check" and "there is no path".
#[derive(Debug, Clone, Copy)]
pub struct Decorated(());

impl RolePolicy {
    /// Proof of decoration, or `None` for a window that gets no chrome.
    ///
    /// The ONLY way to obtain a [`Decorated`].
    #[must_use]
    pub const fn decorated(self) -> Option<Decorated> {
        if self.decorated {
            Some(Decorated(()))
        } else {
            None
        }
    }

    /// Whether this window is decorated, for reporting — NOT for gating a
    /// draw (that is what [`Self::decorated`] is for).
    #[must_use]
    pub const fn is_decorated(self) -> bool {
        self.decorated
    }
}

impl WindowRole {
    /// The policy for this role. The whole table, in one place.
    #[must_use]
    pub const fn policy(self) -> RolePolicy {
        match self {
            Self::Managed => RolePolicy {
                decorated: true,
                movable: true,
                resizable: true,
                snappable: true,
                sized_by_compositor: true,
                centred: false,
            },
            // ★ EVERY FIELD IS THE OPPOSITE, AND THAT IS THE POINT. An
            // overlay is not "a window with a few features off" — it is the
            // seat's own panel, which the operator does not manage.
            Self::Overlay => RolePolicy {
                decorated: false,
                movable: false,
                resizable: false,
                snappable: false,
                sized_by_compositor: false,
                centred: true,
            },
        }
    }

    /// The role of `w`, derived from the ONE existing source of truth: the
    /// placement rule for its `app_id`.
    ///
    /// ★ DERIVED, NOT STORED. `app_id` arrives in a request AFTER the toplevel
    /// is created (`set_app_id`), so a role captured at map time would be
    /// `Managed` for the first frames of every launcher and then never
    /// corrected. `layout.rs` already re-runs placement on commit when the
    /// answer changes (`placement_changed`); deriving here means the role
    /// changes with it, for free.
    #[must_use]
    pub fn of(w: &Window, placement: &crate::config::PlacementConfig) -> Self {
        Self::of_app_id(crate::layout::app_id_of(w).as_deref(), placement)
    }

    /// [`Self::of`] without a `Window`, so the rule is testable.
    #[must_use]
    pub fn of_app_id(app_id: Option<&str>, placement: &crate::config::PlacementConfig) -> Self {
        if crate::placement::for_app_id_in(app_id, placement).is_floating() {
            Self::Overlay
        } else {
            Self::Managed
        }
    }
}

/// The policy for `w` — the call every site makes.
#[must_use]
pub fn policy_of(w: &Window, placement: &crate::config::PlacementConfig) -> RolePolicy {
    WindowRole::of(w, placement).policy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_table() {
        let m = WindowRole::Managed.policy();
        assert!(m.is_decorated() && m.movable && m.resizable && m.snappable);
        assert!(m.sized_by_compositor && !m.centred);

        let o = WindowRole::Overlay.policy();
        assert!(!o.is_decorated() && !o.movable && !o.resizable && !o.snappable);
        assert!(!o.sized_by_compositor && o.centred);
    }

    #[test]
    fn an_overlay_cannot_produce_proof_of_decoration() {
        // The compile-time half, asserted at runtime too so the intent is
        // visible: there is no other constructor for `Decorated`.
        assert!(WindowRole::Overlay.policy().decorated().is_none());
        assert!(WindowRole::Managed.policy().decorated().is_some());
    }

    #[test]
    fn the_launcher_is_an_overlay_and_a_terminal_is_not() {
        // Bound to the SAME placement config the seat runs, so this cannot
        // drift from `FLOATING_APP_IDS`.
        let cfg = crate::config::PlacementConfig::default();
        assert_eq!(
            WindowRole::of_app_id(Some("tobira"), &cfg),
            WindowRole::Overlay
        );
        assert_eq!(
            WindowRole::of_app_id(Some("mado"), &cfg),
            WindowRole::Managed
        );
        // An app that has not sent `set_app_id` yet is Managed — the honest
        // default, and it is corrected on the commit that brings the id.
        assert_eq!(WindowRole::of_app_id(None, &cfg), WindowRole::Managed);
    }
}
