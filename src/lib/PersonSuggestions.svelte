<!--
  "Is this also Anna?": on a named person's page, the groups of faces that
  look like them but were not merged automatically (often the same person
  younger, or from another angle). ✓ adds a group to the person; ✕ says it
  is someone else, and it is not suggested again. Clicking a face shows the
  group's photos large, each around that face, to tell who it is. There,
  photos that are someone else can be left out: they move to a group of
  their own before the rest is added.
-->
<script lang="ts">
  import { errorText, t, tn } from "./i18n";
  import {
    namePerson,
    personFaces,
    personSuggestions,
    photosPage,
    rejectSuggestion,
    splitPerson,
    thumbUrl,
    type FaceBox,
    type Person,
    type PhotoItem,
  } from "./api";
  import { SvelteSet } from "svelte/reactivity";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatNumber, photoCount } from "./format";

  interface Props {
    libraryPath: string;
    name: string;
    clusters: number[];
    canEdit: boolean;
    /** Told the groups the person is made of after one was added. */
    onadded: (clusters: number[]) => void;
  }
  let { libraryPath, name, clusters, canEdit, onadded }: Props = $props();

  let suggestions = $state<Person[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** The group shown large, and its photos (the first ones, newest first). */
  let previewing = $state<Person | null>(null);
  let previewPhotos = $state<PhotoItem[] | null>(null);
  /** Where the group's face is in each photo, by fingerprint. */
  let previewFaces = $state.raw(new Map<string, FaceBox["bounds"]>());
  /**
   * Which photos are someone else: every one when `allOut` ("Leave all
   * out"), none otherwise, except those in `flipped`. Photos not loaded yet
   * follow `allOut` too.
   */
  let allOut = $state(false);
  const flipped = new SvelteSet<string>();
  const isOut = (fingerprint: string) => allOut !== flipped.has(fingerprint);
  const PREVIEW_PAGE = 60;
  let loadingMore = $state(false);

  async function preview(suggestion: Person) {
    previewing = suggestion;
    previewPhotos = null;
    previewFaces = new Map();
    allOut = false;
    flipped.clear();
    try {
      const [photos, faces] = await Promise.all([
        photosPage(libraryPath, { kind: "person", clusters: suggestion.clusters }, "dateDesc", 0, PREVIEW_PAGE),
        personFaces(libraryPath, suggestion.clusters),
      ]);
      if (previewing !== suggestion) return;
      previewFaces = new Map(faces.map((face) => [face.fingerprint, face.bounds]));
      previewPhotos = photos;
    } catch (e) {
      error = errorText(e);
    }
  }

  /** Loads the next photos of a large group. */
  async function loadMore() {
    const suggestion = previewing;
    if (!suggestion || !previewPhotos || loadingMore) return;
    loadingMore = true;
    try {
      const more = await photosPage(
        libraryPath,
        { kind: "person", clusters: suggestion.clusters },
        "dateDesc",
        previewPhotos.length,
        PREVIEW_PAGE,
      );
      if (previewing === suggestion) previewPhotos = [...previewPhotos, ...more];
    } catch (e) {
      error = errorText(e);
    } finally {
      loadingMore = false;
    }
  }

  function toggleOut(fingerprint: string) {
    if (flipped.has(fingerprint)) flipped.delete(fingerprint);
    else flipped.add(fingerprint);
  }

  /** "Leave all out" or "Keep all": every photo, loaded or not. */
  function setAll(out: boolean) {
    allOut = out;
    flipped.clear();
  }

  /**
   * A square crop around the face in a photo (in ten-thousandths), wide
   * enough to see who it is, and where the face is inside it (in percent).
   */
  const RING_MARGIN = 0.2;
  function aroundFace(photo: PhotoItem) {
    const bounds = previewFaces.get(photo.fingerprint);
    const width = photo.width;
    const height = photo.height;
    if (!bounds || !width || !height) return null;
    const [left, top, faceWidth, faceHeight] = bounds;
    const side = Math.min(width, height, Math.max(faceWidth * width, faceHeight * height) * 3.5);
    const centerX = (left + faceWidth / 2) * width;
    const centerY = (top + faceHeight / 2) * height;
    const cropLeft = Math.min(Math.max(0, centerX - side / 2), width - side);
    const cropTop = Math.min(Math.max(0, centerY - side / 2), height - side);
    return {
      crop: [
        Math.round((cropLeft / width) * 10_000),
        Math.round((cropTop / height) * 10_000),
        Math.round((side / width) * 10_000),
        Math.round((side / height) * 10_000),
      ] as [number, number, number, number],
      // A little larger than the face, so the ring goes around it.
      ring: {
        left: (((left - faceWidth * RING_MARGIN) * width - cropLeft) / side) * 100,
        top: (((top - faceHeight * RING_MARGIN) * height - cropTop) / side) * 100,
        width: ((faceWidth * (1 + 2 * RING_MARGIN) * width) / side) * 100,
        height: ((faceHeight * (1 + 2 * RING_MARGIN) * height) / side) * 100,
      },
    };
  }

  const total = $derived(previewing?.photos ?? 0);
  const outCount = $derived(allOut ? total - flipped.size : flipped.size);
  const keptCount = $derived(total - outCount);

  /** Answers for the group being previewed, then closes the preview. */
  async function answer(yes: boolean) {
    const suggestion = previewing;
    previewing = null;
    if (!suggestion) return;
    if (!yes || keptCount <= 0) return reject(suggestion);
    if (outCount === 0) return confirm(suggestion);
    busy = true;
    error = null;
    try {
      if (allOut) {
        // Only the photos ticked back are them: those move to a group of
        // their own, which gets the name; the rest is someone else.
        const theirs = await splitPerson(libraryPath, suggestion.clusters, [...flipped], null);
        if (theirs !== null) {
          await namePerson(libraryPath, [theirs], name);
          onadded([...clusters, theirs]);
        }
        for (const cluster of suggestion.clusters) await rejectSuggestion(libraryPath, cluster, name);
      } else {
        // Some photos are someone else: they go to a group of their own,
        // which is not suggested for this person again, and the rest is added.
        const apart = await splitPerson(libraryPath, suggestion.clusters, [...flipped], null);
        if (apart !== null) await rejectSuggestion(libraryPath, apart, name);
        await namePerson(libraryPath, suggestion.clusters, name);
        onadded([...clusters, ...suggestion.clusters]);
      }
      suggestions = suggestions.filter((s) => s !== suggestion);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    const [path, who, groups] = [libraryPath, name, clusters];
    personSuggestions(path, groups, who)
      .then((found) => (suggestions = found))
      .catch((e) => (error = errorText(e)));
  });

  async function confirm(suggestion: Person) {
    busy = true;
    error = null;
    try {
      await namePerson(libraryPath, suggestion.clusters, name);
      suggestions = suggestions.filter((s) => s !== suggestion);
      onadded([...clusters, ...suggestion.clusters]);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  async function reject(suggestion: Person) {
    error = null;
    try {
      await rejectSuggestion(libraryPath, suggestion.clusters[0], name);
      suggestions = suggestions.filter((s) => s !== suggestion);
    } catch (e) {
      error = errorText(e);
    }
  }
</script>

{#if suggestions.length > 0}
  <section aria-label={t("Suggestions")}>
    <h3>{t("Is this also {name}?", { name })}</h3>
    <ul>
      {#each suggestions as suggestion (suggestion.clusters[0])}
        <li>
          <button
            class="face"
            onclick={() => preview(suggestion)}
            title={t("See the photos of this face")}
            aria-label={t("See the photos of this face")}
          >
            <EditedImage
              src={thumbUrl(suggestion.cover, "preview")}
              edit={{ ...NO_EDIT, crop: suggestion.coverCrop }}
              width={suggestion.coverWidth}
              height={suggestion.coverHeight}
              fit="cover"
            />
          </button>
          <span class="count">{formatNumber(suggestion.photos)}</span>
          <span class="actions">
            <button
              class="yes"
              disabled={!canEdit || busy}
              onclick={() => confirm(suggestion)}
              title={t("Yes, this is {name}", { name })}
              aria-label={t("Yes, this is {name}", { name })}
            >
              ✓
            </button>
            <button
              class="no"
              onclick={() => reject(suggestion)}
              title={t("No, this is someone else")}
              aria-label={t("No, this is someone else")}
            >
              ✕
            </button>
          </span>
        </li>
      {/each}
    </ul>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </section>
{/if}

<svelte:window onkeydown={(event) => previewing && event.key === "Escape" && (previewing = null)} />

{#if previewing}
  {@const shown = previewing}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={(event) => event.target === event.currentTarget && (previewing = null)}>
    <div class="dialog" role="dialog" aria-modal="true" aria-label={t("Is this {name}?", { name })}>
      <header>
        <span class="face large">
          <EditedImage
            src={thumbUrl(shown.cover, "preview")}
            edit={{ ...NO_EDIT, crop: shown.coverCrop }}
            width={shown.coverWidth}
            height={shown.coverHeight}
            fit="cover"
          />
        </span>
        <div class="question">
          <h2>{t("Is this {name}?", { name })}</h2>
          <p>{photoCount(shown.photos)}</p>
        </div>
        <button class="close" onclick={() => (previewing = null)} aria-label={t("Close")} title={t("Close")}>✕</button>
      </header>
      <div class="tools">
        <p class="hint">{t("Click a photo that is someone else to leave it out.")}</p>
        <button class="link" onclick={() => setAll(true)} hidden={allOut && flipped.size === 0}>
          {t("Leave all out")}
        </button>
        <button class="link" onclick={() => setAll(false)} hidden={!allOut && flipped.size === 0}>
          {t("Keep all")}
        </button>
      </div>
      <div class="scroll">
        <ul class="photos" class:many={total > 12}>
          {#if previewPhotos === null}
            {#each { length: Math.min(total, 12) } as _, i (i)}<li class="cell"></li>{/each}
          {:else}
            {#each previewPhotos as photo (photo.fingerprint)}
              {@const face = aroundFace(photo)}
              {@const out = isOut(photo.fingerprint)}
              <li class="cell" class:out>
                <button
                  class="pick"
                  onclick={() => toggleOut(photo.fingerprint)}
                  aria-pressed={!out}
                  aria-label={out ? t("Left out: someone else") : t("Is {name}", { name })}
                  title={out ? t("Click if this is {name} after all", { name }) : t("Click if this is someone else")}
                >
                  <EditedImage
                    src={thumbUrl(photo.fingerprint, "preview")}
                    edit={face ? { ...NO_EDIT, crop: face.crop } : (photo.edit ?? NO_EDIT)}
                    width={photo.width}
                    height={photo.height}
                    fit="cover"
                  />
                  {#if face}
                    <span
                      class="ring"
                      style:left="{face.ring.left}%"
                      style:top="{face.ring.top}%"
                      style:width="{face.ring.width}%"
                      style:height="{face.ring.height}%"
                    ></span>
                  {/if}
                  <span class="tick" aria-hidden="true">{out ? "✕" : "✓"}</span>
                </button>
              </li>
            {/each}
          {/if}
        </ul>
        {#if previewPhotos !== null && previewPhotos.length < total}
          <div class="more">
            <span>{t("{shown} of {total} photos", { shown: previewPhotos.length, total })}</span>
            <button class="answer" disabled={loadingMore} onclick={loadMore}>{t("Show more")}</button>
          </div>
        {/if}
      </div>
      <footer>
        <button class="answer no-answer" onclick={() => answer(false)}>{t("No, this is someone else")}</button>
        <button
          class="answer yes-answer"
          disabled={!canEdit || busy || keptCount <= 0}
          onclick={() => answer(true)}
        >
          {outCount === 0
            ? t("Yes, this is {name}", { name })
            : tn(keptCount, "Yes, this one is {name}", "Yes, these {n} are {name}", { name })}
        </button>
      </footer>
    </div>
  </div>
{/if}

<style>
  section {
    padding: 10px 16px 12px;
    border-bottom: 1px solid var(--border);
  }

  h3 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
  }

  ul {
    display: flex;
    gap: 18px;
    /* Room for the hover ring, which the scrolling row would cut off; the
       negative margin keeps the faces lined up with the title. */
    margin: 0 -6px;
    padding: 6px 6px 4px;
    overflow-x: auto;
    list-style: none;
  }

  li {
    display: flex;
    flex: none;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }

  .face {
    /* The picture fills this circle (see EditedImage), not the window. */
    position: relative;
    display: block;
    width: 80px;
    height: 80px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--surface-hover);
    overflow: hidden;
  }

  button.face {
    cursor: zoom-in;
  }

  button.face:hover,
  button.face:focus-visible {
    outline: 3px solid var(--accent);
    outline-offset: 2px;
  }

  .face.large {
    flex: none;
    width: 96px;
    height: 96px;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.45);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: 14px;
    width: min(960px, calc(100vw - 48px));
    max-height: calc(100vh - 64px);
    padding: 20px 24px;
    border-radius: 14px;
    background: var(--surface);
    box-shadow: 0 12px 48px rgb(0 0 0 / 0.4);
  }

  header {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .question {
    flex: 1;
    min-width: 0;
  }

  .question h2 {
    margin: 0;
    font-size: 20px;
  }

  .question p {
    margin: 2px 0 0;
    color: var(--text-dim);
  }

  .close {
    align-self: flex-start;
    width: 30px;
    height: 30px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--surface-hover);
    color: var(--text);
    cursor: pointer;
  }

  /* The photos scroll on their own; the grid inside keeps its squares. */
  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  ul.photos {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 6px;
    margin: 0;
    padding: 0;
  }

  ul.photos.many {
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  }

  .tools {
    display: flex;
    align-items: baseline;
    gap: 14px;
  }

  .tools .hint {
    flex: 1;
  }

  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .link[hidden] {
    display: none;
  }

  .cell {
    position: relative;
    aspect-ratio: 1;
    border-radius: 6px;
    background: var(--surface-hover);
    overflow: hidden;
  }

  .pick {
    position: absolute;
    inset: 0;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
  }

  .pick:focus-visible {
    outline: 3px solid var(--accent);
    outline-offset: -3px;
  }

  /* Where this face is in the photo. */
  .ring {
    position: absolute;
    border: 2px solid rgb(255 255 255 / 0.9);
    border-radius: 50%;
    box-shadow: 0 0 0 1px rgb(0 0 0 / 0.35);
    pointer-events: none;
  }

  .tick {
    position: absolute;
    right: 8px;
    bottom: 8px;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    font-size: 14px;
    font-weight: 700;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.35);
  }

  .cell.out .tick {
    background: var(--danger);
  }

  .cell.out :global(img) {
    opacity: 0.35;
    filter: grayscale(1);
  }

  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-dim);
  }

  .more {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    padding: 14px 0 4px;
    font-size: 13px;
    color: var(--text-dim);
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  .answer {
    padding: 8px 16px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 14px;
    cursor: pointer;
  }

  .yes-answer {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }

  .answer:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .count {
    font-size: 11px;
    color: var(--text-dim);
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .actions button {
    width: 28px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }

  .yes:hover:not(:disabled) {
    border-color: #16a34a;
    background: #16a34a;
    color: #fff;
  }

  .no:hover {
    border-color: var(--danger);
    background: var(--danger);
    color: #fff;
  }

  .actions button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .error {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--danger);
  }
</style>
