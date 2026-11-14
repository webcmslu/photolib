<!--
  The crop rectangle drawn over a photo while editing: drag it to move it,
  drag a corner to resize it. It fills its parent, which must be exactly the
  picture's area on screen.
-->
<script lang="ts">
  interface Props {
    /** Left, top, width, height as fractions (0 to 1) of the picture. */
    crop: [number, number, number, number];
    /**
     * Width divided by height to keep, measured in the same fractions
     * (so it already accounts for the picture's own shape); `null` is free.
     */
    ratio: number | null;
    onchange: (crop: [number, number, number, number]) => void;
  }
  let { crop, ratio, onchange }: Props = $props();

  const MIN = 0.05;
  let area: HTMLDivElement;
  let drag: {
    pointerId: number;
    startX: number;
    startY: number;
    start: [number, number, number, number];
    /** Which sides move: -1 left/top, 1 right/bottom, 0 the whole box. */
    sideX: number;
    sideY: number;
  } | null = null;

  function begin(event: PointerEvent, sideX: number, sideY: number) {
    event.stopPropagation();
    event.preventDefault();
    drag = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      start: [...crop],
      sideX,
      sideY,
    };
    area.setPointerCapture(event.pointerId);
  }

  function move(event: PointerEvent) {
    if (drag?.pointerId !== event.pointerId) return;
    const box = area.getBoundingClientRect();
    const dx = (event.clientX - drag.startX) / box.width;
    const dy = (event.clientY - drag.startY) / box.height;
    let [left, top, width, height] = drag.start;
    const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

    if (drag.sideX === 0 && drag.sideY === 0) {
      left = clamp(left + dx, 0, 1 - width);
      top = clamp(top + dy, 0, 1 - height);
    } else {
      // The corner opposite the dragged one stays where it is.
      const fixedX = drag.sideX < 0 ? left + width : left;
      const fixedY = drag.sideY < 0 ? top + height : top;
      const roomX = drag.sideX < 0 ? fixedX : 1 - fixedX;
      const roomY = drag.sideY < 0 ? fixedY : 1 - fixedY;
      width = clamp(width + drag.sideX * dx, MIN, roomX);
      height = clamp(height + drag.sideY * dy, MIN, roomY);
      if (ratio !== null) {
        // Follow the larger movement, then shrink to what fits.
        if (width / ratio > height) height = width / ratio;
        else width = height * ratio;
        const shrink = Math.min(1, roomX / width, roomY / height);
        width *= shrink;
        height *= shrink;
      }
      left = drag.sideX < 0 ? fixedX - width : fixedX;
      top = drag.sideY < 0 ? fixedY - height : fixedY;
    }
    onchange([left, top, width, height]);
  }

  function end(event: PointerEvent) {
    if (drag?.pointerId === event.pointerId) drag = null;
  }
</script>

<div
  class="area"
  bind:this={area}
  onpointermove={move}
  onpointerup={end}
  onpointercancel={end}
  role="presentation"
>
  <div
    class="box"
    style:left="{crop[0] * 100}%"
    style:top="{crop[1] * 100}%"
    style:width="{crop[2] * 100}%"
    style:height="{crop[3] * 100}%"
    onpointerdown={(event) => begin(event, 0, 0)}
    role="presentation"
  >
    {#each [[-1, -1], [1, -1], [-1, 1], [1, 1]] as [sideX, sideY] (`${sideX}${sideY}`)}
      <span
        class="handle"
        style:left={sideX < 0 ? "0" : "100%"}
        style:top={sideY < 0 ? "0" : "100%"}
        style:cursor={sideX === sideY ? "nwse-resize" : "nesw-resize"}
        onpointerdown={(event) => begin(event, sideX, sideY)}
        role="presentation"
      ></span>
    {/each}
  </div>
</div>

<style>
  .area {
    position: absolute;
    inset: 0;
    overflow: hidden;
    touch-action: none;
  }

  .box {
    position: absolute;
    border: 1px solid #fff;
    /* Darkens everything outside the box. */
    box-shadow: 0 0 0 9999px rgb(0 0 0 / 0.6);
    cursor: move;
  }

  .handle {
    position: absolute;
    width: 18px;
    height: 18px;
    margin: -9px 0 0 -9px;
    border: 2px solid #fff;
    border-radius: 50%;
    background: var(--accent);
  }
</style>
