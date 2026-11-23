//! Camera RAW files, shown through the JPEG preview the camera embeds in them.
//!
//! Every camera stores a ready-made JPEG inside its RAW files (it is what the
//! camera's own screen shows). Using it means no RAW development is needed:
//! browsing is fast and looks like the camera intended. The preview is often
//! full size, sometimes smaller (about 1600 px on some older cameras).
//!
//! RAW formats differ a lot in where they keep the preview, so instead of one
//! parser per format this looks for JPEG data anywhere in the file and takes
//! the largest image found.

use std::io::Cursor;

use exif::{Exif, Reader};

/// File extensions treated as camera RAW.
pub const EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "nef", "nrw", "arw", "sr2", "dng", "raf", "orf", "rw2", "pef", "srw",
];

/// An SQL condition that is true when `column` (a file path) has a RAW
/// extension. `LIKE` ignores case, so `.CR2` and `.cr2` both match.
pub fn is_raw_sql(column: &str) -> String {
    let tests: Vec<String> = EXTENSIONS
        .iter()
        .map(|extension| format!("{column} LIKE '%.{extension}'"))
        .collect();
    format!("({})", tests.join(" OR "))
}

/// How much of a RAW file is read to find its EXIF block, which sits at the
/// start in every format we know. Reading whole files (20 to 60 MB each)
/// would make indexing over a network very slow.
pub const HEADER_BYTES: usize = 512 * 1024;

