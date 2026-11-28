<!--
  The Collections page, as in Apple Photos: rows of Memories, Pinned,
  Albums, People, Featured Photos, Recent Days and Trips, then the Media
  Types and Utilities. Each row scrolls sideways; its title opens all of it.

  Memories, trips and featured photos are worked out by the backend (see
  collections.rs) and kept here between visits, so the page shows at once
  and refreshes quietly when the library changes.
-->
<script module lang="ts">
  import type { Collections, Listing } from "./api";
  import type { Icon } from "./icons";

  /** A pinned sidebar row. */
  export interface PinnedTile {
    id: string;
    label: string;
    icon: Icon;
    /** What it lists, for its cover photo; none for the map, People… */
    target: Listing | null;
    open: () => void;
  }

  /** The last answer per library, shown while the next one comes. */
  const remembered = new Map<string, { today: number; data: Collections }>();
</script>

<script lang="ts">
  import { errorText, t, tn } from "./i18n";
  import {
    collections,
    listPeople,
    photosPage,
    thumbUrl,
    todayDay,
    type Album,
    type Memory,
    type Only,
    type Person,
    type Trip,
  } from "./api";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatDay, formatDayRange, formatNumber } from "./format";
  import { ICONS } from "./icons";
  import { memorySubtitle, memoryTitle } from "./memories";
  import { showMenu } from "./contextMenu";
  import HomeDialog from "./HomeDialog.svelte";

  interface Props {
    libraryPath: string;
    revision: number;
    albums: Album[];
    pinned: PinnedTile[];
    /** Opens a photo listing, with the title to show over it (and opened on `photo`, a fingerprint). */
    onlisting: (listing: Listing, title: string, photo?: string) => void;
    onsection: (section: "people" | "map" | "duplicates" | "similar") => void;
    onperson: (person: Person) => void;
    onplay: (memory: Memory) => void;
    /** The names of the favourite people, shown first. */
    favoritePeople: string[];
    /** Saves a memory as a video. */
    onexport: (memory: Memory) => void;
  }
  let { libraryPath, revision, albums, pinned, onlisting, onsection, onperson, onplay, favoritePeople, onexport }: Props =
    $props();

  /** The whole page, or all the memories, or all the trips. */
  let page = $state<"home" | "memories" | "trips">("home");
  const today = todayDay();
  let data = $state.raw<Collections | null>(null);
  let everyone = $state.raw<Person[]>([]);
  // Favourites first, then the people with the most photos.
  const people = $derived(
    everyone
      .filter((person) => person.name && !person.hidden)
      .map((person) => ({ person, favorite: favoritePeople.includes(person.name!) }))
      .sort((a, b) => Number(b.favorite) - Number(a.favorite) || b.person.photos - a.person.photos)
      .slice(0, 20),
  );
  let error = $state<string | null>(null);
  /** The "Where is home?" dialog is open. */
  let settingHome = $state(false);
  let pinnedCovers = $state<Record<string, string>>({});

  async function load() {
    try {
      const [found, listed] = await Promise.all([collections(libraryPath, today), listPeople(libraryPath)]);
      remembered.set(libraryPath, { today, data: found });
      data = found;
      everyone = listed;
      error = null;
    } catch (e) {
      error = errorText(e);
    }
  }

  // At once from memory, then fresh. While the library changes (a scan),
  // at most every few seconds.
  let reloadTimer: ReturnType<typeof setTimeout> | undefined;
  let lastRevision = -1;
  $effect(() => {
    const known = remembered.get(libraryPath);
    if (known && known.today === today && data === null) data = known.data;
    if (lastRevision === -1) {
      lastRevision = revision;
      void load();
      return;
    }
    if (revision === lastRevision) return;
    lastRevision = revision;
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(load, 4000);
  });
  $effect(() => () => clearTimeout(reloadTimer));

  // A cover photo for each pinned row that lists photos.
  $effect(() => {
    for (const tile of pinned) {
      if (!tile.target || pinnedCovers[tile.id]) continue;
      photosPage(libraryPath, tile.target, "dateDesc", 0, 1)
        .then(([first]) => {
          if (first) pinnedCovers = { ...pinnedCovers, [tile.id]: first.fingerprint };
        })
        .catch(() => {});
    }
  });

  const todays = $derived(data ? data.today.map((i) => data!.memories[i]).filter(Boolean) : []);

  /** All memories or trips, by year, newest first. */
  function byYear<T extends { toDay: number }>(items: T[]): [number, T[]][] {
    const years = new Map<number, T[]>();
    for (const item of items) {
      const year = new Date(item.toDay * 86_400_000).getUTCFullYear();
      if (!years.has(year)) years.set(year, []);
      years.get(year)!.push(item);
    }
    return [...years];
  }

  const MEDIA: [Only, string, Icon][] = [
    ["videos", t("Videos"), "video"],
    ["selfies", t("Selfies"), "selfie"],
    ["livePhotos", t("Live Photos"), "live"],
    ["portraits", t("Portrait"), "portrait"],
    ["panoramas", t("Panoramas"), "panorama"],
    ["timeLapses", t("Time-lapse"), "timeLapse"],
    ["sloMos", t("Slo-mo"), "sloMo"],
    ["bursts", t("Bursts"), "burst"],
    ["screenshots", t("Screenshots"), "screenshot"],
    ["raw", t("RAW"), "raw"],
  ];
  const media = $derived(MEDIA.filter(([only]) => (data?.media[only] ?? 0) > 0));
  const mediaListing = (only: Only): Listing => ({ kind: "only", within: { kind: "library" }, only });

  const memoryMenu = (event: MouseEvent, memory: Memory) =>
    showMenu(event, [
      { label: t("Play Memory"), action: () => onplay(memory) },
      { label: t("Show the Photos"), action: () => onlisting(memory.listing, memoryTitle(memory)) },
      { label: t("Save as a Video…"), action: () => onexport(memory) },
    ]);
  const tripListing = (trip: Trip): Listing => ({ kind: "days", from: trip.fromDay, to: trip.toDay });
  const tripMemory = (trip: Trip) => data?.memories.find((memory) => memory.id === trip.id);

  /** Scrolls a row by most of its width. */
  function scrollRow(button: HTMLElement, direction: 1 | -1) {
    const row = button.parentElement?.querySelector(".row");
    row?.scrollBy({ left: direction * row.clientWidth * 0.8, behavior: "smooth" });
  }
