//! The local index: one SQLite database per library, in the OS app-data dir.
//!
//! It is a cache of what is in the library folder and can always be rebuilt by
//! rescanning. It never lives on the library volume itself.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::org::{Event, Op};

const SCHEMA_VERSION: i64 = 11;

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
    -- 0 = thumbnails not made yet, 1 = ready, 2 = not a decodable image,
    -- 3 = a video shown without a thumbnail (no tool here can make one),
    -- 4 = a video shown without a thumbnail (the tool failed on it)
    thumb_state   INTEGER NOT NULL DEFAULT 0,
    -- 0 = photo, 1 = video
    kind          INTEGER NOT NULL DEFAULT 0,
    duration_ms   INTEGER,
    -- rel_path without its extension, lowercased: what the two files of a
    -- Live Photo (IMG_1.HEIC + IMG_1.MOV) have in common.
    stem          TEXT NOT NULL,
    -- 1 for a video that may be the moving half of a Live Photo
    live_candidate INTEGER NOT NULL DEFAULT 0,
    -- On the photo of a Live Photo: the fingerprint of its video.
    live_video    TEXT,
    -- 1 on a file that is shown through another one, never listed by itself:
    -- the video of a Live Photo, or the RAW file of a JPEG + RAW pair.
    part          INTEGER NOT NULL DEFAULT 0,
    -- The nearest town to latitude/longitude, such as 'Paris, France'. NULL
    -- until it has been looked up; '' when no town is near. See places.rs.
    place         TEXT,
    -- The same in lowercase and without accents, for searching.
    place_search  TEXT,
    -- On the photo of a JPEG + RAW pair: the fingerprint of its RAW file.
    raw_file      TEXT,
    -- How the photo was taken, read after the scan (see fill_shooting):
    -- 0 until then, 1 once read (even if the photo doesn't say).
    shot_read     INTEGER NOT NULL DEFAULT 0,
    lens          TEXT,
    -- In mm, and its 35 mm equivalent.
    focal_length  REAL,
    focal_35mm    INTEGER,
    -- The f-number (2.8), the exposure time in seconds, the ISO.
    aperture      REAL,
    exposure      REAL,
    iso           INTEGER,
    -- What kind of shot it is, read with the lens: the bits of
    -- metadata::PORTRAIT, SLO_MO, TIME_LAPSE and SCREENSHOT.
    media         INTEGER NOT NULL DEFAULT 0,
    -- The burst the photo was shot in (Apple's burst UUID).
    burst         TEXT
);
-- The order the timeline and the folder view list photos in. Partial, to match
-- their WHERE clause, so skipping to a far offset reads only the index.
-- (fingerprint, thumb_state and part are included only so that the filter
-- can be evaluated from the index alone.)
CREATE INDEX photos_timeline
    ON photos (sort_date DESC, rel_path, fingerprint, thumb_state, part)
    WHERE thumb_state != 2 AND part = 0;
CREATE INDEX photos_folder
    ON photos (folder, sort_date DESC, rel_path, fingerprint, thumb_state, part)
    WHERE thumb_state != 2 AND part = 0;
CREATE INDEX photos_stem ON photos (stem);

-- The merged organization (favorites, hidden, albums): one row per value that
-- the device logs set, holding the winning event. See org.rs.
--   kind            key        photo      value
--   'fav'           photo fp   ''         '1' / '0'
--   'hide'          photo fp   ''         '1' / '0'
--   'album_name'    album id   ''         the name
--   'album_deleted' album id   ''         '1' / '0'
--   'member'        album id   photo fp   '1' / '0'
--   'edit'          photo fp   ''         the edit as JSON, or '' for none
--   'face_name'     photo fp   'x,y'      the name of the person whose face
--                                         is at that point, or '' for none
CREATE TABLE org_state (
    kind    TEXT NOT NULL,
    key     TEXT NOT NULL,
    photo   TEXT NOT NULL,
    value   TEXT NOT NULL,
    -- The event that set the value. The greatest (ts, device) wins.
    ts      INTEGER NOT NULL,
    device  TEXT NOT NULL,
    PRIMARY KEY (kind, key, photo)
) WITHOUT ROWID;

-- How far into each device log the merge has read.
CREATE TABLE log_offsets (
    source  TEXT PRIMARY KEY,
    offset  INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX photos_fingerprint ON photos (fingerprint);
-- A small visual fingerprint of each photo, to find similar shots (see
-- similar.rs). Keyed by content, like the thumbnails it is made from.
CREATE TABLE visual (
    fingerprint TEXT PRIMARY KEY,
    hash        INTEGER NOT NULL
) WITHOUT ROWID;
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
    pub is_video: bool,
    pub duration_ms: Option<u64>,
    pub live_candidate: bool,
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
    // Writes are kept short (see org::sync); this is a margin for a slow disk.
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    migrate(&conn)?;
    attach_faces(&conn, &path.with_file_name("faces.db").to_string_lossy())?;
    Ok(conn)
}

/// The faces found in the photos live in a second database file next to the
/// index, reachable as `faces.…` from the same connection. It is kept apart
/// because finding faces takes hours for a large library: rebuilding the
/// index must not throw that work away. Rows are keyed by photo fingerprint.
const FACES_SCHEMA: &str = "
-- The photos that have been looked at (most have no face at all).
CREATE TABLE IF NOT EXISTS faces.scanned (
    fingerprint TEXT PRIMARY KEY
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS faces.face (
    id          INTEGER PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    -- The box around the face, as fractions of the upright photo.
    left REAL NOT NULL, top REAL NOT NULL, width REAL NOT NULL, height REAL NOT NULL,
    score       REAL NOT NULL,
    -- 128 little-endian 32-bit floats; see faces.rs.
    embedding   BLOB NOT NULL,
    -- The group of look-alike faces this one was put in; NULL until grouped,
    -- and below zero for a face that was taken out of its group by hand: it
    -- belongs to nobody and is not grouped again.
    cluster     INTEGER
);
CREATE INDEX IF NOT EXISTS faces.face_fingerprint ON face (fingerprint);
CREATE INDEX IF NOT EXISTS faces.face_cluster ON face (cluster);
-- Corrections made on this computer (groups are local, so these are too).
-- People hidden from the People screen, e.g. strangers.
CREATE TABLE IF NOT EXISTS faces.hidden_group (
    cluster INTEGER PRIMARY KEY
);
-- A group the user said is not the named person, so it is neither suggested
-- for them again nor merged into them.
CREATE TABLE IF NOT EXISTS faces.not_person (
    cluster INTEGER NOT NULL,
    name    TEXT NOT NULL,
    PRIMARY KEY (cluster, name)
) WITHOUT ROWID;
-- Two groups the user split apart, which must not be merged again (a < b).
CREATE TABLE IF NOT EXISTS faces.kept_apart (
    a INTEGER NOT NULL,
    b INTEGER NOT NULL,
    PRIMARY KEY (a, b)
) WITHOUT ROWID;
";

fn attach_faces(conn: &Connection, file: &str) -> rusqlite::Result<()> {
    conn.execute("ATTACH DATABASE ?1 AS faces", [file])?;
    if file != ":memory:" {
        // The statement answers with the new mode, hence a query.
        conn.query_row("PRAGMA faces.journal_mode = WAL", [], |_| Ok(()))?;
    }
    let new = !conn
        .prepare("SELECT 1 FROM faces.sqlite_master WHERE name = 'face'")?
        .exists([])?;
    conn.execute_batch(FACES_SCHEMA)?;
    if new {
        // Nothing to group again.
        set_grouping_version(conn, crate::people::GROUPING_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    attach_faces(&conn, ":memory:")?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    // Upgrades that keep what is indexed, one version after the other: indexing
    // a whole library again takes long over a network.
    if version == 5 {
        // Two columns that are filled in after the scan.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE photos ADD COLUMN place TEXT;
             ALTER TABLE photos ADD COLUMN place_search TEXT;
             PRAGMA user_version = 6;
             COMMIT;",
        )?;
        version = 6;
    }
    if version == 6 {
        // `live_part` now also covers RAW files paired with a JPEG; the
        // pairing is worked out again after the next scan.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE photos RENAME COLUMN live_part TO part;
             ALTER TABLE photos ADD COLUMN raw_file TEXT;
             PRAGMA user_version = 7;
             COMMIT;",
        )?;
        version = 7;
    }
    if version == 7 {
        // Ratings and labels are new kinds of events. A build that did not
        // know them skipped them and moved on: every log is read again from
        // the start (applying an event twice changes nothing).
        conn.execute_batch(
            "BEGIN;
             DELETE FROM log_offsets;
             PRAGMA user_version = 8;
             COMMIT;",
        )?;
        version = 8;
    }
    if version == 8 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS visual (
                 fingerprint TEXT PRIMARY KEY,
                 hash        INTEGER NOT NULL
             ) WITHOUT ROWID;
             PRAGMA user_version = 9;
             COMMIT;",
        )?;
        version = 9;
    }
    if version == 9 {
        // Lens and exposure, for the camera filter: read for every photo
        // after the next scan, without indexing the library again.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE photos ADD COLUMN shot_read INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE photos ADD COLUMN lens TEXT;
             ALTER TABLE photos ADD COLUMN focal_length REAL;
             ALTER TABLE photos ADD COLUMN focal_35mm INTEGER;
             ALTER TABLE photos ADD COLUMN aperture REAL;
             ALTER TABLE photos ADD COLUMN exposure REAL;
             ALTER TABLE photos ADD COLUMN iso INTEGER;
             PRAGMA user_version = 10;
             COMMIT;",
        )?;
        version = 10;
    }
    if version == 10 {
        // Media types (Portrait, slow motion, bursts...): read again for the
        // files that can be one, which are only videos and phone pictures.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE photos ADD COLUMN media INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE photos ADD COLUMN burst TEXT;
             UPDATE photos SET shot_read = 0
              WHERE kind = 1 OR camera_make LIKE 'apple%'
                 OR lower(rel_path) LIKE '%.png' OR lower(rel_path) LIKE '%.hei_';
             PRAGMA user_version = 11;
             COMMIT;",
        )?;
        version = 11;
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    // The index is only a cache, so any other version (older or from a newer
    // build) is simply rebuilt from scratch on the next scan.
    conn.execute_batch(&format!(
        "BEGIN;
         DROP TABLE IF EXISTS photos;
         DROP TABLE IF EXISTS org_state;
         DROP TABLE IF EXISTS log_offsets;
         DROP TABLE IF EXISTS visual;
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
                             latitude, longitude, kind, duration_ms, live_candidate, stem)
         VALUES (?14, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?15, ?16, ?17, ?18)
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
             latitude = excluded.latitude, longitude = excluded.longitude,
             kind = excluded.kind, duration_ms = excluded.duration_ms,
             live_candidate = excluded.live_candidate,
             -- A new position needs its place looked up again.
             place = CASE WHEN photos.latitude IS excluded.latitude
                           AND photos.longitude IS excluded.longitude
                          THEN photos.place END,
             place_search = CASE WHEN photos.latitude IS excluded.latitude
                                  AND photos.longitude IS excluded.longitude
                                 THEN photos.place_search END,
             -- New content: how it was taken is read again.
             shot_read = CASE WHEN photos.fingerprint = excluded.fingerprint
                              THEN photos.shot_read ELSE 0 END",
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
        row.is_video,
        row.duration_ms.map(|ms| ms as i64),
        row.live_candidate,
        stem_of(&row.rel_path),
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
    /// A video listed without a thumbnail, because this computer has no
    /// tool to make one. Tried again once it has.
    Missing = 3,
    /// A video listed without a thumbnail, because the tool could not make
    /// one. Not tried again.
    NoFrame = 4,
}

/// A photo that still needs thumbnails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingThumb {
    pub rel_path: String,
    pub fingerprint: String,
    pub orientation: Option<u8>,
    pub is_video: bool,
}

/// The Live Photo videos are left out: they are never shown by themselves.
pub fn pending_thumbs(conn: &Connection) -> rusqlite::Result<Vec<PendingThumb>> {
    let mut stmt = conn.prepare(
        "SELECT rel_path, fingerprint, orientation, kind FROM photos
         WHERE thumb_state = 0 AND part = 0 ORDER BY sort_date DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PendingThumb {
            rel_path: row.get(0)?,
            fingerprint: row.get(1)?,
            orientation: row.get(2)?,
            is_video: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// Makes the next thumbnail pass try again the videos that have none, for
/// when a tool that can make them has been installed since.
pub fn retry_missing_thumbs(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE photos SET thumb_state = 0 WHERE thumb_state = 3",
        [],
    )
}

/// Makes the next thumbnail pass try again every photo it gave up on, for a
/// full rescan.
pub fn retry_failed_thumbs(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE photos SET thumb_state = 0 WHERE thumb_state IN (2, 3, 4)",
        [],
    )
}

/// Records a thumbnail made on demand (the grid asked for it before the
/// background pass got there), so that pass doesn't open the file again.
/// Only photos still waiting are touched.
pub fn mark_thumb_ready(
    conn: &Connection,
    fingerprint: &str,
    size: Option<(u32, u32)>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE photos SET thumb_state = 1,
                           width = COALESCE(?2, width), height = COALESCE(?3, height)
         WHERE fingerprint = ?1 AND thumb_state = 0",
        params![
            fingerprint,
            size.map(|(width, _)| width),
            size.map(|(_, height)| height)
        ],
    )
}

