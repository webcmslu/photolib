//! Imports photos and videos from a memory card, a camera or any folder into
//! the library, so they can be removed from the device afterwards:
//!
//! ```text
//! <library>/<folder>/<year>/<year>-<month>/IMG_1234.HEIC
//! ```
//!
//! - **Only what is new** is copied: a file whose content is already in the
//!   library, or was imported before (even if since moved or deleted), is
//!   skipped. Imports are remembered per computer in
//!   `<library>/.photolib/imports/<device>.log`, one fingerprint per line,
//!   like the organization logs: each computer appends to its own file only.
//! - **Every copy is checked:** the file is hashed while it is read from the
//!   device and again once written, under a hidden temporary name, and only
//!   then given its real name.
//! - Files that belong together (a Live Photo and its video, a JPEG and its
//!   RAW, an edited `IMG_E1234`'s base…) share a name stem in one folder of
//!   the device; they go to the same folder, under the same stem, so the
//!   library still pairs them.
//!
//! The device is only read.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use chrono::{DateTime, Datelike};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use walkdir::WalkDir;

use crate::{fingerprint, metadata};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const BUFFER: usize = 1 << 20;

/// A memory card or camera that shows up as a drive with a `DCIM` folder
/// (the folder every camera stores its photos in), or on a Mac an iPhone,
/// iPad or camera connected by USB (`path` is then `camera:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub path: String,
    pub name: String,
    /// A phone that is locked, or does not trust this computer yet.
    pub restricted: bool,
}

const CAMERA_PREFIX: &str = "camera:";

/// The drives and devices connected right now that hold photos.
pub fn sources() -> Vec<Source> {
    #[allow(unused_mut)]
    let mut found = drives();
    #[cfg(target_os = "macos")]
    found.extend(
        crate::camera_macos::cameras()
            .into_iter()
            .map(|camera| Source {
                path: format!("{CAMERA_PREFIX}{}", camera.id),
                name: camera.name,
                restricted: camera.restricted,
            }),
    );
    found
}

/// The mounted drives with a DCIM folder (memory cards, cameras seen as disks).
fn drives() -> Vec<Source> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        roots.extend(children("/Volumes"));
    } else if cfg!(windows) {
        roots.extend((b'A'..=b'Z').map(|letter| PathBuf::from(format!("{}:\\", letter as char))));
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        for base in [
            format!("/media/{user}"),
            format!("/run/media/{user}"),
            "/media".into(),
            "/mnt".into(),
        ] {
            roots.extend(children(&base));
        }
    }
    let mut seen = HashSet::new();
    #[allow(unused_mut)]
    let mut found: Vec<Source> = roots
        .into_iter()
        .filter(|root| root.join("DCIM").is_dir() && seen.insert(root.clone()))
        .map(|root| Source {
            name: drive_name(&root),
            path: root.to_string_lossy().into_owned(),
            restricted: false,
        })
        .collect();
    #[cfg(target_os = "linux")]
    found.extend(phone_mounts());
    found
}

/// Phones and cameras that the desktop (GNOME's gvfs, also used by Xfce,
/// Cinnamon, MATE…) mounts as folders: Android phones (MTP), iPhones (AFC,
/// once paired), cameras (PTP). Their photos are in `DCIM`, at the top or in
/// each storage ("Internal shared storage", "SD card").
#[cfg(target_os = "linux")]
fn phone_mounts() -> Vec<Source> {
    let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for mount in children(&format!("{runtime}/gvfs")) {
        let mount_name = mount
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(device) = gvfs_device_name(&mount_name) else {
            continue;
        };
        if mount.join("DCIM").is_dir() {
            found.push(Source {
                name: device,
                path: mount.to_string_lossy().into_owned(),
                restricted: false,
            });
            continue;
        }
        let storages: Vec<PathBuf> = children(&mount.to_string_lossy())
            .into_iter()
            .filter(|storage| storage.join("DCIM").is_dir())
            .collect();
        if storages.is_empty() && mount_name.starts_with("mtp:") {
            // A locked Android phone shows no storage until it is unlocked
            // and "Allow" (access to phone data) is tapped.
            found.push(Source {
                name: device,
                path: mount.to_string_lossy().into_owned(),
                restricted: true,
            });
            continue;
        }
        let several = storages.len() > 1;
        for storage in storages {
            let name = if several {
                let storage_name = storage
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{device} – {storage_name}")
            } else {
                device.clone()
            };
            found.push(Source {
                name,
                path: storage.to_string_lossy().into_owned(),
                restricted: false,
            });
        }
    }
    found
}

/// A readable name for a gvfs mount of a phone or camera
/// (`mtp:host=SAMSUNG_SAMSUNG_Android_R58M12AB3CD` → "SAMSUNG Android"),
/// `None` for mounts that are not one (network shares…).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gvfs_device_name(mount: &str) -> Option<String> {
    let (scheme, rest) = mount.split_once(':')?;
    if scheme == "afc" {
        return Some("iPhone".into());
    }
    if scheme != "mtp" && scheme != "gphoto2" {
        return None;
    }
    let host = rest
        .split(',')
        .find_map(|part| part.strip_prefix("host="))
        .unwrap_or("");
    let host = percent_decode(host);
    let fallback = if scheme == "mtp" { "Phone" } else { "Camera" };
    if host.is_empty() || host.starts_with('[') {
        // Older gvfs names the USB port only: "[usb:001,005]".
        return Some(fallback.into());
    }
    let mut words: Vec<&str> = host.split('_').filter(|word| !word.is_empty()).collect();
    // The serial number at the end.
    if words.len() > 1
        && words
            .last()
            .is_some_and(|last| last.len() >= 6 && last.chars().any(|c| c.is_ascii_digit()))
    {
        words.pop();
    }
    words.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let name = words.join(" ");
    Some(if name.is_empty() {
        fallback.into()
    } else {
        name
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn children(dir: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// The name a drive shows: its folder name ("EOS_DIGITAL"), or on Windows
/// its letter.
fn drive_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().trim_end_matches('\\').to_string())
}

/// Where a file to import is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    File(PathBuf),
    /// File `index` of a device connected by USB (see camera_macos.rs).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Camera {
        device: String,
        index: usize,
    },
}

