// The library code, shared with the NAS server (core/).
use phototek_core::{
    collections, db, duplicates, enhance, export, faces, fingerprint, geo, indexer, metadata, org,
    original, people, similar, thumbs,
};

mod apple;
#[cfg(target_os = "macos")]
mod camera_macos;
#[cfg(windows)]
mod camera_windows;
/// Phones and cameras connected by USB, where the system has a way to read them.
#[cfg(target_os = "macos")]
use camera_macos as camera;
#[cfg(windows)]
use camera_windows as camera;
mod importer;
mod media_server;
mod recents;
mod rename;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};

use indexer::{Progress, ScanSummary};
use org::OrgPaths;
use recents::{RecentEntry, RecentStore};
use thumbs::{ThumbCache, ThumbSize, ThumbSummary};

/// Name of the event that carries [`IndexEvent`]s to the UI.
const INDEX_EVENT: &str = "index-event";

/// Shared app state.
struct AppState {
    data_dir: PathBuf,
    /// This computer's id in the libraries' shared device logs.
    device: String,
    /// The mutex serializes read-modify-write of the recents file.
    recents: Mutex<RecentStore>,
    /// Which local index each library path uses: the library's id from its
    /// `.photolib/library.json`. Remembered here so that the index is found
    /// again while the library is offline.
    keys: Mutex<HashMap<String, String>>,
    /// The scan that is running now, if any. At most one runs at a time.
    scan: Mutex<Option<RunningScan>>,
    thumbs: ThumbCache,
    /// The library that is open in the window, if any.
    open_library: Mutex<Option<OpenLibrary>>,
    /// Limits on-demand image decoding (thumbnails and converted originals).
    on_demand: Limiter,
    /// The face models, loaded the first time they are needed.
    face_finder: Mutex<Option<Arc<faces::FaceFinder>>>,
    /// Where the viewer loads videos from when the webview can't play them
    /// from `photo://` (Linux; see media_server.rs). `None` elsewhere.
    media_base: Option<String>,
    /// Set to stop the export, or the moving of duplicates, that is running.
    job_cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct OpenLibrary {
    path: String,
    /// False while the folder can't be reached: the app then shows what it
    /// has cached and makes no changes.
    online: bool,
    /// True if this device can write its log into the library folder.
    writable: bool,
}

impl AppState {
    /// The folder in the app-data dir that holds a library's local data.
    fn library_dir(&self, library_path: &str) -> PathBuf {
        let key = match self.keys.lock().unwrap().get(library_path) {
            Some(id) => id.clone(),
            // No `.photolib` yet (read-only folder): fall back to the path.
            None => format!(
                "path-{}",
                &blake3::hash(library_path.as_bytes()).to_hex()[..16]
            ),
        };
        self.data_dir.join("libraries").join(key)
    }

    fn index_db(&self, library_path: &str) -> PathBuf {
        self.library_dir(library_path).join("index.db")
    }

    fn org_paths(&self, library_path: &str) -> OrgPaths {
        OrgPaths::new(
            Path::new(library_path),
            &self.library_dir(library_path).join("local-log"),
        )
    }

    fn keys_file(&self) -> PathBuf {
        self.data_dir.join("library-keys.json")
    }

    fn remember_key(&self, library_path: &str, id: &str) {
        let mut keys = self.keys.lock().unwrap();
        if keys.get(library_path).map(String::as_str) == Some(id) {
            return;
        }
        keys.insert(library_path.to_owned(), id.to_owned());
        // Best effort: if this fails, only offline opening is affected.
        if let Ok(json) = serde_json::to_vec_pretty(&*keys) {
            let _ = fs::write(self.keys_file(), json);
        }
    }

    /// Updates the state of the open library, if it is still `library_path`.
    fn update_open(&self, library_path: &str, update: impl FnOnce(&mut OpenLibrary)) {
        if let Some(open) = self.open_library.lock().unwrap().as_mut() {
            if open.path == library_path {
                update(open);
            }
        }
    }
}

struct RunningScan {
    library_path: String,
    cancel: Arc<AtomicBool>,
}

/// What the UI hears about a scan. Every event names its library, so events
/// from a scan of a library that has since been closed can be ignored.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum IndexEvent {
    Running {
        library_path: String,
        progress: Progress,
    },
    /// The index is up to date; thumbnails are still being made.
    Indexed {
        library_path: String,
        summary: ScanSummary,
    },
    Finished {
        library_path: String,
        summary: ScanSummary,
        thumbnails: ThumbSummary,
    },
    Failed {
        library_path: String,
        message: String,
    },
    /// The folder is reachable (again). Sent at the start of every scan.
    Online {
        library_path: String,
        writable: bool,
    },
    /// The folder can't be reached. The app keeps showing what it has cached.
    Offline { library_path: String },
    /// Changes made on other devices were merged in.
    Synced { library_path: String },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryStats {
    photo_count: u64,
    /// True while a scan of this library is running.
    scanning: bool,
}

/// An opened library folder.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    path: String,
    name: String,
    /// False when the folder is read-only: organization stays local-only.
    writable: bool,
    /// False when the folder can't be reached and only cached data is shown.
    online: bool,
}

/// A recents entry plus whether its folder can be reached right now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentLibrary {
    #[serde(flatten)]
    entry: RecentEntry,
    available: bool,
}

fn with_availability(list: Vec<RecentEntry>) -> Vec<RecentLibrary> {
    list.into_iter()
        .map(|entry| RecentLibrary {
            available: Path::new(&entry.path).is_dir(),
            entry,
        })
        .collect()
}

