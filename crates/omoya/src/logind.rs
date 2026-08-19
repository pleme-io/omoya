//! `smithay::backend::session::Session` over logind's D-Bus, in pure Rust.
//!
//! ── ★ WHY THIS REPLACES libseat, AND WHY IT IS NOT A "REBUILD" ────────────
//! `libseat.so.1` is one of six C libraries this compositor links. It is a
//! **client** of one of two protocols: seatd's socket, or logind's D-Bus. A
//! protocol is a WIRE, and the fleet's posture for a wire is *speak it, own the
//! executor* — magma's stance with the Terraform provider protocol — rather
//! than rebuild the thing on the far side. logind stays; only the C client goes.
//!
//! zbus implements D-Bus itself rather than binding libdbus, so this adds no
//! `.so`. That is the whole point.
//!
//! ── ★ THE WIRE, MEASURED OFF A LIVE BUS (2026-08-19) ──────────────────────
//! Not read from documentation. `busctl introspect org.freedesktop.login1`:
//!
//! ```text
//! TakeControl         method  b     -> -      (force: bool)
//! ReleaseControl      method  -     -> -
//! TakeDevice          method  uu    -> hb     (major,minor) -> (fd, inactive)
//! ReleaseDevice       method  uu    -> -
//! PauseDeviceComplete method  uu    -> -
//! Active              property b            emits-change
//! Seat                property (so)         a STRUCT, not a string
//! PauseDevice         signal  uus            (major,minor,type)
//! ResumeDevice        signal  uuh            (major,minor,NEW fd)
//! ```
//!
//! Four facts in there are traps, and each is handled below:
//!
//! 1. **`TakeDevice` returns `hb`, not `h`.** The bool is `inactive` — the
//!    device was handed over while the session is in the background. Reading
//!    the reply as a bare fd is a deserialisation error at runtime, not a
//!    compile error.
//! 2. **`ResumeDevice` carries a NEW fd.** The caller (libinput, drm) is still
//!    holding the OLD one and has no idea it changed — smithay's `Session` API
//!    hands an fd out once and never revises it. So the new descriptor must be
//!    `dup2`'d ONTO the old number, which is the only way an fd already given
//!    away can be made to point somewhere else.
//! 3. **`PauseDevice` must be acknowledged** with `PauseDeviceComplete`, or
//!    logind waits and **the VT switch hangs**. Nothing in the signature says
//!    so; a client that only listens looks correct and freezes the machine.
//! 4. **`Seat` is `(so)`** — (name, object path). smithay's `Session::seat()`
//!    returns a `String`, so it is the `.0`. Deserialising it as `String`
//!    fails.
//!
//! ── ★ WHY Arc/Mutex WHERE smithay's libseat BACKEND USES Rc/RefCell ───────
//! libseat exposes a pollable fd, so smithay drives it from calloop on the
//! compositor thread and single-threaded cells suffice. D-Bus signals have no
//! equivalent here: they arrive on a blocking `MessageIterator`, which needs a
//! thread. The state that thread touches — the device map it `dup2`s into — is
//! therefore shared, and shared means `Arc<Mutex<..>>`. This is a real
//! divergence from the template, made for a real reason, not a style choice.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::backend::session::{AsErrno, Event as SessionEvent, Session};
use smithay::reexports::calloop::channel::{Channel, SyncSender, sync_channel};
use smithay::reexports::rustix::fs::OFlags;

const DEST: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";
const SEAT_IFACE: &str = "org.freedesktop.login1.Seat";

/// What can go wrong reaching logind.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the system bus is unreachable: {0}")]
    Bus(String),
    /// ★ A REACHABLE STATE, not a misconfiguration. `GetSessionByPID` fails
    /// with "does not belong to any known session" for any process outside
    /// one — which is exactly what happens when a developer runs the
    /// compositor over ssh to test it. Measured on rio.
    #[error("this process is not in a logind session: {0}")]
    NoSession(String),
    #[error("logind refused {method}: {reason}")]
    Refused { method: &'static str, reason: String },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// ★ A device path that cannot be stat'd. Its own arm rather than folded
    /// into `Io`, because logind addresses devices by (major, minor) and the
    /// stat is how a path becomes those — a failure here means we never got as
    /// far as asking logind anything.
    #[error("cannot stat the device node: {0}")]
    Stat(smithay::reexports::rustix::io::Errno),
}

