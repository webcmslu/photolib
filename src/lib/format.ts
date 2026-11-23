// Formatting helpers. Photo dates are wall-clock times encoded as UTC (see
// api.ts), hence `timeZone: "UTC"` everywhere.

import { locale, t, tn } from "./i18n";

const DAY_MS = 86_400_000;

const numberFormat = new Intl.NumberFormat(locale);
const dayFormat = new Intl.DateTimeFormat(locale, {
  weekday: "long",
  day: "numeric",
  month: "long",
  timeZone: "UTC",
});
const monthFormat = new Intl.DateTimeFormat(locale, {
  month: "long",
  year: "numeric",
  timeZone: "UTC",
});
const dateTimeFormat = new Intl.DateTimeFormat(locale, {
  dateStyle: "full",
  timeStyle: "medium",
  timeZone: "UTC",
});

const shortDateFormat = new Intl.DateTimeFormat(locale, {
  dateStyle: "medium",
  timeZone: "UTC",
});

/** French and some other languages write month and day names in lower
 * case; a heading or a line still starts with a capital: "Octobre 2023". */
const capitalized = (text: string) =>
  text.charAt(0).toLocaleUpperCase(locale) + text.slice(1);

export const formatNumber = (value: number) => numberFormat.format(value);

/** "Dec 28, 2024 – Jan 1, 2025", or a single date, from two day numbers. */
export function formatDayRange(first: number, last: number): string {
  return first === last
    ? shortDateFormat.format(first * DAY_MS)
    : shortDateFormat.formatRange(first * DAY_MS, last * DAY_MS);
}

/** "Sunday, 14 July" for a day number (days since 1970-01-01). */
export const formatDay = (day: number) =>
  capitalized(dayFormat.format(day * DAY_MS));

/** "July 2024" for a day number. */
export const formatMonth = (day: number) =>
  capitalized(monthFormat.format(day * DAY_MS));

/** "0:07", "12:34" or "1:02:03" for a video length. */
export function formatDuration(ms: number): string {
  const seconds = Math.round(ms / 1000);
  const two = (value: number) => String(value).padStart(2, "0");
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  return h > 0 ? `${h}:${two(m)}:${two(s)}` : `${m}:${two(s)}`;
}

/** A number that changes exactly when the calendar month does. */
export function monthKey(day: number): number {
  const date = new Date(day * DAY_MS);
  return date.getUTCFullYear() * 12 + date.getUTCMonth();
}

export const formatDateTime = (sortDate: number) =>
  capitalized(dateTimeFormat.format(sortDate));

const oneDecimal = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });

/** "35 mm (52 mm)", "f/2.8", "1/250 s", "ISO 400", "−0.7 EV": how a photo was taken. */
export function formatShooting(shot: {
  focalLength: number | null;
  focalLength35mm: number | null;
  aperture: number | null;
  exposure: number | null;
  iso: number | null;
  exposureBias: number | null;
}): string[] {
  const parts: string[] = [];
  if (shot.focalLength !== null) {
    const mm = `${oneDecimal.format(shot.focalLength)} mm`;
    const equivalent =
      shot.focalLength35mm !== null && Math.abs(shot.focalLength35mm - shot.focalLength) >= 1
        ? ` (${t("{mm} mm in 35 mm", { mm: shot.focalLength35mm })})`
        : "";
    parts.push(mm + equivalent);
  }
  if (shot.aperture !== null) parts.push(`f/${oneDecimal.format(shot.aperture)}`);
  if (shot.exposure !== null) {
    const seconds = shot.exposure;
    // 1/250 s below a third of a second, 0.5 s, 2 s, 30 s above.
    parts.push(seconds < 0.3 ? `1/${Math.round(1 / seconds)} s` : `${oneDecimal.format(seconds)} s`);
  }
  if (shot.iso !== null) parts.push(`ISO ${shot.iso}`);
  if (shot.exposureBias !== null) {
    const ev = oneDecimal.format(Math.abs(shot.exposureBias));
    parts.push(`${shot.exposureBias < 0 ? "−" : "+"}${ev} EV`);
  }
  return parts;
}

export function formatBytes(bytes: number): string {
  const units = [t("bytes"), "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${unit === 0 ? value : numberFormat.format(Number(value.toFixed(1)))} ${units[unit]}`;
}

/** "photo" or "photos", with the count: "1 photo", "1,234 photos". */
export function photoCount(count: number): string {
  return tn(count, "{n} photo", "{n} photos");
}

/** "JPEG + RAW", "HEIC + RAW"…: names a pair after the file that is shown. */
export function rawPairLabel(relPath: string): string {
  const extension = relPath.slice(relPath.lastIndexOf(".") + 1).toUpperCase();
  return `${extension === "JPG" ? "JPEG" : extension} + RAW`;
}

/** Brands whose cameras write a company name rather than the brand. */
const BRAND_ALIASES: [RegExp, string][] = [
  [/^eastman kodak\b/i, "Kodak"],
  [/^hewlett[- ]packard\b/i, "HP"],
];

/** "Co., Ltd.", "Corporation", "Optical"…: words that aren't the brand. */
const COMPANY_WORDS =
  /[\s,]+(co\.?,?\s*ltd\.?|co\.?|ltd\.?|inc\.?|corp\.?|corporation|company|optical|computer|techwin|imaging|electronics?|digital solutions)(?=[\s,.]|$)/gi;

/** The brand a camera's "make" stands for: "OLYMPUS OPTICAL CO.,LTD" → "Olympus". */
export function cameraBrand(make: string): string {
  let brand = make.trim();
  for (const [pattern, name] of BRAND_ALIASES) if (pattern.test(brand)) return name;
  let previous;
  do {
    previous = brand;
    brand = brand.replace(COMPANY_WORDS, "").replace(/[\s,.]+$/, "");
  } while (brand !== previous);
  if (!brand) brand = make.trim();
  // "SAMSUNG" → "Samsung"; short initials like "HP" or "JVC" stay as they are.
  return brand.replace(/\b[A-Z]{4,}\b/g, (word) => word[0] + word.slice(1).toLowerCase());
}

/**
 * A camera's name as people say it: "Canon EOS R6", "Nikon D850" (not
 * "NIKON CORPORATION NIKON D850"), "Kodak EasyShare M853" (not "EASTMAN
 * KODAK COMPANY KODAK EASYSHARE M853 ZOOM DIGITAL CAMERA").
 */
export function cameraName(make: string | null | undefined, model: string | null | undefined): string {
  let name = (model ?? "").trim().replace(/\s+digital camera$/i, "");
  if (!make?.trim()) return name;
  const brand = cameraBrand(make);
  if (!name) return brand;
  // The model often repeats the brand ("KODAK EASYSHARE…", "Canon EOS 6D").
  const words = [brand, ...make.trim().split(/[\s,]+/)].filter((word) => word.length > 1);
  for (const word of words) {
    if (name.toLowerCase().startsWith(word.toLowerCase() + " ") || name.toLowerCase() === word.toLowerCase()) {
      name = name.slice(word.length).trim();
      break;
    }
  }
  return name ? `${brand} ${name}` : brand;
}
