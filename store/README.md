# Store listing data

Everything App Store Connect and the Google Play Console ask for, held as data
and rendered into **`index.html`**: a copy-paste sheet in the order the consoles
ask for their fields, with live character counts and a Copy button per field.

The same setup as CasaFlow and LetzDrive (`Documentation/store/` there), for
English, French and German, and both stores.

## Files

| File | What it holds |
|---|---|
| `common.json` | The current listing (subtitle, promotional text, description, keywords, Play texts), plus app identity, URLs, App Review notes, App Privacy answers, age rating, screenshot specs and the guardrails. |
| `<version>.json` | One per store release: `whatsNew` (per store, per locale; empty on 1.0.0), `builds`, `notes`, `holds`, `alternates`. |
| `render.mjs` | Checks every field against its store limit, then writes `index.html`. |
| `sheet.css` / `sheet.js` / `template.html` | The page. Edit these. |
| `index.html` | **Generated** and not committed: the next render overwrites it. |

This repository is public: personal contact details (phone, email) go
straight into the consoles, never into these files.

## Render and open

```bash
npm run store        # render + check
npm run store:open   # render, then open it in the browser
```

`render.mjs` prints every field that is at 92% of its limit or more, and
**exits non-zero if a field is over**.

## Add a version

Copy the newest `<version>.json`, bump it, write `whatsNew` in the three
languages, and render. The release steps are in `docs/mobile.md`.
