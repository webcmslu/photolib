<!--
  A full-screen slideshow: the selected photos, a month, or the whole
  listing, one after the other. How long each photo stays and the
  transition between them are chosen in Settings. Each photo is loaded at full size before
  it is shown; videos are skipped.

  Keys: Space pauses, ← and → step back and forward, Esc ends it.
-->
<script lang="ts">
  import { t } from "./i18n";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { photoUrl, thumbUrl, type PhotoItem } from "./api";
  import EditedImage from "./EditedImage.svelte";
  import type { PhotoPages } from "./pages.svelte";
  import { settings } from "./settings.svelte";

  interface Props {
    pages: PhotoPages;
    /** How many photos the slideshow goes through. */
    count: number;
    /** The listing index of the slideshow's `position`-th photo. */
    indexAt: (position: number) => number;
    onclose: () => void;
  }
  let { pages, count, indexAt, onclose }: Props = $props();

  const slideMs = $derived(settings.slideSeconds * 1000);
  const transition = $derived(settings.transition);
  /** Longest wait for the full-size file before showing the preview instead. */
  const LOAD_LIMIT_MS = 5000;

  /** The two layers that cross-fade: the photo on screen and the one before. */
  interface Layer {
    photo: PhotoItem;
    src: string;
    key: number;
    /** 1 going forward, -1 going back: the side a sliding photo comes from. */
    direction: 1 | -1;
  }
  let shown = $state<Layer | null>(null);
  let previous = $state<Layer | null>(null);
  let position = -1;
  let paused = $state(false);
  let finished = $state(false);
  let controlsVisible = $state(true);
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;
  /** How long the photo that leaves takes to go (see the CSS animations). */
  const LEAVE_MS = 950;

  /** Resolves with the full-size URL once loaded, or the preview's after a while. */
  function preload(photo: PhotoItem): Promise<string> {
    return new Promise((resolve) => {
      const full = photoUrl(photo.fingerprint);
      const image = new Image();
      const fallback = setTimeout(() => resolve(thumbUrl(photo.fingerprint, "preview")), LOAD_LIMIT_MS);
      image.onload = () => {
        clearTimeout(fallback);
        resolve(full);
      };
      image.onerror = () => {
        clearTimeout(fallback);
        resolve(thumbUrl(photo.fingerprint, "preview"));
      };
      image.src = full;
    });
  }

  /** Shows the next photo (not a video) after `from` in the given direction. */
  async function show(from: number, step: 1 | -1) {
    const mine = ++generation;
    clearTimeout(timer);
    for (let next = from + step; next >= 0 && next < count; next += step) {
      const photo = await pages.load(indexAt(next));
      if (mine !== generation) return;
      if (!photo || photo.video) continue;
      const src = await preload(photo);
      if (mine !== generation) return;
      // The photo that leaves goes away while the new one comes in: a
      // photo of another shape doesn't hide all of it, so it must not stay.
      clearTimeout(leaveTimer);
      previous = transition === "none" ? null : shown;
      shown = { photo, src, key: next, direction: step };
      if (previous) leaveTimer = setTimeout(() => (previous = null), LEAVE_MS);
      position = next;
      schedule();
      return;
    }
    if (step === 1) finished = true;
  }

  function schedule() {
    clearTimeout(timer);
    if (!paused && !finished) timer = setTimeout(() => show(position, 1), slideMs);
  }

  function togglePause() {
    paused = !paused;
    if (paused) clearTimeout(timer);
    else if (finished) restart();
    else schedule();
  }

  function restart() {
    finished = false;
    show(-1, 1);
  }

  function step(direction: 1 | -1) {
    finished = false;
    show(position, direction);
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") onclose();
    else if (event.key === " ") togglePause();
    else if (event.key === "ArrowRight") step(1);
    else if (event.key === "ArrowLeft") step(-1);
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  /** The buttons show while the mouse moves, and fade after a moment. */
  function wake() {
    controlsVisible = true;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => (controlsVisible = false), 2500);
  }

  onMount(() => {
    const appWindow = getCurrentWindow();
    let enteredFullscreen = false;
    appWindow
      .isFullscreen()
      .then((already) => {
        if (already) return;
        enteredFullscreen = true;
        return appWindow.setFullscreen(true);
      })
      .catch(() => {});
    wake();
    show(-1, 1);
    return () => {
      generation++;
      clearTimeout(timer);
      clearTimeout(leaveTimer);
      clearTimeout(hideTimer);
      if (enteredFullscreen) appWindow.setFullscreen(false).catch(() => {});
    };
  });
</script>

