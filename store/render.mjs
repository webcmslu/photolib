#!/usr/bin/env node
/**
 * Renders index.html — the copy-paste sheet used while filling in App Store Connect
 * and the Play Console — from common.json + one JSON file per released version.
 *
 *   node Documentation/store/render.mjs
 *
 * Exits non-zero if any field is over its store limit, so a too-long string is
 * caught here rather than by the console's own validation mid-submission.
 */

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const DIR = dirname(fileURLToPath(import.meta.url));
const read = (f) => JSON.parse(readFileSync(join(DIR, f), "utf8"));

/* Store limits, in characters. Keep in sync with Apple's and Google's own fields. */
const LIMITS = {
  "App Store · App Name": 30,
  "App Store · Subtitle": 30,
  "App Store · Promotional Text": 170,
  "App Store · Description": 4000,
  "App Store · Keywords": 100,
  "App Store · What's New": 4000,
  "App Store · Review Notes": 4000,
  "Play · App Name": 30,
  "Play · Short Description": 80,
  "Play · Full Description": 4000,
  "Play · What's New": 500,
};

const common = read("common.json");

const versionFiles = readdirSync(DIR)
  .filter((f) => /^\d+\.\d+\.\d+\.json$/.test(f))
  .sort((a, b) => cmpSemver(b.replace(".json", ""), a.replace(".json", "")));

if (versionFiles.length === 0) {
  console.error("No version files found in", DIR);
  process.exit(1);
}

const versions = versionFiles.map((f) => read(f));

function cmpSemver(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (pa[i] !== pb[i]) return pa[i] - pb[i];
  }
  return 0;
}

/* ---------------------------------------------------------------- validate */

const locales = common.app.locales;
const play = common.app.stores.includes("play"); // Play fields are checked once Android ships
const problems = [];
const report = [];

function check(label, text, limit) {
  const len = [...text].length;
  const pct = Math.round((len / limit) * 100);
  const flag = len > limit ? "OVER" : pct >= 92 ? "tight" : "ok";
  if (len > limit) problems.push(`${label} — ${len} chars, limit ${limit}`);
  report.push({ label, len, limit, pct, flag });
}

check("App Store · Review Notes", common.review.notes, LIMITS["App Store · Review Notes"]);

for (const l of locales) {
  const L = common.listing;
  check(`App Store · App Name [${l}]`, common.app.storeTitle[l], LIMITS["App Store · App Name"]);
  if (play) check(`Play · App Name [${l}]`, common.app.storeTitle[l], LIMITS["Play · App Name"]);
  check(`App Store · Subtitle [${l}]`, L.appStore.subtitle[l], LIMITS["App Store · Subtitle"]);
  check(`App Store · Promotional Text [${l}]`, L.appStore.promotionalText[l], LIMITS["App Store · Promotional Text"]);
  check(`App Store · Description [${l}]`, L.appStore.description[l], LIMITS["App Store · Description"]);
  check(`App Store · Keywords [${l}]`, L.appStore.keywords[l], LIMITS["App Store · Keywords"]);
  if (!play) continue;
  check(`Play · Short Description [${l}]`, L.play.shortDescription[l], LIMITS["Play · Short Description"]);
  check(`Play · Full Description [${l}]`, L.play.fullDescription[l], LIMITS["Play · Full Description"]);
}

for (const v of versions) {
  for (const l of locales) {
    check(`App Store · What's New [${v.version} ${l}]`, v.whatsNew.appStore[l], LIMITS["App Store · What's New"]);
    if (play) check(`Play · What's New [${v.version} ${l}]`, v.whatsNew.play[l], LIMITS["Play · What's New"]);
  }
  for (const alt of v.alternates ?? []) {
    const limit = alt.field === "appStore.promotionalText" ? 170 : 4000;
    for (const l of locales) {
      check(`Alternate · ${alt.field} [${v.version} ${l}]`, alt.values[l], limit);
    }
  }
}

/* ------------------------------------------------------------------ render */

const payload = JSON.stringify({
  generatedAt: new Date().toISOString().slice(0, 16).replace("T", " ") + " UTC",
  common,
  versions,
})
  .replace(/</g, "\\u003c")
  .replace(/>/g, "\\u003e")
  .replace(/\u2028/g, "\\u2028")
  .replace(/\u2029/g, "\\u2029");

const html = readFileSync(join(DIR, "template.html"), "utf8")
  .replace("/*{{STYLE}}*/", () => readFileSync(join(DIR, "sheet.css"), "utf8"))
  .replace("/*{{SCRIPT}}*/", () => readFileSync(join(DIR, "sheet.js"), "utf8"))
  .replace("/*{{DATA}}*/", () => payload);

writeFileSync(join(DIR, "index.html"), html);

/* ------------------------------------------------------------------ output */

const tight = report.filter((r) => r.flag !== "ok");
console.log(`Rendered index.html — ${versions.length} version(s), ${locales.length} locales, ${report.length} fields checked.`);

if (tight.length) {
  console.log("\nFields at or over their limit:");
  for (const r of tight) {
    console.log(`  ${r.flag === "OVER" ? "✗" : "!"} ${r.label.padEnd(46)} ${String(r.len).padStart(5)} / ${r.limit}  (${r.pct}%)`);
  }
}

if (problems.length) {
  console.error(`\n${problems.length} field(s) exceed the store limit — fix the JSON before submitting.`);
  process.exit(1);
}

console.log("All fields within their store limits.");
