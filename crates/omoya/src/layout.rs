//! Tiling — omoya's window arrangement, over `kukaku`'s split algebra.
//!
//! ── ★ THE ALGEBRA IS NOT WRITTEN HERE, AND THAT IS THE POINT ─────────────
//! Splitting an area in two, collapsing a split when one side goes away,
//! moving a divider, computing every leaf's rectangle, finding the neighbour
//! in a direction — none of that is compositor work. It is the same problem a
//! terminal multiplexer solves for panes, and `tear` had already solved it
//! well: a refined `SplitRatio` with the NaN trap closed, 49 tests.
//!
//! So it was extracted rather than re-derived. `kukaku` is generic over what a
//! leaf IS, and its tests run against a leaf id neither consumer uses — which
//! is the evidence the algebra never depended on panes. This module supplies
//! the two things that genuinely are omoya's: what a leaf identifies (a
//! `Window`), and what a rectangle means (pixels).
//!
//! ── ★ WHY A SIDE TABLE AND NOT AN ID INSIDE `Window` ─────────────────────
//! smithay's `Window` is a handle we do not own and cannot add a field to, and
//! it is `PartialEq` by inner pointer identity. So the tree stores a `WindowId`
//! and this module keeps the mapping. The alternative — keying the tree on
//! `Window` directly — would put a refcounted handle inside a `Clone` tree and
//! make every layout operation touch the compositor's object graph.

use std::collections::HashMap;

use kukaku::{Direction, LayoutNode, LeafRemoval, Rect, SplitOrientation};
use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};

/// A window's identity inside the layout tree.
///
/// Deliberately a plain counter and not a hash of the `Window`: smithay's
/// `Window` compares by pointer identity, so a hash would be stable only for
/// as long as the allocation, and a recycled address would silently alias two
/// windows into one leaf.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// The tiling state for one output.
#[derive(Debug, Default)]
pub struct Tiling {
    tree: Option<LayoutNode<WindowId>>,
    windows: HashMap<WindowId, Window>,
    next: u64,
    /// Which leaf the keyboard is on. Kept here rather than derived from
    /// smithay's focus because the layout needs it BEFORE the focus moves —
    /// a new window splits the focused one, and asking the seat afterwards
    /// gives the answer for the window that just arrived.
    focus: Option<WindowId>,
}

impl Tiling {
    /// Add a window, splitting the focused leaf.
    ///
    /// The FIRST window becomes the whole tree; every later one splits
    /// whatever holds focus, alternating orientation with depth so a run of
    /// new windows produces a usable grid rather than a column of slivers.
    pub fn map(&mut self, window: Window) -> WindowId {
        let id = WindowId(self.next);
        self.next += 1;
        self.windows.insert(id, window);

        self.tree = Some(match self.tree.take() {
            None => LayoutNode::leaf(id),
            Some(mut tree) => {
                let target = self.focus.filter(|f| tree.contains_pane(*f));
                match target {
                    Some(t) => {
                        // Direction::Right means "the new leaf goes right of
                        // the target", i.e. a vertical divider. kukaku takes
                        // the direction rather than the orientation because
                        // WHICH SIDE the newcomer lands on is not derivable
                        // from the orientation alone.
                        let dir = if self.depth_of(&tree, t) % 2 == 0 {
                            Direction::Right
                        } else {
                            Direction::Below
                        };
                        // 0.5: an even split. kukaku takes the ratio as a plain f32
                        // and refines it internally, so there is no
                        // "unspecified" to pass — an even split IS the
                        // default, stated rather than implied.
                        tree.split_leaf(t, id, dir, 0.5);
                        tree
                    }
                    // Focus names a window the tree does not hold — possible
                    // if a window was unmapped without the focus moving.
                    // Splitting the root keeps the newcomer visible rather
                    // than dropping it on the floor.
                    None => LayoutNode::split(
                        SplitOrientation::Vertical,
                        tree,
                        LayoutNode::leaf(id),
                    ),
                }
            }
        });
        self.focus = Some(id);
        id
    }

