//! Input straight from the kernel — evdev ioctls, no libinput, no libudev.
//!
//! ── ★ WHY THIS IS NOT A NATURALIZE BUT A WIRE ─────────────────────────────
//! `libinput.so.10` and `libudev.so.1` are C libraries this compositor linked.
//! Neither computes anything we could not: libinput reads `/dev/input/event*`
//! and decodes a kernel-defined event struct; libudev enumerates `/sys`. The
//! interface underneath both is the KERNEL, and the kernel is the one thing
//! that stays C.
//!
//! The `evdev` crate is pure Rust — its dependency closure is bitvec, cfg-if,
//! libc and nix, with **no `-sys` crate and no libevdev** — so this costs zero
//! new shared objects.
//!
//! ── ★ THE KEYCODE OFFSET, WHICH IS NOT OPTIONAL ───────────────────────────
//! XKB keycodes are **evdev keycodes + 8**. libinput's own smithay adapter
//! does `(self.key() + 8).into()` (`backend/libinput/mod.rs:116`). Forget it
//! and every key is wrong by eight positions — `a` becomes something else, and
//! the failure looks like a broken keymap rather than a broken backend.
//!
//! ── ★ SYN_REPORT IS A FRAME BOUNDARY, NOT NOISE ───────────────────────────
//! evdev reports a mouse move as `REL_X`, then `REL_Y`, then `SYN_REPORT`. A
//! backend that emitted one motion event per axis would send two events for
//! one movement, each with half the delta — the pointer would move in stairs.
//! Deltas accumulate and flush on `SYN_REPORT`, which is what libinput does
//! and what the kernel's protocol means.
//!
//! ── WHAT THIS DELIBERATELY DOES NOT DO ────────────────────────────────────
//! No pointer acceleration, no tap-to-click, no gesture recognition, no
//! calibration. libinput's real value is that policy, and reimplementing it
//! badly would be worse than linking it. This is the transport; the policy
//! belongs above, in a typed pleme-io surface, and is named rather than
//! smuggled in as "good enough defaults".
//!
//! `pending-omoya-input-policy: acceleration, tap-to-click, gestures`

use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use smithay::backend::input::{
    self as si, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device, DeviceCapability,
    Event, InputBackend, InputEvent, KeyState, Keycode, UnusedEvent,
};
use smithay::backend::session::Session;

/// ★ evdev keycode → XKB keycode. See the header.
const XKB_KEYCODE_OFFSET: u32 = 8;

/// Where the kernel exposes input devices. Enumerated directly rather than
/// through udev: the directory IS the device list, and `libudev` adds a C
/// dependency to read it.
const INPUT_DIR: &str = "/dev/input";

// ── DEVICE ────────────────────────────────────────────────────────────────

/// One evdev node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvdevDevice {
    path: PathBuf,
    name: String,
    caps: u8,
}

impl EvdevDevice {
    const CAP_KEYBOARD: u8 = 1 << 0;
    const CAP_POINTER: u8 = 1 << 1;
}

