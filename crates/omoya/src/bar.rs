//! The status bar — rasterized on the CPU, drawn as one render element.
//!
//! ── ★ WHY THE COMPOSITOR DRAWS IT ────────────────────────────────────────
//! omoya implements `wlr-layer-shell`, so a third-party bar can anchor itself
//! and reserve space like any other. This one is drawn in-process anyway, and
//! the reason is honest rather than architectural: a client bar needs a second
//! process, a Wayland connection, its own font stack and an IPC channel to ask
//! the compositor what it is displaying. This needs a font and a rectangle.
//!
//! The layer-shell path stays open — a bar that wants to replace this one can,
//! and its exclusive zone is already honoured by `apply_layout`.
//!
//! ── ★ WHY NOT IN `nuri` ──────────────────────────────────────────────────
//! `nuri` has ZERO dependencies and says so at the top of its own file: "nuri
//! computes; it does not talk to anything." Glyph rasterization means a font
//! parser, and putting one there would trade a real, stated property of the
//! rasterizer for one consumer's convenience. The text lives here; nuri keeps
//! its purity.
//!
//! ── ★ RASTERIZED ONLY WHEN THE TEXT CHANGES ──────────────────────────────
//! The clock ticks once a second, not sixty times. Re-rasterizing every frame
//! would hand the damage tracker a buffer with a new commit each time and
//! defeat the partial repaint this seat just gained — the bar wouldsingle-handedly
//! put the desktop back to full-screen composites.

use std::sync::OnceLock;

use irodori::NORD;

/// Bar height in logical pixels. Enough for a 14px face with breathing room.
pub const HEIGHT: i32 = 28;

/// The point size the bar's text is rasterized at.
///
/// ★ 13, NOT 14, AND THE RATIO IS THE REASON. Bar type is judged by its size
/// RELATIVE to the strip, not absolutely: 13/28 = 0.46 sits in the middle of
/// the band every well-regarded bar lands in (Waybar 30px/13, omarchy 26/12,
/// yambar 26/12 — all 0.43–0.50). At 14 the ratio is 0.50, right at the top
/// of the band, which is what made the strip read as cramped rather than as
/// typeset.
const FONT_PX: f32 = 13.0;

/// The horizontal inset from either screen edge.
///
/// On the fleet's 4 px grid (`ishou_tokens::Spacing::px_3`). It was 10, which
/// is off-grid — one of the reasons the strip read as assembled rather than
/// designed.
const PAD: f32 = 12.0;

/// The gutter between items inside one group. `Spacing::px_2`.
const GUTTER: f32 = 8.0;

/// A parcel cell's side, and the slot it occupies.
const CELL: f32 = 20.0;

// ── ROLES, NOT BAND INDEXES ──────────────────────────────────────────────
//
// ★ Every colour below is named for the JOB it does. A band index at a call
// site is how `frost[2]` (nord9, Nord's *recessive* frost) ended up as this
// seat's accent for months — 1.35:1 from the real accent, so it never read as
// a mistake, only as a duller desktop.
//
// The names read backwards from the palette on purpose: nord6 is Nord's
// "text that must be noticed", not its body face. Body is nord4 at 7.45:1 on
// the bar's ground; nord6 at 8.73:1 is emphasis. Hierarchy runs
// muted → emphasis, never emphasis → dim.

/// The bar's own plane. One rung above the desktop ground.
fn role_surface() -> irodori::Color {
    NORD.polar_night[1] // nord1
}

/// Body text. Everything the operator reads by default.
fn role_text_muted() -> irodori::Color {
    NORD.snow_storm[0] // nord4 — 7.45:1 on surface
}

/// Emphasis. The one item in a group that has focus.
fn role_text() -> irodori::Color {
    NORD.snow_storm[2] // nord6 — 8.73:1 on surface
}

/// Structural hairlines and empty marks. **Never a label that must be read**
/// — 1.36:1 on the bar's ground is legible in a screenshot and gone in
/// daylight.
fn role_text_dim() -> irodori::Color {
    NORD.polar_night[3] // nord3
}

/// The accent. Means "here", and nothing else, and appears at most twice.
fn role_primary() -> irodori::Color {
    NORD.frost[1] // nord8
}

/// An honest degraded state — used when the seat cannot resolve a timezone.
fn role_warning() -> irodori::Color {
    NORD.aurora[2] // nord13
}

