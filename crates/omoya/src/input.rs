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
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::SERIAL_COUNTER,
};

use crate::state::Omoya;

impl Omoya {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                // Read BEFORE `event` is consumed by `keyboard.input`.
                let event_state = event.state();
                let Some(keyboard) = self.seat.get_keyboard() else {
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
                    event.key_code(),
                    event.state(),
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
                        if let Some(hk) = crate::chord::hotkey_from(modifiers, handle.modified_sym())
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
                            let hk = crate::chord::hotkey_from(
                                modifiers,
                                handle.modified_sym(),
                            );
                            if let Some(hk) = hk {
                                let m = state.bindings.match_key(
                                    hk,
                                    &awase::mode::MatchContext::default(),
                                );
                                if let awase::mode::MatchResult::Matched {
                                    action,
                                    consume,
                                } = m
                                {
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
                    self.perform(d);
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
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                // Accumulate, then clamp to the output. Without the clamp the
                // pointer walks off the edge and never comes back: nothing
                // else bounds it, and `surface_under` on an off-screen point
                // simply finds nothing, so the seat looks frozen rather than
                // wrong.
                let mut loc = self.pointer_location + event.delta();
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
                        time: event.time_msec(),
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
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        utime: event.time(),
                    },
                );
                pointer.frame(self);
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
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                // Click-to-focus, and raise. This IS window-management policy —
                // omoya's own, not smithay's — and it is the smallest possible
                // amount of it that makes the seat usable.
                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
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
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let horizontal = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 3.0 / 120.
                });
                let vertical = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 3.0 / 120.
                });

                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                if horizontal != 0.0 {
                    frame = frame.relative_direction(
                        Axis::Horizontal,
                        event.relative_direction(Axis::Horizontal),
                    );
                    frame = frame.value(Axis::Horizontal, horizontal);
                    if let Some(v120) = event.amount_v120(Axis::Horizontal) {
                        frame = frame.v120(Axis::Horizontal, v120 as i32);
                    }
                }
                if vertical != 0.0 {
                    frame = frame.relative_direction(
                        Axis::Vertical,
                        event.relative_direction(Axis::Vertical),
                    );
                    frame = frame.value(Axis::Vertical, vertical);
                    if let Some(v120) = event.amount_v120(Axis::Vertical) {
                        frame = frame.v120(Axis::Vertical, v120 as i32);
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
