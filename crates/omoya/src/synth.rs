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
//! ★ **The characters come from the LIVE LAYOUT, not a table.** `expand` takes
//! a `hairetsu::Keymap` and asks it which key produces each character. It used
//! to carry a hardcoded US map whose own doc justified itself — "omoya's xkb
//! replacement serves one layout, us ... this table agrees with that decision"
//! — and that justification died the moment hairetsu shipped a layout
//! registry. On `br`, keycode 47 is `ç`: the table would have synthesised `;`
//! for it, and made `ç` unreachable. Same call, same success, wrong character.
//!
//! The layout used is the one the seat is ACTUALLY serving
//! (`ukeire_keymap_layout`, published only on a successful apply), not the one
//! the config declared — a node whose layout failed to compile is running the
//! bare US keymap, and typing against the layout it asked for would be wrong
//! in a second way.
//!
//! Two consequences worth knowing before calling it:
//!
//! * **AltGr is bracketed like Shift.** On any layout with a level-3 column
//!   (`br`'s `/`, `¬`, `ª`) the character is unreachable without it, and a
//!   stuck AltGr puts every subsequent REAL keystroke on level 3.
//! * **A dead-key character is REFUSED.** `é` on `br` is `dead_acute` then
//!   `e` — two keystrokes, no single keycode. It returns an error naming the
//!   character rather than typing a bare `e`, because "héllo" arriving as
//!   "hllo" is worse than being told.
//!
//! //! ★ **It is queued, not applied.** `Introspect::query` runs on the kanshou
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

/// `KEY_RIGHTALT` — `ISO_Level3_Shift` on any layout that binds it.
///
/// ★ RIGHT alt specifically. On `br` the level-3 column is reached by AltGr,
/// which `level3(ralt_switch)` binds to the RIGHT alt key; the left one stays
/// a plain `Alt_L` and would produce a chord instead of a character.
pub const KEY_RIGHTALT: u32 = 100;

/// The layout synthetic input must type against, from what the seat PUBLISHED.
///
/// ── ★ THE EFFECTIVE LAYOUT, NOT THE DECLARED ONE ────────────────────────
/// `ukeire_keymap_layout` is written only on a SUCCESSFUL `set_xkb_config`
/// and reads `<bare>` when a declared layout failed to compile. That
/// distinction is the whole reason to read it rather than the config: a node
/// that declares an unregistered layout is running the bare US keymap, and
/// synthesising against the layout it *asked* for would type the wrong
/// characters on a seat that is demonstrably not using it.
///
/// So the rule is: type against what the seat is actually serving.
#[must_use]
pub fn keymap_for(published: &str) -> hairetsu::Keymap {
    // `<bare>` and `<xkb default>` are both the built-in US keymap — the same
    // one `Keymap::us()` builds — so they resolve there rather than failing.
    hairetsu::Keymap::for_layout(published).unwrap_or_else(hairetsu::Keymap::us)
}

/// Which evdev key, with which modifiers, produces `c` on THIS layout.
///
/// ── ★ ASKS THE LAYOUT. THE TABLE THAT USED TO LIVE HERE WAS A LIE. ───────
/// `evdev_for` was a hardcoded US map from `char` to `(code, shift)`, and its
/// own doc justified itself: "omoya's xkb replacement serves one layout — us —
/// and refuses anything else. This table agrees with that decision."
///
/// That justification died the moment hairetsu shipped a layout REGISTRY. On
/// `br`, keycode 47 is `ç`; the table would have synthesised `;` for it and
/// made `ç` unreachable — typing the wrong character, silently, on the one
/// node that actually declares `br`.
///
/// Now it is a projection of the same table resolution reads, so synthetic
/// input and real input cannot disagree about a layout.
fn key_for_char(keymap: &hairetsu::Keymap, c: char) -> Result<(u32, hairetsu::ModMask), String> {
    let (xkb, mods) = keymap.key_for_char(c).ok_or_else(|| {
        format!(
            "{c:?} is not reachable by one keystroke on layout {:?} — it may need \
             a dead-key sequence, which synthetic input cannot express",
            keymap.layout_name(0)
        )
    })?;
    // Anything beyond Shift and AltGr would need a LOCK toggled (NumLock) or a
    // chord modifier (Ctrl/Alt), neither of which can be bracketed around a
    // character without changing seat state the caller did not ask for.
    let known = hairetsu::modifier::SHIFT | hairetsu::modifier::MOD5;
    if mods & !known != 0 {
        return Err(format!(
            "{c:?} needs modifiers {mods:#06b} that synthetic input will not \
             hold — refusing rather than typing something else"
        ));
    }
    // XKB keycode -> evdev code. The +8 is XKB's, and subtracting it here is
    // the ONE place the two numbering schemes meet.
    Ok((xkb - 8, mods))
}