/// A photo or video found on the source, before it is compared with the library.
#[derive(Debug, Clone)]
struct Entry {
    origin: Origin,
    name: String,
    /// The folder it is in on the source; with the name, it ties companions together.
    folder: String,
    size: u64,
    /// When it was taken (wall-clock milliseconds), if known.
    taken: Option<i64>,
    is_video: bool,
    /// The file's own date (milliseconds since 1970), kept on the copy.
    modified: Option<i64>,
}

/// Where the RAW file of a JPEG + RAW pair goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RawFiles {
    /// Next to its JPEG (the library shows the pair as one photo).
    #[default]
    Beside,
    /// In a subfolder named after its format: `cr2/`, `nef/`, `dng/`…
    Subfolder,
    /// Not imported.
    Skip,
}

/// One file to import.
#[derive(Debug, Clone)]
struct Item {
    origin: Origin,
    name: String,
    size: u64,
    /// Read from the file before copying; for a phone, worked out from the
    /// download instead (reading every file on a phone is slow).
    fingerprint: Option<String>,
    /// Remembers a file of a device without reading it again (see [`camera_key`]).
    key: Option<String>,
    modified: Option<i64>,
    /// The folder of the library it goes to: `<year>/<year>-<month>`.
    month: String,
    /// Files with the same group number keep a common name stem.
    group: usize,
    /// A camera RAW file.
    raw: bool,
}

impl Item {
    /// Where it goes, from the month's folder.
    fn place(&self, raw_files: RawFiles) -> PathBuf {
        let extension = Path::new(&self.name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase());
        match extension {
            Some(extension) if self.raw && raw_files == RawFiles::Subfolder => {
                Path::new(&extension).to_path_buf()
            }
            _ => PathBuf::new(),
        }
    }
}

/// One photo or video as the import window shows it: a file with its
/// companions (a Live Photo's video, a RAW file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    /// Names it in [`import`]'s selection.
    pub group: usize,
    pub name: String,
    /// When it was taken, in wall-clock milliseconds.
    pub taken: Option<i64>,
    /// Bytes to copy (all of its files that are new).
    pub bytes: u64,
    pub video: bool,
    /// A photo with its video (Live Photo).
    pub live: bool,
    /// A photo with its RAW file (or a RAW file alone).
    pub raw: bool,
    /// Of `bytes`, those of RAW files.
    pub raw_bytes: u64,
    /// Already in the library, or imported before.
    pub already: bool,
    /// None of its new files could be read.
    pub unreadable: bool,
    /// The file whose thumbnail stands for it (see [`thumbnail`]).
    pub thumb: usize,
}

/// What an import would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Everything on the source, newest first.
    pub tiles: Vec<Tile>,
    /// Changes with every preview; thumbnails are asked for with it.
    pub generation: u64,
    /// Photos and videos to copy, and their size in bytes.
    pub new: usize,
    pub new_bytes: u64,
    /// Already in the library, or imported before.
    pub already: usize,
    /// Files that could not be read, and why (the first one).
    pub unreadable: usize,
    pub unreadable_reason: Option<String>,
    /// The day numbers (days since 1970) of the oldest and newest new file.
    pub first_day: Option<i64>,
    pub last_day: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub imported: usize,
    pub bytes: u64,
    pub already: usize,
    /// File names that could not be imported, with why.
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

/// Progress of an import: files and bytes done, out of the total.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: u64,
}

/// What a source holds, compared with the library.
#[derive(Clone)]
struct Survey {
    new: Vec<Item>,
    already: usize,
    unreadable: Vec<(String, String)>,
    days: Vec<i64>,
    tiles: Vec<Tile>,
    /// Every file found, by number, for thumbnails.
    origins: Vec<(Origin, bool)>,
    generation: u64,
    /// The files already in the library, and how that is known (see [`free_up_plan`]).
    known: Vec<Known>,
}

/// A file of the source that is already in the library.
#[derive(Debug, Clone)]
struct Known {
    origin: Origin,
    name: String,
    size: u64,
    taken: Option<i64>,
    group: usize,
    evidence: Evidence,
}

/// How a file of the source is known to be in the library.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Evidence {
    /// Its content (fingerprint), read from it.
    Fingerprint(String),
    /// Photolib imported it (device key, see [`camera_key`]).
    Imported(String),
    /// A file of the same name and size is in the library.
    NameAndSize,
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// Thumbnails in the import window, in pixels.
const THUMB_SIDE: u32 = 320;

/// The last survey, so that importing right after the preview does not read
/// a phone's files a second time: (source, library, when, survey).
static LAST_SURVEY: Mutex<Option<(String, PathBuf, Instant, Survey)>> = Mutex::new(None);
const SURVEY_REUSE: Duration = Duration::from_secs(30 * 60);

/// The photos and videos of a folder (its `DCIM` folder if it has one).
fn folder_entries(source: &Path, unreadable: &mut Vec<(String, String)>) -> io::Result<Vec<Entry>> {
    let top = if source.join("DCIM").is_dir() {
        source.join("DCIM")
    } else {
        source.to_path_buf()
    };
    if !top.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the device is not connected",
        ));
    }
    let mut entries = Vec::new();
    for entry in WalkDir::new(&top)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.into_path();
        let Some(kind) = metadata::kind_of(&path) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stat = match fs::metadata(&path) {
            Ok(stat) => stat,
            Err(e) => {
                unreadable.push((name, e.to_string()));
                continue;
            }
        };
        let modified = stat
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_millis() as i64);
        entries.push(Entry {
            folder: path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            taken: metadata::read(&path).taken_at,
            is_video: kind == metadata::Kind::Video,
            size: stat.len(),
            modified,
            name,
            origin: Origin::File(path),
        });
    }
    Ok(entries)
}

/// The photos and videos of a device connected by USB.
#[cfg(target_os = "macos")]
fn camera_entries(device: &str) -> io::Result<Vec<Entry>> {
    let files = crate::camera_macos::files(device).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(files
        .into_iter()
        .enumerate()
        .filter(|(_, file)| metadata::kind_of(Path::new(&file.name)).is_some())
        .map(|(index, file)| Entry {
            is_video: metadata::kind_of(Path::new(&file.name)) == Some(metadata::Kind::Video),
            // The device gives absolute times; the library keeps wall-clock times.
            taken: file
                .exif
                .or(file.created)
                .map(crate::indexer::wall_clock_ms),
            modified: file.created,
            size: file.size,
            folder: file.folder,
            name: file.name,
            origin: Origin::Camera {
                device: device.to_string(),
                index,
            },
        })
        .collect())
}

