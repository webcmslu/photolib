<!--
  Similar shots: bursts and near-identical photos taken moments apart.
  Each group shows its shots with the one to keep marked (the app suggests
  one: a favorite, then the best rated, then the largest). Clicking a shot
  keeps or drops it. "Hide the others" hides the shots not kept, which is
  shared like any hidden photo and undone from Hidden. No file is touched.
-->
<script lang="ts">
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { applyOps, similarGroups, thumbUrl, type Op, type SimilarGroup } from "./api";
  import { formatDateTime, formatNumber, photoCount } from "./format";
  import { starText } from "./labels";

  interface Props {
    libraryPath: string;
    /** Changes whenever the index changed. */
    revision: number;
    /** False while the library is offline or read-only. */
    canEdit: boolean;
    /** Told after photos were hidden. */
    onchanged: () => void;
  }
  let { libraryPath, revision, canEdit, onchanged }: Props = $props();

  const PAGE = 100;

  let groups = $state<SimilarGroup[] | null>(null);
  let shown = $state(PAGE);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let hiddenNow = $state<number | null>(null);
  /** The user's choice of shots to keep, by group, where it differs from the suggestion. */
  const kept = new SvelteMap<string, SvelteSet<number>>();

  const groupKey = (group: SimilarGroup) => group.shots[0].fingerprint;
  const keptOf = (group: SimilarGroup): Set<number> => kept.get(groupKey(group)) ?? new Set([group.keep]);
  const othersOf = (group: SimilarGroup) => group.shots.filter((_, index) => !keptOf(group).has(index));
  const otherCount = $derived(groups?.reduce((sum, group) => sum + othersOf(group).length, 0) ?? 0);

  async function load() {
    try {
      groups = await similarGroups(libraryPath);
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    void revision;
    if (!busy) load();
  });

  function toggle(group: SimilarGroup, index: number) {
    const now = new SvelteSet(keptOf(group));
    if (now.has(index)) {
      // At least one shot of a group is always kept.
      if (now.size === 1) return;
      now.delete(index);
    } else {
      now.add(index);
    }
    kept.set(groupKey(group), now);
  }

  async function hide(list: SimilarGroup[]) {
    const ops: Op[] = list.flatMap((group) =>
      othersOf(group).map((shot) => ({ op: "hide", photo: shot.fingerprint, on: true }) as Op),
    );
    if (ops.length === 0) return;
    busy = true;
    error = null;
    try {
      await applyOps(libraryPath, ops);
      for (const group of list) kept.delete(groupKey(group));
      hiddenNow = ops.length;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
    await load();
    onchanged();
  }

  /** The group and shot shown large, after a double-click. */
  let preview = $state<{ group: SimilarGroup; index: number } | null>(null);

  function previewKey(event: KeyboardEvent) {
    if (!preview) return;
    const count = preview.group.shots.length;
    if (event.key === "Escape") preview = null;
    else if (event.key === "ArrowRight") preview.index = (preview.index + 1) % count;
    else if (event.key === "ArrowLeft") preview.index = (preview.index + count - 1) % count;
    else if (event.key === " ") toggle(preview.group, preview.index);
    else return;
    event.preventDefault();
  }

  function size(width: number | null, height: number | null): string {
    return width && height ? `${width} × ${height}` : "";
  }
</script>

<svelte:window onkeydown={previewKey} />

<div class="similar">
  <header>
    {#if groups === null}
      <p class="dim">Looking for similar shots…</p>
    {:else}
      <div class="summary">
        <h2>Similar shots</h2>
        <p class="dim">
          {#if groups.length === 0}
            No similar shots found. Photos are compared once their thumbnails are made.
          {:else}
            {formatNumber(groups.length)} {groups.length === 1 ? "group" : "groups"} of shots taken moments apart.
            Click a shot to keep it too, or to drop it.
          {/if}
        </p>
      </div>
      {#if otherCount > 0}
        <button
          class="primary"
          disabled={!canEdit || busy}
          title={canEdit ? "Hide every shot that is not kept, in every group" : "The library is offline or read-only"}
          onclick={() => groups && hide(groups)}
        >
          Hide {photoCount(otherCount)} not kept
        </button>
      {/if}
    {/if}
  </header>

  {#if hiddenNow !== null}
    <div class="notice">
      {photoCount(hiddenNow)} hidden. They are still in the library folder: find them under <strong>Hidden</strong>
      to show them again.
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  {#if groups && groups.length > 0}
    <ul class="groups">
      {#each groups.slice(0, shown) as group (groupKey(group))}
        {@const keep = keptOf(group)}
        {@const others = group.shots.length - keep.size}
        <li class="group">
          <div class="head">
            <span class="dim">{formatDateTime(group.shots[0].sortDate)} · {photoCount(group.shots.length)}</span>
            <button class="secondary" disabled={!canEdit || busy || others === 0} onclick={() => hide([group])}>
              Hide the {others === 1 ? "other" : `${others} others`}
            </button>
          </div>
          <div class="shots">
            {#each group.shots as shot, index (shot.fingerprint)}
              {@const keeps = keep.has(index)}
              <figure class="shot" class:keeps>
                <button
                  class="pick"
                  onclick={() => toggle(group, index)}
                  ondblclick={() => (preview = { group, index })}
                  title={keeps ? "Kept. Click to drop, double-click to open" : "Click to keep, double-click to open"}
                >
                  <img src={thumbUrl(shot.fingerprint)} alt="" loading="lazy" draggable="false" />
                  <span class="badge">{keeps ? "Keep" : "Hide"}</span>
                  {#if shot.favorite || shot.rating > 0}
                    <span class="marks">{shot.favorite ? "♥ " : ""}{starText(shot.rating)}</span>
                  {/if}
                </button>
                <figcaption class="dim">{size(shot.width, shot.height)}</figcaption>
              </figure>
            {/each}
          </div>
        </li>
      {/each}
    </ul>
    {#if shown < groups.length}
      <button class="secondary more" onclick={() => (shown += PAGE)}>
        Show more ({formatNumber(groups.length - shown)} left)
      </button>
    {/if}
  {/if}
</div>

{#if preview}
  {@const shot = preview.group.shots[preview.index]}
  {@const keeps = keptOf(preview.group).has(preview.index)}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="preview" onclick={() => (preview = null)}>
    <img src={thumbUrl(shot.fingerprint, "preview")} alt="" draggable="false" />
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="bar" onclick={(event) => event.stopPropagation()}>
      <span>{preview.index + 1} of {preview.group.shots.length} · {formatDateTime(shot.sortDate)}</span>
      <button class="choice" class:keeps onclick={() => preview && toggle(preview.group, preview.index)}>
        {keeps ? "Keep" : "Hide"}
      </button>
      <span class="hint">← → to move, Space to keep or drop, Esc to close</span>
    </div>
  </div>
{/if}

<style>
  .similar {
    height: 100%;
    overflow-y: auto;
    padding: 16px 20px 32px;
    box-sizing: border-box;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 12px;
  }

  h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 650;
  }

  .summary p {
    margin: 2px 0 0;
    font-size: 13px;
  }

  .dim {
    color: var(--text-dim);
  }

  button {
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .primary {
    padding: 7px 14px;
    border: none;
    border-radius: 7px;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
    white-space: nowrap;
  }

  .secondary {
    padding: 5px 11px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--surface);
    color: var(--text);
    white-space: nowrap;
  }

  .secondary:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .notice {
    margin: 0 0 12px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg);
    font-size: 13px;
  }

  .error {
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .groups {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .group {
    padding: 12px 0;
    border-top: 1px solid var(--border);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
    font-size: 12px;
  }

  .shots {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }

  .shot {
    margin: 0;
    width: 160px;
  }

  .pick {
    position: relative;
    display: block;
    width: 160px;
    height: 160px;
    padding: 0;
    border: 3px solid transparent;
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg);
  }

  .shot.keeps .pick {
    border-color: #16a34a;
  }

  .pick img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    transition: opacity 0.15s;
  }

  .shot:not(.keeps) .pick img {
    opacity: 0.55;
  }

  .badge {
    position: absolute;
    left: 6px;
    top: 6px;
    padding: 1px 8px;
    border-radius: 999px;
    background: rgb(0 0 0 / 0.55);
    color: #fff;
    font-size: 11px;
    font-weight: 600;
  }

  .shot.keeps .badge {
    background: #16a34a;
  }

  .marks {
    position: absolute;
    right: 6px;
    bottom: 5px;
    color: #fff;
    font-size: 11px;
    text-shadow: 0 1px 2px rgb(0 0 0 / 0.7);
  }

  figcaption {
    margin-top: 3px;
    font-size: 11px;
    text-align: center;
  }

  .preview {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 24px;
    background: rgb(0 0 0 / 0.88);
  }

  .preview img {
    min-height: 0;
    max-width: 100%;
    max-height: calc(100% - 48px);
    object-fit: contain;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 14px;
    color: #fff;
    font-size: 13px;
  }

  .hint {
    color: rgb(255 255 255 / 0.6);
    font-size: 12px;
  }

  .choice {
    padding: 3px 14px;
    border: 1px solid rgb(255 255 255 / 0.5);
    border-radius: 999px;
    background: none;
    color: #fff;
  }

  .choice.keeps {
    border-color: #16a34a;
    background: #16a34a;
    font-weight: 600;
  }

  .more {
    display: block;
    margin: 12px auto 0;
  }
</style>
