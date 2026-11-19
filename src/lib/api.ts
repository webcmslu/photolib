// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Edit } from "./edit";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

/** An opened library folder. Mirrors `LibraryInfo` in Rust. */
export interface LibraryInfo {
  path: string;
  name: string;
  /** False when the folder is read-only: organization stays local-only. */
  writable: boolean;
  /** False when the folder can't be reached and only cached data is shown. */
  online: boolean;
}

/** One entry of the recent libraries list. Mirrors `RecentLibrary` in Rust. */
export interface RecentLibrary {
  path: string;
  name: string;
  /** Unix time in milliseconds. */
  lastOpened: number;
  /** False when the folder can't be reached right now (NAS offline, drive unplugged). */
  available: boolean;
}

/** Shows the native folder picker. Resolves to null if the user cancels. */
export async function pickLibraryFolder(): Promise<string | null> {
  const picked = await open({
    directory: true,
    multiple: false,
    title: "Open library folder",
  });
  return typeof picked === "string" ? picked : null;
}

/** Lets the user choose the folder to export photos into. */
export async function pickExportFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, title: "Export to" });
  return typeof picked === "string" ? picked : null;
}

/** The original files, or JPEGs with the edits applied, optionally smaller. */
export type ExportSize = { kind: "original" } | { kind: "jpeg"; longSide: number | null };

export interface ExportSummary {
  exported: number;
  /** Library-relative paths that failed, with why. */
  failed: [string, string][];
  cancelled: boolean;
}

export function exportPhotos(
  path: string,
  photos: string[],
  dest: string,
  size: ExportSize,
): Promise<ExportSummary> {
  return invoke("export_photos", { path, photos, dest, size });
}

export function cancelExport(): Promise<void> {
  return invoke("cancel_export");
}

/** `[done, total]` while an export runs. */
export function onExportProgress(handler: (done: number, total: number) => void): Promise<UnlistenFn> {
  return listen<[number, number]>("export-progress", (event) => handler(...event.payload));
}

/**
 * Lets the user choose an Apple Photos library. On macOS it is a "package":
 * a folder that the file picker shows as one file.
 */
export async function pickPhotosLibrary(): Promise<string | null> {
  const onMac = navigator.userAgent.includes("Mac");
  const picked = await open({
    directory: !onMac,
    multiple: false,
    title: "Choose an Apple Photos library",
    filters: onMac ? [{ name: "Photos Library", extensions: ["photoslibrary"] }] : undefined,
  });
  return typeof picked === "string" ? picked : null;
}

/** Mirrors `ImportSummary` in src-tauri/src/apple.rs. */
export interface ImportSummary {
  /** Photos and videos in the Apple Photos library (not in its trash). */
  assets: number;
  /** How many of them were found in this library. */
  matched: number;
  favorites: number;
  hidden: number;
  /** Photos in "Recently Deleted" in Photos, found here: they get hidden. */
  deleted: number;
  albums: { name: string; photos: number; matched: number }[];
}

/** Says what importing an Apple Photos library would do. */
export function appleImportPreview(path: string, photosLibrary: string): Promise<ImportSummary> {
  return invoke("apple_import_preview", { path, photosLibrary });
}

/** Imports favorites, hidden photos and albums from an Apple Photos library. */
export function appleImport(path: string, photosLibrary: string): Promise<ImportSummary> {
  return invoke("apple_import", { path, photosLibrary });
}

export function openLibrary(path: string): Promise<LibraryInfo> {
  return invoke("open_library", { path });
}

export function listRecentLibraries(): Promise<RecentLibrary[]> {
  return invoke("list_recent_libraries");
}

export function removeRecentLibrary(path: string): Promise<RecentLibrary[]> {
  return invoke("remove_recent_library", { path });
}

// --- Indexing ---------------------------------------------------------------

/** Mirrors `Progress` in src-tauri/src/indexer.rs. */
export interface ScanProgress {
  phase: "scanning" | "indexing" | "thumbnails" | "faces";
  /** Image files found so far by the folder walk. */
  discovered: number;
  /** Items the current phase has to process, and how many are done. */
  total: number;
  done: number;
  /** Files the index already held when the scan started. */
  known: number;
}

/** Mirrors `ScanSummary` in src-tauri/src/indexer.rs. */
export interface ScanSummary {
  total: number;
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  failed: number;
  unreadableDirs: number;
  cancelled: boolean;
  durationMs: number;
}

