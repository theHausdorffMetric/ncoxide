//! Image previews: cheap detection (extension gate, then magic bytes and the
//! header), decoding under memory limits, and a background worker that turns
//! a file into a terminal-protocol payload sized for the preview pane.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, Resize};

use crate::platform;

/// Extensions that trigger the header probe. The decoder is chosen by magic
/// bytes, so a mislabelled file falls through to the text/binary path.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "qoi",
];
/// Decoder limits: one preview must never take the whole machine.
const MAX_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

/// Header-level facts about an image file, known before decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageMeta {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub bytes: u64,
}

impl ImageMeta {
    /// `1920×1080 PNG · 2.3 MB`, for titles and status lines.
    pub fn summary(&self) -> String {
        format!(
            "{}×{} {} · {}",
            self.width,
            self.height,
            format_label(self.format),
            platform::format_file_size(self.bytes)
        )
    }
}

fn format_label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Qoi => "QOI",
        _ => "image",
    }
}

/// Whether `path` has an image extension. No I/O — the check compare mode
/// runs per entry while stepping over non-images; the header probe below
/// is the one that confirms a file really is a picture.
pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Identify an image by extension, then confirm by magic bytes and read the
/// header for its dimensions. Cheap: no pixel data is decoded.
pub fn probe(path: &Path) -> Option<ImageMeta> {
    if !has_image_extension(path) {
        return None;
    }
    let bytes = std::fs::metadata(path).ok()?.len();
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let format = reader.format()?;
    let (width, height) = reader.into_dimensions().ok()?;
    Some(ImageMeta {
        width,
        height,
        format,
        bytes,
    })
}

/// Decode under [`MAX_DIMENSION`] / [`MAX_DECODE_ALLOC`] and apply the EXIF
/// orientation, so phone photos come out upright.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = Limits::no_limits();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The protocol payload for one preview-pane size.
pub struct EncodedImage {
    /// The cell area the payload was fitted to; drawn only when the pane
    /// still has exactly this size (a stale payload could overflow it).
    pub target: Size,
    pub protocol: Protocol,
}

impl fmt::Debug for EncodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedImage")
            .field("target", &self.target)
            .field("size", &self.protocol.size())
            .finish_non_exhaustive()
    }
}

/// Which on-screen picture a job is for. The worker coalesces per slot and
/// the app routes results by it, so the locked picture is never starved by
/// a stream of live-preview requests (or vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSlot {
    /// The preview pane, following the cursor.
    Live,
    /// The picture pinned by `L` for side-by-side compare.
    Locked,
}

/// A decode+encode request. `generation` lets the app drop results that
/// arrive after the cursor has moved on.
pub struct ImageJob {
    pub slot: ImageSlot,
    pub generation: u64,
    pub path: PathBuf,
    pub target: Size,
    pub picker: Picker,
}

pub struct ImageResult {
    pub slot: ImageSlot,
    pub generation: u64,
    pub outcome: Result<EncodedImage, String>,
}

/// Decode `job.path` and fit it to `job.target` cells. Blocking; the worker
/// runs it off the UI thread.
pub fn encode(job: &ImageJob) -> Result<EncodedImage, String> {
    let img = decode(&job.path)?;
    let protocol = job
        .picker
        .new_protocol(img, job.target, Resize::Fit(Some(FilterType::Triangle)))
        .map_err(|e| e.to_string())?;
    Ok(EncodedImage {
        target: job.target,
        protocol,
    })
}

/// One long-lived decode/encode thread. Queued jobs coalesce per slot: when
/// several are waiting for the same slot only the newest runs (the cursor
/// moved on), while a job for the other slot is kept — after a resize both
/// pictures need a new payload. The app drops results whose generation is
/// stale. Ends when the worker is dropped.
pub struct ImageWorker {
    tx: Sender<ImageJob>,
    rx: Receiver<ImageResult>,
}

impl ImageWorker {
    pub fn spawn() -> Self {
        let (tx, jobs) = mpsc::channel::<ImageJob>();
        let (results, rx) = mpsc::channel::<ImageResult>();
        let spawned = std::thread::Builder::new()
            .name("ncoxide-image".into())
            .spawn(move || Self::serve(&jobs, &results));
        if let Err(e) = spawned {
            log::warn!("image worker could not start: {e}");
        }
        ImageWorker { tx, rx }
    }

    /// The worker loop: block while idle, otherwise fold everything queued
    /// into at most one job per slot and encode the oldest of those. Draining
    /// again before each encode means a slot's job that went stale while the
    /// other slot was encoding is replaced, not run.
    fn serve(jobs: &Receiver<ImageJob>, results: &Sender<ImageResult>) {
        let mut pending: Vec<ImageJob> = Vec::new();
        loop {
            if pending.is_empty() {
                match jobs.recv() {
                    Ok(job) => pending.push(job),
                    Err(_) => return,
                }
            }
            while let Ok(newer) = jobs.try_recv() {
                pending.retain(|j| j.slot != newer.slot);
                pending.push(newer);
            }
            let job = pending.remove(0);
            let result = ImageResult {
                slot: job.slot,
                generation: job.generation,
                outcome: encode(&job),
            };
            if results.send(result).is_err() {
                return;
            }
        }
    }

    pub fn submit(&self, job: ImageJob) {
        // A dead worker just never answers; the pane keeps its meta line.
        let _ = self.tx.send(job);
    }

