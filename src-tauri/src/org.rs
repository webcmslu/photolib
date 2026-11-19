//! Shared organization: favorites, hidden photos and albums.
//!
//! The source of truth is a set of append-only logs in the library folder, so
//! every computer that opens the folder sees the same organization:
//!
//! ```text
//! <library>/.photolib/library.json          {"format": 1, "id": "..."}
//! <library>/.photolib/devices/<device>.log  one JSON event per line
//! ```
//!
//! **Each device appends only to its own log.** Nothing is ever rewritten and
//! no two computers write the same file, so this needs no file locking and is
//! safe on SMB, NFS and exFAT.
//!
//! Every event sets one value: "photo X is a favorite: yes/no", "album A is
//! named N", "photo X is in album A: yes/no". Each value is a last-writer-wins
//! register: when devices disagree, the event with the greatest
//! `(timestamp, device id)` wins. Merging is therefore the same whatever order
//! the logs are read in, and reading an event twice changes nothing.
//!
//! Timestamps are a hybrid logical clock: wall-clock milliseconds, but never
//! less than one more than the newest timestamp this device has seen. An edit
//! made after seeing another device's edit thus always wins over it, even if
//! this computer's clock is behind.
//!
//! The merged state lives in the local index database (see `db.rs`), which is
//! only a cache: it can be rebuilt from the logs at any time.
//!
//! When the library folder is read-only, events go to a log in the local
//! app-data dir instead. If the folder later becomes writable, that local log
//! is moved into the shared one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db;

/// The `.photolib` format this build reads and writes.
const FORMAT: u32 = 1;
const MAX_ALBUM_NAME_CHARS: usize = 200;

/// One change to the organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Fav {
        photo: String,
        on: bool,
    },
    Hide {
        photo: String,
        on: bool,
    },
    /// Creates the album if needed, and names or renames it.
    AlbumName {
        album: String,
        name: String,
    },
    AlbumDeleted {
        album: String,
        on: bool,
    },
    AlbumMember {
        album: String,
        photo: String,
        on: bool,
    },
    /// Names the person whose face is at a point of a photo (x and y in
    /// ten-thousandths of the upright photo); an empty name removes it.
    /// Every computer groups faces by itself; a group takes the name of the
    /// named faces in it, and groups with the same name are one person.
    FaceName {
        photo: String,
        x: u16,
        y: u16,
        name: String,
    },
    /// Sets how a photo is shown (rotation, crop, light and color), or
    /// with `edit: null` shows it as it is again. The file is never changed.
    Edit {
        photo: String,
        edit: Option<Edit>,
    },
}

/// Adjustments to how a photo is shown. They are applied when the photo is
/// displayed; the original file stays as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Edit {
    /// Clockwise quarter turns: 0, 90, 180 or 270 degrees.
    #[serde(default)]
    pub rotate: u16,
    /// Mirrored left to right (before the rotation).
    #[serde(default)]
    pub flip: bool,
    /// The part to keep, as left, top, width and height in ten-thousandths
    /// of the rotated picture. `None` keeps all of it.
    #[serde(default)]
    pub crop: Option<[u16; 4]>,
    /// From -100 to 100; 0 changes nothing.
    #[serde(default)]
    pub brightness: i8,
    #[serde(default)]
    pub contrast: i8,
    #[serde(default)]
    pub saturation: i8,
}

impl Edit {
    fn is_valid(&self) -> bool {
        let crop_ok = self.crop.is_none_or(|[left, top, width, height]| {
            width >= 100
                && height >= 100
                && u32::from(left) + u32::from(width) <= 10_000
                && u32::from(top) + u32::from(height) <= 10_000
        });
        let in_range = |value: i8| (-100..=100).contains(&value);
        matches!(self.rotate, 0 | 90 | 180 | 270)
            && crop_ok
            && in_range(self.brightness)
            && in_range(self.contrast)
            && in_range(self.saturation)
    }
}

