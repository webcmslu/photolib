// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { t } from "./i18n";
import {
  convertFileSrc as tauriFileSrc,
  invoke as tauriInvoke,
  type InvokeArgs,
  type InvokeOptions,
} from "@tauri-apps/api/core";
import type { Edit, Enhance } from "./edit";
import type { LabelColor } from "./labels";
import { listen as tauriListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { server } from "./platform";

// In the desktop app, commands are Tauri commands. With a Phototek server
// (in a browser it served, or chosen in the app), they are
// `POST /api/<command>` with the same arguments and results (see
// src-tauri/server/src/commands.rs).
async function invoke<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  const remote = server();
  if (!remote) return tauriInvoke<T>(command, args, options);
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (remote.token) headers.Authorization = `Bearer ${remote.token}`;
  const response = await fetch(`${remote.url}/api/${command}`, {
    method: "POST",
    headers,
    body: JSON.stringify(args ?? {}),
  });
  const body = await response.json().catch(() => null);
  if (!response.ok) throw body ?? response.statusText;
  return body as T;
}

/** The URL of a thumbnail or a photo: a Tauri protocol, or the server's (with the token: an `<img>` can't send headers). */
function convertFileSrc(path: string, protocol: string): string {
  const remote = server();
  if (!remote) return tauriFileSrc(path, protocol);
  return `${remote.url}/${protocol}/${path}${remote.token ? `?t=${remote.token}` : ""}`;
}

/**
 * Events: from the app (Tauri), or from the server's `/api/events` stream,
 * one connection shared by all the listeners and closed with the last one.
 */
function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  const remote = server();
  if (!remote) return tauriListen<T>(event, handler);
  const key = `${remote.url}|${remote.token ?? ""}`;
  if (!events || events.key !== key) {
    events?.source.close();
    const query = remote.token ? `?t=${remote.token}` : "";
    events = { key, source: new EventSource(`${remote.url}/api/events${query}`), listeners: 0 };
  }
  const shared = events;
  const onMessage = (message: MessageEvent<string>) => {
    try {
      handler({ event, id: 0, payload: JSON.parse(message.data) as T } as Parameters<EventCallback<T>>[0]);
    } catch {
      // A malformed event is skipped.
    }
  };
  shared.source.addEventListener(event, onMessage);
  shared.listeners++;
  return Promise.resolve(() => {
    shared.source.removeEventListener(event, onMessage);
    if (--shared.listeners === 0) {
      shared.source.close();
      if (events === shared) events = null;
    }
  });
}
let events: { key: string; source: EventSource; listeners: number } | null = null;

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
    title: t("Open library folder"),
  });
  return typeof picked === "string" ? picked : null;
}

/** Lets the user choose the folder to export photos into. */
export async function pickExportFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, title: t("Export to") });
  return typeof picked === "string" ? picked : null;
}

// --- Import from a memory card, camera or folder (see importer.rs) -------------

/** A connected drive with a DCIM folder, or a phone or camera on USB (Mac, Windows; on Linux the desktop mounts them as folders). */
export interface ImportSource {
  path: string;
  name: string;
  /** A phone that is locked, or does not trust this computer yet. */
  restricted: boolean;
}

/** A photo or video of the source with its companions (Live Photo video, RAW). */
export interface ImportTile {
  group: number;
  name: string;
  taken: number | null;
  bytes: number;
  video: boolean;
  live: boolean;
  raw: boolean;
  /** Of `bytes`, those of RAW files. */
  rawBytes: number;
  already: boolean;
  unreadable: boolean;
  /** For the thumbnail: importthumb://localhost/<generation>/<thumb>. */
  thumb: number;
}

export interface ImportPreview {
  /** Newest first. */
  tiles: ImportTile[];
  generation: number;
  new: number;
  newBytes: number;
  already: number;
  unreadable: number;
  /** Why (the first file that could not be read). */
  unreadableReason: string | null;
  /** Day numbers of the oldest and newest new file. */
  firstDay: number | null;
  lastDay: number | null;
}

