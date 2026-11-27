// Makes an MP4 of a memory, as it plays in the player: the title over the
// cover coming into focus, each photo drifting slowly and fading into the
// next, upright ones on a blurred copy of themselves, the music, and an end
// card. Everything happens here, with the web view's own encoders
// (WebCodecs: H.264 video, AAC audio), so nothing extra is installed.

import { ArrayBufferTarget, Muxer } from "mp4-muxer";
import { photoUrl, thumbUrl, type PhotoItem } from "./api";
import { NO_EDIT, type Edit } from "./edit";
import { renderMusic } from "./memories";

export type VideoQuality = "high" | "small";

export interface VideoRequest {
  /** Seeds the music, as in the player. */
  memoryId: string;
  title: string;
  subtitle: string;
  cover: string;
  photos: PhotoItem[];
  quality: VideoQuality;
  /** Called with how far along it is, from 0 to 1. */
  onprogress: (done: number) => void;
  signal: AbortSignal;
}

// Timings of the player (MemoryPlayer.svelte), in seconds.
const TITLE_S = 4.2;
const SLIDE_S = 4.6;
const FADE_S = 1.2;
/** A photo drifts a little longer than it shows, since it fades out. */
const DRIFT_S = SLIDE_S + 1.6;
const END_S = 3.5;
const FPS = 30;

const SIZES: Record<
  VideoQuality,
  { width: number; height: number; bitrate: number }
> = {
  high: { width: 1920, height: 1080, bitrate: 8_000_000 },
  small: { width: 1280, height: 720, bitrate: 3_500_000 },
};

/** The player's six drifts: scale and shift (in fractions of the frame) at the start and end. */
const DRIFTS: [number, number, number, number, number, number][] = [
  [1.02, 0, 0, 1.14, -0.02, -0.015],
  [1.14, 0.02, 0.01, 1.03, 0, 0],
  [1.06, -0.02, 0, 1.12, 0.02, -0.01],
  [1.03, 0.015, -0.015, 1.15, -0.01, 0.015],
  [1.12, 0, 0.02, 1.04, 0, -0.01],
  [1.04, 0.02, 0.015, 1.13, -0.02, -0.01],
];

/**
 * Safari's AAC encoder describes its sound with a whole MPEG-4 descriptor
 * (03 … 04 … 05 len config), where the MP4 file wants only the config inside
 * (for AAC-LC at 48 kHz in stereo, "11 90"): nested twice, the Mac and
 * phones refuse to play the sound. Chrome already gives the config alone.
 */
function plainAudioConfig(
  meta: EncodedAudioChunkMetadata | undefined,
): EncodedAudioChunkMetadata | undefined {
  const description = meta?.decoderConfig?.description;
  if (!meta?.decoderConfig || !description) return meta;
  const bytes = ArrayBuffer.isView(description)
    ? new Uint8Array(
        description.buffer,
        description.byteOffset,
        description.byteLength,
      )
    : new Uint8Array(description);
  if (bytes[0] !== 0x03) return meta;
  // Each descriptor: a tag, a length in 7-bit pieces, then its content.
  const config = (from: number): Uint8Array | null => {
    let at = from;
    while (at < bytes.length) {
      const tag = bytes[at++];
      let length = 0;
      for (let i = 0; i < 4; i++) {
        const byte = bytes[at++];
        length = (length << 7) | (byte & 0x7f);
        if (!(byte & 0x80)) break;
      }
      if (tag === 0x05) return bytes.slice(at, at + length);
      // Into the ES descriptor (after its 3 bytes of id and flags) and the
      // decoder config (after its 13 bytes of fields); past anything else.
      if (tag === 0x03) at += 3;
      else if (tag === 0x04) at += 13;
      else at += length;
    }
    return null;
  };
  const found = config(0);
  return found
    ? { ...meta, decoderConfig: { ...meta.decoderConfig, description: found } }
    : meta;
}

