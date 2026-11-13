# Photolib

A cross-platform photo browser where the library is any folder: a NAS mount, an
external drive or a local disk. Tauri 2 (Rust) + Svelte 5 (TypeScript).

"Photolib" is a working name.

## Status

Milestone 1 (skeleton): open a library folder, recent libraries list.

## Develop

Prerequisites: Node 20+, Rust 1.85+, and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm install
npm run tauri dev      # run the app with hot reload
npm run check          # type-check the UI
cd src-tauri && cargo test
npm run tauri build    # release bundle
```

## Layout

- `src/` is the Svelte UI. `src/lib/api.ts` wraps every Rust command.
- `src-tauri/src/lib.rs` holds the Tauri commands; `recents.rs` the recent libraries list.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.
