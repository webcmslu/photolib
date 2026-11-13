// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

/** An opened library folder. Mirrors `LibraryInfo` in Rust. */
export interface LibraryInfo {
  path: string;
  name: string;
  /** False when the folder is read-only: organization stays local-only. */
  writable: boolean;
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
  phase: "scanning" | "indexing" | "thumbnails";
  /** Image files found so far by the folder walk. */
  discovered: number;
  /** Items the current phase has to process, and how many are done. */
  total: number;
  done: number;
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
  | { state: "failed"; libraryPath: string; message: string };

export interface LibraryStats {
  photoCount: number;
  scanning: boolean;
}

/** Subscribes to scan events. Resolves to a function that unsubscribes. */
export function onIndexEvent(handler: (event: IndexEvent) => void): Promise<UnlistenFn> {
  return listen<IndexEvent>("index-event", (e) => handler(e.payload));
}

export function rescanLibrary(path: string): Promise<void> {
  return invoke("rescan_library", { path });
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
  /** Days since 1970-01-01. */
  day: number;
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
}

/** Day groups, newest first, of the whole library (`folder` null) or one folder. */
export function timeline(path: string, folder: string | null): Promise<DayGroup[]> {
  return invoke("timeline", { path, folder });
}

/** A slice of the photo listing, in the same order as `timeline`. */
export function photosPage(
  path: string,
  folder: string | null,
  offset: number,
  limit: number,
): Promise<PhotoItem[]> {
  return invoke("photos_page", { path, folder, offset, limit });
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

/** URL of the full-size photo, served by the app's `photo://` protocol. */
export function photoUrl(fingerprint: string): string {
  return convertFileSrc(fingerprint, "photo");
}