/// Moves every face of group `from` into group `into`, with the corrections
/// made to `from`.
pub fn merge_face_group(tx: &Transaction<'_>, from: i64, into: i64) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE faces.face SET cluster = ?2 WHERE cluster = ?1",
        [from, into],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO faces.not_person (cluster, name)
         SELECT ?2, name FROM faces.not_person WHERE cluster = ?1",
        [from, into],
    )?;
    tx.execute("DELETE FROM faces.not_person WHERE cluster = ?1", [from])?;
    tx.execute("DELETE FROM faces.hidden_group WHERE cluster = ?1", [from])?;
    // What was kept apart from `from` is now kept apart from `into`.
    tx.execute(
        "INSERT OR IGNORE INTO faces.kept_apart (a, b)
         SELECT MIN(other, ?2), MAX(other, ?2) FROM (
             SELECT b AS other FROM faces.kept_apart WHERE a = ?1
             UNION SELECT a FROM faces.kept_apart WHERE b = ?1)
         WHERE other != ?2",
        [from, into],
    )?;
    tx.execute(
        "DELETE FROM faces.kept_apart WHERE a = ?1 OR b = ?1",
        [from],
    )?;
    Ok(())
}

/// The pairs of groups that must not be merged, `(a, b)` with `a < b`.
pub fn kept_apart(conn: &Connection) -> rusqlite::Result<HashSet<(i64, i64)>> {
    conn.prepare("SELECT a, b FROM faces.kept_apart")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Moves the faces of `photos` that are in `clusters` into a new group,
/// kept apart from those clusters. Returns the new group's id, or `None` if
/// none of those photos had a face there.
pub fn split_faces(
    conn: &mut Connection,
    clusters: &[i64],
    photos: &[String],
) -> rusqlite::Result<Option<i64>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let new: i64 = tx.query_row(
        "SELECT MAX(
             (SELECT COALESCE(MAX(cluster), 0) FROM faces.face),
             (SELECT COALESCE(MAX(b), 0) FROM faces.kept_apart)) + 1",
        [],
        |row| row.get(0),
    )?;
    let mut moved = 0;
    {
        let mut update = tx.prepare(
            "UPDATE faces.face SET cluster = ?3 WHERE fingerprint = ?1 AND cluster = ?2",
        )?;
        for photo in photos {
            for cluster in clusters {
                moved += update.execute(params![photo, cluster, new])?;
            }
        }
    }
    if moved == 0 {
        return Ok(None);
    }
    for cluster in clusters {
        tx.execute(
            "INSERT OR IGNORE INTO faces.kept_apart (a, b) VALUES (?1, ?2)",
            [*cluster, new],
        )?;
    }
    tx.commit()?;
    Ok(Some(new))
}

/// The groups hidden from the People screen.
pub fn hidden_face_groups(conn: &Connection) -> rusqlite::Result<HashSet<i64>> {
    conn.prepare("SELECT cluster FROM faces.hidden_group")?
        .query_map([], |row| row.get(0))?
        .collect()
}

pub fn set_face_groups_hidden(
    conn: &mut Connection,
    clusters: &[i64],
    hidden: bool,
) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for cluster in clusters {
        if hidden {
            tx.execute(
                "INSERT OR IGNORE INTO faces.hidden_group (cluster) VALUES (?1)",
                [cluster],
            )?;
        } else {
            tx.execute(
                "DELETE FROM faces.hidden_group WHERE cluster = ?1",
                [cluster],
            )?;
        }
    }
    tx.commit()
}

/// `(group, lowercased name)` pairs the user said don't go together.
pub fn not_person(conn: &Connection) -> rusqlite::Result<HashSet<(i64, String)>> {
    conn.prepare("SELECT cluster, name FROM faces.not_person")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

pub fn add_not_person(conn: &Connection, cluster: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO faces.not_person (cluster, name) VALUES (?1, ?2)",
        params![cluster, name.trim().to_lowercase()],
    )?;
    Ok(())
}

/// Where a face of a person's groups is in a photo.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceBox {
    pub fingerprint: String,
    /// Left, top, width, height, as fractions of the upright photo.
    pub bounds: [f64; 4],
}

/// The faces in `clusters`, one per photo (the most certain one).
pub fn faces_in(conn: &Connection, clusters: &[i64]) -> rusqlite::Result<Vec<FaceBox>> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint, left, top, width, height FROM faces.face
         WHERE cluster = ?1 ORDER BY score DESC",
    )?;
    let mut seen = std::collections::HashSet::new();
    let mut boxes = Vec::new();
    for cluster in clusters {
        let rows = stmt.query_map([cluster], |row| {
            Ok(FaceBox {
                fingerprint: row.get(0)?,
                bounds: [row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?],
            })
        })?;
        for face in rows {
            let face = face?;
            if seen.insert(face.fingerprint.clone()) {
                boxes.push(face);
            }
        }
    }
    Ok(boxes)
}

/// Takes the faces of `clusters` in the given photos out of their group, for
/// good. Returns how many faces were taken out.
pub fn remove_faces(
    conn: &mut Connection,
    clusters: &[i64],
    photos: &[String],
) -> rusqlite::Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut removed = 0;
    {
        let mut update = tx.prepare(
            "UPDATE faces.face SET cluster = -id WHERE fingerprint = ?1 AND cluster = ?2",
        )?;
        for photo in photos {
            for cluster in clusters {
                removed += update.execute(params![photo, cluster])?;
            }
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Names the place of every photo that has a position but no place yet.
/// `lookup` gives the display name and the search text for a position.
/// Returns how many photos were looked up.
pub fn fill_places(
    conn: &mut Connection,
    lookup: impl Fn(f64, f64) -> Option<(String, String)>,
) -> rusqlite::Result<usize> {
    let pending: Vec<(i64, f64, f64)> = conn
        .prepare(
            "SELECT id, latitude, longitude FROM photos
                  WHERE place IS NULL AND latitude IS NOT NULL AND longitude IS NOT NULL",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut update =
            tx.prepare("UPDATE photos SET place = ?2, place_search = ?3 WHERE id = ?1")?;
        for (id, latitude, longitude) in &pending {
            let (place, search) = lookup(*latitude, *longitude).unwrap_or_default();
            update.execute(params![id, place, search])?;
        }
    }
    tx.commit()?;
    Ok(pending.len())
}

/// Files whose lens, exposure and media type have not been read yet:
/// (id, rel_path), at most `limit`. Files that aren't pictures are marked
/// as read.
pub fn pending_shooting(conn: &Connection, limit: u32) -> rusqlite::Result<Vec<(i64, String)>> {
    conn.execute(
        "UPDATE photos SET shot_read = 1 WHERE shot_read = 0 AND kind = 0 AND thumb_state = 2",
        [],
    )?;
    conn.prepare_cached("SELECT id, rel_path FROM photos WHERE shot_read = 0 LIMIT ?1")?
        .query_map([limit], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Saves what was read by [`pending_shooting`]'s caller (`None`: nothing).
pub fn set_shooting(
    conn: &mut Connection,
    read: &[(
        i64,
        Option<crate::metadata::Shooting>,
        crate::metadata::Media,
    )],
) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut update = tx.prepare_cached(
            "UPDATE photos SET shot_read = 1, lens = ?2, focal_length = ?3, focal_35mm = ?4,
                               aperture = ?5, exposure = ?6, iso = ?7, media = ?8, burst = ?9
             WHERE id = ?1",
        )?;
        for (id, shot, media) in read {
            let shot = shot.clone().unwrap_or_default();
            update.execute(params![
                id,
                shot.lens,
                shot.focal_length,
                shot.focal_length_35mm,
                shot.aperture,
                shot.exposure,
                shot.iso,
                media.flags,
                media.burst
            ])?;
        }
    }
    tx.commit()
}

/// Pairs the files that are shown through another one, and returns how many
/// rows changed:
/// - a Live Photo's video, a short (or Apple-tagged) video next to a photo
///   with the same name;
/// - a RAW file and a JPEG (or HEIC, PNG...) with the same name, which the
///   camera saved together: in the same folder, or in different folders
///   (such as `trip/` and `trip/cr2/`) when both have the same EXIF capture
///   time, since camera file names repeat every 10,000 shots. The JPEG is
///   shown, as cameras and Apple Photos do.
///
/// The second file then disappears from the listings, and the photo gets a
/// link to it.
pub fn pair_parts(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(&format!(
        "CREATE TEMP TABLE IF NOT EXISTS live_pairs (
             image_id INTEGER PRIMARY KEY, video_id INTEGER, video_fp TEXT);
         CREATE TEMP TABLE IF NOT EXISTS raw_pairs (
             image_id INTEGER PRIMARY KEY, raw_id INTEGER, raw_fp TEXT);
         DELETE FROM live_pairs;
         DELETE FROM raw_pairs;
         INSERT OR REPLACE INTO live_pairs
             SELECT i.id, v.id, v.fingerprint
             FROM photos v JOIN photos i ON i.stem = v.stem AND i.kind = 0
             WHERE v.kind = 1 AND v.live_candidate = 1
             ORDER BY v.rel_path DESC;
         -- The JPEG must be decodable, or the photo would vanish altogether.
         -- First pairs across folders, then pairs in one folder, which win.
         -- (`i.part = 0` always holds for a JPEG; it lets the join use the
         -- timeline index on sort_date.)
         INSERT OR REPLACE INTO raw_pairs
             SELECT i.id, r.id, r.fingerprint
             FROM photos r JOIN photos i ON i.sort_date = r.sort_date
                 AND i.thumb_state != 2 AND i.part = 0 AND i.kind = 0
                 AND i.date_source = 'exif' AND NOT {image_is_raw}
                 AND {name_i} = {name_r}
             WHERE r.kind = 0 AND r.date_source = 'exif' AND {raw}
             ORDER BY r.rel_path DESC;
         INSERT OR REPLACE INTO raw_pairs
             SELECT i.id, r.id, r.fingerprint
             FROM photos r JOIN photos i ON i.stem = r.stem AND i.kind = 0
                 AND i.thumb_state != 2 AND NOT {image_is_raw}
             WHERE r.kind = 0 AND {raw}
             ORDER BY r.rel_path DESC;",
        image_is_raw = crate::raw::is_raw_sql("i.rel_path"),
        raw = crate::raw::is_raw_sql("r.rel_path"),
        name_i = file_name_sql("i.stem"),
        name_r = file_name_sql("r.stem"),
    ))?;
    let mut changed = 0;
    // Only rows whose pairing really changed are written.
    for update in [
        "UPDATE photos SET live_video = NULL
         WHERE live_video IS NOT NULL AND id NOT IN (SELECT image_id FROM live_pairs)",
        "UPDATE photos SET raw_file = NULL
         WHERE raw_file IS NOT NULL AND id NOT IN (SELECT image_id FROM raw_pairs)",
        "UPDATE photos SET part = 0
         WHERE part = 1 AND id NOT IN (SELECT video_id FROM live_pairs
                                       UNION SELECT raw_id FROM raw_pairs)",
        "UPDATE photos
         SET live_video = (SELECT video_fp FROM live_pairs WHERE image_id = photos.id)
         WHERE id IN (SELECT image_id FROM live_pairs)
           AND live_video IS NOT (SELECT video_fp FROM live_pairs WHERE image_id = photos.id)",
        "UPDATE photos
         SET raw_file = (SELECT raw_fp FROM raw_pairs WHERE image_id = photos.id)
         WHERE id IN (SELECT image_id FROM raw_pairs)
           AND raw_file IS NOT (SELECT raw_fp FROM raw_pairs WHERE image_id = photos.id)",
        "UPDATE photos SET part = 1
         WHERE part = 0 AND id IN (SELECT video_id FROM live_pairs
                                   UNION SELECT raw_id FROM raw_pairs)",
    ] {
        changed += tx.execute(update, [])?;
    }
    tx.commit()?;
    Ok(changed)
}

/// SQL for the last part of a `/`-separated path in `column`: `rtrim` with
/// every character but `/` strips the name, which tells where it starts.
fn file_name_sql(column: &str) -> String {
    format!("substr({column}, length(rtrim({column}, replace({column}, '/', ''))) + 1)")
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

/// Finds where a photo with this fingerprint is (its path and its EXIF
/// orientation), to make its thumbnail on demand or to show the original.
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

/// The fingerprint and EXIF orientation of the file at `rel_path`.
pub fn find_by_path(
    conn: &Connection,
    rel_path: &str,
) -> rusqlite::Result<Option<(String, Option<u8>)>> {
    conn.query_row(
        "SELECT fingerprint, orientation FROM photos WHERE rel_path = ?1",
        [rel_path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// What exporting a photo needs to know about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportItem {
    pub rel_path: String,
    pub orientation: Option<u8>,
    pub video: bool,
    pub sort_date: i64,
    pub edit: Option<crate::org::Edit>,
}

/// The photos to export, in the order given; unknown fingerprints are left out.
pub fn export_items(
    conn: &Connection,
    fingerprints: &[String],
) -> rusqlite::Result<Vec<ExportItem>> {
    let mut stmt = conn.prepare(
        "SELECT rel_path, orientation, kind = 1, sort_date,
                (SELECT value FROM org_state WHERE kind = 'edit' AND key = photos.fingerprint)
         FROM photos WHERE fingerprint = ?1 LIMIT 1",
    )?;
    let mut items = Vec::new();
    for fingerprint in fingerprints {
        let item = stmt
            .query_row([fingerprint], |row| {
                Ok(ExportItem {
                    rel_path: row.get(0)?,
                    orientation: row.get(1)?,
                    video: row.get(2)?,
                    sort_date: row.get(3)?,
                    edit: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|json| serde_json::from_str(&json).ok()),
                })
            })
            .optional()?;
        items.extend(item);
    }
    Ok(items)
}

/// `"2024/summer/img_1"` for `"2024/summer/IMG_1.HEIC"`.
fn stem_of(rel_path: &str) -> String {
    let name_at = rel_path.rfind('/').map_or(0, |slash| slash + 1);
    let stem = match rel_path[name_at..].rfind('.') {
        Some(dot) if dot > 0 => &rel_path[..name_at + dot],
        _ => rel_path,
    };
    stem.to_lowercase()
}

/// The folder part of a relative path: `"2024/summer"` for
/// `"2024/summer/a.jpg"`, and `""` for a file in the library root.
fn folder_of(rel_path: &str) -> &str {
    rel_path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// What a photo listing shows.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Listing {
    /// Every photo that is not hidden.
    Library,
    /// The photos inside one folder: directly, or also in its subfolders.
    Folder {
        folder: String,
        #[serde(default)]
        subfolders: bool,
    },
    Favorites,
    Album {
        album: String,
    },
    Hidden,
    /// The photos stored more than once (the same content in several files),
    /// every copy listed.
    Duplicates,
    /// The photos that show one person: the given groups of faces.
    Person {
        clusters: Vec<i64>,
    },
    /// The photos that match what was typed in the search box.
    Search {
        query: String,
    },
    /// The photos taken inside a rectangle on the map.
    Area {
        south: f64,
        west: f64,
        north: f64,
        east: f64,
    },
    /// The photos taken from the day `from` to the day `to`, both included
    /// (days since 1970-01-01, wall-clock): a trip, a season, a recent day.
    Days {
        from: i64,
        to: i64,
    },
    /// A chosen set of photos: a memory, the featured photos.
    Picked {
        fingerprints: Vec<String>,
    },
    /// Another listing narrowed down by the filter menu.
    Only {
        within: Box<Listing>,
        only: Only,
    },
    /// Another listing narrowed down to a camera, a lens, an aperture or a
    /// focal length (each optional).
    Gear {
        within: Box<Listing>,
        #[serde(flatten)]
        gear: Gear,
    },
}

/// What the camera filter narrows a listing down to.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gear {
    #[serde(default)]
    pub camera_make: Option<String>,
    #[serde(default)]
    pub camera_model: Option<String>,
    #[serde(default)]
    pub lens: Option<String>,
    /// The f-number, to one decimal (2.8).
    #[serde(default)]
    pub aperture: Option<f64>,
    /// The focal length in whole mm.
    #[serde(default)]
    pub focal_length: Option<f64>,
}

/// A text value written into SQL as a literal.
fn sql_text(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A number written into SQL: `{:?}` prints a finite float exactly, and
/// anything else matches nothing.
fn sql_number(value: f64) -> String {
    if value.is_finite() {
        format!("{value:?}")
    } else {
        "NULL".into()
    }
}

/// The aperture as the filter compares it (f/2.8, not f/2.83).
const APERTURE: &str = "round(aperture, 1)";
/// The focal length as the filter compares it, in whole mm.
const FOCAL: &str = "CAST(round(focal_length) AS INTEGER)";

impl Gear {
    fn condition(&self) -> String {
        let mut parts = Vec::new();
        if let Some(make) = &self.camera_make {
            parts.push(format!("camera_make = {}", sql_text(make)));
        }
        if let Some(model) = &self.camera_model {
            parts.push(format!("camera_model = {}", sql_text(model)));
        }
        if let Some(lens) = &self.lens {
            parts.push(format!("lens = {}", sql_text(lens)));
        }
        if let Some(aperture) = self.aperture {
            parts.push(format!(
                "{APERTURE} = {}",
                sql_number((aperture * 10.0).round() / 10.0)
            ));
        }
        if let Some(focal) = self.focal_length {
            parts.push(format!("{FOCAL} = {}", sql_number(focal.round())));
        }
        if parts.is_empty() {
            "1".into()
        } else {
            parts.join(" AND ")
        }
    }
}

/// The cameras, lenses, apertures and focal lengths of a listing, with how
/// many photos have each: what the camera filter offers.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GearOptions {
    /// (make, model, photos)
    pub cameras: Vec<(Option<String>, Option<String>, u32)>,
    pub lenses: Vec<(String, u32)>,
    pub apertures: Vec<(f64, u32)>,
    pub focal_lengths: Vec<(f64, u32)>,
    /// Photos whose lens and exposure are not read yet (until the scan ends).
    pub unread: u32,
}

/// What the camera filter can narrow `listing` down to.
pub fn gear_options(conn: &Connection, listing: &Listing) -> rusqlite::Result<GearOptions> {
    let (filter, param) = listing.filter();
    let mut options = GearOptions::default();
    let mut stmt = conn.prepare(&format!(
        "SELECT camera_make, camera_model, COUNT(*) FROM photos {filter}
           AND (camera_make IS NOT NULL OR camera_model IS NOT NULL)
         GROUP BY camera_make, camera_model ORDER BY COUNT(*) DESC"
    ))?;
    options.cameras = stmt
        .query_map([param], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT lens, COUNT(*) FROM photos {filter} AND lens IS NOT NULL
         GROUP BY lens ORDER BY COUNT(*) DESC"
    ))?;
    options.lenses = stmt
        .query_map([param], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {APERTURE} AS f, COUNT(*) FROM photos {filter} AND aperture IS NOT NULL
         GROUP BY f ORDER BY f"
    ))?;
    options.apertures = stmt
        .query_map([param], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {FOCAL} AS mm, COUNT(*) FROM photos {filter} AND focal_length IS NOT NULL
         GROUP BY mm ORDER BY mm"
    ))?;
    options.focal_lengths = stmt
        .query_map([param], |row| {
            Ok((row.get::<_, i64>(0)? as f64, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    options.unread = conn.query_row(
        &format!("SELECT COUNT(*) FROM photos {filter} AND shot_read = 0 AND kind = 0"),
        [param],
        |row| row.get(0),
    )?;
    Ok(options)
}

/// What the filter menu can narrow a listing down to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Only {
    Favorites,
    Edited,
    Photos,
    Videos,
    /// RAW files, on their own or behind a JPEG or HEIC of the same shot.
    Raw,
    /// Taken with the front camera.
    Selfies,
    LivePhotos,
    /// With depth data: an iPhone's Portrait mode.
    Portraits,
    /// Much wider (or taller) than a camera's picture.
    Panoramas,
    TimeLapses,
    SloMos,
    /// Shot in a burst, several a second.
    Bursts,
    Screenshots,
    NotInAlbum,
    /// Rated at least this many stars.
    Stars1,
    Stars2,
    Stars3,
    Stars4,
    Stars5,
    /// With this colour label.
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl Only {
    fn condition(self) -> String {
        match self {
            Only::Favorites => IS_FAVORITE.to_owned(),
            Only::Edited => "fingerprint IN (SELECT key FROM org_state
                                 WHERE kind = 'edit' AND value != '')"
                .to_owned(),
            Only::Photos => "kind = 0".to_owned(),
            Only::Videos => "kind = 1".to_owned(),
            Only::Raw => format!(
                "(raw_file IS NOT NULL OR {})",
                crate::raw::is_raw_sql("rel_path")
            ),
            Only::Selfies => "(kind = 0 AND lens LIKE '%front%')".to_owned(),
            Only::LivePhotos => "live_video IS NOT NULL".to_owned(),
            Only::Portraits => format!("(media & {}) != 0", crate::metadata::PORTRAIT),
            Only::Panoramas => format!(
                "(kind = 0 AND camera_model IS NOT NULL AND width > 0 AND height > 0
                  AND MAX(width, height) >= {PANORAMA_RATIO} * MIN(width, height))"
            ),
            Only::TimeLapses => format!("(media & {}) != 0", crate::metadata::TIME_LAPSE),
            Only::SloMos => format!("(media & {}) != 0", crate::metadata::SLO_MO),
            Only::Bursts => "burst IS NOT NULL".to_owned(),
            // Marked by iOS, or named so by macOS, Windows and Android.
            Only::Screenshots => format!(
                "(kind = 0 AND ((media & {}) != 0
                  OR (camera_model IS NULL AND (lower(rel_path) LIKE '%screenshot%'
                      OR lower(rel_path) LIKE '%screen shot%'
                      OR lower(rel_path) LIKE '%capture d%cran%'
                      OR lower(rel_path) LIKE '%bildschirmfoto%'))))",
                crate::metadata::SCREENSHOT
            ),
            Only::NotInAlbum => "fingerprint NOT IN (
                     SELECT m.photo FROM org_state m
                     WHERE m.kind = 'member' AND m.value = '1'
                       AND NOT EXISTS (SELECT 1 FROM org_state d
                           WHERE d.kind = 'album_deleted' AND d.key = m.key AND d.value = '1'))"
                .to_owned(),
            Only::Stars1 | Only::Stars2 | Only::Stars3 | Only::Stars4 | Only::Stars5 => {
                let least = match self {
                    Only::Stars1 => 1,
                    Only::Stars2 => 2,
                    Only::Stars3 => 3,
                    Only::Stars4 => 4,
                    _ => 5,
                };
                format!(
                    "fingerprint IN (SELECT key FROM org_state
                         WHERE kind = 'rating' AND CAST(value AS INTEGER) >= {least})"
                )
            }
            Only::Red | Only::Orange | Only::Yellow | Only::Green | Only::Blue | Only::Purple => {
                let color = match self {
                    Only::Red => "red",
                    Only::Orange => "orange",
                    Only::Yellow => "yellow",
                    Only::Green => "green",
                    Only::Blue => "blue",
                    _ => "purple",
                };
                format!(
                    "fingerprint IN (SELECT key FROM org_state
                         WHERE kind = 'label' AND value = '{color}')"
                )
            }
        }
    }
}

