//! Similar shots: bursts and near-identical photos taken moments apart.
//!
//! Each photo gets a small visual fingerprint (a "difference hash" of its
//! cached grid thumbnail: 64 bits, one per pair of neighbouring pixels in a
//! 9 x 8 grey version, set when the left one is brighter). Two photos taken
//! at most [`WINDOW_MS`] apart whose hashes differ in at most
//! [`MAX_DISTANCE`] bits are similar, and similar photos form groups.
//!
//! The app never removes anything here: the user keeps the best shot of a
//! group and hides the others, which is shared and can be undone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use image::imageops::{self, FilterType};
use image::RgbImage;
use rayon::prelude::*;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;

use crate::indexer::{Phase, Progress};
use crate::thumbs::{ThumbCache, ThumbSize};

/// Photos further apart than this are never grouped.
pub const WINDOW_MS: i64 = 60 * 1000;
/// How many of the 64 bits two similar photos may differ in.
pub const MAX_DISTANCE: u32 = 8;

const WRITE_BATCH: usize = 500;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// The difference hash of a picture.
pub fn dhash(picture: &RgbImage) -> u64 {
    let grey = imageops::grayscale(picture);
    let small = imageops::resize(&grey, 9, 8, FilterType::Triangle);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if small.get_pixel(x, y).0[0] > small.get_pixel(x + 1, y).0[0] {
                hash |= 1;
            }
        }
    }
    hash
}

/// Hashes the photos that have a thumbnail but no hash yet. Returns how many
/// were hashed.
pub fn hash_pending(
    conn: &mut Connection,
    cache: &ThumbCache,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> rusqlite::Result<usize> {
    let pending: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT fingerprint FROM photos
             WHERE kind = 0 AND thumb_state = 1 AND part = 0
               AND fingerprint NOT IN (SELECT fingerprint FROM visual)",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let total = pending.len() as u64;
    if total == 0 {
        return Ok(0);
    }
    let report = |done| {
        on_progress(Progress {
            phase: Phase::Comparing,
            discovered: 0,
            total,
            done,
            known: 0,
        })
    };
    report(0);

    let mut done = 0u64;
    let mut last_progress = Instant::now();
    std::thread::scope(|scope| -> rusqlite::Result<()> {
        let (sender, receiver) = mpsc::sync_channel::<(&String, Option<u64>)>(WRITE_BATCH);
        let pending = &pending;
        scope.spawn(move || {
            pending
                .par_iter()
                .for_each_with(sender, |sender, fingerprint| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let hash = image::open(cache.path(ThumbSize::Grid, fingerprint))
                        .ok()
                        .map(|picture| dhash(&picture.into_rgb8()));
                    let _ = sender.send((fingerprint, hash));
                });
        });

        // Written in short transactions, never while waiting.
        let mut batch: Vec<(&String, u64)> = Vec::with_capacity(WRITE_BATCH);
        let mut write = |batch: &mut Vec<(&String, u64)>| -> rusqlite::Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            {
                let mut insert = tx
                    .prepare("INSERT OR REPLACE INTO visual (fingerprint, hash) VALUES (?1, ?2)")?;
                for (fingerprint, hash) in batch.drain(..) {
                    // Stored as the same 64 bits, signed.
                    insert.execute(params![fingerprint, hash as i64])?;
                }
            }
            tx.commit()
        };
        loop {
            match receiver.recv_timeout(PROGRESS_INTERVAL) {
                Ok((fingerprint, hash)) => {
                    // A thumbnail that can't be read is tried again next time.
                    if let Some(hash) = hash {
                        batch.push((fingerprint, hash));
                    }
                    done += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if batch.len() >= WRITE_BATCH {
                write(&mut batch)?;
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                report(done);
            }
        }
        write(&mut batch)
    })?;
    report(total);
    Ok(done as usize)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub fingerprint: String,
    pub rel_path: String,
    pub sort_date: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub favorite: bool,
    pub rating: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// In the order they were taken.
    pub shots: Vec<Shot>,
    /// The shot the app suggests keeping.
    pub keep: usize,
}

/// The groups of similar shots among the photos that are not hidden,
/// newest first.
pub fn groups(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut stmt = conn.prepare(
        "SELECT p.fingerprint, p.rel_path, p.sort_date, p.width, p.height, v.hash,
                p.fingerprint IN (SELECT key FROM org_state WHERE kind = 'fav' AND value = '1'),
                (SELECT value FROM org_state WHERE kind = 'rating' AND key = p.fingerprint)
         FROM photos p JOIN visual v ON v.fingerprint = p.fingerprint
         WHERE p.kind = 0 AND p.thumb_state != 2 AND p.part = 0
           AND p.fingerprint NOT IN (SELECT key FROM org_state WHERE kind = 'hide' AND value = '1')
         ORDER BY p.sort_date, p.rel_path",
    )?;
    let mut shots: Vec<(Shot, u64)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let fingerprint: String = row.get(0)?;
        // Exact copies are the Duplicates' business: one of each here.
        if !seen.insert(fingerprint.clone()) {
            continue;
        }
        shots.push((
            Shot {
                fingerprint,
                rel_path: row.get(1)?,
                sort_date: row.get(2)?,
                width: row.get(3)?,
                height: row.get(4)?,
                favorite: row.get(6)?,
                rating: row
                    .get::<_, Option<String>>(7)?
                    .and_then(|stars| stars.parse().ok())
                    .unwrap_or(0),
            },
            row.get::<_, i64>(5)? as u64,
        ));
    }
    Ok(group_shots(shots))
}

