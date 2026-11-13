mod recents;

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Manager, State};

use recents::{RecentEntry, RecentStore};

/// Shared app state. The mutex serializes read-modify-write of the recents file.
struct AppState {
    recents: Mutex<RecentStore>,
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

// The commands are marked `async` so they run off the main thread: a stalled
// network mount must not freeze the window.

#[tauri::command(async)]
fn open_library(path: String, state: State<'_, AppState>) -> Result<LibraryInfo, String> {
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("Can't open \"{path}\": the folder is not reachable."));
    }
    // Make sure we can actually list it, not just see that it exists.
    fs::read_dir(dir).map_err(|e| format!("Can't read \"{path}\": {e}"))?;

    let name = library_name(dir);

    let store = state.recents.lock().map_err(|e| e.to_string())?;
    let mut list = store.load();
    recents::touch(&mut list, &path, &name, now_ms());
    store
        .save(&list)
        .map_err(|e| format!("Couldn't save the recent libraries list: {e}"))?;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            app.manage(AppState {
                recents: Mutex::new(RecentStore::new(&data_dir)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_library,
            list_recent_libraries,
            remove_recent_library
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
