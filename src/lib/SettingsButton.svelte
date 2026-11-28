<!--
  The ⚙ button and the Settings window: how slideshows run, and whether
  faces are searched to group photos by person.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import {
    contentStatus,
    disableContent,
    disableFaces,
    enableContent,
    enableFaces,
    facesStatus,
    onContentDownload,
    onFacesDownload,
    type FacesStatus,
  } from "./api";
  import { server } from "./platform";
  import { formatBytes } from "./format";
  import { chooseLanguage, errorText, languageChoice, LANGUAGES, t, type LanguageChoice } from "./i18n";
  import type { Order } from "./api";
  import { ORDERS, saveSettings, settings, SLIDE_SECONDS, TRANSITIONS, type Transition } from "./settings.svelte";

  interface Props {
    /** Told when face grouping was switched on or off. */
    onfaceschange?: () => void;
  }
  let { onfaceschange }: Props = $props();

  let open = $state(false);
  let faces = $state<FacesStatus | null>(null);
  /** Bytes received while the models download; `null` when not downloading. */
  let received = $state<number | null>(null);
  let error = $state<string | null>(null);
  /** Search by content (what photos show), and its download, as for the faces. */
  let content = $state<FacesStatus | null>(null);
  let contentReceived = $state<number | null>(null);
  /** On a Phototek server, these are set on the server, not here. */
  const onServer = $derived(open && server() !== null);

  onMount(() => {
    const unlisten = onFacesDownload((bytes) => {
      if (received !== null) received = bytes;
    });
    const unlistenContent = onContentDownload((bytes) => {
      if (contentReceived !== null) contentReceived = bytes;
    });
    return () => {
      unlisten.then((stop) => stop());
      unlistenContent.then((stop) => stop());
    };
  });

  async function show() {
    open = true;
    error = null;
    try {
      [faces, content] = await Promise.all([facesStatus(), contentStatus()]);
    } catch (e) {
      error = errorText(e);
    }
  }

  async function setContent(on: boolean) {
    error = null;
    try {
      if (on) {
        contentReceived = content?.downloaded ? null : 0;
        await enableContent();
      } else {
        await disableContent();
      }
      content = await contentStatus();
    } catch (e) {
      error = errorText(e);
      content = await contentStatus().catch(() => content);
    } finally {
      contentReceived = null;
    }
  }

  async function setFaces(on: boolean) {
    error = null;
    try {
      if (on) {
        received = faces?.downloaded ? null : 0;
        await enableFaces();
      } else {
        await disableFaces();
      }
      faces = await facesStatus();
      onfaceschange?.();
    } catch (e) {
      error = errorText(e);
      faces = await facesStatus().catch(() => faces);
    } finally {
      received = null;
    }
  }

  function setOrder(value: string) {
    settings.order = value as Order;
    saveSettings();
  }

  function setSeconds(value: string) {
    settings.slideSeconds = Number(value);
    saveSettings();
  }

  function setTransition(value: string) {
    settings.transition = value as Transition;
    saveSettings();
  }
</script>

<svelte:window onkeydown={(event) => open && event.key === "Escape" && (open = false)} />

<button class="settings-button" onclick={show} title={t("Settings")} aria-label={t("Settings")}>
  <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    <circle cx="8" cy="8" r="2.2" />
    <path
      d="M8 1.5v1.6M8 12.9v1.6M1.5 8h1.6M12.9 8h1.6M3.4 3.4l1.15 1.15M11.45 11.45l1.15 1.15M3.4 12.6l1.15-1.15M11.45 4.55l1.15-1.15"
    />
    <circle cx="8" cy="8" r="4.6" />
  </svg>