/** Mirrors `ThumbSummary` in src-tauri/src/thumbs.rs. */
export interface ThumbSummary {
  generated: number;
  /** Files that are not decodable images. */
  undecodable: number;
  /** Files that could not be read this time; retried on the next scan. */
  unreadable: number;
  cancelled: boolean;
  durationMs: number;
}

/** Mirrors `IndexEvent` in src-tauri/src/lib.rs. */
export type IndexEvent =
  | { state: "running"; libraryPath: string; progress: ScanProgress }
  | { state: "indexed"; libraryPath: string; summary: ScanSummary }
  | { state: "finished"; libraryPath: string; summary: ScanSummary; thumbnails: ThumbSummary }
  | { state: "failed"; libraryPath: string; message: string }
  /** The folder is reachable (again). Sent at the start of every scan. */
  | { state: "online"; libraryPath: string; writable: boolean }
  /** The folder can't be reached; cached data stays on screen. */
  | { state: "offline"; libraryPath: string }
  /** Changes made on other devices were merged in. */
  | { state: "synced"; libraryPath: string };

export interface LibraryStats {
  photoCount: number;
  scanning: boolean;
}

/** Subscribes to scan events. Resolves to a function that unsubscribes. */
export function onIndexEvent(handler: (event: IndexEvent) => void): Promise<UnlistenFn> {
  return listen<IndexEvent>("index-event", (e) => handler(e.payload));
}

/** Checks for new and changed files, or with `everything`, reads every file again. */
export function rescanLibrary(path: string, everything = false): Promise<void> {
  return invoke("rescan_library", { path, everything });
}

export function closeLibrary(path: string): Promise<void> {
  return invoke("close_library", { path });
}

export function libraryStats(path: string): Promise<LibraryStats> {
  return invoke("library_stats", { path });
}

// --- Photos and thumbnails --------------------------------------------------

/**
 * Dates of photos (`sortDate`, `DayGroup.day`) are the wall-clock time the
 * camera showed, encoded as if that time were UTC. Always format them with
 * `timeZone: "UTC"`.
 */

/** One day of the timeline. Mirrors `DayGroup` in src-tauri/src/db.rs. */
export interface DayGroup {
  /** Days since 1970-01-01; null when sorted by name (one group for all). */
  day: number | null;
  count: number;
}

/** Mirrors `PhotoItem` in src-tauri/src/db.rs. */
export interface PhotoItem {
  fingerprint: string;
  relPath: string;
  sortDate: number;
  /** Upright size in pixels, when known. */
  width: number | null;
  height: number | null;
  favorite: boolean;
  video: boolean;
  /** Length of a video, when known. */
  durationMs: number | null;
  /** For a Live Photo: the fingerprint of its video. */
  liveVideo: string | null;
  /** For a JPEG + RAW pair: the fingerprint of its RAW file. */
  rawFile: string | null;
  /** How the photo is shown, if it was edited. */
  edit: Edit | null;
}

/** Mirrors `FolderEntry` in src-tauri/src/db.rs. */
export interface FolderEntry {
  /** Path relative to the library root; "" is the root itself. */
  folder: string;
  /** Photos directly in this folder. */
  count: number;
}

/** Mirrors `PhotoDetails` in src-tauri/src/db.rs. */
export interface PhotoDetails {
  relPath: string;
  fingerprint: string;
  size: number;
  sortDate: number;
  dateFromExif: boolean;
  width: number | null;
  height: number | null;
  cameraMake: string | null;
  cameraModel: string | null;
  latitude: number | null;
  longitude: number | null;
  favorite: boolean;
  hidden: boolean;
  /** Ids of the albums the photo is in. */
  albums: string[];
  video: boolean;
  durationMs: number | null;
  liveVideo: string | null;
  /** For a JPEG + RAW pair: where its RAW file is. */
  rawPath: string | null;
  /** The nearest town, such as "Paris, France". */
  place: string | null;
}

/** What a photo listing shows. Mirrors `Listing` in src-tauri/src/db.rs. */
export type Listing =
  /** Every photo that is not hidden. */
  | { kind: "library" }
  /** The photos inside one folder ("" is the library root), and in its subfolders if asked. */
  | { kind: "folder"; folder: string; subfolders?: boolean }
  | { kind: "favorites" }
  | { kind: "album"; album: string }
  | { kind: "hidden" }
  /** The photos that show one person: the given groups of faces. */
  | { kind: "person"; clusters: number[] }
  /** The photos that match what was typed in the search box. */
  | { kind: "search"; query: string }
  /** The photos taken inside a rectangle on the map. */
  | { kind: "area"; south: number; west: number; north: number; east: number }
  /** Another listing narrowed down by the filter menu. */
  | { kind: "only"; within: Listing; only: Only };

