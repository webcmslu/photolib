//! Groups the photos by who is in them.
//!
//! 1. **Finding faces** ([`find_faces`]): every photo's cached preview is
//!    searched for faces (see faces.rs). This reads the local thumbnail
//!    cache only, so it also works while the library is offline.
//! 2. **Grouping** ([`group_faces`]): each new face joins the existing group
//!    it resembles most, or starts a new group.
//! 3. **People** ([`people`]): a group is a person. Groups are local to this
//!    computer, but names are shared: a name is attached to one face of a
//!    photo (`face_name` in the library's log), every computer gives it to
//!    the group that face is in, and groups with the same name are shown as
//!    one person. That is also how two groups of the same person are merged:
//!    give them the same name.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::Connection;
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
const MAX_PEOPLE: usize = 500;
const WRITE_BATCH: usize = 50;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

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

        let mut in_batch = 0;
        let mut tx = conn.transaction()?;
        for (fingerprint, faces) in receiver {
            if let Some(faces) = faces {
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
            done += 1;
            in_batch += 1;
            if in_batch >= WRITE_BATCH {
                tx.commit()?;
                tx = conn.transaction()?;
                in_batch = 0;
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                report(done);
            }
        }
        tx.commit()
    })?;
    Ok(done)
}

/// Puts every face that has no group yet into one. Returns how many were.
pub fn group_faces(conn: &mut Connection) -> rusqlite::Result<usize> {
    let faces = db::face_embeddings(conn)?;
    let assigned = assign_groups(&faces);
    db::set_face_clusters(conn, &assigned)?;
    Ok(assigned.len())
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
        let Some(cluster) = cluster else { continue };
        let index = *index_of.entry(*cluster).or_insert_with(|| {
            groups.push(Group {
                id: *cluster,
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
    /// The photo to show them with, and the square part of it around their
    /// face: left, top, width, height in ten-thousandths of the photo.
    pub cover: String,
    pub cover_crop: [u16; 4],
    pub cover_width: Option<u32>,
    pub cover_height: Option<u32>,
}

/// The people of a library: named ones first, then by number of photos.
pub fn people(conn: &Connection) -> rusqlite::Result<Vec<Person>> {
    let mut name_of: HashMap<i64, String> = HashMap::new();
    for (fingerprint, x, y, name) in db::face_names(conn)? {
        if let Some(cluster) = db::cluster_at(conn, &fingerprint, x, y)? {
            name_of.entry(cluster).or_insert(name);
        }
    }

    let mut named: HashMap<String, Person> = HashMap::new();
    let mut unnamed: Vec<Person> = Vec::new();
    let mut groups = db::face_groups(conn)?;
    // Largest first, so that a person's biggest group provides the cover.
    groups.sort_by(|a, b| b.photos.cmp(&a.photos).then(a.cluster.cmp(&b.cluster)));
    for group in groups {
        let person = Person {
            name: name_of.get(&group.cluster).cloned(),
            clusters: vec![group.cluster],
            photos: group.photos,
            cover_crop: square_around(group.cover_bounds, group.cover_size),
            cover: group.cover,
            cover_width: group.cover_size.map(|(width, _)| width),
            cover_height: group.cover_size.map(|(_, height)| height),
        };
        match &person.name {
            Some(name) => match named.get_mut(&name.to_lowercase()) {
                Some(same) => {
                    same.clusters.push(group.cluster);
                    same.photos += group.photos;
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
}
