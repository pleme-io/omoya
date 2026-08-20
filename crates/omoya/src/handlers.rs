//! The smithay handler set.
//!
//! This is the library's contract, not a design surface: smithay dispatches
//! protocol requests through these traits and the `delegate_*` macros wire the
//! generated glue. The shape follows smithay's own `smallvil` example, which is
//! what that example is for.
//!
//! **What is deliberately absent: interactive move/resize grabs.** They are
//! ~500 lines in smallvil and prove nothing about whether omoya composites, so
//! M2 omits them. `move_request`/`resize_request` therefore accept the request
//! and do nothing, which is a legal compositor response — a client asking to be
//! dragged is asking, not commanding. Window-management POLICY is omoya's own
//! essence (`theory/OMOYA.md` §3) and lands with the mode faces, not here.

use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{PopupKind, PopupManager, Space, Window, find_popup_root_surface,
        get_popup_toplevel_coords},
    input::{Seat, SeatHandler, SeatState},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
    },
};
use smithay::backend::renderer::utils::on_commit_buffer_handler;

use crate::state::{ClientState, Omoya};

// ── wl_compositor / wl_shm ───────────────────────────────────────────────────

impl CompositorHandler for Omoya {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client is inserted with a ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
            {
                window.on_commit();
            }
        }
        handle_commit(&mut self.popups, &self.space, surface);
    }
}

impl BufferHandler for Omoya {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Omoya {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Omoya);
delegate_shm!(Omoya);

// ── xdg_shell ────────────────────────────────────────────────────────────────

impl XdgShellHandler for Omoya {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window.clone(), (0, 0), true);

        // ★ GIVE IT KEYBOARD FOCUS. Focus used to be set in exactly one place —
        // the pointer-button arm in `input.rs` — so a freshly mapped window had
        // none until it was clicked. On a seat that also draws no cursor, that
        // meant the only way to type into the only window was to click an
        // invisible pointer onto it, and a login therefore produced a terminal
        // that ignored the keyboard.
        //
        // Focus-follows-map is the right default for a seat that has no window
        // management yet: with one window it is unambiguous, and when a second
        // arrives the newest is the one the operator just asked for.
        //
        // `send_pending_configure` is what actually tells the client, because
        // `Activated` is xdg-shell STATE rather than a method — the same
        // pending-state seam the click path uses a few lines down in `input.rs`.
        if let Some(kb) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(t) = window.toplevel() {
                t.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Activated);
                });
                t.send_pending_configure();
            }
            kb.set_focus(self, window.toplevel().map(|t| t.wl_surface().clone()), serial);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// M2: accepted and ignored — see the module header.
    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    /// M2: accepted and ignored — see the module header.
    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

delegate_xdg_shell!(Omoya);

/// Called on every `WlSurface::commit`.
///
/// The load-bearing half is the initial configure: a toplevel that never
/// receives one never maps, and the symptom is a client that starts, connects,
/// and shows nothing — which reads as "the app is broken" rather than "the
/// compositor never answered".
pub fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .expect("a toplevel always carries its surface data")
                .lock()
                .expect("surface data mutex poisoned")
                .initial_configure_sent
        });
        if !initial_configure_sent
            && let Some(toplevel) = window.toplevel()
        {
            toplevel.send_configure();
        }
    }

    popups.commit(surface);
    if let Some(PopupKind::Xdg(xdg)) = popups.find_popup(surface)
        && !xdg.is_initial_configure_sent()
    {
        xdg.send_configure().expect("initial popup configure failed");
    }
}

impl Omoya {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
        else {
            return;
        };
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(window) else {
            return;
        };

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

// ── wl_seat ──────────────────────────────────────────────────────────────────

impl SeatHandler for Omoya {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Omoya> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Omoya);

// ── wl_data_device ───────────────────────────────────────────────────────────

impl SelectionHandler for Omoya {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Omoya {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Omoya {}
impl ServerDndGrabHandler for Omoya {}

delegate_data_device!(Omoya);

// ── wl_output / xdg_output ───────────────────────────────────────────────────

impl OutputHandler for Omoya {}
delegate_output!(Omoya);
