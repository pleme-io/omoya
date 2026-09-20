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

use crate::placement::Placement;
use kukaku::{Direction, LayoutNode, LeafRemoval, Rect, SplitOrientation};
use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};

/// The space left between tiled windows, and between a window and the screen
/// edge, in logical pixels.
///
/// ★ NOT DECORATION — it is what makes a border VISIBLE and a tiling layout
/// legible. With windows flush against each other there is nowhere to draw a
/// focus indicator and no visual seam, so a two-window split reads as one
/// confusing surface.
///
/// ★ 4, NOT 8, AND THE REASON IS THAT GAPS COMPOUND. This is a per-window
/// inset, so the space BETWEEN two adjacent windows is 2×GAP. At 8 that was
/// 16 px of empty ground down the middle of a 1080p screen — the single
/// loudest "this is a rice" tell, and the one that reads as wasted panel
/// rather than as breathing room. At 4 the seam is 8 px: unmistakable, and
/// quiet.
///
/// The floor is set by the border, not by taste: `GAP >= BORDER * 2`, or two
/// focused neighbours' rings would touch. 4 sits exactly on that floor, which
/// is why it is the smallest honest value and not merely a smaller one.
///
/// Part of `shitsurai` (設え), the seat's visual design — see
/// `docs/SHITSURAI.md`. Distances there come from a 4 px grid; a 7 or a 13
/// ── ★ RAISED TO 12 ON 2026-09-03 AND REVERTED THE SAME DAY ─────────────
/// An agent (me) raised this to 12 to answer "the look and feel is absolutely
/// just bad", citing an external ricing guide, without having read the
/// paragraph directly above — which had already considered and rejected
/// exactly that direction, with the reason.
///
/// The complaint was real; the diagnosis was wrong. Measured afterwards: the
/// desktop ground, every mado window and tobira all paint the byte-identical
/// #2E3440, a 1.00:1 contrast collision. Nothing on screen has an edge, and
/// no amount of empty space between two surfaces of the same colour creates
/// one. SwayFX and niri both ship with every effect OFF and read well on a
/// gap plus a coloured focus ring — the gap was never the variable.
///
/// See docs/LOOK-AND-FEEL-PLAN.md. If this value ever moves it moves as a
/// `(defface)`-backed token with a consumer, not as a constant someone edits.
pub const GAP: i32 = 4;