/// ASCII → (evdev code, needs shift). **RETIRED FROM PRODUCTION, KEPT AS AN
/// ORACLE.**
///
/// ★ This is the hardcoded US table `expand` used to call. It is `#[cfg(test)]`
/// now rather than deleted, because it is the best available check on its own
/// replacement: `the_layout_lookup_agrees_with_the_retired_us_table` asserts
/// that the layout-derived lookup returns exactly what this returned, for every
/// printable ASCII character, on `us`.
///
/// That is the risk the rewrite actually carries — not "does br work" (a new
/// capability, separately tested) but "did us change". An independently-written
/// table that predates the change is a stronger answer to that than any
/// assertion written alongside the new code.
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
#[cfg(test)]
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

    let find = |s: &str, base: u32| {
        s.chars()
            .position(|x| x == c)
            .map(|i| base + u32::try_from(i).unwrap_or(0))
    };

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
pub fn expand(s: &Synth, keymap: &hairetsu::Keymap) -> Result<Vec<Step>, String> {
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
            let mut altgr_held = false;
            for c in t.chars() {
                let (code, mods) = key_for_char(keymap, c)?;
                let shift = mods & hairetsu::modifier::SHIFT != 0;
                let altgr = mods & hairetsu::modifier::MOD5 != 0;
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
                // ★ AltGr gets the same bracketing, and it is not optional on
                // a non-US layout: on `br` the whole level-3 column (`/` on q,
                // `¬`, `ª`, `º`) is unreachable without it.
                if altgr != altgr_held {
                    out.push(Step::Key {
                        code: KEY_RIGHTALT,
                        state: if altgr {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                    });
                    altgr_held = altgr;
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
            if altgr_held {
                // Same rule, and worse if broken: a stuck AltGr puts every
                // real keystroke on the level-3 column, so the keyboard types
                // symbols the operator cannot explain.
                out.push(Step::Key {
                    code: KEY_RIGHTALT,
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

    /// The layout the ASCII cases are written against, named rather than
    /// implied — these assertions are about `us` specifically, and `br` moves
    /// several of the keycodes they pin.
    fn us() -> hairetsu::Keymap {
        hairetsu::Keymap::for_layout("us").expect("us is registered")
    }

    #[test]
    fn a_lowercase_run_needs_no_shift() {
        let steps = expand(&Synth::Text("abc".into()), &us()).unwrap();
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
        let steps = expand(&Synth::Text("ABC".into()), &us()).unwrap();
        let shifts = steps
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    Step::Key {
                        code: KEY_LEFTSHIFT,
                        ..
                    }
                )
            })
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
            let steps = expand(&Synth::Text(text.into()), &us()).unwrap();
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
        let steps = expand(&Synth::Text("Hello, World!".into()), &us()).unwrap();
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
    fn the_same_character_synthesises_differently_per_layout() {
        // ★ THE WHOLE POINT, and the regression this replaced. A hardcoded US
        // table types the same keycodes whatever the seat is running: on `br`
        // it would have sent keycode 39 for `;` — which on that layout is `ç` —
        // and `ç` itself would have been unreachable.
        let br = hairetsu::Keymap::for_layout("br").expect("br is registered");

        let semi_us = expand(&Synth::Text(";".into()), &us()).unwrap();
        let semi_br = expand(&Synth::Text(";".into()), &br).unwrap();
        assert_ne!(
            semi_us, semi_br,
            "`;` must not synthesise the same keycode on us and br"
        );

        // `ç` is typeable on br and refused on us — a refusal, never the
        // nearest key.
        assert!(expand(&Synth::Text("ç".into()), &br).is_ok());
        assert!(expand(&Synth::Text("ç".into()), &us()).is_err());
    }

    #[test]
    fn an_altgr_character_brackets_right_alt_and_releases_it() {
        // The level-3 column had no consumer before `br`. If AltGr is not
        // held, the character is simply wrong; if it is not RELEASED, every
        // subsequent real keystroke lands on level 3.
        let br = hairetsu::Keymap::for_layout("br").expect("br");
        let steps = expand(&Synth::Text("¬".into()), &br).unwrap();
        let downs = steps
            .iter()
            .filter(|s| matches!(s, Step::Key { code, state } if *code == KEY_RIGHTALT && *state == KeyState::Pressed))
            .count();
        let ups = steps
            .iter()
            .filter(|s| matches!(s, Step::Key { code, state } if *code == KEY_RIGHTALT && *state == KeyState::Released))
            .count();
        assert_eq!(downs, 1, "AltGr must be pressed once: {steps:?}");
        assert_eq!(
            ups, 1,
            "AltGr must be released, or the seat is stuck on level 3"
        );
    }

    #[test]
    fn an_unmappable_character_is_refused_not_skipped() {
        // ★ Typing "héllo" and getting "hllo" is worse than an error: the
        // caller believes it sent what it asked for.
        let e = expand(&Synth::Text("héllo".into()), &us()).unwrap_err();
        assert!(e.contains('é'), "the refusal must name the character: {e}");
        assert!(expand(&Synth::Text("hello".into()), &us()).is_ok());
    }

    #[test]
    fn the_layout_table_round_trips_the_printable_ascii_it_claims() {
        // Not "every char maps" — a claim about the ones we DO map, so a typo
        // in a row string (two chars sharing a code, or an off-by-one base) is
        // caught rather than silently typing the wrong letter.
        let mut seen: std::collections::HashMap<(u32, bool), char> =
            std::collections::HashMap::new();
        for c in (0x20u8..0x7f).map(char::from) {
            if let Some(k) = evdev_for(c) {
                if let Some(prev) = seen.insert(k, c) {
                    panic!("{c:?} and {prev:?} both map to {k:?}");
                }
            }
        }
        // ── ★ THE DIFFERENTIAL ────────────────────────────────────────
        // The retired table vs the layout-derived lookup, on `us`, for every
        // printable ASCII character. This is what proves the rewrite did not
        // change the layout everyone is actually running.
        let us = hairetsu::Keymap::for_layout("us").expect("us is registered");
        for c in (0x20u8..0x7f).map(char::from) {
            let old = evdev_for(c);
            let new = key_for_char(&us, c)
                .ok()
                .map(|(code, mods)| (code, mods & hairetsu::modifier::SHIFT != 0));
            assert_eq!(
                old, new,
                "{c:?}: retired table says {old:?}, layout lookup says {new:?}"
            );
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