#[cfg(not(target_os = "macos"))]
fn camera_entries(_device: &str) -> io::Result<Vec<Entry>> {
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "the device is not connected",
    ))
}

/// Remembers a file of a device by name, size and date, so that connecting
/// it again does not read every file to recognize what was imported.
fn camera_key(entry: &Entry) -> Option<String> {
    matches!(entry.origin, Origin::Camera { .. }).then(|| {
        format!(
            "c:{}:{}:{}",
            entry.name,
            entry.size,
            entry.modified.unwrap_or(0)
        )
    })
}

/// Reads what `source` holds and sorts out what the library at `library`
/// does not have yet. Reports `(done, total)` as it reads the files.
fn survey(
    source: &str,
    library: &Path,
    conn: &Connection,
    on_read: &dyn Fn(usize, usize),
) -> io::Result<Survey> {
    let mut unreadable = Vec::new();
    let entries = match source.strip_prefix(CAMERA_PREFIX) {
        Some(device) => camera_entries(device)?,
        None => folder_entries(Path::new(source), &mut unreadable)?,
    };

    // Grouped by folder and name stem, each file with its number.
    let origins: Vec<(Origin, bool)> = entries
        .iter()
        .map(|entry| (entry.origin.clone(), entry.is_video))
        .collect();
    let mut groups: BTreeMap<(String, String), Vec<(usize, Entry)>> = BTreeMap::new();
    for (id, entry) in entries.into_iter().enumerate() {
        let stem = group_stem(Path::new(&entry.name));
        groups
            .entry((entry.folder.clone(), stem))
            .or_default()
            .push((id, entry));
    }

    let imported = imported_before(library);
    let mut survey = Survey {
        new: Vec::new(),
        already: 0,
        unreadable,
        days: Vec::new(),
        tiles: Vec::new(),
        origins,
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
        known: Vec::new(),
    };
    let mut in_library = conn
        .prepare("SELECT 1 FROM photos WHERE fingerprint = ?1 LIMIT 1")
        .map_err(io::Error::other)?;
    // A phone's photos are recognized by name and size: an original copied
    // before, by Photolib or another app, has both.
    let named = names_in_library(conn).map_err(io::Error::other)?;
    let total: usize = groups.values().map(Vec::len).sum();
    let mut done = 0;
    let mut last_report = Instant::now();
    for (group, (_, mut files)) in groups.into_iter().enumerate() {
        files.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        // The group's date: its photo's, else its video's, else the file's.
        let mut dated: Vec<(bool, i64)> = files
            .iter()
            .filter_map(|(_, entry)| entry.taken.map(|ms| (entry.is_video, ms)))
            .collect();
        dated.sort();
        let taken = dated.first().map(|&(_, ms)| ms).or_else(|| {
            files
                .iter()
                .find_map(|(_, entry)| entry.modified.map(crate::indexer::wall_clock_ms))
        });
        // Its tile shows the photo (not the video of a Live Photo).
        // …and the JPEG of a JPEG + RAW pair, which is quicker to read.
        let kind = |entry: &Entry| metadata::kind_of(Path::new(&entry.name));
        let main = files
            .iter()
            .find(|(_, entry)| kind(entry) == Some(metadata::Kind::Image))
            .or_else(|| files.iter().find(|(_, entry)| !entry.is_video))
            .unwrap_or(&files[0]);
        let mut tile = Tile {
            group,
            name: main.1.name.clone(),
            taken,
            bytes: 0,
            video: files.iter().all(|(_, entry)| entry.is_video),
            live: files.iter().any(|(_, e)| e.is_video) && files.iter().any(|(_, e)| !e.is_video),
            raw: files.iter().any(|(_, entry)| {
                metadata::kind_of(Path::new(&entry.name)) == Some(metadata::Kind::Raw)
            }),
            raw_bytes: 0,
            already: false,
            unreadable: false,
            thumb: main.0,
        };
        let (mut new_files, mut unreadable_files) = (0, 0);
        let date = taken
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_default();
        let month = format!("{0}/{0}-{1:02}", date.year(), date.month());
        for (_, entry) in files {
            done += 1;
            if last_report.elapsed() >= PROGRESS_INTERVAL {
                last_report = Instant::now();
                on_read(done, total);
            }
            let key = camera_key(&entry);
            let mut remember = |evidence: Evidence| {
                survey.known.push(Known {
                    origin: entry.origin.clone(),
                    name: entry.name.clone(),
                    size: entry.size,
                    taken,
                    group,
                    evidence,
                });
            };
            let fingerprint = match &entry.origin {
                Origin::Camera { .. } => {
                    let named_alike = named.contains(&(entry.name.to_lowercase(), entry.size));
                    let imported_key = key.as_ref().filter(|key| imported.contains(*key));
                    if let Some(key) = imported_key {
                        remember(Evidence::Imported(key.clone()));
                        survey.already += 1;
                        continue;
                    }
                    if named_alike {
                        remember(Evidence::NameAndSize);
                        survey.already += 1;
                        continue;
                    }
                    None
                }
                Origin::File(path) => {
                    let fingerprint = match fingerprint::fingerprint(path) {
                        Ok(fingerprint) => fingerprint,
                        Err(e) => {
                            survey.unreadable.push((entry.name, e.to_string()));
                            unreadable_files += 1;
                            continue;
                        }
                    };
                    let known = imported.contains(&fingerprint)
                        || in_library
                            .query_row([&fingerprint], |_| Ok(()))
                            .optional()
                            .map_err(io::Error::other)?
                            .is_some();
                    if known {
                        remember(Evidence::Fingerprint(fingerprint));
                        survey.already += 1;
                        continue;
                    }
                    Some(fingerprint)
                }
            };
            if let Some(ms) = taken {
                survey.days.push(ms.div_euclid(86_400_000));
            }
            new_files += 1;
            tile.bytes += entry.size;
            let raw = metadata::kind_of(Path::new(&entry.name)) == Some(metadata::Kind::Raw);
            if raw {
                tile.raw_bytes += entry.size;
            }
            survey.new.push(Item {
                origin: entry.origin,
                name: entry.name,
                size: entry.size,
                fingerprint,
                key,
                modified: entry.modified,
                month: month.clone(),
                group,
                raw,
            });
        }
        tile.unreadable = new_files == 0 && unreadable_files > 0;
        tile.already = new_files == 0 && unreadable_files == 0;
        survey.tiles.push(tile);
    }
    // Newest first, as in the library.
    survey
        .tiles
        .sort_by(|a, b| b.taken.cmp(&a.taken).then(a.name.cmp(&b.name)));
    on_read(total, total);
    Ok(survey)
}

