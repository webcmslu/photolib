<!--
  "Check Anna's photos": the faces of a named person that look least like
  their other faces of the same years, least alike first, 40 at a time, each
  photo cut around that face. Clicking one marks it as someone else. "Done"
  takes those out of the person and records the others as checked, so they
  aren't shown again; then the next ones come. A relative who looks alike
  can still pass for the person: the list goes on as far as the user wants.
-->
<script lang="ts">
  import { errorText, t, tn } from "./i18n";
  import { checkPerson, confirmPersonFaces, removeFromPerson, thumbUrl, type DoubtfulFace } from "./api";
  import { SvelteSet } from "svelte/reactivity";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatNumber } from "./format";

  interface Props {
    libraryPath: string;
    name: string;
    clusters: number[];
    canEdit: boolean;
    /** Told after photos were taken out of the person. */
    onchanged: () => void;
    onclose: () => void;
  }
  let { libraryPath, name, clusters, canEdit, onchanged, onclose }: Props = $props();

  const PAGE = 40;
  let faces = $state<DoubtfulFace[] | null>(null);
  let doubtful = $state(0);
  let unchecked = $state(0);
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** The photos marked as someone else. */
  const out = new SvelteSet<string>();

  async function load() {
    error = null;
    try {
      const check = await checkPerson(libraryPath, clusters, PAGE);
      faces = check.faces;
      doubtful = check.doubtful;
      unchecked = check.unchecked;
      out.clear();
    } catch (e) {
      error = errorText(e);
      faces = faces ?? [];
    }
  }
  $effect(() => {
    void [libraryPath, clusters];
    load();
  });

  function toggle(fingerprint: string) {
    if (out.has(fingerprint)) out.delete(fingerprint);
    else out.add(fingerprint);
  }

  /** Takes out the photos marked, records the others as checked, shows the next ones. */
  async function done() {
    if (!faces) return;
    busy = true;
    error = null;
    try {
      const outside = faces.filter((face) => out.has(face.fingerprint)).map((face) => face.fingerprint);
      const theirs = faces.filter((face) => !out.has(face.fingerprint)).map((face) => face.fingerprint);
      if (theirs.length > 0) await confirmPersonFaces(libraryPath, clusters, theirs);
      if (outside.length > 0) {
        await removeFromPerson(libraryPath, clusters, outside, name);
        onchanged();
      }
      await load();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  /** A square crop around the face (in ten-thousandths), and the ring around it (in percent). */
  const RING_MARGIN = 0.2;
  function aroundFace(face: DoubtfulFace) {
    const { width, height } = face;
    if (!width || !height) return null;
    const [left, top, faceWidth, faceHeight] = face.bounds;
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
      ring: {
        left: (((left - faceWidth * RING_MARGIN) * width - cropLeft) / side) * 100,
        top: (((top - faceHeight * RING_MARGIN) * height - cropTop) / side) * 100,
        width: ((faceWidth * (1 + 2 * RING_MARGIN) * width) / side) * 100,
        height: ((faceHeight * (1 + 2 * RING_MARGIN) * height) / side) * 100,
      },
    };
  }
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && onclose()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(event) => event.target === event.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label={t("Check {name}'s photos", { name })}>
    <header>
      <div class="question">
        <h2>{t("Is this {name}?", { name })}</h2>
        <p>
          {#if faces === null}
            {t("Looking at {name}'s faces…", { name })}
          {:else if unchecked === 0}
            {t("Every photo of {name} has been checked.", { name })}
          {:else if doubtful > 0}
            {tn(doubtful, "1 face looks doubtful.", "{n} faces look doubtful.")}
            {t("The least like {name} first; {n} not checked yet.", { name, n: formatNumber(unchecked) })}
          {:else}
            {t("None looks doubtful. The least like {name} first; {n} not checked yet.", {
              name,
              n: formatNumber(unchecked),
            })}
          {/if}
        </p>
      </div>
      <button class="close" onclick={onclose} aria-label={t("Close")} title={t("Close")}>✕</button>
    </header>
    <p class="hint">{t("Click the photos that are someone else. The others count as {name} and won't be shown again.", { name })}</p>
    <div class="scroll">
      <ul class="photos">
        {#if faces === null}
          {#each { length: 12 } as _, i (i)}<li class="cell"></li>{/each}
        {:else}
          {#each faces as face (face.fingerprint)}
            {@const around = aroundFace(face)}
            {@const isOut = out.has(face.fingerprint)}
            <li class="cell" class:out={isOut}>
              <button
                class="pick"
                onclick={() => toggle(face.fingerprint)}
                aria-pressed={!isOut}
                aria-label={isOut ? t("Left out: someone else") : t("Is {name}", { name })}
                title={isOut ? t("Click if this is {name} after all", { name }) : t("Click if this is someone else")}
              >
                <EditedImage
                  src={thumbUrl(face.fingerprint, "preview")}
                  edit={around ? { ...NO_EDIT, crop: around.crop } : NO_EDIT}
                  width={face.width}
                  height={face.height}
                  fit="cover"
                />
                {#if around}
                  <span
                    class="ring"
                    style:left="{around.ring.left}%"
                    style:top="{around.ring.top}%"
                    style:width="{around.ring.width}%"
                    style:height="{around.ring.height}%"
                  ></span>
                {/if}
                <span class="tick" aria-hidden="true">{isOut ? "✕" : "✓"}</span>
              </button>
            </li>
          {/each}
        {/if}
      </ul>
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <footer>
      <button class="answer" onclick={onclose}>{t("Close")}</button>
      <button
        class="answer yes-answer"
        disabled={!canEdit || busy || !faces || faces.length === 0}
        onclick={done}
      >
        {out.size === 0
          ? t("All are {name}: next", { name })
          : tn(out.size, "Take out 1 photo: next", "Take out {n} photos: next")}
      </button>
    </footer>
  </div>
</div>

<style>
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

  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-dim);
  }

  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  ul.photos {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
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

  .error {
    margin: 0;
    color: var(--danger);
  }
</style>
