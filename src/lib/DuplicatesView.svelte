<!--
  Photos stored more than once: one row per photo with all its copies, and
  the copy to keep chosen (the app suggests one; a click changes it). One
  button moves every other copy into the "Duplicates to delete" folder at
  the top of the library, in the same folders as before, after checking
  byte for byte that it is the same file. Nothing is deleted: the user
  checks that folder and deletes it in their file manager, or puts the
  copies back.
-->
<script lang="ts">
  import { sep } from "@tauri-apps/api/path";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { SvelteMap } from "svelte/reactivity";
  import {
    cancelJob,
    duplicateGroups,
    onDuplicatesProgress,
    PARKED_FOLDER,
    parkDuplicates,
    parkedDuplicates,
    putBackDuplicates,
    thumbUrl,
    type DuplicateGroup,
    type MoveSummary,
  } from "./api";
  import { formatBytes, formatDateTime, formatNumber, photoCount } from "./format";
  import { REVEAL_LABEL, revealPhoto } from "./reveal";

  interface Props {
    libraryPath: string;
    /** Changes whenever the index changed. */
    revision: number;
    /** False while the library is offline or read-only. */
    canEdit: boolean;
    /** Told after copies were moved or put back. */
    onchanged: () => void;
  }
  let { libraryPath, revision, canEdit, onchanged }: Props = $props();

  /** Rows shown at first, and added by "Show more": thousands at once are slow. */
  const PAGE = 200;

  let groups = $state<DuplicateGroup[] | null>(null);
  let shown = $state(PAGE);
  /** The user's choice of copy to keep, where it differs from the suggestion. */
  const kept = new SvelteMap<string, number>();
  let parked = $state(0);
  let error = $state<string | null>(null);

  let confirming = $state(false);
  let progress = $state<[number, number] | null>(null);
  let result = $state<{ verb: string; summary: MoveSummary } | null>(null);

  const keepOf = (group: DuplicateGroup) => kept.get(group.fingerprint) ?? group.keep;
  const extraCount = $derived(groups?.reduce((sum, group) => sum + group.copies.length - 1, 0) ?? 0);

  async function load() {
    try {
      const [found, waiting] = await Promise.all([duplicateGroups(libraryPath), parkedDuplicates(libraryPath)]);
      groups = found;
      parked = waiting;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    void revision;
    if (progress === null) load();
  });

  onMount(() => {
    const unlisten = onDuplicatesProgress((done, total) => {
      if (progress) progress = [done, total];
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  async function moveExtras() {
    if (!groups) return;
    confirming = false;
    error = null;
    result = null;
    const choices = groups.map((group) => {
      const keep = keepOf(group);
      return {
        keep: group.copies[keep].relPath,
        extras: group.copies.filter((_, index) => index !== keep).map((copy) => copy.relPath),
      };
    });
    progress = [0, extraCount];
    try {
      result = { verb: "moved", summary: await parkDuplicates(libraryPath, choices) };
      kept.clear();
    } catch (e) {
      error = String(e);
    } finally {
      progress = null;
    }
    await load();
    onchanged();
  }

  async function putBack() {
    error = null;
    result = null;
    try {
      result = { verb: "put back", summary: await putBackDuplicates(libraryPath) };
    } catch (e) {
      error = String(e);
    }
    await load();
    onchanged();
  }

  function showParkedFolder() {
    const separator = sep();
    const root = libraryPath.endsWith(separator) ? libraryPath : libraryPath + separator;
    revealItemInDir(root + PARKED_FOLDER).catch((e) => (error = String(e)));
  }

  function folderOf(relPath: string): string {
    const slash = relPath.lastIndexOf("/");
    return slash < 0 ? "Top level" : relPath.slice(0, slash);
  }

  function nameOf(relPath: string): string {
    return relPath.slice(relPath.lastIndexOf("/") + 1);
  }
</script>

<div class="duplicates">
  <header>
    {#if groups === null}
      <p class="dim">Looking for duplicates…</p>
    {:else}
      <div class="summary">
        <h2>Duplicates</h2>
        <p class="dim">
          {#if groups.length === 0}
            Every photo is stored once.
          {:else}
            {photoCount(groups.length)} stored more than once: {formatNumber(extraCount)} extra
            {extraCount === 1 ? "copy" : "copies"}.
          {/if}
        </p>
      </div>
      {#if progress}
        <div class="progress">
          <span>Checking and moving {formatNumber(progress[0])} of {formatNumber(progress[1])}…</span>
          <progress value={progress[0]} max={progress[1]}></progress>
          <button class="secondary" onclick={cancelJob}>Stop</button>
        </div>
      {:else if extraCount > 0}
        <button
          class="primary"
          disabled={!canEdit}
          title={canEdit ? "" : "The library is offline or read-only"}
          onclick={() => (confirming = true)}
        >
          Move {formatNumber(extraCount)} extra {extraCount === 1 ? "copy" : "copies"}…
        </button>
      {/if}
    {/if}
  </header>

  {#if confirming}
    <div class="notice">
      <p>
        For each photo, the copy marked <strong>Keep</strong> stays where it is. The other copies go into the folder
        <strong>{PARKED_FOLDER}</strong> at the top of the library, in the same folders as now.
      </p>
      <p>
        Each copy is first compared byte for byte with the one kept, and left alone if it differs. Nothing is
        deleted: look through that folder, then delete it in your file manager. “Put back” returns the copies.
      </p>
      <div class="actions">
        <button class="secondary" onclick={() => (confirming = false)}>Cancel</button>
        <button class="primary" onclick={moveExtras}>Move {formatNumber(extraCount)}</button>
      </div>
    </div>
  {/if}

  {#if result}
    <div class="notice">
      <p>
        {#if result.summary.cancelled}Stopped. {/if}{formatNumber(result.summary.moved)}
        {result.summary.moved === 1 ? "copy" : "copies"}
        {result.verb}.
      </p>
      {#if result.summary.skipped.length > 0}
        <p class="error">{formatNumber(result.summary.skipped.length)} left where {result.summary.skipped.length === 1 ? "it was" : "they were"}:</p>
        <ul class="skipped">
          {#each result.summary.skipped.slice(0, 50) as [path, why] (path)}
            <li><span class="path">{path}</span>: {why}</li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  {#if parked > 0 && !progress}
    <div class="notice parked">
      <p>
        {formatNumber(parked)} {parked === 1 ? "copy waits" : "copies wait"} in <strong>{PARKED_FOLDER}</strong>.
        Delete that folder once you have checked it.
      </p>
      <div class="actions">
        <button class="secondary" onclick={showParkedFolder}>Show the folder</button>
        <button class="secondary" disabled={!canEdit} onclick={putBack}>Put them back</button>
      </div>
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  {#if groups && groups.length > 0}
    <ul class="groups">
      {#each groups.slice(0, shown) as group (group.fingerprint)}
        <li class="group">
          <div class="thumb">
            <img src={thumbUrl(group.fingerprint)} alt="" loading="lazy" draggable="false" />
            {#if group.video}<span class="video">▶</span>{/if}
          </div>
          <div class="copies">
            <div class="when dim">
              {formatDateTime(group.sortDate)} · {formatBytes(group.copies[0].size)}
            </div>
            {#each group.copies as copy, index (copy.relPath)}
              {@const keep = keepOf(group) === index}
              <div class="copy" class:keep>
                <button
                  class="choice"
                  class:keep
                  disabled={progress !== null}
                  onclick={() => kept.set(group.fingerprint, index)}
                  title={keep ? "This copy stays where it is" : "Keep this copy instead"}
                >
                  {keep ? "Keep" : "Move"}
                </button>
                <span class="file" title={copy.relPath}>
                  <span class="name">{nameOf(copy.relPath)}</span>
                  <span class="folder dim">{folderOf(copy.relPath)}</span>
                </span>
                <span class="modified dim" title="File date">{formatDateTime(copy.mtimeMs)}</span>
                <button class="reveal" onclick={() => revealPhoto(libraryPath, copy.relPath).catch(() => {})}>
                  {REVEAL_LABEL}
                </button>
              </div>
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

<style>
  .duplicates {
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

  .progress {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }

  .progress progress {
    width: 180px;
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
    padding: 6px 12px;
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

  .notice p {
    margin: 0 0 8px;
  }

  .notice p:last-child {
    margin-bottom: 0;
  }

  .parked {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .parked p {
    margin: 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .error {
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .skipped {
    max-height: 160px;
    overflow-y: auto;
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
    color: var(--text-dim);
    user-select: text;
    -webkit-user-select: text;
  }

  .path {
    color: var(--text);
  }

  .groups {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .group {
    display: flex;
    gap: 14px;
    padding: 12px 0;
    border-top: 1px solid var(--border);
  }

  .thumb {
    position: relative;
    flex: none;
    width: 96px;
    height: 96px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--bg);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .video {
    position: absolute;
    right: 5px;
    bottom: 4px;
    color: #fff;
    font-size: 12px;
    text-shadow: 0 1px 2px rgb(0 0 0 / 0.6);
  }

  .copies {
    flex: 1;
    min-width: 0;
    display: grid;
    align-content: start;
    gap: 4px;
  }

  .when {
    font-size: 12px;
    margin-bottom: 2px;
  }

  .copy {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    font-size: 13px;
  }

  .choice {
    flex: none;
    width: 54px;
    padding: 2px 0;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-dim);
    font-size: 12px;
  }

  .choice.keep {
    border-color: #16a34a;
    background: #16a34a;
    color: #fff;
    font-weight: 600;
  }

  .file {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 8px;
    overflow: hidden;
    white-space: nowrap;
  }

  .name {
    flex: none;
    max-width: 60%;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .copy:not(.keep) .name {
    color: var(--text-dim);
  }

  .copy.keep .name {
    font-weight: 600;
  }

  .folder {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
  }

  .modified {
    flex: none;
    font-size: 12px;
  }

  .reveal {
    flex: none;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: 12px;
  }

  .reveal:hover {
    text-decoration: underline;
  }

  .more {
    display: block;
    margin: 12px auto 0;
  }
</style>
