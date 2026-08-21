//! Synthetic input: drive the seat from `kanshou`, over the same path a real
//! device takes.
//!
//! ★ **WHY THIS EXISTS, AND WHY IT MUST NOT SHORTCUT.** Two reasons, and the
//! second is the one that decides the design.
//!
//! The operator wants an MCP surface that can type, move the pointer and
//! measure what came back — a desktop you can drive and time from outside.
//!
//! And there is an open bug: keystrokes written into `/dev/input/event1`
//! produce nothing on plo — no frames, no pixels — while injected pointer
//! motion on `event4` works. Everything checkable came back clean (both
//! keyboards open, no `EVIOCGRAB`, `+8` applied, seat has a keyboard, focus
//! set, filter returns `Forward`, all devices `armed=true`), and a separate
//! reader on the same device *does* see the injected events.
//!
//! That bug is why [`Synth::Key`] calls [`crate::state::Omoya::key`] — the
//! exact method the evdev backend calls — rather than reaching for
//! `keyboard.input` itself. It splits the path in half at a known point:
//!
//! | synthetic key works | synthetic key fails |
//! |---|---|
//! | the loss is in the evdev READ path, upstream of `Omoya::key` | the loss is downstream, in the filter or the client |
//!
//! A surface that took its own route to the client would answer neither
//! question while looking like it had.
//!
//! ★ **It is queued, not applied.** `Introspect::query` runs on the kanshou
//! sidecar thread and may not touch `Omoya`. So this lands on
//! `pending_input` and the ping source drains it where `&mut Omoya` is legal —
//! the same shape `Deed` already uses, for the same reason.

use smithay::backend::input::KeyState;

/// One synthetic input action.
#[derive(Debug, Clone, PartialEq)]
pub enum Synth {
    /// A single key, by **evdev** code. The `+8` to XKB is applied on the way
    /// through, exactly as the backend does it, so a caller names the code
    /// they would find in `/usr/include/linux/input-event-codes.h`.
    Key { code: u32, pressed: bool },
    /// Type a string: press and release each character in turn, with shift
    /// held where the character needs it.
    Text(String),
    /// Relative pointer motion, in logical pixels.
    Pointer { dx: f64, dy: f64 },
    /// A pointer button by evdev code (`BTN_LEFT` = 272).
    Button { code: u32, pressed: bool },
}

/// `KEY_LEFTSHIFT`. Held around characters whose keycap they share.
pub const KEY_LEFTSHIFT: u32 = 42;

/// ASCII → (evdev code, needs shift).
///
/// ★ A US layout, and that is a STATED limitation rather than a hidden one.
/// omoya's xkb replacement (`xkbcommon-hairetsu`) serves one layout — `us` —
/// and refuses anything else rather than silently substituting. This table
/// agrees with that decision instead of pretending to more.
///
/// Returns `None` for anything unmapped, and the caller must treat that as a
/// refusal rather than skipping the character — typing `"héllo"` and getting
/// `"hllo"` is worse than being told the `é` is not representable.
#[must_use]
pub fn evdev_for(c: char) -> Option<(u32, bool)> {
    // Rows in evdev order, which is keycap order — not alphabetical.
    const ROW1: &str = "1234567890-=";
    const ROW1S: &str = "!@#$%^&*()_+";
    const ROW2: &str = "qwertyuiop[]";
    const ROW2S: &str = "QWERTYUIOP{}";
    const ROW3: &str = "asdfghjkl;'";
    const ROW3S: &str = "ASDFGHJKL:\"";
    const ROW4: &str = "zxcvbnm,./";
    const ROW4S: &str = "ZXCVBNM<>?";

    let find = |s: &str, base: u32| s.chars().position(|x| x == c).map(|i| base + u32::try_from(i).unwrap_or(0));

    if let Some(k) = find(ROW1, 2) {
        return Some((k, false));
    }
    if let Some(k) = find(ROW1S, 2) {
        return Some((k, true));
    }
    if let Some(k) = find(ROW2, 16) {
        return Some((k, false));
    }
    if let Some(k) = find(ROW2S, 16) {
        return Some((k, true));
    }
    if let Some(k) = find(ROW3, 30) {
        return Some((k, false));
    }
    if let Some(k) = find(ROW3S, 30) {
        return Some((k, true));
    }
    if let Some(k) = find(ROW4, 44) {
        return Some((k, false));
    }
    if let Some(k) = find(ROW4S, 44) {
        return Some((k, true));
    }
    match c {
        '\n' | '\r' => Some((28, false)), // KEY_ENTER
        '\t' => Some((15, false)),        // KEY_TAB
        ' ' => Some((57, false)),         // KEY_SPACE
        '\\' => Some((43, false)),
        '|' => Some((43, true)),
        '`' => Some((41, false)),
        '~' => Some((41, true)),
        _ => None,
    }
}