impl Op {
    /// Rejects events that are malformed, whether they come from the UI or
    /// from a log written by something else.
    pub fn is_valid(&self) -> bool {
        match self {
            Op::Fav { photo, .. } | Op::Hide { photo, .. } => is_id(photo, 64),
            Op::AlbumName { album, name } => {
                is_id(album, 32)
                    && !name.trim().is_empty()
                    && name.chars().count() <= MAX_ALBUM_NAME_CHARS
            }
            Op::AlbumDeleted { album, .. } => is_id(album, 32),
            Op::AlbumMember { album, photo, .. } => is_id(album, 32) && is_id(photo, 64),
            Op::FaceName { photo, x, y, name } => {
                is_id(photo, 64)
                    && *x <= 10_000
                    && *y <= 10_000
                    && name.chars().count() <= MAX_ALBUM_NAME_CHARS
            }
            Op::Edit { photo, edit } => is_id(photo, 64) && edit.is_none_or(|edit| edit.is_valid()),
        }
    }
}

/// One line of a device log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Hybrid logical clock, in milliseconds since the Unix epoch.
    pub ts: i64,
    #[serde(flatten)]
    pub op: Op,
}

fn is_id(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A new random 32-character id, for libraries, devices and albums.
pub fn new_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    Ok(id)
}

/// This computer's device id, created on first use and kept in the app-data dir.
pub fn device_id(app_data_dir: &Path) -> io::Result<String> {
    let file = app_data_dir.join("device-id");
    if let Ok(existing) = fs::read_to_string(&file) {
        if is_id(existing.trim(), 32) {
            return Ok(existing.trim().to_owned());
        }
    }
    let id = new_id()?;
    fs::create_dir_all(app_data_dir)?;
    fs::write(&file, &id)?;
    Ok(id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryMeta {
    pub format: u32,
    pub id: String,
}

#[derive(Debug)]
pub enum MetaError {
    /// The library was organized by a newer version of the app.
    NewerFormat(u32),
    /// `library.json` exists but can't be understood.
    Corrupt(String),
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerFormat(format) => write!(
                f,
                "This library was organized by a newer version of the app (format {format}). \
                 Update the app to open it."
            ),
            Self::Corrupt(why) => {
                write!(f, "The library's .photolib/library.json is damaged: {why}")
            }
        }
    }
}

/// Reads `<root>/.photolib/library.json`, creating it for a new library.
/// Returns `None` when there is none and the folder is read-only.
pub fn read_or_create_meta(root: &Path) -> Result<Option<LibraryMeta>, MetaError> {
    let dir = root.join(".photolib");
    let file = dir.join("library.json");
    match fs::read(&file) {
        Ok(bytes) => {
            let meta: LibraryMeta =
                serde_json::from_slice(&bytes).map_err(|e| MetaError::Corrupt(e.to_string()))?;
            if meta.format > FORMAT {
                return Err(MetaError::NewerFormat(meta.format));
            }
            if !is_id(&meta.id, 32) {
                return Err(MetaError::Corrupt("invalid library id".into()));
            }
            Ok(Some(meta))
        }
        Err(_) => {
            let create = || -> io::Result<LibraryMeta> {
                let meta = LibraryMeta {
                    format: FORMAT,
                    id: new_id()?,
                };
                fs::create_dir_all(&dir)?;
                // Write, then rename: another computer never reads half a file.
                let tmp = dir.join(format!("library.json.{}.tmp", meta.id));
                fs::write(&tmp, serde_json::to_vec_pretty(&meta)?)?;
                fs::rename(&tmp, &file)?;
                Ok(meta)
            };
            Ok(create().ok())
        }
    }
}

/// Where the logs of one library are.
#[derive(Debug, Clone)]
pub struct OrgPaths {
    /// `<library>/.photolib/devices`
    shared: PathBuf,
    /// A folder in the local app-data dir, used while the library is read-only.
    local: PathBuf,
}

impl OrgPaths {
    pub fn new(library_root: &Path, local_dir: &Path) -> Self {
        Self {
            shared: library_root.join(".photolib").join("devices"),
            local: local_dir.to_owned(),
        }
    }

    fn shared_log(&self, device: &str) -> PathBuf {
        self.shared.join(format!("{device}.log"))
    }

    fn local_log(&self, device: &str) -> PathBuf {
        self.local.join(format!("{device}.log"))
    }

    /// True if this device can write its log in the library folder. Creates
    /// the (empty) log as a side effect, which is harmless.
    pub fn shared_is_writable(&self, device: &str) -> bool {
        fs::create_dir_all(&self.shared).is_ok()
            && OpenOptions::new()
                .append(true)
                .create(true)
                .open(self.shared_log(device))
                .is_ok()
    }
}

