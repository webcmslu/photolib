//! `POST /api/<command>`: the desktop app's Tauri commands, with the same
//! names, the same arguments (a JSON object, in camelCase, as the interface
//! sends them) and the same results, so the interface runs unchanged in a
//! browser. A failed command answers 400 with its message, as a JSON string.
//!
//! The library is the server's own: the `path` the interface sends along is
//! ignored. Changes (favourites, albums, hidden photos, people's names) are
//! recorded in the library's shared logs like any device's; the photo files
//! themselves are never changed, and imports, exports and edits that write
//! files stay in the desktop app.

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use phototek_core::{
    clip, collections, db, duplicates, faces, geo, metadata, org, people, similar,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::library::Library;

pub async fn handle(
    State(library): State<Arc<Library>>,
    UrlPath(command): UrlPath<String>,
    body: Option<Json<Value>>,
) -> Response {
    let args = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let answer = tokio::task::spawn_blocking(move || run(&library, &command, args)).await;
    match answer {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(message)) => (StatusCode::BAD_REQUEST, Json(Value::String(message))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("Bad arguments: {e}"))
}

fn answer<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn failed(e: impl ToString) -> String {
    e.to_string()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListingArgs {
    listing: db::Listing,
    #[serde(default)]
    order: Option<db::Order>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageArgs {
    listing: db::Listing,
    #[serde(default)]
    order: Option<db::Order>,
    offset: u32,
    limit: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapArgs {
    south: f64,
    west: f64,
    north: f64,
    east: f64,
    zoom: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhotoArgs {
    rel_path: String,
}

#[derive(Deserialize)]
struct TodayArgs {
    today: i64,
}

#[derive(Deserialize)]
struct ClustersArgs {
    clusters: Vec<i64>,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct CheckArgs {
    clusters: Vec<i64>,
    #[serde(default = "check_page")]
    limit: usize,
}

fn check_page() -> usize {
    40
}

#[derive(Deserialize)]
struct PhotosArgs {
    clusters: Vec<i64>,
    photos: Vec<String>,
}

#[derive(Deserialize)]
struct OpsArgs {
    ops: Vec<org::Op>,
}

#[derive(Deserialize)]
struct NameArgs {
    name: String,
}

#[derive(Deserialize)]
struct HiddenArgs {
    clusters: Vec<i64>,
    hidden: bool,
}

#[derive(Deserialize)]
struct KeyPhotoArgs {
    clusters: Vec<i64>,
    name: String,
    photo: String,
}

#[derive(Deserialize)]
struct RejectArgs {
    cluster: i64,
    name: String,
}

/// Mirrors `LibraryInfo` in the desktop app.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    path: String,
    name: String,
    writable: bool,
    online: bool,
}

fn library_info(library: &Library) -> LibraryInfo {
    LibraryInfo {
        path: library.root().to_string_lossy().into_owned(),
        name: library.name(),
        writable: library.writable(),
        online: true,
    }
}

fn run(library: &Library, command: &str, args: Value) -> Result<Value, String> {
    let index = || library.index();
    match command {
        // The library: always the server's own.
        "open_library" => answer(library_info(library)),
        "list_recent_libraries" => {
            let info = library_info(library);
            answer(
                json!([{ "path": info.path, "name": info.name, "lastOpened": 0, "available": true }]),
            )
        }
        "remove_recent_library" | "close_library" => Ok(Value::Null),
        "rescan_library" => {
            library.request_scan();
            Ok(Value::Null)
        }
        "library_stats" => answer(json!({
            "photoCount": db::count(&index()?).map_err(failed)?,
            "scanning": library.scanning(),
        })),
        // Videos play straight from /photo/<fingerprint>.
        "media_base" => Ok(Value::Null),
        // No installer to offer: the server updates with its container.
        "release_asset_suffix" => Ok(Value::Null),

        // Listings.
        "timeline" => {
            let a: ListingArgs = parse(args)?;
            answer(
                db::timeline_in(&index()?, &a.listing, a.order.unwrap_or_default())
                    .map_err(failed)?,
            )
        }
        "shapes" => {
            let a: ListingArgs = parse(args)?;
            answer(db::shapes(&index()?, &a.listing, a.order.unwrap_or_default()).map_err(failed)?)
        }
        "photos_page" => {
            let a: PageArgs = parse(args)?;
            answer(
                db::photos_page_in(
                    &index()?,
                    &a.listing,
                    a.order.unwrap_or_default(),
                    a.offset,
                    a.limit.min(1000),
                )
                .map_err(failed)?,
            )
        }
        "months" => {
            let a: ListingArgs = parse(args)?;
            answer(db::months(&index()?, &a.listing).map_err(failed)?)
        }
        "gear_options" => {
            let a: ListingArgs = parse(args)?;
            answer(db::gear_options(&index()?, &a.listing).map_err(failed)?)
        }
        "listing_place" => {
            let a: ListingArgs = parse(args)?;
            answer(db::listing_place(&index()?, &a.listing).map_err(failed)?)
        }
        "folders" => answer(db::folders(&index()?).map_err(failed)?),
        "albums" => answer(db::albums(&index()?).map_err(failed)?),
        "map_clusters" => {
            let a: MapArgs = parse(args)?;
            let points =
                db::geo_points(&index()?, a.south, a.west, a.north, a.east).map_err(failed)?;
            answer(geo::cluster(&points, a.zoom))
        }
        "photo_details" => {
            let a: PhotoArgs = parse(args)?;
            answer(db::photo_details(&index()?, &a.rel_path).map_err(failed)?)
        }
        "photo_shooting" => {
            let a: PhotoArgs = parse(args)?;
            if db::find_by_path(&index()?, &a.rel_path)
                .map_err(failed)?
                .is_none()
            {
                return Ok(Value::Null);
            }
            answer(metadata::shooting(&library.root().join(&a.rel_path)))
        }
        "collections" => {
            let a: TodayArgs = parse(args)?;
            answer(collections::collections(&index()?, a.today).map_err(failed)?)
        }
        "similar_groups" => answer(similar::groups(&index()?).map_err(failed)?),
        "duplicate_groups" => answer(duplicates::groups(&index()?).map_err(failed)?),
        "parked_duplicates" => Ok(json!(0)),

        // People.
        "faces_status" => {
            let downloaded = faces::models_present(library.data_dir());
            answer(json!({
                "enabled": library.faces_enabled(),
                "downloaded": downloaded,
                "downloadBytes": faces::DOWNLOAD_BYTES,
            }))
        }
        "content_status" => answer(json!({
            "enabled": library.content_enabled(),
            "downloaded": clip::model_present(library.data_dir()),
            "downloadBytes": clip::DOWNLOAD_BYTES,
        })),
        "people" => answer(people::people(&index()?).map_err(failed)?),
        "person_faces" => {
            let a: ClustersArgs = parse(args)?;
            answer(db::faces_in(&index()?, &a.clusters).map_err(failed)?)
        }
        "person_suggestions" => {
            let a: ClustersArgs = parse(args)?;
            answer(people::suggestions(&index()?, &a.clusters, &a.name).map_err(failed)?)
        }

        // Which of a phone's files the library has (see upload.rs).
        "known_photos" => {
            #[derive(Deserialize)]
            struct Args {
                fingerprints: Vec<String>,
            }
            let a: Args = parse(args)?;
            let conn = index()?;
            let mut known = Vec::new();
            for fingerprint in a.fingerprints.iter().take(5000) {
                if crate::upload::known(&conn, fingerprint).map_err(failed)? {
                    known.push(fingerprint.clone());
                }
            }
            answer(known)
        }
        "check_person" => {
            let a: CheckArgs = parse(args)?;
            answer(people::check_person(&index()?, &a.clusters, a.limit.min(500)).map_err(failed)?)
        }
        "confirm_person_faces" => {
            let a: PhotosArgs = parse(args)?;
            db::confirm_faces(&mut index()?, &a.clusters, &a.photos).map_err(failed)?;
            Ok(Value::Null)
        }

        // Changes, recorded in the library's logs.
        "apply_ops" => {
            let a: OpsArgs = parse(args)?;
            library.record(&mut index()?, a.ops)?;
            Ok(Value::Null)
        }
        "create_album" => {
            let a: NameArgs = parse(args)?;
            let album = org::new_id().map_err(failed)?;
            library.record(
                &mut index()?,
                vec![org::Op::AlbumName {
                    album: album.clone(),
                    name: a.name,
                }],
            )?;
            answer(album)
        }
        "sync_library" => answer(library.sync(&mut index()?)?),
        "name_person" => {
            let a: ClustersArgs = parse(args)?;
            let mut conn = index()?;
            let ops = people::naming_ops(&conn, &a.clusters, a.name.trim()).map_err(failed)?;
            library.record(&mut conn, ops)?;
            Ok(Value::Null)
        }
        "make_key_photo" => {
            let a: KeyPhotoArgs = parse(args)?;
            let mut conn = index()?;
            let op = people::key_photo_op(&conn, &a.clusters, a.name.trim(), &a.photo)
                .map_err(failed)?
                .ok_or("This person's face isn't found in this photo.")?;
            library.record(&mut conn, vec![op])?;
            Ok(Value::Null)
        }
        "remove_from_person" => {
            #[derive(Deserialize)]
            struct Args {
                clusters: Vec<i64>,
                photos: Vec<String>,
                #[serde(default)]
                name: Option<String>,
            }
            let a: Args = parse(args)?;
            let mut conn = index()?;
            let removed = db::remove_faces(&mut conn, &a.clusters, &a.photos).map_err(failed)?;
            // Another of their faces carries the name from now on.
            if let Some(name) = a.name.filter(|name| !name.trim().is_empty()) {
                let ops = people::naming_ops(&conn, &a.clusters, name.trim()).map_err(failed)?;
                library.record(&mut conn, ops)?;
            }
            answer(removed)
        }
        "set_person_hidden" => {
            let a: HiddenArgs = parse(args)?;
            db::set_face_groups_hidden(&mut index()?, &a.clusters, a.hidden).map_err(failed)?;
            Ok(Value::Null)
        }
        "reject_suggestion" => {
            let a: RejectArgs = parse(args)?;
            db::add_not_person(&index()?, a.cluster, &a.name).map_err(failed)?;
            Ok(Value::Null)
        }

        // Imports, exports and edits that write files: in the desktop app.
        _ => Err(format!(
            "\"{command}\" can't be done on the server: use the Phototek app on a computer."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_answer_like_the_app() {
        let photos = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let library =
            Library::open(photos.path().into(), data.path().into(), false, false).unwrap();

        let stats = run(&library, "library_stats", json!({ "path": "" })).unwrap();
        assert_eq!(stats["photoCount"], 0);
        let opened = run(&library, "open_library", json!({ "path": "" })).unwrap();
        assert_eq!(opened["online"], true);
        // Arguments come in camelCase, as the interface sends them.
        let days = run(
            &library,
            "timeline",
            json!({ "path": "", "listing": { "kind": "library" }, "order": "dateDesc" }),
        )
        .unwrap();
        assert_eq!(days, json!([]));
        let page = run(
            &library,
            "photos_page",
            json!({ "path": "", "listing": { "kind": "favorites" }, "offset": 0, "limit": 10 }),
        )
        .unwrap();
        assert_eq!(page, json!([]));

        // Bad arguments and changes are refused, with a message.
        assert!(run(&library, "timeline", json!({})).is_err());
        let refused = run(&library, "export_photos", json!({})).unwrap_err();
        assert!(refused.contains("export_photos"));

        // Changes go into the library's logs and show at once.
        let album = run(
            &library,
            "create_album",
            json!({ "path": "", "name": "Noël" }),
        )
        .unwrap();
        let albums = run(&library, "albums", json!({ "path": "" })).unwrap();
        assert_eq!(albums[0]["id"], album);
        assert_eq!(albums[0]["name"], "Noël");
        assert!(photos.path().join(".photolib/devices").is_dir());
    }
}
