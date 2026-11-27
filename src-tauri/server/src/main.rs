//! Phototek on a NAS. Indexes one photo folder where it is stored (fast:
//! the disk is local), makes the thumbnails, finds the people and works out
//! the memories, and serves all of it over HTTP:
//!
//! - `POST /api/<command>` answers the same commands as the desktop app's
//!   Tauri commands, with the same arguments and results in JSON, so the
//!   same interface runs in a browser, on a phone or on a TV;
//! - `GET /thumb/<grid|preview>/<fingerprint>` and `GET /photo/<fingerprint>`
//!   serve the thumbnails and the photos and videos themselves;
//! - everything else is the interface (the app's `dist` folder).
//!
//! Set up with environment variables:
//!
//! | Variable              | Meaning                                    | Default    |
//! |-----------------------|--------------------------------------------|------------|
//! | `PHOTOTEK_LIBRARY`    | The photo folder                           | `/photos`  |
//! | `PHOTOTEK_DATA`       | Index, thumbnails and face models          | `/data`    |
//! | `PHOTOTEK_WEB`        | The interface's files                      | `/web`     |
//! | `PHOTOTEK_PORT`       | The port to listen on                      | `8080`     |
//! | `PHOTOTEK_PASSWORD`   | Asked by the browser when set              | none       |
//! | `PHOTOTEK_SCAN_EVERY` | Minutes between checks for new photos: `0` only at start, `off` never | `30` |
//! | `PHOTOTEK_FACES`      | `1` to find and group faces                | off        |
//!
//! Changes made in a browser (favourites, albums, hidden photos, names) go
//! into the library's shared logs (`.photolib/devices/`), as any device's,
//! so the desktop apps see them; the photo files are never changed.

mod commands;
mod library;
mod media;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use tower_http::services::{ServeDir, ServeFile};

use library::Library;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

#[tokio::main]
async fn main() {
    let library_dir = PathBuf::from(env_or("PHOTOTEK_LIBRARY", "/photos"));
    let data_dir = PathBuf::from(env_or("PHOTOTEK_DATA", "/data"));
    let web_dir = PathBuf::from(env_or("PHOTOTEK_WEB", "/web"));
    let port: u16 = env_or("PHOTOTEK_PORT", "8080").parse().unwrap_or(8080);
    let scan_every = match env_or("PHOTOTEK_SCAN_EVERY", "30").as_str() {
        "off" => None,
        minutes => Some(minutes.parse().unwrap_or(30)),
    };
    let faces = env_or("PHOTOTEK_FACES", "0") == "1";
    let password = std::env::var("PHOTOTEK_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());

    let library = match Library::open(library_dir, data_dir, faces) {
        Ok(library) => Arc::new(library),
        Err(e) => {
            eprintln!("phototek-server: {e}");
            std::process::exit(1);
        }
    };
    library::start_scans(library.clone(), scan_every);

    let index = web_dir.join("index.html");
    let app = Router::new()
        .route("/api/{command}", post(commands::handle))
        .route("/thumb/{size}/{fingerprint}", get(media::thumb))
        .route("/photo/{fingerprint}", get(media::photo))
        .fallback_service(ServeDir::new(&web_dir).not_found_service(ServeFile::new(index)))
        .layer(middleware::from_fn_with_state(
            Arc::new(password),
            require_password,
        ))
        .with_state(library.clone());

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("phototek-server: can't listen on port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "Phototek: {} ({} photos), on http://{address}",
        library.name(),
        library.photo_count()
    );
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("phototek-server: {e}");
    }
}

/// With `PHOTOTEK_PASSWORD` set, the browser asks for it (any user name).
async fn require_password(
    State(password): State<Arc<Option<String>>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(password) = password.as_deref() else {
        return next.run(request).await;
    };
    let given = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .and_then(|decoded| String::from_utf8(decoded).ok())
        .and_then(|pair| pair.split_once(':').map(|(_, given)| given.to_owned()));
    // Compared through hashes, so the time taken says nothing about the password.
    let matches = given
        .is_some_and(|given| blake3::hash(given.as_bytes()) == blake3::hash(password.as_bytes()));
    if matches {
        return next.run(request).await;
    }
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"Phototek\", charset=\"UTF-8\""),
    );
    response
}
