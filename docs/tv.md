# The TV app

Phototek on the living-room TV: the library of a Phototek server (see
[server.md](server.md)), browsed with the remote. LG webOS first; the same
app will serve Samsung Tizen and Android TV, which also run web apps.

## What it does

- **Home:** rows of large cards, as on the TV's own apps: memories (today's
  first), recent days, trips, people, albums, and All Photos, Favourites,
  Videos.
- **A memory** plays full screen with its music (the same player as the
  other apps).
- **A grid** of photos, loaded as you move down; **OK** opens a photo full
  screen. **◀ ▶** move through the photos, **OK** shows or hides the date,
  **▶︎❙❙** starts or stops a slideshow, videos play.
- **Back** goes back one screen, and leaves the app from the home screen.
- **First start:** the server's address (e.g. `192.168.1.20:8080`) and its
  password, or **Try the demo**. The TV remembers the server.

## How it is made

- `tv/index.html` and `src/tv/`: its own page, with the Svelte code and the
  server calls of the other apps (`src/lib/api.ts`, `MemoryPlayer.svelte`).
- `src/tv/remote.ts`: the remote's keys (webOS Back is key code 461, Tizen's
  10009) and moving the focus with the arrows to the nearest item that way.
  Anything with `data-focus` can take the focus; `data-first` takes it first.
- `vite.tv.config.ts` builds it into `dist-tv/` with relative paths, for
  webOS 6 (2021 TVs, Chromium 79) and later.
- `tv/public/appinfo.json`: the webOS app description (id
  `lu.webcms.phototek`, version, icons, `disableBackHistoryAPI` so the app
  handles Back itself).

## Try it on a computer

```bash
npm run dev:tv      # http://localhost:1430, at 1920 × 1080
```

The arrows, Enter (OK) and Escape (Back) act as the remote.

## Install it on an LG TV (Developer Mode)

1. On the TV, install **Developer Mode** from the LG Content Store and sign in
   with an LG developer account (free, at webostv.developer.lge.com). Switch
   Developer Mode on and restart the TV; note the IP address and the
   passphrase it shows.
2. On the Mac, once: `npm install -g @webos-tools/cli`, then
   `ares-setup-device` and add the TV with the name `tv` and its IP address,
   and `ares-novacom --device tv --getkey` with the passphrase.
3. `tools/build-tv.sh install` builds, packages and installs it.

Developer Mode lasts 50 hours at a time; the app's settings page has the
button that extends it.

## LG Content Store

Publishing needs an LG Seller Lounge account (seller.lgappstv.com), the
`.ipk`, a 1920 × 1080 screenshot set, icons (80 × 80 and 130 × 130, in
`tv/public/`), and the descriptions (`store/` has them for the phone stores).
LG reviews the app with a remote: the demo server (`Try the demo`) is how
the reviewer sees photos.

## Tested

On 2026-11-28 in Chrome at 1920 × 1080, driven by its keys against the demo
server: connecting, the rows, a day's grid, the viewer, Back with the focus
put back, and a memory playing. Not yet on a real TV.
