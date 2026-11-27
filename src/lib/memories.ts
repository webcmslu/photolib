// How memories are worded, and the soft music they play to.

import type { Memory } from "./api";
import { formatDayRange } from "./format";
import { locale, t, tn } from "./i18n";

const DAY_MS = 86_400_000;
const capitalized = (text: string) =>
  text.charAt(0).toLocaleUpperCase(locale) + text.slice(1);
const monthYear = new Intl.DateTimeFormat(locale, {
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
const dayMonth = new Intl.DateTimeFormat(locale, {
  day: "numeric",
  month: "long",
  timeZone: "UTC",
});
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
      return memory.title
        ? formatDayRange(memory.fromDay, memory.toDay)
        : memory.people.join(", ");
    case "onThisDay": {
      const date = dayMonth.format(today * DAY_MS);
      if (memory.years.length === 1) {
        const ago = yearOf(today) - memory.years[0];
        return tn(ago, "{date}, {n} year ago", "{date}, {n} years ago", {
          date,
        });
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
 * The one audio context, made (or woken) from a click: the Mac's web view
 * keeps sound started any later paused, and the memory then plays silently.
 */
let shared: AudioContext | null = null;
/** The reverb's echo, made once per sample rate. */
const impulses = new Map<number, AudioBuffer>();
/** Each style's loudness, measured to peak at about half of full scale. */
const PADS_VOLUME = 1.5;
const LIGHT_VOLUME = 2.4;

/** Lets sound play: call it straight from the click or key that starts a memory. */
export function unlockAudio() {
  try {
    shared ??= new AudioContext();
    if (shared.state !== "running") void shared.resume().catch(() => {});
  } catch {
    // No sound on this computer: memories play silently.
  }
}

export type MusicStyle = "pads" | "light";
export const randomStyle = (): MusicStyle =>
  Math.random() < 0.5 ? "pads" : "light";

/** A reverb's echo: decaying noise. */
function impulseFor(context: BaseAudioContext): AudioBuffer {
  let impulse = impulses.get(context.sampleRate);
  if (!impulse) {
    const length = context.sampleRate * 3;
    impulse = context.createBuffer(2, length, context.sampleRate);
    for (let channel = 0; channel < 2; channel++) {
      const data = impulse.getChannelData(channel);
      for (let i = 0; i < length; i++)
        data[i] = (Math.random() * 2 - 1) * (1 - i / length) ** 3;
    }
    impulses.set(context.sampleRate, impulse);
  }
  return impulse;
}

/**
 * Sets up a piece made up as it goes (nothing to license or to download),
 * in one of two styles: slow dark pad chords with a few bells, or a light
 * arpeggio, as on a soft piano. Each memory keeps its own chords (and
 * tempo), picked from its id. The music goes into `master` (silent until
 * its gain is raised to `volume`), through a limiter to the speakers;
 * `writeUntil` writes the notes up to a time.
 */
function compose(context: BaseAudioContext, seed: string, style: MusicStyle) {
  // Background level: about half of full scale at its loudest, with a
  // limiter so that chords and bells together never crackle.
  const volume = style === "pads" ? PADS_VOLUME : LIGHT_VOLUME;
  const master = context.createGain();
  master.gain.setValueAtTime(0, context.currentTime);
  const limiter = context.createDynamicsCompressor();
  limiter.threshold.value = -10;
  limiter.knee.value = 6;
  limiter.ratio.value = 8;
  master.connect(limiter).connect(context.destination);

  const reverb = context.createConvolver();
  reverb.buffer = impulseFor(context);
  const wet = context.createGain();
  wet.gain.value = style === "pads" ? 0.6 : 0.45;
  reverb.connect(wet).connect(master);
  const dry = context.createGain();
  dry.gain.value = style === "pads" ? 0.5 : 0.7;
  dry.connect(master);
  const tone = context.createBiquadFilter();
  tone.type = "lowpass";
  tone.frequency.value = style === "pads" ? 1400 : 3200;
  tone.connect(dry);
  tone.connect(reverb);

  let hash = 0;
  for (const char of seed) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  const frequency = (note: number) => 440 * 2 ** ((note - 69) / 12);

  function voice(
    type: OscillatorType,
    note: number,
    start: number,
    peak: number,
    decay: number,
    to: AudioNode,
  ) {
    const oscillator = context.createOscillator();
    oscillator.type = type;
    oscillator.frequency.value = frequency(note);
    const gain = context.createGain();
    gain.gain.setValueAtTime(0, start);
    gain.gain.linearRampToValueAtTime(peak, start + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, start + decay);
    oscillator.connect(gain);
    gain.connect(to);
    oscillator.start(start);
    oscillator.stop(start + decay + 0.05);
    return gain;
  }
  /** A chord held for `length` seconds, swelling in and fading out. */
  function pad(
    notes: number[],
    start: number,
    length: number,
    level: number,
    detunes: number[],
  ) {
    for (const note of notes) {
      for (const detune of detunes) {
        const oscillator = context.createOscillator();
        oscillator.type = "triangle";
        oscillator.frequency.value = frequency(note);
        oscillator.detune.value = detune;
        const gain = context.createGain();
        gain.gain.setValueAtTime(0, start);
        gain.gain.linearRampToValueAtTime(level, start + 1.4);
        gain.gain.linearRampToValueAtTime(level * 0.8, start + length);
        gain.gain.linearRampToValueAtTime(0, start + length + 1.2);
        oscillator.connect(gain).connect(tone);
        oscillator.start(start);
        oscillator.stop(start + length + 1.3);
      }
    }
  }

  /** Writes one chord's worth of music at `start`, and says how long it lasts. */
  let chord: (step: number, start: number) => number;
  if (style === "pads") {
    // Four-note chords, low and slow (MIDI notes).
    const progressions = [
      // I – vi – IV – V
      [
        [48, 55, 64, 71],
        [45, 52, 60, 67],
        [41, 48, 57, 64],
        [43, 50, 59, 66],
      ],
      // vi – IV – I – V
      [
        [45, 52, 60, 64],
        [41, 48, 57, 60],
        [48, 55, 64, 67],
        [43, 50, 59, 62],
      ],
      // I – iii – IV – iv
      [
        [48, 55, 64, 67],
        [52, 59, 67, 71],
        [53, 60, 69, 72],
        [53, 60, 68, 72],
      ],
    ];
    const chords = progressions[hash % progressions.length];
    const CHORD_S = 4.8;
    chord = (step, start) => {
      const notes = chords[step % chords.length];
      pad(notes, start, CHORD_S, 0.05, [-6, 6]);
      // A few notes of the chord, an octave up, as bells on random beats.
      for (let beat = 0; beat < 4; beat++) {
        if (Math.random() < 0.55) {
          const note = notes[1 + Math.floor(Math.random() * 3)] + 12;
          const bell = voice(
            "sine",
            note,
            start + beat * (CHORD_S / 4) + Math.random() * 0.08,
            0.07,
            2.4,
            reverb,
          );
          bell.connect(dry);
        }
      }
      return CHORD_S;
    };
  } else {
    // Three-note chords around middle C.
    const progressions = [
      // C – G – Am – F
      [
        [60, 64, 67],
        [55, 59, 62],
        [57, 60, 64],
        [53, 57, 60],
      ],
      // F – C – G – Am
      [
        [53, 57, 60],
        [60, 64, 67],
        [55, 59, 62],
        [57, 60, 64],
      ],
      // C – Am – F – G
      [
        [60, 64, 67],
        [57, 60, 64],
        [53, 57, 60],
        [55, 59, 62],
      ],
      // D – A – Bm – G
      [
        [62, 66, 69],
        [57, 61, 64],
        [59, 62, 66],
        [55, 59, 62],
      ],
    ];
    const chords = progressions[hash % progressions.length];
    // Arpeggio shapes over the chord's notes (3, 4 and 5 are an octave up).
    const patterns = [
      [0, 1, 2, 3, 2, 1, 2, 1],
      [0, 2, 1, 3, 2, 4, 3, 2],
      [0, 1, 2, 1, 3, 2, 4, 2],
    ];
    const pattern = patterns[(hash >>> 4) % patterns.length];
    const STEP_S = [0.34, 0.37, 0.4][(hash >>> 8) % 3];
    const CHORD_S = STEP_S * 16; // two turns of the arpeggio per chord
    chord = (step, start) => {
      const notes = chords[step % chords.length];
      const tones = [...notes, ...notes.map((note) => note + 12)];
      pad(notes, start, CHORD_S, 0.018, [0]);
      // The low note, held under the chord.
      voice("sine", notes[0] - 24, start, 0.12, CHORD_S, tone);
      for (let i = 0; i < 16; i++) {
        const note = tones[pattern[i % pattern.length]] + 12;
        // The first chord comes in gently; each bar leans on its first note.
        const loud = (step === 0 ? 0.6 : 1) * (i % 8 === 0 ? 1 : 0.7);
        // A plucked note: a sine with a touch of its octave, like a soft piano.
        const at = start + i * STEP_S + Math.random() * 0.012;
        voice("sine", note, at, 0.09 * loud, 1.6, tone);
        voice("triangle", note + 12, at, 0.025 * loud, 0.6, tone);
      }
      // Now and then, a high bell.
      if (step > 0 && Math.random() < 0.5)
        voice("sine", notes[2] + 36, start + STEP_S * 8, 0.03, 2.5, reverb);
      return CHORD_S;
    };
  }

  let step = 0;
  let next = context.currentTime + 0.15;
  return {
    master,
    limiter,
    volume,
    /** Writes the music, chord by chord, until at least `time`. */
    writeUntil(time: number) {
      while (next < time) next += chord(step++, next);
    },
  };
}

/** Plays a memory's music, in a style picked at random unless given. */
export function playMusic(
  seed: string,
  style: MusicStyle = randomStyle(),
): { stop: () => void; setVolume: (on: boolean) => void } {
  unlockAudio();
  if (!shared) throw new Error("no sound");
  const context = shared;
  const music = compose(context, seed, style);
  const { master, limiter, volume } = music;
  master.gain.linearRampToValueAtTime(volume, context.currentTime + 2.5);
  // Writes the music a little ahead.
  const write = () => music.writeUntil(context.currentTime + 8);
  write();
  const timer = setInterval(write, 500);

  function fadeTo(value: number, seconds: number) {
    const now = context.currentTime;
    master.gain.cancelScheduledValues(now);
    master.gain.setValueAtTime(master.gain.value, now);
    master.gain.linearRampToValueAtTime(value, now + seconds);
  }
  return {
    setVolume(on: boolean) {
      unlockAudio();
      fadeTo(on ? volume : 0, 0.6);
    },
    stop() {
      clearInterval(timer);
      fadeTo(0, 1.2);
      setTimeout(() => {
        master.disconnect();
        limiter.disconnect();
      }, 1400);
    },
  };
}

/** Renders `seconds` of a memory's music, fading in and out, for its video (48 kHz stereo). */
export function renderMusic(
  seed: string,
  seconds: number,
  style: MusicStyle = randomStyle(),
): Promise<AudioBuffer> {
  const RATE = 48_000;
  const context = new OfflineAudioContext(2, Math.ceil(seconds * RATE), RATE);
  const music = compose(context, seed, style);
  const gain = music.master.gain;
  gain.linearRampToValueAtTime(music.volume, 2.5);
  gain.setValueAtTime(music.volume, Math.max(2.5, seconds - 3));
  gain.linearRampToValueAtTime(0, seconds);
  music.writeUntil(seconds);
  return context.startRendering();
}
