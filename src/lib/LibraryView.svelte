<!--
  An open library: a sidebar (Library, Favorites, Hidden, Map, albums,
  folders), the photo grid for whatever is selected there (or the map), the
  scan status, and the viewer on top.
-->
<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    appleImport,
    appleImportPreview,
    applyOps,
    closeLibrary,
    createAlbum,
    libraryStats,
    listAlbums,
    listFolders,
    listMonths,
    onIndexEvent,
    pickPhotosLibrary,
    removeFromPerson,
    splitPerson,
    rescanLibrary,
    setPersonHidden,
    syncLibrary,
    timeline,
    shapes,
    type Album,
    type DayGroup,
    type FolderEntry,
    type ImportSummary,
    type IndexEvent,
    type LibraryInfo,
    type Listing,
    type MonthSummary,
    type Only,
    type Op,
    type ScanProgress,
    type ScanSummary,
    type ThumbSummary,
  } from "./api";
  import { formatDayRange, formatNumber, monthKey, photoCount } from "./format";
  import MapView from "./MapView.svelte";
  import Overview from "./Overview.svelte";
  import PeopleView from "./PeopleView.svelte";
  import PersonSuggestions from "./PersonSuggestions.svelte";
  import { PhotoPages } from "./pages.svelte";
  import PhotoGrid from "./PhotoGrid.svelte";
  import Viewer from "./Viewer.svelte";
  import Slideshow from "./Slideshow.svelte";
  import AboutButton from "./AboutButton.svelte";
  import SettingsButton from "./SettingsButton.svelte";
  import ExportDialog from "./ExportDialog.svelte";
  import DuplicatesView from "./DuplicatesView.svelte";
  import { REVEAL_LABEL, revealPhoto } from "./reveal";
  import { showMenu, type MenuEntry } from "./contextMenu";
  import { LABELS, labelForKey, type LabelColor } from "./labels";
  import { sep } from "@tauri-apps/api/path";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { saveSettings, settings } from "./settings.svelte";

  interface Props {
    library: LibraryInfo;
    onclose: () => void;
  }
  let { library, onclose }: Props = $props();

  /** How often an open library is rescanned for changes made outside the app. */
  const RESCAN_INTERVAL_MS = 15 * 60 * 1000;
  /** How often an offline library is tried again. */
  const OFFLINE_RETRY_MS = 30 * 1000;
  /** How often changes made on other computers are picked up. */
  const SYNC_INTERVAL_MS = 60 * 1000;
  /** How often the grid picks up newly indexed photos during a long first scan. */
  const LIVE_REFRESH_MS = 3000;

  // --- Scan and connection status ------------------------------------------

  let photoTotal = $state<number | null>(null);
  let scanning = $state(true); // opening a library starts a scan
  let progress = $state<ScanProgress | null>(null);
  let lastScan = $state<ScanSummary | null>(null);
  let lastThumbs = $state<ThumbSummary | null>(null);
  let error = $state<string | null>(null);
  // Initial values only: this component is re-created for another library.
  // svelte-ignore state_referenced_locally
  let online = $state(library.online);
  // svelte-ignore state_referenced_locally
  let writable = $state(library.writable);
  let lastLiveRefresh = 0;
  /** `progress.done` at the last live refresh, so a scan with nothing new does not reload. */
  let liveRefreshDone = 0;
  let lastScanStart = Date.now();

  onMount(() => {
    const unlisten = onIndexEvent(handleIndexEvent);
    const scanTimer = setInterval(() => {
      const due = online ? RESCAN_INTERVAL_MS : OFFLINE_RETRY_MS;
      if (!scanning && Date.now() - lastScanStart >= due) rescan();
    }, 5000);
    const syncTimer = setInterval(async () => {
      if (!online) return;
      try {
        if (await syncLibrary(library.path)) refreshAll();
      } catch {
        // Not reachable this time; the next round tries again.
      }
    }, SYNC_INTERVAL_MS);

    // What the index already holds from an earlier session shows at once.
    libraryStats(library.path)
      .then((stats) => {
        photoTotal ??= stats.photoCount;
        // The scan may have finished before we were listening.
        if (!stats.scanning && !progress) scanning = false;
      })
      .catch((e) => (error = String(e)));
    loadSidebar();

    return () => {
      clearInterval(scanTimer);
      clearInterval(syncTimer);
      unlisten.then((stop) => stop());
      closeLibrary(library.path).catch(() => {});
    };
  });

  function handleIndexEvent(event: IndexEvent) {
    if (event.libraryPath !== library.path) return;
    switch (event.state) {
      case "running": {
        scanning = true;
        progress = event.progress;
        // New photos are written while the folders are still being walked,
        // so show them as they come instead of waiting for the walk to end.
        const { phase, done } = event.progress;
        const now = Date.now();
        if (
          (phase === "scanning" || phase === "indexing") &&
          done > liveRefreshDone &&
          now - lastLiveRefresh > LIVE_REFRESH_MS
        ) {
          lastLiveRefresh = now;
          liveRefreshDone = done;
          refreshAll();
        }
        break;
      }
      case "online":
        // Sent at the start of every scan.
        liveRefreshDone = 0;
        online = true;
        writable = event.writable;
        break;
      case "offline":
        online = false;
        scanning = false;
        progress = null;
        break;
      case "synced":
        refreshAll();
        break;
      case "indexed":
        // The index is complete; thumbnails are still being made.
        lastScan = event.summary;
        photoTotal = event.summary.total;
        refreshAll();
        break;
      case "finished":
        scanning = false;
        progress = null;
        lastScan = event.summary;
        lastThumbs = event.thumbnails;
        photoTotal = event.summary.total;
        // Thumbnail making may have found files that are not images after all.
        if (event.thumbnails.undecodable > 0) refreshAll();
        break;
      case "failed":
        scanning = false;
        progress = null;
        error = event.message;
        break;
    }
  }

  let scanMenuOpen = $state(false);

  async function rescan(everything = false) {
    scanMenuOpen = false;
    error = null;
    scanning = true;
    lastScanStart = Date.now();
    try {
      await rescanLibrary(library.path, everything);
    } catch (e) {
      scanning = false;
      error = String(e);
    }
  }

  const status = $derived.by(() => {
    if (scanning) {
      if (progress?.phase === "indexing" && progress.total > 0) {
        return `Indexing ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      if (progress?.phase === "faces") {
        return `Finding faces: ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      if (progress?.phase === "comparing") {
        return `Comparing photos: ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      if (progress?.phase === "thumbnails") {
        return `Making thumbnails: ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      if (progress?.phase === "scanning" && progress.known > 0) {
        // A library seen before: only new and changed files are read.
        return `Checking for new photos: ${formatNumber(Math.min(progress.discovered, progress.known))} of ${formatNumber(progress.known)}`;
      }
      return progress ? `Scanning… ${formatNumber(progress.discovered)} found` : "Scanning…";
    }
    return photoTotal === null ? "" : photoCount(photoTotal);
  });

  const unreadable = $derived((lastScan?.failed ?? 0) + (lastThumbs?.unreadable ?? 0));

  // --- Sidebar: what can be listed ------------------------------------------

  let albums = $state<Album[]>([]);
  let folders = $state<FolderEntry[]>([]);
  // Raw (not a deep proxy): it is passed to the backend as is.
  let listing = $state.raw<Listing>({ kind: "library" });
  /** What the main area shows: a photo grid, the map or the people. */
  let section = $state<"photos" | "map" | "people" | "duplicates">("photos");
  /** The name of the person whose photos are listed. */
  let personName = $state<string | null>(null);
  let personHidden = $state(false);

  async function loadSidebar() {
    try {
      [albums, folders] = await Promise.all([listAlbums(library.path), listFolders(library.path)]);
      // The album or folder being shown may be gone (deleted on another computer).
      const gone =
        (listing.kind === "album" && !albums.some((a) => a.id === (listing as { album: string }).album)) ||
        (listing.kind === "folder" && !folderTree.paths.has((listing as { folder: string }).folder));
      if (gone) listing = { kind: "library" };
    } catch (e) {
      error = String(e);
    }
  }

  /** Folders whose subfolders are shown in the sidebar, kept per library. */
  // svelte-ignore state_referenced_locally
  const openFoldersKey = `photolib.openFolders.${library.path}`;
  let openFolders = $state<Set<string>>(
    (() => {
      try {
        return new Set(JSON.parse(localStorage.getItem(openFoldersKey) ?? "[]") as string[]);
      } catch {
        return new Set<string>();
      }
    })(),
  );
  function toggleFolder(path: string) {
    const next = new Set(openFolders);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    openFolders = next;
    localStorage.setItem(openFoldersKey, JSON.stringify([...next]));
  }

  /**
   * Folder rows for the sidebar, as a tree: every folder with photos plus its
   * ancestors. A folder lists its subfolders too, so its count includes
   * theirs. Rows under a closed folder are left out.
   */
  const folderTree = $derived.by(() => {
    const own = new Map(folders.map((f) => [f.folder, f.count]));
    const totals = new Map<string, number>();
    const parents = new Set<string>();
    for (const { folder, count } of folders) {
      if (folder === "") continue;
      const parts = folder.split("/");
      for (let depth = 1; depth <= parts.length; depth++) {
        const path = parts.slice(0, depth).join("/");
        totals.set(path, (totals.get(path) ?? 0) + count);
        if (depth < parts.length) parents.add(path);
      }
    }
    const all = [...totals.keys()].sort((a, b) => {
      const partsA = a.split("/");
      const partsB = b.split("/");
      for (let i = 0; i < Math.min(partsA.length, partsB.length); i++) {
        const order = partsA[i].localeCompare(partsB[i], undefined, { numeric: true });
        if (order !== 0) return order;
      }
      return partsA.length - partsB.length;
    });
    const rows: {
      path: string;
      name: string;
      depth: number;
      count: number;
      hasChildren: boolean;
    }[] = [];
    if (own.has("")) {
      rows.push({ path: "", name: "Top level", depth: 0, count: own.get("") ?? 0, hasChildren: false });
    }
    for (const path of all) {
      const parts = path.split("/");
      // Shown only if every folder above it is open.
      const hidden = parts.slice(0, -1).some((_, i) => !openFolders.has(parts.slice(0, i + 1).join("/")));
      if (hidden) continue;
      rows.push({
        path,
        name: parts[parts.length - 1],
        depth: parts.length - 1,
        count: totals.get(path) ?? 0,
        hasChildren: parents.has(path),
      });
    }
    return { rows, paths: new Set(["", ...totals.keys()]) };
  });

  /** The listing for a folder row: the folder with its subfolders. */
  const folderListing = (path: string): Listing =>
    path === "" ? { kind: "folder", folder: "" } : { kind: "folder", folder: path, subfolders: true };

  const currentAlbum = $derived(
    listing.kind === "album" ? (albums.find((a) => a.id === (listing as { album: string }).album) ?? null) : null,
  );

  const title = $derived.by(() => {
    switch (listing.kind) {
      case "library":
        return "Library";
      case "favorites":
        return "Favorites";
      case "hidden":
        return "Hidden";
      case "duplicates":
        return "Duplicates";
      case "album":
        return currentAlbum?.name ?? "Album";
      case "folder":
        return listing.folder || "Top level";
      case "area":
        return "Photos from the map";
      case "person":
        return personName ?? "Unnamed person";
      case "search":
        return `Search: ${listing.query}`;
    }
  });

  // --- Import from Apple Photos ---------------------------------------------------

  /** Apple Photos libraries exist only on a Mac. */
  const IS_MAC = navigator.userAgent.includes("Mac");

  /** The import being confirmed: which library, and what it would do. */
  let importing = $state<{ library: string; summary: ImportSummary; busy: boolean } | null>(null);
  /** What the last import did, shown once. */
  let imported = $state<ImportSummary | null>(null);

  async function chooseAppleLibrary() {
    error = null;
    imported = null;
    try {
      const chosen = await pickPhotosLibrary();
      if (!chosen) return;
      importing = {
        library: chosen,
        summary: await appleImportPreview(library.path, chosen),
        busy: false,
      };
    } catch (e) {
      error = String(e);
    }
  }

  async function confirmImport() {
    if (!importing) return;
    importing.busy = true;
    try {
      imported = await appleImport(library.path, importing.library);
      refreshAll();
    } catch (e) {
      error = String(e);
    }
    importing = null;
  }

  // --- Search -------------------------------------------------------------------

  let searchText = $state("");
  /** What was listed before the search, to go back to when it is cleared. */
  let beforeSearch: Listing = { kind: "library" };
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  /** Lists the matches a moment after the user stops typing. */
  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      const query = searchText.trim();
      if (query === "") {
        if (listing.kind === "search") listing = beforeSearch;
        return;
      }
      if (listing.kind !== "search") beforeSearch = listing;
      listing = { kind: "search", query };
      section = "photos";
    }, 250);
  }

  function clearSearch() {
    clearTimeout(searchTimer);
    searchText = "";
    if (listing.kind === "search") listing = beforeSearch;
  }

  // Choosing something in the sidebar ends the search.
  $effect(() => {
    if (listing.kind !== "search") searchText = "";
  });

  const emptyText = $derived.by(() => {
    if (only) return "Nothing here matches the filter.";
    switch (listing.kind) {
      case "favorites":
        return "No favorites yet. Open a photo and press the heart, or select photos in the grid.";
      case "hidden":
        return "No hidden photos.";
      case "duplicates":
        return "No duplicates: every photo is stored once.";
      case "album":
        return "This album is empty. Select photos in the grid and choose “Add to album”.";
      case "search":
        return "Nothing matches. Try a place, a camera, a folder or file name, a year (2024), a month (july) or “video”.";
      default:
        return scanning ? "Looking for photos…" : "No photos found in this folder.";
    }
  });

  // --- The listing on screen ---------------------------------------------------

  /**
   * The filter menu's choice. It belongs to the listing it was chosen for,
   * so opening another album or folder shows all of it again.
   */
  let filter = $state.raw<{ listing: Listing; only: Only } | null>(null);
  const only = $derived(filter?.listing === listing ? filter.only : null);
  let filterMenuOpen = $state(false);
  const FILTERS: [Only | null, string][] = [
    [null, "All Items"],
    ["favorites", "Favorites"],
    ["edited", "Edited"],
    ["photos", "Photos"],
    ["videos", "Videos"],
    ["notInAlbum", "Not in an Album"],
  ];
  function chooseFilter(choice: Only | null) {
    filter = choice === null ? null : { listing, only: choice };
    filterMenuOpen = false;
  }

  /** The grid's order, a setting shared by every library. */
  const order = $derived(settings.order);
  let sortMenuOpen = $state(false);
  const byName = $derived(order === "nameAsc" || order === "nameDesc");
  const ascending = $derived(order === "dateAsc" || order === "nameAsc");
  function chooseSort(by: "date" | "name" | null, up: boolean | null) {
    const name = by === null ? byName : by === "name";
    const asc = up === null ? ascending : up;
    settings.order = name ? (asc ? "nameAsc" : "nameDesc") : asc ? "dateAsc" : "dateDesc";
    saveSettings();
    sortMenuOpen = false;
  }

  const pages = $derived(
    new PhotoPages(library.path, only ? { kind: "only", within: listing, only } : listing, order),
  );
  let groups = $state<DayGroup[]>([]);
  /** False until the first timeline for the current listing has arrived. */
  let groupsLoaded = $state(false);
  const listedTotal = $derived(groups.reduce((sum, group) => sum + group.count, 0));
  /** Bumped whenever the listing is reloaded. */
  let revision = $state(0);

  // --- How the listing is shown ----------------------------------------------

  /** Reads a number the user set earlier (a size), within its limits. */
  function remembered(key: string, fallback: number, min: number, max: number): number {
    const stored = Number(localStorage.getItem(key));
    return stored >= min && stored <= max ? stored : fallback;
  }

  const CELL_MIN = 70;
  const CELL_MAX = 360;
  /** The width the grid's cells aim for: the zoom level. */
  let cellSize = $state(remembered("photolib.cellSize", 150, CELL_MIN, CELL_MAX));
  $effect(() => localStorage.setItem("photolib.cellSize", String(cellSize)));
  /** The days on screen in the grid, shown under the title. */
  let shownRange = $state<{ oldest: number; newest: number } | null>(null);
  function setShownRange(range: { oldest: number; newest: number } | null) {
    // Called on every scroll: only a real change re-renders the toolbar.
    if (range?.oldest !== shownRange?.oldest || range?.newest !== shownRange?.newest) {
      shownRange = range;
    }
  }

  /** Square thumbnails, or rows of photos at their own shape ("river"). */
  let gridFit = $state<"square" | "river">(
    // "whole" was the name of an earlier, letterboxed version of the river.
    ["river", "whole"].includes(localStorage.getItem("photolib.gridFit") ?? "") ? "river" : "square",
  );
  $effect(() => localStorage.setItem("photolib.gridFit", gridFit));
  /** Every photo's shape, for the river layout; loaded only for it. */
  let gridShapes = $state.raw<number[] | null>(null);
  async function loadShapes(target: PhotoPages) {
    try {
      const loaded = await shapes(target.libraryPath, target.listing, target.order);
      if (target === pages) gridShapes = loaded;
    } catch (e) {
      error = String(e);
    }
  }
  $effect(() => {
    if (gridFit === "river") loadShapes(pages);
    else gridShapes = null;
  });

  const SIDEBAR_MIN = 150;
  const SIDEBAR_MAX = 600;
  let sidebarWidth = $state(remembered("photolib.sidebarWidth", 220, SIDEBAR_MIN, SIDEBAR_MAX));
  $effect(() => localStorage.setItem("photolib.sidebarWidth", String(sidebarWidth)));

  let resizing: { pointerId: number; startX: number; startWidth: number } | null = null;
  function startResize(event: PointerEvent) {
    resizing = { pointerId: event.pointerId, startX: event.clientX, startWidth: sidebarWidth };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  function resize(event: PointerEvent) {
    if (resizing?.pointerId !== event.pointerId) return;
    const wanted = resizing.startWidth + event.clientX - resizing.startX;
    sidebarWidth = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, wanted));
  }
  function resizeWithKeys(event: KeyboardEvent) {
    const step = event.key === "ArrowLeft" ? -16 : event.key === "ArrowRight" ? 16 : 0;
    if (step === 0) return;
    event.preventDefault();
    sidebarWidth = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, sidebarWidth + step));
  }

  /** Years and months are overviews; "all" is the photo grid. */
  let viewMode = $state<"years" | "months" | "all">("all");
  let months = $state<MonthSummary[]>([]);
  /** Where the next view starts: a year for the months, a day for the grid. */
  let startYear = $state<number | null>(null);
  let startDay = $state<number | null>(null);

  async function loadMonths(target: PhotoPages) {
    try {
      const loaded = await listMonths(target.libraryPath, target.listing);
      if (target === pages) months = loaded;
    } catch (e) {
      error = String(e);
    }
  }

  // The overviews need the months; the grid does not.
  $effect(() => {
    if (viewMode !== "all") loadMonths(pages);
  });

  function setViewMode(mode: "years" | "months" | "all") {
    startYear = null;
    startDay = null;
    viewMode = mode;
  }

  /** Ctrl/Cmd with + or - zooms the grid, like in a browser. */
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "F5" && !slideshow && viewerIndex === null && section === "photos") {
      event.preventDefault();
      startSlideshow();
      return;
    }
    if (event.key === "Escape" && (filterMenuOpen || sortMenuOpen || scanMenuOpen)) {
      filterMenuOpen = false;
      sortMenuOpen = false;
      scanMenuOpen = false;
      return;
    }
    if (
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      viewerIndex === null &&
      section === "photos" &&
      selected.size > 0 &&
      !(event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement)
    ) {
      if (/^[1-5]$/.test(event.key)) {
        event.preventDefault();
        rateSelection(Number(event.key));
        return;
      }
      const color = labelForKey(event.key);
      if (color) {
        event.preventDefault();
        labelSelection(color);
        return;
      }
    }
    if (!(event.ctrlKey || event.metaKey) || viewerIndex !== null) return;
    const factor = event.key === "+" || event.key === "=" ? 1.25 : event.key === "-" ? 0.8 : 0;
    if (factor === 0) return;
    event.preventDefault();
    cellSize = Math.round(Math.min(CELL_MAX, Math.max(CELL_MIN, cellSize * factor)));
  }

  let grid = $state<PhotoGrid>();
  let viewerIndex = $state<number | null>(null);
  const selected = new SvelteSet<number>();

  /** The running slideshow: the selected photos, or else the whole listing. */
  let slideshow = $state<{ count: number; indexAt: (position: number) => number } | null>(null);
  function startSlideshow() {
    if (listedTotal === 0) return;
    if (selected.size > 0) {
      const order = [...selected].sort((a, b) => a - b);
      slideshow = { count: order.length, indexAt: (position) => order[position] };
    } else {
      slideshow = { count: listedTotal, indexAt: (position) => position };
    }
  }
  /** A slideshow of one month of the listing: `day` is any day in it. */
  function slideshowOfMonth(day: number) {
    const key = monthKey(day);
    let index = 0;
    let first = -1;
    let count = 0;
    for (const group of groups) {
      if (group.day !== null && monthKey(group.day) === key) {
        if (first < 0) first = index;
        count += group.count;
      } else if (first >= 0) {
        break; // a month's days are next to each other
      }
      index += group.count;
    }
    if (count > 0) slideshow = { count, indexAt: (position) => first + position };
  }
  /** Changes when face grouping is switched on or off in Settings. */
  let facesRevision = $state(0);
  let selectionAnchor: number | null = null;

  // Start afresh whenever something else is listed.
  $effect(() => {
    const target = pages;
    // Only a change of `pages` may re-run this, not the state it resets.
    untrack(() => {
      groups = [];
      months = [];
      startYear = null;
      startDay = null;
      groupsLoaded = false;
      viewerIndex = null;
      selected.clear();
      selectionAnchor = null;
      cancelAlbumEdits();
      loadGroups(target);
    });
  });

  // When photos leave the listing under the viewer (hidden, removed from the
  // album), stay on a valid photo, or close when none is left.
  $effect(() => {
    if (viewerIndex === null || !groupsLoaded) return;
    if (listedTotal === 0) viewerIndex = null;
    else if (viewerIndex >= listedTotal) viewerIndex = listedTotal - 1;
  });

  async function loadGroups(target: PhotoPages) {
    try {
      const loaded = await timeline(target.libraryPath, target.listing, target.order);
      // The river's shapes must match the listing, so they come along.
      if (gridFit === "river") await loadShapes(target);
      if (target !== pages) return; // something else is listed by now
      groups = loaded;
      groupsLoaded = true;
    } catch (e) {
      error = String(e);
    }
  }

  /** Re-reads everything on screen after the index or the organization changed. */
  function refreshAll() {
    pages.refresh();
    revision++;
    // Indexes shift when photos come and go, so a selection can't be kept.
    selected.clear();
    selectionAnchor = null;
    loadGroups(pages);
    if (viewMode !== "all") loadMonths(pages);
    loadSidebar();
  }

  function closeViewer() {
    const index = viewerIndex;
    viewerIndex = null;
    if (index !== null) grid?.reveal(index);
  }

  // --- Organizing ----------------------------------------------------------------

  /** Records changes, then reloads what is on screen. */
  async function apply(ops: Op[]) {
    if (ops.length === 0) return;
    error = null;
    try {
      await applyOps(library.path, ops);
    } catch (e) {
      error = String(e);
    }
    refreshAll();
  }

  function toggleSelected(index: number, range: boolean) {
    if (range && selectionAnchor !== null) {
      const from = Math.min(selectionAnchor, index);
      const to = Math.max(selectionAnchor, index);
      for (let i = from; i <= to; i++) selected.add(i);
    } else if (selected.has(index)) {
      selected.delete(index);
    } else {
      selected.add(index);
    }
    selectionAnchor = index;
  }

  /** Applies one change per selected photo. */
  /** Takes the selected photos out of the person listed: it wasn't them. */
  async function removeSelectionFromPerson() {
    if (listing.kind !== "person") return;
    const target = pages;
    const clusters = listing.clusters;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    try {
      await removeFromPerson(
        library.path,
        clusters,
        photos.flatMap((photo) => (photo ? [photo.fingerprint] : [])),
        personName,
      );
      selected.clear();
      refreshAll();
    } catch (e) {
      error = String(e);
    }
  }

  /** The selected photos show someone else: they become a person of their own, opened to be named. */
  async function splitSelectionFromPerson() {
    if (listing.kind !== "person") return;
    const target = pages;
    const clusters = listing.clusters;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    try {
      const made = await splitPerson(
        library.path,
        clusters,
        photos.flatMap((photo) => (photo ? [photo.fingerprint] : [])),
        personName,
      );
      selected.clear();
      if (made !== null) {
        personName = null;
        personHidden = false;
        listing = { kind: "person", clusters: [made] };
      }
      refreshAll();
    } catch (e) {
      error = String(e);
    }
  }

  /** The fingerprints being exported, while the Export dialog is open. */
  let exporting = $state<string[] | null>(null);
  async function exportSelection() {
    const target = pages;
    const photos = await Promise.all([...selected].sort((a, b) => a - b).map((index) => target.load(index)));
    if (target !== pages) return;
    exporting = photos.flatMap((photo) => (photo ? [photo.fingerprint] : []));
  }

  async function revealSelected() {
    const [index] = selected;
    const photo = index === undefined ? null : await pages.load(index);
    if (photo) revealPhoto(library.path, photo.relPath).catch((e) => (error = String(e)));
  }

  async function togglePersonHidden() {
    if (listing.kind !== "person") return;
    try {
      await setPersonHidden(library.path, listing.clusters, !personHidden);
      personHidden = !personHidden;
    } catch (e) {
      error = String(e);
    }
  }

  async function applyToSelection(makeOp: (fingerprint: string) => Op) {
    const target = pages;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    await apply(photos.flatMap((photo) => (photo ? [makeOp(photo.fingerprint)] : [])));
  }

  function addSelectionToAlbum(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const album = select.value;
    select.value = "";
    if (album) applyToSelection((photo) => ({ op: "album_member", album, photo, on: true }));
  }

  let newAlbumName = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let confirmingDelete = $state(false);

  function cancelAlbumEdits() {
    renaming = null;
    confirmingDelete = false;
  }

  async function submitNewAlbum(event: SubmitEvent) {
    event.preventDefault();
    const name = newAlbumName?.trim();
    newAlbumName = null;
    if (!name) return;
    try {
      const album = await createAlbum(library.path, name);
      await loadSidebar();
      listing = { kind: "album", album };
    } catch (e) {
      error = String(e);
    }
  }

  async function submitRename(event: SubmitEvent) {
    event.preventDefault();
    const name = renaming?.trim();
    renaming = null;
    if (name && currentAlbum && name !== currentAlbum.name) {
      await apply([{ op: "album_name", album: currentAlbum.id, name }]);
    }
  }

  async function deleteAlbum() {
    if (!currentAlbum) return;
    const album = currentAlbum.id;
    listing = { kind: "library" };
    await apply([{ op: "album_deleted", album, on: true }]);
  }

  // --- Ratings and labels -------------------------------------------------------

  /** Gives the selection `stars`; if they all have it already, takes it away. */
  async function rateSelection(stars: number) {
    if (!online) return;
    const target = pages;
    const photos = (await Promise.all([...selected].map((index) => target.load(index)))).flatMap((p) => (p ? [p] : []));
    if (target !== pages || photos.length === 0) return;
    const value = photos.every((photo) => photo.rating === stars) ? 0 : stars;
    await apply(photos.map((photo) => ({ op: "rating", photo: photo.fingerprint, stars: value })));
  }

  /** Gives the selection a label; if they all have it already, takes it away. */
  async function labelSelection(color: LabelColor | "") {
    if (!online) return;
    const target = pages;
    const photos = (await Promise.all([...selected].map((index) => target.load(index)))).flatMap((p) => (p ? [p] : []));
    if (target !== pages || photos.length === 0) return;
    const value = color && photos.every((photo) => photo.label === color) ? "" : color;
    await apply(photos.map((photo) => ({ op: "label", photo: photo.fingerprint, color: value })));
  }

  const ratingMenu = (): MenuEntry => ({
    label: "Rating",
    enabled: online,
    items: [
      { label: "No Rating", action: () => applyToSelection((photo) => ({ op: "rating", photo, stars: 0 })) },
      ...[1, 2, 3, 4, 5].map((stars) => ({
        label: `${"★".repeat(stars)}    ${stars}`,
        action: () => applyToSelection((photo) => ({ op: "rating", photo, stars })),
      })),
    ],
  });

  const labelMenu = (): MenuEntry => ({
    label: "Label",
    enabled: online,
    items: [
      { label: "No Label", action: () => applyToSelection((photo) => ({ op: "label", photo, color: "" })) },
      ...LABELS.map((label) => ({
        label: label.key ? `${label.name}    ${label.key}` : label.name,
        action: () => applyToSelection((photo) => ({ op: "label", photo, color: label.color })),
      })),
    ],
  });

  // --- Right-click menus --------------------------------------------------------

  /** A photo's menu acts on the selection; a photo outside it becomes the selection. */
  function photoMenu(index: number, event: MouseEvent) {
    if (!selected.has(index)) {
      selected.clear();
      selected.add(index);
    }
    const one = selected.size === 1;
    const fav = (on: boolean) => () => applyToSelection((photo) => ({ op: "fav", photo, on }));
    const entries: MenuEntry[] = [
      { label: "Open", enabled: one, action: () => (viewerIndex = index) },
      { label: one ? "Slideshow from here" : "Slideshow of these", action: () => {
          if (one) {
            const total = listedTotal;
            slideshow = { count: total - index, indexAt: (position) => index + position };
          } else startSlideshow();
        } },
      "-",
      { label: "Add to Favorites", enabled: online, action: fav(true) },
      { label: "Remove from Favorites", enabled: online, action: fav(false) },
      ratingMenu(),
      labelMenu(),
      {
        label: "Add to Album",
        enabled: online && albums.length > 0,
        items: albums.map((album) => ({
          label: album.name,
          action: () => applyToSelection((photo) => ({ op: "album_member", album: album.id, photo, on: true })),
        })),
      },
    ];
    if (listing.kind === "album") {
      const album = listing.album;
      entries.push({
        label: "Remove from Album",
        enabled: online,
        action: () => applyToSelection((photo) => ({ op: "album_member", album, photo, on: false })),
      });
    }
    entries.push({
      label: listing.kind === "hidden" ? "Unhide" : "Hide",
      enabled: online,
      action: () => applyToSelection((photo) => ({ op: "hide", photo, on: listing.kind !== "hidden" })),
    });
    if (listing.kind === "person") {
      entries.push(
        "-",
        { label: `Not ${personName ?? "this person"}`, enabled: online || personName === null, action: removeSelectionFromPerson },
        { label: "Someone else", enabled: online || personName === null, action: splitSelectionFromPerson },
      );
    }
    entries.push(
      "-",
      { label: REVEAL_LABEL, enabled: online && one, action: revealSelected },
      { label: "Export…", enabled: online, action: exportSelection },
    );
    showMenu(event, entries);
  }

  function albumMenu(event: MouseEvent, album: { id: string; name: string }) {
    const open = () => (listing = { kind: "album", album: album.id }, (section = "photos"));
    showMenu(event, [
      { label: "Open", action: open },
      "-",
      { label: "Rename…", enabled: online, action: () => (open(), (renaming = album.name)) },
      { label: "Delete Album…", enabled: online, action: () => (open(), (confirmingDelete = true)) },
    ]);
  }

  function folderMenu(event: MouseEvent, folder: string) {
    const separator = sep();
    const root = library.path.endsWith(separator) ? library.path : library.path + separator;
    showMenu(event, [
      { label: "Open", action: () => ((listing = folderListing(folder)), (section = "photos")) },
      "-",
      {
        label: REVEAL_LABEL,
        enabled: online,
        action: () => revealItemInDir(root + folder.split("/").join(separator)).catch((e) => (error = String(e))),
      },
    ]);
  }

  /** Puts the cursor in a text field as soon as it appears. */
  function focus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<svelte:window
  {onkeydown}
  onclick={() => {
    filterMenuOpen = false;
    sortMenuOpen = false;
    scanMenuOpen = false;
  }}
