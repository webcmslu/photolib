//! The Collections page: memories, trips, featured photos and recent days,
//! worked out from what the index already knows: dates, places, people,
//! favourites, ratings, folder names and the visual fingerprints. Nothing is
//! stored; it is computed when the page opens (well under a second for a
//! library of 80,000 photos).
//!
//! - **Home** is where the most days were photographed, year by year (people
//!   move). A day is away when its photos are far from that year's home.
//! - **Trips** are runs of away days, named after the town most of the
//!   photos were taken in, or else after the country.
//! - **Events** come from folder names: "2013-08 Sardaigne/GoPro" holds the
//!   event "Sardaigne". They reach back to before phones had GPS.
//! - **Memories** are trips, events, a person over the years, two people
//!   together, a season, a year, and the photos taken on this day in earlier
//!   years. Each one plays a few dozen of its best photos, spread over time:
//!   one per burst, no near-duplicates, favourites and faces first.
//! - Which memories and featured photos the page shows changes every day.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;
use serde::Serialize;

use crate::db::{self, Listing, Only, DAY_MS};

/// A day is away from home beyond this distance.
const AWAY_KM: f64 = 80.0;
/// Away days this far apart (or closer) belong to the same trip.
const TRIP_GAP_DAYS: i64 = 3;
/// The side of the squares home is looked for in, in degrees (about 20 km).
const HOME_CELL: f64 = 0.2;
/// An event folder's photos fit in this many days (outliers aside).
const EVENT_DAYS: i64 = 21;
const EVENT_PHOTOS: usize = 15;
/// A day with this many photos is an event even without a folder name.
const BUSY_DAY: usize = 60;
const PERSON_PHOTOS: usize = 40;
const TOGETHER_PHOTOS: usize = 25;
const SEASON_PHOTOS: usize = 120;
const YEAR_PHOTOS: usize = 300;
/// Days on either side of today's date that count for "On This Day".
const THIS_DAY_REACH: i64 = 3;
const THIS_DAY_PHOTOS: usize = 12;
/// Visual fingerprints this close are the same shot taken twice.
const NEAR_DUPLICATE_BITS: u32 = 8;
/// Shots this close in time are compared for near-duplicates.
const NEAR_DUPLICATE_MS: i64 = 120_000;
/// Featured photos offered at a time (the page shows the first few).
const FEATURED: usize = 60;
const RECENT_DAYS: usize = 12;
/// Videos this long (in ms) play in memories, as short clips.
const CLIP_MS: std::ops::RangeInclusive<i64> = 2_000..=60_000;
/// At most one slide in this many is a clip.
const CLIP_SHARE: usize = 5;