/// A day in milliseconds, the unit of `sort_date`.
pub const DAY_MS: i64 = 86_400_000;

/// A panorama is at least this many times longer than it is high (a camera's
/// picture is 1.8 at most).
const PANORAMA_RATIO: f64 = 2.5;

const IS_HIDDEN: &str =
    "fingerprint IN (SELECT key FROM org_state WHERE kind = 'hide' AND value = '1')";
const IS_FAVORITE: &str =
    "fingerprint IN (SELECT key FROM org_state WHERE kind = 'fav' AND value = '1')";
/// Rows that can be listed: decodable, and not the video half of a Live Photo.
const IS_LISTED: &str = "thumb_state != 2 AND part = 0";

impl Listing {
    /// The `WHERE` clause and its one parameter (`?1`, NULL when unused).
    /// Undecodable files and Live Photo videos are indexed but never listed,
    /// and hidden photos appear only in the "Hidden" listing.
    fn filter(&self) -> (String, Option<&str>) {
        let visible = format!("{IS_LISTED} AND NOT {IS_HIDDEN}");
        match self {
            Listing::Library => (format!("WHERE {visible} AND ?1 IS NULL"), None),
            Listing::Folder {
                folder,
                subfolders: false,
            } => (format!("WHERE {visible} AND folder = ?1"), Some(folder)),
            // `substr` rather than LIKE, which would read `_` and `%` in a
            // folder name as wildcards.
            Listing::Folder {
                folder,
                subfolders: true,
            } => (
                format!(
                    "WHERE {visible} AND (folder = ?1
                         OR substr(folder, 1, length(?1) + 1) = ?1 || '/')"
                ),
                Some(folder),
            ),
            Listing::Favorites => (
                format!("WHERE {visible} AND {IS_FAVORITE} AND ?1 IS NULL"),
                None,
            ),
            Listing::Album { album } => (
                format!(
                    "WHERE {visible} AND fingerprint IN (SELECT photo FROM org_state
                         WHERE kind = 'member' AND key = ?1 AND value = '1')"
                ),
                Some(album),
            ),
            Listing::Hidden => (
                format!("WHERE {IS_LISTED} AND {IS_HIDDEN} AND ?1 IS NULL"),
                None,
            ),
            Listing::Duplicates => (
                format!(
                    "WHERE {visible} AND fingerprint IN (SELECT fingerprint FROM photos
                         WHERE {IS_LISTED} GROUP BY fingerprint HAVING COUNT(*) > 1)
                       AND ?1 IS NULL"
                ),
                None,
            ),
            Listing::Person { clusters } => {
                let ids: Vec<String> = clusters.iter().map(i64::to_string).collect();
                (
                    format!(
                        "WHERE {visible} AND fingerprint IN (SELECT fingerprint FROM faces.face
                             WHERE cluster IN ({})) AND ?1 IS NULL",
                        ids.join(", ")
                    ),
                    None,
                )
            }
            Listing::Search { query } => (
                format!(
                    "WHERE {visible} AND ({}) AND ?1 IS NULL",
                    crate::search::condition(query)
                ),
                None,
            ),
            Listing::Area {
                south,
                west,
                north,
                east,
            } => {
                // Numbers are written into the SQL text: `{:?}` prints a
                // finite float exactly, and anything else selects nothing.
                let bounds = [south, west, north, east];
                let inside = if bounds.iter().all(|value| value.is_finite()) {
                    format!(
                        "latitude BETWEEN {south:?} AND {north:?}
                         AND longitude BETWEEN {west:?} AND {east:?}"
                    )
                } else {
                    "0".to_owned()
                };
                (format!("WHERE {visible} AND {inside} AND ?1 IS NULL"), None)
            }
            Listing::Days { from, to } => (
                format!(
                    "WHERE {visible} AND sort_date >= {} AND sort_date < {} AND ?1 IS NULL",
                    from.saturating_mul(DAY_MS),
                    to.saturating_add(1).saturating_mul(DAY_MS)
                ),
                None,
            ),
            Listing::Picked { fingerprints } => {
                let picked: Vec<String> = fingerprints
                    .iter()
                    .filter(|fingerprint| crate::thumbs::is_fingerprint(fingerprint))
                    .map(|fingerprint| sql_text(fingerprint))
                    .collect();
                let inside = if picked.is_empty() {
                    "0".to_owned()
                } else {
                    format!("fingerprint IN ({})", picked.join(", "))
                };
                (format!("WHERE {visible} AND {inside} AND ?1 IS NULL"), None)
            }
            Listing::Only { within, only } => {
                let (filter, param) = within.filter();
                (format!("{filter} AND {}", only.condition()), param)
            }
            Listing::Gear { within, gear } => {
                let (filter, param) = within.filter();
                (format!("{filter} AND {}", gear.condition()), param)
            }
        }
    }
}

