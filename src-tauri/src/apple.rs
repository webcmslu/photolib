//! Imports favorites, hidden photos and albums from an Apple Photos library.
//!
//! An Apple Photos library is a folder (`Something.photoslibrary`) with a
//! SQLite database, `database/Photos.sqlite`, and the original files under
//! `originals/`. The database is undocumented; what is used here was checked
//! against a real library (macOS 27) and is read defensively:
//!
//! - `ZASSET`: one row per photo, with `ZFAVORITE`, `ZHIDDEN`,
//!   `ZTRASHEDSTATE`, and where its original is (`ZDIRECTORY`/`ZFILENAME`);
//! - `ZADDITIONALASSETATTRIBUTES`: the file's name before it was imported
//!   into Photos (`ZORIGINALFILENAME`) and its size (`ZORIGINALFILESIZE`);
//! - `ZGENERICALBUM`: albums; the ones the user made have `ZKIND = 2`;
//! - a table named like `Z_34ASSETS` with columns like `Z_34ALBUMS` and
//!   `Z_3ASSETS` says which photos are in which album. The numbers change
//!   between macOS versions, so the table is looked up by its shape.
//!
//! The Photos library is only read, never written. A photo of it is matched
//! to a photo of our library by original file name and size, or failing
//! that by the content fingerprint of its original file.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::db;
use crate::fingerprint;
use crate::org::{self, Op};

/// Apple's `ZKIND` for an album the user made.
const USER_ALBUM: i64 = 2;

/// What an import would do (or did).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// Photos and videos in the Apple Photos library (not in its trash).
    pub assets: u32,
    /// How many of them were found in this library.
    pub matched: u32,
    /// Of the matched ones: favorites, hidden.
    pub favorites: u32,
    pub hidden: u32,
    pub albums: Vec<AlbumSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub name: String,
    /// Photos of the album in Apple Photos, and how many of them were found.
    pub photos: u32,
    pub matched: u32,
}

/// The import worked out: what to record, and the summary for the user.
pub struct Plan {
    pub summary: ImportSummary,
    favorites: Vec<String>,
    hidden: Vec<String>,
    /// Album name and the fingerprints of its photos.
    albums: Vec<(String, Vec<String>)>,
}

struct Asset {
    id: i64,
    directory: Option<String>,
    file_name: Option<String>,
    original_name: Option<String>,
    original_size: Option<i64>,
    favorite: bool,
    hidden: bool,
}

/// Reads the Apple Photos library at `photos_library` and matches it against
/// the index in `conn`.
pub fn plan(conn: &Connection, photos_library: &Path) -> Result<Plan, String> {
    let apple = open(photos_library)?;
    let failed = |e: rusqlite::Error| format!("Couldn't read the Photos library: {e}");

    let assets = read_assets(&apple).map_err(failed)?;
    let album_rows = read_albums(&apple).map_err(failed)?;
    let memberships = read_memberships(&apple).map_err(failed)?;

    // Our photos, by (lowercase file name, size) and by fingerprint.
    let mut by_name: HashMap<(String, u64), Vec<String>> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    for (rel_path, size, fingerprint) in db::files(conn).map_err(failed)? {
        let name = rel_path
            .rsplit('/')
            .next()
            .unwrap_or(&rel_path)
            .to_lowercase();
        let same = by_name.entry((name, size)).or_default();
        if !same.contains(&fingerprint) {
            same.push(fingerprint.clone());
        }
        known.insert(fingerprint);
    }

    let mut fingerprint_of: HashMap<i64, String> = HashMap::new();
    let mut plan = Plan {
        summary: ImportSummary {
            assets: assets.len() as u32,
            ..ImportSummary::default()
        },
        favorites: Vec::new(),
        hidden: Vec::new(),
        albums: Vec::new(),
    };
    for asset in &assets {
        let Some(fingerprint) = match_asset(asset, photos_library, &by_name, &known) else {
            continue;
        };
        plan.summary.matched += 1;
        if asset.favorite {
            plan.summary.favorites += 1;
            plan.favorites.push(fingerprint.clone());
        }
        if asset.hidden {
            plan.summary.hidden += 1;
            plan.hidden.push(fingerprint.clone());
        }
        fingerprint_of.insert(asset.id, fingerprint);
    }

    let live: HashSet<i64> = assets.iter().map(|asset| asset.id).collect();
    for (album_id, name) in album_rows {
        let members: Vec<i64> = memberships
            .iter()
            .filter(|(album, asset)| *album == album_id && live.contains(asset))
            .map(|(_, asset)| *asset)
            .collect();
        let mut found: Vec<String> = members
            .iter()
            .filter_map(|asset| fingerprint_of.get(asset).cloned())
            .collect();
        found.sort();
        found.dedup();
        plan.summary.albums.push(AlbumSummary {
            name: name.clone(),
            photos: members.len() as u32,
            matched: found.len() as u32,
        });
        if !found.is_empty() {
            plan.albums.push((name, found));
        }
    }
    plan.summary
        .albums
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(plan)
}

