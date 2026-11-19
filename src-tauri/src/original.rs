//! "Apply to original": draws a photo's edit into its file, for people who
//! want the change outside Photolib too. Everything else in the app leaves
//! files alone; this is only done when asked, after a warning.
//!
//! Only JPEG files are rewritten. The new file keeps the original's metadata
//! (EXIF with the date, camera and position, XMP, the colour profile), with
//! the EXIF orientation set to "upright" since the pixels now are, and the
//! small embedded thumbnail dropped since it would show the old picture.
//! The new file is written next to the old one and then renamed over it, so
//! a failure halfway leaves the original as it was.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;

use crate::export::apply_edit;
use crate::org::{Edit, Op};
use crate::thumbs::{self, ThumbError};

const QUALITY: u8 = 95;

#[derive(Debug)]
pub enum RewriteError {
    /// Not a JPEG, or a kind of JPEG this can't rewrite safely (CMYK).
    Unsupported,
    Io(io::Error),
    Decode(String),
}

impl std::fmt::Display for RewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "Only JPEG photos can be changed."),
            Self::Io(e) => write!(f, "Couldn't write to {e}"),
            Self::Decode(why) => write!(f, "can't decode image: {why}"),
        }
    }
}

impl From<io::Error> for RewriteError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ThumbError> for RewriteError {
    fn from(e: ThumbError) -> Self {
        match e {
            ThumbError::Io(e) => Self::Io(e),
            other => Self::Decode(other.to_string()),
        }
    }
}

/// True for the files [`rewrite`] can change.
pub fn is_rewritable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"))
}

/// Replaces the JPEG at `source` with the picture as `edit` draws it.
/// `orientation` is the EXIF orientation the index read from it.
pub fn rewrite(source: &Path, orientation: Option<u8>, edit: &Edit) -> Result<(), RewriteError> {
    if !is_rewritable(source) {
        return Err(RewriteError::Unsupported);
    }
    let original = fs::read(source)?;
    let segments = metadata_segments(&original).ok_or(RewriteError::Unsupported)?;

    let picture = apply_edit(thumbs::decode_upright(source, orientation)?, edit);
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, QUALITY)
        .encode_image(&picture)
        .map_err(|e| RewriteError::Decode(e.to_string()))?;
    let bytes = with_segments(&encoded, &segments).ok_or(RewriteError::Unsupported)?;

    // Check what is about to replace the original before it does.
    let check = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|e| RewriteError::Decode(e.to_string()))?;
    if (check.width(), check.height()) != picture.dimensions() {
        return Err(RewriteError::Decode("the new file doesn't read back".into()));
    }

    let partial = partial_path(source);
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&partial)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&partial, source)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    Ok(result?)
}

/// `.<name>.photolib-new` next to `source`: hidden, so a scan meanwhile skips it.
fn partial_path(source: &Path) -> PathBuf {
    let name = source.file_name().and_then(|n| n.to_str()).unwrap_or("photo");
    source.with_file_name(format!(".{name}.photolib-new"))
}

/// The metadata segments of a JPEG to carry over (APP1 to APP13 and APP15:
/// EXIF, XMP, ICC profile, IPTC…), fixed up for an upright picture. `None`
/// if the file is not a JPEG this can rewrite.
fn metadata_segments(jpeg: &[u8]) -> Option<Vec<Vec<u8>>> {
    if jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut segments = Vec::new();
    let mut at = 2;
    loop {
        // Fill bytes (0xFF) may come before a marker.
        while jpeg.get(at)? == &0xFF && jpeg.get(at + 1)? == &0xFF {
            at += 1;
        }
        if jpeg.get(at)? != &0xFF {
            return None;
        }
        let marker = *jpeg.get(at + 1)?;
        if marker == 0xDA || marker == 0xD9 {
            break; // the image data: no more metadata
        }
        let length = usize::from(u16::from_be_bytes([*jpeg.get(at + 2)?, *jpeg.get(at + 3)?]));
        let body = jpeg.get(at + 4..at + 2 + length)?;
        match marker {
            // A frame header with 4 components: CMYK, which the decoder
            // turns into RGB, so its colour profile would no longer fit.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                if body.get(5) == Some(&4) {
                    return None;
                }
            }
            0xE1 if body.starts_with(b"Exif\0\0") => {
                let mut exif = body.to_vec();
                make_exif_upright(&mut exif[6..]);
                segments.push(segment(marker, &exif)?);
            }
            0xE1 if body.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
                segments.push(segment(marker, &make_xmp_upright(body))?);
            }
            // APP0 (JFIF) and APP14 (Adobe) describe the old encoding.
            0xE1..=0xED | 0xEF => segments.push(segment(marker, body)?),
            _ => {}
        }
        at += 2 + length;
    }
    Some(segments)
}

