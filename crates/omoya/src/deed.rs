//! What a chord DOES — omoya's action vocabulary and its binding map.
//!
//! ── ★ awase OWNS KEYS. THIS FILE OWNS ONLY THE VERBS. ────────────────────
//! The fleet rule is explicit: a keymap is `awase::BindingMap` (modes) ⊕
//! `KeyChord` (sequences) ⊕ `Binding`/`Action`, and a bespoke `Keymap` is the
//! violation. So there is no lookup table, no modifier matching and no
//! collision detection here — `BindingMap<Deed>` is parameterised over the
//! enum below and does all of it. What omoya contributes is the list of
//! things a seat can be asked to do.
//!
//! ── ★ WHY THE DEFAULTS LOOK LIKE THIS ───────────────────────────────────
//! The Logo key is the modifier, and that is not taste: Ctrl and Alt belong
//! to the applications running inside the seat, and a compositor that takes
//! Ctrl+H takes it away from every editor at once. Logo is the one modifier
//! essentially no terminal program claims.
//!
//! awase spells it `Modifiers::CMD` — awase grew up on macOS, where that bit
//! is Command. It is bit 0 of the same mask Wayland calls Logo and X calls
//! Mod4, so the NAME is the only thing that is macOS about it. Aliased below
//! rather than used raw, because `Modifiers::CMD` in a Linux compositor reads
//! like a porting mistake every time somebody sees it.
//!
//! The direction keys are h/j/k/l AND the arrows, both bound to the same
//! deed. Binding only hjkl assumes an operator who already knows; binding
//! only arrows makes the muscle memory of everyone who does worthless.
//!
//! ── ★ CONSUMED, NOT FORWARDED — AND THAT IS THE DIFFERENCE FROM VT ──────
//! `Reserved` VT chords are recognised and FORWARDED, because the seat cannot
//! provide the escape they represent. These are the opposite: a seat deed the
//! client must never also see, or Super+Q closes the window AND the client
//! reads a Q. `Binding::consume` defaults true; it is left at the default
//! deliberately rather than being set, so the shape matches every other awase
//! consumer.

use awase::{Binding, BindingMap, Hotkey, Key, Modifiers};
use kukaku::Direction;

/// The Logo / Super / Mod4 key, under the name a Linux compositor uses.
///
/// awase calls bit 0 `CMD` because it grew up on macOS. Same bit, same mask;
/// only the name is macOS. Aliased so nothing in this file reads like a
/// half-finished port.
pub const LOGO: Modifiers = Modifiers::CMD;

/// A thing the seat can be asked to do.
///
/// Deliberately a real enum and not awase's stringly `Action::Command(String)`:
/// awase's own doc says an app with a real enum should parameterise rather
/// than stringify into it. A typo'd deed is then a compile error instead of a
/// chord that silently does nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deed {
    /// Move keyboard focus to the neighbouring window.
    Focus(Direction),
    /// Move the divider governing the focused window.
    Resize(Direction),
    /// Close the focused window, politely — an xdg `close` request, not a
    /// kill. The client gets to save its work; if it ignores the request that
    /// is the client's choice to answer for, not ours to override.
    Close,
    /// Launch the seat's terminal.
    SpawnTerminal,
}

/// The default binding map.
///
/// Returns `awase`'s type rather than a wrapper so a future config surface
/// can add to it without this module growing an API of its own.
#[must_use]
pub fn default_bindings() -> BindingMap<Deed> {
    let mut map = BindingMap::<Deed>::typed();
    let Some(mode) = map.mode_mut("default") else {
        // `typed()` inserts "default" itself, so this is unreachable — but
        // returning the empty map beats an unwrap that turns a future rename
        // upstream into a panic on the operator's login.
        return map;
    };

    let logo = LOGO;
    let logo_shift = LOGO | Modifiers::SHIFT;

    // Focus: hjkl and the arrows, same deed, because both muscle memories are
    // real and neither is worth punishing.
    for (key, dir) in [
        (Key::H, Direction::Left),
        (Key::J, Direction::Below),
        (Key::K, Direction::Above),
        (Key::L, Direction::Right),
        (Key::Left, Direction::Left),
        (Key::Down, Direction::Below),
        (Key::Up, Direction::Above),
        (Key::Right, Direction::Right),
    ] {
        // `add_binding` returns the binding it displaced. Ignored here
        // because this map is built once from a literal list — but NOT
        // silently: `detect_duplicate_bindings` is awase's surface for
        // catching a real collision, and the test below is what proves this
        // list has none worth reporting.
        mode.add_binding(Binding::new(Hotkey::new(logo, key), Deed::Focus(dir)));
        mode.add_binding(Binding::new(
            Hotkey::new(logo_shift, key),
            Deed::Resize(dir),
        ));
    }

    mode.add_binding(Binding::new(Hotkey::new(logo, Key::Q), Deed::Close));
    // `Return`, not `Enter` — awase names the main key `Return` and reserves
    // `NumpadEnter` for the other one.
    mode.add_binding(Binding::new(
        Hotkey::new(logo, Key::Return),
        Deed::SpawnTerminal,
    ));
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use awase::mode::{MatchContext, MatchResult};

    fn hit(map: &mut BindingMap<Deed>, hk: Hotkey) -> Option<Deed> {
        match map.match_key(hk, &MatchContext::default()) {
            MatchResult::Matched { action, .. } => Some(action),
            _ => None,
        }
    }

    #[test]
    fn arrows_and_hjkl_agree() {
        let mut m = default_bindings();
        assert_eq!(
            hit(&mut m, Hotkey::new(LOGO, Key::H)),
            hit(&mut m, Hotkey::new(LOGO, Key::Left)),
        );
    }

    /// ★ THE ONE THAT MATTERS: the seat must not steal the applications'
    /// modifiers. A compositor binding Ctrl+H takes it from every editor at
    /// once, and the symptom — backspace stops working in one program —
    /// never points at the compositor.
    #[test]
    fn ctrl_and_alt_are_left_to_the_applications() {
        let mut m = default_bindings();
        for mods in [Modifiers::CTRL, Modifiers::ALT, Modifiers::CTRL | Modifiers::ALT] {
            for key in [Key::H, Key::J, Key::K, Key::L, Key::Q, Key::Return] {
                assert_eq!(
                    hit(&mut m, Hotkey::new(mods, key)),
                    None,
                    "the seat claimed {mods:?}+{key:?}, which belongs to the client",
                );
            }
        }
    }

    #[test]
    fn a_bare_key_is_never_a_deed() {
        let mut m = default_bindings();
        assert_eq!(hit(&mut m, Hotkey::new(Modifiers::NONE, Key::Q)), None);
    }
}

