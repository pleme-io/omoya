//! Which windows tile, and which float above the tiling.
//!
//! ── ★ THE DEFECT THIS EXISTS FOR ──────────────────────────────────────────
//! `new_toplevel` mapped **every** window into the tiling tree, so `tobira` —
//! the launcher — became a quarter of the screen next to the terminals it was
//! summoned over. The operator's description was exact: *"it seems focused on
//! one window or something and that looks weird, should be at the middle."*
//! A launcher is not a window you arrange; it is a thing that appears over
//! whatever you were doing and then goes away.
//!
//! ── ★ WHY app_id AND NOT layer-shell, WHICH IS THE RIGHT ANSWER ───────────
//! The correct mechanism for "an overlay above everything, positioned by the
//! compositor" is `zwlr_layer_shell_v1` — it is what rofi, wofi and fuzzel
//! use, and it makes placement a PROTOCOL fact instead of a name-matching
//! rule. omoya already implements `WlrLayerShellHandler`.
//!
//! tobira cannot use it: it renders through **winit**, and winit has no
//! layer-shell support — a winit window is an `xdg_toplevel` and nothing else.
//! So the choice is not "rule versus protocol", it is "rule versus a launcher
//! that tiles". The rule is the interim; `pending-tobira-layer-shell` is the
//! destination, and it is a tobira change (dropping winit for a
//! smithay-client-toolkit surface), not an omoya one.
//!
//! ── ★ WHY IT IS RE-EVALUATED EVERY LAYOUT PASS ────────────────────────────
//! The obvious place to decide is `new_toplevel`, and it does not work:
//! `app_id` arrives in a SEPARATE request after the toplevel is created, so at
//! `new_toplevel` it is usually `None` and every window looks like a tiler.
//! Deciding once, early, would make the rule fire only for clients that happen
//! to set `app_id` before the compositor gets around to asking — which is a
//! race, and races in window placement look like "it works sometimes".
//!
//! So `apply_layout` re-derives it each pass and MOVES a window between the
//! tree and the floating set as the answer changes. That is idempotent, it
//! self-corrects when `app_id` lands late, and it costs one string compare per
//! window per layout pass.

/// Where a window belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// Joins the binary-split tree and gets whatever rectangle it earns.
    Tiled,
    /// Floats above the tree, centred, sized as a fraction of the usable area.
    ///
    /// A fraction rather than a pixel size because the launcher should look
    /// the same on a 1080p seat and a 4K one, and because the usable area is
    /// already bar- and layer-shell-adjusted by the time this is applied.
    Floating { width: f64, height: f64 },
}

impl Placement {
    #[must_use]
    pub fn is_floating(self) -> bool {
        matches!(self, Self::Floating { .. })
    }
}

/// Apps that float, by `app_id`.
///
/// ★ A SHORT, EXPLICIT LIST — deliberately not a heuristic. The tempting
/// version guesses from size hints or the absence of a title, and every such
/// guess is wrong for something: a small terminal is not a dialog, and a
/// title-less window is usually just a client that has not set one yet.
/// Guessing wrong here does not produce a subtle bug, it produces a window
/// the operator cannot arrange.
///
/// Matching is on the FULL `app_id`, not a prefix or substring: `tobira`
/// must not also capture a hypothetical `tobira-settings` that genuinely
/// wants to be a normal window.
pub const FLOATING_APP_IDS: &[&str] = &[
    // 扉 — the launcher. Summoned over your work, dismissed, gone.
    "tobira",
];

/// The size a floating window is given, as a fraction of the usable area.
///
/// Wide enough for a result list to be readable and short enough that the
/// work behind it stays visible, which is the whole point of an overlay.
const FLOAT_W: f64 = 0.46;
const FLOAT_H: f64 = 0.52;

