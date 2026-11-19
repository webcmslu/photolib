<!--
  Imports the new photos and videos of a memory card, camera or folder into
  the library: <folder>/<year>/<year>-<month>. Each copy is checked; the
  device is only read, so the photos can be deleted there afterwards.
-->
<script lang="ts">
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import {
    cancelJob,
    importFiles,
    importPreview,
    importSources,
    onImportProgress,
    onImportSurvey,
    pickImportFolder,
    type ImportPreview,
    type ImportProgress,
    type ImportSource,
    type ImportSummary,
  } from "./api";
  import { formatBytes, formatDayRange } from "./format";
  import { errorText, t, tn } from "./i18n";

  interface Props {
    libraryPath: string;
    /** The drive to import from, if one was picked already. */
    source: ImportSource | null;
    onclose: () => void;
    /** Called after files were imported, to look for them in the library. */
    onimported: () => void;
  }
  let { libraryPath, source: initial, onclose, onimported }: Props = $props();

  let sources = $state<ImportSource[]>([]);
  let source = $state<ImportSource | null>(null);
  let preview = $state<ImportPreview | null>(null);
  let folder = $state("");
  let progress = $state<ImportProgress | null>(null);
  let summary = $state<ImportSummary | null>(null);
  let error = $state<string | null>(null);
  /** Files of the source read so far, while looking for new ones. */
  let surveyed = $state<[number, number] | null>(null);
  /** The phone is locked or doesn't trust this computer yet. */
  let needsTrust = $state(false);
  const running = $derived(progress !== null && summary === null);

  /** The folder each source was last imported into, by name. */
  const folderKey = (name: string) => `photolib.importFolder.${name}`;
  function rememberedFolder(name: string): string {
    try {
      return localStorage.getItem(folderKey(name)) ?? name;
    } catch {
      return name;
    }
  }

  const now = new Date();
  const exampleMonth = `${now.getFullYear()}/${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;

  onMount(() => {
    if (initial) choose(initial);
    else importSources().then((found) => (sources = found)).catch(() => {});
    const unlisten = onImportProgress((next) => {
      if (progress) progress = next;
    });
    const unlistenSurvey = onImportSurvey((done, total) => (surveyed = [done, total]));
    return () => {
      unlisten.then((stop) => stop());
      unlistenSurvey.then((stop) => stop());
    };
  });

  async function choose(chosen: ImportSource) {
    source = chosen;
    folder = rememberedFolder(chosen.name);
    preview = null;
    error = null;
    surveyed = null;
    needsTrust = chosen.restricted;
    if (needsTrust) {
      error = t("Unlock the device and tap “Trust” on it, then try again.");
      return;
    }
    try {
      preview = await importPreview(libraryPath, chosen.path);
    } catch (e) {
      error = errorText(e);
      needsTrust = error === t("Unlock the device and tap “Trust” on it, then try again.");
    }
  }

  /** Asks again for the state of a phone (unlocked and trusted since?). */
  async function retry(current: ImportSource) {
    const found = await importSources().catch(() => [] as ImportSource[]);
    await choose(found.find((s) => s.path === current.path) ?? { ...current, restricted: false });
  }

  async function chooseFolder() {
    const path = await pickImportFolder().catch(() => null);
    if (!path) return;
    const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    await choose({ path, name, restricted: false });
  }

  async function start() {
    if (!source || !preview || !folder.trim()) return;
    error = null;
    try {
      localStorage.setItem(folderKey(source.name), folder.trim());
    } catch {
      // Not remembered; no harm.
    }
    progress = { done: 0, total: preview.new, bytes: 0, totalBytes: preview.newBytes };
    try {
      summary = await importFiles(libraryPath, source.path, folder.trim());
      if (summary.imported > 0) onimported();
    } catch (e) {
      error = errorText(e);
      progress = null;
    }
  }

  function close() {
    if (running) cancelJob();
    else onclose();
  }

  const items = (n: number) => tn(n, "{n} photo or video", "{n} photos and videos");
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={() => !running && onclose()}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="box" role="dialog" aria-modal="true" aria-label={t("Import")} tabindex="-1" onclick={(event) => event.stopPropagation()}>
    <header>
      <h2>{source ? t("Import from “{name}”", { name: source.name }) : t("Import photos")}</h2>
      {#if !running}
        <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
      {/if}
    </header>

    {#if summary}
      <section>
        <p>
          {summary.cancelled ? t("Stopped:") + " " : ""}{t("{items} imported ({size}).", { items: items(summary.imported), size: formatBytes(summary.bytes) })}
        </p>
        {#if summary.failed.length > 0}
          <p class="error">{t("{n} could not be imported:", { n: summary.failed.length })}</p>
          <ul class="failed">
            {#each summary.failed.slice(0, 20) as [name, why] (name)}
              <li><span class="path">{name}</span> {why}</li>
            {/each}
          </ul>
        {/if}
        <p class="note">
          {t("Each copy was checked against the original. The photos are still on the device: delete them there once you have seen them here.")}
        </p>
      </section>
      <footer>
        {#if summary.imported > 0}
          <button class="secondary" onclick={() => revealItemInDir(`${libraryPath}/${folder.trim()}`).catch(() => {})}>
            {t("Show the folder")}
          </button>
        {/if}
        <button class="primary" onclick={onclose}>{t("Done")}</button>
      </footer>
    {:else if progress}
      <section>
        <p>
          {t("Importing {done} of {total}…", { done: progress.done, total: progress.total })}
          <span class="note">{formatBytes(progress.bytes)} / {formatBytes(progress.totalBytes)}</span>
        </p>
        <progress value={progress.bytes} max={Math.max(1, progress.totalBytes)}></progress>
      </section>
      <footer>
        <button class="secondary" onclick={cancelJob}>{t("Stop")}</button>
      </footer>
    {:else if source}
      <section>
        {#if preview}
          {#if preview.new > 0}
            <p>
              <strong>{t("{items} to import ({size})", { items: items(preview.new), size: formatBytes(preview.newBytes) })}</strong>
              {#if preview.firstDay !== null && preview.lastDay !== null}
                <br /><span class="note">{formatDayRange(preview.firstDay, preview.lastDay)}</span>
              {/if}
            </p>
          {:else}
            <p><strong>{t("Nothing new to import.")}</strong></p>
          {/if}
          {#if preview.already > 0}
            <p class="note">{t("{items} already in the library, skipped.", { items: items(preview.already) })}</p>
          {/if}
          {#if preview.unreadable > 0}
            <p class="error">{t("{n} can't be read.", { n: preview.unreadable })}</p>
          {/if}
          {#if preview.new > 0}
            <label class="folder">
              <span class="label">{t("Into the folder")}</span>
              <input type="text" bind:value={folder} spellcheck="false" />
              <span class="note">{t("In the library, by month: {example}", { example: `${folder.trim() || "…"}/${exampleMonth}/` })}</span>
            </label>
          {/if}
        {:else if !error}
          <p>
            {t("Looking for new photos…")}
            {#if surveyed && surveyed[1] > 0}
              <span class="note">{t("{done} of {total}", { done: surveyed[0], total: surveyed[1] })}</span>
            {/if}
          </p>
          {#if surveyed && surveyed[1] > 0}
            <progress value={surveyed[0]} max={surveyed[1]}></progress>
          {:else}
            <progress></progress>
          {/if}
          <p class="note">{t("A phone with many photos can take a few minutes the first time.")}</p>
        {/if}
        {#if error}
          <p class="error" role="alert">{error}</p>
          {#if needsTrust}
            <button class="source" onclick={() => source && retry(source)}>{t("Try again")}</button>
          {/if}
        {/if}
      </section>
      <footer>
        <button class="secondary" onclick={onclose}>{t("Cancel")}</button>
        <button class="primary" onclick={start} disabled={!preview || preview.new === 0 || !folder.trim()}>
          {t("Import")}
        </button>
      </footer>
    {:else}
      <section>
        {#if sources.length > 0}
          <p>{t("Connected:")}</p>
          {#each sources as found (found.path)}
            <button class="source" onclick={() => choose(found)}>
              {found.name}
              {#if found.restricted}<span class="note">— {t("unlock it and tap “Trust”")}</span>{/if}
            </button>
          {/each}
        {:else}
          <p class="note">{t("No phone, memory card or camera is connected. You can also import from any folder.")}</p>
        {/if}
      </section>
      <footer>
        <button class="secondary" onclick={onclose}>{t("Cancel")}</button>
        <button class="primary" onclick={chooseFolder}>{t("Choose a folder…")}</button>
      </footer>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.45);
  }

  .box {
    width: min(420px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    border-radius: 14px;
    background: var(--surface);
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.18);
    color: var(--text);
    outline: none;
    text-align: left;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px 12px 20px;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }

  .close {
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--text-dim);
    font-size: 14px;
    cursor: pointer;
  }

  .close:hover {
    background: var(--surface-hover);
  }

  section {
    padding: 14px 20px;
    font-size: 13px;
  }

  section p {
    margin: 0 0 8px;
  }


  .label {
    font-weight: 600;
  }

  .note {
    font-size: 12px;
    color: var(--text-dim);
  }

  p.note {
    margin-top: 8px;
  }

  progress {
    width: 100%;
  }

  .error {
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .failed {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
    color: var(--text-dim);
    user-select: text;
    -webkit-user-select: text;
  }

  .path {
    color: var(--text);
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 20px 16px;
    border-top: 1px solid var(--border);
  }

  footer button {
    padding: 7px 14px;
    border-radius: 7px;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .primary {
    border: none;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }

  .secondary {
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
  }

  .secondary:hover {
    background: var(--surface-hover);
  }

  .folder {
    display: grid;
    gap: 4px;
    margin-top: 12px;
  }

  .folder input {
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  .source {
    display: block;
    width: 100%;
    margin: 0 0 6px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .source:hover {
    background: var(--surface-hover);
  }

  footer button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
