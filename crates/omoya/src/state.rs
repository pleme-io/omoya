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
    /// The keymap. `awase::BindingMap`, not a bespoke table — the fleet rule
    /// is that awase owns keys and a hand-rolled `Keymap` is the violation.
    /// The introspection sidecar, so the compositor can PUBLISH what it did.
    ///
    /// Held by the state rather than only by the render loop because the
    /// layout is decided in `apply_layout`, which the render loop never sees.
    /// A fact that only the renderer can publish is a fact nobody can ask
    /// about until it has already turned into pixels.
    pub introspect: std::sync::Arc<crate::introspect::OmoyaIntrospect>,
    pub bindings: awase::BindingMap<crate::deed::Deed>,
    /// What `Logo+Return` launches — the same command omoya was given after
    /// `--`, so the chord opens another of whatever the seat opened first
    /// rather than a second hardcoded guess at the operator's terminal.
    pub session_command: Option<Vec<String>>,
    /// What `Ctrl+Space` launches — the seat's application launcher, given by
    /// `--launcher <cmd>`.
    ///
    /// A SEPARATE field from `session_command` rather than a reuse, because
    /// the two answer different questions: `session_command` is "what did this
    /// seat open first", and the launcher is a tool the seat offers whether or
    /// not anything was opened. Folding them would mean `Ctrl+Space` on a bare
    /// seat opened a terminal, which is the wrong key doing the right thing —
    /// the operator would learn a chord that silently changes meaning.
    ///
    /// `None` is a real state and is REPORTED at the chord rather than
    /// defaulted: a seat with no launcher configured must say so, not quietly
    /// open something else.
    pub launcher_command: Option<Vec<String>>,
    /// How the windows are arranged. See `layout.rs` — the algebra is
    /// `kukaku`'s, and only "a leaf is a Window" and "a rect is pixels" are
    /// omoya's.
    pub tiling: crate::layout::Tiling,
    pub loop_signal: LoopSignal,

    /// Whether a frame is owed, and why. See [`crate::owed::Owed`].
    ///
    /// ★ **The render loop used to compose a full frame every tick and then
    /// ask whether the damage was empty.** Measured on plo: 38.2% of a core
    /// while presenting ZERO frames — the whole composite paid for, then
    /// discarded, sixty times a second. `mado` had the same defect with the
    /// operands reversed (it decided to skip and rendered anyway), which is
    /// what made this a `mekuri` extraction rather than a local fix.
    ///
    /// A `Gate` is deliberately not `Clone`: one screen, one drain point. Use
    /// [`mekuri::Gate::ledger`] (or `introspect.mark`) to MARK from anywhere;
    /// only the render loop calls `open`.
    pub owed: mekuri::Gate<crate::owed::Owed>,

    /// `wp_presentation` — when a frame actually reached the screen.
    ///
    /// Added because every client that animates smoothly wants it: a seat
    /// without `wp_presentation` forces them all onto frame-callback timing
    /// alone, with no way to learn when a buffer actually reached the screen.
    ///
    /// ★ IT IS ALSO A HYPOTHESIS, AND IT IS LABELLED AS ONE. On the vkms gate
    /// a second `weston-presentation-shm` mapped, stayed alive, logged
    /// nothing and drew nothing, while the layout tree was verifiably correct
    /// (`0,0 512x768 | 512,0 512x768`). A client built around presentation
    /// feedback waiting for a global that never arrives fits that shape — but
    /// **fits is not proves**, and the first instance of the same binary DID
    /// draw, which the hypothesis does not yet explain. The `elements` leaf
    /// beside `windows` is the measurement that settles it either way; this
    /// protocol earns its place on the first reason regardless of the second.
    /// `xdg-decoration` — who draws the titlebar, the client or us.
    ///
    /// ★ THE ANSWER IS "NEITHER", AND THAT IS WHY THE PROTOCOL IS HERE.
    /// A client with no decoration protocol falls back to drawing its OWN
    /// titlebar, and mado deliberately does: it overrides the fleet's
    /// `decorations_linux = false` back to `true` because on GNOME an
    /// undecorated window is "a fixed rectangle you cannot drag or maximize"
    /// (mado `config.rs`, reported 2026-08-17). That is right on GNOME and
    /// exactly wrong here — a tiling compositor OWNS geometry, so drag and
    /// maximize are not the client's to offer, and the titlebar is a
    /// near-white 35px band across the top of a Nord seat.
    ///
    /// Answering `ServerSide` is the protocol-correct way to say "I will
    /// handle it", and every toolkit that speaks the protocol then drops its
    /// own chrome. omoya draws no frame, which for a tiling seat is the
    /// intended look — the window IS its parcel.
    ///
    /// ★ FIXED AT THE COMPOSITOR, NOT IN mado'S CONFIG. Turning mado's flag
    /// off would fix mado on this seat and break it on GNOME, and would do
    /// nothing for any other client. One protocol handler fixes the class.
    /// `wlr-layer-shell` — bars, docks, notifications and lock surfaces.
    ///
    /// ★ THE PREREQUISITE FOR A STATUS BAR, AND FOR TILING AROUND ONE.
    /// A bar is not a toplevel: it anchors to an edge, it is not in the
    /// window cycle, and it RESERVES SPACE the tiler must not use. That
    /// reservation is the whole reason this belongs in the compositor rather
    /// than being a window that happens to sit at the top — a normal window
    /// would be tiled into a half like any other.
    ///
    /// `space_render_elements` already draws layer surfaces (it calls
    /// `layer_map_for_output` itself), so once the shell exists the bar
    /// composites for free; what omoya has to add is honouring the exclusive
    /// zone in `apply_layout`.
    pub layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState,
    pub xdg_decoration_state: smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    pub presentation_state: smithay::wayland::presentation::PresentationState,
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
    pub fn new(
        event_loop: &mut EventLoop<CalloopData>,
        display: Display<Self>,
        mode: SeatMode,
        introspect: std::sync::Arc<crate::introspect::OmoyaIntrospect>,
    ) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        // CLOCK_MONOTONIC. The clock id is not cosmetic: it tells the client
        // which clock the timestamps are on, so naming the wrong one makes
        // every presentation time silently WRONG rather than absent.
        //
        // Built here beside its siblings rather than inline in the struct
        // literal, because `dh` is moved into `display_handle` there.
        let layer_shell_state =
            smithay::wayland::shell::wlr_layer::WlrLayerShellState::new::<Self>(&dh);
        let xdg_decoration_state =
            smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<Self>(&dh);
        let presentation_state =
            smithay::wayland::presentation::PresentationState::new::<Self>(&dh, 1);
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

        // The gate, and the handle the socket thread marks through. Installed
        // before `introspect` is moved into the struct below — the sidecar is
        // already `Arc`-shared with the kanshou thread by this point, so this
        // is the one moment both halves are reachable at once.
        //
        // The first frame is owed unconditionally: nothing has committed yet,
        // and a seat that comes up showing whatever the framebuffer happened
        // to contain — waiting for a client to dirty it — is a black screen
        // with no error.
        let owed: mekuri::Gate<crate::owed::Owed> = mekuri::Gate::new();
        owed.mark(crate::owed::Owed::Resume);
        if introspect.owed.set(owed.ledger()).is_err() {
            // Only reachable if a second Omoya were built against one
            // sidecar. Reported rather than ignored: it would mean the socket
            // thread marks a gate nobody drains, and every `do` verb would
            // queue forever while reporting success.
            tracing::error!("the introspect sidecar already had a mekuri ledger installed");
        }

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
            introspect,
            owed,
            tiling: crate::layout::Tiling::default(),
            bindings: {
                let (map, clashes) = crate::deed::default_bindings();
                // Reported, never fatal. A keymap typo must not take down the
                // seat during login — but it must not be invisible either,
                // because a chord bound twice runs whichever line came last
                // and the source stops describing the behaviour.
                if !clashes.is_empty() {
                    tracing::error!(?clashes, "duplicate key bindings — later ones were refused");
                }
                map
            },
            session_command: None,
            launcher_command: None,
            loop_signal,
            socket_name,
            layer_shell_state,
            xdg_decoration_state,
            presentation_state,
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
