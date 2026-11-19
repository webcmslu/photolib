<!--
  "Is this also Anna?": on a named person's page, the groups of faces that
  look like them but were not merged automatically (often the same person
  younger, or from another angle). ✓ adds a group to the person; ✕ says it
  is someone else, and it is not suggested again.
-->
<script lang="ts">
  import { namePerson, personSuggestions, rejectSuggestion, thumbUrl, type Person } from "./api";
  import { NO_EDIT } from "./edit";
  import EditedImage from "./EditedImage.svelte";
  import { formatNumber } from "./format";

  interface Props {
    libraryPath: string;
    name: string;
    clusters: number[];
    canEdit: boolean;
    /** Told the groups the person is made of after one was added. */
    onadded: (clusters: number[]) => void;
  }
  let { libraryPath, name, clusters, canEdit, onadded }: Props = $props();

  let suggestions = $state<Person[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  $effect(() => {
    const [path, who, groups] = [libraryPath, name, clusters];
    personSuggestions(path, groups, who)
      .then((found) => (suggestions = found))
      .catch((e) => (error = String(e)));
  });

  async function confirm(suggestion: Person) {
    busy = true;
    error = null;
    try {
      await namePerson(libraryPath, suggestion.clusters, name);
      suggestions = suggestions.filter((s) => s !== suggestion);
      onadded([...clusters, ...suggestion.clusters]);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function reject(suggestion: Person) {
    error = null;
    try {
      await rejectSuggestion(libraryPath, suggestion.clusters[0], name);
      suggestions = suggestions.filter((s) => s !== suggestion);
    } catch (e) {
      error = String(e);
    }
  }
</script>

{#if suggestions.length > 0}
  <section aria-label="Suggestions">
    <h3>Is this also {name}?</h3>
    <ul>
      {#each suggestions as suggestion (suggestion.clusters[0])}
        <li>
          <span class="face">
            <EditedImage
              src={thumbUrl(suggestion.cover, "preview")}
              edit={{ ...NO_EDIT, crop: suggestion.coverCrop }}
              width={suggestion.coverWidth}
              height={suggestion.coverHeight}
              fit="cover"
            />
          </span>
          <span class="count">{formatNumber(suggestion.photos)}</span>
          <span class="actions">
            <button
              class="yes"
              disabled={!canEdit || busy}
              onclick={() => confirm(suggestion)}
              title="Yes, this is {name}"
              aria-label="Yes, this is {name}"
            >
              ✓
            </button>
            <button
              class="no"
              onclick={() => reject(suggestion)}
              title="No, this is someone else"
              aria-label="No, this is someone else"
            >
              ✕
            </button>
          </span>
        </li>
      {/each}
    </ul>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </section>
{/if}

<style>
  section {
    padding: 10px 16px 12px;
    border-bottom: 1px solid var(--border);
  }

  h3 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
  }

  ul {
    display: flex;
    gap: 14px;
    margin: 0;
    padding: 0 0 2px;
    overflow-x: auto;
    list-style: none;
  }

  li {
    display: flex;
    flex: none;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }

  .face {
    display: block;
    width: 64px;
    height: 64px;
    border-radius: 50%;
    background: var(--surface-hover);
    overflow: hidden;
  }

  .count {
    font-size: 11px;
    color: var(--text-dim);
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .actions button {
    width: 28px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }

  .yes:hover:not(:disabled) {
    border-color: #16a34a;
    background: #16a34a;
    color: #fff;
  }

  .no:hover {
    border-color: var(--danger);
    background: var(--danger);
    color: #fff;
  }

  .actions button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .error {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--danger);
  }
</style>
