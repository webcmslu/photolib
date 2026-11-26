// Loads a photo listing in pages, on demand, so the grid never needs the
// whole library in memory.
import { photosPage, type Listing, type Order, type PhotoItem } from "./api";

const PAGE_SIZE = 200;
/** Pages kept in memory (40,000 photos). The farthest ones are dropped first. */
const MAX_PAGES = 200;

export class PhotoPages {
  /** Bumped whenever a page arrives, so that readers of `get` re-render. */
  #version = $state(0);
  #pages = new Map<number, PhotoItem[]>();
  /** Pages from before the last `refresh`, shown until their replacement arrives. */
  #stale = new Map<number, PhotoItem[]>();
  #loading = new Map<number, Promise<void>>();
  /** Bumped by `refresh`; answers to requests made before it are thrown away. */
  #generation = 0;
  /**
   * Changes shown before they are saved (a heart, stars, a label), so a tap
   * shows at once even when the library is on a slow NAS. Each one stays
   * until a page loaded after the save shows the photo, or is taken back if
   * the save failed. `settled` is the refresh generation that carries it.
   */
  #pending = new Map<string, { patch: Partial<PhotoItem>; owner: object; settled: number | null }>();

  constructor(
    readonly libraryPath: string,
    readonly listing: Listing,
    readonly order: Order = "dateDesc",
  ) {}

  /** The photo at `index` of the listing, if its page is loaded. Reactive. */
  get(index: number): PhotoItem | undefined {
    void this.#version;
    const page = Math.floor(index / PAGE_SIZE);
    return this.patched((this.#pages.get(page) ?? this.#stale.get(page))?.[index % PAGE_SIZE]);
  }

  /** `item` with the changes not saved yet. Reactive. */
  patched(item: PhotoItem | undefined): PhotoItem | undefined {
    void this.#version;
    const pending = item && this.#pending.get(item.fingerprint);
    return pending ? { ...item, ...pending.patch } : item;
  }

  /**
   * Shows `patches` (by fingerprint) right away. Call `settle` once they are
   * saved, before the refresh that reloads them, or `revert` if saving failed.
   */
  optimistic(patches: Map<string, Partial<PhotoItem>>): { settle: () => void; revert: () => void } {
    const owner = {};
    for (const [fingerprint, patch] of patches) {
      const earlier = this.#pending.get(fingerprint)?.patch;
      this.#pending.set(fingerprint, { patch: { ...earlier, ...patch }, owner, settled: null });
    }
    this.#version++;
    const mine = () => [...this.#pending].filter(([, entry]) => entry.owner === owner);
    return {
      // The next refresh is generation + 1: its pages carry the saved values.
      settle: () => mine().forEach(([, entry]) => (entry.settled = this.#generation + 1)),
      revert: () => {
        for (const [fingerprint] of mine()) this.#pending.delete(fingerprint);
        this.#version++;
      },
    };
  }

  /** Starts loading the pages that cover `first..=last`, unless already there. */
  ensure(first: number, last: number): void {
    const firstPage = Math.floor(Math.max(0, first) / PAGE_SIZE);
    const lastPage = Math.floor(Math.max(0, last) / PAGE_SIZE);
    for (let page = firstPage; page <= lastPage; page++) void this.#load(page);
  }

  /** Loads (if needed) and returns the photo at `index`. */
  async load(index: number): Promise<PhotoItem | undefined> {
    if (index < 0) return undefined;
    const page = Math.floor(index / PAGE_SIZE);
    await this.#load(page);
    return this.patched(this.#pages.get(page)?.[index % PAGE_SIZE]);
  }

  /**
   * Reloads after the index or the organization changed. What is on screen
   * stays until the fresh pages arrive, so nothing flickers.
   */
  refresh(): void {
    this.#generation++;
    const visible = [...this.#pages.keys()];
    this.#stale = this.#pages;
    this.#pages = new Map();
    this.#loading.clear();
    for (const page of visible) void this.#load(page);
  }

  /**
   * Takes photos out of the listing at once (hidden, taken out of the album…),
   * before the change is saved: the photos after them move up. Only loaded
   * pages are changed; the refresh after the save puts everything right.
   * Returns where the photos were, the last one first.
   */
  removeLocally(fingerprints: Set<string>): { index: number; video: boolean }[] {
    const found: { index: number; video: boolean }[] = [];
    for (const [page, items] of this.#pages) {
      items.forEach((item, position) => {
        if (fingerprints.has(item.fingerprint)) found.push({ index: page * PAGE_SIZE + position, video: item.video });
      });
    }
    found.sort((a, b) => b.index - a.index);
    for (const { index } of found) this.#removeAt(index);
    if (found.length > 0) this.#version++;
    return found;
  }

  /** Removes the photo at `index`, pulling each following loaded page up by one. */
  #removeAt(index: number): void {
    let page = Math.floor(index / PAGE_SIZE);
    const current = this.#pages.get(page);
    if (!current) return;
    let items = current.filter((_, position) => position !== index % PAGE_SIZE);
    for (;;) {
      const next = this.#pages.get(page + 1);
      if (!next) {
        this.#pages.set(page, items);
        return;
      }
      this.#pages.set(page, [...items, next[0]]);
      items = next.slice(1);
      page++;
    }
  }

  #load(page: number): Promise<void> {
    if (this.#pages.has(page)) return Promise.resolve();
    let pending = this.#loading.get(page);
    if (!pending) {
      const generation = this.#generation;
      pending = photosPage(this.libraryPath, this.listing, this.order, page * PAGE_SIZE, PAGE_SIZE)
        .then((items) => {
          if (generation !== this.#generation) return;
          // Saved changes are in these photos now; the pending copies can go.
          for (const item of items) {
            const pending = this.#pending.get(item.fingerprint);
            if (pending?.settled != null && pending.settled <= generation) this.#pending.delete(item.fingerprint);
          }
          this.#pages.set(page, items);
          this.#stale.delete(page);
          this.#evictFarFrom(page);
          this.#version++;
        })
        .catch(() => {
          // Leave the page unloaded; the next `ensure` tries again.
        })
        .finally(() => {
          if (generation === this.#generation) this.#loading.delete(page);
        });
      this.#loading.set(page, pending);
    }
    return pending;
  }

  #evictFarFrom(page: number): void {
    while (this.#pages.size > MAX_PAGES) {
      let farthest = page;
      for (const key of this.#pages.keys()) {
        if (Math.abs(key - page) > Math.abs(farthest - page)) farthest = key;
      }
      this.#pages.delete(farthest);
    }
  }
}
