//! The local index: one SQLite database per library, in the OS app-data dir.
//!
//! It is a cache of what is in the library folder and can always be rebuilt by
//! rescanning. It never lives on the library volume itself.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

const SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE photos (
    id            INTEGER PRIMARY KEY,
    -- Path relative to the library root, always with '/' separators.
    rel_path      TEXT NOT NULL UNIQUE,
    -- The folder part of rel_path ('' for files in the library root).
    folder        TEXT NOT NULL,
    size          INTEGER NOT NULL,
    mtime_ms      INTEGER NOT NULL,
    fingerprint   TEXT NOT NULL,
    -- Wall-clock date used for the timeline (see PhotoMeta::taken_at for the
    -- encoding): the EXIF date when there is one, else the file's mtime.
    sort_date     INTEGER NOT NULL,
    -- 'exif' or 'file'
    date_source   TEXT NOT NULL,
    orientation   INTEGER,
    -- Upright size, as displayed (EXIF orientation already applied).
    width         INTEGER,
    height        INTEGER,
    camera_make   TEXT,
    camera_model  TEXT,
    latitude      REAL,
    longitude     REAL,
    -- 0 = thumbnails not made yet, 1 = ready, 2 = not a decodable image
    thumb_state   INTEGER NOT NULL DEFAULT 0
);
-- The order the timeline and the folder view list photos in. Partial, to match
-- their WHERE clause, so skipping to a far offset reads only the index.
CREATE INDEX photos_timeline ON photos (sort_date DESC, rel_path) WHERE thumb_state != 2;
CREATE INDEX photos_folder ON photos (folder, sort_date DESC, rel_path) WHERE thumb_state != 2;
CREATE INDEX photos_fingerprint ON photos (fingerprint);
";

/// One indexed file.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoRow {
    pub rel_path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub fingerprint: String,
    pub sort_date: i64,
    pub date_from_exif: bool,
    pub orientation: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// Opens (creating if needed) the index database at `path`.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        // If this fails, `Connection::open` reports the real problem.
        let _ = fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    migrate(&conn)?;
    Ok(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    // The index is only a cache, so any other version (older or from a newer
    // build) is simply rebuilt from scratch on the next scan.
    conn.execute_batch(&format!(
        "BEGIN;
         DROP TABLE IF EXISTS photos;
         {SCHEMA}
         PRAGMA user_version = {SCHEMA_VERSION};
         COMMIT;"
    ))
}

/// Returns `rel_path -> (size, mtime_ms)` for every indexed file, which is what
/// an incremental rescan compares against.
pub fn load_known(conn: &Connection) -> rusqlite::Result<HashMap<String, (u64, i64)>> {
    let mut stmt = conn.prepare("SELECT rel_path, size, mtime_ms FROM photos")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)? as u64, row.get(2)?),
        ))
    })?;
    rows.collect()
}

/// Inserts a row, or replaces the row that has the same `rel_path`.
pub fn upsert(tx: &Transaction<'_>, row: &PhotoRow) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO photos (folder, rel_path, size, mtime_ms, fingerprint, sort_date, date_source,
                             orientation, width, height, camera_make, camera_model,
                             latitude, longitude)
         VALUES (?14, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (rel_path) DO UPDATE SET
             size = excluded.size, mtime_ms = excluded.mtime_ms,
             fingerprint = excluded.fingerprint, sort_date = excluded.sort_date,
             date_source = excluded.date_source, orientation = excluded.orientation,
             width = COALESCE(excluded.width, photos.width),
             height = COALESCE(excluded.height, photos.height),
             -- New content needs new thumbnails. (SET expressions all see the old row.)
             thumb_state = CASE WHEN photos.fingerprint = excluded.fingerprint
                                THEN photos.thumb_state ELSE 0 END,
             camera_make = excluded.camera_make, camera_model = excluded.camera_model,
             latitude = excluded.latitude, longitude = excluded.longitude",
    )?
    .execute(params![
        row.rel_path,
        row.size as i64,
        row.mtime_ms,
        row.fingerprint,
        row.sort_date,
        if row.date_from_exif { "exif" } else { "file" },
        row.orientation,
        row.width,
        row.height,
        row.camera_make,
        row.camera_model,
        row.latitude,
        row.longitude,
        folder_of(&row.rel_path),
    ])?;
    Ok(())
}