<svelte:window onkeydown={onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="slideshow t-{transition}"
  class:idle={!controlsVisible}
  class:paused
  style:--slide-ms="{slideMs}ms"
  onmousemove={wake} role="dialog" aria-label={t("Slideshow")} aria-modal="true" tabindex="-1">
  {#each [previous, shown] as layer, which (layer?.key ?? `empty-${which}`)}
    {#if layer}
      <div class="layer" class:current={which === 1} class:back={layer.direction === -1}
        class:leaving-back={which === 0 && shown?.direction === -1}>
        <div class="motion pan-{layer.key % 4}">
        {#if layer.photo.edit}
          <EditedImage
            src={layer.src}
            edit={layer.photo.edit}
            width={layer.photo.width}
            height={layer.photo.height}
            fit="contain"
          />
        {:else}
          <img src={layer.src} alt="" draggable="false" />
        {/if}
        </div>
      </div>
    {/if}
  {/each}

  {#if finished}
    <p class="end">{t("End of the slideshow")}</p>
  {/if}

  <div class="controls" class:visible={controlsVisible || paused || finished}>
    <button onclick={() => step(-1)} title={t("Previous (←)")} aria-label={t("Previous")}>‹</button>
    <button onclick={togglePause} title={paused ? t("Play (Space)") : t("Pause (Space)")} aria-label={paused ? t("Play") : t("Pause")}>
      {paused || finished ? "▶" : "❚❚"}
    </button>
    <button onclick={() => step(1)} title={t("Next (→)")} aria-label={t("Next")}>›</button>
    <button onclick={onclose} title={t("End (Esc)")} aria-label={t("End the slideshow")}>✕</button>
  </div>
</div>

<style>
  .slideshow {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #000;
    overflow: hidden;
    outline: none;
  }

  .slideshow.idle {
    cursor: none;
  }

  /* The new photo comes in over the previous one, which goes away at the
     same time and is then removed. */
  .layer {
    position: absolute;
    inset: 0;
  }

  .motion {
    width: 100%;
    height: 100%;
  }

  .t-fade .layer.current,
  .t-kenburns .layer.current {
    animation: fade-in 0.9s ease;
  }

  .t-fade .layer:not(.current),
  .t-kenburns .layer:not(.current),
  .t-zoom .layer:not(.current) {
    animation: fade-out 0.9s ease forwards;
  }

  .t-slide .layer:not(.current) {
    animation: slide-out 0.7s cubic-bezier(0.2, 0.7, 0.2, 1) forwards;
  }

  .t-slide .layer:not(.current).leaving-back {
    animation-name: slide-out-back;
  }

  .t-slide .layer.current {
    animation: slide-in 0.7s cubic-bezier(0.2, 0.7, 0.2, 1);
  }

  .t-slide .layer.current.back {
    animation-name: slide-in-back;
  }

  .t-zoom .layer.current {
    animation: zoom-in 0.9s ease;
  }

  /* Pan and zoom: each photo drifts slowly for as long as it is on screen
     (plus the fade into the next), in one of four directions. */
  .t-kenburns .motion {
    animation: var(--pan) calc(var(--slide-ms) + 900ms) linear forwards;
  }

  .t-kenburns.paused .motion {
    animation-play-state: paused;
  }

  .pan-0 {
    --pan: pan-0;
  }
  .pan-1 {
    --pan: pan-1;
  }
  .pan-2 {
    --pan: pan-2;
  }
  .pan-3 {
    --pan: pan-3;
  }

  @keyframes fade-in {
    from {
      opacity: 0;
    }
  }

  @keyframes fade-out {
    to {
      opacity: 0;
    }
  }

  @keyframes slide-out {
    to {
      transform: translateX(-100%);
    }
  }

  @keyframes slide-out-back {
    to {
      transform: translateX(100%);
    }
  }

  @keyframes slide-in {
    from {
      transform: translateX(100%);
    }
  }

  @keyframes slide-in-back {
    from {
      transform: translateX(-100%);
    }
  }

  @keyframes zoom-in {
    from {
      opacity: 0;
      transform: scale(1.12);
    }
  }

  @keyframes pan-0 {
    from {
      transform: scale(1) translate(0, 0);
    }
    to {
      transform: scale(1.12) translate(-2%, -1.5%);
    }
  }

  @keyframes pan-1 {
    from {
      transform: scale(1.12) translate(2%, 1%);
    }
    to {
      transform: scale(1) translate(0, 0);
    }
  }

  @keyframes pan-2 {
    from {
      transform: scale(1) translate(0, 0);
    }
    to {
      transform: scale(1.12) translate(2%, -1%);
    }
  }

  @keyframes pan-3 {
    from {
      transform: scale(1.12) translate(-2%, 1.5%);
    }
    to {
      transform: scale(1) translate(0, 0);
    }
  }

  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .end {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    margin: 0;
    color: rgb(255 255 255 / 0.85);
    font-size: 18px;
    background: rgb(0 0 0 / 0.55);
  }

  .controls {
    position: absolute;
    left: 50%;
    bottom: 28px;
    display: flex;
    gap: 6px;
    padding: 6px;
    border-radius: 999px;
    background: rgb(30 30 30 / 0.75);
    opacity: 0;
    transform: translateX(-50%);
    transition: opacity 0.3s;
  }

  .controls.visible {
    opacity: 1;
  }

  .controls button {
    width: 38px;
    height: 38px;
    border: none;
    border-radius: 50%;
    background: none;
    color: #fff;
    font-size: 17px;
    cursor: pointer;
  }

  .controls button:hover {
    background: rgb(255 255 255 / 0.15);
  }
</style>
