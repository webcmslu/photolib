# Search by content

Typing "beach", "plage" or "Strand" in the search box finds the photos that
show a beach, as well as those whose folder, camera or place has the word.
It works in the desktop app (Settings → Search) and on the Phototek server
(`PHOTOTEK_CONTENT=1`).

## How it works

1. **The model.** OpenAI's CLIP (ViT-B/32, MIT licence), as exported to ONNX
   by [Xenova](https://huggingface.co/Xenova/clip-vit-base-patch32). Only its
   image half runs in the app: `clip_vit_b32_vision.onnx`, 352 MB, downloaded
   once into the app's data folder and checked against its BLAKE3 hash. The
   same file is mirrored in the repository's `models` release with its
   licence. It runs with `tract` (pure Rust), like the face models.
2. **Each photo** (and video, from its thumbnail) becomes 512 numbers, a
   point, from its grid thumbnail (center 224 × 224). About 40 ms a photo on
   an Apple M chip, on half the cores: 60,000 photos take about 10 minutes.
   On a NAS Celeron without AVX, count a few seconds a photo: a day or two
   the first time, then only new photos.
3. **The concepts.** `core/data/concepts.tsv` lists about 220 things people
   photograph (places, animals, food, events, activities, objects, documents),
   each with a short English description for CLIP and the words that find it
   in English, French and German. Their points, made once with CLIP's text
   half (250 MB, not shipped), are in `core/data/concepts.bin`.
4. **Tags.** A photo is tagged with a concept when CLIP is clearly sure
   (a 30% share of all the concepts), or when the concept is among the
   photo's first 4 and stands out by 2.6 standard deviations from the others
   (`clip.rs`). That gives about 2.6 tags a photo. Tags and points are kept
   in `content.db` next to the index; changing the list or the rule only
   works the tags out again from the points, in seconds.
5. **Search.** A word that names a concept (`clip::concepts_for`, accents
   and case ignored, multi-word phrases like "salle de bain" kept together)
   also matches the photos tagged with it (`search.rs`).

## Tuning notes (2026-11-28, on the family library)

- On a random 3,000-photo sample, even the weakest "beach" matches were all
  beaches; the beach photos left out were mostly sea views (tagged "sea").
  "Snow" and "cake" were as clean at the margin. Events are looser:
  "Christmas" also brings indoor family scenes in winter.
- The quantized model (89 MB) runs 7× slower in tract and gives different
  points: the full model is the better choice.
- Without its text half, the app can't search for any free text: only the
  words in the list. Adding a concept is cheap (below).

## Add or change concepts

1. Edit `core/data/concepts.tsv` (a key in lower case, the description, then
   the English, French and German words; a word can be several words).
2. Download `text_model.onnx` and `tokenizer.json` from
   https://huggingface.co/Xenova/clip-vit-base-patch32 into a folder, then:
   ```sh
   cd src-tauri/core
   cargo run --release --example clip_concepts -- <that folder>
   ```
   This rewrites `concepts.bin`. The app works the tags out again at the
   next scan.