/// Display name for a library: the folder's own name, or the whole path for a
/// root such as `/` or `D:\`.
fn library_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Starts a background scan of `library_path` unless one is already running
/// for it. A running scan of a different library is cancelled first, and so
/// is a running check of this one when everything is to be read again.
fn start_scan(app: &AppHandle, library_path: &str, mode: indexer::Mode) {
    let state = app.state::<AppState>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut current = state.scan.lock().unwrap();
        if let Some(running) = current.as_ref() {
            if running.library_path == library_path && mode == indexer::Mode::Changes {
                return;
            }
            running.cancel.store(true, Ordering::Relaxed);
        }
        *current = Some(RunningScan {
            library_path: library_path.to_owned(),
            cancel: cancel.clone(),
        });
    }

    let app = app.clone();
    let library_path = library_path.to_owned();
    let cache = state.thumbs.clone();
    std::thread::spawn(move || {
        let result = run_scan(&app, &library_path, mode, &cache, &cancel);

        // Clear the slot first (if it is still ours), so that by the time the
        // UI hears the scan is over, a new one can be started.
        {
            let state = app.state::<AppState>();
            let mut current = state.scan.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|running| Arc::ptr_eq(&running.cancel, &cancel))
            {
                *current = None;
            }
        }

        let event = match result {
            Ok((summary, thumbnails)) => IndexEvent::Finished {
                library_path,
                summary,
                thumbnails,
            },
            Err(indexer::ScanError::RootUnreachable(_)) => {
                app.state::<AppState>()
                    .update_open(&library_path, |open| open.online = false);
                IndexEvent::Offline { library_path }
            }
            Err(e) => IndexEvent::Failed {
                library_path,
                message: e.to_string(),
            },
        };
        let _ = app.emit(INDEX_EVENT, event);
    });
}

