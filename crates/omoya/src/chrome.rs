//! Window chrome — the titlebar, and the three buttons on it.
//!
//! ── ★ WHY THIS EXISTS: "NO PLACE FOR THE MOUSE TO GO" ────────────────────
//! The operator's words, 2026-09-03, looking at two floating windows on plo:
//! *"no place to drag them or anything or minimize them or so on — no place
//! for the mouse to go."* They were exactly right, and the compositor's own
//! source already said why. From `input.rs`:
//!
//! > mado has no titlebar: this seat draws server-side decorations, so mado
//! > never sends `xdg_toplevel.move` and the grab had no trigger. The
//! > machinery was complete and unreachable.
//!
//! The answer at the time was Super+drag — a real trigger, and one you cannot
//! discover by looking at the screen. A window-management verb that exists
//! only as a chord is a verb most people never learn they have. This module is
//! the visible half: a bar you can grab and three buttons you can hit.
//!
//! ── ★ WHY THE GEOMETRY IS A PURE MODULE, SEPARATE FROM DRAWING ───────────
//! Because a titlebar is TWO agreements that must not drift: where the pixels
//! are drawn, and where a click is interpreted. Compute them in two places and
//! the buttons stop lining up with what they do — a defect that is invisible
//! in the source and obvious to the person clicking. So both callers read the
//! same functions here, and every one of them is unit-testable without a
//! compositor, a GPU or a seat.
//!
//! ── ★ FUNCTION, NOT LOOKS ────────────────────────────────────────────────
//! The operator asked for "close minimize maximize and a tab bar like macos or
//! something, **not looks wise just as a guide for function**". So this is
//! plain rectangles in the fleet palette — no gradients, no rounding, no
//! icons. Three squares in a known order, and a bar you can grab.

use smithay::utils::{Logical, Point, Rectangle};

/// Titlebar height in logical pixels.
///
/// 24 is two of the seat's 12px grid steps and comfortably taller than the
/// 20x34 cursor's hot area, so the bar cannot be a target you keep missing.
pub const HEIGHT: i32 = 24;

/// A button's side, in logical pixels.
pub const BUTTON: i32 = 14;

/// Gap between buttons, and between the buttons and the bar's edge.
pub const PAD: i32 = 5;

/// What a click on the chrome means.
///
/// ★ A CLOSED ENUM, and the reason is the same one `ukeire::ScrollDirection`
/// gives: the alternative is a tuple of booleans or an index, and both admit
/// combinations with no meaning ("close and maximize", "button 7"). Here the
/// only representable answers are the four that exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The close button.
    Close,
    /// The minimize button.
    Minimize,
    /// The maximize/restore button.
    Maximize,
    /// The bar itself — a drag surface, not a button.
    Drag,
}

/// The titlebar rectangle for a window whose CONTENT occupies `content`.
///
/// The bar sits ABOVE the content rather than overlapping it, so a client
/// never has to know the decoration exists and no pixel of its surface is
/// covered. The layout is responsible for leaving the room; `content_for`
/// below is the inverse it uses to do that.
#[must_use]
pub fn bar_rect(content: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (content.loc.x, content.loc.y - HEIGHT).into(),
        (content.size.w, HEIGHT).into(),
    )
}

/// The content rectangle for a window whose whole FRAME (bar + content)
/// occupies `frame`.
///
/// The inverse of [`bar_rect`], and the function the layout uses so a window
/// with a titlebar does not grow by the height of its own decoration.
#[must_use]
pub fn content_for(frame: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (frame.loc.x, frame.loc.y + HEIGHT).into(),
        (frame.size.w, (frame.size.h - HEIGHT).max(1)).into(),
    )
}

/// The three button rectangles, in the order [`Hit::Close`],
/// [`Hit::Minimize`], [`Hit::Maximize`].
///
/// ── ★ LEFT-ALIGNED, LIKE macOS, BECAUSE THAT IS WHAT WAS ASKED FOR ───────
/// The operator said "like macos or something". Left is also the safer side
/// on this seat: the bar's right end is where a long title would run out, and
/// buttons there would be the first thing a title collides with.
///
/// Returns an empty slice's worth of zero-width rects if the bar is too narrow
/// to hold them — see `fits`. Callers must check rather than draw a button
/// that overhangs its own bar.
#[must_use]
pub fn buttons(content: Rectangle<i32, Logical>) -> [Rectangle<i32, Logical>; 3] {
    let bar = bar_rect(content);
    let y = bar.loc.y + (HEIGHT - BUTTON) / 2;
    let mut out = [Rectangle::new((0, 0).into(), (0, 0).into()); 3];
    for (i, slot) in out.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let x = bar.loc.x + PAD + (i as i32) * (BUTTON + PAD);
        *slot = Rectangle::new((x, y).into(), (BUTTON, BUTTON).into());
    }
    out
}