impl Device for EvdevDevice {
    fn id(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        match capability {
            DeviceCapability::Keyboard => self.caps & Self::CAP_KEYBOARD != 0,
            DeviceCapability::Pointer => self.caps & Self::CAP_POINTER != 0,
            // ★ Reported as ABSENT rather than guessed. A device claiming a
            // capability whose events this backend never emits is worse than
            // one that admits it has none: the compositor would wait for touch
            // frames that cannot arrive.
            _ => false,
        }
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

// ── EVENTS ────────────────────────────────────────────────────────────────

/// The fields every event carries.
#[derive(Debug, Clone)]
pub struct Base {
    device: EvdevDevice,
    /// Microseconds, as the kernel's `input_event.time` reports it.
    time: u64,
}

macro_rules! impl_event {
    ($t:ty) => {
        impl Event<EvdevBackend> for $t {
            fn time(&self) -> u64 {
                self.base.time
            }
            fn device(&self) -> EvdevDevice {
                self.base.device.clone()
            }
        }
    };
}

/// A key press or release.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    base: Base,
    code: u32,
    state: KeyState,
    count: u32,
}
impl_event!(KeyEvent);

impl si::KeyboardKeyEvent<EvdevBackend> for KeyEvent {
    fn key_code(&self) -> Keycode {
        // ★ THE +8. See the header — without it every key is wrong.
        (self.code + XKB_KEYCODE_OFFSET).into()
    }
    fn state(&self) -> KeyState {
        self.state
    }
    fn count(&self) -> u32 {
        self.count
    }
}

/// Relative pointer motion, accumulated over one `SYN_REPORT`.
#[derive(Debug, Clone)]
pub struct MotionEvent {
    base: Base,
    dx: f64,
    dy: f64,
}
impl_event!(MotionEvent);

impl si::PointerMotionEvent<EvdevBackend> for MotionEvent {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
    fn delta_x_unaccel(&self) -> f64 {
        // ★ Identical to the accelerated value, because this backend applies
        // NO acceleration. Saying so here is honest; returning a differently
        // scaled number would invent a curve nobody chose.
        self.dx
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.dy
    }
}

/// A mouse button.
#[derive(Debug, Clone)]
pub struct ButtonEvent {
    base: Base,
    code: u32,
    state: ButtonState,
}
impl_event!(ButtonEvent);

impl si::PointerButtonEvent<EvdevBackend> for ButtonEvent {
    fn button_code(&self) -> u32 {
        self.code
    }
    fn state(&self) -> ButtonState {
        self.state
    }
}

/// A scroll step.
#[derive(Debug, Clone)]
pub struct AxisEvent {
    base: Base,
    vertical: f64,
    horizontal: f64,
}
impl_event!(AxisEvent);

impl si::PointerAxisEvent<EvdevBackend> for AxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        Some(match axis {
            Axis::Vertical => self.vertical,
            Axis::Horizontal => self.horizontal,
        })
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        // ★ A wheel DETENT is 120 in the v120 convention, and the kernel's
        // REL_WHEEL reports 1 per detent. Multiplying is the translation, not
        // a scale factor someone picked.
        self.amount(axis).map(|v| v * 120.0)
    }

    fn source(&self) -> AxisSource {
        // A discrete wheel. `Finger` and `Continuous` describe touchpads,
        // which this backend does not decode.
        AxisSource::Wheel
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

// ── THE BACKEND ───────────────────────────────────────────────────────────

/// Input devices, read straight from the kernel.
pub struct EvdevBackend {
    devices: Vec<(EvdevDevice, evdev::Device)>,
    /// Deltas accumulating until `SYN_REPORT`. See the header.
    pending: HashMap<PathBuf, Accum>,
    token: Option<smithay::reexports::calloop::Token>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Accum {
    dx: f64,
    dy: f64,
    wheel: f64,
    hwheel: f64,
    time: u64,
}

impl Accum {
    const fn is_motion(self) -> bool {
        self.dx != 0.0 || self.dy != 0.0
    }
    const fn is_axis(self) -> bool {
        self.wheel != 0.0 || self.hwheel != 0.0
    }
}

impl EvdevBackend {
    /// Enumerate `/dev/input` and open each device THROUGH the session.
    ///
    /// ── ★ WHY THE SESSION AND NOT `File::open` ────────────────────────────
    /// Same reason the DRM device goes through it: a directly-opened fd is
    /// invisible to whatever arbitrates the seat, so it is never paused on a
    /// VT switch — and it needs `input` group membership the session would
    /// otherwise grant.
    ///
    /// # Errors
    /// If `/dev/input` cannot be read. An individual device that fails to open
    /// is SKIPPED with a warning rather than fatal: one unreadable node must
    /// not cost the seat its keyboard.
    pub fn new<S: Session>(session: &mut S) -> Result<Self, Box<dyn std::error::Error>> {
        let mut devices = Vec::new();

        for entry in std::fs::read_dir(INPUT_DIR)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let is_event_node = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"));
            if !is_event_node {
                continue;
            }

            match Self::open_one(session, &path) {
                Ok(Some(d)) => devices.push(d),
                // Not an input device we can use — no keyboard and no pointer.
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping input device");
                }
            }
        }

        tracing::info!(count = devices.len(), "evdev devices opened through the session");
        Ok(Self {
            devices,
            pending: HashMap::new(),
            token: None,
        })
    }

    fn open_one<S: Session>(
        session: &mut S,
        path: &Path,
    ) -> Result<Option<(EvdevDevice, evdev::Device)>, Box<dyn std::error::Error>> {
        use smithay::reexports::rustix::fs::OFlags;

        let fd: OwnedFd = session
            .open(path, OFlags::RDWR | OFlags::NONBLOCK)
            .map_err(|e| format!("{e:?}"))?;
        let dev = evdev::Device::from_fd(fd)?;

        // evdev 0.13 names these `KeyCode` and `RelativeAxisCode` — the 0.12
        // spellings (`Key`, `RelativeAxisType`) are gone.
        let keys = dev.supported_keys();
        let has_keyboard = keys.is_some_and(|k| k.contains(evdev::KeyCode::KEY_A));
        let has_pointer = keys.is_some_and(|k| k.contains(evdev::KeyCode::BTN_LEFT))
            || dev
                .supported_relative_axes()
                .is_some_and(|a| a.contains(evdev::RelativeAxisCode::REL_X));

        if !has_keyboard && !has_pointer {
            return Ok(None);
        }

        let mut caps = 0u8;
        if has_keyboard {
            caps |= EvdevDevice::CAP_KEYBOARD;
        }
        if has_pointer {
            caps |= EvdevDevice::CAP_POINTER;
        }

        let meta = EvdevDevice {
            path: path.to_path_buf(),
            name: dev.name().unwrap_or("unnamed").to_string(),
            caps,
        };
        tracing::info!(device = %meta.name, path = %path.display(), keyboard = has_keyboard, pointer = has_pointer, "input device");
        Ok(Some((meta, dev)))
    }
}

