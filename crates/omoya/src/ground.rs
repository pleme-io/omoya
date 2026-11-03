use std::io::BufReader;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::SystemTime;

use smithay::backend::renderer::damage::OutputDamageTracker;

use crate::nuri_renderer::PaintsGround;

const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("cannot read it: {0}")]
    Unreadable(#[from] std::io::Error),
    #[error("not a PNG this seat can decode: {0}")]
    Invalid(#[from] png::DecodingError),
    #[error("too large to decode")]
    TooLarge,
}

#[derive(Debug)]
pub struct Painted {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

impl Painted {
    #[must_use]
    pub const fn width(&self) -> i32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> i32 {
        self.height
    }

    #[must_use]
    pub fn stride(&self) -> usize {
        usize::try_from(self.width).unwrap_or(0) * 4
    }

    #[must_use]
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    modified: SystemTime,
}

impl Stamp {
    fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            modified: metadata.modified()?,
        })
    }
}

#[derive(Debug, Default)]
enum Seen {
    #[default]
    Never,
    Absent,
    At(Stamp),
}

#[derive(Debug, Default)]
pub struct Refresh {
    pub repaint: bool,
    pub refused: Option<Refusal>,
}

#[derive(Debug, Default)]
pub struct Ground {
    seen: Seen,
    shown: bool,
    pending: Option<Option<Painted>>,
}

impl Ground {
    #[must_use]
    pub fn refresh(&mut self, path: Option<&Path>, width: i32, height: i32) -> Refresh {
        let Some(path) = path else {
            self.seen = Seen::Never;
            return Refresh {
                repaint: self.go_flat(),
                refused: None,
            };
        };
        let stamp = match Stamp::of(path) {
            Ok(stamp) => stamp,
            Err(_) if matches!(self.seen, Seen::Absent) => return Refresh::default(),
            Err(error) => {
                self.seen = Seen::Absent;
                return Refresh {
                    repaint: self.go_flat(),
                    refused: Some(error.into()),
                };
            }
        };
        if matches!(self.seen, Seen::At(seen) if seen == stamp) {
            return Refresh::default();
        }
        self.seen = Seen::At(stamp);
        match paint(path, width, height) {
            Ok(painted) => {
                self.shown = true;
                self.pending = Some(Some(painted));
                Refresh {
                    repaint: true,
                    refused: None,
                }
            }
            Err(refused) => Refresh {
                repaint: self.go_flat(),
                refused: Some(refused),
            },
        }
    }

    fn go_flat(&mut self) -> bool {
        if !self.shown {
            return false;
        }
        self.shown = false;
        self.pending = Some(None);
        true
    }

    #[must_use]
    fn take_pending(&mut self) -> Option<Option<Painted>> {
        self.pending.take()
    }
}

pub fn adopt<R: PaintsGround>(
    ground: &mut Ground,
    renderer: &mut R,
    tracker: &mut OutputDamageTracker,
) {
    if let Some(next) = ground.take_pending() {
        renderer.set_ground(next);
        *tracker = OutputDamageTracker::from_mode_source(tracker.mode().clone());
    }
}

pub(crate) fn open_regular(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ))
    }
}

fn paint(path: &Path, width: i32, height: i32) -> Result<Painted, Refusal> {
    let image = decode(path)?;
    let bgra = cover(
        image,
        usize::try_from(width).unwrap_or(0),
        usize::try_from(height).unwrap_or(0),
    );
    Ok(Painted {
        width,
        height,
        bgra,
    })
}

