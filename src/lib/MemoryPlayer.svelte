<!--
  Plays a memory full screen, as in Apple Photos: its title over the cover
  photo coming into focus, then its photos one after the other, each one
  drifting slowly (the "Ken Burns" effect), to soft music made up on the
  spot. Upright photos sit on a blurred copy of themselves. At the end it
  offers to play again or to show all the memory's photos.

  Keys: Space pauses, ← and → step back and forward, M mutes, Esc ends it.
-->
<script lang="ts">
  import { t, tn } from "./i18n";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { photosPage, photoUrl, thumbUrl, todayDay, type Memory, type PhotoItem } from "./api";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { memorySubtitle, memoryTitle, musicWanted, playMusic, setMusicWanted } from "./memories";

  interface Props {
    libraryPath: string;
    memory: Memory;
    onclose: () => void;
    /** Shows every photo of the memory in the grid. */
    onshowall: () => void;
    /** Saves the memory as a video. */
    onexport: () => void;
  }
  let { libraryPath, memory, onclose, onshowall, onexport }: Props = $props();

  const TITLE_MS = 4200;
  const SLIDE_MS = 4600;
  /** Longest wait for the full-size file before showing the preview instead. */
  const LOAD_LIMIT_MS = 2500;

  const title = $derived(memoryTitle(memory));
  const subtitle = $derived(memorySubtitle(memory, todayDay()));

  let photos = $state<PhotoItem[]>([]);
  /** -1 is the title, `photos.length` the end. */
  let position = $state(-1);
  let paused = $state(false);
  let controlsVisible = $state(true);
  let musicOn = $state(musicWanted());
  let music: ReturnType<typeof playMusic> | null = null;

  interface Layer {
    photo: PhotoItem;
    src: string;
    key: number;
    /** Where it drifts from and to, and whether it zooms in or out. */
    drift: string;
  }
  let shown = $state<Layer | null>(null);
  let leaving = $state<Layer | null>(null);
  let layerKey = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  /** Bumped on every move, so a slow load can't show a photo late. */
  let generation = 0;
  const loaded = new Map<string, Promise<string>>();

  /** Resolves with the full-size URL once loaded, or the preview's after a while. */
  function preload(photo: PhotoItem): Promise<string> {
    const known = loaded.get(photo.fingerprint);
    if (known) return known;
    const promise = new Promise<string>((resolve) => {
      const full = photoUrl(photo.fingerprint);
      const preview = thumbUrl(photo.fingerprint, "preview");
      const image = new Image();
      const fallback = setTimeout(() => resolve(preview), LOAD_LIMIT_MS);
      image.onload = () => {
        clearTimeout(fallback);
        resolve(full);
      };
      image.onerror = () => {
        clearTimeout(fallback);
        resolve(preview);
      };
      image.src = full;
    });
    loaded.set(photo.fingerprint, promise);
    return promise;
  }

  const DRIFTS = ["drift-a", "drift-b", "drift-c", "drift-d", "drift-e", "drift-f"];

  async function go(to: number) {
    const mine = ++generation;
    clearTimeout(timer);
    if (to < 0) {
      position = -1;
      shown = null;
      leaving = null;
      schedule(TITLE_MS);
      return;
    }
    if (to >= photos.length) {
      position = photos.length;
      leaving = shown;
      shown = null;
      return;
    }
    const photo = photos[to];
    const src = await preload(photo);
    if (mine !== generation) return;
    leaving = shown;
    shown = { photo, src, key: ++layerKey, drift: DRIFTS[(to * 7 + memory.id.length) % DRIFTS.length] };
    position = to;
    // The next one loads while this one shows.
    if (photos[to + 1]) void preload(photos[to + 1]);
    schedule(SLIDE_MS);
  }

  function schedule(ms: number) {
    clearTimeout(timer);
    if (!paused) timer = setTimeout(() => go(position + 1), ms);
  }

  function togglePause() {
    paused = !paused;
    if (paused) clearTimeout(timer);
    else if (position < photos.length) schedule(position < 0 ? TITLE_MS / 2 : SLIDE_MS / 2);
    wake();
  }

  /** Starts the music; without sound (no audio device) the memory plays on silently. */
  function startMusic() {
    try {
      music = playMusic(memory.id);
    } catch {
      music = null;
    }
  }

  function toggleMusic() {
    musicOn = !musicOn;
    setMusicWanted(musicOn);
    if (musicOn && !music) startMusic();
    else music?.setVolume(musicOn);
    wake();
  }

  function restart() {
    paused = false;
    void go(-1);
  }

  /** Shows the controls, and hides them again after a moment of stillness. */
  function wake() {
    controlsVisible = true;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => (controlsVisible = false), 2500);
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") onclose();
    else if (event.key === " ") togglePause();
    else if (event.key === "ArrowRight") void go(Math.min(position + 1, photos.length));
    else if (event.key === "ArrowLeft") void go(Math.max(position - 1, -1));
    else if (event.key === "m" || event.key === "M") toggleMusic();
    else return;
    event.preventDefault();
    event.stopPropagation();
    wake();
  }

  /** Upright photos (and very wide ones) are shown whole, on a blurred copy. */
  const whole = (photo: PhotoItem) => {
    let width = photo.width ?? 4;
    let height = photo.height ?? 3;
    if (photo.edit && photo.edit.rotate % 180 !== 0) [width, height] = [height, width];
    // Crops are in ten-thousandths of the turned picture.
    const crop = photo.edit?.crop;
    if (crop) [width, height] = [width * crop[2], height * crop[3]];
    const ratio = width / Math.max(1, height);
    return ratio < 1.1 || ratio > 2.4;
  };

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
    if (musicOn) startMusic();
    wake();
    photosPage(libraryPath, { kind: "picked", fingerprints: memory.photos }, "dateAsc", 0, memory.photos.length)
      .then((items) => {
        photos = items.filter((item) => !item.video);
        if (photos[0]) void preload(photos[0]);
      })
      .catch(() => (photos = []));
    void go(-1);
    return () => {
      generation++;
      clearTimeout(timer);
      clearTimeout(hideTimer);
      music?.stop();
      if (enteredFullscreen) appWindow.setFullscreen(false).catch(() => {});
    };
  });
