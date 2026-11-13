//! Scans a library folder into the local index.
//!
//! A scan has two phases:
//! 1. **Scanning**: walk the folder and `stat` every image. A file whose size and
//!    mtime match the index is left alone, which makes rescans cheap.
//! 2. **Indexing**: fingerprint and read EXIF for the new or changed files, in
//!    parallel, while this thread writes the results to SQLite in batches.
//!
//! The scanner only ever reads the library folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;
use walkdir::{DirEntry, WalkDir};

use crate::db::{self, PhotoRow};
use crate::{fingerprint, metadata};

/// File types the index picks up. RAW and video come in phase 2.
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "heic", "heif", "gif", "tif", "tiff",
];

/// Folders that NAS systems fill with their own thumbnails and trash.
const SKIPPED_DIRS: &[&str] = &[
    "@eaDir",
    "#recycle",
    "$RECYCLE.BIN",
    "System Volume Information",
];

const WRITE_BATCH: usize = 500;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Scanning,
    Indexing,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    /// Image files found so far by the walk.
    pub discovered: u64,
    /// New or changed files that need indexing (known once the walk is done).
    pub to_index: u64,
    /// How many of `to_index` are done.
    pub indexed: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    /// Photos in the index after the scan.
    pub total: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
    /// Files that could not be read.
    pub failed: u64,
    /// Folders that could not be listed. When this is not zero, nothing was
    /// removed from the index: the files may just be temporarily unreachable.
    pub unreadable_dirs: u64,
    pub cancelled: bool,
    pub duration_ms: u64,
}

#[derive(Debug)]
pub enum ScanError {
    /// The library folder itself can't be listed (unplugged drive, NAS offline).
    RootUnreachable(String),
    Database(rusqlite::Error),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootUnreachable(why) => write!(f, "The library folder is not reachable: {why}"),
            Self::Database(e) => write!(f, "Index database error: {e}"),
        }
    }
}

impl From<rusqlite::Error> for ScanError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

/// A new or changed file waiting to be indexed.
struct Candidate {
    abs_path: PathBuf,
    rel_path: String,
    size: u64,
    mtime_ms: i64,
    is_new: bool,
}

/// Brings the index in `conn` up to date with the folder at `root`.
///
/// Set `cancel` from another thread to stop early; whatever was indexed so far
/// is kept. `on_progress` is called from this thread only.
pub fn scan(
    root: &Path,
    conn: &mut Connection,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(Progress),
) -> Result<ScanSummary, ScanError> {
    let started = Instant::now();
    let mut summary = ScanSummary::default();
    let known = db::load_known(conn)?;

    // Phase 1: walk.
    let mut seen: HashSet<String> = HashSet::with_capacity(known.len());
    let mut todo: Vec<Candidate> = Vec::new();
    let mut discovered = 0u64;
    let mut last_progress = Instant::now();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_skipped(entry));

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.depth() == 0 => return Err(ScanError::RootUnreachable(e.to_string())),
            Err(_) => {
                summary.unreadable_dirs += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() || !has_image_extension(entry.path()) {
            continue;
        }
        let Some(rel_path) = relative_path(root, entry.path()) else {
            summary.failed += 1; // name is not valid Unicode
            continue;
        };
        discovered += 1;
        seen.insert(rel_path.clone());

        let Ok(meta) = entry.metadata() else {
            summary.failed += 1;
            continue;
        };
        let size = meta.len();
        let mtime_ms = meta.modified().map(unix_ms).unwrap_or(0);

        match known.get(&rel_path) {
            Some(&(known_size, known_mtime)) if known_size == size && known_mtime == mtime_ms => {
                summary.unchanged += 1;
            }
            previous => todo.push(Candidate {
                abs_path: entry.into_path(),
                rel_path,
                size,
                mtime_ms,
                is_new: previous.is_none(),
            }),
        }

        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            on_progress(Progress {
                phase: Phase::Scanning,
                discovered,
                to_index: 0,
                indexed: 0,
            });
        }
    }

    // Phase 2: index new and changed files. Workers read files in parallel and
    // send rows over a bounded channel; this thread is the only database writer.
    let to_index = todo.len() as u64;
    let mut indexed = 0u64;
    on_progress(Progress {
        phase: Phase::Indexing,
        discovered,
        to_index,
        indexed,
    });

    let write_result = std::thread::scope(|scope| -> rusqlite::Result<()> {
        let (sender, receiver) = mpsc::sync_channel::<(bool, Option<PhotoRow>)>(WRITE_BATCH);
        let todo = &todo;
        scope.spawn(move || {
            todo.par_iter().for_each_with(sender, |sender, candidate| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                // A send error means the writer stopped; nothing left to do.
                let _ = sender.send((candidate.is_new, index_file(candidate)));
            });
        });

        let mut pending = 0usize;
        let mut tx = conn.transaction()?;
        for (is_new, row) in receiver {
            match row {
                Some(row) => {
                    db::upsert(&tx, &row)?;
                    if is_new {
                        summary.added += 1;
                    } else {
                        summary.updated += 1;
                    }
                }
                None => summary.failed += 1,
            }
            indexed += 1;
            pending += 1;
            if pending >= WRITE_BATCH {
                tx.commit()?;
                tx = conn.transaction()?;
                pending = 0;
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                on_progress(Progress {
                    phase: Phase::Indexing,
                    discovered,
                    to_index,
                    indexed,
                });
            }
        }
        tx.commit()
    });
    write_result?;

    summary.cancelled = cancel.load(Ordering::Relaxed);

    // Forget files that are gone, but only after a complete, clean walk.
    // Otherwise an unplugged drive would look like "everything was deleted".
    if !summary.cancelled && summary.unreadable_dirs == 0 {
        let tx = conn.transaction()?;
        for rel_path in known.keys().filter(|path| !seen.contains(*path)) {
            db::delete(&tx, rel_path)?;
            summary.removed += 1;
        }
        tx.commit()?;
    }

    summary.total = db::count(conn)?;
    summary.duration_ms = started.elapsed().as_millis() as u64;
    Ok(summary)
}

