// The TV app (LG webOS first): the same Svelte code as the other apps, with
// its own page for the remote control (src/tv/), built into dist-tv/ with
// relative paths so it runs from the app package (tools/build-tv.sh).
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { fileURLToPath } from "node:url";

export default defineConfig({
  root: fileURLToPath(new URL("./tv", import.meta.url)),
  base: "./",
  plugins: [svelte({ configFile: fileURLToPath(new URL("./svelte.config.js", import.meta.url)) })],
  build: {
    outDir: fileURLToPath(new URL("./dist-tv", import.meta.url)),
    emptyOutDir: true,
    // webOS 6 (2021 TVs) runs Chromium 79.
    target: "chrome79",
  },
  server: { port: 1430 },
});