#[derive(Debug)]
struct Image {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

fn flat_bgr() -> [u8; 3] {
    let [r, g, b, _] = crate::theme::background_for_surface(false);
    let byte = |channel: f32| (channel * 255.0).round() as u8;
    [byte(b), byte(g), byte(r)]
}

fn decode(path: &Path) -> Result<Image, Refusal> {
    let mut decoder = png::Decoder::new(BufReader::new(open_regular(path)?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let length = reader
        .output_buffer_size()
        .filter(|length| *length <= MAX_DECODED_BYTES)
        .ok_or(Refusal::TooLarge)?;
    let mut raw = vec![0_u8; length];
    let frame = reader.next_frame(&mut raw)?;
    raw.truncate(frame.buffer_size());
    let flat = flat_bgr();
    let samples = frame.color_type.samples();
    let mut bgra = Vec::with_capacity(raw.len() / samples * 4);
    for px in raw.chunks_exact(samples) {
        bgra.extend_from_slice(&opaque(px, flat));
    }
    Ok(Image {
        width: frame.width as usize,
        height: frame.height as usize,
        bgra,
    })
}

fn opaque(px: &[u8], [flat_b, flat_g, flat_r]: [u8; 3]) -> [u8; 4] {
    let (r, g, b, a) = match *px {
        [v] => (v, v, v, 0xff),
        [v, a] => (v, v, v, a),
        [r, g, b] => (r, g, b, 0xff),
        [r, g, b, a] => (r, g, b, a),
        _ => (flat_r, flat_g, flat_b, 0xff),
    };
    let alpha = u32::from(a);
    let over = |top: u8, under: u8| {
        let blended = (u32::from(top) * alpha + u32::from(under) * (255 - alpha) + 127) / 255;
        u8::try_from(blended).unwrap_or(u8::MAX)
    };
    [over(b, flat_b), over(g, flat_g), over(r, flat_r), 0xff]
}

struct Sample {
    near: usize,
    far: usize,
    weight: u32,
}

impl Sample {
    fn at(at: u64, out: u64, crop: u64, origin: u64) -> Self {
        let centre = (((2 * at + 1) * crop) << 16) / (2 * out);
        let position = centre.saturating_sub(1 << 15);
        let index = (position >> 16).min(crop - 1);
        Self {
            near: (origin + index) as usize,
            far: (origin + (index + 1).min(crop - 1)) as usize,
            weight: (position & 0xffff) as u32,
        }
    }
}

fn mix(from: u32, to: u32, weight: u32) -> u32 {
    (from * (0x1_0000 - weight) + to * weight) >> 16
}

fn cover(image: Image, width: usize, height: usize) -> Vec<u8> {
    if (image.width, image.height) == (width, height) {
        return image.bgra;
    }
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (source_w, source_h) = (image.width as u64, image.height as u64);
    let (out_w, out_h) = (width as u64, height as u64);
    let (crop_w, crop_h) = if source_w * out_h > source_h * out_w {
        (
            ((source_h * out_w + out_h / 2) / out_h).clamp(1, source_w),
            source_h,
        )
    } else {
        (
            source_w,
            ((source_w * out_h + out_w / 2) / out_w).clamp(1, source_h),
        )
    };
    let left = (source_w - crop_w) / 2;
    let top = (source_h - crop_h) / 2;
    let columns: Vec<Sample> = (0..out_w)
        .map(|x| Sample::at(x, out_w, crop_w, left))
        .collect();
    let stride = image.width * 4;
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..out_h {
        let row = Sample::at(y, out_h, crop_h, top);
        let (upper, lower) = (row.near * stride, row.far * stride);
        for column in &columns {
            let (near, far) = (column.near * 4, column.far * 4);
            for channel in 0..3 {
                let texel = |at: usize| u32::from(image.bgra[at + channel]);
                let above = mix(texel(upper + near), texel(upper + far), column.weight);
                let below = mix(texel(lower + near), texel(lower + far), column.weight);
                out.push(u8::try_from(mix(above, below, row.weight)).unwrap_or(u8::MAX));
            }
            out.push(0xff);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};

    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("omoya-tests")
            .join(std::process::id().to_string())
            .join(name);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    pub fn fifo(path: &Path) {
        use smithay::reexports::rustix::fs::{CWD, FileType, Mode, mknodat};
        mknodat(CWD, path, FileType::Fifo, Mode::from(0o600), 0).expect("a fifo");
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{FileExt, symlink};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
    use smithay::backend::allocator::{Fourcc, Modifier};
    use smithay::backend::renderer::Bind;
    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::element::memory::{
        MemoryRenderBuffer, MemoryRenderBufferRenderElement,
    };
    use smithay::utils::Transform;

    use super::fixture::{fifo, scratch};
    use super::*;
    use crate::config::FlushPolicy;
    use crate::nuri_renderer::{ArmFlush, NuriRenderer};
    use crate::scanout::SlotAges;

    const RED: [u8; 4] = [0, 0, 0xff, 0xff];
    const GREEN: [u8; 4] = [0, 0xff, 0, 0xff];
    const BLUE: [u8; 4] = [0xff, 0, 0, 0xff];
    const WHITE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
    const BLACK: [u8; 4] = [0, 0, 0, 0xff];

    fn image(width: usize, height: usize, paint: impl Fn(usize, usize) -> [u8; 4]) -> Image {
        let mut bgra = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&paint(x, y));
            }
        }
        Image {
            width,
            height,
            bgra,
        }
    }

    fn encode(path: &Path, width: u32, height: u32, color: png::ColorType, data: &[u8]) {
        encode_at(path, width, height, color, png::BitDepth::Eight, data);
    }

    fn encode_at(
        path: &Path,
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        data: &[u8],
    ) {
        let file = std::fs::File::create(path).expect("a png file");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder
            .write_header()
            .expect("a png header")
            .write_image_data(data)
            .expect("png pixels");
    }

    fn rgba_of(source: &Image) -> Vec<u8> {
        source
            .bgra
            .chunks_exact(4)
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect()
    }

    fn write_image(path: &Path, source: &Image) {
        encode(
            path,
            u32::try_from(source.width).expect("a width"),
            u32::try_from(source.height).expect("a height"),
            png::ColorType::Rgba,
            &rgba_of(source),
        );
    }

    fn stamp_at(path: &Path, seconds: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("a file to stamp")
            .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .expect("a new mtime");
    }

    fn painted(ground: &mut Ground) -> Vec<u8> {
        ground
            .take_pending()
            .expect("a pending ground")
            .expect("an image, not the flat ground")
            .bgra
    }

    #[test]
    fn a_same_size_image_is_copied_straight() {
        let source = image(7, 5, |x, y| {
            [
                u8::try_from(x * 31).expect("a byte"),
                u8::try_from(y * 47).expect("a byte"),
                u8::try_from(x * y).expect("a byte"),
                0xff,
            ]
        });
        let want = source.bgra.clone();
        assert_eq!(cover(source, 7, 5), want);
    }

    #[test]
    fn a_wide_image_keeps_its_centre_and_loses_its_sides() {
        let source = image(400, 100, |x, _| {
            if x < 150 {
                RED
            } else if x < 250 {
                GREEN
            } else {
                BLUE
            }
        });
        let out = cover(source, 100, 100);
        assert_eq!(out.len(), 100 * 100 * 4);
        assert!(out.chunks_exact(4).all(|px| px == GREEN));
    }

    #[test]
    fn a_tall_image_keeps_its_centre_and_loses_top_and_bottom() {
        let source = image(100, 400, |_, y| {
            if y < 150 {
                RED
            } else if y < 250 {
                GREEN
            } else {
                BLUE
            }
        });
        let out = cover(source, 100, 100);
        assert_eq!(out.len(), 100 * 100 * 4);
        assert!(out.chunks_exact(4).all(|px| px == GREEN));
    }

    #[test]
    fn covering_keeps_the_aspect_rather_than_stretching() {
        let source = image(200, 100, |x, y| {
            if (90..110).contains(&x) && (40..60).contains(&y) {
                BLACK
            } else {
                WHITE
            }
        });
        let out = cover(source, 100, 100);
        let black: Vec<(usize, usize)> = out
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, px)| *px == BLACK)
            .map(|(i, _)| (i % 100, i / 100))
            .collect();
        assert_eq!(black.len(), 400, "a stretched square would lose half");
        assert_eq!(black.first(), Some(&(40, 40)));
        assert_eq!(black.last(), Some(&(59, 59)));
    }

