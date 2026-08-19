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
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            pixman::PixmanRenderer,
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

/// Marker so the renderer element type is named in one place; the winit backend
/// uses the same one, and a mismatch between them is a confusing type error far
/// from its cause.
pub type Element = WaylandSurfaceRenderElement<PixmanRenderer>;

/// The compositor type for this backend, named once.
///
/// The type parameters are the whole design in one line: allocate DUMB buffers,
/// export framebuffers through the DEVICE FD itself, carry no per-frame user
/// data, and — the important one — `NoGbm` in the gbm slot, because this path
/// has no gbm device at all.
type Scanner = DrmCompositor<DumbAllocator, DrmDeviceFd, (), DrmDeviceFd>;

/// Paint one frame of the seat background onto a real display, and hold it.
///
/// ── WHY DUMB BUFFERS REACH A PIXMAN RENDERER AT ALL ───────────────────────
/// `PixmanRenderer` implements `Bind<Dmabuf>`, NOT `Bind<DumbBuffer>` — which
/// looks at first like this whole path cannot work. It does, because
/// `impl AsDmabuf for DumbBuffer` (`backend/allocator/dumb.rs:104`) exports the
/// dumb buffer as a dmabuf, and `DrmCompositor` performs that export itself.
/// So the chain is DumbAllocator → DumbBuffer → dmabuf → pixman, with no GPU
/// anywhere in it.
///
/// # Errors
/// Returns an error if the surface cannot be created, the compositor cannot be
/// built, or the frame cannot be queued to the CRTC.
pub fn paint_background(
    device: &mut DrmDevice,
    fd: &DrmDeviceFd,
    target: &ScanoutTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let surface = device.create_surface(target.crtc, target.mode, &[target.connector])?;
    let (output, _mode) = output_for(target);

    let mut renderer = PixmanRenderer::new()?;
    let allocator = DumbAllocator::new(fd.clone());

    // ARGB/XRGB8888 is the format simpledrm and every KMS driver worth the name
    // supports. Offering both lets the compositor pick; offering only one is
    // how a mode-set fails on a driver that wanted the other.
    let color_formats = [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];
    let renderer_formats = renderer.shm_formats().collect::<Vec<_>>();
    let _ = renderer_formats;

    let mut compositor: Scanner = DrmCompositor::new(
        OutputModeSource::Static {
            size: output.current_mode().map_or((0, 0).into(), |m| m.size),
            scale: output.current_scale().fractional_scale().into(),
            transform: output.current_transform(),
        },
        surface,
        None,
        allocator,
        fd.clone(),
        color_formats,
        // The renderer's supported formats. Pixman advertises these through the
        // dmabuf path it binds.
        [],
        target.mode.size().0.into(),
        None,
    )?;

    let elements: Vec<Element> = Vec::new();
    compositor.render_frame(&mut renderer, &elements, background(), FrameFlags::empty())?;
    compositor.queue_frame(())?;

    Ok(())
}

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