/// Decide where a window with this `app_id` belongs.
/// Decide placement against a CONFIGURED rule set.
///
/// ★ Split from [`for_app_id`] so the const remains the default and the thing
/// the tests are written against, while an operator can add an `app_id` in
/// yaml without a recompile. A config field nothing reads is decoration, and
/// this is the function that stops `placement.floating_app_ids` from being it.
#[must_use]
pub fn for_app_id_in(app_id: Option<&str>, cfg: &crate::config::PlacementConfig) -> Placement {
    match app_id {
        Some(id) if cfg.floating_app_ids.iter().any(|f| f == id) => Placement::Floating {
            width: cfg.float_width,
            height: cfg.float_height,
        },
        _ => Placement::Tiled,
    }
}

#[must_use]
pub fn for_app_id(app_id: Option<&str>) -> Placement {
    match app_id {
        Some(id) if FLOATING_APP_IDS.contains(&id) => Placement::Floating {
            width: FLOAT_W,
            height: FLOAT_H,
        },
        // ★ `None` IS TILED, and that is the safe direction. An unknown window
        // that tiles is merely arranged oddly; an unknown window that floats
        // is one the operator cannot move, resize or reach with a direction
        // key. When the identity is not known yet, behave like the common case.
        _ => Placement::Tiled,
    }
}

/// Centre a floating window inside `usable`.
///
/// Returned in the same absolute coordinates the tiler uses, so the caller can
/// hand it straight to `map_element` without a second frame of reference to
/// get wrong.
#[must_use]
pub fn centred(
    usable: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    width: f64,
    height: f64,
) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let w = ((f64::from(usable.size.w) * width) as i32).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let h = ((f64::from(usable.size.h) * height) as i32).max(1);
    // Integer-divide AFTER subtracting, so an odd leftover pixel goes to the
    // right/bottom rather than making the window one pixel wider than asked.
    smithay::utils::Rectangle::new(
        (
            usable.loc.x + (usable.size.w - w) / 2,
            usable.loc.y + (usable.size.h - h) / 2,
        )
            .into(),
        (w, h).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Rectangle;

    #[test]
    fn a_configured_rule_set_is_honoured_and_an_empty_one_floats_nothing() {
        // ★ An EMPTY list must float NOTHING, not fall back to the default.
        // An operator writing `floating_app_ids: []` is saying "tile
        // everything"; silently reinstating tobira would ignore them while
        // looking like it worked.
        let empty = crate::config::PlacementConfig {
            floating_app_ids: vec![],
            float_width: 0.5,
            float_height: 0.5,
        };
        assert_eq!(for_app_id_in(Some("tobira"), &empty), Placement::Tiled);

        // And an arbitrary configured app floats, not just the built-in one.
        let custom = crate::config::PlacementConfig {
            floating_app_ids: vec!["my-dialog".into()],
            float_width: 0.3,
            float_height: 0.4,
        };
        assert_eq!(
            for_app_id_in(Some("my-dialog"), &custom),
            Placement::Floating { width: 0.3, height: 0.4 }
        );
        assert_eq!(for_app_id_in(Some("tobira"), &custom), Placement::Tiled);
    }

    #[test]
    fn the_launcher_floats_and_everything_else_tiles() {
        assert!(for_app_id(Some("tobira")).is_floating());
        for other in ["mado", "google-chrome", "chromium-browser", ""] {
            assert_eq!(
                for_app_id(Some(other)),
                Placement::Tiled,
                "{other} must tile"
            );
        }
    }

    #[test]
    fn an_unknown_app_id_tiles_rather_than_floats() {
        // ★ THE SAFE DIRECTION, and the reason it matters: `app_id` arrives in
        // a separate request AFTER the toplevel exists, so `None` is the normal
        // state for a frame or two on every window. Floating on `None` would
        // make every window briefly un-arrangeable, and any client that never
        // sets an app_id permanently so.
        assert_eq!(for_app_id(None), Placement::Tiled);
    }

    #[test]
    fn matching_is_exact_not_a_prefix() {
        // `tobira-settings` is a different program and wants to be a window.
        assert_eq!(for_app_id(Some("tobira-settings")), Placement::Tiled);
        assert_eq!(for_app_id(Some("tobirato")), Placement::Tiled);
        assert!(for_app_id(Some("tobira")).is_floating());
    }

    #[test]
    fn a_floating_window_is_actually_centred() {
        // ★ The operator's complaint was literally "should be at the middle".
        // Assert the MARGINS match rather than the position, because that is
        // the property "centred" means and it survives a size change.
        let usable = Rectangle::new((0, 28).into(), (1920, 1052).into());
        let r = centred(usable, 0.5, 0.5);
        let left = r.loc.x - usable.loc.x;
        let right = (usable.loc.x + usable.size.w) - (r.loc.x + r.size.w);
        let top = r.loc.y - usable.loc.y;
        let bottom = (usable.loc.y + usable.size.h) - (r.loc.y + r.size.h);
        assert!((left - right).abs() <= 1, "h-margins {left} vs {right}");
        assert!((top - bottom).abs() <= 1, "v-margins {top} vs {bottom}");
    }

    #[test]
    fn it_is_centred_in_the_USABLE_area_not_the_output() {
        // ★ The bar reserves the top strip. Centring against the raw output
        // would push the launcher upward by half the bar height — visibly off,
        // and exactly the kind of "slightly wrong" that is hard to name.
        let usable = Rectangle::new((0, 28).into(), (1920, 1052).into());
        let r = centred(usable, 0.5, 0.5);
        assert!(r.loc.y >= usable.loc.y, "must not intrude into the bar");
        let centre_y = r.loc.y + r.size.h / 2;
        let usable_centre = usable.loc.y + usable.size.h / 2;
        assert!(
            (centre_y - usable_centre).abs() <= 1,
            "centre {centre_y} vs usable centre {usable_centre}"
        );
    }

    #[test]
    fn a_degenerate_area_still_yields_a_usable_rectangle() {
        // A zero-size output is reachable while a mode is being set. A window
        // configured to 0x0 is a protocol error on some clients, so clamp.
        let r = centred(Rectangle::new((0, 0).into(), (0, 0).into()), 0.5, 0.5);
        assert!(r.size.w >= 1 && r.size.h >= 1, "got {:?}", r.size);
    }
}

