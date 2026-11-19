//! Copies photos out of the library into a folder: the original files as
//! they are, or JPEGs drawn the way the app shows them (rotation, mirror,
//! crop and light/color adjustments applied), optionally made smaller for
//! email and messages. The library is only read.

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use image::codecs::jpeg::JpegEncoder;
use image::{imageops, RgbImage};
use serde::{Deserialize, Serialize};

use crate::db::ExportItem;
use crate::org::Edit;
use crate::thumbs::{self, ThumbError};

const JPEG_QUALITY: u8 = 90;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

/// What to write for each photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Size {
    /// The original files, unchanged (edits are not applied).
    Original,
    /// A JPEG with the edits applied, its long side at most `long_side`
    /// pixels (`None`: full size).
    #[serde(rename_all = "camelCase")]
    Jpeg { long_side: Option<u32> },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub exported: usize,
    /// Paths (relative to the library) that could not be exported, with why.
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

/// Exports `items` from the library at `root` into `dest`. Files never
/// overwrite each other: a name already taken gets " (2)", " (3)"…
pub fn export(
    root: &Path,
    items: &[ExportItem],
    dest: &Path,
    size: Size,
    cancel: &AtomicBool,
    on_progress: impl Fn(usize, usize),
) -> io::Result<Summary> {
    fs::create_dir_all(dest)?;
    let mut summary = Summary::default();
    let mut last_progress = Instant::now();
    on_progress(0, items.len());
    for (done, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        match export_one(root, item, dest, size) {
            Ok(()) => summary.exported += 1,
            Err(e) => summary.failed.push((item.rel_path.clone(), e.to_string())),
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            on_progress(done + 1, items.len());
        }
    }
    on_progress(items.len(), items.len());
    Ok(summary)
}