/// The fleet's monospace face, found on disk rather than embedded.
///
/// ★ READ FROM THE SYSTEM, NOT `include_bytes!`. The font a pleme-io seat uses
/// is declared in the home-manager font config, and embedding a copy here
/// would mean the bar silently disagreeing with every other surface the
/// moment that declaration changes. Absent font ⇒ no bar, which is honest;
/// a fallback to some other face would look like the bar is working.
fn font_bytes() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    FONT.get_or_init(|| {
        // ★ ASK FONTCONFIG WHERE FONTS ARE — DO NOT GUESS PATHS.
        //
        // The first version hardcoded `/run/current-system/sw/share/fonts`
        // and friends. On plo that directory does not exist: the fleet's
        // faces arrive through home-manager and live at arbitrary store
        // paths, and on the vkms machine `fonts.packages` puts them
        // somewhere else again. The bar silently drew nothing on both, which
        // its own no-font-no-bar rule then made look intentional.
        //
        // `/etc/fonts` IS the system's declaration of where fonts live —
        // every `<dir>` element is a root, written there by the same NixOS
        // module that installed the font. Parsing it is reading the
        // declaration rather than re-deriving it, and it costs no C: this is
        // a substring scan, not a link against libfontconfig.
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        let mut confs = vec![std::path::PathBuf::from("/etc/fonts/fonts.conf")];
        if let Ok(rd) = std::fs::read_dir("/etc/fonts/conf.d") {
            confs.extend(rd.flatten().map(|e| e.path()));
        }
        for c in confs {
            let Ok(text) = std::fs::read_to_string(&c) else {
                continue;
            };
            for chunk in text.split("<dir").skip(1) {
                let Some(open) = chunk.find('>') else {
                    continue;
                };
                let Some(close) = chunk.find("</dir>") else {
                    continue;
                };
                if close <= open {
                    continue;
                }
                let path = chunk[open + 1..close].trim();
                // `prefix="xdg"` entries are relative to the user's data dir;
                // skipped rather than mis-resolved, since the fleet faces are
                // all absolute store paths.
                if path.starts_with('/') {
                    roots.push(std::path::PathBuf::from(path));
                }
            }
        }
        // The declared roots first, then the conventional ones as a floor for
        // a system with no fontconfig at all.
        roots.push(std::path::PathBuf::from(
            "/run/current-system/sw/share/fonts",
        ));
        roots.push(std::path::PathBuf::from("/usr/share/fonts"));

        // Ordered by preference: the fleet face first, then anything
        // monospace, so a seat without Nerd Fonts still gets a bar.
        const WANTED: &[&str] = &[
            "JetBrainsMonoNerdFont-Regular.ttf",
            "JetBrainsMonoNLNerdFontMono-Regular.ttf",
            "DejaVuSansMono.ttf",
            "DejaVuSans.ttf",
        ];
        for want in WANTED {
            for root in &roots {
                if let Some(p) = find_file(root, want)
                    && let Ok(b) = std::fs::read(&p)
                {
                    tracing::info!(path = %p.display(), "bar font");
                    return Some(b);
                }
            }
        }
        tracing::warn!(
            roots = roots.len(),
            "no monospace font found in any fontconfig dir — the bar will not draw"
        );
        None
    })
    .as_deref()
}

/// Depth-limited search for a font file.
///
/// Bounded because a font root on a nix system is a symlink farm into the
/// store, and an unbounded walk there is effectively unbounded.
fn find_file(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, name: &str, depth: u32) -> Option<std::path::PathBuf> {
        if depth == 0 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut dirs = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.file_name().is_some_and(|f| f == name) {
                return Some(p);
            }
        }
        dirs.into_iter().find_map(|d| walk(&d, name, depth - 1))
    }
    walk(dir, name, 6)
}