/// Offset a floating window so successive ones do not stack exactly.
///
/// ── ★ WHY CASCADE AT ALL ─────────────────────────────────────────────────
/// [`centred`] is right for ONE floating window — a launcher, summoned and
/// dismissed. It is wrong for a floating DESKTOP: three terminals opened in
/// `LayoutMode::Floating` would be three identical rectangles in the exact
/// centre, with only the topmost reachable by pointer and no visual evidence
/// the other two exist. The seat would look like it had lost them.
///
/// The offset wraps rather than marching off-screen: after enough windows the
/// cascade returns to the origin and overlaps an earlier one, which is
/// recoverable, while a window placed past the edge is not.
#[must_use]
pub fn cascaded(
    usable: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    width: f64,
    height: f64,
    index: usize,
    step: i32,
) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
    let base = centred(usable, width, height);
    if step <= 0 {
        return base;
    }
    // How many steps fit before the window's far edge would leave the zone.
    // Computed rather than fixed, so a small window on a large screen
    // cascades further than a large one on a small screen.
    let room_x = (usable.loc.x + usable.size.w - (base.loc.x + base.size.w)).max(0);
    let room_y = (usable.loc.y + usable.size.h - (base.loc.y + base.size.h)).max(0);
    let span = (room_x.min(room_y) / step).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let n = (index % (span as usize).max(1)) as i32;
    smithay::utils::Rectangle::new(
        (base.loc.x + n * step, base.loc.y + n * step).into(),
        base.size,
    )
}

