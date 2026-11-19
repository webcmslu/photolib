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

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use chrono::{DateTime, Datelike};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use walkdir::WalkDir;

use crate::{fingerprint, metadata};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const BUFFER: usize = 1 << 20;

/// A memory card, camera or phone that shows up as a drive with a `DCIM`
/// folder (the folder every camera stores its photos in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub path: String,
    pub name: String,
}

/// The drives connected right now that hold photos.
pub fn sources() -> Vec<Source> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        roots.extend(children("/Volumes"));
    } else if cfg!(windows) {
        roots.extend((b'A'..=b'Z').map(|letter| PathBuf::from(format!("{}:\\", letter as char))));
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        for base in [format!("/media/{user}"), format!("/run/media/{user}"), "/media".into(), "/mnt".into()] {
            roots.extend(children(&base));
        }
    }
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|root| root.join("DCIM").is_dir() && seen.insert(root.clone()))
        .map(|root| Source {
            name: drive_name(&root),
            path: root.to_string_lossy().into_owned(),
        })
        .collect()
}

fn children(dir: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// The name a drive shows: its folder name ("EOS_DIGITAL"), or on Windows
/// its letter.
fn drive_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().trim_end_matches('\\').to_string())
}

/// One file to import.
#[derive(Debug, Clone)]
struct Item {
    source: PathBuf,
    name: String,
    size: u64,
    fingerprint: String,
    /// The folder of the library it goes to: `<year>/<year>-<month>`.
    month: String,
    /// Files with the same group number keep a common name stem.
    group: usize,
}

/// What an import would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Photos and videos to copy, and their size in bytes.
    pub new: usize,
    pub new_bytes: u64,
    /// Already in the library, or imported before.
    pub already: usize,
    /// Files that could not be read.
    pub unreadable: usize,
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
struct Survey {
    new: Vec<Item>,
    already: usize,
    unreadable: Vec<(String, String)>,
    days: Vec<i64>,
}

/// Reads what `source` holds (its `DCIM` folder if it has one) and sorts out
/// what the library at `library` does not have yet.
fn survey(source: &Path, library: &Path, conn: &Connection) -> io::Result<Survey> {
    let top = if source.join("DCIM").is_dir() { source.join("DCIM") } else { source.to_path_buf() };
    if !top.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the device is not connected"));
    }
    // Media files, grouped by folder and name stem.
    let mut groups: BTreeMap<(PathBuf, String), Vec<PathBuf>> = BTreeMap::new();
    for entry in WalkDir::new(&top)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.into_path();
        if metadata::kind_of(&path).is_none() {
            continue;
        }
        let stem = group_stem(&path);
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
        groups.entry((parent, stem)).or_default().push(path);
    }

    let imported = imported_before(library);
    let mut survey = Survey { new: Vec::new(), already: 0, unreadable: Vec::new(), days: Vec::new() };
    let mut in_library = conn.prepare("SELECT 1 FROM photos WHERE fingerprint = ?1 LIMIT 1")
        .map_err(io::Error::other)?;
    for (group, (_, mut files)) in groups.into_iter().enumerate() {
        files.sort();
        // The group's date: its photo's, else its video's, else the file's.
        let mut dated: Vec<(bool, i64)> = files
            .iter()
            .filter_map(|path| {
                let is_video = metadata::kind_of(path) == Some(metadata::Kind::Video);
                metadata::read(path).taken_at.map(|ms| (is_video, ms))
            })
            .collect();
        dated.sort();
        let taken = dated.first().map(|&(_, ms)| ms).or_else(|| {
            files.iter().find_map(|path| {
                let modified = fs::metadata(path).ok()?.modified().ok()?;
                let unix_ms = modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
                Some(crate::indexer::wall_clock_ms(unix_ms))
            })
        });
        let date = taken.and_then(DateTime::from_timestamp_millis).unwrap_or_default();
        let month = format!("{0}/{0}-{1:02}", date.year(), date.month());
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (size, fingerprint) = match fs::metadata(&path).and_then(|m| Ok((m.len(), fingerprint::fingerprint(&path)?))) {
                Ok(read) => read,
                Err(e) => {
                    survey.unreadable.push((name, e.to_string()));
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
                survey.already += 1;
                continue;
            }
            if let Some(ms) = taken {
                survey.days.push(ms.div_euclid(86_400_000));
            }
            survey.new.push(Item { source: path, name, size, fingerprint, month: month.clone(), group });
        }
    }
    Ok(survey)
}

/// The part of a file name that ties it to its companions: `IMG_1234` for
/// `IMG_1234.HEIC`, `IMG_1234.MOV` and the edited `IMG_E1234.HEIC`.
fn group_stem(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default();
    match stem.strip_prefix("IMG_E") {
        Some(number) if number.chars().all(|c| c.is_ascii_digit()) => format!("IMG_{number}"),
        _ => stem,
    }
}

/// What importing `source` into the library would do.
pub fn preview(source: &Path, library: &Path, conn: &Connection) -> io::Result<Preview> {
    let survey = survey(source, library, conn)?;
    Ok(Preview {
        new: survey.new.len(),
        new_bytes: survey.new.iter().map(|item| item.size).sum(),
        already: survey.already,
        unreadable: survey.unreadable.len(),
        first_day: survey.days.iter().min().copied(),
        last_day: survey.days.iter().max().copied(),
    })
}

