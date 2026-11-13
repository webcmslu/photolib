//! Thumbnail generation and the local thumbnail cache.
//!
//! Every photo gets two JPEGs, made from a single decode of the original:
//! - **grid**: short side about 256 px, for the photo grid;
//! - **preview**: long side about 1024 px, for the viewer and for browsing
//!   while the library is offline.
//!
//! Files are named by content fingerprint, so the cache survives renames and is
//! shared by every library on this computer. EXIF orientation is baked in:
//! cached thumbnails are always upright.

use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fast_image_resize as fir;
use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageReader, RgbImage};
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;

use crate::db::{self, ThumbState};
use crate::indexer::{Phase, Progress};

const GRID_SHORT_SIDE: u32 = 256;
/// Caps the long side of grid thumbnails so panoramas stay small.
const GRID_MAX_LONG_SIDE: u32 = 768;
const PREVIEW_LONG_SIDE: u32 = 1024;
const GRID_QUALITY: u8 = 80;
const PREVIEW_QUALITY: u8 = 82;

const WRITE_BATCH: usize = 100;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Grid,
    Preview,
}

impl ThumbSize {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "grid" => Some(Self::Grid),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Preview => "preview",
        }
    }
}

#[derive(Debug)]
pub enum ThumbError {
    /// The original could not be read, or the cache could not be written.
    /// Worth retrying later: the library may just be offline.
    Io(io::Error),
    /// The file was read but is not an image we can decode. Retrying won't help.
    Decode(String),
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "read/write error: {e}"),
            Self::Decode(why) => write!(f, "can't decode image: {why}"),
        }
    }
}

impl From<io::Error> for ThumbError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<image::ImageError> for ThumbError {
    fn from(e: image::ImageError) -> Self {
        match e {
            image::ImageError::IoError(io) => Self::Io(io),
            other => Self::Decode(other.to_string()),
        }
    }
}

/// True for a fingerprint as produced by `fingerprint::fingerprint`. Checked
/// before a fingerprint from outside (a `thumb://` URL) is used in a file path.
pub fn is_fingerprint(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Clone)]
pub struct ThumbCache {
    root: PathBuf,
}

