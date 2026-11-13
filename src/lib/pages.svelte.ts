// Loads a photo listing in pages, on demand, so the grid never needs the
// whole library in memory.
import { photosPage, type Listing, type PhotoItem } from "./api";

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

  constructor(
    readonly libraryPath: string,
    readonly listing: Listing,
  ) {}

  /** The photo at `index` of the listing, if its page is loaded. Reactive. */
  get(index: number): PhotoItem | undefined {
    void this.#version;
    const page = Math.floor(index / PAGE_SIZE);
    return (this.#pages.get(page) ?? this.#stale.get(page))?.[index % PAGE_SIZE];
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
    return this.#pages.get(page)?.[index % PAGE_SIZE];
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

  #load(page: number): Promise<void> {
    if (this.#pages.has(page)) return Promise.resolve();
    let pending = this.#loading.get(page);
    if (!pending) {
      const generation = this.#generation;
      pending = photosPage(this.libraryPath, this.listing, page * PAGE_SIZE, PAGE_SIZE)
        .then((items) => {
          if (generation !== this.#generation) return;
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
