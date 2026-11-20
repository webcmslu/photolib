<!--
  Imports the new photos and videos of a memory card, camera or folder into
  the library: <folder>/<year>/<year>-<month>. Each copy is checked; the
  device is only read, so the photos can be deleted there afterwards.
-->
<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    cancelJob,
    freeUp,
    freeUpPlan,
    importFiles,
    importPreview,
    importSources,
    onImportProgress,
    onImportSurvey,
    pickImportFolder,
    type FreeUpPlan,
    type FreeUpSummary,
    type ImportPreview,
    type ImportProgress,
    type ImportSource,
    type ImportSummary,
    type ImportTile,
    type RawFiles,
  } from "./api";
  import { formatBytes, formatDateTime, formatDayRange } from "./format";
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
  /** The source is a folder picked by hand: never offered for clearing. */
  let pickedFolder = $state(false);

  // --- Choosing what to import ------------------------------------------------

  /** The `group` of each tile to import. Everything new to begin with. */
  const selected = new SvelteSet<number>();
  /** Also show what is already in the library (always, if nothing is new). */
  let showAlready = $state(false);
  const shown = $derived(
    preview ? preview.tiles.filter((tile) => showAlready || preview!.new === 0 || !tile.already) : [],
  );
  const selectable = (tile: ImportTile) => !tile.already && !tile.unreadable;
  const selectedTiles = $derived(preview ? preview.tiles.filter((tile) => selected.has(tile.group)) : []);
  /** Where RAW files go; remembered for each source. */
  let rawFiles = $state<RawFiles>("beside");
  const rawKey = (name: string) => `photolib.importRaw.${name}`;
  const hasRaw = $derived(!!preview?.tiles.some((tile) => tile.raw && selectable(tile)));
  const selectedBytes = $derived(
    selectedTiles.reduce((sum, tile) => sum + tile.bytes - (rawFiles === "skip" ? tile.rawBytes : 0), 0),
  );
  /** Some selected photos come with their RAW file, which counts in the size. */
  const selectedRaw = $derived(rawFiles !== "skip" && selectedTiles.some((tile) => tile.rawBytes > 0));
  /** Where a shift-click selects from. */
  let anchor: number | null = null;

  function selectAll() {
    for (const tile of preview?.tiles ?? []) if (selectable(tile)) selected.add(tile.group);
  }

  /** Click: one tile on or off; shift-click: all from the last click to this one. */
  function toggle(index: number, range: boolean) {
    const tile = shown[index];
    if (!tile || !selectable(tile)) return;
    const on = !selected.has(tile.group);
    if (range && anchor !== null) {
      const [from, to] = anchor < index ? [anchor, index] : [index, anchor];
      for (const other of shown.slice(from, to + 1)) {
        if (!selectable(other)) continue;
        if (on) selected.add(other.group);
        else selected.delete(other.group);
      }
    } else if (on) {
      selected.add(tile.group);
    } else {
      selected.delete(tile.group);
    }
    anchor = index;
  }

  const thumbUrl = (tile: ImportTile) =>
    preview ? convertFileSrc(`${preview.generation}/${tile.thumb}`, "importthumb") : "";
  /** Tiles whose thumbnail could not be made: they show their name instead. */
  const noThumb = new SvelteSet<number>();

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

  async function choose(chosen: ImportSource, picked = false) {
    source = chosen;
    pickedFolder = picked;
    freeing = null;
    folder = rememberedFolder(chosen.name);
    try {
      const saved = localStorage.getItem(rawKey(chosen.name));
      rawFiles = saved === "subfolder" || saved === "skip" ? saved : "beside";
    } catch {
      rawFiles = "beside";
    }
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
      selected.clear();
      noThumb.clear();
      anchor = null;
      selectAll();
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
    await choose({ path, name, restricted: false }, true);
  }

  async function start() {
    if (!source || !preview || !folder.trim() || selected.size === 0) return;
    error = null;
    try {
      localStorage.setItem(folderKey(source.name), folder.trim());
      localStorage.setItem(rawKey(source.name), rawFiles);
    } catch {
      // Not remembered; no harm.
    }
    progress = { done: 0, total: selectedTiles.length, bytes: 0, totalBytes: selectedBytes };
    try {
      summary = await importFiles(libraryPath, source.path, folder.trim(), [...selected], rawFiles);
      if (summary.imported > 0) onimported();
    } catch (e) {
      error = errorText(e);
      progress = null;
    }
  }

  // --- Freeing up space on the device -------------------------------------------

  /** The "free up space" step, once opened. */
  let freeing = $state<{
    plan: FreeUpPlan | null;
    summary: FreeUpSummary | null;
    busy: boolean;
    error: string | null;
  } | null>(null);
  /** Days of recent photos that stay on the device; remembered for each source. */
  let keepDays = $state(30);
  let deleting = $state(false);
  const keepKey = (name: string) => `photolib.freeUpKeep.${name}`;
  const canFreeUp = $derived(!!source && !pickedFolder && !source.restricted);

  async function openFreeUp(lookAgain: boolean) {
    if (!source) return;
    try {
      const saved = Number(localStorage.getItem(keepKey(source.name)));
      keepDays = localStorage.getItem(keepKey(source.name)) !== null && Number.isFinite(saved) ? saved : 30;
    } catch {
      keepDays = 30;
    }
    freeing = { plan: null, summary: null, busy: true, error: null };
    try {
      // After an import, what the device holds is looked at again.
      if (lookAgain) {
        surveyed = null;
        summary = null;
        progress = null;
        preview = await importPreview(libraryPath, source.path);
      }
      await planFreeUp();
    } catch (e) {
      freeing = { plan: null, summary: null, busy: false, error: errorText(e) };
    }
  }

  async function planFreeUp() {
    if (!source || !freeing) return;
    const days = Math.max(0, Math.min(3650, Math.round(keepDays || 0)));
    freeing.busy = true;
    freeing.error = null;
    try {
      freeing.plan = await freeUpPlan(libraryPath, source.path, days);
    } catch (e) {
      freeing.error = errorText(e);
    } finally {
      freeing.busy = false;
    }
  }

  async function startFreeUp() {
    if (!source || !freeing?.plan || freeing.plan.photos === 0) return;
    const days = Math.max(0, Math.min(3650, Math.round(keepDays || 0)));
    try {
      localStorage.setItem(keepKey(source.name), String(days));
    } catch {
      // Not remembered; no harm.
    }
    freeing.busy = true;
    freeing.error = null;
    deleting = true;
    try {
      freeing.summary = await freeUp(libraryPath, source.path, days);
    } catch (e) {
      freeing.error = errorText(e);
    } finally {
      freeing.busy = false;
      deleting = false;
    }
  }

  /** Back to the photos of the device, looked at again after deleting. */
  async function leaveFreeUp() {
    const deleted = !!freeing?.summary;
    freeing = null;
    if (deleted && source) await choose(source, pickedFolder);
  }

  function close() {
    if (freeing?.busy) return;
    if (running) cancelJob();
    else onclose();
  }

  const items = (n: number) => tn(n, "{n} photo or video", "{n} photos and videos");
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={() => !running && !freeing?.busy && onclose()}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="box" class:wide={!!preview && !summary && !progress && !freeing} role="dialog" aria-modal="true" aria-label={t("Import")} tabindex="-1" onclick={(event) => event.stopPropagation()}>
    <header>
      <h2>{source ? t("Import from “{name}”", { name: source.name }) : t("Import photos")}</h2>
      {#if !running && !freeing?.busy}
        <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
      {/if}
    </header>

    {#if freeing && source}
      {@const plan = freeing.plan}
      {#if freeing.summary}
        <section>
          <p>{t("{items} deleted from “{name}” ({size} freed).", { items: items(freeing.summary.deleted), name: source.name, size: formatBytes(freeing.summary.bytes) })}</p>
          {#if freeing.summary.failed.length > 0}
            <p class="error">{t("{n} could not be deleted:", { n: freeing.summary.failed.length })}</p>
            <ul class="failed">
              {#each freeing.summary.failed.slice(0, 20) as [name, why] (name)}
                <li><span class="path">{name}</span> {why}</li>
              {/each}
            </ul>
          {/if}
          {#if source.path.startsWith("camera:")}
            <p class="note">{t("On an iPhone, deleted photos may wait in “Recently Deleted” in the Photos app: empty it there to get the space back.")}</p>
          {/if}
        </section>
        <footer>
          <button class="secondary" onclick={leaveFreeUp}>{t("Back to the photos")}</button>
          <button class="primary" onclick={onclose}>{t("Done")}</button>
        </footer>
      {:else}
        <section>
          <p class="lead">{t("Free up space on “{name}”", { name: source.name })}</p>
          <p class="note">{t("Only photos and videos whose copy is in this library, and found on its disk, are deleted from the device; a photo goes with its Live Photo video and RAW file, or stays whole.")}</p>
          <label class="keep">
            {t("Keep the last")}
            <input type="number" min="0" max="3650" bind:value={keepDays} onchange={planFreeUp} disabled={freeing.busy} />
            {t("days on the device")}
          </label>
          {#if freeing.error}
            <p class="error" role="alert">{freeing.error}</p>
          {:else if !plan}
            {#if surveyed && surveyed[1] > 0 && surveyed[0] < surveyed[1]}
              <p class="note">{t("Looking at the device again…")} {t("{done} of {total}", { done: surveyed[0], total: surveyed[1] })}</p>
              <progress value={surveyed[0]} max={surveyed[1]}></progress>
            {:else}
              <progress></progress>
            {/if}
          {:else}
            {#if plan.photos > 0}
              <p><strong>{t("{items} can be deleted ({size}).", { items: items(plan.photos), size: formatBytes(plan.bytes) })}</strong></p>
            {:else}
              <p><strong>{t("Nothing can be deleted from the device yet.")}</strong></p>
            {/if}
            {#if plan.keptRecent > 0}
              <p class="note">{tn(plan.keptRecent, "{n} recent photo or video stays.", "{n} recent photos and videos stay.")}</p>
            {/if}
            {#if plan.notChecked > 0}
              <p class="note">
                {tn(plan.notChecked, "{n} stays: its copy is not in the library yet, or could not be found.", "{n} stay: their copy is not in the library yet, or could not be found.")}
                {t("Photos just imported count once the library has finished reading them.")}
                <button class="link" onclick={() => openFreeUp(true)} disabled={freeing.busy}>{t("Check again")}</button>
              </p>
            {/if}
            {#if plan.photos > 0}
              {#if source.path.startsWith("camera:")}
                <p class="warning">{t("If iCloud Photos is on for this iPhone, they are deleted from iCloud and your other devices too.")}</p>
              {:else}
                <p class="warning">{t("They are deleted from the card for good.")}</p>
              {/if}
            {/if}
          {/if}
        </section>
        <footer>
          <button class="secondary" onclick={leaveFreeUp} disabled={freeing.busy}>{t("Back")}</button>
          <button class="danger" onclick={startFreeUp} disabled={freeing.busy || !plan || plan.photos === 0}>
            {deleting
              ? t("Deleting…")
              : plan && plan.photos > 0
                ? t("Delete {items} from “{name}”", { items: items(plan.photos), name: source.name })
                : t("Delete")}
          </button>
        </footer>
      {/if}
    {:else if summary}
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
        {#if canFreeUp && summary.imported > 0 && !summary.cancelled}
          <button class="secondary" onclick={() => openFreeUp(true)}>{t("Free up space on the device…")}</button>
        {/if}
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
    {:else if source && preview}
      <section class="bar">
        <span>
          {#if preview.tiles.length === 0}
            <strong>{t("The device shows no photos.")}</strong>
          {:else if preview.new === 0 && preview.already === 0}
            <strong>{t("None of these files could be read.")}</strong>
          {:else if preview.new === 0}
            <strong>{t("Everything here is already in the library.")}</strong>
          {:else}
            <strong>{t("{items} to import ({size})", { items: items(preview.tiles.filter(selectable).length), size: formatBytes(preview.newBytes) })}</strong>
          {/if}
          {#if preview.firstDay !== null && preview.lastDay !== null}
            <span class="note">· {formatDayRange(preview.firstDay, preview.lastDay)}</span>
          {/if}
          {#if preview.unreadable > 0}
            <span class="error" title={preview.unreadableReason ?? ""}>
              · {t("{n} can't be read.", { n: preview.unreadable })}
              {#if preview.unreadableReason}({preview.unreadableReason}){/if}
            </span>
          {/if}
        </span>
        <span class="spacer"></span>
        {#if canFreeUp && preview.already > 0}
          <button class="secondary small" onclick={() => openFreeUp(false)}>{t("Free up space…")}</button>
        {/if}
        {#if preview.new > 0}
          {#if preview.already > 0}
            <label class="check">
              <input type="checkbox" bind:checked={showAlready} />
              {t("Show the {n} already imported", { n: preview.tiles.length - preview.tiles.filter(selectable).length })}
            </label>
          {/if}
          <button class="secondary small" onclick={selectAll}>{t("Select all")}</button>
          <button class="secondary small" onclick={() => selected.clear()} disabled={selected.size === 0}>{t("Select none")}</button>
        {/if}
      </section>
      <div class="grid" role="listbox" aria-multiselectable="true" aria-label={t("Photos to import")}>
        {#each shown as tile, index (tile.group)}
          <button
            class="tile"
            class:on={selected.has(tile.group)}
            class:done={tile.already}
            role="option"
            aria-selected={selected.has(tile.group)}
            disabled={!selectable(tile)}
            title={`${tile.name}${tile.taken !== null ? " · " + formatDateTime(tile.taken) : ""}`}
            onclick={(event) => toggle(index, event.shiftKey)}
          >
            {#if noThumb.has(tile.group)}
              <span class="name">{tile.name}</span>
            {:else}
              <img src={thumbUrl(tile)} alt={tile.name} loading="lazy" draggable="false" onerror={() => noThumb.add(tile.group)} />
            {/if}
            {#if tile.live}<span class="badge">LIVE</span>{:else if tile.video}<span class="badge">▶</span>{:else if tile.raw}<span class="badge">RAW</span>{/if}
            {#if tile.already}
              <span class="state">✓ {t("In the library")}</span>
            {:else if tile.unreadable}
              <span class="state">{t("Can't be read")}</span>
            {:else}
              <span class="mark" aria-hidden="true">{selected.has(tile.group) ? "✓" : ""}</span>
            {/if}
          </button>
        {/each}
      </div>
      {#if preview.tiles.length === 0 && source.path.startsWith("camera:")}
        <p class="note empty">{t("If the phone has photos, unlock it, wait a moment and try again.")}</p>
      {/if}
      <footer class="split">
        {#if preview.tiles.length === 0}
          <button class="secondary" onclick={() => source && retry(source)}>{t("Try again")}</button>
        {/if}
        {#if preview.new > 0}
          <label class="folder">
            <span class="label">{t("Into the folder")}</span>
            <input type="text" bind:value={folder} spellcheck="false" />
            <span class="note">{`${folder.trim() || "…"}/${exampleMonth}/`}</span>
          </label>
          {#if hasRaw}
            <label class="folder">
              <span class="label">{t("RAW files")}</span>
              <select bind:value={rawFiles}>
                <option value="beside">{t("Next to the JPEGs")}</option>
                <option value="subfolder">{t("In a subfolder (cr2, nef…)")}</option>
                <option value="skip">{t("Don't import")}</option>
              </select>
            </label>
          {/if}
        {/if}
        <span class="spacer"></span>
        {#if error}
          <span class="error" role="alert">{error}</span>
        {/if}
        <button class="secondary" onclick={onclose}>{t("Cancel")}</button>
        <button class="primary" onclick={start} disabled={selected.size === 0 || !folder.trim()}>
          {selected.size === 0
            ? t("Import")
            : selectedRaw
              ? t("Import {items} ({size} with their RAW files)", { items: items(selectedTiles.length), size: formatBytes(selectedBytes) })
              : t("Import {items} ({size})", { items: items(selectedTiles.length), size: formatBytes(selectedBytes) })}
        </button>
      </footer>
    {:else if source}
      <section>
        {#if !error}
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

  .box.wide {
    display: flex;
    flex-direction: column;
    width: min(1100px, calc(100vw - 32px));
    height: calc(100vh - 48px);
    overflow: hidden;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    border-bottom: 1px solid var(--border);
  }

  .spacer {
    flex: 1;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .bar button.small {
    padding: 4px 10px;
    border-radius: 6px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .grid {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    /* Fixed rows: WebKit does not size grid items by aspect-ratio reliably. */
    grid-auto-rows: 130px;
    gap: 4px;
    padding: 8px 12px;
    align-content: start;
  }

  .tile {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    padding: 0;
    border: 3px solid transparent;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-hover);
    cursor: pointer;
  }

  .tile.on {
    border-color: var(--accent);
  }

  .tile.done {
    cursor: default;
  }

  .tile.done img {
    opacity: 0.4;
  }

  .tile img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  .tile .name {
    display: block;
    padding: 6px;
    font-size: 11px;
    color: var(--text-dim);
    word-break: break-all;
  }

  .mark {
    position: absolute;
    top: 5px;
    left: 5px;
    width: 20px;
    height: 20px;
    border: 2px solid #fff;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.25);
    color: #fff;
    font-size: 12px;
    line-height: 16px;
    text-align: center;
    box-shadow: 0 0 3px rgb(0 0 0 / 0.5);
  }

  .tile.on .mark {
    border-color: var(--accent);
    background: var(--accent);
  }

  .badge {
    position: absolute;
    top: 5px;
    right: 5px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgb(0 0 0 / 0.55);
    color: #fff;
    font-size: 10px;
    font-weight: 700;
  }

  .state {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 3px 5px;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    font-size: 11px;
  }

  footer.split {
    align-items: center;
    flex-wrap: wrap;
  }

  footer.split .folder {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
  }

  footer.split .error {
    font-size: 12px;
  }

  .empty {
    padding: 0 20px;
  }

  .folder {
    display: grid;
    gap: 4px;
    margin-top: 12px;
  }

  .folder select {
    padding: 5px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
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

  .lead {
    font-weight: 600;
  }

  .keep {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 12px 0;
  }

  .keep input {
    width: 64px;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  .warning {
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: var(--text);
  }

  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }

  .danger {
    border: none;
    background: var(--danger);
    color: #fff;
    font-weight: 600;
  }

  footer button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