/// Groups shots sorted by date: each is compared with those taken up to
/// [`WINDOW_MS`] after it.
fn group_shots(shots: Vec<(Shot, u64)>) -> Vec<Group> {
    group_with(shots, WINDOW_MS, MAX_DISTANCE)
}

/// The file name without folder and extension, in lowercase: a JPEG and the
/// RAW file of the same shot share it, even in different folders.
fn shot_name(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    stem.to_lowercase()
}

fn group_with(shots: Vec<(Shot, u64)>, window_ms: i64, max_distance: u32) -> Vec<Group> {
    let names: Vec<String> = shots
        .iter()
        .map(|(shot, _)| shot_name(&shot.rel_path))
        .collect();
    let mut parent: Vec<usize> = (0..shots.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..shots.len() {
        for j in i + 1..shots.len() {
            if shots[j].0.sort_date - shots[i].0.sort_date > window_ms {
                break;
            }
            // The same shot in two formats (JPEG and RAW) is not a choice
            // between shots.
            if names[i] == names[j] {
                continue;
            }
            if (shots[i].1 ^ shots[j].1).count_ones() <= max_distance {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                if a != b {
                    parent[b] = a;
                }
            }
        }
    }
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..shots.len() {
        let r = root(&mut parent, i);
        members.entry(r).or_default().push(i);
    }
    let mut groups: Vec<Group> = members
        .into_values()
        .filter(|indexes| indexes.len() > 1)
        .map(|indexes| {
            let shots: Vec<Shot> = indexes.into_iter().map(|i| shots[i].0.clone()).collect();
            let keep = suggested(&shots);
            Group { shots, keep }
        })
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.shots[0].sort_date));
    groups
}