/** Whether this computer can make memory videos. */
export async function canMakeVideos(): Promise<boolean> {
  if (
    typeof VideoEncoder === "undefined" ||
    typeof AudioEncoder === "undefined"
  )
    return false;
  try {
    const video = await VideoEncoder.isConfigSupported(videoConfig("high"));
    return Boolean(video.supported) && (await audioCodec()) !== null;
  } catch {
    return false;
  }
}

function videoConfig(quality: VideoQuality): VideoEncoderConfig {
  const { width, height, bitrate } = SIZES[quality];
  // H.264 High profile, level 4.0: up to 1080p at 30 frames a second.
  return {
    codec: "avc1.640028",
    width,
    height,
    bitrate,
    framerate: FPS,
    latencyMode: "quality",
  };
}

/** AAC where the web view has it (every phone plays it), else Opus. */
async function audioCodec(): Promise<"aac" | "opus" | null> {
  for (const [codec, name] of [
    ["mp4a.40.2", "aac"],
    ["opus", "opus"],
  ] as const) {
    try {
      const { supported } = await AudioEncoder.isConfigSupported({
        codec,
        sampleRate: 48_000,
        numberOfChannels: 2,
        bitrate: 160_000,
      });
      if (supported) return name;
    } catch {
      // Not this one.
    }
  }
  return null;
}

/**
 * Loads a photo full size (drawable on a canvas), or its preview if that
 * fails, shrunk once to at most `longest` pixels: drawing a 24-megapixel
 * photo for every frame would be slow.
 */
async function loadImage(
  fingerprint: string,
  longest: number,
): Promise<HTMLCanvasElement> {
  for (const src of [photoUrl(fingerprint), thumbUrl(fingerprint, "preview")]) {
    // fetch + createImageBitmap rather than an <img>: WebKit may never
    // finish decoding an image in a window that isn't shown.
    let bitmap: ImageBitmap;
    try {
      bitmap = await withinSeconds(
        20,
        fetch(src).then(async (response) => {
          if (!response.ok) throw new Error(String(response.status));
          return createImageBitmap(await response.blob());
        }),
      );
    } catch {
      continue; // try the preview
    }
    const scale = Math.min(1, longest / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    const context = canvas.getContext("2d")!;
    context.imageSmoothingQuality = "high";
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    return canvas;
  }
  throw new Error(`Could not load ${fingerprint}`);
}

/** `promise`, or a failure after `seconds`. */
function withinSeconds<T>(seconds: number, promise: Promise<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("Too slow")),
      seconds * 1000,
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/** Waits for the next task, without the slowing down timers get in a window that isn't in front. */
function nextTask(): Promise<void> {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = () => resolve();
    channel.port2.postMessage(null);
  });
}

// The canvas's own `filter` would do the blurs and the light adjustments,
// but WebKit accepts it and draws without it: they are done by hand here.

/** A small blurred copy of a picture, `across` pixels wide: drawn large, it stays smooth. */
function blurred(
  image: CanvasImageSource,
  width: number,
  height: number,
  across: number,
  radius: number,
  brightness = 1,
): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = across;
  canvas.height = Math.max(1, Math.round((across * height) / width));
  const context = canvas.getContext("2d")!;
  context.imageSmoothingQuality = "high";
  drawCover(context, image, canvas.width, canvas.height);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  // Three box blurs one after the other look like a Gaussian blur.
  for (let pass = 0; pass < 3; pass++) {
    boxBlur(pixels.data, canvas.width, canvas.height, radius, 1, canvas.width);
    boxBlur(pixels.data, canvas.height, canvas.width, radius, canvas.width, 1);
  }
  context.putImageData(pixels, 0, 0);
  if (brightness < 1) darken(context, canvas.width, canvas.height, brightness);
  return canvas;
}

/**
 * Blurs the RGBA pixels along one direction: `lines` lines of `length`
 * pixels, `step` pixels apart along a line and `lineStep` between lines.
 */