</script>

{#snippet icon(name: Icon, size = 18)}
  <svg class="icon" viewBox="0 0 20 20" width={size} height={size} aria-hidden="true">
    {#each ICONS[name] as d, i (i)}<path {d} />{/each}
  </svg>
{/snippet}

{#snippet header(label: string, open: (() => void) | null)}
  <h2>
    {#if open}
      <button class="section-title" onclick={open}>
        {label}
        <svg viewBox="0 0 10 10" width="11" height="11" aria-hidden="true"><path d="M3.5 1.5 7 5l-3.5 3.5" /></svg>
      </button>
    {:else}
      <span class="section-title">{label}</span>
    {/if}
  </h2>
{/snippet}

{#snippet scroller()}
  <button class="arrow left" onclick={(event) => scrollRow(event.currentTarget, -1)} aria-label={t("Scroll left")}>
    ‹
  </button>
  <button class="arrow right" onclick={(event) => scrollRow(event.currentTarget, 1)} aria-label={t("Scroll right")}>
    ›
  </button>
{/snippet}

{#snippet memoryCard(memory: Memory, large: boolean)}
  <button
    class="memory"
    class:large
    onclick={() => onplay(memory)}
    oncontextmenu={(event) => memoryMenu(event, memory)}
    title={t("Play this memory")}
  >
    <img src={thumbUrl(memory.cover, "preview")} alt="" loading="lazy" draggable="false" />
    <span class="memory-text">
      <span class="memory-title">{memoryTitle(memory)}</span>
      <span class="memory-subtitle">{memorySubtitle(memory, today)}</span>
    </span>
    <span class="play" aria-hidden="true">
      <svg viewBox="0 0 20 20" width="14" height="14"><path d="M6 4v12l10-6z" /></svg>
    </span>
  </button>
{/snippet}

{#snippet tripCard(trip: Trip, wide: boolean)}
  {@const memory = tripMemory(trip)}
  <div class="trip" class:wide>
    <button class="trip-open" onclick={() => onlisting(tripListing(trip), trip.title)}>
      <img src={thumbUrl(trip.cover, "preview")} alt="" loading="lazy" draggable="false" />
      <span class="trip-text">
        <span class="trip-title">{trip.title}</span>
        <span class="trip-date">{formatDayRange(trip.fromDay, trip.toDay)} · {formatNumber(trip.count)}</span>
      </span>
    </button>
    {#if memory}
      <button class="trip-play" onclick={() => onplay(memory)} aria-label={t("Play this memory")} title={t("Play this memory")}>
        <svg viewBox="0 0 20 20" width="13" height="13" aria-hidden="true"><path d="M6 4v12l10-6z" /></svg>
      </button>
    {/if}
  </div>
{/snippet}

<div class="collections">
  {#if page !== "home"}
    <div class="subpage-bar">
      <button class="secondary" onclick={() => (page = "home")}>‹ {t("Collections")}</button>
      <strong>{page === "memories" ? t("Memories") : t("Trips")}</strong>
    </div>
  {/if}

  {#if error && !data}
    <p class="error" role="alert">{error}</p>
  {:else if !data}
    <div class="loading">
      {#each { length: 3 } as _, i (i)}<div class="skeleton"></div>{/each}
    </div>
  {:else if page === "memories"}
    {#each byYear(data.memories) as [year, memories] (year)}
      <h3 class="year">{year}</h3>
      <div class="grid memories-grid">
        {#each memories as memory (memory.id)}{@render memoryCard(memory, false)}{/each}
      </div>
    {/each}
  {:else if page === "trips"}
    {#each byYear(data.trips) as [year, trips] (year)}
      <h3 class="year">{year}</h3>
      <div class="grid trips-grid">
        {#each trips as trip (trip.id)}{@render tripCard(trip, true)}{/each}
      </div>
    {/each}
  {:else}
    {#if todays.length > 0}
      <section>
        {@render header(t("Memories"), () => (page = "memories"))}
        <div class="row-wrap">
          <div class="row">
            {#each todays as memory (memory.id)}{@render memoryCard(memory, true)}{/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if pinned.length > 0}
      <section>
        {@render header(t("Pinned"), null)}
        <div class="row-wrap">
          <div class="row">
            {#each pinned as tile (tile.id)}
              <button class="tile" onclick={tile.open}>
                <span class="tile-image" class:empty={!pinnedCovers[tile.id]}>
                  {#if pinnedCovers[tile.id]}
                    <img src={thumbUrl(pinnedCovers[tile.id])} alt="" loading="lazy" draggable="false" />
                    <span class="badge">{@render icon(tile.icon, 15)}</span>
                  {:else}
                    {@render icon(tile.icon, 34)}
                  {/if}
                </span>
                <span class="tile-name">{tile.label}</span>
              </button>
            {/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if albums.length > 0}
      <section>
        {@render header(t("Albums"), null)}
        <div class="row-wrap">
          <div class="row">
            {#each albums as album (album.id)}
              <button class="tile" onclick={() => onlisting({ kind: "album", album: album.id }, album.name)}>
                <span class="tile-image" class:empty={!album.cover}>
                  {#if album.cover}
                    <img src={thumbUrl(album.cover)} alt="" loading="lazy" draggable="false" />
                  {:else}
                    {@render icon("album", 34)}
                  {/if}
                </span>
                <span class="tile-name">{album.name}</span>
                <span class="tile-count">{formatNumber(album.count)}</span>
              </button>
            {/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if people.length > 0 || data.pets}
      {@const petsOnly = people.length === 0 ? data.pets : null}
      <section>
        {@render header(petsOnly ? t("Pets") : t("People"), () =>
          petsOnly ? onlisting(petsOnly.listing, t("Pets")) : onsection("people"),
        )}
        <div class="row-wrap">
          <div class="row">
            {#each people as { person, favorite } (person.clusters[0])}
              <button class="person" onclick={() => onperson(person)}>
                <span class="face">
                  <EditedImage
                    src={thumbUrl(person.cover, "preview")}
                    edit={{ ...NO_EDIT, crop: person.coverCrop }}
                    width={person.coverWidth}
                    height={person.coverHeight}
                    fit="cover"
                  />
                </span>
                <span class="tile-name">
                  {#if favorite}<span class="favorite" aria-label={t("Favorites")}>♥</span>{/if}{person.name}
                </span>
              </button>
            {/each}
            {#if data.pets}
              {@const pets = data.pets}
              <button class="person" onclick={() => onlisting(pets.listing, t("Pets"))}>
                <span class="face">
                  <img src={thumbUrl(pets.cover, "preview")} alt="" loading="lazy" draggable="false" />
                </span>
                <span class="tile-name">🐾 {t("Pets")}</span>
              </button>
            {/if}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if data.featured.length > 0}
      {@const featured: Listing = { kind: "picked", fingerprints: data.featured }}
      <section>
        {@render header(t("Featured Photos"), () => onlisting(featured, t("Featured Photos")))}
        <div class="row-wrap">
          <div class="row">
            {#each data.featured.slice(0, 16) as fingerprint (fingerprint)}
              <button class="featured" onclick={() => onlisting(featured, t("Featured Photos"), fingerprint)}>
                <img src={thumbUrl(fingerprint, "preview")} alt="" loading="lazy" draggable="false" />
              </button>
            {/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if data.recentDays.length > 0}
      <section>
        {@render header(t("Recent Days"), () => onlisting({ kind: "library" }, t("Library")))}
        <div class="row-wrap">
          <div class="row">
            {#each data.recentDays as day (day.day)}
              <button class="day" onclick={() => onlisting({ kind: "days", from: day.day, to: day.day }, formatDay(day.day))}>
                <img src={thumbUrl(day.cover, "preview")} alt="" loading="lazy" draggable="false" />
                <span class="day-text">
                  <span>{formatDay(day.day)}</span>
                  <span class="dim">{tn(day.count, "{n} item", "{n} items")}</span>
                </span>
              </button>
            {/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}

    {#if data.trips.length > 0}
      <section>
        {@render header(t("Trips"), () => (page = "trips"))}
        <div class="row-wrap">
          <div class="row">
            {#each data.trips as trip (trip.id)}{@render tripCard(trip, false)}{/each}
          </div>
          {@render scroller()}
        </div>
      </section>
    {/if}
    <!-- Trips are the days far from home: where home is can be set by hand. -->
    <p class="home-line">
      {data.home ? t("Home: {town}", { town: data.home }) : t("Home isn't known yet")}
      · <button class="link" onclick={() => (settingHome = true)}>{t("Change…")}</button>
    </p>

    {#if media.length > 0}
      <section>
        {@render header(t("Media Types"), null)}
        <div class="list">
          {#each media as [only, name, kind] (only)}
            <button class="list-row" onclick={() => onlisting(mediaListing(only), name)}>
              {@render icon(kind)}
              <span class="list-name">{name}</span>
              <span class="list-count">{formatNumber(data.media[only] ?? 0)}</span>
              <span class="chevron" aria-hidden="true">›</span>
            </button>
          {/each}
        </div>
      </section>
    {/if}

    <section>
      {@render header(t("Utilities"), null)}
      <div class="list">
        <button class="list-row" onclick={() => onlisting({ kind: "favorites" }, t("Favorites"))}>
          {@render icon("heart")}<span class="list-name">{t("Favorites")}</span>
          <span class="list-count">{formatNumber(data.favorites)}</span><span class="chevron" aria-hidden="true">›</span>
        </button>
        <button class="list-row" onclick={() => onlisting({ kind: "hidden" }, t("Hidden"))}>
          {@render icon("hidden")}<span class="list-name">{t("Hidden")}</span>
          <span class="list-count">{formatNumber(data.hidden)}</span><span class="chevron" aria-hidden="true">›</span>
        </button>
        <button class="list-row" onclick={() => onsection("duplicates")}>
          {@render icon("duplicates")}<span class="list-name">{t("Duplicates")}</span>
          <span class="list-count">{formatNumber(data.duplicates)}</span><span class="chevron" aria-hidden="true">›</span>
        </button>
        <button class="list-row" onclick={() => onsection("similar")}>
          {@render icon("similar")}<span class="list-name">{t("Similar shots")}</span>
          <span class="list-count"></span><span class="chevron" aria-hidden="true">›</span>
        </button>
        <button class="list-row" onclick={() => onsection("map")}>
          {@render icon("map")}<span class="list-name">{t("Map")}</span>
          <span class="list-count"></span><span class="chevron" aria-hidden="true">›</span>
        </button>
        <button class="list-row" onclick={() => onsection("people")}>
          {@render icon("person")}<span class="list-name">{t("People")}</span>
          <span class="list-count"></span><span class="chevron" aria-hidden="true">›</span>
        </button>
      </div>
    </section>
  {/if}
</div>

{#if settingHome && data}
  <HomeDialog
    {libraryPath}
    current={data.home}
    canEdit={true}
    onchanged={load}
    onclose={() => (settingHome = false)}
  />
{/if}

<style>
  .collections {
    height: 100%;
    overflow-y: auto;
    padding: 8px 28px 48px;
    box-sizing: border-box;
  }
  .subpage-bar {
    position: sticky;
    top: -8px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 0;
    background: var(--bg);
  }
  section {
    margin-top: 18px;
  }
  h2 {
    margin: 0 0 10px;
    font-size: 20px;
  }
  .section-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-weight: 700;
    color: inherit;
    background: none;
    border: none;
    padding: 0;
  }
  button.section-title:hover {
    color: var(--accent);
  }
  .section-title svg path {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.8;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .year {
    margin: 22px 0 10px;
    font-size: 22px;
  }

  .row-wrap {
    position: relative;
  }
  .row {
    display: flex;
    gap: 12px;
    overflow-x: auto;
    scroll-snap-type: x proximity;
    padding-bottom: 6px;
    scrollbar-width: none;
  }
  .row::-webkit-scrollbar {
    display: none;
  }
  .row > :global(*) {
    scroll-snap-align: start;
    flex: none;
  }
  .arrow {
    position: absolute;
    top: 50%;
    transform: translateY(-60%);
    width: 34px;
    height: 34px;
    border-radius: 50%;
    border: none;
    background: rgb(255 255 255 / 0.92);
    color: #222;
    font-size: 22px;
    line-height: 1;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.25);
    opacity: 0;
    transition: opacity 0.2s;
  }
  .arrow.left {
    left: -14px;
  }
  .arrow.right {
    right: -14px;
  }
  .row-wrap:hover .arrow {
    opacity: 1;
  }

  button.memory,
  button.tile,
  button.person,
  button.featured,
  button.day,
  .trip-open,
  .list-row {
    font: inherit;
    color: inherit;
    background: none;
    border: none;
    padding: 0;
    text-align: left;
  }

  .memory {
    position: relative;
    width: 230px;
    height: 300px;
    border-radius: 14px;
    overflow: hidden;
    background: var(--surface);
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.12);
    transition: transform 0.2s;
  }
  .memory.large {
    width: 280px;
    height: 370px;
  }
  .memory:hover {
    transform: scale(1.02);
  }
  .memory img,
  .featured img,
  .day img,
  .trip-open img,
  .tile-image img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .memory::after {
    content: "";
    position: absolute;
    inset: 45% 0 0;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.65));
  }
  .memory-text {
    position: absolute;
    left: 16px;
    right: 16px;
    bottom: 16px;
    z-index: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: #fff;
    text-shadow: 0 1px 8px rgb(0 0 0 / 0.4);
  }
  .memory-title {
    font-size: 22px;
    font-weight: 700;
    line-height: 1.1;
    letter-spacing: -0.01em;
    overflow-wrap: anywhere;
  }
  .memory:not(.large) .memory-title {
    font-size: 18px;
  }
  .memory-subtitle {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    opacity: 0.9;
  }
  .play {
    position: absolute;
    right: 12px;
    top: 12px;
    z-index: 1;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.35);
    backdrop-filter: blur(6px);
    opacity: 0;
    transition: opacity 0.2s;
  }
  .play path,
  .trip-play path {
    fill: #fff;
  }
  .memory:hover .play {
    opacity: 1;
  }

  .tile,
  .person {
    width: 150px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tile-image {
    position: relative;
    width: 150px;
    height: 150px;
    border-radius: 10px;
    overflow: hidden;
    display: grid;
    place-items: center;
    margin-bottom: 4px;
  }
  .tile-image.empty {
    background: var(--surface);
    color: var(--text-dim);
  }
  .badge {
    position: absolute;
    left: 8px;
    bottom: 8px;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgb(255 255 255 / 0.85);
    color: #222;
  }
  .tile-name {
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tile-count {
    font-size: 12px;
    color: var(--text-dim);
  }
  .person {
    width: 120px;
    align-items: center;
    text-align: center;
  }
  .face {
    position: relative;
    width: 112px;
    height: 112px;
    border-radius: 50%;
    overflow: hidden;
    display: block;
    margin-bottom: 6px;
    background: var(--surface);
    transition: transform 0.2s;
  }
  .person:hover .face {
    transform: scale(1.04);
  }

  .featured {
    width: 190px;
    height: 190px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
  }
  .day {
    position: relative;
    width: 180px;
    height: 180px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
  }
  .day-text,
  .trip-text {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    flex-direction: column;
    padding: 24px 10px 8px;
    color: #fff;
    font-size: 12px;
    font-weight: 600;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.6));
  }
  .day-text .dim,
  .trip-date {
    font-weight: 500;
    opacity: 0.85;
  }

  .trip {
    position: relative;
    width: 240px;
    height: 180px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
  }
  .trip.wide {
    width: 100%;
  }
  .trip-open {
    width: 100%;
    height: 100%;
  }
  .trip-title {
    font-size: 16px;
    font-weight: 700;
  }
  .trip-play {
    position: absolute;
    right: 10px;
    top: 10px;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    border: none;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.4);
    backdrop-filter: blur(6px);
  }

  .list {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 240px), 1fr));
    column-gap: 28px;
  }
  .list-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 2px;
    border-bottom: 1px solid var(--border);
  }
  .list-row:hover {
    color: var(--accent);
  }
  .list-name {
    flex: 1;
  }
  .list-count,
  .chevron {
    color: var(--text-dim);
    font-size: 13px;
  }
  .icon path {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .grid {
    display: grid;
    gap: 14px;
  }
  .memories-grid {
    grid-template-columns: repeat(auto-fill, 230px);
  }
  .trips-grid {
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  }

  .loading {
    display: flex;
    gap: 12px;
    margin-top: 48px;
  }
  .skeleton {
    width: 280px;
    height: 370px;
    border-radius: 14px;
    background: var(--surface);
    animation: pulse 1.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.5;
    }
  }
  .error {
    color: var(--danger);
  }

  .favorite {
    margin-right: 4px;
    color: var(--danger);
  }

  /* The pets card: a photo in the circle, as a person's face. */
  .face img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .home-line {
    margin: 4px 0 18px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .home-line .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
</style>
