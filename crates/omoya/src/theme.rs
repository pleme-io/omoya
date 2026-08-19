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

/// The seat's background, linear RGBA, ready for a renderer's clear colour.
///
/// `polar_night[0]` is Nord0 — the same swatch `pleme.theme` lowers onto stylix
/// for every foreign app on the machine, so the compositor's empty desktop and
/// a GTK window's background are the same colour by construction rather than by
/// two people copying the same string.
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
