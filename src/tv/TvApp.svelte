<!--
  The TV app: a Phototek server's library, with the remote control. Screens
  pile up (home, a grid of photos, a photo, a memory) and Back goes down the
  pile; each keeps its place while another is on top.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { openLibrary, type Listing, type Memory, type PhotoItem } from "../lib/api";
  import { t } from "../lib/i18n";
  import { unlockAudio } from "../lib/memories";
  import MemoryPlayer from "../lib/MemoryPlayer.svelte";
  import { savedServers, useServer, type Server } from "../lib/platform";
  import { focusFirst, focusOn, moveFocus, remoteKey } from "./remote";
  import TvConnect from "./TvConnect.svelte";
  import TvGrid from "./TvGrid.svelte";
  import TvHome from "./TvHome.svelte";
  import TvViewer from "./TvViewer.svelte";

  type Screen =
    | { kind: "connect" }
    | { kind: "home" }
    | { kind: "grid"; title: string; listing: Listing }
    | { kind: "viewer"; title: string; photos: (index: number) => Promise<PhotoItem | null>; total: number; index: number }
    | { kind: "memory"; memory: Memory };

  let screens = $state<Screen[]>([]);
  let libraryPath = $state("");
  let libraryName = $state("");
  /** Where the focus was on each screen, to come back to it. */
  const focusWas = new Map<number, HTMLElement>();
  /** The viewer on top handles its own keys. */
  let viewer = $state<{ handleKey: (key: string) => boolean } | null>(null);

  const top = $derived(screens.at(-1));

  async function connected(server: Server) {
    useServer(server);
    const library = await openLibrary("");
    libraryPath = library.path;
    libraryName = library.name;
    screens = [{ kind: "home" }];
    await tick();
    focusFirst();
  }

  async function push(screen: Screen) {
    focusWas.set(screens.length - 1, document.activeElement as HTMLElement);
    screens = [...screens, screen];
    await tick();
    focusFirst();
  }

  async function back() {
    if (screens.length <= 1) {
      // Back on the first screen leaves the app, as TV apps do.
      window.close();
      return;
    }
    screens = screens.slice(0, -1);
    await tick();
    const was = focusWas.get(screens.length - 1);
    if (was?.isConnected) focusOn(was);
    else focusFirst();
  }

  async function showConnect() {
    screens = [{ kind: "connect" }];
    await tick();
    focusFirst();
  }

  function disconnect() {
    useServer(null);
    showConnect();
  }

  onMount(() => {
    const [last] = savedServers();
    if (last) {
      connected(last).catch(() => showConnect());
    } else {
      showConnect();
    }
  });

  function onkeydown(event: KeyboardEvent) {
    const key = remoteKey(event);
    if (!key) return;
    const typing = (event.target as HTMLElement | null)?.tagName === "INPUT";
    // A memory plays with its own keys (Escape, space, the arrows).
    if (top?.kind === "memory") {
      if (key === "back") {
        event.preventDefault();
        back();
      } else if (key === "ok" || key === "playPause" || key === "play" || key === "pause") {
        event.preventDefault();
        window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
      }
      return;
    }
    if (top?.kind === "viewer" && viewer?.handleKey(key)) {
      event.preventDefault();
      return;
    }
    if (key === "back") {
      if (typing && event.key === "Backspace") return;
      event.preventDefault();
      back();
    } else if (key === "up" || key === "down" || ((key === "left" || key === "right") && !typing)) {
      event.preventDefault();
      moveFocus(key);
    }
    // OK clicks the focused button, as browsers do with Enter.
  }
</script>

<svelte:window {onkeydown} />

{#each screens as screen, index (index)}
  <div class="screen" class:hidden={index < screens.length - 1} data-screen>
    {#if screen.kind === "connect"}
      <TvConnect onconnected={connected} />
    {:else if screen.kind === "home"}
      <TvHome
        {libraryPath}
        {libraryName}
        onlisting={(title, listing) => push({ kind: "grid", title, listing })}
        onmemory={(memory) => {
          unlockAudio();
          push({ kind: "memory", memory });
        }}
        onservers={disconnect}
      />
    {:else if screen.kind === "grid"}
      <TvGrid
        {libraryPath}
        title={screen.title}
        listing={screen.listing}
        onopen={(photos, total, index) => push({ kind: "viewer", title: screen.title, photos, total, index })}
      />
    {:else if screen.kind === "viewer"}
      <TvViewer bind:this={viewer} photos={screen.photos} total={screen.total} start={screen.index} />
    {:else if screen.kind === "memory"}
      <MemoryPlayer
        {libraryPath}
        memory={screen.memory}
        onclose={back}
        onshowall={() => {
          const memory = screen.memory;
          screens = screens.slice(0, -1);
          push({ kind: "grid", title: t("Memory"), listing: memory.listing });
        }}
      />
    {/if}
  </div>
{/each}

<style>
  .screen {
    position: fixed;
    inset: 0;
    background: var(--bg);
  }

  .hidden {
    visibility: hidden;
  }
</style>
