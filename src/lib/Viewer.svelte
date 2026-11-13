<!--
  Full-window photo viewer: arrow keys to move, wheel / double-click / +/- to
  zoom, drag to pan, "i" for the info panel, "f" for full screen, "." to
  favorite, "h" to hide, Esc to close. Videos play in place (space to pause);
  a Live Photo plays its short video with the LIVE button or "l".

  It shows the cached preview at once and swaps in the full-size photo when it
  has loaded, so moving through photos stays fast on a slow network.
-->
<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import {
    photoDetails,
    photoUrl,
    thumbUrl,
    videoUrl,
    type Album,
    type Op,
    type PhotoDetails,
    type PhotoItem,
  } from "./api";
  import { formatBytes, formatDateTime, formatDuration, formatNumber } from "./format";
  import type { PhotoPages } from "./pages.svelte";

  interface Props {
    pages: PhotoPages;
    /** Number of photos in the listing. */
    total: number;
    /** Listing index of the photo on screen. */
    index: number;
    /** Changes whenever the listing was reloaded; the photo is then re-read. */
    revision: number;
    /** False while the library is offline: the organizing buttons are disabled. */
    canEdit: boolean;
    albums: Album[];
    /** Id of the album being viewed, if the listing is an album. */
    currentAlbum: string | null;
    /** True when the listing is "Hidden". */
    showingHidden: boolean;
    /** Records organization changes, then reloads the listing. */
    apply: (ops: Op[]) => Promise<void>;
    onclose: () => void;
  }
  let {
    pages,
    total,
    index = $bindable(),
    revision,
    canEdit,
    albums,
    currentAlbum,
    showingHidden,
    apply,
    onclose,
  }: Props = $props();

  const MAX_SCALE = 10;
  const DOUBLE_CLICK_SCALE = 3;
  /** Don't fetch the full-size photo while the user is flipping through. */
  const FULL_SIZE_DELAY_MS = 200;

  let photo = $state<PhotoItem | undefined>();
  let details = $state<PhotoDetails | null>(null);
  let showInfo = $state(false);
  /** Fingerprint whose full-size image has loaded. */
  let fullSizeReady = $state<string | null>(null);

  // Zoom: `scale` 1 fits the window; `x`/`y` shift the image from the centre.
  let scale = $state(1);
  let x = $state(0);
  let y = $state(0);
  let stage: HTMLDivElement;
  let image = $state<HTMLImageElement>();
  let player = $state<HTMLVideoElement>();
  /** True while a Live Photo's video is playing over its photo. */
  let livePlaying = $state(false);
  /** Fingerprint of a video that the webview could not play. */
  let unplayable = $state<string | null>(null);

  // A new photo starts fitted to the window.
  $effect(() => {
    void index;
    scale = 1;
    x = 0;
    y = 0;
    livePlaying = false;
  });

  // Load the photo for the current index, then its neighbours' previews.
  $effect(() => {
    const wanted = index;
    void revision;
    pages.load(wanted).then((loaded) => {
      if (wanted !== index) return;
      photo = loaded;
      for (const neighbour of [wanted + 1, wanted - 1]) {
        pages.load(neighbour).then((item) => {
          if (item) new Image().src = thumbUrl(item.fingerprint, "preview");
        });
      }
    });
  });

  // Fetch the full-size photo once the user has paused on it.
  $effect(() => {
    const fingerprint = photo?.fingerprint;
    if (!fingerprint || photo?.video || fullSizeReady === fingerprint) return;
    const loader = new Image();
    const timer = setTimeout(() => {
      loader.onload = () => {
        if (photo?.fingerprint === fingerprint) fullSizeReady = fingerprint;
      };
      loader.src = photoUrl(fingerprint);
    }, FULL_SIZE_DELAY_MS);
    return () => {
      clearTimeout(timer);
      loader.onload = null;
      loader.src = "";
    };
  });

  $effect(() => {
    const relPath = photo?.relPath;
    if (!showInfo || !relPath) return;
    photoDetails(pages.libraryPath, relPath)
      .then((found) => {
        if (photo?.relPath === relPath) details = found;
      })
      .catch(() => {});
  });

  const src = $derived(
    photo
      ? fullSizeReady === photo.fingerprint
        ? photoUrl(photo.fingerprint)
        : thumbUrl(photo.fingerprint, "preview")
      : null,
  );
  const fileName = $derived(photo?.relPath.split("/").pop() ?? "");
  const folder = $derived(photo?.relPath.split("/").slice(0, -1).join("/") ?? "");

  function go(step: number) {
    const next = index + step;
    if (next >= 0 && next < total) index = next;
  }

  /** Keeps the image from being dragged further than its own edge. */
  function clampPan() {
    if (!image || !image.naturalWidth) return;
    // Size of the photo on screen at scale 1 (it is fitted into the stage).
    const fit = Math.min(
      stage.clientWidth / image.naturalWidth,
      stage.clientHeight / image.naturalHeight,
    );
    const maxX = Math.max(0, (image.naturalWidth * fit * scale - stage.clientWidth) / 2);
    const maxY = Math.max(0, (image.naturalHeight * fit * scale - stage.clientHeight) / 2);
    x = Math.min(maxX, Math.max(-maxX, x));
    y = Math.min(maxY, Math.max(-maxY, y));
  }

  /** Zooms to `target`, keeping the point under (clientX, clientY) in place. */
  function zoomTo(target: number, clientX?: number, clientY?: number) {
    if (photo?.video) return; // videos stay fitted to the window
    const next = Math.min(MAX_SCALE, Math.max(1, target));
    const box = stage.getBoundingClientRect();
    const px = (clientX ?? box.left + box.width / 2) - (box.left + box.width / 2);
    const py = (clientY ?? box.top + box.height / 2) - (box.top + box.height / 2);
    const ratio = next / scale;
    x = px - (px - x) * ratio;
    y = py - (py - y) * ratio;
    scale = next;
    clampPan();
  }

  function onwheel(event: WheelEvent) {
    event.preventDefault();
    zoomTo(scale * Math.exp(-event.deltaY * 0.0015), event.clientX, event.clientY);
  }

  function ondblclick(event: MouseEvent) {
    zoomTo(scale > 1 ? 1 : DOUBLE_CLICK_SCALE, event.clientX, event.clientY);
  }

  let drag: { pointerId: number; startX: number; startY: number; x: number; y: number } | null =
    null;

  function onpointerdown(event: PointerEvent) {
    if (scale <= 1 || event.button !== 0) return;
    drag = { pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, x, y };
    stage.setPointerCapture(event.pointerId);
  }

  function onpointermove(event: PointerEvent) {
    if (!drag || drag.pointerId !== event.pointerId) return;
    x = drag.x + event.clientX - drag.startX;
    y = drag.y + event.clientY - drag.startY;
    clampPan();
  }

  function onpointerup(event: PointerEvent) {
    if (drag?.pointerId === event.pointerId) drag = null;
  }

  let enteredFullscreen = false;
  async function toggleFullscreen() {
    const appWindow = getCurrentWindow();
    const next = !(await appWindow.isFullscreen());
    await appWindow.setFullscreen(next);
    enteredFullscreen = next;
  }

  function close() {
    if (enteredFullscreen) getCurrentWindow().setFullscreen(false).catch(() => {});
    onclose();
  }

  function toggleFavorite() {
    if (photo && canEdit) apply([{ op: "fav", photo: photo.fingerprint, on: !photo.favorite }]);
  }

  function toggleHidden() {
    if (photo && canEdit) apply([{ op: "hide", photo: photo.fingerprint, on: !showingHidden }]);
  }

  function addToAlbum(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const album = select.value;
    select.value = "";
    if (photo && album) apply([{ op: "album_member", album, photo: photo.fingerprint, on: true }]);
  }

  function removeFromAlbum() {
    if (photo && currentAlbum) {
      apply([{ op: "album_member", album: currentAlbum, photo: photo.fingerprint, on: false }]);
    }
  }

  function togglePlay() {
    if (photo?.video) {
      if (player?.paused) player.play().catch(() => {});
      else player?.pause();
    } else if (photo?.liveVideo) {
      livePlaying = !livePlaying;
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.target instanceof HTMLSelectElement && event.key !== "Escape") return;
    switch (event.key) {
      case ".":
        toggleFavorite();
        break;
      case "h":
        toggleHidden();
        break;
      case "ArrowLeft":
        go(-1);
        break;
      case "ArrowRight":
        go(1);
        break;
      case "Home":
        index = 0;
        break;
      case "End":
        index = total - 1;
        break;
      case "Escape":
        close();
        break;
      case "i":
        showInfo = !showInfo;
        break;
      case "f":
        toggleFullscreen();
        break;
      case " ":
      case "l":
        togglePlay();
        break;
      case "+":
      case "=":
        zoomTo(scale * 1.5);
        break;
      case "-":
        zoomTo(scale / 1.5);
        break;
      case "0":
        zoomTo(1);
        break;
      default:
        return;
    }
    event.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="viewer" role="dialog" aria-modal="true" aria-label="Photo viewer">
  <div class="main">
    <div class="bar">
      <button onclick={close} title="Close (Esc)">‹ Back</button>
      <div class="caption">
        <strong>{fileName}</strong>
        {#if photo}<span>{formatDateTime(photo.sortDate)}</span>{/if}
      </div>
      <span class="position">{formatNumber(index + 1)} of {formatNumber(total)}</span>
      <button
        onclick={toggleFavorite}
        disabled={!canEdit || !photo}
        aria-pressed={photo?.favorite ?? false}
        title={photo?.favorite ? "Remove from favorites (.)" : "Add to favorites (.)"}
      >
        {photo?.favorite ? "♥" : "♡"}
      </button>
      {#if albums.length > 0}
        <select onchange={addToAlbum} disabled={!canEdit || !photo} title="Add to album">
          <option value="">Add to album…</option>
          {#each albums as album (album.id)}
            <option value={album.id}>{album.name}</option>
          {/each}
        </select>
      {/if}
      {#if currentAlbum}
        <button onclick={removeFromAlbum} disabled={!canEdit || !photo}>Remove from album</button>
      {/if}
      <button onclick={toggleHidden} disabled={!canEdit || !photo} title="{showingHidden ? 'Unhide' : 'Hide'} (H)">
        {showingHidden ? "Unhide" : "Hide"}
      </button>
      {#if photo?.liveVideo}
        <button onclick={togglePlay} aria-pressed={livePlaying} title="Play the Live Photo (L)">
          LIVE
        </button>
      {/if}
      <button onclick={() => zoomTo(scale / 1.5)} disabled={scale <= 1} title="Zoom out (-)">−</button>
      <button
        onclick={() => zoomTo(scale * 1.5)}
        disabled={scale >= MAX_SCALE || photo?.video}
        title="Zoom in (+)">+</button
      >
      <button onclick={toggleFullscreen} title="Full screen (F)">⛶</button>
      <button onclick={() => (showInfo = !showInfo)} aria-pressed={showInfo} title="Info (I)">ⓘ</button>
    </div>

    <div
      class="stage"
      class:zoomed={scale > 1}
      bind:this={stage}
      {onwheel}
      {ondblclick}
      {onpointerdown}
      {onpointermove}
      {onpointerup}
      onpointercancel={onpointerup}
      role="presentation"
    >
      {#if photo?.video}
        <!-- Keyed, so that moving to another video starts a fresh player. -->
        {#key photo.fingerprint}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video
            bind:this={player}
            src={videoUrl(photo.fingerprint)}
            poster={thumbUrl(photo.fingerprint, "preview")}
            controls
            autoplay
            playsinline
            onerror={() => (unplayable = photo?.fingerprint ?? null)}
          ></video>
        {/key}
        {#if unplayable === photo.fingerprint}
          <p class="notice">This video can't be played here.</p>
        {/if}
      {:else if src}
        <img
          bind:this={image}
          {src}
          alt={fileName}
          draggable="false"
          style:transform="translate({x}px, {y}px) scale({scale})"
        />
        {#if livePlaying && photo?.liveVideo}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video
            class="live"
            src={videoUrl(photo.liveVideo)}
            autoplay
            playsinline
            onended={() => (livePlaying = false)}
            onerror={() => (livePlaying = false)}
            style:transform="translate({x}px, {y}px) scale({scale})"
          ></video>
        {/if}
      {/if}

      {#if index > 0}
        <button class="nav previous" onclick={() => go(-1)} aria-label="Previous photo">‹</button>
      {/if}
      {#if index < total - 1}
        <button class="nav next" onclick={() => go(1)} aria-label="Next photo">›</button>
      {/if}
    </div>
  </div>

  {#if showInfo}
    <aside>
      <h2>Info</h2>
      <dl>
        <dt>Name</dt>
        <dd>{fileName}</dd>
        {#if folder}
          <dt>Folder</dt>
          <dd>{folder}</dd>
        {/if}
        {#if details && details.relPath === photo?.relPath}
          <dt>Date</dt>
          <dd>
            {formatDateTime(details.sortDate)}
            {#if !details.dateFromExif}
              <small>File date. The photo has no date taken.</small>
            {/if}
          </dd>
          {#if details.width && details.height}
            <dt>Dimensions</dt>
            <dd>
              {formatNumber(details.width)} × {formatNumber(details.height)}
              <small>{((details.width * details.height) / 1e6).toFixed(1)} megapixels</small>
            </dd>
          {/if}
          <dt>Size</dt>
          <dd>{formatBytes(details.size)}</dd>
          {#if details.durationMs !== null}
            <dt>Length</dt>
            <dd>{formatDuration(details.durationMs)}</dd>
          {/if}
          {#if details.liveVideo}
            <dt>Kind</dt>
            <dd>Live Photo</dd>
          {/if}
          {#if details.cameraMake || details.cameraModel}
            <dt>Camera</dt>
            <dd>{[details.cameraMake, details.cameraModel].filter(Boolean).join(" ")}</dd>
          {/if}
          {#if details.latitude !== null && details.longitude !== null}
            <dt>Location</dt>
            <dd>{details.latitude.toFixed(5)}, {details.longitude.toFixed(5)}</dd>
          {/if}
        {/if}
      </dl>
    </aside>
  {/if}
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: flex;
    background: #000;
    color: #f2f2f4;
  }

  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    background: #1c1c1e;
  }

  .bar button {
    min-width: 32px;
    padding: 4px 10px;
    border: none;
    border-radius: 6px;
    background: none;
    font-size: 15px;
    cursor: pointer;
  }

  .bar button:hover:not(:disabled),
  .bar button[aria-pressed="true"] {
    background: #3a3a3d;
  }

  .bar select {
    max-width: 150px;
    padding: 4px 6px;
    border: 1px solid #3d3d42;
    border-radius: 6px;
    background: #1c1c1e;
    color: inherit;
    font: inherit;
  }

  .bar button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .caption {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 0 8px;
    white-space: nowrap;
    overflow: hidden;
  }

  .caption span,
  .position {
    font-size: 12px;
    color: #a0a0a8;
  }

  .position {
    padding: 0 8px;
    white-space: nowrap;
  }

  .stage {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: hidden;
    touch-action: none;
  }

  .stage.zoomed {
    cursor: grab;
  }

  .stage.zoomed:active {
    cursor: grabbing;
  }

  /* The image box fills the stage and the photo is fitted inside it, so the
     preview and the full-size photo occupy exactly the same place. */
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    will-change: transform;
  }

  video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #000;
  }

  /* A Live Photo's video covers its photo exactly while it plays. */
  video.live {
    background: none;
    pointer-events: none;
  }

  .notice {
    position: absolute;
    left: 50%;
    bottom: 72px;
    transform: translateX(-50%);
    margin: 0;
    padding: 6px 12px;
    border-radius: 6px;
    background: rgb(0 0 0 / 0.7);
    font-size: 13px;
  }

  .nav {
    position: absolute;
    top: 50%;
    width: 44px;
    height: 64px;
    margin-top: -32px;
    border: none;
    border-radius: 8px;
    background: rgb(0 0 0 / 0.35);
    font-size: 30px;
    line-height: 1;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .stage:hover .nav,
  .nav:focus-visible {
    opacity: 1;
  }

  .previous {
    left: 12px;
  }

  .next {
    right: 12px;
  }

  aside {
    width: 300px;
    padding: 16px;
    overflow-y: auto;
    background: #1c1c1e;
    border-left: 1px solid #3d3d42;
    user-select: text;
    -webkit-user-select: text;
  }

  aside h2 {
    margin: 0 0 12px;
    font-size: 15px;
  }

  dl {
    margin: 0;
  }

  dt {
    margin-top: 12px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: #a0a0a8;
  }

  dd {
    margin: 2px 0 0;
    overflow-wrap: anywhere;
  }

  small {
    display: block;
    font-size: 12px;
    color: #a0a0a8;
  }
</style>
