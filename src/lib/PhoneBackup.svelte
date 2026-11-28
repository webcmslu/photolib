<!--
  "This phone", in Settings on the phone app: back up the phone's photos and
  videos to the Phototek server in use (into Phones/<name>/ in its library),
  each time the app opens or now, and free up space by removing from the
  phone those safely on the server.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { errorText, t, tn } from "./i18n";
  import { formatNumber } from "./format";
  import {
    autoBackup,
    backupStatus,
    freeSpace,
    onBackupProgress,
    phoneName,
    savePhoneName,
    setAutoBackup,
    spaceToFree,
    startBackup,
    stopBackup,
    type BackupProgress,
  } from "./phone";

  let progress = $state<BackupProgress | null>(null);
  let name = $state("");
  let auto = $state(autoBackup());
  let error = $state<string | null>(null);
  /** Freeing up space: how many can go (asked first), then how many went. */
  let toFree = $state<number | null>(null);
  let freed = $state<number | null>(null);
  let busy = $state(false);

  onMount(() => {
    phoneName().then((found) => (name = found));
    backupStatus()
      .then((status) => (progress = status))
      .catch((e) => (error = errorText(e)));
    const unlisten = onBackupProgress((update) => {
      progress = update;
      if (update.error) error = update.error;
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  async function backUp() {
    error = null;
    freed = null;
    try {
      await startBackup();
      if (progress) progress = { ...progress, running: true, sent: 0, failed: 0, error: null };
    } catch (e) {
      error = errorText(e);
    }
  }

  async function askToFree() {
    error = null;
    freed = null;
    busy = true;
    try {
      toFree = await spaceToFree();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  async function free() {
    busy = true;
    error = null;
    try {
      freed = await freeSpace();
      toFree = null;
      progress = await backupStatus();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<section>
  <h3>{t("This phone")}</h3>
  <label class="row">
    <span>{t("Its photos go into")}</span>
    <span class="folder">
      Phones/<input
        bind:value={name}
        onchange={() => name.trim() && savePhoneName(name)}
        autocapitalize="off"
        spellcheck="false"
        aria-label={t("This phone's name")}
      />
    </span>
  </label>

  {#if progress}
    <p class="status">
      {t("{saved} of {total} photos and videos are on the server.", {
        saved: formatNumber(progress.saved),
        total: formatNumber(progress.total),
      })}
    </p>
    {#if progress.running}
      <progress value={progress.saved} max={Math.max(progress.total, 1)}></progress>
      <p class="note">
        {tn(progress.sent, "Backing up… 1 sent so far.", "Backing up… {n} sent so far.")}
      </p>
    {:else if progress.failed > 0}
      <p class="note">
        {tn(progress.failed, "1 couldn't be sent; it will be tried again.", "{n} couldn't be sent; they will be tried again.")}
      </p>
    {/if}
  {/if}

  <div class="buttons">
    {#if progress?.running}
      <button onclick={() => stopBackup()}>{t("Stop")}</button>
    {:else}
      <button class="primary" onclick={backUp} disabled={progress === null && error !== null}>{t("Back Up Now")}</button>
    {/if}
    <button onclick={askToFree} disabled={busy || progress?.running}>{t("Free Up Space…")}</button>
  </div>

  {#if toFree !== null}
    <div class="confirm">
      {#if toFree === 0}
        <p>{t("Nothing to remove yet: back up first.")}</p>
        <button onclick={() => (toFree = null)}>{t("OK")}</button>
      {:else}
        <p>
          {tn(
            toFree,
            "Remove 1 photo or video from this phone? It is safely on the server.",
            "Remove {n} photos and videos from this phone? They are safely on the server.",
          )}
          {t("The phone asks you to confirm, and keeps them 30 days in its trash (Recently Deleted on an iPhone).")}
        </p>
        <div class="buttons">
          <button onclick={() => (toFree = null)}>{t("Cancel")}</button>
          <button class="danger" onclick={free} disabled={busy}>{tn(toFree, "Remove 1", "Remove {n}")}</button>
        </div>
      {/if}
    </div>
  {/if}
  {#if freed !== null}
    <p class="note">{tn(freed, "1 photo or video removed from this phone.", "{n} photos and videos removed from this phone.")}</p>
  {/if}

  <label class="row">
    <span>{t("Back up each time the app opens")}</span>
    <input
      type="checkbox"
      role="switch"
      class="switch"
      bind:checked={auto}
      onchange={() => setAutoBackup(auto)}
    />
  </label>
  <p class="note">
    {t("Only what the server doesn't have is sent, in full quality, into the folder above, and only to your own server.")}
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  /* As the other sections of Settings (SettingsButton.svelte). */
  section {
    padding: 14px 20px 16px;
    border-top: 1px solid var(--border);
    font-size: 13px;
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

  .note {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--text-dim);
  }

  .error {
    margin: 8px 0 0;
    color: var(--danger);
  }

  .folder {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--text-dim);
  }

  .folder input {
    width: 9em;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
  }

  .status {
    margin: 8px 0 4px;
  }

  progress {
    width: 100%;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 8px 0;
  }

  .buttons button {
    padding: 6px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }

  .buttons button.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-text);
  }

  .buttons button.danger {
    border-color: var(--danger);
    background: var(--danger);
    color: #fff;
  }

  .buttons button:disabled {
    opacity: 0.5;
  }

  .confirm {
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg);
  }

  .confirm p {
    margin: 0 0 6px;
  }
</style>
