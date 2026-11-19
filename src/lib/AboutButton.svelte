<!--
  The ⓘ button and the Info dialog: who makes the app, its version, and a
  notice with a direct download when a newer version is out (the button then
  shows a green dot). Same design as Marka's, in Photolib's colors.
-->
<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { about, checkForUpdate, installUpdate } from "./about.svelte";

  let open = $state(false);
  const WEBSITE = "https://webcms.lu";

  onMount(() => {
    checkForUpdate();
  });

  function visit(event: MouseEvent, url: string) {
    event.preventDefault();
    openUrl(url).catch(() => {});
  }
</script>

<svelte:window onkeydown={(event) => open && event.key === "Escape" && (open = false)} />

<button
  class="info-button"
  onclick={() => (open = true)}
  title={about.update ? `About ${about.name}: version ${about.update.version} is available` : `About ${about.name}`}
  aria-label="About {about.name}"
>
  <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.5" />
    <path d="M8 7.2v4M8 4.8v.1" />
  </svg>
  {#if about.update}<span class="dot"></span>{/if}
</button>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={() => (open = false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="box" role="dialog" aria-modal="true" aria-label="About {about.name}" tabindex="-1" onclick={(event) => event.stopPropagation()}>
      <div class="top">
        <span class="logo">{about.name.charAt(0)}</span>
        <span class="name">{about.name}</span>
        {#if about.version}<span class="version">v{about.version}</span>{/if}
      </div>
      {#if about.update}
        <div class="update">
          <span>v{about.update.version} is available</span>
          {#if about.installing}
            <span class="installing">
              {about.installing.total
                ? `Downloading… ${Math.round((100 * about.installing.received) / about.installing.total)}%`
                : "Downloading…"}
            </span>
          {:else if about.installable}
            <button class="install" onclick={installUpdate}>Install and restart</button>
          {:else}
            <a href={about.update.url} onclick={(event) => visit(event, about.update?.url ?? "")}>Download ↗</a>
          {/if}
        </div>
        {#if about.installError}
          <p class="install-error" role="alert">Couldn't install it: {about.installError}</p>
        {/if}
      {/if}
      <div class="body">
        <p class="label">Developed by</p>
        <p class="developer">WebCMS Sàrl</p>
        <a href={WEBSITE} onclick={(event) => visit(event, WEBSITE)}>webcms.lu</a>
      </div>
      <div class="footer">© 2026 WebCMS Sàrl · GPL v3</div>
      <button class="close" onclick={() => (open = false)} aria-label="Close">✕</button>
    </div>
  </div>
{/if}

<style>
  .info-button {
    position: relative;
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

  .info-button:hover {
    background: var(--surface-hover);
  }

  .info-button svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
  }

  .dot {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 7px;
    height: 7px;
    border: 1.5px solid var(--surface);
    border-radius: 50%;
    background: #22c55e;
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
    position: relative;
    width: 320px;
    overflow: hidden;
    border-radius: 20px;
    background: var(--surface);
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.18);
    outline: none;
  }

  .top {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 24px 0 20px;
    background: linear-gradient(135deg, #0a6cff, #3d8bff 55%, #19b5c9);
  }

  .logo {
    color: #fff;
    font-size: 28px;
    font-weight: 700;
    line-height: 1;
  }

  .name {
    color: rgb(255 255 255 / 0.85);
    font-size: 12px;
    font-weight: 500;
    letter-spacing: 2px;
    text-transform: uppercase;
  }

  .version {
    color: rgb(255 255 255 / 0.65);
    font-size: 11px;
  }

  .update {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 20px;
    border-bottom: 1px solid #bbf7d0;
    background: #f0fdf4;
    color: #166534;
    font-size: 13px;
    font-weight: 500;
  }

  .update a {
    color: #16a34a;
    font-size: 12px;
    font-weight: 600;
    text-decoration: none;
    white-space: nowrap;
  }

  .update a:hover {
    text-decoration: underline;
  }

  .install {
    padding: 4px 10px;
    border: none;
    border-radius: 6px;
    background: #16a34a;
    color: #fff;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
  }

  .install:hover {
    background: #15803d;
  }

  .installing {
    font-size: 12px;
    white-space: nowrap;
  }

  .install-error {
    margin: 0;
    padding: 8px 20px;
    border-bottom: 1px solid var(--border);
    color: var(--danger);
    font-size: 12px;
    user-select: text;
    -webkit-user-select: text;
  }

  @media (prefers-color-scheme: dark) {
    .update {
      border-bottom-color: #14532d;
      background: #052e16;
      color: #86efac;
    }

    .update a {
      color: #4ade80;
    }
  }

  .body {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 24px 24px 20px;
  }

  .body p {
    margin: 0;
  }

  .label {
    color: var(--text-dim);
    font-size: 11px;
  }

  .developer {
    color: var(--text);
    font-size: 18px;
    font-weight: 700;
  }

  .body a {
    color: var(--accent);
    font-size: 14px;
    font-weight: 500;
  }

  .footer {
    padding: 14px 24px 20px;
    border-top: 1px solid var(--border);
    color: var(--text-dim);
    font-size: 11px;
    text-align: center;
  }

  .close {
    position: absolute;
    top: 10px;
    right: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: rgb(255 255 255 / 0.2);
    color: #fff;
    font-size: 13px;
    cursor: pointer;
  }

  .close:hover {
    background: rgb(255 255 255 / 0.35);
  }
</style>
