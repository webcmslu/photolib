// How memories are worded, and the soft music they play to.

import type { Memory } from "./api";
import { formatDayRange } from "./format";
import { locale, t, tn } from "./i18n";

const DAY_MS = 86_400_000;
const capitalized = (text: string) => text.charAt(0).toLocaleUpperCase(locale) + text.slice(1);
const monthYear = new Intl.DateTimeFormat(locale, { month: "short", year: "numeric", timeZone: "UTC" });
const dayMonth = new Intl.DateTimeFormat(locale, { day: "numeric", month: "long", timeZone: "UTC" });
const yearOf = (day: number) => new Date(day * DAY_MS).getUTCFullYear();
const monthOf = (day: number) => new Date(day * DAY_MS).getUTCMonth();

/** "Aug 2018", or "Jul – Aug 2018" across months. */
function months(from: number, to: number): string {
  return capitalized(
    monthOf(from) === monthOf(to) && yearOf(from) === yearOf(to)
      ? monthYear.format(from * DAY_MS)
      : monthYear.formatRange(from * DAY_MS, to * DAY_MS),
  );
}

function seasonName(memory: Memory): string {
  // The season starts on its first day; winter takes the year of its January.
  const month = monthOf(memory.fromDay);
  // A string: as a number it would be written "2 018" in French.
  const year = String(yearOf(memory.toDay));
  if (month === 11) return t("Winter {year}", { year });
  if (month < 5) return t("Spring {year}", { year });
  if (month < 8) return t("Summer {year}", { year });
  return t("Autumn {year}", { year });
}

export function memoryTitle(memory: Memory): string {
  switch (memory.kind) {
    case "trip":
    case "event":
      return memory.title ?? formatDayRange(memory.fromDay, memory.toDay);
    case "onThisDay":
      return t("On This Day");
    case "person":
      return memory.title ?? memory.people[0] ?? "";
    case "together":
      return memory.people.join(" & ");
    case "season":
      return seasonName(memory);
    case "year":
      return t("Best of {year}", { year: String(yearOf(memory.fromDay)) });
  }
}

export function memorySubtitle(memory: Memory, today: number): string {
  switch (memory.kind) {
    case "trip":
      return t("{date} Trip", { date: months(memory.fromDay, memory.toDay) });
    case "event":
      return memory.title ? formatDayRange(memory.fromDay, memory.toDay) : memory.people.join(", ");
    case "onThisDay": {
      const date = dayMonth.format(today * DAY_MS);
      if (memory.years.length === 1) {
        const ago = yearOf(today) - memory.years[0];
        return tn(ago, "{date}, {n} year ago", "{date}, {n} years ago", { date });
      }
      return t("{date}, over the years", { date });
    }
    case "person":
      return t("Over the Years");
    case "together":
      return t("Together");
    case "season":
      return months(memory.fromDay, memory.toDay);
    case "year":
      return t("Year in review");
  }
}

// --- Music --------------------------------------------------------------------

/** Whether memories play their music, kept per computer. */
const MUSIC_KEY = "photolib.memoryMusic";
export function musicWanted(): boolean {
  try {
    return localStorage.getItem(MUSIC_KEY) !== "off";
  } catch {
    return true;
  }
}
export function setMusicWanted(on: boolean) {
  try {
    localStorage.setItem(MUSIC_KEY, on ? "on" : "off");
  } catch {
    // Not remembered this time.
  }
}

/**
 * Plays a slow, soft piece made up as it goes (nothing to license or to
 * download): pad chords that change every few seconds, with a few bell
 * notes over them, through a little reverb. Returns a function that fades
 * it out and stops it.
 */
