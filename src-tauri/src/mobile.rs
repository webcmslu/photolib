//! The phone app (iOS, Android): it shows the library of a Phototek server
//! (see server/), so the interface talks to it over HTTP. The commands here
//! are the phone's own: backing up its photos to that server and freeing up
//! space once they are there (the photo library itself is reached through
//! plugins/photos).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_photos::{Asset, PhotosExt};

use crate::phone_fingerprint::fingerprint;

#[tauri::mobile_entry_point]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_photos::init())
        .manage(Backup::default())
        .setup(|_app| {
            #[cfg(target_os = "ios")]
            full_screen_page(_app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            phone_name,
            phone_backup_status,
            start_phone_backup,
            stop_phone_backup,
            phone_space_to_free,
            free_phone_space,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Phototek");
}

/// WKWebView shrinks the page by the safe areas (the notch, the home
/// indicator) unless told not to. The page then fills the screen and keeps
/// clear of them itself, with `env(safe-area-inset-*)`.
#[cfg(target_os = "ios")]
fn full_screen_page(app: &tauri::App) -> tauri::Result<()> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    /// UIScrollViewContentInsetAdjustmentBehavior.never
    const NEVER: isize = 2;
    if let Some(window) = app.get_webview_window("main") {
        window.with_webview(|webview| unsafe {
            let wk = webview.inner() as *mut AnyObject;
            let scroll: *mut AnyObject = msg_send![wk, scrollView];
            let _: () = msg_send![scroll, setContentInsetAdjustmentBehavior: NEVER];
        })?;
    }
    Ok(())
}

/// The server the interface is connected to, as it passes it along.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Server {
    url: String,
    token: Option<String>,
}

#[derive(Default)]
struct Backup {
    running: AtomicBool,
    stop: AtomicBool,
    /// Serializes reading and writing the record of what was saved.
    record: Mutex<()>,
}

/// How the backup is going, sent as `phone-backup` events.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    running: bool,
    /// Photos and videos on the phone.
    total: usize,
    /// How many of them are on the server.
    saved: usize,
    /// Sent during this backup (the others the server had already).
    sent: usize,
    /// Couldn't be read or sent this time (tried again next time).
    failed: usize,
    /// Why the backup stopped, if it failed.
    error: Option<String>,
}

/// What the phone recorded as saved on one server: asset id → the
/// fingerprints of its files (a Live Photo has two).
#[derive(Default, Serialize, Deserialize)]
struct Record {
    saved: HashMap<String, Vec<String>>,
}

fn record_path(app: &AppHandle, server: &Server) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let key = blake3::hash(server.url.trim_end_matches('/').as_bytes()).to_hex();
    Ok(dir.join(format!("backup-{}.json", &key[..16])))
}

fn read_record(path: &Path) -> Record {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_record(path: &Path, record: &Record) -> Result<(), String> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, serde_json::to_vec(record).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build()
}

fn authorized(request: ureq::Request, server: &Server) -> ureq::Request {
    match &server.token {
        Some(token) => request.set("Authorization", &format!("Bearer {token}")),
        None => request,
    }
}

fn api_url(server: &Server, path: &str) -> String {
    format!("{}/api/{path}", server.url.trim_end_matches('/'))
}

/// Which of these fingerprints the server's library has.
fn known_on_server(server: &Server, fingerprints: &[String]) -> Result<Vec<String>, String> {
    let mut known = Vec::new();
    for batch in fingerprints.chunks(1000) {
        let response = authorized(agent().post(&api_url(server, "known_photos")), server)
            .send_json(json!({ "fingerprints": batch }))
            .map_err(describe)?;
        let mut found: Vec<String> = response.into_json().map_err(|e| e.to_string())?;
        known.append(&mut found);
    }
    Ok(known)
}

/// A server error's own message (the server answers with a JSON string).
fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(_, response) => response
            .into_json::<String>()
            .unwrap_or_else(|_| "The Phototek server refused it.".to_owned()),
        other => format!("Can't reach the Phototek server: {other}"),
    }
}

