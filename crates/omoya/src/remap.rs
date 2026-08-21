//! Keycode remapping, applied before xkb sees the key.
//!
//! ★ **WHY NOT AN XKB OPTION.** The obvious way to get CapsLock→Escape is
//! `XkbConfig { options: Some("caps:escape") }`, and on a normal compositor
//! that is the right answer. It does not work here, and the way it fails is
//! the reason this module exists rather than a one-line config change.
//!
//! omoya replaces `xkbcommon` with `xkbcommon-hairetsu`, a pure-Rust keymap
//! (the seat links no `libxkbcommon`). Its `Keymap::new_from_names` signature
//! is:
//!
//! ```ignore
//! pub fn new_from_names<S>(_context, _rules, _model, layout, variant,
//!                          _options: Option<String>, _flags) -> Option<Self>
//! ```
//!
//! `_options` — underscore-prefixed, **ignored entirely**. Passing
//! `"caps:escape"` would not error and would not panic. It would compile the
//! plain `us` keymap and silently do nothing, and the config would look
//! correct in every file that declares it. That is the exact failure shape
//! this seat has been finding all day: a setting that reads as applied and is
//! inert.
//!
//! So the remap happens HERE, above xkb, where it is typed, testable, and
//! true. When hairetsu grows real option support this module is the thing to
//! delete — and `the_remap_is_still_needed` is the test that will say so.
//!
//! ★ **It is a KEYCODE remap, not a keysym remap, and that is deliberate.**
//! Rewriting the code before xkb means everything downstream — the chord
//! filter, `awase`'s bindings, the client's own keymap — sees a real Escape.
//! Remapping the keysym afterwards would leave the chord layer still seeing
//! CapsLock, so `Ctrl+[` style bindings and any client reading raw keycodes
//! would disagree with the operator's fingers.

use smithay::backend::input::Keycode;

/// evdev `KEY_CAPSLOCK`. XKB keycodes are evdev + 8, so 58 + 8 = 66.
pub const EVDEV_CAPSLOCK: u32 = 58;
/// evdev `KEY_ESC` → XKB 9.
pub const EVDEV_ESC: u32 = 1;

/// The offset between an evdev code and the XKB keycode xkb expects.
///
/// Stated once, here, because it is applied in three places (the evdev
/// backend, the synthetic-input path, and this table) and a mismatch between
/// any two of them produces the wrong letter rather than an error.
pub const XKB_OFFSET: u32 = 8;

/// The seat's default remaps, as `(from_evdev, to_evdev)`.
///
/// ★ CapsLock is Escape. It is a default rather than an option because a key
/// that toggles a mode nobody wants, sitting on the home row under the
/// strongest finger, is the single worst-placed key on the board — and every
/// operator on this fleet remaps it anyway. Making it the default means the
/// seat ships correct instead of shipping a footgun plus a note.
///
/// **CapsLock's locking behaviour is gone entirely, not merely rebound.** The
/// code never reaches xkb, so no `Lock` modifier is ever latched and the LED
/// never lights. That is the intent: a half-remapped CapsLock that still
/// toggles a hidden lock is worse than either extreme.
pub const DEFAULT_REMAPS: &[(u32, u32)] = &[(EVDEV_CAPSLOCK, EVDEV_ESC)];

/// Apply the seat's remaps to an XKB keycode.
///
/// Takes and returns XKB keycodes (evdev + 8) because that is what
/// `Omoya::key` handles — doing it in evdev space would mean remapping in the
/// backend AND in the synthetic path, i.e. two places that can disagree.
#[must_use]
pub fn apply(code: Keycode) -> Keycode {
    let raw = code.raw();
    for (from, to) in DEFAULT_REMAPS {
        if raw == from + XKB_OFFSET {
            return Keycode::new(to + XKB_OFFSET);
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capslock_becomes_escape() {
        // 58 + 8 = 66 in, 1 + 8 = 9 out.
        assert_eq!(apply(Keycode::new(66)).raw(), 9);
    }

    #[test]
    fn every_other_key_passes_through_untouched() {
        // ★ THE HALF THAT MATTERS MORE. A remap that also perturbed an
        // unrelated key would be discovered by typing, one wrong letter at a
        // time, and blamed on the layout.
        for evdev in 0u32..=255 {
            if DEFAULT_REMAPS.iter().any(|(f, _)| *f == evdev) {
                continue;
            }
            let k = Keycode::new(evdev + XKB_OFFSET);
            assert_eq!(
                apply(k).raw(),
                k.raw(),
                "evdev {evdev} was rewritten and should not have been"
            );
        }
    }

    #[test]
    fn the_remap_is_idempotent() {
        // Escape must not map onward to anything. A table with a chain in it
        // (a -> b, b -> c) would make the result depend on iteration order,
        // which is exactly the kind of thing that works until someone adds a
        // second entry.
        let esc = apply(Keycode::new(EVDEV_CAPSLOCK + XKB_OFFSET));
        assert_eq!(apply(esc).raw(), esc.raw(), "applying twice must be stable");
    }

    #[test]
    fn no_two_remaps_share_a_source() {
        // Two entries with the same `from` means the first wins silently.
        let mut froms: Vec<u32> = DEFAULT_REMAPS.iter().map(|(f, _)| *f).collect();
        let n = froms.len();
        froms.sort_unstable();
        froms.dedup();
        assert_eq!(froms.len(), n, "a source keycode is remapped twice");
    }

    #[test]
    fn no_remap_forms_a_chain() {
        // Guards the idempotence above for any FUTURE entry, not just today's.
        for (_, to) in DEFAULT_REMAPS {
            assert!(
                !DEFAULT_REMAPS.iter().any(|(f, _)| f == to),
                "evdev {to} is both a target and a source — the table chains"
            );
        }
    }

    #[test]
    fn the_remap_is_still_needed() {
        // ★ THE DELETION TRIPWIRE. This module exists ONLY because
        // `xkbcommon-hairetsu`'s `new_from_names` ignores its `options`
        // parameter. If that ever changes, `caps:escape` becomes the right
        // implementation and this file should go — but nothing would prompt
        // anyone to check.
        //
        // Asserting the source text is deliberate and is the honest tier: this
        // is a REMINDER, not a proof of behaviour. It fails when the signature
        // changes, which is the moment to reconsider.
        let src = include_str!("../../xkbcommon-hairetsu/src/xkb/mod.rs");
        assert!(
            src.contains("_options: Option<String>"),
            "hairetsu's new_from_names no longer ignores `options` — the xkb \
             `caps:escape` option may now work, and this module should be \
             deleted in favour of it"
        );
    }
}
