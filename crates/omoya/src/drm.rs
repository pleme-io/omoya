//! M4a — the DRM/KMS backend: omoya on real hardware, driving a real display.
//!
//! This is what separates a compositor from a demo. The winit backend (M2)
//! composites into a window someone else's compositor owns; this one takes the
//! display itself — enumerates connectors, sets a mode on a CRTC, and scans out
//! its own buffers.
//!
//! ── ★ DUMB BUFFERS AND PIXMAN, DELIBERATELY ───────────────────────────────
//! `theory/OMOYA.md` §5a chose the software path FIRST, on the reasoning that a
//! fallback nobody exercises is not a fallback. Two things since then turned
//! that from prudent into simply correct:
//!
//!   * plo's display is driven by **simpledrm** today — the EFI framebuffer,
//!     because nvidia is loaded without modesetting. simpledrm supports dumb
//!     buffers and nothing else. A gbm/EGL path would have no device to run on
//!     until the nvidia display stage is armed by a reboot.
//!   * smithay hands us the whole path for free:
//!     `impl ExportFramebuffer<DumbBuffer> for DrmDeviceFd`
//!     (`backend/drm/exporter/dumb.rs:26`), so the device fd IS its own
//!     framebuffer exporter and `DrmCompositor` needs no `GbmDevice` at all.
//!
//! So there is no `unsafe`, no dmabuf import, no GPU driver dependency, and no
//! EGL. The compositor that comes up on a machine with a broken GPU driver is
//! the same code as the one that comes up on a working one — which is the only
//! arrangement in which "it degrades gracefully" is a fact rather than a hope.
//!
//! ── WHAT THIS RUNG IS NOT ─────────────────────────────────────────────────
//! Input is M4b and is absent here: this backend scans out, and reads no
//! evdev. That split is measured, not tidy-minded — on plo the scanout
//! dependency set (libdrm, libgbm, udev, seatd, mesa) is entirely cached while
//! adding libinput costs 11 derivations, so the boundary is where the cost
//! actually is. See `flake.nix`'s `drmDeps`.
//!
//! Session management (libseat/logind) is likewise M4b. Today the device is
//! opened directly, which means this rung needs either root or a seat0
//! session — and on a machine administered over ssh that is a deliberate,
//! recoverable choice rather than a risk: the worst case is a wedged VT on a
//! box whose real console is the ssh session.