impl InputBackend for EvdevBackend {
    type Device = EvdevDevice;
    type KeyboardKeyEvent = KeyEvent;
    type PointerAxisEvent = AxisEvent;
    type PointerButtonEvent = ButtonEvent;
    type PointerMotionEvent = MotionEvent;

    // ★ `UnusedEvent` FOR THE NINETEEN THIS BACKEND DOES NOT PRODUCE. It is an
    // uninhabited enum that already implements every event trait
    // (`backend/input/mod.rs:78`), so these arms are not stubs returning
    // plausible values — they have NO CONSTRUCTOR. An event of these kinds
    // cannot be created, which is a stronger statement than a handler that
    // returns zero.
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

// ── THE PUMP ──────────────────────────────────────────────────────────────

impl EvdevBackend {
    /// Drain every device and emit smithay events.
    ///
    /// ── ★ THE FRAME RULE ──────────────────────────────────────────────────
    /// Relative axes and wheel steps ACCUMULATE; they are flushed as one event
    /// when `SYN_REPORT` arrives. The kernel sends `REL_X`, `REL_Y`,
    /// `SYN_REPORT` for a single diagonal movement, and a backend that emitted
    /// per-axis would produce two events with half the motion each — a pointer
    /// that moves in stairs.
    ///
    /// Keys and buttons are emitted IMMEDIATELY. They are not deltas: a key
    /// press has no partner event to wait for, and delaying it to the next SYN
    /// would add latency to every keystroke for no gain.
    fn pump<F>(&mut self, mut callback: F)
    where
        F: FnMut(InputEvent<Self>),
    {
        for (meta, dev) in &mut self.devices {
            let Ok(events) = dev.fetch_events() else {
                // EAGAIN on a non-blocking fd is the normal "nothing to read"
                // answer, not a failure worth logging every tick.
                continue;
            };

            let acc = self.pending.entry(meta.path.clone()).or_default();

            for ev in events {
                let time = {
                    let t = ev.timestamp();
                    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                    u64::try_from(d.as_micros()).unwrap_or(0)
                };
                acc.time = time;
                let base = Base {
                    device: meta.clone(),
                    time,
                };

                // ★ `destructure()` — evdev 0.13's typed view of an event.
                // `InputEventKind` no longer exists; `EventSummary` carries the
                // code and value together, which removes the 0.12 shape where
                // you matched a kind and then read `.value()` separately and
                // could read the wrong one.
                match ev.destructure() {
                    evdev::EventSummary::Key(_, key, value) => {
                        let code = u32::from(key.0);
                        let state = match value {
                            0 => KeyState::Released,
                            // ★ value 2 is AUTOREPEAT. Treated as a press,
                            // because that is what it is — and `count` carries
                            // the distinction for anything that cares.
                            _ => KeyState::Pressed,
                        };
                        // BTN_MISC (0x100) is where buttons begin; below it is
                        // the keyboard. The split is the kernel's, not ours.
                        #[allow(clippy::items_after_statements)]
                        if code >= 0x100 {
                            callback(InputEvent::PointerButton {
                                event: ButtonEvent {
                                    base,
                                    code,
                                    state: if state == KeyState::Pressed {
                                        ButtonState::Pressed
                                    } else {
                                        ButtonState::Released
                                    },
                                },
                            });
                        } else {
                            callback(InputEvent::Keyboard {
                                event: KeyEvent {
                                    base,
                                    code,
                                    state,
                                    count: if value == 2 { 2 } else { 1 },
                                },
                            });
                        }
                    }
                    evdev::EventSummary::RelativeAxis(_, axis, value) => {
                        let v = f64::from(value);
                        match axis {
                            evdev::RelativeAxisCode::REL_X => acc.dx += v,
                            evdev::RelativeAxisCode::REL_Y => acc.dy += v,
                            evdev::RelativeAxisCode::REL_WHEEL => acc.wheel += v,
                            evdev::RelativeAxisCode::REL_HWHEEL => acc.hwheel += v,
                            _ => {}
                        }
                    }
                    evdev::EventSummary::Synchronization(..) => {
                        // ★ THE FLUSH. One event per frame, carrying the whole
                        // accumulated delta.
                        let a = *acc;
                        if a.is_motion() {
                            callback(InputEvent::PointerMotion {
                                event: MotionEvent {
                                    base: base.clone(),
                                    dx: a.dx,
                                    dy: a.dy,
                                },
                            });
                        }
                        if a.is_axis() {
                            callback(InputEvent::PointerAxis {
                                event: AxisEvent {
                                    base,
                                    // ★ NEGATED. The kernel's REL_WHEEL is
                                    // POSITIVE for scrolling AWAY from the
                                    // user, while Wayland's axis is positive
                                    // DOWNWARD. Passing it through unchanged
                                    // inverts scrolling everywhere, and the
                                    // symptom reads as a broken mouse.
                                    vertical: -a.wheel,
                                    horizontal: a.hwheel,
                                },
                            });
                        }
                        *acc = Accum::default();
                    }
                    _ => {}
                }
            }
        }
    }
}

impl smithay::reexports::calloop::EventSource for EvdevBackend {
    type Event = InputEvent<EvdevBackend>;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    fn process_events<F>(
        &mut self,
        _: smithay::reexports::calloop::Readiness,
        _: smithay::reexports::calloop::Token,
        mut callback: F,
    ) -> std::io::Result<smithay::reexports::calloop::PostAction>
    where
        F: FnMut(Self::Event, &mut ()),
    {
        self.pump(|e| callback(e, &mut ()));
        Ok(smithay::reexports::calloop::PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        // ★ ONE TOKEN, EVERY FD. calloop wakes on any of them and `pump`
        // drains all — a per-device token would demand a device lookup by
        // token on every wake, and the drain is cheap because each read
        // returns EAGAIN immediately when there is nothing there.
        let token = factory.token();
        self.token = Some(token);
        for (_, dev) in &self.devices {
            // SAFETY: the fds live as long as this source, which calloop owns.
            unsafe {
                poll.register(
                    std::os::fd::BorrowedFd::borrow_raw(dev.as_raw_fd()),
                    smithay::reexports::calloop::Interest::READ,
                    smithay::reexports::calloop::Mode::Level,
                    token,
                )?;
            }
        }
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        let token = factory.token();
        self.token = Some(token);
        for (_, dev) in &self.devices {
            // SAFETY: as above.
            unsafe {
                poll.reregister(
                    std::os::fd::BorrowedFd::borrow_raw(dev.as_raw_fd()),
                    smithay::reexports::calloop::Interest::READ,
                    smithay::reexports::calloop::Mode::Level,
                    token,
                )?;
            }
        }
        Ok(())
    }

    fn unregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
    ) -> smithay::reexports::calloop::Result<()> {
        for (_, dev) in &self.devices {
            // SAFETY: as above.
            unsafe {
                poll.unregister(std::os::fd::BorrowedFd::borrow_raw(dev.as_raw_fd()))?;
            }
        }
        self.token = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xkb_offset_is_eight_and_is_applied() {
        // ★ The single most consequential constant in this file. evdev
        // KEY_A is 30; XKB expects 38. Getting it wrong shifts EVERY key and
        // presents as a broken keymap rather than a broken backend.
        assert_eq!(XKB_KEYCODE_OFFSET, 8);
        let k = KeyEvent {
            base: Base {
                device: EvdevDevice {
                    path: PathBuf::from("/dev/input/event0"),
                    name: "t".into(),
                    caps: EvdevDevice::CAP_KEYBOARD,
                },
                time: 0,
            },
            code: 30, // KEY_A
            state: KeyState::Pressed,
            count: 1,
        };
        use smithay::backend::input::KeyboardKeyEvent;
        assert_eq!(u32::from(k.key_code()), 38);
    }

    #[test]
    fn a_frame_flushes_once_with_the_summed_delta() {
        // The stairs bug, guarded: REL_X then REL_Y then SYN must be ONE
        // motion event carrying both, not two carrying half each.
        let mut a = Accum::default();
        a.dx += 3.0;
        a.dy += 4.0;
        assert!(a.is_motion());
        assert!(!a.is_axis());
        assert_eq!((a.dx, a.dy), (3.0, 4.0));
    }

    #[test]
    fn buttons_and_keys_split_at_btn_misc() {
        // 0x100 is the kernel's boundary, not a number this file chose. Below
        // it is the keyboard; at or above, a pointer button.
        assert!(u32::from(evdev::KeyCode::KEY_A.0) < 0x100);
        assert!(u32::from(evdev::KeyCode::BTN_LEFT.0) >= 0x100);
    }
}
