<!--
  The photos on a world map. Photos taken close together are drawn as one
  marker with a count; zooming in splits them up. Clicking a marker lists
  its photos.

  The map pictures ("tiles") come from OpenStreetMap over the internet. The
  markers do not: they work without a connection, on a plain background.
-->
<script module lang="ts">
  import type { LatLngBoundsLiteral } from "leaflet";

  /** Where each library's map was left, so that coming back shows the same place. */
  const lastView = new Map<string, { center: [number, number]; zoom: number }>();
  /** The part of the world the tiles cover. */
  const WORLD: LatLngBoundsLiteral = [
    [-85, -180],
    [85, 180],
  ];
</script>

<script lang="ts">
  import { t } from "./i18n";
  import L from "leaflet";
  import "leaflet/dist/leaflet.css";
  import { onMount } from "svelte";
  import { mapClusters, thumbUrl, type Listing, type MapCluster } from "./api";
  import { formatNumber } from "./format";
  import { showMenu } from "./contextMenu";

  interface Props {
    libraryPath: string;
    /** Changes whenever the index or the organization changed. */
    revision: number;
    /** Called with the listing of a clicked marker's photos. */
    onopen: (area: Listing) => void;
    /** Pins the part of the map a marker stands for. */
    onpin: (area: Listing & { kind: "area" }) => void;
  }
  let { libraryPath, revision, onopen, onpin }: Props = $props();

  let container: HTMLDivElement;
  let map: L.Map | undefined;
  let markers: L.LayerGroup | undefined;
  /** `null` until the first answer; then the number of markers on screen. */
  let shown = $state<number | null>(null);
  let request = 0;

  onMount(() => {
    const created = L.map(container, {
      minZoom: 2,
      maxZoom: 19,
      maxBounds: WORLD,
      maxBoundsViscosity: 1,
      zoomSnap: 1,
    });
    map = created;
    L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
      maxZoom: 19,
      attribution: "© OpenStreetMap contributors",
    }).addTo(created);
    markers = L.layerGroup().addTo(created);
    created.attributionControl.setPrefix(false);
    created.on("moveend", () => {
      const center = created.getCenter();
      lastView.set(libraryPath, { center: [center.lat, center.lng], zoom: created.getZoom() });
      load();
    });

    const before = lastView.get(libraryPath);
    if (before) {
      created.setView(before.center, before.zoom);
    } else {
      // Start on the area that holds all the photos.
      created.fitBounds(WORLD);
      mapClusters(libraryPath, { south: -90, west: -180, north: 90, east: 180 }, 2)
        .then((all) => {
          if (all.length === 0) return;
          created.fitBounds(
            [
              [Math.min(...all.map((c) => c.south)), Math.min(...all.map((c) => c.west))],
              [Math.max(...all.map((c) => c.north)), Math.max(...all.map((c) => c.east))],
            ],
            { padding: [60, 60], maxZoom: 14 },
          );
        })
        .catch(() => {});
    }
    return () => created.remove();
  });

  // Photos came or went: draw the markers again.
  $effect(() => {
    void revision;
    load();
  });

  async function load() {
    if (!map || !markers) return;
    const mine = ++request;
    // A bit more than what is visible, so markers don't pop in at the edges.
    const bounds = map.getBounds().pad(0.25);
    const zoom = map.getZoom();
    let clusters: MapCluster[];
    try {
      clusters = await mapClusters(
        libraryPath,
        {
          south: bounds.getSouth(),
          west: bounds.getWest(),
          north: bounds.getNorth(),
          east: bounds.getEast(),
        },
        zoom,
      );
    } catch {
      return;
    }
    if (mine !== request || !markers) return; // the map moved on meanwhile
    markers.clearLayers();
    for (const cluster of clusters) {
      markerFor(cluster).addTo(markers);
    }
    shown = clusters.length;
  }

  function markerFor(cluster: MapCluster): L.Marker {
    const element = document.createElement("div");
    const image = document.createElement("img");
    image.src = thumbUrl(cluster.fingerprint);
    image.alt = "";
    image.draggable = false;
    element.append(image);
    if (cluster.count > 1) {
      const count = document.createElement("span");
      count.textContent = formatNumber(cluster.count);
      element.append(count);
    }
    const label =
      cluster.count === 1 ? "1 photo here" : `${formatNumber(cluster.count)} photos here`;
    return L.marker([cluster.latitude, cluster.longitude], {
      icon: L.divIcon({ html: element, className: "photo-marker", iconSize: [56, 56] }),
      title: label,
      alt: label,
      keyboard: true,
    })
      .on("click", () => onopen(areaOf(cluster)))
      .on("contextmenu", (event: L.LeafletMouseEvent) => {
        const area = areaOf(cluster);
        showMenu(event.originalEvent, [
          { label: t("Open"), action: () => onopen(area) },
          "-",
          { label: t("Pin to Sidebar"), action: () => onpin(area) },
        ]);
      });
  }

  function areaOf(cluster: MapCluster): Listing & { kind: "area" } {
    // Float rounding must not leave a photo just outside its own rectangle.
    const margin = 1e-7;
    return {
      kind: "area",
      south: cluster.south - margin,
      west: cluster.west - margin,
      north: cluster.north + margin,
      east: cluster.east + margin,
    };
  }
</script>

<div class="map" bind:this={container}></div>
{#if shown === 0}
  <p class="empty">{t("No photos with a location in this part of the map.")}</p>
{/if}

<style>
  .map {
    position: absolute;
    inset: 0;
    background: var(--surface-hover);
    /* Keeps the map's own layers below the app's viewer. */
    isolation: isolate;
  }

  .empty {
    position: absolute;
    left: 50%;
    bottom: 28px;
    transform: translateX(-50%);
    margin: 0;
    padding: 6px 12px;
    border-radius: 6px;
    background: rgb(0 0 0 / 0.7);
    color: #fff;
    font-size: 13px;
    pointer-events: none;
  }

  /* The markers are made by Leaflet, outside this component's markup. */
  .map :global(.photo-marker) {
    border: 2px solid #fff;
    border-radius: 8px;
    background: #888;
    box-shadow: 0 1px 5px rgb(0 0 0 / 0.5);
    cursor: pointer;
  }

  .map :global(.photo-marker:hover),
  .map :global(.photo-marker:focus-visible) {
    border-color: var(--accent);
    z-index: 10000 !important;
  }

  .map :global(.photo-marker div) {
    position: relative;
    width: 100%;
    height: 100%;
  }

  .map :global(.photo-marker img) {
    display: block;
    width: 100%;
    height: 100%;
    border-radius: 6px;
    object-fit: cover;
  }

  .map :global(.photo-marker span) {
    position: absolute;
    top: -9px;
    right: -9px;
    min-width: 20px;
    padding: 1px 5px;
    border-radius: 10px;
    background: var(--accent);
    color: #fff;
    font-size: 11px;
    font-weight: 600;
    line-height: 18px;
    text-align: center;
  }
</style>