export interface ImportSummary {
  imported: number;
  bytes: number;
  already: number;
  failed: [string, string][];
  cancelled: boolean;
}

export interface ImportProgress {
  done: number;
  total: number;
  bytes: number;
  totalBytes: number;
}

export function importSources(): Promise<ImportSource[]> {
  return invoke("import_sources");
}

export function importPreview(path: string, source: string): Promise<ImportPreview> {
  return invoke("import_preview", { path, source });
}

/** Where the RAW file of a JPEG + RAW pair goes. */
export type RawFiles = "beside" | "subfolder" | "skip";

/** Imports what is new on `source`, or only the tiles whose `group` is in `groups`. */
export function importFiles(
  path: string,
  source: string,
  folder: string,
  groups: number[] | null = null,
  rawFiles: RawFiles = "beside",
): Promise<ImportSummary> {
  return invoke("import_files", { path, source, folder, groups, rawFiles });
}

/** What freeing up space on a device would delete (from the last preview). */
export interface FreeUpPlan {
  /** Photos and videos that are safely in the library, their files and size. */
  photos: number;
  files: number;
  bytes: number;
  /** Kept because they were taken in the last days. */
  keptRecent: number;
  /** Kept because their copy in the library could not be found. */
  notChecked: number;
}

export interface FreeUpSummary {
  deleted: number;
  bytes: number;
  failed: [string, string][];
}

export function freeUpPlan(path: string, source: string, keepDays: number): Promise<FreeUpPlan> {
  return invoke("free_up_plan", { path, source, keepDays });
}

/** Deletes from a phone, camera or memory card what is safely in the library. */
export function freeUp(path: string, source: string, keepDays: number): Promise<FreeUpSummary> {
  return invoke("free_up", { path, source, keepDays });
}

export function onImportProgress(handler: (progress: ImportProgress) => void): Promise<UnlistenFn> {
  return listen<ImportProgress>("import-progress", (event) => handler(event.payload));
}

/** `[done, total]` while the files of a source are read before an import. */
export function onImportSurvey(handler: (done: number, total: number) => void): Promise<UnlistenFn> {
  return listen<[number, number]>("import-survey", (event) => handler(...event.payload));
}

export async function pickImportFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, title: t("Import from") });
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

/** Stops the export, or the moving of duplicates, that is running. */
export function cancelJob(): Promise<void> {
  return invoke("cancel_job");
}

export interface DuplicateCopy {
  relPath: string;
  size: number;
  mtimeMs: number;
}

/** A photo stored in several files. */
export interface DuplicateGroup {
  fingerprint: string;
  sortDate: number;
  video: boolean;
  copies: DuplicateCopy[];
  /** The copy the app suggests keeping. */
  keep: number;
}

/** Where extra copies are moved, at the top of the library. */
export const PARKED_FOLDER = "Duplicates to delete";

export function duplicateGroups(path: string): Promise<DuplicateGroup[]> {
  return invoke("duplicate_groups", { path });
}

export interface MoveSummary {
  moved: number;
  /** Copies left where they were, with why. */
  skipped: [string, string][];
  cancelled: boolean;
}

/** Moves the extra copies into the Duplicates to delete folder. */
export function parkDuplicates(
  path: string,
  choices: { keep: string; extras: string[] }[],
): Promise<MoveSummary> {
  return invoke("park_duplicates", { path, choices });
}

/** How many copies wait in the Duplicates to delete folder. */
export function parkedDuplicates(path: string): Promise<number> {
  return invoke("parked_duplicates", { path });
}

export function putBackDuplicates(path: string): Promise<MoveSummary> {
  return invoke("put_back_duplicates", { path });
}