    #[test]
    fn a_small_image_scales_up_to_cover_from_its_centre() {
        let source = image(4, 2, |x, _| match x {
            0 => RED,
            3 => BLUE,
            _ => GREEN,
        });
        let out = cover(source, 8, 8);
        assert_eq!(out.len(), 8 * 8 * 4);
        assert!(out.chunks_exact(4).all(|px| px == GREEN));
    }

    #[test]
    fn scaling_down_blends_the_pixels_it_covers() {
        let source = image(2, 2, |x, y| match (x, y) {
            (0, 0) => [0, 0, 0, 0xff],
            (1, 1) => [200, 200, 200, 0xff],
            _ => [100, 100, 100, 0xff],
        });
        assert_eq!(cover(source, 1, 1), vec![100, 100, 100, 0xff]);
    }

    #[test]
    fn transparency_lands_on_the_flat_ground_and_the_result_is_opaque() {
        let dir = scratch("transparent");
        let path = dir.join("g.png");
        encode(
            &path,
            2,
            1,
            png::ColorType::Rgba,
            &[9, 9, 9, 0, 255, 255, 255, 128],
        );
        let mut ground = Ground::default();
        assert!(ground.refresh(Some(&path), 2, 1).repaint);
        let out = painted(&mut ground);
        let [b, g, r] = flat_bgr();
        assert_eq!(out[..4], [b, g, r, 0xff]);
        assert_eq!(out[7], 0xff);
        assert!(out[4] > b && out[4] < 255, "half alpha lands between");
    }

