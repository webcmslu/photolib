<!--
  An open library: the timeline ("Library") and the folder view, the scan
  status, and the viewer on top.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import {
    closeLibrary,
    libraryStats,
    listFolders,
    onIndexEvent,
    rescanLibrary,
    timeline,
    type DayGroup,
    type FolderEntry,
    type IndexEvent,
    type LibraryInfo,
    type ScanProgress,
    type ScanSummary,
    type ThumbSummary,
  } from "./api";
  import { formatNumber, photoCount } from "./format";
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
  /** How often the grid picks up newly indexed photos during a long first scan. */
  const LIVE_REFRESH_MS = 3000;

  // --- Scan status ----------------------------------------------------------

  let photoTotal = $state<number | null>(null);
  let scanning = $state(true); // opening a library starts a scan
  let progress = $state<ScanProgress | null>(null);
  let lastScan = $state<ScanSummary | null>(null);
  let lastThumbs = $state<ThumbSummary | null>(null);
  let scanError = $state<string | null>(null);
  let lastLiveRefresh = 0;

  onMount(() => {
    const unlisten = onIndexEvent(handleIndexEvent);
    const timer = setInterval(() => {
      if (!scanning) rescan();
    }, RESCAN_INTERVAL_MS);

    // What the index already holds from an earlier session shows at once.
    libraryStats(library.path)
      .then((stats) => {
        photoTotal ??= stats.photoCount;
        // The scan may have finished before we were listening.
        if (!stats.scanning && !progress) scanning = false;
      })
      .catch((e) => (scanError = String(e)));

    return () => {
      clearInterval(timer);
      unlisten.then((stop) => stop());
      closeLibrary(library.path).catch(() => {});
    };
  });

  function handleIndexEvent(event: IndexEvent) {
    if (event.libraryPath !== library.path) return;
    if (event.state === "running") {
      scanning = true;
      progress = event.progress;
      const now = Date.now();
      if (event.progress.phase === "indexing" && now - lastLiveRefresh > LIVE_REFRESH_MS) {
        lastLiveRefresh = now;
        refreshListing();
      }
    } else if (event.state === "indexed") {
      // The index is complete; thumbnails are still being made.
      lastScan = event.summary;
      photoTotal = event.summary.total;
      refreshListing();
    } else if (event.state === "finished") {
      scanning = false;
      progress = null;
      lastScan = event.summary;
      lastThumbs = event.thumbnails;
      photoTotal = event.summary.total;
      // Thumbnail making may have found files that are not images after all.
      if (event.thumbnails.undecodable > 0) refreshListing();
    } else {
      scanning = false;
      progress = null;
      scanError = event.message;
    }
  }

  async function rescan() {
    scanError = null;
    scanning = true;
    try {
      await rescanLibrary(library.path);
    } catch (e) {
      scanning = false;
      scanError = String(e);
    }
  }

  const status = $derived.by(() => {
    if (scanning) {
      if (progress?.phase === "indexing" && progress.total > 0) {
        return `Indexing ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      if (progress?.phase === "thumbnails") {
        return `Making thumbnails: ${formatNumber(progress.done)} of ${formatNumber(progress.total)}`;
      }
      return progress
        ? `Scanning… ${formatNumber(progress.discovered)} found`
        : "Scanning…";
    }
    return photoTotal === null ? "" : photoCount(photoTotal);
  });

  const unreadable = $derived((lastScan?.failed ?? 0) + (lastThumbs?.unreadable ?? 0));

  // --- What is listed -------------------------------------------------------

  let view = $state<"library" | "folders">("library");
  let folders = $state<FolderEntry[]>([]);
  let selectedFolder = $state<string | null>(null);

  /** The folder being listed, or null for the whole library. */
  const listedFolder = $derived(view === "folders" ? (selectedFolder ?? "") : null);
  const pages = $derived(new PhotoPages(library.path, listedFolder));
  let groups = $state<DayGroup[]>([]);
  /** False until the first timeline for the current listing has arrived. */
  let groupsLoaded = $state(false);
  const listedTotal = $derived(groups.reduce((sum, group) => sum + group.count, 0));

  let grid = $state<PhotoGrid>();
  let viewerIndex = $state<number | null>(null);

  // Load the day groups whenever the listing changes.
  $effect(() => {
    const target = pages;
    groups = [];
    groupsLoaded = false;
    viewerIndex = null;
    loadGroups(target);
  });

  // Keep the folder list current while the folder view is showing.
  $effect(() => {
    if (view === "folders") loadFolders();
  });

  async function loadGroups(target: PhotoPages) {
    try {
      const loaded = await timeline(target.libraryPath, target.folder);
      if (target !== pages) return; // the listing changed meanwhile
      groups = loaded;
      groupsLoaded = true;
    } catch (e) {
      scanError = String(e);
    }
  }

  async function loadFolders() {
    try {
      folders = await listFolders(library.path);
      if (selectedFolder === null || !folders.some((f) => f.folder === selectedFolder)) {
        selectedFolder = folders[0]?.folder ?? null;
      }
    } catch (e) {
      scanError = String(e);
    }
  }

  /** Re-reads the listing after the index changed. */
  function refreshListing() {
    pages.reset();
    loadGroups(pages);
    if (view === "folders") loadFolders();
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
      rows.unshift({ path: "", name: library.name, depth: 0, count: counts.get("") ?? 0 });
    }
    return rows;
  });

  function closeViewer() {
    const index = viewerIndex;
    viewerIndex = null;
    if (index !== null) grid?.reveal(index);
  }
</script>

<div class="library">
  <header>
    <button class="link" onclick={onclose}>‹ Libraries</button>
    <strong class="name" title={library.path}>{library.name}</strong>

    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={view === "library"} onclick={() => (view = "library")}>
        Library
      </button>
      <button role="tab" aria-selected={view === "folders"} onclick={() => (view = "folders")}>
        Folders
      </button>
    </div>

    <div class="status" role="status">
      {#if scanning && progress && progress.phase !== "scanning" && progress.total > 0}
        <progress value={progress.done} max={progress.total}></progress>
      {/if}
      <span>{status}</span>
    </div>
    <button class="secondary" onclick={rescan} disabled={scanning}>Rescan</button>
  </header>

  {#if !library.writable}
    <p class="banner">
      This folder is read-only. You can browse it, but albums and favorites will be kept on
      this computer only.
    </p>
  {/if}
  {#if scanError}
    <p class="banner error" role="alert">{scanError}</p>
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
    {#if view === "folders"}
      <nav aria-label="Folders">
        {#each folderRows as row (row.path)}
          <button
            class="folder"
            class:selected={row.path === selectedFolder}
            style:padding-left="{12 + row.depth * 14}px"
            disabled={row.count === 0}
            onclick={() => (selectedFolder = row.path)}
            title={row.path || library.name}
          >
            <span class="folder-name">{row.name}</span>
            {#if row.count > 0}<span class="folder-count">{formatNumber(row.count)}</span>{/if}
          </button>
        {/each}
      </nav>
    {/if}

    <main>
      {#if groups.length > 0}
        <PhotoGrid bind:this={grid} {groups} {pages} onopen={(index) => (viewerIndex = index)} />
      {:else if groupsLoaded}
        <p class="empty">
          {#if scanning}
            Looking for photos…
          {:else}
            No photos found in this folder.
          {/if}
        </p>
      {/if}
    </main>
  </div>
</div>

{#if viewerIndex !== null}
  <Viewer {pages} total={listedTotal} bind:index={viewerIndex} onclose={closeViewer} />
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
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tabs {
    display: flex;
    margin: 0 auto;
    padding: 2px;
    border-radius: 8px;
    background: var(--surface-hover);
  }

  .tabs button {
    padding: 4px 16px;
    border: none;
    border-radius: 6px;
    background: none;
    cursor: pointer;
  }

  .tabs button[aria-selected="true"] {
    background: var(--surface);
    font-weight: 600;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.15);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  progress {
    width: 90px;
    accent-color: var(--accent);
  }

  .secondary {
    padding: 5px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    cursor: pointer;
  }

  .secondary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .banner {
    margin: 0;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--warn-bg);
    color: var(--warn);
  }

  .banner.error {
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  nav {
    width: 240px;
    flex-shrink: 0;
    padding: 8px 0;
    overflow-y: auto;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }

  .folder {
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

  .folder:disabled {
    color: var(--text-dim);
    cursor: default;
  }

  .folder:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .folder.selected,
  .folder.selected:hover {
    background: var(--accent);
    color: var(--accent-text);
  }

  .folder-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .folder-count {
    font-size: 12px;
    opacity: 0.7;
  }

  main {
    flex: 1;
    min-width: 0;
  }

  .empty {
    margin: 0;
    padding-top: 120px;
    text-align: center;
    color: var(--text-dim);
  }
</style>