/// Pull a floating window flush with the zone's edges when it is already
/// close to them.
///
/// ── ★ WHY A THRESHOLD AND NOT ALWAYS ─────────────────────────────────────
/// Snapping that always applies is maximising, and it takes away the operator
/// deliberately parking a window off-centre. Snapping that never applies
/// leaves a one- or two-pixel seam nobody can close by hand at 1920x1080.
/// The threshold is exactly what makes it read as *alignment* rather than as
/// the compositor overriding a choice.
///
/// ★ `threshold <= 0` is a no-op, deliberately: "off" is expressible in the
/// same integer as "how close", so disabling snapping needs no second field
/// that could disagree with this one.
///
/// Each axis is decided independently — a window flush to the left edge but
/// floating vertically snaps horizontally only, which is what the operator
/// asked for by putting it there.
#[must_use]
pub fn snap_to_edges(
    rect: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    usable: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    threshold: i32,
) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
    if threshold <= 0 {
        return rect;
    }
    let (zl, zt) = (usable.loc.x, usable.loc.y);
    let (zr, zb) = (usable.loc.x + usable.size.w, usable.loc.y + usable.size.h);
    let (mut x, mut y) = (rect.loc.x, rect.loc.y);
    let (w, h) = (rect.size.w, rect.size.h);

    // ★ LEFT WINS A TIE, and the size never changes. Snapping both edges of
    // one axis would RESIZE the window to fit the zone, which is a different
    // operation than aligning it — and one the operator did not ask for by
    // dragging near an edge. So each axis snaps at most one edge, and the
    // near edge is preferred because that is the one being aimed at.
    if (x - zl).abs() <= threshold {
        x = zl;
    } else if ((x + w) - zr).abs() <= threshold {
        x = zr - w;
    }
    if (y - zt).abs() <= threshold {
        y = zt;
    } else if ((y + h) - zb).abs() <= threshold {
        y = zb - h;
    }
    smithay::utils::Rectangle::new((x, y).into(), (w, h).into())
}

#[cfg(test)]
mod layout_mode_tests {
    use super::*;
    use smithay::utils::Rectangle;

    fn zone() -> Rectangle<i32, smithay::utils::Logical> {
        Rectangle::new((0, 28).into(), (1920, 1052).into())
    }

    #[test]
    fn snapping_is_a_no_op_at_zero_threshold() {
        let r = Rectangle::new((3, 31).into(), (100, 100).into());
        assert_eq!(snap_to_edges(r, zone(), 0), r, "0 must disable snapping");
    }

    #[test]
    fn a_near_edge_snaps_flush_and_the_size_is_unchanged() {
        let r = Rectangle::new((5, 33).into(), (400, 300).into());
        let s = snap_to_edges(r, zone(), 16);
        assert_eq!((s.loc.x, s.loc.y), (0, 28), "should sit flush at top-left");
        assert_eq!(s.size, r.size, "snapping must never resize");
    }

    #[test]
    fn a_far_edge_is_left_alone() {
        let r = Rectangle::new((500, 500).into(), (400, 300).into());
        assert_eq!(snap_to_edges(r, zone(), 16), r, "beyond the threshold is a choice");
    }

    #[test]
    fn the_right_and_bottom_edges_snap_too() {
        let z = zone();
        let r = Rectangle::new((z.size.w - 400 - 6, z.loc.y + z.size.h - 300 - 6).into(),
                               (400, 300).into());
        let s = snap_to_edges(r, z, 16);
        assert_eq!(s.loc.x + s.size.w, z.loc.x + z.size.w);
        assert_eq!(s.loc.y + s.size.h, z.loc.y + z.size.h);
    }

    #[test]
    fn cascade_separates_windows_and_wraps_rather_than_leaving_the_zone() {
        let z = zone();
        let a = cascaded(z, 0.6, 0.6, 0, 24);
        let b = cascaded(z, 0.6, 0.6, 1, 24);
        assert_ne!((a.loc.x, a.loc.y), (b.loc.x, b.loc.y), "must not stack");
        for i in 0..50 {
            let r = cascaded(z, 0.6, 0.6, i, 24);
            assert!(r.loc.x >= z.loc.x && r.loc.y >= z.loc.y);
            assert!(r.loc.x + r.size.w <= z.loc.x + z.size.w,
                    "window {i} left the zone horizontally");
            assert!(r.loc.y + r.size.h <= z.loc.y + z.size.h,
                    "window {i} left the zone vertically");
        }
    }

    #[test]
    fn a_zero_step_falls_back_to_centred() {
        let z = zone();
        assert_eq!(cascaded(z, 0.6, 0.6, 7, 0), centred(z, 0.6, 0.6));
    }
}
