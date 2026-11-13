//! Reads the EXIF fields the browser needs: date taken, orientation, camera, GPS.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::NaiveDate;
use exif::{Exif, In, Tag, Value};

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
}

/// Reads metadata from a file. Files without EXIF (or with broken EXIF) give an
/// empty `PhotoMeta`: missing metadata is normal, not an error.
pub fn read(path: &Path) -> PhotoMeta {
    let Ok(file) = File::open(path) else {
        return PhotoMeta::default();
    };
    match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(exif) => from_exif(&exif),
        Err(_) => PhotoMeta::default(),
    }
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
    }
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

    /// Adds an EXIF APP1 segment right after the SOI marker of a real JPEG.
    pub fn insert_exif(jpeg: &[u8], fields: &[Field]) -> Vec<u8> {
        assert_eq!(jpeg[..2], [0xFF, 0xD8], "not a JPEG");
        let mut writer = Writer::new();
        for field in fields {
            writer.push_field(field);
        }
        let mut tiff = Cursor::new(Vec::new());
        writer.write(&mut tiff, false).unwrap();
        let tiff = tiff.into_inner();

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
