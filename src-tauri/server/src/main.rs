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

mod access;
mod commands;
mod library;
mod media;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{header, Method};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
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

    let access = Arc::new(access::Access::new(password, library.data_dir()));
    let index = web_dir.join("index.html");
    let app = Router::new()
        .route("/api/events", get(events))
        .route("/api/{command}", post(commands::handle))
        .route("/thumb/{size}/{fingerprint}", get(media::thumb))
        .route("/photo/{fingerprint}", get(media::photo))
        .fallback_service(ServeDir::new(&web_dir).not_found_service(ServeFile::new(index)))
        .with_state(library.clone())
        .merge(
            Router::new()
                .route("/api/login", post(access::login))
                .with_state(access.clone()),
        )
        .layer(middleware::from_fn_with_state(access, access::require))
        // The apps (desktop, mobile) call from their own origin; outside the
        // password check, which preflight requests can't pass.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::RANGE]),
        );

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

/// `GET /api/events`: the events the desktop app gets from Tauri (scan
/// progress, new photos, changes from other devices), as server-sent events
/// named like the Tauri events, with the payload as JSON.
async fn events(
    axum::extract::State(library): axum::extract::State<Arc<Library>>,
) -> axum::response::sse::Sse<
    impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::StreamExt;
    let stream =
        tokio_stream::wrappers::BroadcastStream::new(library.subscribe()).filter_map(|message| {
            // A listener too slow to keep up just misses some progress.
            let (name, payload) = message.ok()?;
            Some(Ok(Event::default().event(name).data(payload.to_string())))
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