/// One day of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayGroup {
    /// Days since 1970-01-01, in the photos' wall-clock time. `None` when the
    /// listing is sorted by name: then one group holds every photo.
    pub day: Option<i64>,
    pub count: u32,
    /// How many of `count` are videos.
    pub videos: u32,
}

/// The order of a listing in the photo grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Order {
    /// Date taken, newest first.
    #[default]
    DateDesc,
    DateAsc,
    /// File name, A to Z (ignoring case); ties broken by folder.
    NameAsc,
    NameDesc,
}

impl Order {
    fn by_name(self) -> bool {
        matches!(self, Order::NameAsc | Order::NameDesc)
    }

    /// The `ORDER BY` terms. The date orders walk the timeline index, one
    /// way or the other.
    fn sql(self) -> String {
        let name = format!("{} COLLATE NOCASE", file_name_sql("rel_path"));
        match self {
            Order::DateDesc => "sort_date DESC, rel_path".to_owned(),
            Order::DateAsc => "sort_date, rel_path DESC".to_owned(),
            Order::NameAsc => format!("{name}, rel_path"),
            Order::NameDesc => format!("{name} DESC, rel_path DESC"),
        }
    }
}

/// How many photos each day has, newest day first. Together with
/// [`photos_page`] (same order) this is all the grid needs to lay itself out
/// without loading every photo.
#[cfg(test)]
pub fn timeline(conn: &Connection, listing: &Listing) -> rusqlite::Result<Vec<DayGroup>> {
    timeline_in(conn, listing, Order::DateDesc)
}

/// [`timeline`] for a listing in the given order.
pub fn timeline_in(
    conn: &Connection,
    listing: &Listing,
    order: Order,
) -> rusqlite::Result<Vec<DayGroup>> {
    let (filter, param) = listing.filter();
    if order.by_name() {
        let (count, videos): (u32, u32) = conn.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(kind = 1), 0) FROM photos {filter}"),
            [param],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        return Ok(if count == 0 {
            Vec::new()
        } else {
            vec![DayGroup {
                day: None,
                count,
                videos,
            }]
        });
    }
    let direction = if order == Order::DateAsc { "" } else { "DESC" };
    // Integer division truncates toward zero, so shift pre-1970 dates down a
    // day first to get a floor.
    let mut stmt = conn.prepare(&format!(
        "SELECT (sort_date - 86400000 * (sort_date < 0 AND sort_date % 86400000 != 0))
                    / 86400000 AS day,
                COUNT(*), SUM(kind = 1)
         FROM photos {filter} GROUP BY day ORDER BY day {direction}"
    ))?;
    let rows = stmt.query_map([param], |row| {
        Ok(DayGroup {
            day: Some(row.get(0)?),
            count: row.get(1)?,
            videos: row.get(2)?,
        })
    })?;
    rows.collect()
}

/// One month of a listing, for the Years and Months overviews.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthSummary {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    pub count: u32,
    /// The days of the month with the most photos (at most `MONTH_DAYS`), in
    /// date order: what the Months view shows as cards.
    pub days: Vec<DaySummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaySummary {
    /// Days since 1970-01-01, as in [`DayGroup`].
    pub day: i64,
    pub count: u32,
    /// The photo that stands for the day: a favorite if there is one, a
    /// photo rather than a video, otherwise a random-looking pick.
    pub cover: String,
}

/// How many days of each month the Months view shows.
const MONTH_DAYS: u32 = 4;

/// The months of a listing, newest first.
pub fn months(conn: &Connection, listing: &Listing) -> rusqlite::Result<Vec<MonthSummary>> {
    let (filter, param) = listing.filter();
    // Ordering by fingerprint (a hash) picks the same random-looking cover
    // every time, instead of the day's first or last shot.
    let mut stmt = conn.prepare(&format!(
        "WITH listed AS (
             SELECT (sort_date - 86400000 * (sort_date < 0 AND sort_date % 86400000 != 0))
                        / 86400000 AS day,
                    {IS_FAVORITE} AS favorite, kind, fingerprint
             FROM photos {filter}),
         days AS (
             SELECT day, fingerprint,
                    COUNT(*) OVER (PARTITION BY day) AS count,
                    ROW_NUMBER() OVER (
                        PARTITION BY day ORDER BY favorite DESC, kind, fingerprint) AS pick
             FROM listed),
         ranked AS (
             SELECT day, fingerprint, count,
                    strftime('%Y-%m', day * 86400, 'unixepoch') AS ym,
                    SUM(count) OVER (PARTITION BY strftime('%Y-%m', day * 86400, 'unixepoch'))
                        AS total,
                    ROW_NUMBER() OVER (
                        PARTITION BY strftime('%Y-%m', day * 86400, 'unixepoch')
                        ORDER BY count DESC, day DESC) AS rank
             FROM days WHERE pick = 1)
         SELECT ym, total, day, count, fingerprint FROM ranked
         WHERE rank <= {MONTH_DAYS}
         ORDER BY ym DESC, day"
    ))?;
    let mut months: Vec<MonthSummary> = Vec::new();
    let mut rows = stmt.query([param])?;
    while let Some(row) = rows.next()? {
        let ym: String = row.get(0)?;
        let parsed = ym.split_once('-').and_then(|(year, month)| {
            // Years before 0 come out as "-0001-05": not worth showing right.
            Some((year.parse::<i32>().ok()?, month.parse::<u32>().ok()?))
        });
        let Some((year, month)) = parsed else {
            continue;
        };
        let day = DaySummary {
            day: row.get(2)?,
            count: row.get(3)?,
            cover: row.get(4)?,
        };
        match months.last_mut() {
            Some(last) if last.year == year && last.month == month => last.days.push(day),
            _ => months.push(MonthSummary {
                year,
                month,
                count: row.get(1)?,
                days: vec![day],
            }),
        }
    }
    Ok(months)
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
    pub favorite: bool,
    pub video: bool,
    pub duration_ms: Option<u64>,
    /// For a Live Photo: the fingerprint of its video.
    pub live_video: Option<String>,
    /// For a JPEG + RAW pair: the fingerprint of its RAW file.
    pub raw_file: Option<String>,
    /// How the photo is shown, if it was edited.
    pub edit: Option<crate::org::Edit>,
    /// 0 (not rated) to 5 stars.
    pub rating: u8,
    /// Its colour label, if any.
    pub label: Option<String>,
}

/// A slice of the listing, newest first (ties broken by path).
#[cfg(test)]
pub fn photos_page(
    conn: &Connection,
    listing: &Listing,
    offset: u32,
    limit: u32,
) -> rusqlite::Result<Vec<PhotoItem>> {
    photos_page_in(conn, listing, Order::DateDesc, offset, limit)
}

/// [`photos_page`] for a listing in the given order.
pub fn photos_page_in(
    conn: &Connection,
    listing: &Listing,
    order: Order,
    offset: u32,
    limit: u32,
) -> rusqlite::Result<Vec<PhotoItem>> {
    let (filter, param) = listing.filter();
    let order = order.sql();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT fingerprint, rel_path, sort_date, width, height, {IS_FAVORITE},
                kind, duration_ms, live_video,
                (SELECT value FROM org_state WHERE kind = 'edit' AND key = photos.fingerprint),
                raw_file,
                (SELECT value FROM org_state WHERE kind = 'rating' AND key = photos.fingerprint),
                (SELECT value FROM org_state WHERE kind = 'label' AND key = photos.fingerprint)
         FROM photos {filter} ORDER BY {order} LIMIT ?3 OFFSET ?2"
    ))?;
    let rows = stmt.query_map(params![param, offset, limit], |row| {
        Ok(PhotoItem {
            fingerprint: row.get(0)?,
            rel_path: row.get(1)?,
            sort_date: row.get(2)?,
            width: row.get(3)?,
            height: row.get(4)?,
            favorite: row.get(5)?,
            video: row.get(6)?,
            duration_ms: row.get::<_, Option<i64>>(7)?.map(|ms| ms as u64),
            live_video: row.get(8)?,
            edit: row
                .get::<_, Option<String>>(9)?
                .and_then(|json| serde_json::from_str(&json).ok()),
            raw_file: row.get(10)?,
            rating: row
                .get::<_, Option<String>>(11)?
                .and_then(|stars| stars.parse().ok())
                .unwrap_or(0),
            label: row
                .get::<_, Option<String>>(12)?
                .filter(|color| !color.is_empty()),
        })
    })?;
    rows.collect()
}

/// The shape of every photo of a listing, in grid order: width divided by
/// height as shown (after any edit), times 1000, kept between 1/4 and 4 so
/// that a panorama can't take a whole row. 1500 (3:2) when unknown. The
/// "river" layout needs them all to place the rows before photos load.
pub fn shapes(conn: &Connection, listing: &Listing, order: Order) -> rusqlite::Result<Vec<u16>> {
    let (filter, param) = listing.filter();
    let mut stmt = conn.prepare(&format!(
        "SELECT width, height,
                (SELECT value FROM org_state WHERE kind = 'edit' AND key = photos.fingerprint)
         FROM photos {filter} ORDER BY {}",
        order.sql()
    ))?;
    let rows = stmt.query_map([param], |row| {
        let width: Option<u32> = row.get(0)?;
        let height: Option<u32> = row.get(1)?;
        let edit: Option<String> = row.get(2)?;
        let (Some(width), Some(height)) = (width.filter(|w| *w > 0), height.filter(|h| *h > 0))
        else {
            return Ok(1500);
        };
        let (mut width, mut height) = (f64::from(width), f64::from(height));
        if let Some(edit) =
            edit.and_then(|json| serde_json::from_str::<crate::org::Edit>(&json).ok())
        {
            if edit.rotate % 180 != 0 {
                std::mem::swap(&mut width, &mut height);
            }
            if let Some([_, _, crop_width, crop_height]) = edit.crop {
                width *= f64::from(crop_width.max(1));
                height *= f64::from(crop_height.max(1));
            }
        }
        Ok((width / height * 1000.0).clamp(250.0, 4000.0).round() as u16)
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
        "SELECT folder, COUNT(*) FROM photos
         WHERE {IS_LISTED} AND NOT {IS_HIDDEN} GROUP BY folder ORDER BY folder"
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
    pub favorite: bool,
    pub hidden: bool,
    /// Ids of the albums the photo is in.
    pub albums: Vec<String>,
    pub video: bool,
    pub duration_ms: Option<u64>,
    /// For a Live Photo: the fingerprint of its video.
    pub live_video: Option<String>,
    /// For a JPEG + RAW pair: where its RAW file is.
    pub raw_path: Option<String>,
    /// The nearest town, such as "Paris, France".
    pub place: Option<String>,
}

pub fn photo_details(conn: &Connection, rel_path: &str) -> rusqlite::Result<Option<PhotoDetails>> {
    let details = conn
        .query_row(
            &format!(
                "SELECT rel_path, fingerprint, size, sort_date, date_source, width, height,
                        camera_make, camera_model, latitude, longitude,
                        {IS_FAVORITE}, {IS_HIDDEN}, kind, duration_ms, live_video,
                        NULLIF(place, ''),
                        (SELECT r.rel_path FROM photos r
                         WHERE r.fingerprint = photos.raw_file LIMIT 1)
                 FROM photos WHERE rel_path = ?1"
            ),
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
                    favorite: row.get(11)?,
                    hidden: row.get(12)?,
                    albums: Vec::new(),
                    video: row.get(13)?,
                    duration_ms: row.get::<_, Option<i64>>(14)?.map(|ms| ms as u64),
                    live_video: row.get(15)?,
                    place: row.get(16)?,
                    raw_path: row.get(17)?,
                })
            },
        )
        .optional()?;
    let Some(mut details) = details else {
        return Ok(None);
    };
    let mut stmt = conn.prepare_cached(
        "SELECT key FROM org_state WHERE kind = 'member' AND photo = ?1 AND value = '1'",
    )?;
    details.albums = stmt
        .query_map([&details.fingerprint], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(details))
}

/// A photo's position, for the map.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
    pub fingerprint: String,
}

/// The listed, non-hidden photos that have a position inside the rectangle,
/// newest first.
pub fn geo_points(
    conn: &Connection,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> rusqlite::Result<Vec<GeoPoint>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT latitude, longitude, fingerprint FROM photos
         WHERE {IS_LISTED} AND NOT {IS_HIDDEN}
           AND latitude BETWEEN ?1 AND ?3 AND longitude BETWEEN ?2 AND ?4
         ORDER BY sort_date DESC, rel_path"
    ))?;
    let rows = stmt.query_map(params![south, west, north, east], |row| {
        Ok(GeoPoint {
            latitude: row.get(0)?,
            longitude: row.get(1)?,
            fingerprint: row.get(2)?,
        })
    })?;
    rows.collect()
}

/// Every file: `(rel_path, size, fingerprint)`, where the fingerprint is the
/// one of the photo the file is listed as. So the RAW file of a JPEG + RAW
/// pair, or the video of a Live Photo, gives the fingerprint of its JPEG.
pub fn files(conn: &Connection) -> rusqlite::Result<Vec<(String, u64, String)>> {
    // Which photo each paired file is shown through.
    let mut shown_as: HashMap<String, String> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT raw_file, fingerprint FROM photos WHERE raw_file IS NOT NULL
         UNION ALL
         SELECT live_video, fingerprint FROM photos WHERE live_video IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        shown_as.insert(row.get(0)?, row.get(1)?);
    }
    let mut stmt = conn.prepare("SELECT rel_path, size, fingerprint, part FROM photos")?;
    let rows = stmt.query_map([], |row| {
        let fingerprint: String = row.get(2)?;
        let part: bool = row.get(3)?;
        let listed_as = match shown_as.get(&fingerprint) {
            Some(main) if part => main.clone(),
            _ => fingerprint,
        };
        Ok((row.get(0)?, row.get::<_, i64>(1)? as u64, listed_as))
    })?;
    rows.collect()
}

