// Loads the photo listing in pages, on demand, so the grid never needs the
// whole library in memory.
import { photosPage, type PhotoItem } from "./api";

const PAGE_SIZE = 200;
/** Pages kept in memory (40,000 photos). The farthest ones are dropped first. */
const MAX_PAGES = 200;

export class PhotoPages {
  /** Bumped whenever a page arrives, so that readers of `get` re-render. */
  #version = $state(0);
  #pages = new Map<number, PhotoItem[]>();
  #loading = new Map<number, Promise<void>>();
  /** Bumped by `reset`; answers to requests made before it are thrown away. */
  #generation = 0;

  constructor(
    readonly libraryPath: string,
    /** Null lists the whole library. */
    readonly folder: string | null,
  ) {}

  /** The photo at `index` of the listing, if its page is loaded. Reactive. */
  get(index: number): PhotoItem | undefined {
    void this.#version;
    return this.#pages.get(Math.floor(index / PAGE_SIZE))?.[index % PAGE_SIZE];
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
    await this.#load(Math.floor(index / PAGE_SIZE));
    return this.get(index);
  }

  /** Forgets everything, after the index changed. */
  reset(): void {
    this.#generation++;
    this.#pages.clear();
    this.#loading.clear();
    this.#version++;
  }

  #load(page: number): Promise<void> {
    if (this.#pages.has(page)) return Promise.resolve();
    let pending = this.#loading.get(page);
    if (!pending) {
      const generation = this.#generation;
      pending = photosPage(this.libraryPath, this.folder, page * PAGE_SIZE, PAGE_SIZE)
        .then((items) => {
          if (generation !== this.#generation) return;
          this.#pages.set(page, items);
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
