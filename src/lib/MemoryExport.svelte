<!--
  Saves a memory as an MP4 video, with its music, to share with family and
  friends: best quality (1080p) or smaller for messages (720p). The video is
  made on this computer (see memoryVideo.ts); the library is not changed.
-->
<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { photosPage, saveVideo, todayDay, type Memory } from "./api";
  import { errorText, t } from "./i18n";
  import { memorySubtitle, memoryTitle } from "./memories";
  import { canMakeVideos, makeMemoryVideo, type VideoQuality } from "./memoryVideo";

  interface Props {
    libraryPath: string;
    memory: Memory;
    onclose: () => void;
  }
  let { libraryPath, memory, onclose }: Props = $props();

  const title = $derived(memoryTitle(memory));
  const KEY = "photolib.memoryVideoQuality";
  let quality = $state<VideoQuality>(remembered());
  function remembered(): VideoQuality {
    try {
      return localStorage.getItem(KEY) === "small" ? "small" : "high";
    } catch {
      return "high";
    }
  }

  /** `null` while checking whether this computer can make videos. */
  let supported = $state<boolean | null>(null);
  let progress = $state<number | null>(null);
  let saved = $state<string | null>(null);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;
  const running = $derived(progress !== null && saved === null);

  onMount(() => {
    canMakeVideos().then((can) => (supported = can));
    return () => controller?.abort();
  });

  /** A file name from the title: no characters that file systems refuse. */
  const fileName = () => `${title.replace(/[\\/:*?"<>|]+/g, " ").trim() || "Memory"}.mp4`;

  async function start() {
    error = null;
    try {
      localStorage.setItem(KEY, quality);
    } catch {
      // Not remembered this time.
    }
    const path = await save({
      defaultPath: fileName(),
      filters: [{ name: t("Video"), extensions: ["mp4"] }],
    }).catch(() => null);
    if (!path) return;
    const target = /\.mp4$/i.test(path) ? path : `${path}.mp4`;
    controller = new AbortController();
    progress = 0;
    try {
      const items = await photosPage(
        libraryPath,
        { kind: "picked", fingerprints: memory.photos },
        "dateAsc",
        0,
        memory.photos.length,
      );
      const bytes = await makeMemoryVideo({
        memoryId: memory.id,
        title,
        subtitle: memorySubtitle(memory, todayDay()),
        cover: memory.cover,
        photos: items.filter((item) => !item.video),
        quality,
        signal: controller.signal,
        onprogress: (done) => (progress = done),
      });
      await saveVideo(target, bytes);
      saved = target;
    } catch (e) {
      progress = null;
      if (!controller.signal.aborted) error = errorText(e);
    } finally {
      controller = null;
    }
  }

  function stop() {
    controller?.abort();
    progress = null;
  }

  const close = () => {
    if (running) stop();
    else onclose();
  };
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={() => !running && onclose()}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="box"
    role="dialog"
    aria-modal="true"
    aria-label={t("Save as a Video")}
    tabindex="-1"
    onclick={(event) => event.stopPropagation()}
  >
    <header>
      <h2>{t("Save “{title}” as a Video", { title })}</h2>
      {#if !running}
        <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
      {/if}
    </header>

    {#if saved}
      <section>
        <p>{t("The video is ready to share.")}</p>
      </section>
      <footer>
        <button class="secondary" onclick={() => saved && revealItemInDir(saved).catch(() => {})}>
          {t("Show the video")}
        </button>
        <button class="primary" onclick={onclose}>{t("Done")}</button>
      </footer>
    {:else if progress !== null}
      <section>
        <p>{t("Making the video… {percent}%", { percent: String(Math.floor(progress * 100)) })}</p>
        <progress value={progress} max="1"></progress>
        <p class="note">{t("The photos play through once, so it takes a little while.")}</p>
      </section>
      <footer>
        <button class="secondary" onclick={stop}>{t("Stop")}</button>
      </footer>
    {:else}
      <section>
        {#if supported === false}
          <p class="error">{t("This computer can't make videos.")}</p>
        {:else}
          <label class="choice">
            <input type="radio" name="quality" value="high" bind:group={quality} />
            <span>
              <span class="label">{t("Best quality")}</span>
              <span class="note">{t("1080p, for the TV and the computer.")}</span>
            </span>
          </label>
          <label class="choice">
            <input type="radio" name="quality" value="small" bind:group={quality} />
            <span>
              <span class="label">{t("Smaller")}</span>
              <span class="note">{t("720p, for messages and email.")}</span>
            </span>
          </label>
          <p class="note">{t("With the music, the title and the photos as the memory plays them.")}</p>
        {/if}
        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}
      </section>
      <footer>
        <button class="secondary" onclick={onclose}>{t("Cancel")}</button>
        <button class="primary" disabled={!supported} onclick={start}>{t("Save…")}</button>
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
    width: min(440px, calc(100vw - 32px));
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
    gap: 12px;
    padding: 14px 16px 12px 20px;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }

  .close {
    flex: none;
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

  footer button:disabled {
    opacity: 0.5;
    cursor: default;
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
