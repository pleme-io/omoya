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

mod bar;
mod chord;
mod config;
mod cursor;
mod deed;
mod denpa;
#[cfg(target_os = "linux")]
mod drm;
mod evdev_backend;
mod grab;
mod handlers;
mod input;
mod introspect;
mod layout;
mod localtime;
mod logind;
mod mcp;
mod nuri_renderer;
mod owed;
mod placement;
mod remap;
mod rouka;
mod scanout;
mod stale;
mod state;
mod synth;
mod theme;
mod truedamage;
mod uevent;
mod wash;
mod windowmode;
/// The nested development backend. Off by default — it drags in winit, which
/// `dlopen`s libxkbcommon behind `ldd`'s back. See `Cargo.toml`'s `[features]`.
#[cfg(feature = "nested")]
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
    /// The seat's application launcher, bound to `Ctrl+Space`.
    ///
    /// Its own flag rather than a slot in `--`, because `--` is positional and
    /// terminal: everything after it is one command. A seat wants both a first
    /// program AND a launcher, and there is no second `--`.
    launcher: Option<Vec<String>>,
    /// Which backend drives the pixels.
    ///
    /// Typed rather than auto-detected, deliberately. Guessing between "nested
    /// in someone else's session" and "take the display" is a guess that, when
    /// wrong in the second direction, blanks the console of a machine the
    /// operator may be sitting in front of. An operator asks for DRM.
    backend: Backend,
    /// Which `Session` implementation arbitrates devices. See [`SessionBackend`].
    session: SessionBackend,
    /// Which implementation supplies input. See [`InputBackendKind`].
    input: InputBackendKind,
    /// Which rasterizer paints. See [`RendererKind`].
    renderer: RendererKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    /// smithay `backend_winit` — composites into a window belonging to an
    /// existing X11 or Wayland session. Needs no DRM device, no root, no VT.
    ///
    /// Development only, and absent unless built with `--features nested`:
    /// the variant is gated rather than left to fail at runtime, so
    /// `--backend nested` on a shipped binary is a clear "unknown backend"
    /// rather than a start-up that dies later for an unrelated-looking reason.
    #[cfg(feature = "nested")]
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
    /// logind over D-Bus, pure Rust — and now the only one.
    ///
    /// ★ STILL NOT THE FULL ANSWER. logind is a C DAEMON, so this retires a
    /// linked C library and leaves a C process holding the seat. The thing
    /// that removes the daemon is a session speaking VT and DRM-master ioctls
    /// directly, which is `pending-omoya-direct-session`.
    Logind,
}

/// Which implementation supplies input events.
///
/// ── ★ WHY BOTH, AND WHY libinput IS STILL THE DEFAULT ─────────────────────
/// `Evdev` reads `/dev/input/event*` directly and retires TWO C libraries —
/// `libinput.so.10` and `libudev.so.1`. It is the destination.
///
/// It is not the default yet because libinput's real value is POLICY —
/// pointer acceleration, tap-to-click, gesture recognition, per-device
/// calibration — and the evdev backend implements none of it. A seat that
/// swapped silently would feel wrong in ways nobody could name: the pointer
/// too fast, a touchpad tap doing nothing. That is a worse failure than a
/// missing feature, because it presents as "the compositor is bad".
///
/// `pending-omoya-input-policy: acceleration, tap-to-click, gestures`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputBackendKind {
    /// evdev straight from the kernel, pure Rust — and now the only one.
    Evdev,
}

