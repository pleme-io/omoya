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
    /// Launch the seat's application launcher.
    ///
    /// A SECOND spawn verb rather than a parameterised one, and that is the
    /// whole reason `perform` can stay a remote-safe surface: neither arm
    /// carries a command, so there is no string for a caller to fill in. Both
    /// run a command this seat was configured with at startup. Collapsing them
    /// into `Spawn(String)` would turn the kanshou verb surface into a remote
    /// shell, which is exactly what `parse`'s doc refuses.
    SpawnLauncher,
}

impl Deed {
    /// Parse a remotely-requested verb.
    ///
    /// ★ THE LEGALITY GATE, AND IT IS A PARSE — NOT A CHECK AFTER THE FACT.
    /// A remote caller names a verb by string; anything this function does not
    /// recognise has no `Deed` and therefore no path to `perform`. The set of
    /// remotely-invocable actions is exactly the set of arms below, which is
    /// the whole point: there is no "and also run this command" case, because
    /// `SpawnTerminal` launches the seat's OWN configured command and takes no
    /// argument. A verb surface that accepted a command string would be a
    /// remote shell wearing a compositor's clothes.
    ///
    /// Returns `None` rather than a default, so an unknown verb is REFUSED and
    /// says so (kotae) instead of silently doing something adjacent.
    #[must_use]
    pub fn parse(verb: &str) -> Option<Self> {
        Some(match verb {
            "focus-left" => Self::Focus(Direction::Left),
            "focus-right" => Self::Focus(Direction::Right),
            "focus-up" => Self::Focus(Direction::Above),
            "focus-down" => Self::Focus(Direction::Below),
            "resize-left" => Self::Resize(Direction::Left),
            "resize-right" => Self::Resize(Direction::Right),
            "resize-up" => Self::Resize(Direction::Above),
            "resize-down" => Self::Resize(Direction::Below),
            "close" => Self::Close,
            "spawn-terminal" => Self::SpawnTerminal,
            "spawn-launcher" => Self::SpawnLauncher,
            _ => return None,
        })
    }

    /// Every verb `parse` accepts, for an agent that wants to enumerate rather
    /// than guess. Kept beside `parse` so the two cannot drift — a catalog
    /// listing a verb the parser refuses is worse than no catalog.
    pub const VERBS: &'static [&'static str] = &[
        "focus-left",
        "focus-right",
        "focus-up",
        "focus-down",
        "resize-left",
        "resize-right",
        "resize-up",
        "resize-down",
        "close",
        "spawn-terminal",
        "spawn-launcher",
    ];
}