// --- Faces and people (see faces.rs and people.rs) ------------------------------

/// The photos (not videos) that have a preview and have not been searched
/// for faces yet: fingerprints, newest first.
pub fn pending_faces(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT fingerprint FROM photos
         WHERE kind = 0 AND thumb_state = 1 AND part = 0
           AND fingerprint NOT IN (SELECT fingerprint FROM faces.scanned)
         ORDER BY sort_date DESC",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

/// A face as it is stored: its box, how sure the detector was, its embedding.
pub struct NewFace<'a> {
    pub bounds: [f32; 4],
    pub score: f32,
    pub embedding: &'a [f32],
}

/// Records that a photo was searched, and the faces found in it.
pub fn save_faces(
    tx: &Transaction<'_>,
    fingerprint: &str,
    faces: &[NewFace<'_>],
) -> rusqlite::Result<()> {
    tx.prepare_cached("INSERT OR IGNORE INTO faces.scanned (fingerprint) VALUES (?1)")?
        .execute([fingerprint])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO faces.face (fingerprint, left, top, width, height, score, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for face in faces {
        let bytes: Vec<u8> = face
            .embedding
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let [left, top, width, height] = face.bounds;
        insert.execute(params![
            fingerprint,
            left,
            top,
            width,
            height,
            face.score,
            bytes
        ])?;
    }
    Ok(())
}

/// A face's id, its group (if any) and its embedding.
pub type StoredFace = (i64, Option<i64>, Vec<f32>);

/// A face as the grouping sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupingFace {
    pub id: i64,
    /// `None` until grouped; 0 when grouped into nothing (see people.rs);
    /// below zero when taken out by hand.
    pub cluster: Option<i64>,
    pub fingerprint: String,
    /// Left, top, width, height as fractions of the upright photo.
    pub bounds: [f64; 4],
    pub score: f32,
    /// How wide it is in the photo's preview (about 1024 px long), the
    /// picture the faces were found in.
    pub preview_px: f32,
    pub embedding: Vec<f32>,
}

/// Every face, with what the grouping needs to know about it.
pub fn grouping_faces(conn: &Connection) -> rusqlite::Result<Vec<GroupingFace>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.cluster, f.fingerprint, f.left, f.top, f.width, f.height, f.score,
                p.width, p.height, f.embedding
         FROM faces.face f
         LEFT JOIN (SELECT fingerprint, MAX(width) AS width, MAX(height) AS height
                    FROM photos GROUP BY fingerprint) p ON p.fingerprint = f.fingerprint
         ORDER BY f.id",
    )?;
    let rows = stmt.query_map([], |row| {
        let width: f64 = row.get(5)?;
        let size: (Option<f64>, Option<f64>) = (row.get(8)?, row.get(9)?);
        let preview_px = match size {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => width * w * (1024.0 / w.max(h)).min(1.0),
            _ => width * 1024.0,
        };
        let bytes: Vec<u8> = row.get(10)?;
        Ok(GroupingFace {
            id: row.get(0)?,
            cluster: row.get(1)?,
            fingerprint: row.get(2)?,
            bounds: [row.get(3)?, row.get(4)?, width, row.get(6)?],
            score: row.get(7)?,
            preview_px: preview_px as f32,
            embedding: bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes(*chunk))
                .collect(),
        })
    })?;
    rows.collect()
}

/// The version of the grouping that made the face groups (see people.rs).
pub fn grouping_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA faces.user_version", [], |row| row.get(0))
}

pub fn set_grouping_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA faces.user_version = {version}"))
}

/// Takes every face out of its group, to group them all again, except the
/// faces taken out by hand. Returns the group each face was in.
pub fn ungroup_faces(conn: &mut Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let old: HashMap<i64, i64> = tx
        .prepare("SELECT id, cluster FROM faces.face WHERE cluster > 0")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    tx.execute(
        "UPDATE faces.face SET cluster = NULL WHERE cluster >= 0",
        [],
    )?;
    tx.commit()?;
    Ok(old)
}

/// The groups each face is in now: (face, group), for grouped faces.
pub fn face_clusters(conn: &Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    conn.prepare("SELECT id, cluster FROM faces.face WHERE cluster > 0")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Moves what was said about groups (hidden, not that person, kept apart)
/// from old group ids to new ones; what has no new group is dropped.
pub fn remap_face_groups(conn: &mut Connection, map: &HashMap<i64, i64>) -> rusqlite::Result<()> {
    let hidden = hidden_face_groups(conn)?;
    let refused = not_person(conn)?;
    let apart = kept_apart(conn)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "DELETE FROM faces.hidden_group; DELETE FROM faces.not_person; DELETE FROM faces.kept_apart;",
    )?;
    for cluster in hidden.iter().filter_map(|c| map.get(c)) {
        tx.execute(
            "INSERT OR IGNORE INTO faces.hidden_group (cluster) VALUES (?1)",
            [cluster],
        )?;
    }
    for (cluster, name) in &refused {
        if let Some(cluster) = map.get(cluster) {
            tx.execute(
                "INSERT OR IGNORE INTO faces.not_person (cluster, name) VALUES (?1, ?2)",
                params![cluster, name],
            )?;
        }
    }
    for (a, b) in &apart {
        if let (Some(a), Some(b)) = (map.get(a), map.get(b)) {
            if a != b {
                tx.execute(
                    "INSERT OR IGNORE INTO faces.kept_apart (a, b) VALUES (?1, ?2)",
                    [a.min(b), a.max(b)],
                )?;
            }
        }
    }
    tx.commit()
}

/// Pairs of groups (a < b) that both have a face in the same photo: they
/// are two people.
pub fn groups_seen_together(conn: &Connection) -> rusqlite::Result<HashSet<(i64, i64)>> {
    conn.prepare(
        "SELECT DISTINCT a.cluster, b.cluster FROM faces.face a
         JOIN faces.face b ON b.fingerprint = a.fingerprint AND b.cluster > a.cluster
         WHERE a.cluster > 0",
    )?
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
    .collect()
}

/// Every face, for grouping.
pub fn face_embeddings(conn: &Connection) -> rusqlite::Result<Vec<StoredFace>> {
    let mut stmt = conn.prepare("SELECT id, cluster, embedding FROM faces.face ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        let bytes: Vec<u8> = row.get(2)?;
        let embedding = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        Ok((row.get(0)?, row.get(1)?, embedding))
    })?;
    rows.collect()
}

pub fn set_face_clusters(conn: &mut Connection, groups: &[(i64, i64)]) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut update = tx.prepare("UPDATE faces.face SET cluster = ?2 WHERE id = ?1")?;
        for (face, cluster) in groups {
            update.execute(params![face, cluster])?;
        }
    }
    tx.commit()
}

/// One group of look-alike faces, as far as the listed photos go.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceGroup {
    pub cluster: i64,
    /// Photos (not hidden) the group appears in.
    pub photos: u32,
    /// The group's largest, clearest face: its photo, the photo's upright
    /// size when known, and the face's box.
    pub cover: String,
    pub cover_size: Option<(u32, u32)>,
    pub cover_bounds: [f64; 4],
}

pub fn face_groups(conn: &Connection) -> rusqlite::Result<Vec<FaceGroup>> {
    // With a single MAX(), SQLite takes the other columns from the row that
    // holds the maximum: the cover face.
    let mut stmt = conn.prepare(&format!(
        "SELECT f.cluster, COUNT(DISTINCT f.fingerprint), MAX(f.width * f.height * f.score),
                f.fingerprint, p.width, p.height, f.left, f.top, f.width, f.height
         FROM faces.face f JOIN photos p ON p.fingerprint = f.fingerprint
         WHERE f.cluster > 0
           AND p.thumb_state != 2 AND p.part = 0
           AND NOT p.{IS_HIDDEN}
         GROUP BY f.cluster"
    ))?;
    let rows = stmt.query_map([], |row| {
        let size: (Option<u32>, Option<u32>) = (row.get(4)?, row.get(5)?);
        Ok(FaceGroup {
            cluster: row.get(0)?,
            photos: row.get(1)?,
            cover: row.get(3)?,
            cover_size: size.0.zip(size.1),
            cover_bounds: [row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?],
        })
    })?;
    rows.collect()
}

/// The names given to faces: `(photo fingerprint, x, y, name)`, where x and
/// y are a point inside the face in ten-thousandths of the photo.
pub fn face_names(conn: &Connection) -> rusqlite::Result<Vec<(String, f64, f64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT key, photo, value FROM org_state WHERE kind = 'face_name' AND value != ''",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut names = Vec::new();
    for row in rows {
        let (fingerprint, point, name) = row?;
        let parsed = point
            .split_once(',')
            .and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?)));
        if let Some((x, y)) = parsed {
            names.push((fingerprint, x / 10_000.0, y / 10_000.0, name));
        }
    }
    Ok(names)
}

/// The key photos chosen for people, by lowercase name: the photo and a
/// point of the face (fractions of the photo).
pub fn face_keys(conn: &Connection) -> rusqlite::Result<HashMap<String, (String, f64, f64)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM org_state WHERE kind = 'face_key'")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut keys = HashMap::new();
    for row in rows {
        let (name, value) = row?;
        let mut parts = value.split(',');
        let (Some(photo), Some(x), Some(y)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) {
            keys.insert(name, (photo.to_owned(), x / 10_000.0, y / 10_000.0));
        }
    }
    Ok(keys)
}

/// A photo's upright size when known, and a face's box in it.
pub type FaceSpot = (Option<(u32, u32)>, [f64; 4]);

/// A face of one of `clusters` in a listed photo: the photo's upright size
/// when known, and the face's box. With a point (fractions of the photo),
/// the face that contains it; without, the largest.
pub fn face_of(
    conn: &Connection,
    fingerprint: &str,
    clusters: &[i64],
    point: Option<(f64, f64)>,
) -> rusqlite::Result<Option<FaceSpot>> {
    let list = clusters
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let (x, y) = point.unwrap_or((-1.0, -1.0));
    conn.query_row(
        &format!(
            "SELECT p.width, p.height, f.left, f.top, f.width, f.height
             FROM faces.face f JOIN photos p ON p.fingerprint = f.fingerprint
             WHERE f.fingerprint = ?1 AND f.cluster IN ({list})
               AND p.thumb_state != 2 AND p.part = 0 AND NOT p.{IS_HIDDEN}
               AND (?2 < 0 OR (?2 BETWEEN f.left AND f.left + f.width
                               AND ?3 BETWEEN f.top AND f.top + f.height))
             ORDER BY f.width * f.height DESC LIMIT 1"
        ),
        params![fingerprint, x, y],
        |row| {
            let size: (Option<u32>, Option<u32>) = (row.get(0)?, row.get(1)?);
            Ok((
                size.0.zip(size.1),
                [row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?],
            ))
        },
    )
    .optional()
}

/// The group of the face in `fingerprint` that contains the point (x, y),
/// given as fractions of the photo.
pub fn cluster_at(
    conn: &Connection,
    fingerprint: &str,
    x: f64,
    y: f64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT cluster FROM faces.face
         WHERE fingerprint = ?1 AND cluster > 0
           AND ?2 BETWEEN left AND left + width AND ?3 BETWEEN top AND top + height
         ORDER BY width * height DESC LIMIT 1",
        params![fingerprint, x, y],
        |row| row.get(0),
    )
    .optional()
}

// --- Organization (see org.rs) ------------------------------------------------

/// Applies one event from a device log: sets the value it names, unless a
/// later event (greater `(ts, device)`) already did.
pub fn apply_event(tx: &Transaction<'_>, device: &str, event: &Event) -> rusqlite::Result<()> {
    let flag = |on: &bool| if *on { "1" } else { "0" };
    let edit_json: String;
    let face_point: String;
    let face_key: (String, String);
    let rating: String;
    let (kind, key, photo, value) = match &event.op {
        Op::Fav { photo, on } => ("fav", photo.as_str(), "", flag(on)),
        Op::Hide { photo, on } => ("hide", photo.as_str(), "", flag(on)),
        Op::AlbumName { album, name } => ("album_name", album.as_str(), "", name.trim()),
        Op::AlbumDeleted { album, on } => ("album_deleted", album.as_str(), "", flag(on)),
        Op::AlbumMember { album, photo, on } => {
            ("member", album.as_str(), photo.as_str(), flag(on))
        }
        Op::FaceName { photo, x, y, name } => {
            face_point = format!("{x},{y}");
            (
                "face_name",
                photo.as_str(),
                face_point.as_str(),
                name.trim(),
            )
        }
        Op::FaceKey { photo, x, y, name } => {
            face_key = (name.trim().to_lowercase(), format!("{photo},{x},{y}"));
            ("face_key", face_key.0.as_str(), "", face_key.1.as_str())
        }
        Op::Rating { photo, stars } => {
            rating = stars.to_string();
            ("rating", photo.as_str(), "", rating.as_str())
        }
        Op::Label { photo, color } => ("label", photo.as_str(), "", color.as_str()),
        Op::Edit { photo, edit } => {
            // The edit as JSON; an empty value means "not edited".
            edit_json = edit
                .and_then(|edit| serde_json::to_string(&edit).ok())
                .unwrap_or_default();
            ("edit", photo.as_str(), "", edit_json.as_str())
        }
    };
    tx.prepare_cached(
        "INSERT INTO org_state (kind, key, photo, value, ts, device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (kind, key, photo) DO UPDATE SET
             value = excluded.value, ts = excluded.ts, device = excluded.device
         WHERE (excluded.ts, excluded.device) > (org_state.ts, org_state.device)",
    )?
    .execute(params![kind, key, photo, value, event.ts, device])?;
    Ok(())
}

