<!--
  The Years and Months overviews of a listing.

  - "years": one large card per year, with a cover photo.
  - "months": one block per month, a mosaic of a few of its photos, the
    first one large.

  Clicking a year opens the months at that year; clicking a month opens the
  photo grid at that month.
-->
<script lang="ts">
  import { thumbUrl, type MonthSummary } from "./api";
  import { formatMonth, photoCount } from "./format";

  interface Props {
    mode: "years" | "months";
    months: MonthSummary[];
    /** A year to start scrolled to, in the months mode. */
    startYear?: number | null;
    onyear: (year: number) => void;
    /** Called with `year * 12 + month - 1`. */
    onmonth: (month: number) => void;
  }
  let { mode, months, startYear = null, onyear, onmonth }: Props = $props();

  const DAY_MS = 86_400_000;
  const monthTitle = (entry: MonthSummary) =>
    formatMonth(Date.UTC(entry.year, entry.month - 1, 1) / DAY_MS);

  const years = $derived.by(() => {
    const byYear = new Map<number, { year: number; count: number; cover: string }>();
    for (const entry of months) {
      const known = byYear.get(entry.year);
      if (known) known.count += entry.count;
      else byYear.set(entry.year, { year: entry.year, count: entry.count, cover: entry.covers[0] });
    }
    return [...byYear.values()];
  });

  /** Scrolls the first month of `startYear` into view when it is rendered. */
  function scrollIfStart(node: HTMLElement, isStart: boolean) {
    if (isStart) node.scrollIntoView({ block: "start" });
  }
</script>

<div class="scroller">
  {#if mode === "years"}
    <div class="years">
      {#each years as entry (entry.year)}
        <button class="year" onclick={() => onyear(entry.year)}>
          <img src={thumbUrl(entry.cover, "preview")} alt="" loading="lazy" draggable="false" />
          <span class="label">
            <strong>{entry.year}</strong>
            <span>{photoCount(entry.count)}</span>
          </span>
        </button>
      {/each}
    </div>
  {:else}
    {#each months as entry, index (entry.year * 12 + entry.month)}
      <section
        use:scrollIfStart={entry.year === startYear && months[index - 1]?.year !== startYear}
      >
        <button
          class="month"
          class:few={entry.covers.length < 7}
          onclick={() => onmonth(entry.year * 12 + entry.month - 1)}
          aria-label="{monthTitle(entry)}, {photoCount(entry.count)}"
        >
          {#each entry.covers.slice(0, entry.covers.length < 7 ? 1 : 7) as cover, position (cover)}
            <img
              src={thumbUrl(cover, position === 0 ? "preview" : "grid")}
              alt=""
              loading="lazy"
              draggable="false"
            />
          {/each}
          <span class="label">
            <strong>{monthTitle(entry)}</strong>
            <span>{photoCount(entry.count)}</span>
          </span>
        </button>
      </section>
    {/each}
  {/if}
</div>

<style>
  .scroller {
    height: 100%;
    overflow-y: auto;
    padding: 16px;
  }

  .years {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 16px;
  }

  button {
    position: relative;
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    border-radius: 12px;
    background: var(--surface-hover);
    overflow: hidden;
    cursor: pointer;
  }

  button:hover img {
    filter: brightness(1.08);
  }

  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .year {
    aspect-ratio: 3 / 2;
  }

  section {
    max-width: 1100px;
    margin: 0 auto 16px;
    /* Lets the browser skip the months that are off screen. */
    content-visibility: auto;
    contain-intrinsic-size: auto 360px;
  }

  /* A mosaic 6 cells wide and 2 high: the first photo takes 3 by 2 on the
     left, the next six fill the right half. */
  .month {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    grid-template-rows: repeat(2, 1fr);
    gap: 3px;
    aspect-ratio: 3 / 1;
  }

  .month img:first-child {
    grid-column: span 3;
    grid-row: span 2;
  }

  /* With only a few photos, one wide picture. */
  .month.few {
    display: block;
    aspect-ratio: 3 / 1;
  }

  .label {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 18px 40px;
    background: linear-gradient(rgb(0 0 0 / 0.55), transparent);
    color: #fff;
    text-align: left;
    pointer-events: none;
  }

  .label strong {
    font-size: 22px;
    font-weight: 700;
  }

  .label span {
    font-size: 13px;
  }

  .year .label strong {
    font-size: 30px;
  }
</style>
