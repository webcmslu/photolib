// Where the interface runs: in the desktop app (Tauri), or in a browser
// served by the Phototek server on a NAS. In a browser there is no Tauri:
// commands go over HTTP (see `invoke` in api.ts), and the few window
// features used here fall back to what browsers offer.
import { getCurrentWindow } from "@tauri-apps/api/window";

/** True in a browser, served by the Phototek server. */
export const inBrowser = typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);

/** The window's full screen state, in the app or in a browser. */
export function appWindow(): {
  isFullscreen: () => Promise<boolean>;
  setFullscreen: (on: boolean) => Promise<void>;
} {
  if (!inBrowser) return getCurrentWindow();
  return {
    isFullscreen: async () => document.fullscreenElement !== null,
    setFullscreen: async (on) => {
      if (on && !document.fullscreenElement) await document.documentElement.requestFullscreen();
      else if (!on && document.fullscreenElement) await document.exitFullscreen();
    },
  };
}
