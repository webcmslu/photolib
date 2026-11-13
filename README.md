# Photolib

A cross-platform photo browser where the library is any folder: a NAS mount, an
external drive or a local disk. Tauri 2 (Rust) + Svelte 5 (TypeScript).

"Photolib" is a working name.

## Status

- Milestone 1 (skeleton): open a library folder, recent libraries list.
- Milestone 2 (indexer): background scan into a local SQLite index with EXIF
  (date, camera, GPS, orientation) and content fingerprints; progress UI;
  incremental rescan on open, on demand and every 15 minutes.

## Develop

Prerequisites: Node 20+, Rust 1.85+, and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS.

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
- `src-tauri/src/lib.rs` holds the Tauri commands and starts background scans.
- `indexer.rs` walks the folder; `fingerprint.rs` and `metadata.rs` read each
  file; `db.rs` is the SQLite index; `recents.rs` the recent libraries list.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.
