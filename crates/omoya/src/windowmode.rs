//! Close, minimise, maximise and tabs — as STATE, not as chrome.
//!
//! ── ★ WHAT THIS IS AND IS NOT ─────────────────────────────────────────────
//! The operator asked for "close minimize maximize and a tab bar like macOS
//! or something, not looks wise just as a guide for function". So this module
//! is the function and none of the looks: it owns the four bits of per-window
//! state those controls manipulate, and it decides ONE thing —
//! [`Windows::placement_of`], which answers "where does this window go, or is
//! it hidden". Nothing here draws a title bar, a traffic light or a tab strip.
//!
//! ── ★ WHY IT IS PURE, AND WHY THAT IS NOT A STYLE CHOICE ──────────────────
//! `layout.rs` records the receipt: its own tree half was split out into
//! `map_id` because `Window` needs a `ToplevelSurface`, which needs a client,
//! which needs a display — so the first tiling defect "had to be chased
//! through a VM screenshot: there was no cheaper place to ask the question".
//! Every function here takes and returns plain ids and rectangles, so the
//! whole state machine is exercised by unit tests with no seat, no client and
//! no GPU.
//!
//! ── ★ WHY THE TILING TREE IS UNTOUCHED ────────────────────────────────────
//! A tab group is N windows sharing ONE rectangle. `kukaku`'s `LayoutNode` is
//! a geometry tree over an opaque `Id` and has no tabbed container — and it
//! does not need one, because "which of these windows is the visible one" is
//! not a geometry question. Adding a stacking node to kukaku would put that
//! decision in the crate that computes rectangles, where two windows claiming
//! one rect is exactly the state its `validate()` exists to reject.
//!
//! So: kukaku keeps answering "which rectangles exist", this module answers
//! "who is shown in one", and neither knows about the other.
//!
//! ── ★ THE KEY IS THE SURFACE'S PROTOCOL ID ────────────────────────────────
//! Same key `layout.rs` and `truedamage.rs` already use (`surface_id_of` /
//! `protocol_id()`), deliberately not smithay's `Window`: `Window` compares by
//! pointer identity, so a recycled allocation would alias two windows into one
//! entry — the exact hazard `WindowId`'s doc comment records.

use std::collections::HashMap;

/// A window's own display state.
///
/// ★ A CLOSED ENUM, so "minimised AND maximised at once" has no
/// representation. The obvious alternative is two booleans, and it admits
/// that pair — at which point every reader has to be told which wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Wherever the layout puts it.
    #[default]
    Normal,
    /// Filling the usable zone, with the pre-maximise rectangle remembered by
    /// the caller's layout rather than stored here — the layout recomputes it
    /// every pass anyway, so a stored copy would be a second source of truth
    /// that goes stale the moment the output changes mode.
    Maximized,
    /// Alive, mapped to no rectangle, reachable only by restoring.
    ///
    /// ★ NOT a tiny rectangle offscreen, and not an unmapped surface. The
    /// client keeps running and keeps its buffer; it simply gets no placement,
    /// which is what makes restore free and what stops a minimised terminal
    /// from being killed by a compositor that "cleaned up" an invisible
    /// window.
    Minimized,
}

/// Windows shown one-at-a-time in a shared rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Members in tab order. Never empty — an empty group is dissolved by
    /// [`Windows::leave`] rather than kept as a group of nobody.
    pub members: Vec<u32>,
    /// Index into `members` of the visible one. Always in range: every
    /// mutation clamps it, so `members[active]` cannot panic.
    pub active: usize,
}

impl Group {
    fn visible(&self) -> Option<u32> {
        self.members.get(self.active).copied()
    }
}

/// Where a window should go this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Use whatever the layout computed.
    AsLaidOut,
    /// Fill the usable zone.
    Maximized,
    /// Map nothing. Minimised, or an inactive tab.
    Hidden,
}

/// The per-window state behind the four controls.
#[derive(Debug, Default)]
pub struct Windows {
    mode: HashMap<u32, Mode>,
    groups: Vec<Group>,
    /// Minimise order, most recent last — so restore is LIFO, which is what
    /// "un-minimise" means to a person. A set would make restore pick an
    /// arbitrary window.
    minimized_order: Vec<u32>,
}

