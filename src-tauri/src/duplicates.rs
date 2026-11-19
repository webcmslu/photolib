//! Photos stored more than once, and tidying them up.
//!
//! Each photo stored in several files is a group of copies. The app suggests
//! which copy to keep; the others can be moved into a `Duplicates to delete`
//! folder at the top of the library, under the same relative path, for the
//! user to check and delete in their file manager. Nothing is deleted here,
//! and because the relative path is kept, every copy can be put back.
//!
//! The fingerprint only reads the start and end of a file, so a copy is moved
//! only after a byte-for-byte comparison with the copy that is kept.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db;

/// The folder, at the top of the library, that extra copies are moved into.
/// The scan skips it.
pub const PARKED: &str = "Duplicates to delete";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub rel_path: String,
    pub size: u64,
    /// The file's modification time.
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub fingerprint: String,
    pub sort_date: i64,
    pub video: bool,
    pub copies: Vec<Copy>,
    /// The copy the app suggests keeping.
    pub keep: usize,
}

/// Every photo stored more than once, newest first.
pub fn groups(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint, rel_path, size, mtime_ms, sort_date, kind = 1 FROM photos
         WHERE thumb_state != 2 AND part = 0 AND fingerprint IN (
             SELECT fingerprint FROM photos WHERE thumb_state != 2 AND part = 0
             GROUP BY fingerprint HAVING COUNT(*) > 1)
         ORDER BY sort_date DESC, fingerprint, rel_path",
    )?;
    let mut rows = stmt.query([])?;
    let mut groups: Vec<Group> = Vec::new();
    while let Some(row) = rows.next()? {
        let fingerprint: String = row.get(0)?;
        let copy = Copy {
            rel_path: row.get(1)?,
            size: row.get::<_, i64>(2)? as u64,
            mtime_ms: row.get(3)?,
        };
        match groups.last_mut() {
            Some(group) if group.fingerprint == fingerprint => group.copies.push(copy),
            _ => groups.push(Group {
                fingerprint,
                sort_date: row.get(4)?,
                video: row.get(5)?,
                copies: vec![copy],
                keep: 0,
            }),
        }
    }
    for group in &mut groups {
        group.keep = suggested(&group.copies);
    }
    Ok(groups)
}

/// The copy to keep: one named by the camera rather than a long code (as
/// photo exports name them) or marked as a copy, then the oldest file, then
/// the shortest path.
fn suggested(copies: &[Copy]) -> usize {
    copies
        .iter()
        .enumerate()
        .min_by_key(|(_, copy)| {
            let stem = stem(&copy.rel_path);
            (
                looks_generated(stem),
                marked_as_copy(stem),
                copy.mtime_ms,
                copy.rel_path.len(),
                copy.rel_path.clone(),
            )
        })
        .map_or(0, |(index, _)| index)
}

fn stem(rel_path: &str) -> &str {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// A long code such as `a60c2a7e-8ef9-4969-a9c8-00fa6bfceef0`.
fn looks_generated(stem: &str) -> bool {
    stem.len() >= 16
        && stem
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_')
}

/// `IMG_1 (2)`, `IMG_1 copy`, `IMG_1 - Copie`.
fn marked_as_copy(stem: &str) -> bool {
    let lower = stem.to_lowercase();
    let numbered = lower.ends_with(')')
        && lower.rfind(" (").is_some_and(|open| {
            lower[open + 2..lower.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit())
        });
    numbered || lower.contains(" copy") || lower.contains("copie") || lower.contains("kopie")
}

/// For one photo: the copy kept, and the copies to move away.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub keep: String,
    pub extras: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub moved: usize,
    /// Copies left where they are, with why.
    pub skipped: Vec<(String, String)>,
    pub cancelled: bool,
}

/// Moves the extra copies into the `Duplicates to delete` folder, each after
/// checking it is the same file as the copy kept, and takes them out of the
/// index.
pub fn park(
    root: &Path,
    conn: &mut Connection,
    choices: &[Choice],
    cancel: &AtomicBool,
    on_progress: impl Fn(usize, usize),
) -> Summary {
    let total: usize = choices.iter().map(|choice| choice.extras.len()).sum();
    let mut summary = Summary::default();
    let mut done = 0;
    let mut last_progress = Instant::now();
    on_progress(0, total);
    'all: for choice in choices {
        for extra in &choice.extras {
            if cancel.load(Ordering::Relaxed) {
                summary.cancelled = true;
                break 'all;
            }
            match park_one(root, conn, &choice.keep, extra) {
                Ok(()) => summary.moved += 1,
                Err(why) => summary.skipped.push((extra.clone(), why)),
            }
            done += 1;
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                on_progress(done, total);
            }
        }
    }
    on_progress(total, total);
    summary
}