/** What the filter menu can narrow a listing down to. Mirrors `Only`. */
export type Only = "favorites" | "edited" | "photos" | "videos" | "notInAlbum";

/** Day groups of a listing, newest first. */
/** How the photo grid orders a listing. Mirrors `Order` in src-tauri/src/db.rs. */
export type Order = "dateDesc" | "dateAsc" | "nameAsc" | "nameDesc";

export function timeline(path: string, listing: Listing, order: Order = "dateDesc"): Promise<DayGroup[]> {
  return invoke("timeline", { path, listing, order });
}

/** Mirrors `MonthSummary` in src-tauri/src/db.rs. */
export interface MonthSummary {
  year: number;
  /** 1 to 12. */
  month: number;
  count: number;
  /** Its busiest days (at most four), in date order. */
  days: DaySummary[];
}

/** Mirrors `DaySummary` in src-tauri/src/db.rs. */
export interface DaySummary {
  /** Days since 1970-01-01, as in `DayGroup`. */
  day: number;
  count: number;
  /** Fingerprint of the photo that stands for the day. */
  cover: string;
}

/** The months of a listing with a few cover photos each, newest first. */
export function listMonths(path: string, listing: Listing): Promise<MonthSummary[]> {
  return invoke("months", { path, listing });
}

/** Every photo's width / height × 1000, in grid order, for the river layout. */
export function shapes(path: string, listing: Listing, order: Order): Promise<number[]> {
  return invoke("shapes", { path, listing, order });
}

/** A slice of a listing's photos, in the same order as `timeline`. */
export function photosPage(
  path: string,
  listing: Listing,
  order: Order,
  offset: number,
  limit: number,
): Promise<PhotoItem[]> {
  return invoke("photos_page", { path, listing, order, offset, limit });
}

export function listFolders(path: string): Promise<FolderEntry[]> {
  return invoke("folders", { path });
}

export function photoDetails(path: string, relPath: string): Promise<PhotoDetails | null> {
  return invoke("photo_details", { path, relPath });
}

/**
 * URL of a cached thumbnail, served by the app's `thumb://` protocol.
 * "grid" is about 256 px on the short side, "preview" about 1024 px on the long side.
 */
export function thumbUrl(fingerprint: string, size: "grid" | "preview" = "grid"): string {
  return convertFileSrc(`${size}/${fingerprint}`, "thumb");
}

/**
 * URL of the full-size photo, or of the video file, served by the app's
 * `photo://` protocol.
 */
export function photoUrl(fingerprint: string): string {
  return convertFileSrc(fingerprint, "photo");
}

/** A group of photos taken close together. Mirrors `Cluster` in src-tauri/src/geo.rs. */
export interface MapCluster {
  /** Where the marker goes: the average position of the photos. */
  latitude: number;
  longitude: number;
  count: number;
  /** The newest photo, shown on the marker. */
  fingerprint: string;
  /** The rectangle that holds all the photos. */
  south: number;
  west: number;
  north: number;
  east: number;
}

/** The markers for one rectangle of the map at one zoom level. */
export function mapClusters(
  path: string,
  bounds: { south: number; west: number; north: number; east: number },
  zoom: number,
): Promise<MapCluster[]> {
  return invoke("map_clusters", { path, ...bounds, zoom });
}

// --- People (face grouping) ------------------------------------------------------

/** Mirrors `FacesStatus` in src-tauri/src/lib.rs. */
export interface FacesStatus {
  /** True when faces are searched: models downloaded and not switched off in Settings. */
  enabled: boolean;
  /** True once the face models are on this computer. */
  downloaded: boolean;
  /** Size of the download that switching it on needs. */
  downloadBytes: number;
}

/** Mirrors `Person` in src-tauri/src/people.rs. */
export interface Person {
  /** `null` until someone names them. */
  name: string | null;
  /** The face groups that make up this person on this computer. */
  clusters: number[];
  /** Photos they appear in (approximate for merged people). */
  photos: number;
  /** Hidden from the People screen, on this computer. */
  hidden: boolean;
  /** The photo to show them with, and the square around their face in it. */
  cover: string;
  coverCrop: [number, number, number, number];
  coverWidth: number | null;
  coverHeight: number | null;
}

