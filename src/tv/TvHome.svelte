<!--
  The TV app's home: rows of large cards, as on a TV's own apps. Memories
  play with their music; the other cards open a grid of photos.
-->
<script lang="ts">
  import {
    collections,
    listAlbums,
    listPeople,
    thumbUrl,
    todayDay,
    type Album,
    type Collections,
    type Listing,
    type Memory,
    type Person,
  } from "../lib/api";
  import EditedImage from "../lib/EditedImage.svelte";
  import { NO_EDIT } from "../lib/edit";
  import { formatDayRange, photoCount } from "../lib/format";
  import { errorText, locale, t } from "../lib/i18n";
  import { memorySubtitle, memoryTitle } from "../lib/memories";
  import { tick } from "svelte";
  import { focusFirst } from "./remote";

  interface Props {
    libraryPath: string;
    libraryName: string;
    onlisting: (title: string, listing: Listing) => void;
    onmemory: (memory: Memory) => void;
    onservers: () => void;
  }
  let { libraryPath, libraryName, onlisting, onmemory, onservers }: Props = $props();

  let rows = $state<Collections | null>(null);
  let people = $state<Person[]>([]);
  let albums = $state<Album[]>([]);
  let error = $state<string | null>(null);

  $effect(() => {
    const path = libraryPath;
    collections(path, todayDay())
      .then(async (found) => {
        rows = found;
        await tick();
        // The screen opened with only its header: move to the first memory,
        // unless the remote went somewhere already.
        const active = document.activeElement;
        if (!active || active === document.body || active.closest("header")) {
          (active as HTMLElement | null)?.blur();
          focusFirst();
        }
      })
      .catch((e) => (error = errorText(e)));
    listPeople(path)
      .then((found) => (people = found.filter((person) => person.name && !person.hidden).slice(0, 30)))
      .catch(() => (people = []));
    listAlbums(path)
      .then((found) => (albums = found.filter((album) => album.count > 0)))
      .catch(() => (albums = []));
  });

  /** Today's memories first, then the others. */
  const memories = $derived.by(() => {
    if (!rows) return [];
    const today = rows.today.map((index) => rows!.memories[index]);
    const others = rows.memories.filter((_, index) => !rows!.today.includes(index));
    return [...today, ...others].slice(0, 24);
  });

  const dayFormat = new Intl.DateTimeFormat(locale, { weekday: "long", day: "numeric", month: "long" });
  const dayLabel = (day: number) => dayFormat.format(new Date(day * 86_400_000));

  function memoryOf(id: string): Memory | undefined {
    return rows?.memories.find((memory) => memory.id === id);
  }
</script>