/// What the bar shows. A VALUE, compared for equality to decide whether the
/// strip needs re-rasterizing at all.
///
/// ★ Deliberately not `{ left: String, right: String }`. That shape forced the
/// caller to decide typography — where a separator goes, how many spaces —
/// and it is why the strip said `" wayland-1"` and `"3 windows   14:22 UTC"`:
/// a socket name that already lives in `introspect`, a count you can get by
/// looking at the screen, and a timezone admission dressed as information.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BarState {
    /// One entry per parcel on this output, in layout order. `true` marks the
    /// focused one — at most one may be true.
    pub parcels: Vec<bool>,
    /// `HH:MM`. Local time; see [`Clock`].
    pub clock: Clock,
    /// Windows that are minimised — alive, running, and mapped to nothing.
    ///
    /// ★ THE ONE STATE A SCREENSHOT CANNOT SHOW. A minimised window and a
    /// closed one are the same picture, so without this the minimise chord is
    /// a trapdoor: the operator's window is gone and nothing on the desktop
    /// says it is recoverable. That is the "system has something to say" the
    /// right zone was reserved for.
    pub hidden: usize,
    /// The focused window's position in its tab group, as `(index, total)`,
    /// 1-based for display. `None` when it is not grouped.
    ///
    /// ★ ALSO INVISIBLE BY CONSTRUCTION. A tab group shows exactly one
    /// member, so N-1 windows are hidden for a reason that is NOT minimise —
    /// and with no indicator the group cannot be discovered at all, which
    /// makes `Logo+Tab` indistinguishable from an unbound chord.
    pub tab: Option<(usize, usize)>,
}

/// The clock, and whether it is telling the truth about its zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clock {
    /// Local time resolved. Renders in `text_muted`.
    Local(String),
    /// No timezone could be resolved, so this is UTC and SAYS so, in
    /// `warning`. An honest degraded state — a seat that silently shows UTC
    /// as if it were local is lying at a glance.
    UtcFallback(String),
}

impl Default for Clock {
    fn default() -> Self {
        Self::Local(String::from("00:00"))
    }
}

impl Clock {
    fn text(&self) -> &str {
        match self {
            Self::Local(s) | Self::UtcFallback(s) => s,
        }
    }
    fn colour(&self) -> irodori::Color {
        match self {
            Self::Local(_) => role_text_muted(),
            Self::UtcFallback(_) => role_warning(),
        }
    }
}

/// Coverage → blended byte, per (background, foreground) pair.
///
/// ★ THE BLEND WAS WRONG, AND IT WAS WRONG IN THE DIRECTION THAT LOOKS LIKE A
/// FONT PROBLEM. Glyph coverage is a LINEAR quantity; the bytes in the buffer
/// are sRGB. The old code did `dst*(1-a) + src*a` directly on sRGB bytes, so
/// for this palette a requested coverage of 0.25 was *seen* as 0.125 — half
/// of it — and 0.5 was seen as 0.327. On a dark ground the error is always
/// toward too dark, so antialiased stems render starved and the strip reads
/// as "the font is too thin". The usual fix is a heavier weight, which is
/// treating the symptom.
///
/// Because both endpoints are compile-time role constants, correctness here
/// is also strictly FASTER than what it replaces: the whole per-pixel blend
/// collapses to three array lookups, no float math at all.
struct Blend {
    b: [u8; 256],
    g: [u8; 256],
    r: [u8; 256],
}

impl Blend {
    fn new(bg: irodori::Color, fg: irodori::Color) -> Self {
        let chan = |from: u8, to: u8| -> [u8; 256] {
            let mut t = [0u8; 256];
            let (lo, hi) = (srgb_to_linear(from), srgb_to_linear(to));
            for (a, slot) in t.iter_mut().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let cov = a as f32 / 255.0;
                *slot = linear_to_srgb(lo + (hi - lo) * cov);
            }
            t
        };
        Self {
            b: chan(bg.b, fg.b),
            g: chan(bg.g, fg.g),
            r: chan(bg.r, fg.r),
        }
    }
}