export function facesStatus(): Promise<FacesStatus> {
  return invoke("faces_status");
}

/** Stops searching for faces; the people already found stay. */
export function disableFaces(): Promise<void> {
  return invoke("disable_faces");
}

/** Switches face grouping on: downloads the face models if needed, then searches the open library for faces. */
export function enableFaces(): Promise<void> {
  return invoke("enable_faces");
}

/** Calls `handler` with the bytes received so far while the models download. */
export function onFacesDownload(handler: (received: number) => void): Promise<UnlistenFn> {
  return listen<number>("faces-download", (event) => handler(event.payload));
}

export function listPeople(path: string): Promise<Person[]> {
  return invoke("people", { path });
}

/** Unnamed groups of faces that might be the person `name`, most alike first. */
export function personSuggestions(path: string, clusters: number[], name: string): Promise<Person[]> {
  return invoke("person_suggestions", { path, clusters, name });
}

/** Says a suggested group is not the person `name`: it isn't suggested again. */
export function rejectSuggestion(path: string, cluster: number, name: string): Promise<void> {
  return invoke("reject_suggestion", { path, cluster, name });
}

/** Hides a person from the People screen (on this computer), or shows them again. */
export function setPersonHidden(path: string, clusters: number[], hidden: boolean): Promise<void> {
  return invoke("set_person_hidden", { path, clusters, hidden });
}

/** Takes photos out of a person, whose face in them was someone else's. */
export function removeFromPerson(
  path: string,
  clusters: number[],
  photos: string[],
  name: string | null,
): Promise<number> {
  return invoke("remove_from_person", { path, clusters, photos, name });
}

/**
 * Moves these photos' faces in a person into a new person of their own, kept
 * apart from them. Returns the new person's group, or null if none moved.
 */
export function splitPerson(
  path: string,
  clusters: number[],
  photos: string[],
  name: string | null,
): Promise<number | null> {
  return invoke("split_person", { path, clusters, photos, name });
}

/** Names a person; an empty name removes it. The same name merges two people. */
export function namePerson(path: string, clusters: number[], name: string): Promise<void> {
  return invoke("name_person", { path, clusters, name });
}

// The Linux webview can't play video from `photo://`; there the app serves
// videos over local HTTP instead (see src-tauri/src/media_server.rs).
let mediaBase: string | null = null;
invoke<string | null>("media_base")
  .then((base) => (mediaBase = base))
  .catch(() => {});

/** URL a `<video>` element can play the video file from. */
export function videoUrl(fingerprint: string): string {
  return mediaBase ? `${mediaBase}/${fingerprint}` : photoUrl(fingerprint);
}

// --- Organization: favorites, hidden photos, albums ---------------------------
//
// These are shared by every computer that opens the library folder; see
// src-tauri/src/org.rs for how.

/** One change to the organization. Mirrors `Op` in src-tauri/src/org.rs. */
export type Op =
  | { op: "fav"; photo: string; on: boolean }
  | { op: "hide"; photo: string; on: boolean }
  /** Names or renames an album. */
  | { op: "album_name"; album: string; name: string }
  | { op: "album_deleted"; album: string; on: boolean }
  | { op: "album_member"; album: string; photo: string; on: boolean }
  /** Sets how a photo is shown; `null` shows it as it is again. */
  | { op: "edit"; photo: string; edit: Edit | null }
  /** Names the person whose face is at a point of a photo. */
  | { op: "face_name"; photo: string; x: number; y: number; name: string };

/** Mirrors `Album` in src-tauri/src/db.rs. */
export interface Album {
  id: string;
  name: string;
  /** Photos of the album that are indexed and not hidden. */
  count: number;
}

export function listAlbums(path: string): Promise<Album[]> {
  return invoke("albums", { path });
}

/** Records changes. Rejects if the library is offline. */
export function applyOps(path: string, ops: Op[]): Promise<void> {
  return invoke("apply_ops", { path, ops });
}

/** Creates an album and resolves to its id. */
export function createAlbum(path: string, name: string): Promise<string> {
  return invoke("create_album", { path, name });
}

/** Merges changes from other devices. Resolves to true if there were any. */
export function syncLibrary(path: string): Promise<boolean> {
  return invoke("sync_library", { path });
}