function boxBlur(
  data: Uint8ClampedArray,
  length: number,
  lines: number,
  radius: number,
  step: number,
  lineStep: number,
) {
  const line = new Float32Array(length * 3);
  for (let l = 0; l < lines; l++) {
    for (let i = 0; i < length; i++) {
      const at = (l * lineStep + i * step) * 4;
      line[i * 3] = data[at];
      line[i * 3 + 1] = data[at + 1];
      line[i * 3 + 2] = data[at + 2];
    }
    for (let i = 0; i < length; i++) {
      let r = 0;
      let g = 0;
      let b = 0;
      let count = 0;
      for (
        let k = Math.max(0, i - radius);
        k <= Math.min(length - 1, i + radius);
        k++
      ) {
        r += line[k * 3];
        g += line[k * 3 + 1];
        b += line[k * 3 + 2];
        count++;
      }
      const at = (l * lineStep + i * step) * 4;
      data[at] = r / count;
      data[at + 1] = g / count;
      data[at + 2] = b / count;
    }
  }
}

/** Applies the edit's brightness, contrast and saturation to the pixels, as `filterCss` does in the app. */
function applyLight(canvas: HTMLCanvasElement, edit: Edit) {
  if (edit.brightness === 0 && edit.contrast === 0 && edit.saturation === 0)
    return;
  const brightness = 1 + edit.brightness / 200;
  const contrast = 1 + edit.contrast / 200;
  const s = 1 + edit.saturation / 100;
  const context = canvas.getContext("2d")!;
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  const data = pixels.data;
  for (let at = 0; at < data.length; at += 4) {
    // brightness(), then contrast(), then saturate(), as CSS defines them.
    let r = data[at] * brightness;
    let g = data[at + 1] * brightness;
    let b = data[at + 2] * brightness;
    r = (r - 127.5) * contrast + 127.5;
    g = (g - 127.5) * contrast + 127.5;
    b = (b - 127.5) * contrast + 127.5;
    data[at] =
      (0.213 + 0.787 * s) * r +
      (0.715 - 0.715 * s) * g +
      (0.072 - 0.072 * s) * b;
    data[at + 1] =
      (0.213 - 0.213 * s) * r +
      (0.715 + 0.285 * s) * g +
      (0.072 - 0.072 * s) * b;
    data[at + 2] =
      (0.213 - 0.213 * s) * r +
      (0.715 - 0.715 * s) * g +
      (0.072 + 0.928 * s) * b;
  }
  context.putImageData(pixels, 0, 0);
}

/** Darkens what was just drawn, as CSS `brightness(level)` would. */
function darken(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  level: number,
) {
  context.fillStyle = `rgba(0, 0, 0, ${(1 - level).toFixed(3)})`;
  context.fillRect(0, 0, width, height);
}

function sizeOf(image: CanvasImageSource): [number, number] {
  if (image instanceof HTMLImageElement)
    return [image.naturalWidth, image.naturalHeight];
  if (image instanceof HTMLCanvasElement) return [image.width, image.height];
  return [1, 1];
}

/** Draws an unedited picture filling the box. */
function drawCover(
  context: CanvasRenderingContext2D,
  image: CanvasImageSource,
  width: number,
  height: number,
) {
  const [imageWidth, imageHeight] = sizeOf(image);
  const scale = Math.max(width / imageWidth, height / imageHeight);
  context.drawImage(
    image,
    (width - imageWidth * scale) / 2,
    (height - imageHeight * scale) / 2,
    imageWidth * scale,
    imageHeight * scale,
  );
}

/** The size of the kept part of an edited picture: turned, then cropped. */
function keptSize(edit: Edit, width: number, height: number): [number, number] {
  const sideways = edit.rotate % 180 !== 0;
  const [turnedWidth, turnedHeight] = sideways
    ? [height, width]
    : [width, height];
  const [, , cropWidth, cropHeight] = (edit.crop ?? [0, 0, 10_000, 10_000]).map(
    (value) => value / 10_000,
  );
  return [turnedWidth * cropWidth, turnedHeight * cropHeight];
}

/** Upright photos (and very wide ones) are shown whole, on a blurred copy (as in the player). */
function shownWhole(edit: Edit, width: number, height: number): boolean {
  const [keptWidth, keptHeight] = keptSize(edit, width, height);
  const ratio = keptWidth / Math.max(1, keptHeight);
  return ratio < 1.1 || ratio > 2.4;
}