/// Everything on the Collections page that is not already elsewhere.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collections {
    /// Every memory, newest first.
    pub memories: Vec<Memory>,
    /// The memories shown today: positions in `memories`, in order.
    pub today: Vec<usize>,
    pub trips: Vec<Trip>,
    pub featured: Vec<String>,
    pub recent_days: Vec<RecentDay>,
    /// Photos per media type (only the ones that have some).
    pub media: HashMap<Only, u32>,
    pub favorites: u32,
    pub hidden: u32,
    pub duplicates: u32,
    /// The town home is in, these days.
    pub home: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    Trip,
    /// From a folder name, or a busy day.
    Event,
    OnThisDay,
    /// A person over the years.
    Person,
    Together,
    Season,
    Year,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: String,
    pub kind: MemoryKind,
    /// A place, a folder's name, or a person's name. The page words the rest.
    pub title: Option<String>,
    /// The named people it is about (Together), or who is in it most.
    pub people: Vec<String>,
    /// Its first and last day (days since 1970).
    pub from_day: i64,
    pub to_day: i64,
    /// For "On This Day": the earlier years it has photos from.
    pub years: Vec<i32>,
    pub cover: String,
    /// The photos it plays, in date order.
    pub photos: Vec<String>,
    /// All its photos, which `listing` shows.
    pub count: u32,
    pub listing: Listing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trip {
    /// The same as its memory's.
    pub id: String,
    pub title: String,
    pub from_day: i64,
    pub to_day: i64,
    pub count: u32,
    pub cover: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentDay {
    pub day: i64,
    pub count: u32,
    pub cover: String,
}

/// One listed photo, with what the collections need to know about it.
struct Shot {
    fingerprint: String,
    time: i64,
    day: i64,
    /// The date is the camera's, not the file's.
    dated: bool,
    position: Option<(f64, f64)>,
    /// The town, without the country.
    town: Option<String>,
    country: Option<String>,
    folder: String,
    video: bool,
    /// A short video, which can play in a memory.
    clip: bool,
    screenshot: bool,
    burst: Option<String>,
    visual: Option<u64>,
    /// Indices into the names.
    people: Vec<u32>,
    score: f32,
}

impl Shot {
    /// Can be shown as a photo: in a memory, as its cover or as a featured photo.
    fn showable(&self) -> bool {
        !self.video && !self.screenshot
    }

    /// Can play in a memory: a photo, or a short clip.
    fn in_memory(&self) -> bool {
        self.showable() || self.clip
    }
}

/// `today` is the viewer's date (days since 1970, wall-clock).
pub fn collections(conn: &Connection, today: i64) -> rusqlite::Result<Collections> {
    let library = Library::load(conn)?;
    let mut collections = library.build(today);

    let mut kinds = vec![Only::Videos, Only::Raw];
    kinds.extend([
        Only::Selfies,
        Only::LivePhotos,
        Only::Portraits,
        Only::Panoramas,
        Only::TimeLapses,
        Only::SloMos,
        Only::Bursts,
        Only::Screenshots,
        Only::Favorites,
    ]);
    let mut media = db::only_counts(conn, &kinds)?;
    collections.favorites = media.remove(&Only::Favorites).unwrap_or(0);
    media.retain(|_, count| *count > 0);
    collections.media = media;
    (collections.hidden, collections.duplicates) = db::hidden_and_duplicates(conn)?;
    Ok(collections)
}

struct Library {
    /// In date order.
    shots: Vec<Shot>,
    names: Vec<String>,
    /// The face groups of each name, to list a person's photos.
    clusters: Vec<Vec<i64>>,
}

impl Library {
    fn load(conn: &Connection) -> rusqlite::Result<Library> {
        let marked = |kind: &str| -> rusqlite::Result<HashMap<String, String>> {
            conn.prepare("SELECT key, value FROM org_state WHERE kind = ?1 AND value != ''")?
                .query_map([kind], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        };
        let favorites = marked("fav")?;
        let ratings = marked("rating")?;
        let edits = marked("edit")?;

        let visual: HashMap<String, u64> = conn
            .prepare("SELECT fingerprint, hash FROM visual")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<_>>()?;

        // Who is in which photo, by name.
        let group_names = crate::people::group_names(conn)?;
        let mut names: Vec<String> = Vec::new();
        let mut clusters: Vec<Vec<i64>> = Vec::new();
        let mut name_index: HashMap<String, u32> = HashMap::new();
        let mut index_of_group: HashMap<i64, u32> = HashMap::new();
        let mut sorted: Vec<_> = group_names.into_iter().collect();
        sorted.sort();
        for (cluster, name) in sorted {
            let index = *name_index.entry(name.to_lowercase()).or_insert_with(|| {
                names.push(name.clone());
                clusters.push(Vec::new());
                (names.len() - 1) as u32
            });
            clusters[index as usize].push(cluster);
            index_of_group.insert(cluster, index);
        }
        let mut faces: HashMap<String, (u32, Vec<u32>)> = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT fingerprint, cluster FROM faces.face WHERE cluster > 0")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (fingerprint, cluster) = row?;
            let entry = faces.entry(fingerprint).or_default();
            entry.0 += 1;
            if let Some(&index) = index_of_group.get(&cluster) {
                if !entry.1.contains(&index) {
                    entry.1.push(index);
                }
            }
        }

        let mut stmt = conn.prepare(&format!(
            "SELECT fingerprint, sort_date, date_source = 'exif', latitude, longitude, place,
                    folder, kind, media, burst, width, height, lens, camera_model, rel_path,
                    duration_ms
             FROM photos WHERE {}
             ORDER BY sort_date, rel_path",
            db::visible_condition()
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                (
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                ),
                row.get::<_, Option<i64>>(15)?,
            ))
        })?;
        let mut shots = Vec::new();
        for row in rows {
            let (
                fingerprint,
                time,
                dated,
                latitude,
                longitude,
                place,
                folder,
                kind,
                media,
                burst,
                (width, height, lens, camera_model, rel_path),
                duration,
            ) = row?;
            let video = kind == 1;
            let clip = video && duration.is_some_and(|ms| CLIP_MS.contains(&ms));
            let screenshot = !video
                && ((media as u32 & crate::metadata::SCREENSHOT) != 0
                    || (camera_model.is_none() && looks_like_screenshot(&rel_path)));
            let (town, country) = match place.as_deref().and_then(|p| p.rsplit_once(", ")) {
                Some((town, country)) => (Some(town.to_owned()), Some(country.to_owned())),
                None => (None, None),
            };
            let (face_count, people) = faces.remove(&fingerprint).unwrap_or_default();
            let longest = width.unwrap_or(0).max(height.unwrap_or(0));
            let selfie = lens.is_some_and(|lens| lens.to_lowercase().contains("front"));
            let mut score = 1.0;
            if favorites
                .get(&fingerprint)
                .is_some_and(|value| value == "1")
            {
                score += 3.0;
            }
            if let Some(stars) = ratings
                .get(&fingerprint)
                .and_then(|v| v.parse::<f32>().ok())
            {
                score += 0.8 * stars;
            }
            if edits.contains_key(&fingerprint) {
                score += 1.0;
            }
            score += people.len().min(3) as f32;
            if face_count > 0 {
                score += 0.5;
            }
            if selfie {
                score -= 0.5;
            }
            if longest > 0 && longest < 1000 {
                score -= 1.5;
            }
            shots.push(Shot {
                visual: visual.get(&fingerprint).copied(),
                fingerprint,
                time,
                day: time.div_euclid(DAY_MS),
                dated,
                position: latitude.zip(longitude),
                town,
                country,
                folder,
                video,
                clip,
                screenshot,
                burst,
                people,
                score,
            });
        }
        Ok(Library {
            shots,
            names,
            clusters,
        })
    }

    fn build(&self, today: i64) -> Collections {
        let homes = homes(&self.shots);
        let trips = self.trips(&homes);
        let in_trip = |day: i64| trips.iter().any(|trip| day >= trip.from && day <= trip.to);

        let mut memories: Vec<Memory> = Vec::new();
        for trip in &trips {
            memories.push(self.memory(
                format!("trip-{}", trip.from),
                MemoryKind::Trip,
                Some(trip.title.clone()),
                &trip.shots,
                Listing::Days {
                    from: trip.from,
                    to: trip.to,
                },
            ));
        }
        let events = self.events(&in_trip);
        let event_days: HashSet<i64> = events
            .iter()
            .flat_map(|(_, _, shots)| shots.iter().map(|&i| self.shots[i].day))
            .collect();
        for (folder, title, shots) in events {
            memories.push(self.memory(
                format!("event-{folder}"),
                MemoryKind::Event,
                Some(title),
                &shots,
                Listing::Folder {
                    folder,
                    subfolders: true,
                },
            ));
        }
        for (day, shots) in self.busy_days(&in_trip, &event_days) {
            memories.push(self.memory(
                format!("day-{day}"),
                MemoryKind::Event,
                self.town_of(&shots),
                &shots,
                Listing::Days { from: day, to: day },
            ));
        }
        memories.extend(self.people_memories());
        memories.extend(self.season_memories(&trips));
        memories.extend(self.year_memories());
        memories.retain(|memory| !memory.photos.is_empty());
        memories.sort_by(|a, b| b.to_day.cmp(&a.to_day).then(a.id.cmp(&b.id)));

        // On This Day goes first, and only today.
        let mut today_list = Vec::new();
        if let Some(memory) = self.on_this_day(today) {
            memories.insert(0, memory);
            today_list.push(0);
        }
        today_list.extend(pick_today(&memories, today, &today_list));

        let trips_out = trips
            .iter()
            .rev()
            .filter_map(|trip| {
                let id = format!("trip-{}", trip.from);
                let memory = memories.iter().find(|memory| memory.id == id)?;
                Some(Trip {
                    id,
                    title: trip.title.clone(),
                    from_day: trip.from,
                    to_day: trip.to,
                    count: memory.count,
                    cover: memory.cover.clone(),
                })
            })
            .collect();

        Collections {
            featured: self.featured(today),
            recent_days: self.recent_days(),
            home: homes.latest_town.clone(),
            memories,
            today: today_list,
            trips: trips_out,
            ..Default::default()
        }
    }

    /// A memory of the given shots (indices), with its best photos picked.
    fn memory(
        &self,
        id: String,
        kind: MemoryKind,
        title: Option<String>,
        shots: &[usize],
        listing: Listing,
    ) -> Memory {
        let photos = self.curate(shots);
        let cover = self.cover(&photos);
        let from_day = shots.first().map_or(0, |&i| self.shots[i].day);
        let to_day = shots.last().map_or(0, |&i| self.shots[i].day);
        Memory {
            id,
            kind,
            title,
            people: self.main_people(shots),
            from_day,
            to_day,
            years: Vec::new(),
            cover: cover.unwrap_or_default(),
            photos: photos
                .iter()
                .map(|&i| self.shots[i].fingerprint.clone())
                .collect(),
            count: shots.len() as u32,
            listing,
        }
    }

    /// The photos a memory plays: one per burst, no near-duplicates, then
    /// the best of each stretch of time. A few dozen at most.
    fn curate(&self, shots: &[usize]) -> Vec<usize> {
        let mut kept: Vec<usize> = Vec::new();
        let mut best_of_burst: HashMap<&str, usize> = HashMap::new();
        for &i in shots {
            let shot = &self.shots[i];
            if !shot.in_memory() {
                continue;
            }
            if let Some(burst) = shot.burst.as_deref() {
                match best_of_burst.get(burst) {
                    Some(&k) if self.shots[kept[k]].score >= shot.score => continue,
                    Some(&k) => {
                        kept[k] = i;
                        continue;
                    }
                    None => {
                        best_of_burst.insert(burst, kept.len());
                    }
                }
            }
            kept.push(i);
        }
        // The same scene shot twice in a row: keep the better one.
        let mut distinct: Vec<usize> = Vec::with_capacity(kept.len());
        for i in kept {
            let shot = &self.shots[i];
            let twin = distinct.iter().rev().take(4).position(|&k| {
                let other = &self.shots[k];
                shot.time - other.time <= NEAR_DUPLICATE_MS
                    && matches!((shot.visual, other.visual),
                        (Some(a), Some(b)) if (a ^ b).count_ones() <= NEAR_DUPLICATE_BITS)
            });
            match twin {
                Some(back) => {
                    let at = distinct.len() - 1 - back;
                    if shot.score > self.shots[distinct[at]].score {
                        distinct[at] = i;
                    }
                }
                None => distinct.push(i),
            }
        }
        let wanted = ((distinct.len() as f64).sqrt() * 2.5).clamp(12.0, 45.0) as usize;
        let buckets = wanted.min(distinct.len());
        // The best of each of `buckets` equal stretches, with a clip now and
        // then (they play muted, under the music) but mostly photos.
        let clips_allowed = buckets / CLIP_SHARE;
        let mut clips = 0;
        let mut chosen = Vec::with_capacity(buckets);
        for bucket in 0..buckets {
            let start = bucket * distinct.len() / buckets;
            let end = (bucket + 1) * distinct.len() / buckets;
            let best = distinct[start..end]
                .iter()
                .copied()
                .filter(|&i| !self.shots[i].clip || clips < clips_allowed)
                .max_by(|&a, &b| {
                    self.shots[a]
                        .score
                        .total_cmp(&self.shots[b].score)
                        .then(b.cmp(&a))
                });
            if let Some(best) = best {
                clips += usize::from(self.shots[best].clip);
                chosen.push(best);
            }
        }
        chosen
    }

    fn cover(&self, photos: &[usize]) -> Option<String> {
        photos
            .iter()
            .filter(|&&i| self.shots[i].showable())
            .max_by(|&&a, &&b| {
                let rank = |i: usize| {
                    let shot = &self.shots[i];
                    shot.score + if shot.people.is_empty() { 0.0 } else { 0.5 }
                };
                rank(a).total_cmp(&rank(b)).then(b.cmp(&a))
            })
            .map(|&i| self.shots[i].fingerprint.clone())
    }

    /// The named people in at least a fifth of the shots, most seen first.
    fn main_people(&self, shots: &[usize]) -> Vec<String> {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for &i in shots {
            for &person in &self.shots[i].people {
                *counts.entry(person).or_default() += 1;
            }
        }
        let mut people: Vec<(u32, usize)> = counts
            .into_iter()
            .filter(|(_, count)| count * 5 >= shots.len())
            .collect();
        people.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        people
            .into_iter()
            .take(3)
            .map(|(person, _)| self.names[person as usize].clone())
            .collect()
    }

    /// The town most of the shots were taken in, if most were.
    fn town_of(&self, shots: &[usize]) -> Option<String> {
        let mut towns: HashMap<&str, usize> = HashMap::new();
        let mut placed = 0;
        for &i in shots {
            if let Some(town) = self.shots[i].town.as_deref() {
                *towns.entry(town).or_default() += 1;
                placed += 1;
            }
        }
        let (town, count) = towns
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))?;
        (count * 2 > placed).then(|| town.to_owned())
    }

    fn trips(&self, homes: &Homes) -> Vec<FoundTrip> {
        // Where each day was spent: the middle of its photos.
        let mut days: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
        for shot in self.shots.iter().filter(|shot| shot.dated) {
            if let Some(position) = shot.position {
                days.entry(shot.day).or_default().push(position);
            }
        }
        let mut runs: Vec<(i64, i64, f64)> = Vec::new();
        let mut current: Option<(i64, i64, f64)> = None;
        for (&day, positions) in &days {
            let Some(home) = homes.at(year_of(day)) else {
                continue;
            };
            let (latitude, longitude) = middle(positions);
            let distance = crate::places::distance_km(latitude, longitude, home.0, home.1);
            if distance <= AWAY_KM {
                runs.extend(current.take());
                continue;
            }
            current = match current {
                Some((from, to, far)) if day - to <= TRIP_GAP_DAYS => {
                    Some((from, day, far.max(distance)))
                }
                other => {
                    runs.extend(other);
                    Some((day, day, distance))
                }
            };
        }
        runs.extend(current);

        let mut trips = Vec::new();
        for (from, to, farthest) in runs {
            let shots = self.dated_between(from, to);
            let long_enough =
                (to > from && shots.len() >= 10) || (shots.len() >= 25 && farthest >= 150.0);
            if !long_enough {
                continue;
            }
            let Some(title) = self.trip_title(&shots, homes) else {
                continue;
            };
            trips.push(FoundTrip {
                from,
                to,
                title,
                shots,
            });
        }
        trips
    }

    /// Indices of the shots with a camera date from day `from` to day `to`.
    fn dated_between(&self, from: i64, to: i64) -> Vec<usize> {
        let start = self.shots.partition_point(|shot| shot.day < from);
        let end = self.shots.partition_point(|shot| shot.day <= to);
        (start..end).filter(|&i| self.shots[i].dated).collect()
    }

    /// The town most of a trip's photos away from home were taken in, or the
    /// countries.
    fn trip_title(&self, shots: &[usize], homes: &Homes) -> Option<String> {
        let mut towns: HashMap<&str, usize> = HashMap::new();
        let mut countries: HashMap<&str, usize> = HashMap::new();
        let mut placed = 0;
        for &i in shots {
            let shot = &self.shots[i];
            let (Some(town), Some(country), Some(position)) =
                (shot.town.as_deref(), shot.country.as_deref(), shot.position)
            else {
                continue;
            };
            let home = homes.at(year_of(shot.day))?;
            if crate::places::distance_km(position.0, position.1, home.0, home.1) <= AWAY_KM {
                continue;
            }
            *towns.entry(town).or_default() += 1;
            *countries.entry(country).or_default() += 1;
            placed += 1;
        }
        let towns = ranked(towns);
        let (town, count) = *towns.first()?;
        if count * 2 >= placed {
            return Some(town.to_owned());
        }
        let countries = ranked(countries);
        Some(match countries.as_slice() {
            [(only, _)] => (*only).to_owned(),
            [(first, a), (second, b), ..] if *b * 4 >= *a => format!("{first} & {second}"),
            [(first, _), ..] => (*first).to_owned(),
            [] => return None,
        })
    }
}

