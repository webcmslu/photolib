// What the Info dialog shows: the app's name and version, and whether a newer
// version has been released. Checked once per launch, silently: with no
// network (or a GitHub hiccup) there is simply no notice.
import { getName, getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";

const RELEASES = "https://github.com/webcmslu/photolib/releases/latest";
const LATEST_API = "https://api.github.com/repos/webcmslu/photolib/releases/latest";

export const about = $state({
  name: "Photolib",
  version: "",
  /** A newer release, with the link to the installer for this computer. */
  update: null as { version: string; url: string } | null,
});

/** True if version `a` ("1.2.3") is newer than `b`. */
export function isNewer(a: string, b: string): boolean {
  const [pa, pb] = [a, b].map((v) => v.split(".").map((part) => Number.parseInt(part, 10) || 0));
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const [x, y] = [pa[i] ?? 0, pb[i] ?? 0];
    if (x !== y) return x > y;
  }
  return false;
}

let checked = false;

export async function checkForUpdate(): Promise<void> {
  if (checked) return;
  checked = true;
  try {
    [about.name, about.version] = await Promise.all([getName(), getVersion()]);
    const [response, suffix] = await Promise.all([
      fetch(LATEST_API, { headers: { Accept: "application/vnd.github+json" } }),
      invoke<string | null>("release_asset_suffix"),
    ]);
    if (!response.ok) return;
    const release = (await response.json()) as {
      tag_name?: string;
      assets?: { name: string; browser_download_url: string }[];
    };
    const latest = String(release.tag_name ?? "").replace(/^v/, "");
    if (!latest || !isNewer(latest, about.version)) return;
    // Straight to the installer for this system, else the release page.
    const asset = suffix ? release.assets?.find((a) => a.name.endsWith(suffix)) : undefined;
    about.update = { version: latest, url: asset?.browser_download_url ?? RELEASES };
  } catch {
    // No network, or no answer: no notice.
  }
}