/**
 * Draws an edited photo (turned, mirrored, cropped, lightened…) in the box
 * `width` x `height`, filling it ("cover") or fitting inside ("contain").
 */
function drawEdited(
  context: CanvasRenderingContext2D,
  image: HTMLCanvasElement,
  edit: Edit,
  width: number,
  height: number,
  fit: "cover" | "contain",
) {
  const imageWidth = image.width;
  const imageHeight = image.height;
  const sideways = edit.rotate % 180 !== 0;
  const [turnedWidth, turnedHeight] = sideways
    ? [imageHeight, imageWidth]
    : [imageWidth, imageHeight];
  const [left, top, cropWidth, cropHeight] = (
    edit.crop ?? [0, 0, 10_000, 10_000]
  ).map((value) => value / 10_000);
  const keptWidth = turnedWidth * cropWidth;
  const keptHeight = turnedHeight * cropHeight;
  const scales = [width / keptWidth, height / keptHeight];
  const scale = fit === "cover" ? Math.max(...scales) : Math.min(...scales);
  const clipWidth = Math.min(width, keptWidth * scale);
  const clipHeight = Math.min(height, keptHeight * scale);
  // How far the middle of the kept part is from the middle of the picture.
  const shiftX = (left + cropWidth / 2 - 0.5) * turnedWidth * scale;
  const shiftY = (top + cropHeight / 2 - 0.5) * turnedHeight * scale;

  context.save();
  context.beginPath();
  context.rect(
    (width - clipWidth) / 2,
    (height - clipHeight) / 2,
    clipWidth,
    clipHeight,
  );
  context.clip();
  context.translate(width / 2 - shiftX, height / 2 - shiftY);
  context.rotate((edit.rotate * Math.PI) / 180);
  if (edit.flip) context.scale(-1, 1);
  context.drawImage(
    image,
    (-imageWidth * scale) / 2,
    (-imageHeight * scale) / 2,
    imageWidth * scale,
    imageHeight * scale,
  );
  context.restore();
}

const easeOut = (x: number) => 1 - (1 - Math.min(1, Math.max(0, x))) ** 3;
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
/** Slow, then faster, then slow again. */
const smooth = (x: number) => {
  const t = clamp01(x);
  return t * t * (3 - 2 * t);
};

interface Slide {
  photo: PhotoItem;
  image: HTMLCanvasElement;
  edit: Edit;
  whole: boolean;
  backdrop: HTMLCanvasElement | null;
  drift: (typeof DRIFTS)[number];
}