pub fn delete(tx: &Transaction<'_>, rel_path: &str) -> rusqlite::Result<()> {
    tx.prepare_cached("DELETE FROM photos WHERE rel_path = ?1")?
        .execute([rel_path])?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbState {
    Ready = 1,
    Failed = 2,
}

/// A photo that still needs thumbnails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingThumb {
    pub rel_path: String,
    pub fingerprint: String,
    pub orientation: Option<u8>,
}

pub fn pending_thumbs(conn: &Connection) -> rusqlite::Result<Vec<PendingThumb>> {
    let mut stmt = conn.prepare(
        "SELECT rel_path, fingerprint, orientation FROM photos
         WHERE thumb_state = 0 ORDER BY sort_date DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PendingThumb {
            rel_path: row.get(0)?,
            fingerprint: row.get(1)?,
            orientation: row.get(2)?,
        })
    })?;
    rows.collect()
}

/// Records the outcome of thumbnail generation. `size` is the upright
/// width and height, when the image was decoded.
pub fn set_thumb_state(
    tx: &Transaction<'_>,
    rel_path: &str,
    state: ThumbState,
    size: Option<(u32, u32)>,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "UPDATE photos SET thumb_state = ?2,
                           width = COALESCE(?3, width), height = COALESCE(?4, height)
         WHERE rel_path = ?1",
    )?
    .execute(params![
        rel_path,
        state as i64,
        size.map(|(width, _)| width),
        size.map(|(_, height)| height),
    ])?;
    Ok(())
}