fn segment(marker: u8, body: &[u8]) -> Option<Vec<u8>> {
    let length = u16::try_from(body.len() + 2).ok()?;
    let mut out = vec![0xFF, marker];
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(body);
    Some(out)
}

/// The encoder's JPEG with `segments` put in after its start, in place of its
/// JFIF header.
fn with_segments(encoded: &[u8], segments: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut rest = encoded.get(2..)?;
    if rest.get(..2)? == [0xFF, 0xE0] {
        let length = usize::from(u16::from_be_bytes([*rest.get(2)?, *rest.get(3)?]));
        rest = rest.get(2 + length..)?;
    }
    let mut out = Vec::with_capacity(encoded.len() + segments.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&[0xFF, 0xD8]);
    for segment in segments {
        out.extend_from_slice(segment);
    }
    out.extend_from_slice(rest);
    Some(out)
}

/// In EXIF data (from its "II"/"MM" header): sets the orientation to 1 and
/// unlinks the embedded thumbnail (the second IFD). Leaves data it can't
/// follow alone.
fn make_exif_upright(tiff: &mut [u8]) {
    let little = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return,
    };
    let read16 = |data: &[u8], at: usize| -> Option<u16> {
        let bytes = [*data.get(at)?, *data.get(at + 1)?];
        Some(if little { u16::from_le_bytes(bytes) } else { u16::from_be_bytes(bytes) })
    };
    let read32 = |data: &[u8], at: usize| -> Option<u32> {
        let bytes = [*data.get(at)?, *data.get(at + 1)?, *data.get(at + 2)?, *data.get(at + 3)?];
        Some(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    };
    let Some(ifd0) = read32(tiff, 4).map(|offset| offset as usize) else { return };
    let Some(count) = read16(tiff, ifd0).map(usize::from) else { return };
    for entry in 0..count {
        let at = ifd0 + 2 + entry * 12;
        // Tag 0x0112, type SHORT: the value sits in the entry itself.
        if read16(tiff, at) == Some(0x0112) && read16(tiff, at + 2) == Some(3) {
            let one = if little { [1, 0] } else { [0, 1] };
            if let Some(value) = tiff.get_mut(at + 8..at + 10) {
                value.copy_from_slice(&one);
            }
        }
    }
    if let Some(next) = tiff.get_mut(ifd0 + 2 + count * 12..ifd0 + 6 + count * 12) {
        next.fill(0);
    }
}

/// XMP with any `tiff:Orientation` set to 1.
fn make_xmp_upright(xmp: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(xmp) else {
        return xmp.to_vec();
    };
    let text = set_values(text, "tiff:Orientation=\"", "\"");
    set_values(&text, "<tiff:Orientation>", "<").into_bytes()
}

/// `text` with every value between `open` and `close` replaced by "1".
fn set_values(text: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let value_start = start + open.len();
        let Some(length) = rest[value_start..].find(close) else { break };
        out.push_str(&rest[..value_start]);
        out.push('1');
        rest = &rest[value_start + length..];
    }
    out.push_str(rest);
    out
}

/// The events that give the rewritten file (`fingerprint`) what the old one
/// had: `organization` as [`crate::db::organization_of`] lists it. Face names
/// move with the edit, and are dropped when cropped away.
pub fn carried_over(organization: &[(String, String, String, String)], fingerprint: &str, edit: &Edit) -> Vec<Op> {
    let photo = || fingerprint.to_string();
    organization
        .iter()
        .filter_map(|(kind, key, point, value)| match kind.as_str() {
            "fav" => Some(Op::Fav { photo: photo(), on: true }),
            "hide" => Some(Op::Hide { photo: photo(), on: true }),
            "rating" => value.parse().ok().map(|stars| Op::Rating { photo: photo(), stars }),
            "label" => Some(Op::Label { photo: photo(), color: value.clone() }),
            "member" => Some(Op::AlbumMember { album: key.clone(), photo: photo(), on: true }),
            "face_name" => {
                let (x, y) = point.split_once(',')?;
                let (x, y) = edited_point(edit, x.parse().ok()?, y.parse().ok()?)?;
                Some(Op::FaceName { photo: photo(), x, y, name: value.clone() })
            }
            _ => None,
        })
        .collect()
}