    /// Remove a window and collapse its split.
    ///
    /// Returns `true` if the tree still holds anything. `LeafRemoval::WasRoot`
    /// is not an error: a tree with no leaves has no representation in
    /// `kukaku` by design, so the empty case is the ABSENCE of a tree here.
    pub fn unmap(&mut self, window: &Window) -> bool {
        let Some(id) = self.id_of(window) else {
            return self.tree.is_some();
        };
        self.windows.remove(&id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        match self.tree.as_mut().map(|t| t.remove_leaf(id)) {
            Some(LeafRemoval::WasRoot) | None => {
                self.tree = None;
                false
            }
            Some(_) => {
                // Focus lands on whatever is left, so the next window has
                // something to split.
                if self.focus.is_none() {
                    self.focus = self.tree.as_ref().and_then(|t| t.panes().first().copied());
                }
                true
            }
        }
    }

    /// Every window and the rectangle it should occupy, in `bounds`.
    ///
    /// `bounds` is in PIXELS. kukaku's `Rect` is unitless `u16`, which is what
    /// lets one algebra serve an 80x24 grid and a 1920x1080 panel — the unit
    /// lives at the call site, here, and nowhere inside the tree.
    #[must_use]
    pub fn arrange(&self, bounds: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        // Saturating rather than `as`: a negative or oversized logical rect is
        // a bug elsewhere, and `as u16` would wrap it into a plausible-looking
        // small rectangle instead of clamping to something visible.
        let to_u16 = |v: i32| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
        let b = Rect::new(
            to_u16(bounds.loc.x),
            to_u16(bounds.loc.y),
            to_u16(bounds.size.w),
            to_u16(bounds.size.h),
        );
        tree.compute_rects(b)
            .into_iter()
            .filter_map(|(id, r)| {
                let w = self.windows.get(&id)?.clone();
                Some((
                    w,
                    Rectangle::new(
                        (i32::from(r.x), i32::from(r.y)).into(),
                        (i32::from(r.w), i32::from(r.h)).into(),
                    ),
                ))
            })
            .collect()
    }

    /// Move focus to the neighbouring window in `direction`.
    ///
    /// Returns the newly focused window, or `None` if there is nothing that
    /// way — which is a finding, not a failure: the operator pressed a
    /// direction at the edge of the screen.
    pub fn focus_direction(
        &mut self,
        direction: Direction,
        bounds: Rectangle<i32, Logical>,
    ) -> Option<Window> {
        let tree = self.tree.as_ref()?;
        let from = self.focus?;
        let to_u16 = |v: i32| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
        let b = Rect::new(0, 0, to_u16(bounds.size.w), to_u16(bounds.size.h));
        let next = tree.neighbor(from, direction, b)?;
        self.focus = Some(next);
        self.windows.get(&next).cloned()
    }

    /// Move the divider governing the focused window.
    pub fn resize_focused(&mut self, direction: Direction, delta: f32) -> bool {
        let (Some(tree), Some(f)) = (self.tree.as_mut(), self.focus) else {
            return false;
        };
        tree.resize_leaf(f, direction, delta)
    }

    /// Point focus at the window under the pointer / just clicked.
    pub fn focus_window(&mut self, window: &Window) {
        if let Some(id) = self.id_of(window) {
            self.focus = Some(id);
        }
    }

    /// The focused window, if any.
    #[must_use]
    pub fn focused(&self) -> Option<Window> {
        self.focus.and_then(|f| self.windows.get(&f).cloned())
    }

    /// How many windows the tree holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tree.as_ref().map_or(0, LayoutNode::pane_count)
    }

    /// Whether the tree is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tree.is_none()
    }

    fn id_of(&self, window: &Window) -> Option<WindowId> {
        self.windows.iter().find_map(|(id, w)| (w == window).then_some(*id))
    }

    /// Depth of a leaf, used only to alternate split orientation.
    fn depth_of(&self, tree: &LayoutNode<WindowId>, target: WindowId) -> usize {
        fn walk(n: &LayoutNode<WindowId>, target: WindowId, d: usize) -> Option<usize> {
            match n {
                LayoutNode::Leaf { pane } => (*pane == target).then_some(d),
                LayoutNode::Split { a, b, .. } => {
                    walk(a, target, d + 1).or_else(|| walk(b, target, d + 1))
                }
            }
        }
        walk(tree, target, 0).unwrap_or(0)
    }
}

// ── ★ THE COMPOSITOR SIDE: TURN THE TREE INTO POSITIONS AND CONFIGURES ───

impl crate::state::Omoya {
    /// Re-place every window according to the layout tree.
    ///
    /// ★ TWO HALVES, AND ONLY ONE OF THEM IS OBVIOUS. Moving the element in
    /// the `Space` decides where the compositor DRAWS it. Sending the size in
    /// an xdg configure is what decides how big the client RENDERS itself, and
    /// without it every window paints at whatever size it chose and then gets
    /// drawn at a position that assumes otherwise — overlapping content inside
    /// non-overlapping rectangles, which looks like a compositing bug rather
    /// than a missing message.
    ///
    /// Idempotent by construction: it reads the tree and writes positions, so
    /// calling it after any map, unmap or resize is always correct and never
    /// accumulates.
    pub fn apply_layout(&mut self) {
        // One output today. `outputs()` is the honest source rather than a
        // stored size, because the output can change mode under us and a
        // cached extent would tile into a screen that no longer exists.
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };

        for (window, rect) in self.tiling.arrange(geo) {
            if let Some(t) = window.toplevel() {
                t.with_pending_state(|state| {
                    state.size = Some(rect.size);
                });
                // `send_pending_configure` and not `send_configure`: the
                // former is a no-op when nothing changed, so a layout pass
                // over a settled screen sends no messages at all. Calling the
                // unconditional form here would configure every window on
                // every map and make clients redraw for nothing — which,
                // with damage tracking now live, would be the one thing that
                // reliably defeats it.
                t.send_pending_configure();
            }
            self.space.map_element(window, rect.loc, false);
        }
    }
}