/// Names by how often they come, most first.
fn ranked(counts: HashMap<&str, usize>) -> Vec<(&str, usize)> {
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ranked
}

impl Library {
    /// Events named after folders: (folder, title, shots).
    fn events(&self, in_trip: &dyn Fn(i64) -> bool) -> Vec<(String, String, Vec<usize>)> {
        let mut by_folder: HashMap<String, (String, Vec<usize>)> = HashMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if !shot.dated {
                continue;
            }
            if let Some((folder, title)) = event_of(&shot.folder) {
                by_folder
                    .entry(folder)
                    .or_insert_with(|| (title, Vec::new()))
                    .1
                    .push(i);
            }
        }
        let mut events: Vec<(String, String, Vec<usize>)> = by_folder
            .into_iter()
            .filter_map(|(folder, (title, shots))| {
                if shots.len() < EVENT_PHOTOS {
                    return None;
                }
                // A few stray dates (a wrong camera clock) don't count.
                let days: Vec<i64> = shots.iter().map(|&i| self.shots[i].day).collect();
                let low = days[days.len() / 10];
                let high = days[days.len() * 9 / 10];
                if high - low > EVENT_DAYS {
                    return None;
                }
                // A trip covers it already.
                let during_trips = days.iter().filter(|&&day| in_trip(day)).count();
                if during_trips * 2 > days.len() {
                    return None;
                }
                let shots: Vec<usize> = shots
                    .into_iter()
                    .filter(|&i| (low..=high).contains(&self.shots[i].day))
                    .collect();
                Some((folder, title, shots))
            })
            .collect();
        events.sort_by(|a, b| a.0.cmp(&b.0));
        events
    }

    /// Busy days that are neither a trip nor a folder's event.
    fn busy_days(
        &self,
        in_trip: &dyn Fn(i64) -> bool,
        event_days: &HashSet<i64>,
    ) -> Vec<(i64, Vec<usize>)> {
        let mut days: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if shot.dated && !shot.video {
                days.entry(shot.day).or_default().push(i);
            }
        }
        days.into_iter()
            .filter(|(day, shots)| {
                shots.len() >= BUSY_DAY && !in_trip(*day) && !event_days.contains(day)
            })
            .collect()
    }

    fn people_memories(&self) -> Vec<Memory> {
        let mut by_person: Vec<Vec<usize>> = vec![Vec::new(); self.names.len()];
        let mut pairs: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if !shot.dated {
                continue;
            }
            for (k, &a) in shot.people.iter().enumerate() {
                by_person[a as usize].push(i);
                for &b in &shot.people[k + 1..] {
                    pairs.entry((a.min(b), a.max(b))).or_default().push(i);
                }
            }
        }
        let mut memories = Vec::new();
        for (person, shots) in by_person.iter().enumerate() {
            let years: HashSet<i32> = shots.iter().map(|&i| year_of(self.shots[i].day)).collect();
            if shots.len() < PERSON_PHOTOS || years.len() < 2 {
                continue;
            }
            let mut memory = self.memory(
                format!("person-{}", self.names[person].to_lowercase()),
                MemoryKind::Person,
                Some(self.names[person].clone()),
                shots,
                Listing::Person {
                    clusters: self.clusters[person].clone(),
                },
            );
            memory.people = vec![self.names[person].clone()];
            memories.push(memory);
        }
        let mut pairs: Vec<_> = pairs
            .into_iter()
            .filter(|(_, shots)| shots.len() >= TOGETHER_PHOTOS)
            .collect();
        pairs.sort_by_key(|((a, b), _)| (*a, *b));
        for ((a, b), shots) in pairs {
            let (a, b) = (&self.names[a as usize], &self.names[b as usize]);
            let fingerprints = shots
                .iter()
                .map(|&i| self.shots[i].fingerprint.clone())
                .collect();
            let mut memory = self.memory(
                format!("together-{}-{}", a.to_lowercase(), b.to_lowercase()),
                MemoryKind::Together,
                None,
                &shots,
                Listing::Picked { fingerprints },
            );
            memory.people = vec![a.clone(), b.clone()];
            memories.push(memory);
        }
        memories
    }

    /// Seasons with many photos that are not mostly one trip.
    fn season_memories(&self, trips: &[FoundTrip]) -> Vec<Memory> {
        let mut seasons: BTreeMap<(i32, u32), Vec<usize>> = BTreeMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if shot.dated {
                seasons.entry(season_of(shot.day)).or_default().push(i);
            }
        }
        let mut memories = Vec::new();
        for ((year, season), shots) in seasons {
            if shots.len() < SEASON_PHOTOS {
                continue;
            }
            let largest_trip = trips
                .iter()
                .map(|trip| {
                    shots
                        .iter()
                        .filter(|&&i| trip.shots.binary_search(&i).is_ok())
                        .count()
                })
                .max()
                .unwrap_or(0);
            if largest_trip * 10 > shots.len() * 7 {
                continue;
            }
            let (from, to) = season_days(year, season);
            memories.push(self.memory(
                format!("season-{year}-{season}"),
                MemoryKind::Season,
                None,
                &shots,
                Listing::Days { from, to },
            ));
        }
        memories
    }

    fn year_memories(&self) -> Vec<Memory> {
        let mut years: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if shot.dated {
                years.entry(year_of(shot.day)).or_default().push(i);
            }
        }
        years
            .into_iter()
            .filter(|(_, shots)| shots.len() >= YEAR_PHOTOS)
            .map(|(year, shots)| {
                let from = days_of(year, 1, 1);
                let to = days_of(year + 1, 1, 1) - 1;
                self.memory(
                    format!("year-{year}"),
                    MemoryKind::Year,
                    None,
                    &shots,
                    Listing::Days { from, to },
                )
            })
            .collect()
    }

    /// The photos taken around today's date in earlier years.
    fn on_this_day(&self, today: i64) -> Option<Memory> {
        let date = date_of(today);
        let mut shots = Vec::new();
        let mut years = Vec::new();
        for year in (date.year() - 100..date.year()).rev() {
            // 29 February becomes the 28th in other years.
            let Some(then) = NaiveDate::from_ymd_opt(year, date.month(), date.day())
                .or_else(|| NaiveDate::from_ymd_opt(year, date.month(), date.day() - 1))
            else {
                continue;
            };
            let center = days_from_date(then);
            let found = self.dated_between(center - THIS_DAY_REACH, center + THIS_DAY_REACH);
            let found: Vec<usize> = found
                .into_iter()
                .filter(|&i| self.shots[i].showable())
                .collect();
            if !found.is_empty() {
                years.push(year);
                shots.extend(found);
            }
        }
        if shots.len() < THIS_DAY_PHOTOS {
            return None;
        }
        shots.sort_unstable();
        let fingerprints = shots
            .iter()
            .map(|&i| self.shots[i].fingerprint.clone())
            .collect();
        let mut memory = self.memory(
            format!("this-day-{today}"),
            MemoryKind::OnThisDay,
            None,
            &shots,
            Listing::Picked { fingerprints },
        );
        years.sort_unstable();
        memory.years = years;
        Some(memory)
    }

    /// Today's featured photos: the best photo of each of the best days,
    /// a different handful every day.
    fn featured(&self, today: i64) -> Vec<String> {
        let mut best_of_day: HashMap<i64, usize> = HashMap::new();
        for (i, shot) in self.shots.iter().enumerate() {
            if !shot.dated || !shot.showable() || shot.score < 2.5 {
                continue;
            }
            let best = best_of_day.entry(shot.day).or_insert(i);
            if shot.score > self.shots[*best].score {
                *best = i;
            }
        }
        let mut candidates: Vec<usize> = best_of_day.into_values().collect();
        candidates.sort_by(|&a, &b| {
            self.shots[b]
                .score
                .total_cmp(&self.shots[a].score)
                .then(a.cmp(&b))
        });
        candidates.truncate(FEATURED * 8);
        let mut keyed: Vec<(f64, usize)> = candidates
            .into_iter()
            .map(|i| {
                let shot = &self.shots[i];
                (weighted_key(today, &shot.fingerprint, shot.score as f64), i)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        keyed
            .into_iter()
            .take(FEATURED)
            .map(|(_, i)| self.shots[i].fingerprint.clone())
            .collect()
    }

    fn recent_days(&self) -> Vec<RecentDay> {
        let mut days: Vec<RecentDay> = Vec::new();
        let mut best = 0.0f32;
        for shot in self.shots.iter().rev() {
            match days.last_mut() {
                Some(last) if last.day == shot.day => {
                    last.count += 1;
                    if shot.showable() && shot.score > best {
                        best = shot.score;
                        last.cover = shot.fingerprint.clone();
                    }
                }
                _ => {
                    if days.len() == RECENT_DAYS {
                        break;
                    }
                    best = if shot.showable() {
                        shot.score
                    } else {
                        f32::MIN
                    };
                    days.push(RecentDay {
                        day: shot.day,
                        count: 1,
                        cover: shot.fingerprint.clone(),
                    });
                }
            }
        }
        days
    }
}

struct FoundTrip {
    from: i64,
    to: i64,
    title: String,
    /// Indices of its dated shots, in order.
    shots: Vec<usize>,
}

/// Where home was, year by year.
struct Homes {
    by_year: BTreeMap<i32, (f64, f64)>,
    latest_town: Option<String>,
}

impl Homes {
    /// Home in `year`, or in the nearest year that has one.
    fn at(&self, year: i32) -> Option<(f64, f64)> {
        let after = self.by_year.range(year..).next();
        let before = self.by_year.range(..year).next_back();
        match (before, after) {
            (Some((&b, &before)), Some((&a, &after))) => {
                Some(if year - b < a - year { before } else { after })
            }
            (Some((_, &home)), None) | (None, Some((_, &home))) => Some(home),
            (None, None) => None,
        }
    }
}

/// The photos taken in one square: the sums of their latitudes and
/// longitudes, how many there are, and the towns they were taken in.
type Spot<'a> = (f64, f64, usize, HashMap<&'a str, usize>);