/** `[done, total]` while extra copies are moved. */
export function onDuplicatesProgress(handler: (done: number, total: number) => void): Promise<UnlistenFn> {
  return listen<[number, number]>("duplicates-progress", (event) => handler(...event.payload));
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
    title: t("Choose an Apple Photos library"),
    filters: onMac ? [{ name: t("Photos Library"), extensions: ["photoslibrary"] }] : undefined,
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

/** Mirrors `Progress` in src-tauri/core/src/indexer.rs. */
export interface ScanProgress {
  phase: "scanning" | "indexing" | "thumbnails" | "comparing" | "faces" | "content";
  /** Image files found so far by the folder walk. */
  discovered: number;
  /** Items the current phase has to process, and how many are done. */
  total: number;
  done: number;
  /** Files the index already held when the scan started. */
  known: number;
}

/** Mirrors `ScanSummary` in src-tauri/core/src/indexer.rs. */
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

/** Mirrors `ThumbSummary` in src-tauri/core/src/thumbs.rs. */
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

/** One day of the timeline. Mirrors `DayGroup` in src-tauri/core/src/db.rs. */
export interface DayGroup {
  /** Days since 1970-01-01; null when sorted by name (one group for all). */
  day: number | null;
  count: number;
  /** How many of `count` are videos. */
  videos: number;
}

/** Mirrors `PhotoItem` in src-tauri/core/src/db.rs. */
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
  /** 0 (not rated) to 5 stars. */
  rating: number;
  /** Its colour label, if any. */
  label: LabelColor | null;
}

/** Mirrors `FolderEntry` in src-tauri/core/src/db.rs. */
export interface FolderEntry {
  /** Path relative to the library root; "" is the root itself. */
  folder: string;
  /** Photos directly in this folder. */
  count: number;
}

/** Mirrors `PhotoDetails` in src-tauri/core/src/db.rs. */
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

/** What a photo listing shows. Mirrors `Listing` in src-tauri/core/src/db.rs. */
export type Listing =
  /** Every photo that is not hidden. */
  | { kind: "library" }
  /** The photos inside one folder ("" is the library root), and in its subfolders if asked. */
  | { kind: "folder"; folder: string; subfolders?: boolean }
  | { kind: "favorites" }
  | { kind: "album"; album: string }
  | { kind: "hidden" }
  /** The photos stored more than once, every copy listed. */
  | { kind: "duplicates" }
  /** The photos that show one person: the given groups of faces. */
  | { kind: "person"; clusters: number[] }
  /** The photos that match what was typed in the search box. */
  | { kind: "search"; query: string }
  /** The photos taken inside a rectangle on the map. */
  | { kind: "area"; south: number; west: number; north: number; east: number }
  /** The photos taken from the day `from` to the day `to` (days since 1970), both included. */
  | { kind: "days"; from: number; to: number }
  /** A chosen set of photos: a memory, the featured photos. */
  | { kind: "picked"; fingerprints: string[] }
  /** Another listing narrowed down to a camera, a lens, an aperture or a focal length. */
  | ({ kind: "gear"; within: Listing } & Gear)
  /** Another listing narrowed down by the filter menu. */
  | { kind: "only"; within: Listing; only: Only };

/** What the camera filter narrows a listing down to (each optional). Mirrors `Gear` in db.rs. */
export interface Gear {
  cameraMake?: string | null;
  cameraModel?: string | null;
  lens?: string | null;
  /** The f-number, to one decimal. */
  aperture?: number | null;
  /** The focal length in whole mm. */
  focalLength?: number | null;
}

/** What the camera filter offers for a listing, with photo counts. */
export interface GearOptions {
  cameras: [string | null, string | null, number][];
  lenses: [string, number][];
  apertures: [number, number][];
  focalLengths: [number, number][];
  /** Photos whose lens and settings are not read yet. */
  unread: number;
}

export function gearOptions(path: string, listing: Listing): Promise<GearOptions> {
  return invoke("gear_options", { path, listing });
}

/** What the filter menu can narrow a listing down to. Mirrors `Only`. */
export type Only =
  | "favorites"
  | "edited"
  | "photos"
  | "videos"
  /** RAW files, alone or behind a JPEG or HEIC of the same shot. */
  | "raw"
  /** Taken with the front camera. */
  | "selfies"
  | "livePhotos"
  /** With depth data: an iPhone's Portrait mode. */
  | "portraits"
  | "panoramas"
  | "timeLapses"
  | "sloMos"
  | "bursts"
  | "screenshots"
  | "notInAlbum"
  /** Rated at least this many stars. */
  | "stars1"
  | "stars2"
  | "stars3"
  | "stars4"
  | "stars5"
  | LabelColor;

