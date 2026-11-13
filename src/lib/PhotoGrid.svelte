<!--
  A virtualized photo grid grouped by day.

  The layout is computed from the day counts alone (every cell is a square), so
  the scroll height is right from the start and only the cells near the
  viewport exist in the DOM, whatever the size of the library. Photo data is
  pulled from `pages` for the visible range only.
-->
<script lang="ts">
  import { thumbUrl, type DayGroup } from "./api";
  import { formatDay, formatMonth, formatNumber, monthKey } from "./format";
  import type { PhotoPages } from "./pages.svelte";

  interface Props {
    /** Day groups, newest first. */
    groups: DayGroup[];
    pages: PhotoPages;
    /** Called with the listing index of the photo that was opened. */
    onopen: (index: number) => void;
  }
  let { groups, pages, onopen }: Props = $props();

  const TARGET_CELL = 150;
  const GAP = 3;
  const SIDE_PADDING = 16;
  const MONTH_HEADER = 52;
  const DAY_HEADER = 34;
  /** Rendered beyond the viewport on each side, so scrolling reveals ready cells. */
  const OVERSCAN = 800;
  /** Wait for the scroll to settle this long before asking for photo data. */
  const LOAD_DELAY_MS = 60;

  let scroller: HTMLDivElement;
  let clientWidth = $state(0);
  let clientHeight = $state(0);
  let scrollTop = $state(0);

  interface Section {
    day: number;
    count: number;
    /** Listing index of the section's first photo. */
    start: number;
    top: number;
    monthTitle: string | null;
    headerHeight: number;
    height: number;
  }

  const layout = $derived.by(() => {
    const width = Math.max(0, clientWidth - 2 * SIDE_PADDING);
    const columns = Math.max(2, Math.floor((width + GAP) / (TARGET_CELL + GAP)));
    const cell = (width - GAP * (columns - 1)) / columns;
    const pitch = cell + GAP;

    const sections: Section[] = [];
    let top = 0;
    let start = 0;
    let previousMonth: number | null = null;
    for (const group of groups) {
      const month = monthKey(group.day);
      const monthTitle = month !== previousMonth ? formatMonth(group.day) : null;
      previousMonth = month;
      const headerHeight = DAY_HEADER + (monthTitle ? MONTH_HEADER : 0);
      const height = headerHeight + Math.ceil(group.count / columns) * pitch;
      sections.push({ ...group, start, top, monthTitle, headerHeight, height });
      top += height;
      start += group.count;
    }
    return { sections, columns, cell, pitch, height: top, total: start };
  });

  /** Index of the last section whose `top` is at or above `y`. */
  function sectionAt(y: number): number {
    const { sections } = layout;
    let low = 0;
    let high = sections.length - 1;
    while (low < high) {
      const mid = (low + high + 1) >> 1;
      if (sections[mid].top <= y) low = mid;
      else high = mid - 1;
    }
    return low;
  }

  const visible = $derived.by(() => {
    const { sections, columns, cell, pitch } = layout;
    const headers: Section[] = [];
    const cells: { index: number; x: number; y: number }[] = [];
    if (sections.length === 0 || cell <= 0) return { headers, cells, first: 0, last: -1 };

    const viewTop = scrollTop - OVERSCAN;
    const viewBottom = scrollTop + clientHeight + OVERSCAN;
    for (let s = sectionAt(viewTop); s < sections.length; s++) {
      const section = sections[s];
      if (section.top > viewBottom) break;
      headers.push(section);

      const gridTop = section.top + section.headerHeight;
      const rows = Math.ceil(section.count / columns);
      const firstRow = Math.max(0, Math.floor((viewTop - gridTop) / pitch));
      const lastRow = Math.min(rows - 1, Math.floor((viewBottom - gridTop) / pitch));
      for (let row = firstRow; row <= lastRow; row++) {
        const rowEnd = Math.min(section.count, (row + 1) * columns);
        for (let i = row * columns; i < rowEnd; i++) {
          cells.push({
            index: section.start + i,
            x: SIDE_PADDING + (i % columns) * pitch,
            y: gridTop + row * pitch,
          });
        }
      }
    }
    return {
      headers,
      cells,
      first: cells.length > 0 ? cells[0].index : 0,
      last: cells.length > 0 ? cells[cells.length - 1].index : -1,
    };
  });

  // Ask for the photos in view once the scroll position has settled. While the
  // scrollbar is dragged across the library, cells stay placeholders instead of
  // requesting (and making thumbnails for) everything that flies past.
  $effect(() => {
    const { first, last } = visible;
    const target = pages;
    if (last < first) return;
    const timer = setTimeout(() => target.ensure(first - 200, last + 200), LOAD_DELAY_MS);
    return () => clearTimeout(timer);
  });

  let scrollFrame = 0;
  function onscroll() {
    if (scrollFrame) return;
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = 0;
      scrollTop = scroller.scrollTop;
    });
  }

  /** Scrolls just enough to bring the photo at `index` into view. */
  export function reveal(index: number) {
    const { sections, columns, pitch, cell } = layout;
    let section: Section | undefined;
    for (const candidate of sections) {
      if (candidate.start > index) break;
      section = candidate;
    }
    if (!section) return;
    const row = Math.floor((index - section.start) / columns);
    const y = section.top + section.headerHeight + row * pitch;
    if (y < scroller.scrollTop) {
      scroller.scrollTop = Math.max(0, y - section.headerHeight);
    } else if (y + cell > scroller.scrollTop + clientHeight) {
      scroller.scrollTop = y + cell - clientHeight + GAP;
    }
  }
</script>

<div class="scroller" bind:this={scroller} bind:clientWidth bind:clientHeight {onscroll}>
  <div class="canvas" style:height="{layout.height}px" style:--cell="{layout.cell}px">
    {#each visible.headers as section (section.day)}
      <header style:transform="translateY({section.top}px)" style:height="{section.headerHeight}px">
        {#if section.monthTitle}
          <h2>{section.monthTitle}</h2>
        {/if}
        <h3>
          {formatDay(section.day)}
          <span class="count">{formatNumber(section.count)}</span>
        </h3>
      </header>
    {/each}

    {#each visible.cells as cell (cell.index)}
      {@const photo = pages.get(cell.index)}
      <button
        class="cell"
        style:transform="translate({cell.x}px, {cell.y}px)"
        onclick={() => onopen(cell.index)}
        aria-label={photo?.relPath ?? "Photo"}
      >
        {#if photo}
          <img src={thumbUrl(photo.fingerprint)} alt="" draggable="false" decoding="async" />
        {/if}
      </button>
    {/each}
  </div>
</div>

<style>
  .scroller {
    height: 100%;
    overflow-y: auto;
    /* Keep the scroll position when content above changes size. */
    overflow-anchor: none;
  }

  .canvas {
    position: relative;
    /* Lets the browser skip layout and paint work outside this box. */
    contain: strict;
  }

  header {
    position: absolute;
    top: 0;
    left: 16px;
    right: 16px;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    padding-bottom: 8px;
  }

  h2 {
    margin: 0 0 6px;
    font-size: 20px;
    font-weight: 650;
  }

  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }

  .count {
    margin-left: 6px;
    font-weight: 400;
    color: var(--text-dim);
  }

  .cell {
    position: absolute;
    top: 0;
    left: 0;
    width: var(--cell);
    height: var(--cell);
    padding: 0;
    border: none;
    background: var(--surface-hover);
    cursor: pointer;
    overflow: hidden;
  }

  .cell:focus-visible {
    outline-offset: -2px;
    z-index: 1;
  }

  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