/// The body of a background scan: merge the organization logs, index the
/// folder, then make the missing thumbnails. Emits events along the way.
fn run_scan(
    app: &AppHandle,
    library_path: &str,
    mode: indexer::Mode,
    cache: &ThumbCache,
    cancel: &AtomicBool,
) -> Result<(ScanSummary, ThumbSummary), indexer::ScanError> {
    let state = app.state::<AppState>();
    let root = Path::new(library_path);
    let emit = |event: IndexEvent| {
        let _ = app.emit(INDEX_EVENT, event);
    };
    let on_progress = |progress| {
        emit(IndexEvent::Running {
            library_path: library_path.to_owned(),
            progress,
        })
    };

    let mut conn = db::open(&state.index_db(library_path))?;

    if root.is_dir() {
        let paths = state.org_paths(library_path);
        let writable = paths.shared_is_writable(&state.device);
        state.update_open(library_path, |open| {
            open.online = true;
            open.writable = writable;
        });
        emit(IndexEvent::Online {
            library_path: library_path.to_owned(),
            writable,
        });
        if org::sync(&mut conn, &paths, &state.device, writable)? > 0 {
            emit(IndexEvent::Synced {
                library_path: library_path.to_owned(),
            });
        }
    }

    let summary = match indexer::scan_with(root, &mut conn, mode, cancel, &on_progress) {
        Ok(summary) => summary,
        Err(e @ indexer::ScanError::RootUnreachable(_)) => {
            // Similar shots and faces are found in the cached thumbnails: no
            // need for the library to be reachable.
            similar::hash_pending(&mut conn, cache, cancel, &on_progress)?;
            find_people(app, library_path, &mut conn, cache, cancel, &on_progress)?;
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    if summary.cancelled {
        return Ok((summary, ThumbSummary::default()));
    }
    emit(IndexEvent::Indexed {
        library_path: library_path.to_owned(),
        summary: summary.clone(),
    });

    let thumbnails = thumbs::generate_missing(root, &mut conn, cache, cancel, &on_progress)?;
    // A JPEG that turned out not to be decodable can't stand for its RAW
    // file: show the RAW file by itself again.
    if thumbnails.undecodable > 0 {
        db::pair_parts(&mut conn)?;
    }

    // Then what is worked out from the thumbnails: which photos look alike,
    // and last, only if the user switched it on, the faces.
    if !thumbnails.cancelled {
        similar::hash_pending(&mut conn, cache, cancel, &on_progress)?;
        find_people(app, library_path, &mut conn, cache, cancel, &on_progress)?;
    }
    Ok((summary, thumbnails))
}

/// If face grouping is switched on: looks for faces in the photos not
/// searched yet, and puts them into groups.
fn find_people(
    app: &AppHandle,
    library_path: &str,
    conn: &mut rusqlite::Connection,
    cache: &ThumbCache,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(indexer::Progress),
) -> rusqlite::Result<()> {
    let Some(finder) = face_finder(&app.state::<AppState>()) else {
        return Ok(());
    };
    let searched = people::find_faces(conn, cache, &finder, cancel, on_progress)?;
    if people::group_faces(conn)? > 0 || searched > 0 {
        // Makes the UI read the people again.
        let _ = app.emit(
            INDEX_EVENT,
            IndexEvent::Synced {
                library_path: library_path.to_owned(),
            },
        );
    }
    Ok(())
}

/// The loaded face models, or `None` if face grouping is not switched on
/// (the models were never downloaded) or they can't be loaded.
fn face_finder(state: &AppState) -> Option<Arc<faces::FaceFinder>> {
    let mut slot = state.face_finder.lock().unwrap();
    if faces::switched_off(&state.data_dir) {
        *slot = None; // frees the models' memory
        return None;
    }
    if slot.is_none() && faces::models_present(&state.data_dir) {
        *slot = faces::FaceFinder::load(&state.data_dir).ok().map(Arc::new);
    }
    slot.clone()
}

/// Answers a `thumb://localhost/<grid|preview>/<fingerprint>` request with the
/// cached JPEG, making it first if it is missing and the library is reachable.
fn thumb_response(app: &AppHandle, uri_path: &str) -> Response<Vec<u8>> {
    let status = |code: StatusCode| {
        Response::builder()
            .status(code)
            .body(Vec::new())
            .expect("static response")
    };

    // The UI builds the URL with `convertFileSrc`, which percent-encodes the '/'.
    let path = uri_path.replace("%2F", "/").replace("%2f", "/");
    let Some((size, fingerprint)) = path.trim_start_matches('/').split_once('/') else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Some(size) = ThumbSize::parse(size) else {
        return status(StatusCode::BAD_REQUEST);
    };
    if !thumbs::is_fingerprint(fingerprint) {
        return status(StatusCode::BAD_REQUEST);
    }

    let state = app.state::<AppState>();
    let file = state.thumbs.path(size, fingerprint);
    let bytes = fs::read(&file).or_else(|_| {
        make_thumb_on_demand(&state, fingerprint)?;
        fs::read(&file).map_err(drop)
    });
    match bytes {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            // Readable by a canvas: a memory is drawn on one to make its video.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            // The URL names the content, so the webview may cache it forever.
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .body(bytes)
            .expect("static response"),
        Err(()) => status(StatusCode::NOT_FOUND),
    }
}

/// Finds the original file of a photo of the open library, and its EXIF
/// orientation. Only files that are in the index can be reached this way.
fn find_original(state: &AppState, fingerprint: &str) -> Result<(PathBuf, Option<u8>), ()> {
    let library_path = state
        .open_library
        .lock()
        .unwrap()
        .as_ref()
        .map(|open| open.path.clone())
        .ok_or(())?;
    let conn = db::open(&state.index_db(&library_path)).map_err(drop)?;
    let (rel_path, orientation) = db::find_by_fingerprint(&conn, fingerprint)
        .map_err(drop)?
        .ok_or(())?;
    Ok((Path::new(&library_path).join(rel_path), orientation))
}

/// Generates the thumbnails of one photo of the open library right now,
/// instead of waiting for the background pass to reach it.
fn make_thumb_on_demand(state: &AppState, fingerprint: &str) -> Result<(), ()> {
    let (source, orientation) = find_original(state, fingerprint)?;
    // Scrolling through photos that have no thumbnails yet asks for many at
    // once; decode only a few at a time.
    let _permit = state.on_demand.acquire();
    if state.thumbs.has_both(fingerprint) {
        return Ok(()); // made while we were waiting
    }
    let size = if metadata::kind_of(&source) == Some(metadata::Kind::Video) {
        state
            .thumbs
            .generate_from_video(&source, fingerprint)
            .map_err(drop)?;
        None
    } else {
        Some(
            state
                .thumbs
                .generate(&source, fingerprint, orientation)
                .map_err(drop)?,
        )
    };
    // Tell the index, so the background pass doesn't open the file again.
    // Off this thread: the pass may be holding the database for a moment.
    if let Some(open) = state.open_library.lock().unwrap().as_ref() {
        let db_path = state.index_db(&open.path);
        let fingerprint = fingerprint.to_owned();
        std::thread::spawn(move || {
            if let Ok(conn) = db::open(&db_path) {
                let _ = db::mark_thumb_ready(&conn, &fingerprint, size);
            }
        });
    }
    Ok(())
}

/// Answers a `photo://localhost/<fingerprint>` request with the full-size
/// original, for the viewer. Formats the webview can't show are converted to
/// JPEG on the fly. Videos are served in pieces: `range` is the request's
/// `Range` header.
fn photo_response(app: &AppHandle, uri_path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    let status = |code: StatusCode| {
        Response::builder()
            .status(code)
            .body(Vec::new())
            .expect("static response")
    };

    let fingerprint = uri_path.trim_start_matches('/');
    if !thumbs::is_fingerprint(fingerprint) {
        return status(StatusCode::BAD_REQUEST);
    }
    let state = app.state::<AppState>();
    let Ok((source, orientation)) = find_original(&state, fingerprint) else {
        return status(StatusCode::NOT_FOUND);
    };
    if metadata::kind_of(&source) == Some(metadata::Kind::Video) {
        return video_response(&source, range).unwrap_or_else(|_| status(StatusCode::NOT_FOUND));
    }

    let result = match thumbs::is_web_image(&source) {
        Some(mime) => fs::read(&source).map(|bytes| (mime, bytes)).map_err(drop),
        None => {
            let _permit = state.on_demand.acquire();
            thumbs::render_jpeg(&source, orientation)
                .map(|bytes| ("image/jpeg", bytes))
                .map_err(drop)
        }
    };
    match result {
        Ok((mime, bytes)) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .body(bytes)
            .expect("static response"),
        Err(()) => status(StatusCode::NOT_FOUND),
    }
}

/// The most video data sent in one response. Players ask for the rest as
/// they need it, so a long video is never loaded into memory whole.
const VIDEO_CHUNK: u64 = 4 * 1024 * 1024;

/// Serves a video file, honoring `Range` requests: that is how the player
/// seeks, and WebKit refuses to play a video from a server without them.
fn video_response(source: &Path, range: Option<&str>) -> std::io::Result<Response<Vec<u8>>> {
    use std::io::{Read, Seek, SeekFrom};

    let mime = media_server::mime_of(source);
    let mut file = fs::File::open(source)?;
    let len = file.metadata()?.len();
    let response = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=3600");

    let Some(range) = range else {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return Ok(response.body(bytes).expect("static response"));
    };
    let Some((first, last)) = media_server::parse_range(range, len) else {
        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .body(Vec::new())
            .expect("static response"));
    };
    let last = last.min(first + VIDEO_CHUNK - 1);
    let mut bytes = vec![0u8; (last - first + 1) as usize];
    file.seek(SeekFrom::Start(first))?;
    file.read_exact(&mut bytes)?;
    Ok(response
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}"))
        .body(bytes)
        .expect("static response"))
}

/// Lets at most `permits` threads through at a time.
struct Limiter {
    available: Mutex<usize>,
    freed: Condvar,
}

struct Permit<'a>(&'a Limiter);

impl Limiter {
    fn new(permits: usize) -> Self {
        Self {
            available: Mutex::new(permits),
            freed: Condvar::new(),
        }
    }

    fn acquire(&self) -> Permit<'_> {
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            available = self.freed.wait(available).unwrap();
        }
        *available -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.available.lock().unwrap() += 1;
        self.0.freed.notify_one();
    }
}

// The commands are marked `async` so they run off the main thread: a stalled
// network mount must not freeze the window.

