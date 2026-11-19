<!--
  The Years and Months overviews of a listing.

  - "years": a square card per year, with a cover photo.
  - "months": each month shows its busiest days (at most four) as cards with
    the day of the month in the corner. The busiest day gets the large card
    when there are three or four, on the left and on the right in turn; one
    or two days share the width evenly.

  Clicking a year zooms into the months at that year. Clicking a day, or a
  month's title, opens the photo grid there.
-->
<script lang="ts">
  import { t } from "./i18n";
  import { flushSync } from "svelte";
  import { thumbUrl, type DaySummary, type MonthSummary } from "./api";
  import { formatMonth, photoCount } from "./format";

  interface Props {
    mode: "years" | "months";
    months: MonthSummary[];
    /** A year to start scrolled to, in the months mode. */
    startYear?: number | null;
    onyear: (year: number) => void;
    /**
     * Called with a day (days since 1970) to open the photo grid at. For a
     * month's title it is the month's last day, and `month` is true.
     */
    onday: (day: number, month?: boolean) => void;
    /** Starts a slideshow of the month that `day` is in. */
    onslideshow?: (day: number) => void;
  }
  let { mode, months, startYear = null, onyear, onday, onslideshow }: Props = $props();

  const DAY_MS = 86_400_000;
  const monthTitle = (entry: MonthSummary) =>
    formatMonth(Date.UTC(entry.year, entry.month - 1, 1) / DAY_MS);
  /** The last day of a month: the grid lists newest first, so it starts there. */
  const lastDay = (entry: MonthSummary) => Date.UTC(entry.year, entry.month, 0) / DAY_MS;
  const dayOfMonth = (day: number) => new Date(day * DAY_MS).getUTCDate();

  /** The cards of a month in the order they are laid out. */
  function cards(entry: MonthSummary): DaySummary[] {
    if (entry.days.length <= 2) return entry.days;
    // The busiest day takes the large card; the rest follow in date order.
    const busiest = entry.days.reduce((best, day) => (day.count > best.count ? day : best));
    return [busiest, ...entry.days.filter((day) => day !== busiest)];
  }

  /**
   * The months whose large card goes on the right: every other month that
   * has one, counted down the page, as in Apple Photos.
   */
  const mirrored = $derived.by(() => {
    const keys = new Set<number>();
    let large = 0;
    for (const entry of months) {
      if (entry.days.length <= 2) continue;
      if (large % 2 === 1) keys.add(entry.year * 12 + entry.month);
      large++;
    }
    return keys;
  });

  const years = $derived.by(() => {
    // Each year shows the cover of its busiest day.
    const byYear = new Map<number, { year: number; count: number; cover: DaySummary }>();
    for (const entry of months) {
      const busiest = entry.days.reduce((best, day) => (day.count > best.count ? day : best));
      const known = byYear.get(entry.year);
      if (!known) {
        byYear.set(entry.year, { year: entry.year, count: entry.count, cover: busiest });
        continue;
      }
      known.count += entry.count;
      if (busiest.count > known.cover.count) known.cover = busiest;
    }
    return [...byYear.values()];
  });

  const reducedMotion =
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  /**
   * Opens a year. Where the webview can animate between two states of the
   * page, the year's card grows into its months, as in Apple Photos.
   */
  function openYear(year: number, card: HTMLElement) {
    if (reducedMotion || !("startViewTransition" in document)) {
      onyear(year);
      return;
    }
    card.style.viewTransitionName = "zoom";
    document.startViewTransition(() => {
      card.style.viewTransitionName = "";
      onyear(year);
      flushSync();
    });
  }

  /** Scrolls the first month of `startYear` into view when it is rendered. */
  function scrollIfStart(node: HTMLElement, isStart: boolean) {
    if (isStart) node.scrollIntoView({ block: "start" });
  }
</script>

