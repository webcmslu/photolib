<!--
  A full-screen slideshow: the selected photos, or the whole listing, one
  after the other with a cross-fade. Each photo is loaded at full size before
  it is shown; videos are skipped.

  Keys: Space pauses, ← and → step back and forward, Esc ends it.
-->
<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { photoUrl, thumbUrl, type PhotoItem } from "./api";
  import EditedImage from "./EditedImage.svelte";
  import type { PhotoPages } from "./pages.svelte";

  interface Props {
    pages: PhotoPages;
    /** How many photos the slideshow goes through. */
    count: number;
    /** The listing index of the slideshow's `position`-th photo. */
    indexAt: (position: number) => number;
    onclose: () => void;
  }
  let { pages, count, indexAt, onclose }: Props = $props();

  const SLIDE_MS = 4000;
  /** Longest wait for the full-size file before showing the preview instead. */
  const LOAD_LIMIT_MS = 5000;

  /** The two layers that cross-fade: the photo on screen and the one before. */
  let shown = $state<{ photo: PhotoItem; src: string; key: number } | null>(null);
  let previous = $state<{ photo: PhotoItem; src: string; key: number } | null>(null);
  let position = -1;
  let paused = $state(false);
  let finished = $state(false);
  let controlsVisible = $state(true);
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;

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
      previous = shown;
      shown = { photo, src, key: next };
      position = next;
      schedule();
      return;
    }
    if (step === 1) finished = true;
  }

  function schedule() {
    clearTimeout(timer);
    if (!paused && !finished) timer = setTimeout(() => show(position, 1), SLIDE_MS);
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
      clearTimeout(hideTimer);
      if (enteredFullscreen) appWindow.setFullscreen(false).catch(() => {});
    };
  });
</script>

<svelte:window onkeydown={onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="slideshow" class:idle={!controlsVisible} onmousemove={wake} role="dialog" aria-label="Slideshow" aria-modal="true" tabindex="-1">
  {#each [previous, shown] as layer, which (layer?.key ?? `empty-${which}`)}
    {#if layer}
      <div class="layer" class:current={which === 1}>
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
    {/if}
  {/each}

  {#if finished}
    <p class="end">End of the slideshow</p>
  {/if}

  <div class="controls" class:visible={controlsVisible || paused || finished}>
    <button onclick={() => step(-1)} title="Previous (←)" aria-label="Previous">‹</button>
    <button onclick={togglePause} title={paused ? "Play (Space)" : "Pause (Space)"} aria-label={paused ? "Play" : "Pause"}>
      {paused || finished ? "▶" : "❚❚"}
    </button>
    <button onclick={() => step(1)} title="Next (→)" aria-label="Next">›</button>
    <button onclick={onclose} title="End (Esc)" aria-label="End the slideshow">✕</button>
  </div>
</div>

<style>
  .slideshow {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #000;
    outline: none;
  }

  .slideshow.idle {
    cursor: none;
  }

  /* The new photo fades in over the previous one, which stays as it is
     underneath until it is replaced: no dip to black between photos. */
  .layer {
    position: absolute;
    inset: 0;
  }

  .layer.current {
    animation: fade-in 0.9s ease;
  }

  @keyframes fade-in {
    from {
      opacity: 0;
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
