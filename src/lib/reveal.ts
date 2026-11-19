// Shows a photo's file in the system's file manager, selected, so that the
// user can move, rename or delete it there: the app itself never changes
// the library's files.
import { sep } from "@tauri-apps/api/path";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

const platform = navigator.userAgent;

/** "Show in Finder", "Show in Explorer" or "Show in Files". */
export const REVEAL_LABEL = platform.includes("Mac")
  ? "Show in Finder"
  : platform.includes("Windows")
    ? "Show in Explorer"
    : "Show in Files";

export function revealPhoto(libraryPath: string, relPath: string): Promise<void> {
  const separator = sep();
  const root = libraryPath.endsWith(separator) ? libraryPath : libraryPath + separator;
  return revealItemInDir(root + relPath.split("/").join(separator));
}
