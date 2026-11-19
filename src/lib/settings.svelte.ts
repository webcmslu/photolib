// Preferences of this computer, chosen in the Settings window. Kept in the
// webview's storage: they are about how this computer shows photos, not
// about the library, so they don't go into the shared folder.

import type { Order } from "./api";

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

/** The orders of the photo grid, as the Settings window names them. */
export const ORDERS: Record<Order, string> = {
  dateDesc: "Newest first",
  dateAsc: "Oldest first",
  nameAsc: "By name, A to Z",
  nameDesc: "By name, Z to A",
};

const KEY = "photolib.settings";
/** Where the order was kept before it moved into the settings. */
const OLD_ORDER_KEY = "photolib.order";

interface Settings {
  /** How long each photo of a slideshow stays on screen. */
  slideSeconds: number;
  transition: Transition;
  /** The order of the photos, also changed with the Sort button. */
  order: Order;
}

const DEFAULTS: Settings = { slideSeconds: 4, transition: "fade", order: "dateDesc" };

function isOrder(value: unknown): value is Order {
  return typeof value === "string" && value in ORDERS;
}

function load(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Settings>;
    const oldOrder = localStorage.getItem(OLD_ORDER_KEY);
    return {
      slideSeconds: SLIDE_SECONDS.includes(Number(saved.slideSeconds))
        ? Number(saved.slideSeconds)
        : DEFAULTS.slideSeconds,
      transition:
        saved.transition && saved.transition in TRANSITIONS ? saved.transition : DEFAULTS.transition,
      order: isOrder(saved.order) ? saved.order : isOrder(oldOrder) ? oldOrder : DEFAULTS.order,
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