impl From<smithay::reexports::rustix::io::Errno> for Error {
    fn from(e: smithay::reexports::rustix::io::Errno) -> Self {
        Self::Stat(e)
    }
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        match self {
            // ★ A stat failure HAS a real errno, so pass it through — libinput
            // can distinguish ENOENT from EACCES and recover differently.
            Self::Stat(e) => Some(e.raw_os_error()),
            // D-Bus errors carry NAMES, not errnos, so there is nothing honest
            // to return. smithay's own call site defaults to EPERM on `None`
            // (`backend/libinput/mod.rs:692`), which is the correct floor: it
            // makes libinput treat the device as unavailable rather than
            // inventing an errno that sends a caller down the wrong recovery
            // path.
            _ => None,
        }
    }
}

/// The shared half — touched by both the compositor thread and the signal
/// thread.
struct Inner {
    conn: zbus::blocking::Connection,
    session_path: zvariant::OwnedObjectPath,
    seat_path: zvariant::OwnedObjectPath,
    seat_name: String,
    active: AtomicBool,
    /// (major, minor) → the raw fd number handed to the caller.
    ///
    /// ★ Recorded so `ResumeDevice` can `dup2` onto it. The hazard is inherent
    /// to the protocol rather than to this code: if a caller closes the fd
    /// WITHOUT going through `Session::close`, the number can be reused by an
    /// unrelated open and a later resume would `dup2` over it. `close` removes
    /// the entry, so the map stays true for callers that use the API as
    /// smithay defines it — which libinput and drm both do.
    devices: Mutex<HashMap<(u32, u32), RawFd>>,
}

impl Inner {
    fn call<B>(&self, iface: &str, path: &zvariant::OwnedObjectPath, method: &'static str, body: &B)
        -> Result<zbus::Message, Error>
    where
        B: serde::Serialize + zvariant::DynamicType,
    {
        self.conn
            .call_method(Some(DEST), path.as_str(), Some(iface), method, body)
            .map_err(|e| Error::Refused { method, reason: e.to_string() })
    }
}

/// The compositor-facing half. Cheap to clone; smithay hands it to libinput.
#[derive(Clone)]
pub struct LogindSession {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LogindSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogindSession")
            .field("seat", &self.inner.seat_name)
            .field("active", &self.inner.active.load(Ordering::Relaxed))
            .finish()
    }
}

/// The event source. **Insert this into the event loop** — see `main.rs`, where
/// dropping the equivalent libseat value silently killed the session.
pub struct LogindSessionNotifier {
    rx: Channel<SessionEvent>,
    /// Held so the signal thread's `Arc` is not the last one, and so the
    /// session outlives every `open`.
    _inner: Arc<Inner>,
}

impl LogindSession {
    /// Take control of this process's logind session.
    ///
    /// # Errors
    /// [`Error::NoSession`] when the process is not in a session — the ssh case
    /// above; [`Error::Bus`] when the system bus is unreachable.
    pub fn new() -> Result<(Self, LogindSessionNotifier), Error> {
        let conn = zbus::blocking::Connection::system().map_err(|e| Error::Bus(e.to_string()))?;

        // ★ GetSessionByPID, not "auto". The `auto` path resolves relative to
        // the CALLER's session and is empty for a process that has none, which
        // fails later and more confusingly than asking directly.
        let pid = std::process::id();
        let reply = conn
            .call_method(Some(DEST), MANAGER_PATH, Some(MANAGER_IFACE), "GetSessionByPID", &(pid,))
            .map_err(|e| Error::NoSession(e.to_string()))?;
        let session_path: zvariant::OwnedObjectPath = reply
            .body()
            .deserialize()
            .map_err(|e| Error::NoSession(e.to_string()))?;

        // `Seat` is `(so)` — see the header. Read via the Properties interface.
        let props = zbus::blocking::fdo::PropertiesProxy::builder(&conn)
            .destination(DEST)
            .and_then(|b| b.path(session_path.clone()))
            .map_err(|e| Error::Bus(e.to_string()))?
            .build()
            .map_err(|e| Error::Bus(e.to_string()))?;

        let seat_val = props
            .get(SESSION_IFACE.try_into().expect("a valid interface name"), "Seat")
            .map_err(|e| Error::Refused { method: "Get(Seat)", reason: e.to_string() })?;
        let (seat_name, seat_path): (String, zvariant::OwnedObjectPath) =
            seat_val.try_into().map_err(|e: zvariant::Error| Error::Refused {
                method: "Get(Seat)",
                reason: format!("Seat is (so), not a string: {e}"),
            })?;

        let active_val = props
            .get(SESSION_IFACE.try_into().expect("a valid interface name"), "Active")
            .map_err(|e| Error::Refused { method: "Get(Active)", reason: e.to_string() })?;
        let active: bool = active_val.try_into().unwrap_or(false);

        let inner = Arc::new(Inner {
            conn,
            session_path,
            seat_path,
            seat_name,
            active: AtomicBool::new(active),
            devices: Mutex::new(HashMap::new()),
        });

        // ★ TakeControl(force=false). `true` would STEAL the session from
        // whatever holds it — which on a working seat is the display manager,
        // i.e. exactly the thing that started us. Forcing is how a compositor
        // takes over a machine it was supposed to share.
        inner.call(SESSION_IFACE, &inner.session_path, "TakeControl", &(false,))?;

        // Rendezvous rather than unbounded: session events are rare and
        // ordering matters more than throughput. A pause that overtakes its
        // resume would leave the compositor believing it is backgrounded.
        let (tx, rx) = sync_channel::<SessionEvent>(8);
        spawn_signal_thread(Arc::clone(&inner), tx);

        Ok((
            Self { inner: Arc::clone(&inner) },
            LogindSessionNotifier { rx, _inner: inner },
        ))
    }
}

