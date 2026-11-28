<!--
  The first screen of the TV app: the Phototek server to show (on the NAS,
  e.g. 192.168.1.20:8080), its password if it has one, the servers used
  before, or the demo.
-->
<script lang="ts">
  import { errorText, t } from "../lib/i18n";
  import { DEMO_SERVER, logIn, savedServers, saveServer, serverUrl, type Server } from "../lib/platform";

  interface Props {
    /** Told once the server answered; it opens the library. */
    onconnected: (server: Server) => Promise<void>;
  }
  let { onconnected }: Props = $props();

  let address = $state("");
  let password = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  const servers = savedServers();

  async function connect(url: string, typedPassword: string | null, known?: Server) {
    busy = true;
    error = null;
    try {
      const token = typedPassword === null ? (known?.token ?? null) : await logIn(url, typedPassword);
      const server = { url, token, name: known?.name };
      await onconnected(server);
      saveServer(server);
    } catch (e) {
      const message = e instanceof Error ? e.message : errorText(e);
      error =
        message === "wrong password"
          ? t("Wrong password.")
          : t("Can't reach the Phototek server at {address}: {message}", { address: url, message });
    } finally {
      busy = false;
    }
  }
</script>

<div class="connect">
  <h1>Phototek</h1>
  <p class="lead">{t("Your photos, from the Phototek server on your NAS.")}</p>

  <form
    onsubmit={(event) => {
      event.preventDefault();
      if (address.trim()) connect(serverUrl(address), password);
    }}
  >
    <label>
      <span>{t("Server address")}</span>
      <input data-focus bind:value={address} placeholder="192.168.1.20:8080" autocapitalize="off" spellcheck="false" />
    </label>
    <label>
      <span>{t("Password, if it has one")}</span>
      <input data-focus type="password" bind:value={password} />
    </label>
    <button data-focus class="primary" type="submit" disabled={busy}>{busy ? t("Connecting…") : t("Connect")}</button>
  </form>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <div class="others">
    {#each servers as server (server.url)}
      <button data-focus onclick={() => connect(server.url, null, server)} disabled={busy}>
        {server.name ?? server.url}
      </button>
    {/each}
    {#if !servers.some((server) => server.url === DEMO_SERVER)}
      <button data-focus onclick={() => connect(DEMO_SERVER, "")} disabled={busy}>{t("Try the demo")}</button>
    {/if}
  </div>
</div>

<style>
  .connect {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding-top: 140px;
  }

  h1 {
    margin: 0;
    font-size: 72px;
  }

  .lead {
    margin: 8px 0 48px;
    color: var(--text-dim);
  }

  form {
    display: grid;
    gap: 22px;
    width: 760px;
  }

  label {
    display: grid;
    gap: 8px;
    color: var(--text-dim);
  }

  input {
    padding: 16px 20px;
    border: 2px solid var(--border);
    border-radius: 14px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  button {
    padding: 16px 32px;
    border: none;
    border-radius: 14px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  button.primary {
    background: var(--accent);
    color: var(--accent-text);
  }

  .others {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 20px;
    margin-top: 48px;
  }

  .error {
    margin: 24px 0 0;
    color: var(--danger);
  }
</style>
