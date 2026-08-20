//! omoya's runtime state — the smithay protocol state, plus the typed mode
//! machine that decides what the seat is *for*.
//!
//! The smithay half follows the shape of smithay's own `smallvil` example,
//! which is what that example exists to teach: the handler set and the
//! `delegate_*` wiring are the library's contract, not a design choice we get
//! to make differently. What is ours is above it — the mode, the palette, and
//! the chord catalog.

use std::{ffi::OsString, sync::Arc};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        dmabuf::{DmabufGlobal, DmabufState},
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::CalloopData;

/// Which face the compositor is wearing.
///
/// A runtime enum here rather than `omoya_spec`'s typestate, because the CLI
/// hands us a *string* and the event loop is one long-lived object. The
/// typestate is what governs the TRANSITIONS (see `omoya_spec::Compositor`);
/// this records where we are for the renderer and the logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatMode {
    /// The greeter. Composites zero clients — see `theory/OMOYA.md` §4.1.
    Entrance,
    /// The operator's desktop. The only mode that composites foreign clients.
    Session,
}

impl SeatMode {
    /// Parse the `--mode` argument.
    ///
    /// `lock` is deliberately NOT accepted: lock is not a mode you *launch*
    /// into, it is a state the session enters in-process, and accepting it here
    /// would advertise a spawn shape `theory/OMOYA.md` §4.2 rejects.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "entrance" => Ok(Self::Entrance),
            "session" => Ok(Self::Session),
            "lock" => Err(
                "`lock` is not a launchable mode — it is in-process session state (OMOYA.md §4.2)"
                    .to_string(),
            ),
            other => Err(format!("unknown mode `{other}` (entrance | session)")),
        }
    }

    /// The name, matching `omoya_spec`'s `ModeState::NAME`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Entrance => "entrance",
            Self::Session => "session",
        }
    }
}

pub struct Omoya {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    /// Which face we are wearing.
    pub mode: SeatMode,
    /// The chords the seat owes the operator and must never swallow.
    ///
    /// Held rather than consulted-on-demand so it is impossible to handle input
    /// without it being in scope — see `crate::input`.
    pub reserved: awase::Reserved,
    /// Reserved chords omoya RECOGNISED but could not act on, because the
    /// nested backend does not own a VT. Counted rather than ignored: this is
    /// the number M4's test asserts goes to zero once the DRM backend can
    /// actually perform the switch, and until then it is the honest record
    /// that the escape hatch is the HOST's, not ours.
    pub owed_vt_switches: u64,

    pub space: Space<Window>,
    /// How the windows are arranged. See `layout.rs` — the algebra is
    /// `kukaku`'s, and only "a leaf is a Window" and "a rect is pixels" are
    /// omoya's.
    pub tiling: crate::layout::Tiling,
    pub loop_signal: LoopSignal,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    /// The dmabuf protocol delegate.
    ///
    /// ★ ALWAYS CONSTRUCTED, GLOBAL CREATED LATER. `DmabufState::new()` costs
    /// a map; the GLOBAL is a PROMISE to clients that omoya can texture what
    /// they allocate, and only the render loop knows which renderer will try.
    /// Splitting the two is what keeps the advertised format list and the
    /// renderer's real capability from becoming two hand-maintained lists that
    /// drift — the shape that produced an invisible window here already.
    /// How to switch VT, if a session that can is running.
    ///
    /// ★ THE ESCAPE HATCH, AND IT WAS NEVER CONNECTED. `change_vt` is
    /// implemented on the session (`logind.rs`) and was called from nowhere;
    /// `input.rs` recognised Ctrl+Alt+F<n>, counted it, and forwarded.
    ///
    /// That is worse on this seat than on most, and the reason is measured:
    /// logind's `TakeControl` puts the VT in `K_OFF`, so the KERNEL is not
    /// processing VT-switch keys either. With the compositor also not acting on
    /// them, Ctrl+Alt+F2 reaches nothing at all — the only way out of a wedged
    /// seat is ssh, on a machine whose console IS the seat.
    ///
    /// A boxed closure rather than a session handle because `Omoya` must not be
    /// generic over the session type for one call site.
    pub vt_switch: Option<Box<dyn FnMut(i32) -> Result<(), String>>>,
    pub dmabuf_state: DmabufState,
    /// `Some` once a renderer has advertised its formats. `None` means clients
    /// see `wl_shm` only, which is a working seat rather than a broken one.
    pub dmabuf_global: Option<DmabufGlobal>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Omoya>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,

