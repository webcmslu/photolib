# The TV app

Phototek on the living-room TV: the library of a Phototek server (see
[server.md](server.md)), browsed with the remote. One app for LG (webOS),
Samsung (Tizen) and Android TV.

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

## Samsung TVs (Tizen)

The same files, with `tv/tizen/config.xml` instead of `appinfo.json`:
`tools/build-tv.sh tizen` puts them in `build/tizen/`. The app registers the
media keys (Tizen sends them only when asked) and leaves through Tizen's own
call on Back from the home screen.

To package and install: Tizen Studio with the **TV Extensions**, a Samsung
certificate profile made in its Certificate Manager (named `phototek`, or set
`TIZEN_PROFILE`), and the TV in Developer Mode (Apps → type 12345 on the
remote → Developer Mode on, with the Mac's IP address). Then
`tools/build-tv.sh tizen` makes the `.wgt`, and Tizen Studio's Device
Manager installs it. Publishing goes through the Samsung Apps TV Seller
Office.

## Android TV

The Android app itself: on a TV (Android detects it, `isTelevision` in
plugins/photos), it opens this interface instead of the phone's. The
Android build bundles it (`build.beforeBuildCommand` in
`tauri.android.conf.json` copies `dist-tv/` into `dist/tv/`). The app shows
in the TV's launcher with its banner (`res/drawable-xhdpi/banner.png`), and
the manifest says it needs no touch screen, which Google Play asks of TV
apps. The remote's Back button reaches the app through Tauri's
`onBackButtonPress`. Most Android TVs give web views 960 × 540: the layout
(made for 1920 × 1080) scales to fit.

A test build can act as a TV on a phone: `VITE_PHOTOTEK_TV=1`.

## LG Content Store

Publishing needs an LG Seller Lounge account (seller.lgappstv.com), the
`.ipk`, a 1920 × 1080 screenshot set, icons (80 × 80 and 130 × 130, in
`tv/public/`), and the descriptions (`store/` has them for the phone stores).
LG reviews the app with a remote: the demo server (`Try the demo`) is how
the reviewer sees photos.

## Tested

On 2026-11-28, against the demo server:

- in Chrome at 1920 × 1080 and 960 × 540, driven by its keys: connecting, the
  rows, a day's grid, the viewer, Back with the focus put back, a memory
  playing;
- in the Android app on the emulator (as a TV, `VITE_PHOTOTEK_TV=1`): the
  switch to this interface, the D-pad, OK, and the Back button.

Not yet on a real TV, nor on Tizen (it needs Tizen Studio).
