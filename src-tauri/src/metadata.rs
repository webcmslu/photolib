//! Reads the metadata the browser needs: date taken, orientation, camera, GPS.
//! Photos carry it as EXIF; videos have their own format (see video.rs).

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use chrono::NaiveDate;
use exif::{Exif, In, Tag, Value};

use crate::{raw, video};

/// What kind of file something is, judged by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A format decoded directly: JPEG, PNG, HEIC...
    Image,
    /// A camera RAW file, shown through its embedded preview.
    Raw,
    Video,
}

/// Image file types the index picks up, besides RAW.
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "heic", "heif", "gif", "tif", "tiff",
];

/// `None` for files the app does not show.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let ext = ext.as_str();
    if IMAGE_EXTENSIONS.contains(&ext) {
        Some(Kind::Image)
    } else if raw::EXTENSIONS.contains(&ext) {
        Some(Kind::Raw)
    } else if video::EXTENSIONS.contains(&ext) {
        Some(Kind::Video)
    } else {
        None
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct PhotoMeta {
    /// Date taken as wall-clock time where the photo was shot, encoded as
    /// milliseconds since the Unix epoch *as if that wall-clock time were UTC*.
    /// EXIF dates carry no reliable time zone, and a timeline should show the
    /// time the camera showed.
    pub taken_at: Option<i64>,
    /// EXIF orientation, 1 to 8.
    pub orientation: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub is_video: bool,
    pub duration_ms: Option<u64>,
    /// True for a video that may be the moving half of a Live Photo.
    pub live_candidate: bool,
}

/// Reads metadata from a file. Files without metadata (or with broken
/// metadata) give an empty `PhotoMeta`: that is normal, not an error.
pub fn read(path: &Path) -> PhotoMeta {
    match kind_of(path) {
        Some(Kind::Video) => video::read(path),
        Some(Kind::Raw) => read_raw(path).unwrap_or_default(),
        _ => read_image(path),
    }
}

fn read_image(path: &Path) -> PhotoMeta {
    let Ok(file) = File::open(path) else {
        return PhotoMeta::default();
    };
    match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(exif) => from_exif(&exif),
        Err(_) => PhotoMeta::default(),
    }
}

/// RAW files: the EXIF is normally within the first part of the file. Only
/// when that gives no date is the whole file read, to look in the embedded
/// preview instead.
fn read_raw(path: &Path) -> Option<PhotoMeta> {
    let mut header = Vec::new();
    File::open(path)
        .ok()?
        .take(raw::HEADER_BYTES as u64)
        .read_to_end(&mut header)
        .ok()?;
    let meta = raw::exif_of_header(&header)
        .map(|exif| from_exif(&exif))
        .unwrap_or_default();
    if meta.taken_at.is_some() {
        return Some(meta);
    }
    let from_preview = raw::exif_of_preview(&fs::read(path).ok()?).map(|exif| from_exif(&exif));
    Some(match from_preview {
        Some(preview) if preview.taken_at.is_some() => preview,
        _ => meta,
    })
}

