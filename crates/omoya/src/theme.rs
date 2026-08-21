//! The compositor's colours — a **projection** of the fleet palette, never a
//! second copy of it.
//!
//! `theory/THEME-ARCHITECTURE.md` settles where Nord lives: it is born in
//! ishou's Rust tokens, which themselves read `irodori::NORD`. So this module
//! contains no hex value. It contains the conversion nobody else can do for us
//! — sRGB bytes to the **linear** floats a compositor's clear colour is
//! specified in.
//!
//! ★ That conversion is the thing a naive compositor gets wrong, and it is
//! wrong in a way that looks *almost* right: skipping it makes the background
//! noticeably paler than every other Nord surface on the machine, which reads
//! as "the theme is slightly off" rather than as a bug with a cause.

use irodori::NORD;

/// One sRGB channel byte to linear float, per the sRGB transfer function.
///
/// The `0.04045` knee and the `2.4` exponent are the sRGB spec's, not a
/// tunable — a "close enough" gamma of 2.2 is visibly wrong in the darks, which
/// is exactly where a Nord background lives.
fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The seat's background as LINEAR RGBA — for a framebuffer whose format is
/// itself sRGB-encoded, where the GPU performs the linear→sRGB conversion on
/// write.
///
/// ★ This is NOT what the nested winit backend wants. See
/// [`background_for_surface`] — read it before using this.
#[must_use]
pub fn background_linear() -> [f32; 4] {
    let c = NORD.polar_night[0];
    [
        srgb_to_linear(c.r),
        srgb_to_linear(c.g),
        srgb_to_linear(c.b),
        1.0,
    ]
}

/// The seat's background as sRGB-normalized RGBA — for a framebuffer that is
/// NOT sRGB-encoded, where whatever we write lands in the pixel unconverted.
#[must_use]
pub fn background_srgb() -> [f32; 4] {
    let c = NORD.polar_night[0];
    [
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        1.0,
    ]
}

/// Pick the encoding a renderer's clear colour must be in, given whether its
/// framebuffer format is sRGB.
///
/// ── WHY THIS IS A FUNCTION AND NOT A CONSTANT (measured 2026-08-19) ────────
/// omoya's first rendered frame on plo came out at **`srgb(7,9,13)`**. Nord0 is
/// `#2E3440` = (46,52,64), and (7,9,13) is *exactly* its LINEAR value written
/// out as raw bytes:
///
///     46/255 → 0.0273 → ×255 =  7
///     52/255 → 0.0343 → ×255 =  9
///     64/255 → 0.0508 → ×255 = 13
///
/// So the paint was right and the ENCODING was wrong: smithay's winit backend
/// reported `Selected color format: … srgb: false`, i.e. a framebuffer that
/// applies no conversion — and linear values written there render far too dark.
///
/// The failure mode is the nasty one this module's header already warned about
/// in the other direction: it does not look broken, it looks like *the theme is
/// slightly off*. The frame is a plausible dark grey; only arithmetic on the
/// measured pixel says which grey it should have been.
///
/// A real DRM/KMS scanout target usually IS sRGB, so the answer genuinely
/// differs per backend. Hence a parameter, not a constant.
#[must_use]
pub fn background_for_surface(format_is_srgb: bool) -> [f32; 4] {
    if format_is_srgb {
        background_linear()
    } else {
        background_srgb()
    }
}

/// The pointer's colour as LINEAR RGBA. See [`cursor_for_surface`].
#[must_use]
pub fn cursor_linear() -> [f32; 4] {
    let c = NORD.snow_storm[2];
    [
        srgb_to_linear(c.r),
        srgb_to_linear(c.g),
        srgb_to_linear(c.b),
        1.0,
    ]
}

/// The pointer's colour as raw sRGB bytes. See [`cursor_for_surface`].
#[must_use]
pub fn cursor_srgb() -> [f32; 4] {
    let c = NORD.snow_storm[2];
    [
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        1.0,
    ]
}

