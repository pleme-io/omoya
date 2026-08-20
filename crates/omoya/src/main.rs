//! omoya (母屋) — the pleme-io-native Wayland compositor.
//!
//! `theory/OMOYA.md` M2: a nested compositor that composites real clients, with
//! the Nord background sourced from the fleet palette and the mode machine from
//! `omoya-spec`.
//!
//! ```text
//! omoya --mode session            # a desktop, nested in the current session
//! omoya --mode session -- mado    # …and spawn mado into it
//! ```
//!
//! **Not shipped, and the doc says which parts:** no DRM (M4), no PAM (M3 via
//! mukae), no lock (M7), no chrome (M9). What this binary answers is the one
//! question worth answering first — *can omoya composite at all?*

mod logind;
mod chord;
#[cfg(target_os = "linux")]
mod drm;
mod handlers;
mod introspect;
mod input;
mod state;
mod theme;
mod winit;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};

use state::{Omoya, SeatMode};

pub struct CalloopData {
    state: Omoya,
    display_handle: DisplayHandle,
}

/// What the operator asked for on the command line.
struct Args {
    mode: SeatMode,
    /// A command to spawn into the new seat once it is up.
    spawn: Option<Vec<String>>,
    /// Which backend drives the pixels.
    ///
    /// Typed rather than auto-detected, deliberately. Guessing between "nested
    /// in someone else's session" and "take the display" is a guess that, when
    /// wrong in the second direction, blanks the console of a machine the
    /// operator may be sitting in front of. An operator asks for DRM.
    backend: Backend,
    /// Which `Session` implementation arbitrates devices. See [`SessionBackend`].
    session: SessionBackend,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    /// smithay `backend_winit` — composites into a window belonging to an
    /// existing X11 or Wayland session. Needs no DRM device, no root, no VT.
    Nested,
    /// smithay `backend_drm` — takes a display. M4a: scanout only, no input.
    Drm,
}

/// Which implementation of `smithay::backend::session::Session` to use.
///
/// ── ★ WHY BOTH EXIST, AND WHY libseat IS STILL THE DEFAULT ────────────────
/// `Logind` is the pure-Rust one (`crate::logind`) and it is the destination:
/// it retires `libseat.so.1`, one of the six C libraries this compositor
/// links. It is NOT the default yet, because a session backend owns the
/// descriptors the display depends on, plo is the operator's live desktop, and
/// nothing here has yet survived a VT switch on real hardware.
///
/// So both ship, selection is typed, and the default is the code that has been
/// running. The flip is a one-line change gated on a witnessed VT switch —
/// which is the point of making it selectable rather than swapping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionBackend {
    /// libseat — the C library, and what runs today.
    LibSeat,
    /// logind over D-Bus, pure Rust. The destination.
    Logind,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = SeatMode::Session;
    let mut spawn = None;
    let mut backend = Backend::Nested;
    let mut session = SessionBackend::LibSeat;
    let mut it = std::env::args().skip(1);

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mode" => {
                let v = it.next().ok_or("--mode needs a value (entrance | session)")?;
                mode = SeatMode::parse(&v)?;
            }
            "--backend" => {
                let v = it.next().ok_or("--backend needs a value (nested | drm)")?;
                backend = match v.as_str() {
                    "nested" | "winit" => Backend::Nested,
                    "drm" | "kms" => Backend::Drm,
                    other => {
                        return Err(format!(
                            "unknown backend `{other}` — expected `nested` or `drm`"
                        ));
                    }
                };
            }
            "--session" => {
                let v = it.next().ok_or("--session needs a value (libseat | logind)")?;
                session = match v.as_str() {
                    "libseat" => SessionBackend::LibSeat,
                    "logind" => SessionBackend::Logind,
                    other => {
                        return Err(format!(
                            "unknown session backend `{other}` — expected `libseat` or `logind`"
                        ));
                    }
                };
            }
            "--" => {
                let rest: Vec<String> = it.by_ref().collect();
                if !rest.is_empty() {
                    spawn = Some(rest);
                }
                break;
            }
            "-h" | "--help" => {
                return Err(concat!(
                    "omoya — the pleme-io Wayland compositor\n\n",
                    "  omoya [--mode entrance|session] [--backend nested|drm]\n",
                    "        [--session libseat|logind] [-- CMD ARGS...]\n\n",
                    "--backend nested  composite into an existing session's window\n",
                    "--backend drm     take a display (M4a: scanout only, no input)\n",
                    "--session libseat the C library (default — what has been running)\n",
                    "--session logind  logind over D-Bus, pure Rust. Retires libseat.so.1;\n",
                    "                  not yet the default, see SessionBackend.\n\n",
                    "`lock` is not a launchable mode: it is in-process session\n",
                    "state (theory/OMOYA.md §4.2)."
                )
                .to_string());
            }
            other => return Err(format!("unknown argument `{other}` (try --help)")),
        }
    }
    Ok(Args {
        mode,
        session,
        spawn,
        backend,
    })
}

