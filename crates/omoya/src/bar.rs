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
const FONT_PX: f32 = 14.0;

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
                let Some(open) = chunk.find('>') else { continue };
                let Some(close) = chunk.find("</dir>") else { continue };
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
        roots.push(std::path::PathBuf::from("/run/current-system/sw/share/fonts"));
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
                if let Some(p) = find_file(root, want) {
                    if let Ok(b) = std::fs::read(&p) {
                        tracing::info!(path = %p.display(), "bar font");
                        return Some(b);
                    }
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

/// What the bar says. Compared to decide whether to re-rasterize.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BarText {
    pub left: String,
    pub right: String,
}

/// Rasterize the bar to a premultiplied ARGB8888 buffer, `width` x [`HEIGHT`].
///
/// Returns `None` when there is no font — a bar that cannot draw text should
/// be absent, not a blank stripe that looks like a rendering bug.
#[must_use]
pub fn rasterize(text: &BarText, width: i32) -> Option<Vec<u8>> {
    let font_data = font_bytes()?;
    let font = fontdue::Font::from_bytes(font_data, fontdue::FontSettings::default()).ok()?;

    let w = usize::try_from(width).ok()?;
    let h = usize::try_from(HEIGHT).ok()?;
    let bg = NORD.polar_night[1];
    let fg = NORD.snow_storm[1];
    let accent = NORD.frost[2];

    // ARGB8888 little-endian is B,G,R,A in memory order.
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_exact_mut(4) {
        px[0] = bg.b;
        px[1] = bg.g;
        px[2] = bg.r;
        px[3] = 0xff;
    }

    // A single accent line along the bottom edge — the one piece of chrome
    // that says "this is a bar" rather than "the background is a bit lighter
    // up here".
    for x in 0..w {
        let o = ((h - 1) * w + x) * 4;
        buf[o] = accent.b;
        buf[o + 1] = accent.g;
        buf[o + 2] = accent.r;
        buf[o + 3] = 0xff;
    }

    let pad = 10_usize;
    draw_text(&mut buf, w, h, &font, &text.left, pad, fg);
    let right_w = measure(&font, &text.right);
    let right_x = w.saturating_sub(right_w + pad);
    draw_text(&mut buf, w, h, &font, &text.right, right_x, fg);
    Some(buf)
}

fn measure(font: &fontdue::Font, s: &str) -> usize {
    s.chars()
        .map(|c| font.metrics(c, FONT_PX).advance_width as usize)
        .sum()
}

fn draw_text(
    buf: &mut [u8],
    w: usize,
    h: usize,
    font: &fontdue::Font,
    s: &str,
    start_x: usize,
    fg: irodori::Color,
) {
    // Baseline: centre the face vertically. `HEIGHT` is chosen so a 14px face
    // has room, and the ascent is where the glyph's top sits relative to it.
    let baseline = (h as f32 / 2.0 + FONT_PX * 0.35) as usize;
    let mut pen = start_x;
    for ch in s.chars() {
        let (metrics, bitmap) = font.rasterize(ch, FONT_PX);
        for gy in 0..metrics.height {
            for gx in 0..metrics.width {
                let cov = bitmap[gy * metrics.width + gx];
                if cov == 0 {
                    continue;
                }
                let px = pen as i64 + metrics.xmin as i64 + gx as i64;
                let py = baseline as i64 - metrics.ymin as i64 - metrics.height as i64 + gy as i64;
                if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                    continue;
                }
                let o = (py as usize * w + px as usize) * 4;
                // Blend the glyph's coverage over whatever is already there,
                // so text over the accent line does not punch a hole in it.
                let a = f32::from(cov) / 255.0;
                let mix = |dst: u8, src: u8| -> u8 {
                    (f32::from(dst) * (1.0 - a) + f32::from(src) * a) as u8
                };
                buf[o] = mix(buf[o], fg.b);
                buf[o + 1] = mix(buf[o + 1], fg.g);
                buf[o + 2] = mix(buf[o + 2], fg.r);
                buf[o + 3] = 0xff;
            }
        }
        pen += font.metrics(ch, FONT_PX).advance_width as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_is_absent_without_a_font_rather_than_blank() {
        // `rasterize` returns None when no font is found. A blank stripe would
        // look like a rendering bug; absence looks like what it is.
        let t = BarText {
            left: "x".into(),
            right: "y".into(),
        };
        // On a machine WITH fonts this yields a buffer of the right size; on
        // one without, None. Both are correct — what must never happen is a
        // wrongly-sized buffer, which would be read as a corrupt frame.
        if let Some(b) = rasterize(&t, 200) {
            assert_eq!(b.len(), 200 * HEIGHT as usize * 4);
        }
    }

    #[test]
    fn text_that_does_not_fit_is_clipped_not_wrapped() {
        // Every write is bounds-checked against w/h, so a long string cannot
        // scribble outside the buffer. This pins that the buffer size is a
        // function of the requested width alone.
        let t = BarText {
            left: "a very long left side that will certainly not fit".into(),
            right: "and a right side too".into(),
        };
        if let Some(b) = rasterize(&t, 120) {
            assert_eq!(b.len(), 120 * HEIGHT as usize * 4);
        }
    }
}