/// The default binding map, and every chord that collided while building it.
///
/// ★ `try_bind`, NOT `add_binding`, BECAUSE awase ASKED. `add_binding` is
/// `#[must_use]` and its note says exactly why: *"the returned Some(prev) is a
/// DUPLICATE binding being silently discarded — handle it, or use `try_bind`
/// to make it an error."* Silencing that with `let _ =` would mean a chord
/// bound twice keeps whichever line came last, which is the worst kind of
/// keymap bug — the binding you read in the source is not the one that runs.
///
/// Collisions are RETURNED rather than panicked on. A panic here fires during
/// the operator's login and takes the whole seat down over a keymap typo; the
/// test below is what makes the list collision-free, and the caller logs
/// anything that somehow survives it.
///
/// Returns `awase`'s type rather than a wrapper so a future config surface
/// can add to it without this module growing an API of its own.
#[must_use]
pub fn default_bindings() -> (BindingMap<Deed>, Vec<Hotkey>) {
    let mut map = BindingMap::<Deed>::typed();
    let mut clashes = Vec::new();
    let Some(mode) = map.mode_mut("default") else {
        // `typed()` inserts "default" itself, so this is unreachable — but
        // returning the empty map beats an unwrap that turns a future rename
        // upstream into a panic on the operator's login.
        return (map, clashes);
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
        if let Err(prev) = mode.try_bind(Binding::new(Hotkey::new(logo, key), Deed::Focus(dir))) {
            clashes.push(prev.hotkey);
        }
        if let Err(prev) = mode.try_bind(Binding::new(
            Hotkey::new(logo_shift, key),
            Deed::Resize(dir),
        )) {
            clashes.push(prev.hotkey);
        }
    }

    if let Err(prev) = mode.try_bind(Binding::new(Hotkey::new(logo, Key::Q), Deed::Close)) {
        clashes.push(prev.hotkey);
    }
    // `Return`, not `Enter` — awase names the main key `Return` and reserves
    // `NumpadEnter` for the other one.
    if let Err(prev) = mode.try_bind(Binding::new(
        Hotkey::new(logo, Key::Return),
        Deed::SpawnTerminal,
    )) {
        clashes.push(prev.hotkey);
    }

    // ★ Ctrl+Space, and it is the ONE deliberate exception to this module's
    // "Ctrl belongs to the applications" rule. The header argues that a
    // compositor taking Ctrl+H takes it from every editor at once, and that
    // still holds — but Ctrl+Space is the launcher chord the operator already
    // has in muscle memory from macOS, and unlike Ctrl+H almost nothing inside
    // a terminal claims it. `Binding::consume` is left at its default (true),
    // so a client never also sees it: a launcher that opened AND typed a space
    // into whatever was focused would be worse than no launcher.
    //
    // The exception is worth naming rather than quietly making, because the
    // next person to reach for a Ctrl chord should have to argue against this
    // paragraph instead of citing this line as precedent.
    if let Err(prev) = mode.try_bind(Binding::new(
        Hotkey::new(Modifiers::CTRL, Key::Space),
        Deed::SpawnLauncher,
    )) {
        clashes.push(prev.hotkey);
    }
    (map, clashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    // `MatchResult` from `mode`, `MatchContext` from the crate root —
    // `mode` imports the latter PRIVATELY for its own use, so the obvious
    // symmetric path compiles to "private struct".
    use awase::MatchContext;
    use awase::mode::MatchResult;

    fn hit(map: &mut BindingMap<Deed>, hk: Hotkey) -> Option<Deed> {
        match map.match_key(hk, &MatchContext::default()) {
            MatchResult::Matched { action, .. } => Some(action),
            _ => None,
        }
    }

    #[test]
    fn arrows_and_hjkl_agree() {
        let (mut m, _) = default_bindings();
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
        let (mut m, _) = default_bindings();
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

    /// ★ THE LIST ITSELF HAS NO COLLISION. `try_bind` makes one a returned
    /// error rather than a silent last-wins overwrite, and this is what turns
    /// that from a runtime report into a build-time fact.
    #[test]
    fn no_chord_is_bound_twice() {
        let (_, clashes) = default_bindings();
        assert!(clashes.is_empty(), "duplicate bindings: {clashes:?}");
    }

    /// ★ THE CATALOG AND THE PARSER MUST NOT DRIFT. A `VERBS` entry the
    /// parser refuses tells an agent to try something that cannot work, which
    /// is worse than publishing no catalog at all.
    #[test]
    fn every_advertised_verb_parses() {
        for v in Deed::VERBS {
            assert!(Deed::parse(v).is_some(), "advertised but unparseable: {v}");
        }
        assert_eq!(Deed::parse("rm -rf /"), None);
        assert_eq!(Deed::parse(""), None);
        assert_eq!(Deed::parse("focus"), None, "a prefix is not a verb");
    }

    #[test]
    fn a_bare_key_is_never_a_deed() {
        let (mut m, _) = default_bindings();
        assert_eq!(hit(&mut m, Hotkey::new(Modifiers::NONE, Key::Q)), None);
    }
}

// ── ★ PERFORMING A DEED — the compositor side ────────────────────────────

/// What became of a deed.
///
/// ★ Exists because `do` answered `"queued: <verb>"` and nothing more. A deed
/// that becomes a NO-OP was indistinguishable from one that worked, which is
/// the exact shape of "the gesture did nothing and nothing said why".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeedOutcome {
    Performed,
    /// Carries a REASON rather than a bool: "nothing happened" and "nothing
    /// could have happened, here is why" are different answers, and only the
    /// second lets a caller stop guessing.
    Refused(&'static str),
}

impl DeedOutcome {
    /// The JSON a caller reads back from `deed_result`.
    #[must_use]
    pub fn to_json(&self, verb: &str, id: u64) -> serde_json::Value {
        match self {
            Self::Performed => serde_json::json!({
                "deed_id": id, "verb": verb, "outcome": "performed"
            }),
            Self::Refused(reason) => serde_json::json!({
                "deed_id": id, "verb": verb, "outcome": "refused", "reason": reason
            }),
        }
    }
}

impl crate::state::Omoya {
    /// Carry out one deed.
    ///
    /// Deliberately total over `Deed` with no `_ =>` arm: adding a verb to the
    /// enum must be a compile error here rather than a chord that silently
    /// does nothing. A binding that resolves to an unhandled action is the
    /// worst failure this layer has, because the key visibly stops reaching
    /// the application AND produces no effect — the operator concludes the
    /// keyboard is broken.
    pub fn perform(&mut self, deed: Deed) -> DeedOutcome {
        match deed {
            Deed::Focus(dir) => {
                self.focus_direction(dir);
                DeedOutcome::Performed
            }
            Deed::Resize(dir) => {
                // 0.05 of the parent, matching kukaku's MIN_RATIO: one press
                // is the smallest move that cannot collapse a pane, so a held
                // key ramps smoothly instead of snapping to an edge.
                if self.tiling.resize_focused(dir, 0.05) {
                    self.apply_layout();
                    DeedOutcome::Performed
                } else {
                    // ── ★ THE SILENT NO-OP, NAMED ──────────────────────────
                    //
                    // `resize_focused` already returned false here and the
                    // answer was DISCARDED, so the caller was told "queued" and
                    // nothing else. In floating mode this is not an edge case:
                    // `layout.rs` unmaps EVERY window from the tiling tree, so
                    // a resize deed can never do anything -- independently of
                    // the move_request/resize_request stubs, which are a
                    // separate hole in the same symptom.
                    //
                    // An operator reporting "I cannot resize" was therefore
                    // right twice over, and no leaf could say so.
                    DeedOutcome::Refused("no tiled window to resize (floating mode unmaps all)")
                }
            }
            Deed::Close => {
                self.close_focused();
                DeedOutcome::Performed
            }
            Deed::SpawnTerminal => {
                self.spawn_terminal();
                DeedOutcome::Performed
            }
            Deed::SpawnLauncher => {
                self.spawn_launcher();
                DeedOutcome::Performed
            }
        }
    }

    fn focus_direction(&mut self, dir: Direction) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        // Same zone the layout used — asking for a neighbour inside a
        // different rectangle than the one the windows were placed in gives
        // answers that are subtly wrong near the edges.
        let geo = {
            let mut map = smithay::desktop::layer_map_for_output(&output);
            map.arrange();
            let z = map.non_exclusive_zone();
            smithay::utils::Rectangle::new(
                (geo.loc.x + z.loc.x, geo.loc.y + z.loc.y).into(),
                z.size,
            )
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
        self.spawn_configured(
            self.session_command.clone(),
            "terminal",
            "Logo+Return pressed but this seat has no terminal command",
        );
    }

    fn spawn_launcher(&mut self) {
        self.spawn_configured(
            self.launcher_command.clone(),
            "launcher",
            "Ctrl+Space pressed but this seat has no launcher command \
             (start omoya with --launcher <cmd>)",
        );
    }

    /// Spawn one of the seat's own configured commands into its own display.
    ///
    /// ★ ONE body, two callers, because the second copy was going to be a
    /// hand-edit of the first and the `WAYLAND_DISPLAY` line is exactly the
    /// kind of thing a hand-edit drops. A launcher spawned without it connects
    /// to whatever display the compositor's own environment names — which on a
    /// nested run is the OUTER session, so the window opens on the wrong seat
    /// and the operator sees nothing.
    ///
    /// Takes the resolved command rather than reaching for a field, so the
    /// only thing that decides WHICH command runs is the caller's arm in
    /// `perform` — there is no place for a lookup to pick the wrong one.
    fn spawn_configured(&self, cmd: Option<Vec<String>>, what: &'static str, absent: &str) {
        let Some(cmd) = cmd else {
            // Nothing to spawn is a real state, not an error: omoya can be
            // run with no `-- <cmd>` at all. Logged rather than silent,
            // because a chord that does nothing is otherwise indistinguishable
            // from a chord that is not bound.
            tracing::warn!("{absent}");
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
            Ok(child) => tracing::info!(pid = child.id(), program, what, "spawned into the seat"),
            Err(e) => tracing::error!(error = %e, program, what, "spawn failed"),
        }
    }
}

#[cfg(test)]
mod deed_outcome_tests {
    use super::DeedOutcome;

    /// ★ "The gesture did nothing and nothing said why" (plo, 2026-08-29).
    ///
    /// A refusal must carry a REASON. `resize_focused` already returned false
    /// and the answer was DISCARDED, so `do` replied "queued" and the operator
    /// learned nothing.
    #[test]
    fn a_refusal_names_its_reason() {
        let j = DeedOutcome::Refused("no tiled window to resize").to_json("resize-left", 3);
        assert_eq!(j["outcome"], "refused");
        assert_eq!(j["reason"], "no tiled window to resize");
        assert_eq!(j["deed_id"], 3);
        assert_eq!(j["verb"], "resize-left");
    }

    /// Anti-vacuity: performed and refused must not serialise the same.
    /// A single "outcome" string that was always present would pass the test
    /// above while telling a caller nothing.
    #[test]
    fn performed_and_refused_are_distinguishable() {
        let p = DeedOutcome::Performed.to_json("focus-right", 1);
        let r = DeedOutcome::Refused("x").to_json("focus-right", 1);
        assert_ne!(p, r);
        assert_eq!(p["outcome"], "performed");
        assert!(p.get("reason").is_none(), "a success carries no reason");
    }
}