</button>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={() => (open = false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="box" role="dialog" aria-modal="true" aria-label={t("Settings")} tabindex="-1" onclick={(event) => event.stopPropagation()}>
      <header>
        <h2>{t("Settings")}</h2>
        <button class="close" onclick={() => (open = false)} aria-label={t("Close")}>✕</button>
      </header>

      <section>
        <h3>{t("Language")}</h3>
        <label class="row">
          <span>{t("Language")}</span>
          <select
            value={languageChoice()}
            onchange={(event) => chooseLanguage(event.currentTarget.value as LanguageChoice)}
          >
            {#each Object.entries(LANGUAGES) as [value, label] (value)}
              <option {value}>{value === "system" ? t(label) : label}</option>
            {/each}
          </select>
        </label>
        <p class="note">{t("The app reloads in the language chosen.")}</p>
      </section>

      <section>
        <h3>{t("Photos")}</h3>
        <label class="row">
          <span>{t("Show photos")}</span>
          <select value={settings.order} onchange={(event) => setOrder(event.currentTarget.value)}>
            {#each Object.entries(ORDERS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </label>
        <p class="note">{t("The Sort button above the photos changes it too.")}</p>
      </section>

      <section>
        <h3>{t("Slideshow")}</h3>
        <label class="row">
          <span>{t("Each photo stays")}</span>
          <select value={String(settings.slideSeconds)} onchange={(event) => setSeconds(event.currentTarget.value)}>
            {#each SLIDE_SECONDS as seconds (seconds)}
              <option value={String(seconds)}>{t("{n} seconds", { n: seconds })}</option>
            {/each}
          </select>
        </label>
        <label class="row">
          <span>{t("Transition")}</span>
          <select value={settings.transition} onchange={(event) => setTransition(event.currentTarget.value)}>
            {#each Object.entries(TRANSITIONS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </label>
      </section>

      <section>
        <h3>{t("People")}</h3>
        <label class="row">
          <span>{t("Find faces and group photos by person")}</span>
          <input
            type="checkbox"
            role="switch"
            class="switch"
            checked={faces?.enabled ?? false}
            disabled={faces === null || received !== null || onServer}
            onchange={(event) => setFaces(event.currentTarget.checked)}
          />
        </label>
        {#if received !== null && faces}
          <progress value={received} max={faces.downloadBytes}></progress>
        {/if}
        <p class="note">
          {#if onServer}
            {t("Set on the Phototek server (PHOTOTEK_FACES).")}
          {:else}
            {t("Everything happens on this computer: no photo and no face leaves it.")}
            {#if faces && !faces.downloaded}
              {t("Switching it on downloads two model files ({size}) once.", { size: formatBytes(faces.downloadBytes) })}
            {:else if faces && !faces.enabled}
              {t("The people already found stay; no new faces are searched while it is off.")}
            {/if}
          {/if}
        </p>
      </section>

      <section>
        <h3>{t("Search")}</h3>
        <label class="row">
          <span>{t("Find what photos show: a beach, a dog, a birthday…")}</span>
          <input
            type="checkbox"
            role="switch"
            class="switch"
            checked={content?.enabled ?? false}
            disabled={content === null || contentReceived !== null || onServer}
            onchange={(event) => setContent(event.currentTarget.checked)}
          />
        </label>
        {#if contentReceived !== null && content}
          <progress value={contentReceived} max={content.downloadBytes}></progress>
        {/if}
        <p class="note">
          {#if onServer}
            {t("Set on the Phototek server (PHOTOTEK_CONTENT).")}
          {:else}
            {t("Then type what you're looking for in the search box, in English, French or German.")}
            {t("Everything happens on this computer: no photo leaves it.")}
            {#if content && !content.downloaded}
              {t("Switching it on downloads a model file ({size}) once.", { size: formatBytes(content.downloadBytes) })}
            {/if}
          {/if}
        </p>
      </section>

      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .settings-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 28px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }

  .settings-button:hover {
    background: var(--surface-hover);
  }

  .settings-button svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.45);
  }

  .box {
    width: min(440px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    border-radius: 14px;
    background: var(--surface);
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.18);
    color: var(--text);
    outline: none;
    text-align: left;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px 12px 20px;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }

  .close {
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--text-dim);
    font-size: 14px;
    cursor: pointer;
  }

  .close:hover {
    background: var(--surface-hover);
  }

  section {
    padding: 14px 20px 16px;
  }

  section + section {
    border-top: 1px solid var(--border);
  }

  h3 {
    margin: 0 0 8px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 34px;
    font-size: 13px;
  }

  select {
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }

  .switch {
    appearance: none;
    position: relative;
    flex: none;
    width: 36px;
    height: 20px;
    margin: 0;
    border-radius: 999px;
    background: var(--border);
    cursor: pointer;
    transition: background 0.15s;
  }

  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.3);
    transition: transform 0.15s;
  }

  .switch:checked {
    background: var(--accent);
  }

  .switch:checked::after {
    transform: translateX(16px);
  }

  .switch:disabled {
    opacity: 0.5;
    cursor: default;
  }

  progress {
    width: 100%;
    margin-top: 6px;
  }

  .note {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--text-dim);
  }

  .error {
    margin: 0;
    padding: 0 20px 16px;
    color: var(--danger);
    font-size: 12px;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