/// A small JPEG of file `id` of the last preview (`generation`), for the
/// import window. `None` if there is none (a video on a drive, say).
pub fn thumbnail(generation: u64, id: usize) -> Option<Vec<u8>> {
    let (origin, is_video) = {
        let last = LAST_SURVEY.lock().unwrap();
        let (_, _, _, survey) = last.as_ref()?;
        if survey.generation != generation {
            return None;
        }
        survey.origins.get(id)?.clone()
    };
    match origin {
        #[cfg(target_os = "macos")]
        Origin::Camera { device, index } => {
            crate::camera_macos::thumbnail(&device, index, THUMB_SIDE).ok()
        }
        #[cfg(not(target_os = "macos"))]
        Origin::Camera { .. } => None,
        Origin::File(_) if is_video => None,
        Origin::File(path) => {
            let orientation = metadata::read(&path).orientation;
            let picture = crate::thumbs::decode_upright(&path, orientation).ok()?;
            let (width, height) = picture.dimensions();
            let scale = f64::from(THUMB_SIDE) / f64::from(width.max(height));
            let picture = if scale < 1.0 {
                let w = ((f64::from(width) * scale).round() as u32).max(1);
                let h = ((f64::from(height) * scale).round() as u32).max(1);
                crate::thumbs::resize(picture, w, h).ok()?
            } else {
                picture
            };
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
                .encode_image(&picture)
                .ok()?;
            Some(jpeg)
        }
    }
}

/// The (lowercase file name, size) of every file in the library.
fn names_in_library(conn: &Connection) -> rusqlite::Result<HashSet<(String, u64)>> {
    let mut statement = conn.prepare("SELECT rel_path, size FROM photos")?;
    let rows = statement.query_map([], |row| {
        let rel_path: String = row.get(0)?;
        let size: i64 = row.get(1)?;
        let name = rel_path
            .rsplit('/')
            .next()
            .unwrap_or(&rel_path)
            .to_lowercase();
        Ok((name, size as u64))
    })?;
    rows.collect()
}

/// The part of a file name that ties it to its companions: `IMG_1234` for
/// `IMG_1234.HEIC`, `IMG_1234.MOV` and the edited `IMG_E1234.HEIC`.
fn group_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    match stem.strip_prefix("IMG_E") {
        Some(number) if number.chars().all(|c| c.is_ascii_digit()) => format!("IMG_{number}"),
        _ => stem,
    }
}

/// What importing `source` (a folder, or `camera:<id>`) into the library
/// would do. Reports `(done, total)` as it reads the source's files.
pub fn preview(
    source: &str,
    library: &Path,
    conn: &Connection,
    on_read: impl Fn(usize, usize),
) -> io::Result<Preview> {
    let survey = survey(source, library, conn, &on_read)?;
    let preview = Preview {
        tiles: survey.tiles.clone(),
        generation: survey.generation,
        new: survey.new.len(),
        new_bytes: survey.new.iter().map(|item| item.size).sum(),
        already: survey.already,
        unreadable: survey.unreadable.len(),
        unreadable_reason: survey.unreadable.first().map(|(_, why)| why.clone()),
        first_day: survey.days.iter().min().copied(),
        last_day: survey.days.iter().max().copied(),
    };
    *LAST_SURVEY.lock().unwrap() = Some((
        source.to_string(),
        library.to_path_buf(),
        Instant::now(),
        survey,
    ));
    Ok(preview)
}

/// Copies what is new on `source` into `<library>/<folder>/<year>/<year>-<month>`:
/// everything, or only the tiles whose `group` is in `selection`.
/// Returns what was done; the caller then scans the library for the new files.
#[allow(clippy::too_many_arguments)]
pub fn import(
    source: &str,
    library: &Path,
    conn: &Connection,
    folder: &str,
    selection: Option<&HashSet<usize>>,
    raw_files: RawFiles,
    device: &str,
    cancel: &AtomicBool,
    on_read: impl Fn(usize, usize),
    on_progress: impl Fn(Progress),
) -> io::Result<Summary> {
    let folder = clean_folder_name(folder)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Not a valid folder name."))?;
    let recent =
        LAST_SURVEY
            .lock()
            .unwrap()
            .clone()
            .filter(|(last_source, last_library, when, _)| {
                last_source == source && last_library == library && when.elapsed() < SURVEY_REUSE
            });
    let mut survey = match recent {
        Some((_, _, _, survey)) => survey,
        None => survey(source, library, conn, &on_read)?,
    };
    if let Some(selection) = selection {
        survey.new.retain(|item| selection.contains(&item.group));
    }
    let mut history = History::open(library, device)?;
    if raw_files == RawFiles::Skip {
        // Left out on purpose: remembered, so that they are not offered again.
        for item in survey.new.iter().filter(|item| item.raw) {
            for line in item.fingerprint.iter().chain(item.key.iter()) {
                history.add(line)?;
            }
        }
        survey.new.retain(|item| !item.raw);
    }
    let mut summary = Summary {
        already: survey.already,
        failed: survey.unreadable,
        ..Summary::default()
    };
    let mut progress = Progress {
        done: 0,
        total: survey.new.len(),
        bytes: 0,
        total_bytes: survey.new.iter().map(|item| item.size).sum(),
    };
    on_progress(progress);
    let mut last_progress = Instant::now();

    // For downloads, whose fingerprint is only known afterwards.
    let imported = imported_before(library);
    let mut in_library = conn
        .prepare("SELECT 1 FROM photos WHERE fingerprint = ?1 LIMIT 1")
        .map_err(io::Error::other)?;
    let mut known = |fingerprint: &str| {
        imported.contains(fingerprint)
            || in_library
                .query_row([fingerprint], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
                .is_some()
    };
    // Each group goes in one folder, under one stem: worked out at its first file.
    let mut stems: BTreeMap<usize, (PathBuf, Option<u32>)> = BTreeMap::new();
    let mut by_group: BTreeMap<usize, Vec<&Item>> = BTreeMap::new();
    for item in &survey.new {
        by_group.entry(item.group).or_default().push(item);
    }
    for item in &survey.new {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let (dir, suffix) = stems.entry(item.group).or_insert_with(|| {
            let dir = library.join(&folder).join(&item.month);
            let places: Vec<(PathBuf, &str)> = by_group[&item.group]
                .iter()
                .map(|i| (i.place(raw_files), i.name.as_str()))
                .collect();
            let suffix = free_suffix(&dir, &places);
            (dir, suffix)
        });
        let target = dir
            .join(item.place(raw_files))
            .join(with_suffix(&item.name, *suffix));
        let copied = match (&item.origin, &item.fingerprint) {
            (Origin::File(path), Some(fingerprint)) => {
                copy_checked(path, &target, fingerprint, cancel, |chunk| {
                    progress.bytes += chunk;
                    if last_progress.elapsed() >= PROGRESS_INTERVAL {
                        last_progress = Instant::now();
                        on_progress(progress);
                    }
                })
                .map(|()| Some(fingerprint.clone()))
            }
            (Origin::File(_), None) => Err(io::Error::other("not read")),
            (Origin::Camera { device, index }, _) => {
                let copied = download_checked(device, *index, item, &target, cancel, &mut known);
                progress.bytes += item.size;
                copied
            }
        };
        match copied {
            Ok(Some(fingerprint)) => {
                history.add(&fingerprint)?;
                if let Some(key) = &item.key {
                    history.add(key)?;
                    // Which library file the device's file became, so that it
                    // can be deleted from the device once that is checked.
                    history.add(&format!("m:{key}\t{fingerprint}"))?;
                }
                summary.imported += 1;
                summary.bytes += item.size;
            }
            Ok(None) => {
                // Already in the library under another name: not copied, and
                // remembered so that it is not offered again.
                if let Some(key) = &item.key {
                    history.add(key)?;
                }
                summary.already += 1;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                summary.cancelled = true;
                break;
            }
            Err(e) => summary.failed.push((item.name.clone(), e.to_string())),
        }
        progress.done += 1;
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            on_progress(progress);
        }
    }
    on_progress(progress);
    Ok(summary)
}

