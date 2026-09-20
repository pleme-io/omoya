//! Input routing.
//!
//! ★ The one thing here that is omoya's and not smallvil's: **every keyboard
//! event is checked against `awase::Reserved::fleet_linux()` before it is
//! forwarded to a client.**
//!
//! A compositor is the last thing between a held-down `Ctrl+Alt+F2` and the
//! kernel, and one that silently eats it has taken away the operator's escape
//! hatch out of a wedged session — on a machine whose only display is that
//! session, that is a soft brick recoverable only by power-cycling. The catalog
//! exists precisely so this is a *lookup* rather than a thing each compositor
//! remembers.
//!
//! Today (M2, nested) omoya **cannot** act on the chord: VT switching needs the
//! DRM/VT backend, which is M4. So what happens here is that the chord is
//! recognised and logged as owed rather than silently forwarded — and the
//! `owed_vt_switches` counter is what M4's test will assert against. Recognising
//! it and saying so is honest; pretending to handle it would not be.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, Keycode, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::SERIAL_COUNTER,
};

use crate::state::Omoya;

/// Two titlebar presses on the same window within this are a double-click.
/// 400 ms sits between macOS's and Windows' defaults (both configurable,
/// both near 500), short enough that two separate drags are not mistaken
/// for one.
pub const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

impl Omoya {
    /// Handle one key, from wherever it came.
    ///
    /// ★ EXTRACTED SO THERE IS EXACTLY ONE ANSWER TO "WHAT DOES THIS KEY DO".
    /// The reserved-chord filter, the deed dispatch and the forward-to-client
    /// decision all live here, and both the evdev backend and the kanshou
    /// write surface call it. A synthetic key that took a shortcut past the
    /// filter would exercise a path no real key uses, which is precisely the
    /// thing that makes an injection test worthless.
    ///
    /// `code` is an XKB keycode — i.e. the evdev code plus 8. The backend
    /// applies that offset in `KeyboardKeyEvent::key_code`; a caller
    /// synthesising a key must apply it too.
    pub fn key(&mut self, code: Keycode, state: KeyState, time: u32) {
        // ★ REMAP FIRST, SO EVERYTHING DOWNSTREAM AGREES. CapsLock is Escape
        // on this seat (see `crate::remap`, and why it cannot be an xkb
        // option here). Doing it above the chord filter means `awase`, the
        // deed dispatch and the client's own keymap all see a real Escape —
        // remapping the keysym later would leave the chord layer still
        // matching CapsLock, so bindings and fingers would disagree.
        let code = crate::remap::apply_table(code, &self.remaps);
        self.key_remapped(code, state, time);
    }

    /// Release every key the seat still believes is held.
    ///
    /// ── ★ CALLED ON VT RESUME, BECAUSE THE RELEASES NEVER ARRIVED ────────
    /// Ctrl+Alt+F2 hands the seat away between the PRESS and the RELEASE:
    /// logind pauses the devices, the releases go to the other VT, and
    /// `xkb_state` comes back still holding Ctrl and Alt down. Every
    /// subsequent keystroke is then a chord — the operator returns to a
    /// keyboard that types nothing and looks broken, and the only cure is
    /// tapping both modifiers to "un-stick" them, which nobody guesses.
    ///
    /// Routed through `key_remapped`, not `key`: `pressed_keys()` holds codes
    /// that have ALREADY been through the remap table, and re-applying it
    /// would double-map any chain an operator configured (the default
    /// CapsLock→Escape happens to be idempotent, which is exactly the kind of
    /// accident that makes this bite someone else later).
    pub fn release_all_keys(&mut self, time: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let held: Vec<Keycode> = keyboard.pressed_keys().into_iter().collect();
        if held.is_empty() {
            return;
        }
        tracing::info!(
            count = held.len(),
            "releasing keys the seat still believed were held"
        );
        for code in held {
            self.key_remapped(code, KeyState::Released, time);
        }
    }