/// The favorite, else the best rated, else the largest, else the first.
fn suggested(shots: &[Shot]) -> usize {
    shots
        .iter()
        .enumerate()
        .max_by_key(|(index, shot)| {
            let pixels = u64::from(shot.width.unwrap_or(0)) * u64::from(shot.height.unwrap_or(0));
            (
                shot.favorite,
                shot.rating,
                pixels,
                std::cmp::Reverse(*index),
            )
        })
        .map_or(0, |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn gradient(shift: u32, flip: bool) -> RgbImage {
        RgbImage::from_fn(64, 48, |x, y| {
            let v = ((x * 4 + y + shift) % 256) as u8;
            let v = if flip { 255 - v } else { v };
            Rgb([v, v, v])
        })
    }

    #[test]
    fn similar_pictures_have_close_hashes() {
        let a = dhash(&gradient(0, false));
        let b = dhash(&gradient(2, false));
        let c = dhash(&gradient(0, true));
        assert!((a ^ b).count_ones() <= MAX_DISTANCE);
        assert!((a ^ c).count_ones() > MAX_DISTANCE);
    }

    fn shot(name: &str, sort_date: i64, favorite: bool, rating: u8) -> Shot {
        Shot {
            fingerprint: name.repeat(64),
            rel_path: format!("{name}.jpg"),
            sort_date,
            width: Some(4000),
            height: Some(3000),
            favorite,
            rating,
        }
    }

    #[test]
    fn groups_close_shots_taken_moments_apart() {
        let alike = 0b1010_1010u64;
        let groups = group_shots(vec![
            (shot("a", 0, false, 0), alike),
            (shot("b", 2_000, false, 3), alike ^ 0b11),
            (shot("c", 4_000, false, 0), alike ^ 0b111),
            // Alike, but taken an hour later: another moment.
            (shot("d", 3_600_000, false, 0), alike),
            // Taken moments later, but another picture.
            (shot("e", 5_000, false, 0), !alike),
        ]);
        assert_eq!(groups.len(), 1);
        let names: Vec<&str> = groups[0]
            .shots
            .iter()
            .map(|s| s.rel_path.as_str())
            .collect();
        assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg"]);
        // The best rated one is suggested.
        assert_eq!(groups[0].keep, 1);

        // A JPEG and its RAW file are one shot, not two to choose from.
        let mut raw = shot("f", 0, false, 0);
        raw.rel_path = "cr2/A.CR2".into();
        let mut jpeg = shot("g", 0, false, 0);
        jpeg.rel_path = "a.jpg".into();
        assert!(group_shots(vec![(jpeg, alike), (raw, alike)]).is_empty());
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;

    /// How many groups each setting makes on a copy of a real library's
    /// index (`PHOTOTEK_BENCH_DIR` holds `index.db`; `PHOTOTEK_APP_DATA` is
    /// the app's data folder with the thumbnails). Prints numbers only.
    #[test]
    #[ignore]
    fn thresholds_on_a_real_library() {
        let index = std::env::var("PHOTOTEK_BENCH_DIR").unwrap();
        let data = std::path::PathBuf::from(std::env::var("PHOTOTEK_APP_DATA").unwrap());
        let conn = Connection::open(format!("{index}/index.db")).unwrap();
        let cache = ThumbCache::new(&data);
        let rows: Vec<(String, String, i64)> = conn
            .prepare(
                "SELECT fingerprint, MIN(rel_path), sort_date FROM photos
                 WHERE kind = 0 AND part = 0 AND thumb_state != 2
                 GROUP BY fingerprint ORDER BY sort_date DESC LIMIT 15000",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut hashed: Vec<(Shot, u64)> = rows
            .par_iter()
            .filter_map(|(fp, path, date)| {
                let picture = image::open(cache.path(ThumbSize::Grid, fp)).ok()?;
                Some((
                    Shot {
                        fingerprint: fp.clone(),
                        rel_path: path.clone(),
                        sort_date: *date,
                        width: None,
                        height: None,
                        favorite: false,
                        rating: 0,
                    },
                    dhash(&picture.into_rgb8()),
                ))
            })
            .collect();
        hashed.sort_by_key(|(s, _)| s.sort_date);
        println!("hashed {}", hashed.len());
        for (window_s, distance) in [
            (60, 6),
            (60, 10),
            (180, 6),
            (180, 8),
            (180, 10),
            (180, 12),
            (600, 10),
        ] {
            let groups = group_with(hashed.clone(), window_s * 1000, distance);
            let photos: usize = groups.iter().map(|g| g.shots.len()).sum();
            let largest = groups.iter().map(|g| g.shots.len()).max().unwrap_or(0);
            println!(
                "window {window_s:>3}s distance {distance:>2}: {} groups, {} photos, {} would be hidden, largest {}",
                groups.len(),
                photos,
                photos - groups.len(),
                largest
            );
        }
    }
}
