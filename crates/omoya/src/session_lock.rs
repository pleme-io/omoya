use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use omoya_spec::session_lock::{Destroyed, LockId, Requested, SessionLock, Unlocked};
use smithay::backend::renderer::buffer_dimensions;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::{
    ext_session_lock_manager_v1::{self, ExtSessionLockManagerV1},
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
    backend::ClientId,
    protocol::{wl_output::WlOutput, wl_surface::WlSurface},
};
use smithay::utils::{IsAlive, SERIAL_COUNTER, Serial};
use smithay::wayland::compositor::{self, BufferAssignment, SurfaceAttributes};

use crate::state::Omoya;

const ROLE: &str = "ext_session_lock_surface_v1";
const RESPAWN_FIRST: Duration = Duration::from_secs(1);
const RESPAWN_LAST: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct LockData {
    id: LockId,
}

#[derive(Debug)]
pub struct LockSurfaceData {
    surface: Weak<WlSurface>,
}

#[derive(Debug, Default)]
struct Configures {
    pending: Vec<(Serial, (u32, u32))>,
    acked: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct MappedSurface {
    pub lock: LockId,
    pub surface: WlSurface,
    pub object: ExtSessionLockSurfaceV1,
    pub output: WlOutput,
}

#[derive(Debug, Default)]
pub struct LockState {
    pub machine: SessionLock,
    next_id: u64,
    objects: BTreeMap<LockId, ExtSessionLockV1>,
    pub surfaces: Vec<MappedSurface>,
    respawn_at: Option<Instant>,
    respawn_wait: Option<Duration>,
    pub spawned: u64,
}

impl LockState {
    fn mint(&mut self) -> LockId {
        self.next_id += 1;
        LockId(self.next_id)
    }

    #[must_use]
    pub fn surfaces_of_holder(&self) -> Vec<WlSurface> {
        let Some(holder) = self.machine.holder() else {
            return Vec::new();
        };
        self.surfaces
            .iter()
            .filter(|m| m.lock == holder && m.surface.alive())
            .map(|m| m.surface.clone())
            .collect()
    }

    fn arm_respawn(&mut self, now: Instant) {
        self.respawn_wait = None;
        self.respawn_at = Some(now);
    }