<div class="scroller">
  {#if mode === "years"}
    <div class="years">
      {#each years as entry (entry.year)}
        <button
          class="card year"
          onclick={(event) => openYear(entry.year, event.currentTarget)}
          aria-label="{entry.year}, {photoCount(entry.count)}"
        >
          <img src={thumbUrl(entry.cover.cover, "preview")} alt="" loading="lazy" draggable="false" />
          <span class="year-label">{entry.year}</span>
        </button>
      {/each}
    </div>
  {:else}
    {#each months as entry, index (entry.year * 12 + entry.month)}
      {@const isStart = entry.year === startYear && months[index - 1]?.year !== startYear}
      <section use:scrollIfStart={isStart} class:zoom-target={isStart}>
        <h2>
          <button class="title" onclick={() => onday(lastDay(entry), true)}>{monthTitle(entry)}</button>
          <span class="count">{photoCount(entry.count)}</span>
          {#if onslideshow}
            <button
              class="play"
              onclick={() => onslideshow(lastDay(entry))}
              title={t("Slideshow of {month}", { month: monthTitle(entry) })}
              aria-label={t("Slideshow of {month}", { month: monthTitle(entry) })}
            >
              <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"><path d="M4 2.5v11L13.5 8z" /></svg>
            </button>
          {/if}
        </h2>
        <div
          class="days"
          class:mirrored={mirrored.has(entry.year * 12 + entry.month)}
          data-cards={entry.days.length}
        >
          {#each cards(entry) as day, position (day.day)}
            <button
              class="card"
              class:large={position === 0 && entry.days.length > 2}
              onclick={() => onday(day.day)}
              aria-label="{monthTitle(entry)} {dayOfMonth(day.day)}, {photoCount(day.count)}"
            >
              <img src={thumbUrl(day.cover, "preview")} alt="" loading="lazy" draggable="false" />
              <span class="day-label">{dayOfMonth(day.day)}</span>
            </button>
          {/each}
        </div>
      </section>
    {/each}
  {/if}
</div>

<style>
  .scroller {
    height: 100%;
    overflow-y: auto;
    padding: 20px 16px;
  }

  /* Both overviews sit in a centered column, as in Apple Photos. */
  .years,
  section {
    max-width: 1040px;
    margin: 0 auto;
  }

  .card {
    position: relative;
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    border-radius: 12px;
    background: var(--surface-hover);
    box-shadow:
      0 1px 2px rgb(0 0 0 / 0.12),
      0 6px 18px rgb(0 0 0 / 0.1);
    overflow: hidden;
    cursor: pointer;
  }

  .card:hover img {
    filter: brightness(1.08);
  }

  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  /* --- Years ------------------------------------------------------------- */

  .years {
    display: grid;
    /* Three per row at full width, fewer when the window is narrow. */
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 260px), 1fr));
    gap: 32px;
  }

  .year {
    aspect-ratio: 1;
  }

  .year-label,
  .day-label {
    position: absolute;
    top: 0;
    left: 0;
    color: #fff;
    text-shadow: 0 1px 6px rgb(0 0 0 / 0.55);
    pointer-events: none;
  }

  .year-label {
    padding: 18px 22px;
    font-size: 28px;
    font-weight: 700;
  }

  /* --- Months ------------------------------------------------------------ */

  section {
    margin-bottom: 40px;
    /* Lets the browser skip the months that are off screen. */
    content-visibility: auto;
    contain-intrinsic-size: auto 600px;
  }

  /* The year card that was clicked grows into this month (see openYear). */
  section.zoom-target .days {
    view-transition-name: zoom;
  }

  h2 {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin: 0 0 12px;
  }

  .title {
    padding: 0;
    border: none;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 22px;
    font-weight: 700;
    cursor: pointer;
  }

  .title:hover {
    text-decoration: underline;
  }

  .count {
    color: var(--text-dim);
    font-size: 13px;
    font-weight: 400;
  }

  .play {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--surface-hover);
    color: var(--text-dim);
    cursor: pointer;
    opacity: 0.55;
    transition: opacity 0.15s;
    align-self: center;
  }

  h2:hover .play,
  .play:focus-visible {
    opacity: 1;
  }

  .play:hover {
    background: var(--accent);
    color: var(--accent-text);
  }

  .play svg {
    fill: currentColor;
  }

  .days {
    display: grid;
    gap: 20px;
  }

  /* One day: a single wide card. */
  .days[data-cards="1"] {
    aspect-ratio: 5 / 2;
  }

  /* Two days: two squares side by side. */
  .days[data-cards="2"] {
    grid-template-columns: 1fr 1fr;
  }

  .days[data-cards="2"] .card {
    aspect-ratio: 1;
  }

  /* Three or four: the busiest day large on the left, the others stacked
     on the right. */
  .days[data-cards="3"],
  .days[data-cards="4"] {
    grid-template-columns: 2.4fr 1fr;
    aspect-ratio: 1.9 / 1;
  }

  .days[data-cards="3"] {
    grid-template-rows: repeat(2, minmax(0, 1fr));
  }

  .days[data-cards="4"] {
    grid-template-rows: repeat(3, minmax(0, 1fr));
  }

  .large {
    grid-row: 1 / -1;
  }

  /* Every other such month: the stacked cards on the left. */
  .days.mirrored {
    grid-template-columns: 1fr 2.4fr;
  }

  .mirrored .large {
    grid-column: 2;
  }

  .day-label {
    padding: 10px 12px;
    font-size: 15px;
    font-weight: 600;
  }

  .large .day-label,
  .days[data-cards="1"] .day-label,
  .days[data-cards="2"] .day-label {
    padding: 14px 16px;
    font-size: 17px;
  }

  /* On a narrow window the stacked cards would get too small: one column. */
  @media (max-width: 640px) {
    .days[data-cards="3"],
    .days[data-cards="4"] {
      grid-template-columns: 1fr;
      grid-template-rows: none;
      aspect-ratio: auto;
    }

    .days[data-cards="3"] .card,
    .days[data-cards="4"] .card {
      aspect-ratio: 3 / 2;
    }

    .large,
    .mirrored .large {
      grid-row: auto;
      grid-column: auto;
    }
  }

  /* The zoom from a year into its months. */
  :global(::view-transition-group(zoom)) {
    animation-duration: 0.35s;
    animation-timing-function: cubic-bezier(0.2, 0.8, 0.2, 1);
  }
</style>