/// Appends events to a log as complete lines, in a single write.
///
/// Network shares support less than local disks: a share mounted by the
/// Linux file manager (GVFS) can't seek in a file opened for appending, and
/// macOS SMB shares refuse the full sync to disk. So the last byte is read
/// through a separate read-only handle, and a sync the share doesn't support
/// is skipped.
fn append(log: &Path, events: &[Event]) -> io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }

    let mut buffer = Vec::new();
    // If an earlier write was cut short, start on a fresh line so that only
    // the torn line is lost, not ours.
    if !ends_with_newline(log)? {
        buffer.push(b'\n');
    }
    for event in events {
        serde_json::to_writer(&mut buffer, event)?;
        buffer.push(b'\n');
    }

    let mut file = OpenOptions::new().append(true).create(true).open(log)?;
    file.write_all(&buffer)?;
    match file.sync_data() {
        Err(e) if sync_unsupported(&e) => Ok(()),
        result => result,
    }
}

/// True if the log is missing, empty, or ends with a complete line.
fn ends_with_newline(log: &Path) -> io::Result<bool> {
    let mut file = match File::open(log) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() == 0 {
        return Ok(true);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// True for the errors of a file system that has no way to sync to disk:
/// the data was written, it just can't be forced out any sooner.
fn sync_unsupported(e: &io::Error) -> bool {
    // ENOTSUP is 45 on macOS; EOPNOTSUPP is 95 and EINVAL 22 on Linux.
    matches!(
        e.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    ) || matches!(e.raw_os_error(), Some(45) if cfg!(target_os = "macos"))
        || matches!(e.raw_os_error(), Some(95) if cfg!(target_os = "linux"))
}

/// Reads the complete lines of `log` from byte `offset` on. Returns the valid
/// events and the offset to continue from next time.
///
/// Lines that can't be parsed are skipped: a torn write, or an event type from
/// a newer version, must not block everything after it.
fn read_from(log: &Path, offset: u64) -> io::Result<(Vec<Event>, u64)> {
    let mut file = File::open(log)?;
    let len = file.metadata()?.len();
    // A log never shrinks; if it did, it was replaced. Start over.
    let offset = if offset > len { 0 } else { offset };
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity((len - offset) as usize);
    file.read_to_end(&mut bytes)?;

    // Stop at the last newline: a final line without one may still be in flight.
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let events = bytes[..complete]
        .split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Event>(line).ok())
        .filter(|event| event.op.is_valid())
        .collect();
    Ok((events, offset + complete as u64))
}

/// `<device>` for a file named `<device>.log` with a valid device id.
fn log_device(path: &Path) -> Option<String> {
    if path.extension()?.to_str()? != "log" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    is_id(stem, 32).then(|| stem.to_owned())
}

fn logs_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(String, PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            Some((log_device(&path)?, path))
        })
        .collect();
    logs.sort();
    logs
}

/// Merges every event not seen yet, from every device's log, into the local
/// database. Returns how many new events were read.
///
/// With `writable`, a local-only log from the time the folder was read-only is
/// first moved into this device's shared log.
pub fn sync(
    conn: &mut Connection,
    paths: &OrgPaths,
    device: &str,
    writable: bool,
) -> rusqlite::Result<usize> {
    if writable {
        // On failure the local log simply stays, and still gets merged below.
        let _ = publish_local_log(paths, device);
    }

    let sources = logs_in(&paths.shared)
        .into_iter()
        .map(|(device, path)| (format!("shared:{device}"), device, path))
        .chain(
            logs_in(&paths.local)
                .into_iter()
                .map(|(device, path)| (format!("local:{device}"), device, path)),
        );

    let mut read = 0;
    let tx = conn.transaction()?;
    for (source, log_device, path) in sources {
        let offset = db::log_offset(&tx, &source)?;
        // An unreadable log is skipped this time and picked up on a later sync.
        let Ok((events, next_offset)) = read_from(&path, offset) else {
            continue;
        };
        for event in &events {
            db::apply_event(&tx, &log_device, event)?;
        }
        if next_offset != offset {
            db::set_log_offset(&tx, &source, next_offset)?;
        }
        read += events.len();
    }
    tx.commit()?;
    Ok(read)
}

/// Appends this device's local-only log to its shared log, then deletes it.
fn publish_local_log(paths: &OrgPaths, device: &str) -> io::Result<()> {
    let local = paths.local_log(device);
    let (events, _) = match read_from(&local, 0) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    append(&paths.shared_log(device), &events)?;
    fs::remove_file(&local)
}