/// Finds where a photo with this fingerprint is, to make its thumbnail on demand.
pub fn find_by_fingerprint(
    conn: &Connection,
    fingerprint: &str,
) -> rusqlite::Result<Option<(String, Option<u8>)>> {
    conn.query_row(
        "SELECT rel_path, orientation FROM photos WHERE fingerprint = ?1 LIMIT 1",
        [fingerprint],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// The folder part of a relative path: `"2024/summer"` for
/// `"2024/summer/a.jpg"`, and `""` for a file in the library root.
fn folder_of(rel_path: &str) -> &str {
    rel_path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Undecodable files are indexed but never listed.
const LISTED: &str = "thumb_state != 2";

/// `WHERE` clause for listing either the whole library or one folder (`?1`).
fn listing_filter(folder: Option<&str>) -> String {
    match folder {
        Some(_) => format!("WHERE {LISTED} AND folder = ?1"),
        None => format!("WHERE {LISTED} AND ?1 IS NULL"),
    }
}

/// One day of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayGroup {
    /// Days since 1970-01-01, in the photos' wall-clock time.
    pub day: i64,
    pub count: u32,
}

/// How many photos each day has, newest day first. Together with
/// [`photos_page`] (same order) this is all the grid needs to lay itself out
/// without loading every photo.
pub fn timeline(conn: &Connection, folder: Option<&str>) -> rusqlite::Result<Vec<DayGroup>> {
    // Integer division truncates toward zero, so shift pre-1970 dates down a
    // day first to get a floor.
    let mut stmt = conn.prepare(&format!(
        "SELECT (sort_date - 86400000 * (sort_date < 0 AND sort_date % 86400000 != 0))
                    / 86400000 AS day,
                COUNT(*)
         FROM photos {} GROUP BY day ORDER BY day DESC",
        listing_filter(folder)
    ))?;
    let rows = stmt.query_map([folder], |row| {
        Ok(DayGroup {
            day: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// A photo as the grid lists it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    pub fingerprint: String,
    pub rel_path: String,
    pub sort_date: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// A slice of the listing, newest first (ties broken by path).
pub fn photos_page(
    conn: &Connection,
    folder: Option<&str>,
    offset: u32,
    limit: u32,
) -> rusqlite::Result<Vec<PhotoItem>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT fingerprint, rel_path, sort_date, width, height
         FROM photos {} ORDER BY sort_date DESC, rel_path LIMIT ?3 OFFSET ?2",
        listing_filter(folder)
    ))?;
    let rows = stmt.query_map(params![folder, offset, limit], |row| {
        Ok(PhotoItem {
            fingerprint: row.get(0)?,
            rel_path: row.get(1)?,
            sort_date: row.get(2)?,
            width: row.get(3)?,
            height: row.get(4)?,
        })
    })?;
    rows.collect()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    /// Path relative to the library root; `""` is the root itself.
    pub folder: String,
    /// Photos directly in this folder (not in its subfolders).
    pub count: u32,
}

/// Every folder that directly contains photos, sorted by path.
pub fn folders(conn: &Connection) -> rusqlite::Result<Vec<FolderEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT folder, COUNT(*) FROM photos WHERE {LISTED} GROUP BY folder ORDER BY folder"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok(FolderEntry {
            folder: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// Everything the info panel shows about one photo.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetails {
    pub rel_path: String,
    pub fingerprint: String,
    pub size: u64,
    pub sort_date: i64,
    /// False when the photo has no EXIF date and the file date is shown instead.
    pub date_from_exif: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

pub fn photo_details(conn: &Connection, rel_path: &str) -> rusqlite::Result<Option<PhotoDetails>> {
    conn.query_row(
        "SELECT rel_path, fingerprint, size, sort_date, date_source, width, height,
                camera_make, camera_model, latitude, longitude
         FROM photos WHERE rel_path = ?1",
        [rel_path],
        |row| {
            Ok(PhotoDetails {
                rel_path: row.get(0)?,
                fingerprint: row.get(1)?,
                size: row.get::<_, i64>(2)? as u64,
                sort_date: row.get(3)?,
                date_from_exif: row.get::<_, String>(4)? == "exif",
                width: row.get(5)?,
                height: row.get(6)?,
                camera_make: row.get(7)?,
                camera_model: row.get(8)?,
                latitude: row.get(9)?,
                longitude: row.get(10)?,
            })
        },
    )
    .optional()
}

pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM photos", [], |row| {
        row.get::<_, i64>(0).map(|n| n as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rel_path: &str, size: u64) -> PhotoRow {
        PhotoRow {
            rel_path: rel_path.into(),
            size,
            mtime_ms: 1,
            fingerprint: "f".into(),
            sort_date: 2,
            date_from_exif: true,
            orientation: Some(6),
            width: None,
            height: None,
            camera_make: Some("Canon".into()),
            camera_model: None,
            latitude: Some(49.6),
            longitude: Some(6.1),
        }
    }

    #[test]
    fn upsert_replaces_by_path() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("a.jpg", 10)).unwrap();
        upsert(&tx, &row("b.jpg", 20)).unwrap();
        upsert(&tx, &row("a.jpg", 11)).unwrap();
        tx.commit().unwrap();

        assert_eq!(count(&conn).unwrap(), 2);
        assert_eq!(load_known(&conn).unwrap()["a.jpg"], (11, 1));

        let tx = conn.transaction().unwrap();
        delete(&tx, "a.jpg").unwrap();
        tx.commit().unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
    }

    #[test]
    fn thumb_state_resets_only_when_content_changes() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("a.jpg", 10)).unwrap();
        assert_eq!(pending_thumbs(&tx).unwrap().len(), 1);

        set_thumb_state(&tx, "a.jpg", ThumbState::Ready, Some((400, 300))).unwrap();
        assert!(pending_thumbs(&tx).unwrap().is_empty());

        // Same content (e.g. only the mtime changed): thumbnails and size stay.
        upsert(&tx, &row("a.jpg", 10)).unwrap();
        assert!(pending_thumbs(&tx).unwrap().is_empty());
        let width: u32 = tx
            .query_row("SELECT width FROM photos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(width, 400);

        let mut changed = row("a.jpg", 11);
        changed.fingerprint = "other".into();
        upsert(&tx, &changed).unwrap();
        assert_eq!(pending_thumbs(&tx).unwrap().len(), 1);

        assert_eq!(
            find_by_fingerprint(&tx, "other").unwrap(),
            Some(("a.jpg".into(), Some(6)))
        );
        assert_eq!(find_by_fingerprint(&tx, "nope").unwrap(), None);
    }

    const DAY: i64 = 86_400_000;

    fn dated(rel_path: &str, sort_date: i64) -> PhotoRow {
        PhotoRow {
            sort_date,
            ..row(rel_path, 1)
        }
    }

    #[test]
    fn timeline_and_pages_agree() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &dated("b.jpg", 10 * DAY + 5)).unwrap();
        upsert(&tx, &dated("a.jpg", 10 * DAY + 5)).unwrap(); // same instant: path breaks the tie
        upsert(&tx, &dated("trip/c.jpg", 10 * DAY + 9)).unwrap();
        upsert(&tx, &dated("trip/d.jpg", 3 * DAY)).unwrap();
        upsert(&tx, &dated("trip/deep/e.jpg", -1)).unwrap(); // 1969-12-31
        upsert(&tx, &dated("broken.jpg", 10 * DAY)).unwrap();
        set_thumb_state(&tx, "broken.jpg", ThumbState::Failed, None).unwrap();
        tx.commit().unwrap();

        let days = |folder| -> Vec<(i64, u32)> {
            timeline(&conn, folder)
                .unwrap()
                .into_iter()
                .map(|g| (g.day, g.count))
                .collect()
        };
        let paths = |folder, offset, limit| -> Vec<String> {
            photos_page(&conn, folder, offset, limit)
                .unwrap()
                .into_iter()
                .map(|p| p.rel_path)
                .collect()
        };

        assert_eq!(days(None), [(10, 3), (3, 1), (-1, 1)]);
        assert_eq!(
            paths(None, 0, 10),
            [
                "trip/c.jpg",
                "a.jpg",
                "b.jpg",
                "trip/d.jpg",
                "trip/deep/e.jpg"
            ]
        );
        assert_eq!(paths(None, 1, 2), ["a.jpg", "b.jpg"]);
        assert!(paths(None, 5, 10).is_empty());

        // A folder lists only the photos directly inside it.
        assert_eq!(days(Some("trip")), [(10, 1), (3, 1)]);
        assert_eq!(paths(Some("trip"), 0, 10), ["trip/c.jpg", "trip/d.jpg"]);
        assert_eq!(paths(Some(""), 0, 10), ["a.jpg", "b.jpg"]);

        let listed: Vec<(String, u32)> = folders(&conn)
            .unwrap()
            .into_iter()
            .map(|f| (f.folder, f.count))
            .collect();
        assert_eq!(
            listed,
            [("".into(), 2), ("trip".into(), 2), ("trip/deep".into(), 1)]
        );
    }

    #[test]
    fn listing_queries_use_their_indexes() {
        let conn = open_in_memory().unwrap();
        let plan = |sql: &str, folder: Option<&str>| -> String {
            conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params![folder, 0, 10], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ")
        };
        let page = |folder| {
            format!(
                "SELECT fingerprint FROM photos {} ORDER BY sort_date DESC, rel_path LIMIT ?3 OFFSET ?2",
                listing_filter(folder)
            )
        };

        let all = plan(&page(None), None);
        assert!(
            all.contains("photos_timeline") && !all.contains("TEMP B-TREE"),
            "{all}"
        );
        let one = plan(&page(Some("trip")), Some("trip"));
        assert!(
            one.contains("photos_folder") && !one.contains("TEMP B-TREE"),
            "{one}"
        );
    }

    #[test]
    fn details_of_one_photo() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("trip/a.jpg", 1234)).unwrap();
        tx.commit().unwrap();

        let details = photo_details(&conn, "trip/a.jpg").unwrap().unwrap();
        assert_eq!(details.size, 1234);
        assert!(details.date_from_exif);
        assert_eq!(details.camera_make.as_deref(), Some("Canon"));
        assert_eq!(photo_details(&conn, "nope.jpg").unwrap(), None);
    }

    #[test]
    fn reopening_keeps_data_and_unknown_versions_are_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib/index.db");

        let mut conn = open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("a.jpg", 10)).unwrap();
        tx.commit().unwrap();
        drop(conn);

        let conn = open(&path).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);

        assert_eq!(count(&open(&path).unwrap()).unwrap(), 0);
    }
}