/// Expand one action into the flat key/button/motion steps to perform.
///
/// Separated from application so it is testable without a compositor — the
/// shift bracketing and the press/release pairing are where the bugs live,
/// and neither needs a seat to check.
#[must_use]
pub fn expand(s: &Synth) -> Result<Vec<Step>, String> {
    Ok(match s {
        Synth::Key { code, pressed } => vec![Step::Key {
            code: *code,
            state: if *pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        }],
        Synth::Text(t) => {
            let mut out = Vec::with_capacity(t.len() * 2);
            let mut shift_held = false;
            for c in t.chars() {
                let (code, shift) = evdev_for(c).ok_or_else(|| {
                    format!("{c:?} is not on the us layout this seat serves")
                })?;
                // ★ Bracket the shift, and only change it when it CHANGES.
                // Pressing and releasing shift around every character works
                // but generates 4x the events, and a run of capitals then
                // reads as N separate shift-chords to anything watching for
                // one.
                if shift != shift_held {
                    out.push(Step::Key {
                        code: KEY_LEFTSHIFT,
                        state: if shift {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                    });
                    shift_held = shift;
                }
                out.push(Step::Key {
                    code,
                    state: KeyState::Pressed,
                });
                out.push(Step::Key {
                    code,
                    state: KeyState::Released,
                });
            }
            if shift_held {
                // ★ NEVER LEAVE A MODIFIER DOWN. A synthetic shift left
                // pressed makes every subsequent REAL keystroke uppercase,
                // and the operator has no way to release a key nobody is
                // holding.
                out.push(Step::Key {
                    code: KEY_LEFTSHIFT,
                    state: KeyState::Released,
                });
            }
            out
        }
        Synth::Pointer { dx, dy } => vec![Step::Motion { dx: *dx, dy: *dy }],
        Synth::Button { code, pressed } => vec![Step::Button {
            code: *code,
            pressed: *pressed,
        }],
    })
}

/// One elementary action, after expansion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Key { code: u32, state: KeyState },
    Motion { dx: f64, dy: f64 },
    Button { code: u32, pressed: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowercase_run_needs_no_shift() {
        let steps = expand(&Synth::Text("abc".into())).unwrap();
        assert!(
            !steps.iter().any(|s| matches!(
                s,
                Step::Key {
                    code: KEY_LEFTSHIFT,
                    ..
                }
            )),
            "no shift for lowercase"
        );
        assert_eq!(steps.len(), 6, "three characters, press and release each");
    }

    #[test]
    fn shift_is_held_across_a_run_not_tapped_per_character() {
        let steps = expand(&Synth::Text("ABC".into())).unwrap();
        let shifts = steps
            .iter()
            .filter(|s| matches!(s, Step::Key { code: KEY_LEFTSHIFT, .. }))
            .count();
        assert_eq!(
            shifts, 2,
            "one press before the run and one release after, not two per char"
        );
    }

    #[test]
    fn the_modifier_is_never_left_down() {
        // ★ THE ONE THAT WOULD RUIN THE OPERATOR'S DAY. A synthetic shift left
        // pressed makes every subsequent REAL keystroke uppercase, and there
        // is no key to lift.
        for text in ["A", "aA", "Aa", "!", "hello World!"] {
            let steps = expand(&Synth::Text(text.into())).unwrap();
            let mut held = false;
            for s in &steps {
                if let Step::Key {
                    code: KEY_LEFTSHIFT,
                    state,
                } = s
                {
                    held = *state == KeyState::Pressed;
                }
            }
            assert!(!held, "{text:?} left shift down");
        }
    }

    #[test]
    fn every_press_has_its_release() {
        let steps = expand(&Synth::Text("Hello, World!".into())).unwrap();
        let mut down = std::collections::HashSet::new();
        for s in &steps {
            if let Step::Key { code, state } = s {
                match state {
                    KeyState::Pressed => {
                        assert!(down.insert(*code), "{code} pressed twice without release");
                    }
                    KeyState::Released => {
                        assert!(down.remove(code), "{code} released without press");
                    }
                }
            }
        }
        assert!(down.is_empty(), "keys left down: {down:?}");
    }

    #[test]
    fn an_unmappable_character_is_refused_not_skipped() {
        // ★ Typing "héllo" and getting "hllo" is worse than an error: the
        // caller believes it sent what it asked for.
        let e = expand(&Synth::Text("héllo".into())).unwrap_err();
        assert!(e.contains('é'), "the refusal must name the character: {e}");
        assert!(expand(&Synth::Text("hello".into())).is_ok());
    }

    #[test]
    fn the_layout_table_round_trips_the_printable_ascii_it_claims() {
        // Not "every char maps" — a claim about the ones we DO map, so a typo
        // in a row string (two chars sharing a code, or an off-by-one base) is
        // caught rather than silently typing the wrong letter.
        let mut seen: std::collections::HashMap<(u32, bool), char> = std::collections::HashMap::new();
        for c in (0x20u8..0x7f).map(char::from) {
            if let Some(k) = evdev_for(c) {
                if let Some(prev) = seen.insert(k, c) {
                    panic!("{c:?} and {prev:?} both map to {k:?}");
                }
            }
        }
        // Spot-check the anchors of each row against the kernel's own codes.
        assert_eq!(evdev_for('a'), Some((30, false)), "KEY_A");
        assert_eq!(evdev_for('z'), Some((44, false)), "KEY_Z");
        assert_eq!(evdev_for('1'), Some((2, false)), "KEY_1");
        assert_eq!(evdev_for('q'), Some((16, false)), "KEY_Q");
        assert_eq!(evdev_for(' '), Some((57, false)), "KEY_SPACE");
        assert_eq!(evdev_for('\n'), Some((28, false)), "KEY_ENTER");
        assert_eq!(evdev_for('A'), Some((30, true)), "shifted KEY_A");
    }
}