/// Which rasterizer paints the frame.
///
/// ── ★ THE ONLY TRUE NATURALIZE OF THE SIX ─────────────────────────────────
/// The other five C libraries wrap a kernel interface; `libpixman` wraps
/// nothing. It is arithmetic over memory, which is why it is the one that had
/// to be REBUILT rather than re-addressed — and `nuri` is that rebuild: 485
/// lines, zero dependencies, 11 tests that run without a seat.
///
/// pixman remains available because it is the incumbent and this is a display:
/// a rasterizer that is subtly wrong shows a machine's owner a broken screen,
/// and having the previous one one flag away is what makes the comparison
/// cheap rather than a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendererKind {
    /// nuri — pleme-io's own, and now the only one.
    Nuri,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = SeatMode::Session;
    let mut spawn = None;
    let mut launcher = None;
    // The nested backend is the convenient default when it exists, because
    // that build is for developing on a machine that already has a session.
    // A shipped binary has no such backend and takes a seat: Drm.
    #[cfg(feature = "nested")]
    let mut backend = Backend::Nested;
    #[cfg(not(feature = "nested"))]
    let mut backend = Backend::Drm;
    let mut session = SessionBackend::Logind;
    // ★ THE DEFAULTS ARE OURS NOW. Each was proven before it was promoted:
    // nuri has 11 green tests and a mapped Bind; evdev decodes the kernel's own
    // event stream; logind took a real seat session on vkms with no group
    // membership. Leaving them selectable-but-off would have been a way of
    // never having to stand behind them.
    let mut input = InputBackendKind::Evdev;
    let mut renderer = RendererKind::Nuri;
    let mut it = std::env::args().skip(1);

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mode" => {
                let v = it
                    .next()
                    .ok_or("--mode needs a value (entrance | session)")?;
                mode = SeatMode::parse(&v)?;
            }
            "--backend" => {
                let v = it.next().ok_or("--backend needs a value (nested | drm)")?;
                backend = match v.as_str() {
                    #[cfg(feature = "nested")]
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
                let v = it
                    .next()
                    .ok_or("--session needs a value (libseat | logind)")?;
                session = match v.as_str() {
                    "logind" => SessionBackend::Logind,
                    // ★ Not in the build: backend_session_libseat is off and
                    // libseat.so.1 is not linked.
                    "libseat" => {
                        return Err("session backend `libseat` is not in this build — the \
                             backend_session_libseat feature is off and libseat is not \
                             linked. `logind` is the only backend."
                            .to_string());
                    }
                    other => {
                        return Err(format!(
                            "unknown session backend `{other}` — expected `libseat` or `logind`"
                        ));
                    }
                };
            }
            "--input" => {
                let v = it
                    .next()
                    .ok_or("--input needs a value (libinput | evdev)")?;
                input = match v.as_str() {
                    "evdev" => InputBackendKind::Evdev,
                    // ★ Not in the build: backend_libinput/backend_udev are off.
                    // Someone reaching for this wants the input POLICY that came
                    // with libinput — acceleration, tap-to-click, gestures — and
                    // deserves that answer rather than "unknown value".
                    "libinput" => {
                        return Err("input backend `libinput` is not in this build — the \
                             backend_libinput/backend_udev features are off. `evdev` is \
                             the only backend, and it implements no pointer \
                             acceleration, tap-to-click or gestures \
                             (pending-omoya-input-policy)."
                            .to_string());
                    }
                    other => {
                        return Err(format!(
                            "unknown input backend `{other}` — expected `evdev`"
                        ));
                    }
                };
            }
            "--renderer" => {
                let v = it
                    .next()
                    .ok_or("--renderer needs a value (nuri | pixman)")?;
                renderer = match v.as_str() {
                    "nuri" => RendererKind::Nuri,
                    // ★ `pixman` is not merely unsupported, it is NOT IN THE
                    // BUILD: the `renderer_pixman` feature is off, so the
                    // library is not linked. Naming that is better than a
                    // generic "unknown value", because someone reaching for it
                    // is asking for a fallback that no longer exists.
                    "pixman" => {
                        return Err("renderer `pixman` is not in this build — the \
                             renderer_pixman feature is off and libpixman is not \
                             linked. `nuri` is the only renderer."
                            .to_string());
                    }
                    other => {
                        return Err(format!("unknown renderer `{other}` — expected `nuri`"));
                    }
                };
            }
            "--launcher" => {
                // Split on whitespace so a Nix module can pass one string.
                // Deliberately NOT a shell parse: no quoting, no expansion, no
                // `sh -c`. A launcher command needing shell syntax is a
                // launcher command that should be a script, and running one
                // through a shell here would put an argv-injection surface on
                // a chord the operator presses constantly.
                let v = it
                    .next()
                    .ok_or("--launcher needs a command (e.g. --launcher tobira)")?;
                let parts: Vec<String> = v.split_whitespace().map(str::to_owned).collect();
                if parts.is_empty() {
                    return Err("--launcher was given an empty command".into());
                }
                launcher = Some(parts);
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
                    "        [--session logind|libseat] [--input evdev|libinput]\n",
                    "        [--renderer nuri|pixman] [--launcher CMD]\n",
                    "        [-- CMD ARGS...]\n\n",
                    "--backend nested  composite into an existing session's window\n",
                    "--backend drm     take a display (M4a: scanout only, no input)\n",
                    "--session libseat the C library (default — what has been running)\n",
                    "\n",
                    "  ★ THE DEFAULTS ARE PURE RUST. Each C library is one flag away.\n\n",
                    "--session logind  The session backend. logind over D-Bus, pure\n",
                    "                  Rust; libseat is NOT LINKED. logind itself is a C\n",
                    "                  daemon — see pending-omoya-direct-session.\n",
                    "--input evdev     DEFAULT. Kernel evdev, pure Rust. NO pointer\n",
                    "                  acceleration, tap-to-click or gestures — that is\n",
                    "                  libinput policy nobody has reimplemented.\n",
                    "--input libinput  the C library, and it carries that policy.\n",
                    "--renderer nuri   The renderer. pleme-io's own, zero dependencies.\n",
                    "                  libpixman is NOT LINKED — the feature is off.\n\n",
                    "--launcher CMD    what Ctrl+Space opens. Split on whitespace, NOT\n",
                    "                  through a shell — a launcher needing shell syntax\n",
                    "                  should be a script. Absent means Ctrl+Space says so\n",
                    "                  in the log rather than opening something else.\n\n",
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
        input,
        renderer,
        spawn,
        launcher,
        backend,
    })
}