/// Downloads file `index` of a device to `target` through a hidden folder,
/// checks its size, and returns its fingerprint; `None` (and no copy) if
/// `known` says the library has it already.
#[cfg(target_os = "macos")]
fn download_checked(
    device: &str,
    index: usize,
    item: &Item,
    target: &Path,
    cancel: &AtomicBool,
    known: &mut dyn FnMut(&str) -> bool,
) -> io::Result<Option<String>> {
    let dir = target.parent().expect("a folder");
    let staging = dir.join(".photolib-import");
    fs::create_dir_all(&staging)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let partial = staging.join(&name);
    let result = (|| -> io::Result<Option<String>> {
        crate::camera_macos::download(device, index, &staging, &name, cancel).map_err(|why| {
            if why == "cancelled" {
                io::ErrorKind::Interrupted.into()
            } else {
                io::Error::other(why)
            }
        })?;
        if fs::metadata(&partial)?.len() != item.size {
            return Err(io::Error::other("the copy doesn't match the original"));
        }
        let fingerprint = fingerprint::fingerprint(&partial)?;
        if known(&fingerprint) {
            fs::remove_file(&partial)?;
            return Ok(None);
        }
        if let Some(ms) = item.modified {
            let when = UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
            OpenOptions::new()
                .write(true)
                .open(&partial)?
                .set_modified(when)?;
        }
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a file with that name appeared",
            ));
        }
        fs::rename(&partial, target)?;
        Ok(Some(fingerprint))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    let _ = fs::remove_dir(&staging); // only once empty
    result
}

#[cfg(not(target_os = "macos"))]
fn download_checked(
    _: &str,
    _: usize,
    _: &Item,
    _: &Path,
    _: &AtomicBool,
    _: &mut dyn FnMut(&str) -> bool,
) -> io::Result<Option<String>> {
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "the device is not connected",
    ))
}

/// What freeing up space on a source would delete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeUpPlan {
    /// Photos and videos (tiles) that would be deleted, their files and bytes.
    pub photos: usize,
    pub files: usize,
    pub bytes: u64,
    /// Kept because they are recent.
    pub kept_recent: usize,
    /// Kept because they could not be checked in the library (missing files,
    /// a library that can't be reached, a companion that is not imported).
    pub not_checked: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeUpSummary {
    pub deleted: usize,
    pub bytes: u64,
    /// File names that could not be deleted, with why.
    pub failed: Vec<(String, String)>,
}