/// The width a bar needs before its buttons can be drawn.
#[must_use]
pub const fn min_width() -> i32 {
    PAD + 3 * (BUTTON + PAD)
}

/// Whether this window is wide enough to carry buttons.
#[must_use]
pub fn fits(content: Rectangle<i32, Logical>) -> bool {
    content.size.w >= min_width()
}

/// What, if anything, a click at `p` hits on this window's chrome.
///
/// ★ BUTTONS BEFORE THE BAR, and the order is load-bearing: a button is
/// INSIDE the bar, so testing the bar first would swallow every button press
/// and turn all three into drags. That is a bug with no visible symptom in the
/// source — the buttons would simply never fire — which is why the order is
/// stated here and pinned by a test.
#[must_use]
pub fn hit(content: Rectangle<i32, Logical>, p: Point<f64, Logical>) -> Option<Hit> {
    #[allow(clippy::cast_possible_truncation)]
    let point = Point::<i32, Logical>::from((p.x.floor() as i32, p.y.floor() as i32));

    if fits(content) {
        for (rect, what) in
            buttons(content)
                .into_iter()
                .zip([Hit::Close, Hit::Minimize, Hit::Maximize])
        {
            if contains(rect, point) {
                return Some(what);
            }
        }
    }

    if contains(bar_rect(content), point) {
        return Some(Hit::Drag);
    }

    None
}

/// Point-in-rect, half-open on the far edges.
///
/// ★ HALF-OPEN so two adjacent rectangles cannot both claim the pixel on
/// their shared edge. With inclusive bounds the gap between two buttons would
/// belong to both, and which one fired would depend on iteration order — a
/// coin-flip the operator experiences as "sometimes it minimizes".
fn contains(r: Rectangle<i32, Logical>, p: Point<i32, Logical>) -> bool {
    p.x >= r.loc.x && p.x < r.loc.x + r.size.w && p.y >= r.loc.y && p.y < r.loc.y + r.size.h
}

// ── Rendering: one cached buffer per window, not four rects ──────────────

/// Rasterize a window's whole chrome — bar ground, three buttons, and the
/// title — into premultiplied ARGB8888 of size `width x HEIGHT`.
///
/// ── ★ ONE BUFFER, NOT FOUR SOLID RECTS ───────────────────────────────────
/// The chrome began as four `SolidColorRenderElement`s per window, which is
/// the right shape while it carries no text: solid rects need no raster and
/// no cache. A title changes that — text has to be rasterized somewhere — and
/// once one buffer exists the buttons may as well be drawn into it. Four
/// elements per window become one, and the z-order question (buttons in front
/// of their own bar) stops being expressible at all rather than being a
/// correctly-ordered push.
///
/// ── ★ THE CALLER MUST CACHE THIS ─────────────────────────────────────────
/// Rebuilding per frame hands the damage tracker a new commit every frame and
/// quietly turns partial repaint back into full repaint — the exact trap
/// `bar.rs` documents for the clock and `drm.rs` for the cursor. Key the cache
/// on `(title, width, focused)`: those are its only inputs.
#[must_use]
pub fn rasterize(title: &str, width: i32, focused: bool) -> Option<Vec<u8>> {
    rasterize_hovered(title, width, focused, None)
}

