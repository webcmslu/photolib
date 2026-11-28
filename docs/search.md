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
   photo's first 4 and stands out by 2.6 standard deviations from the others.
   Once the library has 300 photos, the concept must also be 2 standard
   deviations above how close it usually is to *this library's* photos:
   some concepts sit close to every family photo. That calibration is worked
   out again whenever the library grows by a quarter (`clip.rs`). Tags and
   points are kept in `content.db` next to the index; changing the list or
   the rule only works the tags out again from the points (15 seconds for
   58,000 photos).
5. **Search.** A word that names a concept (`clip::concepts_for`, accents
   and case ignored, multi-word phrases like "salle de bain" kept together)
   also matches the photos tagged with it (`search.rs`).

## Tuning notes (2026-11-28, on the family library: 58,357 photos)

- Looking at all of them took 17 minutes on the MacBook Pro.
- With the first rule alone, "birthday" tagged 6,849 photos: any gathering
  around a table. Two fixes: concrete descriptions ("a birthday cake with
  lit candles", not "a birthday party"), and the per-library calibration.
  Birthday went down to 757, mostly cakes, candles and banners.
- Searches checked by eye on random results: beach (2,638), dog (34), wedding
  (1,694), cake (753), snow (507), fireworks (244), horse (162). Beach and
  dog were all right; wedding brought wedding dresses and ceremonies; sledding
  is the weakest (snowy scenes, some sleds).
- A concept described by one vague word ("sledding", "party") draws too much:
  describe what it looks like.
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