/// The phone's name, as its owner set it (iOS gives only "iPhone" to apps).
#[tauri::command(async)]
fn phone_name(app: AppHandle) -> Result<String, String> {
    app.photos().device_name().map_err(|e| e.to_string())
}

/// How many photos are on the phone, and how many of them on `server`.
#[tauri::command(async)]
fn phone_backup_status(
    app: AppHandle,
    server: Server,
    backup: State<'_, Backup>,
) -> Result<Progress, String> {
    let access = app.photos().request_access().map_err(|e| e.to_string())?;
    if !access.granted {
        return Err(
            "Phototek may not see this phone's photos: allow it in the Settings app.".into(),
        );
    }
    let assets = app.photos().list().map_err(|e| e.to_string())?;
    let path = record_path(&app, &server)?;
    let record = {
        let _lock = backup.record.lock().unwrap();
        read_record(&path)
    };
    Ok(Progress {
        running: backup.running.load(Ordering::Relaxed),
        total: assets.len(),
        saved: assets
            .iter()
            .filter(|asset| record.saved.contains_key(&asset.id))
            .count(),
        ..Progress::default()
    })
}

/// Sends the photos and videos the server doesn't have yet, oldest first,
/// in the background; `phone-backup` events tell how it goes.
#[tauri::command(async)]
fn start_phone_backup(
    app: AppHandle,
    server: Server,
    device: String,
    backup: State<'_, Backup>,
) -> Result<(), String> {
    if backup.running.swap(true, Ordering::SeqCst) {
        return Ok(()); // already running
    }
    backup.stop.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let backup = app.state::<Backup>();
        let mut progress = Progress {
            running: true,
            ..Progress::default()
        };
        if let Err(e) = back_up(&app, &server, &device, &backup, &mut progress) {
            progress.error = Some(e);
        }
        backup.running.store(false, Ordering::SeqCst);
        progress.running = false;
        eprintln!(
            "Phone backup: {} of {} saved, {} sent, {} failed{}",
            progress.saved,
            progress.total,
            progress.sent,
            progress.failed,
            progress
                .error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        );
        let _ = app.emit("phone-backup", &progress);
    });
    Ok(())
}

#[tauri::command]
fn stop_phone_backup(backup: State<'_, Backup>) {
    backup.stop.store(true, Ordering::SeqCst);
}

/// How many photos at a time are copied out, checked and sent.
const BATCH: usize = 10;

