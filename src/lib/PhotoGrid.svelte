<!--
  A virtualized photo grid grouped by day.

  The layout is computed from the day counts alone (every cell is a square), so
  the scroll height is right from the start and only the cells near the
  viewport exist in the DOM, whatever the size of the library. Photo data is
  pulled from `pages` for the visible range only.

  A day with enough photos shows its first one as a large "featured" card,
  several cells wide and two rows high, with the others flowing around it.
-->
<script lang="ts">
  import { thumbUrl, type DayGroup } from "./api";
  import { formatDay, formatDuration, formatMonth, formatNumber, monthKey, rawPairLabel } from "./format";
  import EditedImage from "./EditedImage.svelte";
  import type { PhotoPages } from "./pages.svelte";

  interface Props {
    /** Day groups, newest first. */
    groups: DayGroup[];
    pages: PhotoPages;
    /** Listing indexes of the selected photos. */
    selected: ReadonlySet<number>;
    /** Called with the listing index of the photo that was opened. */
    onopen: (index: number) => void;
    /** Called to select or deselect a photo; `range` extends from the last one. */
    ontoggle: (index: number, range: boolean) => void;
    /** The width cells aim for, in pixels: smaller means more per row. */
    cellSize?: number;
    /** A day (days since 1970) to start scrolled to. */
    startAt?: number | null;
    /**
     * "square" crops each photo to fill a square cell; "river" lays the
     * photos in rows at their own shape, each row filling the width.
     */
    fit?: "square" | "river";
    /** For "river": every photo's width / height × 1000, in listing order. */
    shapes?: number[] | null;
    /** Told the oldest and newest day on screen whenever they change. */
    onrange?: (range: { oldest: number; newest: number } | null) => void;
  }
  let {
    groups,
    pages,
    selected,
    onopen,
    ontoggle,
    cellSize = 150,
    startAt = null,
    fit = "square",
    shapes = null,
    onrange,
  }: Props = $props();

  /** A plain click opens a photo, unless a selection is under way. */
  function onclick(event: MouseEvent, index: number) {
    if (event.shiftKey) ontoggle(index, true);
    else if (event.ctrlKey || event.metaKey || selected.size > 0) ontoggle(index, false);
    else onopen(index);
  }

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

  /** A row of the river layout; `xs` and `widths` are per photo. */
  interface RiverRow {
    top: number;
    height: number;
    /** Listing index of the row's first photo. */
    start: number;
    xs: number[];
    widths: number[];
  }

  interface Section {
    /** Null when the listing is sorted by name: one section, no heading. */
    day: number | null;
    count: number;
    /** Listing index of the section's first photo. */
    start: number;
    top: number;
    monthTitle: string | null;
    headerHeight: number;
    height: number;
    /** Rows of cells. */
    rows: number;
    /** Size of the featured first card in cells; 0 when there is none. */
    featuredWidth: number;
    featuredHeight: number;
  }

  /**
   * How large a day's featured card is, in cells. A day gets one only if it
   * has enough photos to fill the space beside it, so no gaps appear.
   */
  function featuredSize(count: number, columns: number): [number, number] {
    for (const [width, height] of [
      [3, 2],
      [2, 2],
    ]) {
      if (columns > width && count >= 1 + height * (columns - width)) return [width, height];
    }
    return [0, 0];
  }

  /** Where the photo number `i` of a section goes: its row and column. */
  function place(section: Section, i: number, columns: number): { row: number; column: number } {
    const { featuredWidth: width, featuredHeight: height } = section;
    if (width === 0) return { row: Math.floor(i / columns), column: i % columns };
    if (i === 0) return { row: 0, column: 0 };
    // Beside the featured card there are `side` cells per row.
    const side = columns - width;
    const beside = i - 1;
    if (beside < height * side) {
      return { row: Math.floor(beside / side), column: width + (beside % side) };
    }
    const below = beside - height * side;
    return { row: height + Math.floor(below / columns), column: below % columns };
  }

  const layout = $derived.by(() => {
    const width = Math.max(0, clientWidth - 2 * SIDE_PADDING);
    const columns = Math.max(2, Math.floor((width + GAP) / (cellSize + GAP)));
    const cell = (width - GAP * (columns - 1)) / columns;
    const pitch = cell + GAP;

    const sections: Section[] = [];
    let top = 0;
    let start = 0;
    let previousMonth: number | null = null;
    const total = groups.reduce((sum, group) => sum + group.count, 0);
    // The river needs every photo's shape; until they are there, squares.
    const river = fit === "river" && shapes !== null && shapes.length === total && width > 0;
    const riverRows: RiverRow[] = [];
    for (const group of groups) {
      const day = group.day;
      const month = day === null ? null : monthKey(day);
      const monthTitle = day !== null && month !== previousMonth ? formatMonth(day) : null;
      previousMonth = month;
      // Sorted by name, there are no days: just a margin above the photos.
      const headerHeight = day === null ? SIDE_PADDING : DAY_HEADER + (monthTitle ? MONTH_HEADER : 0);
      const [featuredWidth, featuredHeight] =
        day === null || river ? [0, 0] : featuredSize(group.count, columns);
      let rows: number;
      let height: number;
      if (river && shapes) {
        // Fill each row with photos at `cellSize` high until it is full,
        // then scale the row to the exact width. A day's last row keeps the
        // target height and is not stretched.
        const firstRow = riverRows.length;
        let y = top + headerHeight;
        const end = start + group.count;
        for (let i = start; i < end; ) {
          let aspects = 0;
          let count = 0;
          while (i + count < end) {
            aspects += shapes[i + count] / 1000;
            count++;
            if (aspects * cellSize + GAP * (count - 1) >= width) break;
          }
          const full = aspects * cellSize + GAP * (count - 1) >= width;
          const rowHeight = full ? (width - GAP * (count - 1)) / aspects : cellSize;
          const xs: number[] = [];
          const widths: number[] = [];
          let x = SIDE_PADDING;
          for (let k = 0; k < count; k++) {
            const w = (shapes[i + k] / 1000) * rowHeight;
            xs.push(x);
            widths.push(w);
            x += w + GAP;
          }
          riverRows.push({ top: y, height: rowHeight, start: i, xs, widths });
          y += rowHeight + GAP;
          i += count;
        }
        rows = riverRows.length - firstRow;
        height = y - top;
      } else {
        rows =
          featuredWidth === 0
            ? Math.ceil(group.count / columns)
            : featuredHeight +
              Math.ceil((group.count - 1 - featuredHeight * (columns - featuredWidth)) / columns);
        height = headerHeight + rows * pitch;
      }
      sections.push({
        ...group,
        start,
        top,
        monthTitle,
        headerHeight,
        height,
        rows,
        featuredWidth,
        featuredHeight,
      });
      top += height;
      start += group.count;
    }
    return { sections, columns, cell, pitch, height: top, total: start, river, riverRows };
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

  // --- The year scrubber on the right, as in Windows Photos -----------------

  let railHeight = $state(0);
  /** Month under the pointer while it is over the scrubber. */
  let railHover = $state<{ y: number; label: string } | null>(null);
  let railDragging = false;
  const DAY_MS = 86_400_000;
  const yearOf = (day: number) => new Date(day * DAY_MS).getUTCFullYear();

  /** Worth showing only for a long grid that spans several years. */
  const showRail = $derived(
    layout.sections.length > 0 &&
      layout.sections[0].day !== null &&
      layout.height > clientHeight * 3 &&
      yearOf(layout.sections[0].day) !== yearOf(layout.sections[layout.sections.length - 1].day ?? 0),
  );

  /** Where each year starts, as a fraction of the grid's height. */
  const yearStarts = $derived.by(() => {
    const starts: { year: number; top: number }[] = [];
    for (const section of layout.sections) {
      if (section.day === null) continue;
      const year = yearOf(section.day);
      if (starts[starts.length - 1]?.year !== year) starts.push({ year, top: section.top });
    }
    return starts;
  });

  /** The year labels that fit, at least 16 px apart. */
  const railLabels = $derived.by(() => {
    const labels: { year: number; y: number }[] = [];
    if (layout.height <= 0) return labels;
    for (const start of yearStarts) {
      const y = (start.top / layout.height) * railHeight;
      if (labels.length === 0 || y - labels[labels.length - 1].y >= 16) {
        labels.push({ year: start.year, y });
      }
    }
    return labels;
  });

  /** The section at a point of the scrubber. */
  function sectionAtRail(y: number): Section | undefined {
    const fraction = Math.min(1, Math.max(0, y / railHeight));
    return layout.sections[sectionAt(fraction * layout.height)];
  }

  function railJump(event: PointerEvent) {
    const y = event.clientY - (event.currentTarget as HTMLElement).getBoundingClientRect().top;
    const fraction = Math.min(1, Math.max(0, y / railHeight));
    scroller.scrollTop = fraction * layout.height - clientHeight / 2;
  }

  function railMove(event: PointerEvent) {
    const y = event.clientY - (event.currentTarget as HTMLElement).getBoundingClientRect().top;
    const section = sectionAtRail(y);
    railHover = section?.day != null ? { y, label: formatMonth(section.day) } : null;
    if (railDragging) railJump(event);
  }

  /** The arrow keys move to the previous or next year. */
  function railKey(event: KeyboardEvent) {
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    let current = 0;
    yearStarts.forEach((start, index) => {
      if (start.top <= scrollTop + 1) current = index;
    });
    const target = yearStarts[Math.min(yearStarts.length - 1, Math.max(0, current + step))];
    if (target) scroller.scrollTop = target.top;
  }

  /** The days on screen (the sections are newest first). */
  const shownDays = $derived.by(() => {
    const { sections } = layout;
    const bottom = scrollTop + clientHeight;
    let newest: number | null = null;
    let oldest: number | null = null;
    for (let s = sectionAt(scrollTop); s < sections.length; s++) {
      const section = sections[s];
      if (section.top >= bottom) break;
      if (section.top + section.height <= scrollTop) continue;
      if (section.day === null) continue;
      // Newest first, or oldest first when sorted that way.
      newest = newest === null ? section.day : Math.max(newest, section.day);
      oldest = oldest === null ? section.day : Math.min(oldest, section.day);
    }
    return newest === null || oldest === null ? null : { oldest, newest };
  });
  $effect(() => onrange?.(shownDays));

  const visible = $derived.by(() => {
    const { sections, columns, cell, pitch } = layout;
    const headers: Section[] = [];
    /** `size` is the cell's width and height in pixels. */
    const cells: { index: number; x: number; y: number; width: number; height: number }[] = [];
    if (sections.length === 0 || cell <= 0) return { headers, cells, first: 0, last: -1 };

    const viewTop = scrollTop - OVERSCAN;
    const viewBottom = scrollTop + clientHeight + OVERSCAN;
    if (layout.river) {
      const rows = layout.riverRows;
      // First row reaching into the view, by binary search on `top`.
      let low = 0;
      let high = rows.length;
      while (low < high) {
        const mid = (low + high) >> 1;
        if (rows[mid].top + rows[mid].height < viewTop) low = mid + 1;
        else high = mid;
      }
      for (let r = low; r < rows.length && rows[r].top <= viewBottom; r++) {
        const row = rows[r];
        row.xs.forEach((x, k) =>
          cells.push({ index: row.start + k, x, y: row.top, width: row.widths[k], height: row.height }),
        );
      }
    }
    for (let s = sectionAt(viewTop); s < sections.length; s++) {
      const section = sections[s];
      if (section.top > viewBottom) break;
      headers.push(section);
      if (layout.river) continue;

      const gridTop = section.top + section.headerHeight;
      const { featuredWidth, featuredHeight } = section;
      const firstRow = Math.max(0, Math.floor((viewTop - gridTop) / pitch));
      const lastRow = Math.min(section.rows - 1, Math.floor((viewBottom - gridTop) / pitch));
      const add = (i: number, row: number, column: number, across = 1, down = 1) =>
        cells.push({
          index: section.start + i,
          x: SIDE_PADDING + column * pitch,
          y: gridTop + row * pitch,
          width: across * pitch - GAP,
          height: down * pitch - GAP,
        });

      if (featuredWidth > 0 && firstRow < featuredHeight && lastRow >= 0) {
        add(0, 0, 0, featuredWidth, featuredHeight);
      }
      const side = columns - featuredWidth;
      const beside = featuredHeight * side;
      for (let row = firstRow; row <= lastRow; row++) {
        if (featuredWidth > 0 && row < featuredHeight) {
          for (let slot = 0; slot < side; slot++) {
            add(1 + row * side + slot, row, featuredWidth + slot);
          }
        } else {
          // Photos before this row: the featured one and those beside it.
          const before =
            featuredWidth > 0 ? 1 + beside + (row - featuredHeight) * columns : row * columns;
          const rowEnd = Math.min(section.count, before + columns);
          for (let i = before; i < rowEnd; i++) add(i, row, i - before);
        }
      }
    }
    let first = 0;
    let last = -1;
    if (cells.length > 0) {
      first = cells[0].index;
      last = cells[0].index;
      for (const { index } of cells) {
        first = Math.min(first, index);
        last = Math.max(last, index);
      }
    }
    return { headers, cells, first, last };
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
    if (layout.river) {
      const rows = layout.riverRows;
      // Last row starting at or before the photo.
      let low = 0;
      let high = rows.length - 1;
      while (low < high) {
        const mid = (low + high + 1) >> 1;
        if (rows[mid].start <= index) low = mid;
        else high = mid - 1;
      }
      const row = rows[low];
      if (!row) return;
      if (row.top < scroller.scrollTop) scroller.scrollTop = Math.max(0, row.top - DAY_HEADER);
      else if (row.top + row.height > scroller.scrollTop + clientHeight) {
        scroller.scrollTop = row.top + row.height - clientHeight + GAP;
      }
      return;
    }
    const { sections, columns, pitch, cell } = layout;
    let section: Section | undefined;
    for (const candidate of sections) {
      if (candidate.start > index) break;
      section = candidate;
    }
    if (!section) return;
    const { row } = place(section, index - section.start, columns);
    const y = section.top + section.headerHeight + row * pitch;
    if (y < scroller.scrollTop) {
      scroller.scrollTop = Math.max(0, y - section.headerHeight);
    } else if (y + cell > scroller.scrollTop + clientHeight) {
      scroller.scrollTop = y + cell - clientHeight + GAP;
    }
  }

  /** Scrolls to a day (days since 1970), or the nearest older one listed. */
  export function scrollToDay(target: number) {
    const { sections } = layout;
    const ascending = (sections[0]?.day ?? 0) < (sections[sections.length - 1]?.day ?? 0);
    const section = sections.find(
      (candidate) =>
        candidate.day !== null && (ascending ? candidate.day >= target : candidate.day <= target),
    );
    if (section) scroller.scrollTop = section.top;
  }

  // Start at the day asked for, once there is a layout to scroll in.
  let started = false;
  $effect(() => {
    if (started || startAt === null || clientWidth === 0 || layout.sections.length === 0) return;
    started = true;
    scrollToDay(startAt);
    scrollTop = scroller.scrollTop;
  });
</script>

<div class="frame">
<div class="scroller" bind:this={scroller} bind:clientWidth bind:clientHeight {onscroll}>
  <div class="canvas" style:height="{layout.height}px">
    {#each visible.headers as section (section.day)}
      {#if section.day !== null}
        <header style:transform="translateY({section.top}px)" style:height="{section.headerHeight}px">
          {#if section.monthTitle}
            <h2>{section.monthTitle}</h2>
          {/if}
          <h3>
            {formatDay(section.day)}
            <span class="count">{formatNumber(section.count)}</span>
          </h3>
        </header>
      {/if}
    {/each}

    {#each visible.cells as cell (cell.index)}
      {@const photo = pages.get(cell.index)}
      {@const isSelected = selected.has(cell.index)}
      <div
        class="cell"
        class:selected={isSelected}
        style:transform="translate({cell.x}px, {cell.y}px)"
        style:width="{cell.width}px"
        style:height="{cell.height}px"
      >
        <button
          class="open"
          onclick={(event) => onclick(event, cell.index)}
          aria-label={photo?.relPath ?? "Photo"}
        >
          {#if photo?.edit}
            <EditedImage
              src={thumbUrl(photo.fingerprint, cell.width > 320 ? "preview" : "grid")}
              edit={photo.edit}
              width={photo.width}
              height={photo.height}
              fit="cover"
            />
          {:else if photo}
            <!-- A video without a thumbnail keeps the cell's plain background. -->
            <img
              src={thumbUrl(photo.fingerprint, cell.width > 320 ? "preview" : "grid")}
              alt=""
              draggable="false"
              decoding="async"
              onerror={(event) => ((event.currentTarget as HTMLImageElement).style.visibility = "hidden")}
              onload={(event) => ((event.currentTarget as HTMLImageElement).style.visibility = "")}
            />
          {/if}
        </button>
        {#if photo?.video}
          <span class="badge">▶ {photo.durationMs === null ? "" : formatDuration(photo.durationMs)}</span>
        {:else if photo?.liveVideo}
          <span class="badge">LIVE</span>
        {:else if photo?.rawFile}
          <!-- Short on small cells, so it leaves room for the heart. -->
          <span class="badge">{cell.width < 190 ? "RAW" : rawPairLabel(photo.relPath)}</span>
        {/if}
        {#if photo?.favorite}
          <span class="favorite" aria-label="Favorite">♥</span>
        {/if}
        <button
          class="check"
          role="checkbox"
          aria-checked={isSelected}
          aria-label="Select"
          onclick={(event) => ontoggle(cell.index, event.shiftKey)}
        >
          {isSelected ? "✓" : ""}
        </button>
      </div>
    {/each}
  </div>
</div>
  {#if showRail}
    <div
      class="rail"
      role="slider"
      tabindex="0"
      aria-label="Years"
      aria-orientation="vertical"
      aria-valuemin={yearStarts[yearStarts.length - 1]?.year}
      aria-valuemax={yearStarts[0]?.year}
      aria-valuenow={yearOf(layout.sections[sectionAt(scrollTop)]?.day ?? 0)}
      bind:clientHeight={railHeight}
      onpointerdown={(event) => {
        railDragging = true;
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
        railJump(event);
      }}
      onpointermove={railMove}
      onpointerup={() => (railDragging = false)}
      onpointercancel={() => (railDragging = false)}
      onpointerleave={() => (railHover = null)}
      onkeydown={railKey}
    >
      <span
        class="position"
        style:top="{(scrollTop / layout.height) * railHeight}px"
        style:height="{Math.max(3, (clientHeight / layout.height) * railHeight)}px"
      ></span>
      {#each railLabels as label (label.year)}
        <span class="year" style:top="{label.y}px">{label.year}</span>
      {/each}
      {#if railHover}
        <span class="hover" style:top="{railHover.y}px">{railHover.label}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .rail {
    position: relative;
    flex: none;
    width: 46px;
    margin: 8px 0;
    cursor: pointer;
    touch-action: none;
    user-select: none;
  }

  .rail:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .rail .position {
    position: absolute;
    right: 4px;
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
    opacity: 0.7;
  }

  .rail .year {
    position: absolute;
    right: 12px;
    color: var(--text-dim);
    font-size: 11px;
    line-height: 1;
    pointer-events: none;
  }

  .rail .hover {
    position: absolute;
    right: 50px;
    z-index: 2;
    padding: 3px 8px;
    border-radius: 6px;
    background: var(--surface);
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.2);
    color: var(--text);
    font-size: 12px;
    white-space: nowrap;
    transform: translateY(-50%);
    pointer-events: none;
  }

  .frame {
    display: flex;
    height: 100%;
  }

  .scroller {
    flex: 1;
    min-width: 0;
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
    background: var(--surface-hover);
  }

  .open {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
    overflow: hidden;
  }

  .open:focus-visible {
    outline-offset: -2px;
  }

  .cell.selected .open {
    outline: 3px solid var(--accent);
    outline-offset: -3px;
  }

  .cell.selected img {
    opacity: 0.8;
  }

  .favorite {
    position: absolute;
    left: 6px;
    bottom: 4px;
    color: #fff;
    font-size: 15px;
    text-shadow: 0 0 3px rgb(0 0 0 / 0.8);
    pointer-events: none;
  }

  /* The round checkbox in the corner: shown on hover, and always once
     anything is selected. */
  .badge {
    position: absolute;
    right: 6px;
    bottom: 6px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgb(0 0 0 / 0.55);
    color: #fff;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    pointer-events: none;
  }

  .check {
    position: absolute;
    top: 6px;
    left: 6px;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 2px solid #fff;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.25);
    color: #fff;
    font-size: 13px;
    line-height: 1;
    cursor: pointer;
    opacity: 0;
  }

  .cell:hover .check,
  .check:focus-visible,
  .cell.selected .check {
    opacity: 1;
  }

  .cell.selected .check {
    border-color: var(--accent);
    background: var(--accent);
  }

  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
