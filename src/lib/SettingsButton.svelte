<!--
  The ⚙ button and the Settings window: how slideshows run, and whether
  faces are searched to group photos by person.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { disableFaces, enableFaces, facesStatus, onFacesDownload, type FacesStatus } from "./api";
  import { formatBytes } from "./format";
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

  onMount(() => {
    const unlisten = onFacesDownload((bytes) => {
      if (received !== null) received = bytes;
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  async function show() {
    open = true;
    error = null;
    try {
      faces = await facesStatus();
    } catch (e) {
      error = String(e);
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
      error = String(e);
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

<button class="settings-button" onclick={show} title="Settings" aria-label="Settings">
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
    <div class="box" role="dialog" aria-modal="true" aria-label="Settings" tabindex="-1" onclick={(event) => event.stopPropagation()}>
      <header>
        <h2>Settings</h2>
        <button class="close" onclick={() => (open = false)} aria-label="Close">✕</button>
      </header>

      <section>
        <h3>Photos</h3>
        <label class="row">
          <span>Show photos</span>
          <select value={settings.order} onchange={(event) => setOrder(event.currentTarget.value)}>
            {#each Object.entries(ORDERS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </label>
        <p class="note">The Sort button above the photos changes it too.</p>
      </section>

      <section>
        <h3>Slideshow</h3>
        <label class="row">
          <span>Each photo stays</span>
          <select value={String(settings.slideSeconds)} onchange={(event) => setSeconds(event.currentTarget.value)}>
            {#each SLIDE_SECONDS as seconds (seconds)}
              <option value={String(seconds)}>{seconds} seconds</option>
            {/each}
          </select>
        </label>
        <label class="row">
          <span>Transition</span>
          <select value={settings.transition} onchange={(event) => setTransition(event.currentTarget.value)}>
            {#each Object.entries(TRANSITIONS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </label>
      </section>

      <section>
        <h3>People</h3>
        <label class="row">
          <span>Find faces and group photos by person</span>
          <input
            type="checkbox"
            role="switch"
            class="switch"
            checked={faces?.enabled ?? false}
            disabled={faces === null || received !== null}
            onchange={(event) => setFaces(event.currentTarget.checked)}
          />
        </label>
        {#if received !== null && faces}
          <progress value={received} max={faces.downloadBytes}></progress>
        {/if}
        <p class="note">
          Everything happens on this computer: no photo and no face leaves it.
          {#if faces && !faces.downloaded}
            Switching it on downloads two model files ({formatBytes(faces.downloadBytes)}) once.
          {:else if faces && !faces.enabled}
            The people already found stay; no new faces are searched while it is off.
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