/// Reads one file. `None` means it could not be read.
fn index_file(candidate: &Candidate) -> Option<PhotoRow> {
    let fingerprint = fingerprint::fingerprint(&candidate.abs_path).ok()?;
    let meta = metadata::read(&candidate.abs_path);
    Some(PhotoRow {
        rel_path: candidate.rel_path.clone(),
        size: candidate.size,
        mtime_ms: candidate.mtime_ms,
        fingerprint,
        sort_date: meta
            .taken_at
            .unwrap_or_else(|| wall_clock_ms(candidate.mtime_ms)),
        date_from_exif: meta.taken_at.is_some(),
        orientation: meta.orientation,
        width: meta.width,
        height: meta.height,
        camera_make: meta.camera_make,
        camera_model: meta.camera_model,
        latitude: meta.latitude,
        longitude: meta.longitude,
    })
}

/// Hidden entries (`.photolib`, `.DS_Store`, macOS `._name.jpg` companions) and
/// NAS housekeeping folders.
fn is_skipped(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || (entry.file_type().is_dir() && SKIPPED_DIRS.contains(&name.as_ref()))
}

fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// Path relative to the library root with '/' separators, so the same library
/// indexes identically on Windows, macOS and Linux.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}

fn unix_ms(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    }
}

/// Converts a real instant to the "wall-clock as if UTC" encoding used for
/// `sort_date`, using this computer's time zone.
fn wall_clock_ms(unix_ms: i64) -> i64 {
    DateTime::from_timestamp_millis(unix_ms)
        .map(|utc| {
            utc.with_timezone(&Local)
                .naive_local()
                .and_utc()
                .timestamp_millis()
        })
        .unwrap_or(unix_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::testing::{ascii_field as ascii, jpeg_with_exif};
    use exif::Tag;
    use std::fs;

    fn photo(date: &str) -> Vec<u8> {
        jpeg_with_exif(&[ascii(Tag::DateTimeOriginal, date)])
    }

    fn run(root: &Path, conn: &mut Connection) -> ScanSummary {
        scan(root, conn, &AtomicBool::new(false), &|_| {}).unwrap()
    }

    fn fingerprint_of(conn: &Connection, rel_path: &str) -> String {
        conn.query_row(
            "SELECT fingerprint FROM photos WHERE rel_path = ?1",
            [rel_path],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn indexes_images_and_skips_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("2024/summer")).unwrap();
        fs::create_dir_all(root.join(".photolib/devices")).unwrap();
        fs::create_dir_all(root.join("@eaDir")).unwrap();
        fs::write(root.join("a.jpg"), photo("2024:01:01 10:00:00")).unwrap();
        fs::write(
            root.join("2024/summer/b.JPEG"),
            photo("2024:07:01 10:00:00"),
        )
        .unwrap();
        fs::write(root.join("2024/c.png"), b"no exif here").unwrap();
        fs::write(root.join("notes.txt"), b"text").unwrap();
        fs::write(root.join("._a.jpg"), b"appledouble").unwrap();
        fs::write(root.join(".photolib/devices/x.jpg"), b"hidden").unwrap();
        fs::write(root.join("@eaDir/thumb.jpg"), b"nas thumbnail").unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let summary = run(root, &mut conn);

        assert_eq!((summary.added, summary.total, summary.failed), (3, 3, 0));
        let mut paths: Vec<String> = db::load_known(&conn).unwrap().into_keys().collect();
        paths.sort();
        assert_eq!(paths, ["2024/c.png", "2024/summer/b.JPEG", "a.jpg"]);

        let sources: Vec<(String, String)> = conn
            .prepare("SELECT rel_path, date_source FROM photos ORDER BY sort_date")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sources[0], ("a.jpg".into(), "exif".into()));
        assert_eq!(sources[1], ("2024/summer/b.JPEG".into(), "exif".into()));
        assert_eq!(sources[2], ("2024/c.png".into(), "file".into()));
    }

    #[test]
    fn rescan_is_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.jpg"), photo("2024:01:01 10:00:00")).unwrap();
        fs::write(root.join("change.jpg"), photo("2024:01:02 10:00:00")).unwrap();
        fs::write(root.join("delete.jpg"), photo("2024:01:03 10:00:00")).unwrap();

        let mut conn = db::open_in_memory().unwrap();
        assert_eq!(run(root, &mut conn).added, 3);

        let again = run(root, &mut conn);
        assert_eq!(
            (again.added, again.updated, again.removed, again.unchanged),
            (0, 0, 0, 3)
        );

        // Different size, so the change is detected whatever the mtime resolution.
        fs::write(
            root.join("change.jpg"),
            jpeg_with_exif(&[
                ascii(Tag::DateTimeOriginal, "2024:01:02 10:00:00"),
                ascii(Tag::Make, "Canon"),
            ]),
        )
        .unwrap();
        fs::remove_file(root.join("delete.jpg")).unwrap();
        fs::write(root.join("new.jpg"), photo("2024:01:04 10:00:00")).unwrap();

        let third = run(root, &mut conn);
        assert_eq!(
            (
                third.added,
                third.updated,
                third.removed,
                third.unchanged,
                third.total
            ),
            (1, 1, 1, 1, 3)
        );
    }

    #[test]
    fn rename_keeps_the_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("old.jpg"), photo("2024:01:01 10:00:00")).unwrap();

        let mut conn = db::open_in_memory().unwrap();
        run(root, &mut conn);
        let before = fingerprint_of(&conn, "old.jpg");

        fs::create_dir(root.join("album")).unwrap();
        fs::rename(root.join("old.jpg"), root.join("album/new.jpg")).unwrap();
        let summary = run(root, &mut conn);

        assert_eq!((summary.added, summary.removed, summary.total), (1, 1, 1));
        assert_eq!(fingerprint_of(&conn, "album/new.jpg"), before);
    }

    #[test]
    fn unreachable_root_is_an_error_and_keeps_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.jpg"), photo("2024:01:01 10:00:00")).unwrap();

        let mut conn = db::open_in_memory().unwrap();
        run(&root, &mut conn);

        fs::remove_dir_all(&root).unwrap(); // "the NAS went away"
        let result = scan(&root, &mut conn, &AtomicBool::new(false), &|_| {});

        assert!(matches!(result, Err(ScanError::RootUnreachable(_))));
        assert_eq!(db::count(&conn).unwrap(), 1);
    }

    #[test]
    fn cancelled_scan_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.jpg"), photo("2024:01:01 10:00:00")).unwrap();

        let mut conn = db::open_in_memory().unwrap();
        run(root, &mut conn);
        fs::remove_file(root.join("a.jpg")).unwrap();

        let summary = scan(root, &mut conn, &AtomicBool::new(true), &|_| {}).unwrap();
        assert!(summary.cancelled);
        assert_eq!((summary.removed, summary.total), (0, 1));
    }

    #[test]
    fn reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.jpg"), photo("2024:01:01 10:00:00")).unwrap();

        let phases = std::cell::RefCell::new(Vec::new());
        let mut conn = db::open_in_memory().unwrap();
        scan(dir.path(), &mut conn, &AtomicBool::new(false), &|p| {
            phases.borrow_mut().push((p.phase, p.to_index))
        })
        .unwrap();

        assert!(phases.borrow().contains(&(Phase::Indexing, 1)));
    }

    /// The plan's integration check: `cargo test --release -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: generates and indexes 10,000 files"]
    fn benchmark_10k_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10_000u32 {
            let folder = dir.path().join(format!("{:03}", i / 100));
            fs::create_dir_all(&folder).unwrap();
            let date = format!(
                "20{:02}:{:02}:{:02} 12:00:00",
                i % 25,
                i % 12 + 1,
                i % 28 + 1
            );
            let mut data = jpeg_with_exif(&[
                ascii(Tag::DateTimeOriginal, &date),
                ascii(Tag::Model, &format!("Camera {i}")),
            ]);
            data.resize(200 * 1024, (i % 251) as u8); // photo-sized enough to hit both hash chunks
            fs::write(folder.join(format!("img_{i:05}.jpg")), data).unwrap();
        }

        let mut conn = db::open(&dir.path().join(".index/index.db")).unwrap();
        let first = run(dir.path(), &mut conn);
        let second = run(dir.path(), &mut conn);
        println!(
            "first scan:  {} files in {} ms",
            first.added, first.duration_ms
        );
        println!(
            "rescan:      {} unchanged in {} ms",
            second.unchanged, second.duration_ms
        );
        assert_eq!((first.added, second.unchanged), (10_000, 10_000));
    }
}
