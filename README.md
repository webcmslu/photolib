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

HEIC in the installers: the deb includes it (it depends on `libheif1` and
`libheif-plugin-libde265`). The AppImage bundles libheif but not its HEVC
decoder plugin, so HEIC works there only if the host has
`libheif-plugin-libde265` installed. The macOS and Windows installers
are currently built **without** HEIC, because the app would otherwise depend
on a libheif that is not on the user's machine. A local build on a Mac with
`brew install libheif` does have HEIC.

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
- `org.rs` is the shared organization: the `.photolib` log format and the
  merge. Its header comment describes the format.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.
