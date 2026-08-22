//! The keysym → `awase::Hotkey` adapter — omoya's half of the reserved-chord
//! contract.
//!
//! `awase::Reserved::fleet_linux()` says WHICH chords a compositor must not
//! swallow. It says nothing about xkb, because it cannot: awase is a portable
//! keymap library and keysyms are a Wayland/X detail. This module is the join,
//! and `theory/OMOYA.md` §5 calls it out as "what omoya genuinely needs from
//! awase".
//!
//! ── WHY THE COVERAGE IS TEST-ENFORCED, NOT EYEBALLED ──────────────────────
//! Only keys that appear in the catalog can ever match a claim, so in principle
//! this module could map exactly those and nothing else. The trap is that it
//! would then be silently coupled to the catalog's CURRENT contents: the day
//! awase claims a chord on a key this file does not translate, `claim_on`
//! stops matching, omoya forwards the chord, and nothing anywhere says so. The
//! failure is invisible precisely because a reserved chord's whole job is to be
//! rare.
//!
//! So `tests::every_claimed_chord_is_reachable` walks `fleet_linux()` and
//! asserts each claim's key round-trips through this adapter. Adding a claim on
//! an unmapped key is then a RED TEST rather than a silent hole — the
//! invariant-by-consistency shape, where the checker re-derives the coverage
//! from the live catalog instead of trusting a hand-list.

use awase::{Hotkey, Key, Modifiers};
use smithay::input::keyboard::{Keysym, ModifiersState, keysyms};

/// Translate xkb modifier state into awase's.
///
/// `caps_lock` is deliberately NOT carried. It is a LATCH, not a held
/// modifier: with caps on, every chord would arrive carrying `CAPS_LOCK` and
/// match nothing, so a machine with caps lock engaged would silently lose its
/// VT-switch escape hatch. `num_lock`, `iso_level3_shift` and level5 are
/// dropped for the same reason — awase has no arm for them and inventing one
/// here would put the fleet's chord vocabulary in two places.
#[must_use]
pub fn modifiers_from(state: &ModifiersState) -> Modifiers {
    let mut m = Modifiers::NONE;
    if state.ctrl {
        m = m | Modifiers::CTRL;
    }
    if state.alt {
        m = m | Modifiers::ALT;
    }
    if state.shift {
        m = m | Modifiers::SHIFT;
    }
    if state.logo {
        m = m | Modifiers::CMD;
    }
    m
}