/// Works out which files of the last preview of `source` can be deleted
/// from it: every file of a photo (with its video, its RAW file) must be in
/// the library at `library`, and found on its disk. Photos taken in the last
/// `keep_days` days stay.
fn free_up_files(
    source: &str,
    library: &Path,
    conn: &Connection,
    keep_days: u32,
) -> io::Result<(FreeUpPlan, Vec<Known>)> {
    let survey = LAST_SURVEY
        .lock()
        .unwrap()
        .clone()
        .filter(|(last_source, last_library, _, _)| {
            last_source == source && last_library == library
        })
        .map(|(_, _, _, survey)| survey)
        .ok_or_else(|| io::Error::other("Look at the device's photos again first."))?;

    // Where each fingerprint and each (name, size) is in the library.
    let mut by_fingerprint: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_name: BTreeMap<(String, u64), Vec<String>> = BTreeMap::new();
    {
        let mut statement = conn
            .prepare("SELECT rel_path, size, fingerprint FROM photos")
            .map_err(io::Error::other)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(io::Error::other)?;
        for row in rows {
            let (rel_path, size, fingerprint) = row.map_err(io::Error::other)?;
            let name = rel_path
                .rsplit('/')
                .next()
                .unwrap_or(&rel_path)
                .to_lowercase();
            by_name
                .entry((name, size as u64))
                .or_default()
                .push(rel_path.clone());
            by_fingerprint
                .entry(fingerprint)
                .or_default()
                .push(rel_path);
        }
    }
    let mapping = imported_map(library);
    let on_disk = |paths: Option<&Vec<String>>| {
        paths.is_some_and(|paths| {
            paths
                .iter()
                .any(|rel_path| library.join(rel_path).is_file())
        })
    };
    let safe = |file: &Known| match &file.evidence {
        Evidence::Fingerprint(fingerprint) => on_disk(by_fingerprint.get(fingerprint)),
        Evidence::Imported(key) => mapping
            .get(key)
            .is_some_and(|fingerprint| on_disk(by_fingerprint.get(fingerprint))),
        Evidence::NameAndSize => on_disk(by_name.get(&(file.name.to_lowercase(), file.size))),
    };

    // Every file of a group, known or not.
    let mut group_sizes: BTreeMap<usize, usize> = BTreeMap::new();
    for item in &survey.new {
        *group_sizes.entry(item.group).or_default() += 1;
    }
    let mut known_by_group: BTreeMap<usize, Vec<&Known>> = BTreeMap::new();
    for file in &survey.known {
        known_by_group.entry(file.group).or_default().push(file);
    }
    let now = crate::indexer::wall_clock_ms(
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let cutoff = now - i64::from(keep_days) * 86_400_000;

    let mut plan = FreeUpPlan::default();
    let mut files = Vec::new();
    for (group, known) in known_by_group {
        // A photo whose video or RAW file is not in the library stays whole.
        let complete = !group_sizes.contains_key(&group);
        let taken = known.iter().filter_map(|file| file.taken).max();
        if keep_days > 0 && taken.is_none_or(|taken| taken >= cutoff) {
            plan.kept_recent += 1;
            continue;
        }
        if !complete || !known.iter().all(|file| safe(file)) {
            plan.not_checked += 1;
            continue;
        }
        plan.photos += 1;
        plan.files += known.len();
        plan.bytes += known.iter().map(|file| file.size).sum::<u64>();
        files.extend(known.into_iter().cloned());
    }
    Ok((plan, files))
}

/// What freeing up space on `source` would delete (see [`free_up`]).
pub fn free_up_plan(
    source: &str,
    library: &Path,
    conn: &Connection,
    keep_days: u32,
) -> io::Result<FreeUpPlan> {
    free_up_files(source, library, conn, keep_days).map(|(plan, _)| plan)
}

/// Deletes from `source` the photos and videos that are safely in the
/// library (see [`free_up_files`]). Only for phones, cameras and drives
/// with a DCIM folder, never for any other folder.
pub fn free_up(
    source: &str,
    library: &Path,
    conn: &Connection,
    keep_days: u32,
) -> io::Result<FreeUpSummary> {
    let is_device =
        source.starts_with(CAMERA_PREFIX) || drives().iter().any(|found| found.path == source);
    if !is_device {
        return Err(io::Error::other(
            "Only a phone, a camera or a memory card can be cleared.",
        ));
    }
    delete_safe_files(source, library, conn, keep_days)
}

/// [`free_up`], whatever the source.
fn delete_safe_files(
    source: &str,
    library: &Path,
    conn: &Connection,
    keep_days: u32,
) -> io::Result<FreeUpSummary> {
    let (_, files) = free_up_files(source, library, conn, keep_days)?;
    let mut summary = FreeUpSummary::default();
    let mut on_device: BTreeMap<String, Vec<&Known>> = BTreeMap::new();
    for file in &files {
        match &file.origin {
            Origin::File(path) => {
                // The card may have changed since it was looked at.
                let unchanged = fs::metadata(path).is_ok_and(|m| m.len() == file.size);
                if !unchanged {
                    summary.failed.push((
                        file.name.clone(),
                        "changed on the device since it was looked at".into(),
                    ));
                    continue;
                }
                match fs::remove_file(path) {
                    Ok(()) => {
                        summary.deleted += 1;
                        summary.bytes += file.size;
                    }
                    Err(e) => summary.failed.push((file.name.clone(), e.to_string())),
                }
            }
            Origin::Camera { device, .. } => {
                on_device.entry(device.clone()).or_default().push(file)
            }
        }
    }
    for (device, files) in on_device {
        delete_on_device(&device, &files, &mut summary);
    }
    // What the source holds has changed: the next look reads it again.
    *LAST_SURVEY.lock().unwrap() = None;
    Ok(summary)
}

#[cfg(target_os = "macos")]
fn delete_on_device(device: &str, files: &[&Known], summary: &mut FreeUpSummary) {
    let wanted: Vec<(usize, &str, u64)> = files
        .iter()
        .filter_map(|file| match file.origin {
            Origin::Camera { index, .. } => Some((index, file.name.as_str(), file.size)),
            Origin::File(_) => None,
        })
        .collect();
    let by_index: BTreeMap<usize, &Known> = files
        .iter()
        .filter_map(|file| match file.origin {
            Origin::Camera { index, .. } => Some((index, *file)),
            Origin::File(_) => None,
        })
        .collect();
    match crate::camera_macos::delete(device, &wanted) {
        Ok(result) => {
            for index in result.deleted {
                if let Some(file) = by_index.get(&index) {
                    summary.deleted += 1;
                    summary.bytes += file.size;
                }
            }
            for (index, why) in result.failed {
                if let Some(file) = by_index.get(&index) {
                    summary.failed.push((file.name.clone(), why));
                }
            }
        }
        Err(why) => summary
            .failed
            .extend(files.iter().map(|file| (file.name.clone(), why.clone()))),
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_on_device(_: &str, files: &[&Known], summary: &mut FreeUpSummary) {
    summary.failed.extend(
        files
            .iter()
            .map(|file| (file.name.clone(), "not connected".to_string())),
    );
}

/// A folder name typed by the user, made safe: no slashes, no leading dot.
pub fn clean_folder_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    (!cleaned.is_empty() && cleaned.len() <= 100).then_some(cleaned)
}

/// The first suffix (none, then 2, 3…) under which none of `places`
/// (subfolder, name) is taken in `dir`.
fn free_suffix(dir: &Path, places: &[(PathBuf, &str)]) -> Option<u32> {
    let free = |suffix: Option<u32>| {
        places
            .iter()
            .all(|(sub, name)| !dir.join(sub).join(with_suffix(name, suffix)).exists())
    };
    if free(None) {
        return None;
    }
    (2..)
        .map(Some)
        .find(|&suffix| free(suffix))
        .expect("some name is free")
}

/// `IMG_1234 (2).HEIC` for `IMG_1234.HEIC` and 2.
fn with_suffix(name: &str, suffix: Option<u32>) -> String {
    let Some(n) = suffix else {
        return name.to_string();
    };
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Copies `source` to `target` through a hidden temporary file, checking the
/// written copy against what was read, and that what was read is still the
/// file that was surveyed. Keeps the file's modification time.
fn copy_checked(
    source: &Path,
    target: &Path,
    expected_fingerprint: &str,
    cancel: &AtomicBool,
    mut on_bytes: impl FnMut(u64),
) -> io::Result<()> {
    let dir = target.parent().expect("a folder");
    fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let partial = dir.join(format!(".{name}.photolib-new"));
    let result = (|| -> io::Result<()> {
        let mut input = File::open(source)?;
        let modified = input.metadata()?.modified().ok();
        let mut output = File::create(&partial)?;
        let mut read_hash = blake3::Hasher::new();
        let mut buffer = vec![0u8; BUFFER];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            read_hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
            on_bytes(n as u64);
        }
        if let Some(modified) = modified {
            output.set_modified(modified)?;
        }
        output.sync_all()?;
        drop(output);

        let mut written_hash = blake3::Hasher::new();
        let mut written = File::open(&partial)?;
        loop {
            let n = written.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            written_hash.update(&buffer[..n]);
        }
        if written_hash.finalize() != read_hash.finalize() {
            return Err(io::Error::other("the copy doesn't match the original"));
        }
        if fingerprint::fingerprint(&partial)? != expected_fingerprint {
            return Err(io::Error::other(
                "the file changed on the device during the import",
            ));
        }
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a file with that name appeared",
            ));
        }
        fs::rename(&partial, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// The fingerprints (and device keys, see [`camera_key`]) imported into
/// this library before, by any computer.
fn imported_before(library: &Path) -> HashSet<String> {
    let mut seen = HashSet::new();
    let Ok(entries) = fs::read_dir(history_dir(library)) else {
        return seen;
    };
    for entry in entries.filter_map(Result::ok) {
        if let Ok(file) = File::open(entry.path()) {
            seen.extend(
                BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .map(|line| line.trim().to_string())
                    .filter(|line| crate::thumbs::is_fingerprint(line) || line.starts_with("c:")),
            );
        }
    }
    seen
}

/// For files Photolib imported from a device: device key → fingerprint of
/// the library file it became (`m:` lines of the import logs).
fn imported_map(library: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(entries) = fs::read_dir(history_dir(library)) else {
        return map;
    };
    for entry in entries.filter_map(Result::ok) {
        if let Ok(file) = File::open(entry.path()) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                if let Some((key, fingerprint)) =
                    line.strip_prefix("m:").and_then(|l| l.split_once('\t'))
                {
                    map.insert(key.to_string(), fingerprint.trim().to_string());
                }
            }
        }
    }
    map
}

fn history_dir(library: &Path) -> PathBuf {
    library.join(".photolib").join("imports")
}

/// This computer's import log.
struct History {
    file: File,
}

impl History {
    fn open(library: &Path, device: &str) -> io::Result<Self> {
        let dir = history_dir(library);
        fs::create_dir_all(&dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{device}.log")))?;
        Ok(Self { file })
    }

    /// Records a fingerprint, or a device key.
    fn add(&mut self, line: &str) -> io::Result<()> {
        self.file.write_all(format!("{line}\n").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the last survey: one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn index(dir: &Path) -> Connection {
        crate::db::open(&dir.join("index.db")).unwrap()
    }

    /// A card with a Live Photo (HEIC + MOV, renamed .jpg here to be decodable
    /// without HEIC support) and a plain photo, in a DCIM folder.
    fn card(dir: &Path) -> PathBuf {
        let card = dir.join("card");
        let folder = card.join("DCIM/100APPLE");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("IMG_0001.JPG"), b"photo one").unwrap();
        fs::write(folder.join("IMG_0001.MOV"), b"video of photo one").unwrap();
        fs::write(folder.join("IMG_0002.JPG"), b"photo two").unwrap();
        fs::write(folder.join("notes.txt"), b"not a photo").unwrap();
        card
    }

    #[test]
    fn imports_only_new_files_once_and_keeps_companions_together() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let card = card(dir.path());
        let library = dir.path().join("library");
        fs::create_dir_all(&library).unwrap();
        let conn = index(dir.path());

        let card = card.to_string_lossy().into_owned();
        let first = preview(&card, &library, &conn, |_, _| {}).unwrap();
        assert_eq!((first.new, first.already), (3, 0));
        // Two tiles: the Live Photo (photo + video) and the plain photo.
        assert_eq!(first.tiles.len(), 2);
        let live = first.tiles.iter().find(|tile| tile.live).unwrap();
        assert_eq!(live.name, "IMG_0001.JPG");
        assert!(!live.already && !live.video);

        // A file of the same name, different content, is already there.
        let modified = fs::metadata(Path::new(&card).join("DCIM/100APPLE/IMG_0001.JPG"))
            .unwrap()
            .modified()
            .unwrap();
        let file_date = DateTime::<chrono::Local>::from(modified);
        let month = format!("{0}/{0}-{1:02}", file_date.year(), file_date.month());
        let dest = library.join("Card").join(&month);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("IMG_0001.MOV"), b"something else").unwrap();

        let cancel = AtomicBool::new(false);
        let summary = import(
            &card,
            &library,
            &conn,
            "Card",
            None,
            RawFiles::Beside,
            "dev1",
            &cancel,
            |_, _| {},
            |_| {},
        )
        .unwrap();
        assert_eq!(
            (summary.imported, summary.failed.len()),
            (3, 0),
            "{summary:?}"
        );
        // The Live Photo moved to a free stem together.
        assert_eq!(
            fs::read(dest.join("IMG_0001 (2).JPG")).unwrap(),
            b"photo one"
        );
        assert_eq!(
            fs::read(dest.join("IMG_0001 (2).MOV")).unwrap(),
            b"video of photo one"
        );
        assert_eq!(fs::read(dest.join("IMG_0002.JPG")).unwrap(), b"photo two");
        // No temporary files left.
        assert!(fs::read_dir(&dest).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.')));

        // Connected again: nothing new.
        let again = preview(&card, &library, &conn, |_, _| {}).unwrap();
        assert_eq!((again.new, again.already), (0, 3));
    }

    #[test]
    fn imports_only_the_selected_tiles() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let card = card(dir.path()).to_string_lossy().into_owned();
        let library = dir.path().join("library");
        fs::create_dir_all(&library).unwrap();
        let conn = index(dir.path());
        let preview = preview(&card, &library, &conn, |_, _| {}).unwrap();
        let plain = preview.tiles.iter().find(|tile| !tile.live).unwrap();
        let selection = HashSet::from([plain.group]);
        let cancel = AtomicBool::new(false);
        let summary = import(
            &card,
            &library,
            &conn,
            "Card",
            Some(&selection),
            RawFiles::Beside,
            "dev1",
            &cancel,
            |_, _| {},
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.imported, 1);
        // The Live Photo is still to import; the plain photo is done.
        let after = super::preview(&card, &library, &conn, |_, _| {}).unwrap();
        assert_eq!((after.new, after.already), (2, 1));
    }

    #[test]
    fn raw_files_go_beside_in_a_subfolder_or_nowhere() {
        let _serial = serial();
        for (choice, expected) in [
            (RawFiles::Beside, Some("IMG_0005.CR2")),
            (RawFiles::Subfolder, Some("cr2/IMG_0005.CR2")),
            (RawFiles::Skip, None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("card/DCIM/100CANON");
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("IMG_0005.JPG"), b"jpeg").unwrap();
            fs::write(folder.join("IMG_0005.CR2"), b"raw file").unwrap();
            let card = dir.path().join("card").to_string_lossy().into_owned();
            let library = dir.path().join("library");
            fs::create_dir_all(&library).unwrap();
            let conn = index(dir.path());
            let first = preview(&card, &library, &conn, |_, _| {}).unwrap();
            assert_eq!(first.tiles.len(), 1);
            assert!(first.tiles[0].raw && first.tiles[0].raw_bytes == 8);
            let cancel = AtomicBool::new(false);
            let summary = import(
                &card,
                &library,
                &conn,
                "Card",
                None,
                choice,
                "dev1",
                &cancel,
                |_, _| {},
                |_| {},
            )
            .unwrap();
            assert_eq!(summary.imported, if expected.is_some() { 2 } else { 1 });
            let month = WalkDir::new(library.join("Card"))
                .min_depth(2)
                .max_depth(2)
                .into_iter()
                .next()
                .unwrap()
                .unwrap()
                .into_path();
            assert!(month.join("IMG_0005.JPG").is_file());
            match expected {
                Some(raw) => assert!(month.join(raw).is_file(), "{choice:?}"),
                None => {
                    assert!(!month.join("IMG_0005.CR2").exists());
                    // Skipped on purpose: not offered again.
                    let again = preview(&card, &library, &conn, |_, _| {}).unwrap();
                    assert_eq!(again.new, 0);
                }
            }
        }
    }

    #[test]
    fn frees_up_only_what_is_safely_in_the_library() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let card = card(dir.path()).to_string_lossy().into_owned();
        let library = dir.path().join("library");
        fs::create_dir_all(&library).unwrap();
        let conn = index(dir.path());
        preview(&card, &library, &conn, |_, _| {}).unwrap();
        let cancel = AtomicBool::new(false);
        let summary = import(
            &card,
            &library,
            &conn,
            "Card",
            None,
            RawFiles::Beside,
            "dev1",
            &cancel,
            |_, _| {},
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.imported, 3);
        // The library indexes the copies (as the scan after an import does).
        for entry in WalkDir::new(&library).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() && !entry.path().starts_with(library.join(".photolib")) {
                let rel = entry
                    .path()
                    .strip_prefix(&library)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                crate::indexer::reindex_file(&library, &conn, &rel).unwrap();
            }
        }
        preview(&card, &library, &conn, |_, _| {}).unwrap();

        // Taken today (no EXIF: the file date): kept when keeping 30 days.
        let recent = free_up_plan(&card, &library, &conn, 30).unwrap();
        assert_eq!((recent.photos, recent.kept_recent), (0, 2));

        // One copy goes missing from the library: that photo stays on the card.
        let missing = WalkDir::new(&library)
            .into_iter()
            .filter_map(Result::ok)
            .find(|e| e.file_name() == "IMG_0002.JPG")
            .unwrap()
            .into_path();
        fs::remove_file(&missing).unwrap();
        let plan = free_up_plan(&card, &library, &conn, 0).unwrap();
        assert_eq!((plan.photos, plan.files, plan.not_checked), (1, 2, 1));

        let freed = delete_safe_files(&card, &library, &conn, 0).unwrap();
        assert_eq!((freed.deleted, freed.failed.len()), (2, 0));
        let folder = Path::new(&card).join("DCIM/100APPLE");
        assert!(!folder.join("IMG_0001.JPG").exists() && !folder.join("IMG_0001.MOV").exists());
        assert!(folder.join("IMG_0002.JPG").exists());
        // A folder that is not a device is never cleared.
        assert!(free_up(&card, &library, &conn, 0).is_err());
    }

    #[test]
    fn phones_mounted_by_gvfs_get_readable_names() {
        let name = |mount: &str| gvfs_device_name(mount);
        assert_eq!(
            name("mtp:host=SAMSUNG_SAMSUNG_Android_R58M12AB3CD").as_deref(),
            Some("SAMSUNG Android")
        );
        assert_eq!(
            name("mtp:host=HUAWEI_ELE-L29_7XBNW19A12003456").as_deref(),
            Some("HUAWEI ELE-L29")
        );
        assert_eq!(
            name("mtp:host=%5Busb%3A001%2C005%5D").as_deref(),
            Some("Phone")
        );
        assert_eq!(
            name("afc:host=00008030-001A2B3C4D5E6F,port=3").as_deref(),
            Some("iPhone")
        );
        assert_eq!(
            name("gphoto2:host=Canon_Inc._Canon_Digital_Camera").as_deref(),
            Some("Canon Inc. Canon Digital Camera")
        );
        assert_eq!(name("smb-share:server=nas,share=photos"), None);
    }

    #[test]
    fn folder_names_are_made_safe() {
        assert_eq!(
            clean_folder_name("Olivier's iPhone").as_deref(),
            Some("Olivier's iPhone")
        );
        assert_eq!(clean_folder_name("../x/y").as_deref(), Some("-x-y"));
        assert_eq!(clean_folder_name("  .. "), None);
        assert_eq!(group_stem(Path::new("IMG_E0042.HEIC")), "IMG_0042");
        assert_eq!(with_suffix("IMG_1.HEIC", Some(3)), "IMG_1 (3).HEIC");
    }
}
