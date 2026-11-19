// Star ratings and colour labels, shared through the library like
// favorites. Mirrors `LABELS` in src-tauri/src/org.rs.

export type LabelColor =
  "red" | "orange" | "yellow" | "green" | "blue" | "purple";

export const LABELS: {
  color: LabelColor;
  name: string;
  css: string;
  key?: string;
}[] = [
  { color: "red", name: "Red", css: "#ef4444", key: "6" },
  { color: "orange", name: "Orange", css: "#f97316" },
  { color: "yellow", name: "Yellow", css: "#eab308", key: "7" },
  { color: "green", name: "Green", css: "#22c55e", key: "8" },
  { color: "blue", name: "Blue", css: "#3b82f6", key: "9" },
  { color: "purple", name: "Purple", css: "#a855f7" },
];

export const labelCss = (color: string | null) =>
  LABELS.find((label) => label.color === color)?.css ?? null;

/** The label a key (6 to 9) gives, as in Lightroom. */
export const labelForKey = (key: string) =>
  LABELS.find((label) => label.key === key)?.color ?? null;

/** "★★★☆☆" for 3. */
export const starText = (stars: number) =>
  "★".repeat(stars) + "☆".repeat(5 - stars);
