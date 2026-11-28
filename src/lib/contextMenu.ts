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
import { noDesktop } from "./platform";

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
  if (noDesktop) return showPageMenu(event.clientX, event.clientY, entries);
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

// --- In a browser: a menu drawn in the page --------------------------------

let openMenu: HTMLElement | null = null;

function closePageMenu() {
  openMenu?.remove();
  openMenu = null;
}

/** Draws the menu at (`x`, `y`); a submenu's entries follow it, indented. */
function showPageMenu(x: number, y: number, entries: MenuEntry[]) {
  closePageMenu();
  const menu = document.createElement("div");
  menu.setAttribute("role", "menu");
  menu.style.cssText = `position: fixed; z-index: 10000; min-width: 180px; max-height: calc(100vh - 16px);
    overflow-y: auto; padding: 5px; border-radius: 9px; background: var(--surface, #fff); color: var(--text, #111);
    border: 1px solid var(--border, #ccc); box-shadow: 0 8px 28px rgb(0 0 0 / 0.25); font: 13px system-ui, sans-serif;`;
  const add = (list: MenuEntry[], depth: number) => {
    for (const entry of list) {
      if (entry === "-") {
        const line = document.createElement("hr");
        line.style.cssText = "margin: 4px 6px; border: none; border-top: 1px solid var(--border, #ccc);";
        menu.append(line);
        continue;
      }
      const item = document.createElement("button");
      item.textContent = entry.label;
      item.setAttribute("role", "menuitem");
      const enabled = entry.enabled ?? ("items" in entry ? entry.items.length > 0 : true);
      item.disabled = !enabled || "items" in entry;
      item.style.cssText = `display: block; width: 100%; padding: 5px 10px 5px ${10 + depth * 14}px; border: none;
        border-radius: 5px; background: none; color: inherit; font: inherit; text-align: left; cursor: default;
        opacity: ${enabled ? 1 : 0.45}; ${"items" in entry ? "font-weight: 600;" : ""}`;
      if ("action" in entry && enabled) {
        item.onmouseenter = () => (item.style.background = "var(--accent, #0a84ff)");
        item.onmouseleave = () => (item.style.background = "none");
        item.onclick = () => {
          closePageMenu();
          entry.action();
        };
      }
      menu.append(item);
      if ("items" in entry) add(entry.items, depth + 1);
    }
  };
  add(entries, 0);
  document.body.append(menu);
  // Inside the window, even near its edges.
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, innerHeight - box.height - 8))}px`;
  openMenu = menu;
  setTimeout(() => {
    const away = (event: Event) => {
      if (event instanceof KeyboardEvent && event.key !== "Escape") return;
      if (event.target instanceof Node && menu.contains(event.target) && event.type !== "keydown") return;
      closePageMenu();
      removeEventListener("pointerdown", away, true);
      removeEventListener("keydown", away, true);
      removeEventListener("blur", away);
    };
    addEventListener("pointerdown", away, true);
    addEventListener("keydown", away, true);
    addEventListener("blur", away);
  });
}