/// Attach input and wire the session notifier into the event loop.
///
/// ── ★ WHY THE NOTIFIER IS INSERTED AND NEVER JUST HELD ────────────────────
/// It is the session's only strong reference AND its event source. Binding it
/// to `_notifier` — which is what this code did until 2026-08-19 — dropped it
/// at the end of the match arm, so every later `Session::open` failed with
/// `SessionLost` and VT-switch events were never delivered at all. Inserting it
/// into the loop fixes both: the loop owns it, so it outlives every open.
fn attach_session<S, N>(
    event_loop: &mut smithay::reexports::calloop::EventLoop<'static, CalloopData>,
    session: S,
    notifier: N,
    introspect: &std::sync::Arc<crate::introspect::OmoyaIntrospect>,
) where
    S: smithay::backend::session::Session + Clone + 'static,
    N: smithay::reexports::calloop::EventSource<
            Event = smithay::backend::session::Event,
            Metadata = (),
            Ret = (),
        > + 'static,
    N::Error: Into<Box<dyn std::error::Error + Sync + Send>>,
{
    use std::sync::atomic::Ordering;

    if let Err(e) = crate::drm::attach_input(event_loop, session) {
        tracing::error!(error = %e, "input attach failed — seat is look-only");
    } else {
        introspect.input_attached.store(1, Ordering::Relaxed);
    }

    let intro = introspect.clone();
    match event_loop.handle().insert_source(notifier, move |event, (), _data| {
        use smithay::backend::session::Event as SessionEvent;
        match event {
            SessionEvent::ActivateSession => {
                intro.session_active.store(1, Ordering::Relaxed);
                tracing::info!("session ACTIVATED — the seat is ours again");
            }
            SessionEvent::PauseSession => {
                intro.session_active.store(0, Ordering::Relaxed);
                tracing::info!("session PAUSED — another VT holds the seat");
            }
        }
        intro.session_events.fetch_add(1, Ordering::Relaxed);
    }) {
        Ok(_token) => introspect.session_active.store(1, Ordering::Relaxed),
        // Reported, not fatal: a seat that cannot observe VT switches is
        // degraded; a seat that refuses to start is worse.
        Err(e) => tracing::error!(
            error = %e,
            "session notifier not inserted — VT switches will go unobserved and \
             later device opens will fail"
        ),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    // ★ The introspection sidecar, spawned BEFORE either backend runs so an
    // agent can observe a seat that is failing to come up — which is exactly
    // when observation matters and exactly when a late-registered surface is
    // useless.
    //
    // `spawn_sidecar` is infallible by construction (it returns Option and has
    // no panic arm), so a socket that cannot bind degrades to "no
    // introspection" rather than "no compositor". That property is why this
    // call can sit on the startup path at all.
    let introspect = crate::introspect::OmoyaIntrospect::new();

    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let _ = introspect.mode.set(args.mode.name().to_string());
    match kanshou::Server::spawn_sidecar("omoya", introspect.clone()) {
        Some(path) => tracing::info!(socket = %path.display(), "introspection sidecar up"),
        None => tracing::warn!(
            "introspection sidecar did NOT start — omoya runs, but `gen kanshou \
             query omoya ...` will find nothing"
        ),
    }

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;
    let display: Display<Omoya> = Display::new()?;
    let display_handle = display.handle();
    let state = Omoya::new(&mut event_loop, display, args.mode);

    let mut data = CalloopData {
        state,
        display_handle,
    };

    match args.backend {
        Backend::Nested => {
            introspect.backend.store(0, std::sync::atomic::Ordering::Relaxed);
            crate::winit::init_winit(&mut event_loop, &mut data)?;
        }
        Backend::Drm => {
            // M4a is a SCANOUT PROBE, not yet a render loop: it opens the
            // device, takes master, finds a connected connector and its
            // preferred mode, and reports what it would drive. Saying that
            // plainly is the honest rung — a `--backend drm` that silently did
            // nothing, or that claimed a display it never programmed, is
            // exactly the round-up this project keeps refusing.
            let path = std::path::Path::new("/dev/dri/card0");
            let (device, _fd) = crate::drm::open_device(path)?;
            let target = crate::drm::probe(&device)?;
            tracing::info!(
                connector = %target.name,
                mode = %format_args!("{}x{}", target.mode.size().0, target.mode.size().1),
                refresh_hz = target.mode.vrefresh(),
                frame_interval_us = crate::drm::frame_interval(&target).as_micros() as u64,
                "DRM scanout target acquired — M4a probe"
            );
            // Hold the display and serve clients — M4b. The one-shot paint
            // below is kept as `--backend drm-probe` would be if it earned a
            // flag; today the persistent loop supersedes it.
            introspect.backend.store(1, std::sync::atomic::Ordering::Relaxed);
            introspect
                .output_w
                .store(u64::from(target.mode.size().0), std::sync::atomic::Ordering::Relaxed);
            introspect
                .output_h
                .store(u64::from(target.mode.size().1), std::sync::atomic::Ordering::Relaxed);
            introspect
                .refresh_hz
                .store(u64::from(target.mode.vrefresh()), std::sync::atomic::Ordering::Relaxed);

            let mut device = device;
            crate::drm::run(&mut event_loop, &mut data, &mut device, &_fd, &target, introspect.clone())?;

            // M4c. A failure here is REPORTED, not fatal: a seat that renders
            // but cannot be typed into is a bad seat, and a seat that refuses
            // to come up at all is a worse one — on a machine whose only
            // console this may be, degrading to "visible but not typeable"
            // beats degrading to "black".
            // ── ★ ONE SHAPE, TWO BACKENDS ────────────────────────────────
            // Both arms do exactly the same three things — attach input,
            // insert the notifier, publish that the session is live — and the
            // only difference is which `Session` impl arbitrates devices. The
            // duplication is real and deliberate: the two notifier types are
            // distinct concrete types, and unifying them behind a trait object
            // would need `EventSource` to be object-safe, which it is not.
            //
            // A macro would hide three lines and cost the reader the ability to
            // see that the arms ARE the same, which is the property that makes
            // flipping the default safe.
            match args.session {
                SessionBackend::LibSeat => {
                    match smithay::backend::session::libseat::LibSeatSession::new() {
                        Ok((session, notifier)) => {
                            attach_session(&mut event_loop, session, notifier, &introspect);
                        }
                        Err(e) => tracing::error!(
                            error = %e,
                            "no libseat session — seat is look-only. Run from a seat0 \
                             session (a VT), not over ssh"
                        ),
                    }
                }
                SessionBackend::Logind => match crate::logind::LogindSession::new() {
                    Ok((session, notifier)) => {
                        attach_session(&mut event_loop, session, notifier, &introspect);
                    }
                    Err(e) => tracing::error!(
                        error = %e,
                        "no logind session — seat is look-only. `--session logind` needs \
                         this process to be IN a logind session; over ssh it is not"
                    ),
                },
            }
            // Fall through to the shared event loop below, which is what M2
            // already runs. That sharing is the point: one compositor, two
            // backends, not two programs.
        }
    }

    // Announce the socket BEFORE spawning, so a client started here finds it.
    let socket = data.state.socket_name.clone();
    let _ = introspect
        .socket
        .set(socket.to_string_lossy().into_owned());
    tracing::info!(
        socket = ?socket,
        mode = args.mode.name(),
        "omoya is up — WAYLAND_DISPLAY is set for children"
    );

    if let Some(cmd) = args.spawn
        && let Some((program, rest)) = cmd.split_first()
    {
        match std::process::Command::new(program)
            .args(rest)
            .env("WAYLAND_DISPLAY", &socket)
            .spawn()
        {
            Ok(child) => tracing::info!(pid = child.id(), program, "spawned into the seat"),
            // Deliberately NOT fatal: a compositor whose first client fails to
            // start is still a working compositor, and exiting here would make
            // a typo in the command look like omoya crashing.
            Err(e) => tracing::error!(program, error = %e, "could not spawn"),
        }
    }

    event_loop.run(None, &mut data, |_| {})?;
    Ok(())
}