    /// Where the pointer is, in logical coordinates.
    ///
    /// ── ★ WHY THIS HAS TO BE STATE ────────────────────────────────────────
    /// A tablet reports an ABSOLUTE position, so the handler can compute the
    /// location from the event alone and needs no memory. A mouse reports a
    /// DELTA, and a delta is meaningless without the point it moves from — so
    /// the compositor is the only thing that can hold it.
    ///
    /// Its absence is exactly why relative motion went unhandled: the
    /// `PointerMotionAbsolute` arm could be written without touching state,
    /// and the mouse arm could not, so the mouse arm was never written.
    pub pointer_location: Point<f64, Logical>,
}

impl Omoya {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: Display<Self>, mode: SeatMode) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "omoya");
        // Repeat rate/delay: the fleet's, not the toolkit default. 25/600 is
        // deliberately slower to repeat than a gaming default — a greeter
        // repeating a held key into a password field is a hazard, and the
        // typed answer (awase's `KeyRepeatGate`) lands with the entrance face.
        seat.add_keyboard(Default::default(), 600, 25)
            .expect("a seat without a keyboard cannot authenticate anyone");
        seat.add_pointer();

        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        let reserved = awase::Reserved::fleet_linux();
        tracing::info!(
            mode = mode.name(),
            reserved_chords = reserved.len(),
            "omoya starting"
        );

        Self {
            start_time,
            display_handle: dh,
            // (0, 0) until the first motion. Not centred on the output,
            // because at construction there is no output yet — the DRM backend
            // adds one later, and a "centre" computed before that would be a
            // guess dressed as a position.
            pointer_location: (0.0, 0.0).into(),
            mode,
            reserved,
            owed_vt_switches: 0,
            space,
            tiling: crate::layout::Tiling::default(),
            loop_signal,
            socket_name,
            compositor_state,
            xdg_shell_state,
            shm_state,
            vt_switch: None,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
        }
    }

    fn init_wayland_listener(
        display: Display<Omoya>,
        event_loop: &mut EventLoop<CalloopData>,
    ) -> OsString {
        let listening_socket =
            ListeningSocketSource::new_auto().expect("could not create a wayland socket");
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .expect("could not accept a client");
            })
            .expect("failed to init the wayland event source");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // SAFETY: the display is not dropped here — smithay's own
                    // documented pattern for driving client dispatch from
                    // calloop.
                    unsafe {
                        display
                            .get_mut()
                            .dispatch_clients(&mut state.state)
                            .expect("client dispatch failed");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("failed to insert the display source");

        socket_name
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    use super::SeatMode;
    use omoya_spec::ModeState;

    #[test]
    fn the_two_launchable_modes_parse() {
        assert_eq!(SeatMode::parse("entrance"), Ok(SeatMode::Entrance));
        assert_eq!(SeatMode::parse("session"), Ok(SeatMode::Session));
    }

    /// ★ `lock` is refused with a REASON, not merely rejected as unknown.
    ///
    /// Lock is in-process session state (OMOYA.md §4.2); accepting `--mode
    /// lock` would advertise a spawn shape the design rejects, and rejecting it
    /// as "unknown mode" would hide why.
    #[test]
    fn lock_is_refused_as_a_launchable_mode_and_says_why() {
        let err = SeatMode::parse("lock").unwrap_err();
        assert!(err.contains("in-process"), "{err}");
    }

    #[test]
    fn an_unknown_mode_lists_the_real_ones() {
        let err = SeatMode::parse("kiosk").unwrap_err();
        assert!(err.contains("entrance"));
        assert!(err.contains("session"));
    }

    #[test]
    fn mode_names_match_omoya_spec() {
        assert_eq!(SeatMode::Entrance.name(), omoya_spec::Entrance::NAME);
        assert_eq!(SeatMode::Session.name(), omoya_spec::Session::NAME);
    }
}
