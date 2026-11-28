<!--
  One photo or video full screen on the TV. Left and right move through the
  listing, OK shows or hides when it was taken (or pauses a video), Play
  starts a slideshow. The preview shows at once, the full photo once loaded.
-->
<script lang="ts">
  import { photoUrl, thumbUrl, type PhotoItem } from "../lib/api";
  import EditedImage from "../lib/EditedImage.svelte";
  import { NO_EDIT } from "../lib/edit";
  import { locale, t } from "../lib/i18n";

  interface Props {
    photos: (index: number) => Promise<PhotoItem | null>;
    total: number;
    start: number;
  }
  let { photos, total, start }: Props = $props();

  const SLIDE_MS = 5000;
  let index = $state(0);
  let photo = $state<PhotoItem | null>(null);
  /** The full-size photo, once it has loaded. */
  let full = $state<string | null>(null);
  let info = $state(true);
  let playing = $state(false);
  let video = $state<HTMLVideoElement | null>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let infoTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    show(start);
  });

  async function show(at: number) {
    index = Math.max(0, Math.min(at, total - 1));
    const wanted = index;
    const found = await photos(wanted);
    if (wanted !== index || !found) return;
    photo = found;
    full = null;
    if (!found.video) {
      const url = photoUrl(found.fingerprint);
      const image = new Image();
      image.onload = () => {
        if (photo?.fingerprint === found.fingerprint) full = url;
      };
      image.src = url;
    }
    showInfoAWhile();
    if (playing) schedule();
  }

  function showInfoAWhile() {
    info = true;
    if (infoTimer) clearTimeout(infoTimer);
    infoTimer = setTimeout(() => (info = false), 3500);
  }

  function schedule() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      if (index < total - 1) show(index + 1);
      else playing = false;
    }, SLIDE_MS);
  }

  function togglePlay() {
    playing = !playing;
    if (playing) schedule();
    else if (timer) clearTimeout(timer);
  }

  /** The remote's keys; false for those the app handles (Back). */
  export function handleKey(key: string): boolean {
    switch (key) {
      case "left":
        if (index > 0) show(index - 1);
        return true;
      case "right":
        if (index < total - 1) show(index + 1);
        return true;
      case "ok":
        if (photo?.video && video) {
          if (video.paused) video.play();
          else video.pause();
        } else if (info) {
          info = false;
        } else {
          showInfoAWhile();
        }
        return true;
      case "play":
      case "pause":
      case "playPause":
        togglePlay();
        return true;
      case "up":
      case "down":
        return true;
    }
    return false;
  }

  $effect(() => () => {
    if (timer) clearTimeout(timer);
    if (infoTimer) clearTimeout(infoTimer);
  });

  const dateFormat = new Intl.DateTimeFormat(locale, { dateStyle: "full", timeStyle: "short" });
</script>

<div class="viewer">
  {#if photo}
    {#if photo.video}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video bind:this={video} src={photoUrl(photo.fingerprint)} autoplay></video>
    {:else}
      {#key photo.fingerprint}
        <EditedImage
          src={full ?? thumbUrl(photo.fingerprint, "preview")}
          edit={photo.edit ?? NO_EDIT}
          width={photo.width}
          height={photo.height}
          fit="contain"
        />
      {/key}
    {/if}
    <div class="info" class:shown={info}>
      <span>{dateFormat.format(new Date(photo.sortDate))}</span>
      <span class="position">
        {#if playing}▶ {/if}{index + 1} / {total}
      </span>
    </div>
    {#if info}
      <p class="hint">{t("◀ ▶ to move · OK for details · ▶︎❙❙ for a slideshow")}</p>
    {/if}
  {/if}
</div>

<style>
  .viewer {
    position: relative;
    width: 100%;
    height: 100%;
    background: #000;
  }

  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .info {
    position: absolute;
    inset: auto 0 0;
    display: flex;
    justify-content: space-between;
    padding: 80px 96px 48px;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.7));
    color: #fff;
    font-size: 28px;
    opacity: 0;
    transition: opacity 0.4s;
  }

  .info.shown {
    opacity: 1;
  }

  .position {
    color: rgb(255 255 255 / 0.75);
  }

  .hint {
    position: absolute;
    top: 40px;
    left: 0;
    right: 0;
    margin: 0;
    text-align: center;
    color: rgb(255 255 255 / 0.85);
    font-size: 22px;
    text-shadow: 0 1px 6px rgb(0 0 0 / 0.9);
  }
</style>