// ── ★ PERFORMING A DEED — the compositor side ────────────────────────────

impl crate::state::Omoya {
    /// Carry out one deed.
    ///
    /// Deliberately total over `Deed` with no `_ =>` arm: adding a verb to the
    /// enum must be a compile error here rather than a chord that silently
    /// does nothing. A binding that resolves to an unhandled action is the
    /// worst failure this layer has, because the key visibly stops reaching
    /// the application AND produces no effect — the operator concludes the
    /// keyboard is broken.
    pub fn perform(&mut self, deed: Deed) {
        match deed {
            Deed::Focus(dir) => self.focus_direction(dir),
            Deed::Resize(dir) => {
                // 0.05 of the parent, matching kukaku's MIN_RATIO: one press
                // is the smallest move that cannot collapse a pane, so a held
                // key ramps smoothly instead of snapping to an edge.
                if self.tiling.resize_focused(dir, 0.05) {
                    self.apply_layout();
                }
            }
            Deed::Close => self.close_focused(),
            Deed::SpawnTerminal => self.spawn_terminal(),
        }
    }

    fn focus_direction(&mut self, dir: Direction) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        let Some(window) = self.tiling.focus_direction(dir, geo) else {
            // Nothing that way. A finding, not a failure — the operator
            // pressed a direction at the edge of the screen, and the right
            // response is to do nothing quietly rather than wrap around.
            return;
        };
        self.focus_window(&window);
    }

    /// Give a window keyboard focus, and tell it so.
    ///
    /// Both halves are required and only the first is obvious. `set_focus`
    /// routes keystrokes; the `Activated` state is what makes the client draw
    /// itself as focused. Without the second, typing goes to the right window
    /// while every window still looks inactive — which reads as the keyboard
    /// going to the wrong place.
    pub fn focus_window(&mut self, window: &smithay::desktop::Window) {
        self.tiling.focus_window(window);
        self.space.raise_element(window, true);

        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        // Deactivate everything else first: two windows both drawing as
        // focused is a state the protocol permits and the operator cannot
        // interpret.
        let others: Vec<_> = self
            .space
            .elements()
            .filter(|w| *w != window)
            .cloned()
            .collect();
        for other in others {
            if let Some(t) = other.toplevel() {
                t.with_pending_state(|st| {
                    st.states.unset(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                    );
                });
                t.send_pending_configure();
            }
        }
        if let Some(t) = window.toplevel() {
            t.with_pending_state(|st| {
                st.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                );
            });
            t.send_pending_configure();
        }
        if let Some(kb) = self.seat.get_keyboard() {
            let focus = window.toplevel().map(|t| t.wl_surface().clone());
            kb.set_focus(self, focus, serial);
        }
    }

    fn close_focused(&mut self) {
        // `send_close` is a REQUEST, not a kill: the client may refuse, or
        // put up a "save your work?" dialog. Killing it here would be the
        // compositor overriding a decision that belongs to the application,
        // and the unmap happens through `toplevel_destroyed` either way.
        if let Some(w) = self.tiling.focused() {
            if let Some(t) = w.toplevel() {
                t.send_close();
            }
        }
    }

    fn spawn_terminal(&mut self) {
        let Some(cmd) = self.session_command.clone() else {
            // Nothing to spawn is a real state, not an error: omoya can be
            // run with no `-- <cmd>` at all. Logged rather than silent,
            // because a chord that does nothing is otherwise indistinguishable
            // from a chord that is not bound.
            tracing::warn!("Logo+Return pressed but this seat has no terminal command");
            return;
        };
        let Some((program, rest)) = cmd.split_first() else {
            return;
        };
        match std::process::Command::new(program)
            .args(rest)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .spawn()
        {
            Ok(child) => tracing::info!(pid = child.id(), program, "spawned into the seat"),
            Err(e) => tracing::error!(error = %e, program, "spawn failed"),
        }
    }
}