fn export_one(root: &Path, item: &ExportItem, dest: &Path, size: Size) -> Result<(), ThumbError> {
    let source = root.join(&item.rel_path);
    let name = Path::new(&item.rel_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    let target = match size {
        // Videos are always copied as they are.
        Size::Original => copy(&source, dest, &name)?,
        _ if item.video => copy(&source, dest, &name)?,
        Size::Jpeg { long_side } => {
            let mut picture = thumbs::decode_upright(&source, item.orientation)?;
            if let Some(edit) = &item.edit {
                picture = apply_edit(picture, edit);
            }
            if let Some(long_side) = long_side {
                let (width, height) = picture.dimensions();
                if width.max(height) > long_side {
                    let (w, h) = thumbs::fit_long_side(width, height, long_side);
                    picture = thumbs::resize(picture, w, h)?;
                }
            }
            let stem = Path::new(&name)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or(name);
            let target = free_path(dest, &format!("{stem}.jpg"));
            write_jpeg(&target, &picture)?;
            target
        }
    };
    // The copy carries the date the photo was taken, so it sorts right.
    if let Ok(millis) = u64::try_from(item.sort_date) {
        if let Ok(file) = File::options().write(true).open(&target) {
            let _ = file.set_modified(UNIX_EPOCH + Duration::from_millis(millis));
        }
    }
    Ok(())
}

fn copy(source: &Path, dest: &Path, name: &str) -> io::Result<PathBuf> {
    let target = free_path(dest, name);
    fs::copy(source, &target)?;
    Ok(target)
}

/// `dest/name`, or `dest/stem (2).ext` and so on if that is taken.
fn free_path(dest: &Path, name: &str) -> PathBuf {
    let first = dest.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map_or_else(|| name.into(), |s| s.to_string_lossy());
    let ext = path.extension().map(|e| e.to_string_lossy());
    (2..)
        .map(|n| match &ext {
            Some(ext) => dest.join(format!("{stem} ({n}).{ext}")),
            None => dest.join(format!("{stem} ({n})")),
        })
        .find(|candidate| !candidate.exists())
        .expect("some name is free")
}

/// Writes through a temporary name, so a half-written file never stays.
fn write_jpeg(target: &Path, picture: &RgbImage) -> Result<(), ThumbError> {
    let partial = target.with_extension("jpg.part");
    let result = (|| -> Result<(), ThumbError> {
        let mut writer = BufWriter::new(File::create(&partial)?);
        JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY).encode_image(picture)?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        fs::rename(&partial, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// Draws the edit the way the app shows it (see src/lib/edit.ts): mirror,
/// then the clockwise turn, then the crop, then light and color.
pub fn apply_edit(mut picture: RgbImage, edit: &Edit) -> RgbImage {
    if edit.flip {
        imageops::flip_horizontal_in_place(&mut picture);
    }
    picture = match edit.rotate % 360 {
        90 => imageops::rotate90(&picture),
        180 => imageops::rotate180(&picture),
        270 => imageops::rotate270(&picture),
        _ => picture,
    };
    if let Some([left, top, width, height]) = edit.crop {
        let (w, h) = picture.dimensions();
        let scale = |part: u16, side: u32| (u64::from(part) * u64::from(side) / 10_000) as u32;
        let x = scale(left, w).min(w - 1);
        let y = scale(top, h).min(h - 1);
        let cw = scale(width, w).clamp(1, w - x);
        let ch = scale(height, h).clamp(1, h - y);
        picture = imageops::crop_imm(&picture, x, y, cw, ch).to_image();
    }
    adjust(&mut picture, edit);
    picture
}

/// Brightness, contrast and saturation, as the CSS filters the app uses:
/// `brightness(1 + b/200) contrast(1 + c/200) saturate(1 + s/100)`.
fn adjust(picture: &mut RgbImage, edit: &Edit) {
    if edit.brightness == 0 && edit.contrast == 0 && edit.saturation == 0 {
        return;
    }
    let brightness = 1.0 + f32::from(edit.brightness) / 200.0;
    let contrast = 1.0 + f32::from(edit.contrast) / 200.0;
    let s = 1.0 + f32::from(edit.saturation) / 100.0;
    // Brightness and contrast act on each channel alone: a table.
    let table: Vec<f32> = (0..=255u8)
        .map(|v| {
            let v = f32::from(v) / 255.0 * brightness;
            ((v - 0.5) * contrast + 0.5).clamp(0.0, 1.0)
        })
        .collect();
    // The saturate() matrix of the Filter Effects spec.
    let m = [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ];
    let saturate = edit.saturation != 0;
    for pixel in picture.pixels_mut() {
        let [r, g, b] = pixel.0.map(|v| table[usize::from(v)]);
        let out = if saturate {
            m.map(|row| row[0] * r + row[1] * g + row[2] * b)
        } else {
            [r, g, b]
        };
        pixel.0 = out.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn edit() -> Edit {
        Edit {
            rotate: 0,
            flip: false,
            crop: None,
            brightness: 0,
            contrast: 0,
            saturation: 0,
        }
    }

    /// 4 x 2: red on the left half, blue on the right.
    fn picture() -> RgbImage {
        RgbImage::from_fn(4, 2, |x, _| {
            if x < 2 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        })
    }

    #[test]
    fn edits_turn_mirror_and_crop_like_the_screen() {
        let turned = apply_edit(
            picture(),
            &Edit {
                rotate: 90,
                ..edit()
            },
        );
        // A clockwise quarter turn: the left (red) half goes to the top.
        assert_eq!(turned.dimensions(), (2, 4));
        assert_eq!(turned.get_pixel(0, 0), &Rgb([255, 0, 0]));
        assert_eq!(turned.get_pixel(0, 3), &Rgb([0, 0, 255]));

        let mirrored = apply_edit(
            picture(),
            &Edit {
                flip: true,
                ..edit()
            },
        );
        assert_eq!(mirrored.get_pixel(0, 0), &Rgb([0, 0, 255]));

        // Keep the right half of the rotated picture... of the plain one here.
        let cropped = apply_edit(
            picture(),
            &Edit {
                crop: Some([5_000, 0, 5_000, 10_000]),
                ..edit()
            },
        );
        assert_eq!(cropped.dimensions(), (2, 2));
        assert!(cropped.pixels().all(|p| *p == Rgb([0, 0, 255])));
    }

    #[test]
    fn adjustments_follow_the_css_filters() {
        let grey = RgbImage::from_pixel(1, 1, Rgb([100, 100, 100]));
        // brightness(1.5): 100 * 1.5.
        let brighter = apply_edit(
            grey.clone(),
            &Edit {
                brightness: 100,
                ..edit()
            },
        );
        assert_eq!(brighter.get_pixel(0, 0), &Rgb([150, 150, 150]));
        // saturate(0) of grey is the same grey; of red, a grey.
        let flat = apply_edit(
            grey,
            &Edit {
                saturation: -100,
                ..edit()
            },
        );
        assert_eq!(flat.get_pixel(0, 0), &Rgb([100, 100, 100]));
        let red = RgbImage::from_pixel(1, 1, Rgb([255, 0, 0]));
        let [r, g, b] = apply_edit(
            red,
            &Edit {
                saturation: -100,
                ..edit()
            },
        )
        .get_pixel(0, 0)
        .0;
        assert!(r == g && g == b);
    }

    #[test]
    fn exports_originals_and_smaller_jpegs_without_overwriting() {
        let library = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let big = RgbImage::from_pixel(3000, 2000, Rgb([10, 200, 30]));
        big.save(library.path().join("a.jpg")).unwrap();
        fs::write(library.path().join("clip.mov"), b"not really a video").unwrap();
        let item = |rel_path: &str, video: bool| ExportItem {
            rel_path: rel_path.into(),
            orientation: None,
            video,
            sort_date: 1_600_000_000_000,
            edit: Some(Edit {
                rotate: 90,
                ..edit()
            }),
        };
        let items = [item("a.jpg", false), item("clip.mov", true)];
        let never = AtomicBool::new(false);

        let summary = export(
            library.path(),
            &items,
            dest.path(),
            Size::Original,
            &never,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(summary.exported, 2);
        assert_eq!(
            fs::read(dest.path().join("a.jpg")).unwrap(),
            fs::read(library.path().join("a.jpg")).unwrap()
        );
        let modified = fs::metadata(dest.path().join("a.jpg"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            modified,
            UNIX_EPOCH + Duration::from_millis(1_600_000_000_000)
        );

        let small = Size::Jpeg {
            long_side: Some(1280),
        };
        let summary = export(
            library.path(),
            &items,
            dest.path(),
            small,
            &never,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(summary.exported, 2);
        // Turned on its side by the edit, and made smaller; the name was taken.
        let resized = image::open(dest.path().join("a (2).jpg")).unwrap();
        assert_eq!((resized.width(), resized.height()), (853, 1280));
        assert!(dest.path().join("clip (2).mov").is_file());

        let missing = [item("gone.jpg", false)];
        let summary = export(
            library.path(),
            &missing,
            dest.path(),
            small,
            &never,
            |_, _| {},
        )
        .unwrap();
        assert_eq!((summary.exported, summary.failed.len()), (0, 1));
        assert!(!dest.path().join("gone.jpg.part").exists());
    }
}
