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
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
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
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };

                let mut owed: Option<String> = None;
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
                            state.owed_vt_switches += 1;
                            owed = Some(format!("{hk} — {}", claim.purpose));
                        }
                        FilterResult::Forward
                    },
                );
                if let Some(what) = owed {
                    tracing::info!(
                        chord = %what,
                        owed_total = self.owed_vt_switches,
                        "reserved chord recognised but NOT acted on (nested backend owns no VT)"
                    );
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
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
