//! The local index: one SQLite database per library, in the OS app-data dir.
//!
//! It is a cache of what is in the library folder and can always be rebuilt by
//! rescanning. It never lives on the library volume itself.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = "
CREATE TABLE photos (
    id            INTEGER PRIMARY KEY,
    -- Path relative to the library root, always with '/' separators.
    rel_path      TEXT NOT NULL UNIQUE,
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
CREATE INDEX photos_sort_date ON photos (sort_date);
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
        "INSERT INTO photos (rel_path, size, mtime_ms, fingerprint, sort_date, date_source,
                             orientation, width, height, camera_make, camera_model,
                             latitude, longitude)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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

/// A photo as the UI lists it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    pub fingerprint: String,
    pub rel_path: String,
    pub sort_date: i64,
}

/// The newest photos, newest first.
pub fn newest_photos(conn: &Connection, limit: u32) -> rusqlite::Result<Vec<PhotoItem>> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint, rel_path, sort_date FROM photos
         WHERE thumb_state != 2 ORDER BY sort_date DESC, rel_path LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(PhotoItem {
            fingerprint: row.get(0)?,
            rel_path: row.get(1)?,
            sort_date: row.get(2)?,
        })
    })?;
    rows.collect()
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