    /// [`Self::key`] with the remap already applied.
    fn key_remapped(&mut self, code: Keycode, state: KeyState, time: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let event_state = state;
        let Some(keyboard) = self.seat.get_keyboard() else {
            // ★ THE SILENT DROP, NAMED. A seat with no keyboard swallows every
            // key and says nothing — and from outside, that is indistinguishable
            // from a keyboard nobody is typing on. `add_keyboard` currently
            // `.expect()`s at startup so this should be unreachable, which is
            // exactly why it earns a log rather than a bare `return`.
            tracing::error!("a key arrived but the seat has no keyboard — dropping it");
            return;
        };

        let mut owed: Option<String> = None;
        let mut switched: Option<i32> = None;
        // The deed a chord asked for, carried OUT of the filter
        // closure. It cannot be performed inside: the closure holds
        // `&mut Omoya` as `state`, and every deed needs the whole
        // compositor — spawning reads the session command, focus
        // moves the seat's keyboard focus, closing sends a configure.
        // Deciding inside and acting outside is what keeps the filter
        // a pure classification.
        let mut deed: Option<crate::deed::Deed> = None;
        keyboard.input::<(), _>(
            self,
            code,
            state,
            serial,
            time,
            |state, modifiers, handle| {
                // ★ The reserved-chord check, for real — the adapter it
                // used to wait for is `crate::chord`.
                //
                // What happens on a HIT is deliberately not "swallow".
                // In the nested backend omoya does not own the VT: the
                // host X server or compositor does, and it is the one
                // that must see Ctrl+Alt+F<n>. Eating it here would
                // take away an escape hatch omoya cannot itself
                // provide — strictly worse than forwarding.
                //
                // So M2 RECOGNISES and COUNTS. M4, which owns the VT,
                // swaps the Forward below for the actual switch, and
                // `owed_vt_switches` returning to zero is how its test
                // proves it. Recognising it and saying so is honest;
                // pretending to handle it would not be.
                // ★ PRESSES ONLY. smithay runs this filter for BOTH
                // directions — `input_intercept` calls it after
                // `key_input(keycode, state)` for Pressed and Released alike —
                // and on the release the modifiers are still held and
                // `modified_sym()` is unchanged, so `claim_on` matched a SECOND
                // time. One press of Ctrl+Alt+Delete took `owed_vt_switches`
                // from 0 to 2 (nothing decrements it: `vt_of` is None for
                // Delete), and a real VT chord invoked `sw(vt)` twice.
                //
                // The deed arm below has carried this guard from the start,
                // with a comment about why feeding releases to a stateful
                // matcher is wrong. The same reasoning was simply never
                // applied one arm up.
                if event_state == KeyState::Pressed
                    && let Some(hk) = crate::chord::hotkey_from(modifiers, handle.modified_sym())
                    && let Some(claim) = state.reserved.claim_on(&hk)
                {
                    // ★ ACT ON IT. This counted and forwarded, which
                    // was right in the NESTED backend — there the host
                    // owns the VT and eating the chord would remove an
                    // escape omoya cannot provide. On DRM omoya owns
                    // the seat, and forwarding hands it to a kernel
                    // that logind's TakeControl has already put in
                    // K_OFF, so it reaches nothing at all.
                    //
                    // The counter still increments and is only undone
                    // by a switch that returns Ok, so it keeps meaning
                    // "chords seen that produced no switch" rather than
                    // becoming decoration.
                    state.owed_vt_switches += 1;
                    owed = Some(format!("{hk} — {}", claim.purpose));
                    if let Some(vt) = crate::chord::vt_of(&hk) {
                        if let Some(sw) = state.vt_switch.as_mut() {
                            match sw(vt) {
                                Ok(()) => {
                                    state.owed_vt_switches =
                                        state.owed_vt_switches.saturating_sub(1);
                                    switched = Some(vt);
                                }
                                Err(e) => tracing::error!(
                                    vt, error = %e,
                                    "VT switch REFUSED — this seat has no escape hatch"
                                ),
                            }
                        } else {
                            tracing::error!(
                                vt,
                                "VT chord seen but no session can switch — no escape hatch"
                            );
                        }
                    }
                }

                // ── ★ SEAT DEEDS: CLASSIFY HERE, ACT BELOW ────────
                //
                // Only on PRESS. `match_key` is stateful — it drives
                // awase's chord sequencing — so feeding it releases
                // too would advance a pending sequence twice per
                // keystroke and make every two-key chord unreachable.
                //
                // And CONSUMED, which is the opposite of the VT arm
                // above: a VT chord is forwarded because the seat
                // cannot provide the escape it represents, while a
                // seat deed must never also reach the client, or
                // Logo+Q closes the window AND the client reads a Q.
                if event_state == KeyState::Pressed {
                    let hk = crate::chord::hotkey_from(modifiers, handle.modified_sym());
                    if let Some(hk) = hk {
                        let m = state
                            .bindings
                            .match_key(hk, &awase::MatchContext::default());
                        if let awase::mode::MatchResult::Matched { action, consume } = m {
                            deed = Some(action);
                            if consume {
                                return FilterResult::Intercept(());
                            }
                        }
                    }
                }
                FilterResult::Forward
            },
        );
        if let Some(d) = deed {
            // ★ COUNTED HERE, AND THE ABSENCE OF THIS COUNTER IS WHY A DEAD
            // KEYMAP SURVIVED FOR DAYS.
            //
            // `deeds_performed` counts only deeds requested over kanshou — its
            // own increment site says "requested over kanshou" — so the
            // KEYBOARD path had no counter at all. When `chord::key_from`
            // translated none of the seat's own keys, every chord silently
            // stopped working and every published number stayed exactly as it
            // had been. There was nothing to look at.
            //
            // `chord_deeds` is the number that would have said so on day one:
            // a seat whose operator is typing and whose chord counter never
            // moves is a seat whose keymap is not connected.
            // ★ COUNTED ON THE OUTCOME, like the kanshou drain. A chord
            // that resolves to a deed the seat then declines is not a
            // performed chord — counting it as one is how a keymap that
            // "works" can sit on top of a seat where nothing moves.
            let outcome = self.perform(d);
            let counter = match outcome {
                crate::deed::DeedOutcome::Performed => &self.introspect.chord_deeds,
                crate::deed::DeedOutcome::Refused(reason) => {
                    tracing::info!(reason, "chord deed refused");
                    // ★ THE CHORD PATH'S OWN COUNTER, BOTH WAYS. The performed
                    // arm above already uses `chord_deeds`, whose doc says
                    // "two paths reaching one action need two counters, or the
                    // quiet one is invisible" — and then the refusal arm merged
                    // the two paths back into one. `deeds_performed` is written
                    // ONLY by the kanshou drain, so an operator pressing
                    // Logo+Left with nothing to the left made an agent read
                    // `performed: 0, refused: 1` and conclude a deed it never
                    // sent had been declined.
                    &self.introspect.chord_deeds_refused
                }
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(vt) = switched {
            tracing::info!(vt, "VT switch performed — the seat released the display");
        }
        if let Some(what) = owed {
            tracing::info!(
                chord = %what,
                owed_total = self.owed_vt_switches,
                "reserved chord recognised"
            );
        }
    }

    /// Apply one expanded synthetic step.
    ///
    /// ★ Every arm routes through the SAME method the evdev backend does, so
    /// "it works when synthesised" and "it works from the keyboard" are
    /// claims about the same code. That is the entire diagnostic value of
    /// this surface; a shortcut here would answer a question nobody asked.
    pub fn apply_step(&mut self, step: crate::synth::Step) {
        // ── ★ THE SAME BASE AS A REAL EVENT, WHICH THIS WAS NOT ─────────
        //
        // The comment here used to assert "the two are the same monotonic
        // base, so a client cannot tell them apart by timestamp" and the code
        // said otherwise. Synthetic events were stamped
        // `start_time.elapsed()` — ms since the compositor started — while a
        // real event carries `Event::time_msec()`, which for this backend is
        // `evdev`'s `timestamp()`: a `SystemTime`, i.e. CLOCK_REALTIME,
        // because omoya never issues `EVIOCSCLOCKID`.
        //
        // Both go into the same `wl_pointer`/`wl_keyboard` time field, so the
        // first synthetic event after a real one moved the clock BACKWARDS by
        // ~1.6e9 ms inside a single focus. Toolkits difference successive
        // timestamps for double-click and key-repeat.
        //
        // ★ THE DESTINATION IS CLOCK_MONOTONIC FOR BOTH, and this is not it.
        // The protocol wants a monotonic base, and getting the DEVICE onto one
        // means `EVIOCSCLOCKID` — an ioctl the `evdev` crate does not expose,
        // so a raw `_IOW('E', 0xa0, int)` and a new unsafe seam, or projecting
        // each device stamp onto `start_time` (which needs that `Instant`
        // threaded into `EvdevBackend`). Either is a real change. Until one
        // lands, matching the device's base removes the DIVERGENCE, which is
        // the part a client can actually see: an NTP step now moves both paths
        // together instead of separating them forever.
        #[allow(clippy::cast_possible_truncation)]
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u32);
        match step {
            crate::synth::Step::Key { code, state } => {
                // The `+8`, applied here exactly as `KeyboardKeyEvent::key_code`
                // applies it for a real device.
                self.key(Keycode::new(code + 8), state, time);
            }
            crate::synth::Step::Motion { dx, dy } => {
                self.pointer_motion(dx, dy, time);
            }
            crate::synth::Step::Button { code, pressed } => {
                self.pointer_button(code, pressed, time);
            }
        }
    }

    /// Move the pointer by a relative delta.
    ///
    /// Extracted so the evdev backend and the kanshou write surface move the
    /// pointer by the same code — including the clamp, which is the part that
    /// is easy to omit and whose absence looks like a frozen seat rather than
    /// a missing bound.
    pub fn pointer_motion(&mut self, dx: f64, dy: f64, time: u32) {
        let delta: smithay::utils::Point<f64, smithay::utils::Logical> = (dx, dy).into();

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        // Accumulate, then clamp to the output. Without the clamp the
        // pointer walks off the edge and never comes back: nothing
        // else bounds it, and `surface_under` on an off-screen point
        // simply finds nothing, so the seat looks frozen rather than
        // wrong.
        let mut loc = self.pointer_location + delta;
        if let Some(output) = self.space.outputs().next().cloned()
            && let Some(geo) = self.space.output_geometry(&output)
        {
            let max_x = f64::from(geo.loc.x + geo.size.w);
            let max_y = f64::from(geo.loc.y + geo.size.h);
            loc.x = loc.x.clamp(f64::from(geo.loc.x), max_x);
            loc.y = loc.y.clamp(f64::from(geo.loc.y), max_y);
        }
        self.pointer_location = loc;

        let serial = SERIAL_COUNTER.next_serial();
        let under = self.surface_under(loc);
        pointer.motion(
            self,
            under.clone(),
            &MotionEvent {
                location: loc,
                serial,
                time: time,
            },
        );
        // ★ `relative_motion` IN ADDITION to `motion`, not instead.
        // `motion` is what moves the cursor; `relative_motion` is the
        // zwp_relative_pointer protocol, which is how a game or a 3D
        // viewport gets un-clamped deltas after locking the pointer.
        // Sending only the first makes those clients unusable in a way
        // that looks like the compositor ignoring them.
        pointer.relative_motion(
            self,
            under,
            &RelativeMotionEvent {
                delta: delta,
                delta_unaccel: delta,
                utime: u64::from(time) * 1000,
            },
        );
        pointer.frame(self);
    }

    /// Press or release a pointer button, by evdev code.
    ///
    /// Extracted alongside `pointer_motion` and for the same reason: the
    /// click-to-focus-and-raise policy below is omoya's own, and a synthetic
    /// click that skipped it would move focus differently from a real one.
    pub fn pointer_button(&mut self, code: u32, pressed: bool, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let button = code;
        let button_state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };

        // Click-to-focus, and raise. This IS window-management policy —
        // omoya's own, not smithay's — and it is the smallest possible
        // amount of it that makes the seat usable.
        if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
            // ── ★ HOISTED OUT OF THE `element_under` GUARD (2026-09-03) ──
            //
            // This block used to sit INSIDE `if let Some(..) = element_under(..)`,
            // which it can never satisfy: the bar is drawn ABOVE the content
            // (`chrome::bar_rect`), is a `MemoryRenderBufferRenderElement` rather
            // than a `wl_surface`, and so is in no bbox and no input region.
            // smithay's `element_under` filters on exactly those, so a click on a
            // titlebar over bare desktop returned `None` and the whole handler was
            // skipped — close, minimize, maximize AND drag, all of them, always.
            //
            // ★ IT APPEARED TO WORK ONLY BY ACCIDENT OF OVERLAP. plo's
            // `cascade_step` is 24 and `chrome::HEIGHT` is 24, so window N's bar
            // lands exactly on window N-1's first content row; `element_under`
            // then returned N-1 and the scan below found N's bar. The BOTTOM
            // window — the one the seat opens with — has empty desktop under its
            // bar and was completely inert. That is precisely the operator's
            // report: the launcher could be dragged, the startup mado could not.
            //
            // The comment below already stated the rule this violated. It was
            // written correctly and then placed inside the guard it forbids.
            // ── ★ THE TITLEBAR: WHERE THE MOUSE CAN NOW GO ───────────────
            //
            // Hit-tested BEFORE the Super+drag path and before focus,
            // because a click on the chrome is never a click on the
            // client — the bar is the compositor's own surface and the
            // window beneath must not also see it.
            //
            // Geometry comes from `chrome`, the same module the renderer
            // draws from, so a button responds exactly where it is drawn.
            //
            // ★ `element_under` finds a window by its CONTENT rect, and
            // the bar is ABOVE that — outside it. So the chrome is
            // hit-tested over every window in the space rather than only
            // the one under the pointer, or the bar would be unclickable
            // for the very reason it is visible.
            // The same floating-only guard the renderer and the layout
            // carry. A click that hit chrome nobody drew would be a
            // window closing because the operator clicked empty desktop.
            if button == 0x110 && self.config.layout.mode == crate::config::LayoutMode::Floating {
                let p = pointer.current_location();
                // ★ `.rev()` — FRONT TO BACK. `Space::elements()` yields
                // back-to-front (smithay's own doc) and `find_map` takes the
                // FIRST match, so without this the BOTTOM window wins wherever
                // two bars overlap — and at `cascade_step == chrome::HEIGHT`
                // they overlap constantly. Clicking the visible top window's
                // close button would close the one behind it.
                let chrome_hit = self.space.elements().rev().find_map(|w| {
                    // ★ THE ROLE DECIDES, AND IT DECIDES BY TYPE. An overlay's
                    // policy yields no `Decorated`, so `chrome::hit` cannot be
                    // called for it — the launcher has no bar to hit.
                    let decorated =
                        crate::role::policy_of(w, &self.config.placement).decorated()?;
                    let geo = self.space.element_geometry(w)?;
                    crate::chrome::hit(decorated, geo, p).map(|h| (w.clone(), geo, h))
                });
                if let Some((w, geo, what)) = chrome_hit {
                    self.space.raise_element(&w, true);
                    // ★ FOCUS FIRST, THEN DELEGATE TO THE DEED. The three
                    // buttons are the SAME verbs as Logo+Q / Logo+M /
                    // Logo+F, and every one of those acts on the focused
                    // window. Re-implementing them here would be two
                    // implementations of one verb, free to diverge — the
                    // shape where "the button does something subtly
                    // different from the shortcut" comes from. Focusing
                    // the clicked window first makes "act on focus" and
                    // "act on what I clicked" the same statement.
                    keyboard.set_focus(self, w.toplevel().map(|t| t.wl_surface().clone()), serial);
                    match what {
                        crate::chrome::Hit::Close => {
                            // A titlebar button on a window that cannot take
                            // the deed: nothing to report to, so the refusal
                            // is logged rather than counted.
                            if let crate::deed::DeedOutcome::Refused(reason) =
                                self.perform(crate::deed::Deed::Close)
                            {
                                tracing::info!(reason, "titlebar deed refused");
                            }
                        }
                        crate::chrome::Hit::Minimize => {
                            // A titlebar button on a window that cannot take
                            // the deed: nothing to report to, so the refusal
                            // is logged rather than counted.
                            if let crate::deed::DeedOutcome::Refused(reason) =
                                self.perform(crate::deed::Deed::Minimize)
                            {
                                tracing::info!(reason, "titlebar deed refused");
                            }
                        }
                        crate::chrome::Hit::Maximize => {
                            // A titlebar button on a window that cannot take
                            // the deed: nothing to report to, so the refusal
                            // is logged rather than counted.
                            if let crate::deed::DeedOutcome::Refused(reason) =
                                self.perform(crate::deed::Deed::ToggleMaximize)
                            {
                                tracing::info!(reason, "titlebar deed refused");
                            }
                        }
                        crate::chrome::Hit::Drag
                            if self.last_titlebar_press.as_ref().is_some_and(|(prev, at)| {
                                *prev == w && at.elapsed() <= DOUBLE_CLICK
                            }) =>
                        {
                            // ── ★ DOUBLE-CLICK THE TITLEBAR: MAXIMISE ────
                            // The same verb as the button and Logo+F, so the
                            // three can never disagree about what maximise is.
                            self.last_titlebar_press = None;
                            if let crate::deed::DeedOutcome::Refused(reason) =
                                self.perform(crate::deed::Deed::ToggleMaximize)
                            {
                                tracing::info!(reason, "titlebar double-click refused");
                            }
                        }
                        crate::chrome::Hit::Drag => {
                            self.last_titlebar_press = Some((w.clone(), std::time::Instant::now()));
                            #[allow(clippy::cast_possible_truncation)]
                            let offset =
                                smithay::utils::Point::<i32, smithay::utils::Logical>::from((
                                    geo.loc.x - p.x as i32,
                                    geo.loc.y - p.y as i32,
                                ));
                            let start_data = smithay::input::pointer::GrabStartData {
                                focus: None,
                                button,
                                location: p,
                            };
                            pointer.set_grab(
                                self,
                                crate::grab::MoveGrab {
                                    start_data,
                                    window: w.clone(),
                                    offset,
                                },
                                serial,
                                smithay::input::pointer::Focus::Clear,
                            );
                        }
                    }
                    return;
                }

                // ── ★ THE MARGIN AROUND A FRAME RESIZES IT ───────────────
                // A press just OUTSIDE a floating window's frame grabs that
                // edge (or corner), as on macOS and Windows. Outside, because
                // inside belongs to the client. Front to back, for the same
                // reason as the titlebar scan above: where two margins meet,
                // the visible window wins. Overlays (the launcher) are not
                // resizable — they size themselves.
                let border = self.border_under(p);
                if let Some((w, edges)) = border {
                    self.space.raise_element(&w, true);
                    keyboard.set_focus(self, w.toplevel().map(|t| t.wl_surface().clone()), serial);
                    let start_data = smithay::input::pointer::GrabStartData {
                        focus: None,
                        button,
                        location: p,
                    };
                    if let Some(grab) = crate::grab::ResizeGrab::begin(self, w, edges, start_data) {
                        self.active_resize = Some(edges);
                        pointer.set_grab(self, grab, serial, smithay::input::pointer::Focus::Clear);
                    }
                    return;
                }
            }

            if let Some((window, _loc)) = self
                .space
                .element_under(pointer.current_location())
                .map(|(w, l)| (w.clone(), l))
            {
                // ── ★ SUPER + DRAG MOVES THE WINDOW ─────────────────────────
                //
                // `MoveGrab` already existed and was reachable only through
                // `move_request` — i.e. only when a CLIENT asks, which a client
                // does when the operator drags ITS titlebar. mado has no
                // titlebar: this seat draws server-side decorations, so mado
                // never sends `xdg_toplevel.move` and the grab had no trigger.
                // The machinery was complete and unreachable, which is why the
                // operator still could not drag a window after it landed.
                //
                // Super+drag is the compositor-side trigger. It needs no client
                // cooperation, so it works for every toplevel including ones
                // with server-side decorations and ones that implement no move
                // protocol at all.

                let logo_held = keyboard.modifier_state().logo;
                // ── ★ LOGO + RIGHT-DRAG RESIZES, FROM THE NEAREST CORNER ──────
                // The companion to Logo+left-drag move, and the reason an
                // operator never has to aim at the 8 px margin: grab anywhere
                // in the window, and the quadrant the pointer is in picks the
                // corner that moves. Floating only — the tree owns tiled rects.
                if logo_held
                    && button == 0x111
                    && self.config.layout.mode == crate::config::LayoutMode::Floating
                    && crate::role::policy_of(&window, &self.config.placement).resizable
                {
                    if let Some(geo) = self.space.element_geometry(&window) {
                        let p = pointer.current_location();
                        let (cx, cy) = (
                            f64::from(geo.loc.x) + f64::from(geo.size.w) / 2.0,
                            f64::from(geo.loc.y) + f64::from(geo.size.h) / 2.0,
                        );
                        let edges = crate::grab::Edges {
                            left: p.x < cx,
                            right: p.x >= cx,
                            top: p.y < cy,
                            bottom: p.y >= cy,
                        };
                        let start_data = smithay::input::pointer::GrabStartData {
                            focus: None,
                            button,
                            location: p,
                        };
                        self.space.raise_element(&window, true);
                        if let Some(grab) =
                            crate::grab::ResizeGrab::begin(self, window.clone(), edges, start_data)
                        {
                            self.active_resize = Some(edges);
                            pointer.set_grab(
                                self,
                                grab,
                                serial,
                                smithay::input::pointer::Focus::Clear,
                            );
                        }
                        return;
                    }
                }
                if logo_held
                    && button == 0x110
                    && crate::role::policy_of(&window, &self.config.placement).movable
                {
                    if let Some(geo) = self.space.element_geometry(&window) {
                        let p = pointer.current_location();
                        #[allow(clippy::cast_possible_truncation)]
                        let offset = smithay::utils::Point::<i32, smithay::utils::Logical>::from((
                            geo.loc.x - p.x as i32,
                            geo.loc.y - p.y as i32,
                        ));
                        let start_data = smithay::input::pointer::GrabStartData {
                            focus: None,
                            button,
                            location: p,
                        };
                        self.space.raise_element(&window, true);
                        pointer.set_grab(
                            self,
                            crate::grab::MoveGrab {
                                start_data,
                                window: window.clone(),
                                offset,
                            },
                            serial,
                            smithay::input::pointer::Focus::Clear,
                        );
                        return;
                    }
                }

                self.space.raise_element(&window, true);
                keyboard.set_focus(
                    self,
                    window.toplevel().map(|t| t.wl_surface().clone()),
                    serial,
                );
                self.space.elements().for_each(|w| {
                    if let Some(t) = w.toplevel() {
                        t.send_pending_configure();
                    }
                });
            } else {
                // Clicking the background deactivates every toplevel.
                // There is no `set_activated` on `ToplevelSurface` —
                // activation is xdg-shell STATE, so it goes through the
                // pending-state seam and reaches the client only on the
                // configure below. Same shape smithay's own
                // `desktop::Window` uses internally.
                self.space.elements().for_each(|w| {
                    if let Some(t) = w.toplevel() {
                        t.with_pending_state(|state| {
                            state.states.unset(xdg_toplevel::State::Activated);
                        });
                        t.send_pending_configure();
                    }
                });
                keyboard.set_focus(self, None, serial);
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time: time,
            },
        );
        pointer.frame(self);
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        // ★ THE POINTER IS OURS TO DRAW, SO IT IS OURS TO MARK.
        //
        // Nothing commits when the mouse moves — the cursor is omoya's own
        // render element, not a client surface — so a damage-driven loop that
        // only listened to commits would leave the pointer frozen on screen
        // while every window kept updating normally. That reads as "the mouse
        // is broken", which is a long way from the actual cause.
        //
        // Matched on a reference, before `event` is consumed by the arms
        // below, and covering BOTH motion shapes: mice emit relative motion
        // and tablets/touchscreens absolute, so listening to one is the
        // asymmetry that makes a bug appear on exactly one class of device.
        // Buttons and axes count too — a click can move focus, and the focus
        // border is likewise drawn by us.
        match &event {
            InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::PointerButton { .. }
            | InputEvent::PointerAxis { .. } => self.owed.mark(crate::owed::Owed::Pointer),
            _ => {}
        }

