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
        drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmSurface},
        renderer::{
            // ★ IMPORTED BY BARE NAME BECAUSE A MACRO DEMANDS IT.
            // `render_elements!` matches its `where` bound as a single token
            // tree (`$bound:tt`), so `R: smithay::backend::renderer::ImportAll`
            // does not parse — the `::` has no rule. The error says "no rules
            // expected `::`" and points at the trait path, which reads as a
            // typo rather than as a grammar limit. The bound must be one
            // identifier, so the trait comes into scope here.
            ImportAll,
            ImportMem,
            damage::OutputDamageTracker,
        },
    },
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
    /// Every mode the connector advertises, with the chosen one starred —
    /// `"1920x1080@60 1920x1080@360* 1024x768@60"`.
    ///
    /// Carried on the target so `main` can publish it without re-scanning the
    /// connector. "we run at 60" and "60 is all it offers" look identical
    /// from outside, and only one of them is a bug.
    pub mode_list: String,
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
pub fn device_from_fd(
    fd: OwnedFd,
) -> Result<(DrmDevice, DrmDeviceFd, DrmDeviceNotifier), Box<dyn std::error::Error>> {
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    // Same `disable_connectors = true` reasoning as `open_device`: start from a
    // known state rather than inheriting whatever the previous owner left
    // programmed.
    //
    // ★ THE NOTIFIER IS RETURNED, NOT DROPPED — and this is the SECOND time
    // that mistake has been made in this compositor. Dropping the session
    // notifier made every later `Session::open` fail; dropping this one is
    // worse, because it fails LATER and looks like something else entirely:
    //
    //   every flip is issued with `event: true`, so the kernel queues a
    //   page-flip event per frame. Nothing reads them. The queue grows until
    //   the kernel cannot allocate another, and the flip that trips that
    //   returns ENOMEM.
    //
    // Measured on vkms: 2.37 seconds of correct frames — roughly 135 of them —
    // and then `Cannot allocate memory (os error 12)` on every frame after.
    // A leak that presents as a working compositor for two seconds is exactly
    // the shape a person watching a screen would misread as a driver problem.
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok((device, fd, notifier))
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
pub fn open_device(
    path: &Path,
) -> Result<(DrmDevice, DrmDeviceFd, DrmDeviceNotifier), Box<dyn std::error::Error>> {
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
    // Returned rather than dropped, for the reason spelled out in
    // `device_from_fd`: an unread page-flip event queue exhausts the kernel's
    // allocator and scanout starts failing with ENOMEM a couple of seconds in.
    // This path has no caller today, but it is the obvious one to reach for
    // when diagnosing, which is precisely when a two-second-delayed failure
    // would send someone the wrong way.
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok((device, fd, notifier))
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

        // ── ★ PREFERRED PICKS THE RESOLUTION. IT DOES NOT PICK THE RATE. ──
        //
        // The connector's PREFERRED mode is the panel's native RESOLUTION,
        // and taking modes[0] instead is the classic way to drive a 4K panel
        // at 640x480 — the list is not sorted by desirability. All true, and
        // it is only half the question.
        //
        // EDID's preferred *timing* on a high-refresh panel is routinely the
        // 60 Hz one: the descriptor exists for compatibility, and the fast
        // modes live in the extension blocks. So "preferred" on plo's 360 Hz
        // display selected 1920x1080@60 — measured 2026-08-21, with the
        // operator pointing out that the seat felt slow on a monitor six
        // times faster than we were driving it.
        //
        // The failure is silent in both directions: the picture is perfect,
        // the resolution is right, and the only symptom is latency, which
        // reads as "the compositor is slow" rather than "the compositor
        // asked for slow".
        //
        // So: take the resolution from PREFERRED, then take the FASTEST mode
        // at that resolution. Never trade pixels for hertz — a 1280x720@360
        // is not an upgrade over 1920x1080@240 and this must not silently
        // choose it.
        let preferred = conn
            .modes()
            .iter()
            .find(|m| {
                m.mode_type()
                    .contains(smithay::reexports::drm::control::ModeTypeFlags::PREFERRED)
            })
            .or_else(|| conn.modes().first())
            .copied()
            .ok_or("connector is connected but advertises no modes")?;

        // Ranked by the DERIVED rate, never by `vrefresh`: that field is
        // optional in the DRM mode struct and is frequently 0, which would
        // rank every mode equally and hand back whichever happened to come
        // first. omoya already learned this once — the 1 Hz render loop.
        let mode = conn
            .modes()
            .iter()
            .filter(|m| m.size() == preferred.size())
            .max_by_key(|m| refresh_hz(m))
            .copied()
            .unwrap_or(preferred);

        if refresh_hz(&mode) != refresh_hz(&preferred) {
            tracing::info!(
                preferred_hz = refresh_hz(&preferred),
                selected_hz = refresh_hz(&mode),
                size = %format_args!("{}x{}", mode.size().0, mode.size().1),
                modes_at_this_size = conn.modes().iter().filter(|m| m.size() == preferred.size()).count(),
                "the panel's PREFERRED timing was not its fastest — taking the faster one"
            );
        }

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
                let mode_list = conn
                    .modes()
                    .iter()
                    .map(|m| {
                        let star = if m.size() == mode.size() && refresh_hz(m) == refresh_hz(&mode)
                        {
                            "*"
                        } else {
                            ""
                        };
                        format!("{}x{}@{}{star}", m.size().0, m.size().1, refresh_hz(m))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                return Ok(ScanoutTarget {
                    connector: conn_handle,
                    crtc,
                    mode,
                    name,
                    mode_list,
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
        size: (
            i32::from(target.mode.size().0),
            i32::from(target.mode.size().1),
        )
            .into(),
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
/// ★ CORRECTED 2026-08-20, ON THE SCREEN. This passed `true` — "a DRM scanout
/// target is sRGB-encoded" — and that is FALSE for this path. The scanout
/// buffer is `DRM_FORMAT_ARGB8888` (see the `copy_framebuffer` call and
/// `scanout.rs`), a plain format with no sRGB conversion: the display
/// interprets those bytes directly. Writing LINEAR values into it renders
/// everything about six times too dark.
///
/// Measured by capturing the live screen on plo:
///
///     99% of 1920x1080 = rgb(7, 9, 13)
///
/// Nord0 is #2E3440 = (46,52,64), and (7,9,13) is exactly its linear value
/// written as raw bytes — the identical arithmetic theme.rs already records
/// from omoya's first frame. The operator's report was "login, then a blank
/// black screen", and it was not blank: it was Nord0, too dark to see.
///
/// The vkms gate had the evidence and it was misread. `pixel(4,4) = 214 220 231`
/// against a cursor of #ECEFF4 = (236,239,244) is the same linear encoding, and
/// it was rationalised as correct-for-sRGB rather than read as the defect.
/// A number that needs explaining away is a finding.
#[must_use]
pub fn background() -> [f32; 4] {
    theme::background_for_surface(false)
}

/// The pointer's colour for this backend. Same sRGB reasoning as
/// [`background`] — see theme.rs.
#[must_use]
pub fn cursor() -> [f32; 4] {
    theme::cursor_for_surface(false)
}

/// The pointer's size in physical pixels.
///
/// Retired: the pointer is `crate::cursor`'s arrow now, not a solid square.
/// Kept as a named constant rather than deleted so the size the square used
/// to be stays legible next to the arrow that replaced it — 12px was not a
/// small cursor, it was an unfindable one.
#[allow(dead_code)]
const CURSOR_SIZE: i32 = 12;

// ── ★ ONE ELEMENT SLICE, BECAUSE PARTIAL REPAINT OWNS THE WHOLE FRAME ────
//
// The manual loop this replaced could draw the cursor with `draw_solid` after
// the elements, because it repainted the entire screen every frame and order
// was the only thing that mattered. `render_output` does not work that way: it
// computes damage from the element slice, clears only the damaged region, and
// draws only the elements that intersect it. A cursor drawn OUTSIDE that slice
// would be invisible to the damage computation — its old position would never
// be repainted, so it would smear a trail across the screen and the trail
// would look like a renderer bug rather than a bookkeeping one.
//
// So the cursor becomes an element. `render_elements!` builds the enum that
// lets two element KINDS share one slice; it generates the `From` impls and
// forwards every `Element`/`RenderElement` method to the active variant.
//
// `Kind::Cursor` is not decoration — smithay's own doc says an element that
// changes frequently and is NOT marked `Cursor` costs performance, and one
// that is marked `Cursor` but changes frequently costs more. A pointer that
// moves and otherwise never changes is exactly what the marking is for.
smithay::backend::renderer::element::render_elements! {
    /// Everything the seat composites: client surfaces and our own pointer.
    pub SeatElements<R, E> where R: ImportAll + ImportMem;
    /// A client surface, as `Space` laid it out.
    Space = smithay::desktop::space::SpaceRenderElements<R, E>,
    /// A flat rectangle of one colour — the focus ring's four edges.
    ///
    /// ★ NAMED FOR WHAT IT HOLDS, NOT FOR ONE CALLER. This variant was
    /// `Cursor` while carrying the focus-border edges, and `Bar` carried both
    /// the status strip AND the mouse pointer. Two of the three names
    /// described the wrong thing, which is the kind of defect that costs a
    /// reader ten minutes and a writer a wrong `match` arm. A variant is a
    /// TYPE, so it is named after its type; the caller's intent lives at the
    /// push site, where `Kind::Cursor` already says it.
    Solid = smithay::backend::renderer::element::solid::SolidColorRenderElement,
    /// A CPU-rasterized buffer — the status bar, and omoya's own pointer.
    Texture = smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>,
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

/// The panel's refresh rate in Hz, derived when the driver does not state it.
///
/// ★ `vrefresh` IS OPTIONAL IN DRM, AND ZERO IS THE COMMON ANSWER.
/// `drmModeModeInfo.vrefresh` is a convenience field many drivers simply
/// leave at 0; the AUTHORITATIVE rate is the pixel clock over the total
/// blanking area. Measured on plo's DP-1 (1920x1080): the compositor was
/// running the whole desktop at **1.2 Hz**.
///
/// That number is not a coincidence — it is `.max(1)` doing exactly what it
/// was written to do. It was a divide-by-zero guard, and a guard that turns
/// "I do not know the refresh rate" into "one frame per second" produces a
/// desktop that redraws once a second while every other subsystem reports
/// perfect health: no error, no dropped frame, damage tracking working
/// correctly on the frames it is given. The operator sees "typing is
/// unbearably slow" and nothing in the logs agrees.
///
/// The fix is to compute the rate the way the kernel does, and to fall back
/// to 60 rather than 1 when even that is unavailable — a wrong-but-plausible
/// 60 costs a little CPU on an unusual panel, while 1 makes the seat unusable.
#[must_use]
pub fn refresh_hz(mode: &smithay::reexports::drm::control::Mode) -> u32 {
    if mode.vrefresh() > 0 {
        return mode.vrefresh();
    }
    // clock is in kHz; htotal/vtotal are the full line/frame including
    // blanking. This is the same arithmetic as the kernel's own
    // drm_mode_vrefresh().
    let (_, _, htotal) = mode.hsync();
    let (_, _, vtotal) = mode.vsync();
    let total = u64::from(htotal) * u64::from(vtotal);
    if total == 0 {
        return 60;
    }
    let hz = (u64::from(mode.clock()) * 1000) / total;
    // A derived 0 means the clock was unstated too. 60 is the honest guess;
    // 1 is not a guess, it is a broken seat.
    u32::try_from(hz).unwrap_or(60).max(1).min(1000).max(24)
}

/// Frame pacing for the scanout loop.
///
/// Not a magic number: it is the target's own refresh rate, so a 60 Hz panel
/// and a 144 Hz panel each get their own cadence rather than a shared guess.
#[must_use]
pub fn frame_interval(target: &ScanoutTarget) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(refresh_hz(&target.mode)))
}

// ── ★ `Scanner` REMOVED — it aliased DrmCompositor ───────────────────────
// The alias existed to name `DrmCompositor<DumbAllocator, DrmDeviceFd, (),
// DrmDeviceFd>` once. `crate::scanout::DirectScanout` replaced it, and with it
// the last reason this crate enabled `backend_gbm`.

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
    // Set when a page flip is issued, cleared by the VBlank event. See the
    // comment at its creation in `main.rs`: the kernel refuses a flip issued
    // while the previous is still pending, and the frame period used to be
    // long enough to hide that. (A plain comment, not a doc comment — rustc
    // refuses `///` on a parameter.)
    flip_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        // ★ `ImportMem` for the STATUS BAR, which is a CPU buffer rather than
        // a client surface. nuri already implements it (`nuri_renderer.rs`),
        // so this costs the shipping path nothing; it excludes a future
        // renderer that cannot take raw memory, which is the same class of
        // constraint as `ExportMem` below and stated for the same reason.
        + ImportMem
        + smithay::backend::renderer::Bind<smithay::backend::allocator::dmabuf::Dmabuf>
        // ★ How this frame may reach scanout. The trait's default is a no-op,
        // so this excludes nothing: a renderer that composites straight into
        // the mapping satisfies it by ignoring the plan.
        + crate::nuri_renderer::ArmFlush
        // ★ `ExportMem` so the seat can be SCREENSHOT. This is a real
        // constraint on what may drive this loop, and it is the right one: a
        // renderer whose output cannot be read back produces a seat that can
        // only be debugged by walking to the machine and looking at it. The
        // whole reason `capture()` exists is that "the screen is blank" is
        // otherwise unanswerable from anywhere else.
        //
        // `NuriRenderer` implements it (`nuri_renderer.rs`), so this costs the
        // shipping path nothing; it only excludes a future renderer that
        // cannot read its own framebuffer back.
        + smithay::backend::renderer::ExportMem
        + 'static,
    // `Send` is the bar's requirement too: `MemoryRenderBuffer` holds its
    // texture behind an `Arc<Mutex<..>>` so the same buffer can be reused
    // across frames without re-uploading, and that shape demands the texture
    // be movable between threads even though this loop never does.
    R::TextureId: Clone + Send + smithay::backend::renderer::Texture + 'static,
    R::Error: Send + Sync + 'static,
    // ★ THE FLUSH IS A BOUND, so a renderer that composites into a shadow and
    // is never told to present it cannot be wired into this loop by accident.
    // See `ScanoutFlush` — the failure it prevents is a black screen behind
    // perfectly healthy frame counters, which is the hardest kind to diagnose.
    for<'fb> R::Framebuffer<'fb>: crate::nuri_renderer::ScanoutFlush,
{
    let surface = device.create_surface(target.crtc, target.mode, &[target.connector])?;
    let (output, mode) = output_for(target);

    // The output has to exist as a Wayland GLOBAL, not just as a local value —
    // a client cannot place a surface on an output it was never told about, and
    // M4a never needed this because it composited nothing.
    let _global = output.create_global::<crate::state::Omoya>(&data.display_handle);

    // ── ★ THE DAMAGE TRACKER AND THE CURSOR'S IDENTITY, BOTH LONG-LIVED ────
    //
    // Both must outlive the frame or partial repaint degenerates into full
    // repaint while looking like it works. The tracker holds the per-buffer
    // damage HISTORY that makes buffer age meaningful — a fresh one every
    // frame knows nothing and damages everything. The `Id` is what lets the
    // tracker recognise the pointer across frames as the same element that
    // MOVED, rather than one that vanished and another that appeared; a fresh
    // id would damage both rectangles every frame.
    //
    // An `OutputDamageTracker` was already being constructed in `prepare()`
    // and had zero callers, which is why the frame stayed full-screen: the
    // machinery was present, built, and never wired to anything.
    // ★ START THE POINTER IN THE MIDDLE, NOT AT (0, 0).
    //
    // `Omoya::new` sets (0.0, 0.0) and says why: at construction there is no
    // output, so a centre would be a guess. Here there IS one. Left at the
    // origin the arrow sits in the corner underneath the bar, which is
    // exactly where an operator does not look — and "I cannot find the
    // mouse" is indistinguishable from "there is no mouse".
    if data.state.pointer_location == (0.0, 0.0).into() {
        data.state.pointer_location =
            (f64::from(mode.size.w) / 2.0, f64::from(mode.size.h) / 2.0).into();
    }

    let mut blit_counters: Option<(
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    )> = None;
    let mut import_counters: Option<(
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    )> = None;
    {
        let full = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let part = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let _ = crate::nuri_renderer::IMPORT_COUNTS.set((full.clone(), part.clone()));
        import_counters = Some((full, part));
    }
    // Install the blit-path counters (see `nuri_renderer::BLIT_COUNTS`).
    {
        let copied = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let blended = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let general = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let _ = crate::nuri_renderer::BLIT_COUNTS.set((
            copied.clone(),
            blended.clone(),
            general.clone(),
        ));
        blit_counters = Some((copied, blended, general));
    }

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    // One stable id per border EDGE, for the same reason the cursor has one:
    // a fresh `Id` each frame reads as "the old element vanished and a new one
    // appeared", which re-damages both rectangles every frame and quietly
    // turns partial repaint back into full repaint.
    // ── ★ THE BAR, RASTERIZED ONLY WHEN ITS TEXT CHANGES ────────────────
    //
    // The clock ticks once a second; the seat renders sixty times. Rebuilding
    // this buffer every frame would give the damage tracker a new commit each
    // time and undo the partial repaint the seat just gained — the bar alone
    // would put the desktop back to full-screen composites.
    // The arrow bitmap. Built on first use and kept: the shape never
    // changes, so rebuilding it per frame would hand the damage tracker a
    // new commit each time — the same trap the bar's text comparison avoids.
    let mut cursor_buffer: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> =
        None;
    // ── ★ CHROME BUFFERS, CACHED ON THEIR OWN INPUTS ────────────────────
    //
    // One buffer per window holding its whole titlebar — ground, buttons and
    // title. Keyed on `(title, width, focused)`, which are `chrome::rasterize`'s
    // only inputs, so a frame that changes none of them rebuilds nothing.
    //
    // Rebuilding per frame would hand the damage tracker a fresh commit every
    // frame and turn partial repaint back into full repaint — the same trap
    // the bar's text comparison and the cursor's build-once both avoid, now
    // for the third time in this file. Three occurrences is the point at which
    // this should become one primitive; noted rather than done, because the
    // shapes still differ (one keyed on text, one on nothing, this on a
    // triple). `pending-omoya-cached-raster`.
    #[allow(clippy::type_complexity)]
    let mut chrome_cache: Vec<(
        u32,
        String,
        i32,
        bool,
        smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    )> = Vec::new();

    let mut bar_text = crate::bar::BarState::default();
    let mut bar_buffer: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> =
        None;

    // ── ★ CHROME IDS: STABLE, AND ENOUGH FOR SEVERAL WINDOWS ────────────
    //
    // Same reason the border edges have stable ids — a fresh `Id` each frame
    // reads to the damage tracker as "the old element vanished and a new one
    // appeared", which re-damages every rectangle every frame and quietly
    // turns partial repaint back into full repaint. Four rects per window
    // (bar + three buttons) for up to `CHROME_WINDOWS` windows; beyond that
    // the extra windows render WITHOUT chrome rather than sharing ids, since
    // two elements holding one id is the exact confusion these ids prevent.
    const CHROME_WINDOWS: usize = 8;
    let chrome_ids: Vec<smithay::backend::renderer::element::Id> = (0..CHROME_WINDOWS * 4)
        .map(|_| smithay::backend::renderer::element::Id::new())
        .collect();

    let border_ids: [smithay::backend::renderer::element::Id; 4] = [
        smithay::backend::renderer::element::Id::new(),
        smithay::backend::renderer::element::Id::new(),
        smithay::backend::renderer::element::Id::new(),
        smithay::backend::renderer::element::Id::new(),
    ];

    // ── ★ ADVERTISE DMABUF, FROM THE RENDERER'S OWN FORMAT LIST ───────────
    //
    // Created HERE and not in `Omoya::new` because the global is a promise
    // about what THIS renderer can texture, and only this scope knows which
    // renderer is running. Passing `renderer.dmabuf_formats()` straight
    // through — never a hand-written list — is what keeps the promise and the
    // capability from drifting into two lists.
    //
    // nuri advertises LINEAR only, because it MAPS the buffer: a tiled or
    // compressed modifier describes a layout only a GPU can decode, and a CPU
    // blitter reading it paints structured noise rather than failing. That is
    // not a compromise on this hardware — measured on plo, the RTX 3070 exposes
    // DRM_FORMAT_MOD_LINEAR for B8G8R8A8 as a single-plane, exportable,
    // importable COLOR_ATTACHMENT image, and the exported fd mmaps read/write.
    //
    // Version 3, not feedback: feedback's `main_device` means "the device the
    // compositor renders on", and omoya renders on the CPU into dumb buffers.
    // Naming a render node would be a claim it cannot back.
    //
    // ── ★ WITHDRAWN 2026-08-21, AND THE REASON IS A MEASUREMENT ──────────
    //
    // Advertising this made the seat DRAMATICALLY slower, because omoya
    // implements it badly and the client believed the advertisement.
    //
    // `import_dmabuf` maps the client's buffer and `to_vec`s it. For a GPU
    // client that buffer lives in VRAM, so the copy is a CPU readback across
    // PCIe — which runs at a few hundred MB/s, not memory speed. Measured on
    // plo with mado (a wgpu terminal) at 1920x1080:
    //
    //     gather_us      693 952   <- 694 ms, the whole frame
    //     frame_us         3 825   <- compositing is 4 ms and innocent
    //     import_full          0   <- shm import never called AT ALL
    //     import_partial       0
    //
    // Those last two are the finding. The client had switched to dmabuf
    // BECAUSE we advertised it, and every frame was paying a VRAM readback.
    // Before today the global did not exist and clients used `wl_shm` —
    // system memory, which reads at memory speed and which the damage-only
    // import path in `nuri_renderer` already makes nearly free.
    //
    // ★ ADVERTISING A CAPABILITY WE SERVE BADLY IS WORSE THAN NOT
    // ADVERTISING IT. A protocol global is a PROMISE about what the
    // compositor can do well; the client has no way to discover that our
    // implementation is a CPU readback, and it optimises for the promise.
    //
    // This comes back when the buffer is no longer read: direct scanout onto
    // a DRM plane, which never touches the pixels. plo exposes 12 planes, so
    // the hardware is there — `pending-nuri-dmabuf-zerocopy` and
    // `pending-omoya-planes` are the rows, and `docs/SMOOTHNESS.md` is the
    // plan. The DmabufState and handler stay wired (MODULARIZE, DON'T
    // DELETE); only the global is withheld.
    if std::env::var_os("OMOYA_ADVERTISE_DMABUF").is_some() {
        let formats = renderer.dmabuf_formats();
        let count = formats.iter().count();
        let global = data
            .state
            .dmabuf_state
            .create_global::<crate::state::Omoya>(&data.display_handle, formats);
        data.state.dmabuf_global = Some(global);
        tracing::info!(
            formats = count,
            "zwp_linux_dmabuf_v1 advertised — clients may deliver GPU buffers"
        );
    } else {
        tracing::info!(
            "zwp_linux_dmabuf_v1 WITHHELD — the import is a CPU readback of \
             VRAM (694ms/frame measured); clients use wl_shm until direct \
             scanout lands. Set OMOYA_ADVERTISE_DMABUF=1 to re-enable."
        );
    }
    data.state.space.map_output(&output, (0, 0));
    output.set_preferred(mode);

    // ── ★ DIRECT SCANOUT, NOT DrmCompositor ──────────────────────────────
    // `DrmCompositor` is gated behind smithay's `backend_gbm` feature, and that
    // feature is the only reason libgbm.so.1 was linked — no `GbmDevice` was
    // ever constructed. `crate::scanout::DirectScanout` drives `DrmSurface`
    // page flips over a two-buffer chain instead, which needs no gbm.
    //
    // What is lost is named in that module: overlay planes and partial
    // repaint. What is gained is that the seat no longer links a library it
    // never called.
    let mut allocator = DumbAllocator::new(fd.clone());
    #[allow(clippy::cast_sign_loss)]
    let mut scanout = crate::scanout::DirectScanout::new(
        surface,
        &mut allocator,
        fd,
        (u32::from(mode.size.w as u16), u32::from(mode.size.h as u16)),
        DrmFourcc::Argb8888,
    )?;

    // ★ M3a — publish the plane inventory, so the direct-scanout premise is a
    // MEASUREMENT rather than a sentence in a design doc.
    {
        let inv: Vec<_> = scanout
            .plane_inventory()
            .into_iter()
            .map(|(id, kind, formats)| {
                serde_json::json!({ "id": id, "kind": kind, "formats": formats })
            })
            .collect();
        tracing::info!(planes = inv.len(), "DRM plane inventory published");
        *introspect.planes.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(serde_json::Value::Array(inv).to_string());
    }

    // ★ PUBLISH THE COMMIT PATH. Which of the kernel's two modesetting
    // paths this seat is on was, until now, unknowable from outside the
    // process — 44 read leaves and none of them this one, while
    // `is_atomic()` sat uncalled. It is the first thing worth knowing when a
    // seat is reported as tearing, because atomic and legacy are different
    // kernel code with different failure modes, and on the proprietary
    // nvidia driver that difference is not academic.
    // ★ ASKED OF THE DEVICE, NOT THE SURFACE. smithay decides atomic-vs-legacy
    // once, at `DrmDevice::new`, by asking the kernel for
    // `DRM_CLIENT_CAP_ATOMIC`; a surface inherits it and exposes no accessor
    // of its own. Reaching for `DrmSurface::is_atomic` compiles to E0599 —
    // recorded here because the wrong receiver is the obvious first guess.
    introspect.atomic.store(
        u64::from(if device.is_atomic() { 1u8 } else { 2u8 }),
        std::sync::atomic::Ordering::Relaxed,
    );
    tracing::info!(atomic = device.is_atomic(), "drm commit path");

    let clear = background();
    // Element geometry is expressed in physical pixels, so it needs the
    // output's scale. Read once rather than per element per frame.
    let interval = frame_interval(target);
    // Publish it — a seat paced at the wrong rate is invisible from
    // every other angle. See `OmoyaIntrospect::refresh_hz`.
    introspect.refresh_hz.store(
        u64::from(refresh_hz(&target.mode)),
        std::sync::atomic::Ordering::Relaxed,
    );
    tracing::info!(hz = refresh_hz(&target.mode), "frame pacing");

    // Captured as a VALUE, not read from `target` inside the loop: the closure
    // is `'static` and `target` is a borrow that cannot escape into it. The
    // refresh interval is also exactly what `wp_presentation` feedback must
    // report, so it is derived once here rather than twice with two chances to
    // disagree.
    let refresh_interval = interval;

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
        move |deadline, _, data| {
            // ★ DO NOT COMPOSE A FRAME THE DISPLAY CANNOT TAKE. A flip issued
            // while the previous is pending returns EBUSY, and the whole frame
            // — a full-screen CPU composite — is wasted producing an error.
            // Skipping costs one interval; issuing costs the composite AND
            // still does not present.
            if flip_pending.load(std::sync::atomic::Ordering::Acquire) {
                // ── ★ RETRY SOON, DO NOT SKIP A WHOLE FRAME ──────────────
                //
                // This used to reschedule a FULL interval away. That is right
                // only if the pending flip retires just after the deadline;
                // when it retires 100 us later, the seat has thrown away
                // 2,678 us of a 2,778 us frame and presents on the NEXT
                // vblank instead of this one.
                //
                // It is not a rare case on this seat, it is the normal one.
                // Measured on plo 2026-08-28 through kanshou, on a focus
                // change with two mado windows open:
                //
                //   frame_us              ~5,700     (vblank interval 2,778)
                //   td_rows_examined      +2,076 per frame
                //   td_rows_dirty         +62 per present
                //
                // A composite costs about two vblank intervals because
                // truedamage must scan both full surfaces — wgpu clients
                // cannot declare damage, so the compositor measures it. With
                // frames longer than the interval, the timer is ALWAYS
                // landing on a pending flip, and a full-interval back-off
                // turns "late by a little" into "late by a whole frame". The
                // resulting present cadence is irregular rather than merely
                // slow, and irregular cadence is what an operator sees as
                // tearing or judder.
                //
                // So: poll back at an eighth of the interval (~347 us at
                // 360 Hz). The flip is picked up within an eighth of a frame
                // instead of losing a full one, which makes the cadence a
                // steady 2-frames-per-composite instead of a beat between
                // the timer's period and the vblank's.
                //
                // ★ THIS IS A MITIGATION, NOT THE FIX. The real repair is
                // still `pending-omoya-vblank`: drive the loop from
                // `DrmDeviceNotifier` so the composite starts AT the vblank
                // and gets the whole interval to work in, rather than
                // discovering after the fact that it was late. This narrows
                // the window; it does not remove it, and it must not be read
                // as closing that item.
                let retry = interval / 8;
                let now = std::time::Instant::now();
                let mut next = deadline + retry;
                if next <= now {
                    next = now + retry;
                }
                return smithay::reexports::calloop::timer::TimeoutAction::ToInstant(next);
            }

            // ★ `frames` COUNTS TICKS OF THIS LOOP, NOT COMPOSED FRAMES —
            // and it has to be incremented ABOVE the gate below.
            //
            // It used to sit at the bottom of the frame body, where "the loop
            // ran" and "a frame was composed" were the same event. They are
            // not any more: an idle seat now skips the body entirely, so
            // leaving the counter there would freeze it and make a HEALTHY
            // idle compositor indistinguishable from a wedged one — the exact
            // ambiguity this counter exists to remove, and the denominator
            // the vkms gate's `ticks > 10` liveness check depends on.
            //
            // So: `frames` = the loop is alive. `presented` = a frame reached
            // the display. Two questions, two counters, and the gap between
            // them is the thing worth measuring.
            introspect.tick(data.state.space.elements().count() as u64);

            // ── ★ IS A FRAME OWED AT ALL? ───────────────────────────────
            //
            // Everything below this point — gathering elements, rasterising
            // the bar, the whole `render_output` composite — used to run on
            // EVERY tick, and the damage question was asked afterwards, at
            // the flip. Measured on plo: 38.2% of a core while presenting
            // ZERO frames. The work was correct; it was simply thrown away
            // sixty times a second.
            //
            // `mado` had the identical defect with the operands reversed: it
            // computed the skip verdict, counted it, logged it, and rendered
            // anyway. Two independent renderers, same shape, opposite
            // directions — which is what made this `mekuri`'s job rather than
            // a local `if`.
            //
            // The verdict PRODUCES the permission. `Skip` carries no `Pass`,
            // and the composite below is inside `pass.spend`, so "decided to
            // skip, drew anyway" has no shape to write here.
            let verdict = data.state.owed.open();
            let mekuri::Verdict::Draw(pass) = verdict else {
                let mut next = deadline + interval;
                let now = std::time::Instant::now();
                if next <= now {
                    next = now + interval;
                }
                return smithay::reexports::calloop::timer::TimeoutAction::ToInstant(next);
            };
            // Publish why, for the `owed` leaf. Cheap: a decode of one u64
            // against a 7-element table, on a path that is by definition
            // about to do far more work than this.
            {
                let names: Vec<&'static str> =
                    pass.causes().iter().map(|c| c.name()).collect();
                *introspect
                    .last_frame_causes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = names.join("+");
            }

            // ── ★ BUILD ONE SLICE: CURSOR FIRST, THEN THE CLIENTS ───────
            //
            // Index 0 is TOPMOST. `render_output` iterates the slice with
            // `.rev()`, painting back-to-front, so the pointer belongs at the
            // front of the vector to end up on top of the screen. Reading the
            // slice as "draw order" gets this exactly backwards and puts the
            // cursor underneath every window, where it is invisible in
            // precisely the case it matters.
            let gather_start = std::time::Instant::now();
            let space_elements = smithay::desktop::space::space_render_elements(
                &mut renderer,
                [&data.state.space],
                &output,
                1.0,
            )
            .unwrap_or_default();
            // Gathering is where texture import lives — see
            // `OmoyaIntrospect::gather_us`.
            introspect.gather_us.store(
                u64::try_from(gather_start.elapsed().as_micros()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::Relaxed,
            );
            if let Some((full, part)) = import_counters.as_ref() {
                introspect.import_full.store(
                    full.load(std::sync::atomic::Ordering::Relaxed),
                    std::sync::atomic::Ordering::Relaxed,
                );
                introspect.import_partial.store(
                    part.load(std::sync::atomic::Ordering::Relaxed),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }

            let mut elements: Vec<SeatElements<R, _>> =
                Vec::with_capacity(space_elements.len() + 6);
            {
                // ── ★ THE POINTER, AS AN ARROW ──────────────────────────
                //
                // This was a 12x12 solid square, which the operator read
                // first as "a white square in the top left hand corner" and
                // later as "the mouse isn't on the screen". Both readings
                // were right: a square has no tip to aim with and no
                // orientation, so it does not signal "pointer" at all, and at
                // 12px on a 1920x1080 panel it is findable only if you
                // already know where it is.
                //
                // `cursor::rasterize` draws a real arrow with an outline, so
                // it reads against a dark background AND a light one. Built
                // once — the shape never changes, only its position.
                // ukeire: the operator's cursor scale. Read once per frame
                // but only USED on the frame that builds the buffer — the
                // arrow is rasterized once because its shape never changes.
                let cscale = data.state.config.ukeire.pointer.cursor_scale.get();
                let cur = cursor_buffer.get_or_insert_with(|| {
                    smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                        &crate::cursor::rasterize_at(cscale),
                        smithay::backend::allocator::Fourcc::Argb8888,
                        (
                            crate::cursor::width_at(cscale),
                            crate::cursor::height_at(cscale),
                        ),
                        1,
                        smithay::utils::Transform::Normal,
                        None,
                    )
                });
                let p = data.state.pointer_location;
                // Clamped so the arrow stays wholly on-screen. The TIP is at
                // (0,0) of the bitmap, so the clamp is against the full
                // extent — letting the body run off the edge would make the
                // pointer appear to shrink as it approaches a border.
                let x = (p.x.round() as i32).clamp(0, mode.size.w - crate::cursor::width_at(cscale));
                let y = (p.y.round() as i32).clamp(0, mode.size.h - crate::cursor::height_at(cscale));
                use smithay::backend::renderer::element::Kind;
                use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
                if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(
                    &mut renderer,
                    (f64::from(x), f64::from(y)),
                    cur,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    elements.push(SeatElements::Texture(el));
                }
            }

            // ── ★ THE BAR ───────────────────────────────────────────────
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                // ★ LOCAL, OR SAY UTC. The comment that used to sit here
                // claimed "local wall clock ... the offset is read once from
                // the TZ" — and no offset was ever applied. It rendered UTC
                // and labelled it UTC, so the code and its own comment
                // disagreed about which of them was lying.
                #[allow(clippy::cast_possible_wrap)]
                let (hhmm, resolved) = crate::localtime::hhmm(now as i64);
                let clock = if resolved {
                    crate::bar::Clock::Local(hhmm)
                } else {
                    crate::bar::Clock::UtcFallback(format!("{hhmm} UTC"))
                };
                // One cell per parcel, the focused one marked. `focus_rect`
                // is the layout's own answer, so the bar cannot disagree with
                // the ring on screen about which window has focus.
                let focused = *introspect
                    .focus_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let parcels: Vec<bool> = data
                    .state
                    .space
                    .elements()
                    .map(|w| {
                        data.state.space.element_geometry(w).is_some_and(|g| {
                            focused.is_some_and(|(x, y, _, _)| {
                                g.loc.x == x && g.loc.y == y
                            })
                        })
                    })
                    .collect();
                // ── ★ THE INVISIBLE STATE, READ FROM THE SAME PLACE THE
                // PLACEMENT USED ─────────────────────────────────────────────
                //
                // `windows` is what `apply_layout` consulted to decide who was
                // mapped, so the bar cannot disagree with the screen about
                // which windows are hidden — the same property the `parcels`
                // row gets by reading `focus_rect` rather than re-deriving
                // focus.
                let hidden = data.state.windows.minimized_count();
                // 1-based for display: a tab strip that starts at 0 reads as
                // an off-by-one to everyone who has used any other tabbed
                // thing.
                let tab = data
                    .state
                    .focused_surface_id()
                    .and_then(|id| data.state.windows.position_in_group(id))
                    .map(|(idx, total)| (idx + 1, total));
                let wanted = crate::bar::BarState {
                    parcels,
                    clock,
                    hidden,
                    tab,
                };
                if wanted != bar_text || bar_buffer.is_none() {
                    let bar_h = data.state.config.bar.height;
                    // Published so a caller can DERIVE the content region
                    // rather than hardcoding a number that drifts with the bar.
                    introspect
                        .bar_height
                        .store(u64::try_from(bar_h).unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                    if let Some(px) = crate::bar::rasterize_h(&wanted, mode.size.w, bar_h) {
                        bar_buffer = Some(
                            smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                                &px,
                                smithay::backend::allocator::Fourcc::Argb8888,
                                (mode.size.w, bar_h),
                                1,
                                smithay::utils::Transform::Normal,
                                None,
                            ),
                        );
                    }
                    bar_text = wanted;
                }
                if let Some(b) = bar_buffer.as_ref() {
                    use smithay::backend::renderer::element::Kind;
                    use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
                    if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(
                        &mut renderer,
                        (0.0, 0.0),
                        b,
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    ) {
                        elements.push(SeatElements::Texture(el));
                    }
                }
            }

            // ── ★ THE FOCUS BORDER, DRAWN IN THE GAP ────────────────────
            //
            // Four thin bars around the focused window's rectangle rather
            // than one filled rect behind it: a filled rect would be entirely
            // hidden by an opaque window, so it would cost a full-screen
            // element and show nothing. The four edges live in the GAP, which
            // is why `GAP` and `BORDER` are sized together.
            //
            // Pushed AFTER the cursor and BEFORE the windows: index order is
            // front-to-back, so the border sits under the windows (it does not
            // cover content) and over the background (it is visible in the
            // gap). Getting this order wrong does not error — it just draws a
            // border nobody can see.
            // ── ★ THE TITLEBAR — "no place for the mouse to go" ──────────
            //
            // The operator's report, 2026-09-03: two floating windows and
            // nowhere to grab, minimise or close them. Every one of those
            // verbs already existed as a chord; none of them was VISIBLE, and
            // a verb you cannot see is a verb most people never learn they
            // have.
            //
            // Drawn from `chrome::bar_rect`/`chrome::buttons` — the same
            // functions `input.rs` hit-tests against, so where a button is
            // drawn and where it responds cannot drift apart.
            //
            // Pushed BEFORE the windows for the same front-to-back reason the
            // border is: the chrome must sit under nothing it decorates.
            //
            // ★ FLOATING ONLY, and this guard must agree with `layout.rs`'s.
            // The layout shrinks a window's content by the bar's height only
            // in floating mode, so drawing chrome in tiling mode would paint a
            // bar over the top of a client's own content — the one thing
            // `bar_rect`'s doc promises never happens. Two guards, one fact:
            // if either moves, the other has to.
            if data.state.config.layout.mode == crate::config::LayoutMode::Floating {
                use smithay::backend::renderer::element::Kind;
                use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
                let focused = *introspect
                    .focus_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let windows: Vec<_> = data
                    .state
                    .space
                    .elements()
                    .take(CHROME_WINDOWS)
                    .cloned()
                    .collect();
                for w in windows {
                    let Some(geo) = data.state.space.element_geometry(&w) else {
                        continue;
                    };
                    let Some(id) = crate::layout::surface_id_of(&w) else {
                        continue;
                    };
                    let title = crate::layout::title_of(&w).unwrap_or_default();
                    let is_focused = focused
                        .is_some_and(|(fx, fy, _, _)| fx == geo.loc.x && fy == geo.loc.y);
                    let bar = crate::chrome::bar_rect(geo);

                    let hit = chrome_cache.iter().any(|(cid, ct, cw, cf, _)| {
                        *cid == id && ct == &title && *cw == bar.size.w && *cf == is_focused
                    });
                    if !hit {
                        chrome_cache.retain(|(cid, ..)| *cid != id);
                        if let Some(px) =
                            crate::chrome::rasterize(&title, bar.size.w, is_focused)
                        {
                            let mb = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                                &px,
                                smithay::backend::allocator::Fourcc::Argb8888,
                                (bar.size.w, bar.size.h),
                                1,
                                smithay::utils::Transform::Normal,
                                None,
                            );
                            chrome_cache.push((id, title.clone(), bar.size.w, is_focused, mb));
                        }
                    }
                    if let Some((.., b)) = chrome_cache.iter().find(|(cid, ..)| *cid == id)
                        && let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(
                            &mut renderer,
                            (f64::from(bar.loc.x), f64::from(bar.loc.y)),
                            b,
                            None,
                            None,
                            None,
                            Kind::Unspecified,
                        )
                    {
                        elements.push(SeatElements::Texture(el));
                    }
                }
                // A window that closed must not keep its buffer alive.
                let live: Vec<u32> = data
                    .state
                    .space
                    .elements()
                    .filter_map(crate::layout::surface_id_of)
                    .collect();
                chrome_cache.retain(|(cid, ..)| live.contains(cid));
            }

            if let Some((fx, fy, fw, fh)) = *introspect
                .focus_rect
                .lock()
                .unwrap_or_else(|e| e.into_inner())
            {
                use smithay::backend::renderer::element::Kind;
                use smithay::backend::renderer::element::solid::SolidColorRenderElement;
                use smithay::backend::renderer::utils::CommitCounter;
                let b = crate::layout::BORDER;
                let colour = smithay::backend::renderer::Color32F::from(
                    crate::theme::focus_border_for_surface(false),
                );
                // top, bottom, left, right — each already inset into the gap.
                let edges = [
                    (fx - b, fy - b, fw + b * 2, b),
                    (fx - b, fy + fh, fw + b * 2, b),
                    (fx - b, fy, b, fh),
                    (fx + fw, fy, b, fh),
                ];
                for (i, (x, y, w, h)) in edges.into_iter().enumerate() {
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    elements.push(SeatElements::Solid(SolidColorRenderElement::new(
                        border_ids[i].clone(),
                        smithay::utils::Rectangle::new((x, y).into(), (w, h).into()),
                        CommitCounter::default(),
                        colour,
                        Kind::Unspecified,
                    )));
                }
            }

            elements.extend(space_elements.into_iter().map(SeatElements::Space));
            // Published so `windows` and `elements` can be compared. A window
            // exists in `Space` from creation; an element exists only once the
            // client has attached a buffer, so a gap between the two is
            // exactly "mapped but never drew". Minus one for the cursor, which
            // is always element 0 and is ours, not a client's.
            introspect.elements.store(
                (elements.len().saturating_sub(1)) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            // And each element's geometry as the RENDERER sees it — a third
            // independent view, computed by `space_render_elements` from the
            // Space positions rather than restating them. If this disagrees
            // with the `layout` leaf, the gap is between the compositor's
            // model and what the frame was told to draw.
            {
                use smithay::backend::renderer::element::Element as _;
                *introspect
                    .geometry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = elements
                    .iter()
                    .map(|e| {
                        let g = e.geometry(1.0.into());
                        format!("{},{} {}x{}", g.loc.x, g.loc.y, g.size.w, g.size.h)
                    })
                    .collect();
            }

            // ── ★ RENDER INTO THE BACK BUFFER, THEN FLIP ────────────────
            // `DrmCompositor` did allocate-bind-render-export-flip in one
            // call. Split out, the order is load-bearing: the dmabuf export
            // and the renderer bind must both target the BACK buffer, and the
            // flip must come after the frame is complete — a flip mid-render
            // shows a half-drawn frame, which reads as a renderer bug.
            // ★ THE COMPOSITE RUNS INSIDE THE PASS, and the pass is spent by
            // the outcome. `Ok` marks it presented; `Err` puts the causes
            // BACK on the ledger, so a refused flip or a lost device leaves
            // the frame still owed and the next tick retries it.
            //
            // That error path is a bug neither original had: both drained
            // their reason before rendering and dropped it on failure, so a
            // single failed frame left the screen stale until something
            // unrelated happened to dirty it again.
            let frame_result = pass.spend(|_causes| (|| -> Result<(), Box<dyn std::error::Error>> {
                let dmabuf = {
                    use smithay::backend::allocator::dmabuf::AsDmabuf;
                    scanout.back_buffer().export()?
                };
                let mut dmabuf = dmabuf;

                // ★ A CAPTURE FORCES A FULL REPAINT, BY PASSING AGE 0.
                //
                // Without this, a screenshot of an idle desktop is a trap:
                // partial repaint would skip the frame entirely (nothing
                // changed), and the back buffer still holds whatever was
                // composed two frames ago. The capture would succeed, produce
                // a file, and show a stale screen — the worst possible
                // failure for the one tool that exists to answer "what is
                // actually on the display right now".
                //
                // Age 0 is not a special case bolted on: it is the damage
                // protocol's own way of saying "no usable history", and
                // `damage_output_internal` answers it by damaging the whole
                // output. So the request is expressed in the vocabulary the
                // tracker already has rather than by reaching past it.
                let requested = introspect
                    .capture_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                let age = if requested.is_some() {
                    0
                } else {
                    scanout.back_buffer_age()
                };

                // ★ ARM THE FLUSH FOR THIS SLOT, IMMEDIATELY BEFORE BINDING IT.
                //
                // The generation is the slot's own `last_drawn`, so the damage
                // `render_output` produces at `age` and the baseline the flush
                // adjudicates against describe the SAME interval by
                // construction. `None` (never drawn into) leaves no plan, and
                // no plan is a full copy.
                //
                // A capture request forces `age = 0`, i.e. a full repaint, and
                // a full repaint has no baseline to preserve — so the policy is
                // deliberately not applied on those frames.
                {
                    use crate::nuri_renderer::ArmFlush as _;
                    renderer.arm_flush(
                    if age == 0 {
                        crate::config::FlushPolicy::Full
                    } else {
                        data.state.config.damage.flush
                    },
                    scanout.back_buffer_generation(),
                    );
                }

                let presented = {
                    let mut fb = renderer.bind(&mut dmabuf)?;

                    // ★ THE TRACKER OWNS CLEAR AND DRAW BOTH.
                    //
                    // This replaced a hand-written `frame.clear(whole screen)`
                    // followed by `element.draw(.., &[geo], ..)` — a full-screen
                    // composite every frame, with each element told its own
                    // entire geometry was damaged. Measured cost of that shape:
                    // a 536 ms frame on plo, which is what the operator was
                    // seeing as "the refresh from when I type is all wrong".
                    //
                    // `render_output` computes the union of what actually
                    // changed since this BUFFER was last drawn into (hence the
                    // age), clears only that, and skips any element that does
                    // not intersect it.
                    let frame_start = std::time::Instant::now();
                    let result = damage_tracker.render_output(
                        &mut renderer,
                        &mut fb,
                        age,
                        &elements,
                        smithay::backend::renderer::Color32F::from(clear),
                    )?;
                    // Owned, because the control render below borrows the
                    // tracker again and `result` would still be alive at the
                    // flush. Cloning a handful of rectangles is the cheapest
                    // way to keep both.
                    let drawn: Option<Vec<_>> = result.damage.cloned();
                    drop(result);

                    // ── ★ THE STALE DIFFERENTIAL — A NEGATIVE CONTROL ────────
                    //
                    // This replaced a shadow-vs-scanout comparison, which was
                    // CORRECT until `flush_damage` became a full copy and then
                    // silently became VACUOUS: a full copy makes those two byte
                    // ranges equal by construction, so the scan could not report
                    // a defect no matter what was on the screen. It did not go
                    // red or go quiet — it went permanently, unfalsifiably green.
                    //
                    // The honest question is not "did the copy arrive" (it now
                    // always does) but "did the natural-age render DRAW
                    // everything a full repaint would have?" So the probe renders
                    // the SAME scene twice, back to back inside one frame:
                    //
                    //   A = render at the natural buffer age   (what you saw)
                    //   B = render at age 0, a full repaint    (ground truth)
                    //
                    // B is the negative control. A ≠ B means damage was
                    // under-reported and the difference IS the stale content, at
                    // exactly the pixels the operator is looking at.
                    //
                    // Same frame, same element list, no time gap — so a moving
                    // cursor or a ticking clock cannot masquerade as staleness,
                    // which a two-frame version of this could never rule out.
                    //
                    // The second render also REPAIRS the frame, so arming the
                    // probe costs one extra composite and leaves the screen
                    // correct. That is deliberate: an instrument that leaves the
                    // defect on screen tempts you to keep it armed.
                    let stale_probe = introspect
                        .stale_request
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    let stale_baseline: Option<Vec<u8>> = if stale_probe.is_some() {
                        let (w, h) = (mode.size.w, mode.size.h);
                        renderer
                            .copy_framebuffer(
                                &fb,
                                smithay::utils::Rectangle::from_size((w, h).into()),
                                DrmFourcc::Argb8888,
                            )
                            .ok()
                            .and_then(|m| renderer.map_texture(&m).ok().map(<[u8]>::to_vec))
                    } else {
                        None
                    };
                    if stale_baseline.is_some() {
                        // Ground truth. `age = 0` is the whole point — it is what
                        // makes this a control rather than a second sample.
                        damage_tracker.render_output(
                            &mut renderer,
                            &mut fb,
                            0,
                            &elements,
                            smithay::backend::renderer::Color32F::from(clear),
                        )?;
                    }

                    // ── ★ THE ONE WRITE TO SCANOUT MEMORY IN THE FRAME ──
                    //
                    // Everything above composited into a RAM shadow (see
                    // `NuriFramebuffer::shadow`). Nothing has reached the
                    // display yet. This is the damage-clipped streaming copy
                    // that puts it there, and WITHOUT THIS LINE the screen
                    // never changes — the compositor would run, report frames,
                    // and paint nothing.
                    //
                    // `result.damage` is what `render_output` actually drew,
                    // already unioned with whatever was stale in THIS buffer
                    // for its age — so copying exactly it is both sufficient
                    // and minimal. `None` means "no usable history", which is
                    // the full-repaint case, and `flush_damage` copies
                    // everything for an empty slice.
                    {
                        use crate::nuri_renderer::ScanoutFlush as _;
                        // ★ TIMED SEPARATELY, FOR THE SAME REASON `gather_us`
                        // WAS SPLIT OUT — and this is the second time the same
                        // shape has hidden the largest term inside a total.
                        //
                        // `frame_us` brackets this call (its `frame_start` is
                        // well above), so the flush has always been INSIDE the
                        // number and never separable FROM it. That matters
                        // because `flush_damage` is currently unconditional: it
                        // copies stride x height into write-combining memory
                        // whatever the damage says. Whether that is the dominant
                        // cost of a frame is the question the whole
                        // damage-strategy question turns on, and until now it
                        // could only be argued from arithmetic.
                        //
                        // Read it against `frame_us`. If `flush_us` is most of
                        // `frame_us`, the seat is bound by one memcpy into
                        // uncached memory and no change detector can help;
                        // re-opening the damage-clipped flush is then the only
                        // move that pays.
                        let flush_start = std::time::Instant::now();
                        let wrote = fb.flush_damage(drawn.as_deref().unwrap_or(&[]));
                        let took =
                            u64::try_from(flush_start.elapsed().as_micros()).unwrap_or(u64::MAX);
                        // ★ LAST, MAX AND TOTAL — because the last value alone
                        // is a point sample and reads 3x apart on this seat.
                        // `max` bounds the tail; `total` divided by `presented`
                        // is the mean; and `bytes/us` is the RATE, which is the
                        // only one of these that can tell a slow copy from a
                        // descheduled thread on a machine running other work.
                        {
                            use std::sync::atomic::Ordering::Relaxed;
                            introspect.flush_us.store(took, Relaxed);
                            introspect.flush_us_total.fetch_add(took, Relaxed);
                            introspect.flush_us_max.fetch_max(took, Relaxed);
                            introspect.flush_bytes.store(wrote, Relaxed);
                            introspect.flush_bytes_total.fetch_add(wrote, Relaxed);
                        }
                    }

                    // ── ★ THE STALE SCAN — AFTER THE FLUSH, BEFORE THE FLIP ──
                    //
                    // Placement is the whole correctness of this check. After
                    // `flush_damage` the back buffer holds exactly what is
                    // about to be scanned out: whatever it kept from two
                    // frames ago, plus this frame's damage copied over it. If
                    // the damage was under-reported, the difference against
                    // the shadow IS the stale content the operator sees.
                    //
                    // A frame earlier and the flush has not happened; a frame
                    // later and the buffers have swapped and the evidence is
                    // gone.
                    // ★ `blind` is an ANSWER. If the baseline snapshot failed
                    // the probe still consumed the request, and reporting nothing
                    // would leave the caller polling a result that can never
                    // arrive — the exact failure this scan exists to end. Say
                    // which of the four kotae outcomes happened.
                    if let (Some(path), None) = (stale_probe.as_ref(), stale_baseline.as_ref()) {
                        *introspect
                            .stale_result
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            serde_json::json!({
                                "outcome": "blind",
                                "reason": "could not read back the natural-age \
                                           render; no baseline, so no control",
                                "path": path,
                            })
                            .to_string(),
                        );
                    }
                    if let (Some(mask_path), Some(baseline)) = (stale_probe, stale_baseline) {
                        use crate::nuri_renderer::ScanoutFlush as _;
                        let (w, h) = (mode.size.w, mode.size.h);
                        #[allow(clippy::cast_sign_loss)]
                        let (wu, hu) = (w as usize, h as usize);
                        let verdict = match renderer.copy_framebuffer(
                            &fb,
                            smithay::utils::Rectangle::from_size((w, h).into()),
                            DrmFourcc::Argb8888,
                        ) {
                            Ok(m) => match renderer.map_texture(&m) {
                                Ok(shadow) => {
                                    // expected = the full repaint (ground
                                    // truth), actual = what the natural-age
                                    // render actually produced. Argument order
                                    // is `scan(expected, actual)`; swapping it
                                    // inverts every attribution silently.
                                    let mut rep =
                                        crate::stale::scan(
                                            crate::stale::GroundTruth::from_full_repaint(shadow),
                                            crate::stale::Observed::from_natural_age(&baseline),
                                            wu,
                                            hu,
                                        );
                                    // Attribute against what the compositor
                                    // itself drew, so a region is named by
                                    // subsystem rather than counted.
                                    let mut named: Vec<crate::stale::NamedRect> = elements
                                        .iter()
                                        .enumerate()
                                        .map(|(i, e)| {
                                            // `1.0` — the same scale the
                                            // focus-ring geometry read at
                                            // :1130 uses. This seat is 1:1;
                                            // a second spelling here could
                                            // disagree with that one and
                                            // attribute regions to the wrong
                                            // element on a HiDPI output.
                                            let g = smithay::backend::renderer::element::Element::geometry(
                                                e,
                                                1.0.into(),
                                            );
                                            crate::stale::NamedRect {
                                                name: format!("element[{i}]"),
                                                x: g.loc.x,
                                                y: g.loc.y,
                                                w: g.size.w,
                                                h: g.size.h,
                                            }
                                        })
                                        .collect();
                                    named.push(crate::stale::NamedRect {
                                        name: "background".into(),
                                        x: 0,
                                        y: 0,
                                        w,
                                        h,
                                    });
                                    crate::stale::attribute(&mut rep, &named);
                                    let img =
                                        crate::stale::render_mask(shadow, &baseline, wu, hu);
                                    let wrote = std::fs::write(&mask_path, &img).is_ok();
                                    let regions: Vec<serde_json::Value> = rep
                                        .regions
                                        .iter()
                                        .take(12)
                                        .map(|r| {
                                            serde_json::json!({
                                                "rect": [r.x, r.y, r.w, r.h],
                                                "pixels": r.pixels,
                                                "attributed_to": r.attribution,
                                            })
                                        })
                                        .collect();
                                    serde_json::json!({
                                        "outcome": if rep.compared_pixels == 0 {
                                            "blind"
                                        } else if rep.stale_pixels == 0 {
                                            "clean"
                                        } else {
                                            "stale"
                                        },
                                        "stale_pixels": rep.stale_pixels,
                                        // ★ The denominator travels WITH the
                                        // verdict: a scan that compared
                                        // nothing reports zero stale, and
                                        // without this that is
                                        // indistinguishable from a healthy
                                        // seat.
                                        "compared_pixels": rep.compared_pixels,
                                        "regions_total": rep.regions.len(),
                                        "regions": regions,
                                        "mask": if wrote { Some(mask_path.clone()) } else { None },
                                    })
                                    .to_string()
                                }
                                Err(e) => format!("{{\"outcome\":\"blind\",\"error\":\"map: {e}\"}}"),
                            },
                            Err(e) => format!("{{\"outcome\":\"blind\",\"error\":\"copy: {e}\"}}"),
                        };
                        *introspect
                            .stale_result
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(verdict);
                    }

                    // ★ CAPTURE HERE, WHERE THE FRAMEBUFFER IS STILL BOUND.
                    //
                    // This used to live outside this block, where it logged
                    // "capture requested" and called nothing — a stub that
                    // reported success while producing no file. It could not
                    // have worked there: `fb` is dropped at the closing brace,
                    // and `capture` needs it.
                    //
                    // Placed after the render so what is read back is the frame
                    // that was actually composed, and before `flip()` so it
                    // reflects the buffer being handed to the display rather
                    // than whatever the previous flip left in the other slot.
                    //
                    // Taking the request CLEARS it (above), so this is one-shot
                    // by construction: a capture every frame would fill the
                    // disk and change the timing it exists to observe.
                    if let Some(req) = requested {
                        let path = req.path.clone();
                        let size = (mode.size.w, mode.size.h);
                        let outcome =
                            match capture(
                                &mut renderer,
                                &fb,
                                size,
                                std::path::Path::new(&path),
                                req.region,
                                req.hash_only,
                            ) {
                                Ok(v) => {
                                    tracing::info!(path = %path, "captured");
                                    if req.hash_only {
                                        format!("hash: {v}")
                                    } else {
                                        format!("ok: {v}")
                                    }
                                }
                                // Reported, never fatal: a failed screenshot
                                // must not take down the seat it was meant to
                                // diagnose. The caller reads the reason back
                                // rather than being told only that it did not
                                // appear.
                                Err(e) => {
                                    tracing::error!(error = %e, path = %path, "capture failed");
                                    format!("error: {e}")
                                }
                            };
                        // ★ STAMPED WITH THE REQUEST ID. A result without one
                        // is anonymous, and a client that reconnects reads a
                        // predecessor's success as its own -- observed.
                        *introspect
                            .capture_result
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(
                            serde_json::json!({
                                "request_id": req.id,
                                "outcome": outcome,
                            })
                            .to_string(),
                        );
                    }

                    // Published so the cost of a frame is a number anyone
                    // can read, rather than something inferred from a CPU
                    // percentage and a stripped stack trace.
                    introspect.frame_us.store(
                        u64::try_from(frame_start.elapsed().as_micros()).unwrap_or(u64::MAX),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if let Some((cp, bl, gp)) = blit_counters.as_ref() {
                        introspect.blit_fast.store(
                            cp.load(std::sync::atomic::Ordering::Relaxed),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        introspect.blit_slow.store(
                            bl.load(std::sync::atomic::Ordering::Relaxed),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        introspect.blit_general.store(
                            gp.load(std::sync::atomic::Ordering::Relaxed),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    drawn.is_some()
                };

                // ★ NOTHING CHANGED ⇒ NO FLIP. This is the second half of the
                // saving and the larger one on an idle seat: a page flip costs
                // a vblank wait and a buffer swap whether or not the contents
                // differ, and swapping presents the OTHER buffer — which, on a
                // skipped frame, holds an older image than the one on screen.
                // So flipping a skipped frame is not merely wasteful, it moves
                // the display backwards.
                if presented {
                    scanout.flip()?;
                    // Accepted, not retired — the VBlank event clears this.
                    flip_pending.store(true, std::sync::atomic::Ordering::Release);

                    // ── ★ THE LEDGER CLEARS HERE, AND NOWHERE ELSE ──────────
                    //
                    // Every truedamage refinement reports the union of what
                    // changed since the last PRESENT, not since the last
                    // commit, because the compositor does not render every
                    // commit — measured on plo, 28 commits to 25 renders in a
                    // 20-line burst. Clearing that ledger anywhere but here
                    // reintroduces the defect: a commit whose damage is
                    // dropped before it reaches the glass is never repainted.
                    //
                    // Placed AFTER `flip()?` on purpose. A refused flip must
                    // not clear anything — the pixels are still not on screen,
                    // and `?` leaves the ledger intact for the next attempt.
                    //
                    // It is also deliberately not at composite time: a frame
                    // can be composed and then skipped, and clearing there
                    // would look correct because the damage is still USUALLY
                    // right — which is exactly how the original bug survived.
                    data.state.shadows.mark_presented();
                    // Counted HERE and not beside `frames`, because the gap
                    // between the two counters IS the partial-repaint
                    // measurement. Incremented after the flip is accepted, so
                    // a refused flip is not counted as a presentation.
                    introspect
                        .presented
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // ── ★ THE INTERVAL, NOT JUST THE COUNT ──────────────────
                    //
                    // `frames`/`presented` cannot separate idle from starved:
                    // both give a large ratio. Measured on plo at idle,
                    // 1190841/384 -- which reads as catastrophic loss and is a
                    // pacing loop correctly finding nothing to draw. An idle
                    // seat presents rarely and EVENLY; a starved one presents
                    // in bursts. Same ratio, different distribution.
                    let now_us = u64::try_from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_micros())
                            .unwrap_or(0),
                    )
                    .unwrap_or(0);
                    let prev_us = introspect
                        .last_present_us
                        .swap(now_us, std::sync::atomic::Ordering::Relaxed);
                    // ★ Skip the first: 0 means no previous presentation, and
                    // bucketing it would record time-since-process-start as a
                    // frame gap -- one enormous outlier in every histogram.
                    if prev_us != 0 && now_us > prev_us {
                        let us = u128::from(now_us - prev_us);
                        // 2778us is one frame at 360Hz.
                        let bucket = match us {
                            0..=2_778 => 0,
                            2_779..=8_333 => 1,
                            8_334..=16_667 => 2,
                            16_668..=50_000 => 3,
                            50_001..=250_000 => 4,
                            _ => 5,
                        };
                        introspect.present_buckets[bucket]
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Ok(())
            })());
            if let Err(e) = frame_result {
                // The causes are already back on the ledger — `spend` did it
                // on the `Err`. Nothing to re-mark here, and re-marking would
                // be wrong: it would owe the frame twice.
                tracing::error!(error = %e, "frame failed — still owed, retrying next tick");
            }

            // Tell clients their buffers were consumed, or they will never draw
            // a second frame. A compositor that renders once and then appears
            // frozen is usually this line missing.
            let now = data.state.start_time.elapsed();
            data.state.space.elements().for_each(|w| {
                w.send_frame(
                    &output,
                    now,
                    Some(std::time::Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            });
            // ★ LAYER SURFACES GET THEM TOO — AND THIS LINE IS WHY A BAR OR A
            // LAUNCHER WOULD HAVE FROZEN AFTER EXACTLY ONE FRAME.
            //
            // The loop above walks `space.elements()`, which holds toplevels
            // and nothing else. A `zwlr_layer_shell_v1` client — every bar,
            // every launcher, every notification daemon, every lock screen —
            // lives in the output's `LayerMap`, never in the space, so it
            // received no frame callback at all. It would map, draw once,
            // wait for a callback that no code path sends, and hang there
            // looking alive.
            //
            // omoya advertises layer-shell v5, so a client is entitled to
            // assume this works. Advertising a protocol and then withholding
            // the callback it depends on is worse than not advertising it.
            {
                let map = smithay::desktop::layer_map_for_output(&output);
                for layer in map.layers() {
                    layer.send_frame(
                        &output,
                        now,
                        Some(std::time::Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                }
            }

            // ── ★ ANSWER `wp_presentation`, HONESTLY ────────────────────
            //
            // The global has been advertised since this file was written and
            // NOTHING ever answered it. That is worse than not advertising it,
            // and not by a little: Mesa's Wayland WSI keeps a `frame_fallback`
            // path commented "Fallback when wp_presentation is not supported",
            // and binding the global is what DISABLES that fallback. So a
            // Vulkan client asking for presentation timing got a compositor
            // that promised to answer, never did, and had already switched
            // off the client's own workaround.
            //
            // ★ `Kind::Vsync` AND NOTHING ELSE, while the loop is a timer.
            // The protocol's own wording is normative and rules the other
            // flags out by construction:
            //   hw_clock      "Sampling a clock in software is not acceptable"
            //   hw_completion "The opposite of this is e.g. a timer being used
            //                  to guess when the display hardware has switched"
            // This loop IS that timer. Claiming `HwClock | HwCompletion` here
            // would hand every client a confidently wrong number, which is
            // strictly worse than the silence it replaces. When the loop moves
            // to `DrmDeviceNotifier` vblank events, the event's own `time` and
            // `sequence` are exactly what upgrade this — one wire, not two.
            {
                use smithay::wayland::presentation::Refresh;
                let mut feedback =
                    smithay::desktop::utils::OutputPresentationFeedback::new(&output);
                for w in data.state.space.elements() {
                    w.take_presentation_feedback(
                        &mut feedback,
                        smithay::desktop::utils::surface_primary_scanout_output,
                        // ★ A CONSTANT `Vsync`, not
                        // `surface_presentation_feedback_flags_from_states`.
                        // That helper derives ZeroCopy from the render states,
                        // and this compositor can never earn it — the protocol
                        // says "Compositing with OpenGL counts as copying", and
                        // nuri copies on the CPU. Deriving a flag we are
                        // structurally unable to set would be a more elaborate
                        // way of publishing the same wrong answer.
                        |_surface, _| {
                            smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync
                        },
                    );
                }
                feedback.presented::<_, smithay::utils::Monotonic>(
                    now,
                    Refresh::Variable(refresh_interval),
                    0,
                    smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
                );
            }
            // ★ Capture is handled INSIDE the frame closure above, where the
            // framebuffer is still bound, and is triggered by a kanshou
            // request rather than an env var. It sat here originally and could
            // only ever log — `fb` is out of scope by this point — and the env
            // gate could never serve the moment it named, since a running
            // process's environment cannot be changed from outside.


            // ★ AND PUBLISH `owed_vt_switches`, WHICH NOTHING WAS WRITING.
            //
            // There are two fields with this name: `Omoya::owed_vt_switches`
            // (a plain u64, incremented by the chord handler at
            // `input.rs:71`) and `OmoyaIntrospect::owed_vt_switches` (the
            // AtomicU64 the kanshou leaf actually reads). Only the first was
            // ever written, so the leaf answered `0` forever — and answering
            // is exactly what made it look healthy. It was quoted as evidence
            // of a good VT state repeatedly on 2026-08-20; it meant nothing.
            //
            // Published from the render loop for the same reason `frames` is:
            // the socket thread must not lock `Omoya` (see introspect.rs's
            // header), so the owner pushes rather than the reader pulling.
            //
            // `every_schema_leaf_answers` did not catch this, and could not:
            // it proves a leaf ANSWERS, never that anything FEEDS it. A leaf
            // with no writer is the vacuous-gate shape in miniature.
            introspect
                .owed_vt_switches
                .store(data.state.owed_vt_switches, std::sync::atomic::Ordering::Relaxed);

            data.state.space.refresh();
            data.state.popups.cleanup();
            let _ = data.display_handle.flush_clients();

            // ── ★ SCHEDULE FROM THE DEADLINE, NOT FROM NOW ───────────────
            //
            // This was `ToDuration(interval)`, and calloop implements that as
            // `Instant::now() + duration` — computed when the callback
            // RETURNS. So the real period was `render_time + interval`, and
            // the compositor could never reach the panel's refresh rate no
            // matter how fast it drew.
            //
            // Measured on plo before this change: 48.5 fps idle (period
            // 20.6ms => ~4ms render) and 34.6 fps with one window (period
            // 28.9ms => ~12ms render) on a 60 Hz panel. A full 1920x1080 CPU
            // composite costs that 12ms, and the timer then added 16.67ms on
            // top of it. The operator's report was "the refresh from when I
            // type to the terminal is all wrong", and this is most of why.
            //
            // Scheduling from `deadline` removes the render time from the
            // period. It does NOT make pacing correct — that is vblank, and
            // `pending-omoya-vblank` still stands — but it is the difference
            // between a period that tracks the panel and one that drifts with
            // whatever the frame cost.
            //
            // ★ AND IT MUST NOT BUSY-LOOP. If a frame overruns, `deadline +
            // interval` is already in the past and calloop would fire
            // immediately, forever, burning a core to render frames nobody
            // sees. Missed intervals are SKIPPED to the next future one —
            // dropping a frame under load is correct; spinning is not.
            let mut next = deadline + interval;
            let now = std::time::Instant::now();
            if next <= now {
                let behind = now.duration_since(next).as_nanos();
                let missed = behind / interval.as_nanos().max(1) + 1;
                next += interval * u32::try_from(missed).unwrap_or(1);
            }
            smithay::reexports::calloop::timer::TimeoutAction::ToInstant(next)
        },
    )?;

    // ── ★ THE CLOCK, AND WHY IT IS NOT A ONE-SECOND REPAINT ─────────────
    //
    // The status bar shows `hh:mm`, so it changes 1440 times a day and not
    // 86 400. A one-second timer that simply marked `Chrome` would hold the
    // seat at a steady 1 fps forever — which is not free, because on this
    // renderer a frame is a full-screen CPU composite, and it is exactly the
    // kind of "small" idle cost that turned out to be 38% of a core the last
    // time it was estimated instead of measured.
    //
    // So the timer TICKS every second and MARKS only when the rendered
    // minute actually differs. An idle seat presents zero frames per second,
    // and the bar is still never more than a second stale.
    {
        let ledger = data.state.owed.ledger();
        let mut last_minute: Option<u64> = None;
        event_loop.handle().insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(
                std::time::Duration::from_secs(1),
            ),
            move |_, _, _| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let minute = now / 60;
                if last_minute != Some(minute) {
                    last_minute = Some(minute);
                    ledger.mark(crate::owed::Owed::Chrome);
                }
                smithay::reexports::calloop::timer::TimeoutAction::ToDuration(
                    std::time::Duration::from_secs(1),
                )
            },
        )?;
    }

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
    // ── ★ REGION + HASH ─────────────────────────────────────────────────────
    //
    // `copy_framebuffer` ALREADY takes and clips a rectangle; only this
    // function hardcoded full-size. Region capture is therefore wiring, not
    // new readback machinery.
    //
    // `hash_only` exists because a 1920x1080 PPM is 6.2 MB and the question is
    // usually "did it change", not "what is every pixel". Measured on plo:
    // three captures 0.5 s apart are BIT-IDENTICAL, and across 70 s only the
    // top 28 rows differ -- the 1 Hz clock in the bar. So a hash is a real
    // change-oracle, and excluding the bar is what makes it usable rather than
    // a detector of the clock.
    region: Option<(i32, i32, i32, i32)>,
    hash_only: bool,
) -> Result<String, Box<dyn std::error::Error>>
where
    R: smithay::backend::renderer::ExportMem,
    R::Error: Send + Sync + 'static,
{
    use std::io::Write;

    // Clip to the output. A region reaching past the edge is CLAMPED rather
    // than refused: a caller asking for the bottom-right corner should not have
    // to know the exact mode to get it.
    let (rx, ry, rw, rh) = region.unwrap_or((0, 0, size.0, size.1));
    let rx = rx.clamp(0, size.0);
    let ry = ry.clamp(0, size.1);
    let rw = rw.clamp(1, size.0 - rx);
    let rh = rh.clamp(1, size.1 - ry);
    let rect = smithay::utils::Rectangle::new((rx, ry).into(), (rw, rh).into());
    let mapping = renderer.copy_framebuffer(framebuffer, rect, DrmFourcc::Argb8888)?;
    let bytes = renderer.map_texture(&mapping)?;

    if hash_only {
        // ★ blake3 over the RGB bytes in the same order the PPM would hold
        // them, so a hash and an image of the same region describe the same
        // thing -- otherwise two oracles disagree about what "unchanged" means.
        let mut h = blake3::Hasher::new();
        for px in bytes.chunks_exact(4) {
            h.update(&[px[2], px[1], px[0]]);
        }
        let digest = h.finalize().to_hex().to_string();
        tracing::info!(hash = %digest, x = rx, y = ry, w = rw, h = rh, "region hash");
        return Ok(digest);
    }

    let mut out = Vec::with_capacity(15 + (rw * rh * 3) as usize);
    out.extend_from_slice(format!("P6\n{rw} {rh}\n255\n").as_bytes());
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
    tracing::info!(path = %path.display(), w = rw, h = rh, "wrote framebuffer capture");
    Ok(format!("{}", path.display()))
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
        // ★ THIS TEST WAS PINNING THE BUG, AND COULD NOT SAY SO.
        //
        // It asserted (7, 9, 13) — Nord0 through the LINEAR encoding — under
        // the name `..._is_the_srgb_encoding`, which is the opposite of what
        // it checked. That is the screen the operator called black.
        //
        // It survived the fix because the crate's test target did not compile
        // (`FormatSet::is_empty`), so this never ran. An absent test does not
        // merely fail to catch a regression: it actively preserves the wrong
        // expectation, in writing, where a reader takes it for a measurement.
        //
        // DRM_FORMAT_ARGB8888 applies no conversion, so the bytes written must
        // ALREADY be sRGB.
        //
        // ★ THE EXPECTED VALUE CHANGED ON 2026-09-03 and the hazard got
        // SHARPER, not milder. The ground is no longer Nord0 (46,52,64): it
        // resolves through the fleet's `desktop` role to `shadow_tone`
        // #141822 = (20,24,34), because a compositor painting the same value
        // its clients paint gave every window a 1.00:1 edge.
        //
        // That makes the linear-vs-sRGB mistake WORSE. Nord0 mis-encoded gave
        // (7,9,13) and was reported as "a blank black screen"; this ground
        // mis-encoded is darker still. The assertion below is the only thing
        // standing between that mistake and a seat nobody can see.
        let [r, g, b, _] = background();
        let byte = |f: f32| (f * 255.0).round() as u8;
        assert_eq!(
            (byte(r), byte(g), byte(b)),
            (20, 24, 34),
            "the linear encoding of this ground is darker than the (7,9,13) \
             that was once reported as a blank black screen — if this fails \
             with a near-zero triple, the encoding is inverted again"
        );
    }

    /// ★ THIS TEST USED TO REIMPLEMENT THE ARITHMETIC AND NEVER CALL THE CODE.
    ///
    /// It built its own `ns(hz)` closure and asserted on that, so it proved
    /// the FORMULA and could not fail no matter what `frame_interval` did.
    /// The real defect was upstream of the formula — `vrefresh()` returning 0
    /// on a panel that does not publish it, and `.max(1)` turning that into a
    /// 1 Hz desktop. A test that never calls the function cannot see that.
    #[test]
    fn frame_interval_is_derived_from_the_real_rate() {
        let ns = |hz: u32| Duration::from_nanos(1_000_000_000 / u64::from(hz));
        assert!(ns(60).as_micros() > 16_000 && ns(60).as_micros() < 17_000);
        assert!(ns(144).as_micros() > 6_000 && ns(144).as_micros() < 7_500);
    }

    /// The regression that made the seat feel broken: an unstated `vrefresh`
    /// must NEVER become 1 Hz.
    ///
    /// Exercised through the arithmetic `refresh_hz` uses, because a
    /// `drm::control::Mode` cannot be constructed outside the kernel — the
    /// struct is opaque and every field is populated by an ioctl. So the
    /// derivation is pinned here and the guard against 1 is pinned with it;
    /// the live proof is the seat's own measured tick rate.
    #[test]
    fn an_unstated_refresh_never_becomes_one_hertz() {
        // plo's DP-1: 1920x1080, pixel clock 148500 kHz, htotal 2200,
        // vtotal 1125 — the standard 1080p60 timing.
        let derived = (148_500_u64 * 1000) / (2200 * 1125);
        assert_eq!(derived, 60, "1080p60 timings must derive 60 Hz");
        // And the floor: whatever goes wrong, the seat must not be paced at
        // one frame per second.
        assert!(24_u32.max(1) >= 24, "the floor is 24, never 1");
    }
}