#[derive(Debug)]
pub enum RecordError {
    Invalid,
    Log(io::Error),
    Database(rusqlite::Error),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => write!(f, "Invalid change."),
            Self::Log(e) => write!(f, "Couldn't save the change: {e}"),
            Self::Database(e) => write!(f, "Index database error: {e}"),
        }
    }
}

/// Records changes made on this device: appends them to its log (shared when
/// `writable`, local otherwise), then applies them to the local database.
///
/// The log is written first. If that fails nothing changes; if the database
/// update fails afterwards, the next sync applies the events from the log.
pub fn record(
    conn: &mut Connection,
    paths: &OrgPaths,
    device: &str,
    writable: bool,
    ops: Vec<Op>,
) -> Result<(), RecordError> {
    if !ops.iter().all(Op::is_valid) {
        return Err(RecordError::Invalid);
    }
    let newest_seen = db::max_event_ts(conn).map_err(RecordError::Database)?;
    let first_ts = now_ms().max(newest_seen + 1);
    let events: Vec<Event> = ops
        .into_iter()
        .zip(first_ts..)
        .map(|(op, ts)| Event { ts, op })
        .collect();

    let log = if writable {
        paths.shared_log(device)
    } else {
        paths.local_log(device)
    };
    append(&log, &events).map_err(RecordError::Log)?;

    let tx = conn.transaction().map_err(RecordError::Database)?;
    for event in &events {
        db::apply_event(&tx, device, event).map_err(RecordError::Database)?;
    }
    tx.commit().map_err(RecordError::Database)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Listing;

    const DEVICE_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DEVICE_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ALBUM: &str = "11111111111111111111111111111111";

    fn photo(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn fav(n: u8, on: bool) -> Op {
        Op::Fav {
            photo: photo(n),
            on,
        }
    }

    fn member(n: u8, on: bool) -> Op {
        Op::AlbumMember {
            album: ALBUM.into(),
            photo: photo(n),
            on,
        }
    }

    fn named(name: &str) -> Op {
        Op::AlbumName {
            album: ALBUM.into(),
            name: name.into(),
        }
    }

    /// A library folder plus one local app-data dir per device.
    struct World {
        dir: tempfile::TempDir,
    }

    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("library")).unwrap();
            Self { dir }
        }

        fn paths(&self, device: &str) -> OrgPaths {
            OrgPaths::new(
                &self.dir.path().join("library"),
                &self.dir.path().join(format!("local-{device}")),
            )
        }

        /// Writes events straight into a device's shared log, with chosen timestamps.
        fn write(&self, device: &str, events: &[(i64, Op)]) {
            let events: Vec<Event> = events
                .iter()
                .map(|(ts, op)| Event {
                    ts: *ts,
                    op: op.clone(),
                })
                .collect();
            append(&self.paths(device).shared_log(device), &events).unwrap();
        }
    }

    /// The whole organization state as sorted text, to compare two databases.
    fn state(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT kind || ' ' || key || ' ' || photo || ' = ' || value FROM org_state ORDER BY 1")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get(0)).unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    }

    fn synced(world: &World, device: &str) -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        sync(&mut conn, &world.paths(device), device, true).unwrap();
        conn
    }

    #[test]
    fn log_lines_round_trip_and_are_readable() {
        let event = Event {
            ts: 5,
            op: fav(1, true),
        };
        let line = serde_json::to_string(&event).unwrap();
        assert_eq!(
            line,
            format!(r#"{{"ts":5,"op":"fav","photo":"{}","on":true}}"#, photo(1))
        );
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), event);
    }

    #[test]
    fn conflicting_edits_converge_on_every_device() {
        let world = World::new();
        // Both devices set the same things, at interleaved times.
        world.write(
            DEVICE_A,
            &[
                (100, fav(1, true)),
                (300, named("Holiday")),
                (310, member(1, true)),
                (320, member(2, true)),
                (500, fav(2, true)),
            ],
        );
        world.write(
            DEVICE_B,
            &[
                (200, fav(1, false)),    // later than A's: wins
                (250, named("Summer")),  // earlier than A's: loses
                (400, member(2, false)), // later than A's add: wins
                (500, fav(2, false)),    // same instant as A's: device id breaks the tie
            ],
        );

        let on_a = synced(&world, DEVICE_A);
        let on_b = synced(&world, DEVICE_B);
        assert_eq!(state(&on_a), state(&on_b));

        assert_eq!(
            state(&on_a),
            [
                format!("album_name {ALBUM}  = Holiday"),
                format!("fav {}  = 0", photo(1)),
                format!("fav {}  = 0", photo(2)), // "bbbb…" > "aaaa…"
                format!("member {ALBUM} {} = 1", photo(1)),
                format!("member {ALBUM} {} = 0", photo(2)),
            ]
        );
    }

    #[test]
    fn merge_order_and_repeats_do_not_matter() {
        let world = World::new();
        world.write(DEVICE_A, &[(100, fav(1, true)), (300, fav(1, true))]);
        world.write(DEVICE_B, &[(200, fav(1, false))]);
        let expected = state(&synced(&world, DEVICE_A));

        // Apply the same events by hand in the opposite order, twice over.
        let mut conn = db::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for _ in 0..2 {
            db::apply_event(
                &tx,
                DEVICE_A,
                &Event {
                    ts: 300,
                    op: fav(1, true),
                },
            )
            .unwrap();
            db::apply_event(
                &tx,
                DEVICE_B,
                &Event {
                    ts: 200,
                    op: fav(1, false),
                },
            )
            .unwrap();
            db::apply_event(
                &tx,
                DEVICE_A,
                &Event {
                    ts: 100,
                    op: fav(1, true),
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(state(&conn), expected);
    }

    #[test]
    fn sync_reads_only_what_is_new() {
        let world = World::new();
        world.write(DEVICE_B, &[(100, fav(1, true))]);
        let mut conn = db::open_in_memory().unwrap();
        let paths = world.paths(DEVICE_A);

        assert_eq!(sync(&mut conn, &paths, DEVICE_A, true).unwrap(), 1);
        assert_eq!(sync(&mut conn, &paths, DEVICE_A, true).unwrap(), 0);

        world.write(DEVICE_B, &[(200, fav(2, true))]);
        assert_eq!(sync(&mut conn, &paths, DEVICE_A, true).unwrap(), 1);
        assert_eq!(state(&conn).len(), 2);
    }

    #[test]
    fn damaged_and_unknown_lines_are_skipped() {
        let world = World::new();
        let log = world.paths(DEVICE_B).shared_log(DEVICE_B);
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        let good = |ts, n| {
            serde_json::to_string(&Event {
                ts,
                op: fav(n, true),
            })
            .unwrap()
        };
        let text = format!(
            "{}\n{{\"ts\":2,\"op\":\"from_the_future\",\"x\":1}}\nnot json at all\n\
             {{\"ts\":3,\"op\":\"fav\",\"photo\":\"../../etc\",\"on\":true}}\n{}\n{}",
            good(1, 1),
            good(4, 2),
            &good(5, 3)[..20], // a write that is still in flight: no newline yet
        );
        fs::write(&log, text).unwrap();
        // Files that are not device logs are ignored.
        fs::write(log.with_file_name("notes.txt"), "hello").unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let paths = world.paths(DEVICE_A);
        assert_eq!(sync(&mut conn, &paths, DEVICE_A, true).unwrap(), 2);

        // The torn line is completed later (here: started over on a new line).
        append(
            &log,
            &[Event {
                ts: 6,
                op: fav(3, true),
            }],
        )
        .unwrap();
        assert_eq!(sync(&mut conn, &paths, DEVICE_A, true).unwrap(), 1);
        assert_eq!(state(&conn).len(), 3);
    }

    #[test]
    fn recording_writes_the_log_and_orders_after_everything_seen() {
        let world = World::new();
        let far_future = now_ms() + 1_000_000;
        world.write(DEVICE_B, &[(far_future, fav(1, true))]);

        let paths = world.paths(DEVICE_A);
        let mut conn = db::open_in_memory().unwrap();
        sync(&mut conn, &paths, DEVICE_A, true).unwrap();
        // A's clock is "behind" B's, yet un-favoriting after seeing B's event wins.
        record(
            &mut conn,
            &paths,
            DEVICE_A,
            true,
            vec![fav(1, false), fav(2, true)],
        )
        .unwrap();
        assert_eq!(
            state(&conn),
            [
                format!("fav {}  = 0", photo(1)),
                format!("fav {}  = 1", photo(2))
            ]
        );

        // B sees the same once it reads A's log.
        assert_eq!(state(&synced(&world, DEVICE_B)), state(&conn));

        let invalid = Op::Fav {
            photo: "nope".into(),
            on: true,
        };
        assert!(matches!(
            record(&mut conn, &paths, DEVICE_A, true, vec![invalid]),
            Err(RecordError::Invalid)
        ));
    }

    #[test]
    fn read_only_library_keeps_changes_locally_then_publishes_them() {
        let world = World::new();
        let paths = world.paths(DEVICE_A);
        let mut conn = db::open_in_memory().unwrap();

        record(&mut conn, &paths, DEVICE_A, false, vec![fav(1, true)]).unwrap();
        assert!(paths.local_log(DEVICE_A).is_file());
        assert!(!paths.shared_log(DEVICE_A).exists());
        // A rebuilt index gets the local changes back.
        let mut rebuilt = db::open_in_memory().unwrap();
        sync(&mut rebuilt, &paths, DEVICE_A, false).unwrap();
        assert_eq!(state(&rebuilt), state(&conn));

        // The folder becomes writable: the local log moves into the shared one.
        sync(&mut conn, &paths, DEVICE_A, true).unwrap();
        assert!(!paths.local_log(DEVICE_A).exists());
        assert_eq!(state(&synced(&world, DEVICE_B)), state(&conn));
    }

    #[test]
    fn library_meta_is_created_once_and_newer_formats_are_refused() {
        let world = World::new();
        let root = world.dir.path().join("library");

        let created = read_or_create_meta(&root).unwrap().unwrap();
        assert_eq!(read_or_create_meta(&root).unwrap().unwrap(), created);

        fs::write(
            root.join(".photolib/library.json"),
            format!(r#"{{"format": 99, "id": "{ALBUM}"}}"#),
        )
        .unwrap();
        assert!(matches!(
            read_or_create_meta(&root),
            Err(MetaError::NewerFormat(99))
        ));

        // Where `.photolib` can't be created (read-only folder), there is no meta.
        let not_a_folder = world.dir.path().join("file");
        fs::write(&not_a_folder, "x").unwrap();
        assert!(read_or_create_meta(&not_a_folder).unwrap().is_none());
    }

    #[test]
    fn device_id_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let first = device_id(dir.path()).unwrap();
        assert!(is_id(&first, 32));
        assert_eq!(device_id(dir.path()).unwrap(), first);
    }

    #[test]
    fn listings_follow_the_organization() {
        use crate::db::{photos_page, upsert, PhotoRow};

        let world = World::new();
        let paths = world.paths(DEVICE_A);
        let mut conn = db::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        for n in 1..=4u8 {
            upsert(
                &tx,
                &PhotoRow {
                    rel_path: format!("{n}.jpg"),
                    size: 1,
                    mtime_ms: 0,
                    fingerprint: photo(n),
                    sort_date: i64::from(n),
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
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        record(
            &mut conn,
            &paths,
            DEVICE_A,
            true,
            vec![
                fav(1, true),
                fav(3, true),
                Op::Hide {
                    photo: photo(3),
                    on: true,
                },
                named("Holiday"),
                member(2, true),
                member(3, true),
                member(4, true),
                member(4, false),
            ],
        )
        .unwrap();

        let list = |listing: &Listing| -> Vec<String> {
            photos_page(&conn, listing, 0, 10)
                .unwrap()
                .into_iter()
                .map(|p| format!("{}{}", p.rel_path, if p.favorite { "*" } else { "" }))
                .collect()
        };
        // Hidden photos leave every listing except "Hidden".
        assert_eq!(list(&Listing::Library), ["4.jpg", "2.jpg", "1.jpg*"]);
        assert_eq!(list(&Listing::Favorites), ["1.jpg*"]);
        assert_eq!(list(&Listing::Hidden), ["3.jpg*"]);
        assert_eq!(
            list(&Listing::Album {
                album: ALBUM.into()
            }),
            ["2.jpg"]
        );

        let albums = db::albums(&conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!((albums[0].name.as_str(), albums[0].count), ("Holiday", 1));

        record(
            &mut conn,
            &paths,
            DEVICE_A,
            true,
            vec![Op::AlbumDeleted {
                album: ALBUM.into(),
                on: true,
            }],
        )
        .unwrap();
        assert!(db::albums(&conn).unwrap().is_empty());
    }
}
