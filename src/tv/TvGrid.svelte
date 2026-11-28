<!--
  A grid of photos on the TV, newest first, loaded as the focus moves down.
  OK opens a photo full screen.
-->
<script lang="ts">
  import { photosPage, shapes, thumbUrl, type Listing, type PhotoItem } from "../lib/api";
  import { formatDuration, photoCount } from "../lib/format";
  import { errorText, t } from "../lib/i18n";
  import { tick } from "svelte";
  import { focusFirst } from "./remote";

  interface Props {
    libraryPath: string;
    title: string;
    listing: Listing;
    /** Opens the photo at `index`; `photos` gives any of them. */
    onopen: (photos: (index: number) => Promise<PhotoItem | null>, total: number, index: number) => void;
  }
  let { libraryPath, title, listing, onopen }: Props = $props();

  const PAGE = 120;
  /** Load the next page when the focus is this close to the end. */
  const AHEAD = 36;
  let items = $state<PhotoItem[]>([]);
  let total = $state<number | null>(null);
  let error = $state<string | null>(null);
  let loading: Promise<void> | null = null;

  function loadMore(): Promise<void> {
    if (loading) return loading;
    if (total !== null && items.length >= total) return Promise.resolve();
    loading = photosPage(libraryPath, listing, "dateDesc", items.length, PAGE)
      .then(async (page) => {
        const first = items.length === 0;
        items = [...items, ...page];
        if (page.length < PAGE) total = items.length;
        // The screen opened before its photos came: focus the first one.
        if (first) {
          await tick();
          focusFirst();
        }
      })
      .catch((e) => {
        error = errorText(e);
      })
      .finally(() => (loading = null));
    return loading;
  }

  $effect(() => {
    void [libraryPath, listing];
    items = [];
    total = null;
    shapes(libraryPath, listing, "dateDesc")
      .then((all) => (total = all.length))
      .catch(() => {});
    loadMore();
  });

  /** Any photo of the listing, loading pages as needed (for the viewer). */
  async function photo(index: number): Promise<PhotoItem | null> {
    while (index >= items.length && (total === null || items.length < total)) {
      const before = items.length;
      await loadMore();
      if (items.length === before) break;
    }
    return items[index] ?? null;
  }
</script>

<div class="grid-screen">
  <header>
    <h1>{title}</h1>
    {#if total !== null}<span>{photoCount(total)}</span>{/if}
  </header>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if total === 0}
    <p class="empty">{t("No photos here.")}</p>
  {/if}
  <div class="grid">
    {#each items as item, index (item.fingerprint + index)}
      <button
        data-focus
        class="tile"
        onclick={() => onopen(photo, total ?? items.length, index)}
        onfocus={() => index > items.length - AHEAD && loadMore()}
      >
        <img src={thumbUrl(item.fingerprint, "grid")} alt="" loading="lazy" />
        {#if item.video}<span class="badge">▶ {item.durationMs ? formatDuration(item.durationMs) : ""}</span>{/if}
        {#if item.favorite}<span class="heart">♥</span>{/if}
      </button>
    {/each}
  </div>
</div>

<style>
  .grid-screen {
    height: 100%;
    overflow-y: auto;
    padding: 56px 96px 80px;
  }

  header {
    display: flex;
    align-items: baseline;
    gap: 24px;
    margin-bottom: 32px;
  }

  h1 {
    margin: 0;
    font-size: 48px;
  }

  header span {
    color: var(--text-dim);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 20px;
  }

  .tile {
    position: relative;
    aspect-ratio: 1;
    padding: 0;
    border: none;
    border-radius: 12px;
    background: var(--surface);
    overflow: hidden;
  }

  .tile img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .badge,
  .heart {
    position: absolute;
    bottom: 10px;
    padding: 2px 10px;
    border-radius: 8px;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    font-size: 20px;
  }

  .badge {
    right: 10px;
  }

  .heart {
    left: 10px;
  }

  .empty,
  .error {
    color: var(--text-dim);
  }

  .error {
    color: var(--danger);
  }
</style>