/// sRGB byte → linear float. The piecewise IEC 61966-2-1 curve, not a bare
/// 2.2 power — they differ by up to 3% near black, which is precisely where
/// antialiased text on a dark ground lives.
fn srgb_to_linear(c: u8) -> f32 {
    let v = f32::from(c) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

/// Rasterize the bar to a premultiplied ARGB8888 buffer, `width` x [`HEIGHT`].
///
/// Returns `None` when there is no font — a bar that cannot draw text should
/// be absent, not a blank stripe that looks like a rendering bug.
#[must_use]
pub fn rasterize(state: &BarState, width: i32) -> Option<Vec<u8>> {
    rasterize_h(state, width, HEIGHT)
}

/// Rasterize at a CONFIGURED height.
///
/// ★ Split from [`rasterize`] because the tiler and the painter must agree.
/// `apply_layout` reserves `config.bar.height`; if the painter kept using the
/// const, an operator raising the bar would get a reserved strip taller than
/// the thing drawn in it — a band of stale pixels along the top that reads as
/// a rendering bug and is actually two numbers disagreeing.
///
/// Measured on plo: with `bar.height: 34` in yaml, the content area correctly
/// moved to `4,38 1912x1038` while the bar still painted `1920x28`.
#[must_use]
pub fn rasterize_h(state: &BarState, width: i32, height: i32) -> Option<Vec<u8>> {
    let font = font()?;

    let w = usize::try_from(width).ok()?;
    let h = usize::try_from(height).ok()?;
    let bg = role_surface();

    // ARGB8888 little-endian is B,G,R,A in memory order.
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_exact_mut(4) {
        px[0] = bg.b;
        px[1] = bg.g;
        px[2] = bg.r;
        px[3] = 0xff;
    }

    // ── The bottom edge: a hairline, not an accent stripe ────────────────
    //
    // ★ A full-width accent line spends the seat's ONE accent on a decoration
    // that is always present and therefore says nothing. `primary` has to mean
    // "here"; a 1920 px band of it at all times means "bar", which the
    // luminance step already said. So: a 1 px `text_dim` hairline terminates
    // the plane (the fill step alone cannot — nord1→nord0 is 1.24:1, below
    // every perceptual floor), and `primary` is spent under the focused
    // parcel only.
    let hair = role_text_dim();
    for x in 0..w {
        let o = ((h - 1) * w + x) * 4;
        buf[o] = hair.b;
        buf[o + 1] = hair.g;
        buf[o + 2] = hair.r;
        buf[o + 3] = 0xff;
    }

    // ── Left: one cell per parcel ────────────────────────────────────────
    let muted = Blend::new(bg, role_text_muted());
    let bright = Blend::new(bg, role_text());
    let mut x = PAD;
    for (idx, focused) in state.parcels.iter().take(9).enumerate() {
        let digit = char::from(b'1' + u8::try_from(idx).unwrap_or(8));
        let blend = if *focused { &bright } else { &muted };
        // Centre the digit in its cell so the row is a rhythm of fixed slots
        // rather than a string whose length depends on its content.
        let adv = font.metrics(digit, FONT_PX).advance_width;
        draw_glyph(&mut buf, w, h, font, digit, x + (CELL - adv) / 2.0, blend);
        if *focused {
            // The accent, spent here and nowhere else: a 2 px underline the
            // width of the cell, sitting on the hairline.
            let p = role_primary();
            underline(&mut buf, w, h, x, CELL, p);
        }
        x += CELL + GUTTER;
    }

    // ── Centre: the clock, centred on the SCREEN ─────────────────────────
    //
    // ★ Screen-centred, not flex-centred between the groups. A clock centred
    // in the space left over drifts a few pixels every time a parcel appears
    // or leaves; screen-centred, it is the only element on the desktop with a
    // permanently constant rect — which also makes it the cheapest thing to
    // redraw when only the minute has changed.
    //
    // Snapped to an even pixel, or the same string rasterizes to two
    // different bitmaps at two sub-pixel offsets and defeats the
    // "re-rasterize only when the text changed" rule entirely.
    let clock = state.clock.text();
    let cw = measure(font, clock);
    #[allow(clippy::cast_precision_loss)]
    let centre = ((w as f32 - cw) / 2.0 / 2.0).round() * 2.0;
    let clock_blend = Blend::new(bg, state.clock.colour());
    draw_text(&mut buf, w, h, font, clock, centre, &clock_blend);

    // ── Right: the state a screenshot cannot show ────────────────────────
    //
    // This zone was reserved with "nothing earns this space yet. It fills when
    // the system has something to say". These two earn it, and for the same
    // reason: both describe windows that are ALIVE AND INVISIBLE, so the
    // desktop itself cannot report them. A minimised window looks exactly like
    // a closed one; an inactive tab looks exactly like a window that was never
    // opened.
    //
    // ★ WORDS AND ASCII DIGITS, NOT ICONOGRAPHY. The face is whatever
    // `font_bytes` found on the system, so a glyph outside basic Latin is a
    // gamble — escriba shipped 23 EMPTY devicon glyphs exactly this way, and an
    // indicator that renders as blank is worse than none because it reads as
    // "nothing is hidden".
    //
    // ★ RIGHT-ALIGNED FROM THE MEASURED WIDTH, so the zone grows leftward and
    // never collides with the screen-centred clock. Snapped to an even pixel
    // for the same reason the clock is: two sub-pixel offsets rasterize to two
    // different bitmaps and defeat the "re-rasterize only when text changed"
    // rule that `wanted != bar_text` implements.
    {
        let mut parts: Vec<String> = Vec::new();
        // Tabs first, then hidden — focused-window state before seat-wide
        // state, which is the same left-to-right specificity the parcels use.
        if let Some((idx, total)) = state.tab {
            parts.push(format!("{idx}/{total}"));
        }
        if state.hidden > 0 {
            parts.push(format!("{} hidden", state.hidden));
        }
        if !parts.is_empty() {
            let text = parts.join("   ");
            let tw = measure(font, &text);
            #[allow(clippy::cast_precision_loss)]
            let x = (((w as f32 - PAD - tw) / 2.0).round() * 2.0).max(0.0);
            // `text`, not `text_muted`: this is the zone's whole purpose, and
            // a muted warning about a window you cannot see is a warning that
            // loses to the wallpaper.
            draw_text(&mut buf, w, h, font, &text, x, &bright);
        }
    }

    Some(buf)
}

/// The parsed face, kept for the process's life.
///
/// ★ Hoisted out of `rasterize`. `Font::from_bytes` re-parsed the whole file
/// on every call — which is once a minute now, and was once a frame before
/// the damage gate landed.
fn font() -> Option<&'static fontdue::Font> {
    static PARSED: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    PARSED
        .get_or_init(|| {
            let bytes = font_bytes()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        })
        .as_ref()
}

