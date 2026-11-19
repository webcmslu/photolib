// Lists the texts of the UI that are missing from a translation, and the
// translations no longer used. Run by `npm run check`; fails on missing ones.
//
//   node tools/i18n-check.mjs          check
//   node tools/i18n-check.mjs --list   print every text, one JSON string per line

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const SRC = "src";
const LOCALES = ["fr", "de"];

function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "locales" ? [] : files(path);
    return /\.(svelte|ts)$/.test(entry.name) ? [path] : [];
  });
}

/** A JS string literal starting at `at` (a quote), unescaped, and where it ends. */
function literal(source, at) {
  const quote = source[at];
  let text = "";
  for (let i = at + 1; i < source.length; i++) {
    const c = source[i];
    if (c === "\\") {
      text += JSON.parse(`"\\${source[++i]}"`);
    } else if (c === quote) {
      return { text, end: i + 1 };
    } else {
      text += c;
    }
  }
  return null;
}

function textsIn(source) {
  const found = [];
  const call = /\b(t|tn)\(\s*/g;
  let match;
  while ((match = call.exec(source))) {
    let at = call.lastIndex;
    if (match[1] === "tn") {
      // Skip the count: up to the first comma outside brackets.
      let depth = 0;
      for (; at < source.length; at++) {
        const c = source[at];
        if ("([{".includes(c)) depth++;
        else if (")]}".includes(c)) depth--;
        else if (c === "," && depth === 0) break;
      }
      at++;
    }
    for (let n = 0; n < (match[1] === "tn" ? 2 : 1); n++) {
      while (/\s/.test(source[at])) at++;
      if (source[at] !== '"' && source[at] !== "'") break;
      const lit = literal(source, at);
      if (!lit) break;
      found.push(lit.text);
      at = lit.end;
      while (/[\s,]/.test(source[at])) at++;
    }
  }
  // Texts translated through a variable: the Rust messages and "System language".
  const block = /const RUST_MESSAGES = \[([\s\S]*?)\];/.exec(source);
  if (block) for (const m of block[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)) found.push(JSON.parse(`"${m[1]}"`));
  const system = /system: "([^"]+)"/.exec(source);
  if (system && source.includes("LANGUAGES")) found.push(system[1]);
  return found;
}

const texts = new Set(files(SRC).flatMap((path) => textsIn(readFileSync(path, "utf8"))));

if (process.argv.includes("--list")) {
  for (const text of [...texts].sort()) console.log(JSON.stringify(text));
  process.exit(0);
}

let failed = false;
for (const locale of LOCALES) {
  const path = join(SRC, "lib", "locales", `${locale}.ts`);
  const source = readFileSync(path, "utf8");
  const keys = new Set();
  // Entries are written one per line: "English": "Translation",
  for (const m of source.matchAll(/^\s*("(?:[^"\\]|\\.)*")\s*:/gm)) keys.add(JSON.parse(m[1]));
  const missing = [...texts].filter((text) => !keys.has(text));
  const unused = [...keys].filter((key) => !texts.has(key));
  // Each {name} of the English must be in the translation too.
  const entries = new Map(
    [...source.matchAll(/^\s*("(?:[^"\\]|\\.)*")\s*:\s*("(?:[^"\\]|\\.)*")/gm)].map((m) => [JSON.parse(m[1]), JSON.parse(m[2])]),
  );
  const broken = [...entries].filter(([en, tr]) => {
    const names = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join();
    return names(en) !== names(tr);
  });
  if (missing.length || broken.length) failed = true;
  console.log(`${locale}: ${texts.size - missing.length} of ${texts.size} translated`);
  for (const text of missing) console.log(`  missing: ${JSON.stringify(text)}`);
  for (const [en] of broken) console.log(`  placeholders differ: ${JSON.stringify(en)}`);
  for (const key of unused) console.log(`  unused: ${JSON.stringify(key)}`);
}
process.exit(failed ? 1 : 0);
