<!--
  Draws a photo with its edit applied (rotation, mirror, crop, adjustments),
  filling its parent ("cover") or fitted inside it ("contain").
-->
<script lang="ts" module>
  /** Gives each image its own filter id. */
  let nextId = 0;
</script>

<script lang="ts">
  import { enhanceTables, filterCss, place, type Edit } from "./edit";

  interface Props {
    src: string;
    alt?: string;
    edit: Edit;
    /** The photo's upright size, when known; else it is read from the image. */
    width: number | null;
    height: number | null;
    fit: "cover" | "contain";
  }
  let { src, alt = "", edit, width, height, fit }: Props = $props();

  let boxWidth = $state(0);
  let boxHeight = $state(0);
  let loaded = $state<{ width: number; height: number } | null>(null);

  // The magic wand's curves can't be written as CSS filters; they are an
  // SVG filter that the image refers to.
  const filterId = `enhance-${nextId++}`;
  const tables = $derived(edit.enhance ? enhanceTables(edit.enhance) : null);
  const filter = $derived(
    [tables ? `url(#${filterId})` : "", filterCss(edit)].filter(Boolean).join(" "),
  );

  const placement = $derived.by(() => {
    const natural = width && height ? { width, height } : loaded;
    if (!natural || boxWidth === 0 || boxHeight === 0) return null;
    return place(edit, natural.width, natural.height, boxWidth, boxHeight, fit);
  });
</script>

{#if tables && edit.enhance}
  <svg class="filters" aria-hidden="true">
    <filter id={filterId} color-interpolation-filters="sRGB">
      <feComponentTransfer>
        <feFuncR type="table" tableValues={tables[0]} />
        <feFuncG type="table" tableValues={tables[1]} />
        <feFuncB type="table" tableValues={tables[2]} />
      </feComponentTransfer>
      {#if edit.enhance.vibrance > 0}
        <feColorMatrix type="saturate" values={String(1 + edit.enhance.vibrance / 100)} />
      {/if}
    </filter>
  </svg>
{/if}

<div class="box" bind:clientWidth={boxWidth} bind:clientHeight={boxHeight}>
  <div
    class="clip"
    style:width="{placement?.clipWidth ?? 0}px"
    style:height="{placement?.clipHeight ?? 0}px"
  >
    <img
      {src}
      {alt}
      draggable="false"
      decoding="async"
      style:width="{placement?.imageWidth ?? 0}px"
      style:height="{placement?.imageHeight ?? 0}px"
      style:transform={placement?.transform}
      style:filter={filter || null}
      style:visibility={placement ? "visible" : "hidden"}
      onload={(event) => {
        const image = event.currentTarget as HTMLImageElement;
        loaded = { width: image.naturalWidth, height: image.naturalHeight };
      }}
    />
  </div>
</div>

<style>
  .filters {
    position: absolute;
    width: 0;
    height: 0;
  }

  .box {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .clip {
    position: relative;
    flex-shrink: 0;
    overflow: hidden;
  }

  img {
    position: absolute;
    left: 50%;
    top: 50%;
    max-width: none;
  }
</style>
