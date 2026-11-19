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