    fn respawn_due(&mut self, now: Instant) -> bool {
        if !self.machine.needs_a_client() {
            self.respawn_at = None;
            self.respawn_wait = None;
            return false;
        }
        let at = *self.respawn_at.get_or_insert(now);
        if now < at {
            return false;
        }
        let wait = self
            .respawn_wait
            .map_or(RESPAWN_FIRST, |w| (w * 2).min(RESPAWN_LAST));
        self.respawn_wait = Some(wait);
        self.respawn_at = Some(now + wait);
        true
    }
}

impl GlobalDispatch<ExtSessionLockManagerV1, ()> for Omoya {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtSessionLockManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Omoya {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ExtSessionLockManagerV1,
        request: ext_session_lock_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_manager_v1::Request::Lock { id } => {
                let lock_id = state.session_lock.mint();
                let was_open = !state.session_lock.machine.hides_desktop();
                let lock = data_init.init(id, LockData { id: lock_id });
                match state.session_lock.machine.request(lock_id) {
                    Requested::Finished => lock.finished(),
                    Requested::Granted { locked_now } => {
                        state.session_lock.objects.insert(lock_id, lock.clone());
                        if was_open {
                            state.session_shut();
                        }
                        if locked_now {
                            lock.locked();
                        }
                        state.publish_lock();
                    }
                }
            }
            ext_session_lock_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

fn holds_a_buffer(surface: &WlSurface) -> bool {
    compositor::with_states(surface, |states| {
        let mut guard = states.cached_state.get::<SurfaceAttributes>();
        let pending = matches!(guard.pending().buffer, Some(BufferAssignment::NewBuffer(_)));
        let current = matches!(guard.current().buffer, Some(BufferAssignment::NewBuffer(_)));
        pending || current
    })
}

fn check_commit(surface: &WlSurface, object: &ExtSessionLockSurfaceV1) {
    compositor::with_states(surface, |states| {
        let Some(configures) = states.data_map.get::<Mutex<Configures>>() else {
            return;
        };
        let configures = configures.lock().unwrap_or_else(|e| e.into_inner());
        let Some(size) = configures.acked else {
            object.post_error(
                ext_session_lock_surface_v1::Error::CommitBeforeFirstAck,
                "a commit before the first ack_configure",
            );
            return;
        };
        let mut guard = states.cached_state.get::<SurfaceAttributes>();
        let attrs = guard.pending();
        match attrs.buffer.as_ref() {
            Some(BufferAssignment::Removed) => object.post_error(
                ext_session_lock_surface_v1::Error::NullBuffer,
                "a lock surface attached a null buffer",
            ),
            Some(BufferAssignment::NewBuffer(buffer)) => {
                if let Some(dims) = buffer_dimensions(buffer) {
                    let logical =
                        dims.to_logical(attrs.buffer_scale, attrs.buffer_transform.into());
                    if (logical.w.max(0) as u32, logical.h.max(0) as u32) != size {
                        object.post_error(
                            ext_session_lock_surface_v1::Error::DimensionsMismatch,
                            "a lock surface's buffer does not match its configure",
                        );
                    }
                }
            }
            None => {}
        }
    });
}

impl Dispatch<ExtSessionLockV1, LockData> for Omoya {
    fn request(
        state: &mut Self,
        _client: &Client,
        lock: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &LockData,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_v1::Request::GetLockSurface {
                id,
                surface,
                output,
            } => {
                if compositor::give_role(&surface, ROLE).is_err() {
                    lock.post_error(
                        ext_session_lock_v1::Error::Role,
                        "the surface already has a role",
                    );
                    return;
                }
                let taken = state
                    .session_lock
                    .surfaces
                    .iter()
                    .any(|m| m.lock == data.id && m.output == output);
                if taken {
                    lock.post_error(
                        ext_session_lock_v1::Error::DuplicateOutput,
                        "this lock already has a surface on that output",
                    );
                    return;
                }
                if holds_a_buffer(&surface) {
                    lock.post_error(
                        ext_session_lock_v1::Error::AlreadyConstructed,
                        "the surface already has a buffer attached",
                    );
                    return;
                }
                let object = data_init.init(
                    id,
                    LockSurfaceData {
                        surface: surface.downgrade(),
                    },
                );
                let hook_object = object.clone();
                compositor::add_pre_commit_hook::<Self, _>(
                    &surface,
                    move |_state, _dh, committed| {
                        check_commit(committed, &hook_object);
                    },
                );
                state.session_lock.surfaces.push(MappedSurface {
                    lock: data.id,
                    surface: surface.clone(),
                    object: object.clone(),
                    output: output.clone(),
                });
                state.configure_lock_surface(&surface, &object);
            }
            ext_session_lock_v1::Request::UnlockAndDestroy => {
                match state.session_lock.machine.unlock(data.id) {
                    Unlocked::Opened => state.session_opened(),
                    Unlocked::Refused => lock.post_error(
                        ext_session_lock_v1::Error::InvalidUnlock,
                        "this object does not hold the lock, or was not told it is locked",
                    ),
                }
            }
            ext_session_lock_v1::Request::Destroy => {
                match state.session_lock.machine.destroy(data.id) {
                    Destroyed::Quiet => {}
                    Destroyed::InvalidDestroy => lock.post_error(
                        ext_session_lock_v1::Error::InvalidDestroy,
                        "a held lock is released with unlock_and_destroy",
                    ),
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &ExtSessionLockV1,
        data: &LockData,
    ) {
        state.session_lock.objects.remove(&data.id);
        if state.session_lock.machine.holder_gone(data.id) {
            state.session_lock.arm_respawn(Instant::now());
            state.drop_seat_focus();
            state.owed.mark(crate::owed::Owed::Lock);
            state.publish_lock();
            state
                .introspect
                .lock_holder_deaths
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!("the lock client left without unlocking; the screen stays locked");
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, LockSurfaceData> for Omoya {
    fn request(
        state: &mut Self,
        _client: &Client,
        object: &ExtSessionLockSurfaceV1,
        request: ext_session_lock_surface_v1::Request,
        data: &LockSurfaceData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_surface_v1::Request::AckConfigure { serial } => {
                let Ok(surface) = data.surface.upgrade() else {
                    return;
                };
                let serial = Serial::from(serial);
                let known = compositor::with_states(&surface, |states| {
                    let Some(configures) = states.data_map.get::<Mutex<Configures>>() else {
                        return false;
                    };
                    let mut c = configures.lock().unwrap_or_else(|e| e.into_inner());
                    let Some((_, size)) = c.pending.iter().find(|(s, _)| *s == serial).copied()
                    else {
                        return false;
                    };
                    c.pending.retain(|(s, _)| *s > serial);
                    c.acked = Some(size);
                    true
                });
                if !known {
                    object.post_error(
                        ext_session_lock_surface_v1::Error::InvalidSerial,
                        "an ack of a configure never sent",
                    );
                }
                let _ = state;
            }
            ext_session_lock_surface_v1::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ExtSessionLockSurfaceV1,
        _data: &LockSurfaceData,
    ) {
        state
            .session_lock
            .surfaces
            .retain(|m| m.object != *resource);
        state.owed.mark(crate::owed::Owed::Lock);
    }
}

impl Omoya {
    pub fn create_session_lock_global(display: &DisplayHandle) {
        let _ = display.create_global::<Self, ExtSessionLockManagerV1, ()>(1, ());
    }

    #[must_use]
    pub const fn scene_hidden(&self) -> bool {
        self.session_lock.machine.hides_desktop() || self.panel.shows_nothing()
    }

    fn configure_lock_surface(&mut self, surface: &WlSurface, object: &ExtSessionLockSurfaceV1) {
        let size = self
            .space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map_or((0, 0), |g| (g.size.w.max(0) as u32, g.size.h.max(0) as u32));
        let serial = SERIAL_COUNTER.next_serial();
        compositor::with_states(surface, |states| {
            let holder = states
                .data_map
                .get_or_insert_threadsafe(|| Mutex::new(Configures::default()));
            holder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .push((serial, size));
        });
        if let Some(output) = self.space.outputs().next() {
            output.enter(surface);
        }
        object.configure(u32::from(serial), size.0, size.1);
    }

    pub fn lock_surface_committed(&mut self, surface: &WlSurface) {
        if self
            .session_lock
            .surfaces_of_holder()
            .iter()
            .any(|s| s == surface)
        {
            self.owed.mark(crate::owed::Owed::Lock);
            self.focus_holder_surface();
        }
    }

    pub fn focus_holder_surface(&mut self) {
        let Some(surface) = self.session_lock.surfaces_of_holder().into_iter().next() else {
            return;
        };
        let Some(kb) = self.seat.get_keyboard() else {
            return;
        };
        if kb.current_focus().as_ref() != Some(&surface) {
            kb.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        }
        self.refresh_pointer_focus();
    }

    #[must_use]
    pub fn keyboard_may_reach_a_client(
        &self,
        keyboard: &smithay::input::keyboard::KeyboardHandle<Self>,
    ) -> bool {
        if !self.session_lock.machine.hides_desktop() {
            return true;
        }
        keyboard
            .current_focus()
            .is_some_and(|f| self.session_lock.surfaces_of_holder().contains(&f))
    }

    pub fn drop_seat_focus(&mut self) {
        let serial = SERIAL_COUNTER.next_serial();
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, None, serial);
        }
        self.refresh_pointer_focus();
    }

    pub fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location = self.pointer_location;
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &smithay::input::pointer::MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(self);
    }

    pub fn session_shut(&mut self) {
        if let Some(pointer) = self.seat.get_pointer() {
            let serial = SERIAL_COUNTER.next_serial();
            pointer.unset_grab(self, serial, 0);
        }
        self.active_resize = None;
        self.snap_preview = None;
        self.drop_seat_focus();
        self.owed.mark(crate::owed::Owed::Lock);
        self.introspect
            .lock_engaged
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.power_hint(crate::power::PowerCommand::LockedHint(true));
        self.publish_lock();
        tracing::info!("the session is locked");
    }

    pub fn session_opened(&mut self) {
        self.session_lock.surfaces.clear();
        self.session_lock.arm_respawn(Instant::now());
        self.idle.input(Instant::now());
        self.power_hint(crate::power::PowerCommand::LockedHint(false));
        self.owed.mark(crate::owed::Owed::Lock);
        self.owed.mark(crate::owed::Owed::Windows);
        self.focus_any_visible();
        self.refresh_pointer_focus();
        self.publish_lock();
        tracing::info!("the session is unlocked");
    }

    #[must_use]
    pub fn lock_client(&self) -> Option<Vec<String>> {
        self.config.lock.client.clone().filter(|c| !c.is_empty())
    }

    pub fn lock_session(&mut self) -> bool {
        if self.lock_client().is_none() {
            return false;
        }
        if self.session_lock.machine.compositor_lock() {
            self.session_lock.arm_respawn(Instant::now());
            self.session_shut();
            self.supervise_lock(Instant::now());
        }
        true
    }

    pub fn supervise_lock(&mut self, now: Instant) {
        if !self.session_lock.respawn_due(now) {
            return;
        }
        let Some(client) = self.lock_client() else {
            return;
        };
        self.session_lock.spawned += 1;
        if !crate::spawn::into_seat(&client, &self.socket_name, "lock client") {
            tracing::error!(
                ?client,
                "the lock client did not start; the screen stays locked"
            );
        }
    }

    pub fn announce_lock(&mut self, id: Option<LockId>) {
        if let Some(lock) = id.and_then(|id| self.session_lock.objects.get(&id)) {
            lock.locked();
        }
        self.publish_lock();
        self.finish_sleep_wait();
    }

    pub fn lock_frame_flipped(&mut self) {
        self.session_lock.machine.frame_flipped();
    }

    pub fn frame_retired(&mut self) {
        let id = self.session_lock.machine.frame_retired();
        self.announce_lock(id);
        if self.panel_flip_in_flight {
            self.panel_flip_in_flight = false;
            self.panel_dark_due = true;
        }
    }

    pub fn lock_scene_on_glass(&mut self) {
        let id = self.session_lock.machine.on_glass_now();
        self.announce_lock(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_client_respawns_at_once_then_with_backoff_up_to_a_ceiling() {
        let mut s = LockState::default();
        s.machine.compositor_lock();
        let t0 = Instant::now();
        assert!(s.respawn_due(t0), "the first start is immediate");
        assert!(!s.respawn_due(t0 + Duration::from_millis(500)));
        assert!(s.respawn_due(t0 + Duration::from_secs(1)));
        assert!(!s.respawn_due(t0 + Duration::from_secs(2)));
        assert!(s.respawn_due(t0 + Duration::from_secs(3)));
        let mut at = t0 + Duration::from_secs(3);
        for _ in 0..10 {
            at += RESPAWN_LAST;
            assert!(s.respawn_due(at));
        }
        assert!(!s.respawn_due(at + RESPAWN_LAST - Duration::from_secs(1)));
    }

    #[test]
    fn a_live_holder_or_an_open_seat_starts_no_client() {
        let mut s = LockState::default();
        assert!(!s.respawn_due(Instant::now()));
        s.machine.request(LockId(1));
        assert!(!s.respawn_due(Instant::now()));
    }

    #[test]
    fn a_new_death_starts_the_backoff_over() {
        let mut s = LockState::default();
        s.machine.compositor_lock();
        let t0 = Instant::now();
        assert!(s.respawn_due(t0));
        assert!(s.respawn_due(t0 + Duration::from_secs(1)));
        s.arm_respawn(t0 + Duration::from_secs(2));
        assert!(s.respawn_due(t0 + Duration::from_secs(2)));
        assert!(!s.respawn_due(t0 + Duration::from_millis(2500)));
    }

    #[test]
    fn the_scene_is_replaced_after_every_other_element_is_gathered() {
        let code: String = include_str!("drm.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let last_gather = code
            .rfind("elements.extend(lower_elements")
            .expect("the scene gathers its lower layers");
        let cut = code
            .find("elements.clear()")
            .expect("a hidden scene clears what was gathered");
        let render = code
            .find("damage_tracker.render_output(")
            .expect("the scene renders");
        assert!(
            last_gather < cut,
            "an element is gathered after the lock cuts the scene"
        );
        assert!(cut < render, "the lock cuts the scene after it renders");
        assert!(code.contains("data.state.scene_hidden()"));
    }

    #[test]
    fn the_seat_never_links_smithays_session_lock() {
        for (name, code) in [
            ("handlers.rs", include_str!("handlers.rs")),
            ("state.rs", include_str!("state.rs")),
            ("main.rs", include_str!("main.rs")),
        ] {
            assert!(
                !code.contains("wayland::session_lock") && !code.contains("delegate_session_lock"),
                "{name} reaches smithay 0.7.0's session_lock, which holds the bypass"
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod wire {
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Once;
    use std::time::{Duration, Instant};

    use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
    use smithay::reexports::calloop::EventLoop;
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::Transform;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_compositor::WlCompositor, wl_output::WlOutput as COutput, wl_registry::WlRegistry,
        wl_surface::WlSurface as CSurface,
    };
    use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
    use wayland_protocols::ext::session_lock::v1::client::{
        ext_session_lock_manager_v1::ExtSessionLockManagerV1 as CManager,
        ext_session_lock_surface_v1::{self as csurf, ExtSessionLockSurfaceV1 as CLockSurface},
        ext_session_lock_v1::{self as clock, ExtSessionLockV1 as CLock},
    };

    use crate::CalloopData;
    use crate::deed::{Deed, DeedOutcome};
    use crate::state::{Omoya, SeatMode};

    static ENV: Once = Once::new();

    fn runtime_dir() {
        ENV.call_once(|| {
            let dir: PathBuf =
                std::env::temp_dir().join(format!("omoya-lock-wire-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir) };
        });
    }

    #[derive(Default)]
    struct Seen {
        locked: bool,
        finished: bool,
        configure: Option<(u32, u32, u32)>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for Seen {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    macro_rules! ignore {
        ($t:ty) => {
            impl Dispatch<$t, ()> for Seen {
                fn event(
                    _: &mut Self,
                    _: &$t,
                    _: <$t as wayland_client::Proxy>::Event,
                    _: &(),
                    _: &Connection,
                    _: &QueueHandle<Self>,
                ) {
                }
            }
        };
    }
    ignore!(WlCompositor);
    ignore!(COutput);
    ignore!(CSurface);
    ignore!(CManager);

    impl Dispatch<CLock, ()> for Seen {
        fn event(
            state: &mut Self,
            _: &CLock,
            event: clock::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                clock::Event::Locked => state.locked = true,
                clock::Event::Finished => state.finished = true,
                _ => {}
            }
        }
    }

    impl Dispatch<CLockSurface, ()> for Seen {
        fn event(
            state: &mut Self,
            _: &CLockSurface,
            event: csurf::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let csurf::Event::Configure {
                serial,
                width,
                height,
            } = event
            {
                state.configure = Some((serial, width, height));
            }
        }
    }

    struct Client {
        conn: Connection,
        queue: EventQueue<Seen>,
        seen: Seen,
        manager: CManager,
        compositor: WlCompositor,
        output: COutput,
    }

    pub(crate) struct Bench {
        pub(crate) event_loop: EventLoop<'static, CalloopData>,
        pub(crate) data: CalloopData,
    }

    impl Bench {
        pub(crate) fn new() -> Self {
            runtime_dir();
            let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new().unwrap();
            let display: Display<Omoya> = Display::new().unwrap();
            let dh = display.handle();
            let introspect = crate::introspect::OmoyaIntrospect::new();
            let state = Omoya::new(&mut event_loop, display, SeatMode::Session, introspect);
            let mut data = CalloopData {
                state,
                display_handle: dh.clone(),
            };
            let output = Output::new(
                "test".to_owned(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "test".to_owned(),
                    model: "test".to_owned(),
                },
            );
            let mode = OutputMode {
                size: (800, 600).into(),
                refresh: 60_000,
            };
            output.change_current_state(
                Some(mode),
                Some(Transform::Normal),
                None,
                Some((0, 0).into()),
            );
            output.set_preferred(mode);
            let _ = output.create_global::<Omoya>(&dh);
            data.state.space.map_output(&output, (0, 0));
            Self { event_loop, data }
        }

        pub(crate) fn connect<T: Send + 'static>(
            &mut self,
            setup: impl FnOnce(UnixStream) -> T + Send + 'static,
        ) -> T {
            let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                .join(&self.data.state.socket_name);
            let stream = UnixStream::connect(path).unwrap();
            let joining = std::thread::spawn(move || setup(stream));
            while !joining.is_finished() {
                self.turn();
            }
            joining.join().unwrap()
        }

        pub(crate) fn turn(&mut self) {
            self.event_loop
                .dispatch(Duration::from_millis(1), &mut self.data)
                .unwrap();
            let _ = self.data.display_handle.flush_clients();
        }

        fn client(&mut self) -> Client {
            self.connect(|stream| {
                let conn = Connection::from_socket(stream).unwrap();
                let (globals, queue) = registry_queue_init::<Seen>(&conn)
                    .map_or_else(|_| panic!("no registry"), |pair| pair);
                let qh = queue.handle();
                let manager: CManager = globals.bind(&qh, 1..=1, ()).unwrap();
                let compositor: WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
                let output: COutput = globals.bind(&qh, 1..=4, ()).unwrap();
                Client {
                    conn,
                    queue,
                    seen: Seen::default(),
                    manager,
                    compositor,
                    output,
                }
            })
        }

        fn pump(&mut self, clients: &mut [&mut Client]) {
            for _ in 0..30 {
                for c in clients.iter_mut() {
                    let _ = c.queue.flush();
                }
                self.event_loop
                    .dispatch(Duration::from_millis(1), &mut self.data)
                    .unwrap();
                let _ = self.data.display_handle.flush_clients();
                for c in clients.iter_mut() {
                    if let Some(guard) = c.queue.prepare_read() {
                        let _ = guard.read();
                    }
                    let _ = c.queue.dispatch_pending(&mut c.seen);
                }
            }
        }

        fn state(&mut self) -> &mut Omoya {
            &mut self.data.state
        }
    }

    impl Client {
        fn lock(&mut self) -> CLock {
            self.manager.lock(&self.queue.handle(), ())
        }

        fn surface(&mut self, lock: &CLock) -> (CSurface, CLockSurface) {
            let qh = self.queue.handle();
            let surface = self.compositor.create_surface(&qh, ());
            let object = lock.get_lock_surface(&surface, &self.output, &qh, ());
            (surface, object)
        }
    }

    #[test]
    fn locked_is_told_only_after_a_lock_frame_retires() {
        let mut b = Bench::new();
        let mut a = b.client();
        let _lock = a.lock();
        b.pump(&mut [&mut a]);
        assert!(b.state().session_lock.machine.hides_desktop());
        assert!(!a.seen.locked, "no frame has reached the glass");
        b.state().lock_frame_flipped();
        b.pump(&mut [&mut a]);
        assert!(!a.seen.locked, "a flip issued is not on the glass");
        b.state().frame_retired();
        b.pump(&mut [&mut a]);
        assert!(a.seen.locked);
    }

    #[test]
    fn the_bypass_a_second_client_is_finished_and_its_unlock_opens_nothing() {
        let mut b = Bench::new();
        let mut a = b.client();
        let _held = a.lock();
        b.pump(&mut [&mut a]);
        b.state().lock_frame_flipped();
        b.state().frame_retired();
        b.pump(&mut [&mut a]);
        assert!(a.seen.locked);

        let mut intruder = b.client();
        let second = intruder.lock();
        b.pump(&mut [&mut a, &mut intruder]);
        assert!(intruder.seen.finished);
        assert!(!intruder.seen.locked);
        second.unlock_and_destroy();
        b.pump(&mut [&mut a, &mut intruder]);
        assert!(
            b.state().session_lock.machine.hides_desktop(),
            "the 0.7.0 bypass: this is where smithay's dispatch unlocks the screen"
        );
        assert!(
            intruder.conn.protocol_error().is_some(),
            "the unlock is a protocol error"
        );
        assert!(a.conn.protocol_error().is_none(), "the holder is untouched");
    }

    #[test]
    fn a_holder_not_yet_told_cannot_unlock() {
        let mut b = Bench::new();
        let mut a = b.client();
        let lock = a.lock();
        b.pump(&mut [&mut a]);
        lock.unlock_and_destroy();
        b.pump(&mut [&mut a]);
        assert!(b.state().session_lock.machine.hides_desktop());
        assert!(a.conn.protocol_error().is_some());
    }

    #[test]
    fn the_holder_unlocks_and_the_seat_opens() {
        let mut b = Bench::new();
        let mut a = b.client();
        let lock = a.lock();
        b.pump(&mut [&mut a]);
        b.state().lock_scene_on_glass();
        b.pump(&mut [&mut a]);
        assert!(a.seen.locked);
        lock.unlock_and_destroy();
        b.pump(&mut [&mut a]);
        assert!(!b.state().session_lock.machine.hides_desktop());
        assert!(a.conn.protocol_error().is_none());
    }

    #[test]
    fn the_crash_a_dead_holder_leaves_the_seat_shut_and_a_new_client_takes_over() {
        let mut b = Bench::new();
        let mut a = b.client();
        let _lock = a.lock();
        b.pump(&mut [&mut a]);
        b.state().lock_scene_on_glass();
        b.pump(&mut [&mut a]);
        assert!(a.seen.locked);
        drop(a);
        let mut late = b.client();
        b.pump(&mut [&mut late]);
        {
            let s = b.state();
            assert!(
                s.session_lock.machine.hides_desktop(),
                "a crash must not open the seat"
            );
            assert!(s.session_lock.machine.needs_a_client());
            assert!(s.surface_under((10.0, 10.0).into()).is_none());
            assert_eq!(
                *s.introspect.lock_state.lock().unwrap(),
                "locked, no client"
            );
            assert_eq!(
                s.introspect
                    .lock_holder_deaths
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
        }
        let takeover = late.lock();
        b.pump(&mut [&mut late]);
        assert!(
            late.seen.locked,
            "the screen is already locked, so it is told at once"
        );
        assert!(!late.seen.finished);
        takeover.unlock_and_destroy();
        b.pump(&mut [&mut late]);
        assert!(!b.state().session_lock.machine.hides_desktop());
    }

    #[test]
    fn the_lock_surface_is_configured_to_the_output_and_takes_the_keys_after_its_ack() {
        let mut b = Bench::new();
        let mut a = b.client();
        let lock = a.lock();
        let (surface, object) = a.surface(&lock);
        b.pump(&mut [&mut a]);
        let (serial, w, h) = a.seen.configure.expect("a configure");
        assert_eq!((w, h), (800, 600));
        object.ack_configure(serial);
        surface.commit();
        b.pump(&mut [&mut a]);
        assert!(
            a.conn.protocol_error().is_none(),
            "{:?}",
            a.conn.protocol_error()
        );
        let s = b.state();
        let kb = s.seat.get_keyboard().unwrap();
        assert!(
            kb.current_focus().is_some(),
            "the holder's surface takes the keyboard"
        );
        assert!(s.keyboard_may_reach_a_client(&kb));
        assert!(s.surface_under((10.0, 10.0).into()).is_some());
    }

    #[test]
    fn a_commit_before_the_ack_is_a_protocol_error_and_changes_nothing() {
        let mut b = Bench::new();
        let mut a = b.client();
        let lock = a.lock();
        let (surface, _object) = a.surface(&lock);
        b.pump(&mut [&mut a]);
        surface.commit();
        b.pump(&mut [&mut a]);
        assert!(a.conn.protocol_error().is_some());
        assert!(b.state().session_lock.machine.hides_desktop());
    }

    #[test]
    fn behind_a_lock_only_the_marked_deeds_run_and_no_key_reaches_a_client_without_a_surface() {
        let mut b = Bench::new();
        let mut a = b.client();
        let _lock = a.lock();
        b.pump(&mut [&mut a]);
        let s = b.state();
        for deed in [
            Deed::Close,
            Deed::SpawnTerminal,
            Deed::Minimize,
            Deed::NextLayout,
        ] {
            assert_eq!(
                s.perform(deed),
                DeedOutcome::Refused("the session is locked")
            );
        }
        s.world.behind_lock.push("spawn_terminal".to_owned());
        assert_ne!(
            s.perform(Deed::SpawnTerminal),
            DeedOutcome::Refused("the session is locked")
        );
        assert_ne!(
            s.perform(Deed::Mute),
            DeedOutcome::Refused("the session is locked")
        );
        let kb = s.seat.get_keyboard().unwrap();
        assert!(!s.keyboard_may_reach_a_client(&kb));
    }

    #[test]
    fn a_compositor_lock_with_no_client_configured_is_refused_and_opens_nothing() {
        let mut b = Bench::new();
        let s = b.state();
        assert!(!s.lock_session());
        assert!(!s.session_lock.machine.hides_desktop());
        assert_eq!(
            s.perform(Deed::Lock),
            DeedOutcome::Refused("this seat has no lock client (lock.client)")
        );
    }

    #[test]
    fn a_compositor_lock_hides_the_seat_before_any_client_exists() {
        let mut b = Bench::new();
        let s = b.state();
        s.config.lock.client = Some(vec!["/nonexistent/lock-client".to_owned()]);
        assert_eq!(s.perform(Deed::Lock), DeedOutcome::Performed);
        assert!(s.session_lock.machine.hides_desktop());
        assert!(s.scene_hidden(), "a lock must hide the scene");
        assert!(s.session_lock.machine.needs_a_client());
        assert_eq!(s.session_lock.spawned, 1, "the client is started at once");
        assert_eq!(
            *s.introspect.lock_state.lock().unwrap(),
            "locked, no client",
            "the leaf must not wait for a tick"
        );
    }

    #[test]
    fn a_blank_panel_wakes_on_input_and_the_press_is_swallowed() {
        let mut b = Bench::new();
        let s = b.state();
        s.blank_panel();
        assert!(s.scene_hidden());
        assert!(s.note_input(), "input wakes a panel that shows nothing");
        assert!(s.panel_wake_due);
        s.panel = omoya_spec::idle::Panel::On;
        assert!(!s.note_input(), "input on a lit panel wakes nothing");
    }

    #[test]
    fn a_sleep_with_no_lock_client_lets_go_of_the_inhibitor_and_stays_open() {
        let mut b = Bench::new();
        let s = b.state();
        s.sleep_starts();
        assert!(!s.sleep_wait);
        assert!(!s.session_lock.machine.hides_desktop());
    }

    #[test]
    fn a_sleep_locks_first_and_waits_for_the_lock_frame_to_retire() {
        let mut b = Bench::new();
        let s = b.state();
        s.config.lock.client = Some(vec!["/nonexistent/lock-client".to_owned()]);
        s.sleep_starts();
        assert!(s.session_lock.machine.hides_desktop());
        assert!(
            s.sleep_wait,
            "the seat holds the sleep until the screen is dark"
        );
        s.lock_frame_flipped();
        assert!(s.sleep_wait);
        s.frame_retired();
        assert!(!s.sleep_wait, "a retired lock frame releases the inhibitor");
    }

    #[test]
    fn a_resume_is_counted_and_returns_through_the_vt_path() {
        let mut b = Bench::new();
        let s = b.state();
        s.sleep_ended();
        assert_eq!(
            s.introspect
                .resumes
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!(!s.presence.is_away());
    }

    #[test]
    fn idle_blanks_then_locks_on_the_seat_tick() {
        let mut b = Bench::new();
        let s = b.state();
        s.config.lock.client = Some(vec!["/nonexistent/lock-client".to_owned()]);
        s.idle.set_policy(omoya_spec::idle::IdlePolicy {
            blank_after: Some(Duration::from_secs(10)),
            lock_after: Some(Duration::from_secs(20)),
        });
        let t0 = Instant::now();
        s.idle.input(t0);
        s.seat_tick(t0 + Duration::from_secs(5));
        assert!(!s.scene_hidden());
        s.seat_tick(t0 + Duration::from_secs(10));
        assert_eq!(s.panel, omoya_spec::idle::Panel::Darkening);
        assert!(!s.session_lock.machine.hides_desktop());
        s.seat_tick(t0 + Duration::from_secs(20));
        assert!(s.session_lock.machine.hides_desktop());
    }
}