fn park_one(root: &Path, conn: &mut Connection, keep: &str, extra: &str) -> Result<(), String> {
    if extra == keep || inside_parked(extra) || inside_parked(keep) {
        return Err("not a copy to move".into());
    }
    let kept = library_path(root, keep);
    let source = library_path(root, extra);
    match same_content(&kept, &source) {
        Ok(true) => {}
        Ok(false) => return Err(format!("not exactly the same file as {keep}")),
        Err(e) => return Err(format!("couldn't compare it with {keep}: {e}")),
    }
    let target = library_path(&root.join(PARKED), extra);
    if target.exists() {
        return Err(format!("{PARKED}/{extra} already exists"));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("couldn't make a folder: {e}"))?;
    }
    fs::rename(&source, &target).map_err(|e| format!("couldn't move it: {e}"))?;
    // The index forgets it at once (a scan would too).
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    db::delete(&tx, extra).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn inside_parked(rel_path: &str) -> bool {
    rel_path == PARKED || rel_path.starts_with(&format!("{PARKED}/"))
}

/// `root` joined with a '/'-separated relative path.
fn library_path(root: &Path, rel_path: &str) -> PathBuf {
    rel_path
        .split('/')
        .fold(root.to_owned(), |path, part| path.join(part))
}

/// True if the two files hold exactly the same bytes.
fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let mut buffer_a = vec![0u8; 1 << 20];
    let mut buffer_b = vec![0u8; 1 << 20];
    loop {
        let read = read_full(&mut a, &mut buffer_a)?;
        if read_full(&mut b, &mut buffer_b[..read.max(1)])? != read
            || buffer_a[..read] != buffer_b[..read]
        {
            return Ok(false);
        }
        if read < buffer_a.len() {
            return Ok(true);
        }
    }
}

/// Reads until `buffer` is full or the file ends; returns how much was read.
fn read_full(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// The files waiting in the `Duplicates to delete` folder, as paths relative
/// to the library (where they came from). System files are left out.
fn parked_files(root: &Path) -> Vec<(PathBuf, String)> {
    let parked = root.join(PARKED);
    WalkDir::new(&parked)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .filter_map(|entry| {
            let rel: Vec<String> = entry
                .path()
                .strip_prefix(&parked)
                .ok()?
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            Some((entry.path().to_owned(), rel.join("/")))
        })
        .collect()
}

/// How many copies wait in the `Duplicates to delete` folder.
pub fn parked_count(root: &Path) -> usize {
    parked_files(root).len()
}

/// Moves every copy in the `Duplicates to delete` folder back where it came
/// from, unless a file has taken its place since. Then removes the emptied
/// folders.
pub fn put_back(root: &Path) -> Summary {
    let mut summary = Summary::default();
    for (path, rel_path) in parked_files(root) {
        let target = library_path(root, &rel_path);
        if target.exists() {
            summary
                .skipped
                .push((rel_path, "another file is in its place now".into()));
            continue;
        }
        let moved = target
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::rename(&path, &target));
        match moved {
            Ok(()) => summary.moved += 1,
            Err(e) => summary
                .skipped
                .push((rel_path, format!("couldn't move it back: {e}"))),
        }
    }
    remove_empty_folders(&root.join(PARKED));
    summary
}

