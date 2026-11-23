<!--
  Full-window photo viewer: arrow keys to move, wheel / double-click / +/- to
  zoom, drag to pan, "i" for the info panel, "f" for full screen, "." to
  favorite, 1 to 5 to rate, 6 to 9 for a colour label, "h" to hide, Esc to close. Videos play in place (space to pause);
  a Live Photo plays its short video with the LIVE button or "l". "e" opens
  the editing panel: rotate, mirror, crop and adjust, without changing the file.
  The magic wand (✨) enhances the photo in one click, also without changing it.

  It shows the cached preview at once and swaps in the full-size photo when it
  has loaded, so moving through photos stays fast on a slow network.
-->
<script lang="ts">
  import { errorText, t, tn } from "./i18n";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import {
    applyEditToOriginal,
    autoEnhance,
    photoDetails,
    photoShooting,
    photoUrl,
    thumbUrl,
    videoUrl,
    type Album,
    type Op,
    type PhotoDetails,
    type PhotoItem,
    type Shooting,
  } from "./api";
  import CropBox from "./CropBox.svelte";
  import { editedSize, isNeutral, NO_EDIT, type Edit } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatBytes, formatDateTime, formatDuration, formatNumber, formatShooting, rawPairLabel } from "./format";
  import type { PhotoPages } from "./pages.svelte";
  import { REVEAL_LABEL, revealPhoto } from "./reveal";
  import { LABELS, labelForKey, type LabelColor } from "./labels";

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
  /** Lens, aperture, shutter speed…: read from the file, after the details. */
  let shooting = $state<Shooting | null>(null);
  let showInfo = $state(false);

  // --- The filmstrip under the photo, as in Windows Photos ---------------------

  /** Shown unless switched off (T), remembered. */
  let showStrip = $state(localStorage.getItem("photolib.filmstrip") !== "off");
  $effect(() => localStorage.setItem("photolib.filmstrip", showStrip ? "on" : "off"));
  /** Thumbnails on each side of the current photo. */
  const STRIP_REACH = 40;
  const stripFirst = $derived(Math.max(0, index - STRIP_REACH));
  const stripLast = $derived(Math.min(total - 1, index + STRIP_REACH));
  const stripIndices = $derived(
    Array.from({ length: Math.max(0, stripLast - stripFirst + 1) }, (_, i) => stripFirst + i),
  );
  let strip = $state<HTMLDivElement>();
  /** The strip the centering was last done in: a new one jumps, no glide. */
  let centeredIn: HTMLDivElement | undefined;
  $effect(() => {
    if (showStrip) pages.ensure(stripFirst, stripLast);
  });
  // Keeps the current photo's thumbnail in the middle of the strip.
  $effect(() => {
    void index;
    void stripIndices;
    const current = strip?.querySelector<HTMLElement>(".thumb.current");
    if (!strip || !current) return;
    strip.scrollTo({
      left: current.offsetLeft - (strip.clientWidth - current.offsetWidth) / 2,
      behavior: centeredIn === strip ? "smooth" : "instant",
    });
    centeredIn = strip;
  });
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
    editing = false;
  });

  // --- Editing ---------------------------------------------------------------

  const ASPECTS = [
    ["free", t("Free"), null],
    ["original", t("Original"), 0],
    ["1:1", t("Square"), 1],
    ["4:3", "4:3", 4 / 3],
    ["3:2", "3:2", 3 / 2],
    ["16:9", "16:9", 16 / 9],
  ] as const;

  let editing = $state(false);
  let draft = $state<Edit>({ ...NO_EDIT });
  let aspect = $state<(typeof ASPECTS)[number][0]>("free");
  let stageWidth = $state(0);
  let stageHeight = $state(0);
  /** The photo's proportions when the index does not know its size. */
  let probed = $state<{ fingerprint: string; width: number; height: number } | null>(null);

  $effect(() => {
    const current = photo;
    if (!current || current.video || (current.width && current.height)) return;
    const probe = new Image();
    probe.onload = () => {
      probed = {
        fingerprint: current.fingerprint,
        width: probe.naturalWidth,
        height: probe.naturalHeight,
      };
    };
    probe.src = thumbUrl(current.fingerprint, "preview");
  });

  /** The photo's upright size (only its proportions matter here). */
  const natural = $derived.by(() => {
    if (!photo) return null;
    if (photo.width && photo.height) return { width: photo.width, height: photo.height };
    return probed?.fingerprint === photo.fingerprint ? probed : null;
  });

  /** True while "Show original" is held down in the editing panel. */
  let comparing = $state(false);
  /** True while the magic wand is working the photo out. */
  let enhancing = $state(false);
  /** A problem to show over the photo, such as the wand failing. */
  let notice = $state<string | null>(null);
  $effect(() => {
    void index;
    notice = null;
  });

  /** What is drawn: while editing, the draft without its crop (the crop box shows that). */
  const shownEdit = $derived<Edit | null>(
    comparing
      ? { ...NO_EDIT }
      : editing
        ? { ...draft, crop: null }
        : (photo?.edit ?? null),
  );

  /** Where the whole (rotated) picture is on the stage while editing. */
  const editArea = $derived.by(() => {
    if (!editing || !natural || stageWidth === 0 || stageHeight === 0) return null;
    const [width, height] = editedSize({ ...draft, crop: null }, natural.width, natural.height);
    const fit = Math.min(stageWidth / width, stageHeight / height);
    return {
      width: width * fit,
      height: height * fit,
      left: (stageWidth - width * fit) / 2,
      top: (stageHeight - height * fit) / 2,
    };
  });

  /** The ratio the crop box keeps, in the box's own fractions of the picture. */
  const cropRatio = $derived.by(() => {
    const wanted = ASPECTS.find(([id]) => id === aspect)?.[2] ?? null;
    if (wanted === null || !editArea) return null;
    return wanted === 0 ? 1 : (wanted * editArea.height) / editArea.width;
  });

  const cropFractions = $derived<[number, number, number, number]>(
    draft.crop ? (draft.crop.map((value) => value / 10_000) as [number, number, number, number]) : [0, 0, 1, 1],
  );

  function setCrop([left, top, width, height]: [number, number, number, number]) {
    const whole = left <= 0.0005 && top <= 0.0005 && width >= 0.999 && height >= 0.999;
    const units = (value: number) => Math.round(value * 10_000);
    // Rounding must not push the box past the edge.
    const l = units(left);
    const t = units(top);
    draft.crop = whole
      ? null
      : [l, t, Math.min(units(width), 10_000 - l), Math.min(units(height), 10_000 - t)];
  }

  function startEditing() {
    if (!photo || photo.video || !canEdit) return;
    draft = { ...NO_EDIT, ...(photo.edit ?? {}) };
    confirmingOriginal = false;
    aspect = "free";
    showInfo = false;
    scale = 1;
    x = 0;
    y = 0;
    editing = true;
  }

  /** A new shape: the largest centered crop of that shape. */
  function chooseAspect(id: typeof aspect) {
    aspect = id;
    const ratio = cropRatio;
    if (ratio === null) return;
    const width = ratio >= 1 ? 1 : ratio;
    const height = ratio >= 1 ? 1 / ratio : 1;
    setCrop([(1 - width) / 2, (1 - height) / 2, width, height]);
  }

  /** Turning or mirroring starts again from the whole picture. */
  function turn(degrees: number) {
    draft.rotate = (draft.rotate + degrees + 360) % 360;
    draft.crop = null;
    aspect = "free";
  }

  /** The wand's corrections for the photo on screen, or null if it failed. */
  async function enhanceFor(fingerprint: string): Promise<Edit["enhance"]> {
    enhancing = true;
    try {
      return await autoEnhance(fingerprint);
    } catch (e) {
      notice = t("Couldn't enhance the photo: {detail}", { detail: errorText(e) });
      return null;
    } finally {
      enhancing = false;
    }
  }

  /** The magic wand outside the editing panel: enhances, or takes it away. */
  async function toggleEnhance() {
    const current = photo;
    if (!current || current.video || !canEdit || enhancing) return;
    const edit: Edit = { ...NO_EDIT, ...(current.edit ?? {}) };
    if (edit.enhance) {
      edit.enhance = null;
    } else {
      edit.enhance = await enhanceFor(current.fingerprint);
      if (!edit.enhance) return;
    }
    await apply([{ op: "edit", photo: current.fingerprint, edit: isNeutral(edit) ? null : edit }]);
    flash(edit.enhance ? t("✨ Enhanced") : t("Enhancement removed"));
  }

  /** Shows `text` over the photo for a moment. */
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  function flash(text: string) {
    notice = text;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => {
      if (notice === text) notice = null;
    }, 1500);
  }

  /** The magic wand in the editing panel: changes the draft only. */
  async function toggleDraftEnhance() {
    if (!photo || enhancing) return;
    if (draft.enhance) {
      draft.enhance = null;
      return;
    }
    const fingerprint = photo.fingerprint;
    const enhance = await enhanceFor(fingerprint);
    if (enhance && editing && photo?.fingerprint === fingerprint) draft.enhance = enhance;
  }

  // --- Apply to original: draws the edit into the file, after a warning -------

  let confirmingOriginal = $state(false);
  let rewriting = $state(false);
  /** Only JPEG files are rewritten (see original.rs). */
  const rewritable = $derived(!!photo && /\.jpe?g$/i.test(photo.relPath));

  async function applyToOriginal() {
    const current = photo;
    if (!current || rewriting || isNeutral(draft)) return;
    rewriting = true;
    try {
      await applyEditToOriginal(
        pages.libraryPath,
        current.relPath,
        current.fingerprint,
        $state.snapshot(draft),
      );
      editing = false;
      // The file has new content, so a new fingerprint: list it again.
      await apply([]);
    } catch (e) {
      notice = t("Couldn't change the file: {detail}", { detail: errorText(e) });
    } finally {
      rewriting = false;
      confirmingOriginal = false;
    }
  }

  async function saveEdit() {
    if (!photo) return;
    const edit = isNeutral(draft) ? null : $state.snapshot(draft);
    editing = false;
    await apply([{ op: "edit", photo: photo.fingerprint, edit }]);
  }

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

  $effect(() => {
    const relPath = photo?.relPath;
    shooting = null;
    if (!showInfo || !relPath) return;
    photoShooting(pages.libraryPath, relPath)
      .then((found) => {
        if (photo?.relPath === relPath) shooting = found;
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
    // The size of what is shown: the edited picture, or the image itself.
    let size: [number, number] | null = null;
    if (shownEdit && natural) size = editedSize(shownEdit, natural.width, natural.height);
    else if (image?.naturalWidth) size = [image.naturalWidth, image.naturalHeight];
    if (!size) return;
    // Size of the photo on screen at scale 1 (it is fitted into the stage).
    const fit = Math.min(stage.clientWidth / size[0], stage.clientHeight / size[1]);
    const maxX = Math.max(0, (size[0] * fit * scale - stage.clientWidth) / 2);
    const maxY = Math.max(0, (size[1] * fit * scale - stage.clientHeight) / 2);
    x = Math.min(maxX, Math.max(-maxX, x));
    y = Math.min(maxY, Math.max(-maxY, y));
  }

  /** Zooms to `target`, keeping the point under (clientX, clientY) in place. */
  function zoomTo(target: number, clientX?: number, clientY?: number) {
    if (photo?.video || editing) return; // these stay fitted to the window
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

  /** Gives the photo `stars`, or takes its rating away if it has them already. */
  function rate(stars: number) {
    if (photo && canEdit) {
      apply([{ op: "rating", photo: photo.fingerprint, stars: photo.rating === stars ? 0 : stars }]);
    }
  }

  /** Gives the photo a label, or takes it away if it has that one already. */
  function setLabel(color: LabelColor) {
    if (photo && canEdit) {
      apply([{ op: "label", photo: photo.fingerprint, color: photo.label === color ? "" : color }]);
    }
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
    if (editing) {
      // Only Escape (cancel) and Enter (save) while editing.
      if (event.key === "Escape") editing = false;
      else if (event.key === "Enter" && !(event.target instanceof HTMLButtonElement)) saveEdit();
      else return;
      event.preventDefault();
      return;
    }
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
      case "t":
        showStrip = !showStrip;
        break;
      case "f":
        toggleFullscreen();
        break;
      case "e":
        startEditing();
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
      case "1":
      case "2":
      case "3":
      case "4":
      case "5":
        rate(Number(event.key));
        break;
      case "6":
      case "7":
      case "8":
      case "9":
        setLabel(labelForKey(event.key)!);
        break;
      default:
        return;
    }
    event.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="viewer" role="dialog" aria-modal="true" aria-label={t("Photo viewer")}>
  <div class="main">
    <div class="bar">
      <button onclick={close} title={t("Close (Esc)")}>‹ {t("Back")}</button>
      <div class="caption">
        <strong>{fileName}</strong>
        {#if photo}<span>{formatDateTime(photo.sortDate)}</span>{/if}
      </div>
      <span class="position">{t("{index} of {count}", { index: index + 1, count: total })}</span>
      <button
        onclick={toggleFavorite}
        disabled={!canEdit || !photo}
        aria-pressed={photo?.favorite ?? false}
        title={photo?.favorite ? t("Remove from favorites (.)") : t("Add to favorites (.)")}
      >
        {photo?.favorite ? "♥" : "♡"}
      </button>
      {#if albums.length > 0}
        <select onchange={addToAlbum} disabled={!canEdit || !photo} title={t("Add to album")}>
          <option value="">{t("Add to album…")}</option>
          {#each albums as album (album.id)}
            <option value={album.id}>{album.name}</option>
          {/each}
        </select>
      {/if}
      {#if currentAlbum}
        <button onclick={removeFromAlbum} disabled={!canEdit || !photo}>{t("Remove from album")}</button>
      {/if}
      <button onclick={toggleHidden} disabled={!canEdit || !photo} title="{showingHidden ? t('Unhide') : t('Hide')} (H)">
        {showingHidden ? t("Unhide") : t("Hide")}
      </button>
      <button
        class="wand"
        class:busy={enhancing}
        onclick={toggleEnhance}
        disabled={!canEdit || !photo || photo.video || editing || enhancing}
        aria-pressed={!!photo?.edit?.enhance}
        title={photo?.edit?.enhance ? t("Enhanced: click to remove") : t("Enhance")}
      >
        ✨
      </button>
      <button
        onclick={() => (editing ? (editing = false) : startEditing())}
        disabled={!canEdit || !photo || photo.video}
        aria-pressed={editing}
        title="{t('Edit')} (E)"
      >
        {t("Edit")}
      </button>
      {#if photo?.liveVideo}
        <button onclick={togglePlay} aria-pressed={livePlaying} title={t("Play the Live Photo (L)")}>
          LIVE
        </button>
      {/if}
      <button onclick={() => zoomTo(scale / 1.5)} disabled={scale <= 1} title={t("Zoom out (-)")}>−</button>
      <button
        onclick={() => zoomTo(scale * 1.5)}
        disabled={scale >= MAX_SCALE || photo?.video}
        title={t("Zoom in (+)")}>+</button
      >
      <button onclick={toggleFullscreen} title={t("Full screen (F)")}>⛶</button>
      <button onclick={() => (showStrip = !showStrip)} aria-pressed={showStrip} title={t("Filmstrip (T)")}>
        ▭▭▭
      </button>
      <button onclick={() => (showInfo = !showInfo)} aria-pressed={showInfo} title={t("Info (I)")}>ⓘ</button>
    </div>

    <div
      class="stage"
      class:zoomed={scale > 1}
      bind:this={stage}
      bind:clientWidth={stageWidth}
      bind:clientHeight={stageHeight}
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
          <p class="notice">{t("This video can't be played here.")}</p>
        {/if}
      {:else if src && shownEdit}
        <div class="edited" style:transform="translate({x}px, {y}px) scale({scale})">
          <EditedImage
            {src}
            alt={fileName}
            edit={shownEdit}
            width={natural?.width ?? null}
            height={natural?.height ?? null}
            fit="contain"
          />
        </div>
        {#if editArea}
          <div
            class="crop-area"
            style:left="{editArea.left}px"
            style:top="{editArea.top}px"
            style:width="{editArea.width}px"
            style:height="{editArea.height}px"
          >
            <CropBox crop={cropFractions} ratio={cropRatio} onchange={setCrop} />
          </div>
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
      {#if notice}
        <p class="notice">{notice}</p>
      {/if}

      {#if index > 0 && !editing}
        <button class="nav previous" onclick={() => go(-1)} aria-label={t("Previous photo")}>‹</button>
      {/if}
      {#if index < total - 1}
        <button class="nav next" onclick={() => go(1)} aria-label={t("Next photo")}>›</button>
      {/if}
    </div>

    {#if showStrip && !editing && total > 1}
      <div class="strip" bind:this={strip}>
        {#each stripIndices as stripIndex (stripIndex)}
          {@const item = pages.get(stripIndex)}
          <button
            class="thumb"
            class:current={stripIndex === index}
            onclick={() => (index = stripIndex)}
            aria-label={t("Photo {n}", { n: stripIndex + 1 })}
            aria-current={stripIndex === index}
          >
            {#if item}
              <img src={thumbUrl(item.fingerprint, "grid")} alt="" draggable="false" loading="lazy" />
            {/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>

  {#if editing}
    <aside class="edit">
      <h2>{t("Edit")}</h2>
      <p class="note">{t("The original file is not changed.")}</p>

      <h3>{t("Turn")}</h3>
      <div class="buttons">
        <button onclick={() => turn(-90)} title={t("Turn left")}>⟲ {t("Left")}</button>
        <button onclick={() => turn(90)} title={t("Turn right")}>⟳ {t("Right")}</button>
        <button
          onclick={() => {
            draft.flip = !draft.flip;
            draft.crop = null;
          }}
          aria-pressed={draft.flip}
        >
          {t("Mirror")}
        </button>
      </div>

      <h3>{t("Crop")}</h3>
      <p class="note">{t("Drag the box or its corners on the photo.")}</p>
      <div class="buttons">
        {#each ASPECTS as [id, label] (id)}
          <button onclick={() => chooseAspect(id)} aria-pressed={aspect === id}>{label}</button>
        {/each}
      </div>

      <h3>{t("Enhance")}</h3>
      <div class="buttons">
        <button onclick={toggleDraftEnhance} aria-pressed={!!draft.enhance} disabled={enhancing}>
          ✨ {t("Auto")}
        </button>
        <button
          onpointerdown={() => (comparing = true)}
          onpointerup={() => (comparing = false)}
          onpointerleave={() => (comparing = false)}
          onpointercancel={() => (comparing = false)}
          title={t("Hold to see the original")}
        >
          {t("Show original")}
        </button>
      </div>

      <h3>{t("Adjust")}</h3>
      {#each [["brightness", t("Brightness")], ["contrast", t("Contrast")], ["saturation", t("Color")]] as const as [key, label] (key)}
        <label>
          <span>{label}</span>
          <input type="range" min="-100" max="100" step="5" bind:value={draft[key]} />
          <output>{draft[key]}</output>
        </label>
      {/each}

      <div class="buttons end">
        <button
          onclick={() => {
            draft = { ...NO_EDIT };
            aspect = "free";
          }}
          disabled={isNeutral(draft)}
        >
          {t("Revert to original")}
        </button>
        <span class="spacer"></span>
        <button onclick={() => (editing = false)}>{t("Cancel")}</button>
        <button class="primary" onclick={saveEdit}>{t("Done")}</button>
      </div>

      <h3>{t("Original file")}</h3>
      {#if confirmingOriginal}
        <div class="warning" role="alert">
          <p>
            {t("This replaces the file “{name}” with the edited photo. It can't be undone, and other apps will see the change.", { name: fileName })}
          </p>
          {#if photo?.rawFile}
            <p>{t("Its RAW file is not changed.")}</p>
          {/if}
          <div class="buttons">
            <button onclick={() => (confirmingOriginal = false)} disabled={rewriting}>{t("Cancel")}</button>
            <button class="danger" onclick={applyToOriginal} disabled={rewriting}>
              {rewriting ? t("Replacing…") : t("Replace the file")}
            </button>
          </div>
        </div>
      {:else}
        <div class="buttons">
          <button onclick={() => (confirmingOriginal = true)} disabled={!rewritable || isNeutral(draft)}>
            {t("Apply to original…")}
          </button>
        </div>
        <p class="note">
          {rewritable ? t("Writes the edit into the file itself.") : t("Only JPEG photos can be changed.")}
        </p>
      {/if}
    </aside>
  {:else if showInfo}
    <aside>
      <h2>{t("Info")}</h2>
      <dl>
        <dt>{t("Name")}</dt>
        <dd>{fileName}</dd>
        {#if folder}
          <dt>{t("Folder")}</dt>
          <dd>{folder}</dd>
        {/if}
        {#if photo}
          <dt>{t("Rating")}</dt>
          <dd class="stars">
            {#each [1, 2, 3, 4, 5] as stars (stars)}
              <button
                class:on={photo.rating >= stars}
                disabled={!canEdit}
                onclick={() => rate(stars)}
                title="{tn(stars, '{n} star', '{n} stars')} ({stars})"
                aria-label={tn(stars, "{n} star", "{n} stars")}
              >
                ★
              </button>
            {/each}
          </dd>
          <dt>{t("Label")}</dt>
          <dd class="labels">
            {#each LABELS as label (label.color)}
              <button
                class:on={photo.label === label.color}
                style:background={label.css}
                disabled={!canEdit}
                onclick={() => setLabel(label.color)}
                title={label.key ? `${label.name} (${label.key})` : label.name}
                aria-label={label.name}
                aria-pressed={photo.label === label.color}
              ></button>
            {/each}
          </dd>
          <dd>
            <button
              class="reveal"
              onclick={() => photo && revealPhoto(pages.libraryPath, photo.relPath).catch(() => {})}
            >
              {REVEAL_LABEL}
            </button>
          </dd>
        {/if}
        {#if details && details.relPath === photo?.relPath}
          <dt>{t("Date")}</dt>
          <dd>
            {formatDateTime(details.sortDate)}
            {#if !details.dateFromExif}
              <small>{t("File date. The photo has no date taken.")}</small>
            {/if}
          </dd>
          {#if details.width && details.height}
            <dt>{t("Dimensions")}</dt>
            <dd>
              {formatNumber(details.width)} × {formatNumber(details.height)}
              <small>{t("{n} megapixels", { n: Number(((details.width * details.height) / 1e6).toFixed(1)) })}</small>
            </dd>
          {/if}
          <dt>{t("Size")}</dt>
          <dd>{formatBytes(details.size)}</dd>
          {#if details.durationMs !== null}
            <dt>{t("Length")}</dt>
            <dd>{formatDuration(details.durationMs)}</dd>
          {/if}
          {#if details.liveVideo}
            <dt>{t("Kind")}</dt>
            <dd>Live Photo</dd>
          {/if}
          {#if details.rawPath}
            <dt>{t("Kind")}</dt>
            <dd>
              {rawPairLabel(details.relPath)}
              <small>{details.rawPath.slice(details.rawPath.lastIndexOf("/") + 1)}</small>
            </dd>
          {/if}
          {#if details.cameraMake || details.cameraModel}
            <dt>{t("Camera")}</dt>
            <dd>{[details.cameraMake, details.cameraModel].filter(Boolean).join(" ")}</dd>
          {/if}
          {#if shooting}
            {#if shooting.lens}
              <dt>{t("Lens")}</dt>
              <dd>{shooting.lens}</dd>
            {/if}
            {@const settings = formatShooting(shooting)}
            {#if settings.length > 0 || shooting.flash}
              <dt>{t("Exposure")}</dt>
              <dd class="shot">
                {#each settings as part (part)}<span>{part}</span>{/each}
                {#if shooting.flash}<span>{t("Flash fired")}</span>{/if}
              </dd>
            {/if}
            {#if shooting.software}
              <dt>{t("Software")}</dt>
              <dd>{shooting.software}</dd>
            {/if}
          {/if}
          {#if details.latitude !== null && details.longitude !== null}
            <dt>{t("Location")}</dt>
            <dd>
              {#if details.place}{details.place}<br />{/if}
              <small>{details.latitude.toFixed(5)}, {details.longitude.toFixed(5)}</small>
            </dd>
          {/if}
        {/if}
      </dl>
    </aside>
  {/if}
</div>

<style>
  .shot {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
  }

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

  /* The wand is grey when off and golden when on, so its state shows at a
     glance (the favorite button changes shape instead). */
  .bar button.wand {
    filter: grayscale(1);
    opacity: 0.6;
  }

  .bar button.wand[aria-pressed="true"] {
    filter: none;
    opacity: 1;
    background: rgb(255 204 0 / 0.22);
    box-shadow: inset 0 0 0 1px rgb(255 204 0 / 0.7);
  }

  .bar button.wand.busy {
    animation: pulse 0.8s ease-in-out infinite alternate;
  }

  @keyframes pulse {
    to {
      opacity: 0.25;
    }
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

  .strip {
    flex: none;
    display: flex;
    gap: 4px;
    height: 64px;
    padding: 6px 50%;
    overflow-x: auto;
    scrollbar-width: none;
    background: #000;
  }

  .strip::-webkit-scrollbar {
    display: none;
  }

  .thumb {
    flex: none;
    width: 52px;
    height: 52px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 4px;
    background: #222;
    overflow: hidden;
    opacity: 0.6;
    cursor: pointer;
  }

  .thumb:hover {
    opacity: 0.9;
  }

  .thumb.current {
    border-color: #fff;
    opacity: 1;
  }

  .thumb img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
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
  .stage img {
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

  .edited {
    position: absolute;
    inset: 0;
    will-change: transform;
  }

  .crop-area {
    position: absolute;
  }

  aside.edit h3 {
    margin: 18px 0 6px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: #a1a1a6;
  }

  .note {
    margin: 0 0 8px;
    font-size: 12px;
    color: #a1a1a6;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .buttons.end {
    margin-top: 24px;
  }

  .buttons .spacer {
    flex: 1;
  }

  aside.edit button {
    padding: 5px 10px;
    border: 1px solid #4a4a50;
    border-radius: 6px;
    background: #2c2c2e;
    color: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  aside.edit button[aria-pressed="true"],
  aside.edit button.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: #fff;
  }

  aside.edit button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  aside.edit button.danger {
    border-color: #d70015;
    background: #d70015;
    color: #fff;
  }

  .warning {
    padding: 10px;
    border: 1px solid #d70015;
    border-radius: 8px;
    background: rgb(215 0 21 / 0.12);
    font-size: 12px;
  }

  .warning p {
    margin: 0 0 8px;
  }

  aside.edit label {
    display: grid;
    grid-template-columns: 80px 1fr 34px;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
    font-size: 13px;
  }

  aside.edit output {
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: #a1a1a6;
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

  .stars {
    display: flex;
    gap: 2px;
  }

  .stars button {
    padding: 0 1px;
    border: none;
    background: none;
    color: #5a5a62;
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
  }

  .stars button.on {
    color: #facc15;
  }

  .labels {
    display: flex;
    gap: 8px;
    padding-top: 3px;
  }

  .labels button {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    cursor: pointer;
    opacity: 0.55;
  }

  .labels button.on {
    border-color: #fff;
    opacity: 1;
  }

  .stars button:disabled,
  .labels button:disabled {
    cursor: default;
  }

  .reveal {
    padding: 0;
    border: none;
    background: none;
    color: #6ea8fe;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .reveal:hover {
    text-decoration: underline;
  }
</style>
