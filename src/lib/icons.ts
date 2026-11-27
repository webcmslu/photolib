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
  | "albumOff"
  | "selfie"
  | "live"
  | "portrait"
  | "panorama"
  | "timeLapse"
  | "sloMo"
  | "burst"
  | "screenshot"
  | "collections"
  | "memories";

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
  // A phone with a person on its screen.
  selfie: [
    "M6 2.5h8A1.5 1.5 0 0 1 15.5 4v12a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 16V4A1.5 1.5 0 0 1 6 2.5z",
    "M11.6 8.5a1.6 1.6 0 1 1-3.2 0a1.6 1.6 0 1 1 3.2 0",
    "M7 13.5c.7-1.3 1.8-2 3-2s2.3.7 3 2",
  ],
  // Rings: a picture that moves.
  live: [
    "M17.5 10a7.5 7.5 0 1 1-15 0a7.5 7.5 0 1 1 15 0",
    "M14.5 10a4.5 4.5 0 1 1-9 0a4.5 4.5 0 1 1 9 0",
    "M11.5 10a1.5 1.5 0 1 1-3 0a1.5 1.5 0 1 1 3 0",
  ],
  // A cube: a picture with depth.
  portrait: [
    "M10 2.5l6.5 3.75v7.5L10 17.5l-6.5-3.75v-7.5z",
    "M3.5 6.25 10 10l6.5-3.75",
    "M10 10v7.5",
  ],
  // A wide, curved picture.
  panorama: [
    "M2.5 5.5c5-1.5 10-1.5 15 0v9c-5-1.5-10-1.5-15 0z",
    "M5 12.3l3-2.6 2.5 2.1 2-1.6 2.5 2.1",
  ],
  // A dashed circle: time going fast.
  timeLapse: [
    "M8.31 3.21A7 7 0 0 1 11.69 3.21",
    "M13.61 4.0A7 7 0 0 1 16.0 6.39",
    "M16.79 8.31A7 7 0 0 1 16.79 11.69",
    "M16.0 13.61A7 7 0 0 1 13.61 16.0",
    "M11.69 16.79A7 7 0 0 1 8.31 16.79",
    "M6.39 16.0A7 7 0 0 1 4.0 13.61",
    "M3.21 11.69A7 7 0 0 1 3.21 8.31",
    "M4.0 6.39A7 7 0 0 1 6.39 4.0",
  ],
  // A circle of short ticks: time going slow.
  sloMo: [
    "M10.0 4.5L10.0 2.5",
    "M12.1 4.92L12.87 3.07",
    "M13.89 6.11L15.3 4.7",
    "M15.08 7.9L16.93 7.13",
    "M15.5 10.0L17.5 10.0",
    "M15.08 12.1L16.93 12.87",
    "M13.89 13.89L15.3 15.3",
    "M12.1 15.08L12.87 16.93",
    "M10.0 15.5L10.0 17.5",
    "M7.9 15.08L7.13 16.93",
    "M6.11 13.89L4.7 15.3",
    "M4.92 12.1L3.07 12.87",
    "M4.5 10.0L2.5 10.0",
    "M4.92 7.9L3.07 7.13",
    "M6.11 6.11L4.7 4.7",
    "M7.9 4.92L7.13 3.07",
  ],
  // A stack of three pictures.
  burst: [
    "M8 7h7.5A1.5 1.5 0 0 1 17 8.5V16a1.5 1.5 0 0 1-1.5 1.5H8A1.5 1.5 0 0 1 6.5 16V8.5A1.5 1.5 0 0 1 8 7z",
    "M4.5 15V6.5A1.5 1.5 0 0 1 6 5h9",
    "M2.5 13V4.5A1.5 1.5 0 0 1 4 3h9",
  ],
  // The corners of a screen.
  screenshot: [
    "M3 7V4.5A1.5 1.5 0 0 1 4.5 3H7",
    "M13 3h2.5A1.5 1.5 0 0 1 17 4.5V7",
    "M17 13v2.5a1.5 1.5 0 0 1-1.5 1.5H13",
    "M7 17H4.5A1.5 1.5 0 0 1 3 15.5V13",
  ],
  // Four tiles of different sizes, as in Apple Photos' Collections.
  collections: [
    "M4.5 3h4A1.5 1.5 0 0 1 10 4.5v6A1.5 1.5 0 0 1 8.5 12h-4A1.5 1.5 0 0 1 3 10.5v-6A1.5 1.5 0 0 1 4.5 3z",
    "M13.5 3h2A1.5 1.5 0 0 1 17 4.5v2A1.5 1.5 0 0 1 15.5 8h-2A1.5 1.5 0 0 1 12 6.5v-2A1.5 1.5 0 0 1 13.5 3z",
    "M13.5 10h2a1.5 1.5 0 0 1 1.5 1.5v4a1.5 1.5 0 0 1-1.5 1.5h-2a1.5 1.5 0 0 1-1.5-1.5v-4a1.5 1.5 0 0 1 1.5-1.5z",
    "M4.5 14h4a1.5 1.5 0 0 1 1.5 1.5v0A1.5 1.5 0 0 1 8.5 17h-4A1.5 1.5 0 0 1 3 15.5v0A1.5 1.5 0 0 1 4.5 14z",
  ],
  // A photo with a play button: a memory plays.
  memories: [
    "M4.5 3.5h11A1.5 1.5 0 0 1 17 5v10a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 15V5a1.5 1.5 0 0 1 1.5-1.5z",
    "M8.5 7.2v5.6l4.6-2.8z",
  ],
};
