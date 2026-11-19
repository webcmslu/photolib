<!--
  Every keyboard shortcut, opened with "?" from anywhere. It takes the keys
  while open, so Escape closes it and not the viewer below.
-->
<script lang="ts">
  import { t } from "./i18n";

  interface Props {
    onclose: () => void;
  }
  let { onclose }: Props = $props();

  const MOD = navigator.userAgent.includes("Mac") ? "⌘" : "Ctrl";

  const GROUPS: { title: string; keys: [string, string][] }[] = [
    {
      title: t("Photos"),
      keys: [
        [t("Click"), t("Open a photo")],
        [t("{mod}-click  Shift-click", { mod: MOD }), t("Select, select a range")],
        [`${MOD} A`, t("Select all")],
        ["Esc", t("Clear the selection")],
        ["Enter", t("Open the selected photo")],
        [".", t("Favorite or not")],
        ["H", t("Hide or show")],
        ["1 – 5", t("Rate with stars (again to clear)")],
        ["6 – 9", t("Label red, yellow, green, blue")],
        [`${MOD} E`, t("Export the selection")],
        [t("{mod} F  or  /", { mod: MOD }), t("Search")],
        [`${MOD} +  ${MOD} −`, t("Larger or smaller thumbnails")],
        ["F5", t("Slideshow")],
      ],
    },
    {
      title: t("Viewer"),
      keys: [
        ["← →", t("Previous, next")],
        [t("Home  End"), t("First, last")],
        ["+  −  0", t("Zoom in, out, fit")],
        [".", t("Favorite or not")],
        ["H", t("Hide or show")],
        ["1 – 5   6 – 9", t("Rate, label")],
        ["I", t("Info panel")],
        ["T", t("Filmstrip")],
        ["F", t("Full screen")],
        ["E", t("Edit (Enter saves, Esc cancels)")],
        [t("Space  or  L"), t("Play the video or Live Photo")],
        ["Esc", t("Back to the grid")],
      ],
    },
    {
      title: t("Slideshow and similar shots"),
      keys: [
        [t("Space"), t("Pause, or keep / drop a similar shot")],
        ["← →", t("Previous, next")],
        ["Esc", t("Stop")],
      ],
    },
  ];

  function onkeydown(event: KeyboardEvent) {
    // Nothing below sees the keys while the panel is open.
    event.stopImmediatePropagation();
    if (event.key === "Escape" || event.key === "?") {
      event.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="box" role="dialog" aria-modal="true" aria-label={t("Keyboard shortcuts")} tabindex="-1" onclick={(event) => event.stopPropagation()}>
    <header>
      <h2>{t("Keyboard shortcuts")}</h2>
      <button class="close" onclick={onclose} aria-label={t("Close")}>✕</button>
    </header>
    <div class="groups">
      {#each GROUPS as group (group.title)}
        <section>
          <h3>{group.title}</h3>
          <dl>
            {#each group.keys as [keys, what] (keys + what)}
              <dt>{#each keys.split(/\s{2,}/) as key, i (i)}<kbd>{key}</kbd>{/each}</dt>
              <dd>{what}</dd>
            {/each}
          </dl>
        </section>
      {/each}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 2000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.45);
  }

  .box {
    width: min(760px, calc(100vw - 32px));
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

  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: 8px 28px;
    padding: 14px 20px 18px;
  }

  h3 {
    margin: 4px 0 8px;
    font-size: 12px;
    font-weight: 650;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 14px;
    margin: 0 0 10px;
    font-size: 13px;
  }

  dt {
    display: flex;
    gap: 4px;
    white-space: nowrap;
  }

  dd {
    margin: 0;
    color: var(--text-dim);
  }

  kbd {
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    background: var(--bg);
    font: inherit;
    font-size: 12px;
  }
</style>