/** Day groups of a listing, newest first. */
/** How the photo grid orders a listing. Mirrors `Order` in src-tauri/core/src/db.rs. */
export type Order = "dateDesc" | "dateAsc" | "nameAsc" | "nameDesc";

export function timeline(path: string, listing: Listing, order: Order = "dateDesc"): Promise<DayGroup[]> {
  return invoke("timeline", { path, listing, order });
}

/** Mirrors `MonthSummary` in src-tauri/core/src/db.rs. */
export interface MonthSummary {
  year: number;
  /** 1 to 12. */
  month: number;
  count: number;
  /** Its busiest days (at most four), in date order. */
  days: DaySummary[];
}

/** Mirrors `DaySummary` in src-tauri/core/src/db.rs. */
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

/** How a photo was taken; mirrors `Shooting` in src-tauri/core/src/metadata.rs. */
export interface Shooting {
  lens: string | null;
  /** In mm, and the 35 mm equivalent. */
  focalLength: number | null;
  focalLength35mm: number | null;
  /** The f-number: 2.8 for f/2.8. */
  aperture: number | null;
  /** Exposure time in seconds. */
  exposure: number | null;
  iso: number | null;
  /** Exposure compensation in EV. */
  exposureBias: number | null;
  flash: boolean | null;
  software: string | null;
}