/>

<div class="library">
  <header>
    <button class="link" onclick={onclose}>‹ Libraries</button>
    <strong class="name" title={library.path}>{library.name}</strong>
    <div class="search">
      <input
        type="search"
        bind:value={searchText}
        oninput={onSearchInput}
        onkeydown={(event) => event.key === "Escape" && clearSearch()}
        placeholder="Search: place, camera, name, 2024, july, video…"
        aria-label="Search photos"
      />
    </div>
    <div class="status" role="status">
      {#if scanning && progress && progress.phase !== "scanning" && progress.total > 0}
        <progress value={progress.done} max={progress.total}></progress>
      {:else if scanning && progress?.phase === "scanning" && progress.known > 0}
        <progress value={Math.min(progress.discovered, progress.known)} max={progress.known}></progress>
      {/if}
      <span>{status}</span>
    </div>
    <span class="menu-anchor scan-buttons">
      <button
        class="secondary"
        onclick={() => rescan()}
        disabled={scanning}
        title="Look for photos added or changed since the last check"
      >
        Check for new
      </button>
      <button
        class="secondary more"
        aria-haspopup="menu"
        aria-expanded={scanMenuOpen}
        aria-label="More scan options"
        title="More scan options"
        onclick={(event) => {
          event.stopPropagation();
          filterMenuOpen = false;
          sortMenuOpen = false;
          scanMenuOpen = !scanMenuOpen;
        }}
      >
        <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      {#if scanMenuOpen}
        <div class="menu scan-menu" role="menu">
          <button role="menuitem" disabled={scanning} onclick={() => rescan()}>
            <span class="mark"></span>
            <span>Check for new photos<small>Reads only files added or changed</small></span>
          </button>
          <button role="menuitem" onclick={() => rescan(true)}>
            <span class="mark"></span>
            <span>Rescan everything<small>Reads every file again; slow on a large library</small></span>
          </button>
          {#if IS_MAC}
            <hr />
            <button
              role="menuitem"
              disabled={!online}
              onclick={() => {
                scanMenuOpen = false;
                chooseAppleLibrary();
              }}
            >
              <span class="mark"></span>
              <span>Import from Apple Photos…<small>Favorites, hidden photos and albums</small></span>
            </button>
          {/if}
        </div>
      {/if}
    </span>
    <SettingsButton onfaceschange={() => facesRevision++} />
    <AboutButton />
  </header>

  {#if !online}
    <p class="banner">
      This library can't be reached right now. You are browsing what this computer has cached;
      full-size photos and changes are unavailable until it is back.
    </p>
  {:else if !writable}
    <p class="banner">
      This folder is read-only. Favorites, albums and hidden photos are kept on this computer
      only.
    </p>
  {/if}
  {#if imported}
    <p class="banner">
      Imported from Apple Photos: {formatNumber(imported.favorites)} favorites,
      {formatNumber(imported.hidden + imported.deleted)} hidden,
      {formatNumber(imported.albums.filter((album) => album.matched > 0).length)} album(s).
      <button class="dismiss" onclick={() => (imported = null)} aria-label="Dismiss">×</button>
    </p>
  {/if}
  {#if error}
    <p class="banner error" role="alert">
      {error}
      <button class="dismiss" onclick={() => (error = null)} aria-label="Dismiss">×</button>
    </p>
  {/if}
  {#if lastScan && lastScan.unreadableDirs > 0}
    <p class="banner">
      {formatNumber(lastScan.unreadableDirs)}
      {lastScan.unreadableDirs === 1 ? "folder" : "folders"} could not be read, so photos that
      may have been deleted were kept in the index.
    </p>
  {/if}
  {#if !scanning && unreadable > 0}
    <p class="banner">
      {formatNumber(unreadable)}
      {unreadable === 1 ? "file" : "files"} could not be read. They will be tried again on the
      next scan.
    </p>
  {/if}

  <div class="body">
    <nav aria-label="Library sections" style:width="{sidebarWidth}px">
      {#snippet row(target: Listing, label: string, count: number | null, depth = 0, enabled = true)}
        <button
          class="row"
          class:selected={section === "photos" && JSON.stringify(target) === JSON.stringify(listing)}
          style:padding-left="{12 + depth * 14}px"
          disabled={!enabled}
          onclick={() => {
            listing = target;
            section = "photos";
          }}
          title={label}
        >
          <span class="row-name">{label}</span>
          {#if count !== null && count > 0}<span class="row-count">{formatNumber(count)}</span>{/if}
        </button>
      {/snippet}

      {@render row({ kind: "library" }, "Library", null)}
      {@render row({ kind: "favorites" }, "Favorites", null)}
      {@render row({ kind: "hidden" }, "Hidden", null)}
      <button
        class="row"
        class:selected={section === "map" || (section === "photos" && listing.kind === "area")}
        onclick={() => (section = "map")}
      >
        <span class="row-name">Map</span>
      </button>
      <button
        class="row"
        class:selected={section === "people" || (section === "photos" && listing.kind === "person")}
        onclick={() => (section = "people")}
      >
        <span class="row-name">People</span>
      </button>
      <button class="row" class:selected={section === "duplicates"} onclick={() => (section = "duplicates")}>
        <span class="row-name">Duplicates</span>
      </button>

      <h2>
        Albums
        <button
          class="add"
          onclick={() => (newAlbumName = "")}
          disabled={!online}
          aria-label="New album"
          title="New album"
        >
          +
        </button>
      </h2>
      {#if newAlbumName !== null}
        <form onsubmit={submitNewAlbum}>
          <input
            bind:value={newAlbumName}
            use:focus
            placeholder="Album name"
            maxlength="200"
            onblur={() => (newAlbumName = null)}
            onkeydown={(event) => event.key === "Escape" && (newAlbumName = null)}
          />
        </form>
      {/if}
      {#each albums as album (album.id)}
        <div class="menu-target" oncontextmenu={(event) => albumMenu(event, album)} role="presentation">
          {@render row({ kind: "album", album: album.id }, album.name, album.count)}
        </div>
      {:else}
        {#if newAlbumName === null}<p class="hint">No albums yet</p>{/if}
      {/each}

      <h2>Folders</h2>
      {#each folderTree.rows as folder (folder.path)}
        <div class="tree-row" oncontextmenu={(event) => folderMenu(event, folder.path)} role="presentation">
          {#if folder.hasChildren}
            <button
              class="twisty"
              class:open={openFolders.has(folder.path)}
              style:left="{folder.depth * 14}px"
              aria-expanded={openFolders.has(folder.path)}
              aria-label="{openFolders.has(folder.path) ? 'Hide' : 'Show'} the folders in {folder.name}"
              onclick={() => toggleFolder(folder.path)}
            >
              <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
                <path d="M3.5 1.5 7 5l-3.5 3.5" />
              </svg>
            </button>
          {/if}
          {@render row(folderListing(folder.path), folder.name, folder.count, folder.depth + 1)}
        </div>
      {/each}
    </nav>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the sidebar"
      aria-valuenow={sidebarWidth}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      tabindex="0"
      onpointerdown={startResize}
      onpointermove={resize}
      onpointerup={() => (resizing = null)}
      onpointercancel={() => (resizing = null)}
      onkeydown={resizeWithKeys}
      ondblclick={() => (sidebarWidth = 220)}
    ></div>

    <main>
      {#if section === "map"}
        <div class="map">
          <MapView
            libraryPath={library.path}
            {revision}
            onopen={(area) => {
              listing = area;
              section = "photos";
            }}
          />
        </div>
      {:else if section === "people"}
        <div class="map">
          {#key facesRevision}
          <PeopleView
            libraryPath={library.path}
            {revision}
            canEdit={online}
            searching={scanning && progress?.phase === "faces"}
            onopen={(person) => {
              personName = person.name;
              personHidden = person.hidden;
              listing = { kind: "person", clusters: person.clusters };
              section = "photos";
            }}
          />
          {/key}
        </div>
      {:else if section === "duplicates"}
        <div class="map">
          <DuplicatesView
            libraryPath={library.path}
            {revision}
            canEdit={online && writable}
            onchanged={refreshAll}
          />
        </div>
      {:else}
      <div class="toolbar">
        {#if selected.size > 0}
          <strong>{formatNumber(selected.size)} selected</strong>
          <span class="spacer"></span>
          <button
            class="secondary"
            disabled={!online}
            onclick={() => applyToSelection((photo) => ({ op: "fav", photo, on: true }))}
          >
            ♥ Favorite
          </button>
          {#if listing.kind === "favorites"}
            <button
              class="secondary"
              disabled={!online}
              onclick={() => applyToSelection((photo) => ({ op: "fav", photo, on: false }))}
            >
              Remove from favorites
            </button>
          {/if}
          {#if albums.length > 0}
            <select onchange={addSelectionToAlbum} disabled={!online}>
              <option value="">Add to album…</option>
              {#each albums as album (album.id)}
                <option value={album.id}>{album.name}</option>
              {/each}
            </select>
          {/if}
          {#if currentAlbum}
            {@const album = currentAlbum.id}
            <button
              class="secondary"
              disabled={!online}
              onclick={() =>
                applyToSelection((photo) => ({ op: "album_member", album, photo, on: false }))}
            >
              Remove from album
            </button>
          {/if}
          {#if listing.kind === "person"}
            <button
              class="secondary"
              disabled={!online && personName !== null}
              onclick={removeSelectionFromPerson}
              title="Their face in these photos is someone else's: take the photos out"
            >
              Not {personName ?? "this person"}
            </button>
            <button
              class="secondary"
              disabled={!online && personName !== null}
              onclick={splitSelectionFromPerson}
              title="These photos show one other person: make them a person of their own"
            >
              Someone else
            </button>
          {/if}
          <button
            class="secondary"
            disabled={!online}
            onclick={() =>
              applyToSelection((photo) => ({ op: "hide", photo, on: listing.kind !== "hidden" }))}
          >
            {listing.kind === "hidden" ? "Unhide" : "Hide"}
          </button>
          {#if selected.size === 1}
            <button class="secondary" disabled={!online} onclick={revealSelected}>{REVEAL_LABEL}</button>
          {/if}
          <button
            class="secondary"
            disabled={!online}
            onclick={exportSelection}
            title="Copy the selected photos into a folder, optionally smaller"
          >
            Export…
          </button>
          <button class="secondary" onclick={() => selected.clear()}>Clear</button>
        {:else if renaming !== null}
          <form onsubmit={submitRename}>
            <input
              bind:value={renaming}
              use:focus
              maxlength="200"
              onblur={() => (renaming = null)}
              onkeydown={(event) => event.key === "Escape" && (renaming = null)}
            />
          </form>
        {:else}
          {#if listing.kind === "person"}
            <button class="secondary" onclick={() => (section = "people")}>‹ People</button>
          {/if}
          {#if listing.kind === "area"}
            <button class="secondary" onclick={() => (section = "map")}>‹ Map</button>
          {/if}
          <span class="heading">
            <span class="heading-line">
              <strong class="title">{title}</strong>
              {#if groupsLoaded}<span class="dim">{photoCount(listedTotal)}</span>{/if}
            </span>
            <!-- Always there, with a fixed width, so that scrolling never
                 moves the rest of the toolbar. -->
            <span class="range" class:unused={viewMode !== "all" || !shownRange}>
              {shownRange ? formatDayRange(shownRange.oldest, shownRange.newest) : ""}
            </span>
          </span>
          {#if listing.kind === "person"}
            <button
              class="secondary"
              onclick={togglePersonHidden}
              title={personHidden
                ? "List this person on the People screen again"
                : "Leave this person out of the People screen (e.g. a stranger)"}
            >
              {personHidden ? "Show in People" : "Hide from People"}
            </button>
          {/if}
          <span class="spacer"></span>
          <div class="segmented" role="group" aria-label="View">
            {#each [["years", "Years"], ["months", "Months"], ["all", "All Photos"]] as const as [mode, label] (mode)}
              <button aria-pressed={viewMode === mode} onclick={() => setViewMode(mode)}>
                {label}
              </button>
            {/each}
          </div>
          <span class="spacer"></span>
          <!-- Kept in place (only hidden) in the overviews, so that the
               view switch beside it does not move under the pointer. -->
          <input
            class="zoom"
            class:unused={viewMode !== "all"}
            disabled={viewMode !== "all"}
            type="range"
            min={CELL_MIN}
            max={CELL_MAX}
            step="10"
            bind:value={cellSize}
            aria-label="Thumbnail size"
            title="Thumbnail size (Ctrl/Cmd + and -)"
          />
          <button
            class="secondary icon"
            onclick={startSlideshow}
            disabled={listedTotal === 0}
            title={selected.size > 0 ? "Slideshow of the selected photos (F5)" : "Slideshow (F5)"}
            aria-label="Start a slideshow"
          >
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <rect x="1.5" y="3" width="13" height="10" rx="2" />
              <path d="M6.5 5.8v4.4L10 8z" />
            </svg>
          </button>
          <span class="menu-anchor">
            <button
              class="secondary icon"
              class:unused={viewMode !== "all"}
              disabled={viewMode !== "all"}
              aria-haspopup="menu"
              aria-expanded={sortMenuOpen}
              onclick={(event) => {
                event.stopPropagation();
                filterMenuOpen = false;
                scanMenuOpen = false;
                sortMenuOpen = !sortMenuOpen;
              }}
              title="Sort"
              aria-label="Sort"
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <path d="M5 13V3M2.5 5.5 5 3l2.5 2.5M11 3v10M8.5 10.5 11 13l2.5-2.5" />
              </svg>
            </button>
            {#if sortMenuOpen}
              <div class="menu" role="menu">
                <button role="menuitemradio" aria-checked={!byName} onclick={() => chooseSort("date", null)}>
                  <span class="mark">{!byName ? "✓" : ""}</span>Date taken
                </button>
                <button role="menuitemradio" aria-checked={byName} onclick={() => chooseSort("name", null)}>
                  <span class="mark">{byName ? "✓" : ""}</span>Name
                </button>
                <hr />
                <button role="menuitemradio" aria-checked={ascending} onclick={() => chooseSort(null, true)}>
                  <span class="mark">{ascending ? "✓" : ""}</span>{byName ? "A to Z" : "Oldest first"}
                </button>
                <button role="menuitemradio" aria-checked={!ascending} onclick={() => chooseSort(null, false)}>
                  <span class="mark">{!ascending ? "✓" : ""}</span>{byName ? "Z to A" : "Newest first"}
                </button>
              </div>
            {/if}
          </span>
          <span class="menu-anchor">
            <button
              class="secondary icon"
              class:active={only !== null}
              aria-haspopup="menu"
              aria-expanded={filterMenuOpen}
              onclick={(event) => {
                event.stopPropagation();
                sortMenuOpen = false;
                scanMenuOpen = false;
                filterMenuOpen = !filterMenuOpen;
              }}
              title={only ? `Showing: ${FILTERS.find(([value]) => value === only)?.[1]}` : "Filter"}
              aria-label="Filter"
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <path d="M2 4h12M4 8h8M6 12h4" />
              </svg>
            </button>
            {#if filterMenuOpen}
              <div class="menu" role="menu">
                {#each FILTERS as [value, label] (label)}
                  <button
                    role="menuitemradio"
                    aria-checked={only === value}
                    onclick={() => chooseFilter(value)}
                  >
                    <span class="mark">{only === value ? "✓" : ""}</span>{label}
                  </button>
                {/each}
                <hr />
                {#each [1, 2, 3, 4, 5] as stars (stars)}
                  {@const value = `stars${stars}` as Only}
                  <button role="menuitemradio" aria-checked={only === value} onclick={() => chooseFilter(value)}>
                    <span class="mark">{only === value ? "✓" : ""}</span>{"★".repeat(stars)}{stars < 5 ? " or more" : ""}
                  </button>
                {/each}
                <hr />
                {#each LABELS as label (label.color)}
                  <button role="menuitemradio" aria-checked={only === label.color} onclick={() => chooseFilter(label.color)}>
                    <span class="mark">{only === label.color ? "✓" : ""}</span><span
                      class="swatch"
                      style:background={label.css}
                    ></span>{label.name}
                  </button>
                {/each}
              </div>
            {/if}
          </span>
          <button
            class="secondary icon"
            class:unused={viewMode !== "all"}
            disabled={viewMode !== "all"}
            aria-pressed={gridFit === "river"}
            onclick={() => (gridFit = gridFit === "square" ? "river" : "square")}
            title={gridFit === "square" ? "Show photos in rows at their own shape" : "Show square thumbnails"}
            aria-label="Square thumbnails or rows of whole photos"
          >
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              {#if gridFit === "square"}
                <rect x="1.5" y="1.5" width="5.5" height="5.5" rx="1" />
                <rect x="9" y="1.5" width="5.5" height="5.5" rx="1" />
                <rect x="1.5" y="9" width="5.5" height="5.5" rx="1" />
                <rect x="9" y="9" width="5.5" height="5.5" rx="1" />
              {:else}
                <rect x="1.5" y="1.5" width="8" height="5.5" rx="1" />
                <rect x="11" y="1.5" width="3.5" height="5.5" rx="1" />
                <rect x="1.5" y="9" width="4" height="5.5" rx="1" />
                <rect x="7" y="9" width="7.5" height="5.5" rx="1" />
              {/if}
            </svg>
          </button>
          {#if currentAlbum}
            {#if confirmingDelete}
              <span class="dim">Delete this album? The photos stay in the library.</span>
              <button class="secondary danger" onclick={deleteAlbum}>Delete</button>
              <button class="secondary" onclick={() => (confirmingDelete = false)}>Cancel</button>
            {:else}
              <button
                class="secondary"
                disabled={!online}
                onclick={() => (renaming = currentAlbum?.name ?? "")}
              >
                Rename
              </button>
              <button class="secondary" disabled={!online} onclick={() => (confirmingDelete = true)}>
                Delete album
              </button>
            {/if}
          {/if}
        {/if}
      </div>

      {#if listing.kind === "person" && personName}
        <PersonSuggestions
          libraryPath={library.path}
          name={personName}
          clusters={listing.clusters}
          canEdit={online}
          onadded={(clusters) => (listing = { kind: "person", clusters })}
        />
      {/if}
      <div class="grid">
        {#if groups.length > 0 && viewMode !== "all"}
          <Overview
            mode={viewMode}
            months={ascending && !byName ? [...months].reverse() : months}
            {startYear}
            onyear={(year) => {
              startYear = year;
              viewMode = "months";
            }}
            onday={(day, month) => {
              // Oldest first, a month opens at its first day.
              startDay =
                month && order === "dateAsc" ? day - new Date(day * 86_400_000).getUTCDate() + 1 : day;
              viewMode = "all";
            }}
            onslideshow={slideshowOfMonth}
          />
        {:else if groups.length > 0}
          <PhotoGrid
            bind:this={grid}
            {groups}
            {pages}
            {selected}
            {cellSize}
            fit={gridFit}
            shapes={gridShapes}
            onrange={setShownRange}
            startAt={startDay}
            onopen={(index) => (viewerIndex = index)}
            oncontext={photoMenu}
            ontoggle={toggleSelected}
            onmonthslideshow={slideshowOfMonth}
          />
        {:else if groupsLoaded}
          <p class="empty">{emptyText}</p>
        {/if}
      </div>
      {/if}
    </main>
  </div>
</div>

{#if exporting}
  <ExportDialog libraryPath={library.path} photos={exporting} onclose={() => (exporting = null)} />
{/if}

{#if slideshow}
  <Slideshow
    {pages}
    count={slideshow.count}
    indexAt={slideshow.indexAt}
    onclose={() => (slideshow = null)}
  />
{/if}

{#if importing}
  {@const summary = importing.summary}
  <div class="backdrop">
    <div class="dialog" role="dialog" aria-modal="true" aria-label="Import from Apple Photos">
      <h2>Import from Apple Photos</h2>
      <p>
        {formatNumber(summary.matched)} of the {formatNumber(summary.assets)} photos in that
        library were found in this one. Importing will:
      </p>
      <ul>
        <li>mark {formatNumber(summary.favorites)} as favorites;</li>
        <li>hide {formatNumber(summary.hidden)};</li>
        {#if summary.deleted > 0}
          <li>
            hide {formatNumber(summary.deleted)} more that
            {summary.deleted === 1 ? "is" : "are"} in “Recently Deleted” in Photos;
          </li>
        {/if}
        <li>
          create or fill {formatNumber(summary.albums.filter((album) => album.matched > 0).length)}
          {summary.albums.filter((album) => album.matched > 0).length === 1 ? "album" : "albums"}:
        </li>
      </ul>
      {#if summary.albums.length > 0}
        <div class="albums">
          {#each summary.albums as album (album.name)}
            <div class:dim={album.matched === 0}>
              <span>{album.name}</span>
              <span>{formatNumber(album.matched)} of {formatNumber(album.photos)}</span>
            </div>
          {/each}
        </div>
      {/if}
      <p class="dim small">
        Nothing is removed, and the Apple Photos library is only read. Photos that are not in
        this folder are skipped.
      </p>
      <div class="actions">
        <button class="secondary" onclick={() => (importing = null)} disabled={importing.busy}>
          Cancel
        </button>
        <button
          class="secondary default"
          onclick={confirmImport}
          disabled={importing.busy || summary.matched === 0}
        >
          {importing.busy ? "Importing…" : "Import"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if viewerIndex !== null}
  <Viewer
    {pages}
    total={listedTotal}
    bind:index={viewerIndex}
    {revision}
    canEdit={online}
    {albums}
    currentAlbum={currentAlbum?.id ?? null}
    showingHidden={listing.kind === "hidden"}
    {apply}
    onclose={closeViewer}
  />
{/if}

<style>
  .library {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .link {
    border: none;
    background: none;
    padding: 4px 0;
    color: var(--accent);
    cursor: pointer;
    white-space: nowrap;
  }

  .name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    /* A fixed width with digits of equal width: the counter changes many
       times a second during a scan and must not push the search box around. */
    justify-content: flex-end;
    width: 260px;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  progress {
    width: 90px;
    accent-color: var(--accent);
  }

  .secondary,
  select {
    padding: 5px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: inherit;
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
  }

  /* Drawn like the buttons beside it: the system's own dropdown is shorter
     and rounder. The arrow is drawn here instead. */
  select {
    appearance: none;
    -webkit-appearance: none;
    padding-right: 28px;
    line-height: inherit;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 12'%3E%3Cpath d='M2 4.5 5 1.5 8 4.5M2 7.5 5 10.5 8 7.5' fill='none' stroke='%23888' stroke-width='1.4' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    background-size: 9px 11px;
  }

  .secondary:disabled,
  select:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .danger {
    border-color: var(--danger);
    color: var(--danger);
  }

  .banner {
    margin: 0;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--warn-bg);
    color: var(--warn);
  }

  .banner.error {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .dismiss {
    border: none;
    background: none;
    font-size: 16px;
    cursor: pointer;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  nav {
    flex-shrink: 0;
    padding: 8px 0 16px;
    overflow-y: auto;
    background: var(--bg);
  }

  .search {
    flex: 1;
    display: flex;
    justify-content: center;
    min-width: 0;
  }

  .search input {
    width: 100%;
    max-width: 420px;
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }

  /* The line between the sidebar and the photos; drag it to resize. */
  .resizer {
    flex-shrink: 0;
    width: 5px;
    margin-right: -4px;
    border-left: 1px solid var(--border);
    cursor: col-resize;
    touch-action: none;
    z-index: 1;
  }

  .resizer:hover,
  .resizer:focus-visible {
    border-left: 3px solid var(--accent);
    outline: none;
  }

  .segmented {
    display: flex;
    flex-shrink: 0;
    white-space: nowrap;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }

  .segmented button {
    padding: 4px 12px;
    border: none;
    background: var(--surface);
    font-size: 13px;
    cursor: pointer;
  }

  .segmented button + button {
    border-left: 1px solid var(--border);
  }

  .segmented button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--accent-text);
  }

  .toolbar input.zoom {
    width: 120px;
  }

  .toolbar input.zoom.unused,
  .toolbar button.unused {
    visibility: hidden;
  }

  .toolbar button.icon {
    display: inline-flex;
    align-items: center;
    padding: 4px 6px;
  }

  .toolbar button.icon.active {
    background: var(--accent);
    color: var(--accent-text);
  }

  .menu-anchor {
    position: relative;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    min-width: 190px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.22);
  }

  .menu button {
    display: flex;
    align-items: center;
    padding: 6px 10px 6px 4px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .menu button:hover,
  .menu button:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
  }

  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    margin-right: 8px;
    border-radius: 50%;
    vertical-align: -1px;
  }

  .menu hr {
    width: 100%;
    margin: 4px 0;
    border: none;
    border-top: 1px solid var(--border);
  }

  .menu .mark {
    width: 20px;
    text-align: center;
  }

  .scan-buttons {
    display: inline-flex;
  }

  .scan-buttons > button:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }

  .scan-buttons > .more {
    margin-left: -1px;
    padding: 0 7px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }

  .scan-buttons > .more svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .scan-menu {
    min-width: 250px;
  }

  .scan-menu small {
    display: block;
    margin-top: 1px;
    opacity: 0.7;
    font-size: 11px;
  }

  .menu button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .menu button:disabled:hover {
    background: none;
    color: var(--text);
  }

  .toolbar button.icon svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
  }

  nav h2 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 18px 12px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }

  .add {
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: none;
    font-size: 16px;
    line-height: 1;
    color: var(--text-dim);
    cursor: pointer;
  }

  .add:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.4);
    z-index: 20;
  }

  .dialog {
    width: min(460px, calc(100vw - 40px));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    padding: 20px 24px;
    border-radius: 12px;
    background: var(--surface);
    box-shadow: 0 10px 40px rgb(0 0 0 / 0.35);
    font-size: 14px;
    line-height: 1.45;
  }

  .dialog h2 {
    margin: 0 0 8px;
    font-size: 17px;
  }

  .dialog ul {
    margin: 8px 0;
    padding-left: 20px;
  }

  .dialog .albums {
    max-height: 180px;
    overflow-y: auto;
    margin: 0 0 8px 20px;
    font-size: 13px;
  }

  .dialog .albums div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 2px 0;
  }

  .dialog .small {
    font-size: 12px;
    white-space: normal;
  }

  .dialog .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }

  .dialog .default {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-text);
  }

  .hint {
    margin: 2px 12px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 5px 12px;
    border: none;
    background: none;
    text-align: left;
    cursor: pointer;
  }

  .row:disabled {
    color: var(--text-dim);
    cursor: default;
  }

  .row:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .row.selected,
  .row.selected:hover {
    background: var(--accent);
    color: var(--accent-text);
  }

  .row-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-count {
    font-size: 12px;
    opacity: 0.7;
  }

  form {
    display: contents;
  }

  input {
    padding: 4px 8px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--bg);
    color: inherit;
    font: inherit;
    user-select: text;
    -webkit-user-select: text;
  }

  nav input {
    width: calc(100% - 24px);
    margin: 2px 12px;
    background: var(--surface);
  }

  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 46px;
    padding: 6px 16px;
    border-bottom: 1px solid var(--border);
  }

  .toolbar input {
    width: 280px;
  }

  .tree-row {
    position: relative;
  }

  .twisty {
    position: absolute;
    top: 50%;
    z-index: 1;
    width: 22px;
    height: 22px;
    margin-left: 4px;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-dim);
    line-height: 0;
    text-align: center;
    cursor: pointer;
    transform: translateY(-50%);
    transition: transform 0.12s;
  }

  .twisty svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .twisty.open {
    transform: translateY(-50%) rotate(90deg);
  }

  .heading {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .heading-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .range {
    width: 17em;
    max-width: 100%;
    min-height: 1.2em;
    overflow: hidden;
    color: var(--text-dim);
    font-size: 11px;
    line-height: 1.2;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .range.unused {
    visibility: hidden;
  }

  /* No room for the dates on a narrow window. */
  @media (max-width: 900px) {
    .range {
      display: none;
    }
  }

  .title {
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dim {
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .grid {
    flex: 1;
    min-height: 0;
  }

  .map {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .empty {
    max-width: 380px;
    margin: 0 auto;
    padding-top: 100px;
    text-align: center;
    color: var(--text-dim);
  }
</style>
