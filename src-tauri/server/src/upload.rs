//! `POST /api/upload`: a phone sends a photo or video of its own, which goes
//! into the library, in `Phones/<phone>/<year>/<month>/`, unless the library
//! already has the very same file (anywhere in it). The body is the file
//! itself, streamed; the query says which phone, the file's name and when it
//! was taken:
//!
//!     POST /api/upload?device=Anna%27s%20iPhone&name=IMG_1234.HEIC&taken=1732780800000
//!
//! The answer: `{"fingerprint": "…", "stored": true}`, `stored` false when
//! the library already had it. `known_photos` (commands.rs) tells a phone
//! which of its files the library has, before it sends them.
//!
//! Only the uploads folder is written to, and never over another file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Datelike};
use phototek_core::{fingerprint, indexer};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::library::Library;

/// Where uploads go, under the library folder.
pub const UPLOADS: &str = "Phones";

#[derive(Deserialize)]
pub struct UploadArgs {
    /// The phone's name, as its owner calls it.
    #[serde(default)]
    device: String,
    /// The file's name on the phone.
    name: String,
    /// When it was taken (ms since 1970), for the folder and the file's date.
    #[serde(default)]
    taken: Option<i64>,
}

type Failure = (StatusCode, String);

fn failed(e: impl std::fmt::Display) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub async fn upload(
    State(library): State<Arc<Library>>,
    Query(args): Query<UploadArgs>,
    body: Body,
) -> Response {
    match receive(library.clone(), args, body).await {
        Ok((fingerprint, stored)) => {
            if stored {
                // Thumbnails, similar shots, faces… for the new file.
                library.request_scan();
            }
            Json(json!({ "fingerprint": fingerprint, "stored": stored })).into_response()
        }
        Err((status, message)) => (status, Json(message)).into_response(),
    }
}

async fn receive(
    library: Arc<Library>,
    args: UploadArgs,
    body: Body,
) -> Result<(String, bool), Failure> {
    let device = clean_name(&args.device).unwrap_or_else(|| "Phone".to_owned());
    let name = clean_name(&args.name)
        .ok_or((StatusCode::BAD_REQUEST, "A file name is needed.".to_owned()))?;
    // Received next to the library's own data, which scans skip, then moved
    // into place once complete, unless the library has it already.
    let incoming = library.root().join(".photolib").join("incoming");
    tokio::fs::create_dir_all(&incoming).await.map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            "The photo folder is read-only on the server: uploads can't be stored.".to_owned(),
        )
    })?;
    let part = incoming.join(format!("{}.part", random_name()));
    if let Err(e) = write_body(&part, body).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err((
            StatusCode::BAD_REQUEST,
            format!("The upload was cut off: {e}"),
        ));
    }
    tokio::task::spawn_blocking(move || {
        let result = store(&library, &part, &device, &name, args.taken);
        // Gone when it was moved into place; otherwise not needed.
        let _ = std::fs::remove_file(&part);
        result
    })
    .await
    .map_err(failed)?
}

/// Puts the received file into the library, unless it is there already.
fn store(
    library: &Library,
    part: &Path,
    device: &str,
    name: &str,
    taken: Option<i64>,
) -> Result<(String, bool), Failure> {
    let fingerprint = fingerprint::fingerprint(part).map_err(failed)?;
    let conn = library.index().map_err(failed)?;
    if known(&conn, &fingerprint).map_err(failed)? {
        return Ok((fingerprint, false));
    }
    let when = taken
        .and_then(DateTime::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now);
    let folder = library
        .root()
        .join(UPLOADS)
        .join(device)
        .join(format!("{:04}", when.year()))
        .join(format!("{:02}", when.month()));
    std::fs::create_dir_all(&folder).map_err(failed)?;
    let target = move_in(part, &folder, name).map_err(failed)?;
    if let Some(taken) = taken.filter(|ms| *ms > 0) {
        // A file without a date inside gets the right one from this.
        let time = UNIX_EPOCH + Duration::from_millis(taken as u64);
        let _ = std::fs::File::options()
            .write(true)
            .open(&target)
            .and_then(|file| file.set_modified(time));
    }
    let rel_path = target
        .strip_prefix(library.root())
        .map_err(failed)?
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    indexer::reindex_file(library.root(), &conn, &rel_path).map_err(failed)?;
    Ok((fingerprint, true))
}

/// True when the library has a file with this fingerprint.
pub fn known(conn: &rusqlite::Connection, fingerprint: &str) -> rusqlite::Result<bool> {
    conn.prepare_cached("SELECT 1 FROM photos WHERE fingerprint = ?1 LIMIT 1")?
        .exists([fingerprint])
}

/// Moves `part` into `folder` as `name`, or `name (2)`… if a different file
/// already has that name: never over another file.
fn move_in(part: &Path, folder: &Path, name: &str) -> std::io::Result<PathBuf> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    for n in 1..1000 {
        let candidate = if n == 1 {
            folder.join(name)
        } else {
            folder.join(format!("{stem} ({n}){extension}"))
        };
        // A link fails if the name is taken, where a rename would replace.
        match std::fs::hard_link(part, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // A file system without links: rename, having checked the name.
            Err(_) if !candidate.exists() => {
                std::fs::rename(part, &candidate)?;
                return Ok(candidate);
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with that name"))
}

async fn write_body(path: &Path, body: Body) -> std::io::Result<()> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(std::io::Error::other)?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await
}

/// A name usable as one path component: no folders, nothing hidden, no
/// characters Windows or a NAS share would refuse.
fn clean_name(text: &str) -> Option<String> {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_owned();
    (!cleaned.is_empty() && cleaned.len() <= 200).then_some(cleaned)
}

fn random_name() -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_one_harmless_component() {
        assert_eq!(
            clean_name("IMG_1234.HEIC").as_deref(),
            Some("IMG_1234.HEIC")
        );
        assert_eq!(
            clean_name("../../etc/passwd").as_deref(),
            Some("_.._etc_passwd")
        );
        assert_eq!(
            clean_name("Anna's iPhone").as_deref(),
            Some("Anna's iPhone")
        );
        assert_eq!(clean_name(".hidden").as_deref(), Some("hidden"));
        assert_eq!(clean_name("  "), None);
        assert_eq!(clean_name(".."), None);
    }

    #[test]
    fn photos_land_in_the_phones_folder_once() {
        let photos = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let library =
            Library::open(photos.path().into(), data.path().into(), false, false).unwrap();
        let send = |name: &str, bytes: &[u8]| {
            let part = photos.path().join(format!("{name}.part"));
            std::fs::write(&part, bytes).unwrap();
            let result = store(
                &library,
                &part,
                "Anna's iPhone",
                name,
                Some(1_732_780_800_000),
            );
            let _ = std::fs::remove_file(&part);
            result.unwrap()
        };
        let (fingerprint, stored) = send("IMG_1.JPG", b"first photo");
        assert!(stored);
        let folder = photos.path().join("Phones/Anna's iPhone/2024/11");
        assert_eq!(
            std::fs::read(folder.join("IMG_1.JPG")).unwrap(),
            b"first photo"
        );
        assert!(known(&library.index().unwrap(), &fingerprint).unwrap());
        // The same file again: not stored twice.
        assert_eq!(send("IMG_1.JPG", b"first photo"), (fingerprint, false));
        // Another file with the same name: kept beside it.
        assert!(send("IMG_1.JPG", b"another photo").1);
        assert_eq!(
            std::fs::read(folder.join("IMG_1 (2).JPG")).unwrap(),
            b"another photo"
        );
    }
}
