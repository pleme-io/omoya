use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Clock, Monotonic};
use smithay::wayland::shm::{BufferData, with_buffer_contents, with_buffer_contents_mut};

use crate::state::Omoya;

const MANAGER_VERSION: u32 = 3;
const BYTES_PER_PIXEL: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Region {
    #[must_use]
    pub fn inside(self, output: (i32, i32)) -> Self {
        let x = self.x.clamp(0, output.0);
        let y = self.y.clamp(0, output.1);
        let right = self.x.saturating_add(self.w).clamp(x, output.0);
        let bottom = self.y.saturating_add(self.h).clamp(y, output.1);
        Self {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

#[derive(Debug)]
pub struct FrameData {
    region: Region,
}

#[derive(Debug)]
pub struct PendingCopy {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    region: Region,
    with_damage: bool,
}

#[derive(Debug, Default)]
pub struct ScreenShots {
    pub pending: Vec<PendingCopy>,
    pub to_file: Option<PathBuf>,
}

impl ScreenShots {
    #[must_use]
    pub fn due(&self) -> bool {
        !self.pending.is_empty() || self.to_file.is_some()
    }
}

#[must_use]
pub fn copy_region(
    source: &[u8],
    source_width: i32,
    region: Region,
    target: &mut [u8],
    target_stride: i32,
) -> bool {
    let row_bytes = (region.w * BYTES_PER_PIXEL) as usize;
    if region.w <= 0 || region.h <= 0 || target_stride < region.w * BYTES_PER_PIXEL {
        return false;
    }
    for row in 0..region.h {
        let from = (((region.y + row) * source_width + region.x) * BYTES_PER_PIXEL) as usize;
        let to = (row * target_stride) as usize;
        let (Some(input), Some(output)) = (
            source.get(from..from + row_bytes),
            target.get_mut(to..to + row_bytes),
        ) else {
            return false;
        };
        output.copy_from_slice(input);
    }
    true
}

#[must_use]
pub fn buffer_fits(data: &BufferData, region: Region) -> bool {
    let format_ok = matches!(
        data.format,
        wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888
    );
    format_ok
        && data.width == region.w
        && data.height == region.h
        && data.stride >= region.w * BYTES_PER_PIXEL
}

#[must_use]
pub fn rgba_of(bgra: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
    }
    out
}

pub fn write_png(path: &Path, bgra: &[u8], size: (u32, u32)) -> Result<(), String> {
    let part = path.with_extension("part");
    let written = encode_png(&part, bgra, size).and_then(|()| {
        std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

fn encode_png(path: &Path, bgra: &[u8], size: (u32, u32)) -> Result<(), String> {
    use std::io::Write as _;
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buffered = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(&mut buffered, size.0, size.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&rgba_of(bgra))
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    buffered.flush().map_err(|e| e.to_string())
}

#[must_use]
pub fn shot_name(unix_secs: u64) -> String {
    format!("omoya-{unix_secs}.png")
}

#[must_use]
pub fn free_shot_path(dir: &Path, unix_secs: u64) -> PathBuf {
    let first = dir.join(shot_name(unix_secs));
    if !first.exists() {
        return first;
    }
    (1u32..)
        .map(|n| dir.join(format!("omoya-{unix_secs}-{n}.png")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[must_use]
pub fn default_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("XDG_PICTURES_DIR") {
        return PathBuf::from(explicit);
    }
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("Pictures")
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Omoya {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Omoya {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = state.shot_output_size();
        let (frame, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, .. } => (
                frame,
                Region {
                    x: 0,
                    y: 0,
                    w: output.0,
                    h: output.1,
                },
            ),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                x,
                y,
                width,
                height,
                ..
            } => (
                frame,
                Region {
                    x,
                    y,
                    w: width,
                    h: height,
                }
                .inside(output),
            ),
            _ => return,
        };
        let frame = data_init.init(frame, FrameData { region });
        if state.session_lock.machine.hides_desktop() || region.w <= 0 || region.h <= 0 {
            frame.failed();
            return;
        }
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            region.w as u32,
            region.h as u32,
            (region.w * BYTES_PER_PIXEL) as u32,
        );
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Omoya {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        let fits = with_buffer_contents(&buffer, |_, _, info| buffer_fits(&info, data.region))
            .unwrap_or(false);
        if !fits {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "the buffer does not match what this frame announced",
            );
            return;
        }
        if state.session_lock.machine.hides_desktop() {
            frame.failed();
            return;
        }
        state.shots.pending.push(PendingCopy {
            frame: frame.clone(),
            buffer,
            region: data.region,
            with_damage,
        });
        state.owed.mark(crate::owed::Owed::Capture);
    }
}

fn deliver(copy: &PendingCopy, source: &[u8], source_width: i32, now: Duration) -> bool {
    let done = with_buffer_contents_mut(&copy.buffer, |ptr, len, info| {
        let bytes = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        let offset = usize::try_from(info.offset).unwrap_or(usize::MAX);
        bytes.get_mut(offset..).is_some_and(|target| {
            copy_region(source, source_width, copy.region, target, info.stride)
        })
    });
    if !matches!(done, Ok(true)) {
        return false;
    }
    if copy.with_damage {
        copy.frame
            .damage(0, 0, copy.region.w as u32, copy.region.h as u32);
    }
    copy.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
    let secs = now.as_secs();
    copy.frame.ready(
        (secs >> 32) as u32,
        (secs & 0xffff_ffff) as u32,
        now.subsec_nanos(),
    );
    true
}

impl Omoya {
    pub fn create_screencopy_global(display: &DisplayHandle) {
        let _ = display.create_global::<Self, ZwlrScreencopyManagerV1, ()>(MANAGER_VERSION, ());
    }

    #[must_use]
    pub fn shot_output_size(&self) -> (i32, i32) {
        self.space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map_or((0, 0), |g| (g.size.w, g.size.h))
    }

    pub fn fail_pending_shots(&mut self) {
        for copy in self.shots.pending.drain(..) {
            copy.frame.failed();
        }
        self.shots.to_file = None;
    }

    pub fn deliver_shots(&mut self, bgra: &[u8], size: (i32, i32)) {
        let locked = self.session_lock.machine.hides_desktop() || self.panel.shows_nothing();
        if locked {
            self.fail_pending_shots();
            return;
        }
        let now: Duration = Clock::<Monotonic>::new().now().into();
        for copy in self.shots.pending.drain(..) {
            if !deliver(&copy, bgra, size.0, now) {
                copy.frame.failed();
            }
        }
        let Some(dir) = self.shots.to_file.take() else {
            return;
        };
        let pixels = bgra.to_vec();
        let sender = self.world.seat_tx.clone();
        let wide = u32::try_from(size.0).unwrap_or(0);
        let tall = u32::try_from(size.1).unwrap_or(0);
        let spawned = std::thread::Builder::new()
            .name("omoya-screenshot".to_owned())
            .spawn(move || {
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let result = std::fs::create_dir_all(&dir)
                    .map_err(|e| e.to_string())
                    .and_then(|()| write_png(&free_shot_path(&dir, secs), &pixels, (wide, tall)));
                if let Some(sender) = sender {
                    let message = match result {
                        Ok(()) => crate::world::SeatMessage::Flash("shot saved".to_owned()),
                        Err(why) => crate::world::SeatMessage::PowerRefused(format!("shot: {why}")),
                    };
                    let _ = sender.send(message);
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "no thread for the screenshot");
        }
    }

    pub fn screenshot(&mut self, dir: Option<PathBuf>) {
        self.shots.to_file = Some(dir.unwrap_or_else(default_dir));
        self.owed.mark(crate::owed::Owed::Capture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omoya-shots-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_shot_that_fails_leaves_nothing_a_reader_could_open() {
        let dir = shots_dir("failed");
        let path = dir.join("a.png");
        assert!(write_png(&path, &[0u8; 4], (2, 2)).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shot_appears_whole_and_only_under_its_name() {
        let dir = shots_dir("whole");
        let path = dir.join("a.png");
        write_png(&path, &[9u8; 16], (2, 2)).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a.png")]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_shots_in_one_second_are_two_files() {
        let dir = shots_dir("second");
        let first = free_shot_path(&dir, 7);
        std::fs::write(&first, b"x").unwrap();
        let second = free_shot_path(&dir, 7);
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_region_stays_inside_the_output() {
        let full = (1920, 1200);
        assert_eq!(
            Region {
                x: 10,
                y: 20,
                w: 30,
                h: 40
            }
            .inside(full),
            Region {
                x: 10,
                y: 20,
                w: 30,
                h: 40
            }
        );
        assert_eq!(
            Region {
                x: -5,
                y: 1190,
                w: 100,
                h: 100
            }
            .inside(full),
            Region {
                x: 0,
                y: 1190,
                w: 95,
                h: 10
            }
        );
        let off = Region {
            x: 3000,
            y: 0,
            w: 100,
            h: 100,
        }
        .inside(full);
        assert_eq!((off.w, off.h), (0, 100));
    }

    fn image(w: i32, h: i32) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&[x as u8, y as u8, 0xee, 0xff]);
            }
        }
        out
    }

    #[test]
    fn a_region_copies_the_right_pixels_at_the_targets_stride() {
        let source = image(10, 10);
        let region = Region {
            x: 2,
            y: 3,
            w: 2,
            h: 2,
        };
        let mut target = vec![0u8; 2 * 12];
        assert!(copy_region(&source, 10, region, &mut target, 12));
        assert_eq!(&target[0..8], &[2, 3, 0xee, 0xff, 3, 3, 0xee, 0xff]);
        assert_eq!(&target[12..20], &[2, 4, 0xee, 0xff, 3, 4, 0xee, 0xff]);
        assert_eq!(
            &target[8..12],
            &[0, 0, 0, 0],
            "the stride's gap stays untouched"
        );
    }

    #[test]
    fn a_copy_that_cannot_fit_says_so_and_writes_past_nothing() {
        let source = image(4, 4);
        let region = Region {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        };
        let mut small = vec![0u8; 8];
        assert!(!copy_region(&source, 4, region, &mut small, 16));
        assert!(
            !copy_region(&source, 4, region, &mut vec![0; 64], 4),
            "a stride under the row"
        );
        let beyond = Region {
            x: 0,
            y: 3,
            w: 4,
            h: 4,
        };
        assert!(!copy_region(&source, 4, beyond, &mut vec![0; 64], 16));
        assert!(!copy_region(
            &source,
            4,
            Region {
                x: 0,
                y: 0,
                w: 0,
                h: 4
            },
            &mut [],
            16
        ));
    }

    #[test]
    fn a_shot_decodes_back_to_the_pixels_it_was_given() {
        let dir = std::env::temp_dir().join(format!("omoya-shot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let bgra = [10u8, 20, 30, 0, 40, 50, 60, 0];
        write_png(&path, &bgra, (2, 1)).unwrap();
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..8], &[30, 20, 10, 0xff, 60, 50, 40, 0xff]);
    }

    #[test]
    fn a_shot_outside_any_folder_it_can_write_is_an_error() {
        assert!(write_png(Path::new("/nonexistent/folder/t.png"), &[0; 4], (1, 1)).is_err());
    }
}

#[cfg(test)]
mod wire {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::fd::{AsFd, FromRawFd, OwnedFd};

    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_buffer::WlBuffer as CBuffer,
        wl_output::WlOutput as COutput,
        wl_registry::WlRegistry,
        wl_shm::{self as cshm, WlShm},
        wl_shm_pool::WlShmPool,
    };
    use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
    use wayland_protocols_wlr::screencopy::v1::client::{
        zwlr_screencopy_frame_v1::{self as cframe, ZwlrScreencopyFrameV1 as CFrame},
        zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 as CManager,
    };

    use crate::session_lock::wire::Bench;

    #[derive(Default)]
    struct Seen {
        buffer: Option<(u32, u32, u32)>,
        ready: bool,
        failed: bool,
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
    ignore!(CManager);
    ignore!(COutput);
    ignore!(WlShm);
    ignore!(WlShmPool);
    ignore!(CBuffer);

    impl Dispatch<CFrame, ()> for Seen {
        fn event(
            state: &mut Self,
            _: &CFrame,
            event: cframe::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                cframe::Event::Buffer {
                    width,
                    height,
                    stride,
                    ..
                } => state.buffer = Some((width, height, stride)),
                cframe::Event::Ready { .. } => state.ready = true,
                cframe::Event::Failed => state.failed = true,
                _ => {}
            }
        }
    }

    struct Client {
        conn: Connection,
        queue: EventQueue<Seen>,
        seen: Seen,
        manager: CManager,
        output: COutput,
        shm: WlShm,
    }

    fn client(b: &mut Bench) -> Client {
        b.connect(|stream| {
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, queue) =
                registry_queue_init::<Seen>(&conn).map_or_else(|_| panic!("no registry"), |p| p);
            let qh = queue.handle();
            let manager: CManager = globals.bind(&qh, 1..=3, ()).unwrap();
            let output: COutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let shm: WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            Client {
                conn,
                queue,
                seen: Seen::default(),
                manager,
                output,
                shm,
            }
        })
    }

    fn pump(b: &mut Bench, c: &mut Client) {
        for _ in 0..30 {
            let _ = c.queue.flush();
            b.turn();
            if let Some(guard) = c.queue.prepare_read() {
                let _ = guard.read();
            }
            let _ = c.queue.dispatch_pending(&mut c.seen);
        }
    }

    fn shm_file(size: i32) -> std::fs::File {
        let name = c"omoya-test";
        let fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
        assert!(fd >= 0);
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let file = std::fs::File::from(owned);
        file.set_len(u64::try_from(size).unwrap()).unwrap();
        file
    }

    fn buffer(c: &Client, file: &std::fs::File, w: i32, h: i32) -> CBuffer {
        let qh = c.queue.handle();
        let pool = c.shm.create_pool(file.as_fd(), w * h * 4, &qh, ());
        pool.create_buffer(0, w, h, w * 4, cshm::Format::Xrgb8888, &qh, ())
    }

    fn pixels(w: i32, h: i32) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&[(x % 251) as u8, (y % 251) as u8, 0x5a, 0xff]);
            }
        }
        out
    }

    #[test]
    fn grim_gets_the_composed_frame_in_its_own_buffer() {
        let mut b = Bench::new();
        let mut c = client(&mut b);
        let frame = c
            .manager
            .capture_output(0, &c.output, &c.queue.handle(), ());
        pump(&mut b, &mut c);
        assert_eq!(c.seen.buffer, Some((800, 600, 3200)));
        let mut file = shm_file(800 * 600 * 4);
        let target = buffer(&c, &file, 800, 600);
        frame.copy(&target);
        pump(&mut b, &mut c);
        assert!(!c.seen.ready, "the copy waits for a frame");
        assert!(b.data.state.shots.due());
        b.data.state.deliver_shots(&pixels(800, 600), (800, 600));
        pump(&mut b, &mut c);
        assert!(c.seen.ready);
        file.seek(SeekFrom::Start(u64::from(3200u32) * 3 + 8))
            .unwrap();
        let mut px = [0u8; 4];
        file.read_exact(&mut px).unwrap();
        assert_eq!(px, [2, 3, 0x5a, 0xff]);
    }

    #[test]
    fn a_region_is_copied_from_its_own_place() {
        let mut b = Bench::new();
        let mut c = client(&mut b);
        let frame =
            c.manager
                .capture_output_region(0, &c.output, 10, 20, 4, 2, &c.queue.handle(), ());
        pump(&mut b, &mut c);
        assert_eq!(c.seen.buffer, Some((4, 2, 16)));
        let mut file = shm_file(4 * 2 * 4);
        let target = buffer(&c, &file, 4, 2);
        frame.copy(&target);
        pump(&mut b, &mut c);
        b.data.state.deliver_shots(&pixels(800, 600), (800, 600));
        pump(&mut b, &mut c);
        assert!(c.seen.ready);
        let mut px = [0u8; 4];
        file.seek(SeekFrom::Start(0)).unwrap();
        file.read_exact(&mut px).unwrap();
        assert_eq!(px, [10, 20, 0x5a, 0xff]);
    }

    #[test]
    fn a_wrong_buffer_is_a_protocol_error() {
        let mut b = Bench::new();
        let mut c = client(&mut b);
        let frame = c
            .manager
            .capture_output(0, &c.output, &c.queue.handle(), ());
        pump(&mut b, &mut c);
        let file = shm_file(4 * 4 * 4);
        let target = buffer(&c, &file, 4, 4);
        frame.copy(&target);
        pump(&mut b, &mut c);
        assert!(c.conn.protocol_error().is_some());
        assert!(!b.data.state.shots.due());
    }

    #[test]
    fn a_locked_seat_gives_no_frame_to_anyone() {
        let mut b = Bench::new();
        let mut c = client(&mut b);
        b.data.state.session_lock.machine.compositor_lock();
        let _frame = c
            .manager
            .capture_output(0, &c.output, &c.queue.handle(), ());
        pump(&mut b, &mut c);
        assert!(c.seen.failed);
        assert_eq!(c.seen.buffer, None);
    }

    #[test]
    fn a_lock_that_lands_after_the_copy_asked_still_gives_no_frame() {
        let mut b = Bench::new();
        let mut c = client(&mut b);
        let frame = c
            .manager
            .capture_output(0, &c.output, &c.queue.handle(), ());
        pump(&mut b, &mut c);
        let file = shm_file(800 * 600 * 4);
        let target = buffer(&c, &file, 800, 600);
        frame.copy(&target);
        pump(&mut b, &mut c);
        assert!(b.data.state.shots.due());
        b.data.state.session_lock.machine.compositor_lock();
        b.data.state.deliver_shots(&pixels(800, 600), (800, 600));
        pump(&mut b, &mut c);
        assert!(c.seen.failed);
        assert!(!c.seen.ready);
        assert!(!b.data.state.shots.due());
    }

    #[test]
    fn a_screenshot_is_refused_behind_a_lock_by_the_deed_and_by_a_command() {
        use crate::deed::{Deed, DeedOutcome, SeatCommand};
        let mut b = Bench::new();
        let s = &mut b.data.state;
        s.world
            .commands
            .insert("shot".to_owned(), SeatCommand::Screenshot("/tmp".into()));
        s.world.behind_lock.push("shot".to_owned());
        s.session_lock.machine.compositor_lock();
        let refused = DeedOutcome::Refused("the session is locked");
        assert_eq!(s.perform(Deed::Screenshot), refused);
        assert_eq!(s.perform(Deed::Command("shot".to_owned())), refused);
        assert!(!s.shots.due());
    }
}