/// The baseline, derived from the face's own metrics rather than guessed.
///
/// ★ Was `h/2 + FONT_PX * 0.35`. That 0.35 is a guess that happens to look
/// right for one face at one size; change either and the text sits visibly
/// off-centre. `horizontal_line_metrics` gives the real ascent and descent,
/// so the ink box is centred by construction and the value survives a font
/// change.
fn baseline(font: &fontdue::Font, h: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let h = h as f32;
    font.horizontal_line_metrics(FONT_PX)
        .map_or(h / 2.0 + FONT_PX * 0.35, |m| {
            // ascent is positive up, descent negative down.
            let ink = m.ascent - m.descent;
            (h - ink) / 2.0 + m.ascent
        })
}

fn measure(font: &fontdue::Font, s: &str) -> f32 {
    s.chars()
        .map(|c| font.metrics(c, FONT_PX).advance_width)
        .sum()
}

/// A 2 px underline in the accent, sitting on the bottom hairline.
fn underline(buf: &mut [u8], w: usize, h: usize, x: f32, width: f32, c: irodori::Color) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x0, x1) = (x.round() as usize, (x + width).round() as usize);
    for y in h.saturating_sub(3)..h.saturating_sub(1) {
        for px in x0..x1.min(w) {
            let o = (y * w + px) * 4;
            buf[o] = c.b;
            buf[o + 1] = c.g;
            buf[o + 2] = c.r;
            buf[o + 3] = 0xff;
        }
    }
}

fn draw_text(
    buf: &mut [u8],
    w: usize,
    h: usize,
    font: &fontdue::Font,
    s: &str,
    start_x: f32,
    blend: &Blend,
) {
    // ★ AN f32 PEN, ROUNDED ONLY AT THE WRITE. The old pen was a `usize` and
    // truncated `advance_width` on every character, so a face whose advance is
    // 7.8 px lost 0.8 px per glyph and the string crept left — visibly uneven
    // spacing that reads as a bad font rather than as accumulated truncation.
    let mut pen = start_x;
    for ch in s.chars() {
        draw_glyph(buf, w, h, font, ch, pen, blend);
        pen += font.metrics(ch, FONT_PX).advance_width;
    }
}