export function playMusic(seed: string): { stop: () => void; setVolume: (on: boolean) => void } {
  const context = new AudioContext();
  const master = context.createGain();
  master.gain.value = 0;
  master.gain.linearRampToValueAtTime(0.16, context.currentTime + 3);
  master.connect(context.destination);

  // A reverb made of decaying noise.
  const reverb = context.createConvolver();
  const length = context.sampleRate * 3;
  const impulse = context.createBuffer(2, length, context.sampleRate);
  for (let channel = 0; channel < 2; channel++) {
    const data = impulse.getChannelData(channel);
    for (let i = 0; i < length; i++) data[i] = (Math.random() * 2 - 1) * (1 - i / length) ** 3;
  }
  reverb.buffer = impulse;
  const wet = context.createGain();
  wet.gain.value = 0.6;
  reverb.connect(wet).connect(master);
  const dry = context.createGain();
  dry.gain.value = 0.5;
  dry.connect(master);
  const lowpass = context.createBiquadFilter();
  lowpass.type = "lowpass";
  lowpass.frequency.value = 1400;
  lowpass.connect(dry);
  lowpass.connect(reverb);

  // A progression picked from the memory, so each one has its own.
  const progressions = [
    // I – vi – IV – V, in MIDI notes
    [[48, 55, 64, 71], [45, 52, 60, 67], [41, 48, 57, 64], [43, 50, 59, 66]],
    // vi – IV – I – V
    [[45, 52, 60, 64], [41, 48, 57, 60], [48, 55, 64, 67], [43, 50, 59, 62]],
    // I – iii – IV – iv
    [[48, 55, 64, 67], [52, 59, 67, 71], [53, 60, 69, 72], [53, 60, 68, 72]],
  ];
  let hash = 0;
  for (const char of seed) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  const chords = progressions[hash % progressions.length];
  const frequency = (note: number) => 440 * 2 ** ((note - 69) / 12);
  const CHORD_S = 4.8;

  const sources: OscillatorNode[] = [];
  function pad(notes: number[], start: number) {
    for (const note of notes) {
      for (const detune of [-6, 6]) {
        const oscillator = context.createOscillator();
        oscillator.type = "triangle";
        oscillator.frequency.value = frequency(note);
        oscillator.detune.value = detune;
        const gain = context.createGain();
        gain.gain.setValueAtTime(0, start);
        gain.gain.linearRampToValueAtTime(0.05, start + 1.6);
        gain.gain.linearRampToValueAtTime(0.04, start + CHORD_S);
        gain.gain.linearRampToValueAtTime(0, start + CHORD_S + 1.5);
        oscillator.connect(gain).connect(lowpass);
        oscillator.start(start);
        oscillator.stop(start + CHORD_S + 1.6);
        sources.push(oscillator);
      }
    }
  }
  function bell(note: number, start: number) {
    const oscillator = context.createOscillator();
    oscillator.type = "sine";
    oscillator.frequency.value = frequency(note);
    const gain = context.createGain();
    gain.gain.setValueAtTime(0, start);
    gain.gain.linearRampToValueAtTime(0.07, start + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, start + 2.4);
    oscillator.connect(gain);
    gain.connect(reverb);
    gain.connect(dry);
    oscillator.start(start);
    oscillator.stop(start + 2.5);
    sources.push(oscillator);
  }

  let step = 0;
  let next = context.currentTime + 0.1;
  // Schedules a little ahead, chord by chord.
  const timer = setInterval(() => {
    while (next < context.currentTime + CHORD_S * 2) {
      const chord = chords[step % chords.length];
      pad(chord, next);
      // Two or three notes of the chord, an octave up, at random beats.
      for (let beat = 0; beat < 4; beat++) {
        if (Math.random() < 0.55) {
          const note = chord[1 + Math.floor(Math.random() * 3)] + 12;
          bell(note, next + beat * (CHORD_S / 4) + Math.random() * 0.08);
        }
      }
      step++;
      next += CHORD_S;
    }
    // Forget the notes that are over.
    sources.splice(0, Math.max(0, sources.length - 64));
  }, 1000);

  return {
    setVolume(on: boolean) {
      master.gain.cancelScheduledValues(context.currentTime);
      master.gain.setValueAtTime(master.gain.value, context.currentTime);
      master.gain.linearRampToValueAtTime(on ? 0.16 : 0, context.currentTime + 0.6);
    },
    stop() {
      clearInterval(timer);
      master.gain.cancelScheduledValues(context.currentTime);
      master.gain.setValueAtTime(master.gain.value, context.currentTime);
      master.gain.linearRampToValueAtTime(0, context.currentTime + 1.2);
      setTimeout(() => context.close().catch(() => {}), 1400);
    },
  };
}
