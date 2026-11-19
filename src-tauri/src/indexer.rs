//! Scans a library folder into the local index.
//!
//! A scan does two things at once:
//! - **Walk**: list the folder and `stat` every image. A file whose size and
//!   mtime match the index is left alone, which makes rescans cheap.
//! - **Index**: fingerprint and read EXIF for the new or changed files, in
//!   parallel, while the calling thread writes the results to SQLite.
//!
//! Progress says `Scanning` while the walk runs and `Indexing` once it is done
//! and only the remaining files are being read.
//!
//! The scanner only ever reads the library folder.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use rayon::prelude::*;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use walkdir::{DirEntry, WalkDir};

use crate::db::{self, PhotoRow};
use crate::{fingerprint, metadata, places};

/// Folders that NAS systems fill with their own thumbnails and trash.
const SKIPPED_DIRS: &[&str] = &[
    "@eaDir",
    "#recycle",
    "$RECYCLE.BIN",
    "System Volume Information",
];

const WRITE_BATCH: usize = 500;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// New rows become visible to the UI at least this often during a scan.
const COMMIT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Scanning,
    Indexing,
    Thumbnails,
    /// Working out which photos look alike (see similar.rs).
    Comparing,
    Faces,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    /// Image files found so far by the walk.
    pub discovered: u64,
    /// Items the current phase has to process: new or changed files found so
    /// far when scanning or indexing, photos without thumbnails when making
    /// thumbnails.
    pub total: u64,
    /// How many of `total` are done.
    pub done: u64,
    /// Files the index already held when the scan started, so the walk of a
    /// known library reads as "checked 12,000 of 80,000".
    pub known: u64,
}

/// What a scan reads again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only files that are new or whose size or date changed: the usual check.
    #[default]
    Changes,
    /// Every file, as on the first scan, and photos whose thumbnail could not
    /// be made get another try. Thumbnails of unchanged content are kept.
    Everything,
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

/// A new or changed file waiting to be done.
struct Candidate {
    abs_path: PathBuf,
    rel_path: String,
    size: u64,
    mtime_ms: i64,
    is_new: bool,
}

/// What the walk found, besides the candidates it handed on.
#[derive(Default)]
struct Walked {
    seen: HashSet<String>,
    unchanged: u64,
    failed: u64,
    unreadable_dirs: u64,
}

/// Brings the index in `conn` up to date with the folder at `root`.
///
/// The walk, the file reading and the database writes run at the same time,
/// and rows are committed at least every `COMMIT_INTERVAL`, so on a slow NAS
/// photos show up while the folders are still being listed.
///
/// Set `cancel` from another thread to stop early; whatever was done so far
/// is kept. `on_progress` is called from this thread only.
#[cfg(test)]
pub fn scan(
    root: &Path,
    conn: &mut Connection,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(Progress),
) -> Result<ScanSummary, ScanError> {
    scan_with(root, conn, Mode::Changes, cancel, on_progress)
}

