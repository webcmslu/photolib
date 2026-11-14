<!--
  Draws a photo with its edit applied (rotation, mirror, crop, adjustments),
  filling its parent ("cover") or fitted inside it ("contain").
-->
<script lang="ts">
  import { filterCss, place, type Edit } from "./edit";

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

  const placement = $derived.by(() => {
    const natural = width && height ? { width, height } : loaded;
    if (!natural || boxWidth === 0 || boxHeight === 0) return null;
    return place(edit, natural.width, natural.height, boxWidth, boxHeight, fit);
  });
</script>

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
      style:filter={filterCss(edit)}
      style:visibility={placement ? "visible" : "hidden"}
      onload={(event) => {
        const image = event.currentTarget as HTMLImageElement;
        loaded = { width: image.naturalWidth, height: image.naturalHeight };
      }}
    />
  </div>
</div>

<style>
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
