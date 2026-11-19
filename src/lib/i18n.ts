// Translations. The English text is written in the code, wrapped in `t()`,
// and is its own key: locales/fr.ts and locales/de.ts map it to French and
// German. A text missing there shows in English. `npm run check` lists the
// missing ones (tools/i18n-check.mjs).
//
// The language is chosen in Settings and applies after a reload, so
// everything, including texts built once at start, follows it.

import de from "./locales/de";
import fr from "./locales/fr";

export const LANGUAGES = {
  system: "System language",
  en: "English",
  fr: "Français",
  de: "Deutsch",
} as const;
export type LanguageChoice = keyof typeof LANGUAGES;
type Language = Exclude<LanguageChoice, "system">;

const KEY = "photolib.language";

export function languageChoice(): LanguageChoice {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved && saved in LANGUAGES) return saved as LanguageChoice;
  } catch {
    // Storage unavailable: follow the system.
  }
  return "system";
}

/** Saves the choice and reloads the window so that every text follows it. */
export function chooseLanguage(choice: LanguageChoice) {
  try {
    localStorage.setItem(KEY, choice);
  } catch {
    return;
  }
  location.reload();
}

function systemLanguage(): Language {
  for (const tag of navigator.languages ?? [navigator.language]) {
    const code = tag.slice(0, 2).toLowerCase();
    if (code === "fr" || code === "de" || code === "en") return code;
  }
  return "en";
}

const choice = languageChoice();
export const language: Language = choice === "system" ? systemLanguage() : choice;

/** For Intl: the system's own variant (fr-CH, de-AT…) when it is the same language. */
export const locale: string =
  (navigator.languages ?? [navigator.language]).find((tag) => tag.slice(0, 2).toLowerCase() === language) ??
  language;

const CATALOGS: Record<Language, Record<string, string> | null> = { en: null, fr, de };
const catalog = CATALOGS[language];
const numbers = new Intl.NumberFormat(locale);
const plurals = new Intl.PluralRules(locale);

type Params = Record<string, string | number>;

/**
 * The text in the chosen language, with `{name}` replaced by `params.name`.
 * Numbers are written the local way (1,234 or 1 234); pass a string for a
 * number that must stay as it is, such as a year.
 */
export function t(text: string, params?: Params): string {
  const translated = catalog?.[text] || text;
  if (!params) return translated;
  return translated.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = params[name];
    if (value === undefined) return whole;
    return typeof value === "number" ? numbers.format(value) : value;
  });
}

/**
 * `one` or `other` by the rules of the chosen language (in French 0 is
 * singular too), translated, with `{n}` the count.
 */
export function tn(count: number, one: string, other: string, params?: Params): string {
  return t(plurals.select(count) === "one" ? one : other, { n: count, ...params });
}

/**
 * Messages that come from the Rust side, in English: a fixed sentence, or
 * a fixed start followed by the system's own explanation ("{detail}"), which
 * stays as the system wrote it.
 */
const RUST_MESSAGES = [
  "The library is offline, so changes can't be saved right now.",
  "This library is not open.",
  "The library can't be reached right now.",
  "The library folder is read-only, so copies can't be moved.",
  "Couldn't save the change: {detail}",
  "Couldn't save the recent libraries list: {detail}",
  "Couldn't save the setting: {detail}",
  "Couldn't download the face models: {detail}",
  "Couldn't write to {detail}",
  "Couldn't read the Photos library: {detail}",
  "Index database error: {detail}",
];

/** An error to show: a message from the Rust side is translated when it is a known one. */
export function errorText(error: unknown): string {
  const text = String(error);
  if (!catalog) return text;
  for (const message of RUST_MESSAGES) {
    const at = message.indexOf("{detail}");
    if (at < 0) {
      if (text === message) return t(message);
    } else if (text.startsWith(message.slice(0, at))) {
      return t(message, { detail: text.slice(at) });
    }
  }
  return text;
}