/// The signal listener. Runs on its own thread because zbus's blocking signal
/// surface is an iterator, not a pollable fd — see the header.
fn spawn_signal_thread(inner: Arc<Inner>, tx: SyncSender<SessionEvent>) {
    let _ = std::thread::Builder::new()
        .name("omoya-logind".into())
        .spawn(move || {
            let rule = format!(
                "type='signal',sender='{DEST}',path='{}'",
                inner.session_path.as_str()
            );
            let Ok(iter) = zbus::blocking::MessageIterator::for_match_rule(
                rule.as_str(),
                &inner.conn,
                Some(64),
            ) else {
                tracing::error!("logind signal match failed — VT switches will go unobserved");
                return;
            };

            for msg in iter.flatten() {
                let Some(member) = msg.header().member().map(|m| m.to_string()) else {
                    continue;
                };
                match member.as_str() {
                    "PauseDevice" => handle_pause(&inner, &msg, &tx),
                    "ResumeDevice" => handle_resume(&inner, &msg),
                    "PropertiesChanged" => handle_properties(&inner, &msg, &tx),
                    _ => {}
                }
            }
        });
}

fn handle_pause(inner: &Arc<Inner>, msg: &zbus::Message, _tx: &SyncSender<SessionEvent>) {
    let Ok((major, minor, kind)) = msg.body().deserialize::<(u32, u32, String)>() else {
        return;
    };
    // ★ THE ACK THAT KEEPS THE MACHINE FROM FREEZING. logind sends
    // `pause` and WAITS for PauseDeviceComplete before proceeding with the VT
    // switch. A client that only listens looks correct and hangs the switch.
    // "gone" means the device is already removed — acking it is meaningless
    // and logind does not wait for it.
    if kind != "gone" {
        if let Err(e) = inner.call(
            SESSION_IFACE,
            &inner.session_path,
            "PauseDeviceComplete",
            &(major, minor),
        ) {
            tracing::error!(error = %e, major, minor, "PauseDeviceComplete failed — a VT switch may hang");
        }
    }
}

fn handle_resume(inner: &Arc<Inner>, msg: &zbus::Message) {
    let Ok((major, minor, new_fd)) = msg.body().deserialize::<(u32, u32, zvariant::OwnedFd)>()
    else {
        return;
    };
    let new_fd = std::os::fd::OwnedFd::from(new_fd);

    let Ok(devices) = inner.devices.lock() else { return };
    let Some(&old) = devices.get(&(major, minor)) else {
        // Resumed a device we never took. Not an error — logind resumes
        // everything it paused, and we may have released one meanwhile.
        return;
    };

    // ★ dup2 ONTO the fd the caller already holds. libinput and drm were handed
    // that number once and will never ask again, so the only way to give them
    // the resumed device is to change what their number points at. This is what
    // libseat does; it is the protocol's shape, not a trick.
    // ★ `rustix::io::dup2` takes `&mut OwnedFd` because it normally MANAGES the
    // target's lifetime. Here the target belongs to libinput or drm, not to us,
    // so it is wrapped for the call and released with `into_raw_fd` — which
    // gives up ownership WITHOUT closing. Letting the wrapper drop would close
    // the caller's device out from under it, turning a resume into the exact
    // failure it exists to prevent.
    let mut target = unsafe { OwnedFd::from_raw_fd(old) };
    let result = smithay::reexports::rustix::io::dup2(&new_fd, &mut target);
    let _ = target.into_raw_fd();
    if let Err(e) = result {
        tracing::error!(error = %e, major, minor, "dup2 on resume failed — the device is now dead to its holder");
    }
}