/// Translate an xkb keysym into an `awase::Key`.
///
/// Returns `None` for anything outside the mapped set. That is a correct
/// answer, not a gap: `None` means "this keystroke is the client's", which is
/// the safe default for everything that is not a seat chord.
///
/// ★ **THIS TABLE WAS THE BUG, AND THE SHAPE OF IT IS WORTH KEEPING.**
/// (Measured on plo, 2026-08-21.) It used to map F1–F12, Delete and Backspace
/// and nothing else — correct for the day it was written, when the only
/// chords were the VT-switch escape hatch. `deed.rs` then grew a full keymap:
/// Logo+Return for the terminal, Logo+hjkl and Logo+arrows for focus,
/// Logo+Shift+those for resize, Logo+Q to close. **Not one of those keys was
/// in this table**, so every one of them translated to `None` and never
/// reached `BindingMap` at all.
///
/// ★ CORRECTED — the evidence, stated honestly. This first read "the seat had
/// been up for days with `deeds_performed == 0`". That was TRUE and it was not
/// evidence: `deeds_performed` is incremented only where KANSHOU-requested
/// deeds are drained, so it would have read zero whether the keymap worked or
/// not. The real evidence was behavioural — Logo+Return produced no window on
/// a live seat while `do/spawn-terminal` over kanshou opened one in a second,
/// which splits the fault cleanly on the near side of `BindingMap`. The
/// keyboard path having no counter AT ALL is the second defect here, and
/// `chord_deeds` now closes it. Both halves were
/// individually correct and individually tested — `default_bindings()` builds
/// a collision-free map with passing tests, and this function returns exactly
/// what it promises — because **`deed.rs`'s tests construct `Hotkey::new(LOGO,
/// Key::H)` BY HAND and never go through this translator.** Two correct halves,
/// no test spanning the seam, one dead keymap.
///
/// `every_bound_key_is_reachable` below is that missing test, and it is the
/// only thing keeping this table honest: it walks the real binding map and
/// fails if any bound key has no keysym that reaches it. Add a binding whose
/// key is not here and the build goes red instead of the chord going quiet.
#[must_use]
pub fn key_from(sym: Keysym) -> Option<Key> {
    let k = match sym.raw() {
        keysyms::KEY_F1 => Key::F1,
        keysyms::KEY_F2 => Key::F2,
        keysyms::KEY_F3 => Key::F3,
        keysyms::KEY_F4 => Key::F4,
        keysyms::KEY_F5 => Key::F5,
        keysyms::KEY_F6 => Key::F6,
        keysyms::KEY_F7 => Key::F7,
        keysyms::KEY_F8 => Key::F8,
        keysyms::KEY_F9 => Key::F9,
        keysyms::KEY_F10 => Key::F10,
        keysyms::KEY_F11 => Key::F11,
        keysyms::KEY_F12 => Key::F12,
        // Both spellings reach the same key. A bare Delete is KEY_Delete;
        // the numpad's, with num-lock off, is KEY_KP_Delete — and
        // Ctrl+Alt+numpad-Del is the same reboot request to a user.
        keysyms::KEY_Delete | keysyms::KEY_KP_Delete => Key::Delete,
        keysyms::KEY_BackSpace => Key::Backspace,

        // ── The seat's own chords ──────────────────────────────────────
        // ★ BOTH CASES OF EVERY LETTER. xkb applies Shift before we see the
        // keysym, so `Logo+Shift+H` (resize left) arrives as `KEY_H`, not
        // `KEY_h`. Mapping only lowercase would leave focus working and
        // resize silently dead — the half-broken variant of the bug this
        // whole comment is about.
        keysyms::KEY_h | keysyms::KEY_H => Key::H,
        keysyms::KEY_j | keysyms::KEY_J => Key::J,
        keysyms::KEY_k | keysyms::KEY_K => Key::K,
        keysyms::KEY_l | keysyms::KEY_L => Key::L,
        keysyms::KEY_q | keysyms::KEY_Q => Key::Q,

        keysyms::KEY_Left => Key::Left,
        keysyms::KEY_Right => Key::Right,
        keysyms::KEY_Up => Key::Up,
        keysyms::KEY_Down => Key::Down,

        // `Return` only. awase reserves `NumpadEnter` for `KEY_KP_Enter`, and
        // conflating them would bind a key the operator did not ask for.
        keysyms::KEY_Return => Key::Return,
        keysyms::KEY_space => Key::Space,

        _ => return None,
    };
    Some(k)
}

