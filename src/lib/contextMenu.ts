// Right-click menus, drawn by the system (a real macOS, Windows or Linux
// menu) rather than in the page. Everywhere else the webview's own menu
// ("Reload", "Inspect Element") is turned off; text fields keep theirs, for
// cut, copy and paste.
import {
  Menu,
  MenuItem,
  PredefinedMenuItem,
  Submenu,
} from "@tauri-apps/api/menu";

export type MenuEntry =
  | { label: string; action: () => void; enabled?: boolean }
  | { label: string; items: MenuEntry[]; enabled?: boolean }
  /** A line between groups of entries. */
  | "-";

type Built = MenuItem | PredefinedMenuItem | Submenu;

async function build(entries: MenuEntry[]): Promise<Built[]> {
  const built: Built[] = [];
  for (const entry of entries) {
    if (entry === "-") {
      if (built.length > 0)
        built.push(await PredefinedMenuItem.new({ item: "Separator" }));
    } else if ("items" in entry) {
      built.push(
        await Submenu.new({
          text: entry.label,
          enabled: entry.enabled ?? entry.items.length > 0,
          items: await build(entry.items),
        }),
      );
    } else {
      built.push(
        await MenuItem.new({
          text: entry.label,
          enabled: entry.enabled ?? true,
          action: entry.action,
        }),
      );
    }
  }
  return built;
}

/** Shows a menu at the mouse, for a `contextmenu` event. */
export async function showMenu(
  event: MouseEvent,
  entries: MenuEntry[],
): Promise<void> {
  event.preventDefault();
  event.stopPropagation();
  try {
    const menu = await Menu.new({ items: await build(entries) });
    await menu.popup();
  } catch {
    // No menu this time; nothing else depends on it.
  }
}

/** Turns off the webview's own menu everywhere but in text fields. */
export function suppressDefaultMenu(event: MouseEvent) {
  const target = event.target as HTMLElement | null;
  if (target?.closest("input, textarea, [contenteditable='true']")) return;
  event.preventDefault();
}