<div class="home">
  <header>
    <h1>{libraryName}</h1>
    <button data-focus class="small" onclick={onservers}>{t("Phototek servers")}</button>
  </header>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if memories.length > 0}
    <section>
      <h2>{t("Memories")}</h2>
      <div class="row">
        {#each memories as memory, index (memory.id)}
          <button data-focus data-first={index === 0 || undefined} class="card wide" onclick={() => onmemory(memory)}>
            <img src={thumbUrl(memory.cover, "preview")} alt="" />
            <span class="caption">
              <strong>{memoryTitle(memory)}</strong>
              <span>{memorySubtitle(memory, todayDay())}</span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if rows && rows.recentDays.length > 0}
    <section>
      <h2>{t("Recent Days")}</h2>
      <div class="row">
        {#each rows.recentDays as day (day.day)}
          <button
            data-focus
            class="card"
            onclick={() => onlisting(dayLabel(day.day), { kind: "days", from: day.day, to: day.day })}
          >
            <img src={thumbUrl(day.cover, "preview")} alt="" />
            <span class="caption">
              <strong>{dayLabel(day.day)}</strong>
              <span>{photoCount(day.count)}</span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if rows && rows.trips.length > 0}
    <section>
      <h2>{t("Trips")}</h2>
      <div class="row">
        {#each rows.trips as trip (trip.id)}
          {@const memory = memoryOf(trip.id)}
          <button
            data-focus
            class="card"
            onclick={() =>
              memory
                ? onlisting(trip.title, memory.listing)
                : onlisting(trip.title, { kind: "days", from: trip.fromDay, to: trip.toDay })}
          >
            <img src={thumbUrl(trip.cover, "preview")} alt="" />
            <span class="caption">
              <strong>{trip.title}</strong>
              <span>{formatDayRange(trip.fromDay, trip.toDay)}</span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if people.length > 0}
    <section>
      <h2>{t("People")}</h2>
      <div class="row">
        {#each people as person (person.clusters[0])}
          <button
            data-focus
            class="person"
            onclick={() => onlisting(person.name ?? "", { kind: "person", clusters: person.clusters })}
          >
            <span class="face">
              <EditedImage
                src={thumbUrl(person.cover, "preview")}
                edit={{ ...NO_EDIT, crop: person.coverCrop }}
                width={person.coverWidth}
                height={person.coverHeight}
                fit="cover"
              />
            </span>
            <strong>{person.name}</strong>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if albums.length > 0}
    <section>
      <h2>{t("Albums")}</h2>
      <div class="row">
        {#each albums as album (album.id)}
          <button data-focus class="card" onclick={() => onlisting(album.name, { kind: "album", album: album.id })}>
            {#if album.cover}<img src={thumbUrl(album.cover, "preview")} alt="" />{/if}
            <span class="caption">
              <strong>{album.name}</strong>
              <span>{photoCount(album.count)}</span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  <section>
    <h2>{t("Library")}</h2>
    <div class="row">
      <button data-focus class="tile" onclick={() => onlisting(t("All Photos"), { kind: "library" })}>
        {t("All Photos")}
      </button>
      {#if rows && rows.favorites > 0}
        <button data-focus class="tile" onclick={() => onlisting(t("Favorites"), { kind: "favorites" })}>
          ♥ {t("Favorites")}
        </button>
      {/if}
      {#if rows?.media.videos}
        <button
          data-focus
          class="tile"
          onclick={() => onlisting(t("Videos"), { kind: "only", within: { kind: "library" }, only: "videos" })}
        >
          ▶ {t("Videos")}
        </button>
      {/if}
    </div>
  </section>
</div>

<style>
  .home {
    height: 100%;
    overflow-y: auto;
    padding: 56px 0 80px;
    scroll-behavior: smooth;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 96px;
  }

  h1 {
    margin: 0;
    font-size: 48px;
  }

  h2 {
    margin: 36px 96px 18px;
    font-size: 30px;
    font-weight: 600;
  }

  .row {
    display: flex;
    gap: 28px;
    padding: 16px 96px 24px;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .row::-webkit-scrollbar {
    display: none;
  }

  button {
    flex: none;
    padding: 0;
    border: none;
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
  }

  .small {
    padding: 12px 24px;
    border-radius: 12px;
    background: var(--surface);
    font-size: 22px;
  }

  .card {
    position: relative;
    width: 360px;
    height: 240px;
    border-radius: 18px;
    background: var(--surface);
    overflow: hidden;
  }

  .card.wide {
    width: 520px;
    height: 300px;
  }

  .card img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .caption {
    position: absolute;
    inset: auto 0 0;
    display: grid;
    gap: 2px;
    padding: 48px 20px 16px;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.75));
    font-size: 22px;
  }

  .caption strong {
    font-size: 26px;
  }

  .caption span {
    color: rgb(255 255 255 / 0.8);
  }

  .person {
    display: grid;
    justify-items: center;
    gap: 12px;
    width: 200px;
    border-radius: 24px;
    padding-bottom: 8px;
  }

  .face {
    position: relative;
    display: block;
    width: 180px;
    height: 180px;
    border-radius: 50%;
    background: var(--surface);
    overflow: hidden;
  }

  .tile {
    display: grid;
    place-items: center;
    width: 360px;
    height: 160px;
    border-radius: 18px;
    background: var(--surface);
    font-size: 30px;
    font-weight: 600;
    text-align: center;
  }

  .error {
    margin: 24px 96px;
    color: var(--danger);
  }
</style>
