//! `POST /api/<command>`: the desktop app's Tauri commands, with the same
//! names, the same arguments (a JSON object, in camelCase, as the interface
//! sends them) and the same results, so the interface runs unchanged in a
//! browser. A failed command answers 400 with its message, as a JSON string.
//!
//! The library is the server's own: the `path` the interface sends along is
//! ignored. Commands that change the library are not served yet.

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use phototek_core::{collections, db, duplicates, faces, geo, metadata, people, similar};
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
        // Changes are made in the desktop app for now.
        writable: false,
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
        "people" => answer(people::people(&index()?).map_err(failed)?),
        "person_faces" => {
            let a: ClustersArgs = parse(args)?;
            answer(db::faces_in(&index()?, &a.clusters).map_err(failed)?)
        }
        "person_suggestions" => {
            let a: ClustersArgs = parse(args)?;
            answer(people::suggestions(&index()?, &a.clusters, &a.name).map_err(failed)?)
        }

        // Whatever changes the library, for now only in the desktop app.
        _ => Err(format!(
            "\"{command}\" can't be done on the server yet: use the Phototek app on a computer."
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
        let library = Library::open(photos.path().into(), data.path().into(), false).unwrap();

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
        let refused = run(&library, "apply_ops", json!({})).unwrap_err();
        assert!(refused.contains("apply_ops"));
    }
}
