// Edits change how a photo is shown, never its file: rotation, mirror, crop
// and light/color adjustments are applied with CSS when the photo is drawn.
// Mirrors `Edit` in src-tauri/src/org.rs.

export interface Edit {
  /** Clockwise rotation in degrees: 0, 90, 180 or 270. */
  rotate: number;
  /** Mirrored left to right (before the rotation). */
  flip: boolean;
  /**
   * The part to keep: left, top, width, height in ten-thousandths of the
   * rotated picture. `null` keeps all of it.
   */
  crop: [number, number, number, number] | null;
  /** From -100 to 100; 0 changes nothing. */
  brightness: number;
  contrast: number;
  saturation: number;
}

export const NO_EDIT: Edit = {
  rotate: 0,
  flip: false,
  crop: null,
  brightness: 0,
  contrast: 0,
  saturation: 0,
};

/** True if the edit changes nothing, so it need not be stored. */
export function isNeutral(edit: Edit): boolean {
  return (
    edit.rotate === 0 &&
    !edit.flip &&
    edit.crop === null &&
    edit.brightness === 0 &&
    edit.contrast === 0 &&
    edit.saturation === 0
  );
}

/** The CSS `filter` for the light and color adjustments. */
export function filterCss(edit: Edit): string {
  const parts: string[] = [];
  // -100..100 maps to half..one and a half times the normal value;
  // saturation goes all the way down to black and white.
  if (edit.brightness !== 0) parts.push(`brightness(${1 + edit.brightness / 200})`);
  if (edit.contrast !== 0) parts.push(`contrast(${1 + edit.contrast / 200})`);
  if (edit.saturation !== 0) parts.push(`saturate(${1 + edit.saturation / 100})`);
  return parts.join(" ");
}

/** The size of the picture after rotation and cropping. */
export function editedSize(edit: Edit, width: number, height: number): [number, number] {
  const sideways = edit.rotate % 180 !== 0;
  const [rotatedWidth, rotatedHeight] = sideways ? [height, width] : [width, height];
  const [, , cropWidth, cropHeight] = edit.crop ?? [0, 0, 10_000, 10_000];
  return [(rotatedWidth * cropWidth) / 10_000, (rotatedHeight * cropHeight) / 10_000];
}

export interface Placement {
  /** Size of the window the kept part shows through, centered in the box. */
  clipWidth: number;
  clipHeight: number;
  /** Size to draw the whole (unrotated) image at. */
  imageWidth: number;
  imageHeight: number;
  /** CSS transform for the image, which is centered in the window. */
  transform: string;
}

/**
 * Works out how to draw an edited photo of `width` x `height` pixels in a
 * box: "cover" fills the box (grid cells), "contain" fits inside (viewer).
 */
export function place(
  edit: Edit,
  width: number,
  height: number,
  boxWidth: number,
  boxHeight: number,
  fit: "cover" | "contain",
): Placement {
  const sideways = edit.rotate % 180 !== 0;
  const [rotatedWidth, rotatedHeight] = sideways ? [height, width] : [width, height];
  const [left, top, cropWidth, cropHeight] = (edit.crop ?? [0, 0, 10_000, 10_000]).map(
    (value) => value / 10_000,
  );
  const keptWidth = rotatedWidth * cropWidth;
  const keptHeight = rotatedHeight * cropHeight;
  const scales = [boxWidth / keptWidth, boxHeight / keptHeight];
  const scale = fit === "cover" ? Math.max(...scales) : Math.min(...scales);

  // How far the centre of the kept part is from the centre of the picture.
  const shiftX = (left + cropWidth / 2 - 0.5) * rotatedWidth * scale;
  const shiftY = (top + cropHeight / 2 - 0.5) * rotatedHeight * scale;
  return {
    clipWidth: Math.min(boxWidth, keptWidth * scale),
    clipHeight: Math.min(boxHeight, keptHeight * scale),
    imageWidth: width * scale,
    imageHeight: height * scale,
    // Applied right to left: mirror, rotate, then move the kept part to the middle.
    transform:
      `translate(-50%, -50%) translate(${-shiftX}px, ${-shiftY}px) ` +
      `rotate(${edit.rotate}deg)${edit.flip ? " scaleX(-1)" : ""}`,
  };
}