pub fn from_exif(exif: &Exif) -> PhotoMeta {
    let (latitude, longitude) = match (
        coordinate(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S', 90.0),
        coordinate(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W', 180.0),
    ) {
        // "0, 0" is what cameras write when they have no GPS fix.
        (Some(lat), Some(lon)) if lat != 0.0 || lon != 0.0 => (Some(lat), Some(lon)),
        _ => (None, None),
    };

    PhotoMeta {
        taken_at: [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime]
            .into_iter()
            .find_map(|tag| datetime(exif, tag)),
        orientation: uint(exif, Tag::Orientation)
            .filter(|v| (1..=8).contains(v))
            .map(|v| v as u8),
        width: uint(exif, Tag::PixelXDimension).filter(|v| *v > 0),
        height: uint(exif, Tag::PixelYDimension).filter(|v| *v > 0),
        camera_make: text(exif, Tag::Make),
        camera_model: text(exif, Tag::Model),
        latitude,
        longitude,
        ..PhotoMeta::default()
    }
}

/// How a photo was taken, for the info panel: read from the file when it
/// is shown, not kept in the index.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shooting {
    pub lens: Option<String>,
    /// Focal length in mm, and its 35 mm equivalent.
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<u32>,
    /// The f-number: 2.8 for f/2.8.
    pub aperture: Option<f64>,
    /// Exposure time in seconds.
    pub exposure: Option<f64>,
    pub iso: Option<u32>,
    /// Exposure compensation in EV.
    pub exposure_bias: Option<f64>,
    /// Whether the flash fired; `None` if the photo doesn't say.
    pub flash: Option<bool>,
    /// The program that last wrote the file (a camera's firmware, an editor).
    pub software: Option<String>,
}

impl Shooting {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// How the photo at `path` was taken; `None` for videos and photos that don't say.
pub fn shooting(path: &Path) -> Option<Shooting> {
    let exif = match kind_of(path)? {
        Kind::Video => return None,
        Kind::Raw => {
            let mut header = Vec::new();
            File::open(path)
                .ok()?
                .take(raw::HEADER_BYTES as u64)
                .read_to_end(&mut header)
                .ok()?;
            match raw::exif_of_header(&header) {
                Some(exif) => exif,
                None => raw::exif_of_preview(&fs::read(path).ok()?)?,
            }
        }
        Kind::Image => exif::Reader::new()
            .read_from_container(&mut BufReader::new(File::open(path).ok()?))
            .ok()?,
    };
    let shooting = shooting_of(&exif);
    (!shooting.is_empty()).then_some(shooting)
}

pub fn shooting_of(exif: &Exif) -> Shooting {
    let lens = text(exif, Tag::LensModel).map(|model| {
        // Some cameras write the brand in LensMake only.
        match text(exif, Tag::LensMake) {
            Some(make)
                if !model.to_lowercase().contains(&make.to_lowercase())
                    && !text(exif, Tag::Make)
                        .is_some_and(|camera| camera.eq_ignore_ascii_case(&make)) =>
            {
                format!("{make} {model}")
            }
            _ => model,
        }
    });
    Shooting {
        lens,
        focal_length: rational(exif, Tag::FocalLength).filter(|mm| *mm > 0.0),
        focal_length_35mm: uint(exif, Tag::FocalLengthIn35mmFilm).filter(|mm| *mm > 0),
        aperture: rational(exif, Tag::FNumber)
            .or_else(|| {
                // APEX value: f-number = 2^(value / 2).
                rational(exif, Tag::ApertureValue).map(|apex| 2f64.powf(apex / 2.0))
            })
            .filter(|f| *f > 0.0 && *f < 1000.0),
        exposure: rational(exif, Tag::ExposureTime).filter(|s| *s > 0.0),
        iso: uint(exif, Tag::PhotographicSensitivity).filter(|iso| *iso > 0),
        exposure_bias: rational(exif, Tag::ExposureBiasValue).filter(|ev| ev.abs() > 0.001),
        flash: uint(exif, Tag::Flash).map(|flash| flash & 1 == 1),
        software: text(exif, Tag::Software),
    }
}

/// A rational value (signed or not) as a number.
fn rational(exif: &Exif, tag: Tag) -> Option<f64> {
    let value = match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(values) => values.first().filter(|r| r.denom != 0)?.to_f64(),
        Value::SRational(values) => values.first().filter(|r| r.denom != 0)?.to_f64(),
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn uint(exif: &Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)?.value.get_uint(0)
}

fn ascii(exif: &Exif, tag: Tag) -> Option<&[u8]> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Ascii(parts) => parts.first().map(Vec::as_slice),
        _ => None,
    }
}