impl ThumbCache {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            root: app_data_dir.join("thumbs"),
        }
    }

    /// Where the thumbnail of `fingerprint` is (or would be) cached. Spread
    /// over 256 subfolders so no single folder gets huge.
    pub fn path(&self, size: ThumbSize, fingerprint: &str) -> PathBuf {
        self.root
            .join(size.dir_name())
            .join(&fingerprint[..2])
            .join(format!("{fingerprint}.jpg"))
    }

    pub fn has_both(&self, fingerprint: &str) -> bool {
        self.path(ThumbSize::Grid, fingerprint).is_file()
            && self.path(ThumbSize::Preview, fingerprint).is_file()
    }

    /// Decodes `source` once and writes both thumbnail sizes. Returns the
    /// photo's upright (as displayed) width and height.
    pub fn generate(
        &self,
        source: &Path,
        fingerprint: &str,
        orientation: Option<u8>,
    ) -> Result<(u32, u32), ThumbError> {
        let decoded = decode(source)?;
        // HEIC decoding already applies the rotation stored in the file.
        let exif_orientation = orientation.filter(|_| !decoded.upright).unwrap_or(1);
        let orientation =
            Orientation::from_exif(exif_orientation).unwrap_or(Orientation::NoTransforms);
        // EXIF orientations 5 to 8 turn the image on its side.
        let swaps_sides = (5..=8).contains(&exif_orientation);

        let (width, height) = decoded.pixels.dimensions();
        let (w, h) = fit_long_side(width, height, PREVIEW_LONG_SIDE);
        let mut preview = DynamicImage::ImageRgb8(resize(decoded.pixels, w, h)?);
        preview.apply_orientation(orientation);
        let preview = preview.into_rgb8();

        let (w, h) = grid_size(preview.width(), preview.height());
        let grid = resize(preview.clone(), w, h)?;

        self.write(ThumbSize::Preview, fingerprint, &preview, PREVIEW_QUALITY)?;
        self.write(ThumbSize::Grid, fingerprint, &grid, GRID_QUALITY)?;

        Ok(if swaps_sides {
            (height, width)
        } else {
            (width, height)
        })
    }

    /// Writes to a temp file and renames it, so a reader never sees half a JPEG.
    fn write(
        &self,
        size: ThumbSize,
        fingerprint: &str,
        pixels: &RgbImage,
        quality: u8,
    ) -> Result<(), ThumbError> {
        let path = self.path(size, fingerprint);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!(
            "{}-{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        let result = (|| -> Result<(), ThumbError> {
            let mut out = BufWriter::new(fs::File::create(&tmp)?);
            JpegEncoder::new_with_quality(&mut out, quality).encode_image(pixels)?;
            out.into_inner().map_err(|e| e.into_error())?;
            fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

struct Decoded {
    pixels: RgbImage,
    /// True when the decoder already applied the file's rotation.
    upright: bool,
}

fn decode(source: &Path) -> Result<Decoded, ThumbError> {
    let is_heic = source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif"));
    if is_heic {
        return decode_heic(source);
    }

    let image = ImageReader::open(source)?.with_guessed_format()?.decode()?;
    Ok(Decoded {
        pixels: flatten(image),
        upright: false,
    })
}

#[cfg(feature = "heic")]
fn decode_heic(source: &Path) -> Result<Decoded, ThumbError> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let decode_error = |e: libheif_rs::HeifError| ThumbError::Decode(e.to_string());

    // Read the file ourselves: libheif's own file API takes a `&str` path,
    // which breaks on non-UTF-8 names.
    let bytes = fs::read(source)?;
    let context = HeifContext::read_from_bytes(&bytes).map_err(decode_error)?;
    let handle = context.primary_image_handle().map_err(decode_error)?;
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(decode_error)?;
    let plane = image
        .planes()
        .interleaved
        .ok_or_else(|| ThumbError::Decode("no RGB data".into()))?;

    // Rows may be padded (`stride` > width * 3); copy just the pixels.
    let row_len = plane.width as usize * 3;
    let mut pixels = Vec::with_capacity(row_len * plane.height as usize);
    for row in plane.data.chunks(plane.stride).take(plane.height as usize) {
        pixels.extend_from_slice(&row[..row_len]);
    }
    let pixels = RgbImage::from_raw(plane.width, plane.height, pixels)
        .ok_or_else(|| ThumbError::Decode("unexpected RGB data size".into()))?;
    Ok(Decoded {
        pixels,
        upright: true,
    })
}

#[cfg(not(feature = "heic"))]
fn decode_heic(_source: &Path) -> Result<Decoded, ThumbError> {
    Err(ThumbError::Decode("this build has no HEIC support".into()))
}

/// Converts to 8-bit RGB, putting transparent images on a white background.
fn flatten(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (out, pixel) in rgb.pixels_mut().zip(rgba.pixels()) {
        let alpha = u32::from(pixel[3]);
        for channel in 0..3 {
            let blended = u32::from(pixel[channel]) * alpha + 255 * (255 - alpha);
            out[channel] = ((blended + 127) / 255) as u8;
        }
    }
    rgb
}

fn resize(source: RgbImage, width: u32, height: u32) -> Result<RgbImage, ThumbError> {
    if source.dimensions() == (width, height) {
        return Ok(source);
    }
    let resize_error = |e: &dyn std::fmt::Display| ThumbError::Decode(e.to_string());

    let (src_width, src_height) = source.dimensions();
    let src = fir::images::Image::from_vec_u8(
        src_width,
        src_height,
        source.into_raw(),
        fir::PixelType::U8x3,
    )
    .map_err(|e| resize_error(&e))?;
    let mut dst = fir::images::Image::new(width, height, fir::PixelType::U8x3);
    fir::Resizer::new()
        .resize(&src, &mut dst, None)
        .map_err(|e| resize_error(&e))?;
    RgbImage::from_raw(width, height, dst.into_vec())
        .ok_or_else(|| resize_error(&"unexpected resize output size"))
}

/// Scales down (never up) so the long side is at most `max`.
fn fit_long_side(width: u32, height: u32, max: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max {
        return (width, height);
    }
    scale(width, height, f64::from(max) / f64::from(long))
}

/// Scales down (never up) so the short side is `GRID_SHORT_SIDE`, unless that
/// would make the long side exceed `GRID_MAX_LONG_SIDE`.
fn grid_size(width: u32, height: u32) -> (u32, u32) {
    let short = width.min(height);
    if short <= GRID_SHORT_SIDE {
        return fit_long_side(width, height, GRID_MAX_LONG_SIDE);
    }
    let (w, h) = scale(width, height, f64::from(GRID_SHORT_SIDE) / f64::from(short));
    fit_long_side(w, h, GRID_MAX_LONG_SIDE)
}

fn scale(width: u32, height: u32, factor: f64) -> (u32, u32) {
    let apply = |side: u32| ((f64::from(side) * factor).round() as u32).max(1);
    (apply(width), apply(height))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbSummary {
    pub generated: u64,
    /// Files that are not decodable images. They are not retried.
    pub undecodable: u64,
    /// Files that could not be read this time. They are retried on the next scan.
    pub unreadable: u64,
    pub cancelled: bool,
    pub duration_ms: u64,
}

/// What a worker reports back for one photo.
type Outcome = (String, Result<Option<(u32, u32)>, ThumbError>);

/// Generates thumbnails for every indexed photo that doesn't have them yet,
/// in parallel. Like the scan, this thread is the only database writer.
pub fn generate_missing(
    root: &Path,
    conn: &mut Connection,
    cache: &ThumbCache,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(Progress),
) -> rusqlite::Result<ThumbSummary> {
    let started = Instant::now();
    let mut summary = ThumbSummary::default();
    let pending = db::pending_thumbs(conn)?;
    let total = pending.len() as u64;
    let mut done = 0u64;
    let mut last_progress = Instant::now();
    let report = |done| {
        on_progress(Progress {
            phase: Phase::Thumbnails,
            discovered: 0,
            total,
            done,
        })
    };
    if total > 0 {
        report(0);
    }

    std::thread::scope(|scope| -> rusqlite::Result<()> {
        let (sender, receiver) = mpsc::sync_channel::<Outcome>(WRITE_BATCH);
        let pending = &pending;
        scope.spawn(move || {
            pending.par_iter().for_each_with(sender, |sender, photo| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let result = if cache.has_both(&photo.fingerprint) {
                    Ok(None) // e.g. a renamed file, or a second copy
                } else {
                    let source = root.join(&photo.rel_path);
                    cache
                        .generate(&source, &photo.fingerprint, photo.orientation)
                        .map(Some)
                };
                let _ = sender.send((photo.rel_path.clone(), result));
            });
        });

        let mut in_batch = 0usize;
        let mut tx = conn.transaction()?;
        for (rel_path, result) in receiver {
            match result {
                Ok(size) => {
                    db::set_thumb_state(&tx, &rel_path, ThumbState::Ready, size)?;
                    summary.generated += u64::from(size.is_some());
                }
                Err(ThumbError::Decode(_)) => {
                    db::set_thumb_state(&tx, &rel_path, ThumbState::Failed, None)?;
                    summary.undecodable += 1;
                }
                Err(ThumbError::Io(_)) => summary.unreadable += 1,
            }
            done += 1;
            in_batch += 1;
            if in_batch >= WRITE_BATCH {
                tx.commit()?;
                tx = conn.transaction()?;
                in_batch = 0;
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                report(done);
            }
        }
        tx.commit()
    })?;

    summary.cancelled = cancel.load(Ordering::Relaxed);
    summary.duration_ms = started.elapsed().as_millis() as u64;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::testing::{insert_exif, short_field};
    use exif::Tag;
    use image::{Rgb, Rgba, RgbaImage};

    const FP: &str = "00aa000000000000000000000000000000000000000000000000000000000000";

    fn cache(dir: &Path) -> ThumbCache {
        ThumbCache::new(&dir.join("app-data"))
    }

    /// A JPEG whose left half is red and right half is blue.
    fn red_blue_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 95)
            .encode_image(&image)
            .unwrap();
        bytes
    }

    fn is_red(pixel: &Rgb<u8>) -> bool {
        pixel[0] > 180 && pixel[2] < 80
    }

    fn is_blue(pixel: &Rgb<u8>) -> bool {
        pixel[2] > 180 && pixel[0] < 80
    }

    #[test]
    fn sizes() {
        assert_eq!(fit_long_side(4000, 3000, 1024), (1024, 768));
        assert_eq!(fit_long_side(800, 600, 1024), (800, 600));
        assert_eq!(grid_size(1024, 768), (341, 256));
        assert_eq!(grid_size(768, 1024), (256, 341));
        assert_eq!(grid_size(200, 100), (200, 100));
        // Panorama: capped on the long side instead.
        assert_eq!(grid_size(1024, 128), (768, 96));
        assert_eq!(grid_size(10_000, 1000).0, 768);
    }

    #[test]
    fn fingerprint_check_rejects_path_tricks() {
        assert!(is_fingerprint(FP));
        assert!(!is_fingerprint("../../etc/passwd"));
        assert!(!is_fingerprint(&FP.to_uppercase().replace('0', "A")));
        assert!(!is_fingerprint(&FP[1..]));
    }

    #[test]
    fn generates_both_sizes_from_a_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.jpg");
        fs::write(&source, red_blue_jpeg(2000, 1000)).unwrap();
        let cache = cache(dir.path());

        assert!(!cache.has_both(FP));
        assert_eq!(cache.generate(&source, FP, None).unwrap(), (2000, 1000));
        assert!(cache.has_both(FP));

        let preview = image::open(cache.path(ThumbSize::Preview, FP)).unwrap();
        assert_eq!((preview.width(), preview.height()), (1024, 512));
        let grid = image::open(cache.path(ThumbSize::Grid, FP))
            .unwrap()
            .into_rgb8();
        assert_eq!(grid.dimensions(), (512, 256));
        assert!(is_red(grid.get_pixel(20, 128)));
        assert!(is_blue(grid.get_pixel(490, 128)));
    }

    #[test]
    fn applies_exif_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("rotated.jpg");
        // Orientation 6: the camera was turned, display rotated 90° clockwise.
        let jpeg = insert_exif(
            &red_blue_jpeg(400, 200),
            &[short_field(Tag::Orientation, 6)],
        );
        fs::write(&source, jpeg).unwrap();
        let cache = cache(dir.path());

        assert_eq!(cache.generate(&source, FP, Some(6)).unwrap(), (200, 400));

        let preview = image::open(cache.path(ThumbSize::Preview, FP))
            .unwrap()
            .into_rgb8();
        assert_eq!(preview.dimensions(), (200, 400));
        // The stored left edge (red) ends up at the top.
        assert!(is_red(preview.get_pixel(100, 20)));
        assert!(is_blue(preview.get_pixel(100, 380)));
    }

    #[test]
    fn transparent_png_goes_on_white() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("logo.png");
        RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]))
            .save(&source)
            .unwrap();
        let cache = cache(dir.path());

        cache.generate(&source, FP, None).unwrap();
        let grid = image::open(cache.path(ThumbSize::Grid, FP))
            .unwrap()
            .into_rgb8();
        assert!(grid.get_pixel(32, 32).0.iter().all(|&c| c > 240));
    }

    #[test]
    fn errors_tell_unreadable_from_undecodable() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());

        let missing = cache.generate(&dir.path().join("missing.jpg"), FP, None);
        assert!(matches!(missing, Err(ThumbError::Io(_))));

        let garbage = dir.path().join("garbage.jpg");
        fs::write(&garbage, b"this is not a jpeg").unwrap();
        assert!(matches!(
            cache.generate(&garbage, FP, None),
            Err(ThumbError::Decode(_))
        ));
        assert!(!cache.has_both(FP));
    }

    #[test]
    fn background_pass_fills_the_cache_and_records_outcomes() {
        use crate::indexer;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        fs::create_dir(&root).unwrap();
        let rotated = insert_exif(
            &red_blue_jpeg(400, 200),
            &[short_field(Tag::Orientation, 6)],
        );
        fs::write(root.join("rotated.jpg"), &rotated).unwrap();
        fs::write(root.join("copy.jpg"), &rotated).unwrap();
        fs::write(root.join("plain.jpg"), red_blue_jpeg(300, 300)).unwrap();
        fs::write(root.join("broken.jpg"), b"not an image").unwrap();

        let cache = cache(dir.path());
        let never = AtomicBool::new(false);
        let mut conn = db::open_in_memory().unwrap();
        indexer::scan(&root, &mut conn, &never, &|_| {}).unwrap();

        let first = generate_missing(&root, &mut conn, &cache, &never, &|_| {}).unwrap();
        // The two identical files share one cache entry; whichever worker gets
        // there second may or may not have to generate it again.
        assert!((2..=3).contains(&first.generated), "{first:?}");
        assert_eq!((first.undecodable, first.unreadable), (1, 0));

        let size: (u32, u32) = conn
            .query_row(
                "SELECT width, height FROM photos WHERE rel_path = 'rotated.jpg'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(size, (200, 400));

        // Nothing left to do, and the broken file is not retried.
        let second = generate_missing(&root, &mut conn, &cache, &never, &|_| {}).unwrap();
        assert_eq!(
            second,
            ThumbSummary {
                duration_ms: second.duration_ms,
                ..Default::default()
            }
        );

        // A renamed photo reuses its cached thumbnails without decoding again.
        fs::rename(root.join("plain.jpg"), root.join("renamed.jpg")).unwrap();
        indexer::scan(&root, &mut conn, &never, &|_| {}).unwrap();
        let third = generate_missing(&root, &mut conn, &cache, &never, &|_| {}).unwrap();
        assert_eq!(third.generated, 0);
        assert!(db::pending_thumbs(&conn).unwrap().is_empty());
    }

    #[cfg(feature = "heic")]
    #[test]
    fn generates_from_heic() {
        let dir = tempfile::tempdir().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/red-blue.heic");
        let cache = cache(dir.path());

        assert_eq!(cache.generate(&source, FP, None).unwrap(), (320, 160));
        let grid = image::open(cache.path(ThumbSize::Grid, FP))
            .unwrap()
            .into_rgb8();
        assert_eq!(grid.dimensions(), (320, 160));
        assert!(is_red(grid.get_pixel(20, 80)));
        assert!(is_blue(grid.get_pixel(300, 80)));
    }
}
