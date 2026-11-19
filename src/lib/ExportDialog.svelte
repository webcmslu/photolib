<!--
  Exports the selected photos into a folder: the original files, or JPEGs
  with the edits applied, at full size or smaller for email and messages.
  Videos are always copied as they are.
-->
<script lang="ts">
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import {
    cancelJob,
    exportPhotos,
    onExportProgress,
    pickExportFolder,
    type ExportSize,
    type ExportSummary,
  } from "./api";
  import { photoCount } from "./format";
  import { errorText, t } from "./i18n";

  interface Props {
    libraryPath: string;
    /** Fingerprints of the photos to export. */
    photos: string[];
    onclose: () => void;
  }
  let { libraryPath, photos, onclose }: Props = $props();

  const CHOICES = [
    { id: "original", label: t("Original files"), note: t("Exact copies, as shot. Edits are not applied."), size: { kind: "original" } },
    { id: "full", label: t("Full size JPEG"), note: t("With your edits."), size: { kind: "jpeg", longSide: null } },
    { id: "large", label: t("Large JPEG"), note: t("{n} pixels, with your edits.", { n: "2048" }), size: { kind: "jpeg", longSide: 2048 } },
    { id: "small", label: t("Small JPEG"), note: t("{n} pixels, for email and messages.", { n: "1280" }), size: { kind: "jpeg", longSide: 1280 } },
  ] as const satisfies readonly { id: string; label: string; note: string; size: ExportSize }[];
  type Choice = (typeof CHOICES)[number]["id"];

  const KEY = "photolib.exportSize";
  function remembered(): Choice {
    try {
      const saved = localStorage.getItem(KEY);
      return CHOICES.find((choice) => choice.id === saved)?.id ?? "original";
    } catch {
      return "original";
    }
  }
  let choice = $state<Choice>(remembered());

  let progress = $state<[number, number] | null>(null);
  let result = $state<{ summary: ExportSummary; dest: string } | null>(null);
  let error = $state<string | null>(null);
  const running = $derived(progress !== null && result === null);

  onMount(() => {
    const unlisten = onExportProgress((done, total) => {
      if (progress) progress = [done, total];
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  async function start() {
    error = null;
    try {
      localStorage.setItem(KEY, choice);
    } catch {
      // Not remembered; no harm.
    }
    const dest = await pickExportFolder().catch(() => null);
    if (!dest) return;
    const size = CHOICES.find((c) => c.id === choice)!.size;
    progress = [0, photos.length];
    try {
      result = { summary: await exportPhotos(libraryPath, photos, dest, size), dest };
    } catch (e) {
      error = errorText(e);
      progress = null;
    }
  }

  function close() {
    if (running) cancelJob();
    else onclose();
  }
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={() => !running && onclose()}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="box" role="dialog" aria-modal="true" aria-label={t("Export")} tabindex="-1" onclick={(event) => event.stopPropagation()}>
    <header>
      <h2>{t("Export {photos}", { photos: photoCount(photos.length) })}</h2>
      {#if !running}
        <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
      {/if}
    </header>

    {#if result}
      <section>
        <p>
          {result.summary.cancelled ? t("Stopped:") + " " : ""}{t("{n} exported.", { n: result.summary.exported })}
        </p>
        {#if result.summary.failed.length > 0}
          <p class="error">{t("{n} could not be exported:", { n: result.summary.failed.length })}</p>
          <ul class="failed">
            {#each result.summary.failed.slice(0, 20) as [path, why] (path)}
              <li><span class="path">{path}</span> {why}</li>
            {/each}
          </ul>
        {/if}
      </section>
      <footer>
        <button class="secondary" onclick={() => result && revealItemInDir(result.dest).catch(() => {})}>
          {t("Show the folder")}
        </button>
        <button class="primary" onclick={onclose}>{t("Done")}</button>
      </footer>
    {:else if progress}
      <section>
        <p>{t("Exporting {done} of {total}…", { done: progress[0], total: progress[1] })}</p>
        <progress value={progress[0]} max={progress[1]}></progress>
      </section>
      <footer>
        <button class="secondary" onclick={cancelJob}>{t("Stop")}</button>
      </footer>
    {:else}
      <section>
        {#each CHOICES as option (option.id)}
          <label class="choice">
            <input type="radio" name="size" value={option.id} bind:group={choice} />
            <span>
              <span class="label">{option.label}</span>
              <span class="note">{option.note}</span>
            </span>
          </label>
        {/each}
        <p class="note">{t("Videos are copied as they are. The library is not changed.")}</p>
        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}
      </section>
      <footer>
        <button class="secondary" onclick={onclose}>{t("Cancel")}</button>
        <button class="primary" onclick={start}>{t("Choose a folder…")}</button>
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

  .choice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 6px 0;
    cursor: pointer;
  }

  .choice input {
    margin-top: 2px;
  }

  .choice > span {
    display: grid;
    gap: 1px;
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
</style>