/// Returns the largest JPEG image embedded in `file`.
pub fn embedded_preview(file: &[u8]) -> Option<&[u8]> {
    let mut best: Option<(u64, &[u8])> = None;
    let mut from = 0;
    while let Some(found) = find(&file[from..], &[0xFF, 0xD8, 0xFF]) {
        let start = from + found;
        match jpeg_at(&file[start..]) {
            Some((pixels, len)) => {
                if best.is_none_or(|(most, _)| pixels > most) {
                    best = Some((pixels, &file[start..start + len]));
                }
                // Thumbnails nested inside this JPEG's own EXIF are smaller,
                // so skip the whole image.
                from = start + len;
            }
            None => from = start + 3,
        }
    }
    best.map(|(_, jpeg)| jpeg)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Checks that `data` starts with a complete baseline or progressive JPEG.
/// Returns its pixel count and its length in bytes.
fn jpeg_at(data: &[u8]) -> Option<(u64, usize)> {
    let mut pos = 2; // after the start-of-image marker
    let mut pixels = None;
    loop {
        if *data.get(pos)? != 0xFF {
            return None;
        }
        let marker = *data.get(pos + 1)?;
        let len = usize::from(u16::from_be_bytes([
            *data.get(pos + 2)?,
            *data.get(pos + 3)?,
        ]));
        if len < 2 {
            return None;
        }
        match marker {
            // Baseline, extended and progressive frames. Lossless JPEG (0xC3)
            // is how some RAW formats store the sensor data itself: not a
            // picture we can show.
            0xC0..=0xC2 => {
                let height = u16::from_be_bytes([*data.get(pos + 5)?, *data.get(pos + 6)?]);
                let width = u16::from_be_bytes([*data.get(pos + 7)?, *data.get(pos + 8)?]);
                pixels = Some(u64::from(width) * u64::from(height));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            // Start of scan: the compressed picture follows.
            0xDA => {
                let pixels = pixels.filter(|count| *count > 0)?;
                let end = end_of_scans(data, pos + 2 + len)?;
                return Some((pixels, end));
            }
            // Markers that can't appear before the picture in a real JPEG.
            0x00 | 0xD0..=0xD9 | 0xFF => return None,
            _ => {}
        }
        pos += 2 + len;
    }
}

/// Walks compressed picture data from `pos` to just after the end-of-image
/// marker. Progressive JPEGs have several scans, each with its own header.
fn end_of_scans(data: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        // Inside compressed data a 0xFF byte is always followed by 0x00 or a
        // restart marker; anything else is a real marker.
        let ff = pos + data.get(pos..)?.iter().position(|byte| *byte == 0xFF)?;
        match *data.get(ff + 1)? {
            0x00 | 0xD0..=0xD7 | 0xFF => pos = ff + 1,
            0xD9 => return Some(ff + 2),
            _ => {
                // The header of another scan, or a table: skip its segment.
                let len = usize::from(u16::from_be_bytes([*data.get(ff + 2)?, *data.get(ff + 3)?]));
                if len < 2 {
                    return None;
                }
                pos = ff + 2 + len;
            }
        }
    }
}

/// Reads EXIF from the first bytes of a RAW file. Most RAW formats are TIFF
/// files with a vendor twist; `None` means this one needs [`exif_of_preview`].
pub fn exif_of_header(header: &[u8]) -> Option<Exif> {
    let tiff = tiff_block(header)?;
    // "Continue on error": fields whose data lies beyond the part of the file
    // we read are skipped instead of failing everything.
    let mut reader = Reader::new();
    reader.continue_on_error(true);
    reader
        .read_raw(tiff)
        .or_else(|error| error.distill_partial_result(|_| {}))
        .ok()
}

/// Reads the EXIF block of the embedded preview JPEG, for the RAW formats
/// that are not TIFF-like (Fujifilm RAF).
pub fn exif_of_preview(file: &[u8]) -> Option<Exif> {
    let preview = embedded_preview(file)?;
    Reader::new()
        .read_from_container(&mut Cursor::new(preview))
        .ok()
}

/// Finds the TIFF structure that holds a RAW file's EXIF, as a standard TIFF.
fn tiff_block(header: &[u8]) -> Option<Vec<u8>> {
    let magic = header.get(..4)?;
    match magic {
        // Plain TIFF: Canon CR2, Nikon NEF, Sony ARW, Adobe DNG, Pentax PEF...
        b"II*\0" | b"MM\0*" => Some(header.to_vec()),
        // Olympus ORF and Panasonic RW2 are TIFF with their own magic number.
        b"IIRO" | b"IIRS" | b"IIU\0" => {
            let mut tiff = header.to_vec();
            tiff[2..4].copy_from_slice(b"*\0");
            Some(tiff)
        }
        b"MMOR" => {
            let mut tiff = header.to_vec();
            tiff[2..4].copy_from_slice(b"\0*");
            Some(tiff)
        }
        // Canon CR3 is an MP4-like container; its "CMT1" box holds a TIFF
        // with the camera, orientation and date.
        _ if header.get(4..8) == Some(b"ftyp") => {
            let at = find(header, b"CMT1")? + 4;
            let tiff = header.get(at..)?;
            matches!(tiff.get(..4)?, b"II*\0" | b"MM\0*").then(|| tiff.to_vec())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exif::{In, Tag};
    use image::codecs::jpeg::JpegEncoder;
    use image::RgbImage;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let pixels = RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 5) as u8, ((x + y) * 3) as u8])
        });
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85)
            .encode_image(&pixels)
            .unwrap();
        bytes
    }

    #[test]
    fn finds_the_largest_embedded_jpeg() {
        let small = jpeg(16, 12);
        let large = jpeg(96, 64);
        let mut file = b"II*\0 some header ".to_vec();
        file.extend_from_slice(&small);
        // Junk that looks like the start of a JPEG but is not one.
        file.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x12, 0x00, 0x00, 0xFF, 0xD8, 0xFF]);
        file.extend_from_slice(&large);
        file.extend_from_slice(&[0u8; 100]);

        assert_eq!(embedded_preview(&file), Some(large.as_slice()));
        let decoded = image::load_from_memory(embedded_preview(&file).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (96, 64));
    }

    #[test]
    fn a_whole_jpeg_is_its_own_preview_and_junk_has_none() {
        let whole = jpeg(40, 30);
        assert_eq!(embedded_preview(&whole), Some(whole.as_slice()));
        assert_eq!(embedded_preview(b"no pictures here"), None);
        // A JPEG cut off before its end is not usable.
        assert_eq!(embedded_preview(&whole[..whole.len() - 10]), None);
    }

    /// Checks real camera files. Run it with a folder of RAW samples:
    /// `PHOTOTEK_RAW_SAMPLES=/path cargo test real_raw -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_raw_samples() {
        let dir = std::env::var("PHOTOTEK_RAW_SAMPLES").expect("set PHOTOTEK_RAW_SAMPLES");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if crate::metadata::kind_of(&path) != Some(crate::metadata::Kind::Raw) {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let file = std::fs::read(&path).unwrap();
            let started = std::time::Instant::now();
            let preview = embedded_preview(&file).unwrap_or_else(|| panic!("{name}: no preview"));
            let found_in = started.elapsed();
            let image = image::load_from_memory(preview)
                .unwrap_or_else(|e| panic!("{name}: preview does not decode: {e}"));
            let meta = crate::metadata::read(&path);
            println!(
                "{name}: preview {}x{} ({} KB, found in {found_in:?}), {:?} {:?}, \
                 orientation {:?}, date {:?}, gps {:?}",
                image.width(),
                image.height(),
                preview.len() / 1024,
                meta.camera_make,
                meta.camera_model,
                meta.orientation,
                meta.taken_at
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|date| date.naive_utc().to_string()),
                meta.latitude.zip(meta.longitude),
            );
            assert!(image.width() >= 1000, "{name}: preview is tiny");
            assert!(meta.taken_at.is_some(), "{name}: no date");
            assert!(meta.camera_model.is_some(), "{name}: no camera");
            checked += 1;
        }
        assert!(checked > 0, "no RAW files in the folder");
    }

    #[test]
    fn lossless_jpeg_sensor_data_is_not_a_preview() {
        let mut lossless = jpeg(40, 30);
        let frame = find(&lossless, &[0xFF, 0xC0]).unwrap();
        lossless[frame + 1] = 0xC3;
        assert_eq!(embedded_preview(&lossless), None);
    }

    #[test]
    fn reads_exif_from_tiff_like_headers() {
        // The EXIF block of a JPEG is a small TIFF file: reuse it as a "RAW".
        let fields = [
            crate::metadata::testing::ascii_field(Tag::Model, "TestCam"),
            crate::metadata::testing::short_field(Tag::Orientation, 6),
        ];
        let tiff = crate::metadata::testing::tiff_with_exif(&fields);
        assert_eq!(&tiff[..4], b"II*\0");

        let model = |exif: Exif| {
            exif.get_field(Tag::Model, In::PRIMARY)
                .unwrap()
                .display_value()
                .to_string()
        };
        assert_eq!(model(exif_of_header(&tiff).unwrap()), "\"TestCam\"");

        // Olympus: same structure, different magic number.
        let mut orf = tiff.clone();
        orf[2..4].copy_from_slice(b"RO");
        assert_eq!(model(exif_of_header(&orf).unwrap()), "\"TestCam\"");

        // Canon CR3: the TIFF sits in a "CMT1" box.
        let mut cr3 = b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom....CMT1".to_vec();
        cr3.extend_from_slice(&tiff);
        assert_eq!(model(exif_of_header(&cr3).unwrap()), "\"TestCam\"");

        assert!(exif_of_header(b"FUJIFILMCCD-RAW 0201").is_none());
    }
}