/// [`scan`], choosing what is read again.
pub fn scan_with(
    root: &Path,
    conn: &mut Connection,
    mode: Mode,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(Progress),
) -> Result<ScanSummary, ScanError> {
    let started = Instant::now();
    let mut summary = ScanSummary::default();
    let known = db::load_known(conn)?;
    let known_count = known.len() as u64;
    if mode == Mode::Everything {
        db::retry_failed_thumbs(conn)?;
    }

    // Shared with the walker thread, read here for the progress reports.
    let discovered = AtomicU64::new(0);
    let queued = AtomicU64::new(0);
    let walking = AtomicBool::new(true);

    let walked = std::thread::scope(|scope| -> Result<Walked, ScanError> {
        let (candidate_tx, candidate_rx) = mpsc::sync_channel::<Candidate>(WRITE_BATCH);
        let (row_tx, row_rx) = mpsc::sync_channel::<(bool, Option<PhotoRow>)>(WRITE_BATCH);

        let (known, discovered, queued, walking) = (&known, &discovered, &queued, &walking);
        let walker = scope.spawn(move || {
            let result = walk(root, known, mode, cancel, discovered, queued, candidate_tx);
            walking.store(false, Ordering::Relaxed);
            result
        });

        // Workers read files in parallel as the walk hands them over.
        scope.spawn(move || {
            let _ = candidate_rx.into_iter().par_bridge().try_for_each_with(
                row_tx,
                |row_tx, candidate| {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    // A send error means the writer stopped: stop too, which
                    // in turn stops the walk.
                    row_tx
                        .send((candidate.is_new, index_file(&candidate)))
                        .map_err(|_| ())
                },
            );
        });

        // This thread is the only database writer.
        let mut done = 0u64;
        let mut pending = 0usize;
        let mut last_commit = Instant::now();
        let mut last_progress = Instant::now();
        let mut reported_walk_end = false;
        let mut tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        loop {
            match row_rx.recv_timeout(PROGRESS_INTERVAL) {
                Ok((is_new, Some(row))) => {
                    db::upsert(&tx, &row)?;
                    if is_new {
                        summary.added += 1;
                    } else {
                        summary.updated += 1;
                    }
                    done += 1;
                    pending += 1;
                }
                Ok((_, None)) => {
                    summary.failed += 1;
                    done += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if pending >= WRITE_BATCH || (pending > 0 && last_commit.elapsed() >= COMMIT_INTERVAL) {
                tx.commit()?;
                tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                pending = 0;
                last_commit = Instant::now();
            }
            let still_walking = walking.load(Ordering::Relaxed);
            let walk_just_ended = !still_walking && !reported_walk_end;
            if walk_just_ended || last_progress.elapsed() >= PROGRESS_INTERVAL {
                reported_walk_end |= walk_just_ended;
                last_progress = Instant::now();
                on_progress(Progress {
                    phase: if still_walking {
                        Phase::Scanning
                    } else {
                        Phase::Indexing
                    },
                    discovered: discovered.load(Ordering::Relaxed),
                    total: queued.load(Ordering::Relaxed),
                    done,
                    known: known_count,
                });
            }
        }
        tx.commit()?;
        walker.join().expect("the folder walk panicked")
    })?;

    summary.unchanged = walked.unchanged;
    summary.failed += walked.failed;
    summary.unreadable_dirs = walked.unreadable_dirs;
    summary.cancelled = cancel.load(Ordering::Relaxed);

    // Forget files that are gone, but only after a complete, clean walk.
    // Otherwise an unplugged drive would look like "everything was deleted".
    if !summary.cancelled && summary.unreadable_dirs == 0 {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for rel_path in known.keys().filter(|path| !walked.seen.contains(*path)) {
            db::delete(&tx, rel_path)?;
            summary.removed += 1;
        }
        tx.commit()?;
    }

    // After additions and removals, work out again which videos and RAW
    // files belong to which photos.
    if !summary.cancelled {
        db::pair_parts(conn)?;
        // Name the places of the photos that are new or moved.
        db::fill_places(conn, |latitude, longitude| {
            places::lookup(latitude, longitude).map(|place| (place.name, place.search))
        })?;
    }

    summary.total = db::count(conn)?;
    summary.duration_ms = started.elapsed().as_millis() as u64;
    Ok(summary)
}

/// Walks the folder and `stat`s every image, sending the new and changed ones
/// to `candidates`. Files that match the index are only counted.
fn walk(
    root: &Path,
    known: &HashMap<String, (u64, i64)>,
    mode: Mode,
    cancel: &AtomicBool,
    discovered: &AtomicU64,
    queued: &AtomicU64,
    candidates: mpsc::SyncSender<Candidate>,
) -> Result<Walked, ScanError> {
    let mut walked = Walked {
        seen: HashSet::with_capacity(known.len()),
        ..Walked::default()
    };
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
                walked.unreadable_dirs += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() || metadata::kind_of(entry.path()).is_none() {
            continue;
        }
        let Some(rel_path) = relative_path(root, entry.path()) else {
            walked.failed += 1; // name is not valid Unicode
            continue;
        };
        discovered.fetch_add(1, Ordering::Relaxed);
        walked.seen.insert(rel_path.clone());

        let Ok(meta) = entry.metadata() else {
            walked.failed += 1;
            continue;
        };
        let size = meta.len();
        let mtime_ms = meta.modified().map(unix_ms).unwrap_or(0);

        match known.get(&rel_path) {
            Some(&(known_size, known_mtime))
                if mode == Mode::Changes && known_size == size && known_mtime == mtime_ms =>
            {
                walked.unchanged += 1;
            }
            previous => {
                let candidate = Candidate {
                    abs_path: entry.into_path(),
                    rel_path,
                    size,
                    mtime_ms,
                    is_new: previous.is_none(),
                };
                queued.fetch_add(1, Ordering::Relaxed);
                if candidates.send(candidate).is_err() {
                    break; // the writer stopped (database error)
                }
            }
        }
    }
    Ok(walked)
}

/// Reads one file. `None` means it could not be read.
fn index_file(candidate: &Candidate) -> Option<PhotoRow> {
    let fingerprint = fingerprint::fingerprint(&candidate.abs_path).ok()?;
    let meta = metadata::read(&candidate.abs_path);
    // EXIF gives the stored size; orientations 5 to 8 display it turned sideways.
    let sideways = meta.orientation.is_some_and(|o| o >= 5);
    let (width, height) = if sideways {
        (meta.height, meta.width)
    } else {
        (meta.width, meta.height)
    };
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
        width,
        height,
        camera_make: meta.camera_make,
        camera_model: meta.camera_model,
        latitude: meta.latitude,
        longitude: meta.longitude,
        is_video: meta.is_video,
        duration_ms: meta.duration_ms,
        live_candidate: meta.live_candidate,
    })
}

