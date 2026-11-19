// Preferences of this computer, chosen in the Settings window. Kept in the
// webview's storage: they are about how this computer shows photos, not
// about the library, so they don't go into the shared folder.

export const TRANSITIONS = {
  fade: "Cross-fade",
  slide: "Slide",
  zoom: "Zoom",
  kenburns: "Pan and zoom",
  none: "None",
} as const;
export type Transition = keyof typeof TRANSITIONS;

/** The choices offered for how long each photo stays, in seconds. */
export const SLIDE_SECONDS = [2, 3, 4, 5, 7, 10, 15, 20, 30];

const KEY = "photolib.settings";

interface Settings {
  /** How long each photo of a slideshow stays on screen. */
  slideSeconds: number;
  transition: Transition;
}

const DEFAULTS: Settings = { slideSeconds: 4, transition: "fade" };

function load(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Settings>;
    return {
      slideSeconds: SLIDE_SECONDS.includes(Number(saved.slideSeconds))
        ? Number(saved.slideSeconds)
        : DEFAULTS.slideSeconds,
      transition:
        saved.transition && saved.transition in TRANSITIONS ? saved.transition : DEFAULTS.transition,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

export const settings = $state<Settings>(load());

/** Call after changing `settings`. */
export function saveSettings() {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // Storage unavailable: the choice lasts until the app is closed.
  }
}