/// The log entries that carry out a plan. An album that already exists here
/// under the same name gets the photos added; nothing is ever removed, so
/// importing twice does no harm.
pub fn ops(conn: &Connection, plan: &Plan) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    for photo in &plan.favorites {
        ops.push(Op::Fav {
            photo: photo.clone(),
            on: true,
        });
    }
    for photo in &plan.hidden {
        ops.push(Op::Hide {
            photo: photo.clone(),
            on: true,
        });
    }
    let existing = db::albums(conn).map_err(|e| e.to_string())?;
    for (name, photos) in &plan.albums {
        let same_name = existing
            .iter()
            .find(|album| album.name.to_lowercase() == name.trim().to_lowercase());
        let album = match same_name {
            Some(album) => album.id.clone(),
            None => {
                let id = org::new_id().map_err(|e| e.to_string())?;
                ops.push(Op::AlbumName {
                    album: id.clone(),
                    name: name.trim().chars().take(200).collect(),
                });
                id
            }
        };
        for photo in photos {
            ops.push(Op::AlbumMember {
                album: album.clone(),
                photo: photo.clone(),
                on: true,
            });
        }
    }
    Ok(ops)
}

fn open(photos_library: &Path) -> Result<Connection, String> {
    let database = photos_library.join("database").join("Photos.sqlite");
    if !database.is_file() {
        return Err(format!(
            "\"{}\" is not an Apple Photos library (no database/Photos.sqlite in it).",
            photos_library.display()
        ));
    }
    Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|conn| {
            // Proves the file can really be read (macOS may refuse access).
            conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))?;
            Ok(conn)
        })
        .map_err(|e| {
            format!(
                "Couldn't open the Photos library ({e}). On macOS, the app may need \
                 \"Full Disk Access\" or access to Photos in System Settings > Privacy & Security."
            )
        })
}

fn read_assets(apple: &Connection) -> rusqlite::Result<Vec<Asset>> {
    let mut stmt = apple.prepare(
        "SELECT a.Z_PK, a.ZDIRECTORY, a.ZFILENAME, x.ZORIGINALFILENAME, x.ZORIGINALFILESIZE,
                COALESCE(a.ZFAVORITE, 0), COALESCE(a.ZHIDDEN, 0)
         FROM ZASSET a LEFT JOIN ZADDITIONALASSETATTRIBUTES x ON x.ZASSET = a.Z_PK
         WHERE COALESCE(a.ZTRASHEDSTATE, 0) = 0",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Asset {
            id: row.get(0)?,
            directory: row.get(1)?,
            file_name: row.get(2)?,
            original_name: row.get(3)?,
            original_size: row.get(4)?,
            favorite: row.get::<_, i64>(5)? != 0,
            hidden: row.get::<_, i64>(6)? != 0,
        })
    })?;
    rows.collect()
}