fn back_up(
    app: &AppHandle,
    server: &Server,
    device: &str,
    backup: &Backup,
    progress: &mut Progress,
) -> Result<(), String> {
    let photos = app.photos();
    let access = photos.request_access().map_err(|e| e.to_string())?;
    if !access.granted {
        return Err(
            "Phototek may not see this phone's photos: allow it in the Settings app.".into(),
        );
    }
    let assets = photos.list().map_err(|e| e.to_string())?;
    let path = record_path(app, server)?;
    let mut record = read_record(&path);
    progress.total = assets.len();
    let pending: Vec<&Asset> = assets
        .iter()
        .filter(|asset| !record.saved.contains_key(&asset.id))
        .collect();
    progress.saved = assets.len() - pending.len();
    let _ = app.emit("phone-backup", &*progress);

    let scratch = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("backup");
    for batch in pending.chunks(BATCH) {
        if backup.stop.load(Ordering::Relaxed) {
            break;
        }
        // Copy the originals out and fingerprint them.
        let mut ready: Vec<(&Asset, Vec<(PathBuf, String, String)>)> = Vec::new();
        for (n, asset) in batch.iter().enumerate() {
            let folder = scratch.join(n.to_string());
            let _ = std::fs::remove_dir_all(&folder);
            let exported = photos.export(&asset.id, &folder.to_string_lossy());
            let files: Option<Vec<(PathBuf, String, String)>> = exported.ok().and_then(|files| {
                files
                    .into_iter()
                    .map(|file| {
                        let path = PathBuf::from(&file.path);
                        fingerprint(&path)
                            .ok()
                            .map(|print| (path, file.name, print))
                    })
                    .collect()
            });
            match files {
                Some(files) if !files.is_empty() => ready.push((asset, files)),
                _ => progress.failed += 1,
            }
        }
        // Send what the server doesn't have.
        let prints: Vec<String> = ready
            .iter()
            .flat_map(|(_, files)| files.iter().map(|(_, _, print)| print.clone()))
            .collect();
        let known = known_on_server(server, &prints)?;
        for (asset, files) in &ready {
            let mut all_sent = true;
            for (path, name, print) in files {
                if known.contains(print) {
                    continue;
                }
                match send(server, device, path, name, asset.taken) {
                    Ok(()) => progress.sent += 1,
                    // The server refused or can't be reached: stop there.
                    Err(Failure::Server(e)) => return Err(e),
                    Err(Failure::Local(_)) => all_sent = false,
                }
            }
            if all_sent {
                record.saved.insert(
                    asset.id.clone(),
                    files.iter().map(|(_, _, print)| print.clone()).collect(),
                );
                progress.saved += 1;
            } else {
                progress.failed += 1;
            }
        }
        {
            let _lock = backup.record.lock().unwrap();
            write_record(&path, &record)?;
        }
        let _ = std::fs::remove_dir_all(&scratch);
        let _ = app.emit("phone-backup", &*progress);
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

enum Failure {
    /// The file on the phone couldn't be read.
    Local(String),
    /// The server refused it, or can't be reached.
    Server(String),
}

/// Sends one file to the server's library.
fn send(server: &Server, device: &str, path: &Path, name: &str, taken: i64) -> Result<(), Failure> {
    let file = std::fs::File::open(path).map_err(|e| Failure::Local(e.to_string()))?;
    let size = file
        .metadata()
        .map_err(|e| Failure::Local(e.to_string()))?
        .len();
    authorized(agent().post(&api_url(server, "upload")), server)
        .query("device", device)
        .query("name", name)
        .query("taken", &taken.to_string())
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &size.to_string())
        .send(file)
        .map_err(|e| Failure::Server(describe(e)))?;
    Ok(())
}

/// The photos that can leave the phone: recorded as saved, still on the
/// phone, and found on the server right now.
fn safe_to_delete(app: &AppHandle, server: &Server) -> Result<Vec<String>, String> {
    let assets = app.photos().list().map_err(|e| e.to_string())?;
    let record = read_record(&record_path(app, server)?);
    let candidates: Vec<(&String, &Vec<String>)> = assets
        .iter()
        .filter_map(|asset| record.saved.get_key_value(&asset.id))
        .collect();
    let prints: Vec<String> = candidates
        .iter()
        .flat_map(|(_, prints)| prints.iter().cloned())
        .collect();
    let known: std::collections::HashSet<String> =
        known_on_server(server, &prints)?.into_iter().collect();
    Ok(candidates
        .into_iter()
        .filter(|(_, prints)| prints.iter().all(|print| known.contains(print)))
        .map(|(id, _)| id.clone())
        .collect())
}

/// How many photos and videos free up space would delete.
#[tauri::command(async)]
fn phone_space_to_free(app: AppHandle, server: Server) -> Result<usize, String> {
    Ok(safe_to_delete(&app, &server)?.len())
}

/// Deletes from the phone the photos and videos that are safely on the
/// server; the system asks the user to confirm. Returns how many went.
#[tauri::command(async)]
fn free_phone_space(
    app: AppHandle,
    server: Server,
    backup: State<'_, Backup>,
) -> Result<usize, String> {
    if backup.running.load(Ordering::Relaxed) {
        return Err("Wait for the backup to finish.".into());
    }
    let ids = safe_to_delete(&app, &server)?;
    if ids.is_empty() || !app.photos().delete(&ids).map_err(|e| e.to_string())? {
        return Ok(0);
    }
    let path = record_path(&app, &server)?;
    let _lock = backup.record.lock().unwrap();
    let mut record = read_record(&path);
    for id in &ids {
        record.saved.remove(id);
    }
    write_record(&path, &record)?;
    Ok(ids.len())
}
