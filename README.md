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

## Develop

Prerequisites: Node 20+, Rust 1.85+, the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS,
and libheif 1.17+ for HEIC (`libheif-dev` on Debian, `brew install libheif` on macOS). To build without HEIC,
pass `--no-default-features` to cargo.

```sh
npm install
npm run tauri dev      # run the app with hot reload
npm run check          # type-check the UI
cd src-tauri && cargo test
cargo test --release -- --ignored --nocapture   # 10k-file indexing benchmark
npm run tauri build    # release bundle
```

## Layout

- `src/` is the Svelte UI. `src/lib/api.ts` wraps every Rust command.
  `LibraryView.svelte` is an open library, `PhotoGrid.svelte` the virtualized
  grid, `Viewer.svelte` the viewer, and `pages.svelte.ts` loads the photo
  listing page by page.
- `src-tauri/src/lib.rs` holds the Tauri commands and starts background scans.
- `indexer.rs` walks the folder; `fingerprint.rs` and `metadata.rs` read each
  file; `db.rs` is the SQLite index; `thumbs.rs` makes and caches thumbnails;
  `recents.rs` is the recent libraries list.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.
