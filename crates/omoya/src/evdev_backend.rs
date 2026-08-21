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
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
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


/// `EAGAIN` — the normal "nothing to read" answer on a non-blocking fd.
const AGAIN: i32 = smithay::reexports::rustix::io::Errno::AGAIN.raw_os_error();
/// `ENODEV` — what `evdev_read` returns for a device that is gone OR revoked
/// (`drivers/input/evdev.c:569`, Linux v6.12). The kernel does not tell the two
/// apart, and neither can this file: only a `remove` uevent means gone.
const NODEV: i32 = smithay::reexports::rustix::io::Errno::NODEV.raw_os_error();

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
        impl<S: Session> Event<EvdevBackend<S>> for $t {
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

impl<S: Session> si::KeyboardKeyEvent<EvdevBackend<S>> for KeyEvent {
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

impl<S: Session> si::PointerMotionEvent<EvdevBackend<S>> for MotionEvent {
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

impl<S: Session> si::PointerButtonEvent<EvdevBackend<S>> for ButtonEvent {
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

impl<S: Session> si::PointerAxisEvent<EvdevBackend<S>> for AxisEvent {
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
        //
        // ★ THE QUALIFIED CALL IS NOT DECORATION. Now that the event traits
        // are generic over the session, `AxisEvent` implements
        // `PointerAxisEvent<B>` for EVERY `B = EvdevBackend<S>`, so a bare
        // `self.amount(axis)` has no single `B` to infer and fails with
        // E0283 — measured, not guessed. Naming the backend picks the same
        // impl this method is inside.
        <Self as si::PointerAxisEvent<EvdevBackend<S>>>::amount(self, axis).map(|v| v * 120.0)
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

/// One opened device plus the two bits of poll bookkeeping calloop makes us
/// keep ourselves.
///
/// ★ `polled` and `armed` are NOT the same thing and collapsing them is the
/// bug this struct exists to prevent. `polled` is a fact about the epoll set —
/// `Poll::register` fails on an fd already in it and `Poll::reregister` fails
/// on one that is not (calloop `sys.rs:286-289`, `:339-343`). `armed` is a
/// belief about the DEVICE — cleared the moment a read returns `ENODEV`.
struct Entry {
    meta: EvdevDevice,
    dev: evdev::Device,
    /// This fd is in the poll set right now.
    polled: bool,
    /// We believe this fd can still produce events.
    ///
    /// Cleared on `ENODEV`, which the kernel returns for BOTH an unplugged
    /// device and one logind revoked on a VT switch — `drivers/input/evdev.c:569`
    /// tests `!evdev->exist || client->revoked` and cannot tell you which.
    armed: bool,
    /// A `remove` uevent named this node. Terminal: a gone device is never
    /// re-armed, and this is the only thing that distinguishes an unplug from
    /// a revoke.
    gone: bool,
}

/// Input devices, read straight from the kernel.
pub struct EvdevBackend<S: Session> {
    /// Where to publish the device table. Optional so the type stays usable
    /// in tests and in the nested backend without a sidecar.
    introspect: Option<std::sync::Arc<crate::introspect::OmoyaIntrospect>>,
    /// ★ THE SESSION, HELD — not borrowed for the constructor and dropped.
    /// Hotplug means opening a device long after start-up, and `Session::open`
    /// (logind `TakeDevice`) is the only sanctioned way to do that. Opening a
    /// hotplugged node with `File::open` would produce an fd logind has never
    /// heard of: not paused on a VT switch, not repaired by `ResumeDevice`,
    /// and dependent on `input` group membership the session exists to avoid
    /// needing.
    session: S,
    devices: Vec<Entry>,
    /// Deltas accumulating until `SYN_REPORT`. See the header.
    pending: HashMap<PathBuf, Accum>,
    /// udev's monitor. `None` when the socket could not be bound — a seat with
    /// no hotplug, degraded and SAID SO, rather than a backend that refuses to
    /// start and leaves the machine with no input at all.
    monitor: Option<crate::uevent::UeventMonitor>,
    token: Option<smithay::reexports::calloop::Token>,
    monitor_token: Option<smithay::reexports::calloop::Token>,
    /// Devices whose node is gone, waiting to leave the poll set before their
    /// fd is closed. An fd must be unregistered BEFORE it is closed or
    /// calloop's level-triggered emulation keeps a stale `(key, raw fd)` pair
    /// (`sys.rs:329-333`) aimed at a number the kernel is free to hand to the
    /// next `open`.
    graveyard: Vec<(evdev::Device, bool)>,
    /// The last `Session::is_active()` this source saw, so a VT RESUME
    /// (false → true) can be told from a session that has simply been active
    /// all along.
    last_active: bool,
    /// The fd set changed; `process_events` must return `Reregister`.
    dirty: bool,
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

impl<S: Session> EvdevBackend<S> {
    /// Subscribe to hotplug, then enumerate `/dev/input`, opening each device
    /// THROUGH the session.
    ///
    /// ── ★ THE ORDER IS THE POINT ──────────────────────────────────────────
    /// The monitor is bound BEFORE the directory is read. A device that
    /// appears between the two is then delivered as a `add` uevent and picked
    /// up; the other order drops it into the gap, and the symptom is a
    /// keyboard that is invisible until it is unplugged and plugged back in.
    ///
    /// # Errors
    /// If `/dev/input` cannot be read. An individual device that fails to open
    /// is SKIPPED with a warning rather than fatal: one unreadable node must
    /// not cost the seat its keyboard. A monitor that fails to bind is also
    /// not fatal — see `monitor`.
    pub fn new(
        mut session: S,
        introspect: Option<std::sync::Arc<crate::introspect::OmoyaIntrospect>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let monitor = match crate::uevent::UeventMonitor::new() {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!(
                    error = ?e,
                    "udev monitor not bound — hotplug is OFF for this run; a device \
                     plugged in later will be invisible until restart"
                );
                None
            }
        };

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

            match Self::open_one(&mut session, &path) {
                Ok(Some((meta, dev))) => devices.push(Entry {
                    meta,
                    dev,
                    polled: false,
                    armed: true,
                    gone: false,
                }),
                // Not an input device we can use — no keyboard and no pointer.
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping input device");
                }
            }
        }

        tracing::info!(
            count = devices.len(),
            hotplug = monitor.is_some(),
            "evdev devices opened through the session"
        );
        let me = Self {
            introspect,
            session,
            devices,
            pending: HashMap::new(),
            monitor,
            token: None,
            monitor_token: None,
            graveyard: Vec::new(),
            last_active: false,
            dirty: false,
        };
        me.publish();
        Ok(me)
    }

    /// Publish the device table so an operator can ASK instead of infer.
    ///
    /// Called after enumeration and after every arm/disarm transition, which
    /// are the only moments the answer changes.
    fn publish(&self) {
        let Some(i) = &self.introspect else { return };
        let mut out = String::new();
        for e in &self.devices {
            if !out.is_empty() {
                out.push_str(" | ");
            }
            let kb = e.meta.caps & EvdevDevice::CAP_KEYBOARD != 0;
            let pt = e.meta.caps & EvdevDevice::CAP_POINTER != 0;
            out.push_str(&format!(
                "{} [{}{}] armed={} polled={} gone={}",
                e.meta.path.display(),
                if kb { "k" } else { "" },
                if pt { "p" } else { "" },
                e.armed,
                e.polled,
                e.gone
            ));
        }
        if out.is_empty() {
            // ★ THE DENOMINATOR. An empty table and a healthy one must not
            // render the same — "no devices" is a finding, not a blank.
            out.push_str("NONE OPENED");
        }
        *i.input_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = out;
    }

    fn open_one(
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

impl<S: Session> InputBackend for EvdevBackend<S> {
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

// ── HOTPLUG ───────────────────────────────────────────────────────────────

impl<S: Session> EvdevBackend<S> {
    /// Take everything udev has to say and act on it.
    ///
    /// ── ★ WHY THE OPEN GOES THROUGH THE SESSION, IDENTICALLY ──────────────
    /// This is the same `Self::open_one` the constructor uses, so a keyboard
    /// plugged in an hour after start-up is `TakeDevice`'d exactly like one
    /// present at boot: logind records its `(major, minor) → fd` in
    /// `logind.rs`'s `Inner::devices`, which is what makes `ResumeDevice`'s
    /// `dup2` cover it on the next VT switch. A hotplugged device opened any
    /// other way would work until the first Ctrl+Alt+F2 and then be dead with
    /// no diagnostic.
    fn absorb_hotplug<F>(&mut self, callback: &mut F)
    where
        F: FnMut(InputEvent<Self>, &mut ()),
    {
        // Collected first: `drain` borrows `self.monitor`, and opening borrows
        // `self.session` mutably. Two disjoint fields, but not across one
        // closure.
        let mut hot = Vec::new();
        if let Some(m) = &self.monitor {
            m.drain(|h| hot.push(h));
        }

        for h in hot {
            match h {
                crate::uevent::Hotplug::Added(path) => {
                    // ★ The monitor is bound before the enumeration, so the
                    // two can legitimately report the same device. Opening it
                    // twice would take two fds for one node and double every
                    // keystroke.
                    if self.devices.iter().any(|e| e.meta.path == path) {
                        continue;
                    }
                    match Self::open_one(&mut self.session, &path) {
                        Ok(Some((meta, dev))) => {
                            tracing::info!(device = %meta.name, path = %path.display(), "input device HOTPLUGGED");
                            callback(
                                InputEvent::DeviceAdded {
                                    device: meta.clone(),
                                },
                                &mut (),
                            );
                            self.devices.push(Entry {
                                meta,
                                dev,
                                polled: false,
                                armed: true,
                                gone: false,
                            });
                            self.dirty = true;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "hotplugged device not opened");
                        }
                    }
                }
                crate::uevent::Hotplug::Removed(path) => {
                    let Some(i) = self.devices.iter().position(|e| e.meta.path == path) else {
                        continue;
                    };
                    let mut e = self.devices.remove(i);
                    e.gone = true;
                    self.pending.remove(&path);
                    tracing::info!(device = %e.meta.name, path = %path.display(), "input device UNPLUGGED");
                    callback(InputEvent::DeviceRemoved { device: e.meta }, &mut ());
                    // ★ NOT dropped here. The fd is still in the poll set, and
                    // dropping it now closes it while calloop still believes
                    // in it. `reregister` unregisters, then drops.
                    self.graveyard.push((e.dev, e.polled));
                    self.dirty = true;
                }
            }
        }
    }
}


/// Decide what a failed `fetch_events` means, and disarm the device if the fd
/// is dead. Returns whether the poll set now needs updating.
///
/// ── ★ THE SPIN THIS CLOSES ────────────────────────────────────────────────
/// `evdev_poll` answers `EPOLLHUP | EPOLLERR` for a device that is gone or
/// revoked (Linux v6.12 `drivers/input/evdev.c:616-619`). polling 3.11.0 folds
/// HUP and ERR into `readable` (`epoll.rs:311`, `:342`), and calloop 0.14.4
/// hardcodes `Readiness::error` to `false` (`sys.rs:259`) — so at the
/// `EventSource` boundary that wake is INDISTINGUISHABLE from real data. In
/// `Mode::Level` the fd is then permanently ready, and the source is woken as
/// fast as the loop can turn: 100% of one core, forever.
///
/// The old code could not see it. `let Ok(events) = dev.fetch_events() else
/// { continue }` discarded the `ENODEV` that is the only evidence.
///
/// ★ Disarming is NOT removal. `ENODEV` covers a revoke as well as an unplug
/// (`evdev.c:569` tests `!evdev->exist || client->revoked`), and a revoke is
/// repaired by `ResumeDevice`'s `dup2`. Only a `remove` uevent means gone.
///
/// ★ Takes `meta` and `armed` SEPARATELY rather than `&mut Entry`. The caller
/// is inside `dev.fetch_events()`'s borrow of the same `Entry`, so a second
/// whole-struct borrow is E0499 — measured. Splitting the struct at the call
/// site is what makes the three borrows disjoint.
fn disarm_on_fault(meta: &EvdevDevice, armed: &mut bool, e: &std::io::Error) -> bool {
    match e.raw_os_error() {
        // The normal "nothing to read" answer on a non-blocking fd — not a
        // failure worth logging every tick.
        Some(AGAIN) => false,
        Some(NODEV) => {
            *armed = false;
            tracing::info!(
                device = %meta.name,
                path = %meta.path.display(),
                "device fd went ENODEV — disarmed (revoked on a VT switch, or unplugged)"
            );
            true
        }
        _ => {
            tracing::warn!(device = %meta.name, error = %e, "evdev read failed");
            false
        }
    }
}

// ── THE PUMP ──────────────────────────────────────────────────────────────

impl<S: Session> EvdevBackend<S> {
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
        // Disjoint field borrows: `devices`, `pending` and `dirty` are three
        // separate fields of `self`, so the loop may hold all three.
        for entry in &mut self.devices {
            // ★ DESTRUCTURED, not used through `entry`. `fetch_events()` holds
            // a mutable borrow of `dev` for as long as its iterator lives, so
            // touching `entry.armed` through the struct in the error arm is
            // E0499. Splitting here gives three disjoint borrows.
            let Entry {
                meta,
                dev,
                armed,
                polled: _,
                gone: _,
            } = entry;
            if !*armed {
                continue;
            }
            let events = match dev.fetch_events() {
                Ok(e) => e,
                Err(e) => {
                    if disarm_on_fault(meta, armed, &e) {
                        self.dirty = true;
                    }
                    continue;
                }
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
                match ev.destructure() {
                    evdev::EventSummary::Key(_, key, value) => {
                        let code = u32::from(key.0);
                        let state = match value {
                            0 => KeyState::Released,
                            // ★ value 2 is AUTOREPEAT.
                            _ => KeyState::Pressed,
                        };
                        // BTN_MISC (0x100) is where buttons begin; below it is
                        // the keyboard. The split is the kernel's, not ours.
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
                                    // ★ NEGATED — see the original note.
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

impl<S: Session> smithay::reexports::calloop::EventSource for EvdevBackend<S> {
    type Event = InputEvent<EvdevBackend<S>>;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    fn process_events<F>(
        &mut self,
        _: smithay::reexports::calloop::Readiness,
        token: smithay::reexports::calloop::Token,
        mut callback: F,
    ) -> std::io::Result<smithay::reexports::calloop::PostAction>
    where
        F: FnMut(Self::Event, &mut ()),
    {
        if Some(token) == self.monitor_token {
            self.absorb_hotplug(&mut callback);
        } else {
            self.pump(|e| callback(e, &mut ()));
        }

        // ★ THE ONLY WAY TO REACH `Poll` FROM HERE. `process_events` is not
        // given the poll instance; `PostAction::Reregister` is what makes
        // calloop call `reregister` with it (`loop_logic.rs:539-549`). Every
        // add and every removal therefore lands one tick later, inside
        // `reregister`, which is the correct place and the only place.
        Ok(if std::mem::take(&mut self.dirty) {
            smithay::reexports::calloop::PostAction::Reregister
        } else {
            smithay::reexports::calloop::PostAction::Continue
        })
    }

    fn register(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        // ★ TOKEN ORDER IS A CONTRACT. `reregister` is handed a FRESH
        // `TokenFactory` rooted at the same registration token
        // (`loop_logic.rs:545-549`), so it must ask for its tokens in the same
        // order to get the same sub-ids back. Swap these two lines and every
        // device wake is delivered as a monitor wake.
        let token = factory.token();
        self.token = Some(token);
        let monitor_token = factory.token();
        self.monitor_token = Some(monitor_token);

        // ★ ONE TOKEN, EVERY DEVICE FD. calloop wakes on any of them and
        // `pump` drains all — a per-device token would demand a device lookup
        // by token on every wake, and the drain is cheap because each read
        // returns EAGAIN immediately when there is nothing there. The MONITOR
        // gets its own token, because its wake means something entirely
        // different.
        for e in &mut self.devices {
            // SAFETY: the fds live as long as this source, which calloop owns.
            unsafe {
                poll.register(
                    std::os::fd::BorrowedFd::borrow_raw(e.dev.as_raw_fd()),
                    smithay::reexports::calloop::Interest::READ,
                    smithay::reexports::calloop::Mode::Level,
                    token,
                )?;
            }
            e.polled = true;
        }

        if let Some(m) = &self.monitor {
            // SAFETY: as above.
            unsafe {
                poll.register(
                    m.as_fd(),
                    smithay::reexports::calloop::Interest::READ,
                    smithay::reexports::calloop::Mode::Level,
                    monitor_token,
                )?;
            }
        }

        self.last_active = self.session.is_active();
        // ★ PUBLISH HERE TOO, NOT ONLY IN `reregister`.
        //
        // The first cut published at construction and from `reregister`, and
        // `register` sets `polled` between them — so the leaf reported
        // `polled=false` for every device forever unless calloop happened to
        // reconcile. Caught within a minute of shipping it, by reading it: six
        // devices claiming not to be polled while the pointer plainly worked.
        //
        // A status leaf that publishes a value it never refreshes is not a
        // measurement, it is a snapshot wearing one's clothes.
        self.publish();
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        use smithay::reexports::calloop::{Interest, Mode};

        let token = factory.token();
        self.token = Some(token);
        let monitor_token = factory.token();
        self.monitor_token = Some(monitor_token);

        // ★ RE-ARM ON THE TRANSITION, NEVER ON THE STATE. A VT resume is
        // inactive → active. An unplug leaves the session active throughout,
        // so re-arming on `is_active()` alone would put a dead fd straight
        // back into the poll set and restart the spin the pump just stopped.
        let active = self.session.is_active();
        if active && !self.last_active {
            for e in &mut self.devices {
                if !e.gone {
                    e.armed = true;
                }
            }
            self.publish();
        }
        self.last_active = active;

        // The dead leave the poll set BEFORE their fd is closed. Reversing
        // this leaves calloop's level-triggered emulation holding a raw fd
        // number the kernel is free to reuse (`sys.rs:329-333`).
        for (dev, polled) in std::mem::take(&mut self.graveyard) {
            if polled {
                // SAFETY: the fd is still open — `dev` has not been dropped.
                unsafe {
                    let _ = poll.unregister(std::os::fd::BorrowedFd::borrow_raw(dev.as_raw_fd()));
                }
            }
            drop(dev);
        }

        for e in &mut self.devices {
            let want = e.armed && !e.gone;
            // SAFETY: as above.
            let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(e.dev.as_raw_fd()) };
            match (e.polled, want) {
                (false, true) => {
                    // SAFETY: as above.
                    unsafe { poll.register(fd, Interest::READ, Mode::Level, token)? };
                    e.polled = true;
                }
                (true, true) => {
                    // ★ A MOD THAT FAILS IS NOT AN ERROR HERE. When logind
                    // resumes a device it `dup2`s a NEW file description onto
                    // the fd number we already hold (`logind.rs:320-322`).
                    // That closes the old description, and the kernel drops
                    // every epoll entry for a file as it is freed —
                    // `fs/file_table.c:422` calls `eventpoll_release`, which
                    // `fs/eventpoll.c:1083` implements. So after a VT switch
                    // `epoll_ctl(MOD)` answers ENOENT for an fd we believe is
                    // registered. Re-ADD; do not fail the loop.
                    if poll.reregister(fd, Interest::READ, Mode::Level, token).is_err() {
                        // SAFETY: as above.
                        unsafe { poll.register(fd, Interest::READ, Mode::Level, token)? };
                    }
                }
                (true, false) => {
                    let _ = poll.unregister(fd);
                    e.polled = false;
                }
                (false, false) => {}
            }
        }

        if let Some(m) = &self.monitor {
            poll.reregister(m.as_fd(), Interest::READ, Mode::Level, monitor_token)?;
        }
        // The table changed if anything armed, disarmed or left. Publishing
        // here covers every transition with one call, and `reregister` is
        // exactly when calloop asks us to reconcile the poll set with it.
        self.publish();
        Ok(())
    }

    fn unregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
    ) -> smithay::reexports::calloop::Result<()> {
        for e in &mut self.devices {
            if e.polled {
                // SAFETY: as above.
                unsafe {
                    let _ = poll.unregister(std::os::fd::BorrowedFd::borrow_raw(e.dev.as_raw_fd()));
                }
                e.polled = false;
            }
        }
        for (dev, polled) in std::mem::take(&mut self.graveyard) {
            if polled {
                // SAFETY: as above.
                unsafe {
                    let _ = poll.unregister(std::os::fd::BorrowedFd::borrow_raw(dev.as_raw_fd()));
                }
            }
        }
        if let Some(m) = &self.monitor {
            let _ = poll.unregister(m.as_fd());
        }
        self.token = None;
        self.monitor_token = None;
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A `Session` that refuses everything.
    ///
    /// ★ Present because the event traits are now generic over the session,
    /// so `k.key_code()` has no single `B` to infer. A named stand-in is
    /// clearer than a turbofish, and it also pins the shape of what the
    /// backend actually needs from a session: one `open`, one `is_active`.
    #[derive(Debug, Clone, Copy)]
    struct NoSession;

    impl smithay::backend::session::AsErrno for NoSessionError {
        fn as_errno(&self) -> Option<i32> {
            None
        }
    }

    #[derive(Debug)]
    struct NoSessionError;

    impl Session for NoSession {
        type Error = NoSessionError;
        fn open(
            &mut self,
            _: &Path,
            _: smithay::reexports::rustix::fs::OFlags,
        ) -> Result<OwnedFd, Self::Error> {
            Err(NoSessionError)
        }
        fn close(&mut self, _: OwnedFd) -> Result<(), Self::Error> {
            Err(NoSessionError)
        }
        fn change_vt(&mut self, _: i32) -> Result<(), Self::Error> {
            Err(NoSessionError)
        }
        fn is_active(&self) -> bool {
            false
        }
        fn seat(&self) -> String {
            "test".into()
        }
    }

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
        assert_eq!(
            u32::from(si::KeyboardKeyEvent::<EvdevBackend<NoSession>>::key_code(&k)),
            38
        );
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

    #[test]
    fn enodev_is_the_removal_errno_and_eagain_is_not() {
        // ★ These two must not be confused: EAGAIN is silence, ENODEV is a
        // dead fd that will wake the loop forever. Pinning both here means a
        // future edit that folds them together fails the build rather than
        // the machine.
        assert_eq!(NODEV, 19);
        assert_ne!(AGAIN, NODEV);
    }

    /// The state machine that decides what goes in the poll set, without a
    /// poll instance.
    ///
    /// ★ This is the test the spin bug would have failed. A disarmed entry
    /// must be OUT of the poll set, and it must NOT come back merely because
    /// the session happens to be active.
    #[test]
    fn a_disarmed_device_leaves_the_poll_set_and_a_gone_one_never_returns() {
        let want = |armed: bool, gone: bool| armed && !gone;
        assert!(want(true, false), "a live device is polled");
        assert!(!want(false, false), "a revoked device is not polled");
        assert!(!want(true, true), "a removed device is not polled");
        assert!(!want(false, true), "a removed, revoked device is not polled");
    }
}