/** Read from the file when asked (not kept in the index). */
export function photoShooting(path: string, relPath: string): Promise<Shooting | null> {
  return invoke("photo_shooting", { path, relPath });
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

/** A group of photos taken close together. Mirrors `Cluster` in src-tauri/core/src/geo.rs. */
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

/** Mirrors `Person` in src-tauri/core/src/people.rs. */
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

/** Whether search by content (what photos show) is on, as for the faces. */
export function contentStatus(): Promise<FacesStatus> {
  return invoke("content_status");
}

/** Stops looking at new photos; what was found stays searchable. */
export function disableContent(): Promise<void> {
  return invoke("disable_content");
}

/** Switches search by content on: downloads the model if needed, then looks at the open library's photos. */
export function enableContent(): Promise<void> {
  return invoke("enable_content");
}

/** Calls `handler` with the bytes received so far while the model downloads. */
export function onContentDownload(handler: (received: number) => void): Promise<UnlistenFn> {
  return listen<number>("content-download", (event) => handler(event.payload));
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

/** Where a face of a person's groups is in a photo. Mirrors `FaceBox` in db.rs. */
export interface FaceBox {
  fingerprint: string;
  /** Left, top, width, height, as fractions of the upright photo. */
  bounds: [number, number, number, number];
}

/** The faces of a person's groups, one per photo, to show where they are. */
export function personFaces(path: string, clusters: number[]): Promise<FaceBox[]> {
  return invoke("person_faces", { path, clusters });
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
/** Makes the photo `fingerprint` the one the card of the person `name` shows. */
export function makeKeyPhoto(path: string, clusters: number[], name: string, fingerprint: string): Promise<void> {
  return invoke("make_key_photo", { path, clusters, name, photo: fingerprint });
}

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
  return mediaBase && !server() ? `${mediaBase}/${fingerprint}` : photoUrl(fingerprint);
}

// --- Organization: favorites, hidden photos, albums ---------------------------
//
// These are shared by every computer that opens the library folder; see
// src-tauri/core/src/org.rs for how.

/** One change to the organization. Mirrors `Op` in src-tauri/core/src/org.rs. */
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
  | { op: "face_name"; photo: string; x: number; y: number; name: string }
  /** 1 to 5 stars; 0 takes the rating away. */
  | { op: "rating"; photo: string; stars: number }
  /** A colour label; "" takes it away. */
  | { op: "label"; photo: string; color: LabelColor | "" };

/** Mirrors `Album` in src-tauri/core/src/db.rs. */
export interface Album {
  id: string;
  name: string;
  /** Photos of the album that are indexed and not hidden. */
  count: number;
  /** Its latest photo. */
  cover: string | null;
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

// --- Similar shots -------------------------------------------------------------

/** Mirrors `Shot` in src-tauri/core/src/similar.rs. */
export interface SimilarShot {
  fingerprint: string;
  relPath: string;
  sortDate: number;
  width: number | null;
  height: number | null;
  favorite: boolean;
  rating: number;
}

/** Shots taken moments apart that look alike, in the order they were taken. */
export interface SimilarGroup {
  shots: SimilarShot[];
  /** The shot the app suggests keeping. */
  keep: number;
}

/** Groups of similar shots among the photos that are not hidden, newest first. */
/**
 * Draws `edit` into the photo's file (JPEG only). The photo keeps its
 * favorite, albums, rating, label and named faces.
 */
export function applyEditToOriginal(
  path: string,
  relPath: string,
  fingerprint: string,
  edit: Edit,
): Promise<void> {
  return invoke("apply_edit_to_original", { path, relPath, fingerprint, edit });
}

/** The magic wand's corrections for a photo (see enhance.rs). */
export function autoEnhance(fingerprint: string): Promise<Enhance> {
  return invoke("auto_enhance", { fingerprint });
}

export function similarGroups(path: string): Promise<SimilarGroup[]> {
  return invoke("similar_groups", { path });
}

// --- Collections -----------------------------------------------------------------

/** Mirrors `MemoryKind` in src-tauri/core/src/collections.rs. */
export type MemoryKind = "trip" | "event" | "onThisDay" | "person" | "together" | "season" | "year";

/** Mirrors `Memory` in src-tauri/core/src/collections.rs. */
export interface Memory {
  id: string;
  kind: MemoryKind;
  /** A place, a folder's name or a person's name; the page words the rest. */
  title: string | null;
  /** The people it is about (together), or who is in it most. */
  people: string[];
  /** Its first and last day, in days since 1970. */
  fromDay: number;
  toDay: number;
  /** For "On This Day": the earlier years it has photos from. */
  years: number[];
  cover: string;
  /** The photos it plays, in date order. */
  photos: string[];
  /** All its photos, which `listing` shows. */
  count: number;
  listing: Listing;
}

export interface Trip {
  /** The same as its memory's. */
  id: string;
  title: string;
  fromDay: number;
  toDay: number;
  count: number;
  cover: string;
}

export interface RecentDay {
  day: number;
  count: number;
  cover: string;
}

/** Mirrors `Collections` in src-tauri/core/src/collections.rs. */
export interface Collections {
  /** Every memory, newest first. */
  memories: Memory[];
  /** Today's memories: positions in `memories`. */
  today: number[];
  trips: Trip[];
  featured: string[];
  recentDays: RecentDay[];
  /** Photos per media type, for the types that have some. */
  media: Partial<Record<Only, number>>;
  favorites: number;
  hidden: number;
  duplicates: number;
  /** The town home is in, these days. */
  home: string | null;
}

/** The Collections page's rows. `today` is the viewer's date in days since 1970. */
export function collections(path: string, today: number): Promise<Collections> {
  return invoke("collections", { path, today });
}

/** Today's date as the photos' days count it: wall-clock days since 1970. */
export function todayDay(): number {
  const now = new Date();
  return Math.floor(Date.UTC(now.getFullYear(), now.getMonth(), now.getDate()) / 86_400_000);
}

/** The town most of a listing's photos were taken in ("Cattolica, Italy"), to name a pinned place. */
export function listingPlace(path: string, listing: Listing): Promise<string | null> {
  return invoke("listing_place", { path, listing });
}

/** Writes a memory's video to `path` (an .mp4 chosen in the save dialog). */
export function saveVideo(path: string, bytes: Uint8Array): Promise<void> {
  return invoke("save_video", bytes, { headers: { path: encodeURIComponent(path) } });
}
