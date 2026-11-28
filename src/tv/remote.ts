// The remote control: its keys, and moving the focus with the arrows to the
// nearest item in that direction (any element with `data-focus`, inside the
// screen on top, the last `[data-screen]` that is shown).

/** What a key on the remote means, on webOS, Tizen, Android TV or a keyboard. */
export type RemoteKey = "up" | "down" | "left" | "right" | "ok" | "back" | "play" | "pause" | "playPause" | null;

export function remoteKey(event: KeyboardEvent): RemoteKey {
  switch (event.key) {
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    case "Enter":
      return "ok";
    case "Escape":
    case "Backspace":
    case "GoBack":
    case "BrowserBack":
      return "back";
    case "MediaPlay":
      return "play";
    case "MediaPause":
      return "pause";
    case "MediaPlayPause":
      return "playPause";
  }
  // TVs that send key codes only.
  switch (event.keyCode) {
    case 461: // webOS Back
    case 10009: // Tizen Return
      return "back";
    case 415:
      return "play";
    case 19:
      return "pause";
    case 10252:
      return "playPause";
  }
  return null;
}

/** The screen the remote acts on: the last one shown. */
function topScreen(): HTMLElement {
  const screens = [...document.querySelectorAll<HTMLElement>("[data-screen]")].filter(
    (screen) => getComputedStyle(screen).visibility !== "hidden",
  );
  return screens.at(-1) ?? document.body;
}

function focusables(): HTMLElement[] {
  return [...topScreen().querySelectorAll<HTMLElement>("[data-focus]")].filter((element) => {
    const box = element.getBoundingClientRect();
    return box.width > 0 && box.height > 0 && !element.hasAttribute("disabled");
  });
}

/** Focuses `element` and brings it into view, smoothly. */
export function focusOn(element: HTMLElement | null | undefined) {
  if (!element) return;
  element.focus({ preventScroll: true });
  // At once: smooth scrolling stutters on TVs.
  element.scrollIntoView({ block: "nearest", inline: "center" });
}

/**
 * Focuses the item of the screen on top marked `data-first` (or else the
 * first one), unless one already has the focus.
 */
export function focusFirst() {
  const items = focusables();
  if (items.includes(document.activeElement as HTMLElement)) return;
  focusOn(items.find((item) => item.hasAttribute("data-first")) ?? items[0]);
}

function onScreen(box: DOMRect): boolean {
  return box.right > 0 && box.left < window.innerWidth;
}

/**
 * Moves the focus to the nearest item in that direction: the one whose
 * centre is closest, counting sideways distance more, so that moving down a
 * grid stays in the same column. Returns false at the edge.
 */
export function moveFocus(direction: "up" | "down" | "left" | "right"): boolean {
  const items = focusables();
  const current = document.activeElement as HTMLElement;
  if (!items.includes(current)) {
    focusOn(items[0]);
    return items.length > 0;
  }
  const from = current.getBoundingClientRect();
  const fromX = from.left + from.width / 2;
  const fromY = from.top + from.height / 2;
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  const vertical = direction === "up" || direction === "down";
  for (const item of items) {
    if (item === current) continue;
    const box = item.getBoundingClientRect();
    // Up and down go to what is on screen in the next row, not to a card
    // scrolled away to the side.
    if (vertical && !onScreen(box)) continue;
    const x = box.left + box.width / 2;
    const y = box.top + box.height / 2;
    const along = { up: fromY - y, down: y - fromY, left: fromX - x, right: x - fromX }[direction];
    if (along <= 1) continue;
    const across = vertical ? Math.abs(x - fromX) : Math.abs(y - fromY);
    const score = along + across * 3;
    if (score < bestScore) {
      bestScore = score;
      best = item;
    }
  }
  if (!best) return false;
  focusOn(best);
  return true;
}
