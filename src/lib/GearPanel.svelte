<!--
  The camera filter: narrows the grid to a camera, a lens, an aperture and
  a focal length. Apertures and focal lengths are chips, so going from
  f/2.8 to f/4 to f/8 is one click each. Each list counts the photos that
  the other choices leave.
-->
<script lang="ts">
  import { gearOptions, type Gear, type GearOptions, type Listing } from "./api";
  import { t } from "./i18n";
  import { cameraName, formatNumber } from "./format";

  interface Props {
    libraryPath: string;
    /** The listing being narrowed down (with the filter menu's choice). */
    base: Listing;
    gear: Gear;
    onchange: (gear: Gear) => void;
  }
  let { libraryPath, base, gear, onchange }: Props = $props();

  type Facet = "camera" | "lens" | "aperture" | "focal";

  let options = $state<GearOptions | null>(null);
  let failed = $state(false);

  const isEmpty = (g: Gear) =>
    !g.cameraMake && !g.cameraModel && !g.lens && g.aperture == null && g.focalLength == null;

  /** The choices without one facet: what that facet's list is counted in. */
  function without(g: Gear, facet: Facet): Gear {
    const rest = { ...g };
    if (facet === "camera") {
      rest.cameraMake = null;
      rest.cameraModel = null;
    } else if (facet === "lens") rest.lens = null;
    else if (facet === "aperture") rest.aperture = null;
    else rest.focalLength = null;
    return rest;
  }

  $effect(() => {
    const current = { ...gear };
    const within = base;
    let stale = false;
    // The same listing is asked once, even if several facets need it.
    const asked = new Map<string, Promise<GearOptions>>();
    const ask = (g: Gear) => {
      const listing = isEmpty(g) ? within : ({ kind: "gear", within, ...g } as Listing);
      const key = JSON.stringify(listing);
      if (!asked.has(key)) asked.set(key, gearOptions(libraryPath, listing));
      return asked.get(key)!;
    };
    Promise.all((["camera", "lens", "aperture", "focal"] as Facet[]).map((facet) => ask(without(current, facet))))
      .then(([cameras, lenses, apertures, focals]) => {
        if (stale) return;
        failed = false;
        options = {
          cameras: cameras.cameras,
          lenses: lenses.lenses,
          apertures: apertures.apertures,
          focalLengths: focals.focalLengths,
          unread: cameras.unread,
        };
      })
      .catch(() => {
        if (!stale) failed = true;
      });
    return () => {
      stale = true;
    };
  });

  const cameraKey = (make: string | null | undefined, model: string | null | undefined) =>
    JSON.stringify([make ?? null, model ?? null]);
  const chosenCamera = $derived(
    gear.cameraMake || gear.cameraModel ? cameraKey(gear.cameraMake, gear.cameraModel) : "",
  );

  function chooseCamera(key: string) {
    const [make, model] = key ? (JSON.parse(key) as [string | null, string | null]) : [null, null];
    onchange({ ...gear, cameraMake: make, cameraModel: model });
  }

  const aperture = (f: number) => `f/${formatNumber(f)}`;
  const count = (n: number) => formatNumber(n);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="panel" role="dialog" tabindex="-1" aria-label={t("Camera and lens")} onclick={(event) => event.stopPropagation()}>
  {#if failed}
    <p class="note">{t("The camera filter can't be shown right now.")}</p>
  {:else if !options}
    <p class="note">{t("Loading…")}</p>
  {:else}
    <label>
      <span>{t("Camera")}</span>
      <select value={chosenCamera} onchange={(event) => chooseCamera(event.currentTarget.value)}>
        <option value="">{t("Any camera")}</option>
        {#each options.cameras as [make, model, n] (cameraKey(make, model))}
          <option value={cameraKey(make, model)}>{cameraName(make, model)} ({count(n)})</option>
        {/each}
        {#if chosenCamera && !options.cameras.some(([make, model]) => cameraKey(make, model) === chosenCamera)}
          <option value={chosenCamera}>{cameraName(gear.cameraMake ?? null, gear.cameraModel ?? null)} (0)</option>
        {/if}
      </select>
    </label>
    <label>
      <span>{t("Lens")}</span>
      <select value={gear.lens ?? ""} onchange={(event) => onchange({ ...gear, lens: event.currentTarget.value || null })}>
        <option value="">{t("Any lens")}</option>
        {#each options.lenses as [lens, n] (lens)}
          <option value={lens}>{lens} ({count(n)})</option>
        {/each}
        {#if gear.lens && !options.lenses.some(([lens]) => lens === gear.lens)}
          <option value={gear.lens}>{gear.lens} (0)</option>
        {/if}
      </select>
    </label>

    <div class="group">
      <span>{t("Aperture")}</span>
      <div class="chips">
        <button class:on={gear.aperture == null} onclick={() => onchange({ ...gear, aperture: null })}>{t("Any")}</button>
        {#each options.apertures as [f, n] (f)}
          <button
            class:on={gear.aperture === f}
            title={t("{n} photos", { n: count(n) })}
            onclick={() => onchange({ ...gear, aperture: gear.aperture === f ? null : f })}
          >
            {aperture(f)} <small>{count(n)}</small>
          </button>
        {/each}
      </div>
    </div>
    <div class="group">
      <span>{t("Focal length")}</span>
      <div class="chips">
        <button class:on={gear.focalLength == null} onclick={() => onchange({ ...gear, focalLength: null })}>{t("Any")}</button>
        {#each options.focalLengths as [mm, n] (mm)}
          <button
            class:on={gear.focalLength === mm}
            title={t("{n} photos", { n: count(n) })}
            onclick={() => onchange({ ...gear, focalLength: gear.focalLength === mm ? null : mm })}
          >
            {mm} mm <small>{count(n)}</small>
          </button>
        {/each}
      </div>
    </div>

    {#if options.unread > 0}
      <p class="note">{t("Lens and settings are still being read for {n} photos.", { n: count(options.unread) })}</p>
    {:else if options.cameras.length === 0 && options.apertures.length === 0}
      <p class="note">{t("These photos don't say which camera took them.")}</p>
    {/if}
    {#if !isEmpty(gear)}
      <div class="footer">
        <button class="clear" onclick={() => onchange({})}>{t("Show all")}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .panel {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: min(360px, calc(100vw - 32px));
    max-height: min(560px, calc(100vh - 120px));
    overflow-y: auto;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.22);
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }

  label,
  .group {
    display: grid;
    gap: 4px;
  }

  label > span,
  .group > span {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }

  select {
    width: 100%;
    padding: 5px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    max-height: 150px;
    overflow-y: auto;
  }

  .chips button {
    padding: 3px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    cursor: pointer;
  }

  .chips button:hover {
    background: var(--surface-hover);
  }

  .chips button.on {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-text);
  }

  .chips small {
    opacity: 0.65;
  }

  .note {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }

  .footer {
    display: flex;
    justify-content: flex-end;
  }

  .clear {
    padding: 5px 12px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
</style>