/** Makes the memory's video and returns the MP4 file's bytes. */
export async function makeMemoryVideo(
  request: VideoRequest,
): Promise<Uint8Array> {
  const { photos, quality, signal } = request;
  const { width, height } = SIZES[quality];
  const audio = await audioCodec();
  if (!audio) throw new Error("This computer can't encode sound for videos.");

  const total = TITLE_S + photos.length * SLIDE_S + END_S;
  const frames = Math.ceil(total * FPS);

  // The music comes first: it is quick, and its share of the progress small.
  const music = await renderMusic(request.memoryId, total);
  signal.throwIfAborted();

  const target = new ArrayBufferTarget();
  const muxer = new Muxer({
    target,
    video: { codec: "avc", width, height, frameRate: FPS },
    audio: { codec: audio, numberOfChannels: 2, sampleRate: 48_000 },
    fastStart: "in-memory",
    firstTimestampBehavior: "offset",
  });
  let failure: Error | null = null;
  const videoEncoder = new VideoEncoder({
    output: (chunk, meta) => muxer.addVideoChunk(chunk, meta),
    error: (e) => (failure = e),
  });
  videoEncoder.configure(videoConfig(quality));
  const audioEncoder = new AudioEncoder({
    output: (chunk, meta) => muxer.addAudioChunk(chunk, plainAudioConfig(meta)),
    error: (e) => (failure = e),
  });
  audioEncoder.configure({
    codec: audio === "aac" ? "mp4a.40.2" : "opus",
    sampleRate: 48_000,
    numberOfChannels: 2,
    bitrate: 160_000,
  });

  try {
    // The sound, in pieces of a tenth of a second.
    const left = music.getChannelData(0);
    const right = music.getChannelData(1);
    const PIECE = 4800;
    for (let start = 0; start < music.length; start += PIECE) {
      const length = Math.min(PIECE, music.length - start);
      const data = new Float32Array(length * 2);
      data.set(left.subarray(start, start + length), 0);
      data.set(right.subarray(start, start + length), length);
      const piece = new AudioData({
        format: "f32-planar",
        sampleRate: 48_000,
        numberOfFrames: length,
        numberOfChannels: 2,
        timestamp: Math.round((start / 48_000) * 1e6),
        data,
      });
      audioEncoder.encode(piece);
      piece.close();
    }

    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d", { alpha: false })!;
    context.imageSmoothingQuality = "high";

    // The cover, sharp and in two blurs, for the focus at the start and the end card.
    // Big enough for the drift's zoom, and no bigger.
    const longest = Math.round(Math.max(width, height) * 1.4);
    const coverImage = await loadImage(request.cover, longest);
    const coverSoft = blurred(coverImage, width, height, 320, 2);
    const coverBlur = blurred(coverImage, width, height, 96, 4);
    const coverEnd = blurred(coverImage, width, height, 96, 4, 0.55);

    // Slides load one ahead of time and are let go once shown.
    // A photo that can't be read (gone from the disk) is left out: the one
    // before stays a little longer instead.
    const slides = new Map<number, Promise<Slide | null>>();
    const slide = (index: number): Promise<Slide | null> => {
      let known = slides.get(index);
      if (!known) {
        const photo = photos[index];
        known = loadImage(photo.fingerprint, longest).then(
          (image) => {
            const edit = photo.edit ?? NO_EDIT;
            applyLight(image, edit);
            const whole = shownWhole(edit, image.width, image.height);
            return {
              photo,
              image,
              edit,
              whole,
              backdrop: whole
                ? blurred(image, width, height, 96, 3, 0.5)
                : null,
              drift:
                DRIFTS[(index * 7 + request.memoryId.length) % DRIFTS.length],
            };
          },
          () => null,
        );
        slides.set(index, known);
      }
      return known;
    };

    const drawSlide = (shown: Slide, since: number, alpha: number) => {
      context.save();
      context.globalAlpha = alpha;
      if (shown.backdrop) {
        // As the player's backdrop: blurred, darker, a little zoomed.
        context.save();
        context.translate(width / 2, height / 2);
        context.scale(1.2, 1.2);
        context.translate(-width / 2, -height / 2);
        drawCover(context, shown.backdrop, width, height);
        context.restore();
      }
      const [fromScale, fromX, fromY, toScale, toX, toY] = shown.drift;
      const progress = clamp01(since / DRIFT_S);
      const scale = fromScale + (toScale - fromScale) * progress;
      const x = fromX + (toX - fromX) * progress;
      const y = fromY + (toY - fromY) * progress;
      // As the CSS `scale() translate()` of the player, around the middle.
      context.translate(width / 2, height / 2);
      context.scale(scale, scale);
      context.translate(x * width - width / 2, y * height - height / 2);
      drawEdited(
        context,
        shown.image,
        shown.edit,
        width,
        height,
        shown.whole ? "contain" : "cover",
      );
      context.restore();
    };

    const drawText = (opacity: number, rise: number, middle: boolean) => {
      context.save();
      context.globalAlpha = opacity;
      context.fillStyle = "#fff";
      context.textAlign = "center";
      context.shadowColor = "rgba(0, 0, 0, 0.55)";
      context.shadowBlur = height * 0.017;
      context.shadowOffsetY = height * 0.002;
      const titleSize = Math.round(height * 0.075);
      const subtitleSize = Math.round(height * 0.02);
      const font = `-apple-system, "SF Pro Display", "Segoe UI", system-ui, sans-serif`;
      // As the player: the title at the bottom, or in the middle on the end card.
      const titleBaseline = middle
        ? height * 0.5
        : height * 0.88 - subtitleSize * 2.2;
      context.translate(0, rise);
      context.font = `700 ${titleSize}px ${font}`;
      context.fillText(request.title, width / 2, titleBaseline, width * 0.88);
      context.font = `600 ${subtitleSize}px ${font}`;
      if ("letterSpacing" in context)
        context.letterSpacing = `${(subtitleSize * 0.16).toFixed(1)}px`;
      context.fillText(
        request.subtitle.toLocaleUpperCase(),
        width / 2,
        titleBaseline + subtitleSize * 2.2,
        width * 0.88,
      );
      context.restore();
    };

    const drawTitle = (time: number) => {
      // The cover comes into focus: from the strong blur through the soft
      // one to sharp, zooming out a little, in 3.2 seconds.
      const focus = smooth(time / 3.2);
      const scale = 1.12 - 0.1 * focus;
      context.save();
      context.translate(width / 2, height / 2);
      context.scale(scale, scale);
      context.translate(-width / 2, -height / 2);
      drawCover(context, coverBlur, width, height);
      context.globalAlpha = clamp01(focus * 2);
      drawCover(context, coverSoft, width, height);
      context.globalAlpha = clamp01(focus * 2 - 1);
      drawCover(context, coverImage, width, height);
      context.globalAlpha = 1;
      context.restore();
      darken(context, width, height, 0.8 + 0.12 * focus);
      const rise = easeOut((time - 0.5) / 1.6);
      drawText(rise, (1 - rise) * height * 0.015, false);
    };

    const drawEnd = (time: number, alpha: number) => {
      context.save();
      context.globalAlpha = alpha;
      context.translate(width / 2, height / 2);
      context.scale(1.1, 1.1);
      context.translate(-width / 2, -height / 2);
      drawCover(context, coverEnd, width, height);
      context.restore();
      drawText(alpha * easeOut(time / 1.2), 0, true);
    };

    const slidesStart = TITLE_S;
    const endStart = TITLE_S + photos.length * SLIDE_S;
    for (let frame = 0; frame < frames; frame++) {
      signal.throwIfAborted();
      if (failure) throw failure;
      const time = frame / FPS;
      context.fillStyle = "#000";
      context.fillRect(0, 0, width, height);

      if (time < slidesStart + FADE_S) drawTitle(time);
      const index = Math.floor((time - slidesStart) / SLIDE_S);
      // The photo before still shows while this one fades in.
      for (const shownIndex of [index - 1, index]) {
        if (shownIndex < 0 || shownIndex >= photos.length) continue;
        const since = time - (slidesStart + shownIndex * SLIDE_S);
        if (since > SLIDE_S + FADE_S) continue;
        const shown = await slide(shownIndex);
        if (shown) drawSlide(shown, since, clamp01(since / FADE_S));
      }
      if (time >= endStart)
        drawEnd(time - endStart, clamp01((time - endStart) / FADE_S));

      // Let go of the photos already shown; load the next one.
      if (index >= 2) slides.delete(index - 2);
      if (index + 1 < photos.length && index + 1 >= 0) void slide(index + 1);

      const videoFrame = new VideoFrame(canvas, {
        timestamp: Math.round((frame * 1e6) / FPS),
        duration: Math.round(1e6 / FPS),
      });
      videoEncoder.encode(videoFrame, { keyFrame: frame % (FPS * 2) === 0 });
      videoFrame.close();
      // Don't race ahead of the encoder. (Its "dequeue" event would be
      // neater, but WebKit has the property and never fires it.)
      while (videoEncoder.encodeQueueSize > 8) await nextTask();
      // Let the window show the progress now and then.
      if (frame % 10 === 0) await nextTask();
      if (frame % 15 === 0) request.onprogress(frame / frames);
    }

    await videoEncoder.flush();
    await audioEncoder.flush();
    if (failure) throw failure;
    muxer.finalize();
    request.onprogress(1);
    return new Uint8Array(target.buffer);
  } finally {
    if (videoEncoder.state !== "closed") videoEncoder.close();
    if (audioEncoder.state !== "closed") audioEncoder.close();
  }
}
