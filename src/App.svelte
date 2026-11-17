<script lang="ts">
  import { onMount } from "svelte";
  import {
    listRecentLibraries,
    openLibrary,
    pickLibraryFolder,
    removeRecentLibrary,
    type LibraryInfo,
    type RecentLibrary,
  } from "./lib/api";
  import LibraryView from "./lib/LibraryView.svelte";
  import AboutButton from "./lib/AboutButton.svelte";

  let library = $state<LibraryInfo | null>(null);
  let recents = $state<RecentLibrary[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  onMount(refreshRecents);

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
      library = await openLibrary(path);
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
    library = null;
    refreshRecents();
  }

  const dateFormat = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
</script>

{#if library}
  {#key library.path}
    <LibraryView {library} onclose={closeLibrary} />
  {/key}
{:else}
  <div class="corner"><AboutButton /></div>
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
  .corner {
    position: fixed;
    top: 12px;
    right: 12px;
  }

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
</style>
