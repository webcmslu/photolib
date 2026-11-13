mod db;
mod fingerprint;
mod indexer;
mod metadata;
mod recents;
mod thumbs;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};

use indexer::{Progress, ScanSummary};
use recents::{RecentEntry, RecentStore};
use thumbs::{ThumbCache, ThumbSize, ThumbSummary};

/// Name of the event that carries [`IndexEvent`]s to the UI.
const INDEX_EVENT: &str = "index-event";

/// Shared app state.
struct AppState {
    data_dir: PathBuf,
    /// The mutex serializes read-modify-write of the recents file.
    recents: Mutex<RecentStore>,
    /// The scan that is running now, if any. At most one runs at a time.
    scan: Mutex<Option<RunningScan>>,
    thumbs: ThumbCache,
    /// Path of the library that is open in the window, if any. The `thumb://`
    /// protocol uses it to make a missing thumbnail on demand.
    open_library: Mutex<Option<String>>,
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

/// Checks whether we can create files in `dir` by creating and deleting a probe
/// file. Permission bits are not trustworthy on SMB/NFS/exFAT, so we try for real.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".photolib-probe-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Where the local index of the library at `library_path` lives.
///
/// Keyed by a hash of the path for now. Once `.photolib/library.json` exists
/// (milestone 5) the library id will be the key, so that the same library
/// mounted at a different path reuses its index.
fn index_db_path(data_dir: &Path, library_path: &str) -> PathBuf {
    let key = blake3::hash(library_path.as_bytes()).to_hex();
    data_dir
        .join("libraries")
        .join(&key.as_str()[..16])
        .join("index.db")
}

/// Starts a background scan of `library_path` unless one is already running
/// for it. A running scan of a different library is cancelled first.
fn start_scan(app: &AppHandle, library_path: &str) {
    let state = app.state::<AppState>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut current = state.scan.lock().unwrap();
        if let Some(running) = current.as_ref() {
            if running.library_path == library_path {
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
    let db_path = index_db_path(&state.data_dir, &library_path);
    let cache = state.thumbs.clone();
    std::thread::spawn(move || {
        let result = run_scan(&app, &library_path, &db_path, &cache, &cancel);

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
            Err(e) => IndexEvent::Failed {
                library_path,
                message: e.to_string(),
            },
        };
        let _ = app.emit(INDEX_EVENT, event);
    });
}

/// The body of a background scan: index the folder, then make the missing
/// thumbnails. Emits `Running` and `Indexed` events along the way.
fn run_scan(
    app: &AppHandle,
    library_path: &str,
    db_path: &Path,
    cache: &ThumbCache,
    cancel: &AtomicBool,
) -> Result<(ScanSummary, ThumbSummary), indexer::ScanError> {
    let root = Path::new(library_path);
    let on_progress = |progress| {
        let _ = app.emit(
            INDEX_EVENT,
            IndexEvent::Running {
                library_path: library_path.to_owned(),
                progress,
            },
        );
    };

    let mut conn = db::open(db_path)?;
    let summary = indexer::scan(root, &mut conn, cancel, &on_progress)?;
    if summary.cancelled {
        return Ok((summary, ThumbSummary::default()));
    }
    let _ = app.emit(
        INDEX_EVENT,
        IndexEvent::Indexed {
            library_path: library_path.to_owned(),
            summary: summary.clone(),
        },
    );

    let thumbnails = thumbs::generate_missing(root, &mut conn, cache, cancel, &on_progress)?;
    Ok((summary, thumbnails))
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
            // The URL names the content, so the webview may cache it forever.
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .body(bytes)
            .expect("static response"),
        Err(()) => status(StatusCode::NOT_FOUND),
    }
}

/// Generates the thumbnails of one photo of the open library right now,
/// instead of waiting for the background pass to reach it.
fn make_thumb_on_demand(state: &AppState, fingerprint: &str) -> Result<(), ()> {
    let library_path = state.open_library.lock().unwrap().clone().ok_or(())?;
    let conn = db::open(&index_db_path(&state.data_dir, &library_path)).map_err(drop)?;
    let (rel_path, orientation) = db::find_by_fingerprint(&conn, fingerprint)
        .map_err(drop)?
        .ok_or(())?;
    drop(conn);
    state
        .thumbs
        .generate(
            &Path::new(&library_path).join(rel_path),
            fingerprint,
            orientation,
        )
        .map(drop)
        .map_err(drop)
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
    if !dir.is_dir() {
        return Err(format!(
            "Can't open \"{path}\": the folder is not reachable."
        ));
    }
    // Make sure we can actually list it, not just see that it exists.
    fs::read_dir(dir).map_err(|e| format!("Can't read \"{path}\": {e}"))?;

    let name = library_name(dir);

    {
        let store = state.recents.lock().map_err(|e| e.to_string())?;
        let mut list = store.load();
        recents::touch(&mut list, &path, &name, now_ms());
        store
            .save(&list)
            .map_err(|e| format!("Couldn't save the recent libraries list: {e}"))?;
    }

    *state.open_library.lock().unwrap() = Some(path.clone());
    start_scan(&app, &path);

    Ok(LibraryInfo {
        writable: is_writable(dir),
        path,
        name,
    })
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

/// Rescans a library in the background. Does nothing if it is already being scanned.
#[tauri::command(async)]
fn rescan_library(path: String, app: AppHandle) {
    start_scan(&app, &path);
}

/// Stops the scan of `path`, if one is running. What it indexed so far is kept.
#[tauri::command(async)]
fn close_library(path: String, state: State<'_, AppState>) {
    {
        let mut open = state.open_library.lock().unwrap();
        if open.as_deref() == Some(path.as_str()) {
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
    let conn = db::open(&index_db_path(&state.data_dir, &path)).map_err(|e| e.to_string())?;
    Ok(LibraryStats {
        photo_count: db::count(&conn).map_err(|e| e.to_string())?,
        scanning,
    })
}

/// The newest photos of a library, newest first.
#[tauri::command(async)]
fn newest_photos(
    path: String,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<Vec<db::PhotoItem>, String> {
    let conn = db::open(&index_db_path(&state.data_dir, &path)).map_err(|e| e.to_string())?;
    db::newest_photos(&conn, limit.min(1000)).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            app.manage(AppState {
                recents: Mutex::new(RecentStore::new(&data_dir)),
                scan: Mutex::new(None),
                thumbs: ThumbCache::new(&data_dir),
                open_library: Mutex::new(None),
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
        .invoke_handler(tauri::generate_handler![
            open_library,
            list_recent_libraries,
            remove_recent_library,
            rescan_library,
            close_library,
            library_stats,
            newest_photos
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

    #[test]
    fn writable_probe_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_writable(dir.path()));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(!is_writable(&dir.path().join("does-not-exist")));
    }
}