    #[test]
    fn grey_and_sixteen_bit_pngs_decode_to_opaque_eight_bit() {
        let dir = scratch("depths");
        let path = dir.join("g.png");
        encode_at(
            &path,
            1,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[0xab, 0xcd],
        );
        let mut ground = Ground::default();
        assert!(ground.refresh(Some(&path), 1, 1).repaint);
        assert_eq!(painted(&mut ground), vec![0xab, 0xab, 0xab, 0xff]);
    }

    #[test]
    fn it_decodes_once_and_again_only_when_the_mtime_moves() {
        let dir = scratch("decode-once");
        let path = dir.join("g.png");
        write_image(&path, &image(1, 1, |_, _| RED));
        stamp_at(&path, 1000);

        let mut ground = Ground::default();
        let first = ground.refresh(Some(&path), 1, 1);
        assert!(first.repaint);
        assert!(first.refused.is_none());
        assert_eq!(painted(&mut ground), RED.to_vec());

        std::fs::write(&path, b"not a png any more").expect("a rewrite");
        stamp_at(&path, 1000);
        let unchanged = ground.refresh(Some(&path), 1, 1);
        assert!(!unchanged.repaint, "the same stamp must not repaint");
        assert!(unchanged.refused.is_none(), "the same stamp was never read");
        assert!(ground.take_pending().is_none());

        write_image(&path, &image(1, 1, |_, _| BLUE));
        stamp_at(&path, 2000);
        assert!(ground.refresh(Some(&path), 1, 1).repaint);
        assert_eq!(painted(&mut ground), BLUE.to_vec());
    }

    #[test]
    fn a_symlink_moved_to_another_file_of_the_same_mtime_is_a_new_image() {
        let dir = scratch("nix-store-swap");
        let (red, blue, link) = (dir.join("a.png"), dir.join("b.png"), dir.join("g.png"));
        write_image(&red, &image(1, 1, |_, _| RED));
        write_image(&blue, &image(1, 1, |_, _| BLUE));
        stamp_at(&red, 1);
        stamp_at(&blue, 1);
        symlink(&red, &link).expect("a symlink");

        let mut ground = Ground::default();
        assert!(ground.refresh(Some(&link), 1, 1).repaint);
        assert_eq!(painted(&mut ground), RED.to_vec());

        std::fs::remove_file(&link).expect("the old link gone");
        symlink(&blue, &link).expect("a moved symlink");
        assert!(ground.refresh(Some(&link), 1, 1).repaint);
        assert_eq!(painted(&mut ground), BLUE.to_vec());
    }

    #[test]
    fn a_missing_file_stays_flat_with_one_warning() {
        let dir = scratch("missing");
        let path = dir.join("g.png");

        let mut ground = Ground::default();
        let first = ground.refresh(Some(&path), 1, 1);
        assert!(!first.repaint, "the ground is already flat");
        assert!(first.refused.is_some(), "the first miss warns");
        assert!(ground.take_pending().is_none());

        let second = ground.refresh(Some(&path), 1, 1);
        assert!(!second.repaint);
        assert!(second.refused.is_none(), "one warning, not one per tick");

        write_image(&path, &image(1, 1, |_, _| RED));
        assert!(ground.refresh(Some(&path), 1, 1).repaint);
        assert_eq!(painted(&mut ground), RED.to_vec());

        std::fs::remove_file(&path).expect("the file gone");
        let gone = ground.refresh(Some(&path), 1, 1);
        assert!(gone.repaint, "a shown image falls back to flat");
        assert!(gone.refused.is_some());
        assert!(matches!(ground.take_pending(), Some(None)));
        assert!(ground.refresh(Some(&path), 1, 1).refused.is_none());
    }

