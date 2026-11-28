// The phone app's own commands (src-tauri/src/mobile.rs): backing up the
// phone's photos to the Phototek server in use, and freeing up space. They
// run on the phone, not on the server, so they skip the wrapper in api.ts.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { server } from "./platform";

/** Mirrors `Progress` in mobile.rs. */
export interface BackupProgress {
  running: boolean;
  /** Photos and videos on the phone. */
  total: number;
  /** How many of them are on the server. */
  saved: number;
  /** Sent during this backup. */
  sent: number;
  /** Couldn't be read or sent this time (tried again next time). */
  failed: number;
  error: string | null;
}

function current() {
  const remote = server();
  if (!remote) throw new Error("Not connected to a Phototek server.");
  return { url: remote.url, token: remote.token };
}

const NAME_KEY = "phototek.phoneName";

/** The name this phone's photos are filed under on the server. */
export async function phoneName(): Promise<string> {
  try {
    const saved = localStorage.getItem(NAME_KEY);
    if (saved) return saved;
  } catch {
    // Asked again below.
  }
  return invoke<string>("phone_name").catch(() => "Phone");
}

export function savePhoneName(name: string) {
  try {
    localStorage.setItem(NAME_KEY, name.trim());
  } catch {
    // Asked again next time.
  }
}

const AUTO_KEY = "phototek.autoBackup";

/** Back up each time the app opens. */
export function autoBackup(): boolean {
  try {
    const saved = localStorage.getItem(AUTO_KEY);
    if (saved !== null) return saved === "1";
  } catch {
    // The default below.
  }
  return import.meta.env.VITE_PHOTOTEK_AUTO_BACKUP === "1";
}

export function setAutoBackup(on: boolean) {
  try {
    localStorage.setItem(AUTO_KEY, on ? "1" : "0");
  } catch {
    // Not remembered.
  }
}

/**
 * Backs up while the app is closed too, every few hours (Android), or
 * stops. Resolves with whether the phone does it (not on iPhones yet).
 */
export async function scheduleBackground(on: boolean): Promise<boolean> {
  return invoke<boolean>("schedule_phone_backup", { server: current(), device: await phoneName(), on }).catch(
    () => false,
  );
}

export function backupStatus(): Promise<BackupProgress> {
  return invoke("phone_backup_status", { server: current() });
}

export async function startBackup(): Promise<void> {
  return invoke("start_phone_backup", { server: current(), device: await phoneName() });
}

export function stopBackup(): Promise<void> {
  return invoke("stop_phone_backup");
}

export function onBackupProgress(handler: (progress: BackupProgress) => void): Promise<UnlistenFn> {
  return listen<BackupProgress>("phone-backup", (event) => handler(event.payload));
}

/** How many photos and videos freeing up space would remove from the phone. */
export function spaceToFree(): Promise<number> {
  return invoke("phone_space_to_free", { server: current() });
}

/** Removes from the phone those safely on the server, once the user confirms. */
export function freeSpace(): Promise<number> {
  return invoke("free_phone_space", { server: current() });
}
