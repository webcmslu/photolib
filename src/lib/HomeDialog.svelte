<!--
  "Where is home?": trips are the photos taken far from home, and Phototek
  works home out from where most days were photographed. Here it can be set
  by hand instead, with the year a move happened: every computer and the
  server then use it (it goes into the library's shared log).
-->
<script lang="ts">
  import { applyOps, findTowns, listHomes, type Home, type Town } from "./api";
  import { errorText, t } from "./i18n";

  interface Props {
    libraryPath: string;
    /** The town home is in now, as worked out or set. */
    current: string | null;
    canEdit: boolean;
    onchanged: () => void;
    onclose: () => void;
  }
  let { libraryPath, current, canEdit, onchanged, onclose }: Props = $props();

  let homes = $state<Home[]>([]);
  let typed = $state("");
  let towns = $state<Town[]>([]);
  let chosen = $state<Town | null>(null);
  let since = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function reload() {
    try {
      homes = await listHomes(libraryPath);
    } catch (e) {
      error = errorText(e);
    }
  }
  reload();

  let searching: ReturnType<typeof setTimeout> | null = null;
  function search() {
    chosen = null;
    if (searching) clearTimeout(searching);
    searching = setTimeout(async () => {
      towns = typed.trim().length >= 2 ? await findTowns(typed).catch(() => []) : [];
    }, 200);
  }

  const toUnits = (degrees: number) => Math.round(degrees * 100_000);

  async function save(ops: Parameters<typeof applyOps>[1]) {
    busy = true;
    error = null;
    try {
      await applyOps(libraryPath, ops);
      await reload();
      onchanged();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  function add() {
    if (!chosen) return;
    const year = Number(since);
    const from = since.trim() && year >= 1800 && year <= 2200 ? year : 0;
    save([
      {
        op: "home",
        since: from,
        latitude: toUnits(chosen.latitude),
        longitude: toUnits(chosen.longitude),
        town: chosen.name.split(",")[0],
      },
    ]);
    typed = "";
    towns = [];
    chosen = null;
    since = "";
  }

  function remove(home: Home) {
    save([{ op: "home", since: home.since, latitude: 0, longitude: 0, town: "" }]);
  }
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && onclose()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(event) => event.target === event.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label={t("Where is home?")}>
    <header>
      <h2>{t("Where is home?")}</h2>
      <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
    </header>
    <p class="note">
      {t("Trips are the photos taken far from home. Phototek works home out from where you take photos most;")}
      {current ? t("now: {town}.", { town: current }) : t("it can't tell yet.")}
      {t("Set it here if it's wrong, with the year of each move.")}
    </p>

    {#if homes.length > 0}
      <ul class="homes">
        {#each homes as home (home.since)}
          <li>
            <span>{home.since ? t("Since {year}", { year: String(home.since) }) : t("Always")}</span>
            <strong>{home.town}</strong>
            <button class="link" onclick={() => remove(home)} disabled={!canEdit || busy}>{t("Remove")}</button>
          </li>
        {/each}
      </ul>
    {/if}

    <form
      onsubmit={(event) => {
        event.preventDefault();
        add();
      }}
    >
      <label class="town">
        <span>{t("Town")}</span>
        <input bind:value={typed} oninput={search} placeholder="Luxembourg" autocomplete="off" />
        {#if towns.length > 0 && !chosen}
          <ul class="suggestions">
            {#each towns as town (town.name + town.latitude)}
              <li>
                <button
                  type="button"
                  onclick={() => {
                    chosen = town;
                    typed = town.name;
                  }}>{town.name}</button
                >
              </li>
            {/each}
          </ul>
        {/if}
      </label>
      <label class="year">
        <span>{t("Since (year, optional)")}</span>
        <input bind:value={since} inputmode="numeric" placeholder={homes.length > 0 ? "2015" : ""} />
      </label>
      <button class="primary" type="submit" disabled={!chosen || !canEdit || busy}>{t("Add")}</button>
    </form>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.45);
  }

  .dialog {
    width: min(520px, calc(100vw - 32px));
    padding: 18px 22px 22px;
    border-radius: 14px;
    background: var(--surface);
    box-shadow: 0 12px 48px rgb(0 0 0 / 0.35);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    margin: 0;
    font-size: 18px;
  }

  .close {
    border: none;
    background: none;
    color: var(--text);
    font-size: 16px;
    cursor: pointer;
  }

  .note {
    margin: 10px 0 14px;
    color: var(--text-dim);
    font-size: 13px;
  }

  .homes {
    margin: 0 0 14px;
    padding: 0;
    list-style: none;
  }

  .homes li {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }

  .homes span {
    width: 90px;
    color: var(--text-dim);
    font-size: 13px;
  }

  .homes strong {
    flex: 1;
  }

  form {
    display: flex;
    align-items: flex-end;
    gap: 10px;
  }

  label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .town {
    position: relative;
    flex: 1;
  }

  .year {
    width: 130px;
  }

  label input {
    width: 100%;
    min-width: 0;
  }

  input {
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 14px;
  }

  .suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 1;
    margin: 2px 0 0;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.2);
    list-style: none;
  }

  .suggestions button {
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 14px;
    text-align: left;
    cursor: pointer;
  }

  .suggestions button:hover {
    background: var(--surface-hover);
  }

  .primary {
    padding: 7px 16px;
    border: none;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-text);
    font: inherit;
    cursor: pointer;
  }

  .primary:disabled {
    opacity: 0.5;
  }

  .link {
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .error {
    margin: 10px 0 0;
    color: var(--danger);
  }
</style>