impl Windows {
    /// The one decision this module exists to make.
    ///
    /// ★ Minimised beats tab-inactive beats maximised, and the order is not
    /// arbitrary: a minimised window is hidden even if it is a group's active
    /// member (you minimised it deliberately), and an inactive tab is hidden
    /// even if maximised (it is behind its sibling, and a maximised invisible
    /// window would claim the whole screen while showing nothing).
    #[must_use]
    pub fn placement_of(&self, id: u32) -> Placement {
        if self.mode_of(id) == Mode::Minimized {
            return Placement::Hidden;
        }
        if let Some(g) = self.group_of(id) {
            if g.visible() != Some(id) {
                return Placement::Hidden;
            }
        }
        if self.mode_of(id) == Mode::Maximized {
            return Placement::Maximized;
        }
        Placement::AsLaidOut
    }

    #[must_use]
    pub fn mode_of(&self, id: u32) -> Mode {
        self.mode.get(&id).copied().unwrap_or_default()
    }

    /// Maximise or restore. Returns the mode now in force.
    ///
    /// A minimised window maximises to VISIBLE rather than staying hidden —
    /// asking to maximise something you cannot see is unambiguously a request
    /// to see it.
    pub fn toggle_maximize(&mut self, id: u32) -> Mode {
        let next = if self.mode_of(id) == Mode::Maximized {
            Mode::Normal
        } else {
            Mode::Maximized
        };
        self.set_mode(id, next);
        next
    }

    /// Hide a window without closing it.
    pub fn minimize(&mut self, id: u32) {
        self.set_mode(id, Mode::Minimized);
    }

    /// Un-minimise the most recently minimised window, if any.
    ///
    /// Returns the id so the caller can also give it focus — a restore that
    /// leaves the keyboard elsewhere is a window that reappears and ignores
    /// you.
    pub fn restore_last(&mut self) -> Option<u32> {
        let id = self.minimized_order.pop()?;
        self.mode.insert(id, Mode::Normal);
        Some(id)
    }

    fn set_mode(&mut self, id: u32, next: Mode) {
        self.minimized_order.retain(|m| *m != id);
        if next == Mode::Minimized {
            self.minimized_order.push(id);
        }
        self.mode.insert(id, next);
    }

    /// Put `id` into `host`'s group, creating one if `host` has none.
    ///
    /// The joiner becomes ACTIVE, because joining is something you do while
    /// looking at the window you are moving.
    pub fn join(&mut self, id: u32, host: u32) {
        if id == host {
            return;
        }
        self.leave(id);
        match self.group_index_of(host) {
            Some(gi) => {
                let g = &mut self.groups[gi];
                g.members.push(id);
                g.active = g.members.len() - 1;
            }
            None => self.groups.push(Group {
                members: vec![host, id],
                active: 1,
            }),
        }
    }

    /// Remove `id` from any group, dissolving a group that drops below two.
    ///
    /// ★ BELOW TWO, NOT BELOW ONE. A "group" of one window is a window, and
    /// keeping it would leave a tab strip with a single tab and a cycle verb
    /// that does nothing — state that is invisible and still has to be
    /// reasoned about.
    pub fn leave(&mut self, id: u32) {
        let Some(gi) = self.group_index_of(id) else {
            return;
        };
        let g = &mut self.groups[gi];
        g.members.retain(|m| *m != id);
        if g.members.len() < 2 {
            self.groups.remove(gi);
        } else if g.active >= g.members.len() {
            g.active = g.members.len() - 1;
        }
    }

    /// Show the next (or previous) member of `id`'s group. Returns the newly
    /// visible window, or `None` when `id` is not grouped.
    pub fn cycle(&mut self, id: u32, forward: bool) -> Option<u32> {
        let gi = self.group_index_of(id)?;
        let g = &mut self.groups[gi];
        let n = g.members.len();
        if n == 0 {
            return None;
        }
        // Wrapping both ways, computed in usize so `- 1` at index 0 cannot
        // underflow: + (n - 1) is - 1 modulo n.
        g.active = if forward {
            (g.active + 1) % n
        } else {
            (g.active + n - 1) % n
        };
        g.visible()
    }