/// Copies what is new on `source` into `<library>/<folder>/<year>/<year>-<month>`.
/// Returns what was done; the caller then scans the library for the new files.
pub fn import(
    source: &Path,
    library: &Path,
    conn: &Connection,
    folder: &str,
    device: &str,
    cancel: &AtomicBool,
    on_progress: impl Fn(Progress),
) -> io::Result<Summary> {
    let folder = clean_folder_name(folder)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Not a valid folder name."))?;
    let survey = survey(source, library, conn)?;
    let mut summary = Summary { already: survey.already, failed: survey.unreadable, ..Summary::default() };
    let mut progress = Progress {
        done: 0,
        total: survey.new.len(),
        bytes: 0,
        total_bytes: survey.new.iter().map(|item| item.size).sum(),
    };
    on_progress(progress);
    let mut last_progress = Instant::now();

    let mut history = History::open(library, device)?;
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
            let names: Vec<&str> = by_group[&item.group].iter().map(|i| i.name.as_str()).collect();
            let suffix = free_suffix(&dir, &names);
            (dir, suffix)
        });
        let target = dir.join(with_suffix(&item.name, *suffix));
        match copy_checked(&item.source, &target, &item.fingerprint, cancel, |chunk| {
            progress.bytes += chunk;
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                on_progress(progress);
            }
        }) {
            Ok(()) => {
                history.add(&item.fingerprint)?;
                summary.imported += 1;
                summary.bytes += item.size;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                summary.cancelled = true;
                break;
            }
            Err(e) => summary.failed.push((item.name.clone(), e.to_string())),
        }
        progress.done += 1;
    }
    on_progress(progress);
    Ok(summary)
}

/// A folder name typed by the user, made safe: no slashes, no leading dot.
pub fn clean_folder_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    (!cleaned.is_empty() && cleaned.len() <= 100).then_some(cleaned)
}

/// The first suffix (none, then 2, 3…) under which none of `names` is taken in `dir`.
fn free_suffix(dir: &Path, names: &[&str]) -> Option<u32> {
    let free = |suffix: Option<u32>| names.iter().all(|name| !dir.join(with_suffix(name, suffix)).exists());
    if free(None) {
        return None;
    }
    (2..).map(Some).find(|&suffix| free(suffix)).expect("some name is free")
}

/// `IMG_1234 (2).HEIC` for `IMG_1234.HEIC` and 2.
fn with_suffix(name: &str, suffix: Option<u32>) -> String {
    let Some(n) = suffix else { return name.to_string() };
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
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
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
            return Err(io::Error::other("the file changed on the device during the import"));
        }
        if target.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name appeared"));
        }
        fs::rename(&partial, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// The fingerprints imported into this library before, by any computer.
fn imported_before(library: &Path) -> HashSet<String> {
    let mut seen = HashSet::new();
    let Ok(entries) = fs::read_dir(history_dir(library)) else { return seen };
    for entry in entries.filter_map(Result::ok) {
        if let Ok(file) = File::open(entry.path()) {
            seen.extend(
                BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .map(|line| line.trim().to_string())
                    .filter(|line| crate::thumbs::is_fingerprint(line)),
            );
        }
    }
    seen
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
        let file = OpenOptions::new().create(true).append(true).open(dir.join(format!("{device}.log")))?;
        Ok(Self { file })
    }

    fn add(&mut self, fingerprint: &str) -> io::Result<()> {
        self.file.write_all(format!("{fingerprint}\n").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = tempfile::tempdir().unwrap();
        let card = card(dir.path());
        let library = dir.path().join("library");
        fs::create_dir_all(&library).unwrap();
        let conn = index(dir.path());

        let first = preview(&card, &library, &conn).unwrap();
        assert_eq!((first.new, first.already), (3, 0));

        // A file of the same name, different content, is already there.
        let modified = fs::metadata(card.join("DCIM/100APPLE/IMG_0001.JPG")).unwrap().modified().unwrap();
        let file_date = DateTime::<chrono::Local>::from(modified);
        let month = format!("{0}/{0}-{1:02}", file_date.year(), file_date.month());
        let dest = library.join("Card").join(&month);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("IMG_0001.MOV"), b"something else").unwrap();

        let cancel = AtomicBool::new(false);
        let summary = import(&card, &library, &conn, "Card", "dev1", &cancel, |_| {}).unwrap();
        assert_eq!((summary.imported, summary.failed.len()), (3, 0), "{summary:?}");
        // The Live Photo moved to a free stem together.
        assert_eq!(fs::read(dest.join("IMG_0001 (2).JPG")).unwrap(), b"photo one");
        assert_eq!(fs::read(dest.join("IMG_0001 (2).MOV")).unwrap(), b"video of photo one");
        assert_eq!(fs::read(dest.join("IMG_0002.JPG")).unwrap(), b"photo two");
        // No temporary files left.
        assert!(fs::read_dir(&dest).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with('.')));

        // Connected again: nothing new.
        let again = preview(&card, &library, &conn).unwrap();
        assert_eq!((again.new, again.already), (0, 3));
    }

    #[test]
    fn folder_names_are_made_safe() {
        assert_eq!(clean_folder_name("Olivier's iPhone").as_deref(), Some("Olivier's iPhone"));
        assert_eq!(clean_folder_name("../x/y").as_deref(), Some("-x-y"));
        assert_eq!(clean_folder_name("  .. "), None);
        assert_eq!(group_stem(Path::new("IMG_E0042.HEIC")), "IMG_0042");
        assert_eq!(with_suffix("IMG_1.HEIC", Some(3)), "IMG_1 (3).HEIC");
    }
}