/// Removes the folders under (and including) `dir` that are empty, or hold
/// only a `.DS_Store`.
fn remove_empty_folders(dir: &Path) {
    let folders: Vec<PathBuf> = WalkDir::new(dir)
        .contents_first(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .map(|entry| entry.into_path())
        .collect();
    for folder in folders {
        let _ = fs::remove_file(folder.join(".DS_Store"));
        let _ = fs::remove_dir(&folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PhotoRow;

    fn copy(rel_path: &str, mtime_ms: i64) -> Copy {
        Copy {
            rel_path: rel_path.into(),
            size: 1,
            mtime_ms,
        }
    }

    #[test]
    fn suggests_the_camera_named_oldest_copy() {
        // A camera name beats a long code, even a newer file.
        let copies = [
            copy("2018/a60c2a7e-8ef9-4969-a9c8-00fa6bfceef0.JPG", 1),
            copy("2018/YKOD0972.JPG", 5),
        ];
        assert_eq!(suggested(&copies), 1);
        // A copy marked as such loses to the plain name.
        let copies = [copy("a/IMG_1 (2).jpg", 1), copy("a/IMG_1.jpg", 5)];
        assert_eq!(suggested(&copies), 1);
        let copies = [copy("a/IMG_1 copy.jpg", 1), copy("b/IMG_1.jpg", 5)];
        assert_eq!(suggested(&copies), 1);
        // Otherwise the oldest file, then the shortest path.
        let copies = [copy("backup/2018/IMG_1.jpg", 2), copy("2018/IMG_1.jpg", 3)];
        assert_eq!(suggested(&copies), 0);
        let copies = [copy("backup/2018/IMG_1.jpg", 2), copy("2018/IMG_1.jpg", 2)];
        assert_eq!(suggested(&copies), 1);
        assert!(!looks_generated("YKOD0972"));
        assert!(!marked_as_copy("IMG_2024-07-14"));
    }

    fn photo(rel_path: &str, fingerprint: &str) -> PhotoRow {
        PhotoRow {
            rel_path: rel_path.into(),
            size: 4,
            mtime_ms: 1,
            fingerprint: fingerprint.into(),
            sort_date: 2,
            date_from_exif: true,
            orientation: None,
            width: None,
            height: None,
            camera_make: None,
            camera_model: None,
            latitude: None,
            longitude: None,
            is_video: false,
            duration_ms: None,
            live_candidate: false,
        }
    }

    #[test]
    fn moves_extra_copies_aside_only_when_identical_and_puts_them_back() {
        let library = tempfile::tempdir().unwrap();
        let root = library.path();
        fs::create_dir_all(root.join("2018/backup")).unwrap();
        fs::write(root.join("2018/IMG_1.jpg"), b"same").unwrap();
        fs::write(root.join("2018/backup/IMG_1.jpg"), b"same").unwrap();
        // Same fingerprint in the index, different bytes on disk.
        fs::write(root.join("2018/IMG_2.jpg"), b"one!").unwrap();
        fs::write(root.join("2018/backup/IMG_2.jpg"), b"two!").unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for (rel_path, fingerprint) in [
            ("2018/IMG_1.jpg", "f1"),
            ("2018/backup/IMG_1.jpg", "f1"),
            ("2018/IMG_2.jpg", "f2"),
            ("2018/backup/IMG_2.jpg", "f2"),
            ("2018/IMG_3.jpg", "f3"),
        ] {
            db::upsert(&tx, &photo(rel_path, fingerprint)).unwrap();
        }
        tx.commit().unwrap();

        let found = groups(&conn).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|group| group.copies[group.keep].rel_path.len() < 20));

        let choices: Vec<Choice> = found
            .iter()
            .map(|group| Choice {
                keep: group.copies[group.keep].rel_path.clone(),
                extras: group
                    .copies
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != group.keep)
                    .map(|(_, copy)| copy.rel_path.clone())
                    .collect(),
            })
            .collect();
        let never = AtomicBool::new(false);
        let summary = park(root, &mut conn, &choices, &never, |_, _| {});
        assert_eq!(summary.moved, 1);
        assert_eq!(summary.skipped.len(), 1);
        assert_eq!(summary.skipped[0].0, "2018/backup/IMG_2.jpg");

        assert!(!root.join("2018/backup/IMG_1.jpg").exists());
        assert_eq!(
            fs::read(root.join(PARKED).join("2018/backup/IMG_1.jpg")).unwrap(),
            b"same"
        );
        assert!(root.join("2018/backup/IMG_2.jpg").exists());
        assert_eq!(parked_count(root), 1);
        // The index forgot the moved copy: only the mismatched pair is left.
        assert_eq!(groups(&conn).unwrap().len(), 1);

        let back = put_back(root);
        assert_eq!((back.moved, back.skipped.len()), (1, 0));
        assert_eq!(
            fs::read(root.join("2018/backup/IMG_1.jpg")).unwrap(),
            b"same"
        );
        assert!(!root.join(PARKED).exists());
    }

    #[test]
    fn compares_whole_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![7u8; 3 << 20];
        let mut other = big.clone();
        other[(3 << 20) / 2] = 8;
        fs::write(dir.path().join("a"), &big).unwrap();
        fs::write(dir.path().join("b"), &big).unwrap();
        fs::write(dir.path().join("c"), &other).unwrap();
        assert!(same_content(&dir.path().join("a"), &dir.path().join("b")).unwrap());
        assert!(!same_content(&dir.path().join("a"), &dir.path().join("c")).unwrap());
    }
}
