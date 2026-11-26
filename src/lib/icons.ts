// Line icons of the sidebar rows, drawn on a 20 × 20 grid with a 1.5 px
// stroke and no fill (see `.row-icon` in LibraryView.svelte).

export type Icon =
  | "library"
  | "heart"
  | "hidden"
  | "map"
  | "person"
  | "duplicates"
  | "similar"
  | "album"
  | "folder"
  | "video"
  | "raw"
  | "grid"
  | "sliders"
  | "photo"
  | "albumOff";

/** The paths of each icon. */
export const ICONS: Record<Icon, string[]> = {
  // A photo with a mountain and a sun, another one behind it.
  library: [
    "M5.5 3.5h10A1.5 1.5 0 0 1 17 5v8.5",
    "M4 6h9.5A1.5 1.5 0 0 1 15 7.5v8a1.5 1.5 0 0 1-1.5 1.5H4a1.5 1.5 0 0 1-1.5-1.5v-8A1.5 1.5 0 0 1 4 6z",
    "M2.5 14.5l3.5-3.5 3 3 1.8-1.8 4.2 4.2",
    "M12.5 9.5a1 1 0 1 1-2 0a1 1 0 1 1 2 0",
  ],
  heart: ["M10 16.5S3 12.3 3 7.7a3.6 3.6 0 0 1 7-1.6 3.6 3.6 0 0 1 7 1.6c0 4.6-7 8.8-7 8.8z"],
  // An eye, struck through.
  hidden: [
    "M2.5 10S5.2 5 10 5s7.5 5 7.5 5-2.7 5-7.5 5-7.5-5-7.5-5z",
    "M12.2 10a2.2 2.2 0 1 1-4.4 0a2.2 2.2 0 1 1 4.4 0",
    "M3.5 3.5l13 13",
  ],
  // A folded map.
  map: ["M2.5 5.5l5-2 5 2 5-2v11l-5 2-5-2-5 2z", "M7.5 3.5v11", "M12.5 5.5v11"],
  person: [
    "M17.5 10a7.5 7.5 0 1 1-15 0a7.5 7.5 0 1 1 15 0",
    "M12.5 8a2.5 2.5 0 1 1-5 0a2.5 2.5 0 1 1 5 0",
    "M5 15.6c1.2-1.9 3-2.9 5-2.9s3.8 1 5 2.9",
  ],
  // Two copies of a picture.
  duplicates: [
    "M8 7h7.5A1.5 1.5 0 0 1 17 8.5V16a1.5 1.5 0 0 1-1.5 1.5H8A1.5 1.5 0 0 1 6.5 16V8.5A1.5 1.5 0 0 1 8 7z",
    "M3.5 12.5V4.5A1.5 1.5 0 0 1 5 3h8",
  ],
  // Two overlapping circles: alike, not the same.
  similar: ["M12 10a4.2 4.2 0 1 1-8.4 0a4.2 4.2 0 1 1 8.4 0", "M16.4 10a4.2 4.2 0 1 1-8.4 0a4.2 4.2 0 1 1 8.4 0"],
  // A stack of pictures.
  album: [
    "M4.5 8h11A1.5 1.5 0 0 1 17 9.5v6a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 15.5v-6A1.5 1.5 0 0 1 4.5 8z",
    "M5 5.5h10",
    "M7 3h6",
  ],
  folder: [
    "M2.5 6.5v8.5A1.5 1.5 0 0 0 4 16.5h12a1.5 1.5 0 0 0 1.5-1.5V8A1.5 1.5 0 0 0 16 6.5H9.5L8 4.5H4A1.5 1.5 0 0 0 2.5 6z",
  ],
  // A video camera.
  video: [
    "M4 5.5h7.5A1.5 1.5 0 0 1 13 7v6a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 13V7A1.5 1.5 0 0 1 4 5.5z",
    "M13 8.5l4.5-2.5v8L13 11.5",
  ],
  // An R in a frame.
  raw: [
    "M4.5 3h11A1.5 1.5 0 0 1 17 4.5v11a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 15.5v-11A1.5 1.5 0 0 1 4.5 3z",
    "M8 14V6h2.8a2.2 2.2 0 0 1 0 4.4H8",
    "M10.6 10.4 13 14",
  ],
  // Nine squares: everything.
  grid: [
    "M3.5 3.5h3v3h-3z",
    "M8.5 3.5h3v3h-3z",
    "M13.5 3.5h3v3h-3z",
    "M3.5 8.5h3v3h-3z",
    "M8.5 8.5h3v3h-3z",
    "M13.5 8.5h3v3h-3z",
    "M3.5 13.5h3v3h-3z",
    "M8.5 13.5h3v3h-3z",
    "M13.5 13.5h3v3h-3z",
  ],
  // Three sliders: edited.
  sliders: ["M3 5.5h14", "M3 10h14", "M3 14.5h14", "M12.5 4v3", "M7 8.5v3", "M11 13v3"],
  // A single picture.
  photo: [
    "M4 4.5h12A1.5 1.5 0 0 1 17.5 6v8a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 14V6A1.5 1.5 0 0 1 4 4.5z",
    "M2.5 13l4-4 3.5 3.5 2-2 5.5 5",
    "M14.5 8a1 1 0 1 1-2 0a1 1 0 1 1 2 0",
  ],
  // The album stack, struck through.
  albumOff: [
    "M4.5 8h11A1.5 1.5 0 0 1 17 9.5v6a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 15.5v-6A1.5 1.5 0 0 1 4.5 8z",
    "M5 5.5h10",
    "M7 3h6",
    "M3.5 3.5l13 13",
  ],
};