/// The full translation: modifier state + keysym → a hotkey awase can adjudicate.
#[must_use]
pub fn hotkey_from(state: &ModifiersState, sym: Keysym) -> Option<Hotkey> {
    key_from(sym).map(|key| Hotkey::new(modifiers_from(state), key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use awase::Reserved;

    fn mods(ctrl: bool, alt: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            ..Default::default()
        }
    }

    /// Every keysym this adapter knows how to translate. Used only to answer
    /// "could the adapter ever PRODUCE this key?" without a second hand-list of
    /// the mapping itself.
    const MAPPED_SYMS: &[u32] = &[
        keysyms::KEY_F1,
        keysyms::KEY_F2,
        keysyms::KEY_F3,
        keysyms::KEY_F4,
        keysyms::KEY_F5,
        keysyms::KEY_F6,
        keysyms::KEY_F7,
        keysyms::KEY_F8,
        keysyms::KEY_F9,
        keysyms::KEY_F10,
        keysyms::KEY_F11,
        keysyms::KEY_F12,
        keysyms::KEY_Delete,
        keysyms::KEY_KP_Delete,
        keysyms::KEY_BackSpace,
        // The seat's own chords. Both cases per letter, because xkb applies
        // Shift before we see the keysym.
        keysyms::KEY_h,
        keysyms::KEY_H,
        keysyms::KEY_j,
        keysyms::KEY_J,
        keysyms::KEY_k,
        keysyms::KEY_K,
        keysyms::KEY_l,
        keysyms::KEY_L,
        keysyms::KEY_q,
        keysyms::KEY_Q,
        keysyms::KEY_Left,
        keysyms::KEY_Right,
        keysyms::KEY_Up,
        keysyms::KEY_Down,
        keysyms::KEY_Return,
        keysyms::KEY_space,
    ];

    #[test]
    fn every_claimed_chord_is_reachable() {
        // ★ THE GATE. Walks the LIVE catalog rather than a hand-list, so a new
        // claim on a key this adapter cannot translate fails HERE instead of
        // silently never matching at runtime — where the symptom would be a
        // reserved chord quietly reaching the client, which is exactly the
        // thing nobody notices until they need the escape hatch.
        let reserved = Reserved::fleet_linux();
        assert!(reserved.len() >= 13, "catalog shrank unexpectedly: {}", reserved.len());

        let producible: Vec<Key> = MAPPED_SYMS
            .iter()
            .filter_map(|raw| key_from(Keysym::from(*raw)))
            .collect();

        let mut unreachable = Vec::new();
        for (name, _claim) in reserved.iter() {
            // The catalog is keyed by the chord's canonical spelling; parse it
            // back rather than restating the 13 chords here.
            let hk = Hotkey::parse(name).unwrap_or_else(|e| panic!("catalog key {name:?} does not parse: {e:?}"));
            if !producible.contains(&hk.key) {
                unreachable.push(name.to_string());
            }
        }
        assert!(
            unreachable.is_empty(),
            "awase claims chords omoya cannot translate, so it would FORWARD them: {unreachable:?}"
        );
    }

    #[test]
    fn every_bound_key_is_reachable() {
        // ★ THE TEST THAT WAS MISSING, AND THE REASON THE SEAT HAD NO CHORDS.
        //
        // `every_claimed_chord_is_reachable` above walks `Reserved::fleet_linux()`
        // — the VT-switch escape hatches — and reads as though it covers "the
        // chords". It does not cover omoya's OWN keymap, and when `deed.rs`
        // landed Logo+Return / Logo+hjkl / Logo+arrows / Logo+Q, none of those
        // keys was in `key_from`. Every one translated to `None`, so
        // `BindingMap` was never consulted at all.
        //
        // Measured on plo 2026-08-21: Logo+Return produced no window on a
        // live seat, while `do/spawn-terminal` over kanshou opened one in a
        // second. The deeds worked; nothing could reach them. (The keyboard
        // path had no counter of its own to notice with — see `chord_deeds`.)
        //
        // This walks the LIVE binding map, so a future binding on an
        // untranslatable key fails here instead of going quiet on the seat.
        let (map, _clashes) = crate::deed::default_bindings();
        let mode = map.mode("default").expect("the default mode must exist");
        assert!(
            mode.len() >= 18,
            "the keymap shrank unexpectedly ({}) — this gate is only as good as \
             what it walks",
            mode.len()
        );

        let producible: Vec<Key> = MAPPED_SYMS
            .iter()
            .filter_map(|raw| key_from(Keysym::from(*raw)))
            .collect();

        let mut unreachable = Vec::new();
        for (hk, _binding) in mode.iter() {
            if !producible.contains(&hk.key) {
                unreachable.push(format!("{:?}", hk.key));
            }
        }
        unreachable.sort_unstable();
        unreachable.dedup();
        assert!(
            unreachable.is_empty(),
            "omoya BINDS keys that `key_from` cannot produce, so those chords can \
             never fire: {unreachable:?}"
        );
    }

    #[test]
    fn a_shifted_letter_reaches_the_same_key_as_its_lowercase() {
        // Resize is Logo+Shift+hjkl, and xkb hands us the UPPERCASE keysym for
        // it. Mapping only lowercase would leave focus working and resize
        // silently dead — a half-broken keymap is harder to diagnose than a
        // fully dead one, because the operator concludes the feature is
        // missing rather than that input is broken.
        for (lower, upper) in [
            (keysyms::KEY_h, keysyms::KEY_H),
            (keysyms::KEY_j, keysyms::KEY_J),
            (keysyms::KEY_k, keysyms::KEY_K),
            (keysyms::KEY_l, keysyms::KEY_L),
            (keysyms::KEY_q, keysyms::KEY_Q),
        ] {
            assert_eq!(
                key_from(Keysym::from(lower)),
                key_from(Keysym::from(upper)),
                "case must not change which key this is"
            );
            assert!(key_from(Keysym::from(lower)).is_some());
        }
    }

    #[test]
    fn an_unbound_letter_is_still_the_clients() {
        // The table must grow with the keymap and NOT beyond it. Every keysym
        // mapped here is one the seat can consume, so mapping letters nobody
        // binds would put a chord's worth of risk on the client's keys for no
        // benefit.
        for raw in [keysyms::KEY_a, keysyms::KEY_z, keysyms::KEY_Escape, keysyms::KEY_Tab] {
            assert_eq!(
                key_from(Keysym::from(raw)),
                None,
                "an unbound key must stay the client's"
            );
        }
    }

    #[test]
    fn ctrl_alt_f2_is_the_vt_switch_the_catalog_claims() {
        let hk = hotkey_from(&mods(true, true), Keysym::from(keysyms::KEY_F2))
            .expect("Ctrl+Alt+F2 must translate");
        let reserved = Reserved::fleet_linux();
        let claim = reserved
            .claim_on(&hk)
            .expect("Ctrl+Alt+F2 must be a claimed chord");
        assert!(
            claim.purpose.contains("virtual terminal"),
            "purpose was {:?}",
            claim.purpose
        );
    }

    #[test]
    fn an_ordinary_key_is_not_claimed() {
        // The common case, and the one that must stay cheap and quiet: a key
        // outside the mapped set produces no hotkey at all, so it never reaches
        // the catalog.
        assert!(key_from(Keysym::from(keysyms::KEY_a)).is_none());
        assert!(hotkey_from(&mods(true, true), Keysym::from(keysyms::KEY_a)).is_none());
    }

    #[test]
    fn caps_lock_does_not_leak_into_the_chord() {
        // With caps engaged, Ctrl+Alt+F2 must STILL be Ctrl+Alt+F2. If the
        // latch leaked in, the chord would match nothing and a machine with
        // caps on would quietly lose its escape hatch.
        let state = ModifiersState {
            ctrl: true,
            alt: true,
            caps_lock: true,
            num_lock: true,
            ..Default::default()
        };
        let hk = hotkey_from(&state, Keysym::from(keysyms::KEY_F2)).unwrap();
        assert_eq!(hk.modifiers, Modifiers::CTRL | Modifiers::ALT);
        assert!(Reserved::fleet_linux().claim_on(&hk).is_some());
    }
}

/// Which VT a reserved chord asks for, if it is a VT-switch chord.
///
/// ★ Ctrl+Alt+F1..F12 map to VT 1..12. An `Option`, not an assumption:
/// `Reserved::fleet_linux()` claims chords that are NOT VT switches, and acting
/// on one of those as though it were would throw the seat to another VT for a
/// keystroke that meant something else.
///
/// BOTH modifiers are required. A bare F2 in a terminal must never move the
/// seat, which is the failure this guard exists to prevent rather than a
/// stylistic check.
#[must_use]
pub fn vt_of(hk: &Hotkey) -> Option<i32> {
    use awase::Key;

    // `Modifiers` is a bitflag newtype, not a struct of bools — `contains`
    // is the test. Requiring BOTH means a bare F2 in a terminal can never
    // move the seat, which is the failure this guard exists for.
    if !hk.modifiers.contains(Modifiers::CTRL) || !hk.modifiers.contains(Modifiers::ALT) {
        return None;
    }
    Some(match hk.key {
        Key::F1 => 1,
        Key::F2 => 2,
        Key::F3 => 3,
        Key::F4 => 4,
        Key::F5 => 5,
        Key::F6 => 6,
        Key::F7 => 7,
        Key::F8 => 8,
        Key::F9 => 9,
        Key::F10 => 10,
        Key::F11 => 11,
        Key::F12 => 12,
        _ => return None,
    })
}

#[cfg(test)]
mod vt_tests {
    use super::*;
    use awase::{Key, Modifiers};

    #[test]
    fn ctrl_alt_f2_is_vt_2() {
        let hk = Hotkey::new(Modifiers::CTRL.with(Modifiers::ALT), Key::F2);
        assert_eq!(vt_of(&hk), Some(2));
    }

    #[test]
    fn a_bare_function_key_never_moves_the_seat() {
        // The guard that matters: F2 typed into a terminal must not switch VT.
        let hk = Hotkey::new(Modifiers::NONE, Key::F2);
        assert_eq!(vt_of(&hk), None);
    }

    #[test]
    fn ctrl_alt_on_a_non_function_key_is_not_a_vt_switch() {
        let hk = Hotkey::new(Modifiers::CTRL.with(Modifiers::ALT), Key::A);
        assert_eq!(vt_of(&hk), None);
    }
}