/// How thick the focused window's border is.
///
/// Drawn in the GAP, so it costs no window area — a border that shrank the
/// content would make focusing a window resize it, which is worse than
/// having no border.
pub const BORDER: i32 = 2;

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
        let id = self.map_id();
        self.windows.insert(id, window);
        id
    }

    /// Does the layout tree hold `w`?
    ///
    /// ★ THE MEASUREMENT `ToplevelRow.tiled` NEEDED AND DID NOT HAVE. The
    /// mapping already exists — `windows` is `WindowId -> Window` and the tree
    /// is keyed by the same ids — so this is a lookup, not a walk, and there
    /// was never a reason for the row to carry a literal instead.
    #[must_use]
    pub fn holds(&self, w: &Window) -> bool {
        self.windows
            .iter()
            .any(|(id, held)| held == w && self.tree.as_ref().is_some_and(|t| t.contains_pane(*id)))
    }

    /// The tree half of [`Self::map`], with no `Window` in sight.
    ///
    /// ★ SPLIT OUT SO THE LAYOUT CAN BE TESTED AT ALL. `Window` needs a live
    /// `ToplevelSurface`, which needs a client, which needs a display — so a
    /// unit test of the tree was impossible while the only entry point took
    /// one. That is why the first tiling defect had to be chased through a VM
    /// screenshot: there was no cheaper place to ask the question.
    pub fn map_id(&mut self) -> WindowId {
        let id = WindowId(self.next);
        self.next += 1;

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
                    None => {
                        LayoutNode::split(SplitOrientation::Vertical, tree, LayoutNode::leaf(id))
                    }
                }
            }
        });
        self.focus = Some(id);
        id
    }

    /// The rectangles the tree assigns, by id — [`Self::arrange`] without the
    /// `Window` lookup. The half that is pure geometry, and therefore the
    /// half worth testing.
    #[must_use]
    pub fn arrange_ids(&self, bounds: Rect) -> Vec<(WindowId, Rect)> {
        self.tree
            .as_ref()
            .map(|t| t.compute_rects(bounds))
            .unwrap_or_default()
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
    pub fn arrange(
        &self,
        bounds: Rectangle<i32, Logical>,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
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
        self.arrange_ids(b)
            .into_iter()
            .filter_map(|(id, r)| {
                let w = self.windows.get(&id)?.clone();
                // Inset by the gap. Done HERE rather than inside kukaku on
                // purpose: a gap is a compositor's aesthetic choice, not a
                // property of partitioning a space, and putting it in the
                // algebra would mean every consumer inherits one seat's taste
                // — and that `compute_rects` no longer tiles its bounds
                // exactly, which its own test asserts.
                let (x, y) = (i32::from(r.x) + GAP, i32::from(r.y) + GAP);
                // `max(1)`: a parcel narrower than two gaps would go negative
                // and wrap. One pixel is degenerate but representable; a
                // wrapped u32 is a window the size of the universe.
                let (w_px, h_px) = (
                    (i32::from(r.w) - GAP * 2).max(1),
                    (i32::from(r.h) - GAP * 2).max(1),
                );
                Some((w, Rectangle::new((x, y).into(), (w_px, h_px).into())))
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
        self.windows
            .iter()
            .find_map(|(id, w)| (w == window).then_some(*id))
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
    /// Map `w` so its FRAME (titlebar included) is `frame`.
    ///
    /// ★ CONSTRAIN THE FRAME, NOT THE CONTENT. Maximise used to map the
    /// CONTENT at the zone's origin and skip the `content_for` shrink, so the
    /// titlebar — drawn ABOVE the content — landed under the status bar and
    /// the focus ring at x = -2. One helper for maximise and every snap tile,
    /// so that fix cannot be lost by the next placement that needs it.
    pub fn place_frame(
        &mut self,
        w: &smithay::desktop::Window,
        frame: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) {
        let content = crate::chrome::content_for(frame);
        let content = if content.size.is_empty() {
            frame
        } else {
            content
        };
        if let Some(t) = w.toplevel() {
            t.with_pending_state(|st| st.size = Some(content.size));
            t.send_pending_configure();
        }
        // ★ ACTIVATE ONLY THE FOCUSED WINDOW. `map_element(.., true)` RAISES,
        // so activating every window a layout pass touches rewrote the whole
        // stacking order — silently undoing click-to-raise on the next pass.
        // Focus is granted by `focus_window` / `new_toplevel`, which raise on
        // their own; the layout only places.
        let activate = self.focused_window().as_ref() == Some(w);
        self.space.map_element(w.clone(), content.loc, activate);
    }

    /// Release every standing claim on `w`'s geometry, so a rectangle the
    /// caller is about to write is the one that survives the next
    /// `apply_layout`.
    ///
    /// ── ★ WHY THIS IS ONE FUNCTION (2026-09-19) ─────────────────────────
    /// A window's position can be claimed from three places, and
    /// `apply_layout` resolves them by PRIORITY: `Maximized` beats a snap
    /// tile beats the remembered float position. That order is right, and it
    /// means any site about to write an explicit rectangle must first drop
    /// the claims above it — otherwise the write is overruled on the very
    /// next layout pass.
    ///
    /// Four sites write explicit geometry. **One of them knew.**
    ///
    /// | site | tile | maximised |
    /// |---|---|---|
    /// | `MoveGrab::motion` (drag) | freed | freed |
    /// | `ResizeGrab::motion` (edge drag) | freed | **kept** |
    /// | `Deed::MoveFloat` / keyboard resize | freed | **kept** |
    /// | `Deed::Snap` (Logo+Arrow) | **kept** | **kept** |
    ///
    /// Each miss is a silent no-op that reports `Performed`, and the resize
    /// one is worse than silent: the window visibly follows the pointer for
    /// the whole drag and then jumps back at the next map, unmap or commit,
    /// which reads as a compositing glitch rather than as an ignored verb.
    ///
    /// Having the drag path get it right is what makes this an oversight
    /// rather than a policy — so the answer is one function every site calls,
    /// not three repaired call sites free to diverge again.
    pub fn release_geometry_claims(&mut self, w: &smithay::desktop::Window) {
        crate::snap::set(w, None);
        if let Some(id) = surface_id_of(w) {
            self.windows.unmaximize(id);
        }
    }

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
        // ★ MARKED HERE, NOT AT THE FOUR CALL SITES. Every geometry change —
        // a toplevel mapping or dying, a layer surface arriving or leaving,
        // a re-tile — funnels through this one function, so this is the place
        // that cannot be forgotten by whoever adds the fifth caller.
        //
        // Marked BEFORE the early return below: an `apply_layout` with no
        // output still means the window set changed, and the frame is owed as
        // soon as an output exists. Returning without marking would lose it.
        self.owed.mark(crate::owed::Owed::Windows);

        // One output today. `outputs()` is the honest source rather than a
        // stored size, because the output can change mode under us and a
        // cached extent would tile into a screen that no longer exists.
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };

        // ★ TILE INSIDE THE NON-EXCLUSIVE ZONE, NOT THE WHOLE OUTPUT.
        //
        // A layer surface that anchors to an edge and asks for an exclusive
        // zone — a status bar — is asking the compositor NOT to place windows
        // there. `LayerMap::arrange` computes what is left; using the raw
        // output geometry instead would tile a window under the bar, where it
        // is permanently half-hidden. That looks like a z-order bug and is
        // actually a geometry one.
        //
        // With no layer surfaces this is exactly the output rectangle, so the
        // bar-less case is unchanged rather than special-cased.
        let usable = {
            let mut map = smithay::desktop::layer_map_for_output(&output);
            map.arrange();
            let zone = map.non_exclusive_zone();
            // omoya's own bar reserves its strip the same way a layer surface
            // would — by shrinking the zone before the tiler sees it, rather
            // than by the tiler knowing a bar exists. That keeps one rule:
            // windows fill whatever is left.
            let zone = smithay::utils::Rectangle::new(
                // ★ FROM CONFIG, not the const. `bar::HEIGHT` is still the
                // DEFAULT — `BarConfig::default()` derives from it — but an
                // operator who sets `bar.height` must see the tiler move, or
                // the field is decoration.
                (zone.loc.x, zone.loc.y + self.config.bar.height).into(),
                (zone.size.w, (zone.size.h - self.config.bar.height).max(1)).into(),
            );
            // `non_exclusive_zone` is relative to the output; `arrange` wants
            // absolute coordinates, and on a single output at (0,0) those
            // coincide — offset explicitly so a future second output does not
            // inherit a silent assumption.
            smithay::utils::Rectangle::new(
                (geo.loc.x + zone.loc.x, geo.loc.y + zone.loc.y).into(),
                zone.size,
            )
        };

        // ── ★ FLOAT WHAT SHOULD FLOAT, RE-DERIVED EACH PASS ──────────────
        //
        // `app_id` arrives in a request AFTER the toplevel exists, so a
        // decision made once at `new_toplevel` sees `None` and tiles the
        // launcher. Re-deriving here is idempotent and self-corrects the
        // moment the identity lands — see `crate::placement`.
        // Walk once, publish what we saw, then filter on it — so
        // `window_app_ids` is BY CONSTRUCTION the value the rule matched on
        // rather than a second lookup that could disagree.
        // ★ FROM THE ROSTER, NOT THE SPACE — the layout must not read back
        // the thing it writes. Building this from `space.elements()` made
        // `Placement::Hidden` a one-way door: the hidden arm unmaps, so the
        // next pass could not see the window and never re-mapped it, and
        // `RestoreLast` had nothing to restore.
        //
        // It also stabilises the cascade. `idx` below is this list's order:
        // from the Space that was Z-ORDER, so raising or closing a window
        // renumbered the ones above it and physically moved them. Roster
        // order is the order windows APPEARED, which nothing reorders.
        let seen: Vec<(smithay::desktop::Window, Option<String>)> = self
            .roster
            .iter()
            .map(|w| (w.clone(), app_id_of(w)))
            .collect();
        *self
            .introspect
            .window_app_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = seen.iter().map(|(_, id)| id.clone()).collect();

        // ── ★ THE JOINED TABLE, BUILT FROM THE SAME WALK ────────────────────
        //
        // Built here rather than beside `geometry` in drm.rs on purpose: this
        // walk is per-WINDOW, while drm.rs walks RENDER ELEMENTS and therefore
        // counts the bar and four focus-ring edges among "windows". Sharing the
        // walk is what makes the row's app_id and rect refer to the same thing
        // -- the property the three legacy lists never had.
        // ★ THE MODE DECIDES FIRST, THE PER-APP LIST SECOND.
        //
        // In `Floating` every window floats and `floating_app_ids` becomes
        // redundant rather than ignored — a listed app still floats, it just
        // no longer needs listing. In `Tiling` the list is the only thing that
        // floats, which is the behaviour the seat has always had.
        //
        // Written as `mode == Floating || per_app` rather than as a match with
        // two arms, because the per-app rule must keep applying in BOTH modes:
        // an `if/else` here is how a launcher silently stops floating the day
        // someone adds a third mode.
        let floating_mode = self.config.layout.mode == crate::config::LayoutMode::Floating;
        // ★ Published from the point of DECISION, so the leaf reports the mode
        // the arrangement actually used rather than a re-read of config that
        // could drift from it.
        *self
            .introspect
            .layout_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            if floating_mode { "floating" } else { "tiling" }.to_owned();
        // Read once: `map_element` below activates exactly this window.
        let focused_window = self.focused_window();
        let floats: Vec<smithay::desktop::Window> = seen
            .iter()
            .filter(|(_, id)| {
                floating_mode
                    || crate::placement::for_app_id_in(id.as_deref(), &self.config.placement)
                        .is_floating()
            })
            .map(|(w, _)| w.clone())
            .collect();
        // Record what this pass decided, so `commit` can notice when a
        // late-arriving `app_id` changes the answer. See `Omoya::floating_ids`.
        self.floating_ids = floats.iter().filter_map(surface_id_of).collect();
        for w in &floats {
            // Idempotent: `unmap` returns false for a window the tree does not
            // hold, so a launcher that is already floating costs a lookup.
            self.tiling.unmap(w);
        }

        // Published from the layout pass, which is the one place that has
        // just consulted `windows` for every window — so the leaf reports the
        // state the placement actually used rather than a second read.
        self.introspect.minimized_count.store(
            self.windows.minimized_count() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        *self
            .introspect
            .tab_groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self
            .windows
            .groups()
            .iter()
            .map(|g| g.members.clone())
            .collect();

        let arranged = self.tiling.arrange(usable);
        // Publish what the TREE asked for, before anything is applied. See
        // `OmoyaIntrospect::layout` — a screenshot says where windows ended
        // up and cannot say what was requested, so when the two disagree
        // there is otherwise no way to tell a broken split from a broken
        // placement from an early return above.
        for (window, rect) in &arranged {
            // ★ THE WHOLE PLACEMENT IS HONOURED HERE, NOT JUST `Hidden`.
            //
            // This loop once asked only about `Hidden`, so in
            // `LayoutMode::Tiling` — the DEFAULT — minimise was a silent
            // no-op. The identical hole was left open one arm along:
            // MAXIMISE. `windowmode` recorded `Maximized`, `Deed::Maximize`
            // answered `Performed`, `mode_of` read back `Maximized`, and the
            // window stayed in its tree rect — a verb that reports success,
            // measures as applied, and does nothing to the screen.
            //
            // Matched exhaustively rather than re-tested, so a third arm is
            // a compile error here and not a third silent no-op.
            match surface_id_of(window)
                .map(|id| self.windows.placement_of(id))
                .unwrap_or(crate::windowmode::Placement::AsLaidOut)
            {
                crate::windowmode::Placement::Hidden => {
                    self.space.unmap_elem(window);
                    continue;
                }
                // The usable zone, exactly as the floating arm does it — one
                // meaning of "maximised" for both modes.
                crate::windowmode::Placement::Maximized => {
                    self.place_frame(window, usable);
                    continue;
                }
                crate::windowmode::Placement::AsLaidOut => {}
            }
            if let Some(t) = window.toplevel() {
                t.with_pending_state(|state| {
                    state.size = Some(rect.size);
                });
                // `send_pending_configure` and not `send_configure`: the
                // former is a no-op when nothing changed, so a layout pass
                // over a settled screen sends no messages at all. Calling the
                // unconditional form here would configure every window on
                // every map and make clients redraw for nothing — which,
                // with damage tracking live, would be the one thing that
                // reliably defeats it.
                t.send_pending_configure();
            }
            self.space.map_element(window.clone(), rect.loc, false);
        }

        // ★ MAPPED LAST, SO THEY ARE ON TOP. `Space` stacks in map order, and
        // an overlay behind the windows it overlays is worse than no overlay:
        // it takes the keyboard while showing nothing.
        for (idx, w) in floats.iter().enumerate() {
            // ── ★ THE ONE PLACE THE FOUR CONTROLS TAKE EFFECT ─────────────
            //
            // minimise, maximise and tabs are all "where does this window go,
            // or is it hidden", so they resolve here rather than in three
            // places that could disagree. `windowmode` owns the decision and
            // is tested without a seat; this loop only obeys it.
            let wm_id = surface_id_of(w);
            let placement = wm_id
                .map(|id| self.windows.placement_of(id))
                .unwrap_or(crate::windowmode::Placement::AsLaidOut);
            match placement {
                // Mapped to nothing: minimised, or a tab behind its sibling.
                // The client keeps running and keeps its buffer, so restoring
                // costs a placement and not a relaunch.
                crate::windowmode::Placement::Hidden => {
                    self.space.unmap_elem(w);
                    continue;
                }
                crate::windowmode::Placement::Maximized => {
                    self.place_frame(w, usable);
                    continue;
                }
                crate::windowmode::Placement::AsLaidOut => {}
            }
            // ── ★ A SNAPPED WINDOW TAKES ITS TILE ────────────────────────
            // Checked after `Placement` so minimise and maximise still win,
            // and only in floating mode: a tile is a floating-window idea,
            // and in tiling mode the tree owns every rect.
            if floating_mode {
                if let Some(tile) = crate::snap::tile_of(w) {
                    self.place_frame(w, crate::snap::frame_for(tile, usable));
                    continue;
                }
            }
            // ★ IN FLOATING MODE THE SIZE COMES FROM CONFIG, NOT FROM THE
            // PER-APP RULE. `for_app_id_in` returns `Tiled` for an unlisted
            // app, and this loop used to `continue` on that — correct when
            // the only floaters were listed apps, and a seat that maps
            // NOTHING once the mode makes every window a floater. The window
            // would be unmapped from the tiling tree and then skipped here.
            let placement =
                crate::placement::for_app_id_in(app_id_of(w).as_deref(), &self.config.placement);
            let (width, height) = match placement {
                Placement::Floating { width, height } => (width, height),
                Placement::Tiled if floating_mode => (
                    self.config.placement.float_width,
                    self.config.placement.float_height,
                ),
                Placement::Tiled => continue,
            };
            // ── ★ AN OVERLAY IS CENTRED IN EVERY MODE ────────────────────
            //
            // The distinction already exists in the type and was being
            // thrown away one line up: `Placement::Floating` means "this app
            // is an overlay BY ITS OWN NATURE" (it is in FLOATING_APP_IDS —
            // tobira, the launcher), while `Tiled if floating_mode` means
            // "this window floats only because the MODE makes everything
            // float". Those want opposite placements and were getting the
            // same one.
            //
            // The `else` branch below already says so — "a launcher summoned
            // over a tiled desktop is still centred… cascading it would move
            // it every time" — and then floating mode cascaded it anyway. The
            // operator saw it exactly: "the ctrl-space isn't a nice little
            // centered place, it is another stacked window."
            let is_overlay = matches!(placement, Placement::Floating { .. });
            // Cascade so successive windows are individually reachable, then
            // snap so one nudged toward an edge sits flush with it. Snap
            // AFTER cascade: the cascade decides where the window wants to be
            // and the snap only tidies that answer, whereas snapping first
            // would be immediately overwritten by the offset.
            let rect = if floating_mode && !is_overlay {
                // ── ★ PLACED ONCE, THEN REMEMBERED ───────────────────────
                //
                // This used to derive the position from `idx` on EVERY pass,
                // which meant three things at once. A drag was overwritten by
                // the next `apply_layout` (twelve call sites), so the operator
                // could move a window and watch it snap back on the next
                // click. `idx` is Z-ORDER, so raising or closing a window
                // renumbered the ones above it and physically moved them. And
                // the cascade was the only thing separating windows that all
                // share `float_width`/`float_height` — 24 px against 883x523
                // on plo, a 93% overlap that reads as a stack.
                //
                // Now the cascade decides only where a window STARTS. See
                // `crate::floatpos`; the position lives on the window itself,
                // which needs no id at all. (This note used to justify that
                // with "`surface_id_of` returns a per-client `protocol_id`
                // and every mado on the seat is id 16" — true of
                // `protocol_id`, and false since `winid::of` replaced it with
                // a minted counter. The storage choice still stands on its
                // own; the reason given for it had rotted.)
                // The cascade is computed either way — it is a few
                // multiplications, and it is what supplies the SIZE even when
                // the position is recalled. `width`/`height` are fractions of
                // the zone, so the rect is the only place they become pixels.
                let cascade = crate::placement::cascaded(
                    usable,
                    width,
                    height,
                    idx,
                    self.config.layout.cascade_step,
                );
                // A window the operator has sized keeps ITS size; the config
                // fractions are only the size a window starts at.
                let cascade = match crate::floatpos::recall_size(w) {
                    Some(size) => smithay::utils::Rectangle::new(
                        cascade.loc,
                        (size.w.min(usable.size.w), size.h.min(usable.size.h)).into(),
                    ),
                    None => cascade,
                };
                let first = crate::placement::snap_to_edges(
                    cascade,
                    usable,
                    self.config.layout.snap_threshold,
                );
                match crate::floatpos::recall(w) {
                    Some(remembered) => smithay::utils::Rectangle::new(
                        crate::floatpos::clamped(remembered, first.size, usable),
                        first.size,
                    ),
                    None => {
                        crate::floatpos::remember(w, first.loc);
                        first
                    }
                }
            } else {
                // A launcher is centred whatever the mode: it is a transient
                // overlay, not a member of an arrangement, and cascading it
                // would move it every time it is summoned.
                crate::placement::centred(usable, width, height)
            };
            // ── ★ A FIXED-SIZE CLIENT KEEPS ITS SIZE; WE ONLY POSITION IT ──
            //
            // `min_size == max_size` is how "not resizable" is spelled on the
            // wire, so configuring a different size for such a window is the
            // compositor overruling the protocol. The seat still owns WHERE it
            // goes — position is the compositor's job and size is not always.
            //
            // Re-centred (or re-cascaded) at the client's OWN dimensions, so a
            // small launcher lands where a small launcher should rather than
            // in the top-left of the box the seat would have given it.
            let rect = match client_fixed_size(w) {
                // ★ THE CLIENT OWNS THE SIZE. THE OPERATOR STILL OWNS THE
                // POSITION — and this arm used to take both.
                //
                // It ran AFTER the recall/remember block above and discarded
                // its answer, so a fixed-size window (`min_size == max_size`:
                // a dialog, a splash, a utility panel) was RE-CENTRED on every
                // layout pass. Drag it, and the next map, unmap, commit or
                // click put it back in the middle — the operator's original
                // report, *"I can move it but if I click on it they snap back
                // to their original position"*, still true for this one class
                // of client after `floatpos` closed it for every other.
                //
                // Only the size comes from the client now; where it goes is
                // recalled exactly as any other floating window's is.
                Some(fixed) if floating_mode && !is_overlay => {
                    let loc = match crate::floatpos::recall(w) {
                        Some(remembered) => crate::floatpos::clamped(remembered, fixed, usable),
                        None => {
                            // First sight: centre it at its OWN size, tidy it
                            // against the edges, and record that — so the very
                            // next pass recalls instead of recomputing.
                            let first = crate::placement::snap_to_edges(
                                Rectangle::new(crate::placement::centred_loc(usable, fixed), fixed),
                                usable,
                                self.config.layout.snap_threshold,
                            );
                            crate::floatpos::remember(w, first.loc);
                            first.loc
                        }
                    };
                    Rectangle::new(loc, fixed)
                }
                // An overlay, or tiling mode: centred at the client's own size
                // every time. For the overlay that is its role (`centred:
                // true`) and not an oversight — it is summoned, not arranged.
                Some(fixed) => Rectangle::new(crate::placement::centred_loc(usable, fixed), fixed),
                None => rect,
            };
            // ── ★ ROOM FOR THE TITLEBAR ──────────────────────────────────
            //
            // The chrome sits ABOVE the content, so the content rect is the
            // laid-out frame minus the bar. Computed by `chrome::content_for`
            // rather than here, because the renderer expands it back with
            // `chrome::bar_rect` and two independent copies of that
            // arithmetic drift by a few pixels every configure.
            //
            // Floating only: a tiled window's chrome is a later question, and
            // shrinking tiled windows here would move every one of them for a
            // feature the tiled path does not yet draw.
            // ★ ONLY A DECORATED WINDOW LOSES A BAND TO ITS BAR. An overlay
            // has no titlebar, so shrinking its frame by `chrome::HEIGHT` put
            // its content 24 px below where the seat thought it was — which is
            // why the launcher's bar rendered DETACHED above its panel.
            let role_policy = crate::role::policy_of(w, &self.config.placement);
            let rect = if floating_mode
                && role_policy.is_decorated()
                && !crate::chrome::content_for(rect).size.is_empty()
            {
                crate::chrome::content_for(rect)
            } else {
                rect
            };
            if let Some(t) = w.toplevel() {
                // A fixed-size client is sent NO size: `None` means "you
                // choose", which for a window whose min and max agree is the
                // only answer that is not a contradiction.
                // ★ AN OVERLAY SIZES ITSELF. `sized_by_compositor = false`
                // means the seat places it and says nothing about how big it
                // is; configuring a size would overrule a launcher that
                // shrinks to its own content.
                let send = (role_policy.sized_by_compositor && client_fixed_size(w).is_none())
                    .then_some(rect.size);
                t.with_pending_state(|state| {
                    state.size = send;
                });
                t.send_pending_configure();
            }
            // ★ ONLY THE FOCUSED WINDOW IS ACTIVATED. This passed `true` for
            // every window, and `map_element(.., true)` RAISES — so every
            // layout pass re-stacked the seat in roster order and undid
            // click-to-raise. A new toplevel is raised by `new_toplevel` and
            // focus by `focus_window`, so nothing here needs to.
            let activate = focused_window.as_ref() == Some(w);
            self.space.map_element(w.clone(), rect.loc, activate);
        }

        // ── ★ PUBLISHED AFTER THE PASS, NOT BEFORE IT (2026-09-03) ────
        //
        // This block used to sit at the TOP of `apply_layout`, before the
        // loops below apply any placement — so every rect it reported was
        // one full pass old. Measured immediately after a maximize deed:
        // `toplevels` still said [518,304,883,523] while `geometry` in the
        // same snapshot correctly said 0,28. It is the field a placement
        // bug is diagnosed from, and it was contradicting its neighbour.
        {
            use smithay::reexports::wayland_server::Resource as _;
            let focused = self
                .introspect
                .focus_rect
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let sent = self
                .introspect
                .decoration_sent
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // ★ ASK THE SEAT. This scanned `space.elements()` for a window
            // whose CURRENT geometry equals the PREVIOUS pass's `focus_rect`,
            // which is wrong twice over: a rectangle is not an identity (two
            // maximised windows share one exactly), and `find` takes the first
            // match, which is the BACKMOST since `elements()` yields
            // back-to-front. The row comparison below was already fixed to go
            // by identity — and was handed an id produced by a rectangle
            // match, so the defect simply moved up one line.
            //
            // `focused_surface_id` reads the keyboard's current focus, which
            // is the fact itself rather than a projection of it.
            let focused_id = self.focused_surface_id();
            let rows: Vec<crate::introspect::ToplevelRow> = seen
                .iter()
                .enumerate()
                .map(|(i, (w, app))| {
                    let rect = self
                        .space
                        .element_geometry(w)
                        .map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h));
                    let key = w.toplevel().map(|t| format!("{:?}", t.wl_surface().id()));
                    // ★ FOCUS BY IDENTITY, NOT BY RECTANGLE.
                    //
                    // This compared the window's rect against the focused
                    // rect, so two windows sharing a rect BOTH reported
                    // `focused: true` — observed live, and easy to reach
                    // because every floating window gets the same
                    // float_width/float_height. A window id answers the
                    // question that was being asked.
                    let is_focused = crate::layout::surface_id_of(w)
                        .zip(focused_id)
                        .is_some_and(|(a, b)| a == b);
                    crate::introspect::ToplevelRow {
                        id: i as u64,
                        app_id: app.clone(),
                        decoration_mode_sent: key.and_then(|k| sent.get(&k).cloned()),
                        rect,
                        // ★ NAMED FOR WHAT IT COUNTS. This is the four
                        // focus-RING edges, drawn only for the focused window.
                        // It was called `decoration_elements_drawn`, which
                        // reads as "all chrome" — so `chrome_verdict` reported
                        // "none drawn" for every unfocused window whose
                        // titlebar was demonstrably on screen, and pointed the
                        // investigation at the renderer, which was correct.
                        // The titlebar is counted separately below.
                        decoration_elements_drawn: u32::from(is_focused) * 4,
                        // ★ WHAT THE RENDERER DREW, not what this pass thinks
                        // it should have. `chrome_verdict` exists to catch
                        // "told ServerSide and nothing appeared"; deriving it
                        // here would make it agree with itself and witness
                        // nothing.
                        titlebar_drawn: crate::layout::surface_id_of(w).is_some_and(|id| {
                            self.introspect
                                .chrome_drawn
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .contains(&id)
                        }),
                        focused: is_focused,
                        // ★ MEASURED, NOT ASSERTED. This was the literal
                        // `false` for every row, while the field is documented
                        // as "whether the layout tree holds this window" — so
                        // a seat in `LayoutMode::Tiling` (the DEFAULT) reported
                        // `"floating": false` alongside `"tiled": false` for
                        // every window, a payload contradicting itself. An
                        // agent diagnosing why a resize deed answered "no tiled
                        // window" reads this and concludes the tree is empty.
                        tiled: crate::layout::surface_id_of(w)
                            .is_some_and(|_| self.tiling.holds(w)),
                    }
                })
                .collect();
            drop(sent);
            drop(focused);
            *self
                .introspect
                .toplevels
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = rows;
        }

        // WHICH window has focus, published beside WHERE it is. The renderers
        // need both and only the rect was published, so each of them derived
        // the identity by comparing rectangles — see `focused_id`'s doc.
        self.introspect.focused_id.store(
            u64::from(self.focused_surface_id().unwrap_or(0)),
            std::sync::atomic::Ordering::Relaxed,
        );

        // Where focus is, for the border the render loop draws and for anyone
        // who asks. Published from here because this is where geometry is
        // decided; deriving it in the render loop would be a second source.
        *self
            .introspect
            .focus_rect
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = {
            // ── ★ THE TILING TREE CANNOT ANSWER FOR A FLOATING WINDOW ────
            //
            // This read `tiling.focused()` and then searched `arranged` — the
            // TILED arrangement. Both halves fail in `LayoutMode::Floating`:
            // every float is `unmap`ped from the tree, so the tree has no
            // focus, and `arranged` is empty because nothing is tiled.
            //
            // The consequence was not subtle. `focus_rect` drives BOTH the
            // focus ring in `drm.rs` AND the bar's parcel indicator, so a
            // floating seat drew no ring at all — and since a mado window's
            // background is nord0 and the desktop ground is nord0, a floating
            // window had NO visual boundary whatsoever. Measured on plo
            // 2026-08-28: `focus_rect: "none"` with one window plainly on
            // screen at `518,280 883x547`. The operator's report was "I don't
            // see any floating screens", and they were right — the window was
            // there and nothing distinguished it from the desktop.
            //
            // So: ask the tree, and if it has no answer ask the SPACE, which
            // holds tiled and floating windows alike. `element_geometry` is
            // the position actually mapped, so the ring lands where the
            // window is rather than where the tiler wished it were.
            let tiled = self.tiling.focused().and_then(|f| {
                arranged
                    .iter()
                    .find(|(w, _)| *w == f)
                    .map(|(_, r)| (r.loc.x, r.loc.y, r.size.w, r.size.h))
            });
            tiled.or_else(|| {
                // ★ THE FOCUSED WINDOW, ASKED DIRECTLY. This took
                // `floats.last()` on the reasoning that the last-mapped float
                // holds focus — true only while every float was activated as
                // it was placed, which was itself the re-stacking defect above.
                // It named the NEWEST window, so the focus ring sat on the
                // wrong one whenever focus was not the most recent map.
                let w = focused_window.as_ref()?;
                let g = self.space.element_geometry(w)?;
                Some((g.loc.x, g.loc.y, g.size.w, g.size.h))
            })
        };

        // ★ PUBLISH WHAT `Space` HOLDS, NOT WHAT THE TREE ASKED FOR — read
        // back AFTER the writes.
        //
        // The first version published `arranged` alone, which reported
        // `0,0 512x768 | 512,0 512x768` while only one window was visible.
        // That is the tree's REQUEST, and a request that is correct proves
        // nothing about whether it was applied: `map_element` could be
        // repositioning nothing at all and this leaf would look identical.
        // Reading the position back turns the leaf from a restatement of the
        // input into a measurement of the result.
        *self
            .introspect
            .layout
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = arranged
            .iter()
            .map(|(w, r)| {
                let live = self.space.element_location(w);
                match live {
                    Some(p) if p == r.loc => {
                        format!("{},{} {}x{}", r.loc.x, r.loc.y, r.size.w, r.size.h)
                    }
                    Some(p) => format!(
                        "asked {},{} {}x{} BUT SPACE HAS {},{}",
                        r.loc.x, r.loc.y, r.size.w, r.size.h, p.x, p.y
                    ),
                    None => format!(
                        "asked {},{} {}x{} BUT NOT IN SPACE",
                        r.loc.x, r.loc.y, r.size.w, r.size.h
                    ),
                }
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen the vkms gate runs at.
    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 1024,
        h: 768,
    };

    #[test]
    fn one_window_fills_the_screen() {
        let mut t = Tiling::default();
        let a = t.map_id();
        assert_eq!(t.arrange_ids(SCREEN), vec![(a, SCREEN)]);
    }

    /// ★ THE ONE THE VKMS GATE COULD NOT ASK CHEAPLY.
    ///
    /// When two windows appeared stacked on screen, this question — "does the
    /// TREE separate them?" — cost a five-minute VM run to answer, because
    /// the only way in took a `Window` and a `Window` needs a live client.
    /// It is the same assertion, in milliseconds.
    #[test]
    fn two_windows_get_disjoint_halves() {
        let mut t = Tiling::default();
        let a = t.map_id();
        let b = t.map_id();
        let rects = t.arrange_ids(SCREEN);
        assert_eq!(rects.len(), 2);
        let ra = rects
            .iter()
            .find(|(i, _)| *i == a)
            .expect("a is laid out")
            .1;
        let rb = rects
            .iter()
            .find(|(i, _)| *i == b)
            .expect("b is laid out")
            .1;
        assert_ne!(ra.x, rb.x, "both windows at the same x — that is stacking");
        assert_eq!(
            ra.w + rb.w,
            SCREEN.w,
            "the halves must tile the screen exactly"
        );
        assert_eq!(ra.h, SCREEN.h);
        assert_eq!(rb.h, SCREEN.h);
    }

    /// A third window splits the FOCUSED one, and focus follows the newest.
    /// Orientation alternates with depth, so a run of windows makes a grid
    /// rather than a column of slivers.
    #[test]
    fn a_third_window_splits_the_focused_one_the_other_way() {
        let mut t = Tiling::default();
        let _a = t.map_id();
        let b = t.map_id();
        let c = t.map_id();
        let rects = t.arrange_ids(SCREEN);
        assert_eq!(rects.len(), 3);
        let rb = rects.iter().find(|(i, _)| *i == b).expect("b").1;
        let rc = rects.iter().find(|(i, _)| *i == c).expect("c").1;
        // b was focused, so c split IT — and one level deeper, so the divider
        // turns: same column, stacked vertically.
        assert_eq!(
            rb.x, rc.x,
            "the third window should share the second's column"
        );
        assert_ne!(rb.y, rc.y, "and sit above or below it, not on top of it");
    }

    /// Every rectangle is disjoint, at every size the fleet actually uses.
    /// A tiling that overlaps is not a tiling, and an overlap of one pixel
    /// looks exactly like a correct layout in a screenshot.
    #[test]
    fn rectangles_never_overlap() {
        for (w, h) in [(1024u16, 768u16), (1920, 1080), (3840, 2160), (640, 480)] {
            let screen = Rect { x: 0, y: 0, w, h };
            let mut t = Tiling::default();
            for _ in 0..6 {
                t.map_id();
            }
            let rects = t.arrange_ids(screen);
            assert_eq!(rects.len(), 6, "{w}x{h}");
            for (i, (_, a)) in rects.iter().enumerate() {
                for (_, b) in rects.iter().skip(i + 1) {
                    let disjoint = a.x + a.w <= b.x
                        || b.x + b.w <= a.x
                        || a.y + a.h <= b.y
                        || b.y + b.h <= a.y;
                    assert!(disjoint, "{w}x{h}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    /// The gap's floor is set by the border, not by taste.
    #[test]
    fn the_gap_is_at_least_two_borders_wide() {
        // ★ THE FLOOR IS THE BORDER, NOT TASTE. GAP is a per-window inset, so
        // two adjacent windows are separated by 2*GAP, and each may draw a
        // BORDER-thick focus ring inside its own inset. Below 2*BORDER the two
        // rings would touch and a split would read as one framed surface —
        // which is the exact confusion GAP exists to prevent.
        //
        // shitsurai puts GAP at 4 and BORDER at 2, i.e. EXACTLY on this floor.
        // That makes it the smallest honest value rather than merely a small
        // one, and it means any future "let's tighten the gaps a bit more"
        // fails here instead of silently producing touching rings.
        assert!(
            GAP >= BORDER * 2,
            "GAP ({GAP}) must be at least 2*BORDER ({}) or two focused \
             neighbours' rings meet in the middle",
            BORDER * 2
        );
    }

    /// Gaps must not make windows overlap — the inset shrinks each parcel,
    /// so disjointness is preserved by construction, and this pins it.
    #[test]
    fn the_gap_separates_rather_than_overlaps() {
        // Two 960-wide halves of a 1920 screen, each inset by GAP.
        let half = 960;
        let left_right_edge = 0 + GAP + (half - GAP * 2);
        let right_left_edge = half + GAP;
        assert!(
            left_right_edge < right_left_edge,
            "the inset halves must leave a visible seam: {left_right_edge} \
             then {right_left_edge}"
        );
        // And the seam must be wide enough for a border to sit in.
        assert!(
            right_left_edge - left_right_edge >= BORDER * 2,
            "the gap must fit two borders, or a focused window's edge is \
             drawn over its neighbour"
        );
    }

    #[test]
    fn an_empty_tiling_arranges_nothing() {
        assert!(Tiling::default().arrange_ids(SCREEN).is_empty());
        assert!(Tiling::default().is_empty());
    }
}

/// A window's `app_id`, if the client has set one yet.
///
/// ★ Returns `None` rather than an empty string for "not set", because those
/// are different facts: a client that has not yet sent `set_app_id` will send
/// one, and a client that sent `""` has told us it has no identity. Only the
/// second is stable enough to make a placement decision on, and
/// `placement::for_app_id` treats both as tiled anyway — but a future rule
/// that wants to distinguish them can.
pub fn app_id_of(w: &smithay::desktop::Window) -> Option<String> {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
    let t = w.toplevel()?;
    with_states(t.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .and_then(|d| d.app_id.clone())
    })
}

/// A window's TITLE, as the client set it.
///
/// ── ★ WHY THE TITLEBAR WAS INFORMATION-FREE WITHOUT THIS ─────────────────
/// omoya drew a 24px bar on every floating window and never read
/// `xdg_toplevel.set_title` — zero matches for `.title` across the crate
/// before 2026-09-03. N windows therefore carried N byte-identical bars, so
/// the chrome answered "this is a window" (which you could already see) and
/// never "which window". A row of identical bars is the operator's problem
/// on this seat: mado windows are visually interchangeable.
///
/// Same shape and same reasoning as [`app_id_of`] one function up, including
/// its `None`-vs-empty distinction: a client that has not sent a title yet
/// will send one, and a client that sent `""` has said it has no title. The
/// chrome renders nothing in both cases, but they are different facts and a
/// future rule may care.
pub fn title_of(w: &smithay::desktop::Window) -> Option<String> {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
    let t = w.toplevel()?;
    with_states(t.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .and_then(|d| d.title.clone())
    })
}

/// A window's wl_surface protocol id — a stable per-window key.
///
/// Used only to compare "what floated last pass" against "what should float
/// now"; never to look a window up, so a stale id is harmless rather than a
/// dangling reference.
/// The size a client has declared it cannot be resized away from.
///
/// ── ★ WHY min == max IS THE SIGNAL ────────────────────────────────────────
/// `xdg_toplevel.set_min_size` and `set_max_size` pinned to the same value is
/// how "not resizable" is spelled ON THE WIRE — it is what winit emits for
/// `WindowAttributes::with_resizable(false)`, and it is the only statement of
/// intent a client can make about its own size that the compositor is
/// obliged to respect. A configure that names a different size for such a
/// window is the compositor overruling the protocol.
///
/// This is not a heuristic about window kind. It deliberately does NOT guess
/// from `app_id`, from the absence of a title, or from smallness — the
/// placement module's own header records why every such guess is wrong for
/// something.
///
/// ★ MEASURED, 2026-09-03: tobira sizes itself to its content and grows
/// downward from an anchor (`content_window_size` → `request_inner_size`),
/// and the seat configured it to 0.46 x 0.52 of the output — 883x547 on a
/// 1920x1080 panel. The operator's report was "the launcher takes up this
/// huge square of space", and the launcher had asked for a small panel.
///
/// `None` when either axis is unconstrained (0) or the two disagree: that is
/// a resizable window, and the seat's size is then the right answer.
fn client_fixed_size(w: &smithay::desktop::Window) -> Option<smithay::utils::Size<i32, Logical>> {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::SurfaceCachedState;
    let surface = w.toplevel()?.wl_surface().clone();
    with_states(&surface, |states| {
        let mut guard = states.cached_state.get::<SurfaceCachedState>();
        let cur = guard.current();
        let (min, max) = (cur.min_size, cur.max_size);
        if min == max && min.w > 0 && min.h > 0 {
            Some(min)
        } else {
            None
        }
    })
}

/// The nearest neighbour of `from` in `dir`, by geometry.
///
/// ── ★ WHY GEOMETRY AND NOT THE TREE ─────────────────────────────────────
/// `Tiling::focus_direction` walks the kukaku tree, and `apply_layout` unmaps
/// EVERY window from that tree in floating mode — so directional focus was a
/// silent no-op on a floating seat (plo), while the deed reported
/// `Performed`. Floating windows have no tree to walk; they have positions.
///
/// A candidate must lie in `dir`: its centre strictly beyond `from`'s centre
/// on that axis. Among those, the nearest wins, with sideways distance
/// weighted DOUBLE — a window straight ahead beats a closer one far off to
/// the side, which is what "focus right" means to a person looking at a
/// screen.
#[must_use]
pub fn nearest_in_direction<T: Copy>(
    from: Rectangle<i32, Logical>,
    others: &[(T, Rectangle<i32, Logical>)],
    dir: Direction,
) -> Option<T> {
    let centre = |r: Rectangle<i32, Logical>| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2);
    let (fx, fy) = centre(from);
    others
        .iter()
        .filter_map(|(t, r)| {
            let (cx, cy) = centre(*r);
            let (along, across) = match dir {
                Direction::Left => (fx - cx, (cy - fy).abs()),
                Direction::Right => (cx - fx, (cy - fy).abs()),
                Direction::Above => (fy - cy, (cx - fx).abs()),
                Direction::Below => (cy - fy, (cx - fx).abs()),
            };
            (along > 0).then_some((along + across * 2, *t))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, t)| t)
}

pub fn client_min_frame(w: &smithay::desktop::Window) -> (i32, i32) {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::SurfaceCachedState;
    let Some(t) = w.toplevel() else { return (0, 0) };
    let min = with_states(t.wl_surface(), |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .min_size
    });
    if min.w <= 0 && min.h <= 0 {
        (0, 0)
    } else {
        (min.w.max(0), min.h.max(0) + crate::chrome::HEIGHT)
    }
}

pub fn surface_id_of(w: &smithay::desktop::Window) -> Option<u32> {
    use smithay::reexports::wayland_server::Resource as _;
    Some(crate::winid::of(w.toplevel()?.wl_surface()))
}

/// Should this window float, and does that DISAGREE with the last layout pass?
///
/// The cheap question `commit` asks on every toplevel commit. Cheap because it
/// is one `app_id` read and one hash lookup — no tree walk, no arrangement —
/// so the common answer (`false`) costs nothing on the frame path.
///
/// ── ★ IT MUST ASK THE QUESTION `apply_layout` ANSWERED ──────────────────
/// `floating_ids` is built with the MODE folded in — `floating_mode ||
/// for_app_id_in(..).is_floating()` — so in `LayoutMode::Floating` every
/// window's id is in the set. This predicate computed `should_float` from
/// `for_app_id_in` ALONE, with no mode term, so for any window whose `app_id`
/// is not a listed overlay (mado, namimado, anything but tobira) it was
/// `false != true` → **true, on every commit, forever**. The two sides were
/// asking different questions, so the comparison could never converge, and
/// `handlers.rs`'s stated reason for the guard — "rather than calling
/// `apply_layout` unconditionally" — was defeated on the seat plo actually
/// runs. Every keystroke in a terminal re-laid the whole desktop out.
#[must_use]
pub fn placement_changed(
    w: &smithay::desktop::Window,
    floating_ids: &std::collections::HashSet<u32>,
    placement: &crate::config::PlacementConfig,
    mode: crate::config::LayoutMode,
) -> bool {
    let Some(id) = surface_id_of(w) else {
        return false;
    };
    let should_float = mode == crate::config::LayoutMode::Floating
        || crate::placement::for_app_id_in(app_id_of(w).as_deref(), placement).is_floating();
    should_float != floating_ids.contains(&id)
}

#[cfg(test)]
mod placement_guard_tests {
    use crate::config::{LayoutMode, PlacementConfig};

    /// ★ THE GUARD WAS PERMANENTLY TRUE IN FLOATING MODE.
    ///
    /// `placement_changed` is what `CompositorHandler::commit` asks before
    /// re-laying the desktop out, and it compared a mode-free `should_float`
    /// against a set `apply_layout` builds WITH the mode folded in. On plo —
    /// `layout.mode: floating` — every window that is not a listed overlay
    /// answered `false != true` on every commit, so every keystroke in a
    /// terminal re-laid out the seat.
    ///
    /// Pure, so it needs no seat: the whole defect is in the two expressions
    /// disagreeing, and that is exactly what a table can pin.
    fn changed(mode: LayoutMode, should_float_by_app: bool, in_set: bool) -> bool {
        // The predicate's body, with the id lookup lifted out — the Window
        // half needs a live client and is not what was wrong.
        let should_float = mode == LayoutMode::Floating || should_float_by_app;
        should_float != in_set
    }

    #[test]
    fn floating_mode_converges_instead_of_firing_forever() {
        // An ordinary window in floating mode: `apply_layout` put it in the
        // set, so the guard must agree and stay quiet.
        assert!(
            !changed(LayoutMode::Floating, false, true),
            "an ordinary window in floating mode is settled — this is the row              that was true forever"
        );
        // …and it fires exactly once, on the commit where the set is stale.
        assert!(changed(LayoutMode::Floating, false, false));
    }

    #[test]
    fn tiling_mode_is_unchanged_by_the_fix() {
        // A tiled app is not in the set and should not be.
        assert!(!changed(LayoutMode::Tiling, false, false));
        // An overlay IS, by app_id, in either mode.
        assert!(!changed(LayoutMode::Tiling, true, true));
        assert!(!changed(LayoutMode::Floating, true, true));
        // A disagreement in either direction still fires.
        assert!(changed(LayoutMode::Tiling, true, false));
        assert!(changed(LayoutMode::Tiling, false, true));
    }

    /// The mode term must not be droppable: without it, the first row above
    /// is `false != true` — true — which is the defect.
    #[test]
    fn dropping_the_mode_term_reproduces_the_defect() {
        let without_mode = |should_float_by_app: bool, in_set: bool| should_float_by_app != in_set;
        assert!(
            without_mode(false, true),
            "the pre-fix expression fires on a settled floating window — kept              so the fix cannot be reverted without this failing"
        );
        assert!(!changed(LayoutMode::Floating, false, true));
        let _ = PlacementConfig::default();
    }
}

#[cfg(test)]
mod roster_input_tests {
    /// ★ THE LAYOUT MUST NOT READ BACK THE THING IT WRITES.
    ///
    /// `apply_layout` built its window list from `space.elements()` and then
    /// wrote positions into that same Space. That made `Placement::Hidden` a
    /// ONE-WAY DOOR — the hidden arm unmaps, so the next pass could not see
    /// the window, could not re-map it, and `RestoreLast` had nothing to
    /// restore. Measured on plo: minimize left the seat empty with
    /// `minimized_count: 1`; restore-last reported `minimized_count: 0`,
    /// `toplevels: 0`, an empty screen, and both clients still alive.
    ///
    /// It is a source test for the same reason the chrome one is: the defect
    /// was WHERE the data came from, and no test of the placement arithmetic
    /// could see it. Restoring a minimised window needs a live client, a seat
    /// and a compositor loop; this needs none and fails the moment the input
    /// is wired back to the output.
    #[test]
    fn apply_layout_takes_its_windows_from_the_roster() {
        let src = include_str!("layout.rs");
        let code: String = src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        // The `seen` binding is the layout's input. Find it and check what it
        // is built from — the two candidates are one line apart in the source.
        let at = code
            .find("let seen:")
            .expect("apply_layout must still build a `seen` list");
        // Exactly the `seen` STATEMENT — up to its terminating `;`. A fixed
        // character window ran past it into following code that legitimately
        // mentions the Space, which failed this test for the wrong reason on
        // its first run.
        let rest = &code[at..];
        let end = rest.find(';').map_or(rest.len(), |i| i + 1);
        let window = &rest[..end];

        assert!(
            // `.roster`, not `self.roster`: the method chain is line-broken,
            // so `self` and `.roster` are never adjacent in the source. The
            // first version of this test looked for the contiguous string and
            // failed for that reason rather than for a real regression.
            window.contains(".roster"),
            "`seen` is no longer built from the roster. If it is back on \
             `space.elements()`, the layout is reading the Space it writes and \
             a hidden window becomes invisible to the next pass — minimise \
             turns back into a one-way door."
        );
        assert!(
            !window.contains(".space") && !window.contains("space.elements"),
            "`seen` mentions the Space. The layout's input must be the roster \
             (what EXISTS); the Space is its output (where things SIT)."
        );
    }

    #[test]
    fn directional_focus_picks_the_neighbour_in_that_direction() {
        use kukaku::Direction;
        let r = |x, y, w, h| {
            smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (x, y).into(),
                (w, h).into(),
            )
        };
        let from = r(100, 100, 200, 200); // centre (200, 200)
        let right = r(500, 100, 200, 200);
        let left = r(-300, 100, 200, 200);
        let below = r(100, 500, 200, 200);
        let others = [("right", right), ("left", left), ("below", below)];
        assert_eq!(
            crate::layout::nearest_in_direction(from, &others, Direction::Right),
            Some("right")
        );
        assert_eq!(
            crate::layout::nearest_in_direction(from, &others, Direction::Left),
            Some("left")
        );
        assert_eq!(
            crate::layout::nearest_in_direction(from, &others, Direction::Below),
            Some("below")
        );
        // Nothing above: a direction with no candidate answers None, which is
        // what makes the deed able to report "no window in that direction"
        // instead of claiming a performance.
        assert_eq!(
            crate::layout::nearest_in_direction(from, &others, Direction::Above),
            None
        );
    }

    #[test]
    fn straight_ahead_beats_closer_but_off_to_the_side() {
        use kukaku::Direction;
        let r = |x, y, w, h| {
            smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (x, y).into(),
                (w, h).into(),
            )
        };
        let from = r(0, 0, 100, 100); // centre (50, 50)
        // `askew` is nearer on the x axis but far off the y axis; `ahead` is
        // level with the focused window. A person pressing "right" means the
        // one level with them.
        let ahead = r(400, 0, 100, 100); // centre (450, 50):  along 400, across 0
        let askew = r(300, 600, 100, 100); // centre (350, 650): along 300, across 600
        let others = [("askew", askew), ("ahead", ahead)];
        assert_eq!(
            crate::layout::nearest_in_direction(from, &others, Direction::Right),
            Some("ahead")
        );
    }

    #[test]
    fn a_window_behind_you_is_never_the_neighbour_ahead() {
        use kukaku::Direction;
        let r = |x, y, w, h| {
            smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (x, y).into(),
                (w, h).into(),
            )
        };
        let from = r(500, 0, 100, 100);
        let behind = r(0, 0, 100, 100);
        assert_eq!(
            crate::layout::nearest_in_direction(from, &[("behind", behind)], Direction::Right),
            None
        );
        assert_eq!(
            crate::layout::nearest_in_direction(from, &[("behind", behind)], Direction::Left),
            Some("behind")
        );
        // An empty field has no neighbour in any direction.
        let none: [(
            &str,
            smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        ); 0] = [];
        assert_eq!(
            crate::layout::nearest_in_direction(from, &none, Direction::Left),
            None
        );
    }
}