#[tauri::command(async)]
fn open_library(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibraryInfo, String> {
    let dir = Path::new(&path);
    // E.g. a photo dropped on the window instead of its folder.
    if dir.is_file() {
        return Err(format!(
            "\"{}\" is a file. Open the folder that holds your photos.",
            library_name(dir)
        ));
    }
    // Listing it, not just seeing that it exists, is what proves it is reachable.
    let online = dir.is_dir() && fs::read_dir(dir).is_ok();
    let mut writable = false;
    if online {
        // Creates `.photolib/library.json` the first time, unless the folder
        // is read-only.
        if let Some(meta) = org::read_or_create_meta(dir).map_err(|e| e.to_string())? {
            state.remember_key(&path, &meta.id);
        }
        writable = state.org_paths(&path).shared_is_writable(&state.device);
    } else if !state.index_db(&path).is_file() {
        return Err(format!(
            "Can't open \"{path}\": the folder is not reachable."
        ));
    }

    let name = library_name(dir);
    {
        let store = state.recents.lock().map_err(|e| e.to_string())?;
        let mut list = store.load();
        recents::touch(&mut list, &path, &name, now_ms());
        store
            .save(&list)
            .map_err(|e| format!("Couldn't save the recent libraries list: {e}"))?;
    }

    *state.open_library.lock().unwrap() = Some(OpenLibrary {
        path: path.clone(),
        online,
        writable,
    });
    start_scan(&app, &path, indexer::Mode::Changes);

    Ok(LibraryInfo {
        path,
        name,
        writable,
        online,
    })
}

/// The base URL videos are loaded from, when it is not `photo://`.
#[tauri::command]
fn media_base(state: State<'_, AppState>) -> Option<String> {
    state.media_base.clone()
}

#[tauri::command(async)]
fn list_recent_libraries(state: State<'_, AppState>) -> Result<Vec<RecentLibrary>, String> {
    let list = state.recents.lock().map_err(|e| e.to_string())?.load();
    Ok(with_availability(list))
}

#[tauri::command(async)]
fn remove_recent_library(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibrary>, String> {
    let store = state.recents.lock().map_err(|e| e.to_string())?;
    let mut list = store.load();
    recents::remove(&mut list, &path);
    store
        .save(&list)
        .map_err(|e| format!("Couldn't save the recent libraries list: {e}"))?;
    Ok(with_availability(list))
}

/// Checks a library for new and changed files in the background (does
/// nothing if it is already being scanned), or with `everything`, reads
/// every file again.
#[tauri::command(async)]
fn rescan_library(path: String, everything: Option<bool>, app: AppHandle) {
    let mode = if everything.unwrap_or(false) {
        indexer::Mode::Everything
    } else {
        indexer::Mode::Changes
    };
    start_scan(&app, &path, mode);
}

/// Stops the scan of `path`, if one is running. What it indexed so far is kept.
#[tauri::command(async)]
fn close_library(path: String, state: State<'_, AppState>) {
    {
        let mut open = state.open_library.lock().unwrap();
        if open.as_ref().is_some_and(|open| open.path == path) {
            *open = None;
        }
    }
    if let Some(running) = state.scan.lock().unwrap().as_ref() {
        if running.library_path == path {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[tauri::command(async)]
fn library_stats(path: String, state: State<'_, AppState>) -> Result<LibraryStats, String> {
    let scanning = state
        .scan
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|running| running.library_path == path);
    let conn = db::open(&state.index_db(&path)).map_err(|e| e.to_string())?;
    Ok(LibraryStats {
        photo_count: db::count(&conn).map_err(|e| e.to_string())?,
        scanning,
    })
}

/// Opens the index of the library at `path` for a query from the UI.
fn open_index(state: &AppState, path: &str) -> Result<rusqlite::Connection, String> {
    db::open(&state.index_db(path)).map_err(|e| e.to_string())
}

/// The months of a listing with a few cover photos each, newest first.
#[tauri::command(async)]
fn months(
    path: String,
    listing: db::Listing,
    state: State<'_, AppState>,
) -> Result<Vec<db::MonthSummary>, String> {
    db::months(&open_index(&state, &path)?, &listing).map_err(|e| e.to_string())
}

/// The cameras, lenses, apertures and focal lengths of a listing, for the camera filter.
#[tauri::command(async)]
fn gear_options(
    path: String,
    listing: db::Listing,
    state: State<'_, AppState>,
) -> Result<db::GearOptions, String> {
    db::gear_options(&open_index(&state, &path)?, &listing).map_err(|e| e.to_string())
}

/// Writes a memory's video, sent as the request's body, to the file chosen
/// in the save dialog (its path in the `path` header, percent-encoded).
/// Only `.mp4` files: nothing else is written this way.
#[tauri::command]
fn save_video(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("No video was sent".into());
    };
    let path = request
        .headers()
        .get("path")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned()
        })
        .ok_or("No file was chosen")?;
    let path = std::path::PathBuf::from(path);
    let is_mp4 = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));
    if !is_mp4 || !path.is_absolute() {
        return Err("Videos are saved as .mp4 files".into());
    }
    fs::write(&path, bytes).map_err(|e| e.to_string())
}

/// The town most of a listing's photos were taken in, to name a pinned place.
#[tauri::command(async)]
fn listing_place(
    path: String,
    listing: db::Listing,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    db::listing_place(&open_index(&state, &path)?, &listing).map_err(|e| e.to_string())
}