/// The albums the user made: `(id, title)`.
fn read_albums(apple: &Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = apple.prepare(&format!(
        "SELECT Z_PK, ZTITLE FROM ZGENERICALBUM
         WHERE ZKIND = {USER_ALBUM} AND COALESCE(ZTRASHEDSTATE, 0) = 0
           AND ZTITLE IS NOT NULL AND TRIM(ZTITLE) != ''"
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// Which photo is in which album: `(album id, asset id)`.
fn read_memberships(apple: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    // The table is named `Z_<n>ASSETS` and has the columns `Z_<n>ALBUMS` and
    // `Z_<m>ASSETS`, with numbers that depend on the macOS version.
    let numbered = |name: &str, suffix: &str| {
        name.strip_prefix("Z_")
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    };
    let tables: Vec<String> = apple
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for table in tables.iter().filter(|name| numbered(name, "ASSETS")) {
        let columns: Vec<String> = apple
            .prepare("SELECT name FROM pragma_table_info(?1)")?
            .query_map([table], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let album_column = columns.iter().find(|name| numbered(name, "ALBUMS"));
        let asset_column = columns.iter().find(|name| numbered(name, "ASSETS"));
        if let (Some(album_column), Some(asset_column)) = (album_column, asset_column) {
            // The names consist of "Z_", digits and capital letters only
            // (checked above), so they are safe to put in the SQL text.
            let mut stmt = apple.prepare(&format!(
                "SELECT {album_column}, {asset_column} FROM {table}"
            ))?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            return rows.collect();
        }
    }
    Ok(Vec::new())
}

/// Finds the photo of our library that is the same as an Apple Photos asset.
fn match_asset(
    asset: &Asset,
    photos_library: &Path,
    by_name: &HashMap<(String, u64), Vec<String>>,
    known: &HashSet<String>,
) -> Option<String> {
    // The same name and the same size in bytes: the same file. Only when
    // that points to exactly one photo, though.
    if let (Some(name), Some(size)) = (&asset.original_name, asset.original_size) {
        if let Some(candidates) = by_name.get(&(name.to_lowercase(), size.max(0) as u64)) {
            if let [only] = candidates.as_slice() {
                return Some(only.clone());
            }
        }
    }
    // Otherwise compare the content of the original that Photos keeps, if
    // it is on this disk (it is not when originals live in iCloud).
    let original = photos_library
        .join("originals")
        .join(asset.directory.as_deref()?)
        .join(asset.file_name.as_deref()?);
    let fingerprint = fingerprint::fingerprint(&original).ok()?;
    known.contains(&fingerprint).then_some(fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PhotoRow;
    use crate::org::Event;
    use std::fs;

    /// A Photos library with the tables and columns the import reads, laid
    /// out as in a real one (macOS 27).
    fn fake_library(dir: &Path) -> Connection {
        fs::create_dir_all(dir.join("database")).unwrap();
        let apple = Connection::open(dir.join("database/Photos.sqlite")).unwrap();
        apple
            .execute_batch(
                "CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZFAVORITE INTEGER,
                     ZHIDDEN INTEGER, ZTRASHEDSTATE INTEGER, ZDIRECTORY VARCHAR,
                     ZFILENAME VARCHAR);
                 CREATE TABLE ZADDITIONALASSETATTRIBUTES (Z_PK INTEGER PRIMARY KEY,
                     ZASSET INTEGER, ZORIGINALFILENAME VARCHAR, ZORIGINALFILESIZE INTEGER);
                 CREATE TABLE ZGENERICALBUM (Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER,
                     ZTRASHEDSTATE INTEGER, ZTITLE VARCHAR);
                 CREATE TABLE Z_33KEYASSETS (Z_33ALBUMSBEINGKEYASSETS INTEGER,
                     Z_3KEYASSETS INTEGER);
                 CREATE TABLE Z_34ASSETS (Z_34ALBUMS INTEGER, Z_3ASSETS INTEGER,
                     Z_FOK_3ASSETS INTEGER);
                 CREATE TABLE Z_3MEMORIESBEINGCURATEDASSETS (Z_3CURATEDASSETS INTEGER,
                     Z_60MEMORIESBEINGCURATEDASSETS INTEGER);",
            )
            .unwrap();
        apple
    }

    fn add_asset(
        apple: &Connection,
        id: i64,
        original: (&str, i64),
        favorite: bool,
        hidden: bool,
        trashed: bool,
    ) {
        apple
            .execute(
                "INSERT INTO ZASSET VALUES (?1, ?2, ?3, ?4, 'A', ?5)",
                rusqlite::params![id, favorite, hidden, trashed, format!("UUID-{id}.jpg")],
            )
            .unwrap();
        apple
            .execute(
                "INSERT INTO ZADDITIONALASSETATTRIBUTES VALUES (?1, ?1, ?2, ?3)",
                rusqlite::params![id, original.0, original.1],
            )
            .unwrap();
    }

    fn index_photo(conn: &mut Connection, rel_path: &str, size: u64, fingerprint: &str) {
        let tx = conn.transaction().unwrap();
        db::upsert(
            &tx,
            &PhotoRow {
                rel_path: rel_path.into(),
                size,
                mtime_ms: 0,
                fingerprint: fingerprint.into(),
                sort_date: 0,
                date_from_exif: false,
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
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn imports_favorites_hidden_and_albums() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("Test.photoslibrary");
        let apple = fake_library(&library);
        let fp = |n: u8| format!("{n:02x}").repeat(32);

        let mut conn = db::open_in_memory().unwrap();
        index_photo(&mut conn, "2024/IMG_0001.HEIC", 1000, &fp(1));
        index_photo(&mut conn, "2024/IMG_0002.HEIC", 2000, &fp(2));
        // The same file twice in our library: still one photo.
        index_photo(&mut conn, "copies/img_0002.heic", 2000, &fp(2));
        // Two different photos with the same name and size: ambiguous.
        index_photo(&mut conn, "a/IMG_0003.JPG", 3000, &fp(3));
        index_photo(&mut conn, "b/IMG_0003.JPG", 3000, &fp(30));

        add_asset(&apple, 1, ("IMG_0001.HEIC", 1000), true, false, false);
        add_asset(&apple, 2, ("IMG_0002.HEIC", 2000), false, true, false);
        add_asset(&apple, 3, ("IMG_0003.JPG", 3000), true, false, false);
        add_asset(&apple, 4, ("IMG_0004.JPG", 4000), true, false, false); // not in our library
        add_asset(&apple, 5, ("IMG_0001.HEIC", 1000), true, false, true); // in the trash

        // The ambiguous one is settled by its content: Photos has the original.
        let originals = library.join("originals/A");
        fs::create_dir_all(&originals).unwrap();
        fs::write(originals.join("UUID-3.jpg"), b"the bytes of photo three").unwrap();
        let real = fingerprint::fingerprint(&originals.join("UUID-3.jpg")).unwrap();
        index_photo(&mut conn, "c/renamed.jpg", 24, &real);

        apple
            .execute_batch(
                "INSERT INTO ZGENERICALBUM VALUES (10, 2, 0, 'Holidays');
                 INSERT INTO ZGENERICALBUM VALUES (11, 2, 0, 'Empty here');
                 INSERT INTO ZGENERICALBUM VALUES (12, 1552, 0, 'A smart album');
                 INSERT INTO ZGENERICALBUM VALUES (13, 2, 1, 'Deleted album');
                 INSERT INTO Z_34ASSETS VALUES (10, 1, 0), (10, 2, 0), (10, 4, 0), (10, 5, 0);
                 INSERT INTO Z_34ASSETS VALUES (11, 4, 0), (12, 1, 0), (13, 1, 0);",
            )
            .unwrap();
        drop(apple);

        let plan = plan(&conn, &library).unwrap();
        assert_eq!(
            plan.summary,
            ImportSummary {
                assets: 4,
                matched: 3,
                favorites: 2,
                hidden: 1,
                albums: vec![
                    AlbumSummary {
                        name: "Empty here".into(),
                        photos: 1,
                        matched: 0
                    },
                    AlbumSummary {
                        name: "Holidays".into(),
                        photos: 3,
                        matched: 2
                    },
                ],
            }
        );
        assert_eq!(plan.favorites, [fp(1), real.clone()]);

        // Carrying it out, on top of an album that already has that name.
        let tx = conn.transaction().unwrap();
        let existing = Event {
            ts: 1,
            op: Op::AlbumName {
                album: "ab".repeat(16),
                name: "holidays".into(),
            },
        };
        db::apply_event(&tx, "device", &existing).unwrap();
        tx.commit().unwrap();

        let ops = ops(&conn, &plan).unwrap();
        assert!(ops.iter().all(Op::is_valid));
        // No new album: the two photos join the existing one.
        assert!(!ops.iter().any(|op| matches!(op, Op::AlbumName { .. })));
        let tx = conn.transaction().unwrap();
        for op in ops {
            db::apply_event(&tx, "device", &Event { ts: 2, op }).unwrap();
        }
        tx.commit().unwrap();
        let albums = db::albums(&conn).unwrap();
        assert_eq!(albums.len(), 1);
        // Photo 2 is hidden, so the album shows one of its two photos.
        assert_eq!(albums[0].count, 1);
        let favorites = db::photos_page(&conn, &db::Listing::Favorites, 0, 10).unwrap();
        assert_eq!(favorites.len(), 2);
        assert_eq!(
            db::photos_page(&conn, &db::Listing::Hidden, 0, 10)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_folder_that_is_not_a_photos_library_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();
        let error = plan(&conn, dir.path()).err().unwrap();
        assert!(error.contains("not an Apple Photos library"), "{error}");
    }
}