fn text(exif: &Exif, tag: Tag) -> Option<String> {
    let raw = String::from_utf8_lossy(ascii(exif, tag)?);
    let trimmed = raw.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn datetime(exif: &Exif, tag: Tag) -> Option<i64> {
    let dt = exif::DateTime::from_ascii(ascii(exif, tag)?).ok()?;
    // `from_ymd_opt` rejects the all-zero dates some cameras write.
    let naive = NaiveDate::from_ymd_opt(dt.year.into(), dt.month.into(), dt.day.into())?
        .and_hms_opt(dt.hour.into(), dt.minute.into(), dt.second.into())?;
    Some(naive.and_utc().timestamp_millis())
}

/// Converts a degrees/minutes/seconds GPS value to signed decimal degrees.
fn coordinate(exif: &Exif, tag: Tag, ref_tag: Tag, negative_ref: u8, max: f64) -> Option<f64> {
    let Value::Rational(dms) = &exif.get_field(tag, In::PRIMARY)?.value else {
        return None;
    };
    if dms.len() < 3 {
        return None;
    }
    let degrees = dms[0].to_f64() + dms[1].to_f64() / 60.0 + dms[2].to_f64() / 3600.0;
    if !degrees.is_finite() || !(0.0..=max).contains(&degrees) {
        return None;
    }
    let negative = ascii(exif, ref_tag)
        .and_then(|r| r.first())
        .is_some_and(|r| r.eq_ignore_ascii_case(&negative_ref));
    Some(if negative { -degrees } else { degrees })
}

/// Builds tiny JPEG files carrying real EXIF blocks, for tests.
#[cfg(test)]
pub mod testing {
    use std::io::Cursor;

    use exif::experimental::Writer;
    use exif::{Field, In, Rational, Tag, Value};

    pub fn ascii_field(tag: Tag, text: &str) -> Field {
        Field {
            tag,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![text.as_bytes().to_vec()]),
        }
    }

    pub fn short_field(tag: Tag, value: u16) -> Field {
        Field {
            tag,
            ifd_num: In::PRIMARY,
            value: Value::Short(vec![value]),
        }
    }

    pub fn dms(tag: Tag, d: u32, m: u32, s_hundredths: u32) -> Field {
        let r = |num, denom| Rational { num, denom };
        Field {
            tag,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![r(d, 1), r(m, 1), r(s_hundredths, 100)]),
        }
    }

    /// A JPEG that is just SOI + an EXIF APP1 segment + EOI. Not decodable as an
    /// image, but a valid container as far as EXIF readers are concerned.
    pub fn jpeg_with_exif(fields: &[Field]) -> Vec<u8> {
        insert_exif(&[0xFF, 0xD8, 0xFF, 0xD9], fields)
    }

    /// The fields as a bare little-endian TIFF file, which is what an EXIF
    /// block is (and what most RAW files look like from the outside).
    pub fn tiff_with_exif(fields: &[Field]) -> Vec<u8> {
        let mut writer = Writer::new();
        for field in fields {
            writer.push_field(field);
        }
        let mut tiff = Cursor::new(Vec::new());
        writer.write(&mut tiff, true).unwrap();
        tiff.into_inner()
    }

    /// Adds an EXIF APP1 segment right after the SOI marker of a real JPEG.
    pub fn insert_exif(jpeg: &[u8], fields: &[Field]) -> Vec<u8> {
        assert_eq!(jpeg[..2], [0xFF, 0xD8], "not a JPEG");
        let tiff = tiff_with_exif(fields);

        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(&tiff);
        out.extend_from_slice(&jpeg[2..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::fs;

    fn read_fields(fields: &[exif::Field]) -> PhotoMeta {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        fs::write(&path, jpeg_with_exif(fields)).unwrap();
        read(&path)
    }

    #[test]
    fn reads_how_the_photo_was_taken() {
        use exif::{Field, Rational, SRational};
        let rational = |tag, num, denom| Field {
            tag,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![Rational { num, denom }]),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        fs::write(
            &path,
            jpeg_with_exif(&[
                ascii_field(Tag::Make, "Canon"),
                ascii_field(Tag::LensMake, "Sigma"),
                ascii_field(Tag::LensModel, "35mm F1.4 DG HSM | Art 012"),
                rational(Tag::FocalLength, 35, 1),
                rational(Tag::FNumber, 28, 10),
                rational(Tag::ExposureTime, 1, 250),
                short_field(Tag::PhotographicSensitivity, 400),
                Field {
                    tag: Tag::ExposureBiasValue,
                    ifd_num: In::PRIMARY,
                    value: Value::SRational(vec![SRational { num: -2, denom: 3 }]),
                },
                short_field(Tag::Flash, 0x10),
            ]),
        )
        .unwrap();
        let shooting = shooting(&path).unwrap();
        assert_eq!(
            shooting.lens.as_deref(),
            Some("Sigma 35mm F1.4 DG HSM | Art 012")
        );
        assert_eq!(shooting.focal_length, Some(35.0));
        assert_eq!(shooting.aperture, Some(2.8));
        assert_eq!(shooting.exposure, Some(0.004));
        assert_eq!(shooting.iso, Some(400));
        assert!((shooting.exposure_bias.unwrap() + 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(shooting.flash, Some(false));

        // Nothing about the shot: nothing to show.
        fs::write(&path, jpeg_with_exif(&[ascii_field(Tag::Make, "Canon")])).unwrap();
        assert_eq!(super::shooting(&path), None);
    }

    #[test]
    fn reads_date_camera_and_orientation() {
        let meta = read_fields(&[
            ascii_field(Tag::Make, "Canon"),
            ascii_field(Tag::Model, "EOS R6 "),
            short_field(Tag::Orientation, 6),
            ascii_field(Tag::DateTimeOriginal, "2024:07:14 18:30:05"),
        ]);

        assert_eq!(meta.camera_make.as_deref(), Some("Canon"));
        assert_eq!(meta.camera_model.as_deref(), Some("EOS R6"));
        assert_eq!(meta.orientation, Some(6));
        let expected = NaiveDate::from_ymd_opt(2024, 7, 14)
            .unwrap()
            .and_hms_opt(18, 30, 5)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(meta.taken_at, Some(expected));
    }

    #[test]
    fn date_falls_back_to_other_exif_date_tags() {
        let meta = read_fields(&[
            ascii_field(Tag::DateTimeOriginal, "0000:00:00 00:00:00"),
            ascii_field(Tag::DateTime, "2019:01:02 03:04:05"),
        ]);
        let expected = NaiveDate::from_ymd_opt(2019, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(meta.taken_at, Some(expected));
    }

    #[test]
    fn reads_gps_with_hemispheres() {
        // Rio de Janeiro: 22°54'24.00"S 43°10'22.00"W
        let meta = read_fields(&[
            ascii_field(Tag::GPSLatitudeRef, "S"),
            dms(Tag::GPSLatitude, 22, 54, 2400),
            ascii_field(Tag::GPSLongitudeRef, "W"),
            dms(Tag::GPSLongitude, 43, 10, 2200),
        ]);
        assert!((meta.latitude.unwrap() - -22.906_667).abs() < 1e-5);
        assert!((meta.longitude.unwrap() - -43.172_778).abs() < 1e-5);
    }

    #[test]
    fn zero_gps_is_treated_as_missing() {
        let meta = read_fields(&[
            ascii_field(Tag::GPSLatitudeRef, "N"),
            dms(Tag::GPSLatitude, 0, 0, 0),
            ascii_field(Tag::GPSLongitudeRef, "E"),
            dms(Tag::GPSLongitude, 0, 0, 0),
        ]);
        assert_eq!((meta.latitude, meta.longitude), (None, None));
    }

    #[test]
    fn files_without_exif_give_empty_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.jpg");
        fs::write(&path, b"not really a jpeg").unwrap();
        assert_eq!(read(&path), PhotoMeta::default());
        assert_eq!(read(&dir.path().join("missing.jpg")), PhotoMeta::default());
    }
}