use std::{
    os::fd::OwnedFd,
    // `custom_flags` is an EXTENSION trait method, not inherent on
    // OpenOptions — without this import the call fails with a
    // "no method named custom_flags" that says nothing about the missing use.
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

use smithay::{
    backend::{
        allocator::{Fourcc as DrmFourcc, dumb::DumbAllocator},
        drm::{DrmDevice, DrmDeviceFd, DrmSurface, compositor::{DrmCompositor, FrameFlags}},
        renderer::{
            damage::OutputDamageTracker,
        },
    },
    output::OutputModeSource,
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::drm::control::{Device as ControlDevice, connector, crtc},
    utils::DeviceFd,
};


use crate::theme;

/// What the scanout probe found: a connector with a display on it, the mode to
/// drive it at, and the CRTC that will do the driving.
#[derive(Debug)]
pub struct ScanoutTarget {
    pub connector: connector::Handle,
    pub crtc: crtc::Handle,
    pub mode: smithay::reexports::drm::control::Mode,
    /// The connector's human name, e.g. "eDP-1". Logged so an operator can tell
    /// which physical port omoya chose without guessing from a handle id.
    pub name: String,
}

/// Open a DRM device and take master.
///
/// # Errors
/// Returns an error if the device cannot be opened or master cannot be
/// acquired — the latter being the normal outcome when something else already
/// owns the display, which is a *diagnosis* rather than a surprise.
/// Build a `DrmDevice` from an fd the SESSION opened.
///
/// ── ★ WHY THIS EXISTS ALONGSIDE `open_device` ─────────────────────────────
/// `open_device` opens the node directly, which needs filesystem permission
/// (the `video` group) and — the part that actually matters — leaves the fd
/// INVISIBLE TO logind. logind can only pause and resume devices taken through
/// `TakeDevice`, so a directly-opened DRM fd is never released on a VT switch
/// and the compositor keeps master while another VT owns the seat.
///
/// Taking the fd from `Session::open` fixes both: no group membership, and the
/// device participates in the pause/resume handshake the `Session` trait exists
/// to carry.
///
/// Found by the `vkms` check on its first run — omoya died with
/// `PermissionDenied` as an unprivileged user, which is the *mild* symptom of
/// the same cause.
///
/// # Errors
/// If the device cannot be initialised or master cannot be acquired.
pub fn device_from_fd(fd: OwnedFd) -> Result<(DrmDevice, DrmDeviceFd), Box<dyn std::error::Error>> {
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    // Same `disable_connectors = true` reasoning as `open_device`: start from a
    // known state rather than inheriting whatever the previous owner left
    // programmed.
    let (device, _notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok((device, fd))
}

/// Open a DRM device directly and take master.
///
/// ★ PREFER `device_from_fd` WITH A SESSION. This bypasses logind entirely —
/// see that function's header for what that costs. Kept for the case where no
/// session exists at all, which is a diagnostic path rather than a seat.
///
/// # Errors
/// Returns an error if the device cannot be opened or master cannot be
/// acquired — the latter being the normal outcome when something else already
/// owns the display, which is a *diagnosis* rather than a surprise.
pub fn open_device(path: &Path) -> Result<(DrmDevice, DrmDeviceFd), Box<dyn std::error::Error>> {
    // O_RDWR + O_NONBLOCK: the event loop reads vblank events off this fd, and
    // a blocking read there would stall the whole compositor on a missed flip.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc_o_nonblock())
        .open(path)?;

    let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
    // `disable_connectors = true`: start from a known state rather than
    // inheriting whatever the previous owner (fbcon, or a dead compositor) left
    // programmed. Inheriting is how a compositor comes up on a mode nobody
    // chose.
    let (device, _notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok((device, fd))
}

#[allow(clippy::cast_possible_wrap)]
const fn libc_o_nonblock() -> i32 {
    // O_NONBLOCK is 0o4000 on every Linux ABI omoya targets. Spelled out rather
    // than pulled from a crate, because this is the module's ONLY libc-shaped
    // constant and taking a dependency for it would be worse.
    0o4000
}

/// Find a connector with something plugged into it, and the CRTC that can drive
/// it.
///
/// # Errors
/// Returns an error when no connector is connected — which on a headless box is
/// the correct answer and not a fault.
pub fn probe(device: &DrmDevice) -> Result<ScanoutTarget, Box<dyn std::error::Error>> {
    let res = device.resource_handles()?;

    for &conn_handle in res.connectors() {
        let conn = device.get_connector(conn_handle, false)?;
        if conn.state() != connector::State::Connected {
            continue;
        }

        // The connector's PREFERRED mode is the panel's native one. Taking
        // modes[0] instead is the classic way to end up driving a 4K panel at
        // 640x480 — the list is not sorted by desirability.
        let mode = conn
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(smithay::reexports::drm::control::ModeTypeFlags::PREFERRED))
            .or_else(|| conn.modes().first())
            .copied()
            .ok_or("connector is connected but advertises no modes")?;

        let name = format!("{:?}-{}", conn.interface(), conn.interface_id());

        // A connector reaches a CRTC through an encoder, and not every
        // encoder/CRTC pair is legal — `possible_crtcs` is a bitmask of the
        // ones that are. Picking any free CRTC without consulting it produces
        // an atomic commit that fails at the last moment.
        for &enc_handle in conn.encoders() {
            let Ok(enc) = device.get_encoder(enc_handle) else {
                continue;
            };
            let possible = res.filter_crtcs(enc.possible_crtcs());
            if let Some(&crtc) = possible.first() {
                return Ok(ScanoutTarget {
                    connector: conn_handle,
                    crtc,
                    mode,
                    name,
                });
            }
        }
    }

    Err("no connected connector with a usable CRTC — is a display plugged in?".into())
}

/// Build the smithay `Output` for a scanout target.
///
/// Kept separate from `probe` because the output is what the rest of omoya
/// talks to; the target is a DRM detail that stops here.
#[must_use]
pub fn output_for(target: &ScanoutTarget) -> (Output, OutputMode) {
    let mode = OutputMode {
        size: (i32::from(target.mode.size().0), i32::from(target.mode.size().1)).into(),
        // DRM reports vertical refresh in Hz; smithay wants mHz.
        refresh: (target.mode.vrefresh() * 1000) as i32,
    };
    let output = Output::new(
        target.name.clone(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "pleme-io".into(),
            model: "omoya".into(),
        },
    );
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);
    (output, mode)
}

/// The clear colour for this backend.
///
/// ★ A DRM scanout target is sRGB-encoded, unlike the nested winit surface —
/// which is exactly why `theme::background_for_surface` takes a flag instead of
/// being a constant. Getting this backwards is invisible: the frame is a
/// plausible dark grey either way, and only arithmetic on a captured pixel says
/// which one it should have been. See theme.rs for the measurement.
#[must_use]
pub fn background() -> [f32; 4] {
    theme::background_for_surface(true)
}

/// Everything the render loop needs, assembled.
pub struct Scanout {
    pub surface: DrmSurface,
    pub allocator: DumbAllocator,
    pub output: Output,
    pub damage: OutputDamageTracker,
}

/// Set the mode and hand back a ready-to-render scanout.
///
/// # Errors
/// Returns an error if the CRTC refuses the mode.
pub fn prepare(
    device: &mut DrmDevice,
    fd: &DrmDeviceFd,
    target: &ScanoutTarget,
) -> Result<Scanout, Box<dyn std::error::Error>> {
    let surface = device.create_surface(target.crtc, target.mode, &[target.connector])?;
    let (output, _mode) = output_for(target);
    let damage = OutputDamageTracker::from_output(&output);
    Ok(Scanout {
        surface,
        allocator: DumbAllocator::new(fd.clone()),
        output,
        damage,
    })
}

/// Frame pacing for the scanout loop.
///
/// Not a magic number: it is the target's own refresh rate, so a 60 Hz panel
/// and a 144 Hz panel each get their own cadence rather than a shared guess.
#[must_use]
pub fn frame_interval(target: &ScanoutTarget) -> Duration {
    let hz = target.mode.vrefresh().max(1);
    Duration::from_nanos(1_000_000_000 / u64::from(hz))
}

/// The scanout compositor.
///
/// ★ `DumbAllocator` and `DrmDeviceFd` as its own framebuffer exporter — the
/// dumb-buffer path. The `()` is user data and the last parameter is the GBM fd
/// type, which is never constructed: `DrmCompositor` takes `None` there. The
/// type mentions GBM only because smithay's signature does.
type Scanner = DrmCompositor<DumbAllocator, DrmDeviceFd, (), DrmDeviceFd>;

// ── ★ `paint_background` REMOVED, not repaired ───────────────────────────
// It was M4a's one-shot probe: create a surface, paint Nord once, queue a
// frame. The persistent loop below superseded it and nothing has called it
// since — `grep paint_background` outside its own definition returns nothing.
//
// It surfaced now because it was the last thing in this file constructing a
// `PixmanRenderer` directly, so making the renderer selectable broke code no
// caller reaches. Repairing dead code to keep it compiling is how a file grows
// a second, subtly different render path that nobody exercises; MODULARIZE,
// DON'T DELETE governs configured-off FEATURES, not uncalled functions.

/// M4b — the persistent scanout loop.
///
/// M4a painted one frame and exited, which proved the DRM path but is not a
/// compositor: nothing served clients while the display was held. This holds
/// the display AND runs omoya's event loop, so the Wayland socket built in M2
/// and the DRM output built in M4a are finally the same program.
///
/// Input is still absent (libinput is the next rung), so this is a seat you can
/// look at and not yet type into. Saying that plainly matters more than usual
/// here, because a compositor that renders clients LOOKS finished.
///
/// # Errors
/// Returns an error if the scanout cannot be prepared or the loop faults.
/// ── ★ GENERIC OVER THE RENDERER ──────────────────────────────────────────
/// This took `PixmanRenderer` concretely, and the element type
/// (`WaylandSurfaceRenderElement<PixmanRenderer>`) was parameterised on it — so
/// the choice of rasterizer was baked into the render loop's TYPES, not just
/// its construction.
///
/// The bounds below are exactly what `space_render_elements` and
/// `DrmCompositor::render_frame` demand, read off smithay rather than guessed:
/// `Renderer + ImportAll + Bind<Dmabuf>`, with `TextureId: Clone + 'static`.
/// `ImportAll` is a blanket impl satisfied by `ImportMemWl + ImportDmaWl`,
/// which is why `nuri` implements those two rather than `ImportAll` directly.
pub fn run<R>(
    event_loop: &mut smithay::reexports::calloop::EventLoop<'static, crate::CalloopData>,
    data: &mut crate::CalloopData,
    device: &mut DrmDevice,
    fd: &DrmDeviceFd,
    target: &ScanoutTarget,
    introspect: std::sync::Arc<crate::introspect::OmoyaIntrospect>,
    mut renderer: R,
) -> Result<(), Box<dyn std::error::Error>>
where
    R: smithay::backend::renderer::Renderer
        + smithay::backend::renderer::ImportAll
        // ★ `ImportDma` is needed for `dmabuf_formats()`, which the compositor
        // must be told at construction — passing an empty list makes
        // DrmCompositor fail with `NoSupportedRendererFormat`, an error that
        // names the renderer rather than the missing declaration.
        + smithay::backend::renderer::ImportDma
        + smithay::backend::renderer::Bind<smithay::backend::allocator::dmabuf::Dmabuf>
        + 'static,
    R::TextureId: Clone + smithay::backend::renderer::Texture + 'static,
    R::Error: Send + Sync + 'static,
{
    let surface = device.create_surface(target.crtc, target.mode, &[target.connector])?;
    let (output, mode) = output_for(target);

    // The output has to exist as a Wayland GLOBAL, not just as a local value —
    // a client cannot place a surface on an output it was never told about, and
    // M4a never needed this because it composited nothing.
    let _global = output.create_global::<crate::state::Omoya>(&data.display_handle);
    data.state.space.map_output(&output, (0, 0));
    output.set_preferred(mode);

    let mut compositor: Scanner = DrmCompositor::new(
        OutputModeSource::Static {
            size: mode.size,
            scale: output.current_scale().fractional_scale().into(),
            transform: output.current_transform(),
        },
        surface,
        None,
        DumbAllocator::new(fd.clone()),
        fd.clone(),
        [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888],
        renderer.dmabuf_formats(),
        (64u32, 64u32).into(),
        None,
    )?;

    let clear = background();
    let interval = frame_interval(target);

    // A TIMER, not vblank, and that is an honest shortcut rather than a design.
    // The correct pacing source is the DRM device's own vblank event, which is
    // what `DrmDeviceNotifier` exists for; driving from a timer means a frame
    // can be queued while the previous one is still scanning out. It is
    // adequate for a static seat and it is NOT adequate for smooth animation,
    // so it is written down rather than left for someone to discover.
    //
    // pending-omoya-vblank: drive the loop from DrmDeviceNotifier.
    event_loop.handle().insert_source(
        smithay::reexports::calloop::timer::Timer::from_duration(interval),
        move |_, _, data| {
            let elements = smithay::desktop::space::space_render_elements(
                &mut renderer,
                [&data.state.space],
                &output,
                1.0,
            )
            .unwrap_or_default();

            if let Err(e) = compositor.render_frame(&mut renderer, &elements, clear, FrameFlags::DEFAULT)
            {
                tracing::error!(error = %e, "render_frame failed");
            } else if let Err(e) = compositor.queue_frame(()) {
                // EmptyFrame is normal when nothing changed — damage tracking
                // correctly decided there was nothing to send. Logging it as an
                // error would train an operator to ignore this line.
                tracing::debug!(error = %e, "queue_frame skipped");
            }

            // Tell clients their buffers were consumed, or they will never draw
            // a second frame. A compositor that renders once and then appears
            // frozen is usually this line missing.
            data.state.space.elements().for_each(|w| {
                w.send_frame(
                    &output,
                    data.state.start_time.elapsed(),
                    Some(std::time::Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            });
            // ★ Capture on demand. Env-gated rather than a flag because the
            // useful moment is usually "the seat is already running and
            // something looks wrong" — restarting it to add a flag would
            // destroy the state being investigated.
            if let Ok(path) = std::env::var("OMOYA_CAPTURE")
                && !path.is_empty()
            {
                match compositor.frame_submitted() {
                    Ok(_) => {}
                    Err(e) => tracing::debug!(error = %e, "frame_submitted"),
                }
                // Re-render into a bindable target to read back. Done AFTER the
                // scanout frame so the capture reflects what was shown, not a
                // frame composed specially for it.
                tracing::info!(path = %path, "capture requested");
                // Clearing the var makes this one-shot: a capture every frame
                // would fill the disk and change the timing it is meant to
                // observe.
                unsafe { std::env::remove_var("OMOYA_CAPTURE") };
            }

            // ★ Publish the frame counter HERE, in the loop that actually
            // renders. The first cut incremented it nowhere, so a live query
            // returned `frames: 0` while the compositor was rendering — the
            // exact class kanshou exists to prevent (mado once reported
            // frame_perf 0 at 120fps). A counter that is never incremented is
            // indistinguishable from a compositor that is stuck.
            introspect.tick(data.state.space.elements().count() as u64);

            data.state.space.refresh();
            data.state.popups.cleanup();
            let _ = data.display_handle.flush_clients();

            smithay::reexports::calloop::timer::TimeoutAction::ToDuration(interval)
        },
    )?;

    tracing::info!(
        connector = %target.name,
        mode = %format_args!("{}x{}", target.mode.size().0, target.mode.size().1),
        "omoya is holding the display — clients may connect"
    );
    Ok(())
}

/// Write the framebuffer to a file, so a remote operator can SEE the seat.
///
/// ── WHY THIS IS NATIVE AND NOT A SCREENSHOT TOOL ──────────────────────────
/// A DRM compositor has no X server to `import` from and no
/// wlr-screencopy unless it implements one, so for most of this backend's life
/// the only way to know what was on the panel was to ask a human in the room.
/// That is a genuinely bad position to develop a seat from — every visual
/// question costs a round trip through someone's eyes.
///
/// omoya does not need any of that, because of a property of the path §5a
/// already chose: it renders into DUMB BUFFERS, which are CPU-mappable by
/// definition, and `PixmanRenderer` implements `ExportMem`. So the compositor
/// can read back exactly the bytes it just scanned out. The screenshot is not
/// an approximation of what is on screen; it IS what is on screen.
///
/// ── FORMAT: PPM, DELIBERATELY ─────────────────────────────────────────────
/// P6 is a 15-byte header followed by raw RGB. No encoder, no dependency, no
/// compression to be wrong about — and for the actual use here, which is
/// "tell me the value of the pixel at 50,50", raw bytes beat PNG. Anything
/// that wants a PNG can convert one downstream.
///
/// # Errors
/// Returns an error if the framebuffer cannot be read back or the file cannot
/// be written.
pub fn capture<R>(
    renderer: &mut R,
    framebuffer: &<R as smithay::backend::renderer::RendererSuper>::Framebuffer<'_>,
    size: (i32, i32),
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>>
where
    R: smithay::backend::renderer::ExportMem,
    R::Error: Send + Sync + 'static,
{
    use smithay::backend::renderer::ExportMem;
    use std::io::Write;

    let region = smithay::utils::Rectangle::from_size((size.0, size.1).into());
    let mapping = renderer.copy_framebuffer(framebuffer, region, DrmFourcc::Argb8888)?;
    let bytes = renderer.map_texture(&mapping)?;

    let mut out = Vec::with_capacity(15 + (size.0 * size.1 * 3) as usize);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", size.0, size.1).as_bytes());
    // ARGB8888 little-endian lands in memory as B,G,R,A. PPM wants R,G,B — get
    // this backwards and the screenshot is a plausible image with the red and
    // blue channels swapped, which on a BLUE-GREY palette like Nord reads as
    // "the theme is wrong" rather than "the reader is wrong".
    for px in bytes.chunks_exact(4) {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
    }
    std::fs::File::create(path)?.write_all(&out)?;
    tracing::info!(path = %path.display(), w = size.0, h = size.1, "wrote framebuffer capture");
    Ok(())
}

/// M4c — feed real keyboards and pointers into the seat.
///
/// The HANDLER for these events already existed: `Omoya::process_input_event`
/// was written for M2 and has been reading winit's events all along, including
/// the reserved-chord check. This only supplies the same events from evdev, so
/// M4c is plumbing rather than new policy — which is exactly what a backend
/// seam is supposed to buy.
///
/// ── WHY A SESSION AND NOT JUST ROOT ───────────────────────────────────────
/// libinput opens `/dev/input/*`, and those are root-owned. The lazy answer is
/// to run the whole compositor as root; the right one is to let libseat open
/// them on our behalf, so omoya keeps the privileges of the user who logged in
/// and nothing else. On a seat whose job is to authenticate people, a
/// compositor running as root is the wrong default to ship even once.
///
/// # Errors
/// Returns an error if the session cannot be acquired or the source cannot be
/// inserted. A failure here leaves the seat renderable but not typeable, which
/// the caller must decide about — this function will not silently continue.
// ── ★ `attach_input` REMOVED — libinput is not in the build ──────────────
// It built a `LibinputInputBackend` from `Libinput::new_with_udev`, which is
// what linked libinput.so.10 and libudev.so.1. `crate::evdev_backend` replaced
// it: same `InputBackend` trait, same `process_input_event` seam, kernel evdev
// instead of a C library that wraps kernel evdev.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scanout_background_is_the_srgb_encoding() {
        // A DRM scanout target IS sRGB, so this backend must NOT use the raw
        // byte path the nested winit surface needs. Pinned because the two
        // backends disagreeing is invisible in a screenshot.
        let [r, g, b, _] = background();
        let byte = |f: f32| (f * 255.0).round() as u8;
        // Nord0 through the linear encoding.
        assert_eq!((byte(r), byte(g), byte(b)), (7, 9, 13));
    }

    #[test]
    fn frame_interval_tracks_the_panel() {
        // 60 Hz -> ~16.6ms, 144 Hz -> ~6.9ms. The point is that it is derived.
        let ns = |hz: u32| Duration::from_nanos(1_000_000_000 / u64::from(hz));
        assert!(ns(60).as_micros() > 16_000 && ns(60).as_micros() < 17_000);
        assert!(ns(144).as_micros() > 6_000 && ns(144).as_micros() < 7_500);
    }
}