    #[test]
    fn an_invalid_file_falls_back_flat_warns_once_and_recovers() {
        let dir = scratch("invalid");
        let path = dir.join("g.png");
        std::fs::write(&path, b"not a png").expect("garbage");
        stamp_at(&path, 1000);

        let mut ground = Ground::default();
        let first = ground.refresh(Some(&path), 1, 1);
        assert!(!first.repaint);
        assert!(matches!(first.refused, Some(Refusal::Invalid(_))));
        let second = ground.refresh(Some(&path), 1, 1);
        assert!(!second.repaint);
        assert!(second.refused.is_none(), "one warning per mtime");

        write_image(&path, &image(64, 64, |_, _| GREEN));
        stamp_at(&path, 2000);
        assert!(ground.refresh(Some(&path), 1, 1).repaint);
        assert_eq!(painted(&mut ground), GREEN.to_vec());

        let whole = std::fs::read(&path).expect("the png");
        std::fs::write(&path, &whole[..whole.len() / 2]).expect("a truncated png");
        stamp_at(&path, 3000);
        let truncated = ground.refresh(Some(&path), 1, 1);
        assert!(truncated.repaint, "the shown image falls back to flat");
        assert!(truncated.refused.is_some());
        assert!(matches!(ground.take_pending(), Some(None)));
    }

