// Where the interface runs: in the desktop app (Tauri), or in a browser
// served by the Phototek server on a NAS. In a browser there is no Tauri:
// commands go over HTTP (see `invoke` in api.ts), and the few window
// features used here fall back to what browsers offer.
import { getCurrentWindow } from "@tauri-apps/api/window";

/** True in a browser, served by the Phototek server. */
export const inBrowser = typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);

/** A Phototek server the interface talks to instead of the app's own commands. */
export interface Server {
  /** "" for the server that served the page; else e.g. "http://nas.local:8080". */
  url: string;
  /** From `POST /api/login`, when the server has a password. */
  token: string | null;
  /** Its library's name, once known. */
  name?: string;
}

/** The server in use: the page's own in a browser; in the app, one chosen on the welcome screen. */
let current: Server | null = inBrowser ? { url: "", token: null } : null;

export const server = (): Server | null => current;

export function useServer(next: Server | null) {
  current = next ?? (inBrowser ? { url: "", token: null } : null);
}

const SERVERS_KEY = "phototek.servers";

/** The servers this app connected to, most recent first. */
export function savedServers(): Server[] {
  try {
    const list = JSON.parse(localStorage.getItem(SERVERS_KEY) ?? "[]");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

export function saveServer(entry: Server) {
  const list = [entry, ...savedServers().filter((other) => other.url !== entry.url)].slice(0, 8);
  try {
    localStorage.setItem(SERVERS_KEY, JSON.stringify(list));
  } catch {
    // Not remembered this time.
  }
}

export function forgetServer(url: string) {
  try {
    localStorage.setItem(SERVERS_KEY, JSON.stringify(savedServers().filter((other) => other.url !== url)));
  } catch {
    // Still listed.
  }
}

/** "nas.local:8080" → "http://nas.local:8080", without a trailing slash. */
export function serverUrl(typed: string): string {
  const trimmed = typed.trim().replace(/\/+$/, "");
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

/** Logs in to a server; resolves with its token (`null` when it has no password). */
export async function logIn(url: string, password: string): Promise<string | null> {
  const response = await fetch(`${url}/api/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ password }),
  });
  if (response.status === 401) throw new Error("wrong password");
  if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
  const { token } = (await response.json()) as { token: string };
  return token;
}

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
