//! The local index: one SQLite database per library, in the OS app-data dir.
//!
//! It is a cache of what is in the library folder and can always be rebuilt by
//! rescanning. It never lives on the library volume itself.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::org::{Event, Op};

const SCHEMA_VERSION: i64 = 7;

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
    raw_file      TEXT
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
    -- The group of look-alike faces this one was put in; NULL until grouped.
    cluster     INTEGER
);
CREATE INDEX IF NOT EXISTS faces.face_fingerprint ON face (fingerprint);
CREATE INDEX IF NOT EXISTS faces.face_cluster ON face (cluster);
";

fn attach_faces(conn: &Connection, file: &str) -> rusqlite::Result<()> {
    conn.execute("ATTACH DATABASE ?1 AS faces", [file])?;
    if file != ":memory:" {
        // The statement answers with the new mode, hence a query.
        conn.query_row("PRAGMA faces.journal_mode = WAL", [], |_| Ok(()))?;
    }
    conn.execute_batch(FACES_SCHEMA)
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
             live_candidate = excluded.live_candidate,
             -- A new position needs its place looked up again.
             place = CASE WHEN photos.latitude IS excluded.latitude
                           AND photos.longitude IS excluded.longitude
                          THEN photos.place END,
             place_search = CASE WHEN photos.latitude IS excluded.latitude
                                  AND photos.longitude IS excluded.longitude
                                 THEN photos.place_search END",
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
    let tx = conn.transaction()?;
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

/// Pairs the files that are shown through another one, and returns how many
/// rows changed:
/// - a Live Photo's video, a short (or Apple-tagged) video next to a photo
///   with the same name;
/// - a RAW file next to a JPEG (or HEIC, PNG...) with the same name, which the
///   camera saved together. The JPEG is shown, as cameras and Apple Photos do.
///
/// The second file then disappears from the listings, and the photo gets a
/// link to it.
pub fn pair_parts(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
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
         INSERT OR REPLACE INTO raw_pairs
             SELECT i.id, r.id, r.fingerprint
             FROM photos r JOIN photos i ON i.stem = r.stem AND i.kind = 0
                 AND i.thumb_state != 2 AND NOT {image_is_raw}
             WHERE r.kind = 0 AND {raw}
             ORDER BY r.rel_path DESC;",
        image_is_raw = crate::raw::is_raw_sql("i.rel_path"),
        raw = crate::raw::is_raw_sql("r.rel_path"),
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
}

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

/// One month of a listing, for the Years and Months overviews.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthSummary {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    pub count: u32,
    /// A few photos to show for the month: favorites first, then a sample
    /// spread over the whole month.
    pub covers: Vec<String>,
}

/// How many cover photos each month gets.
const MONTH_COVERS: u32 = 7;

/// The months of a listing, newest first.
pub fn months(conn: &Connection, listing: &Listing) -> rusqlite::Result<Vec<MonthSummary>> {
    let (filter, param) = listing.filter();
    // Ordering by fingerprint (a hash) picks the same random-looking sample
    // every time, instead of seven shots of the month's last event.
    let mut stmt = conn.prepare(&format!(
        "SELECT ym, total, fingerprint FROM (
             SELECT strftime('%Y-%m', sort_date / 1000, 'unixepoch') AS ym,
                    COUNT(*) OVER (PARTITION BY strftime('%Y-%m', sort_date / 1000, 'unixepoch'))
                        AS total,
                    ROW_NUMBER() OVER (
                        PARTITION BY strftime('%Y-%m', sort_date / 1000, 'unixepoch')
                        ORDER BY {IS_FAVORITE} DESC, kind, fingerprint) AS position,
                    fingerprint
             FROM photos {filter})
         WHERE position <= {MONTH_COVERS}
         ORDER BY ym DESC, position"
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
        let fingerprint: String = row.get(2)?;
        match months.last_mut() {
            Some(last) if last.year == year && last.month == month => last.covers.push(fingerprint),
            _ => months.push(MonthSummary {
                year,
                month,
                count: row.get(1)?,
                covers: vec![fingerprint],
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
                kind, duration_ms, live_video,
                (SELECT value FROM org_state WHERE kind = 'edit' AND key = photos.fingerprint),
                raw_file
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
            edit: row
                .get::<_, Option<String>>(9)?
                .and_then(|json| serde_json::from_str(&json).ok()),
            raw_file: row.get(10)?,
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
                         WHERE r.stem = photos.stem AND r.fingerprint = photos.raw_file)
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
    let mut stmt = conn.prepare(
        "SELECT p.rel_path, p.size, COALESCE(main.fingerprint, p.fingerprint)
         FROM photos p LEFT JOIN photos main
             ON p.part = 1 AND main.stem = p.stem AND main.part = 0
            AND (main.raw_file = p.fingerprint OR main.live_video = p.fingerprint)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get::<_, i64>(1)? as u64, row.get(2)?))
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

/// Every face, for grouping.
pub fn face_embeddings(conn: &Connection) -> rusqlite::Result<Vec<StoredFace>> {
    let mut stmt = conn.prepare("SELECT id, cluster, embedding FROM faces.face ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        let bytes: Vec<u8> = row.get(2)?;
        let embedding = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        Ok((row.get(0)?, row.get(1)?, embedding))
    })?;
    rows.collect()
}

pub fn set_face_clusters(conn: &mut Connection, groups: &[(i64, i64)]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
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
         WHERE f.cluster IS NOT NULL
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
         WHERE fingerprint = ?1 AND cluster IS NOT NULL
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
        // A RAW file by itself, and one whose JPEG is in another folder.
        upsert(&tx, &photo("match/IMG_2.NEF")).unwrap();
        upsert(&tx, &photo("other/IMG_2.jpg")).unwrap();
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

        let listed: Vec<(String, Option<String>)> = photos_page(&conn, &Listing::Library, 0, 10)
            .unwrap()
            .into_iter()
            .map(|item| (item.rel_path, item.raw_file))
            .collect();
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
    fn months_have_counts_and_covers() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        // 2024-07-14 and 2023-12-31 as wall-clock milliseconds.
        let july = 1_720_915_200_000;
        let december = 1_703_980_800_000;
        for n in 0..10 {
            let mut photo = dated(&format!("july-{n}.jpg"), july + n * 1000);
            photo.fingerprint = format!("j{n}");
            upsert(&tx, &photo).unwrap();
        }
        let mut old = dated("december.jpg", december);
        old.fingerprint = "d".into();
        upsert(&tx, &old).unwrap();
        let fav = Event {
            ts: 1,
            op: Op::Fav {
                photo: "j7".into(),
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
        assert_eq!(months[0].covers.len(), 7);
        // The favorite leads.
        assert_eq!(months[0].covers[0], "j7");
        assert_eq!(
            months[1],
            MonthSummary {
                year: 2023,
                month: 12,
                count: 1,
                covers: vec!["d".into()],
            }
        );
        assert!(super::months(&conn, &Listing::Favorites).unwrap()[0].count == 1);
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