/// The pointer's colour, in whichever encoding this framebuffer needs.
///
/// ★ `snow_storm[2]` — the palette's BRIGHTEST — against a `polar_night[0]`
/// background, because the one job a pointer has is to be findable. Picking a
/// mid-tone from the same family would be prettier and would reproduce the
/// problem this exists to solve.
///
/// It takes the same `format_is_srgb` flag as [`background_for_surface`] and
/// for the same measured reason: writing linear values into a non-sRGB
/// framebuffer renders them far too dark. On a cursor that failure is worse
/// than on a background — a background that is slightly too dark still looks
/// like a background, while a pointer that is too dark looks like no pointer at
/// all, which is indistinguishable from the bug this replaces.
#[must_use]
pub fn cursor_for_surface(format_is_srgb: bool) -> [f32; 4] {
    if format_is_srgb {
        cursor_linear()
    } else {
        cursor_srgb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_background_is_nord_polar_night_zero() {
        let c = NORD.polar_night[0];
        // Nord0 — asserted here so that a palette change is a visible test
        // failure rather than a silent re-theme of every fleet seat.
        assert_eq!((c.r, c.g, c.b), (0x2E, 0x34, 0x40));
    }

    #[test]
    fn srgb_to_linear_matches_the_spec_at_its_anchors() {
        assert!((srgb_to_linear(0) - 0.0).abs() < 1e-6);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-6);
        // Mid-grey is NOT 0.5 in linear space — this is the whole reason the
        // conversion exists, and the value a gamma-2.2 approximation misses.
        let mid = srgb_to_linear(128);
        assert!(mid > 0.21 && mid < 0.22, "sRGB 128 -> linear {mid}");
    }

    #[test]
    fn a_non_srgb_surface_gets_the_raw_nord_bytes() {
        // ★ THE REGRESSION THIS PINS, in its own units.
        //
        // omoya's first frame on plo measured `srgb(7,9,13)` because the
        // LINEAR encoding was handed to a framebuffer that converts nothing.
        // Rendering Nord0 correctly on that surface means writing 46/52/64
        // straight through — so this test asserts the bytes that come back out
        // of a screenshot, which is the only place the bug was ever visible.
        let [r, g, b, a] = background_for_surface(false);
        let byte = |f: f32| (f * 255.0).round() as u8;
        assert_eq!((byte(r), byte(g), byte(b)), (46, 52, 64));
        assert!((a - 1.0).abs() < f32::EPSILON);

        // And the wrong answer is genuinely the observed one — this asserts the
        // BUG reproduces from the code, so a future refactor that reintroduces
        // it fails here rather than in a screenshot nobody takes.
        let [lr, lg, lb, _] = background_for_surface(true);
        assert_eq!((byte(lr), byte(lg), byte(lb)), (7, 9, 13));
    }

    #[test]
    fn the_background_is_dark_in_linear_space() {
        let [r, g, b, a] = background_linear();
        assert!((a - 1.0).abs() < f32::EPSILON, "opaque");
        for ch in [r, g, b] {
            assert!(ch < 0.06, "Nord0 must stay dark after conversion, got {ch}");
        }
        // And it is bluish: Nord0's blue channel leads.
        assert!(b > r, "Nord0 is a cool grey — blue leads red");
    }
}

/// The focused window's border, in whichever encoding the framebuffer needs.
///
/// ★ `frost[2]` — the palette's ACCENT family, not a foreground tone.
/// A border's job is to answer "where does my typing go" at a glance, from
/// across a room, without being read. `snow_storm` would be brighter but is
/// the TEXT colour, so a border in it competes with the content it frames;
/// `polar_night` would be tasteful and invisible against the background.
/// Frost is the family Nord reserves for accent precisely so that a UI has
/// somewhere to put "this one".
///
/// Takes the same `format_is_srgb` flag as its siblings and for the same
/// measured reason — see [`background_for_surface`]. On a border the failure
/// is the cursor's, in miniature: a border too dark to see is a border that
/// is not there.
#[must_use]
pub fn focus_border_for_surface(format_is_srgb: bool) -> [f32; 4] {
    let c = NORD.frost[2];
    if format_is_srgb {
        [
            f32::from(c.r) / 255.0,
            f32::from(c.g) / 255.0,
            f32::from(c.b) / 255.0,
            1.0,
        ]
    } else {
        [
            srgb_to_linear(c.r),
            srgb_to_linear(c.g),
            srgb_to_linear(c.b),
            1.0,
        ]
    }
}
