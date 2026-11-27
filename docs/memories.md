# Collections and Memories

How Phototek builds the Collections page, and how it finds, chooses and
plays memories.

Everything is computed on the computer from the library's own data: dates,
GPS positions, folder names, faces, favourites, ratings and edits. Nothing is
sent anywhere, and no AI model is involved. The work takes about a second on
an 80,000-photo library. It happens each time Collections opens, and the
result is kept in memory for the rest of the session. It never changes the
library.

| Part | File |
|---|---|
| Finding and choosing (backend) | `src-tauri/src/collections.rs` |
| Page | `src/lib/CollectionsView.svelte` |
| Player | `src/lib/MemoryPlayer.svelte` |
| Titles and music | `src/lib/memories.ts` |

The single command `collections(path, today)` returns everything the page
shows. `today` is the local date as a day number, so the choices follow the
user's calendar.

---

## 1. Home

Trips need to know where home is. For each year, the world is cut into
squares of 0.2° (about 20 km). Home is the square where photos were taken on
the most **different days**: a day counts twice in its own year and once in
the years either side.

Home can therefore move from year to year, for example after moving house.
A year without GPS photos uses the nearest year that has some.

## 2. Finding memories

### Trips (GPS)

- Each day gets the median position of its geotagged photos.
- A day is **away** when that position is more than **80 km** from that
  year's home.
- Away days no more than **3 days** apart join into one trip.
- A trip is kept when it spans several days and has at least 10 photos, or
  when it has at least 25 photos and goes beyond 150 km.
- The title is the town where at least half of the away photos were taken.
  Otherwise it is the country, or the two main towns ("Lisbon & Cascais").
  The place names come from the `place` column ("Town, Country").

### Events (folder names)

- The **shallowest meaningful folder** in a photo's path becomes the event.
  For example, `2013-08 Sardaigne/GoPro` gives "Sardaigne".
- The name is cleaned up:
  - Dates at the start are removed.
  - Trailing numbers and month names are removed.
  - Numbers in the middle stay, so "Anniversaire Théo 6 ans" is unchanged.
- Some folders are not meaningful and are skipped:
  - Camera folders such as `100CANON` and `DSC…`
  - Folders named only with plain words (`photos`, `divers`, file kinds…)
  - Folders named only with month names
- A folder counts as an event when:
  - its photos fit within **21 days** (from the 10th to the 90th percentile,
    so a few stray photos don't matter),
  - it has at least **15 photos**,
  - and it doesn't mostly overlap a trip.
- Events are what give older photos, which have no GPS, their memories.

### Busy days

A day with **60 or more photos** that is not part of a trip or an event
becomes a memory of its own, named after its town.

### People

- **Person:** someone named with at least 40 photos ("Jean-Paul, over the
  years").
- **Together:** two named people seen together in at least 25 photos
  ("Olivier & Emile").

### Calendar

- **Season:** at least 120 photos. Winter takes the year of its January. A
  season is skipped when more than 70% of it is a single trip, since the
  trip already tells that story.
- **Best of the year:** at least 300 photos.
- **On This Day:** photos from earlier years taken within **3 days** of
  today's date, at least 12 of them. It is always shown first.

## 3. Scoring photos

Each photo gets a score:

| Signal | Points |
|---|---|
| Base | 1 |
| Favourite | +3 |
| Rating | +0.8 per star |
| Edited | +1 |
| Each named person (up to 3) | +1 |
| At least one face | +0.5 |
| Selfie (front camera) | −0.5 |
| Longest side under 1000 px | −1.5 |

Screenshots are recognised by their flag, or by having no camera model and a
screenshot-like file name.

## 4. Picking a memory's photos

1. Videos and screenshots are left out.
2. Only the best shot of each burst is kept.
3. Near-duplicates are dropped. These are shots taken less than 2 minutes
   apart whose visual fingerprints differ by 8 bits or fewer (out of 64).
   Each shot is compared with the last 4 kept.
4. The memory is cut into equal stretches of time, and the best photo of
   each stretch is taken, so the whole trip is told and not only its busiest
   day. A memory with `n` photos gets √n × 2.5 stretches, between 12 and 45.

The **cover** is the photo with the best score, with a bonus for photos with
people in them.

"See all photos" at the end of a memory opens its full set:
- trip: its days
- event: its folder
- person: their photos
- other memories: the chosen photos

## 5. Today's choice

- Memories are drawn at random, weighted by quality. Each memory gets the key
  `u^(1/weight)`, where `u` comes from `blake3("{today}/{id}")`. Good
  memories come up more often, older ones still come back, and the choice
  stays the same all day but changes the next day.
- **Quotas per day:**
  - trips and events: 3
  - people: 2
  - together, season and year: 1 each
  - On This Day first, when there is one
- **Featured Photos:** the best photo of each day with a score of at least
  2.5. The top 480 are drawn down to 60 with the same daily key.
- **Recent Days:** the last 12 days with photos.

## 6. Playing

- **Title card:** the cover goes from blurred to sharp, then the title and
  subtitle rise in (4.2 s).
- **Slides:** each photo stays 4.6 s, with one of six slow pan and zoom
  movements and a 1.2 s cross-fade. Upright or very wide photos are shown
  whole over a blurred copy of themselves, so they aren't cropped. Each
  slide loads the full image and uses the preview if the full image takes
  longer than 2.5 s.
- **Controls:** progress bars along the top, and controls that hide after
  2.5 s.

  | Key | Action |
  |---|---|
  | Space | pause |
  | ← / → | previous / next |
  | M | music on or off |
  | Esc | close |

  The music setting is remembered on each computer
  (`photolib.memoryMusic`).
- **Music:** played live with WebAudio, so there is no file to license or
  download. Each time a memory plays, one of two styles is picked at random:
  - **Dark pads:** low four-note triangle-wave chords that change every
    4.8 s, with sine bell notes over them. One of 3 progressions.
  - **Light:** a soft piano-like arpeggio over a quiet pad and a low bass
    note, with an occasional high bell. One of 4 progressions, 3 arpeggio
    patterns and 3 tempos.

  Both go through a reverb made of decaying noise, a low-pass filter and a
  limiter. Both are set to the same loudness: about −20 dB on average,
  peaking at about half of full scale. The chords (and the light style's
  pattern and tempo) come from the memory's id, so a memory always plays the
  same progression in a given style.

  The app's web view keeps sound paused unless it starts from a click, so
  the audio is woken by the click that opens a memory (`unlockAudio`).

## 7. Tuning

The thresholds are constants at the top of `collections.rs`. To try changes
on a real library without the app, run:

```sh
cd src-tauri
PHOTOTEK_COLLECTIONS=<folder with index.db and faces.db> \
PHOTOTEK_COLLECTIONS_JSON=/tmp/collections.json \
cargo test collections_of_a_real_library -- --nocapture
```

Without `PHOTOTEK_COLLECTIONS` the test does nothing. With it, the test prints the home, trips and memories it finds. It also writes them to
the JSON file, together with people and albums.

## Ideas for later

- Memories with video clips.
- Music chosen by the user.
- Choosing home by hand.
- Trips before GPS, from folder names that are place names.
- Keeping the computed collections between sessions.
