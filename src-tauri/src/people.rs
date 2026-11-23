//! Groups the photos by who is in them.
//!
//! 1. **Finding faces** ([`find_faces`]): every photo's cached preview is
//!    searched for faces (see faces.rs). This reads the local thumbnail
//!    cache only, so it also works while the library is offline.
//! 2. **Grouping** ([`group_faces`]): each new face joins the existing group
//!    it resembles most, or starts a new group.
//!    Then groups that are clearly the same person are merged
//!    ([`merge_groups`]): the same person at different ages or angles often
//!    starts several groups.
//! 3. **People** ([`people`]): a group is a person. Groups are local to this
//!    computer, but names are shared: a name is attached to one face of a
//!    photo (`face_name` in the library's log), every computer gives it to
//!    the group that face is in, and groups with the same name are shown as
//!    one person. That is also how two groups of the same person are merged:
//!    give them the same name.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use crate::db::{self, NewFace};
use crate::faces::{similarity, Face, FaceFinder};
use crate::indexer::{Phase, Progress};
use crate::org::Op;
use crate::thumbs::{ThumbCache, ThumbSize};

/// Faces at least this alike are taken for the same person. People who are
/// related score up to about 0.4 with this model; the same person in
/// different photos scores 0.6 and more.
const SAME_PERSON: f32 = 0.45;
/// A group nobody named is shown once it appears in this many photos;
/// smaller ones are mostly strangers in the background.
const MIN_PHOTOS_UNNAMED: u32 = 3;
/// Two groups whose average faces are at least this alike are merged. The
/// average of many faces is steadier than one face, so this is much higher
/// than `SAME_PERSON`; it stays clear of brothers, sisters and parents,
/// whose groups score up to about 0.7. The rest is left to suggestions.
const MERGE_GROUPS: f32 = 0.78;
/// Groups at least this alike to a named person are suggested as maybe them.
const SUGGEST: f32 = 0.5;
const MAX_SUGGESTIONS: usize = 12;
const MAX_PEOPLE: usize = 500;
const WRITE_BATCH: usize = 50;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
/// Found faces are written at least this often.
const WRITE_INTERVAL: Duration = Duration::from_secs(1);