</script>

<svelte:window {onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="player"
  class:idle={!controlsVisible}
  role="dialog"
  aria-modal="true"
  aria-label={title}
  tabindex="-1"
  onmousemove={wake}
>
  <!-- The title, over the cover coming into focus. -->
  {#if position === -1}
    <div class="title-card">
      <img class="title-photo" src={thumbUrl(memory.cover, "preview")} alt="" draggable="false" />
      <div class="title-text">
        <h1>{title}</h1>
        <p>{subtitle}</p>
      </div>
    </div>
  {/if}

  {#each [leaving, shown] as layer, i (layer?.key ?? `none-${i}`)}
    {#if layer}
      <div class="layer" class:leaving={layer === leaving}>
        {#if whole(layer.photo)}
          <img class="backdrop" src={thumbUrl(layer.photo.fingerprint, "preview")} alt="" draggable="false" />
        {/if}
        <div class="drift {layer.drift}" class:paused style:animation-duration="{SLIDE_MS + 1600}ms">
          <EditedImage
            src={layer.src}
            edit={layer.photo.edit ?? NO_EDIT}
            width={layer.photo.width}
            height={layer.photo.height}
            fit={whole(layer.photo) ? "contain" : "cover"}
          />
        </div>
      </div>
    {/if}
  {/each}

  {#if position >= photos.length && photos.length > 0}
    <div class="end-card">
      <img class="title-photo" src={thumbUrl(memory.cover, "preview")} alt="" draggable="false" />
      <div class="title-text">
        <h1>{title}</h1>
        <p>{subtitle}</p>
        <div class="end-buttons">
          <button onclick={restart}>↻ {t("Play Again")}</button>
          <button onclick={onshowall}>
            {tn(memory.count, "See the photo", "See all {n} photos")}
          </button>
          <button onclick={onexport}>{t("Save as a Video…")}</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- One segment per photo, as in a story. -->
  <div class="progress" aria-hidden="true">
    {#each photos as photo, i (photo.fingerprint)}
      <span class="segment">
        <span
          class="fill"
          class:done={i < position}
          class:running={i === position}
          class:paused
          style:animation-duration="{SLIDE_MS}ms"
        ></span>
      </span>
    {/each}
  </div>

  <div class="controls">
    <button onclick={() => go(Math.max(position - 1, -1))} aria-label={t("Previous")} title={t("Previous (←)")}>
      <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><path d="M12.5 4 6.5 10l6 6" /></svg>
    </button>
    <button
      class="play"
      onclick={togglePause}
      aria-label={paused ? t("Play") : t("Pause")}
      title={paused ? t("Play (Space)") : t("Pause (Space)")}
    >
      {#if paused}
        <svg viewBox="0 0 20 20" width="22" height="22" aria-hidden="true"><path class="solid" d="M6 4v12l10-6z" /></svg>
      {:else}
        <svg viewBox="0 0 20 20" width="22" height="22" aria-hidden="true">
          <path class="solid" d="M5.5 4h3v12h-3zM11.5 4h3v12h-3z" />
        </svg>
      {/if}
    </button>
    <button
      onclick={() => go(Math.min(position + 1, photos.length))}
      aria-label={t("Next")}
      title={t("Next (→)")}
    >
      <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><path d="m7.5 4 6 6-6 6" /></svg>
    </button>
    <button
      onclick={toggleMusic}
      aria-pressed={musicOn}
      aria-label={musicOn ? t("Mute the music") : t("Play the music")}
      title={musicOn ? t("Mute the music (M)") : t("Play the music (M)")}
    >
      {#if musicOn}
        <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
          <path d="M3 8h3l4-3.5v11L6 12H3zM13 7.5a3.5 3.5 0 0 1 0 5M15 5a7 7 0 0 1 0 10" />
        </svg>
      {:else}
        <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
          <path d="M3 8h3l4-3.5v11L6 12H3zM13 8l4 4M17 8l-4 4" />
        </svg>
      {/if}
    </button>
    <span class="spacer"></span>
    <span class="caption">{title}</span>
    <span class="spacer"></span>
    <button onclick={onexport} aria-label={t("Save as a Video…")} title={t("Save as a Video…")}>
      <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
        <path d="M10 3v9M6.5 6.5 10 3l3.5 3.5M4.5 10v5.5a1.5 1.5 0 0 0 1.5 1.5h8a1.5 1.5 0 0 0 1.5-1.5V10" />
      </svg>
    </button>
    <button onclick={onshowall} title={t("See all the photos of this memory")}>
      {tn(memory.count, "{n} photo", "{n} photos")}
    </button>
    <button onclick={onclose} aria-label={t("Close")} title={t("Close (Esc)")}>
      <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><path d="m5 5 10 10M15 5 5 15" /></svg>
    </button>
  </div>
</div>

<style>
  .player {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #000;
    color: #fff;
    overflow: hidden;
    outline: none;
  }
  .player.idle {
    cursor: none;
  }

  .title-card,
  .end-card,
  .layer {
    position: absolute;
    inset: 0;
  }
  .title-photo {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    animation: focus 3.2s ease-out both;
  }
  .end-card .title-photo {
    animation: none;
    filter: blur(24px) brightness(0.55);
    transform: scale(1.1);
  }
  @keyframes focus {
    from {
      filter: blur(28px) brightness(0.8);
      transform: scale(1.12);
    }
    to {
      filter: blur(0) brightness(0.92);
      transform: scale(1.02);
    }
  }
  .title-text {
    position: absolute;
    inset: auto 0 12% 0;
    text-align: center;
    text-shadow: 0 2px 18px rgb(0 0 0 / 0.55);
    animation: rise 1.6s 0.5s ease-out both;
  }
  .end-card .title-text {
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    animation-delay: 0s;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(16px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  h1 {
    margin: 0;
    font-size: clamp(32px, 5.5vw, 76px);
    font-weight: 700;
    letter-spacing: -0.02em;
    line-height: 1.05;
    padding: 0 6vw;
  }
  .title-text p {
    margin: 12px 0 0;
    font-size: clamp(13px, 1.3vw, 18px);
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    opacity: 0.9;
  }
  .end-buttons {
    display: flex;
    gap: 12px;
    margin-top: 32px;
  }
  .end-buttons button {
    background: rgb(255 255 255 / 0.18);
    color: #fff;
    border: 1px solid rgb(255 255 255 / 0.35);
    border-radius: 999px;
    padding: 10px 22px;
    font-size: 15px;
    backdrop-filter: blur(10px);
  }
  .end-buttons button:hover {
    background: rgb(255 255 255 / 0.3);
  }

  .layer {
    animation: fade-in 1.2s ease both;
  }
  .layer.leaving {
    animation: fade-out 1.2s ease both;
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
  .backdrop {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    filter: blur(40px) brightness(0.5) saturate(1.2);
    transform: scale(1.2);
  }
  .drift {
    position: absolute;
    inset: 0;
    animation-timing-function: linear;
    animation-fill-mode: both;
  }
  .drift.paused,
  .fill.paused {
    animation-play-state: paused;
  }
  .drift :global(*) {
    width: 100%;
    height: 100%;
  }
  .drift-a {
    animation-name: drift-a;
  }
  .drift-b {
    animation-name: drift-b;
  }
  .drift-c {
    animation-name: drift-c;
  }
  .drift-d {
    animation-name: drift-d;
  }
  .drift-e {
    animation-name: drift-e;
  }
  .drift-f {
    animation-name: drift-f;
  }
  @keyframes drift-a {
    from {
      transform: scale(1.02) translate(0, 0);
    }
    to {
      transform: scale(1.14) translate(-2%, -1.5%);
    }
  }
  @keyframes drift-b {
    from {
      transform: scale(1.14) translate(2%, 1%);
    }
    to {
      transform: scale(1.03) translate(0, 0);
    }
  }
  @keyframes drift-c {
    from {
      transform: scale(1.06) translate(-2%, 0);
    }
    to {
      transform: scale(1.12) translate(2%, -1%);
    }
  }
  @keyframes drift-d {
    from {
      transform: scale(1.03) translate(1.5%, -1.5%);
    }
    to {
      transform: scale(1.15) translate(-1%, 1.5%);
    }
  }
  @keyframes drift-e {
    from {
      transform: scale(1.12) translate(0, 2%);
    }
    to {
      transform: scale(1.04) translate(0, -1%);
    }
  }
  @keyframes drift-f {
    from {
      transform: scale(1.04) translate(2%, 1.5%);
    }
    to {
      transform: scale(1.13) translate(-2%, -1%);
    }
  }

  .progress {
    position: absolute;
    top: 14px;
    left: 20px;
    right: 20px;
    display: flex;
    gap: 3px;
    transition: opacity 0.4s;
  }
  .segment {
    flex: 1;
    height: 3px;
    border-radius: 2px;
    background: rgb(255 255 255 / 0.3);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    width: 0;
    background: #fff;
  }
  .fill.done {
    width: 100%;
  }
  .fill.running {
    animation: fill linear both;
  }
  @keyframes fill {
    to {
      width: 100%;
    }
  }

  .controls {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 40px 20px 18px;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.55));
    transition: opacity 0.4s;
  }
  .idle .controls,
  .idle .progress {
    opacity: 0;
  }
  .controls button {
    background: rgb(255 255 255 / 0.12);
    color: #fff;
    border: none;
    border-radius: 999px;
    min-width: 40px;
    height: 40px;
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    backdrop-filter: blur(10px);
  }
  .controls button:hover {
    background: rgb(255 255 255 / 0.25);
  }
  .controls .play {
    width: 48px;
    height: 48px;
  }
  .controls svg path {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .controls svg path.solid {
    fill: currentColor;
    stroke: none;
  }
  .spacer {
    flex: 1;
  }
  .caption {
    font-weight: 600;
    opacity: 0.85;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  @media (prefers-reduced-motion: reduce) {
    .drift,
    .title-photo {
      animation: none;
    }
  }
</style>
