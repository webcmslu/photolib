<!--
  An open library: a sidebar (Library, Favorites, Hidden, Map, albums,
  folders), the photo grid for whatever is selected there (or the map), the
  scan status, and the viewer on top.
-->
<script lang="ts">
  import { errorText, t, tn } from "./i18n";
  import { onMount, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    appleImport,
    appleImportPreview,
    applyOps,
    closeLibrary,
    importSources,
    type ImportSource,
    createAlbum,
    libraryStats,
    listAlbums,
    listFolders,
    listMonths,
    onIndexEvent,
    pickPhotosLibrary,
    removeFromPerson,
    makeKeyPhoto,
    splitPerson,
    rescanLibrary,
    setPersonHidden,
    syncLibrary,
    timeline,
    shapes,
    listPeople,
    listingPlace,
    type Album,
    type DayGroup,
    type FolderEntry,
    type ImportSummary,
    type IndexEvent,
    type LibraryInfo,
    type Listing,
    type MonthSummary,
    type Only,
    type Op,
    type PhotoItem,
    type ScanProgress,
    type ScanSummary,
    type ThumbSummary,
    type Gear,
    type Memory,
    type Person,
  } from "./api";
  import CollectionsView, { type PinnedTile } from "./CollectionsView.svelte";
  import MemoryPlayer from "./MemoryPlayer.svelte";
  import MemoryExport from "./MemoryExport.svelte";
  import { memoryTitle, unlockAudio } from "./memories";
  import GearPanel from "./GearPanel.svelte";
  import { cameraName, formatDayRange, formatNumber, itemCount, monthKey, photoCount } from "./format";
  import MapView from "./MapView.svelte";
  import Overview from "./Overview.svelte";
  import PeopleView from "./PeopleView.svelte";
  import PersonSuggestions from "./PersonSuggestions.svelte";
  import { PhotoPages } from "./pages.svelte";
  import PhotoGrid from "./PhotoGrid.svelte";
  import Viewer from "./Viewer.svelte";
  import Slideshow from "./Slideshow.svelte";
  import AboutButton from "./AboutButton.svelte";
  import SettingsButton from "./SettingsButton.svelte";
  import ExportDialog from "./ExportDialog.svelte";
  import ImportDialog from "./ImportDialog.svelte";
  import DuplicatesView from "./DuplicatesView.svelte";
  import SimilarView from "./SimilarView.svelte";
  import ShortcutsPanel from "./ShortcutsPanel.svelte";
  import { REVEAL_LABEL, revealPhoto } from "./reveal";
  import { showMenu, type MenuEntry } from "./contextMenu";
  import { LABELS, labelForKey, type LabelColor } from "./labels";
  import { sep } from "@tauri-apps/api/path";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { saveSettings, settings } from "./settings.svelte";
  import { ICONS, type Icon } from "./icons";
  import { inBrowser } from "./platform";

  interface Props {
    library: LibraryInfo;
    onclose: () => void;
  }
  let { library, onclose }: Props = $props();

  /** How often an open library is rescanned for changes made outside the app. */
  const RESCAN_INTERVAL_MS = 15 * 60 * 1000;
  /** How often an offline library is tried again. */
  const OFFLINE_RETRY_MS = 30 * 1000;
  /** How often changes made on other computers are picked up. */
  const SYNC_INTERVAL_MS = 60 * 1000;
  /** How often the grid picks up newly indexed photos during a long first scan. */
  const LIVE_REFRESH_MS = 3000;

  // --- Scan and connection status ------------------------------------------

  let photoTotal = $state<number | null>(null);
  let scanning = $state(true); // opening a library starts a scan
  let progress = $state<ScanProgress | null>(null);
  let lastScan = $state<ScanSummary | null>(null);
  let lastThumbs = $state<ThumbSummary | null>(null);
  let error = $state<string | null>(null);
  // Initial values only: this component is re-created for another library.
  // svelte-ignore state_referenced_locally
  let online = $state(library.online);
  // svelte-ignore state_referenced_locally
  let writable = $state(library.writable);
  let lastLiveRefresh = 0;
  /** `progress.done` at the last live refresh, so a scan with nothing new does not reload. */
  let liveRefreshDone = 0;
  let lastScanStart = Date.now();

  // --- Import from a memory card, camera or folder -------------------------------

  /** The import window, with the drive it imports from if one was chosen. */
  let deviceImport = $state<{ source: ImportSource | null } | null>(null);
  /** Drives with photos connected right now, offered in a banner. */
  let connected = $state<ImportSource[]>([]);
  /** Drives whose banner was closed; offered again once reconnected. */
  const dismissedSources = new SvelteSet<string>();
  const offeredSource = $derived(connected.find((source) => !dismissedSources.has(source.path)) ?? null);

  async function lookForDevices() {
    if (!online || !writable) return;
    try {
      const found = await importSources();
      for (const path of dismissedSources) {
        if (!found.some((source) => source.path === path)) dismissedSources.delete(path);
      }
      connected = found;
    } catch {
      // Tried again in a moment.
    }
  }

  onMount(() => {
    lookForDevices();
    const deviceTimer = setInterval(lookForDevices, 5000);
    return () => clearInterval(deviceTimer);
  });

  onMount(() => {
    const unlisten = onIndexEvent(handleIndexEvent);
    const scanTimer = setInterval(() => {
      const due = online ? RESCAN_INTERVAL_MS : OFFLINE_RETRY_MS;
      if (!scanning && Date.now() - lastScanStart >= due) rescan();
    }, 5000);
    const syncTimer = setInterval(async () => {
      if (!online) return;
      try {
        if (await syncLibrary(library.path)) refreshAll();
      } catch {
        // Not reachable this time; the next round tries again.
      }
    }, SYNC_INTERVAL_MS);

    // What the index already holds from an earlier session shows at once.
    libraryStats(library.path)
      .then((stats) => {
        photoTotal ??= stats.photoCount;
        // The scan may have finished before we were listening.
        if (!stats.scanning && !progress) scanning = false;
      })
      .catch((e) => (error = errorText(e)));
    loadSidebar();

    return () => {
      clearInterval(scanTimer);
      clearInterval(syncTimer);
      unlisten.then((stop) => stop());
      closeLibrary(library.path).catch(() => {});
    };
  });

  function handleIndexEvent(event: IndexEvent) {
    if (event.libraryPath !== library.path) return;
    switch (event.state) {
      case "running": {
        scanning = true;
        progress = event.progress;
        // New photos are written while the folders are still being walked,
        // so show them as they come instead of waiting for the walk to end.
        const { phase, done } = event.progress;
        const now = Date.now();
        if (
          (phase === "scanning" || phase === "indexing") &&
          done > liveRefreshDone &&
          now - lastLiveRefresh > LIVE_REFRESH_MS
        ) {
          lastLiveRefresh = now;
          liveRefreshDone = done;
          refreshAll();
        }
        break;
      }
      case "online":
        // Sent at the start of every scan.
        liveRefreshDone = 0;
        online = true;
        writable = event.writable;
        break;
      case "offline":
        online = false;
        scanning = false;
        progress = null;
        break;
      case "synced":
        refreshAll();
        break;
      case "indexed":
        // The index is complete; thumbnails are still being made.
        lastScan = event.summary;
        photoTotal = event.summary.total;
        refreshAll();
        break;
      case "finished":
        scanning = false;
        progress = null;
        lastScan = event.summary;
        lastThumbs = event.thumbnails;
        photoTotal = event.summary.total;
        // Thumbnail making may have found files that are not images after all.
        if (event.thumbnails.undecodable > 0) refreshAll();
        break;
      case "failed":
        scanning = false;
        progress = null;
        error = errorText(event.message);
        break;
    }
  }

  let scanMenuOpen = $state(false);

  async function rescan(everything = false) {
    scanMenuOpen = false;
    error = null;
    scanning = true;
    lastScanStart = Date.now();
    try {
      await rescanLibrary(library.path, everything);
    } catch (e) {
      scanning = false;
      error = errorText(e);
    }
  }

  const status = $derived.by(() => {
    if (scanning) {
      if (progress?.phase === "indexing" && progress.total > 0) {
        return t("Indexing {done} of {total}", { done: progress.done, total: progress.total });
      }
      if (progress?.phase === "faces") {
        return t("Finding faces: {done} of {total}", { done: progress.done, total: progress.total });
      }
      if (progress?.phase === "comparing") {
        return t("Comparing photos: {done} of {total}", { done: progress.done, total: progress.total });
      }
      if (progress?.phase === "thumbnails") {
        return t("Making thumbnails: {done} of {total}", { done: progress.done, total: progress.total });
      }
      if (progress?.phase === "scanning" && progress.known > 0) {
        // A library seen before: only new and changed files are read.
        return t("Checking for new photos: {done} of {total}", {
          done: Math.min(progress.discovered, progress.known),
          total: progress.known,
        });
      }
      return progress ? t("Scanning… {n} found", { n: progress.discovered }) : t("Scanning…");
    }
    return photoTotal === null ? "" : photoCount(photoTotal);
  });

  const unreadable = $derived((lastScan?.failed ?? 0) + (lastThumbs?.unreadable ?? 0));

  // --- Sidebar: what can be listed ------------------------------------------

  let albums = $state<Album[]>([]);
  let folders = $state<FolderEntry[]>([]);
  // Raw (not a deep proxy): it is passed to the backend as is.
  let listing = $state.raw<Listing>({ kind: "library" });
  /** What the main area shows: a photo grid, the map or the people. */
  let section = $state<"photos" | "collections" | "map" | "people" | "duplicates" | "similar">("photos");
  /** The title of a listing opened from Collections (a trip, a day…). */
  let listingTitle = $state<string | null>(null);
  /** The listing was opened from Collections, which its back button returns to. */
  let fromCollections = $state(false);
  /** The memory playing full screen. */
  let playing = $state<Memory | null>(null);
  /** On a phone: the sidebar, shown as a drawer. */
  let drawerOpen = $state(false);
  /** The memory being saved as a video. */
  let exportingMemory = $state<Memory | null>(null);

  /** A photo to open in the viewer once the listing opened from Collections is loaded. */
  let openingPhoto: string | null = null;

  /** Opens a listing from the Collections page, on one of its photos when `photo` is given. */
  function openFromCollections(target: Listing, name: string, photo?: string) {
    listing = target;
    listingTitle = name;
    section = "photos";
    fromCollections = true;
    openingPhoto = photo ?? null;
  }
  /** The name of the person whose photos are listed. */
  let personName = $state<string | null>(null);
  let personHidden = $state(false);

  async function loadSidebar() {
    try {
      [albums, folders] = await Promise.all([listAlbums(library.path), listFolders(library.path)]);
      void loadPeople();
      // The album or folder being shown may be gone (deleted on another computer).
      const gone =
        (listing.kind === "album" && !albums.some((a) => a.id === (listing as { album: string }).album)) ||
        (listing.kind === "folder" && !folderTree.paths.has((listing as { folder: string }).folder));
      if (gone) listing = { kind: "library" };
    } catch (e) {
      error = errorText(e);
    }
  }

  // --- Sidebar sections ---------------------------------------------------------

  const readStored = <T,>(key: string, fallback: T): T => {
    try {
      const stored = localStorage.getItem(key);
      return stored === null ? fallback : (JSON.parse(stored) as T);
    } catch {
      return fallback;
    }
  };
  const store = (key: string, value: unknown) => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Not remembered this time.
    }
  };

  /** The tip on a person's page about taking photos out, until dismissed. */
  let personHintSeen = $state(readStored("photolib.personHintSeen", false));
  function dismissPersonHint() {
    personHintSeen = true;
    store("photolib.personHintSeen", true);
  }

  /** Which sidebar headings are open. Media Types and Folders start closed. */
  type SidebarGroup = "pinned" | "albums" | "media" | "utilities" | "folders";
  let openGroups = $state<Record<SidebarGroup, boolean>>({
    pinned: true,
    albums: true,
    media: false,
    utilities: true,
    folders: false,
    ...readStored<Partial<Record<SidebarGroup, boolean>>>("photolib.sidebarGroups", {}),
  });
  function toggleGroup(group: SidebarGroup) {
    openGroups[group] = !openGroups[group];
    store("photolib.sidebarGroups", openGroups);
  }

  const mediaListing = (only: Only): Listing => ({ kind: "only", within: { kind: "library" }, only });
  const sameListing = (a: Listing, b: Listing) => JSON.stringify(a) === JSON.stringify(b);

  /**
   * People, places and other listings pinned from People, the Map or a
   * listing's toolbar, by their sidebar id; kept per computer. A person is
   * kept by name: their face groups change as faces are grouped again.
   */
  interface PinnedListing {
    label: string;
    icon: Icon;
    listing: Listing;
    person?: string;
  }
  let pinnedListings = $state<Record<string, PinnedListing>>(readStored("photolib.pinnedListings", {}));
  /** The named people, to find a pinned person's face groups. */
  let namedPeople = $state.raw<Person[]>([]);
  /** The names of the favourite people (the heart on their card); kept per computer. */
  let favoritePeople = $state<string[]>(readStored("photolib.favoritePeople", []));
  function toggleFavoritePerson(name: string) {
    favoritePeople = favoritePeople.includes(name)
      ? favoritePeople.filter((other) => other !== name)
      : [...favoritePeople, name];
    store("photolib.favoritePeople", favoritePeople);
  }
  async function loadPeople() {
    try {
      namedPeople = (await listPeople(library.path)).filter((person) => person.name);
    } catch {
      // Faces not searched on this computer: pinned people stay as they were.
    }
  }

  /** A row of the sidebar that can be pinned: an album, a filter or a view. */
  interface SidebarItem {
    id: string;
    label: string;
    icon: Icon;
    count: number | null;
    selected: boolean;
    /** What it lists, if it is a listing. */
    target: Listing | null;
    open: () => void;
  }
  const sidebarItems = $derived.by(() => {
    const ofListing = (id: string, target: Listing, label: string, icon: Icon, count: number | null = null) => ({
      id,
      label,
      icon,
      count,
      selected: section === "photos" && sameListing(target, listing),
      target,
      open: () => {
        listing = target;
        section = "photos";
      },
    });
    const ofSection = (id: typeof section, label: string, icon: Icon, alsoSelected = false) => ({
      id,
      label,
      icon,
      count: null,
      selected: section === id || alsoSelected,
      target: null,
      open: () => (section = id),
    });
    const items: SidebarItem[] = [
      ofListing("favorites", { kind: "favorites" }, t("Favorites"), "heart"),
      ofListing("hidden", { kind: "hidden" }, t("Hidden"), "hidden"),
      ofSection("map", t("Map"), "map", section === "photos" && listing.kind === "area"),
      ofSection("people", t("People"), "person", section === "photos" && listing.kind === "person"),
      ofSection("duplicates", t("Duplicates"), "duplicates"),
      ofSection("similar", t("Similar shots"), "similar"),
      ofListing("videos", mediaListing("videos"), t("Videos"), "video"),
      ...MEDIA_KINDS.map(([only, name, icon]) => ofListing(only, mediaListing(only), name, icon)),
      ofListing("raw", mediaListing("raw"), t("RAW"), "raw"),
      ...albums.map((album) =>
        ofListing(`album:${album.id}`, { kind: "album", album: album.id }, album.name, "album", album.count),
      ),
      ...Object.entries(pinnedListings).map(([id, pin]) => {
        if (pin.person === undefined) return ofListing(id, pin.listing, pin.label, pin.icon);
        const name = pin.person;
        const person = namedPeople.find((candidate) => candidate.name === name);
        const target: Listing = person ? { kind: "person", clusters: person.clusters } : pin.listing;
        return {
          ...ofListing(id, target, name, "person"),
          open: () => {
            personName = name;
            personHidden = person?.hidden ?? false;
            listing = target;
            section = "photos";
          },
        };
      }),
    ];
    return new Map(items.map((item) => [item.id, item]));
  });
  /** The media types between Videos and RAW, in Apple Photos' order. */
  const MEDIA_KINDS: [Only, string, Icon][] = [
    ["selfies", t("Selfies"), "selfie"],
    ["livePhotos", t("Live Photos"), "live"],
    ["portraits", t("Portrait"), "portrait"],
    ["panoramas", t("Panoramas"), "panorama"],
    ["timeLapses", t("Time-lapse"), "timeLapse"],
    ["sloMos", t("Slo-mo"), "sloMo"],
    ["bursts", t("Bursts"), "burst"],
    ["screenshots", t("Screenshots"), "screenshot"],
  ];
  const MEDIA_TYPES = ["videos", ...MEDIA_KINDS.map(([only]) => only), "raw"];
  const UTILITIES = ["favorites", "hidden", "map", "people", "duplicates", "similar"];

  /** The rows at the top of the sidebar, chosen with Pin and Unpin; kept per computer. */
  let pinned = $state<string[]>(readStored("photolib.pinned", ["favorites", "map", "people", "videos"]));
  const pinnedItems = $derived(pinned.flatMap((id) => sidebarItems.get(id) ?? []));
  /**
   * A pinned row is in the sidebar twice. Only the copy that was clicked is
   * shown selected: "pinned", or "home" for its own section (Utilities…).
   */
  let clickedIn = $state<"pinned" | "home">("pinned");
  const showsSelected = (entry: SidebarItem, place: "pinned" | "home") =>
    entry.selected &&
    (place === (clickedIn === "pinned" && openGroups.pinned ? "pinned" : "home") || !pinned.includes(entry.id));
  function togglePin(id: string) {
    const unpinning = pinned.includes(id);
    pinned = unpinning ? pinned.filter((other) => other !== id) : [...pinned, id];
    store("photolib.pinned", pinned);
    if (unpinning && id in pinnedListings) {
      const { [id]: _, ...rest } = pinnedListings;
      pinnedListings = rest;
      store("photolib.pinnedListings", pinnedListings);
    }
  }
  /** Pins a person, a place or another listing, or unpins it. */
  function togglePinListing(id: string, pin: PinnedListing) {
    if (!pinned.includes(id)) {
      pinnedListings = { ...pinnedListings, [id]: pin };
      store("photolib.pinnedListings", pinnedListings);
    }
    togglePin(id);
  }
  const personPinId = (name: string) => `person:${name}`;
  function togglePinPerson(person: Person) {
    if (!person.name) return;
    togglePinListing(personPinId(person.name), {
      label: person.name,
      icon: "person",
      listing: { kind: "person", clusters: person.clusters },
      person: person.name,
    });
  }
  /** Pins a part of the map, named after the town most of its photos were taken in. */
  async function togglePinArea(area: Listing & { kind: "area" }) {
    const id = `area:${[area.south, area.west, area.north, area.east].map((value) => value.toFixed(5)).join(",")}`;
    if (pinned.includes(id)) return togglePin(id);
    let place: string | null = null;
    try {
      place = await listingPlace(library.path, area);
    } catch {
      // Named "Photos from the map" instead.
    }
    togglePinListing(id, { label: place?.split(",")[0].trim() || t("Photos from the map"), icon: "map", listing: area });
  }
  /** What the toolbar's Pin button pins for the listing shown, if anything. */
  const listingPin = $derived.by((): { id: string; pin: () => void } | null => {
    const shown = listing;
    switch (shown.kind) {
      case "person":
        if (!personName) return null;
        return {
          id: personPinId(personName),
          pin: () => togglePinPerson({ name: personName, clusters: shown.clusters } as Person),
        };
      case "area": {
        const id = `area:${[shown.south, shown.west, shown.north, shown.east].map((value) => value.toFixed(5)).join(",")}`;
        return { id, pin: () => void togglePinArea(shown) };
      }
      case "days": {
        const id = `days:${shown.from}-${shown.to}`;
        const label = listingTitle ?? formatDayRange(shown.from, shown.to);
        return { id, pin: () => togglePinListing(id, { label, icon: "photo", listing: shown }) };
      }
      case "folder": {
        const id = `folder:${shown.folder}`;
        const label = shown.folder.split("/").pop() || t("Top level");
        return { id, pin: () => togglePinListing(id, { label, icon: "folder", listing: shown }) };
      }
      case "search": {
        const id = `search:${shown.query}`;
        const label = shown.query;
        return { id, pin: () => togglePinListing(id, { label, icon: "photo", listing: shown }) };
      }
      default:
        return null;
    }
  });
  const pinEntry = (id: string): MenuEntry => ({
    label: pinned.includes(id) ? t("Unpin") : t("Pin to Sidebar"),
    action: () => togglePin(id),
  });
  function itemMenu(event: MouseEvent, item: SidebarItem) {
    const album = albums.find((candidate) => `album:${candidate.id}` === item.id);
    if (album) return albumMenu(event, album);
    showMenu(event, [{ label: t("Open"), action: item.open }, "-", pinEntry(item.id)]);
  }

  /** Folders whose subfolders are shown in the sidebar, kept per library. */
  // svelte-ignore state_referenced_locally
  const openFoldersKey = `photolib.openFolders.${library.path}`;
  let openFolders = $state<Set<string>>(
    (() => {
      try {
        return new Set(JSON.parse(localStorage.getItem(openFoldersKey) ?? "[]") as string[]);
      } catch {
        return new Set<string>();
      }
    })(),
  );
  function toggleFolder(path: string) {
    const next = new Set(openFolders);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    openFolders = next;
    localStorage.setItem(openFoldersKey, JSON.stringify([...next]));
  }

  /**
   * Folder rows for the sidebar, as a tree: every folder with photos plus its
   * ancestors. A folder lists its subfolders too, so its count includes
   * theirs. Rows under a closed folder are left out.
   */
  const folderTree = $derived.by(() => {
    const own = new Map(folders.map((f) => [f.folder, f.count]));
    const totals = new Map<string, number>();
    const parents = new Set<string>();
    for (const { folder, count } of folders) {
      if (folder === "") continue;
      const parts = folder.split("/");
      for (let depth = 1; depth <= parts.length; depth++) {
        const path = parts.slice(0, depth).join("/");
        totals.set(path, (totals.get(path) ?? 0) + count);
        if (depth < parts.length) parents.add(path);
      }
    }
    const all = [...totals.keys()].sort((a, b) => {
      const partsA = a.split("/");
      const partsB = b.split("/");
      for (let i = 0; i < Math.min(partsA.length, partsB.length); i++) {
        const order = partsA[i].localeCompare(partsB[i], undefined, { numeric: true });
        if (order !== 0) return order;
      }
      return partsA.length - partsB.length;
    });
    const rows: {
      path: string;
      name: string;
      depth: number;
      count: number;
      hasChildren: boolean;
    }[] = [];
    if (own.has("")) {
      rows.push({ path: "", name: t("Top level"), depth: 0, count: own.get("") ?? 0, hasChildren: false });
    }
    for (const path of all) {
      const parts = path.split("/");
      // Shown only if every folder above it is open.
      const hidden = parts.slice(0, -1).some((_, i) => !openFolders.has(parts.slice(0, i + 1).join("/")));
      if (hidden) continue;
      rows.push({
        path,
        name: parts[parts.length - 1],
        depth: parts.length - 1,
        count: totals.get(path) ?? 0,
        hasChildren: parents.has(path),
      });
    }
    return { rows, paths: new Set(["", ...totals.keys()]) };
  });

  /** The listing for a folder row: the folder with its subfolders. */
  const folderListing = (path: string): Listing =>
    path === "" ? { kind: "folder", folder: "" } : { kind: "folder", folder: path, subfolders: true };

  const currentAlbum = $derived(
    listing.kind === "album" ? (albums.find((a) => a.id === (listing as { album: string }).album) ?? null) : null,
  );

  const title = $derived.by(() => {
    switch (listing.kind) {
      case "library":
        return t("Library");
      case "favorites":
        return t("Favorites");
      case "hidden":
        return t("Hidden");
      case "duplicates":
        return t("Duplicates");
      case "album":
        return currentAlbum?.name ?? t("Album");
      case "folder":
        return listing.folder || t("Top level");
      case "area":
        return t("Photos from the map");
      case "person":
        return personName ?? t("Unnamed person");
      case "search":
        return t("Search: {query}", { query: listing.query });
      case "only":
        return filterName(listing.only);
      case "days":
        return listingTitle ?? formatDayRange(listing.from, listing.to);
      case "picked":
        return listingTitle ?? t("Photos");
    }
  });

  // --- Import from Apple Photos ---------------------------------------------------

  /** Apple Photos libraries exist only on a Mac. */
  const IS_MAC = navigator.userAgent.includes("Mac");

  /** The import being confirmed: which library, and what it would do. */
  let importing = $state<{ library: string; summary: ImportSummary; busy: boolean } | null>(null);
  /** What the last import did, shown once. */
  let imported = $state<ImportSummary | null>(null);

  async function chooseAppleLibrary() {
    error = null;
    imported = null;
    try {
      const chosen = await pickPhotosLibrary();
      if (!chosen) return;
      importing = {
        library: chosen,
        summary: await appleImportPreview(library.path, chosen),
        busy: false,
      };
    } catch (e) {
      error = errorText(e);
    }
  }

  async function confirmImport() {
    if (!importing) return;
    importing.busy = true;
    try {
      imported = await appleImport(library.path, importing.library);
      refreshAll();
    } catch (e) {
      error = errorText(e);
    }
    importing = null;
  }

  // --- Search -------------------------------------------------------------------

  let searchText = $state("");
  /** What was listed before the search, to go back to when it is cleared. */
  let beforeSearch: Listing = { kind: "library" };
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  /** Lists the matches a moment after the user stops typing. */
  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      const query = searchText.trim();
      if (query === "") {
        if (listing.kind === "search") listing = beforeSearch;
        return;
      }
      if (listing.kind !== "search") beforeSearch = listing;
      listing = { kind: "search", query };
      section = "photos";
    }, 250);
  }

  function clearSearch() {
    clearTimeout(searchTimer);
    searchText = "";
    if (listing.kind === "search") listing = beforeSearch;
  }

  // Choosing something in the sidebar ends the search.
  $effect(() => {
    if (listing.kind !== "search") searchText = "";
  });

  const emptyText = $derived.by(() => {
    if (only) return t("Nothing here matches the filter.");
    switch (listing.kind) {
      case "favorites":
        return t("No favorites yet. Open a photo and press the heart, or select photos in the grid.");
      case "hidden":
        return t("No hidden photos.");
      case "duplicates":
        return t("No duplicates: every photo is stored once.");
      case "album":
        return t("This album is empty. Select photos in the grid and choose “Add to album”.");
      case "search":
        return t("Nothing matches. Try a place, a camera, a folder or file name, a year (2024), a month (july) or “video”.");
      default:
        return scanning ? t("Looking for photos…") : t("No photos found in this folder.");
    }
  });

  // --- The listing on screen ---------------------------------------------------

  /**
   * The filter menu's choice. It belongs to the listing it was chosen for,
   * so opening another album or folder shows all of it again.
   */
  let filter = $state.raw<{ listing: Listing; only: Only } | null>(null);
  const only = $derived(filter?.listing === listing ? filter.only : null);
  let filterMenuOpen = $state(false);
  const FILTERS: [Only | null, string, Icon][] = [
    [null, t("All Items"), "grid"],
    ["favorites", t("Favorites"), "heart"],
    ["edited", t("Edited"), "sliders"],
    ["photos", t("Photos"), "photo"],
    ["videos", t("Videos"), "video"],
    ["notInAlbum", t("Not in an Album"), "albumOff"],
  ];
  /** The filter's name, also for the stars and colours, which are not in FILTERS. */
  function filterName(value: Only): string {
    if (value === "raw") return t("RAW");
    const kind = MEDIA_KINDS.find(([only]) => only === value);
    if (kind) return kind[1];
    const listed = FILTERS.find(([choice]) => choice === value)?.[1];
    if (listed) return listed;
    const stars = /^stars([1-5])$/.exec(value);
    if (stars) return "★".repeat(Number(stars[1])) + (stars[1] === "5" ? "" : " " + t("or more"));
    return LABELS.find((label) => label.color === value)?.name ?? value;
  }

  function chooseFilter(choice: Only | null) {
    filter = choice === null ? null : { listing, only: choice };
    filterMenuOpen = false;
  }

  /** The grid's order, a setting shared by every library. */
  const order = $derived(settings.order);
  let sortMenuOpen = $state(false);
  const byName = $derived(order === "nameAsc" || order === "nameDesc");
  const ascending = $derived(order === "dateAsc" || order === "nameAsc");
  function chooseSort(by: "date" | "name" | null, up: boolean | null) {
    const name = by === null ? byName : by === "name";
    const asc = up === null ? ascending : up;
    settings.order = name ? (asc ? "nameAsc" : "nameDesc") : asc ? "dateAsc" : "dateDesc";
    saveSettings();
    sortMenuOpen = false;
  }

  /** The camera filter's choice; like the filter menu's, it belongs to its listing. */
  let gearChoice = $state.raw<{ listing: Listing; gear: Gear } | null>(null);
  const gear = $derived<Gear>(gearChoice?.listing === listing ? gearChoice.gear : {});
  const gearActive = $derived(
    !!(gear.cameraMake || gear.cameraModel || gear.lens) || gear.aperture != null || gear.focalLength != null,
  );
  let gearOpen = $state(false);
  /** What the camera filter narrows down: the listing with the filter menu's choice. */
  const filtered = $derived<Listing>(only ? { kind: "only", within: listing, only } : listing);
  /** "f/2.8 · 35 mm" for the button's tooltip. */
  const gearSummary = $derived(
    [
      gear.cameraMake || gear.cameraModel ? cameraName(gear.cameraMake, gear.cameraModel) : null,
      gear.lens,
      gear.aperture != null ? `f/${formatNumber(gear.aperture)}` : null,
      gear.focalLength != null ? `${gear.focalLength} mm` : null,
    ]
      .filter(Boolean)
      .join(" · "),
  );

  const pages = $derived(
    new PhotoPages(library.path, gearActive ? { kind: "gear", within: filtered, ...gear } : filtered, order),
  );
  let groups = $state<DayGroup[]>([]);
  /** False until the first timeline for the current listing has arrived. */
  let groupsLoaded = $state(false);
  const listedTotal = $derived(groups.reduce((sum, group) => sum + group.count, 0));
  const listedVideos = $derived(groups.reduce((sum, group) => sum + group.videos, 0));
  /** Bumped whenever the listing is reloaded. */
  let revision = $state(0);

  // --- How the listing is shown ----------------------------------------------

  /** Reads a number the user set earlier (a size), within its limits. */
  function remembered(key: string, fallback: number, min: number, max: number): number {
    const stored = Number(localStorage.getItem(key));
    return stored >= min && stored <= max ? stored : fallback;
  }

  const CELL_MIN = 70;
  const CELL_MAX = 360;
  /** The width the grid's cells aim for: the zoom level. */
  // About four photos across on a phone, as in the Photos app.
  let cellSize = $state(
    remembered("photolib.cellSize", innerWidth <= 700 ? Math.max(CELL_MIN, Math.floor(innerWidth / 4.5)) : 150, CELL_MIN, CELL_MAX),
  );
  $effect(() => localStorage.setItem("photolib.cellSize", String(cellSize)));
  /** The days on screen in the grid, shown under the title. */
  let shownRange = $state<{ oldest: number; newest: number } | null>(null);
  function setShownRange(range: { oldest: number; newest: number } | null) {
    // Called on every scroll: only a real change re-renders the toolbar.
    if (range?.oldest !== shownRange?.oldest || range?.newest !== shownRange?.newest) {
      shownRange = range;
    }
  }

  /** Square thumbnails, or rows of photos at their own shape ("river"). */
  let gridFit = $state<"square" | "river">(
    // "whole" was the name of an earlier, letterboxed version of the river.
    ["river", "whole"].includes(localStorage.getItem("photolib.gridFit") ?? "") ? "river" : "square",
  );
  $effect(() => localStorage.setItem("photolib.gridFit", gridFit));
  /** Every photo's shape, for the river layout; loaded only for it. */
  let gridShapes = $state.raw<number[] | null>(null);
  async function loadShapes(target: PhotoPages) {
    try {
      const loaded = await shapes(target.libraryPath, target.listing, target.order);
      if (target === pages) gridShapes = loaded;
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    if (gridFit === "river") loadShapes(pages);
    else gridShapes = null;
  });

  const SIDEBAR_MIN = 150;
  const SIDEBAR_MAX = 600;
  let sidebarWidth = $state(remembered("photolib.sidebarWidth", 220, SIDEBAR_MIN, SIDEBAR_MAX));
  $effect(() => localStorage.setItem("photolib.sidebarWidth", String(sidebarWidth)));

  let resizing: { pointerId: number; startX: number; startWidth: number } | null = null;
  function startResize(event: PointerEvent) {
    resizing = { pointerId: event.pointerId, startX: event.clientX, startWidth: sidebarWidth };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  function resize(event: PointerEvent) {
    if (resizing?.pointerId !== event.pointerId) return;
    const wanted = resizing.startWidth + event.clientX - resizing.startX;
    sidebarWidth = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, wanted));
  }
  function resizeWithKeys(event: KeyboardEvent) {
    const step = event.key === "ArrowLeft" ? -16 : event.key === "ArrowRight" ? 16 : 0;
    if (step === 0) return;
    event.preventDefault();
    sidebarWidth = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, sidebarWidth + step));
  }

  /** Years and months are overviews; "all" is the photo grid. */
  let viewMode = $state<"years" | "months" | "all">("all");
  let months = $state<MonthSummary[]>([]);
  /** Where the next view starts: a year for the months, a day for the grid. */
  let startYear = $state<number | null>(null);
  let startDay = $state<number | null>(null);

  async function loadMonths(target: PhotoPages) {
    try {
      const loaded = await listMonths(target.libraryPath, target.listing);
      if (target === pages) months = loaded;
    } catch (e) {
      error = errorText(e);
    }
  }

  // The overviews need the months; the grid does not.
  $effect(() => {
    if (viewMode !== "all") loadMonths(pages);
  });

  function setViewMode(mode: "years" | "months" | "all") {
    startYear = null;
    startDay = null;
    viewMode = mode;
  }

  let showShortcuts = $state(false);
  let searchInput = $state<HTMLInputElement>();

  const typing = (target: EventTarget | null) =>
    target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement;

  /** Gives the selection `on` for a yes/no op; if they all have it already, takes it away. */
  async function toggleSelection(kind: "fav" | "hide") {
    if (!online) return;
    const target = pages;
    const photos = (await Promise.all([...selected].map((index) => target.load(index)))).flatMap((p) => (p ? [p] : []));
    if (target !== pages || photos.length === 0) return;
    const on = kind === "fav" ? !photos.every((photo) => photo.favorite) : listing.kind !== "hidden";
    await apply(photos.map((photo) => ({ op: kind, photo: photo.fingerprint, on })));
  }

  /** The grid's keys, the "?" panel, and Ctrl/Cmd with + or - to zoom like in a browser. */
  function onkeydown(event: KeyboardEvent) {
    if (playing) return;
    if (event.key === "?" && !typing(event.target) && !slideshow) {
      event.preventDefault();
      showShortcuts = true;
      return;
    }
    const mod = event.ctrlKey || event.metaKey;
    const inGrid = section === "photos" && viewerIndex === null && !slideshow;
    if (inGrid && mod && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "f") {
      event.preventDefault();
      searchInput?.focus();
      return;
    }
    if (inGrid && !typing(event.target)) {
      if (event.key === "/" && !mod) {
        event.preventDefault();
        searchInput?.focus();
        return;
      }
      if (mod && !event.altKey && event.key.toLowerCase() === "a") {
        event.preventDefault();
        for (let index = 0; index < listedTotal; index++) selected.add(index);
        return;
      }
      if (mod && !event.altKey && event.key.toLowerCase() === "e" && selected.size > 0) {
        event.preventDefault();
        exportSelection();
        return;
      }
      if (!mod && !event.altKey && selected.size > 0) {
        if (event.key === "Escape" && !(filterMenuOpen || sortMenuOpen || scanMenuOpen || gearOpen)) {
          event.preventDefault();
          selected.clear();
          return;
        }
        if (event.key === "Enter") {
          event.preventDefault();
          viewerIndex = Math.min(...selected);
          return;
        }
        if (event.key === ".") {
          event.preventDefault();
          toggleSelection("fav");
          return;
        }
        if (event.key.toLowerCase() === "h") {
          event.preventDefault();
          toggleSelection("hide");
          return;
        }
      }
    }
    if (event.key === "F5" && !slideshow && viewerIndex === null && section === "photos") {
      event.preventDefault();
      startSlideshow();
      return;
    }
    if (event.key === "Escape" && (filterMenuOpen || sortMenuOpen || scanMenuOpen || gearOpen)) {
      gearOpen = false;
      filterMenuOpen = false;
      sortMenuOpen = false;
      scanMenuOpen = false;
      return;
    }
    if (
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      viewerIndex === null &&
      section === "photos" &&
      selected.size > 0 &&
      !typing(event.target)
    ) {
      if (/^[1-5]$/.test(event.key)) {
        event.preventDefault();
        rateSelection(Number(event.key));
        return;
      }
      const color = labelForKey(event.key);
      if (color) {
        event.preventDefault();
        labelSelection(color);
        return;
      }
    }
    if (!(event.ctrlKey || event.metaKey) || viewerIndex !== null) return;
    const factor = event.key === "+" || event.key === "=" ? 1.25 : event.key === "-" ? 0.8 : 0;
    if (factor === 0) return;
    event.preventDefault();
    cellSize = Math.round(Math.min(CELL_MAX, Math.max(CELL_MIN, cellSize * factor)));
  }

  let grid = $state<PhotoGrid>();
  let viewerIndex = $state<number | null>(null);
  const selected = new SvelteSet<number>();

  /** The running slideshow: the selected photos, or else the whole listing. */
  let slideshow = $state<{ count: number; indexAt: (position: number) => number } | null>(null);
  function startSlideshow() {
    if (listedTotal === 0) return;
    if (selected.size > 0) {
      const order = [...selected].sort((a, b) => a - b);
      slideshow = { count: order.length, indexAt: (position) => order[position] };
    } else {
      slideshow = { count: listedTotal, indexAt: (position) => position };
    }
  }
  /** A slideshow of one month of the listing: `day` is any day in it. */
  function slideshowOfMonth(day: number) {
    const key = monthKey(day);
    let index = 0;
    let first = -1;
    let count = 0;
    for (const group of groups) {
      if (group.day !== null && monthKey(group.day) === key) {
        if (first < 0) first = index;
        count += group.count;
      } else if (first >= 0) {
        break; // a month's days are next to each other
      }
      index += group.count;
    }
    if (count > 0) slideshow = { count, indexAt: (position) => first + position };
  }
  /** Changes when face grouping is switched on or off in Settings. */
  let facesRevision = $state(0);
  let selectionAnchor: number | null = null;

  // Start afresh whenever something else is listed.
  $effect(() => {
    const target = pages;
    // Only a change of `pages` may re-run this, not the state it resets.
    untrack(() => {
      groups = [];
      months = [];
      startYear = null;
      startDay = null;
      groupsLoaded = false;
      viewerIndex = null;
      selected.clear();
      selectionAnchor = null;
      cancelAlbumEdits();
      loadGroups(target);
    });
  });

  // When photos leave the listing under the viewer (hidden, removed from the
  // album), stay on a valid photo, or close when none is left.
  $effect(() => {
    if (viewerIndex === null || !groupsLoaded) return;
    if (listedTotal === 0) viewerIndex = null;
    else if (viewerIndex >= listedTotal) viewerIndex = listedTotal - 1;
  });

  async function loadGroups(target: PhotoPages) {
    try {
      const loaded = await timeline(target.libraryPath, target.listing, target.order);
      // The river's shapes must match the listing, so they come along.
      if (gridFit === "river") await loadShapes(target);
      if (target !== pages) return; // something else is listed by now
      groups = loaded;
      groupsLoaded = true;
      if (openingPhoto !== null) await openOn(target, openingPhoto);
    } catch (e) {
      error = errorText(e);
    }
  }

  /** Opens the viewer on the photo `fingerprint` of the listing (looked for among its first photos). */
  async function openOn(target: PhotoPages, fingerprint: string) {
    openingPhoto = null;
    const total = Math.min(listedTotal, 1000);
    for (let index = 0; index < total; index++) {
      const photo = await target.load(index);
      if (target !== pages) return;
      if (photo?.fingerprint === fingerprint) {
        viewerIndex = index;
        return;
      }
    }
  }

  /** Re-reads everything on screen after the index or the organization changed. */
  function refreshAll() {
    pages.refresh();
    revision++;
    // Indexes shift when photos come and go, so a selection can't be kept.
    selected.clear();
    selectionAnchor = null;
    loadGroups(pages);
    if (viewMode !== "all") loadMonths(pages);
    loadSidebar();
  }

  function closeViewer() {
    const index = viewerIndex;
    viewerIndex = null;
    if (index !== null) grid?.reveal(index);
  }

  // --- Organizing ----------------------------------------------------------------

  /** Records changes, then reloads what is on screen. */
  /** Records `ops` and reloads; with no ops, only reloads (a file changed). */
  /** Whether `op` takes its photo out of the listing on screen. */
  function leavesListing(op: Op): op is Op & { photo: string } {
    switch (op.op) {
      case "hide":
        // Hiding leaves every listing but Hidden; showing again leaves Hidden.
        return op.on !== (listing.kind === "hidden");
      case "album_member":
        return !op.on && listing.kind === "album" && listing.album === op.album;
      case "fav":
        return !op.on && listing.kind === "favorites";
      default:
        return false;
    }
  }

  /** Takes photos out of the grid before the change is saved. */
  function takeOutLocally(fingerprints: Set<string>) {
    const removed = pages.removeLocally(fingerprints);
    if (removed.length === 0) return;
    const next = groups.map((group) => ({ ...group }));
    // `removed` is last first, so the earlier indexes are still right.
    for (const { index, video } of removed) {
      let start = 0;
      for (const group of next) {
        if (index < start + group.count) {
          group.count--;
          if (video) group.videos--;
          break;
        }
        start += group.count;
      }
    }
    groups = next.filter((group) => group.count > 0);
    // Indexes moved: the selection no longer points at the same photos.
    selected.clear();
    selectionAnchor = null;
    // The viewer shows what is at its index now: the next photo.
    revision++;
  }

  /** The last save asked for; the next one waits for it. */
  let saving: Promise<void> = Promise.resolve();

  async function apply(ops: Op[]) {
    if (ops.length === 0) {
      refreshAll();
      return;
    }
    error = null;
    // Hearts, stars and labels show at once; saving (to a NAS, maybe) follows.
    const patches = new Map<string, Partial<PhotoItem>>();
    const add = (photo: string, patch: Partial<PhotoItem>) =>
      patches.set(photo, { ...patches.get(photo), ...patch });
    for (const op of ops) {
      if (op.op === "fav") add(op.photo, { favorite: op.on });
      else if (op.op === "rating") add(op.photo, { rating: op.stars });
      else if (op.op === "label") add(op.photo, { label: op.color || null });
    }
    const shown = patches.size > 0 ? pages.optimistic(patches) : null;
    // Photos that leave what is listed go at once too; if saving fails, the
    // refresh below brings them back.
    const leaving = new Set(ops.flatMap((op) => (leavesListing(op) ? [op.photo] : [])));
    if (leaving.size > 0) takeOutLocally(leaving);
    // One save at a time, in order: a quick second tap must win over the first.
    const save = saving.then(() => applyOps(library.path, ops));
    saving = save.catch(() => {});
    try {
      await save;
      shown?.settle();
    } catch (e) {
      shown?.revert();
      error = errorText(e);
    }
    refreshAll();
  }

  function toggleSelected(index: number, range: boolean) {
    if (range && selectionAnchor !== null) {
      const from = Math.min(selectionAnchor, index);
      const to = Math.max(selectionAnchor, index);
      for (let i = from; i <= to; i++) selected.add(i);
    } else if (selected.has(index)) {
      selected.delete(index);
    } else {
      selected.add(index);
    }
    selectionAnchor = index;
  }

  /** Applies one change per selected photo. */
  /** Takes the selected photos out of the person listed: it wasn't them. */
  async function removeSelectionFromPerson() {
    if (listing.kind !== "person") return;
    const target = pages;
    const clusters = listing.clusters;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    const fingerprints = photos.flatMap((photo) => (photo ? [photo.fingerprint] : []));
    // They leave the grid at once; the refresh brings them back if it failed.
    takeOutLocally(new Set(fingerprints));
    try {
      await removeFromPerson(library.path, clusters, fingerprints, personName);
    } catch (e) {
      error = errorText(e);
    }
    selected.clear();
    refreshAll();
  }

  /** The one selected photo becomes the one the person's card shows. */
  async function makeSelectionKeyPhoto() {
    if (listing.kind !== "person" || personName === null || selected.size !== 1) return;
    const clusters = listing.clusters;
    const name = personName;
    const photo = await pages.load([...selected][0]);
    if (!photo) return;
    try {
      await makeKeyPhoto(library.path, clusters, name, photo.fingerprint);
    } catch (e) {
      error = errorText(e);
    }
  }

  /** The selected photos show someone else: they become a person of their own, opened to be named. */
  async function splitSelectionFromPerson() {
    if (listing.kind !== "person") return;
    const target = pages;
    const clusters = listing.clusters;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    try {
      const made = await splitPerson(
        library.path,
        clusters,
        photos.flatMap((photo) => (photo ? [photo.fingerprint] : [])),
        personName,
      );
      selected.clear();
      if (made !== null) {
        personName = null;
        personHidden = false;
        listing = { kind: "person", clusters: [made] };
      }
      refreshAll();
    } catch (e) {
      error = errorText(e);
    }
  }

  /** The fingerprints being exported, while the Export dialog is open. */
  let exporting = $state<string[] | null>(null);
  async function exportSelection() {
    const target = pages;
    const photos = await Promise.all([...selected].sort((a, b) => a - b).map((index) => target.load(index)));
    if (target !== pages) return;
    exporting = photos.flatMap((photo) => (photo ? [photo.fingerprint] : []));
  }

  async function revealSelected() {
    const [index] = selected;
    const photo = index === undefined ? null : await pages.load(index);
    if (photo) revealPhoto(library.path, photo.relPath).catch((e) => (error = errorText(e)));
  }

  async function togglePersonHidden() {
    if (listing.kind !== "person") return;
    try {
      await setPersonHidden(library.path, listing.clusters, !personHidden);
      personHidden = !personHidden;
    } catch (e) {
      error = errorText(e);
    }
  }

  async function applyToSelection(makeOp: (fingerprint: string) => Op) {
    const target = pages;
    const photos = await Promise.all([...selected].map((index) => target.load(index)));
    if (target !== pages) return;
    await apply(photos.flatMap((photo) => (photo ? [makeOp(photo.fingerprint)] : [])));
  }

  function addSelectionToAlbum(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const album = select.value;
    select.value = "";
    if (album) applyToSelection((photo) => ({ op: "album_member", album, photo, on: true }));
  }

  let newAlbumName = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let confirmingDelete = $state(false);

  function cancelAlbumEdits() {
    renaming = null;
    confirmingDelete = false;
  }

  async function submitNewAlbum(event: SubmitEvent) {
    event.preventDefault();
    const name = newAlbumName?.trim();
    newAlbumName = null;
    if (!name) return;
    try {
      const album = await createAlbum(library.path, name);
      await loadSidebar();
      listing = { kind: "album", album };
    } catch (e) {
      error = errorText(e);
    }
  }

  async function submitRename(event: SubmitEvent) {
    event.preventDefault();
    const name = renaming?.trim();
    renaming = null;
    if (name && currentAlbum && name !== currentAlbum.name) {
      await apply([{ op: "album_name", album: currentAlbum.id, name }]);
    }
  }

  async function deleteAlbum() {
    if (!currentAlbum) return;
    const album = currentAlbum.id;
    listing = { kind: "library" };
    await apply([{ op: "album_deleted", album, on: true }]);
  }

  // --- Ratings and labels -------------------------------------------------------

  /** Gives the selection `stars`; if they all have it already, takes it away. */
  async function rateSelection(stars: number) {
    if (!online) return;
    const target = pages;
    const photos = (await Promise.all([...selected].map((index) => target.load(index)))).flatMap((p) => (p ? [p] : []));
    if (target !== pages || photos.length === 0) return;
    const value = photos.every((photo) => photo.rating === stars) ? 0 : stars;
    await apply(photos.map((photo) => ({ op: "rating", photo: photo.fingerprint, stars: value })));
  }

  /** Gives the selection a label; if they all have it already, takes it away. */
  async function labelSelection(color: LabelColor | "") {
    if (!online) return;
    const target = pages;
    const photos = (await Promise.all([...selected].map((index) => target.load(index)))).flatMap((p) => (p ? [p] : []));
    if (target !== pages || photos.length === 0) return;
    const value = color && photos.every((photo) => photo.label === color) ? "" : color;
    await apply(photos.map((photo) => ({ op: "label", photo: photo.fingerprint, color: value })));
  }

  const ratingMenu = (): MenuEntry => ({
    label: t("Rating"),
    enabled: online,
    items: [
      { label: t("No Rating"), action: () => applyToSelection((photo) => ({ op: "rating", photo, stars: 0 })) },
      ...[1, 2, 3, 4, 5].map((stars) => ({
        label: `${"★".repeat(stars)}    ${stars}`,
        action: () => applyToSelection((photo) => ({ op: "rating", photo, stars })),
      })),
    ],
  });

  const labelMenu = (): MenuEntry => ({
    label: t("Label"),
    enabled: online,
    items: [
      { label: t("No Label"), action: () => applyToSelection((photo) => ({ op: "label", photo, color: "" })) },
      ...LABELS.map((label) => ({
        label: label.key ? `${label.name}    ${label.key}` : label.name,
        action: () => applyToSelection((photo) => ({ op: "label", photo, color: label.color })),
      })),
    ],
  });

  // --- Right-click menus --------------------------------------------------------

  /** A photo's menu acts on the selection; a photo outside it becomes the selection. */
  function photoMenu(index: number, event: MouseEvent) {
    if (!selected.has(index)) {
      selected.clear();
      selected.add(index);
    }
    const one = selected.size === 1;
    const fav = (on: boolean) => () => applyToSelection((photo) => ({ op: "fav", photo, on }));
    const entries: MenuEntry[] = [
      { label: t("Open"), enabled: one, action: () => (viewerIndex = index) },
      { label: one ? t("Slideshow from here") : t("Slideshow of these"), action: () => {
          if (one) {
            const total = listedTotal;
            slideshow = { count: total - index, indexAt: (position) => index + position };
          } else startSlideshow();
        } },
      "-",
      { label: t("Add to Favorites"), enabled: online, action: fav(true) },
      { label: t("Remove from Favorites"), enabled: online, action: fav(false) },
      ratingMenu(),
      labelMenu(),
      {
        label: t("Add to Album"),
        enabled: online && albums.length > 0,
        items: albums.map((album) => ({
          label: album.name,
          action: () => applyToSelection((photo) => ({ op: "album_member", album: album.id, photo, on: true })),
        })),
      },
    ];
    if (listing.kind === "album") {
      const album = listing.album;
      entries.push({
        label: t("Remove from Album"),
        enabled: online,
        action: () => applyToSelection((photo) => ({ op: "album_member", album, photo, on: false })),
      });
    }
    entries.push({
      label: listing.kind === "hidden" ? t("Unhide") : t("Hide"),
      enabled: online,
      action: () => applyToSelection((photo) => ({ op: "hide", photo, on: listing.kind !== "hidden" })),
    });
    if (listing.kind === "person") {
      entries.push(
        "-",
        { label: personName ? t("Not {name}", { name: personName }) : t("Not this person"), enabled: online || personName === null, action: removeSelectionFromPerson },
        { label: t("Someone else"), enabled: online || personName === null, action: splitSelectionFromPerson },
      );
      if (personName !== null) {
        entries.push({ label: t("Make Key Photo"), enabled: online && one, action: makeSelectionKeyPhoto });
      }
    }
    entries.push(
      "-",
      { label: REVEAL_LABEL, enabled: online && one, action: revealSelected },
      { label: t("Export…"), enabled: online, action: exportSelection },
    );
    showMenu(event, entries);
  }

  function albumMenu(event: MouseEvent, album: { id: string; name: string }) {
    const open = () => (listing = { kind: "album", album: album.id }, (section = "photos"));
    showMenu(event, [
      { label: t("Open"), action: open },
      "-",
      { label: t("Rename…"), enabled: online, action: () => (open(), (renaming = album.name)) },
      { label: t("Delete Album…"), enabled: online, action: () => (open(), (confirmingDelete = true)) },
      "-",
      pinEntry(`album:${album.id}`),
    ]);
  }

  function folderMenu(event: MouseEvent, folder: string) {
    const separator = sep();
    const root = library.path.endsWith(separator) ? library.path : library.path + separator;
    showMenu(event, [
      { label: t("Open"), action: () => ((listing = folderListing(folder)), (section = "photos")) },
      "-",
      {
        label: REVEAL_LABEL,
        enabled: online,
        action: () => revealItemInDir(root + folder.split("/").join(separator)).catch((e) => (error = errorText(e))),
      },
    ]);
  }

  /** Puts the cursor in a text field as soon as it appears. */
  function focus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<svelte:window
  {onkeydown}
  onclick={() => {
    filterMenuOpen = false;
    sortMenuOpen = false;
    scanMenuOpen = false;
    gearOpen = false;
  }}