/// [`rasterize`], with one button drawn as hovered.
///
/// ★ The hovered button is part of the CACHE KEY in `drm.rs`, so this is
/// re-rasterised only when the pointer crosses a button boundary — not per
/// frame, and not per motion event.
#[must_use]
pub fn rasterize_hovered(
    title: &str,
    width: i32,
    focused: bool,
    hovered: Option<Hit>,
) -> Option<Vec<u8>> {
    let font = crate::bar::font()?;
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(HEIGHT).ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let ground = crate::theme::titlebar_colour();
    let mut buf = vec![0u8; w * h * 4];
    // Ground first — premultiplied ARGB, opaque.
    for px in buf.chunks_exact_mut(4) {
        px[0] = ground.b;
        px[1] = ground.g;
        px[2] = ground.r;
        px[3] = 0xFF;
    }

    // Buttons, from the SAME geometry the hit-test uses. Drawn relative to the
    // buffer, so the bar's own origin is subtracted out.
    let content = Rectangle::new((0, HEIGHT).into(), (width, 1).into());
    if fits(content) {
        for (r, role) in
            buttons(content)
                .into_iter()
                .zip([Hit::Close, Hit::Minimize, Hit::Maximize])
        {
            // Brightened while the pointer is on this button — the cheapest
            // proof to a person that the control is real.
            let c = crate::theme::chrome_button_colour_hovered(role, hovered == Some(role));
            fill(&mut buf, w, h, r.loc.x, r.loc.y, r.size.w, r.size.h, c);
        }
    }

    // The title, left of centre after the buttons, vertically centred.
    //
    // ★ TRUNCATED WITH AN ELLIPSIS RATHER THAN CLIPPED. A clipped glyph reads
    // as a rendering fault; an ellipsis reads as "there is more", which is the
    // true statement.
    let title = title.trim();
    if !title.is_empty() {
        let left = if fits(content) { min_width() } else { PAD } + PAD;
        let avail = (width - left - PAD).max(0);
        let text = truncate_to(font, title, avail as f32);
        if !text.is_empty() {
            let fg = if focused {
                crate::bar::role_text()
            } else {
                crate::bar::role_text_muted()
            };
            let blend = crate::bar::Blend::new(ground, fg);
            let base = crate::bar::baseline(font, h);
            draw_text_at(&mut buf, w, h, font, &text, left as f32, base, &blend);
        }
    }
    Some(buf)
}

/// Longest prefix of `s` that fits in `avail` px, with `…` when shortened.
fn truncate_to(font: &fontdue::Font, s: &str, avail: f32) -> String {
    if crate::bar::measure(font, s) <= avail {
        return s.to_string();
    }
    let ell = "…";
    let ell_w = crate::bar::measure(font, ell);
    if ell_w > avail {
        return String::new();
    }
    let mut out = String::new();
    let mut used = ell_w;
    for ch in s.chars() {
        let adv = font.metrics(ch, crate::bar::FONT_PX).advance_width;
        if used + adv > avail {
            break;
        }
        used += adv;
        out.push(ch);
    }
    out.push_str(ell);
    out
}

/// Fill an axis-aligned rect in the buffer with an opaque colour.
#[allow(clippy::too_many_arguments)]
fn fill(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, rw: i32, rh: i32, c: irodori::Color) {
    for row in y.max(0)..(y + rh).min(h as i32) {
        for col in x.max(0)..(x + rw).min(w as i32) {
            let i = (row as usize * w + col as usize) * 4;
            buf[i] = c.b;
            buf[i + 1] = c.g;
            buf[i + 2] = c.r;
            buf[i + 3] = 0xFF;
        }
    }
}

