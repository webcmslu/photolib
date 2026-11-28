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
  import { autoBackup, startBackup } from "./lib/phone";
  import SettingsButton from "./lib/SettingsButton.svelte";
  import { suppressDefaultMenu } from "./lib/contextMenu";
  import { errorText, locale, t } from "./lib/i18n";
  import {
    forgetServer,
    inBrowser,
    inPhoneApp,
    DEMO_SERVER,
    logIn,
    savedServers,
    saveServer,
    serverUrl,
    useServer,
    type Server,
  } from "./lib/platform";

  let library = $state<LibraryInfo | null>(null);
  let recents = $state<RecentLibrary[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  /** The Phototek servers connected to before (NAS), and the form to add one. */
  let servers = $state<Server[]>(savedServers());
  let connecting = $state<{ address: string; password: string } | null>(null);

  /** Opens a server's library; with a password, logs in first. */
  async function connect(address: string, password: string | null, known?: Server) {
    busy = true;
    error = null;
    const url = known?.url ?? serverUrl(address);
    try {
      const token = password === null ? (known?.token ?? null) : await logIn(url, password);
      useServer({ url, token });
      const opened = await openLibrary("");
      saveServer({ url, token, name: opened.name });
      if (inPhoneApp && autoBackup()) startBackup().catch(() => {});
      servers = savedServers();
      connecting = null;
      library = opened;
    } catch (e) {
      useServer(null);
      const message = e instanceof Error ? e.message : errorText(e);
      error =
        message === "wrong password"
          ? t("Wrong password.")
          : known && /401|Unauthorized/i.test(message)
            ? t("The server asks for its password again.")
            : t("Can't reach the Phototek server at {address}: {message}", { address: url, message });
      // A saved server whose password changed: ask for it.
      if (known && /401|Unauthorized/i.test(message)) connecting = { address: url, password: "" };
    } finally {
      busy = false;
    }
  }

  /** True while a folder is dragged over the welcome screen. */
  let dropping = $state(false);

  onMount(() => {
    // In a browser, the server has one library: open it straight away.
    if (inBrowser) {
      void open("");
      return;
    }
    // The phone app shows a server's library: the last one used, or the
    // form to add one.
    if (inPhoneApp) {
      const [last] = servers;
      // Development and store screenshots: a server given at build time
      // (never set for a store build).
      const preset = import.meta.env.VITE_PHOTOTEK_SERVER as string | undefined;
      if (last) void connect(last.url, null, last);
      else if (preset) void connect(preset, "");
      else connecting = { address: "", password: "" };
      return;
    }
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
    // Back to this computer's own libraries.
    useServer(null);
    if (inPhoneApp) servers = savedServers();
    else refreshRecents();
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
    <h1>Phototek</h1>
    <p class="dim">
      {inPhoneApp
        ? t("Your photos, from the Phototek server on your NAS.")
        : t("Browse any folder of photos: a NAS, an external drive or a local disk.")}
    </p>

    {#if !inPhoneApp}<div class="drop" class:dropping>
      <button class="primary" onclick={pickAndOpen} disabled={busy}>
        {busy ? t("Opening…") : t("Open library folder…")}
      </button>
      <p class="hint">{dropping ? t("Drop to open this folder") : t("or drop a folder here")}</p>
    </div>{/if}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <!-- A library served by a Phototek server, e.g. on the NAS. -->
    {#if connecting}
      <form
        class="connect"
        onsubmit={(event) => {
          event.preventDefault();
          if (connecting) connect(connecting.address, connecting.password);
        }}
      >
        <label>
          <span>{t("Server address")}</span>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            bind:value={connecting.address}
            placeholder="nas.local:8080"
            autocomplete="url"
            autocapitalize="off"
            spellcheck="false"
            autofocus
            required
          />
        </label>
        <label>
          <span>{t("Password, if it has one")}</span>
          <input type="password" bind:value={connecting.password} autocomplete="current-password" />
        </label>
        <div class="connect-buttons">
          {#if !inPhoneApp || servers.length > 0}
            <button type="button" class="link" onclick={() => (connecting = null)}>{t("Cancel")}</button>
          {/if}
          <button type="submit" class="primary" disabled={busy}>{busy ? t("Connecting…") : t("Connect")}</button>
        </div>
      </form>
    {:else}
      <button class="link connect-link" onclick={() => (connecting = { address: "", password: "" })}>
        {t("Connect to a Phototek server…")}
      </button>
    {/if}

    <!-- Sample photos on our own server: to try the app without a NAS (and
         for App Review). -->
    {#if !servers.some((entry) => entry.url === DEMO_SERVER)}
      <p class="demo">
        {t("No Phototek server yet?")}
        <button class="link" onclick={() => connect(DEMO_SERVER, "")} disabled={busy}>{t("Try the demo")}</button>
      </p>
    {/if}

    {#if servers.length > 0}
      <section>
        <h2>{t("Phototek servers")}</h2>
        <ul>
          {#each servers as entry (entry.url)}
            <li>
              <button class="recent" onclick={() => connect(entry.url, null, entry)} disabled={busy} title={entry.url}>
                <span class="name">{entry.name ?? entry.url}</span>
                <span class="path">{entry.url}</span>
              </button>
              <button
                class="forget"
                onclick={() => {
                  forgetServer(entry.url);
                  servers = savedServers();
                }}
                aria-label={t("Remove {name} from recent libraries", { name: entry.name ?? entry.url })}
                title={t("Remove from list")}
              >
                ×
              </button>
            </li>
          {/each}
        </ul>
      </section>
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
    top: max(12px, env(safe-area-inset-top));
    right: max(12px, env(safe-area-inset-right));
    display: flex;
    gap: 6px;
  }

  .welcome {
    max-width: 560px;
    margin: 0 auto;
    padding: calc(64px + env(safe-area-inset-top)) 24px 32px;
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

  .link {
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }

  .connect-link {
    margin-top: 14px;
    font-size: 13px;
  }

  .demo {
    margin: 10px 0 0;
    font-size: 13px;
    color: var(--text-dim);
  }

  .demo .link {
    font-size: 13px;
  }

  .connect {
    display: grid;
    gap: 10px;
    margin-top: 18px;
    padding: 16px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    text-align: left;
  }

  .connect label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .connect input {
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 14px;
  }

  .connect-buttons {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    margin-top: 4px;
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
