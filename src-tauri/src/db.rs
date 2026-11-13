//! The local index: one SQLite database per library, in the OS app-data dir.
//!
//! It is a cache of what is in the library folder and can always be rebuilt by
//! rescanning. It never lives on the library volume itself.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::org::{Event, Op};

const SCHEMA_VERSION: i64 = 5;

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
    -- 1 on the video of a Live Photo. It is shown through its photo, never
    -- listed by itself.
    live_part     INTEGER NOT NULL DEFAULT 0
);
-- The order the timeline and the folder view list photos in. Partial, to match
-- their WHERE clause, so skipping to a far offset reads only the index.
-- (fingerprint, thumb_state and live_part are included only so that the filter
-- can be evaluated from the index alone.)
CREATE INDEX photos_timeline
    ON photos (sort_date DESC, rel_path, fingerprint, thumb_state, live_part)
    WHERE thumb_state != 2 AND live_part = 0;
CREATE INDEX photos_folder
    ON photos (folder, sort_date DESC, rel_path, fingerprint, thumb_state, live_part)
    WHERE thumb_state != 2 AND live_part = 0;
CREATE INDEX photos_stem ON photos (stem);

-- The merged organization (favorites, hidden, albums): one row per value that
-- the device logs set, holding the winning event. See org.rs.
--   kind            key        photo      value
--   'fav'           photo fp   ''         '1' / '0'
--   'hide'          photo fp   ''         '1' / '0'
--   'album_name'    album id   ''         the name
--   'album_deleted' album id   ''         '1' / '0'
--   'member'        album id   photo fp   '1' / '0'
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
         DROP TABLE IF EXISTS org_state;
         DROP TABLE IF EXISTS log_offsets;
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
             live_candidate = excluded.live_candidate",
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
         WHERE thumb_state = 0 AND live_part = 0 ORDER BY sort_date DESC",
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

/// Pairs each Live Photo's video with its photo: a short (or Apple-tagged)
/// video next to a photo with the same name. The video then disappears from
/// the listings and the photo gets a link to it. Returns how many rows changed.
pub fn pair_live_photos(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS live_pairs (
             image_id INTEGER PRIMARY KEY, video_id INTEGER, video_fp TEXT);
         DELETE FROM live_pairs;
         INSERT OR REPLACE INTO live_pairs
             SELECT i.id, v.id, v.fingerprint
             FROM photos v JOIN photos i ON i.stem = v.stem AND i.kind = 0
             WHERE v.kind = 1 AND v.live_candidate = 1
             ORDER BY v.rel_path DESC;",
    )?;
    let mut changed = 0;
    // Only rows whose pairing really changed are written.
    for update in [
        "UPDATE photos SET live_video = NULL
         WHERE live_video IS NOT NULL AND id NOT IN (SELECT image_id FROM live_pairs)",
        "UPDATE photos SET live_part = 0
         WHERE live_part = 1 AND id NOT IN (SELECT video_id FROM live_pairs)",
        "UPDATE photos
         SET live_video = (SELECT video_fp FROM live_pairs WHERE image_id = photos.id)
         WHERE id IN (SELECT image_id FROM live_pairs)
           AND live_video IS NOT (SELECT video_fp FROM live_pairs WHERE image_id = photos.id)",
        "UPDATE photos SET live_part = 1
         WHERE live_part = 0 AND id IN (SELECT video_id FROM live_pairs)",
    ] {
        changed += tx.execute(update, [])?;
    }
    tx.commit()?;
    Ok(changed)
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
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Listing {
    /// Every photo that is not hidden.
    Library,
    /// The photos directly inside one folder.
    Folder {
        folder: String,
    },
    Favorites,
    Album {
        album: String,
    },
    Hidden,
    /// The photos taken inside a rectangle on the map.
    Area {
        south: f64,
        west: f64,
        north: f64,
        east: f64,
    },
}

const IS_HIDDEN: &str =
    "fingerprint IN (SELECT key FROM org_state WHERE kind = 'hide' AND value = '1')";
const IS_FAVORITE: &str =
    "fingerprint IN (SELECT key FROM org_state WHERE kind = 'fav' AND value = '1')";
/// Rows that can be listed: decodable, and not the video half of a Live Photo.
const IS_LISTED: &str = "thumb_state != 2 AND live_part = 0";