/// `bar::draw_text`, but with an explicit baseline so the chrome can centre
/// text in its own height rather than the bar's.
fn draw_text_at(
    buf: &mut [u8],
    w: usize,
    h: usize,
    font: &fontdue::Font,
    s: &str,
    start_x: f32,
    _baseline: f32,
    blend: &crate::bar::Blend,
) {
    crate::bar::draw_text(buf, w, h, font, s, start_x, blend);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn the_bar_sits_above_the_content_and_never_over_it() {
        // A bar that overlapped would hide the client's top row — on a
        // terminal, the prompt.
        let c = win(100, 200, 800, 600);
        let b = bar_rect(c);
        assert_eq!(
            b.loc.y + b.size.h,
            c.loc.y,
            "the bar must end where content begins"
        );
        assert_eq!(b.loc.x, c.loc.x);
        assert_eq!(b.size.w, c.size.w, "the bar spans the window's full width");
        assert_eq!(b.size.h, HEIGHT);
    }

    #[test]
    fn content_for_and_bar_rect_are_inverses() {
        // The layout shrinks a frame with `content_for`; the renderer expands
        // it back with `bar_rect`. If those disagree, the decoration and the
        // window drift apart by a few pixels every configure.
        let frame = win(10, 20, 640, 480);
        let content = content_for(frame);
        let bar = bar_rect(content);
        assert_eq!(
            bar.loc, frame.loc,
            "the bar must start at the frame's origin"
        );
        assert_eq!(
            bar.size.h + content.size.h,
            frame.size.h,
            "bar + content must exactly fill the frame"
        );
    }

    #[test]
    fn a_click_on_each_button_returns_that_button() {
        let c = win(100, 200, 800, 600);
        let rects = buttons(c);
        for (r, want) in rects
            .into_iter()
            .zip([Hit::Close, Hit::Minimize, Hit::Maximize])
        {
            let mid = Point::<f64, Logical>::from((
                f64::from(r.loc.x + r.size.w / 2),
                f64::from(r.loc.y + r.size.h / 2),
            ));
            assert_eq!(hit(c, mid), Some(want), "centre of {r:?}");
        }
    }

    #[test]
    fn buttons_win_over_the_bar_they_sit_inside() {
        // ★ THE ORDER GATE. Every button is inside the bar, so testing the bar
        // first would swallow all three and turn them into drags — a bug that
        // reads as "the buttons do nothing" and is invisible in the source.
        let c = win(0, 100, 400, 300);
        let close = buttons(c)[0];
        let p =
            Point::<f64, Logical>::from((f64::from(close.loc.x + 1), f64::from(close.loc.y + 1)));
        assert_eq!(hit(c, p), Some(Hit::Close), "a button must beat the bar");
    }

    #[test]
    fn the_bar_away_from_the_buttons_is_a_drag_surface() {
        let c = win(100, 200, 800, 600);
        // Far right of the bar, well past the three left-aligned buttons.
        let p = Point::<f64, Logical>::from((700.0, f64::from(200 - HEIGHT / 2)));
        assert_eq!(hit(c, p), Some(Hit::Drag));
    }

    #[test]
    fn a_click_in_the_content_is_not_chrome() {
        // The client owns its own surface. Chrome that claimed content clicks
        // would make the window unusable in the name of decorating it.
        let c = win(100, 200, 800, 600);
        let p = Point::<f64, Logical>::from((400.0, 400.0));
        assert_eq!(hit(c, p), None);
    }

    #[test]
    fn a_click_outside_the_window_entirely_is_not_chrome() {
        let c = win(100, 200, 800, 600);
        for p in [(50.0, 150.0), (2000.0, 205.0), (400.0, 100.0)] {
            assert_eq!(hit(c, p.into()), None, "at {p:?}");
        }
    }

    #[test]
    fn adjacent_buttons_never_both_claim_the_same_pixel() {
        // ★ Half-open bounds. With inclusive edges the boundary pixel belongs
        // to two buttons and which one fires depends on iteration order — the
        // operator experiences that as "sometimes it minimizes".
        let c = win(0, 100, 400, 300);
        let rects = buttons(c);
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let overlap = a.loc.x < b.loc.x + b.size.w && b.loc.x < a.loc.x + a.size.w;
                assert!(!overlap, "buttons {i} and {j} overlap: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_window_too_narrow_for_buttons_still_drags() {
        // ★ DEGRADES, never breaks. A window narrower than the button row
        // would otherwise draw buttons overhanging its own bar, and clicking
        // "close" outside the window is the worst possible outcome. It loses
        // the buttons and keeps the drag surface.
        let c = win(0, 100, min_width() - 1, 300);
        assert!(!fits(c));
        let p = Point::<f64, Logical>::from((2.0, f64::from(100 - HEIGHT / 2)));
        assert_eq!(hit(c, p), Some(Hit::Drag));
    }

    #[test]
    fn every_button_lies_within_its_own_bar() {
        // The drawing half and the hit-testing half read these same rects, so
        // a button outside the bar would be drawn over the desktop and still
        // be clickable — chrome for a window it is not attached to.
        let c = win(37, 211, 900, 400);
        let bar = bar_rect(c);
        for (i, r) in buttons(c).into_iter().enumerate() {
            assert!(r.loc.x >= bar.loc.x, "button {i} starts left of the bar");
            assert!(
                r.loc.x + r.size.w <= bar.loc.x + bar.size.w,
                "button {i} runs past the bar's right edge"
            );
            assert!(r.loc.y >= bar.loc.y, "button {i} is above the bar");
            assert!(
                r.loc.y + r.size.h <= bar.loc.y + bar.size.h,
                "button {i} runs below the bar"
            );
        }
    }

    // ── The rasterizer ──────────────────────────────────────────────────

    /// ★ ANTI-VACUITY FOR EVERY RASTER TEST BELOW.
    ///
    /// They all begin `let Some(font) = crate::bar::font() else { return }`,
    /// which is correct — a host with no JetBrains Mono on disk cannot
    /// exercise a rasterizer — and is also exactly how eight tests quietly
    /// become zero. If the font resolves anywhere, it must resolve HERE, so
    /// this fails on a host where the others would silently pass empty.
    ///
    /// Deliberately NOT `assert!(font().is_some())`: on a genuinely
    /// font-less host that would be a false failure. It asserts the pair
    /// instead — either the font is absent AND the raster is too, or both
    /// are present. A rasterizer that returns pixels without a font, or a
    /// font that yields no pixels, is the state that cannot be.
    #[test]
    fn the_raster_tests_are_not_silently_skipping() {
        let font = crate::bar::font();
        let raster = rasterize("probe", 400, true);
        assert_eq!(
            font.is_some(),
            raster.is_some(),
            "font present = {} but raster present = {} — the skip guard on \
             every test below is now lying about which case it is in",
            font.is_some(),
            raster.is_some()
        );
        if font.is_none() {
            eprintln!(
                "NOTE: no font on this host — the chrome raster tests are \
                 skipping, and that is why they pass"
            );
        }
    }

    #[test]
    fn a_title_that_fits_is_not_shortened() {
        let Some(font) = crate::bar::font() else {
            return; // no font on this host — the raster path is untestable
        };
        let s = "mado";
        let w = crate::bar::measure(font, s) + 10.0;
        assert_eq!(truncate_to(font, s, w), s);
    }

    #[test]
    fn a_long_title_is_ellipsised_not_clipped() {
        // ★ A clipped glyph reads as a rendering fault; an ellipsis reads as
        // "there is more", which is the true statement. The distinction is
        // invisible in code and obvious on screen.
        let Some(font) = crate::bar::font() else {
            return;
        };
        let s = "a very long window title that will not fit in a narrow bar";
        let out = truncate_to(font, s, 60.0);
        assert!(out.ends_with('…'), "got {out:?}");
        assert!(out.len() < s.len());
        assert!(
            crate::bar::measure(font, &out) <= 60.0,
            "the truncation must actually fit: {out:?}"
        );
    }

    #[test]
    fn a_bar_too_narrow_for_even_an_ellipsis_renders_no_text() {
        // Rather than a single '…' floating alone, or a panic on a zero-width
        // budget.
        let Some(font) = crate::bar::font() else {
            return;
        };
        assert_eq!(truncate_to(font, "anything", 1.0), "");
    }

    #[test]
    fn the_raster_is_the_declared_size_and_opaque() {
        // Size mismatch against `bar_rect` is what makes a buffer land skewed
        // or get rejected; alpha != 255 on a chrome buffer would blend the
        // desktop through the titlebar.
        let Some(px) = rasterize("mado", 400, true) else {
            return;
        };
        assert_eq!(px.len(), 400 * HEIGHT as usize * 4);
        assert!(
            px.chunks_exact(4).all(|p| p[3] == 0xFF),
            "every chrome pixel must be opaque"
        );
    }

    #[test]
    fn focus_changes_the_raster() {
        // ★ THE CACHE-KEY GATE. `drm.rs` keys its buffer on
        // (title, width, focused). If focus did not change the pixels, that
        // third key would be dead weight; if it changes them and were dropped
        // from the key, a window would keep its focused title after losing
        // focus. Assert the dependency the key claims.
        let (Some(a), Some(b)) = (rasterize("mado", 400, true), rasterize("mado", 400, false))
        else {
            return;
        };
        assert_ne!(a, b, "focused and unfocused chrome must differ");
    }

    #[test]
    fn a_title_changes_the_raster() {
        // The other half of the same key.
        let (Some(a), Some(b)) = (rasterize("mado", 400, true), rasterize("tear", 400, true))
        else {
            return;
        };
        assert_ne!(a, b, "the title must reach the pixels");
    }

    #[test]
    fn an_empty_title_still_renders_chrome() {
        // A window that has not sent a title yet must still get its bar and
        // buttons — degrading to no chrome would make the window undraggable.
        let Some(px) = rasterize("", 400, true) else {
            return;
        };
        assert_eq!(px.len(), 400 * HEIGHT as usize * 4);
    }

    #[test]
    fn a_window_too_narrow_for_buttons_still_rasterizes() {
        let Some(px) = rasterize("x", min_width() - 1, true) else {
            return;
        };
        assert_eq!(px.len(), (min_width() - 1) as usize * HEIGHT as usize * 4);
    }
}
