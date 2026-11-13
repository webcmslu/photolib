mod db;
mod fingerprint;
mod indexer;
mod metadata;
mod recents;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use indexer::{Progress, ScanSummary};
use recents::{RecentEntry, RecentStore};

/// Name of the event that carries [`IndexEvent`]s to the UI.
const INDEX_EVENT: &str = "index-event";

/// Shared app state.
struct AppState {
    data_dir: PathBuf,
    /// The mutex serializes read-modify-write of the recents file.
    recents: Mutex<RecentStore>,
    /// The scan that is running now, if any. At most one runs at a time.
    scan: Mutex<Option<RunningScan>>,
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
    Finished {
        library_path: String,
        summary: ScanSummary,
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
    std::thread::spawn(move || {
        let result = db::open(&db_path)
            .map_err(indexer::ScanError::from)
            .and_then(|mut conn| {
                indexer::scan(Path::new(&library_path), &mut conn, &cancel, &|progress| {
                    let _ = app.emit(
                        INDEX_EVENT,
                        IndexEvent::Running {
                            library_path: library_path.clone(),
                            progress,
                        },
                    );
                })
            });

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
            Ok(summary) => IndexEvent::Finished {
                library_path,
                summary,
            },
            Err(e) => IndexEvent::Failed {
                library_path,
                message: e.to_string(),
            },
        };
        let _ = app.emit(INDEX_EVENT, event);
    });
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            app.manage(AppState {
                recents: Mutex::new(RecentStore::new(&data_dir)),
                scan: Mutex::new(None),
                data_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_library,
            list_recent_libraries,
            remove_recent_library,
            rescan_library,
            close_library,
            library_stats
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
