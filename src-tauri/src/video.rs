//! Reads metadata from MP4 and QuickTime (MOV) video files.
//!
//! These files are a tree of "atoms": 4 bytes of size, 4 bytes of type, then
//! the content, which for some types is more atoms. Everything we need is in
//! the `moov` atom. The reader seeks from atom to atom and only reads the few
//! small ones it uses, so a 4 GB video on a NAS costs a handful of small reads.
//!
//! ```text
//! moov
//! ├─ mvhd            creation date (UTC), duration
//! ├─ trak
//! │  └─ tkhd         picture size and rotation (zero size for audio tracks)
//! ├─ meta            Apple's key/value metadata
//! │  ├─ keys         the key names, numbered from 1
//! │  └─ ilst         one atom per value, named by key number
//! └─ udta
//!    ├─ ©xyz         GPS position as text, written by Android phones
//!    └─ loci         GPS position as numbers (the 3GPP way)
//! ```

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, NaiveDateTime};

use crate::metadata::PhotoMeta;

/// Video file types the index picks up: the ones every webview can play.
pub const EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "3gp"];

/// Seconds between 1904-01-01 (the QuickTime epoch) and 1970-01-01.
const QUICKTIME_EPOCH_OFFSET: i64 = 2_082_844_800;
/// Atoms we read whole are tiny; anything larger than this is not what it
/// claims to be.
const MAX_LEAF: u64 = 1024 * 1024;
/// Live Photo videos are about 3 seconds long.
const LIVE_PHOTO_MAX_MS: u64 = 4000;

const KEY_CREATION_DATE: &str = "com.apple.quicktime.creationdate";
const KEY_LOCATION: &str = "com.apple.quicktime.location.ISO6709";
const KEY_MAKE: &str = "com.apple.quicktime.make";
const KEY_MODEL: &str = "com.apple.quicktime.model";
const KEY_CONTENT_ID: &str = "com.apple.quicktime.content.identifier";

/// Reads a video's metadata. A file that can't be parsed gives metadata with
/// only `is_video` set: it is still listed, by file date.
pub fn read(path: &Path) -> PhotoMeta {
    let parsed = File::open(path)
        .ok()
        .and_then(|file| parse(&mut BufReader::new(file)).ok())
        .unwrap_or_default();
    parsed.into_meta(|utc_ms| {
        // Cameras without a time zone setting store UTC; show it as the
        // local time of this computer, like the file-date fallback does.
        DateTime::from_timestamp_millis(utc_ms).map(|utc| {
            utc.with_timezone(&Local)
                .naive_local()
                .and_utc()
                .timestamp_millis()
        })
    })
}

/// What the atoms say, before it is turned into a [`PhotoMeta`].
#[derive(Debug, Default, PartialEq)]
struct Parsed {
    /// From `mvhd`, in UTC milliseconds since the Unix epoch.
    created_utc_ms: Option<i64>,
    duration_ms: Option<u64>,
    /// Upright picture size of the first video track.
    size: Option<(u32, u32)>,
    /// Apple's creation date: local time where the video was shot.
    apple_date: Option<String>,
    location: Option<String>,
    /// Latitude and longitude from a `loci` atom.
    position: Option<(f64, f64)>,
    make: Option<String>,
    model: Option<String>,
    content_id: Option<String>,
}

impl Parsed {
    fn into_meta(self, utc_to_wall_clock: impl Fn(i64) -> Option<i64>) -> PhotoMeta {
        let taken_at = self
            .apple_date
            .as_deref()
            .and_then(parse_apple_date)
            .or_else(|| self.created_utc_ms.and_then(utc_to_wall_clock));
        let position = self
            .location
            .as_deref()
            .and_then(parse_iso6709)
            .or(self.position);
        PhotoMeta {
            taken_at,
            orientation: None,
            width: self.size.map(|(width, _)| width),
            height: self.size.map(|(_, height)| height),
            camera_make: self.make,
            camera_model: self.model,
            latitude: position.map(|(latitude, _)| latitude),
            longitude: position.map(|(_, longitude)| longitude),
            is_video: true,
            duration_ms: self.duration_ms,
            live_candidate: self.content_id.is_some()
                || self.duration_ms.is_some_and(|ms| ms <= LIVE_PHOTO_MAX_MS),
        }
    }
}