/// Take the seat: open the DRM device THROUGH the session, probe it, run the
/// render loop, then attach input and the session notifier.
///
/// ── ★ WHY THE DEVICE OPEN IS IN HERE AND NOT BEFORE ───────────────────────
/// Because it must come from `Session::open`. A DRM fd opened directly is
/// invisible to logind — it cannot be paused on a VT switch — and needs
/// filesystem permission the session would otherwise grant. Both facts point
/// the same way: the session is created first and hands out the device.
fn run_drm_seat<S, N>(
    event_loop: &mut smithay::reexports::calloop::EventLoop<'static, CalloopData>,
    data: &mut CalloopData,
    introspect: &std::sync::Arc<crate::introspect::OmoyaIntrospect>,
    mut session: S,
    notifier: N,
    kind: InputBackendKind,
    renderer: RendererKind,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: smithay::backend::session::Session + Clone + 'static,
    N: smithay::reexports::calloop::EventSource<
            Event = smithay::backend::session::Event,
            Metadata = (),
            Ret = (),
        > + 'static,
    N::Error: Into<Box<dyn std::error::Error + Sync + Send>>,
{
    use smithay::reexports::rustix::fs::OFlags;
    use std::sync::atomic::Ordering;

    // ── ★ FIND THE CARD; DO NOT ASSUME `card0` ────────────────────────────
    //
    // This was `/dev/dri/card0` and that is not a device that exists on every
    // machine. Measured on plo: the only DRM node is **`card1`** (nvidia, with
    // `nvidia_modeset` loaded, monitor on `card1-DP-1`) and there is no
    // `card0` at all — so the compositor could not open a display on the one
    // machine it ships to, while the vkms gate passed because vkms happens to
    // enumerate as `card0`.
    //
    // The number is a kernel enumeration order, not an identity. What we
    // actually want is "the card with something plugged into it", so that is
    // what we look for: open each candidate through the session and keep the
    // first whose `probe` finds a connected connector.
    //
    // Order matters for a reason beyond tidiness — a machine can carry a
    // render-only node or a headless card alongside the real one, and picking
    // by index would land on either.
    let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir("/dev/dri")
        .map_err(|e| format!("cannot enumerate /dev/dri: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"))
        })
        .collect();
    // Stable order so a failure is reproducible rather than dependent on
    // readdir order.
    candidates.sort();
    if candidates.is_empty() {
        return Err("no /dev/dri/card* exists — there is no DRM device to drive".into());
    }

    let mut opened = None;
    let mut refusals: Vec<String> = Vec::new();
    for path in &candidates {
        // O_RDWR so we can modeset; O_NONBLOCK because the event loop reads
        // vblank events off this fd and a blocking read would stall every frame.
        let fd = match session.open(path, OFlags::RDWR | OFlags::NONBLOCK) {
            Ok(fd) => fd,
            Err(e) => {
                refusals.push(format!("{}: session refused ({e:?})", path.display()));
                continue;
            }
        };
        let (device, drm_fd, drm_notifier) = match crate::drm::device_from_fd(fd) {
            Ok(t) => t,
            Err(e) => {
                refusals.push(format!("{}: not a usable DRM device ({e})", path.display()));
                continue;
            }
        };
        match crate::drm::probe(&device) {
            Ok(target) => {
                tracing::info!(card = %path.display(), "DRM device selected — it has a connected connector");
                opened = Some((device, drm_fd, drm_notifier, target));
                break;
            }
            Err(e) => {
                refusals.push(format!("{}: no connected connector ({e})", path.display()));
            }
        }
    }

    // Every refusal is named. A bare "no display found" on a machine with four
    // card nodes is the message that costs an hour.
    let (device, drm_fd, drm_notifier, target) = opened.ok_or_else(|| {
        format!(
            "no DRM device with a connected connector. Tried {}: {}",
            candidates.len(),
            refusals.join("; ")
        )
    })?;

    // ★ DRAIN THE DRM EVENT QUEUE, OR SCANOUT DIES AFTER ~135 FRAMES.
    //
    // Every flip is issued with `event: true`, so the kernel queues a
    // page-flip event per frame. If nothing reads them the queue grows until
    // allocation fails and flips start returning ENOMEM — about 2.4 seconds
    // in. Inserting the notifier is what reads them.
    //
    // This is the same shape as the session notifier a few lines up, and the
    // repeat is the lesson: a smithay `*Notifier` is not a handle you may
    // discard, it is the half of the device that does the work. Both were
    // written as `let (x, _notifier) = …` and both compiled without a warning.
    //
    // Pacing still comes from a timer (`pending-omoya-vblank`); this only
    // consumes the events. Doing both from here is the eventual fix.
    // ★ ONE FLIP IN FLIGHT AT A TIME. The kernel refuses a page flip issued
    // while the previous is still pending, with EBUSY. `DirectScanout` swaps
    // its back buffer when a flip is ACCEPTED rather than when it RETIRES, and
    // the render timer had no idea either way — the frame period simply used
    // to be long enough to hide it.
    //
    // Measured: removing the render time from the period (pacing from the
    // deadline instead of from now) surfaced it immediately as 29 frame
    // failures on vkms, all `Device or resource busy (os error 16)`. The
    // pacing change did not break scanout; it removed the slack that was
    // concealing this.
    //
    // So the vblank event — which the notifier below already had to drain — is
    // now also the signal that the display is ready for another frame. That is
    // a step toward `pending-omoya-vblank` (render FROM vblank) without the
    // restructure: for now the timer proposes and this flag disposes.
    // ★ INSTALL THE ESCAPE HATCH. `Session::change_vt` existed and nothing
    // called it; `input.rs` recognised Ctrl+Alt+F<n>, counted it, and forwarded
    // to a kernel that logind's TakeControl has already put in K_OFF. The
    // result was a seat with no way out except ssh, on a machine whose console
    // IS the seat.
    //
    // Installed here because this is the only scope holding a session. Cloned
    // rather than borrowed — `LogindSession` is Clone precisely so the
    // compositor can keep a handle.
    {
        let mut vt_session = session.clone();
        data.state.vt_switch = Some(Box::new(move |vt: i32| {
            vt_session.change_vt(vt).map_err(|e| format!("{e:?}"))
        }));
    }

    let flip_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip_pending_ev = flip_pending.clone();
    event_loop
        .handle()
        .insert_source(drm_notifier, move |event, _meta, _data| {
            match event {
                smithay::backend::drm::DrmEvent::VBlank(_) => {
                    // The flip retired; the back buffer is free again.
                    flip_pending_ev.store(false, std::sync::atomic::Ordering::Release);
                }
                smithay::backend::drm::DrmEvent::Error(err) => {
                    tracing::warn!(?err, "DRM device reported an error");
                }
            }
        })
        .map_err(|e| format!("inserting the DRM notifier: {e}"))?;

    // `target` came from the selection loop above — probing again here would
    // re-ask a question already answered, and could answer it differently if a
    // cable moved in between.
    tracing::info!(
        connector = %target.name,
        mode = %format_args!("{}x{}", target.mode.size().0, target.mode.size().1),
        refresh_hz = target.mode.vrefresh(),
        "DRM scanout target acquired — opened THROUGH the session"
    );

    introspect.backend.store(1, Ordering::Relaxed);
    introspect
        .output_w
        .store(u64::from(target.mode.size().0), Ordering::Relaxed);
    introspect
        .output_h
        .store(u64::from(target.mode.size().1), Ordering::Relaxed);
    *introspect.modes.lock().unwrap_or_else(|e| e.into_inner()) = target.mode_list.clone();
    // ★ THE SAME DERIVATION AS THE RENDER LOOP, NOT `vrefresh()` RAW.
    //
    // This line stored the panel's raw `vrefresh`, which is an optional DRM
    // field and is 0 on plo's DP-1 — so the leaf reported 0 while the loop
    // paced at 1 Hz, and the one diagnostic that could have named the bug
    // answered with a number that looked like "not measured yet".
    //
    // Two writers to one field is the drift hazard on its own; both now go
    // through `drm::refresh_hz` so they cannot disagree.
    introspect.refresh_hz.store(
        u64::from(crate::drm::refresh_hz(&target.mode)),
        Ordering::Relaxed,
    );

    let mut device = device;
    // ★ THE MATCH IS HERE AND NOT INSIDE `run` because the renderer is a TYPE
    // parameter, not a value: `run` is generic and monomorphises per renderer.
    // A `Box<dyn Renderer>` would not work — the trait has associated types
    // (TextureId, Frame, Framebuffer) and is not object-safe, which is smithay
    // telling us a renderer is a compile-time choice.
    match renderer {
        RendererKind::Nuri => crate::drm::run(
            flip_pending.clone(),
            event_loop,
            data,
            &mut device,
            &drm_fd,
            &target,
            introspect.clone(),
            {
                let mut r = crate::nuri_renderer::NuriRenderer::new();
                r.set_introspect(introspect.clone());
                r
            },
        )?,
    }

    attach_session(event_loop, session, notifier, introspect, kind);
    Ok(())
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
    kind: InputBackendKind,
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

    // ── ★ TWO INPUT TRANSPORTS, ONE SEAM ─────────────────────────────────
    // `process_input_event<I: InputBackend>` (input.rs:36) is already generic,
    // so neither transport required a change to the handling code — the seam
    // existed before either backend did.
    // ★ THE TOKEN IS KEPT. It is the only handle by which the session
    // notifier can make the input source re-register its fds after a VT
    // switch — see the ActivateSession arm below.
    let mut input_token: Option<smithay::reexports::calloop::RegistrationToken> = None;
    let attached = match kind {
        InputBackendKind::Evdev => {
            crate::evdev_backend::EvdevBackend::new(session.clone(), Some(introspect.clone()))
                .map_err(|e| format!("{e}"))
                .and_then(|backend| {
                    event_loop
                        .handle()
                        .insert_source(backend, move |event, (), data| {
                            data.state.process_input_event(event);
                        })
                        .map(|token| input_token = Some(token))
                        .map_err(|e| format!("{e}"))
                })
        }
    };
    match attached {
        Ok(()) => {
            introspect.input_attached.store(1, Ordering::Relaxed);
            tracing::info!(backend = ?kind, "input attached — the seat is now typeable");
        }
        Err(e) => {
            tracing::error!(error = %e, backend = ?kind, "input attach failed — seat is look-only")
        }
    }

    let intro = introspect.clone();
    // Cloned into the callback below. This is an `Rc` into the loop's own
    // internals held by a source the loop owns, i.e. a cycle — accepted
    // because the loop is process-lifetime, and named rather than left to be
    // rediscovered.
    let handle = event_loop.handle();
    match event_loop
        .handle()
        .insert_source(notifier, move |event, (), _data| {
            use smithay::backend::session::Event as SessionEvent;
            match event {
                SessionEvent::ActivateSession => {
                    intro.session_active.store(1, Ordering::Relaxed);
                    // ★ RE-ARM INPUT. logind resumes a device by `dup2`ing a NEW
                    // file description onto the fd NUMBER the evdev backend
                    // already holds (`logind.rs:320-322`), and the kernel drops
                    // every epoll entry for a file as that file is freed —
                    // `fs/file_table.c:422` calls `eventpoll_release`,
                    // `fs/eventpoll.c:1083` implements it. So returning from
                    // another VT leaves the seat visually correct and completely
                    // untypeable until something re-registers those fds.
                    // `LoopHandle::update` is the only public way to make a source
                    // re-register from outside itself (calloop
                    // `loop_logic.rs:199-228`); the backend re-arms on the
                    // inactive → active TRANSITION it observes inside its own
                    // `reregister`.
                    if let Some(tok) = &input_token {
                        if let Err(e) = handle.update(tok) {
                            tracing::error!(
                                error = %e,
                                "input re-arm failed — the seat is back but not typeable"
                            );
                        }
                    }
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
    // ── ★ THE MCP BRANCH RUNS BEFORE ANYTHING ELSE ──────────────────────
    // Two reasons it cannot be folded into `parse_args`:
    //
    // 1. stdout is the JSON-RPC framing channel. The tracing init below
    //    defaults to STDOUT, so a single log line emitted before the
    //    server starts corrupts the protocol for the whole session — and
    //    the failure surfaces at the client as unparseable garbage, not as
    //    a message naming the cause. Redirect to stderr first, always.
    //
    // 2. `omoya mcp` does NOT take a seat. It is a stdio sidecar that
    //    forwards to the compositor already running on this host over
    //    kanshou. Falling through to `parse_args` would try to open DRM
    //    and fight the live session for the operator's screen.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(crate::mcp::serve());
    }

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
    let state = Omoya::new(&mut event_loop, display, args.mode, introspect.clone());

    // ── ★ THE MUTATE PATH: A PING, NOT A POLL ────────────────────────────
    //
    // `Introspect::query` runs on the kanshou sidecar's thread and may not
    // touch `Omoya`, which lives in this single-threaded loop. So the socket
    // thread pushes a `Deed` onto `introspect.pending_deeds` and pings; this
    // source drains it here, where `&mut Omoya` is legal.
    //
    // The ping is what makes it work on a QUIET desktop. mado's equivalent
    // drains once per GUI frame, which is fine for something that renders
    // continuously — but since damage tracking landed, an idle omoya renders
    // nothing at all (measured: 183 ticks, 0 presentations). A queued deed
    // would sit until something else woke the loop, i.e. exactly when a
    // remote caller most needs it, it would look broken.
    match smithay::reexports::calloop::ping::make_ping() {
        Ok((ping, source)) => {
            let sink = introspect.clone();
            if let Err(e) = event_loop
                .handle()
                .insert_source(source, move |(), (), data| {
                    // Drain under the lock, perform outside it: `perform` can
                    // spawn a process and send configures, and holding the
                    // socket thread's lock across that would stall every read.
                    let deeds: Vec<_> = sink
                        .pending_deeds
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .drain(..)
                        .collect();
                    // ★ SYNTHETIC INPUT, ON THE SAME SEAM AND FOR THE SAME
                    // REASON. Drained here because `Omoya::key` needs `&mut
                    // Omoya`, and applied through the very method the evdev
                    // backend calls — see `synth.rs` for why taking a shortcut to
                    // the client would make this surface worthless as a
                    // diagnostic.
                    let synths: Vec<_> = sink
                        .pending_input
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .drain(..)
                        .collect();
                    for sy in synths {
                        match crate::synth::expand(&sy) {
                            Ok(steps) => {
                                for step in steps {
                                    data.state.apply_step(step);
                                    sink.synth_performed
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            // Validated at queue time too, so this is unreachable
                            // unless the two disagree — which is worth saying out
                            // loud rather than dropping.
                            Err(e) => {
                                tracing::error!(error = %e, ?sy, "unexpandable synthetic input")
                            }
                        }
                    }

                    for deed in deeds {
                        tracing::info!(?deed, "performing a deed requested over kanshou");
                        data.state.perform(deed);
                        // Counted HERE, by the thread that did the work — see
                        // `OmoyaIntrospect::deeds_performed`. The `do` leaf's
                        // "queued" answer cannot distinguish a drained deed from
                        // one nothing ever drains.
                        sink.deeds_performed
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                })
            {
                tracing::error!(error = %e, "no deed source — the seat is read-only");
            } else {
                // Published only on success, so `wake.get()` being None is an
                // honest "nothing will drain this" rather than a promise the
                // loop cannot keep.
                let _ = introspect.wake.set(ping);
            }
        }
        // Non-fatal: the seat still runs, it just cannot be driven remotely.
        // Same posture as the kanshou socket itself — degrade to less
        // introspection, never to no compositor.
        Err(e) => tracing::error!(error = %e, "no ping — the seat is read-only"),
    }

    let mut data = CalloopData {
        state,
        display_handle,
    };

    match args.backend {
        #[cfg(feature = "nested")]
        Backend::Nested => {
            introspect
                .backend
                .store(0, std::sync::atomic::Ordering::Relaxed);
            crate::winit::init_winit(&mut event_loop, &mut data)?;
        }
        Backend::Drm => {
            // ── ★ THE SESSION COMES FIRST, AND OPENS THE DEVICE ───────────
            // This used to open /dev/dri/card0 directly and create the session
            // afterwards, as an afterthought for input. That ordering was the
            // bug: a directly-opened DRM fd is INVISIBLE to logind, which can
            // only pause and resume devices taken through `TakeDevice`. So the
            // compositor kept DRM master across a VT switch while another VT
            // owned the seat, and `Session::open` — the trait's entire purpose
            // — was never called for the one device that matters most.
            //
            // Found by the vkms check on its first run: omoya died with
            // PermissionDenied as an unprivileged user, which is the mild
            // symptom of the same cause. Taking the fd from the session also
            // removes the need for `video` group membership.
            match args.session {
                SessionBackend::Logind => match crate::logind::LogindSession::new() {
                    Ok((session, notifier)) => run_drm_seat(
                        &mut event_loop,
                        &mut data,
                        &introspect,
                        session,
                        notifier,
                        args.input,
                        args.renderer,
                    )?,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "no logind session — `--session logind` needs this process to be IN \
                             one; over ssh it is not"
                        );
                        return Err(Box::new(e));
                    }
                },
            }
            // Fall through to the shared event loop below, which is what M2
            // already runs. That sharing is the point: one compositor, two
            // backends, not two programs.
        }
    }

    // Announce the socket BEFORE spawning, so a client started here finds it.
    let socket = data.state.socket_name.clone();
    let _ = introspect.socket.set(socket.to_string_lossy().into_owned());
    tracing::info!(
        socket = ?socket,
        mode = args.mode.name(),
        "omoya is up — WAYLAND_DISPLAY is set for children"
    );

    // Remember it so `Logo+Return` opens another of the same thing. Stored
    // rather than re-read from args at chord time because `args` is consumed
    // below, and because the seat's terminal is state the compositor should be
    // able to answer about, not an argument it happens to still be holding.
    // ── ★ THE CONFIG FILE, WITH FLAGS STILL WINNING ─────────────────────
    //
    // omoya had NO configuration surface until this: a hand-rolled
    // `std::env::args()` loop and 46 operator-visible values as Rust `const`s.
    // A non-US operator could not use this seat, and no key could be rebound,
    // without a recompile.
    //
    // Flags override the file rather than the other way round, and for a
    // compositor that ordering is not a preference: greetd launches omoya with
    // a command line, so a flag is the escape hatch that exists even when the
    // yaml is wrong. A bad file that could only be fixed from inside a seat it
    // prevented from starting would be a trap.
    let cfg = crate::config::load().with_cli_overrides(args.spawn.clone(), args.launcher.clone());
    tracing::info!(
        terminal = ?cfg.terminal,
        launcher = ?cfg.launcher,
        remaps = cfg.remaps.len(),
        "seat configuration resolved"
    );
    data.state.session_command = cfg.terminal.clone();
    data.state.launcher_command = cfg.launcher.clone();
    data.state.remaps = cfg.remap_pairs();
    // ★ RE-SEED THE DAMAGE MODE ONCE THE REAL CONFIG EXISTS.
    //
    // `State::new` seeds `td_mode` from the environment alone, because it runs
    // BEFORE `config::load()` — its `config` field is `prescribed()` until the
    // line below replaces it. Leaving it there would mean the typed knob was
    // parsed, published by `config-show`, and silently ignored by the hot path:
    // the worst of the three possible outcomes, because the config would LOOK
    // authoritative.
    //
    // `resolve` keeps the environment winning when it is set, so this narrows
    // nothing an operator could already do; it only gives the config a say
    // where previously there was none. From here the atomic remains the single
    // source of truth and `td_mode_set` over kanshou still overrides both.
    data.state.introspect.td_mode.store(
        crate::truedamage::Mode::resolve(cfg.damage.authority).to_u64(),
        std::sync::atomic::Ordering::Relaxed,
    );
    data.state.config = cfg;

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

    // ── ★ FLUSH AFTER EVERY DISPATCH. THIS IS WHERE TYPING WAS LOST. ────
    //
    // calloop calls this once per dispatch cycle, after every source has been
    // serviced. It is where a Wayland compositor flushes its clients, and it
    // was `|_| {}`.
    //
    // The whole crate had exactly TWO `flush_clients` calls: one in `winit.rs`
    // (the nested backend, unused on a real seat) and one at the bottom of the
    // DRM frame path — which sits BELOW the mekuri damage gate. So the only
    // flush on a real seat ran as a side effect of compositing a frame.
    //
    // A keystroke owes no frame. It is forwarded to the focused client, the
    // gate finds nothing owed, no frame is composed, no flush happens, the
    // client never receives the bytes sitting in its socket buffer, so it
    // changes nothing, so it commits nothing, so still no frame is owed. A
    // self-sustaining deadlock, and typing simply did nothing.
    //
    // The pointer escaped it for a reason that made the bug look like a
    // keyboard bug: pointer motion marks `Owed::Pointer` — omoya draws the
    // cursor itself — so it always composes a frame and drags a flush along
    // behind it. Mouse perfect, keyboard dead, one missing line.
    //
    // ★ It belongs HERE and not in the frame path. Flushing only when we draw
    // makes client delivery a function of whether the screen changed, which is
    // exactly backwards: a client that needs to be woken in order to change
    // anything must be flushed BEFORE the frame, not after one we only compose
    // if it already changed something.
    event_loop.run(None, &mut data, |data| {
        if let Err(e) = data.display_handle.flush_clients() {
            // Never fatal: one wedged client must not take the seat down. But
            // never silent either — a persistent failure here presents as
            // "input does nothing", which is the hardest symptom to trace back
            // to its cause.
            tracing::warn!(error = %e, "flushing clients failed");
        }
    })?;
    Ok(())
}
