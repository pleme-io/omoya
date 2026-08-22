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
        // ★ BEFORE `on_commit_buffer_handler`, AND THAT ORDER IS THE WHOLE
        // MECHANISM. That call DRAINS `SurfaceAttributes::damage` into the
        // renderer state; refining afterwards would edit a field nothing
        // reads and look exactly like working.
        //
        // A wgpu client cannot declare damage — `present()` takes no damage
        // argument — so every one of them says "the whole surface changed" on
        // every frame. Measured on plo: that turned one keystroke into a
        // 4,207 us frame against a 2,778 us vblank interval. This compares the
        // committed pixels against what the surface last committed and
        // replaces the declaration with the truth. It can only ever SHRINK,
        // and it refuses outright whenever the comparison is not meaningful.
        //
        // ★ TOPLEVELS AND ASYNC SUBSURFACES ONLY. A SYNC subsurface's state is
        // cached and applied on its PARENT's commit, so `current()` here is not
        // the generation that will be shown — refining it would compare the
        // wrong pixels and could under-damage, which is the one direction that
        // leaves stale pixels on a screen. Skipping costs nothing on this seat:
        // the clients whose frames are expensive are toplevels.
        if !is_sync_subsurface(surface)
            && let Some(v) = crate::truedamage::refine_commit(surface, &mut self.shadows)
        {
            self.introspect.publish_truedamage(&self.shadows, &v);
        }
        on_commit_buffer_handler::<Self>(surface);
        // ★ THE ORDINARY REASON A FRAME EXISTS. Every pixel a client changes
        // arrives through here, so this one line is what keeps the seat
        // painting at all — the render loop no longer composes speculatively
        // and will sit idle forever without it.
        self.owed.mark(crate::owed::Owed::Commit);
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

    /// ★ RELEASE THE SHADOW. `truedamage` keeps a full copy of each surface's
    /// last committed pixels — ~8 MB for a fullscreen window — and a shadow
    /// that outlives its surface is a leak whose only symptom is RSS growth on
    /// a seat that stays up for weeks. `td_shadows` publishes the count so the
    /// leak would be observable, but not leaking is better than observing it.
    fn destroyed(&mut self, surface: &WlSurface) {
        use smithay::reexports::wayland_server::Resource as _;
        self.shadows.forget(surface.id().protocol_id());
        self.introspect
            .td_shadows
            .store(self.shadows.len() as u64, std::sync::atomic::Ordering::Relaxed);
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
        // ★ THE TREE DECIDES WHERE IT GOES, NOT (0, 0).
        //
        // Every toplevel used to be mapped at the origin, full stop — so a
        // second window landed exactly on top of the first and the seat had
        // no window management at all, only a stack of one visible thing.
        // `Tiling::map` splits whatever holds focus; `apply_layout` is what
        // turns the resulting tree into positions and configures.
        self.tiling.map(window.clone());
        self.space.map_element(window.clone(), (0, 0), true);
        self.apply_layout();

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

    /// ★ NOTHING IMPLEMENTED THIS, SO A CLOSED WINDOW NEVER LEFT.
    ///
    /// `Space` does not unmap on its own — it holds the `Window` handle until
    /// told otherwise. Without this arm a client that exited stayed in the
    /// element list forever: it kept its slot in the layout, kept being asked
    /// to render (drawing nothing, since its buffers were gone), and kept
    /// keyboard focus if it had it. The visible symptom is a dead rectangle
    /// that swallows every keystroke, which reads as the compositor hanging.
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t == &surface))
            .cloned()
        else {
            return;
        };
        self.space.unmap_elem(&window);
        self.tiling.unmap(&window);
        self.apply_layout();

        // Hand focus to whatever survives, or the seat becomes untypeable —
        // the same failure `new_toplevel`'s focus-on-map exists to prevent,
        // reached from the other direction.
        let next = self.tiling.focused();
        if let Some(kb) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(w) = next.as_ref() {
                if let Some(t) = w.toplevel() {
                    t.with_pending_state(|state| {
                        state.states.set(xdg_toplevel::State::Activated);
                    });
                    t.send_pending_configure();
                }
            }
            let focus = next
                .as_ref()
                .and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
            kb.set_focus(self, focus, serial);
        }
    }
}

delegate_xdg_shell!(Omoya);
// `wp_presentation` — see `Omoya::presentation_state`. No handler trait to
// implement: the protocol is pure output, so the delegate is the whole wiring.
smithay::delegate_presentation!(Omoya);

