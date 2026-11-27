<!--
  The people found in the library's photos. Named people get a large card
  with their name on it, as in Apple Photos; the others a smaller round
  picture below, with "Add name". Clicking a person lists their photos;
  clicking a name (or "Rename…" in the menu) names them. Giving two people
  the same name merges them. The heart on a card makes them a favourite,
  listed first (here and in Collections); the menu also pins them to the
  sidebar.

  Finding faces needs two model files that are downloaded once; until then
  this screen explains that and offers the download.
-->
<script lang="ts">
  import { errorText, t } from "./i18n";
  import { onMount } from "svelte";
  import {
    enableFaces,
    facesStatus,
    listPeople,
    namePerson,
    onFacesDownload,
    setPersonHidden,
    thumbUrl,
    type FacesStatus,
    type Person,
  } from "./api";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatBytes, formatNumber } from "./format";
  import { showMenu } from "./contextMenu";

  interface Props {
    libraryPath: string;
    /** Changes whenever the index or the organization changed. */
    revision: number;
    /** False while the library is offline: people can't be named. */
    canEdit: boolean;
    /** True while faces are being searched. */
    searching: boolean;
    onopen: (person: Person) => void;
    /** The names of the favourite people, listed first. */
    favorites: string[];
    ontogglefavorite: (name: string) => void;
    /** Whether a named person is pinned to the sidebar. */
    pinned: (name: string) => boolean;
    ontogglepin: (person: Person) => void;
  }
  let { libraryPath, revision, canEdit, searching, onopen, favorites, ontogglefavorite, pinned, ontogglepin }: Props =
    $props();

  let status = $state<FacesStatus | null>(null);
  let people = $state<Person[] | null>(null);
  let error = $state<string | null>(null);
  /** Bytes received while the models download; `null` when not downloading. */
  let received = $state<number | null>(null);
  /** The person being named (by their first group), and the text typed. */
  let naming = $state<number | null>(null);
  let nameText = $state("");
  /** True to list the people hidden on this computer too. */
  let showHidden = $state(false);
  const hiddenCount = $derived(people?.filter((person) => person.hidden).length ?? 0);
  const listed = $derived(people?.filter((person) => showHidden || !person.hidden) ?? []);
  const isFavorite = (person: Person) => person.name !== null && favorites.includes(person.name);
  // Favourites first; otherwise in the order the backend gives (most photos first).
  const named = $derived(
    listed
      .filter((person) => person.name)
      .sort((a, b) => Number(isFavorite(b)) - Number(isFavorite(a))),
  );
  const unnamed = $derived(listed.filter((person) => !person.name));

  /**
   * A portrait crop (3:4) around a person's face for their card: wider than
   * the face itself, so it shows the head and shoulders. Crops are in
   * ten-thousandths of the picture's width and height.
   */
  function cardCrop(person: Person): [number, number, number, number] {
    const [x, y, w, h] = person.coverCrop;
    const width = person.coverWidth;
    const height = person.coverHeight;
    if (!width || !height) return person.coverCrop;
    const centerX = ((x + w / 2) / 10_000) * width;
    const centerY = ((y + h / 2) / 10_000) * height;
    const face = Math.max((w / 10_000) * width, (h / 10_000) * height);
    let cropHeight = Math.min(height, face * 2.2);
    let cropWidth = (cropHeight * 3) / 4;
    if (cropWidth > width) {
      cropWidth = width;
      cropHeight = Math.min(height, (cropWidth * 4) / 3);
    }
    // The face a little above the middle, as in a portrait.
    const left = Math.min(Math.max(0, centerX - cropWidth / 2), width - cropWidth);
    const top = Math.min(Math.max(0, centerY - cropHeight * 0.45), height - cropHeight);
    return [
      Math.round((left / width) * 10_000),
      Math.round((top / height) * 10_000),
      Math.round((cropWidth / width) * 10_000),
      Math.round((cropHeight / height) * 10_000),
    ];
  }

  const personMenu = (event: MouseEvent, person: Person) =>
    showMenu(event, [
      { label: t("Open"), action: () => onopen(person) },
      "-",
      { label: person.name ? t("Rename…") : t("Name…"), enabled: canEdit, action: () => startNaming(person) },
      ...(person.name
        ? [
            {
              label: isFavorite(person) ? t("Remove from Favorites") : t("Add to Favorites"),
              action: () => ontogglefavorite(person.name!),
            },
            {
              label: pinned(person.name) ? t("Unpin") : t("Pin to Sidebar"),
              action: () => ontogglepin(person),
            },
          ]
        : []),
      {
        label: person.hidden ? t("Show in People") : t("Hide from People"),
        action: () => setHidden(person, !person.hidden),
      },
    ]);

  async function setHidden(person: Person, hidden: boolean) {
    try {
      await setPersonHidden(libraryPath, person.clusters, hidden);
      people = await listPeople(libraryPath);
    } catch (e) {
      error = errorText(e);
    }
  }

  onMount(() => {
    facesStatus()
      .then((loaded) => (status = loaded))
      .catch((e) => (error = errorText(e)));
    const unlisten = onFacesDownload((bytes) => {
      if (received !== null) received = bytes;
    });
    return () => unlisten.then((stop) => stop());
  });

  $effect(() => {
    void revision;
    if (!status?.enabled) return;
    listPeople(libraryPath)
      .then((loaded) => (people = loaded))
      .catch((e) => (error = errorText(e)));
  });

  async function download() {
    error = null;
    received = 0;
    try {
      await enableFaces();
      status = await facesStatus();
    } catch (e) {
      error = errorText(e);
    } finally {
      received = null;
    }
  }

  function startNaming(person: Person) {
    if (!canEdit) return;
    naming = person.clusters[0];
    nameText = person.name ?? "";
  }

  async function saveName(person: Person) {
    const name = nameText.trim();
    naming = null;
    if (name === (person.name ?? "")) return;
    try {
      await namePerson(libraryPath, person.clusters, name);
      people = await listPeople(libraryPath);
    } catch (e) {
      error = errorText(e);
    }
  }

  function focus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<div class="people">
  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  {#if status && !status.enabled}
    <div class="intro">
      <h2>{t("People")}</h2>
      {#if status.downloaded}
        <p>{t("Finding faces is switched off in Settings. The people found so far are kept.")}</p>
      {:else}
      <p>
        {t("Phototek can find the faces in your photos and group the photos by person. It all happens on this computer: no photo and no face leaves it.")}
      </p>
      <p>
        {t("This needs two model files ({size}), downloaded once from huggingface.co. Searching a large library then takes a while, in the background.", { size: formatBytes(status.downloadBytes) })}
      </p>
      {/if}
      {#if received !== null}
        <progress value={received} max={status.downloadBytes}></progress>
      {:else}
        <button class="primary" onclick={download}>
          {status.downloaded ? t("Switch on and find people") : t("Download and find people")}
        </button>
      {/if}
    </div>
  {:else if people}
    {#if people.length === 0}
      <p class="empty">
        {searching
          ? t("Looking for faces… People appear here as they are found.")
          : t("No people found yet. A person shows up once they are in three photos.")}
      </p>
    {:else}
      {#snippet hideButton(person: Person)}
        <button
          class="hide"
          onclick={() => setHidden(person, !person.hidden)}
          title={person.hidden ? t("Show this person again") : t("Hide this person (e.g. a stranger)")}
          aria-label={person.hidden ? t("Show this person again") : t("Hide this person")}
        >
          {person.hidden ? t("Show") : t("Hide")}
        </button>
      {/snippet}
      {#snippet heartButton(person: Person)}
        {@const on = isFavorite(person)}
        <button
          class="heart"
          class:on
          onclick={() => ontogglefavorite(person.name!)}
          aria-pressed={on}
          aria-label={on ? t("Remove from Favorites") : t("Add to Favorites")}
          title={on ? t("Remove from Favorites") : t("Add to Favorites")}
        >
          <svg viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
            <path d="M10 16.5S3 12.3 3 7.7a3.6 3.6 0 0 1 7-1.6 3.6 3.6 0 0 1 7 1.6c0 4.6-7 8.8-7 8.8z" />
          </svg>
        </button>
      {/snippet}
      {#snippet nameField(person: Person)}
        <form
          onsubmit={(event) => {
            event.preventDefault();
            saveName(person);
          }}
        >
          <input
            bind:value={nameText}
            use:focus
            maxlength="200"
            placeholder={t("Name")}
            onblur={() => saveName(person)}
            onkeydown={(event) => event.key === "Escape" && (naming = null)}
          />
        </form>
      {/snippet}

      {#if named.length > 0}
        <h2>{t("People")}</h2>
        <ul class="cards">
          {#each named as person (person.clusters[0])}
            <li class:hidden-person={person.hidden}>
              {@render hideButton(person)}
              {@render heartButton(person)}
              <button
                class="card"
                onclick={() => onopen(person)}
                oncontextmenu={(event) => personMenu(event, person)}
                aria-label="{person.name}, {formatNumber(person.photos)}"
              >
                <EditedImage
                  src={thumbUrl(person.cover, "preview")}
                  edit={{ ...NO_EDIT, crop: cardCrop(person) }}
                  width={person.coverWidth}
                  height={person.coverHeight}
                  fit="cover"
                />
                {#if naming !== person.clusters[0]}
                  <span class="card-label">
                    <span class="card-name">{person.name}</span>
                    <span class="card-count">{formatNumber(person.photos)}</span>
                  </span>
                {/if}
              </button>
              {#if naming === person.clusters[0]}
                <div class="card-naming">{@render nameField(person)}</div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      {#if unnamed.length > 0}
        {#if named.length > 0}<h2>{t("Not named yet")}</h2>{/if}
        <ul class="faces">
          {#each unnamed as person (person.clusters[0])}
            <li class:hidden-person={person.hidden}>
              {@render hideButton(person)}
              <button
                class="face"
                onclick={() => onopen(person)}
                oncontextmenu={(event) => personMenu(event, person)}
                aria-label={t("Unnamed person")}
              >
                <EditedImage
                  src={thumbUrl(person.cover, "preview")}
                  edit={{ ...NO_EDIT, crop: person.coverCrop }}
                  width={person.coverWidth}
                  height={person.coverHeight}
                  fit="cover"
                />
              </button>
              {#if naming === person.clusters[0]}
                {@render nameField(person)}
              {:else}
                <button
                  class="name unnamed"
                  disabled={!canEdit}
                  onclick={() => startNaming(person)}
                  title={t("Click to name. The same name for two people merges them.")}
                >
                  {t("Add name")}
                </button>
              {/if}
              <span class="count">{formatNumber(person.photos)}</span>
            </li>
          {/each}
        </ul>
      {/if}
      {#if hiddenCount > 0}
        <p class="hidden-toggle">
          <button class="link" onclick={() => (showHidden = !showHidden)}>
            {showHidden ? t("Leave out hidden people") : t("Show hidden people ({n})", { n: hiddenCount })}
          </button>
        </p>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .people {
    height: 100%;
    overflow-y: auto;
    padding: 20px;
  }

  .intro {
    max-width: 460px;
    margin: 60px auto 0;
    line-height: 1.5;
  }

  .intro h2 {
    margin: 0 0 8px;
    font-size: 22px;
  }

  .intro progress {
    width: 100%;
  }

  .primary {
    padding: 8px 16px;
    border: none;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
  }

  .empty,
  .error {
    max-width: 420px;
    margin: 80px auto 0;
    text-align: center;
    color: var(--text-dim);
  }

  .error {
    margin-top: 0;
    color: var(--danger);
  }

  h2 {
    margin: 4px 0 14px;
    font-size: 20px;
    font-weight: 700;
  }

  ul {
    display: grid;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  ul + h2 {
    margin-top: 32px;
  }

  .cards {
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 14px;
  }

  .faces {
    grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
    gap: 20px 12px;
  }

  /* A named person: a portrait card with the name written on it. */
  .card {
    position: relative;
    width: 100%;
    aspect-ratio: 3 / 4;
    padding: 0;
    border: none;
    border-radius: 14px;
    background: var(--surface-hover);
    overflow: hidden;
    cursor: pointer;
  }

  .card:hover {
    outline: 3px solid var(--accent);
    outline-offset: 2px;
  }

  .card-label {
    position: absolute;
    inset: auto 0 0;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    padding: 28px 12px 10px;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.6));
    color: #fff;
    text-align: left;
  }

  .card-name {
    min-width: 0;
    overflow: hidden;
    font-size: 15px;
    font-weight: 700;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-count {
    flex: none;
    font-size: 12px;
    opacity: 0.85;
  }

  .card-naming {
    position: absolute;
    inset: auto 8px 10px;
  }

  .card-naming input {
    width: 100%;
  }

  li {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .hidden-person .face,
  .hidden-person .card {
    opacity: 0.45;
  }

  .hide {
    position: absolute;
    top: 6px;
    right: 6px;
    z-index: 1;
    padding: 2px 7px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-dim);
    font-size: 11px;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  li:hover .hide,
  .hide:focus-visible {
    opacity: 1;
  }

  /* The heart, at the top left of a card: always shown once on. */
  .heart {
    position: absolute;
    top: 6px;
    left: 6px;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.35);
    color: #fff;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .heart svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linejoin: round;
  }

  .heart.on svg {
    fill: currentColor;
  }

  li:hover .heart,
  .heart:focus-visible,
  .heart.on {
    opacity: 1;
  }

  .hide:hover {
    color: var(--text);
  }

  .hidden-toggle {
    margin: 28px 0 0;
    text-align: center;
  }

  .link {
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .face {
    position: relative;
    width: 96px;
    height: 96px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--surface-hover);
    overflow: hidden;
    cursor: pointer;
  }

  .face:hover {
    outline: 3px solid var(--accent);
  }

  .name {
    max-width: 100%;
    padding: 2px 6px;
    border: none;
    border-radius: 4px;
    background: none;
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }

  .name:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .name.unnamed {
    font-weight: 400;
    color: var(--text-dim);
  }

  input {
    width: 104px;
    padding: 2px 6px;
    border: 1px solid var(--accent);
    border-radius: 4px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 14px;
    text-align: center;
  }

  .count {
    font-size: 12px;
    color: var(--text-dim);
  }
</style>