/// What the organization says about one photo: `(kind, key, photo, value)`
/// rows of `org_state` that name it and set something (favorite, hidden,
/// album, rating, label, face name).
pub fn organization_of(
    conn: &Connection,
    fingerprint: &str,
) -> rusqlite::Result<Vec<(String, String, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT kind, key, photo, value FROM org_state
         WHERE ((kind IN ('fav', 'hide', 'rating', 'label', 'face_name') AND key = ?1)
                OR (kind = 'member' AND photo = ?1))
           AND value NOT IN ('', '0')",
    )?;
    let rows = statement.query_map([fingerprint], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    rows.collect()
}

/// The newest event timestamp merged so far (0 if none), for the logical clock.
pub fn max_event_ts(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(ts), 0) FROM org_state", [], |row| {
        row.get(0)
    })
}

pub fn log_offset(conn: &Connection, source: &str) -> rusqlite::Result<u64> {
    let offset: Option<i64> = conn
        .query_row(
            "SELECT offset FROM log_offsets WHERE source = ?1",
            [source],
            |row| row.get(0),
        )
        .optional()?;
    Ok(offset.unwrap_or(0) as u64)
}

pub fn set_log_offset(tx: &Transaction<'_>, source: &str, offset: u64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO log_offsets (source, offset) VALUES (?1, ?2)
         ON CONFLICT (source) DO UPDATE SET offset = excluded.offset",
        params![source, offset as i64],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    /// Photos of the album that are in the index and not hidden.
    pub count: u32,
    /// Its latest photo, to show it with.
    pub cover: Option<String>,
}

/// The albums that have not been deleted, by name.
pub fn albums(conn: &Connection) -> rusqlite::Result<Vec<Album>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT a.key, a.value,
                (SELECT COUNT(*) FROM photos
                 WHERE {IS_LISTED} AND NOT {IS_HIDDEN}
                   AND fingerprint IN (SELECT photo FROM org_state
                                       WHERE kind = 'member' AND key = a.key AND value = '1')),
                (SELECT fingerprint FROM photos
                 WHERE {IS_LISTED} AND NOT {IS_HIDDEN}
                   AND fingerprint IN (SELECT photo FROM org_state
                                       WHERE kind = 'member' AND key = a.key AND value = '1')
                 ORDER BY sort_date DESC LIMIT 1)
         FROM org_state a
         WHERE a.kind = 'album_name'
           AND NOT EXISTS (SELECT 1 FROM org_state d
                           WHERE d.kind = 'album_deleted' AND d.key = a.key AND d.value = '1')
         ORDER BY a.value COLLATE NOCASE, a.key"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok(Album {
            id: row.get(0)?,
            name: row.get(1)?,
            count: row.get(2)?,
            cover: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// The condition a photo meets to be shown in the library (not hidden), for
/// queries outside this file.
pub fn visible_condition() -> String {
    format!("{IS_LISTED} AND NOT {IS_HIDDEN}")
}

/// How many photos of the library each filter keeps: the counts of the
/// Collections page's Media Types and Utilities.
pub fn only_counts(conn: &Connection, kinds: &[Only]) -> rusqlite::Result<HashMap<Only, u32>> {
    if kinds.is_empty() {
        return Ok(HashMap::new());
    }
    let sums: Vec<String> = kinds
        .iter()
        .map(|only| format!("COALESCE(SUM({}), 0)", only.condition()))
        .collect();
    let sql = format!(
        "SELECT {} FROM photos WHERE {IS_LISTED} AND NOT {IS_HIDDEN}",
        sums.join(", ")
    );
    conn.query_row(&sql, [], |row| {
        kinds
            .iter()
            .enumerate()
            .map(|(column, only)| Ok((*only, row.get::<_, i64>(column)? as u32)))
            .collect()
    })
}

/// How many hidden photos there are, and how many are stored more than once.
pub fn hidden_and_duplicates(conn: &Connection) -> rusqlite::Result<(u32, u32)> {
    let hidden = conn.query_row(
        &format!("SELECT COUNT(*) FROM photos WHERE {IS_LISTED} AND {IS_HIDDEN}"),
        [],
        |row| row.get(0),
    )?;
    let duplicates = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT 1 FROM photos WHERE {IS_LISTED} AND NOT {IS_HIDDEN}
                 GROUP BY fingerprint HAVING COUNT(*) > 1)"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok((hidden, duplicates))
}

