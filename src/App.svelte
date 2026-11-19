<script lang="ts">
  import { getCurrentWebview } from "@tauri-apps/api/webview";
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
  import SettingsButton from "./lib/SettingsButton.svelte";
  import { suppressDefaultMenu } from "./lib/contextMenu";
  import { errorText, locale, t } from "./lib/i18n";

  let library = $state<LibraryInfo | null>(null);
  let recents = $state<RecentLibrary[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  /** True while a folder is dragged over the welcome screen. */
  let dropping = $state(false);

  onMount(() => {
    refreshRecents();
    // A folder dropped on the window opens as a library, on the welcome
    // screen only: in a library the window has other things to do.
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const kind = event.payload.type;
      if (library || busy) {
        dropping = false;
        return;
      }
      if (kind === "enter" || kind === "over") dropping = true;
      else if (kind === "leave") dropping = false;
      else if (kind === "drop") {
        dropping = false;
        const [path] = event.payload.paths;
        if (event.payload.paths.length > 1) {
          error = t("Drop one folder at a time.");
        } else if (path) {
          open(path);
        }
      }
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  async function refreshRecents() {
    try {
      recents = await listRecentLibraries();
    } catch (e) {
      error = errorText(e);
    }
  }

  async function open(path: string) {
    busy = true;
    error = null;
    try {
      library = await openLibrary(path);
    } catch (e) {
      error = errorText(e);
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
      error = errorText(e);
    }
  }

  async function forget(path: string) {
    try {
      recents = await removeRecentLibrary(path);
    } catch (e) {
      error = errorText(e);
    }
  }

  function closeLibrary() {
    library = null;
    refreshRecents();
  }

  const dateFormat = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });
</script>

<svelte:window oncontextmenu={suppressDefaultMenu} />

{#if library}
  {#key library.path}
    <LibraryView {library} onclose={closeLibrary} />
  {/key}
{:else}
  <div class="corner"><SettingsButton /><AboutButton /></div>
  <div class="welcome">
    <h1>Photolib</h1>
    <p class="dim">{t("Browse any folder of photos: a NAS, an external drive or a local disk.")}</p>

    <div class="drop" class:dropping>
      <button class="primary" onclick={pickAndOpen} disabled={busy}>
        {busy ? t("Opening…") : t("Open library folder…")}
      </button>
      <p class="hint">{dropping ? t("Drop to open this folder") : t("or drop a folder here")}</p>
    </div>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    {#if recents.length > 0}
      <section>
        <h2>{t("Recent libraries")}</h2>
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
                    {t("Opened {date}", { date: dateFormat.format(recent.lastOpened) })}
                  {:else}
                    {t("Not reachable right now")}
                  {/if}
                </span>
              </button>
              <button
                class="forget"
                onclick={() => forget(recent.path)}
                aria-label={t("Remove {name} from recent libraries", { name: recent.name })}
                title={t("Remove from list")}
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
    display: flex;
    gap: 6px;
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

  .drop {
    margin-top: 24px;
    padding: 24px 16px 18px;
    border: 2px dashed var(--border);
    border-radius: 14px;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .drop.dropping {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }

  .hint {
    margin: 10px 0 0;
    font-size: 12px;
    color: var(--text-dim);
  }

  .dropping .hint {
    color: var(--accent);
    font-weight: 600;
  }

  .primary {
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
