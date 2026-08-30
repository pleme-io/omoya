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
use crate::placement::Placement;
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
/// in a layout expression is a defect.
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
    pub fn arrange(&self, bounds: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
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
        let seen: Vec<(smithay::desktop::Window, Option<String>)> = self
            .space
            .elements()
            .map(|w| (w.clone(), app_id_of(w)))
            .collect();
        *self
            .introspect
            .window_app_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            seen.iter().map(|(_, id)| id.clone()).collect();

        // ── ★ THE JOINED TABLE, BUILT FROM THE SAME WALK ────────────────────
        //
        // Built here rather than beside `geometry` in drm.rs on purpose: this
        // walk is per-WINDOW, while drm.rs walks RENDER ELEMENTS and therefore
        // counts the bar and four focus-ring edges among "windows". Sharing the
        // walk is what makes the row's app_id and rect refer to the same thing
        // -- the property the three legacy lists never had.
        {
            use smithay::reexports::wayland_server::Resource as _;
            let focused = self.introspect.focus_rect.lock().unwrap_or_else(|e| e.into_inner());
            let sent = self
                .introspect
                .decoration_sent
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let rows: Vec<crate::introspect::ToplevelRow> = seen
                .iter()
                .enumerate()
                .map(|(i, (w, app))| {
                    let rect = self.space.element_geometry(w).map(|g| {
                        (g.loc.x, g.loc.y, g.size.w, g.size.h)
                    });
                    let key = w
                        .toplevel()
                        .map(|t| format!("{:?}", t.wl_surface().id()));
                    crate::introspect::ToplevelRow {
                        id: i as u64,
                        app_id: app.clone(),
                        decoration_mode_sent: key.and_then(|k| sent.get(&k).cloned()),
                        rect,
                        // ★ The focus ring is the ONLY chrome omoya draws, and
                        // only for the focused window. Counting it here is what
                        // makes `0` on an unfocused window a readable fact
                        // rather than an absence nobody looked for.
                        decoration_elements_drawn: u32::from(
                            rect.is_some() && *focused == rect.map(|r| (r.0, r.1, r.2, r.3)),
                        ) * 4,
                        focused: rect.is_some()
                            && *focused == rect.map(|r| (r.0, r.1, r.2, r.3)),
                        tiled: false,
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
        let floating_mode =
            self.config.layout.mode == crate::config::LayoutMode::Floating;
        // ★ Published from the point of DECISION, so the leaf reports the mode
        // the arrangement actually used rather than a re-read of config that
        // could drift from it.
        *self
            .introspect
            .layout_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            if floating_mode { "floating" } else { "tiling" }.to_owned();
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

        let arranged = self.tiling.arrange(usable);
        // Publish what the TREE asked for, before anything is applied. See
        // `OmoyaIntrospect::layout` — a screenshot says where windows ended
        // up and cannot say what was requested, so when the two disagree
        // there is otherwise no way to tell a broken split from a broken
        // placement from an early return above.
        for (window, rect) in &arranged {
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
            // ★ IN FLOATING MODE THE SIZE COMES FROM CONFIG, NOT FROM THE
            // PER-APP RULE. `for_app_id_in` returns `Tiled` for an unlisted
            // app, and this loop used to `continue` on that — correct when
            // the only floaters were listed apps, and a seat that maps
            // NOTHING once the mode makes every window a floater. The window
            // would be unmapped from the tiling tree and then skipped here.
            let (width, height) = match crate::placement::for_app_id_in(
                app_id_of(w).as_deref(),
                &self.config.placement,
            ) {
                Placement::Floating { width, height } => (width, height),
                Placement::Tiled if floating_mode => (
                    self.config.placement.float_width,
                    self.config.placement.float_height,
                ),
                Placement::Tiled => continue,
            };
            // Cascade so successive windows are individually reachable, then
            // snap so one nudged toward an edge sits flush with it. Snap
            // AFTER cascade: the cascade decides where the window wants to be
            // and the snap only tidies that answer, whereas snapping first
            // would be immediately overwritten by the offset.
            let rect = if floating_mode {
                crate::placement::snap_to_edges(
                    crate::placement::cascaded(
                        usable,
                        width,
                        height,
                        idx,
                        self.config.layout.cascade_step,
                    ),
                    usable,
                    self.config.layout.snap_threshold,
                )
            } else {
                // A launcher summoned over a tiled desktop is still centred —
                // it is a transient overlay, not a member of a floating
                // arrangement, and cascading it would move it every time.
                crate::placement::centred(usable, width, height)
            };
            if let Some(t) = w.toplevel() {
                t.with_pending_state(|state| {
                    state.size = Some(rect.size);
                });
                t.send_pending_configure();
            }
            // `true` — activate. A launcher that appears without focus is a
            // launcher you have to click before you can type into, which
            // defeats summoning it from the keyboard.
            self.space.map_element(w.clone(), rect.loc, true);
        }

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
                // The last-mapped float is the focused one: `map_element(.., true)`
                // above activates each float as it is placed, so the final
                // one holds focus. Reading the space rather than tracking a
                // second focus field keeps one source of truth.
                let w = floats.last()?;
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
    const SCREEN: Rect = Rect { x: 0, y: 0, w: 1024, h: 768 };

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
        let ra = rects.iter().find(|(i, _)| *i == a).expect("a is laid out").1;
        let rb = rects.iter().find(|(i, _)| *i == b).expect("b is laid out").1;
        assert_ne!(ra.x, rb.x, "both windows at the same x — that is stacking");
        assert_eq!(ra.w + rb.w, SCREEN.w, "the halves must tile the screen exactly");
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
        assert_eq!(rb.x, rc.x, "the third window should share the second's column");
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
fn app_id_of(w: &smithay::desktop::Window) -> Option<String> {
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

/// A window's wl_surface protocol id — a stable per-window key.
///
/// Used only to compare "what floated last pass" against "what should float
/// now"; never to look a window up, so a stale id is harmless rather than a
/// dangling reference.
pub fn surface_id_of(w: &smithay::desktop::Window) -> Option<u32> {
    use smithay::reexports::wayland_server::Resource as _;
    Some(w.toplevel()?.wl_surface().id().protocol_id())
}

/// Should this window float, and does that DISAGREE with the last layout pass?
///
/// The cheap question `commit` asks on every toplevel commit. Cheap because it
/// is one `app_id` read and one hash lookup — no tree walk, no arrangement —
/// so the common answer (`false`) costs nothing on the frame path.
#[must_use]
pub fn placement_changed(
    w: &smithay::desktop::Window,
    floating_ids: &std::collections::HashSet<u32>,
    placement: &crate::config::PlacementConfig,
) -> bool {
    let Some(id) = surface_id_of(w) else {
        return false;
    };
    let should_float =
        crate::placement::for_app_id_in(app_id_of(w).as_deref(), placement).is_floating();
    should_float != floating_ids.contains(&id)
}
