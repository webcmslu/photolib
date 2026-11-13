# Photolib

A cross-platform photo browser where the library is any folder: a NAS mount, an
external drive or a local disk. Tauri 2 (Rust) + Svelte 5 (TypeScript).

"Photolib" is a working name.

## Status

- Milestone 1 (skeleton): open a library folder, recent libraries list.
- Milestone 2 (indexer): background scan into a local SQLite index with EXIF
  (date, camera, GPS, orientation) and content fingerprints; progress UI;
  incremental rescan on open, on demand and every 15 minutes.
- Milestone 3 (thumbnails): grid (~256 px) and preview (~1024 px) JPEGs made in
  parallel after each scan, cached by fingerprint in the app-data dir and served
  to the UI over `thumb://`. JPEG, PNG, WebP, GIF, TIFF and HEIC.
- Milestone 4 (browsing): virtualized timeline grid grouped by month and day,
  folder view, and a viewer with arrow keys, zoom, pan, full screen and an
  info panel.
- Milestone 5 (shared organization): favorites, hidden photos and albums,
  stored as per-device append-only logs in `<library>/.photolib/` and merged
  on every computer that opens the folder. Works offline from the local cache
  (read-only).
- Milestone 6 (packaging): CI checks and release builds for macOS, Windows
  and Linux with GitHub Actions.
- Milestone 7 (RAW, video, Live Photos): camera RAW files shown through
  their embedded JPEG preview (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2 and
  more); MP4/MOV videos with thumbnails, date, length and GPS, played in the
  viewer; an iPhone Live Photo's `.MOV` is attached to its photo instead of
  being listed twice.
- Milestone 8 (map): a Map section shows the photos and videos that have a
  GPS position on an OpenStreetMap map, clustered by zoom level; clicking a
  marker lists its photos. The map tiles are fetched from
  `tile.openstreetmap.org`, the only thing the app loads from the internet.
- Milestone 9 (browsing polish): Years / Months / All Photos views, a
  larger "featured" card for days with many photos, a thumbnail size slider
  (also Ctrl/Cmd + and -), and a resizable sidebar.
- Milestone 10 (search): a search box that matches place, camera, folder
  and file names, years (`2024`), months (`2024-07`, `july`) and kinds
  (`video`, `live`, `raw`, `favorite`). Places are named offline from a
  bundled GeoNames city list (`src-tauri/data/`).

## Develop

Prerequisites: Node 20+, Rust 1.85+ and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS.

HEIC: macOS decodes it with its own ImageIO framework, so nothing extra is
needed there. Linux and Windows use libheif 1.17+ (`libheif-dev` on Debian);
to build without it, pass `--no-default-features` to cargo.

Video thumbnails are made by `ffmpeg` when it is installed (on the PATH or in
a Homebrew folder); on macOS the system's Quick Look is used when it is not.
Without either, videos show a placeholder tile but still play. Playback uses
the webview's own player: on Linux that needs `gstreamer1.0-libav`.

```sh
npm install
npm run tauri dev      # run the app with hot reload
npm run check          # type-check the UI
cd src-tauri && cargo test
cargo test --release benchmark -- --ignored --nocapture   # 10k-file indexing benchmark
# check RAW support against your own camera files:
PHOTOLIB_RAW_SAMPLES=/some/folder cargo test real_raw -- --ignored --nocapture
npm run tauri build    # release bundle
```

## Packaging

Two GitHub Actions workflows live in `.github/workflows/`:

- `ci.yml` runs on every push to `main` and on pull requests: UI type-check
  and build, `cargo fmt --check`, clippy and the Rust tests.
- `release.yml` builds the installers: dmg for macOS (Apple Silicon and
  Intel), msi and exe for Windows, deb and AppImage for Linux. Pushing a tag
  such as `v0.1.0` attaches them to a draft GitHub release; running the
  workflow by hand keeps them as workflow artifacts instead.

macOS builds are signed and notarized only if the `APPLE_*` repository
secrets listed at the top of `release.yml` are set.

HEIC in the installers: the macOS apps always have it (native decoding).
The deb has it too (it depends on `libheif1` and `libheif-plugin-libde265`).
The AppImage bundles libheif but not its HEVC decoder plugin, so HEIC works
there only if the host has `libheif-plugin-libde265` installed. The Windows
installers are currently built **without** HEIC.

To build installers locally: `npm run tauri build`.

## Layout

- `src/` is the Svelte UI. `src/lib/api.ts` wraps every Rust command.
  `LibraryView.svelte` is an open library, `PhotoGrid.svelte` the virtualized
  grid, `Viewer.svelte` the viewer, and `pages.svelte.ts` loads the photo
  listing page by page.
- `src-tauri/src/lib.rs` holds the Tauri commands and starts background scans.
- `indexer.rs` walks the folder; `fingerprint.rs` and `metadata.rs` read each
  file; `db.rs` is the SQLite index; `thumbs.rs` makes and caches thumbnails;
  `recents.rs` is the recent libraries list.
- `raw.rs` finds the preview in RAW files; `video.rs` reads MP4/MOV
  metadata; `frames.rs` grabs a video frame for thumbnails; `media_server.rs`
  serves videos to the Linux webview, which can't play them from `photo://`.
- `search.rs` turns a search into SQL; `places.rs` names the nearest town.
- `geo.rs` clusters photo positions for the map (`MapView.svelte`, Leaflet).
- `org.rs` is the shared organization: the `.photolib` log format and the
  merge. Its header comment describes the format.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.
