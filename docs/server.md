# Phototek server

The Phototek server runs on a NAS, next to the photos. It does the work the
desktop app does, but on the NAS itself:
- indexes the photo folder, which is fast because the disk is local,
- makes the thumbnails,
- finds the people and works out the memories and trips.

Then it serves the Phototek interface to any browser on the home network:
other computers, phones, tablets, and TV browsers such as LG's webOS or
Samsung's Tizen. Later the mobile and TV apps will use the same server.

The desktop apps (Mac, Windows, Linux) don't change and don't need it.

> **Changes:** favourites, albums, hidden photos and people's names can be
> changed in the browser. The server records them in the library's shared
> logs (`.photolib/devices/`), like every desktop app, so everyone sees them,
> and it reads the apps' changes at each scan. It never changes, moves or
> deletes a photo file. Imports, exports and edits applied to files stay in
> the desktop app.

## What's in it

| | |
|---|---|
| `src-tauri/core/` | The library code, shared with the desktop app |
| `src-tauri/server/src/main.rs` | Settings, routes and the password check |
| `src-tauri/server/src/library.rs` | The library and its background scans |
| `src-tauri/server/src/commands.rs` | `POST /api/<command>`: the desktop app's commands, same arguments and results |
| `src-tauri/server/src/access.rs` | Password, app tokens (`POST /api/login`) |
| `GET /api/events` | The app's events (scan progress, new photos, changes from other devices) as server-sent events |
| `src-tauri/server/src/media.rs` | `/thumb/<grid\|preview>/<fingerprint>` and `/photo/<fingerprint>` (with ranges, for videos) |
| `src/lib/platform.ts` | Browser mode: where the interface notices it isn't in Tauri |
| `src/lib/api.ts` | `invoke` goes to `/api/…` in a browser |

## Settings

| Variable | Meaning | Default |
|---|---|---|
| `PHOTOTEK_LIBRARY` | The photo folder | `/photos` |
| `PHOTOTEK_DATA` | Index, thumbnails, face models | `/data` |
| `PHOTOTEK_WEB` | The interface's files | `/web` |
| `PHOTOTEK_PORT` | The port | `8080` |
| `PHOTOTEK_PASSWORD` | Asked by the browser (any user name) | none |
| `PHOTOTEK_SCAN_EVERY` | Minutes between scans; `0` = at start only; `off` = only with "Check for new" | `30` |
| `PHOTOTEK_FACES` | `1` = find and group faces (downloads the models, 39 MB, once) | off |

## Install on an ASUSTOR NAS

1. **Docker.** In **App Central**, install **Docker Engine** and
   **Portainer CE**.
2. **Folders.** In **File Explorer**, create a folder for the server's data,
   e.g. `Docker/phototek`. Note the path of your photos share, e.g.
   `/volume1/Photos`. The first scan of a large library takes a while; after
   that, only new photos are read.
3. **The image** is published with each release as
   `ghcr.io/webcmslu/phototek-server` (for Intel and AMD processors, which
   covers most ASUSTOR models, the AS6208T among them). Nothing to do here:
   the stack below downloads it. To build it yourself instead (e.g. for an
   ARM model, `--platform linux/arm64`):
   ```sh
   docker buildx build --platform linux/amd64 -f src-tauri/server/Dockerfile \
     -t phototek-server --output type=docker,dest=phototek-server.tar .
   ```
   then import the `.tar` in Portainer (**Images → Import**) and write
   `image: phototek-server` in the stack.
4. **The container.** In Portainer, go to **Stacks → Add stack**, paste
   `src-tauri/server/compose.yaml`, change the folders and the password,
   then **Deploy**.
5. **Updates.** In Portainer, open the stack and choose **Update the stack**
   with **Re-pull image** on, or pin a version (`:0.1.20`) in `image:`.
6. **Open it.** Go to `http://<NAS address>:8080` in a browser. On a phone,
   "Add to Home Screen" makes it look like an app.

### Outside the house

Don't open port 8080 on the router. Use ASUSTOR's **EZ-Connect** or a VPN
(e.g. Tailscale, which has an ASUSTOR app), so that only your devices can
reach it.

## From the desktop app

On the welcome screen, **Connect to a Phototek server…** asks for the
address (e.g. `nas.local:8080`) and the password, if there is one. The app
then shows the server's library. It is remembered under **Phototek
servers**, and "‹ Libraries" goes back to the computer's own libraries.

This is also how several computers share one library: each one opens it
through the server instead of indexing it again. The mobile apps will
connect the same way.

**How access works** (`server/src/access.rs`):
- **Browsers** get the password prompt (HTTP Basic).
- **Apps** call `POST /api/login` once and get a token. They send it as
  `Authorization: Bearer …`, or as `?t=…` on photo URLs, since an `<img>`
  can't send headers.
- **Tokens** survive restarts. Only their hashes are stored, in
  `<data>/sessions.json`, and a new password cancels them all.
- **CORS** lets the apps (`tauri://localhost` and the like) call the
  server. WebKit allows these `http://` requests from the app's page once
  the server answers with CORS (tested in WKWebView).

## Running it on a computer (development)

```sh
npm run build                                   # the interface, in dist/
cd src-tauri
PHOTOTEK_LIBRARY=~/Pictures PHOTOTEK_DATA=/tmp/phototek PHOTOTEK_WEB=../dist \
  cargo run -p phototek-server
```

Then open http://localhost:8080.

## Next

- **An ARM image** (`arm64`), for the few ARM NAS.
- **Mobile apps** (Tauri 2 for iOS and Android) and **TV apps** (webOS,
  Tizen), on the same API.
