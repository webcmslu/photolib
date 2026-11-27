//! Thumbnails, photos and videos, as the desktop app's `thumb://` and
//! `photo://` protocols serve them.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use phototek_core::thumbs::ThumbSize;
use phototek_core::{db, media_server, metadata, thumbs};

use crate::library::Library;

/// The most video data sent in one response: players ask for the rest as
/// they need it.
const VIDEO_CHUNK: u64 = 4 * 1024 * 1024;

pub async fn thumb(
    State(library): State<Arc<Library>>,
    UrlPath((size, fingerprint)): UrlPath<(String, String)>,
) -> Response {
    let Some(size) = ThumbSize::parse(&size) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !thumbs::is_fingerprint(&fingerprint) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let found = tokio::task::spawn_blocking(move || {
        let file = library.thumbs().path(size, &fingerprint);
        fs::read(&file).or_else(|_| {
            make_thumb_now(&library, &fingerprint)?;
            fs::read(&file).map_err(drop)
        })
    })
    .await;
    match found {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                // The URL names the content, so it can be cached forever.
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The original of a photo of the library, and its EXIF orientation. Only
/// files in the index can be reached this way.
fn original(library: &Library, fingerprint: &str) -> Result<(PathBuf, Option<u8>), ()> {
    let conn = library.index().map_err(drop)?;
    let (rel_path, orientation) = db::find_by_fingerprint(&conn, fingerprint)
        .map_err(drop)?
        .ok_or(())?;
    Ok((library.root().join(rel_path), orientation))
}

/// Makes the thumbnails of one photo right away, instead of waiting for the
/// background pass to reach it.
fn make_thumb_now(library: &Library, fingerprint: &str) -> Result<(), ()> {
    let (source, orientation) = original(library, fingerprint)?;
    let _permit = library.decoding();
    if library.thumbs().has_both(fingerprint) {
        return Ok(());
    }
    let size = if metadata::kind_of(&source) == Some(metadata::Kind::Video) {
        library
            .thumbs()
            .generate_from_video(&source, fingerprint)
            .map_err(drop)?;
        None
    } else {
        Some(
            library
                .thumbs()
                .generate(&source, fingerprint, orientation)
                .map_err(drop)?,
        )
    };
    if let Ok(conn) = library.index() {
        let _ = db::mark_thumb_ready(&conn, fingerprint, size);
    }
    Ok(())
}

pub async fn photo(
    State(library): State<Arc<Library>>,
    UrlPath(fingerprint): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if !thumbs::is_fingerprint(&fingerprint) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let Ok((source, orientation)) = original(&library, &fingerprint) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if metadata::kind_of(&source) == Some(metadata::Kind::Video) {
            return video(&source, range.as_deref())
                .unwrap_or_else(|_| StatusCode::NOT_FOUND.into_response());
        }
        // Formats browsers can't show (HEIC, RAW…) are converted to JPEG.
        let result = match thumbs::is_web_image(&source) {
            Some(mime) => fs::read(&source).map(|bytes| (mime, bytes)).map_err(drop),
            None => {
                let _permit = library.decoding();
                thumbs::render_jpeg(&source, orientation)
                    .map(|bytes| ("image/jpeg", bytes))
                    .map_err(drop)
            }
        };
        match result {
            Ok((mime, bytes)) => (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "private, max-age=3600"),
                ],
                bytes,
            )
                .into_response(),
            Err(()) => StatusCode::NOT_FOUND.into_response(),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// A video, in pieces when asked (that is how players seek).
fn video(source: &Path, range: Option<&str>) -> std::io::Result<Response> {
    let mime = media_server::mime_of(source);
    let mut file = fs::File::open(source)?;
    let len = file.metadata()?.len();
    let Some(range) = range else {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return Ok((
            [
                (header::CONTENT_TYPE, mime.to_owned()),
                (header::ACCEPT_RANGES, "bytes".to_owned()),
            ],
            bytes,
        )
            .into_response());
    };
    let Some((first, last)) = media_server::parse_range(range, len) else {
        return Ok((
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{len}"))],
        )
            .into_response());
    };
    let last = last.min(first + VIDEO_CHUNK - 1);
    let mut bytes = vec![0u8; (last - first + 1) as usize];
    file.seek(SeekFrom::Start(first))?;
    file.read_exact(&mut bytes)?;
    Ok((
        StatusCode::PARTIAL_CONTENT,
        [
            (header::CONTENT_TYPE, mime.to_owned()),
            (header::ACCEPT_RANGES, "bytes".to_owned()),
            (header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}")),
        ],
        Body::from(bytes),
    )
        .into_response())
}