/// How many photos and videos are indexed. A Live Photo counts once.
pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM photos WHERE part = 0", [], |row| {
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
            is_video: false,
            duration_ms: None,
            live_candidate: false,
        }
    }

    #[test]
    fn ratings_and_labels_show_and_filter() {
        use crate::org::{Event, Op};
        let mut conn = open_in_memory().unwrap();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let tx = conn.transaction().unwrap();
        for (rel_path, fingerprint) in [("a.jpg", &a), ("b.jpg", &b)] {
            upsert(
                &tx,
                &PhotoRow {
                    fingerprint: fingerprint.clone(),
                    ..row(rel_path, 10)
                },
            )
            .unwrap();
        }
        let events = [
            Op::Rating {
                photo: a.clone(),
                stars: 4,
            },
            Op::Rating {
                photo: b.clone(),
                stars: 2,
            },
            Op::Label {
                photo: a.clone(),
                color: "red".into(),
            },
            Op::Label {
                photo: b.clone(),
                color: "blue".into(),
            },
            // Taken away again.
            Op::Label {
                photo: b.clone(),
                color: String::new(),
            },
        ];
        for (ts, op) in events.into_iter().enumerate() {
            apply_event(
                &tx,
                "device",
                &Event {
                    ts: ts as i64 + 1,
                    op,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let shown = |only: Option<Only>| -> Vec<(String, u8, Option<String>)> {
            let listing = match only {
                Some(only) => Listing::Only {
                    within: Box::new(Listing::Library),
                    only,
                },
                None => Listing::Library,
            };
            photos_page_in(&conn, &listing, Order::NameAsc, 0, 10)
                .unwrap()
                .into_iter()
                .map(|photo| (photo.rel_path, photo.rating, photo.label))
                .collect()
        };
        assert_eq!(
            shown(None),
            [
                ("a.jpg".into(), 4, Some("red".into())),
                ("b.jpg".into(), 2, None)
            ]
        );
        assert_eq!(shown(Some(Only::Stars2)).len(), 2);
        assert_eq!(shown(Some(Only::Stars3))[0].0, "a.jpg");
        assert_eq!(shown(Some(Only::Stars5)).len(), 0);
        assert_eq!(shown(Some(Only::Red)).len(), 1);
        assert_eq!(shown(Some(Only::Blue)).len(), 0);
    }

    #[test]
    fn duplicates_lists_every_copy_of_a_photo_stored_twice() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for (rel_path, fingerprint) in [
            ("a.jpg", "same"),
            ("copy/a.jpg", "same"),
            ("b.jpg", "other"),
        ] {
            upsert(
                &tx,
                &PhotoRow {
                    fingerprint: fingerprint.into(),
                    ..row(rel_path, 10)
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let listed: Vec<String> = photos_page(&conn, &Listing::Duplicates, 0, 10)
            .unwrap()
            .into_iter()
            .map(|photo| photo.rel_path)
            .collect();
        assert_eq!(listed, ["a.jpg", "copy/a.jpg"]);
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
    fn filters_by_camera_lens_aperture_and_focal_length() {
        use crate::metadata::{Media, Shooting};
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for (name, make, model) in [
            ("a.jpg", "Canon", "EOS R6"),
            ("b.jpg", "Canon", "EOS R6"),
            ("c.jpg", "Apple", "iPhone 15"),
        ] {
            upsert(
                &tx,
                &PhotoRow {
                    camera_make: Some(make.into()),
                    camera_model: Some(model.into()),
                    ..row(name, 1)
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let pending = pending_shooting(&conn, 10).unwrap();
        assert_eq!(pending.len(), 3);
        let id = |name: &str| pending.iter().find(|(_, p)| p == name).unwrap().0;
        let shot = |lens: &str, focal: f64, aperture: f64| Shooting {
            lens: Some(lens.into()),
            focal_length: Some(focal),
            aperture: Some(aperture),
            ..Shooting::default()
        };
        set_shooting(
            &mut conn,
            &[
                (
                    id("a.jpg"),
                    Some(shot("RF 24-105mm F4 L", 35.2, 2.83)),
                    Media::default(),
                ),
                (
                    id("b.jpg"),
                    Some(shot("RF 24-105mm F4 L", 105.0, 8.0)),
                    Media::default(),
                ),
                (
                    id("c.jpg"),
                    None,
                    Media {
                        flags: crate::metadata::PORTRAIT,
                        burst: Some("8B005247".into()),
                    },
                ),
            ],
        )
        .unwrap();
        assert!(pending_shooting(&conn, 10).unwrap().is_empty());

        let options = gear_options(&conn, &Listing::Library).unwrap();
        assert_eq!(
            options.cameras[0],
            (Some("Canon".into()), Some("EOS R6".into()), 2)
        );
        assert_eq!(options.lenses, vec![("RF 24-105mm F4 L".to_string(), 2)]);
        assert_eq!(options.apertures, vec![(2.8, 1), (8.0, 1)]);
        assert_eq!(options.focal_lengths, vec![(35.0, 1), (105.0, 1)]);
        assert_eq!(options.unread, 0);

        let names = |gear: Gear| -> Vec<String> {
            let listing = Listing::Gear {
                within: Box::new(Listing::Library),
                gear,
            };
            photos_page_in(&conn, &listing, Order::NameAsc, 0, 10)
                .unwrap()
                .into_iter()
                .map(|photo| photo.rel_path)
                .collect()
        };
        let aperture = Gear {
            aperture: Some(2.8),
            ..Gear::default()
        };
        assert_eq!(names(aperture), ["a.jpg"]);
        let canon_105 = Gear {
            camera_make: Some("Canon".into()),
            camera_model: Some("EOS R6".into()),
            focal_length: Some(105.0),
            ..Gear::default()
        };
        assert_eq!(names(canon_105), ["b.jpg"]);
        // A quote in a name is only text.
        let tricky = Gear {
            lens: Some("x' OR '1'='1".into()),
            ..Gear::default()
        };
        assert!(names(tricky).is_empty());

        // As the front end sends it: the gear next to "kind" and "within".
        let listing: Listing = serde_json::from_str(
            r#"{"kind":"gear","within":{"kind":"library"},"lens":"RF 24-105mm F4 L","aperture":8}"#,
        )
        .unwrap();
        let page = photos_page_in(&conn, &listing, Order::NameAsc, 0, 10).unwrap();
        assert_eq!(page[0].rel_path, "b.jpg");
        assert_eq!(page.len(), 1);

        // The media types the sidebar lists.
        conn.execute(
            "UPDATE photos SET lens = 'iPhone 15 front camera 2.69mm f/1.9' WHERE rel_path = 'c.jpg'",
            [],
        )
        .unwrap();
        let only = |only: Only| -> Vec<String> {
            let listing = Listing::Only {
                within: Box::new(Listing::Library),
                only,
            };
            photos_page(&conn, &listing, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| item.rel_path)
                .collect()
        };
        for kind in [Only::Selfies, Only::Portraits, Only::Bursts] {
            assert_eq!(only(kind), ["c.jpg"], "{kind:?}");
        }
        for kind in [
            Only::SloMos,
            Only::TimeLapses,
            Only::Screenshots,
            Only::Panoramas,
        ] {
            assert!(only(kind).is_empty(), "{kind:?}");
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

        let days = |listing: Listing| -> Vec<(i64, u32)> {
            timeline(&conn, &listing)
                .unwrap()
                .into_iter()
                .map(|g| (g.day.unwrap(), g.count))
                .collect()
        };
        let paths = |listing: Listing, offset, limit| -> Vec<String> {
            photos_page(&conn, &listing, offset, limit)
                .unwrap()
                .into_iter()
                .map(|p| p.rel_path)
                .collect()
        };
        let all = || Listing::Library;
        let in_folder = |folder: &str| Listing::Folder {
            folder: folder.into(),
            subfolders: false,
        };

        assert_eq!(days(all()), [(10, 3), (3, 1), (-1, 1)]);
        assert_eq!(
            paths(all(), 0, 10),
            [
                "trip/c.jpg",
                "a.jpg",
                "b.jpg",
                "trip/d.jpg",
                "trip/deep/e.jpg"
            ]
        );
        assert_eq!(paths(all(), 1, 2), ["a.jpg", "b.jpg"]);
        assert!(paths(all(), 5, 10).is_empty());

        // A folder lists only the photos directly inside it.
        assert_eq!(days(in_folder("trip")), [(10, 1), (3, 1)]);
        assert_eq!(
            paths(in_folder("trip"), 0, 10),
            ["trip/c.jpg", "trip/d.jpg"]
        );
        assert_eq!(paths(in_folder(""), 0, 10), ["a.jpg", "b.jpg"]);

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
        let plan = |listing: Listing| -> String {
            let (filter, param) = listing.filter();
            let sql = format!(
                "EXPLAIN QUERY PLAN SELECT fingerprint FROM photos {filter}
                 ORDER BY sort_date DESC, rel_path LIMIT ?3 OFFSET ?2"
            );
            conn.prepare(&sql)
                .unwrap()
                .query_map(params![param, 0, 10], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ")
        };

        // Both must walk their index in order (no sort step), reading only the
        // index while skipping to an offset.
        let all = plan(Listing::Library);
        assert!(
            all.contains("COVERING INDEX photos_timeline") && !all.contains("TEMP B-TREE"),
            "{all}"
        );
        let one = plan(Listing::Folder {
            folder: "trip".into(),
            subfolders: false,
        });
        assert!(
            one.contains("COVERING INDEX photos_folder") && !one.contains("TEMP B-TREE"),
            "{one}"
        );
    }

    fn video(rel_path: &str, fingerprint: &str, live_candidate: bool) -> PhotoRow {
        PhotoRow {
            fingerprint: fingerprint.into(),
            is_video: true,
            duration_ms: Some(2800),
            live_candidate,
            ..row(rel_path, 1)
        }
    }

    #[test]
    fn live_photos_are_paired_by_name() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("trip/IMG_1.HEIC", 1)).unwrap();
        upsert(&tx, &video("trip/IMG_1.mov", "live1", true)).unwrap();
        // Same name in another folder: not a pair.
        upsert(&tx, &video("other/IMG_1.mov", "elsewhere", true)).unwrap();
        // A normal video that happens to sit next to a photo of the same name.
        upsert(&tx, &row("trip/party.jpg", 1)).unwrap();
        upsert(&tx, &video("trip/party.mp4", "long", false)).unwrap();
        tx.commit().unwrap();

        assert_eq!(pair_parts(&mut conn).unwrap(), 2);
        // Running it again changes nothing.
        assert_eq!(pair_parts(&mut conn).unwrap(), 0);

        let listed: Vec<(String, Option<String>, bool)> =
            photos_page(&conn, &Listing::Library, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| (item.rel_path, item.live_video, item.video))
                .collect();
        assert_eq!(
            listed,
            [
                ("other/IMG_1.mov".to_owned(), None, true),
                (
                    "trip/IMG_1.HEIC".to_owned(),
                    Some("live1".to_owned()),
                    false
                ),
                ("trip/party.jpg".to_owned(), None, false),
                ("trip/party.mp4".to_owned(), None, true),
            ]
        );
        // The Live Photo counts once, everywhere.
        assert_eq!(count(&conn).unwrap(), 4);
        let per_day: u32 = timeline(&conn, &Listing::Library)
            .unwrap()
            .iter()
            .map(|group| group.count)
            .sum();
        assert_eq!(per_day, 4);
        let in_trip = folders(&conn)
            .unwrap()
            .into_iter()
            .find(|entry| entry.folder == "trip")
            .unwrap();
        assert_eq!(in_trip.count, 3);
        // Its video needs no thumbnail of its own.
        assert!(pending_thumbs(&conn)
            .unwrap()
            .iter()
            .all(|pending| pending.rel_path != "trip/IMG_1.mov"));
        let details = photo_details(&conn, "trip/IMG_1.HEIC").unwrap().unwrap();
        assert_eq!(details.live_video.as_deref(), Some("live1"));

        // When the photo goes away, its video becomes a normal video again.
        let tx = conn.transaction().unwrap();
        delete(&tx, "trip/IMG_1.HEIC").unwrap();
        tx.commit().unwrap();
        assert_eq!(pair_parts(&mut conn).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 4);
    }

    #[test]
    fn jpeg_and_raw_are_paired_by_name() {
        let mut conn = open_in_memory().unwrap();
        let photo = |rel_path: &str| PhotoRow {
            fingerprint: rel_path.into(),
            ..row(rel_path, 1)
        };
        let tx = conn.transaction().unwrap();
        upsert(&tx, &photo("match/IMG_1.JPG")).unwrap();
        upsert(&tx, &photo("match/IMG_1.CR2")).unwrap();
        // A RAW file by itself: the JPEG of the same name in another folder
        // was taken at another time.
        upsert(&tx, &photo("match/IMG_2.NEF")).unwrap();
        upsert(
            &tx,
            &PhotoRow {
                sort_date: 99,
                ..photo("other/IMG_2.jpg")
            },
        )
        .unwrap();
        // Two RAW files: neither stands for the other.
        upsert(&tx, &photo("match/IMG_3.cr2")).unwrap();
        upsert(&tx, &photo("match/IMG_3.dng")).unwrap();
        // A JPEG that can't be decoded doesn't hide its RAW file.
        upsert(&tx, &photo("match/IMG_4.jpg")).unwrap();
        upsert(&tx, &photo("match/IMG_4.arw")).unwrap();
        set_thumb_state(&tx, "match/IMG_4.jpg", ThumbState::Failed, None).unwrap();
        tx.commit().unwrap();

        assert_eq!(pair_parts(&mut conn).unwrap(), 2);
        assert_eq!(pair_parts(&mut conn).unwrap(), 0);

        let mut listed: Vec<(String, Option<String>)> =
            photos_page(&conn, &Listing::Library, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| (item.rel_path, item.raw_file))
                .collect();
        listed.sort();
        let pair = |path: &str| (path.to_owned(), None::<String>);
        assert_eq!(
            listed,
            [
                (
                    "match/IMG_1.JPG".to_owned(),
                    Some("match/IMG_1.CR2".to_owned())
                ),
                pair("match/IMG_2.NEF"),
                pair("match/IMG_3.cr2"),
                pair("match/IMG_3.dng"),
                pair("match/IMG_4.arw"),
                pair("other/IMG_2.jpg"),
            ]
        );

        // The RAW filter keeps the pairs and the RAW files on their own.
        let raw = Listing::Only {
            within: Box::new(Listing::Library),
            only: Only::Raw,
        };
        let mut raws: Vec<String> = photos_page(&conn, &raw, 0, 10)
            .unwrap()
            .into_iter()
            .map(|item| item.rel_path)
            .collect();
        raws.sort();
        assert_eq!(
            raws,
            [
                "match/IMG_1.JPG",
                "match/IMG_2.NEF",
                "match/IMG_3.cr2",
                "match/IMG_3.dng",
                "match/IMG_4.arw"
            ]
        );
        // The pair counts once (the count includes the undecodable JPEG),
        // and its RAW file needs no thumbnail of its own.
        assert_eq!(count(&conn).unwrap(), 7);
        assert!(pending_thumbs(&conn)
            .unwrap()
            .iter()
            .all(|pending| pending.rel_path != "match/IMG_1.CR2"));
        let details = photo_details(&conn, "match/IMG_1.JPG").unwrap().unwrap();
        assert_eq!(details.raw_path.as_deref(), Some("match/IMG_1.CR2"));
        // Both files of the pair stand for the JPEG (the Apple Photos import
        // may find either).
        let as_listed: HashMap<String, String> = files(&conn)
            .unwrap()
            .into_iter()
            .map(|(rel_path, _, fingerprint)| (rel_path, fingerprint))
            .collect();
        assert_eq!(as_listed["match/IMG_1.CR2"], "match/IMG_1.JPG");
        assert_eq!(as_listed["match/IMG_2.NEF"], "match/IMG_2.NEF");

        // When the JPEG goes away, the RAW file is listed by itself again.
        let tx = conn.transaction().unwrap();
        delete(&tx, "match/IMG_1.JPG").unwrap();
        tx.commit().unwrap();
        assert_eq!(pair_parts(&mut conn).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 7);
    }

    #[test]
    fn raw_files_in_another_folder_pair_by_name_and_capture_time() {
        let mut conn = open_in_memory().unwrap();
        let shot = |rel_path: &str, sort_date: i64, exif: bool| PhotoRow {
            fingerprint: rel_path.into(),
            date_from_exif: exif,
            ..dated(rel_path, sort_date)
        };
        let tx = conn.transaction().unwrap();
        // Olivier's layout: the JPEGs in a folder, their RAW files in `cr2/`.
        upsert(&tx, &shot("print/IMG_9313.JPG", 1_000, true)).unwrap();
        upsert(&tx, &shot("print/cr2/IMG_9313.CR2", 1_000, true)).unwrap();
        // The same name ten thousand shots later, taken at another time.
        upsert(&tx, &shot("2023/IMG_9313.CR2", 9_000, true)).unwrap();
        // Same name and time, but the time comes from the file, not EXIF.
        upsert(&tx, &shot("scans/IMG_7.jpg", 5_000, false)).unwrap();
        upsert(&tx, &shot("scans/raw/IMG_7.dng", 5_000, false)).unwrap();
        // Same time, different names.
        upsert(&tx, &shot("print/IMG_9314.JPG", 2_000, true)).unwrap();
        upsert(&tx, &shot("print/cr2/IMG_9315.CR2", 2_000, true)).unwrap();
        tx.commit().unwrap();

        assert_eq!(pair_parts(&mut conn).unwrap(), 2);
        let mut listed: Vec<(String, Option<String>)> =
            photos_page(&conn, &Listing::Library, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| (item.rel_path, item.raw_file))
                .collect();
        listed.sort();
        let alone = |path: &str| (path.to_owned(), None::<String>);
        assert_eq!(
            listed,
            [
                alone("2023/IMG_9313.CR2"),
                (
                    "print/IMG_9313.JPG".to_owned(),
                    Some("print/cr2/IMG_9313.CR2".to_owned())
                ),
                alone("print/IMG_9314.JPG"),
                alone("print/cr2/IMG_9315.CR2"),
                alone("scans/IMG_7.jpg"),
                alone("scans/raw/IMG_7.dng"),
            ]
        );
        let details = photo_details(&conn, "print/IMG_9313.JPG").unwrap().unwrap();
        assert_eq!(details.raw_path.as_deref(), Some("print/cr2/IMG_9313.CR2"));
        let as_listed: HashMap<String, String> = files(&conn)
            .unwrap()
            .into_iter()
            .map(|(rel_path, _, fingerprint)| (rel_path, fingerprint))
            .collect();
        assert_eq!(as_listed["print/cr2/IMG_9313.CR2"], "print/IMG_9313.JPG");

        // Neither pairing compares every photo with every other one.
        let plan: Vec<String> = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT i.id FROM photos r JOIN photos i
                     ON i.sort_date = r.sort_date AND i.thumb_state != 2 AND i.part = 0
                 WHERE {}",
                crate::raw::is_raw_sql("r.rel_path")
            ))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains("SEARCH i USING")),
            "{plan:?}"
        );
    }

    #[test]
    fn area_listing_and_geo_points() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        let at = |rel_path: &str, latitude: f64, longitude: f64| PhotoRow {
            fingerprint: rel_path.into(),
            latitude: Some(latitude),
            longitude: Some(longitude),
            ..row(rel_path, 1)
        };
        upsert(&tx, &at("luxembourg.jpg", 49.61, 6.13)).unwrap();
        upsert(&tx, &at("paris.jpg", 48.86, 2.35)).unwrap();
        upsert(&tx, &at("sydney.jpg", -33.87, 151.21)).unwrap();
        let nowhere = PhotoRow {
            latitude: None,
            longitude: None,
            ..row("nowhere.jpg", 1)
        };
        upsert(&tx, &nowhere).unwrap();
        tx.commit().unwrap();

        let europe = Listing::Area {
            south: 40.0,
            west: -5.0,
            north: 55.0,
            east: 10.0,
        };
        let mut names: Vec<String> = photos_page(&conn, &europe, 0, 10)
            .unwrap()
            .into_iter()
            .map(|item| item.rel_path)
            .collect();
        names.sort();
        assert_eq!(names, ["luxembourg.jpg", "paris.jpg"]);
        assert_eq!(timeline(&conn, &europe).unwrap()[0].count, 2);
        assert_eq!(
            geo_points(&conn, -90.0, -180.0, 90.0, 180.0).unwrap().len(),
            3
        );
        assert_eq!(
            geo_points(&conn, -40.0, 100.0, 0.0, 180.0).unwrap().len(),
            1
        );

        let broken = Listing::Area {
            south: f64::NAN,
            west: 0.0,
            north: 1.0,
            east: 1.0,
        };
        assert!(photos_page(&conn, &broken, 0, 10).unwrap().is_empty());
    }

    #[test]
    fn months_have_their_busiest_days() {
        const DAY: i64 = 86_400_000;
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        // 2024-07-14 and 2023-12-31 as wall-clock milliseconds.
        let july_14 = 1_720_915_200_000;
        let december = 1_703_980_800_000;
        // July: day 14 has 4 photos, days 1, 2, 3 and 20 have fewer.
        let add = |name: &str, date: i64, video: bool| {
            let mut photo = dated(name, date);
            photo.fingerprint = name.into();
            photo.is_video = video;
            upsert(&tx, &photo).unwrap();
        };
        for n in 0..4 {
            add(&format!("j14-{n}"), july_14 + n * 1000, false);
        }
        add("j01-video", july_14 - 13 * DAY, true);
        add("j02", july_14 - 12 * DAY, false);
        add("j02-video", july_14 - 12 * DAY + 1000, true);
        add("j03", july_14 - 11 * DAY, false);
        add("j20-a", july_14 + 6 * DAY, false);
        add("j20-b", july_14 + 6 * DAY + 1000, false);
        add("december", december, false);
        let fav = Event {
            ts: 1,
            op: Op::Fav {
                photo: "j14-2".into(),
                on: true,
            },
        };
        apply_event(&tx, "device", &fav).unwrap();
        tx.commit().unwrap();

        let months = months(&conn, &Listing::Library).unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(
            (months[0].year, months[0].month, months[0].count),
            (2024, 7, 10)
        );
        let days: Vec<(i64, u32, &str)> = months[0]
            .days
            .iter()
            .map(|day| (day.day - july_14 / DAY, day.count, day.cover.as_str()))
            .collect();
        // The four busiest days, in date order. Day 2 (two files) beats days
        // 1 and 3 (one each); of those, the later one is kept. A favorite
        // stands for its day, and a photo rather than a video.
        assert_eq!(
            days,
            [
                (-12, 2, "j02"),
                (-11, 1, "j03"),
                (0, 4, "j14-2"),
                (6, 2, "j20-a")
            ]
        );
        assert_eq!(
            months[1],
            MonthSummary {
                year: 2023,
                month: 12,
                count: 1,
                days: vec![DaySummary {
                    day: december / DAY,
                    count: 1,
                    cover: "december".into(),
                }],
            }
        );
        assert!(super::months(&conn, &Listing::Favorites).unwrap()[0].count == 1);
    }

    #[test]
    fn shapes_follow_the_grid_order_and_the_edits() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        let photo = |path: &str, date: i64, size: Option<(u32, u32)>| PhotoRow {
            fingerprint: path.into(),
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            ..dated(path, date)
        };
        upsert(&tx, &photo("wide.jpg", 3, Some((4000, 3000)))).unwrap();
        upsert(&tx, &photo("tall.jpg", 2, Some((3000, 4000)))).unwrap();
        upsert(&tx, &photo("unknown.jpg", 1, None)).unwrap();
        upsert(&tx, &photo("panorama.jpg", 0, Some((12000, 1000)))).unwrap();
        // Turned a quarter, then cropped to a square of the turned picture's
        // full height: 3000 wide by 4000 high, keeping 3/4 of the height.
        apply_event(
            &tx,
            "device",
            &Event {
                ts: 1,
                op: Op::Edit {
                    photo: "wide.jpg".into(),
                    edit: Some(crate::org::Edit {
                        rotate: 90,
                        crop: Some([0, 0, 10_000, 7_500]),
                        ..crate::org::Edit::default()
                    }),
                },
            },
        )
        .unwrap();
        tx.commit().unwrap();

        assert_eq!(
            shapes(&conn, &Listing::Library, Order::DateDesc).unwrap(),
            [1000, 750, 1500, 4000]
        );
        assert_eq!(
            shapes(&conn, &Listing::Library, Order::DateAsc).unwrap(),
            [4000, 1500, 750, 1000]
        );
    }

    #[test]
    fn listings_can_be_sorted_by_date_or_name() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        // Two days; names that sort differently from dates and folders.
        for (path, date) in [
            ("b/IMG_2.jpg", 86_400_000),
            ("a/img_3.jpg", 86_400_000 + 5),
            ("c/IMG_1.jpg", 3 * 86_400_000),
        ] {
            upsert(
                &tx,
                &PhotoRow {
                    fingerprint: path.into(),
                    ..dated(path, date)
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let listed = |order: Order| -> (Vec<String>, Vec<(Option<i64>, u32)>) {
            let paths = photos_page_in(&conn, &Listing::Library, order, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| item.rel_path)
                .collect();
            let groups = timeline_in(&conn, &Listing::Library, order)
                .unwrap()
                .into_iter()
                .map(|group| (group.day, group.count))
                .collect();
            (paths, groups)
        };
        assert_eq!(
            listed(Order::DateDesc),
            (
                vec![
                    "c/IMG_1.jpg".into(),
                    "a/img_3.jpg".into(),
                    "b/IMG_2.jpg".into()
                ],
                vec![(Some(3), 1), (Some(1), 2)]
            )
        );
        assert_eq!(
            listed(Order::DateAsc),
            (
                vec![
                    "b/IMG_2.jpg".into(),
                    "a/img_3.jpg".into(),
                    "c/IMG_1.jpg".into()
                ],
                vec![(Some(1), 2), (Some(3), 1)]
            )
        );
        // By file name, whatever the folder and the case.
        assert_eq!(
            listed(Order::NameAsc),
            (
                vec![
                    "c/IMG_1.jpg".into(),
                    "b/IMG_2.jpg".into(),
                    "a/img_3.jpg".into()
                ],
                vec![(None, 3)]
            )
        );
        assert_eq!(listed(Order::NameDesc).0[0], "a/img_3.jpg");

        // Oldest first still walks the timeline index, backwards.
        let (filter, _) = Listing::Library.filter();
        let plan: Vec<String> = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT fingerprint FROM photos {filter}
                 ORDER BY {} LIMIT 10",
                Order::DateAsc.sql()
            ))
            .unwrap()
            .query_map([None::<&str>], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let plan = plan.join(" | ");
        assert!(
            plan.contains("photos_timeline") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
    }

    #[test]
    fn a_folder_can_be_listed_with_its_subfolders() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for path in [
            "trip/a.jpg",
            "trip/day 1/b.jpg",
            "trip/day 1/deeper/c.jpg",
            "tripod/d.jpg",
            "trip_x/e.jpg",
            "f.jpg",
        ] {
            upsert(
                &tx,
                &PhotoRow {
                    fingerprint: path.into(),
                    ..row(path, 1)
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let listed = |folder: &str, subfolders: bool| -> Vec<String> {
            let listing = Listing::Folder {
                folder: folder.into(),
                subfolders,
            };
            let mut paths: Vec<String> = photos_page(&conn, &listing, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| item.rel_path)
                .collect();
            paths.sort();
            paths
        };
        assert_eq!(listed("trip", false), ["trip/a.jpg"]);
        assert_eq!(
            listed("trip", true),
            ["trip/a.jpg", "trip/day 1/b.jpg", "trip/day 1/deeper/c.jpg"]
        );
        assert_eq!(
            listed("trip/day 1", true),
            ["trip/day 1/b.jpg", "trip/day 1/deeper/c.jpg"]
        );
        assert_eq!(listed("trip_x", true), ["trip_x/e.jpg"]);
        assert_eq!(listed("", false), ["f.jpg"]);
    }

    #[test]
    fn listings_can_be_narrowed_down() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for name in [
            "fav.jpg",
            "edited.jpg",
            "in-album.jpg",
            "in-deleted-album.jpg",
        ] {
            upsert(
                &tx,
                &PhotoRow {
                    fingerprint: name.into(),
                    ..row(name, 1)
                },
            )
            .unwrap();
        }
        let mut clip = video("clip.mov", "clip.mov", false);
        clip.duration_ms = Some(60_000);
        upsert(&tx, &clip).unwrap();
        let ops = [
            Op::Fav {
                photo: "fav.jpg".into(),
                on: true,
            },
            Op::Edit {
                photo: "edited.jpg".into(),
                edit: Some(crate::org::Edit {
                    rotate: 90,
                    ..crate::org::Edit::default()
                }),
            },
            Op::AlbumMember {
                album: "trip".into(),
                photo: "in-album.jpg".into(),
                on: true,
            },
            Op::AlbumMember {
                album: "trip".into(),
                photo: "clip.mov".into(),
                on: true,
            },
            Op::AlbumMember {
                album: "gone".into(),
                photo: "in-deleted-album.jpg".into(),
                on: true,
            },
            Op::AlbumDeleted {
                album: "gone".into(),
                on: true,
            },
        ];
        for op in ops {
            apply_event(&tx, "device", &Event { ts: 1, op }).unwrap();
        }
        tx.commit().unwrap();

        let listed = |within: Listing, only: Only| -> Vec<String> {
            let listing = Listing::Only {
                within: Box::new(within),
                only,
            };
            let mut paths: Vec<String> = photos_page(&conn, &listing, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| item.rel_path)
                .collect();
            paths.sort();
            // The other queries of a listing take it too.
            let per_day: u32 = timeline(&conn, &listing)
                .unwrap()
                .iter()
                .map(|g| g.count)
                .sum();
            assert_eq!(per_day as usize, paths.len());
            paths
        };
        assert_eq!(listed(Listing::Library, Only::Favorites), ["fav.jpg"]);
        assert_eq!(listed(Listing::Library, Only::Edited), ["edited.jpg"]);
        assert_eq!(listed(Listing::Library, Only::Videos), ["clip.mov"]);
        assert_eq!(listed(Listing::Library, Only::Photos).len(), 4);
        assert_eq!(
            listed(Listing::Library, Only::NotInAlbum),
            ["edited.jpg", "fav.jpg", "in-deleted-album.jpg"]
        );
        let trip = || Listing::Album {
            album: "trip".into(),
        };
        assert_eq!(listed(trip(), Only::Photos), ["in-album.jpg"]);
        assert_eq!(listed(trip(), Only::Videos), ["clip.mov"]);
        // The timeline tells the videos apart, for "4 photos, 1 video".
        let videos: u32 = timeline(&conn, &Listing::Library)
            .unwrap()
            .iter()
            .map(|g| g.videos)
            .sum();
        assert_eq!(videos, 1);
    }

    #[test]
    fn search_by_words_dates_and_places() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        // 2024-07-14 and 2023-12-31 as wall-clock milliseconds.
        let mut paris = dated("trips/eiffel.jpg", 1_720_915_200_000);
        paris.fingerprint = "p".into();
        paris.latitude = Some(48.8584);
        paris.longitude = Some(2.2945);
        upsert(&tx, &paris).unwrap();
        let mut zurich = dated("trips/lake.heic", 1_703_980_800_000);
        zurich.fingerprint = "z".into();
        zurich.latitude = Some(30.0); // nowhere near a town
        zurich.camera_make = Some("Apple".into());
        zurich.camera_model = Some("iPhone 15".into());
        upsert(&tx, &zurich).unwrap();
        let mut clip = video("2024 clips/party.mp4", "v", false);
        clip.sort_date = 1_600_000_000_000;
        clip.latitude = None;
        upsert(&tx, &clip).unwrap();
        tx.commit().unwrap();

        let looked_up = fill_places(&mut conn, |latitude, _| {
            (latitude > 48.0).then(|| ("Paris, France".to_owned(), "paris france".to_owned()))
        })
        .unwrap();
        assert_eq!(looked_up, 2);
        // Nothing is looked up twice, found or not.
        assert_eq!(fill_places(&mut conn, |_, _| panic!("again")).unwrap(), 0);

        let find = |query: &str| {
            let listing = Listing::Search {
                query: query.into(),
            };
            let mut names: Vec<String> = photos_page(&conn, &listing, 0, 10)
                .unwrap()
                .into_iter()
                .map(|item| item.rel_path)
                .collect();
            names.sort();
            names.join(" ")
        };
        assert_eq!(find("paris"), "trips/eiffel.jpg");
        assert_eq!(find("FRANCE eiffel"), "trips/eiffel.jpg");
        assert_eq!(find("iphone"), "trips/lake.heic");
        assert_eq!(find("canon trips"), "trips/eiffel.jpg");
        // A year matches the date taken, or the text.
        assert_eq!(find("2024"), "2024 clips/party.mp4 trips/eiffel.jpg");
        assert_eq!(find("2023-12"), "trips/lake.heic");
        assert_eq!(find("july"), "trips/eiffel.jpg");
        assert_eq!(find("video"), "2024 clips/party.mp4");
        assert_eq!(find("trips video"), "");
        assert_eq!(find("100%"), "");
        assert_eq!(find(""), "");

        let details = photo_details(&conn, "trips/eiffel.jpg").unwrap().unwrap();
        assert_eq!(details.place.as_deref(), Some("Paris, France"));
        let nowhere = photo_details(&conn, "trips/lake.heic").unwrap().unwrap();
        assert_eq!(nowhere.place, None);
    }

    #[test]
    fn versions_5_and_6_are_upgraded_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let mut conn = open(&path).unwrap();
            let tx = conn.transaction().unwrap();
            upsert(&tx, &row("a.jpg", 1)).unwrap();
            tx.commit().unwrap();
            // Make it look like a version 5 index.
            conn.execute_batch(
                "ALTER TABLE photos DROP COLUMN place;
                 ALTER TABLE photos DROP COLUMN place_search;
                 ALTER TABLE photos DROP COLUMN raw_file;
                 ALTER TABLE photos RENAME COLUMN part TO live_part;
                 ALTER TABLE photos DROP COLUMN shot_read;
                 ALTER TABLE photos DROP COLUMN lens;
                 ALTER TABLE photos DROP COLUMN focal_length;
                 ALTER TABLE photos DROP COLUMN focal_35mm;
                 ALTER TABLE photos DROP COLUMN aperture;
                 ALTER TABLE photos DROP COLUMN exposure;
                 ALTER TABLE photos DROP COLUMN iso;
                 ALTER TABLE photos DROP COLUMN media;
                 ALTER TABLE photos DROP COLUMN burst;
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        }
        let mut conn = open(&path).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
        assert_eq!(fill_places(&mut conn, |_, _| None).unwrap(), 1);
        assert_eq!(pair_parts(&mut conn).unwrap(), 0);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn edits_are_stored_and_listed() {
        use crate::org::Edit;

        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert(&tx, &row("a.jpg", 1)).unwrap();
        let wanted = Edit {
            rotate: 90,
            crop: Some([1000, 0, 5000, 10_000]),
            contrast: 20,
            ..Edit::default()
        };
        let edit = |ts, edit| Event {
            ts,
            op: Op::Edit {
                photo: "f".into(),
                edit,
            },
        };
        apply_event(&tx, "device", &edit(5, Some(wanted))).unwrap();
        // An older event from another computer does not win.
        apply_event(&tx, "other", &edit(3, None)).unwrap();
        tx.commit().unwrap();
        let listed = photos_page(&conn, &Listing::Library, 0, 1).unwrap();
        assert_eq!(listed[0].edit, Some(wanted));

        // A newer "no edit" shows the photo as it is again.
        let tx = conn.transaction().unwrap();
        apply_event(&tx, "other", &edit(9, None)).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            photos_page(&conn, &Listing::Library, 0, 1).unwrap()[0].edit,
            None
        );

        // What the log looks like, and what is refused.
        let line = serde_json::to_string(&edit(5, Some(wanted))).unwrap();
        assert!(
            line.contains(r#""op":"edit""#) && line.contains(r#""rotate":90"#),
            "{line}"
        );
        let op = |edit| Op::Edit {
            photo: "ab".repeat(32),
            edit: Some(edit),
        };
        assert!(op(wanted).is_valid());
        for bad in [
            Edit {
                rotate: 45,
                ..wanted
            },
            Edit {
                crop: Some([6000, 0, 5000, 100]),
                ..wanted
            },
            Edit {
                crop: Some([0, 0, 10, 10]),
                ..wanted
            },
            Edit {
                brightness: 120,
                ..wanted
            },
        ] {
            assert!(!op(bad).is_valid(), "{bad:?}");
        }
    }

    #[test]
    fn stems() {
        assert_eq!(stem_of("2024/Summer/IMG_1.HEIC"), "2024/summer/img_1");
        assert_eq!(stem_of("IMG_1.mov"), "img_1");
        assert_eq!(stem_of("a.b/noext"), "a.b/noext");
        assert_eq!(stem_of("dir/.hidden"), "dir/.hidden");
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