/// Hidden entries (`.photolib`, `.DS_Store`, macOS `._name.jpg` companions),
/// NAS housekeeping folders, and the `Duplicates to delete` folder.
fn is_skipped(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || (entry.file_type().is_dir() && SKIPPED_DIRS.contains(&name.as_ref()))
        // Extra copies set aside for deletion (see duplicates.rs).
        || (entry.depth() == 1 && entry.file_type().is_dir() && name == crate::duplicates::PARKED)
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
        // Extra copies set aside are skipped at the top only.
        fs::create_dir_all(root.join("Duplicates to delete/2024")).unwrap();
        fs::write(
            root.join("Duplicates to delete/2024/a.jpg"),
            photo("2024:01:01 10:00:00"),
        )
        .unwrap();
        fs::create_dir_all(root.join("2024/Duplicates to delete")).unwrap();
        fs::write(
            root.join("2024/Duplicates to delete/d.jpg"),
            photo("2024:01:03 10:00:00"),
        )
        .unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let summary = run(root, &mut conn);

        assert_eq!((summary.added, summary.total, summary.failed), (4, 4, 0));
        let mut paths: Vec<String> = db::load_known(&conn).unwrap().into_keys().collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "2024/Duplicates to delete/d.jpg",
                "2024/c.png",
                "2024/summer/b.JPEG",
                "a.jpg"
            ]
        );

        let sources: Vec<(String, String)> = conn
            .prepare("SELECT rel_path, date_source FROM photos ORDER BY sort_date")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sources[0], ("a.jpg".into(), "exif".into()));
        assert_eq!(sources[2], ("2024/summer/b.JPEG".into(), "exif".into()));
        assert_eq!(sources[3], ("2024/c.png".into(), "file".into()));
    }

    #[test]
    fn everything_mode_reads_every_file_and_keeps_thumbnails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.jpg"), photo("2024:01:01 10:00:00")).unwrap();
        fs::write(root.join("b.jpg"), photo("2024:01:02 10:00:00")).unwrap();
        let mut conn = db::open_in_memory().unwrap();
        run(root, &mut conn);
        conn.execute(
            "UPDATE photos SET thumb_state = 1 WHERE rel_path = 'a.jpg'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE photos SET thumb_state = 2 WHERE rel_path = 'b.jpg'",
            [],
        )
        .unwrap();

        let progress = std::cell::Cell::new(0);
        let full = scan_with(
            root,
            &mut conn,
            Mode::Everything,
            &AtomicBool::new(false),
            &|p| progress.set(p.known),
        )
        .unwrap();
        assert_eq!((full.added, full.updated, full.unchanged), (0, 2, 0));
        assert_eq!(progress.get(), 2);
        let states: Vec<i64> = conn
            .prepare("SELECT thumb_state FROM photos ORDER BY rel_path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // Same content, same thumbnail; the failed one is tried again.
        assert_eq!(states, [1, 0]);
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
            phases.borrow_mut().push((p.phase, p.total))
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