/// Where a point of the photo (x and y in ten-thousandths of the upright
/// photo) is once `edit` is drawn into it, or `None` if it is cropped away.
pub fn edited_point(edit: &Edit, x: u16, y: u16) -> Option<(u16, u16)> {
    let (mut x, mut y) = (f64::from(x) / 10_000.0, f64::from(y) / 10_000.0);
    if edit.flip {
        x = 1.0 - x;
    }
    (x, y) = match edit.rotate % 360 {
        90 => (1.0 - y, x),
        180 => (1.0 - x, 1.0 - y),
        270 => (y, 1.0 - x),
        _ => (x, y),
    };
    if let Some([left, top, width, height]) = edit.crop {
        x = (x - f64::from(left) / 10_000.0) / (f64::from(width) / 10_000.0);
        y = (y - f64::from(top) / 10_000.0) / (f64::from(height) / 10_000.0);
    }
    let units = |v: f64| (0.0..=1.0).contains(&v).then(|| (v * 10_000.0).round() as u16);
    Some((units(x)?, units(y)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org::Enhance;
    use image::{Rgb, RgbImage};

    /// EXIF (big-endian) with an orientation of 6 and a link to a second IFD.
    fn exif_rotated() -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&[0, 1]); // one entry
        tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 26]); // next IFD
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // an empty IFD1
        let mut body = b"Exif\0\0".to_vec();
        body.extend_from_slice(&tiff);
        body
    }

    /// A JPEG of 40 x 20 (stored sideways: orientation 6) with EXIF and XMP.
    fn write_photo(path: &Path) {
        let picture = RgbImage::from_fn(40, 20, |x, _| {
            if x < 20 { Rgb([200, 40, 40]) } else { Rgb([40, 40, 200]) }
        });
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 95).encode_image(&picture).unwrap();
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x tiff:Orientation=\"6\"/>".to_vec();
        let segments = [segment(0xE1, &exif_rotated()).unwrap(), segment(0xE1, &xmp).unwrap()];
        fs::write(path, with_segments(&encoded, &segments).unwrap()).unwrap();
    }

    #[test]
    fn rewrites_upright_and_keeps_the_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        write_photo(&path);
        assert_eq!(crate::metadata::read(&path).orientation, Some(6));

        let edit = Edit {
            enhance: Some(Enhance { vibrance: 20, ..Enhance::IDENTITY }),
            ..Edit::default()
        };
        rewrite(&path, Some(6), &edit).unwrap();

        // Upright pixels (the 40 x 20 picture turned a quarter: 20 x 40)…
        let picture = image::open(&path).unwrap().to_rgb8();
        assert_eq!(picture.dimensions(), (20, 40));
        // …red on top after the clockwise turn, and more saturated.
        let top = picture.get_pixel(10, 5).0;
        assert!(top[0] > 205 && top[2] < 40, "{top:?}");
        // The metadata is still there, saying "upright".
        assert_eq!(crate::metadata::read(&path).orientation, Some(1));
        let bytes = fs::read(&path).unwrap();
        let segments = metadata_segments(&bytes).unwrap();
        assert_eq!(segments.len(), 2);
        assert!(String::from_utf8_lossy(&segments[1]).contains("tiff:Orientation=\"1\""));
        // Nothing left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn only_jpegs_are_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        RgbImage::new(4, 4).save(&path).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(rewrite(&path, None, &Edit::default()), Err(RewriteError::Unsupported)));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_organization_moves_to_the_new_file() {
        let row = |kind: &str, key: &str, photo: &str, value: &str| {
            (kind.to_string(), key.to_string(), photo.to_string(), value.to_string())
        };
        let organization = [
            row("fav", "old", "", "1"),
            row("rating", "old", "", "4"),
            row("member", "album1", "old", "1"),
            row("face_name", "old", "2500,5000", "Ann"),
            row("face_name", "old", "9000,5000", "Bob"),
        ];
        let edit = Edit { crop: Some([0, 0, 5_000, 10_000]), ..Edit::default() };
        let ops = carried_over(&organization, "new", &edit);
        assert_eq!(
            ops,
            [
                Op::Fav { photo: "new".into(), on: true },
                Op::Rating { photo: "new".into(), stars: 4 },
                Op::AlbumMember { album: "album1".into(), photo: "new".into(), on: true },
                // Bob was cropped away.
                Op::FaceName { photo: "new".into(), x: 5_000, y: 5_000, name: "Ann".into() },
            ]
        );
    }

    #[test]
    fn points_follow_the_edit() {
        let turned = Edit { rotate: 90, ..Edit::default() };
        // Top left goes to top right with a clockwise turn.
        assert_eq!(edited_point(&turned, 0, 0), Some((10_000, 0)));
        let mirrored = Edit { flip: true, ..Edit::default() };
        assert_eq!(edited_point(&mirrored, 2_000, 3_000), Some((8_000, 3_000)));
        let cropped = Edit { crop: Some([5_000, 0, 5_000, 10_000]), ..Edit::default() };
        assert_eq!(edited_point(&cropped, 7_500, 1_000), Some((5_000, 1_000)));
        assert_eq!(edited_point(&cropped, 2_000, 1_000), None);
    }
}