    #[must_use]
    pub fn group_of(&self, id: u32) -> Option<&Group> {
        self.group_index_of(id).map(|i| &self.groups[i])
    }

    fn group_index_of(&self, id: u32) -> Option<usize> {
        self.groups.iter().position(|g| g.members.contains(&id))
    }

    /// `id`'s 0-based position in its group and the group size.
    ///
    /// ★ POSITION IN `members`, NOT the active index. The indicator answers
    /// "which of these am I looking at", so it must describe THIS window —
    /// and for the visible member those coincide, which is exactly why
    /// returning `active` would look correct in every hand test and be wrong
    /// the moment anything asks about a window that is not the visible one.
    #[must_use]
    pub fn position_in_group(&self, id: u32) -> Option<(usize, usize)> {
        let g = self.group_of(id)?;
        let idx = g.members.iter().position(|m| *m == id)?;
        Some((idx, g.members.len()))
    }

    /// Forget a window entirely. Called when it is closed.
    ///
    /// ★ WITHOUT THIS, MINIMISE IS A LEAK — and a nasty one, because the state
    /// is keyed by a protocol id the server reuses. A closed minimised window
    /// left in `minimized_order` would be "restored" as some later window that
    /// happened to get the same id.
    pub fn forget(&mut self, id: u32) {
        self.leave(id);
        self.mode.remove(&id);
        self.minimized_order.retain(|m| *m != id);
    }

    /// How many windows are minimised — the count a dock would show.
    #[must_use]
    pub fn minimized_count(&self) -> usize {
        self.minimized_order.len()
    }

    /// Every group, for the introspection leaf.
    #[must_use]
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_window_is_normal_and_laid_out() {
        let w = Windows::default();
        assert_eq!(w.mode_of(7), Mode::Normal);
        assert_eq!(w.placement_of(7), Placement::AsLaidOut);
    }

    #[test]
    fn maximize_toggles_and_reports_the_mode_now_in_force() {
        let mut w = Windows::default();
        assert_eq!(w.toggle_maximize(1), Mode::Maximized);
        assert_eq!(w.placement_of(1), Placement::Maximized);
        assert_eq!(w.toggle_maximize(1), Mode::Normal);
        assert_eq!(w.placement_of(1), Placement::AsLaidOut);
    }

    #[test]
    fn maximizing_a_minimized_window_makes_it_visible() {
        // Asking to maximise something you cannot see is unambiguously a
        // request to SEE it; leaving it hidden would be a control that
        // silently does nothing.
        let mut w = Windows::default();
        w.minimize(1);
        assert_eq!(w.placement_of(1), Placement::Hidden);
        w.toggle_maximize(1);
        assert_eq!(w.placement_of(1), Placement::Maximized);
        assert_eq!(w.minimized_count(), 0, "it must leave the minimise list");
    }

    #[test]
    fn restore_is_lifo_because_that_is_what_un_minimise_means() {
        let mut w = Windows::default();
        w.minimize(1);
        w.minimize(2);
        w.minimize(3);
        assert_eq!(w.restore_last(), Some(3));
        assert_eq!(w.restore_last(), Some(2));
        assert_eq!(w.restore_last(), Some(1));
        assert_eq!(w.restore_last(), None, "nothing left to restore");
    }

    #[test]
    fn minimizing_twice_does_not_double_the_restore_queue() {
        let mut w = Windows::default();
        w.minimize(1);
        w.minimize(1);
        assert_eq!(w.minimized_count(), 1);
        assert_eq!(w.restore_last(), Some(1));
        assert_eq!(w.restore_last(), None);
    }

    #[test]
    fn a_group_shows_exactly_one_member() {
        let mut w = Windows::default();
        w.join(2, 1);
        let shown: Vec<u32> = [1, 2]
            .into_iter()
            .filter(|id| w.placement_of(*id) != Placement::Hidden)
            .collect();
        assert_eq!(shown, vec![2], "the joiner is active");
    }

