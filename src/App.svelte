<script lang="ts">
  import { onMount } from "svelte";
  import {
    closeLibrary as stopLibraryScan,
    libraryStats,
    newestPhotos,
    listRecentLibraries,
    onIndexEvent,
    openLibrary,
    pickLibraryFolder,
    removeRecentLibrary,
    rescanLibrary,
    thumbUrl,
    type IndexEvent,
    type LibraryInfo,
    type PhotoItem,
    type RecentLibrary,
    type ScanProgress,
    type ScanSummary,
    type ThumbSummary,
  } from "./lib/api";

  /** How often an open library is rescanned for changes made outside the app. */
  const RESCAN_INTERVAL_MS = 15 * 60 * 1000;

  let library = $state<LibraryInfo | null>(null);
  let recents = $state<RecentLibrary[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  // Index state of the open library.
  let photoCount = $state<number | null>(null);
  let progress = $state<ScanProgress | null>(null);
  let scanning = $state(false);
  let lastScan = $state<ScanSummary | null>(null);
  let lastThumbs = $state<ThumbSummary | null>(null);
  let newest = $state<PhotoItem[]>([]);

  /** How many of the newest photos the placeholder strip shows. */
  const NEWEST_COUNT = 60;

  async function refreshNewest() {
    const path = library?.path;
    if (!path) return;
    try {
      const photos = await newestPhotos(path, NEWEST_COUNT);
      if (library?.path === path) newest = photos;
    } catch (e) {
      scanError = String(e);
    }
  }
  let scanError = $state<string | null>(null);

  onMount(() => {
    refreshRecents();
    const unlisten = onIndexEvent(handleIndexEvent);
    const timer = setInterval(() => {
      if (library && !scanning) rescan();
    }, RESCAN_INTERVAL_MS);
    return () => {
      clearInterval(timer);
      unlisten.then((stop) => stop());
    };
  });

  function handleIndexEvent(event: IndexEvent) {
    // Ignore events from a library that has been closed in the meantime.
    if (event.libraryPath !== library?.path) return;
    if (event.state === "running") {
      scanning = true;
      progress = event.progress;
    } else if (event.state === "indexed") {
      // The index is complete; thumbnails are still being made.
      lastScan = event.summary;
      photoCount = event.summary.total;
      refreshNewest();
    } else if (event.state === "finished") {
      scanning = false;
      progress = null;
      lastScan = event.summary;
      lastThumbs = event.thumbnails;
      photoCount = event.summary.total;
      refreshNewest();
    } else {
      scanning = false;
      progress = null;
      scanError = event.message;
    }
  }

  async function rescan() {
    if (!library) return;
    scanError = null;
    scanning = true;
    try {
      await rescanLibrary(library.path);
    } catch (e) {
      scanning = false;
      scanError = String(e);
    }
  }

  const numberFormat = new Intl.NumberFormat();
  const n = (value: number) => numberFormat.format(value);

  /** One-line description of what the last scan changed. */
  function describe(summary: ScanSummary): string {
    const parts = [
      summary.added && `${n(summary.added)} added`,
      summary.updated && `${n(summary.updated)} updated`,
      summary.removed && `${n(summary.removed)} removed`,
    ].filter(Boolean);
    const changes = parts.length > 0 ? parts.join(", ") : "No changes";
    const seconds = (summary.durationMs / 1000).toFixed(1);
    return `${summary.cancelled ? "Scan stopped" : "Up to date"} · ${changes} · ${seconds} s`;
  }

  async function refreshRecents() {
    try {
      recents = await listRecentLibraries();
    } catch (e) {
      error = String(e);
    }
  }

  async function open(path: string) {
    busy = true;
    error = null;
    try {
      const opened = await openLibrary(path);
      photoCount = null;
      progress = null;
      lastScan = null;
      lastThumbs = null;
      newest = [];
      scanError = null;
      scanning = true; // open_library starts a scan
      library = opened;
      refreshNewest();
      // Show the count from the previous session while the scan runs.
      const stats = await libraryStats(opened.path);
      if (library?.path === opened.path) {
        photoCount ??= stats.photoCount;
        // In case the scan finished before `library` was set and its events were dropped.
        if (!stats.scanning && !progress) scanning = false;
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
    await refreshRecents();
  }

  async function pickAndOpen() {
    error = null;
    try {
      const path = await pickLibraryFolder();
      if (path) await open(path);
    } catch (e) {
      error = String(e);
    }
  }

  async function forget(path: string) {
    try {
      recents = await removeRecentLibrary(path);
    } catch (e) {
      error = String(e);
    }
  }

  function closeLibrary() {
    if (library) stopLibraryScan(library.path).catch(() => {});
    library = null;
    scanning = false;
    refreshRecents();
  }

  const dateFormat = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
</script>

{#if library}
  <div class="library">
    <header>
      <button class="link" onclick={closeLibrary}>‹ Libraries</button>
      <div class="title">
        <strong>{library.name}</strong>
        <span class="path">{library.path}</span>
      </div>
      <button class="secondary" onclick={rescan} disabled={scanning}>Rescan</button>
    </header>
    {#if !library.writable}
      <p class="banner">
        This folder is read-only. You can browse it, but albums and favorites
        will be kept on this computer only.
      </p>
    {/if}
    {#if lastScan && lastScan.unreadableDirs > 0}
      <p class="banner">
        {n(lastScan.unreadableDirs)}
        {lastScan.unreadableDirs === 1 ? "folder" : "folders"} could not be read, so photos
        that may have been deleted were kept in the index.
      </p>
    {/if}
    <main class="empty">
      <p class="count">
        {#if photoCount === null}
          …
        {:else}
          {n(photoCount)} {photoCount === 1 ? "photo" : "photos"}
        {/if}
      </p>

      {#if scanning}
        <div class="scan" role="status">
          {#if progress?.phase === "indexing" && progress.total > 0}
            <progress value={progress.done} max={progress.total}></progress>
            <p class="dim">
              Indexing {n(progress.done)} of {n(progress.total)} new or changed photos
            </p>
          {:else if progress?.phase === "thumbnails"}
            <progress value={progress.done} max={progress.total}></progress>
            <p class="dim">
              Making thumbnails: {n(progress.done)} of {n(progress.total)}
            </p>
          {:else}
            <progress></progress>
            <p class="dim">
              Scanning folders…
              {#if progress}{n(progress.discovered)} photos found{/if}
            </p>
          {/if}
        </div>
      {:else if scanError}
        <p class="error" role="alert">{scanError}</p>
      {:else if lastScan}
        <p class="dim">{describe(lastScan)}</p>
        {#if lastScan.failed + (lastThumbs?.unreadable ?? 0) > 0}
          <p class="dim">
            {n(lastScan.failed + (lastThumbs?.unreadable ?? 0))} files could not be read.
          </p>
        {/if}
        {#if lastThumbs && lastThumbs.undecodable > 0}
          <p class="dim">
            {n(lastThumbs.undecodable)}
            {lastThumbs.undecodable === 1 ? "file is" : "files are"} not readable as images.
          </p>
        {/if}
      {/if}

      {#if newest.length > 0}
        <h2>Newest photos</h2>
        <div class="strip">
          {#each newest as photo (photo.fingerprint + photo.relPath)}
            <img
              src={thumbUrl(photo.fingerprint)}
              alt={photo.relPath}
              title={photo.relPath}
              loading="lazy"
              decoding="async"
            />
          {/each}
        </div>
        <p class="dim hint">The full timeline grid arrives in the next milestone.</p>
      {/if}
    </main>
  </div>
{:else}
  <div class="welcome">
    <h1>Photolib</h1>
    <p class="dim">Browse any folder of photos: a NAS, an external drive or a local disk.</p>

    <button class="primary" onclick={pickAndOpen} disabled={busy}>
      {busy ? "Opening…" : "Open library folder…"}
    </button>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    {#if recents.length > 0}
      <section>
        <h2>Recent libraries</h2>
        <ul>
          {#each recents as recent (recent.path)}
            <li class:offline={!recent.available}>
              <button
                class="recent"
                onclick={() => open(recent.path)}
                disabled={busy}
                title={recent.path}
              >
                <span class="name">{recent.name}</span>
                <span class="path">{recent.path}</span>
                <span class="meta">
                  {#if recent.available}
                    Opened {dateFormat.format(recent.lastOpened)}
                  {:else}
                    Not reachable right now
                  {/if}
                </span>
              </button>
              <button
                class="forget"
                onclick={() => forget(recent.path)}
                aria-label="Remove {recent.name} from recent libraries"
                title="Remove from list"
              >
                ×
              </button>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>
{/if}

<style>
  .welcome {
    max-width: 560px;
    margin: 0 auto;
    padding: 64px 24px 32px;
    text-align: center;
  }

  h1 {
    margin: 0 0 4px;
    font-size: 28px;
    font-weight: 650;
  }

  h2 {
    margin: 40px 0 8px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
    text-align: left;
  }

  .dim {
    color: var(--text-dim);
  }

  .primary {
    margin-top: 24px;
    padding: 10px 20px;
    border: none;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
    cursor: pointer;
  }

  .primary:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .error {
    margin: 16px 0 0;
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    overflow: hidden;
  }

  li {
    display: flex;
    align-items: stretch;
  }

  li + li {
    border-top: 1px solid var(--border);
  }

  .recent {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
    padding: 10px 14px;
    border: none;
    background: none;
    text-align: left;
    cursor: pointer;
  }

  .recent:hover:not(:disabled),
  .forget:hover {
    background: var(--surface-hover);
  }

  .name {
    font-weight: 600;
  }

  .path,
  .meta {
    font-size: 12px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .offline .name {
    color: var(--text-dim);
  }

  .offline .meta {
    color: var(--warn);
  }

  .forget {
    width: 40px;
    border: none;
    background: none;
    font-size: 18px;
    color: var(--text-dim);
    cursor: pointer;
  }

  .library {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 16px;
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

  .title {
    flex: 1;
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
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

  .count {
    font-size: 22px;
    font-weight: 600;
  }

  .scan {
    display: grid;
    justify-items: center;
    gap: 6px;
    margin-top: 12px;
  }

  progress {
    width: 280px;
    accent-color: var(--accent);
  }

  .hint {
    margin-top: 16px !important;
  }

  .banner {
    border-bottom: 1px solid var(--border);
    margin: 0;
    padding: 8px 16px;
    background: var(--warn-bg);
    color: var(--warn);
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 32px 16px;
    overflow-y: auto;
  }

  .empty h2 {
    align-self: stretch;
    margin: 32px 0 8px;
  }

  .strip {
    align-self: stretch;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 3px;
  }

  .strip img {
    display: block;
    width: 100%;
    aspect-ratio: 1;
    object-fit: cover;
    background: var(--surface-hover);
  }

  .empty p {
    margin: 2px 0;
  }
</style>
