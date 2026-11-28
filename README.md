# Phototek

A cross-platform photo browser where the library is any folder: a NAS mount, an
external drive or a local disk. Tauri 2 (Rust) + Svelte 5 (TypeScript).

Formerly called Photolib. The shared folder in each library is still named
`.photolib`, so every version, older ones included, works on the same library.

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
  info panel (camera, lens, focal length, aperture, shutter speed, ISO).
- Camera filter: the grid narrowed to a camera, a lens, an aperture or a
  focal length (chips, to go from f/2.8 to f/8 in one click each). Lens and
  settings are read into the index after each scan (`fill_shooting`).
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
  being listed twice. Likewise a RAW file saved next to a JPEG (or HEIC) of
  the same name is shown as one "JPEG + RAW" photo.
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
  bundled GeoNames city list (`src-tauri/core/data/`).
- Milestone 11 (basic editing): rotate, mirror, crop and brightness /
  contrast / color adjustments in the viewer. Edits are instructions stored
  in the shared organization log and applied when a photo is drawn; the
  original files are never changed.
- Milestone 12 (people): faces are found in the cached previews and grouped
  by person, entirely on this computer. The two models (YuNet and SFace from
  the OpenCV zoo, 39 MB) are downloaded on first use and run with `tract`,
  a pure-Rust runtime. Names are shared through the library's log; giving
  two groups the same name merges them.
- Search by content: "beach", "plage" or "Strand" finds the photos showing
  one. OpenAI's CLIP (MIT, 352 MB, downloaded when switched on in Settings)
  runs with `tract` on the thumbnails; about 220 concepts with words in
  English, French and German (see docs/search.md).
- Milestone 13 (Apple Photos import): "Import from Apple Photos…" reads a
  `.photoslibrary` (read-only) and brings its favorites, hidden photos and
  albums over, for the photos that are also in the library folder. Photos in
  its "Recently Deleted" are hidden, unless a copy is still in Photos.
- After the milestones, closer to Apple Photos: Months show each month's
  busiest days as cards, Years are square cards that zoom into their months,
  and All Photos has a filter menu (favorites, edited, photos, videos, not in
  an album), and the dates on screen under the title.
- Ideas from Windows Photos: a year scrubber beside the grid; folders as a
  collapsible tree where a folder lists its subfolders too; a slideshow (F5);
  sorting by date taken or name; a filmstrip in the viewer; and a "river"
  layout, rows of photos at their own shape filling the width.
- Settings window: slideshow duration and transition, the photo order
  (newest or oldest first, or by name), and face grouping on or off.
  Slideshows can also start from a month's title; a folder can be dropped
  on the welcome screen to open it.
- Scanning: reopening a library only checks for new and changed files
  ("Check for new"); "Rescan everything" reads every file again.
- People on large libraries: groups that are clearly the same person are
  merged; a named person's page suggests look-alike groups ("Is this also
  …?"); "Not …" takes photos out of a person and "Someone else" splits them
  into a new one; strangers can be hidden. Faces are searched in the cached
  previews, so this works while the library is offline.
- Automatic updates: the Info dialog installs a new version in one click
  (signed with the updater key; see `release.yml`).
- Export: the selected photos are copied into a folder, as the original
  files or as JPEGs with their edits, at full size or smaller.
- Duplicates: photos stored in several files are listed with all their
  copies and a suggested copy to keep. The other copies, each first compared
  byte for byte with the one kept, can be moved into a `Duplicates to
  delete` folder at the top of the library (skipped by the scan) for the
  user to delete in their file manager, or put back. This is the only
  feature that moves files, and only on request; nothing is ever deleted
  or changed.
- Ratings (1 to 5 stars, keys 1–5) and colour labels (keys 6–9), shared
  through the library like favorites, with filters for both.
- Similar shots: bursts and near-identical photos taken within a minute are
  grouped by a visual fingerprint of their thumbnails, with a suggested
  shot to keep; the others can be hidden (not moved or deleted).
- Magic wand (✨ in the viewer): levels, white balance, midtones, contrast
  and vibrance worked out from the preview (`enhance.rs`), stored as part of
  the edit. "Apply to original…" in the editing panel writes an edit into a
  JPEG file after a warning, keeping its metadata (`original.rs`).
- Import (`importer.rs`): copies the new photos and videos of a memory card,
  camera or folder into `<folder>/<year>/<year>-<month>/` in the library,
  checking each copy. A connected drive with a `DCIM` folder is offered in a
  banner. Imports are remembered in `.photolib/imports/<device>.log`, so a
  photo is imported only once.
  Phones: on a Mac through ImageCaptureCore (iPhone; Android in "Transfer
  photos"/PTP mode), on Windows through Windows Portable Devices (iPhone
  with Apple's driver; Android in "File transfer"/MTP or PTP mode), on Linux
  through the folders gvfs mounts (Android MTP, iPhone AFC, cameras PTP).
  "Free up space…" then deletes from the phone or card the photos whose copy
  is in the library and on its disk (with their video and RAW file), except
  those of the last days (30 by default); each file is checked by name and
  size just before it is deleted.
- English, French and German: Settings › Language (the system's language
  by default). The English text in the code is the key; `src/lib/locales/`
  holds the translations and `npm run check` lists any that are missing.
  Search understands French and German month, kind and colour words too.
- Right-click menus (the system's own) on photos, albums, folders and
  people, and "Show in Finder / Explorer / Files" for a photo.

## Develop

Prerequisites: Node 20+, Rust 1.90+ and the
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
cd src-tauri && cargo test --workspace
cargo test --release benchmark -- --ignored --nocapture   # 10k-file indexing benchmark
# (`tools/photos-schema.sh` prints an Apple Photos database's structure)
# check RAW support against your own camera files:
PHOTOTEK_RAW_SAMPLES=/some/folder cargo test real_raw -- --ignored --nocapture
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
  The rest of `src-tauri/src/` is desktop-only: imports from phones and
  cameras (`importer.rs`, `camera_*.rs`), Apple Photos (`apple.rs`), the
  recent libraries (`recents.rs`) and the Linux video server (`media_server.rs`).
- `src-tauri/core/` is the library itself, with no user interface, shared
  with the NAS server. In `src-tauri/core/src/`:
- `indexer.rs` walks the folder; `fingerprint.rs` and `metadata.rs` read each
  file; `db.rs` is the SQLite index; `thumbs.rs` makes and caches thumbnails.
- `raw.rs` finds the preview in RAW files; `video.rs` reads MP4/MOV
  metadata; `frames.rs` grabs a video frame for thumbnails.
- `edit.ts`, `EditedImage.svelte` and `CropBox.svelte` draw and make edits.
- `faces.rs` runs the face models; `people.rs` searches photos, groups faces
  and names people (`PeopleView.svelte`). Faces are stored in `faces.db`
  next to the index, so rebuilding the index keeps them.
- `search.rs` turns a search into SQL; `places.rs` names the nearest town.
- `geo.rs` clusters photo positions for the map (`MapView.svelte`, Leaflet).
- `org.rs` is the shared organization: the `.photolib` log format and the
  merge. Its header comment describes the format.
- Per-machine state (recents now; index and thumbnails later) lives in the OS
  app-data dir, never in the library folder.

## Licence

GPL-3.0 (see `LICENSE`). The iOS and Android apps in the App Store and
Google Play are published by WebCMS, the copyright holder, under the stores'
standard terms (see `docs/mobile.md`).