/// For each year with photos that have a position: the square where the
/// most days were photographed in that year and the years either side.
fn homes(shots: &[Shot]) -> Homes {
    // (year, cell) -> days, and where the photos in each cell were.
    let mut days: HashMap<(i32, (i64, i64)), HashSet<i64>> = HashMap::new();
    let mut spots: HashMap<(i64, i64), Spot> = HashMap::new();
    for shot in shots.iter().filter(|shot| shot.dated) {
        let Some((latitude, longitude)) = shot.position else {
            continue;
        };
        let cell = (
            (latitude / HOME_CELL).floor() as i64,
            (longitude / HOME_CELL).floor() as i64,
        );
        days.entry((year_of(shot.day), cell))
            .or_default()
            .insert(shot.day);
        let spot = spots.entry(cell).or_default();
        spot.0 += latitude;
        spot.1 += longitude;
        spot.2 += 1;
        if let Some(town) = shot.town.as_deref() {
            *spot.3.entry(town).or_default() += 1;
        }
    }
    let years: HashSet<i32> = days.keys().map(|(year, _)| *year).collect();
    let mut by_year = BTreeMap::new();
    let mut latest: Option<(i32, (i64, i64))> = None;
    for year in years {
        let mut score: HashMap<(i64, i64), usize> = HashMap::new();
        for ((other, cell), count) in &days {
            let weight = match (other - year).abs() {
                0 => 2,
                1 => 1,
                _ => 0,
            };
            *score.entry(*cell).or_default() += weight * count.len();
        }
        let Some((cell, _)) = score
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        else {
            continue;
        };
        let spot = &spots[&cell];
        by_year.insert(year, (spot.0 / spot.2 as f64, spot.1 / spot.2 as f64));
        if latest.is_none_or(|(known, _)| year > known) {
            latest = Some((year, cell));
        }
    }
    let latest_town = latest.and_then(|(_, cell)| {
        spots[&cell]
            .3
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(town, _)| (*town).to_owned())
    });
    Homes {
        by_year,
        latest_town,
    }
}