    #[test]
    fn a_fifo_is_refused_and_never_blocks_the_tick() {
        let dir = scratch("fifo");
        let path = dir.join("g.png");
        fifo(&path);
        let (sent, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut ground = Ground::default();
            let refused = ground.refresh(Some(&path), 1, 1).refused.is_some();
            let _ = sent.send(refused);
        });
        assert_eq!(got.recv_timeout(Duration::from_secs(10)), Ok(true));
    }

    #[test]
    fn with_nothing_configured_nothing_happens() {
        let mut ground = Ground::default();
        let refresh = ground.refresh(None, 1, 1);
        assert!(!refresh.repaint);
        assert!(refresh.refused.is_none());
        assert!(ground.take_pending().is_none());
    }

    struct Slot {
        buffer: Dmabuf,
        path: PathBuf,
    }

    impl Slot {
        fn new(path: PathBuf, width: i32, height: i32, format: Fourcc) -> Self {
            let file = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .expect("a slot file");
            file.set_len(u64::try_from(width * height * 4).expect("a length"))
                .expect("a slot length");
            let mut builder = Dmabuf::builder(
                (width, height),
                format,
                Modifier::Linear,
                DmabufFlags::empty(),
            );
            builder.add_plane(
                file.into(),
                0,
                0,
                u32::try_from(width * 4).expect("a stride"),
            );
            Self {
                buffer: builder.build().expect("a dmabuf"),
                path,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            std::fs::read(&self.path).expect("the slot bytes")
        }

        fn poison(&self, offset: u64) {
            std::fs::File::options()
                .write(true)
                .open(&self.path)
                .expect("the slot file")
                .write_all_at(&[0xab; 4], offset)
                .expect("a poisoned pixel");
        }
    }

    struct Chain {
        slots: [Slot; 2],
        ages: SlotAges,
    }

    impl Chain {
        fn new(dir: &Path, format: Fourcc) -> Self {
            Self {
                slots: [
                    Slot::new(dir.join("slot0"), W, H, format),
                    Slot::new(dir.join("slot1"), W, H, format),
                ],
                ages: SlotAges::default(),
            }
        }

        fn frame(
            &mut self,
            renderer: &mut NuriRenderer,
            tracker: &mut OutputDamageTracker,
            elements: &[MemoryRenderBufferRenderElement<NuriRenderer>],
        ) -> Option<u64> {
            let age = self.ages.age();
            renderer.arm_flush(
                if age == 0 {
                    FlushPolicy::Full
                } else {
                    FlushPolicy::Baselined
                },
                self.ages.generation(),
            );
            let wrote = {
                let mut fb = renderer
                    .bind(&mut self.slots[self.ages.back()].buffer)
                    .expect("a bound slot");
                let result = tracker
                    .render_output(
                        renderer,
                        &mut fb,
                        age,
                        elements,
                        crate::theme::background_for_surface(false),
                    )
                    .expect("a rendered frame");
                let drawn = result.damage.cloned();
                drop(result);
                let wrote = fb.flush_damage(drawn.as_deref().into());
                drawn.map(|_| wrote)
            };
            if wrote.is_some() {
                self.ages.presented();
            }
            wrote
        }
    }

    const W: i32 = 8;
    const H: i32 = 6;

    fn offset(x: i32, y: i32) -> usize {
        usize::try_from((y * W + x) * 4).expect("an offset")
    }

    fn overlay(base: &[u8], rects: &[(i32, i32, i32, i32, [u8; 4])]) -> Vec<u8> {
        let mut out = base.to_vec();
        for (x0, y0, w, h, px) in rects {
            for y in *y0..y0 + h {
                for x in *x0..x0 + w {
                    let o = offset(x, y);
                    out[o..o + 4].copy_from_slice(px);
                }
            }
        }
        out
    }

    #[test]
    fn the_ground_paints_only_damage_and_baselined_leaves_no_slot_stale() {
        let dir = scratch("baselined");
        let path = dir.join("g.png");
        let (w, h) = (
            usize::try_from(W).expect("w"),
            usize::try_from(H).expect("h"),
        );
        let first = image(w, h, |x, y| {
            [
                u8::try_from(x * 20 + 40).expect("a byte"),
                u8::try_from(y * 30 + 40).expect("a byte"),
                90,
                0xff,
            ]
        });
        let second = image(w, h, |x, y| {
            [
                90,
                u8::try_from(x * 20 + 40).expect("a byte"),
                u8::try_from(y * 30 + 40).expect("a byte"),
                0xff,
            ]
        });
        write_image(&path, &first);
        stamp_at(&path, 1000);

        let mut renderer = NuriRenderer::new();
        let mut tracker = OutputDamageTracker::new((W, H), 1.0, Transform::Normal);
        let mut chain = Chain::new(&dir, Fourcc::Argb8888);
        let full = u64::try_from(W * H * 4).expect("a size");
        let buffer = |side: usize, px: [u8; 4]| {
            MemoryRenderBuffer::from_slice(
                &px.repeat(side * side),
                Fourcc::Argb8888,
                (
                    i32::try_from(side).expect("a side"),
                    i32::try_from(side).expect("a side"),
                ),
                1,
                Transform::Normal,
                None,
            )
        };
        let (cursor, pinned) = (buffer(2, WHITE), buffer(1, BLACK));
        let scene = |renderer: &mut NuriRenderer, x: i32, y: i32| {
            let element = |renderer: &mut NuriRenderer, at: (f64, f64), b: &MemoryRenderBuffer| {
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    at,
                    b,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .expect("an element")
            };
            vec![
                element(renderer, (f64::from(x), f64::from(y)), &cursor),
                element(renderer, (4.0, 1.0), &pinned),
            ]
        };

        let mut ground = Ground::default();
        assert!(ground.refresh(Some(&path), W, H).repaint);
        adopt(&mut ground, &mut renderer, &mut tracker);

        let at = |source: &Image, x: i32, y: i32| {
            overlay(&source.bgra, &[(x, y, 2, 2, WHITE), (4, 1, 1, 1, BLACK)])
        };

        let elements = scene(&mut renderer, 1, 1);
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            Some(full)
        );
        assert_eq!(chain.slots[0].bytes(), at(&first, 1, 1));
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            Some(full)
        );
        assert_eq!(chain.slots[1].bytes(), at(&first, 1, 1));

        let far_corner = offset(0, 5);
        chain.slots[0].poison(u64::try_from(far_corner).expect("an offset"));
        let elements = scene(&mut renderer, 4, 3);
        let partial = chain
            .frame(&mut renderer, &mut tracker, &elements)
            .expect("a moved cursor owes a frame");
        assert!(
            partial < full,
            "baselined must clip: wrote {partial} of {full}"
        );
        let mut moved = at(&first, 4, 3);
        moved[far_corner..far_corner + 4].fill(0xab);
        assert_eq!(
            chain.slots[0].bytes(),
            moved,
            "the old cursor shows the image, the pinned element and the poison survive"
        );

        assert!(
            chain
                .frame(&mut renderer, &mut tracker, &elements)
                .is_some()
        );
        assert_eq!(chain.slots[1].bytes(), at(&first, 4, 3));
        assert_eq!(chain.frame(&mut renderer, &mut tracker, &elements), None);

        write_image(&path, &second);
        stamp_at(&path, 2000);
        assert!(ground.refresh(Some(&path), W, H).repaint);
        adopt(&mut ground, &mut renderer, &mut tracker);
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            Some(full)
        );
        assert_eq!(chain.slots[0].bytes(), at(&second, 4, 3));
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            Some(full)
        );
        assert_eq!(chain.slots[1].bytes(), at(&second, 4, 3));
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            None,
            "a new ground damages each slot once, then nothing"
        );

        std::fs::remove_file(&path).expect("the image gone");
        assert!(ground.refresh(Some(&path), W, H).repaint);
        adopt(&mut ground, &mut renderer, &mut tracker);
        assert_eq!(
            chain.frame(&mut renderer, &mut tracker, &elements),
            Some(full)
        );
        let [b, g, r] = flat_bgr();
        let flat = [b, g, r, 0xff].repeat(w * h);
        assert_eq!(
            chain.slots[0].bytes(),
            overlay(&flat, &[(4, 3, 2, 2, WHITE), (4, 1, 1, 1, BLACK)])
        );
    }

    fn picture() -> Image {
        image(
            usize::try_from(W).expect("w"),
            usize::try_from(H).expect("h"),
            |x, y| {
                [
                    u8::try_from(x * 20 + 40).expect("a byte"),
                    u8::try_from(y * 30 + 40).expect("a byte"),
                    90,
                    0xff,
                ]
            },
        )
    }

    fn ground_of(dir: &Path, source: &Image) -> Ground {
        let path = dir.join("g.png");
        write_image(&path, source);
        let mut ground = Ground::default();
        assert!(ground.refresh(Some(&path), W, H).repaint);
        ground
    }

    #[test]
    fn either_scanout_format_composes_and_captures_the_same_pixels() {
        for (name, format) in [
            ("format-xrgb", Fourcc::Xrgb8888),
            ("format-argb", Fourcc::Argb8888),
        ] {
            let dir = scratch(name);
            let source = picture();
            let mut ground = ground_of(&dir, &source);
            let mut renderer = NuriRenderer::new();
            let mut tracker = OutputDamageTracker::new((W, H), 1.0, Transform::Normal);
            adopt(&mut ground, &mut renderer, &mut tracker);
            let mut chain = Chain::new(&dir, format);
            let none: [MemoryRenderBufferRenderElement<NuriRenderer>; 0] = [];
            assert!(chain.frame(&mut renderer, &mut tracker, &none).is_some());
            assert_eq!(chain.slots[0].bytes(), source.bgra, "{name}");

            let ppm = dir.join("capture.ppm");
            {
                let fb = renderer
                    .bind(&mut chain.slots[0].buffer)
                    .expect("a bound slot");
                crate::drm::capture(&mut renderer, &fb, (W, H), &ppm, None, false, format)
                    .expect("a capture");
            }
            let written = std::fs::read(&ppm).expect("the capture file");
            let rgb: Vec<u8> = source
                .bgra
                .chunks_exact(4)
                .flat_map(|px| [px[2], px[1], px[0]])
                .collect();
            assert!(written.ends_with(&rgb), "{name}");
        }
    }

    #[test]
    fn with_no_change_the_adopt_step_leaves_the_tracker_history_alone() {
        let mut renderer = NuriRenderer::new();
        let mut tracker = OutputDamageTracker::new((W, H), 1.0, Transform::Normal);
        let none: [MemoryRenderBufferRenderElement<NuriRenderer>; 0] = [];
        assert!(tracker.damage_output(1, &none).expect("a mode").0.is_some());
        let mut ground = Ground::default();
        adopt(&mut ground, &mut renderer, &mut tracker);
        let (damage, _) = tracker.damage_output(1, &none).expect("a mode");
        assert!(damage.is_none(), "an idle seat keeps its history");
    }

    #[test]
    fn the_render_loop_reads_the_knob_and_adopts_the_ground() {
        let source = include_str!("drm.rs");
        let live = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(live, _)| live);
        for wire in [
            "config.ground.image",
            "ground.refresh(",
            "crate::ground::adopt(",
            "Owed::Ground",
        ] {
            assert!(live.contains(wire), "drm.rs does not read `{wire}`");
        }
    }
}