    pub fn try_recv(&self) -> Option<ImageResult> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use ratatui_image::FontSize;
    use ratatui_image::picker::ProtocolType;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ncoxide_img_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small gradient so encoders have more than one colour to work with.
    pub(crate) fn write_png(path: &Path, w: u32, h: u32) {
        image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 10) as u8, 128])
        })
        .save_with_format(path, ImageFormat::Png)
        .unwrap();
    }

    #[test]
    fn test_probe_reads_header_only_facts() {
        let dir = temp_dir("probe");
        let path = dir.join("grad.png");
        write_png(&path, 40, 24);
        let meta = probe(&path).expect("png is an image");
        assert_eq!((meta.width, meta.height), (40, 24));
        assert_eq!(meta.format, ImageFormat::Png);
        assert_eq!(meta.bytes, fs::metadata(&path).unwrap().len());
        assert!(
            meta.summary().starts_with("40×24 PNG · "),
            "{}",
            meta.summary()
        );
    }

    #[test]
    fn test_probe_rejects_text_with_image_extension() {
        let dir = temp_dir("probe_text");
        let path = dir.join("fake.png");
        fs::write(&path, b"not a png at all\n").unwrap();
        assert!(probe(&path).is_none());
    }

    #[test]
    fn test_probe_gates_on_extension() {
        // Real PNG bytes under an unlisted extension are left to the
        // text/binary path: the probe is only paid for likely images.
        let dir = temp_dir("probe_ext");
        let path = dir.join("grad.dat");
        write_png(&path, 4, 4);
        assert!(probe(&path).is_none());
        assert!(probe(Path::new("/nonexistent/x.png")).is_none());
    }

    #[test]
    fn test_decode_dimensions() {
        let dir = temp_dir("decode");
        let path = dir.join("grad.png");
        write_png(&path, 40, 24);
        let img = decode(&path).unwrap();
        assert_eq!((img.width(), img.height()), (40, 24));
        assert!(decode(&dir.join("missing.png")).is_err());
    }

    #[test]
    fn test_every_protocol_encodes_without_panicking() {
        let dir = temp_dir("protocols");
        let path = dir.join("grad.png");
        write_png(&path, 64, 32);
        for proto in [
            ProtocolType::Halfblocks,
            ProtocolType::Sixel,
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
        ] {
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(FontSize::new(8, 16));
            picker.set_protocol_type(proto);
            let job = ImageJob {
                slot: ImageSlot::Live,
                generation: 1,
                path: path.clone(),
                target: Size::new(10, 4),
                picker,
            };
            let enc = encode(&job).unwrap_or_else(|e| panic!("{proto:?}: {e}"));
            let size = enc.protocol.size();
            assert!(size.width <= 10 && size.height <= 4, "{proto:?}: {size:?}");
            assert_eq!(enc.target, Size::new(10, 4));
        }
    }

    #[test]
    fn test_worker_round_trip_and_coalescing() {
        let dir = temp_dir("worker");
        let path = dir.join("grad.png");
        write_png(&path, 40, 24);
        let worker = ImageWorker::spawn();
        // Two jobs back to back: the worker may skip the first, but the
        // newest generation always produces a result.
        for generation in [1, 2] {
            worker.submit(ImageJob {
                slot: ImageSlot::Live,
                generation,
                path: path.clone(),
                target: Size::new(10, 5),
                picker: Picker::halfblocks(),
            });
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut latest = None;
        while Instant::now() < deadline {
            if let Some(r) = worker.try_recv() {
                latest = Some(r);
                if latest.as_ref().is_some_and(|r| r.generation == 2) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = latest.expect("worker answered");
        assert_eq!(result.generation, 2);
        assert_eq!(result.slot, ImageSlot::Live);
        let enc = result.outcome.expect("encode ok");
        assert_eq!(enc.target, Size::new(10, 5));
    }

    #[test]
    fn test_worker_coalesces_per_slot_not_globally() {
        // Live 1, Locked 2, Live 3 queued at once: the newest job of *each*
        // slot must produce a result. A global "newest wins" would starve
        // the locked picture after every resize.
        let dir = temp_dir("worker_slots");
        let path = dir.join("grad.png");
        write_png(&path, 40, 24);
        let worker = ImageWorker::spawn();
        for (slot, generation) in [
            (ImageSlot::Live, 1),
            (ImageSlot::Locked, 2),
            (ImageSlot::Live, 3),
        ] {
            worker.submit(ImageJob {
                slot,
                generation,
                path: path.clone(),
                target: Size::new(10, 5),
                picker: Picker::halfblocks(),
            });
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut live, mut locked) = (None, None);
        while Instant::now() < deadline && (live.is_none() || locked.is_none()) {
            if let Some(r) = worker.try_recv() {
                match r.slot {
                    ImageSlot::Live if r.generation == 3 => live = Some(r),
                    ImageSlot::Locked => locked = Some(r),
                    // Live 1 may or may not run, depending on timing.
                    ImageSlot::Live => {}
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let locked = locked.expect("locked slot answered");
        assert_eq!(locked.generation, 2);
        assert!(locked.outcome.is_ok());
        assert!(live.expect("live slot answered").outcome.is_ok());
    }

    #[test]
    fn test_has_image_extension_is_case_insensitive_and_io_free() {
        assert!(has_image_extension(Path::new("/nope/IMG_0417.JPG")));
        assert!(has_image_extension(Path::new("x.webp")));
        assert!(!has_image_extension(Path::new("x.xmp")));
        assert!(!has_image_extension(Path::new("noext")));
    }
}