        match event {
            InputEvent::Keyboard { event, .. } => {
                // ★ DELEGATED, SO SYNTHETIC INPUT TAKES THE IDENTICAL ROUTE.
                // `key` is also called by the kanshou write surface
                // (`Synth::Key`), and the only way that surface is worth
                // anything as a diagnostic is if it cannot diverge from what a
                // real key does — a second copy of the chord filter would make
                // "it works when I inject it" mean nothing about the keyboard.
                let time = Event::time_msec(&event);
                self.key(event.key_code(), event.state(), time);
            }
            // ── ★ RELATIVE MOTION: WHAT A MOUSE ACTUALLY SENDS ────────────
            // This arm did not exist, and its absence was invisible for a
            // structural reason worth recording: libinput emits
            // `PointerMotion` (a DELTA) for mice, and winit emits only
            // `PointerMotionAbsolute`. So the nested backend — the one used
            // for development — exercised the absolute arm exclusively, while
            // the DRM backend on a real seat sent deltas straight into the
            // catch-all `_ => {}` below. A mouse on plo moved nothing, and no
            // amount of testing in the nested backend could have shown it.
            InputEvent::PointerMotion { event, .. } => {
                let d = event.delta();
                self.pointer_motion(d.x, d.y, event.time_msec());
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                // ★ Keep the shared location current. Both arms move the same
                // pointer, so an absolute event that did not write here would
                // leave the next relative delta accumulating from wherever the
                // mouse last was — the cursor would jump backwards the moment
                // someone touched a tablet and then moved a mouse.
                self.pointer_location = pos;
                let serial = SERIAL_COUNTER.next_serial();
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                self.pointer_button(
                    event.button_code(),
                    event.state() == ButtonState::Pressed,
                    event.time_msec(),
                );
            }
            InputEvent::PointerAxis { event, .. } => {
                // ── ukeire: the seat's scroll intake ─────────────────────
                //
                // ★ THE TWO PATHS TAKE DIFFERENT PARTS OF THE POLICY, AND
                // THAT ASYMMETRY IS DELIBERATE. `amount()` is already in
                // the device's own continuous units — scaling it by a
                // lines-per-detent factor would be applying a discrete-wheel
                // conversion to a trackpad, so it takes the DIRECTION only.
                // `amount_v120` is 120 units per detent and takes the whole
                // multiplier, including the `/120` wire conversion that
                // `v120_multiplier` owns and does not expose.
                let scroll = self.config.ukeire.scroll;
                let mul = scroll.v120_multiplier();
                let sign = scroll.direction.sign();
                // ★ AND A WHEEL'S `amount()` TAKES THE FACTOR TOO (2026-09-19).
                // The asymmetry above is right for a TRACKPAD and wrong for the
                // only device on this seat: evdev's `amount()` returns `Some`
                // for a wheel as well — the raw detent count, 1.0 per click —
                // so the live path was always `a * sign` and
                // `ukeire.scroll.factor` multiplied nothing at all. The knob
                // was inert on the only shipping backend. A wheel is discrete,
                // so lines-per-detent applies to it whichever accessor carries
                // the value; `Finger`/`Continuous` sources keep direction only.
                let wheel = event.source() == AxisSource::Wheel;
                let lines = if wheel { scroll.factor.get() } else { 1.0 };
                let horizontal = event.amount(Axis::Horizontal).map_or_else(
                    || event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * mul,
                    |a| a * sign * lines,
                );
                let vertical = event.amount(Axis::Vertical).map_or_else(
                    || event.amount_v120(Axis::Vertical).unwrap_or(0.0) * mul,
                    |a| a * sign * lines,
                );

                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                if horizontal != 0.0 {
                    frame = frame.relative_direction(
                        Axis::Horizontal,
                        event.relative_direction(Axis::Horizontal),
                    );
                    frame = frame.value(Axis::Horizontal, horizontal);
                    if let Some(v120) = event.amount_v120(Axis::Horizontal) {
                        // ★ THE SAME SIGN *AND THE SAME FACTOR* AS THE VALUE
                        // ABOVE. The sign half was fixed on 2026-09-19 and the
                        // factor half was not, which left `ukeire.scroll.factor`
                        // still inert for every client that reads the discrete
                        // channel — and that is every client: winit "prefer[s]
                        // the discrete values if they are present" and DISCARDS
                        // the absolute delta, and GTK4/Qt6/Chromium all bind
                        // wl_pointer v8 and read `axis_value120`. omoya
                        // advertises wl_seat v9, so both channels are live, and
                        // one detent at the default factor emitted `value = 3.0`
                        // beside `v120 = 120`: two contradictory magnitudes in
                        // ONE frame.
                        frame = frame.v120(Axis::Horizontal, (v120 * sign * lines) as i32);
                    }
                }
                if vertical != 0.0 {
                    frame = frame.relative_direction(
                        Axis::Vertical,
                        event.relative_direction(Axis::Vertical),
                    );
                    frame = frame.value(Axis::Vertical, vertical);
                    if let Some(v120) = event.amount_v120(Axis::Vertical) {
                        // The same sign and the same factor — see the
                        // horizontal arm above for why both halves matter.
                        frame = frame.v120(Axis::Vertical, (v120 * sign * lines) as i32);
                    }
                }
                if event.source() == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod chrome_reachability_tests {
    /// ★ THE BUG THIS PINS: the chrome handler was UNREACHABLE, not wrong.
    ///
    /// Every piece of titlebar logic — `chrome::hit`, the Close/Minimize/
    /// Maximize dispatch, and the `Hit::Drag` → `MoveGrab` — sat inside
    /// `if let Some(..) = self.space.element_under(..)`. That guard can never
    /// hold for a titlebar click: the bar is drawn ABOVE the content
    /// (`chrome::bar_rect`), is a render element rather than a `wl_surface`,
    /// and so is in no bbox and no input region — which is exactly what
    /// smithay's `element_under` filters on.
    ///
    /// It LOOKED like it worked because plo's `cascade_step` (24) equals
    /// `chrome::HEIGHT` (24), so window N's bar lands on window N-1's content
    /// and the scan found it by accident. The bottom window — the one the
    /// seat opens with — had bare desktop under its bar and was fully inert.
    ///
    /// ── ★ WHY THIS IS A SOURCE-ORDER TEST AND NOT A BEHAVIOURAL ONE ──────
    /// `chrome::hit` was always correct in isolation and is unit-tested as
    /// such; no test of it could have caught this, because the defect was
    /// that nothing CALLED it. The invariant that was actually violated is an
    /// ordering one — chrome must be hit-tested BEFORE, and outside, the
    /// client-surface lookup — and that is what this asserts.
    #[test]
    fn chrome_is_hit_tested_before_the_client_surface_lookup() {
        let src = include_str!("input.rs");
        // Cut at the first `#[cfg(test)]` and strip comments, so this test
        // does not match its own prose. Three separate gates fell into that
        // trap in this repo on one day; it is the default behaviour of the
        // technique, not bad luck.
        let code: String = src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        let chrome = code
            .find("chrome_hit")
            .expect("the chrome hit-test must exist in this file");
        let under = code
            .find("element_under")
            .expect("the client-surface lookup must exist in this file");

        assert!(
            chrome < under,
            "the chrome hit-test appears AFTER `element_under` (byte {chrome} \
             vs {under}), which means it is once again nested inside a guard a \
             titlebar click can never satisfy. The bar is not a wl_surface; \
             `element_under` cannot see it. Hoist the block back out."
        );
    }
}
