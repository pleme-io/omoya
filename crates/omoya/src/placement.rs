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