/// Searches the photos that were not searched yet. Returns how many were.
pub fn find_faces(
    conn: &mut Connection,
    cache: &ThumbCache,
    finder: &FaceFinder,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(Progress),
) -> rusqlite::Result<u64> {
    let pending = db::pending_faces(conn)?;
    let total = pending.len() as u64;
    if total == 0 {
        return Ok(0);
    }
    let report = |done| {
        on_progress(Progress {
            phase: Phase::Faces,
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
        // `None` for a photo whose preview could not be read this time: it
        // is tried again on the next pass.
        let (sender, receiver) = mpsc::sync_channel::<(&String, Option<Vec<Face>>)>(WRITE_BATCH);
        let pending = &pending;
        scope.spawn(move || {
            pending
                .par_iter()
                .for_each_with(sender, |sender, fingerprint| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let faces = image::open(cache.path(ThumbSize::Preview, fingerprint))
                        .ok()
                        .and_then(|picture| finder.find(&picture.into_rgb8()).ok());
                    let _ = sender.send((fingerprint, faces));
                });
        });

        // Written in short transactions, never while waiting for the next
        // photo: a change the user makes meanwhile must not wait for it.
        let mut batch: Vec<(&String, Vec<Face>)> = Vec::with_capacity(WRITE_BATCH);
        let mut last_write = Instant::now();
        let mut write = |batch: &mut Vec<(&String, Vec<Face>)>| -> rusqlite::Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for (fingerprint, faces) in batch.drain(..) {
                let rows: Vec<NewFace<'_>> = faces
                    .iter()
                    .map(|face| NewFace {
                        bounds: face.bounds,
                        score: face.score,
                        embedding: &face.embedding,
                    })
                    .collect();
                db::save_faces(&tx, fingerprint, &rows)?;
            }
            tx.commit()
        };
        loop {
            match receiver.recv_timeout(PROGRESS_INTERVAL) {
                Ok((fingerprint, faces)) => {
                    if let Some(faces) = faces {
                        batch.push((fingerprint, faces));
                    }
                    done += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if batch.len() >= WRITE_BATCH || last_write.elapsed() >= WRITE_INTERVAL {
                write(&mut batch)?;
                last_write = Instant::now();
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                report(done);
            }
        }
        write(&mut batch)
    })?;
    Ok(done)
}

/// Puts every face that has no group yet into one, then merges the groups
/// that are clearly the same person. Returns how many faces and groups
/// changed.
pub fn group_faces(conn: &mut Connection) -> rusqlite::Result<usize> {
    let faces = db::face_embeddings(conn)?;
    let assigned = assign_groups(&faces);
    db::set_face_clusters(conn, &assigned)?;
    let merged = if assigned.is_empty() {
        0
    } else {
        merge_groups(conn)?
    };
    Ok(assigned.len() + merged)
}

/// The sum of the embeddings of each group's faces (pointing to the group's
/// average face) and its number of faces.
fn group_sums(conn: &Connection) -> rusqlite::Result<HashMap<i64, (Vec<f32>, usize)>> {
    let mut sums: HashMap<i64, (Vec<f32>, usize)> = HashMap::new();
    for (_, cluster, embedding) in db::face_embeddings(conn)? {
        let Some(cluster) = cluster.filter(|c| *c > 0) else {
            continue;
        };
        let entry = sums
            .entry(cluster)
            .or_insert_with(|| (vec![0.0; embedding.len()], 0));
        for (total, value) in entry.0.iter_mut().zip(&embedding) {
            *total += value;
        }
        entry.1 += 1;
    }
    Ok(sums)
}

fn unit(vector: &[f32]) -> Vec<f32> {
    let length = vector
        .iter()
        .map(|v| v * v)
        .sum::<f32>()
        .sqrt()
        .max(f32::EPSILON);
    vector.iter().map(|v| v / length).collect()
}

/// The (lowercased) name of each named group.
fn group_names(conn: &Connection) -> rusqlite::Result<HashMap<i64, String>> {
    let mut name_of: HashMap<i64, String> = HashMap::new();
    for (fingerprint, x, y, name) in db::face_names(conn)? {
        if let Some(cluster) = db::cluster_at(conn, &fingerprint, x, y)? {
            name_of.entry(cluster).or_insert(name);
        }
    }
    Ok(name_of)
}

/// Merges the groups whose average faces are at least `MERGE_GROUPS` alike,
/// most alike first, never two groups with different names nor a group the
/// user said is not that person. Returns how many groups were merged away.
pub fn merge_groups(conn: &mut Connection) -> rusqlite::Result<usize> {
    let sums = group_sums(conn)?;
    let names: HashMap<i64, String> = group_names(conn)?
        .into_iter()
        .map(|(cluster, name)| (cluster, name.to_lowercase()))
        .collect();
    let refused = db::not_person(conn)?;
    let apart = db::kept_apart(conn)?;
    let merges = plan_merges(&sums, &names, &refused, &apart);
    if merges.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (from, into) in &merges {
        db::merge_face_group(&tx, *from, *into)?;
    }
    tx.commit()?;
    Ok(merges.len())
}

/// Which groups go into which: `(from, into)`, in the order to apply them.
fn plan_merges(
    sums: &HashMap<i64, (Vec<f32>, usize)>,
    names: &HashMap<i64, String>,
    refused: &HashSet<(i64, String)>,
    apart: &HashSet<(i64, i64)>,
) -> Vec<(i64, i64)> {
    // A group of one face is no steadier than a face, and it was already
    // compared with every group when it was made: only larger ones merge.
    let mut ids: Vec<i64> = sums
        .iter()
        .filter(|(_, (_, count))| *count >= 2)
        .map(|(id, _)| *id)
        .collect();
    ids.sort_unstable();
    let units: Vec<Vec<f32>> = ids.iter().map(|id| unit(&sums[id].0)).collect();
    let mut pairs: Vec<(f32, usize, usize)> = (0..ids.len())
        .into_par_iter()
        .flat_map_iter(|a| {
            let units = &units;
            (a + 1..units.len()).filter_map(move |b| {
                let alike = similarity(&units[a], &units[b]);
                (alike >= MERGE_GROUPS).then_some((alike, a, b))
            })
        })
        .collect();
    pairs.sort_by(|x, y| y.0.total_cmp(&x.0));

    // Union-find over the groups; a merged group keeps the id of the larger.
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    let mut sum: Vec<Vec<f32>> = ids.iter().map(|id| sums[id].0.clone()).collect();
    let mut count: Vec<usize> = ids.iter().map(|id| sums[id].1).collect();
    let mut name: Vec<Option<String>> = ids.iter().map(|id| names.get(id).cloned()).collect();
    let mut refusals: Vec<HashSet<String>> = ids
        .iter()
        .map(|id| {
            refused
                .iter()
                .filter(|(cluster, _)| cluster == id)
                .map(|(_, name)| name.clone())
                .collect()
        })
        .collect();
    // The groups each merged group holds, and those it must stay apart from.
    let mut members: Vec<HashSet<i64>> = ids.iter().map(|id| HashSet::from([*id])).collect();
    let mut avoid: Vec<HashSet<i64>> = ids
        .iter()
        .map(|id| {
            apart
                .iter()
                .filter_map(|&(a, b)| (a == *id).then_some(b).or((b == *id).then_some(a)))
                .collect()
        })
        .collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut merges = Vec::new();
    for (_, a, b) in pairs {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        if ra == rb {
            continue;
        }
        // Earlier merges moved both averages: check again.
        if similarity(&unit(&sum[ra]), &unit(&sum[rb])) < MERGE_GROUPS {
            continue;
        }
        let clash = match (&name[ra], &name[rb]) {
            (Some(x), Some(y)) => x != y,
            (Some(x), None) => refusals[rb].contains(x),
            (None, Some(y)) => refusals[ra].contains(y),
            (None, None) => false,
        };
        if clash || !avoid[ra].is_disjoint(&members[rb]) || !avoid[rb].is_disjoint(&members[ra]) {
            continue;
        }
        let (into, from) = if count[ra] >= count[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        parent[from] = into;
        let moved = std::mem::take(&mut sum[from]);
        for (total, value) in sum[into].iter_mut().zip(&moved) {
            *total += value;
        }
        count[into] += count[from];
        if name[into].is_none() {
            name[into] = name[from].take();
        }
        let moved = std::mem::take(&mut refusals[from]);
        refusals[into].extend(moved);
        let moved = std::mem::take(&mut members[from]);
        members[into].extend(moved);
        let moved = std::mem::take(&mut avoid[from]);
        avoid[into].extend(moved);
        merges.push((ids[from], ids[into]));
    }
    // Applied in this order, a group merged into one that is merged later
    // follows it.
    merges
}

/// Unnamed groups that might be the person named `name`, made of
/// `clusters`: most alike first.
pub fn suggestions(
    conn: &Connection,
    clusters: &[i64],
    name: &str,
) -> rusqlite::Result<Vec<Person>> {
    let sums = group_sums(conn)?;
    let names = group_names(conn)?;
    let hidden = db::hidden_face_groups(conn)?;
    let name = name.trim().to_lowercase();
    let refused: HashSet<i64> = db::not_person(conn)?
        .into_iter()
        .filter(|(_, refused)| *refused == name)
        .map(|(cluster, _)| cluster)
        .collect();
    let person: Vec<Vec<f32>> = clusters
        .iter()
        .filter_map(|cluster| sums.get(cluster))
        .map(|(sum, _)| unit(sum))
        .collect();
    if person.is_empty() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(f32, Person)> = Vec::new();
    for group in db::face_groups(conn)? {
        let id = group.cluster;
        if clusters.contains(&id)
            || names.contains_key(&id)
            || hidden.contains(&id)
            || refused.contains(&id)
        {
            continue;
        }
        let Some((sum, _)) = sums.get(&id) else {
            continue;
        };
        let candidate = unit(sum);
        let alike = person
            .iter()
            .map(|face| similarity(face, &candidate))
            .fold(f32::MIN, f32::max);
        if alike >= SUGGEST {
            found.push((alike, person_of(&group, None)));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(found
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, p)| p)
        .collect())
}

fn person_of(group: &db::FaceGroup, name: Option<String>) -> Person {
    Person {
        name,
        clusters: vec![group.cluster],
        photos: group.photos,
        hidden: false,
        cover_crop: square_around(group.cover_bounds, group.cover_size),
        cover: group.cover.clone(),
        cover_width: group.cover_size.map(|(width, _)| width),
        cover_height: group.cover_size.map(|(_, height)| height),
    }
}

/// The grouping itself: `faces` are `(id, group, embedding)`; returns the
/// group chosen for each face that had none.
fn assign_groups(faces: &[db::StoredFace]) -> Vec<(i64, i64)> {
    /// A group is summed up by the sum of its faces' embeddings, which
    /// points to the "average face" of the group.
    struct Group {
        id: i64,
        sum: Vec<f32>,
        length: f32,
    }
    fn add(group: &mut Group, embedding: &[f32]) {
        for (total, value) in group.sum.iter_mut().zip(embedding) {
            *total += value;
        }
        group.length = group.sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut index_of: HashMap<i64, usize> = HashMap::new();
    for (_, cluster, embedding) in faces {
        // Below zero: taken out of its group by hand, it stays out.
        let Some(cluster) = cluster.filter(|c| *c > 0) else {
            continue;
        };
        let index = *index_of.entry(cluster).or_insert_with(|| {
            groups.push(Group {
                id: cluster,
                sum: vec![0.0; embedding.len()],
                length: 0.0,
            });
            groups.len() - 1
        });
        add(&mut groups[index], embedding);
    }

    let mut next_id = groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
    let mut assigned = Vec::new();
    for (face, cluster, embedding) in faces {
        if cluster.is_some() {
            continue;
        }
        let best = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| group.length > 0.0)
            .map(|(index, group)| (index, similarity(&group.sum, embedding) / group.length))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let index = match best {
            Some((index, alike)) if alike >= SAME_PERSON => index,
            _ => {
                groups.push(Group {
                    id: next_id,
                    sum: vec![0.0; embedding.len()],
                    length: 0.0,
                });
                next_id += 1;
                groups.len() - 1
            }
        };
        add(&mut groups[index], embedding);
        assigned.push((*face, groups[index].id));
    }
    assigned
}

/// A person as the People screen shows them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    /// `None` until someone names them.
    pub name: Option<String>,
    /// The face groups that make up this person on this computer.
    pub clusters: Vec<i64>,
    /// Photos they appear in. For a person made of several groups this can
    /// count a photo twice; it is only used for ordering and as a hint.
    pub photos: u32,
    /// Hidden from the People screen (on this computer).
    pub hidden: bool,
    /// The photo to show them with, and the square part of it around their
    /// face: left, top, width, height in ten-thousandths of the photo.
    pub cover: String,
    pub cover_crop: [u16; 4],
    pub cover_width: Option<u32>,
    pub cover_height: Option<u32>,
}

/// The people of a library: named ones first, then by number of photos.
pub fn people(conn: &Connection) -> rusqlite::Result<Vec<Person>> {
    let name_of = group_names(conn)?;
    let hidden = db::hidden_face_groups(conn)?;

    let mut named: HashMap<String, Person> = HashMap::new();
    let mut unnamed: Vec<Person> = Vec::new();
    let mut groups = db::face_groups(conn)?;
    // Largest first, so that a person's biggest group provides the cover.
    groups.sort_by(|a, b| b.photos.cmp(&a.photos).then(a.cluster.cmp(&b.cluster)));
    for group in groups {
        let mut person = person_of(&group, name_of.get(&group.cluster).cloned());
        person.hidden = hidden.contains(&group.cluster);
        match &person.name {
            Some(name) => match named.get_mut(&name.to_lowercase()) {
                Some(same) => {
                    same.clusters.push(group.cluster);
                    same.photos += group.photos;
                    // Hidden only if every group of theirs is.
                    same.hidden &= person.hidden;
                }
                None => {
                    named.insert(name.to_lowercase(), person);
                }
            },
            None if group.photos >= MIN_PHOTOS_UNNAMED => unnamed.push(person),
            None => {}
        }
    }
    let mut all: Vec<Person> = named.into_values().collect();
    all.sort_by(|a, b| b.photos.cmp(&a.photos).then(a.name.cmp(&b.name)));
    all.extend(unnamed);
    all.truncate(MAX_PEOPLE);
    Ok(all)
}

/// A square (in pixels) around a face box, somewhat larger than the box, as
/// ten-thousandths of the photo. `size` is the photo's size when known.
fn square_around(bounds: [f64; 4], size: Option<(u32, u32)>) -> [u16; 4] {
    let (photo_width, photo_height) =
        size.map_or((3.0, 2.0), |(w, h)| (f64::from(w), f64::from(h)));
    let [left, top, width, height] = bounds;
    let center_x = (left + width / 2.0) * photo_width;
    let center_y = (top + height / 2.0) * photo_height;
    // Room for hair and chin, but never more than the photo has.
    let side = (1.6 * (width * photo_width).max(height * photo_height))
        .min(photo_width)
        .min(photo_height);
    let x = (center_x - side / 2.0).clamp(0.0, photo_width - side);
    let y = (center_y - side / 2.0).clamp(0.0, photo_height - side);
    let units =
        |value: f64, whole: f64| (value / whole * 10_000.0).round().clamp(0.0, 10_000.0) as u16;
    let (l, t) = (units(x, photo_width), units(y, photo_height));
    [
        l,
        t,
        units(side, photo_width).clamp(1, 10_000 - l),
        units(side, photo_height).clamp(1, 10_000 - t),
    ]
}

/// The log entries that give the person made of `clusters` a name (or, with
/// an empty name, take it away). One face per group carries the name; faces
/// in these groups that carried another name are renamed too.
pub fn naming_ops(conn: &Connection, clusters: &[i64], name: &str) -> rusqlite::Result<Vec<Op>> {
    let unit = |fraction: f64| (fraction * 10_000.0).round().clamp(0.0, 10_000.0) as u16;
    let mut ops = Vec::new();
    // Faces that already carry a name in these groups.
    for (fingerprint, x, y, _) in db::face_names(conn)? {
        if db::cluster_at(conn, &fingerprint, x, y)?.is_some_and(|c| clusters.contains(&c)) {
            ops.push(Op::FaceName {
                photo: fingerprint,
                x: unit(x),
                y: unit(y),
                name: name.to_owned(),
            });
        }
    }
    // And the cover face of every group, so each group has one.
    for group in db::face_groups(conn)? {
        if !clusters.contains(&group.cluster) {
            continue;
        }
        let [left, top, width, height] = group.cover_bounds;
        let op = Op::FaceName {
            photo: group.cover,
            x: unit(left + width / 2.0),
            y: unit(top + height / 2.0),
            name: name.to_owned(),
        };
        if !ops.contains(&op) {
            ops.push(op);
        }
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measures face search and grouping on a copy of a real library:
    /// `PHOTOTEK_BENCH_DIR` holds `index.db` and `faces.db`, and
    /// `PHOTOTEK_APP_DATA` is the app's data folder (models and thumbnails).
    /// `PHOTOTEK_BENCH_PHOTOS` limits how many photos are searched.
    /// Prints numbers only, never names or file paths.
    #[test]
    #[ignore]
    fn benchmark_real_library() {
        let (Ok(dir), Ok(data)) = (
            std::env::var("PHOTOTEK_BENCH_DIR"),
            std::env::var("PHOTOTEK_APP_DATA"),
        ) else {
            return;
        };
        let limit: u64 = std::env::var("PHOTOTEK_BENCH_PHOTOS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(1000);
        let data = std::path::Path::new(&data);
        let mut conn = db::open(&std::path::Path::new(&dir).join("index.db")).unwrap();
        let cache = ThumbCache::new(data);
        let finder = FaceFinder::load(data).unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let searched = find_faces(&mut conn, &cache, &finder, &cancel, &|progress| {
            if progress.done >= limit {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        let search_ms = started.elapsed().as_millis();
        let faces: i64 = conn
            .query_row("SELECT count(*) FROM faces.face", [], |row| row.get(0))
            .unwrap();
        let scanned: i64 = conn
            .query_row("SELECT count(*) FROM faces.scanned", [], |row| row.get(0))
            .unwrap();
        println!(
            "searched {searched} photos in {search_ms} ms ({:.1} ms/photo); {scanned} scanned in all, {faces} faces",
            search_ms as f64 / searched.max(1) as f64
        );

        let started = Instant::now();
        let grouped = group_faces(&mut conn).unwrap();
        println!(
            "grouped {grouped} faces in {} ms",
            started.elapsed().as_millis()
        );
        let started = Instant::now();
        let merged = merge_groups(&mut conn).unwrap();
        println!(
            "merged {merged} groups in {} ms",
            started.elapsed().as_millis()
        );
        let sizes: Vec<(i64, i64)> = conn
            .prepare(
                "SELECT n, count(*) FROM (SELECT cluster, count(DISTINCT fingerprint) AS n
                   FROM faces.face GROUP BY cluster) GROUP BY n ORDER BY n",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let groups: i64 = sizes.iter().map(|(_, count)| count).sum();
        let shown: i64 = sizes.iter().filter(|(n, _)| *n >= 3).map(|(_, c)| c).sum();
        println!("{groups} groups, {shown} with 3+ photos");
        let mut largest: Vec<(i64, i64)> = sizes.clone();
        largest.reverse();
        println!(
            "photos per group (largest first): {:?}",
            &largest[..largest.len().min(25)]
        );
        println!("people listed: {}", people(&conn).unwrap().len());
        for person in people(&conn).unwrap().iter().take(8) {
            let found = suggestions(&conn, &person.clusters, "someone").unwrap();
            let sizes: Vec<u32> = found.iter().map(|p| p.photos).collect();
            println!(
                "person with {} photos: suggestions {sizes:?}",
                person.photos
            );
        }
    }

    use crate::db::PhotoRow;
    use crate::org::Event;

    /// An embedding pointing mostly along one axis: faces built with the
    /// same `who` are alike, others are not.
    fn face_of(who: usize, variation: f32) -> Vec<f32> {
        let mut embedding = [0.0f32; 8];
        embedding[who] = 1.0;
        embedding[(who + 1) % 8] = variation;
        let length = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        embedding.iter().map(|v| v / length).collect()
    }

    #[test]
    fn grouping_joins_look_alikes_and_keeps_existing_groups() {
        let faces = vec![
            (1, Some(7), face_of(0, 0.0)),
            (2, None, face_of(0, 0.3)),
            (3, None, face_of(1, 0.0)),
            (4, None, face_of(1, 0.2)),
            (5, None, face_of(0, -0.2)),
            (6, None, face_of(5, 0.0)),
        ];
        let assigned = assign_groups(&faces);
        // Faces 2 and 5 join the existing group 7; 3 and 4 form a new one.
        assert_eq!(assigned, [(2, 7), (3, 8), (4, 8), (5, 7), (6, 9)]);
        assert!(assign_groups(&[]).is_empty());
    }

    #[test]
    fn face_squares_stay_inside_the_photo() {
        // A face in the middle of a 4000x3000 photo.
        let [left, top, width, height] = square_around([0.45, 0.4, 0.1, 0.2], Some((4000, 3000)));
        // 0.2 of 3000 is 600 px; the square is 1.6 times that: 960 px.
        assert_eq!((width, height), (2400, 3200));
        assert_eq!((left, top), (3800, 3400));
        // A face in the corner: the square is pushed back inside.
        let [left, top, width, height] = square_around([0.9, 0.0, 0.1, 0.1], Some((1000, 1000)));
        assert_eq!((left, top, width, height), (8400, 0, 1600, 1600));
        // A face that fills the photo: the square is the photo's short side.
        let [_, _, width, height] = square_around([0.0, 0.0, 1.0, 1.0], Some((2000, 1000)));
        assert_eq!((width, height), (5000, 10_000));
    }

    fn photo(conn: &mut Connection, name: &str) {
        let tx = conn.transaction().unwrap();
        db::upsert(
            &tx,
            &PhotoRow {
                rel_path: format!("{name}.jpg"),
                size: 1,
                mtime_ms: 0,
                fingerprint: name.repeat(64 / name.len()),
                sort_date: 0,
                date_from_exif: false,
                orientation: None,
                width: Some(1000),
                height: Some(1000),
                camera_make: None,
                camera_model: None,
                latitude: None,
                longitude: None,
                is_video: false,
                duration_ms: None,
                live_candidate: false,
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn people_are_named_through_one_of_their_faces() {
        let mut conn = db::open_in_memory().unwrap();
        // Alice (who = 0) is in six photos, in three of them under an angle
        // that the grouping takes for someone else (who = 3); Bob (who = 1)
        // is in three; a passer-by (who = 2) in one.
        let cast: [(&str, &[usize]); 6] = [
            ("aa", &[0, 1]),
            ("bb", &[0, 1]),
            ("cc", &[0, 1]),
            ("dd", &[3, 2]),
            ("ee", &[3]),
            ("ff", &[3]),
        ];
        for (name, who) in cast {
            photo(&mut conn, name);
            let embeddings: Vec<Vec<f32>> = who.iter().map(|w| face_of(*w, 0.1)).collect();
            let faces: Vec<NewFace<'_>> = embeddings
                .iter()
                .enumerate()
                .map(|(n, embedding)| NewFace {
                    bounds: [0.1 + 0.4 * n as f32, 0.1, 0.2, 0.2],
                    score: 0.9,
                    embedding,
                })
                .collect();
            let tx = conn.transaction().unwrap();
            db::save_faces(&tx, &name.repeat(32), &faces).unwrap();
            tx.commit().unwrap();
        }
        assert!(db::pending_faces(&conn).unwrap().is_empty());
        assert_eq!(group_faces(&mut conn).unwrap(), 10);
        assert_eq!(group_faces(&mut conn).unwrap(), 0);

        // Three groups are large enough to show; nobody has a name yet.
        let before = people(&conn).unwrap();
        let sizes: Vec<(Option<&str>, u32)> = before
            .iter()
            .map(|person| (person.name.as_deref(), person.photos))
            .collect();
        assert_eq!(sizes, [(None, 3), (None, 3), (None, 3)]);

        // Name both of Alice's groups "Alice": they become one person.
        let record = |conn: &mut Connection, ops: Vec<Op>, ts: i64| {
            assert!(!ops.is_empty() && ops.iter().all(Op::is_valid), "{ops:?}");
            let tx = conn.transaction().unwrap();
            for op in ops {
                db::apply_event(&tx, "device", &Event { ts, op }).unwrap();
            }
            tx.commit().unwrap();
        };
        // The first face of a photo is at (0.2, 0.2).
        let group_in = |photo: &str| {
            let cluster = db::cluster_at(&conn, &photo.repeat(32), 0.2, 0.2).unwrap();
            vec![cluster.unwrap()]
        };
        let (alice_front, alice_side) = (group_in("aa"), group_in("dd"));
        assert_ne!(alice_front, alice_side);
        let ops = naming_ops(&conn, &alice_front, "Alice").unwrap();
        record(&mut conn, ops, 1);
        let ops = naming_ops(&conn, &alice_side, "alice").unwrap();
        record(&mut conn, ops, 2);

        let after = people(&conn).unwrap();
        assert_eq!(after[0].name.as_deref(), Some("Alice"));
        assert_eq!(after[0].clusters.len(), 2);
        assert_eq!(after[0].photos, 6);
        assert_eq!(after[1].name, None);

        // The listing of a person shows the photos of all their groups.
        let listing = db::Listing::Person {
            clusters: after[0].clusters.clone(),
        };
        assert_eq!(db::photos_page(&conn, &listing, 0, 10).unwrap().len(), 6);

        // Renaming changes every face that carried the old name.
        let ops = naming_ops(&conn, &after[0].clusters, "Alice B.").unwrap();
        record(&mut conn, ops, 3);
        let renamed = people(&conn).unwrap();
        assert_eq!(renamed[0].name.as_deref(), Some("Alice B."));
        assert_eq!(renamed[0].clusters.len(), 2);
    }

    #[test]
    fn groups_merge_only_when_clearly_the_same_person() {
        let sum = |faces: &[Vec<f32>]| -> (Vec<f32>, usize) {
            let mut total = vec![0.0; faces[0].len()];
            for face in faces {
                for (t, v) in total.iter_mut().zip(face) {
                    *t += v;
                }
            }
            (total, faces.len())
        };
        let mut sums = HashMap::new();
        // 1 and 2: the same person, 2 the larger group. 3: someone else.
        sums.insert(1, sum(&[face_of(0, 0.1), face_of(0, 0.2)]));
        sums.insert(
            2,
            sum(&[face_of(0, 0.15), face_of(0, 0.0), face_of(0, 0.1)]),
        );
        sums.insert(3, sum(&[face_of(4, 0.0), face_of(4, 0.1)]));
        // A single face just like them is not merged: it was compared already.
        sums.insert(4, sum(&[face_of(0, 0.1)]));
        // 5 and 6 look alike but carry different names; 7 looks like 8, but
        // the user said 7 is not 8's person.
        sums.insert(5, sum(&[face_of(2, 0.0), face_of(2, 0.1)]));
        sums.insert(6, sum(&[face_of(2, 0.05), face_of(2, 0.1)]));
        sums.insert(7, sum(&[face_of(6, 0.0), face_of(6, 0.1)]));
        sums.insert(8, sum(&[face_of(6, 0.05), face_of(6, 0.1)]));
        let names = HashMap::from([
            (5, "anna".to_owned()),
            (6, "eva".to_owned()),
            (8, "tom".to_owned()),
        ]);
        let refused = HashSet::from([(7, "tom".to_owned())]);
        assert_eq!(
            plan_merges(&sums, &names, &refused, &HashSet::new()),
            [(1, 2)]
        );
        // Without the names and the refusal, those merge too.
        let mut all = plan_merges(&sums, &HashMap::new(), &HashSet::new(), &HashSet::new());
        all.sort_unstable();
        assert_eq!(all, [(1, 2), (6, 5), (8, 7)]);
    }

    #[test]
    fn suggestions_corrections_and_hidden_people() {
        let mut conn = db::open_in_memory().unwrap();
        // Alice in aa, bb, cc; someone like her (maybe her, younger) in dd
        // and ee; a stranger in ff.
        let cast = [
            ("aa", face_of(0, 0.0)),
            ("bb", face_of(0, 0.1)),
            ("cc", face_of(0, 0.05)),
            ("dd", face_of(0, 1.0)),
            ("ee", face_of(0, 1.1)),
            ("ff", face_of(4, 0.0)),
        ];
        for (name, embedding) in &cast {
            photo(&mut conn, name);
            let tx = conn.transaction().unwrap();
            let face = NewFace {
                bounds: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                embedding,
            };
            db::save_faces(&tx, &name.repeat(32), &[face]).unwrap();
            tx.commit().unwrap();
        }
        // Grouped by hand, as the grouping might have: dd and ee apart.
        let ids: Vec<i64> = db::face_embeddings(&conn)
            .unwrap()
            .iter()
            .map(|(id, _, _)| *id)
            .collect();
        let groups = [1, 1, 1, 2, 2, 3];
        let assigned: Vec<(i64, i64)> = ids.iter().copied().zip(groups).collect();
        db::set_face_clusters(&mut conn, &assigned).unwrap();
        // 0.7 alike: not merged, only suggested.
        assert_eq!(merge_groups(&mut conn).unwrap(), 0);
        let ops = naming_ops(&conn, &[1], "Alice").unwrap();
        let tx = conn.transaction().unwrap();
        for op in ops {
            db::apply_event(&tx, "device", &Event { ts: 1, op }).unwrap();
        }
        tx.commit().unwrap();

        let suggested = suggestions(&conn, &[1], "Alice").unwrap();
        assert_eq!(
            suggested.iter().map(|p| p.clusters[0]).collect::<Vec<_>>(),
            [2]
        );
        // "Not Alice": no longer suggested, and never merged into her.
        db::add_not_person(&conn, 2, "alice").unwrap();
        assert!(suggestions(&conn, &[1], "Alice").unwrap().is_empty());

        // Hiding someone keeps them, marked hidden. (Groups 2 and 3 are too
        // small to be listed.)
        db::set_face_groups_hidden(&mut conn, &[1], true).unwrap();
        let listed: Vec<bool> = people(&conn).unwrap().iter().map(|p| p.hidden).collect();
        assert_eq!(listed, [true]);
        db::set_face_groups_hidden(&mut conn, &[1], false).unwrap();
        assert!(!people(&conn).unwrap()[0].hidden);

        // Taking a photo out of Alice: her listing loses it, for good.
        let removed = db::remove_faces(&mut conn, &[1], &["aa".repeat(32)]).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(group_faces(&mut conn).unwrap(), 0);
        let listing = db::Listing::Person { clusters: vec![1] };
        assert_eq!(db::photos_page(&conn, &listing, 0, 10).unwrap().len(), 2);
    }

    #[test]
    fn a_split_group_stays_apart_and_loses_the_name() {
        let mut conn = db::open_in_memory().unwrap();
        // Four photos of what the grouping took for one person.
        let shots = ["aa", "bb", "cc", "dd"];
        for (n, name) in shots.iter().enumerate() {
            photo(&mut conn, name);
            let embedding = face_of(0, 0.02 * n as f32);
            let tx = conn.transaction().unwrap();
            let face = NewFace {
                bounds: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                embedding: &embedding,
            };
            db::save_faces(&tx, &name.repeat(32), &[face]).unwrap();
            tx.commit().unwrap();
        }
        group_faces(&mut conn).unwrap();
        let group = db::face_groups(&conn).unwrap()[0].clone();
        let apply = |conn: &mut Connection, ops: Vec<Op>, ts: i64| {
            let tx = conn.transaction().unwrap();
            for (i, op) in ops.into_iter().enumerate() {
                db::apply_event(
                    &tx,
                    "device",
                    &Event {
                        ts: ts + i as i64,
                        op,
                    },
                )
                .unwrap();
            }
            tx.commit().unwrap();
        };
        let ops = naming_ops(&conn, &[group.cluster], "Alice").unwrap();
        apply(&mut conn, ops, 1);

        // The named cover photo and one more are someone else.
        let other = shots
            .iter()
            .map(|s| s.repeat(32))
            .find(|f| *f != group.cover)
            .unwrap();
        let moved = [group.cover.clone(), other];
        let new = db::split_faces(&mut conn, &[group.cluster], &moved)
            .unwrap()
            .unwrap();
        db::add_not_person(&conn, new, "Alice").unwrap();
        let mut ops = naming_ops(&conn, &[new], "").unwrap();
        ops.extend(naming_ops(&conn, &[group.cluster], "Alice").unwrap());
        apply(&mut conn, ops, 10);

        let names: HashMap<i64, String> = group_names(&conn).unwrap().into_iter().collect();
        assert_eq!(names.get(&group.cluster).map(String::as_str), Some("Alice"));
        assert_eq!(names.get(&new), None);
        // Just as alike as before, but never merged back.
        assert_eq!(merge_groups(&mut conn).unwrap(), 0);
        assert!(suggestions(&conn, &[group.cluster], "Alice")
            .unwrap()
            .is_empty());

        // A group merged into one of them takes over the rule.
        let tx = conn.transaction().unwrap();
        db::merge_face_group(&tx, group.cluster, 99).unwrap();
        tx.commit().unwrap();
        assert_eq!(db::kept_apart(&conn).unwrap(), HashSet::from([(new, 99)]));

        // Splitting photos with no face in the group changes nothing.
        assert_eq!(
            db::split_faces(&mut conn, &[new], &["zz".repeat(32)]).unwrap(),
            None
        );
    }
}