/// Day groups of a listing, newest first.
#[tauri::command(async)]
fn timeline(
    path: String,
    listing: db::Listing,
    order: Option<db::Order>,
    state: State<'_, AppState>,
) -> Result<Vec<db::DayGroup>, String> {
    db::timeline_in(
        &open_index(&state, &path)?,
        &listing,
        order.unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

/// The end of the release file name of the installer that fits this
/// computer, so that "a new version is available" can link straight to it.
/// `None` when no installer is made for it.
#[tauri::command]
fn release_asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64.dmg"),
        ("macos", "x86_64") => Some("x64.dmg"),
        ("windows", "x86_64") => Some("x64-setup.exe"),
        ("linux", "x86_64") => {
            // Set by the AppImage runtime when the app runs from one.
            if std::env::var_os("APPIMAGE").is_some() {
                Some("amd64.AppImage")
            } else if Path::new("/etc/debian_version").exists() {
                Some("amd64.deb")
            } else if Path::new("/etc/redhat-release").exists() {
                Some("x86_64.rpm")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The shape of every photo of a listing, in grid order (see [`db::shapes`]).
#[tauri::command(async)]
fn shapes(
    path: String,
    listing: db::Listing,
    order: Option<db::Order>,
    state: State<'_, AppState>,
) -> Result<Vec<u16>, String> {
    db::shapes(
        &open_index(&state, &path)?,
        &listing,
        order.unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

/// A slice of a listing's photos, in the same order as [`timeline`].
#[tauri::command(async)]
fn photos_page(
    path: String,
    listing: db::Listing,
    order: Option<db::Order>,
    offset: u32,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<Vec<db::PhotoItem>, String> {
    db::photos_page_in(
        &open_index(&state, &path)?,
        &listing,
        order.unwrap_or_default(),
        offset,
        limit.min(1000),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn folders(path: String, state: State<'_, AppState>) -> Result<Vec<db::FolderEntry>, String> {
    db::folders(&open_index(&state, &path)?).map_err(|e| e.to_string())
}

/// The markers for the part of the map on screen: the photos taken inside
/// the rectangle, grouped for the zoom level.
#[tauri::command(async)]
fn map_clusters(
    path: String,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
    zoom: u8,
    state: State<'_, AppState>,
) -> Result<Vec<geo::Cluster>, String> {
    let conn = open_index(&state, &path)?;
    let points = db::geo_points(&conn, south, west, north, east).map_err(|e| e.to_string())?;
    Ok(geo::cluster(&points, zoom))
}

#[tauri::command(async)]
fn photo_details(
    path: String,
    rel_path: String,
    state: State<'_, AppState>,
) -> Result<Option<db::PhotoDetails>, String> {
    db::photo_details(&open_index(&state, &path)?, &rel_path).map_err(|e| e.to_string())
}

/// How a photo was taken (lens, aperture, shutter speed, ISO…), read from
/// the file. Only photos in the index can be read this way.
#[tauri::command(async)]
fn photo_shooting(
    path: String,
    rel_path: String,
    state: State<'_, AppState>,
) -> Result<Option<metadata::Shooting>, String> {
    let conn = open_index(&state, &path)?;
    if db::find_by_path(&conn, &rel_path)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Ok(None);
    }
    Ok(metadata::shooting(&Path::new(&path).join(&rel_path)))
}

#[tauri::command(async)]
fn albums(path: String, state: State<'_, AppState>) -> Result<Vec<db::Album>, String> {
    db::albums(&open_index(&state, &path)?).map_err(|e| e.to_string())
}

/// Memories, trips, featured photos and the other rows of the Collections
/// page. `today` is the viewer's date, in days since 1970.
#[tauri::command(async)]
fn collections(
    path: String,
    today: i64,
    state: State<'_, AppState>,
) -> Result<collections::Collections, String> {
    collections::collections(&open_index(&state, &path)?, today).map_err(|e| e.to_string())
}

/// The open library, if it is `path` and can take changes right now.
fn editable_library(state: &AppState, path: &str) -> Result<OpenLibrary, String> {
    let open = state.open_library.lock().unwrap().clone();
    match open {
        Some(open) if open.path == path && open.online => Ok(open),
        Some(open) if open.path == path => {
            Err("The library is offline, so changes can't be saved right now.".into())
        }
        _ => Err("This library is not open.".into()),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FacesStatus {
    /// True when faces are searched: the models are on this computer and
    /// face grouping is not switched off.
    enabled: bool,
    /// True once the face models are on this computer.
    downloaded: bool,
    /// Size of the download that switching it on needs.
    download_bytes: u64,
}

#[tauri::command(async)]
fn faces_status(state: State<'_, AppState>) -> FacesStatus {
    let downloaded = faces::models_present(&state.data_dir);
    FacesStatus {
        enabled: downloaded && !faces::switched_off(&state.data_dir),
        downloaded,
        download_bytes: faces::DOWNLOAD_BYTES,
    }
}

/// Switches face grouping on: downloads the models (the UI hears the bytes
/// received so far in `faces-download` events), then scans the open library
/// again so that its photos are searched for faces.
#[tauri::command(async)]
fn enable_faces(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    faces::set_switched_off(&state.data_dir, false)
        .map_err(|e| format!("Couldn't save the setting: {e}"))?;
    faces::download_models(&state.data_dir, &|received| {
        let _ = app.emit("faces-download", received);
    })
    .map_err(|e| format!("Couldn't download the face models: {e}"))?;
    let open = state.open_library.lock().unwrap().clone();
    if let Some(open) = open {
        start_scan(&app, &open.path, indexer::Mode::Changes);
    }
    Ok(())
}

/// Switches face grouping off: no more faces are searched, and the people
/// already found stay. `enable_faces` switches it back on.
#[tauri::command(async)]
fn disable_faces(state: State<'_, AppState>) -> Result<(), String> {
    faces::set_switched_off(&state.data_dir, true)
        .map_err(|e| format!("Couldn't save the setting: {e}"))?;
    *state.face_finder.lock().unwrap() = None;
    Ok(())
}

/// The people found in a library.
#[tauri::command(async)]
fn people(path: String, state: State<'_, AppState>) -> Result<Vec<people::Person>, String> {
    people::people(&open_index(&state, &path)?).map_err(|e| e.to_string())
}

/// Names (or with an empty name, un-names) the person made of `clusters`.
#[tauri::command(async)]
fn name_person(
    path: String,
    clusters: Vec<i64>,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let open = editable_library(&state, &path)?;
    let mut conn = open_index(&state, &path)?;
    let ops = people::naming_ops(&conn, &clusters, name.trim()).map_err(|e| e.to_string())?;
    org::record(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
        ops,
    )
    .map_err(|e| e.to_string())
}

/// Makes the photo `photo` (a fingerprint) the one the card of the person
/// `name`, made of `clusters`, shows.
#[tauri::command(async)]
fn make_key_photo(
    path: String,
    clusters: Vec<i64>,
    name: String,
    photo: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let open = editable_library(&state, &path)?;
    let mut conn = open_index(&state, &path)?;
    let op = people::key_photo_op(&conn, &clusters, name.trim(), &photo)
        .map_err(|e| e.to_string())?
        .ok_or("This person's face isn't found in this photo.")?;
    org::record(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
        vec![op],
    )
    .map_err(|e| e.to_string())
}

/// Unnamed groups of faces that might be the person `name`, made of `clusters`.
#[tauri::command(async)]
fn person_suggestions(
    path: String,
    clusters: Vec<i64>,
    name: String,
    state: State<'_, AppState>,
) -> Result<Vec<people::Person>, String> {
    people::suggestions(&open_index(&state, &path)?, &clusters, &name).map_err(|e| e.to_string())
}

/// Where the faces of a person's groups are in their photos, to show them.
#[tauri::command(async)]
fn person_faces(
    path: String,
    clusters: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<db::FaceBox>, String> {
    db::faces_in(&open_index(&state, &path)?, &clusters).map_err(|e| e.to_string())
}

/// Records that the group `cluster` is not the person `name`: it is no longer
/// suggested for them.
#[tauri::command(async)]
fn reject_suggestion(
    path: String,
    cluster: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    db::add_not_person(&open_index(&state, &path)?, cluster, &name).map_err(|e| e.to_string())
}

/// Hides a person from the People screen, or shows them again.
#[tauri::command(async)]
fn set_person_hidden(
    path: String,
    clusters: Vec<i64>,
    hidden: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    db::set_face_groups_hidden(&mut open_index(&state, &path)?, &clusters, hidden)
        .map_err(|e| e.to_string())
}

/// Takes `photos` out of the person made of `clusters`: their faces there
/// were someone else. If the person has a name, another of their faces
/// carries it from now on.
#[tauri::command(async)]
fn remove_from_person(
    path: String,
    clusters: Vec<i64>,
    photos: Vec<String>,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let name = name.filter(|name| !name.trim().is_empty());
    // Checked first: the name must be able to move to another face.
    let open = match name {
        Some(_) => Some(editable_library(&state, &path)?),
        None => None,
    };
    let mut conn = open_index(&state, &path)?;
    let removed = db::remove_faces(&mut conn, &clusters, &photos).map_err(|e| e.to_string())?;
    if let (Some(name), Some(open)) = (name, open) {
        let ops = people::naming_ops(&conn, &clusters, name.trim()).map_err(|e| e.to_string())?;
        org::record(
            &mut conn,
            &state.org_paths(&path),
            &state.device,
            open.writable,
            ops,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Moves the faces of `photos` in a person's groups into a new group of
/// their own (a group that mixed two people), which is never merged back.
/// If the person has a name, the new group is not them, and another of their
/// faces carries the name. Returns the new group, if any face moved.
#[tauri::command(async)]
fn split_person(
    path: String,
    clusters: Vec<i64>,
    photos: Vec<String>,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<i64>, String> {
    let name = name.filter(|name| !name.trim().is_empty());
    let open = match name {
        Some(_) => Some(editable_library(&state, &path)?),
        None => None,
    };
    let mut conn = open_index(&state, &path)?;
    let Some(new) = db::split_faces(&mut conn, &clusters, &photos).map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if let (Some(name), Some(open)) = (name, open) {
        db::add_not_person(&conn, new, &name).map_err(|e| e.to_string())?;
        // A face that carried the name may have moved: it loses it, and a
        // face that stayed carries it.
        let mut ops = people::naming_ops(&conn, &[new], "").map_err(|e| e.to_string())?;
        ops.extend(people::naming_ops(&conn, &clusters, name.trim()).map_err(|e| e.to_string())?);
        org::record(
            &mut conn,
            &state.org_paths(&path),
            &state.device,
            open.writable,
            ops,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(Some(new))
}

/// Copies photos into `dest`: the originals, or JPEGs with their edits,
/// optionally smaller. Reports `export-progress` as `[done, total]`.
#[tauri::command(async)]
fn export_photos(
    app: AppHandle,
    path: String,
    photos: Vec<String>,
    dest: String,
    size: export::Size,
    state: State<'_, AppState>,
) -> Result<export::Summary, String> {
    let root = Path::new(&path);
    if !root.is_dir() {
        return Err("The library can't be reached right now.".into());
    }
    let items =
        db::export_items(&open_index(&state, &path)?, &photos).map_err(|e| e.to_string())?;
    state.job_cancel.store(false, Ordering::Relaxed);
    export::export(
        root,
        &items,
        Path::new(&dest),
        size,
        &state.job_cancel,
        |done, total| {
            let _ = app.emit("export-progress", [done, total]);
        },
    )
    .map_err(|e| format!("Couldn't write to {dest}: {e}"))
}

/// Stops the export, or the moving of duplicates, that is running.
#[tauri::command]
fn cancel_job(state: State<'_, AppState>) {
    state.job_cancel.store(true, Ordering::Relaxed);
}

/// The magic wand's corrections for a photo, worked out from its preview.
#[tauri::command(async)]
fn auto_enhance(fingerprint: String, state: State<'_, AppState>) -> Result<org::Enhance, String> {
    if !thumbs::is_fingerprint(&fingerprint) {
        return Err("Not a photo.".into());
    }
    let file = state.thumbs.path(ThumbSize::Preview, &fingerprint);
    if !file.exists() {
        make_thumb_on_demand(&state, &fingerprint)
            .map_err(|()| "The photo can't be read.".to_string())?;
    }
    let preview = image::open(file).map_err(|e| e.to_string())?.to_rgb8();
    Ok(enhance::analyze(&preview))
}

/// Draws `edit` into a photo's file (JPEG only), after the user was warned. The photo keeps its favorite, albums, rating, label and named
/// faces, which follow the file's new content.
#[tauri::command(async)]
fn apply_edit_to_original(
    path: String,
    rel_path: String,
    fingerprint: String,
    edit: org::Edit,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !edit.is_valid() {
        return Err("Not a valid edit.".into());
    }
    let open = editable_library(&state, &path)?;
    if !open.writable {
        return Err("The library folder is read-only, so the file can't be changed.".into());
    }
    let mut conn = open_index(&state, &path)?;
    let root = Path::new(&path);
    let (indexed, orientation) = db::find_by_path(&conn, &rel_path)
        .map_err(|e| e.to_string())?
        .ok_or("This library is not open.")?;
    if indexed != fingerprint {
        return Err(
            "The file has changed since it was read. Try again after the next scan.".into(),
        );
    }
    let organization = db::organization_of(&conn, &fingerprint).map_err(|e| e.to_string())?;

    original::rewrite(&root.join(&rel_path), orientation, &edit).map_err(|e| e.to_string())?;

    let new_fingerprint = indexer::reindex_file(root, &conn, &rel_path)
        .map_err(|e| e.to_string())?
        .ok_or("The photo can't be read.")?;
    let ops = original::carried_over(&organization, &new_fingerprint, &edit);
    if ops.is_empty() {
        return Ok(());
    }
    org::record(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
        ops,
    )
    .map_err(|e| e.to_string())
}

/// Memory cards, cameras and other drives with photos (a `DCIM` folder)
/// connected right now.
#[tauri::command(async)]
fn import_sources() -> Vec<importer::Source> {
    importer::sources()
}

/// What importing from `source` (a drive or any folder) would copy.
#[tauri::command(async)]
fn import_preview(
    app: AppHandle,
    path: String,
    source: String,
    state: State<'_, AppState>,
) -> Result<importer::Preview, String> {
    let conn = open_index(&state, &path)?;
    importer::preview(&source, Path::new(&path), &conn, |done, total| {
        let _ = app.emit("import-survey", [done, total]);
    })
    .map_err(|e| e.to_string())
}

/// Copies the new photos and videos of `source` into `<library>/<folder>/<year>/<year>-<month>`.
/// Reports `import-progress`. The caller then checks the library for new files.
#[tauri::command(async)]
fn import_files(
    app: AppHandle,
    path: String,
    source: String,
    folder: String,
    groups: Option<Vec<usize>>,
    raw_files: Option<importer::RawFiles>,
    state: State<'_, AppState>,
) -> Result<importer::Summary, String> {
    let selection: Option<std::collections::HashSet<usize>> =
        groups.map(|groups| groups.into_iter().collect());
    let open = editable_library(&state, &path)?;
    if !open.writable {
        return Err("The library folder is read-only, so nothing can be imported into it.".into());
    }
    let conn = open_index(&state, &path)?;
    state.job_cancel.store(false, Ordering::Relaxed);
    importer::import(
        &source,
        Path::new(&path),
        &conn,
        &folder,
        selection.as_ref(),
        raw_files.unwrap_or_default(),
        &state.device,
        &state.job_cancel,
        |done, total| {
            let _ = app.emit("import-survey", [done, total]);
        },
        |progress| {
            let _ = app.emit("import-progress", progress);
        },
    )
    .map_err(|e| e.to_string())
}

/// What freeing up space on `source` would delete: photos safely in the
/// library, older than `keep_days` days. Uses the last import preview.
#[tauri::command(async)]
fn free_up_plan(
    path: String,
    source: String,
    keep_days: u32,
    state: State<'_, AppState>,
) -> Result<importer::FreeUpPlan, String> {
    editable_library(&state, &path)?;
    let conn = open_index(&state, &path)?;
    importer::free_up_plan(&source, Path::new(&path), &conn, keep_days).map_err(|e| e.to_string())
}

/// Deletes from a phone, camera or memory card the photos safely in the library.
#[tauri::command(async)]
fn free_up(
    path: String,
    source: String,
    keep_days: u32,
    state: State<'_, AppState>,
) -> Result<importer::FreeUpSummary, String> {
    editable_library(&state, &path)?;
    let conn = open_index(&state, &path)?;
    importer::free_up(&source, Path::new(&path), &conn, keep_days).map_err(|e| e.to_string())
}

/// Groups of similar shots (bursts, near-identical photos), newest first.
#[tauri::command(async)]
fn similar_groups(path: String, state: State<'_, AppState>) -> Result<Vec<similar::Group>, String> {
    similar::groups(&open_index(&state, &path)?).map_err(|e| e.to_string())
}

/// Every photo stored more than once, with the copy the app suggests keeping.
#[tauri::command(async)]
fn duplicate_groups(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<duplicates::Group>, String> {
    duplicates::groups(&open_index(&state, &path)?).map_err(|e| e.to_string())
}

/// Moves extra copies into the library's `Duplicates to delete` folder.
/// Reports `duplicates-progress` as `[done, total]`.
#[tauri::command(async)]
fn park_duplicates(
    app: AppHandle,
    path: String,
    choices: Vec<duplicates::Choice>,
    state: State<'_, AppState>,
) -> Result<duplicates::Summary, String> {
    let open = editable_library(&state, &path)?;
    if !open.writable {
        return Err("The library folder is read-only, so copies can't be moved.".into());
    }
    let mut conn = open_index(&state, &path)?;
    state.job_cancel.store(false, Ordering::Relaxed);
    Ok(duplicates::park(
        Path::new(&path),
        &mut conn,
        &choices,
        &state.job_cancel,
        |done, total| {
            let _ = app.emit("duplicates-progress", [done, total]);
        },
    ))
}

/// How many copies wait in the `Duplicates to delete` folder.
#[tauri::command(async)]
fn parked_duplicates(path: String) -> usize {
    duplicates::parked_count(Path::new(&path))
}

/// Moves the copies in `Duplicates to delete` back where they came from,
/// then looks for them again.
#[tauri::command(async)]
fn put_back_duplicates(
    app: AppHandle,
    path: String,
    state: State<'_, AppState>,
) -> Result<duplicates::Summary, String> {
    editable_library(&state, &path)?;
    let summary = duplicates::put_back(Path::new(&path));
    if summary.moved > 0 {
        start_scan(&app, &path, indexer::Mode::Changes);
    }
    Ok(summary)
}

/// Says what importing an Apple Photos library would do, without doing it.
#[tauri::command(async)]
fn apple_import_preview(
    path: String,
    photos_library: String,
    state: State<'_, AppState>,
) -> Result<apple::ImportSummary, String> {
    let conn = open_index(&state, &path)?;
    Ok(apple::plan(&conn, Path::new(&photos_library))?.summary)
}

/// Imports favorites, hidden photos and albums from an Apple Photos library.
/// The Photos library is only read.
#[tauri::command(async)]
fn apple_import(
    path: String,
    photos_library: String,
    state: State<'_, AppState>,
) -> Result<apple::ImportSummary, String> {
    let open = editable_library(&state, &path)?;
    let mut conn = open_index(&state, &path)?;
    let plan = apple::plan(&conn, Path::new(&photos_library))?;
    let ops = apple::ops(&conn, &plan)?;
    org::record(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
        ops,
    )
    .map_err(|e| e.to_string())?;
    Ok(plan.summary)
}

/// Records organization changes (favorite, hide, album edits) made in the UI.
#[tauri::command(async)]
fn apply_ops(path: String, ops: Vec<org::Op>, state: State<'_, AppState>) -> Result<(), String> {
    let open = editable_library(&state, &path)?;
    let mut conn = open_index(&state, &path)?;
    org::record(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
        ops,
    )
    .map_err(|e| e.to_string())
}

/// Creates an album and returns its id.
#[tauri::command(async)]
fn create_album(path: String, name: String, state: State<'_, AppState>) -> Result<String, String> {
    let album = org::new_id().map_err(|e| e.to_string())?;
    apply_ops(
        path,
        vec![org::Op::AlbumName {
            album: album.clone(),
            name,
        }],
        state,
    )?;
    Ok(album)
}

/// Merges changes other devices made since the last look. Returns true if
/// there were any. Cheap enough to call every minute.
#[tauri::command(async)]
fn sync_library(path: String, state: State<'_, AppState>) -> Result<bool, String> {
    let Ok(open) = editable_library(&state, &path) else {
        return Ok(false);
    };
    let mut conn = open_index(&state, &path)?;
    org::sync(
        &mut conn,
        &state.org_paths(&path),
        &state.device,
        open.writable,
    )
    .map(|read| read > 0)
    .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Workers for commands: enough that slow ones never take them all.
const COMMAND_WORKERS: usize = 32;

pub fn run() {
    // Before anything reads them: the folders the app kept as Photolib.
    rename::move_old_data();
    // Commands run their database queries right on the runtime's workers. The
    // default, one worker per core, let a few slow ones (people, suggestions,
    // a sync over the network) hold up the quick ones the user waits for,
    // such as the photos of the "Is it …?" dialog.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(COMMAND_WORKERS)
        .enable_all()
        .build()
        .expect("the command runtime");
    tauri::async_runtime::set(runtime.handle().clone());
    // It lives as long as the app.
    std::mem::forget(runtime);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let device = org::device_id(&data_dir)?;
            let keys = fs::read(data_dir.join("library-keys.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let media_base = if cfg!(target_os = "linux") {
                let handle = app.handle().clone();
                // Serves only videos that are in the index of the open library.
                let resolve = move |fingerprint: &str| {
                    if !thumbs::is_fingerprint(fingerprint) {
                        return None;
                    }
                    let (source, _) =
                        find_original(&handle.state::<AppState>(), fingerprint).ok()?;
                    (metadata::kind_of(&source) == Some(metadata::Kind::Video)).then_some(source)
                };
                // Without it, videos just don't play: not worth failing startup.
                media_server::start(Arc::new(resolve)).ok()
            } else {
                None
            };
            app.manage(AppState {
                face_finder: Mutex::new(None),
                job_cancel: Arc::new(AtomicBool::new(false)),
                media_base,
                device,
                keys: Mutex::new(keys),
                recents: Mutex::new(RecentStore::new(&data_dir)),
                scan: Mutex::new(None),
                thumbs: ThumbCache::new(&data_dir),
                open_library: Mutex::new(None),
                on_demand: Limiter::new(
                    std::thread::available_parallelism().map_or(2, |n| n.get().div_ceil(2)),
                ),
                data_dir,
            });
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("thumb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            // Not on the rayon pool: the background thumbnail pass keeps that
            // busy, and what the user is looking at must not queue behind it.
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(thumb_response(&app, &path));
            });
        })
        // `importthumb://localhost/<generation>/<file>`: thumbnails in the import window.
        .register_asynchronous_uri_scheme_protocol("importthumb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            // `convertFileSrc` percent-encodes the '/'.
            let path = request.uri().path().replace("%2F", "/").replace("%2f", "/");
            tauri::async_runtime::spawn_blocking(move || {
                let mut parts = path.trim_start_matches('/').split('/');
                let generation = parts.next().and_then(|p| p.parse().ok());
                let id = parts.next().and_then(|p| p.parse().ok());
                let jpeg = generation.zip(id).and_then(|(generation, id)| {
                    let state = app.state::<AppState>();
                    let _permit = state.on_demand.acquire();
                    importer::thumbnail(generation, id)
                });
                let response = match jpeg {
                    Some(bytes) => Response::builder()
                        .header(header::CONTENT_TYPE, "image/jpeg")
                        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                        .body(bytes),
                    None => Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Vec::new()),
                };
                responder.respond(response.expect("static response"));
            });
        })
        .register_asynchronous_uri_scheme_protocol("photo", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            let range = request
                .headers()
                .get(header::RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(photo_response(&app, &path, range.as_deref()));
            });
        })
        .invoke_handler(tauri::generate_handler![
            open_library,
            media_base,
            list_recent_libraries,
            remove_recent_library,
            rescan_library,
            close_library,
            library_stats,
            timeline,
            listing_place,
            save_video,
            photos_page,
            shapes,
            release_asset_suffix,
            folders,
            collections,
            photo_details,
            photo_shooting,
            gear_options,
            apple_import_preview,
            apple_import,
            faces_status,
            enable_faces,
            disable_faces,
            person_suggestions,
            make_key_photo,
            person_faces,
            reject_suggestion,
            set_person_hidden,
            remove_from_person,
            split_person,
            export_photos,
            cancel_job,
            duplicate_groups,
            similar_groups,
            auto_enhance,
            apply_edit_to_original,
            import_sources,
            import_preview,
            import_files,
            free_up_plan,
            free_up,
            park_duplicates,
            parked_duplicates,
            put_back_duplicates,
            people,
            name_person,
            months,
            map_clusters,
            albums,
            apply_ops,
            create_album,
            sync_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_name_is_the_folder_name() {
        assert_eq!(library_name(Path::new("/mnt/nas/Photos")), "Photos");
        assert_eq!(library_name(Path::new("/")), "/");
    }
}