impl Listing {
    /// The `WHERE` clause and its one parameter (`?1`, NULL when unused).
    /// Undecodable files and Live Photo videos are indexed but never listed,
    /// and hidden photos appear only in the "Hidden" listing.
    fn filter(&self) -> (String, Option<&str>) {
        let visible = format!("{IS_LISTED} AND NOT {IS_HIDDEN}");
        match self {
            Listing::Library => (format!("WHERE {visible} AND ?1 IS NULL"), None),
            Listing::Folder { folder } => {
                (format!("WHERE {visible} AND folder = ?1"), Some(folder))
            }
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
        }
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
pub fn timeline(conn: &Connection, listing: &Listing) -> rusqlite::Result<Vec<DayGroup>> {
    let (filter, param) = listing.filter();
    // Integer division truncates toward zero, so shift pre-1970 dates down a
    // day first to get a floor.
    let mut stmt = conn.prepare(&format!(
        "SELECT (sort_date - 86400000 * (sort_date < 0 AND sort_date % 86400000 != 0))
                    / 86400000 AS day,
                COUNT(*)
         FROM photos {filter} GROUP BY day ORDER BY day DESC"
    ))?;
    let rows = stmt.query_map([param], |row| {
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
    pub favorite: bool,
    pub video: bool,
    pub duration_ms: Option<u64>,
    /// For a Live Photo: the fingerprint of its video.
    pub live_video: Option<String>,
}

/// A slice of the listing, newest first (ties broken by path).
pub fn photos_page(
    conn: &Connection,
    listing: &Listing,
    offset: u32,
    limit: u32,
) -> rusqlite::Result<Vec<PhotoItem>> {
    let (filter, param) = listing.filter();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT fingerprint, rel_path, sort_date, width, height, {IS_FAVORITE},
                kind, duration_ms, live_video
         FROM photos {filter} ORDER BY sort_date DESC, rel_path LIMIT ?3 OFFSET ?2"
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
}

pub fn photo_details(conn: &Connection, rel_path: &str) -> rusqlite::Result<Option<PhotoDetails>> {
    let details = conn
        .query_row(
            &format!(
                "SELECT rel_path, fingerprint, size, sort_date, date_source, width, height,
                        camera_make, camera_model, latitude, longitude,
                        {IS_FAVORITE}, {IS_HIDDEN}, kind, duration_ms, live_video
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

// --- Organization (see org.rs) ------------------------------------------------

/// Applies one event from a device log: sets the value it names, unless a
/// later event (greater `(ts, device)`) already did.
pub fn apply_event(tx: &Transaction<'_>, device: &str, event: &Event) -> rusqlite::Result<()> {
    let flag = |on: &bool| if *on { "1" } else { "0" };
    let (kind, key, photo, value) = match &event.op {
        Op::Fav { photo, on } => ("fav", photo.as_str(), "", flag(on)),
        Op::Hide { photo, on } => ("hide", photo.as_str(), "", flag(on)),
        Op::AlbumName { album, name } => ("album_name", album.as_str(), "", name.trim()),
        Op::AlbumDeleted { album, on } => ("album_deleted", album.as_str(), "", flag(on)),
        Op::AlbumMember { album, photo, on } => {
            ("member", album.as_str(), photo.as_str(), flag(on))
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
}

/// The albums that have not been deleted, by name.
pub fn albums(conn: &Connection) -> rusqlite::Result<Vec<Album>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT a.key, a.value,
                (SELECT COUNT(*) FROM photos
                 WHERE {IS_LISTED} AND NOT {IS_HIDDEN}
                   AND fingerprint IN (SELECT photo FROM org_state
                                       WHERE kind = 'member' AND key = a.key AND value = '1'))
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
        })
    })?;
    rows.collect()
}

/// How many photos and videos are indexed. A Live Photo counts once.
pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row(
        "SELECT COUNT(*) FROM photos WHERE live_part = 0",
        [],
        |row| row.get::<_, i64>(0).map(|n| n as u64),
    )
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

        let days = |listing: Listing| -> Vec<(i64, u32)> {
            timeline(&conn, &listing)
                .unwrap()
                .into_iter()
                .map(|g| (g.day, g.count))
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

        assert_eq!(pair_live_photos(&mut conn).unwrap(), 2);
        // Running it again changes nothing.
        assert_eq!(pair_live_photos(&mut conn).unwrap(), 0);

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
        assert_eq!(pair_live_photos(&mut conn).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 4);
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