// ── xdg-decoration ───────────────────────────────────────────────────────
//
// See `Omoya::xdg_decoration_state`. Every arm answers `ServerSide`,
// including `request_mode` when a client ASKS for client-side: on a tiling
// seat the compositor owns geometry, so a client-drawn titlebar is chrome
// for operations it cannot perform. The protocol lets us say so rather than
// leaving the client to guess, which is what produced a near-white 35px band
// across a Nord desktop.
impl smithay::wayland::shell::xdg::decoration::XdgDecorationHandler for Omoya {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(
                smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide,
            );
        });
        toplevel.send_pending_configure();
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        // Deliberately ignores what the client asked for. The protocol allows
        // the compositor the final say, and a tiling seat has a real reason
        // to use it — honouring a ClientSide request here would put back the
        // titlebar this exists to remove.
        self.new_decoration(toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        // "No preference" still means server-side here, for the same reason.
        self.new_decoration(toplevel);
    }
}
smithay::delegate_xdg_decoration!(Omoya);

// ── wlr-layer-shell ──────────────────────────────────────────────────────
//
// See `Omoya::layer_shell_state`. A layer surface is mapped into the
// output's `LayerMap`, which computes exclusive zones; `apply_layout` then
// tiles inside `non_exclusive_zone()` so a bar reserves its strip and the
// windows take what is left.
impl smithay::wayland::shell::wlr_layer::WlrLayerShellHandler for Omoya {
    fn shell_state(&mut self) -> &mut smithay::wayland::shell::wlr_layer::WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: smithay::wayland::shell::wlr_layer::LayerSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: smithay::wayland::shell::wlr_layer::Layer,
        namespace: String,
    ) {
        // One output today, so the client's requested output is ignored
        // rather than honoured-or-refused. When a second lands this becomes a
        // lookup; naming it here so the single-output assumption is visible.
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        // ★ TWO `LayerSurface` TYPES, AND THEY ARE NOT INTERCHANGEABLE.
        // The handler is handed the PROTOCOL object
        // (`wayland::shell::wlr_layer::LayerSurface`); `LayerMap` stores
        // `desktop::LayerSurface`, the desktop-layer wrapper that carries an
        // id, the namespace and a userdata map. `desktop::LayerSurface::new`
        // is the bridge, and the namespace is what a bar is identified by —
        // which is why it is taken rather than ignored.
        let desktop_surface = smithay::desktop::LayerSurface::new(surface, namespace);
        let mut map = smithay::desktop::layer_map_for_output(&output);
        if map.map_layer(&desktop_surface).is_err() {
            tracing::warn!("a layer surface could not be mapped");
            return;
        }
        drop(map);
        // The exclusive zone may have changed, so the tiling has to be
        // recomputed — this is the line that makes a bar actually reserve
        // space instead of being drawn over the windows.
        self.apply_layout();
    }

    fn layer_destroyed(&mut self, surface: smithay::wayland::shell::wlr_layer::LayerSurface) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        {
            let mut map = smithay::desktop::layer_map_for_output(&output);
            // Find the wrapper the map is holding — the handler only has the
            // protocol object, and the two are different types.
            let held = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(l) = held {
                map.unmap_layer(&l);
            }
        }
        // Give the space back. Without this a bar that exits leaves a strip
        // of permanently unused screen, which reads as a rendering bug.
        self.apply_layout();
    }
}
smithay::delegate_layer_shell!(Omoya);

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

// ── zwp_linux_dmabuf_v1 ─────────────────────────────────────────────────────

impl smithay::wayland::dmabuf::DmabufHandler for Omoya {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }

    /// Accept or refuse a client's buffer.
    ///
    /// ★ THE ANSWER COMES FROM THE RENDERER, NOT FROM A LIST HERE.
    /// `NuriRenderer::accepts` is the single predicate; asking it means the
    /// protocol layer cannot say yes to something the raster layer will later
    /// refuse. That divergence does not error — it produces an accepted buffer
    /// that textures to nothing, i.e. an invisible window with a clean
    /// protocol log.
    ///
    /// smithay validates the FOURCC against the advertised set but passes the
    /// client's MODIFIER through untouched, so refusing a non-linear modifier
    /// is ours to do and not optional.
    ///
    /// `notifier.successful()` can fail if the client died between sending the
    /// buffer and this call; that is a race, not an error, and is ignored
    /// deliberately.
    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        if let Err(e) = crate::nuri_renderer::NuriRenderer::accepts(&dmabuf) {
            tracing::debug!(error = %e, "refused a client dmabuf");
            notifier.failed();
            return;
        }
        let _ = notifier.successful::<Omoya>();
    }
}

smithay::delegate_dmabuf!(Omoya);