/>

<div class="library">
  <header>
    <!-- On a phone, the sidebar is a drawer behind this button. -->
    <button
      class="drawer-button"
      onclick={() => (drawerOpen = !drawerOpen)}
      aria-expanded={drawerOpen}
      aria-label={t("Library sections")}
    >
      <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><path d="M3 5.5h14M3 10h14M3 14.5h14" /></svg>
    </button>
    {#if !inBrowser}<button class="link" onclick={onclose}>‹ Libraries</button>{/if}
    <strong class="name" title={library.path}>{library.name}</strong>
    <div class="search">
      <input
        type="search"
        bind:this={searchInput}
        bind:value={searchText}
        oninput={onSearchInput}
        onkeydown={(event) => event.key === "Escape" && clearSearch()}
        placeholder={t("Search: place, camera, name, 2024, july, video…")}
        aria-label={t("Search photos")}
      />
    </div>
    <div class="status" role="status">
      {#if scanning && progress && progress.phase !== "scanning" && progress.total > 0}
        <progress value={progress.done} max={progress.total}></progress>
      {:else if scanning && progress?.phase === "scanning" && progress.known > 0}
        <progress value={Math.min(progress.discovered, progress.known)} max={progress.known}></progress>
      {/if}
      <span>{status}</span>
    </div>
    <span class="menu-anchor scan-buttons">
      <button
        class="secondary"
        onclick={() => rescan()}
        disabled={scanning}
        title={t("Look for photos added or changed since the last check")}
      >
        {t("Check for new")}
      </button>
      <button
        class="secondary more"
        aria-haspopup="menu"
        aria-expanded={scanMenuOpen}
        aria-label={t("More scan options")}
        title={t("More scan options")}
        onclick={(event) => {
          event.stopPropagation();
          filterMenuOpen = false;
          sortMenuOpen = false;
          gearOpen = false;
          scanMenuOpen = !scanMenuOpen;
        }}
      >
        <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      {#if scanMenuOpen}
        <div class="menu scan-menu" role="menu">
          <button role="menuitem" disabled={scanning} onclick={() => rescan()}>
            <span class="mark"></span>
            <span>{t("Check for new photos")}<small>{t("Reads only files added or changed")}</small></span>
          </button>
          <button role="menuitem" onclick={() => rescan(true)}>
            <span class="mark"></span>
            <span>{t("Rescan everything")}<small>{t("Reads every file again; slow on a large library")}</small></span>
          </button>
          {#if IS_MAC}
            <hr />
            <button
              role="menuitem"
              disabled={!online}
              onclick={() => {
                scanMenuOpen = false;
                chooseAppleLibrary();
              }}
            >
              <span class="mark"></span>
              <span>{t("Import from Apple Photos…")}<small>{t("Favorites, hidden photos and albums")}</small></span>
            </button>
          {/if}
          <hr />
          <button
            role="menuitem"
            disabled={!online || !writable}
            onclick={() => {
              scanMenuOpen = false;
              deviceImport = { source: null };
            }}
          >
            <span class="mark"></span>
            <span>{t("Import photos…")}<small>{t("From a phone, a memory card, a camera or a folder")}</small></span>
          </button>
        </div>
      {/if}
    </span>
    <button class="keys-button" onclick={() => (showShortcuts = true)} title={t("Keyboard shortcuts (?)")} aria-label={t("Keyboard shortcuts")}>?</button>
    <SettingsButton onfaceschange={() => facesRevision++} />
    <AboutButton />
  </header>

  {#if !online}
    <p class="banner">
      {t("This library can't be reached right now. You are browsing what this computer has cached; full-size photos and changes are unavailable until it is back.")}
    </p>
  {:else if !writable && !inBrowser}
    <p class="banner">
      {t("This folder is read-only. Favorites, albums and hidden photos are kept on this computer only.")}
    </p>
  {/if}
  {#if offeredSource && !deviceImport}
    <p class="banner device">
      <span>{t("“{name}” is connected.", { name: offeredSource.name })}</span>
      {#if offeredSource.restricted}
        <span>{t("Unlock it and tap “Trust” or “Allow” on it to import its photos.")}</span>
      {:else}
        <button class="link" onclick={() => (deviceImport = { source: offeredSource })}>{t("Import photos…")}</button>
      {/if}
      <button class="dismiss" onclick={() => offeredSource && dismissedSources.add(offeredSource.path)} aria-label={t("Dismiss")}>×</button>
    </p>
  {/if}
  {#if imported}
    <p class="banner">
      {t("Imported from Apple Photos: {favorites} favorites, {hidden} hidden, {albums} album(s).", {
        favorites: imported.favorites,
        hidden: imported.hidden + imported.deleted,
        albums: imported.albums.filter((album) => album.matched > 0).length,
      })}
      <button class="dismiss" onclick={() => (imported = null)} aria-label={t("Dismiss")}>×</button>
    </p>
  {/if}
  {#if error}
    <p class="banner error" role="alert">
      {error}
      <button class="dismiss" onclick={() => (error = null)} aria-label={t("Dismiss")}>×</button>
    </p>
  {/if}
  {#if lastScan && lastScan.unreadableDirs > 0}
    <p class="banner">
      {tn(
        lastScan.unreadableDirs,
        "{n} folder could not be read, so photos that may have been deleted were kept in the index.",
        "{n} folders could not be read, so photos that may have been deleted were kept in the index.",
      )}
    </p>
  {/if}
  {#if !scanning && unreadable > 0}
    <p class="banner">
      {tn(
        unreadable,
        "{n} file could not be read. It will be tried again on the next scan.",
        "{n} files could not be read. They will be tried again on the next scan.",
      )}
    </p>
  {/if}

  <div class="body">
    {#if drawerOpen}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="drawer-backdrop" onclick={() => (drawerOpen = false)}></div>
    {/if}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <nav
      aria-label={t("Library sections")}
      style:width="{sidebarWidth}px"
      class:open={drawerOpen}
      onclick={(event) => {
        // Choosing a section closes the drawer (on a phone).
        if ((event.target as HTMLElement).closest("button.row")) drawerOpen = false;
      }}
    >
      {#snippet icon(name: Icon)}
        <svg class="row-icon" viewBox="0 0 20 20" width="18" height="18" aria-hidden="true">
          {#each ICONS[name] as d, i (i)}<path {d} />{/each}
        </svg>
      {/snippet}
      {#snippet row(target: Listing, label: string, count: number | null, kind: Icon, depth = 0, enabled = true)}
        <button
          class="row"
          class:selected={section === "photos" && JSON.stringify(target) === JSON.stringify(listing)}
          style:padding-left="{12 + depth * 14}px"
          disabled={!enabled}
          onclick={() => {
            listing = target;
            section = "photos";
            fromCollections = false;
          }}
          title={label}
        >
          {@render icon(kind)}
          <span class="row-name">{label}</span>
          {#if count !== null && count > 0}<span class="row-count">{formatNumber(count)}</span>{/if}
        </button>
      {/snippet}

      {#snippet item(entry: SidebarItem, place: "pinned" | "home" = "home")}
        <button
          class="row"
          class:selected={showsSelected(entry, place)}
          onclick={() => {
            clickedIn = place;
            fromCollections = false;
            entry.open();
          }}
          oncontextmenu={(event) => itemMenu(event, entry)}
          title={entry.label}
        >
          {@render icon(entry.icon)}
          <span class="row-name">{entry.label}</span>
          {#if entry.count !== null && entry.count > 0}<span class="row-count">{formatNumber(entry.count)}</span>{/if}
        </button>
      {/snippet}
      {#snippet heading(group: SidebarGroup, label: string)}
        <button class="heading-toggle" aria-expanded={openGroups[group]} onclick={() => toggleGroup(group)}>
          {label}
          <svg class:open={openGroups[group]} viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
            <path d="M3.5 1.5 7 5l-3.5 3.5" />
          </svg>
        </button>
      {/snippet}

      {@render row({ kind: "library" }, t("Library"), null, "library")}
      <button
        class="row"
        class:selected={section === "collections"}
        onclick={() => {
          section = "collections";
          fromCollections = false;
        }}
        title={t("Collections")}
      >
        {@render icon("collections")}
        <span class="row-name">{t("Collections")}</span>
      </button>

      {#if pinnedItems.length > 0}
        <h2>{@render heading("pinned", t("Pinned"))}</h2>
        {#if openGroups.pinned}
          {#each pinnedItems as entry (entry.id)}{@render item(entry, "pinned")}{/each}
        {/if}
      {/if}

      <h2>
        {@render heading("albums", t("Albums"))}
        <button
          class="add"
          onclick={() => (newAlbumName = "")}
          disabled={!online}
          aria-label={t("New album")}
          title={t("New album")}
        >
          +
        </button>
      </h2>
      {#if newAlbumName !== null}
        <form onsubmit={submitNewAlbum}>
          <input
            bind:value={newAlbumName}
            use:focus
            placeholder={t("Album name")}
            maxlength="200"
            onblur={() => (newAlbumName = null)}
            onkeydown={(event) => event.key === "Escape" && (newAlbumName = null)}
          />
        </form>
      {/if}
      {#if openGroups.albums}
        {#each albums as album (album.id)}
          {@const entry = sidebarItems.get(`album:${album.id}`)}
          {#if entry}{@render item(entry)}{/if}
        {:else}
          {#if newAlbumName === null}<p class="hint">{t("No albums yet")}</p>{/if}
        {/each}
      {/if}

      <h2>{@render heading("media", t("Media Types"))}</h2>
      {#if openGroups.media}
        {#each MEDIA_TYPES as id (id)}
          {@const entry = sidebarItems.get(id)}
          {#if entry}{@render item(entry)}{/if}
        {/each}
      {/if}

      <h2>{@render heading("utilities", t("Utilities"))}</h2>
      {#if openGroups.utilities}
        {#each UTILITIES as id (id)}
          {@const entry = sidebarItems.get(id)}
          {#if entry}{@render item(entry)}{/if}
        {/each}
      {/if}

      <h2>{@render heading("folders", t("Folders"))}</h2>
      {#each openGroups.folders ? folderTree.rows : [] as folder (folder.path)}
        <div class="tree-row" oncontextmenu={(event) => folderMenu(event, folder.path)} role="presentation">
          {#if folder.hasChildren}
            <button
              class="twisty"
              class:open={openFolders.has(folder.path)}
              style:left="{folder.depth * 14}px"
              aria-expanded={openFolders.has(folder.path)}
              aria-label={openFolders.has(folder.path)
                ? t("Hide the folders in {name}", { name: folder.name })
                : t("Show the folders in {name}", { name: folder.name })}
              onclick={() => toggleFolder(folder.path)}
            >
              <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
                <path d="M3.5 1.5 7 5l-3.5 3.5" />
              </svg>
            </button>
          {/if}
          {@render row(folderListing(folder.path), folder.name, folder.count, "folder", folder.depth + 1)}
        </div>
      {/each}
    </nav>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label={t("Resize the sidebar")}
      aria-valuenow={sidebarWidth}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      tabindex="0"
      onpointerdown={startResize}
      onpointermove={resize}
      onpointerup={() => (resizing = null)}
      onpointercancel={() => (resizing = null)}
      onkeydown={resizeWithKeys}
      ondblclick={() => (sidebarWidth = 220)}
    ></div>

    <main>
      {#if section === "map"}
        <div class="map">
          <MapView
            libraryPath={library.path}
            {revision}
            onopen={(area) => {
              listing = area;
              section = "photos";
            }}
            onpin={(area) => void togglePinArea(area)}
          />
        </div>
      {:else if section === "collections"}
        <div class="map">
          <CollectionsView
            libraryPath={library.path}
            {revision}
            {albums}
            pinned={pinnedItems.map(
              (entry): PinnedTile => ({
                id: entry.id,
                label: entry.label,
                icon: entry.icon,
                target: entry.target,
                open: () => {
                  fromCollections = entry.target !== null;
                  entry.open();
                },
              }),
            )}
            onlisting={openFromCollections}
            onsection={(id) => (section = id)}
            onperson={(person) => {
              personName = person.name;
              personHidden = person.hidden;
              openFromCollections({ kind: "person", clusters: person.clusters }, person.name ?? "");
            }}
            {favoritePeople}
            onexport={(memory) => (exportingMemory = memory)}
            onplay={(memory) => {
              // Straight from the click, so the memory's music may play.
              unlockAudio();
              playing = memory;
            }}
          />
        </div>
      {:else if section === "people"}
        <div class="map">
          {#key facesRevision}
          <PeopleView
            favorites={favoritePeople}
            ontogglefavorite={toggleFavoritePerson}
            pinned={(name) => pinned.includes(personPinId(name))}
            ontogglepin={togglePinPerson}
            libraryPath={library.path}
            {revision}
            canEdit={online}
            searching={scanning && progress?.phase === "faces"}
            onopen={(person) => {
              personName = person.name;
              personHidden = person.hidden;
              listing = { kind: "person", clusters: person.clusters };
              section = "photos";
            }}
          />
          {/key}
        </div>
      {:else if section === "duplicates"}
        <div class="map">
          <DuplicatesView
            libraryPath={library.path}
            {revision}
            canEdit={online && writable}
            onchanged={refreshAll}
          />
        </div>
      {:else if section === "similar"}
        <div class="map">
          <SimilarView libraryPath={library.path} {revision} canEdit={online && writable} onchanged={refreshAll} />
        </div>
      {:else}
      <div class="toolbar">
        {#if selected.size > 0}
          <strong>{t("{n} selected", { n: selected.size })}</strong>
          <button class="secondary deselect" onclick={() => selected.clear()} title={t("Deselect all (Esc)")}>
            ✕ {t("Deselect all")}
          </button>
          <span class="spacer"></span>
          <button
            class="secondary"
            disabled={!online}
            onclick={() => applyToSelection((photo) => ({ op: "fav", photo, on: true }))}
          >
            ♥ {t("Favorite")}
          </button>
          {#if listing.kind === "favorites"}
            <button
              class="secondary"
              disabled={!online}
              onclick={() => applyToSelection((photo) => ({ op: "fav", photo, on: false }))}
            >
              {t("Remove from favorites")}
            </button>
          {/if}
          {#if albums.length > 0}
            <select onchange={addSelectionToAlbum} disabled={!online}>
              <option value="">{t("Add to album…")}</option>
              {#each albums as album (album.id)}
                <option value={album.id}>{album.name}</option>
              {/each}
            </select>
          {/if}
          {#if currentAlbum}
            {@const album = currentAlbum.id}
            <button
              class="secondary"
              disabled={!online}
              onclick={() =>
                applyToSelection((photo) => ({ op: "album_member", album, photo, on: false }))}
            >
              {t("Remove from album")}
            </button>
          {/if}
          {#if listing.kind === "person"}
            <button
              class="secondary"
              disabled={!online && personName !== null}
              onclick={removeSelectionFromPerson}
              title={t("Their face in these photos is someone else's: take the photos out")}
            >
              {personName ? t("Not {name}", { name: personName }) : t("Not this person")}
            </button>
            <button
              class="secondary"
              disabled={!online && personName !== null}
              onclick={splitSelectionFromPerson}
              title={t("These photos show one other person: make them a person of their own")}
            >
              {t("Someone else")}
            </button>
          {/if}
          <button
            class="secondary"
            disabled={!online}
            onclick={() =>
              applyToSelection((photo) => ({ op: "hide", photo, on: listing.kind !== "hidden" }))}
          >
            {listing.kind === "hidden" ? t("Unhide") : t("Hide")}
          </button>
          {#if selected.size === 1}
            <button class="secondary" disabled={!online} onclick={revealSelected}>{REVEAL_LABEL}</button>
          {/if}
          <button
            class="secondary"
            disabled={!online}
            onclick={exportSelection}
            title={t("Copy the selected photos into a folder, optionally smaller")}
          >
            {t("Export…")}
          </button>
        {:else if renaming !== null}
          <form onsubmit={submitRename}>
            <input
              bind:value={renaming}
              use:focus
              maxlength="200"
              onblur={() => (renaming = null)}
              onkeydown={(event) => event.key === "Escape" && (renaming = null)}
            />
          </form>
        {:else}
          {#if fromCollections}
            <button class="secondary" onclick={() => (section = "collections")}>‹ {t("Collections")}</button>
          {:else if listing.kind === "person"}
            <button class="secondary" onclick={() => (section = "people")}>‹ {t("People")}</button>
          {/if}
          {#if listing.kind === "area"}
            <button class="secondary" onclick={() => (section = "map")}>‹ {t("Map")}</button>
          {/if}
          <span class="heading">
            <span class="heading-line">
              <strong class="title">{title}</strong>
              {#if groupsLoaded}<span class="dim">{itemCount(listedTotal, listedVideos)}</span>{/if}
            </span>
            <!-- Always there, with a fixed width, so that scrolling never
                 moves the rest of the toolbar. -->
            <span class="range" class:unused={viewMode !== "all" || !shownRange}>
              {shownRange ? formatDayRange(shownRange.oldest, shownRange.newest) : ""}
            </span>
          </span>
          {#if listing.kind === "person"}
            <button
              class="secondary"
              onclick={togglePersonHidden}
              title={personHidden
                ? t("List this person on the People screen again")
                : t("Leave this person out of the People screen (e.g. a stranger)")}
            >
              {personHidden ? t("Show in People") : t("Hide from People")}
            </button>
            {#if personName}
              {@const favorite = favoritePeople.includes(personName)}
              <button
                class="secondary icon-button"
                class:on={favorite}
                onclick={() => personName && toggleFavoritePerson(personName)}
                aria-pressed={favorite}
                aria-label={favorite ? t("Remove from Favorites") : t("Add to Favorites")}
                title={favorite ? t("Remove from Favorites") : t("Add to Favorites")}
              >
                <svg viewBox="0 0 20 20" width="15" height="15" aria-hidden="true">
                  <path d={ICONS.heart[0]} />
                </svg>
              </button>
            {/if}
          {/if}
          {#if listingPin}
            {@const pinnedHere = pinned.includes(listingPin.id)}
            <button class="secondary" onclick={listingPin.pin} aria-pressed={pinnedHere}>
              {pinnedHere ? t("Unpin") : t("Pin to Sidebar")}
            </button>
          {/if}
          <span class="spacer"></span>
          <div class="segmented" role="group" aria-label={t("View")}>
            {#each [["years", t("Years")], ["months", t("Months")], ["all", t("All Photos")]] as const as [mode, label] (mode)}
              <button aria-pressed={viewMode === mode} onclick={() => setViewMode(mode)}>
                {label}
              </button>
            {/each}
          </div>
          <span class="spacer"></span>
          <!-- Kept in place (only hidden) in the overviews, so that the
               view switch beside it does not move under the pointer. -->
          <input
            class="zoom"
            class:unused={viewMode !== "all"}
            disabled={viewMode !== "all"}
            type="range"
            min={CELL_MIN}
            max={CELL_MAX}
            step="10"
            bind:value={cellSize}
            aria-label={t("Thumbnail size")}
            title={t("Thumbnail size (Ctrl/Cmd + and -)")}
          />
          <button
            class="secondary icon"
            onclick={startSlideshow}
            disabled={listedTotal === 0}
            title={selected.size > 0 ? t("Slideshow of the selected photos (F5)") : t("Slideshow (F5)")}
            aria-label={t("Start a slideshow")}
          >
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <rect x="1.5" y="3" width="13" height="10" rx="2" />
              <path d="M6.5 5.8v4.4L10 8z" />
            </svg>
          </button>
          <span class="menu-anchor">
            <button
              class="secondary icon"
              class:unused={viewMode !== "all"}
              disabled={viewMode !== "all"}
              aria-haspopup="menu"
              aria-expanded={sortMenuOpen}
              onclick={(event) => {
                event.stopPropagation();
                filterMenuOpen = false;
                scanMenuOpen = false;
                gearOpen = false;
                sortMenuOpen = !sortMenuOpen;
              }}
              title={t("Sort")}
              aria-label={t("Sort")}
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <path d="M5 13V3M2.5 5.5 5 3l2.5 2.5M11 3v10M8.5 10.5 11 13l2.5-2.5" />
              </svg>
            </button>
            {#if sortMenuOpen}
              <div class="menu" role="menu">
                <button role="menuitemradio" aria-checked={!byName} onclick={() => chooseSort("date", null)}>
                  <span class="mark">{!byName ? "✓" : ""}</span>{t("Date taken")}
                </button>
                <button role="menuitemradio" aria-checked={byName} onclick={() => chooseSort("name", null)}>
                  <span class="mark">{byName ? "✓" : ""}</span>{t("Name")}
                </button>
                <hr />
                <button role="menuitemradio" aria-checked={ascending} onclick={() => chooseSort(null, true)}>
                  <span class="mark">{ascending ? "✓" : ""}</span>{byName ? t("A to Z") : t("Oldest first")}
                </button>
                <button role="menuitemradio" aria-checked={!ascending} onclick={() => chooseSort(null, false)}>
                  <span class="mark">{!ascending ? "✓" : ""}</span>{byName ? t("Z to A") : t("Newest first")}
                </button>
              </div>
            {/if}
          </span>
          <span class="menu-anchor">
            <button
              class="secondary icon"
              class:active={only !== null}
              aria-haspopup="menu"
              aria-expanded={filterMenuOpen}
              onclick={(event) => {
                event.stopPropagation();
                sortMenuOpen = false;
                scanMenuOpen = false;
                gearOpen = false;
                filterMenuOpen = !filterMenuOpen;
              }}
              title={only ? t("Showing: {filter}", { filter: filterName(only) }) : t("Filter")}
              aria-label={t("Filter")}
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <path d="M2 4h12M4 8h8M6 12h4" />
              </svg>
            </button>
            {#if filterMenuOpen}
              <div class="menu" role="menu">
                {#each FILTERS as [value, label, kind] (label)}
                  <button
                    role="menuitemradio"
                    aria-checked={only === value}
                    onclick={() => chooseFilter(value)}
                  >
                    <span class="mark">{only === value ? "✓" : ""}</span><svg
                      class="menu-icon"
                      viewBox="0 0 20 20"
                      width="16"
                      height="16"
                      aria-hidden="true">{#each ICONS[kind] as d, i (i)}<path {d} />{/each}</svg
                    >{label}
                  </button>
                {/each}
                <hr />
                {#each [1, 2, 3, 4, 5] as stars (stars)}
                  {@const value = `stars${stars}` as Only}
                  <button role="menuitemradio" aria-checked={only === value} onclick={() => chooseFilter(value)}>
                    <span class="mark">{only === value ? "✓" : ""}</span>{"★".repeat(stars)}{stars < 5 ? " " + t("or more") : ""}
                  </button>
                {/each}
                <hr />
                {#each LABELS as label (label.color)}
                  <button role="menuitemradio" aria-checked={only === label.color} onclick={() => chooseFilter(label.color)}>
                    <span class="mark">{only === label.color ? "✓" : ""}</span><span
                      class="swatch"
                      style:background={label.css}
                    ></span>{label.name}
                  </button>
                {/each}
              </div>
            {/if}
          </span>
          <span class="menu-anchor">
            <button
              class="secondary icon"
              class:active={gearActive}
              aria-haspopup="dialog"
              aria-expanded={gearOpen}
              onclick={(event) => {
                event.stopPropagation();
                filterMenuOpen = false;
                sortMenuOpen = false;
                scanMenuOpen = false;
                gearOpen = !gearOpen;
              }}
              title={gearActive ? t("Showing: {filter}", { filter: gearSummary }) : t("Camera and lens")}
              aria-label={t("Camera and lens")}
            >
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.2" />
                <path d="M8 1.8 10.6 7M14.1 7.2 9.2 10.4M11.6 13 6.4 12.1M3.2 11.8 5.6 6.7M3.4 4 9.1 3.3" />
              </svg>
            </button>
            {#if gearOpen}
              <GearPanel
                libraryPath={library.path}
                base={filtered}
                {gear}
                onchange={(next) => (gearChoice = { listing, gear: next })}
              />
            {/if}
          </span>
          <button
            class="secondary icon"
            class:unused={viewMode !== "all"}
            disabled={viewMode !== "all"}
            aria-pressed={gridFit === "river"}
            onclick={() => (gridFit = gridFit === "square" ? "river" : "square")}
            title={gridFit === "square" ? t("Show photos in rows at their own shape") : t("Show square thumbnails")}
            aria-label={t("Square thumbnails or rows of whole photos")}
          >
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              {#if gridFit === "square"}
                <rect x="1.5" y="1.5" width="5.5" height="5.5" rx="1" />
                <rect x="9" y="1.5" width="5.5" height="5.5" rx="1" />
                <rect x="1.5" y="9" width="5.5" height="5.5" rx="1" />
                <rect x="9" y="9" width="5.5" height="5.5" rx="1" />
              {:else}
                <rect x="1.5" y="1.5" width="8" height="5.5" rx="1" />
                <rect x="11" y="1.5" width="3.5" height="5.5" rx="1" />
                <rect x="1.5" y="9" width="4" height="5.5" rx="1" />
                <rect x="7" y="9" width="7.5" height="5.5" rx="1" />
              {/if}
            </svg>
          </button>
          {#if currentAlbum}
            {#if confirmingDelete}
              <span class="dim">{t("Delete this album? The photos stay in the library.")}</span>
              <button class="secondary danger" onclick={deleteAlbum}>{t("Delete")}</button>
              <button class="secondary" onclick={() => (confirmingDelete = false)}>{t("Cancel")}</button>
            {:else}
              <button
                class="secondary"
                disabled={!online}
                onclick={() => (renaming = currentAlbum?.name ?? "")}
              >
                {t("Rename")}
              </button>
              <button class="secondary" disabled={!online} onclick={() => (confirmingDelete = true)}>
                {t("Delete album")}
              </button>
            {/if}
          {/if}
        {/if}
      </div>

      {#if listing.kind === "person" && !personHintSeen}
        <p class="person-hint">
          <span>
            {personName
              ? t("A photo that isn't {name}? Right-click it and choose “Not {name}”. To take out several, select them first ({key}-click).", {
                  name: personName,
                  key: IS_MAC ? "⌘" : "Ctrl",
                })
              : t("A photo that isn't this person? Right-click it and choose “Not this person”. To take out several, select them first ({key}-click).", {
                  key: IS_MAC ? "⌘" : "Ctrl",
                })}
          </span>
          <button class="link-button" onclick={dismissPersonHint}>{t("Got it")}</button>
        </p>
      {/if}
      {#if listing.kind === "person" && personName}
        <PersonSuggestions
          libraryPath={library.path}
          name={personName}
          clusters={listing.clusters}
          canEdit={online}
          onadded={(clusters) => (listing = { kind: "person", clusters })}
        />
      {/if}
      <div class="grid">
        {#if groups.length > 0 && viewMode !== "all"}
          <Overview
            mode={viewMode}
            months={ascending && !byName ? [...months].reverse() : months}
            {startYear}
            onyear={(year) => {
              startYear = year;
              viewMode = "months";
            }}
            onday={(day, month) => {
              // Oldest first, a month opens at its first day.
              startDay =
                month && order === "dateAsc" ? day - new Date(day * 86_400_000).getUTCDate() + 1 : day;
              viewMode = "all";
            }}
            onslideshow={slideshowOfMonth}
          />
        {:else if groups.length > 0}
          <PhotoGrid
            bind:this={grid}
            {groups}
            {pages}
            {selected}
            {cellSize}
            fit={gridFit}
            shapes={gridShapes}
            onrange={setShownRange}
            startAt={startDay}
            onopen={(index) => (viewerIndex = index)}
            oncontext={photoMenu}
            ontoggle={toggleSelected}
            onmonthslideshow={slideshowOfMonth}
          />
        {:else if groupsLoaded}
          <p class="empty">{emptyText}</p>
        {/if}
      </div>
      {/if}
    </main>
  </div>
</div>

{#if exporting}
  <ExportDialog libraryPath={library.path} photos={exporting} onclose={() => (exporting = null)} />
{/if}
{#if deviceImport}
  <ImportDialog
    libraryPath={library.path}
    source={deviceImport.source}
    onclose={() => {
      if (deviceImport?.source) dismissedSources.add(deviceImport.source.path);
      deviceImport = null;
    }}
    onimported={() => rescan()}
  />
{/if}

{#if playing}
  {@const memory = playing}
  <MemoryPlayer
    libraryPath={library.path}
    {memory}
    onclose={() => (playing = null)}
    onshowall={() => {
      playing = null;
      openFromCollections(memory.listing, memoryTitle(memory));
    }}
    onexport={() => {
      playing = null;
      exportingMemory = memory;
    }}
  />
{/if}
{#if exportingMemory}
  <MemoryExport libraryPath={library.path} memory={exportingMemory} onclose={() => (exportingMemory = null)} />
{/if}

{#if slideshow}
  <Slideshow
    {pages}
    count={slideshow.count}
    indexAt={slideshow.indexAt}
    onclose={() => (slideshow = null)}
  />
{/if}

{#if importing}
  {@const summary = importing.summary}
  <div class="backdrop">
    <div class="dialog" role="dialog" aria-modal="true" aria-label={t("Import from Apple Photos")}>
      <h2>{t("Import from Apple Photos")}</h2>
      <p>
        {t("{matched} of the {total} photos in that library were found in this one. Importing will:", {
          matched: summary.matched,
          total: summary.assets,
        })}
      </p>
      <ul>
        <li>{t("mark {n} as favorites;", { n: summary.favorites })}</li>
        <li>{t("hide {n};", { n: summary.hidden })}</li>
        {#if summary.deleted > 0}
          <li>
            {tn(
              summary.deleted,
              "hide {n} more that is in “Recently Deleted” in Photos;",
              "hide {n} more that are in “Recently Deleted” in Photos;",
            )}
          </li>
        {/if}
        <li>
          {tn(
            summary.albums.filter((album) => album.matched > 0).length,
            "create or fill {n} album:",
            "create or fill {n} albums:",
          )}
        </li>
      </ul>
      {#if summary.albums.length > 0}
        <div class="albums">
          {#each summary.albums as album (album.name)}
            <div class:dim={album.matched === 0}>
              <span>{album.name}</span>
              <span>{t("{index} of {count}", { index: album.matched, count: album.photos })}</span>
            </div>
          {/each}
        </div>
      {/if}
      <p class="dim small">
        {t("Nothing is removed, and the Apple Photos library is only read. Photos that are not in this folder are skipped.")}
      </p>
      <div class="actions">
        <button class="secondary" onclick={() => (importing = null)} disabled={importing.busy}>
          {t("Cancel")}
        </button>
        <button
          class="secondary default"
          onclick={confirmImport}
          disabled={importing.busy || summary.matched === 0}
        >
          {importing.busy ? t("Importing…") : t("Import")}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showShortcuts}
  <ShortcutsPanel onclose={() => (showShortcuts = false)} />
{/if}

{#if viewerIndex !== null}
  <Viewer
    {pages}
    total={listedTotal}
    bind:index={viewerIndex}
    {revision}
    canEdit={online}
    {albums}
    currentAlbum={currentAlbum?.id ?? null}
    showingHidden={listing.kind === "hidden"}
    {apply}
    onclose={closeViewer}
  />
{/if}

<style>
  .library {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .link {
    border: none;
    background: none;
    padding: 4px 0;
    color: var(--accent);
    cursor: pointer;
    white-space: nowrap;
  }

  .name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    /* A fixed width with digits of equal width: the counter changes many
       times a second during a scan and must not push the search box around. */
    justify-content: flex-end;
    width: 260px;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  progress {
    width: 90px;
    accent-color: var(--accent);
  }

  .secondary,
  select {
    padding: 5px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: inherit;
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
  }

  /* Drawn like the buttons beside it: the system's own dropdown is shorter
     and rounder. The arrow is drawn here instead. */
  select {
    appearance: none;
    -webkit-appearance: none;
    padding-right: 28px;
    line-height: inherit;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 12'%3E%3Cpath d='M2 4.5 5 1.5 8 4.5M2 7.5 5 10.5 8 7.5' fill='none' stroke='%23888' stroke-width='1.4' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    background-size: 9px 11px;
  }

  .secondary:disabled,
  select:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .danger {
    border-color: var(--danger);
    color: var(--danger);
  }

  .banner {
    margin: 0;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--warn-bg);
    color: var(--warn);
  }

  .banner.error {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    color: var(--danger);
    user-select: text;
    -webkit-user-select: text;
  }

  .banner.device {
    display: flex;
    align-items: baseline;
    gap: 12px;
    background: var(--surface);
    color: var(--text);
  }

  .banner.device .dismiss {
    margin-left: auto;
    color: var(--text-dim);
  }

  .banner .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }

  .dismiss {
    border: none;
    background: none;
    font-size: 16px;
    cursor: pointer;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  nav {
    flex-shrink: 0;
    padding: 8px 0 16px;
    overflow-y: auto;
    background: var(--bg);
  }

  .search {
    flex: 1;
    display: flex;
    justify-content: center;
    min-width: 0;
  }

  .search input {
    width: 100%;
    max-width: 420px;
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }

  /* The line between the sidebar and the photos; drag it to resize. */
  .resizer {
    flex-shrink: 0;
    width: 5px;
    margin-right: -4px;
    border-left: 1px solid var(--border);
    cursor: col-resize;
    touch-action: none;
    z-index: 1;
  }

  .resizer:hover,
  .resizer:focus-visible {
    border-left: 3px solid var(--accent);
    outline: none;
  }

  .segmented {
    display: flex;
    flex-shrink: 0;
    white-space: nowrap;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }

  .segmented button {
    padding: 4px 12px;
    border: none;
    background: var(--surface);
    font-size: 13px;
    cursor: pointer;
  }

  .segmented button + button {
    border-left: 1px solid var(--border);
  }

  .segmented button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--accent-text);
  }

  .toolbar input.zoom {
    width: 120px;
  }

  .toolbar input.zoom.unused,
  .toolbar button.unused {
    visibility: hidden;
  }

  .toolbar button.icon {
    display: inline-flex;
    align-items: center;
    padding: 4px 6px;
  }

  .toolbar button.icon.active {
    background: var(--accent);
    color: var(--accent-text);
  }

  .menu-anchor {
    position: relative;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    min-width: 190px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.22);
  }

  .menu button {
    display: flex;
    align-items: center;
    padding: 6px 10px 6px 4px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .menu button:hover,
  .menu button:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
  }

  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    margin-right: 8px;
    border-radius: 50%;
    vertical-align: -1px;
  }

  .menu hr {
    width: 100%;
    margin: 4px 0;
    border: none;
    border-top: 1px solid var(--border);
  }

  .menu .mark {
    width: 20px;
    text-align: center;
  }

  .scan-buttons {
    display: inline-flex;
  }

  .scan-buttons > button:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }

  .scan-buttons > .more {
    margin-left: -1px;
    padding: 0 7px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }

  .scan-buttons > .more svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .scan-menu {
    min-width: 250px;
  }

  .scan-menu small {
    display: block;
    margin-top: 1px;
    opacity: 0.7;
    font-size: 11px;
  }

  .menu button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .menu button:disabled:hover {
    background: none;
    color: var(--text);
  }

  .toolbar button.icon svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
  }

  nav h2 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 18px 12px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }

  .add {
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: none;
    font-size: 16px;
    line-height: 1;
    color: var(--text-dim);
    cursor: pointer;
  }

  .add:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.4);
    z-index: 20;
  }

  .dialog {
    width: min(460px, calc(100vw - 40px));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    padding: 20px 24px;
    border-radius: 12px;
    background: var(--surface);
    box-shadow: 0 10px 40px rgb(0 0 0 / 0.35);
    font-size: 14px;
    line-height: 1.45;
  }

  .dialog h2 {
    margin: 0 0 8px;
    font-size: 17px;
  }

  .dialog ul {
    margin: 8px 0;
    padding-left: 20px;
  }

  .dialog .albums {
    max-height: 180px;
    overflow-y: auto;
    margin: 0 0 8px 20px;
    font-size: 13px;
  }

  .dialog .albums div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 2px 0;
  }

  .dialog .small {
    font-size: 12px;
    white-space: normal;
  }

  .dialog .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }

  .dialog .default {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-text);
  }

  .hint {
    margin: 2px 12px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 5px 12px;
    border: none;
    background: none;
    text-align: left;
    cursor: pointer;
  }

  .row:disabled {
    color: var(--text-dim);
    cursor: default;
  }

  .row:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .row.selected,
  .row.selected:hover {
    background: var(--accent);
    color: var(--accent-text);
  }

  .row-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .menu-icon {
    flex: none;
    margin-right: 8px;
    vertical-align: -3px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .row-icon {
    flex: none;
    fill: none;
    stroke: var(--accent);
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .row.selected .row-icon {
    stroke: currentColor;
  }

  .row:disabled .row-icon {
    stroke: currentColor;
  }

  .person-hint {
    display: flex;
    align-items: baseline;
    gap: 14px;
    margin: 0;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    color: var(--text-dim);
  }

  .person-hint span {
    flex: 1;
  }

  .link-button {
    flex: none;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }

  .heading-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    letter-spacing: inherit;
    text-transform: inherit;
    color: inherit;
    cursor: pointer;
  }

  .heading-toggle:hover {
    color: var(--text);
  }

  .heading-toggle svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: transform 0.12s;
  }

  .heading-toggle svg.open {
    transform: rotate(90deg);
  }

  .row-count {
    font-size: 12px;
    opacity: 0.7;
  }

  form {
    display: contents;
  }

  input {
    padding: 4px 8px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--bg);
    color: inherit;
    font: inherit;
    user-select: text;
    -webkit-user-select: text;
  }

  nav input {
    width: calc(100% - 24px);
    margin: 2px 12px;
    background: var(--surface);
  }

  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 46px;
    padding: 6px 16px;
    border-bottom: 1px solid var(--border);
  }

  .toolbar input {
    width: 280px;
  }

  .drawer-button,
  .drawer-backdrop {
    display: none;
  }

  .drawer-button {
    flex: none;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border: none;
    border-radius: 8px;
    background: none;
    color: var(--text);
    cursor: pointer;
  }

  .drawer-button svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
  }

  /* A phone: the sidebar slides in from the left, the top bar keeps only
     the search and the settings, and the toolbar wraps. */
  @media (max-width: 700px) {
    .drawer-button {
      display: grid;
    }

    header {
      gap: 6px;
      padding: max(6px, env(safe-area-inset-top)) 8px 6px;
    }

    header .name,
    header .status,
    .keys-button,
    .scan-buttons {
      display: none;
    }

    nav {
      position: fixed;
      top: 0;
      bottom: 0;
      left: 0;
      z-index: 60;
      width: min(300px, 85vw) !important;
      padding-top: max(8px, env(safe-area-inset-top));
      box-shadow: 4px 0 24px rgb(0 0 0 / 0.3);
      transform: translateX(-105%);
      transition: transform 0.22s ease;
    }

    nav.open {
      transform: none;
    }

    .drawer-backdrop {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 55;
      background: rgb(0 0 0 / 0.4);
    }

    .resizer {
      display: none;
    }

    .toolbar {
      flex-wrap: wrap;
      padding: 6px 10px;
    }

    .toolbar input {
      width: 100%;
    }
  }

  .tree-row {
    position: relative;
  }

  .twisty {
    position: absolute;
    top: 50%;
    z-index: 1;
    width: 22px;
    height: 22px;
    margin-left: 4px;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-dim);
    line-height: 0;
    text-align: center;
    cursor: pointer;
    transform: translateY(-50%);
    transition: transform 0.12s;
  }

  .twisty svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .twisty.open {
    transform: translateY(-50%) rotate(90deg);
  }

  .heading {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .heading-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .range {
    width: 17em;
    max-width: 100%;
    min-height: 1.2em;
    overflow: hidden;
    color: var(--text-dim);
    font-size: 11px;
    line-height: 1.2;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .range.unused {
    visibility: hidden;
  }

  /* No room for the dates on a narrow window. */
  @media (max-width: 900px) {
    .range {
      display: none;
    }
  }

  .title {
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dim {
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .grid {
    flex: 1;
    min-height: 0;
  }

  .map {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .empty {
    max-width: 380px;
    margin: 0 auto;
    padding-top: 100px;
    text-align: center;
    color: var(--text-dim);
  }

  .keys-button {
    width: 30px;
    height: 28px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
  }

  .keys-button:hover {
    background: var(--surface-hover);
  }

  .icon-button {
    display: inline-grid;
    place-items: center;
    padding-inline: 8px;
  }

  .icon-button svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linejoin: round;
  }

  .icon-button.on svg {
    fill: var(--danger);
    stroke: var(--danger);
  }
</style>
