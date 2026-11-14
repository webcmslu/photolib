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
    rescanLibrary,
    syncLibrary,
    timeline,
    type Album,
    type DayGroup,
    type FolderEntry,
    type ImportSummary,
    type IndexEvent,
    type LibraryInfo,
    type Listing,
    type MonthSummary,
    type Op,
    type ScanProgress,
    type ScanSummary,
    type ThumbSummary,
  } from "./api";
  import { formatNumber, photoCount } from "./format";
  import MapView from "./MapView.svelte";
  import Overview from "./Overview.svelte";
  import PeopleView from "./PeopleView.svelte";
  import { PhotoPages } from "./pages.svelte";
  import PhotoGrid from "./PhotoGrid.svelte";
  import Viewer from "./Viewer.svelte";

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
        const now = Date.now();
        if (event.progress.phase === "indexing" && now - lastLiveRefresh > LIVE_REFRESH_MS) {
          lastLiveRefresh = now;
          refreshAll();
        }
        break;
      }
      case "online":
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

  async function rescan() {
    error = null;
    scanning = true;
    lastScanStart = Date.now();
    try {
      await rescanLibrary(library.path);
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
      if (progress?.phase === "thumbnails") {
        return `Making thumbnails: ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
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
  let section = $state<"photos" | "map" | "people">("photos");
  /** The name of the person whose photos are listed. */
  let personName = $state<string | null>(null);

  async function loadSidebar() {
    try {
      [albums, folders] = await Promise.all([listAlbums(library.path), listFolders(library.path)]);
      // The album or folder being shown may be gone (deleted on another computer).
      const gone =
        (listing.kind === "album" && !albums.some((a) => a.id === (listing as { album: string }).album)) ||
        (listing.kind === "folder" &&
          !folders.some((f) => f.folder === (listing as { folder: string }).folder));
      if (gone) listing = { kind: "library" };
    } catch (e) {
      error = String(e);
    }
  }

  /** Folder rows for the sidebar: every folder with photos, plus its ancestors. */
  const folderRows = $derived.by(() => {
    const counts = new Map(folders.map((f) => [f.folder, f.count]));
    const paths = new Set<string>();
    for (const { folder } of folders) {
      const parts = folder === "" ? [] : folder.split("/");
      for (let depth = 1; depth <= parts.length; depth++) {
        paths.add(parts.slice(0, depth).join("/"));
      }
    }
    const rows = [...paths]
      .sort((a, b) => {
        const partsA = a.split("/");
        const partsB = b.split("/");
        for (let i = 0; i < Math.min(partsA.length, partsB.length); i++) {
          const order = partsA[i].localeCompare(partsB[i], undefined, { numeric: true });
          if (order !== 0) return order;
        }
        return partsA.length - partsB.length;
      })
      .map((path) => {
        const parts = path.split("/");
        return {
          path,
          name: parts[parts.length - 1],
          depth: parts.length - 1,
          count: counts.get(path) ?? 0,
        };
      });
    if (counts.has("")) {
      rows.unshift({ path: "", name: "Top level", depth: 0, count: counts.get("") ?? 0 });
    }
    return rows;
  });

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
    switch (listing.kind) {
      case "favorites":
        return "No favorites yet. Open a photo and press the heart, or select photos in the grid.";
      case "hidden":
        return "No hidden photos.";
      case "album":
        return "This album is empty. Select photos in the grid and choose “Add to album”.";
      case "search":
        return "Nothing matches. Try a place, a camera, a folder or file name, a year (2024), a month (july) or “video”.";
      default:
        return scanning ? "Looking for photos…" : "No photos found in this folder.";
    }
  });

  // --- The listing on screen ---------------------------------------------------

  const pages = $derived(new PhotoPages(library.path, listing));
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
  /** Where the next view starts: a year for the months, a month for the grid. */
  let startYear = $state<number | null>(null);
  let startMonth = $state<number | null>(null);

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
    startMonth = null;
    viewMode = mode;
  }

  /** Ctrl/Cmd with + or - zooms the grid, like in a browser. */
  function onkeydown(event: KeyboardEvent) {
    if (!(event.ctrlKey || event.metaKey) || viewerIndex !== null) return;
    const factor = event.key === "+" || event.key === "=" ? 1.25 : event.key === "-" ? 0.8 : 0;
    if (factor === 0) return;
    event.preventDefault();
    cellSize = Math.round(Math.min(CELL_MAX, Math.max(CELL_MIN, cellSize * factor)));
  }

  let grid = $state<PhotoGrid>();
  let viewerIndex = $state<number | null>(null);
  const selected = new SvelteSet<number>();
  let selectionAnchor: number | null = null;

  // Start afresh whenever something else is listed.
  $effect(() => {
    const target = pages;
    // Only a change of `pages` may re-run this, not the state it resets.
    untrack(() => {
      groups = [];
      months = [];
      startYear = null;
      startMonth = null;
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
      const loaded = await timeline(target.libraryPath, target.listing);
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

  /** Puts the cursor in a text field as soon as it appears. */
  function focus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<svelte:window {onkeydown} />

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
      {/if}
      <span>{status}</span>
    </div>
    <button class="secondary" onclick={rescan} disabled={scanning}>Rescan</button>
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
      {formatNumber(imported.hidden)} hidden,
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
        {@render row({ kind: "album", album: album.id }, album.name, album.count)}
      {:else}
        {#if newAlbumName === null}<p class="hint">No albums yet</p>{/if}
      {/each}

      <button class="import" onclick={chooseAppleLibrary} disabled={!online}>
        Import from Apple Photos…
      </button>

      <h2>Folders</h2>
      {#each folderRows as folder (folder.path)}
        {@render row(
          { kind: "folder", folder: folder.path },
          folder.name,
          folder.count,
          folder.depth,
          folder.count > 0,
        )}
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
          <PeopleView
            libraryPath={library.path}
            {revision}
            canEdit={online}
            searching={scanning && progress?.phase === "faces"}
            onopen={(person) => {
              personName = person.name;
              listing = { kind: "person", clusters: person.clusters };
              section = "photos";
            }}
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
          <button
            class="secondary"
            disabled={!online}
            onclick={() =>
              applyToSelection((photo) => ({ op: "hide", photo, on: listing.kind !== "hidden" }))}
          >
            {listing.kind === "hidden" ? "Unhide" : "Hide"}
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
          <strong class="title">{title}</strong>
          {#if groupsLoaded}<span class="dim">{photoCount(listedTotal)}</span>{/if}
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

      <div class="grid">
        {#if groups.length > 0 && viewMode !== "all"}
          <Overview
            mode={viewMode}
            {months}
            {startYear}
            onyear={(year) => {
              startYear = year;
              viewMode = "months";
            }}
            onmonth={(month) => {
              startMonth = month;
              viewMode = "all";
            }}
          />
        {:else if groups.length > 0}
          <PhotoGrid
            bind:this={grid}
            {groups}
            {pages}
            {selected}
            {cellSize}
            startAt={startMonth}
            onopen={(index) => (viewerIndex = index)}
            ontoggle={toggleSelected}
          />
        {:else if groupsLoaded}
          <p class="empty">{emptyText}</p>
        {/if}
      </div>
      {/if}
    </main>
  </div>
</div>

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
    background: var(--surface);
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

  .toolbar input.zoom.unused {
    visibility: hidden;
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

  .import {
    margin: 6px 12px 0;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
  }

  .import:disabled {
    opacity: 0.5;
    cursor: default;
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
  }

  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
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
