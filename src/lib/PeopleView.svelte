<!--
  The people found in the library's photos: one round picture per person.
  Clicking a person lists their photos; clicking the name under it names
  them. Giving two people the same name merges them.

  Finding faces needs two model files that are downloaded once; until then
  this screen explains that and offers the download.
-->
<script lang="ts">
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

  interface Props {
    libraryPath: string;
    /** Changes whenever the index or the organization changed. */
    revision: number;
    /** False while the library is offline: people can't be named. */
    canEdit: boolean;
    /** True while faces are being searched. */
    searching: boolean;
    onopen: (person: Person) => void;
  }
  let { libraryPath, revision, canEdit, searching, onopen }: Props = $props();

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

  async function setHidden(person: Person, hidden: boolean) {
    try {
      await setPersonHidden(libraryPath, person.clusters, hidden);
      people = await listPeople(libraryPath);
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    facesStatus()
      .then((loaded) => (status = loaded))
      .catch((e) => (error = String(e)));
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
      .catch((e) => (error = String(e)));
  });

  async function download() {
    error = null;
    received = 0;
    try {
      await enableFaces();
      status = await facesStatus();
    } catch (e) {
      error = String(e);
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
      error = String(e);
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
      <h2>People</h2>
      {#if status.downloaded}
        <p>Finding faces is switched off in Settings. The people found so far are kept.</p>
      {:else}
      <p>
        Photolib can find the faces in your photos and group the photos by person. It all
        happens on this computer: no photo and no face leaves it.
      </p>
      <p>
        This needs two model files ({formatBytes(status.downloadBytes)}), downloaded once from
        huggingface.co. Searching a large library then takes a while, in the background.
      </p>
      {/if}
      {#if received !== null}
        <progress value={received} max={status.downloadBytes}></progress>
      {:else}
        <button class="primary" onclick={download}>
          {status.downloaded ? "Switch on and find people" : "Download and find people"}
        </button>
      {/if}
    </div>
  {:else if people}
    {#if people.length === 0}
      <p class="empty">
        {searching
          ? "Looking for faces… People appear here as they are found."
          : "No people found yet. A person shows up once they are in three photos."}
      </p>
    {:else}
      <ul>
        {#each listed as person (person.clusters[0])}
          <li class:hidden-person={person.hidden}>
            <button
              class="hide"
              onclick={() => setHidden(person, !person.hidden)}
              title={person.hidden ? "Show this person again" : "Hide this person (e.g. a stranger)"}
              aria-label={person.hidden ? "Show this person again" : "Hide this person"}
            >
              {person.hidden ? "Show" : "Hide"}
            </button>
            <button class="face" onclick={() => onopen(person)} aria-label={person.name ?? "Unnamed person"}>
              <EditedImage
                src={thumbUrl(person.cover, "preview")}
                edit={{ ...NO_EDIT, crop: person.coverCrop }}
                width={person.coverWidth}
                height={person.coverHeight}
                fit="cover"
              />
            </button>
            {#if naming === person.clusters[0]}
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
                  placeholder="Name"
                  onblur={() => saveName(person)}
                  onkeydown={(event) => event.key === "Escape" && (naming = null)}
                />
              </form>
            {:else}
              <button
                class="name"
                class:unnamed={!person.name}
                disabled={!canEdit}
                onclick={() => startNaming(person)}
                title="Click to name. The same name for two people merges them."
              >
                {person.name ?? "Add name"}
              </button>
            {/if}
            <span class="count">{formatNumber(person.photos)}</span>
          </li>
        {/each}
      </ul>
      {#if hiddenCount > 0}
        <p class="hidden-toggle">
          <button class="link" onclick={() => (showHidden = !showHidden)}>
            {showHidden ? "Leave out hidden people" : `Show hidden people (${formatNumber(hiddenCount)})`}
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

  ul {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 24px 16px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .hidden-person .face {
    opacity: 0.45;
  }

  .hide {
    position: absolute;
    top: 0;
    right: calc(50% - 64px);
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
    width: 120px;
    height: 120px;
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
    width: 120px;
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