/// Today's memories (besides On This Day): a few of each kind, drawn at
/// random with a seed that changes every day, larger memories more often.
fn pick_today(memories: &[Memory], today: i64, taken: &[usize]) -> Vec<usize> {
    let quota = |kind: MemoryKind| match kind {
        MemoryKind::Trip | MemoryKind::Event => 3,
        MemoryKind::Person => 2,
        MemoryKind::Together | MemoryKind::Season | MemoryKind::Year => 1,
        MemoryKind::OnThisDay => 0,
    };
    let mut keyed: Vec<(f64, usize)> = memories
        .iter()
        .enumerate()
        .filter(|(i, _)| !taken.contains(i))
        .map(|(i, memory)| {
            let weight = (memory.count as f64 + 1.0).ln();
            (weighted_key(today, &memory.id, weight), i)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut used: HashMap<MemoryKind, usize> = HashMap::new();
    let mut picked = Vec::new();
    for (_, i) in keyed {
        let kind = memories[i].kind;
        let count = used.entry(kind).or_default();
        if *count < quota(kind) {
            *count += 1;
            picked.push(i);
        }
    }
    picked
}

/// A random key that changes with the day: sorting by it draws items at
/// random, each in proportion to its weight.
fn weighted_key(today: i64, id: &str, weight: f64) -> f64 {
    let hash = blake3::hash(format!("{today}/{id}").as_bytes());
    let bits = u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"));
    let unit = (bits >> 11) as f64 / (1u64 << 53) as f64;
    unit.max(f64::MIN_POSITIVE).powf(1.0 / weight.max(0.01))
}

/// The middle of some positions: the median latitude and longitude.
fn middle(positions: &[(f64, f64)]) -> (f64, f64) {
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    (
        median(positions.iter().map(|p| p.0).collect()),
        median(positions.iter().map(|p| p.1).collect()),
    )
}

fn date_of(day: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date") + chrono::Duration::days(day)
}

fn days_from_date(date: NaiveDate) -> i64 {
    (date - NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date")).num_days()
}

fn days_of(year: i32, month: u32, day: u32) -> i64 {
    NaiveDate::from_ymd_opt(year, month, day).map_or(0, days_from_date)
}

fn year_of(day: i64) -> i32 {
    date_of(day).year()
}

/// (year, season): 0 winter (December counts with the next January), 1
/// spring, 2 summer, 3 autumn.
fn season_of(day: i64) -> (i32, u32) {
    let date = date_of(day);
    match date.month() {
        12 => (date.year() + 1, 0),
        1 | 2 => (date.year(), 0),
        3..=5 => (date.year(), 1),
        6..=8 => (date.year(), 2),
        _ => (date.year(), 3),
    }
}

fn season_days(year: i32, season: u32) -> (i64, i64) {
    match season {
        0 => (days_of(year - 1, 12, 1), days_of(year, 3, 1) - 1),
        1 => (days_of(year, 3, 1), days_of(year, 6, 1) - 1),
        2 => (days_of(year, 6, 1), days_of(year, 9, 1) - 1),
        _ => (days_of(year, 9, 1), days_of(year, 12, 1) - 1),
    }
}

fn looks_like_screenshot(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    ["screenshot", "screen shot", "capture d", "bildschirmfoto"]
        .iter()
        .any(|word| lower.contains(word))
}

/// Month names, which Apple's export puts in folder names.
const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "janvier",
    "février",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
    "decembre",
    "januar",
    "februar",
    "märz",
    "juni",
    "juli",
    "oktober",
    "dezember",
];

/// Words that say nothing about an event: cameras, file kinds, months.
const PLAIN_WORDS: &[&str] = &[
    "photo",
    "photos",
    "foto",
    "fotos",
    "picture",
    "pictures",
    "pics",
    "pic",
    "image",
    "images",
    "img",
    "dcim",
    "gopro",
    "iphone",
    "ipad",
    "android",
    "camera",
    "appareil",
    "jpg",
    "jpeg",
    "raw",
    "heic",
    "export",
    "exports",
    "exported",
    "original",
    "originals",
    "edited",
    "edit",
    "retouche",
    "retouches",
    "select",
    "selection",
    "sélection",
    "best",
    "misc",
    "divers",
    "autres",
    "other",
    "others",
    "video",
    "videos",
    "vidéo",
    "vidéos",
    "film",
    "films",
    "movie",
    "movies",
    "scan",
    "scans",
    "new",
    "nouveau",
    "dossier",
    "folder",
    "untitled",
    "sans",
    "titre",
    "webcam",
    "apple",
    "canon",
    "nikon",
    "sony",
    "panasonic",
    "olympus",
    "fuji",
    "fujifilm",
    "samsung",
    "huawei",
    "xiaomi",
    "lumix",
    "test",
    "tests",
    "import",
    "imports",
    "upload",
    "uploads",
    "desktop",
    "bureau",
    "downloads",
    "téléchargements",
    "screenshots",
    "wallpapers",
    "backup",
    "sauvegarde",
    "phone",
    "téléphone",
    "mobile",
    "whatsapp",
    "telegram",
    "messenger",
    "snapseed",
    "lightroom",
    "my",
    "mes",
    "the",
    "les",
    "and",
    "et",
    "und",
];

/// The event a folder is part of: its shallowest folder whose name says
/// something once dates and camera words are taken out. "2013/2013-08
/// Sardaigne/GoPro" → ("2013/2013-08 Sardaigne", "Sardaigne").
fn event_of(folder: &str) -> Option<(String, String)> {
    let mut path = String::new();
    for part in folder.split('/') {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        if let Some(title) = event_title(part) {
            return Some((path, title));
        }
    }
    None
}

fn event_title(name: &str) -> Option<String> {
    // Dates before the name ("2013-08 ", "20091225_", "15.02.03_").
    let name = name.trim_start_matches(|c: char| !c.is_alphabetic());
    let mut words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '_')
        .filter(|word| !word.is_empty())
        .collect();
    // Dates after it ("Ardeche 2003", "Le Ban-Saint-Martin, July 19, 2015"),
    // but not numbers inside ("Anniversaire Théo 6 ans").
    while let Some(last) = words.last() {
        let plain = last
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        let no_letters = !plain.chars().any(char::is_alphabetic);
        if no_letters || (words.len() > 1 && MONTHS.contains(&plain.as_str())) {
            words.pop();
        } else {
            break;
        }
    }
    let says_something = words.iter().any(|word| {
        let plain: String = word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        // Camera folders such as "100APPLE" or "DSC_0001".
        let numbered = plain.chars().filter(char::is_ascii_digit).count() * 2 >= plain.len();
        plain.chars().filter(|c| c.is_alphabetic()).count() >= 2
            && !numbered
            && !PLAIN_WORDS.contains(&plain.as_str())
            && !MONTHS.contains(&plain.as_str())
            && !plain.starts_with("dsc")
    });
    if !says_something {
        return None;
    }
    let title = words.join(" ");
    let title = title
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_owned();
    let mut chars = title.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_come_from_folder_names() {
        let event = |folder: &str| event_of(folder);
        assert_eq!(
            event("2013/2013-08 Sardaigne/GoPro/photos"),
            Some(("2013/2013-08 Sardaigne".into(), "Sardaigne".into()))
        );
        assert_eq!(
            event("2007/Mariage Cyril_angelique 2007_2/Photos Mariage"),
            Some((
                "2007/Mariage Cyril_angelique 2007_2".into(),
                "Mariage Cyril angelique".into()
            ))
        );
        assert_eq!(
            event("2009/20091225_Noël").map(|e| e.1),
            Some("Noël".into())
        );
        assert_eq!(
            event("2003/25.01.03paris").map(|e| e.1),
            Some("Paris".into())
        );
        assert_eq!(
            event("Le Ban-Saint-Martin, July 19, 2015").map(|e| e.1),
            Some("Le Ban-Saint-Martin".into())
        );
        assert_eq!(
            event("2021/Anniversaire Théo 6 ans").map(|e| e.1),
            Some("Anniversaire Théo 6 ans".into())
        );
        assert_eq!(event("2011/2011-10"), None);
        assert_eq!(event("2009/200911 - Novembre"), None);
        assert_eq!(event("DCIM/100APPLE"), None);
        assert_eq!(event(""), None);
        assert_eq!(
            event("nouvel an 2003-2004/Nouvel an 03-04").map(|e| e.1),
            Some("Nouvel an".into())
        );
    }

    #[test]
    fn seasons_and_dates() {
        let day = days_of(2023, 12, 15);
        assert_eq!(season_of(day), (2024, 0));
        assert_eq!(
            season_days(2024, 0),
            (days_of(2023, 12, 1), days_of(2024, 2, 29))
        );
        assert_eq!(year_of(days_of(1999, 12, 31)), 1999);
        assert_eq!(date_of(0), NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
    }

    #[test]
    fn weighted_keys_change_with_the_day() {
        let a = weighted_key(100, "trip-1", 1.0);
        assert_eq!(a, weighted_key(100, "trip-1", 1.0));
        assert_ne!(a, weighted_key(101, "trip-1", 1.0));
        assert!((0.0..=1.0).contains(&a));
    }

    /// Prints the collections of a real library: set PHOTOTEK_COLLECTIONS to
    /// the folder that holds its index.db and faces.db.
    #[test]
    fn collections_of_a_real_library() {
        let Ok(dir) = std::env::var("PHOTOTEK_COLLECTIONS") else {
            return;
        };
        let conn = db::open(&std::path::Path::new(&dir).join("index.db")).unwrap();
        let today = days_from_date(chrono::Local::now().date_naive());
        let started = std::time::Instant::now();
        let found = collections(&conn, today).unwrap();
        println!("computed in {:?}; home {:?}", started.elapsed(), found.home);
        println!("{} trips:", found.trips.len());
        for trip in &found.trips {
            println!(
                "  {} → {}  {:>5}  {}",
                date_of(trip.from_day),
                date_of(trip.to_day),
                trip.count,
                trip.title
            );
        }
        println!("{} memories, today:", found.memories.len());
        for &i in &found.today {
            let memory = &found.memories[i];
            println!(
                "  {:?} {:?} {:?} {} → {} ({} of {}) years {:?}",
                memory.kind,
                memory.title,
                memory.people,
                date_of(memory.from_day),
                date_of(memory.to_day),
                memory.photos.len(),
                memory.count,
                memory.years
            );
        }
        let mut kinds: HashMap<String, usize> = HashMap::new();
        for memory in &found.memories {
            *kinds.entry(format!("{:?}", memory.kind)).or_default() += 1;
        }
        println!("by kind {kinds:?}");
        for memory in found
            .memories
            .iter()
            .filter(|m| m.kind == MemoryKind::Event)
            .take(60)
        {
            println!(
                "  event {:?} {} ({})",
                memory.title,
                date_of(memory.from_day),
                memory.count
            );
        }
        println!(
            "featured {}, recent {:?}",
            found.featured.len(),
            found
                .recent_days
                .iter()
                .map(|d| (date_of(d.day), d.count))
                .collect::<Vec<_>>()
        );
        println!(
            "media {:?} fav {} hidden {} dup {}",
            found.media, found.favorites, found.hidden, found.duplicates
        );
        if let Ok(out) = std::env::var("PHOTOTEK_COLLECTIONS_JSON") {
            let json = serde_json::json!({
                "collections": found,
                "people": crate::people::people(&conn).unwrap(),
                "albums": db::albums(&conn).unwrap(),
            });
            std::fs::write(out, json.to_string()).unwrap();
        }
    }
}