fn handle_properties(inner: &Arc<Inner>, msg: &zbus::Message, tx: &SyncSender<SessionEvent>) {
    // The body must outlive the deserialised borrow — `msg.body()` is a
    // temporary, and inlining it drops the buffer while `Value<'_>` still
    // points into it.
    let body = msg.body();
    let Ok((iface, changed, _inval)) =
        body.deserialize::<(String, HashMap<String, zvariant::Value<'_>>, Vec<String>)>()
    else {
        return;
    };
    if iface != SESSION_IFACE {
        return;
    }
    let Some(v) = changed.get("Active") else { return };
    let Ok(now) = bool::try_from(v.try_clone().unwrap_or(zvariant::Value::Bool(false))) else {
        return;
    };
    // ★ The SESSION-level event, derived from the `Active` property rather than
    // from device pauses. logind pauses devices individually; smithay's
    // SessionEvent is about the seat as a whole, and conflating them would
    // report a session pause every time one device changed hands.
    if inner.active.swap(now, Ordering::Relaxed) != now {
        let _ = tx.send(if now {
            SessionEvent::ActivateSession
        } else {
            SessionEvent::PauseSession
        });
    }
}

impl Session for LogindSession {
    type Error = Error;

    fn open(&mut self, path: &Path, _flags: OFlags) -> Result<OwnedFd, Self::Error> {
        // logind addresses devices by (major, minor), not by path — so the
        // path must be stat'd first. This is why `Session::open` cannot be a
        // thin passthrough.
        let stat = smithay::reexports::rustix::fs::stat(path)?;
        let dev = stat.st_rdev;
        #[allow(clippy::cast_possible_truncation)]
        let (major, minor) = (
            smithay::reexports::rustix::fs::major(dev) as u32,
            smithay::reexports::rustix::fs::minor(dev) as u32,
        );

        let reply = self.inner.call(
            SESSION_IFACE,
            &self.inner.session_path,
            "TakeDevice",
            &(major, minor),
        )?;
        // ★ `hb`, not `h`. The bool is `inactive`; reading this as a bare fd is
        // a runtime deserialisation failure.
        let (fd, _inactive): (zvariant::OwnedFd, bool) =
            reply.body().deserialize().map_err(|e| Error::Refused {
                method: "TakeDevice",
                reason: format!("reply is (fd, inactive), not a bare fd: {e}"),
            })?;

        let owned = OwnedFd::from(fd);
        if let Ok(mut d) = self.inner.devices.lock() {
            d.insert((major, minor), owned.as_raw_fd());
        }
        Ok(owned)
    }

    fn close(&mut self, fd: OwnedFd) -> Result<(), Self::Error> {
        let raw = fd.as_raw_fd();
        let found = self
            .inner
            .devices
            .lock()
            .ok()
            .and_then(|mut d| {
                let key = d.iter().find(|(_, v)| **v == raw).map(|(k, _)| *k);
                key.inspect(|k| {
                    d.remove(k);
                })
            });

        if let Some((major, minor)) = found {
            self.inner.call(
                SESSION_IFACE,
                &self.inner.session_path,
                "ReleaseDevice",
                &(major, minor),
            )?;
        }
        // `fd` drops here, closing it. Released to logind first, so the order
        // is release-then-close rather than close-then-release-a-dead-device.
        Ok(())
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error> {
        // On the SEAT object, not the session — a VT belongs to the seat.
        #[allow(clippy::cast_sign_loss)]
        let n = vt.max(0) as u32;
        self.inner
            .call(SEAT_IFACE, &self.inner.seat_path, "SwitchTo", &(n,))
            .map(|_| ())
    }

    fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Relaxed)
    }

    fn seat(&self) -> String {
        self.inner.seat_name.clone()
    }
}

// ── The calloop event source ──────────────────────────────────────────────

impl smithay::reexports::calloop::EventSource for LogindSessionNotifier {
    type Event = SessionEvent;
    type Metadata = ();
    type Ret = ();
    type Error = smithay::reexports::calloop::channel::ChannelError;

    fn process_events<F>(
        &mut self,
        readiness: smithay::reexports::calloop::Readiness,
        token: smithay::reexports::calloop::Token,
        mut callback: F,
    ) -> Result<smithay::reexports::calloop::PostAction, Self::Error>
    where
        F: FnMut(SessionEvent, &mut ()),
    {
        self.rx.process_events(readiness, token, |event, ()| {
            if let smithay::reexports::calloop::channel::Event::Msg(e) = event {
                callback(e, &mut ());
            }
        })?;
        Ok(smithay::reexports::calloop::PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.rx.register(poll, factory)
    }

    fn reregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
        factory: &mut smithay::reexports::calloop::TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.rx.reregister(poll, factory)
    }

    fn unregister(
        &mut self,
        poll: &mut smithay::reexports::calloop::Poll,
    ) -> smithay::reexports::calloop::Result<()> {
        self.rx.unregister(poll)
    }
}