struct Atom {
    kind: [u8; 4],
    /// Where the content starts and ends in the file.
    start: u64,
    end: u64,
}

/// Reads the atom header at the current position. `None` at `limit`.
fn next_atom<R: Read + Seek>(reader: &mut R, limit: u64) -> io::Result<Option<Atom>> {
    let at = reader.stream_position()?;
    if at + 8 > limit {
        return Ok(None);
    }
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;
    let kind = [header[4], header[5], header[6], header[7]];
    let size32 = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let (start, end) = match size32 {
        // "To the end of the enclosing atom."
        0 => (at + 8, limit),
        // The real size follows as 64 bits.
        1 => {
            let mut large = [0u8; 8];
            reader.read_exact(&mut large)?;
            (at + 16, at.saturating_add(u64::from_be_bytes(large)))
        }
        size => (at + 8, at + size),
    };
    if end < start || end > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad atom size"));
    }
    Ok(Some(Atom { kind, start, end }))
}

fn read_leaf<R: Read + Seek>(reader: &mut R, atom: &Atom) -> io::Result<Vec<u8>> {
    let len = atom.end - atom.start;
    if len > MAX_LEAF {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "atom too large"));
    }
    reader.seek(SeekFrom::Start(atom.start))?;
    let mut content = vec![0u8; len as usize];
    reader.read_exact(&mut content)?;
    Ok(content)
}

fn parse<R: Read + Seek>(reader: &mut R) -> io::Result<Parsed> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut parsed = Parsed::default();
    while let Some(atom) = next_atom(reader, file_len)? {
        if &atom.kind == b"moov" {
            walk(reader, atom.start, atom.end, &mut parsed, 0)?;
            break;
        }
        reader.seek(SeekFrom::Start(atom.end))?;
    }
    Ok(parsed)
}

/// Visits the atoms between `start` and `end`, going into the containers
/// that hold something we use.
fn walk<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    parsed: &mut Parsed,
    depth: u8,
) -> io::Result<()> {
    reader.seek(SeekFrom::Start(start))?;
    while let Some(atom) = next_atom(reader, end)? {
        match &atom.kind {
            b"mvhd" => read_mvhd(&read_leaf(reader, &atom)?, parsed),
            b"tkhd" => read_tkhd(&read_leaf(reader, &atom)?, parsed),
            b"trak" | b"udta" if depth < 4 => {
                walk(reader, atom.start, atom.end, parsed, depth + 1)?
            }
            b"meta" => {
                // A problem in the optional metadata must not lose the rest.
                let _ = read_meta(reader, &atom, parsed);
            }
            // Android: "+48.8566+002.3522/" after a 4-byte text header.
            [0xA9, b'x', b'y', b'z'] => {
                let content = read_leaf(reader, &atom)?;
                if parsed.location.is_none() {
                    parsed.location = content
                        .get(4..)
                        .map(|text| String::from_utf8_lossy(text).into_owned());
                }
            }
            b"loci" => parsed.position = read_loci(&read_leaf(reader, &atom)?),
            _ => {}
        }
        reader.seek(SeekFrom::Start(atom.end))?;
    }
    Ok(())
}