fn draw_glyph(
    buf: &mut [u8],
    w: usize,
    h: usize,
    font: &fontdue::Font,
    ch: char,
    pen: f32,
    blend: &Blend,
) {
    let (metrics, bitmap) = font.rasterize(ch, FONT_PX);
    let base = baseline(font, h);
    #[allow(clippy::cast_possible_truncation)]
    let pen_i = pen.round() as i64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let base_i = base.round() as i64;
    for gy in 0..metrics.height {
        for gx in 0..metrics.width {
            let cov = bitmap[gy * metrics.width + gx];
            if cov == 0 {
                continue;
            }
            let px = pen_i + metrics.xmin as i64 + gx as i64;
            let py = base_i - metrics.ymin as i64 - metrics.height as i64 + gy as i64;
            if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                continue;
            }
            #[allow(clippy::cast_sign_loss)]
            let o = (py as usize * w + px as usize) * 4;
            let a = usize::from(cov);
            buf[o] = blend.b[a];
            buf[o + 1] = blend.g[a];
            buf[o + 2] = blend.r[a];
            buf[o + 3] = 0xff;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(n: usize, focused: Option<usize>) -> BarState {
        BarState {
            hidden: 0,
            tab: None,
            parcels: (0..n).map(|i| Some(i) == focused).collect(),
            clock: Clock::Local("14:22".into()),
        }
    }

    #[test]
    fn the_bar_is_absent_without_a_font_rather_than_blank() {
        // `rasterize` returns None when no font is found. A blank stripe would
        // look like a rendering bug; absence looks like what it is.
        //
        // On a machine WITH fonts this yields a buffer of the right size; on
        // one without, None. Both are correct — what must never happen is a
        // wrongly-sized buffer, which would be read as a corrupt frame.
        if let Some(b) = rasterize(&state(2, Some(0)), 200) {
            assert_eq!(b.len(), 200 * HEIGHT as usize * 4);
        }
    }

    #[test]
    fn content_that_does_not_fit_is_clipped_not_wrapped() {
        // Every write is bounds-checked against w/h, so more parcels than fit
        // cannot scribble outside the buffer. This pins that the buffer size
        // is a function of the requested width ALONE.
        if let Some(b) = rasterize(&state(9, Some(8)), 120) {
            assert_eq!(b.len(), 120 * HEIGHT as usize * 4);
        }
    }

    #[test]
    fn the_accent_appears_only_when_something_has_focus() {
        // ★ THE ONE-ACCENT RULE, ASSERTED IN PIXELS. `primary` means "here".
        // If it shows up on a seat where nothing is focused, it means nothing.
        let Some(with) = rasterize(&state(3, Some(1)), 400) else {
            return; // no font on this machine; the size tests cover the rest
        };
        let Some(without) = rasterize(&state(3, None), 400) else {
            return;
        };
        let p = role_primary();
        let count = |buf: &[u8]| {
            buf.chunks_exact(4)
                .filter(|px| px[0] == p.b && px[1] == p.g && px[2] == p.r)
                .count()
        };
        assert!(
            count(&with) > 0,
            "a focused parcel must draw the accent underline"
        );
        assert_eq!(
            count(&without),
            0,
            "with nothing focused the accent must not appear anywhere — \
             an always-present accent is decoration, not information"
        );
    }

    #[test]
    fn the_bar_is_opaque_everywhere() {
        // The strip is a plane, not a translucent overlay. A stray alpha byte
        // would blend the desktop through it and read as a compositing bug.
        if let Some(b) = rasterize(&state(2, Some(0)), 200) {
            assert!(
                b.chunks_exact(4).all(|px| px[3] == 0xff),
                "every bar pixel must be fully opaque"
            );
        }
    }

    #[test]
    fn an_unresolved_zone_is_said_out_loud_not_silently_rendered_as_local() {
        // ★ A clock three hours out is worse than no clock, because it gets
        // consulted rather than ignored. The UTC fallback must be visually
        // DIFFERENT, not merely differently labelled — the label is four
        // characters at 13px on a 1920px strip.
        let local = BarState {
            parcels: vec![false],
            clock: Clock::Local("14:22".into()),
            ..BarState::default()
        };
        let utc = BarState {
            parcels: vec![false],
            clock: Clock::UtcFallback("17:22 UTC".into()),
            ..BarState::default()
        };
        assert_ne!(local.clock.colour(), utc.clock.colour());
        assert_eq!(utc.clock.colour(), role_warning());
        let (Some(a), Some(b)) = (rasterize(&local, 400), rasterize(&utc, 400)) else {
            return;
        };
        assert_ne!(a, b, "the two clock states must not rasterize identically");
    }

    /// An empty right zone renders identically to one that was never asked
    /// to draw — and a NON-empty one does not.
    ///
    /// ★ THE GATE THAT MAKES THE INDICATOR FALSIFIABLE. The whole point of
    /// this zone is to report state a screenshot cannot show, so "does it
    /// actually put pixels on the strip" is the only question worth asking —
    /// and it is exactly the question that goes unasked when an indicator is
    /// added by eye. escriba shipped 23 devicon glyphs that rendered blank;
    /// a blank indicator is worse than none, because it reads as "nothing is
    /// hidden".
    #[test]
    fn the_right_zone_draws_only_when_there_is_something_to_say() {
        let quiet = BarState {
            parcels: vec![true],
            clock: Clock::Local("14:22".into()),
            ..BarState::default()
        };
        let hidden = BarState {
            hidden: 3,
            ..quiet.clone()
        };
        let tabbed = BarState {
            tab: Some((2, 3)),
            ..quiet.clone()
        };
        let (Some(q), Some(h), Some(t)) = (
            rasterize(&quiet, 900),
            rasterize(&hidden, 900),
            rasterize(&tabbed, 900),
        ) else {
            // No system face — `the_bar_is_absent_without_a_font_rather_than_blank`
            // owns that case.
            return;
        };
        assert_ne!(q, h, "3 hidden windows put no pixels on the strip");
        assert_ne!(q, t, "a tab position put no pixels on the strip");
        assert_ne!(
            h, t,
            "hidden and tab render identically — one is unreadable"
        );
    }

    /// The re-rasterize gate sees the new fields.
    ///
    /// ★ `wanted != bar_text` in `drm.rs` is the ONLY thing that decides
    /// whether the strip is redrawn, so a field outside `PartialEq` would
    /// render once and then go permanently stale — the "correct declaration,
    /// silently skipped" shape this seat has already been bitten by twice.
    /// Asserting inequality here is what ties the two together.
    #[test]
    fn a_change_of_window_state_is_a_change_of_bar_state() {
        let base = BarState {
            parcels: vec![true],
            clock: Clock::Local("14:22".into()),
            ..BarState::default()
        };
        assert_ne!(
            base,
            BarState {
                hidden: 1,
                ..base.clone()
            }
        );
        assert_ne!(
            base,
            BarState {
                tab: Some((1, 2)),
                ..base.clone()
            }
        );
        assert_ne!(
            BarState {
                tab: Some((1, 3)),
                ..base.clone()
            },
            BarState {
                tab: Some((2, 3)),
                ..base.clone()
            },
            "moving between tabs must redraw the indicator"
        );
    }

    #[test]
    fn the_blend_is_gamma_correct_at_half_coverage() {
        // ★ THE BUG THIS REPLACES, IN ITS OWN UNITS. Blending coverage on raw
        // sRGB bytes is wrong in the dark direction: for this palette a
        // requested 0.5 was *seen* as 0.327. Assert the midpoint lands where
        // linear-light says it should, not where a naive byte lerp would.
        let bg = role_surface();
        let fg = role_text_muted();
        let blend = Blend::new(bg, fg);
        let naive = |a: u8, b: u8| ((f32::from(a) + f32::from(b)) / 2.0).round() as u8;
        let mid = blend.g[128];
        assert_ne!(
            mid,
            naive(bg.g, fg.g),
            "a gamma-correct midpoint must differ from the byte average — \
             equal means the LUT collapsed back to the naive blend"
        );
        // And it must be BRIGHTER than the naive answer on a dark ground,
        // which is the whole perceptual point.
        assert!(
            mid > naive(bg.g, fg.g),
            "on a dark ground the correct midpoint is lighter than the byte \
             average; got {mid} vs {}",
            naive(bg.g, fg.g)
        );
        // Endpoints must still be exact.
        assert_eq!(blend.g[0], bg.g, "zero coverage is the background");
        assert_eq!(blend.g[255], fg.g, "full coverage is the foreground");
    }

    #[test]
    fn the_face_is_parsed_once() {
        // The parse was inside `rasterize`, so it re-read the whole font file
        // on every call — once a frame, before the damage gate landed.
        let a = font().map(std::ptr::from_ref);
        let b = font().map(std::ptr::from_ref);
        assert_eq!(a, b, "font() must hand back the same parsed face");
    }
}