    #[test]
    fn cycling_wraps_in_both_directions_without_underflow() {
        let mut w = Windows::default();
        w.join(2, 1);
        w.join(3, 1);
        // active is 3 (last joiner). Forward wraps to the head.
        assert_eq!(w.cycle(1, true), Some(1));
        assert_eq!(w.cycle(1, true), Some(2));
        // Backward from index 1 to 0, then wrap to the tail.
        assert_eq!(w.cycle(1, false), Some(1));
        assert_eq!(w.cycle(1, false), Some(3));
    }

    #[test]
    fn cycling_an_ungrouped_window_is_none_not_a_panic() {
        let mut w = Windows::default();
        assert_eq!(w.cycle(9, true), None);
    }

    #[test]
    fn a_group_of_one_is_dissolved_rather_than_kept() {
        let mut w = Windows::default();
        w.join(2, 1);
        w.leave(2);
        assert!(w.group_of(1).is_none(), "a group of one is a window");
        assert_eq!(w.placement_of(1), Placement::AsLaidOut);
    }

    #[test]
    fn leaving_clamps_the_active_index_instead_of_going_out_of_range() {
        let mut w = Windows::default();
        w.join(2, 1);
        w.join(3, 1);
        // active == 2 (id 3). Remove it and active must fall back in range.
        w.leave(3);
        let g = w.group_of(1).expect("two members remain");
        assert!(g.active < g.members.len(), "active {} out of range", g.active);
    }

    #[test]
    fn joining_moves_a_window_between_groups_rather_than_duplicating_it() {
        let mut w = Windows::default();
        w.join(2, 1);
        w.join(4, 3);
        w.join(2, 3);
        assert_eq!(
            w.group_of(1),
            None,
            "the first group dropped to one member and dissolved"
        );
        let g = w.group_of(3).expect("second group holds it");
        assert_eq!(g.members.iter().filter(|m| **m == 2).count(), 1);
    }

    #[test]
    fn a_window_cannot_join_itself() {
        let mut w = Windows::default();
        w.join(1, 1);
        assert!(w.group_of(1).is_none());
    }

    #[test]
    fn minimized_beats_tab_active() {
        // Deliberate order: you minimised it, so it stays hidden even though
        // it is its group's visible member.
        let mut w = Windows::default();
        w.join(2, 1);
        w.minimize(2);
        assert_eq!(w.placement_of(2), Placement::Hidden);
    }

    #[test]
    fn an_inactive_tab_is_hidden_even_when_maximized() {
        // A maximised invisible window would claim the whole screen and show
        // nothing.
        let mut w = Windows::default();
        w.join(2, 1);
        w.toggle_maximize(1); // 1 is the INACTIVE member
        assert_eq!(w.placement_of(1), Placement::Hidden);
        assert_eq!(w.placement_of(2), Placement::AsLaidOut);
    }

    #[test]
    fn position_describes_the_asked_for_window_not_the_visible_one() {
        // The trap: for the VISIBLE member, position and `active` coincide, so
        // returning `active` passes every hand test and is wrong for anyone
        // else in the group.
        let mut w = Windows::default();
        w.join(2, 1); // members [1, 2], active = 1 (id 2)
        assert_eq!(w.position_in_group(1), Some((0, 2)), "the inactive member");
        assert_eq!(w.position_in_group(2), Some((1, 2)), "the visible member");
        assert_eq!(w.position_in_group(9), None, "ungrouped");
    }

    #[test]
    fn forget_prevents_a_recycled_id_from_being_restored_as_someone_else() {
        // The key is a protocol id the server REUSES. Without `forget`, a
        // closed minimised window would be restored as whatever later window
        // inherited its id.
        let mut w = Windows::default();
        w.join(2, 1);
        w.minimize(2);
        w.forget(2);
        assert_eq!(w.minimized_count(), 0);
        assert_eq!(w.mode_of(2), Mode::Normal);
        assert!(w.group_of(1).is_none(), "the group dissolved with it");
        assert_eq!(w.restore_last(), None);
    }
}