/// The 3GPP location atom: version and flags (4 bytes), language (2), a
/// zero-terminated place name, a role byte, then longitude and latitude as
/// fixed-point numbers with 16 fractional bits.
fn read_loci(data: &[u8]) -> Option<(f64, f64)> {
    let name_len = data.get(6..)?.iter().position(|byte| *byte == 0)?;
    let at = 6 + name_len + 1 + 1;
    let degrees = |at| Some(f64::from(be32(data, at)? as i32) / 65536.0);
    let (longitude, latitude) = (degrees(at)?, degrees(at + 4)?);
    let valid = (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
        && (latitude != 0.0 || longitude != 0.0);
    valid.then_some((latitude, longitude))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    Some((u64::from(be32(data, at)?) << 32) | u64::from(be32(data, at + 4)?))
}

/// The movie header: creation date and duration. Version 1 of the atom uses
/// 64-bit dates and durations.
fn read_mvhd(data: &[u8], parsed: &mut Parsed) {
    let fields = match data.first() {
        Some(0) => (|| {
            Some((
                u64::from(be32(data, 4)?),
                be32(data, 12)?,
                u64::from(be32(data, 16)?),
            ))
        })(),
        Some(1) => (|| Some((be64(data, 4)?, be32(data, 20)?, be64(data, 24)?)))(),
        _ => None,
    };
    let Some((created, timescale, duration)) = fields else {
        return;
    };
    // Zero means "not set"; some cameras write it.
    if created > 0 {
        let unix_seconds = created as i64 - QUICKTIME_EPOCH_OFFSET;
        // Before 1990 there were no such files: the clock was never set.
        if unix_seconds > 631_152_000 {
            parsed.created_utc_ms = Some(unix_seconds * 1000);
        }
    }
    if timescale > 0 && duration != u64::from(u32::MAX) && duration != u64::MAX {
        parsed.duration_ms = Some(duration.saturating_mul(1000) / u64::from(timescale));
    }
}

/// A track header: its picture size and the matrix that rotates it.
fn read_tkhd(data: &[u8], parsed: &mut Parsed) {
    if parsed.size.is_some() {
        return; // keep the first video track
    }
    let matrix_at = match data.first() {
        Some(0) => 40,
        Some(1) => 52,
        _ => return,
    };
    // Width and height are fixed-point numbers with 16 fractional bits.
    let (Some(width), Some(height)) = (be32(data, matrix_at + 36), be32(data, matrix_at + 40))
    else {
        return;
    };
    let (width, height) = (width >> 16, height >> 16);
    if width == 0 || height == 0 {
        return; // an audio or data track
    }
    // The matrix starts with [a, b, u, c, d, v]. A quarter turn has a = d = 0.
    let a = be32(data, matrix_at).unwrap_or(0x1_0000);
    let d = be32(data, matrix_at + 16).unwrap_or(0x1_0000);
    let sideways = a == 0 && d == 0;
    parsed.size = Some(if sideways {
        (height, width)
    } else {
        (width, height)
    });
}

/// Apple's metadata: `keys` lists names, `ilst` holds values by key number.
fn read_meta<R: Read + Seek>(reader: &mut R, meta: &Atom, parsed: &mut Parsed) -> io::Result<()> {
    // In MP4 files `meta` starts with 4 bytes of version and flags; in
    // QuickTime files it does not. Its first child is always `hdlr`.
    reader.seek(SeekFrom::Start(meta.start))?;
    let mut probe = [0u8; 8];
    if reader.read_exact(&mut probe).is_err() {
        return Ok(());
    }
    let children_at = if &probe[4..8] == b"hdlr" {
        meta.start
    } else {
        meta.start + 4
    };

    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<(u32, String)> = Vec::new();
    reader.seek(SeekFrom::Start(children_at))?;
    while let Some(atom) = next_atom(reader, meta.end)? {
        match &atom.kind {
            b"keys" => {
                let data = read_leaf(reader, &atom)?;
                let mut at = 8; // version, flags, count
                while let Some(size) = be32(&data, at) {
                    let size = size as usize;
                    let Some(name) = data.get(at + 8..at + size).filter(|_| size >= 8) else {
                        break;
                    };
                    keys.push(String::from_utf8_lossy(name).into_owned());
                    at += size;
                }
            }
            b"ilst" => {
                reader.seek(SeekFrom::Start(atom.start))?;
                while let Some(item) = next_atom(reader, atom.end)? {
                    let data = read_leaf(reader, &item)?;
                    // The item holds a `data` atom: size, "data", type,
                    // locale, then the value. Type 1 is UTF-8 text.
                    if data.get(4..8) == Some(b"data") && be32(&data, 8) == Some(1) {
                        if let Some(text) = data.get(16..) {
                            values.push((
                                u32::from_be_bytes(item.kind),
                                String::from_utf8_lossy(text).into_owned(),
                            ));
                        }
                    }
                    reader.seek(SeekFrom::Start(item.end))?;
                }
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(atom.end))?;
    }

    for (number, value) in values {
        let Some(key) = (number as usize)
            .checked_sub(1)
            .and_then(|index| keys.get(index))
        else {
            continue;
        };
        let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if value.is_empty() {
            continue;
        }
        let slot = match key.as_str() {
            KEY_CREATION_DATE => &mut parsed.apple_date,
            KEY_LOCATION => &mut parsed.location,
            KEY_MAKE => &mut parsed.make,
            KEY_MODEL => &mut parsed.model,
            KEY_CONTENT_ID => &mut parsed.content_id,
            _ => continue,
        };
        *slot = Some(value.to_owned());
    }
    Ok(())
}

/// Parses `2024-07-14T18:30:05+0200` to the wall-clock time it shows (the
/// time zone is dropped), in the same encoding as `PhotoMeta::taken_at`.
fn parse_apple_date(text: &str) -> Option<i64> {
    let wall_clock = text.get(..19)?;
    NaiveDateTime::parse_from_str(wall_clock, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc().timestamp_millis())
}

/// Parses an ISO 6709 position in decimal degrees, such as
/// `+48.8566+002.3522+035.000/` (latitude, longitude, optional altitude).
fn parse_iso6709(text: &str) -> Option<(f64, f64)> {
    let text = text.trim().trim_end_matches('/');
    let mut starts = text
        .char_indices()
        .filter(|(_, c)| matches!(c, '+' | '-'))
        .map(|(at, _)| at);
    let (first, second) = (starts.next()?, starts.next()?);
    let end = starts.next().unwrap_or(text.len());
    if first != 0 {
        return None;
    }
    let latitude: f64 = text[..second].parse().ok()?;
    let longitude: f64 = text[second..end].parse().ok()?;
    let valid = (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
        && (latitude != 0.0 || longitude != 0.0);
    valid.then_some((latitude, longitude))
}

/// Builds small MP4 files, for tests.
#[cfg(test)]
pub mod testing {
    pub fn atom(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut out = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(content);
        out
    }

    /// A movie header (version 0) with a 600-per-second timescale.
    pub fn mvhd(created_quicktime: u32, duration_ms: u32) -> Vec<u8> {
        let mut content = vec![0u8; 100];
        content[4..8].copy_from_slice(&created_quicktime.to_be_bytes());
        content[12..16].copy_from_slice(&600u32.to_be_bytes());
        content[16..20].copy_from_slice(&(duration_ms * 6 / 10).to_be_bytes());
        atom(b"mvhd", &content)
    }

    /// A track header (version 0). `sideways` gives it a 90° rotation matrix.
    pub fn tkhd(width: u32, height: u32, sideways: bool) -> Vec<u8> {
        let mut content = vec![0u8; 84];
        let one = 0x1_0000u32.to_be_bytes();
        let matrix: [[u8; 4]; 2] = if sideways {
            [[0; 4], [0; 4]]
        } else {
            [one, one]
        };
        content[40..44].copy_from_slice(&matrix[0]);
        content[56..60].copy_from_slice(&matrix[1]);
        content[76..80].copy_from_slice(&(width << 16).to_be_bytes());
        content[80..84].copy_from_slice(&(height << 16).to_be_bytes());
        atom(b"trak", &atom(b"tkhd", &content))
    }

    /// Apple-style metadata with the given keys and text values.
    pub fn meta(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut keys = vec![0u8; 4];
        keys.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        let mut ilst = Vec::new();
        for (index, (key, value)) in entries.iter().enumerate() {
            keys.extend_from_slice(&atom(b"mdta", key.as_bytes()));
            let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
            data.extend_from_slice(value.as_bytes());
            let number = (index as u32 + 1).to_be_bytes();
            ilst.extend_from_slice(&atom(&number, &atom(b"data", &data)));
        }
        let mut content = atom(b"hdlr", &[0u8; 25]);
        content.extend_from_slice(&atom(b"keys", &keys));
        content.extend_from_slice(&atom(b"ilst", &ilst));
        atom(b"meta", &content)
    }

    /// A whole file: `ftyp`, some "video data", then `moov` with `parts`.
    pub fn movie(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut file = atom(b"ftyp", b"qt  \0\0\0\0qt  ");
        file.extend_from_slice(&atom(b"mdat", &[7u8; 300]));
        file.extend_from_slice(&atom(b"moov", &parts.concat()));
        file
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::io::Cursor;

    fn parse_bytes(file: &[u8]) -> Parsed {
        parse(&mut Cursor::new(file)).unwrap()
    }

    fn wall_clock(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    /// 2024-07-14 16:30:05 UTC in QuickTime seconds.
    const CREATED: u32 = (1_720_974_605 + QUICKTIME_EPOCH_OFFSET) as u32;

    #[test]
    fn reads_date_duration_and_size() {
        let file = movie(&[
            mvhd(CREATED, 12_500),
            tkhd(0, 0, false), // audio
            tkhd(1920, 1080, false),
        ]);
        let parsed = parse_bytes(&file);
        assert_eq!(parsed.created_utc_ms, Some(1_720_974_605_000));
        assert_eq!(parsed.duration_ms, Some(12_500));
        assert_eq!(parsed.size, Some((1920, 1080)));

        // With no time zone information, the UTC date is converted by the caller.
        let meta = parsed.into_meta(|utc| Some(utc + 7_200_000));
        assert_eq!(meta.taken_at, Some(wall_clock(2024, 7, 14, 18, 30, 5)));
        assert!(meta.is_video);
        assert_eq!(meta.duration_ms, Some(12_500));
        assert!(!meta.live_candidate);
    }

    #[test]
    fn rotated_video_reports_its_upright_size() {
        let parsed = parse_bytes(&movie(&[mvhd(CREATED, 9000), tkhd(1920, 1080, true)]));
        assert_eq!(parsed.size, Some((1080, 1920)));
    }

    #[test]
    fn apple_metadata_wins_and_marks_live_photos() {
        let file = movie(&[
            mvhd(CREATED, 60_000),
            tkhd(1440, 1080, false),
            meta(&[
                (KEY_MAKE, "Apple"),
                (KEY_CREATION_DATE, "2024-07-14T09:30:05-0700"),
                ("com.apple.quicktime.software", "17.5"),
                (KEY_MODEL, "iPhone 15"),
                (KEY_LOCATION, "+37.7749-122.4194+010.000/"),
                (KEY_CONTENT_ID, "A1B2C3D4-0000-1111-2222-333344445555"),
            ]),
        ]);
        let meta = parse_bytes(&file).into_meta(|_| panic!("the Apple date should be used"));
        // The time shown where it was shot, not UTC.
        assert_eq!(meta.taken_at, Some(wall_clock(2024, 7, 14, 9, 30, 5)));
        assert_eq!(meta.camera_make.as_deref(), Some("Apple"));
        assert_eq!(meta.camera_model.as_deref(), Some("iPhone 15"));
        assert!((meta.latitude.unwrap() - 37.7749).abs() < 1e-9);
        assert!((meta.longitude.unwrap() - -122.4194).abs() < 1e-9);
        // The content identifier is what links it to its photo, whatever its length.
        assert!(meta.live_candidate);
    }

    #[test]
    fn mp4_style_meta_and_android_location() {
        // In MP4 files, `meta` has 4 extra bytes before its children.
        let quicktime_meta = meta(&[(KEY_MODEL, "Pixel 8")]);
        let mut mp4_meta = vec![0u8; 4];
        mp4_meta.extend_from_slice(&quicktime_meta[8..]);
        let xyz = atom(b"\xA9xyz", b"\0\x12\x15\xC7+48.8566+002.3522/");
        let file = movie(&[
            mvhd(CREATED, 2500),
            atom(b"meta", &mp4_meta),
            atom(b"udta", &xyz),
        ]);
        let meta = parse_bytes(&file).into_meta(Some);
        assert_eq!(meta.camera_model.as_deref(), Some("Pixel 8"));
        assert!((meta.latitude.unwrap() - 48.8566).abs() < 1e-9);
        assert!((meta.longitude.unwrap() - 2.3522).abs() < 1e-9);
        // Short enough to be the video half of a Live Photo.
        assert!(meta.live_candidate);
    }

    #[test]
    fn unset_dates_and_broken_files_give_no_metadata() {
        let parsed = parse_bytes(&movie(&[mvhd(0, 5000)]));
        assert_eq!(parsed.created_utc_ms, None);

        assert!(parse(&mut Cursor::new(b"not a video at all")).is_err());
        assert_eq!(parse_bytes(&atom(b"ftyp", b"isom")), Parsed::default());
        let mut truncated = movie(&[mvhd(CREATED, 5000)]);
        truncated.truncate(truncated.len() - 50);
        assert!(parse(&mut Cursor::new(&truncated)).is_err());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.mov");
        std::fs::write(&path, &truncated).unwrap();
        let meta = read(&path);
        assert!(meta.is_video);
        assert_eq!(meta.taken_at, None);
    }

    #[test]
    fn reads_3gpp_location() {
        // Version and flags, language, a place name, a role byte, then
        // longitude 6.1319 and latitude 49.6116, and the rest of the atom.
        let mut loci = b"\0\0\0\0\x15\xC7Home\0\0".to_vec();
        loci.extend_from_slice(&((6.1319f64 * 65536.0) as i32).to_be_bytes());
        loci.extend_from_slice(&((49.6116f64 * 65536.0) as i32).to_be_bytes());
        loci.extend_from_slice(b"\0\0\0\0earth\0\0");
        let file = movie(&[mvhd(CREATED, 9000), atom(b"udta", &atom(b"loci", &loci))]);
        let meta = parse_bytes(&file).into_meta(Some);
        assert!((meta.latitude.unwrap() - 49.6116).abs() < 1e-4);
        assert!((meta.longitude.unwrap() - 6.1319).abs() < 1e-4);

        assert_eq!(read_loci(b"\0\0\0\0\0\0"), None);
        let south_west = {
            let mut loci = b"\0\0\0\0\0\0\0\0".to_vec();
            loci.extend_from_slice(&((-74.006f64 * 65536.0) as i32).to_be_bytes());
            loci.extend_from_slice(&((-33.5f64 * 65536.0) as i32).to_be_bytes());
            loci
        };
        let (latitude, longitude) = read_loci(&south_west).unwrap();
        assert!((latitude - -33.5).abs() < 1e-4 && (longitude - -74.006).abs() < 1e-4);
    }

    #[test]
    fn parses_positions() {
        assert_eq!(parse_iso6709("+48.8566+002.3522/"), Some((48.8566, 2.3522)));
        assert_eq!(
            parse_iso6709("-33.8688+151.2093+058.000/"),
            Some((-33.8688, 151.2093))
        );
        assert_eq!(parse_iso6709("+00.0000+000.0000/"), None);
        assert_eq!(parse_iso6709("garbage"), None);
        assert_eq!(parse_iso6709("+95.0+002.0/"), None);
    }
}
