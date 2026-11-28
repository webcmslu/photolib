//! Groups the photos by who is in them.
//!
//! 1. **Finding faces** ([`find_faces`]): every photo's cached preview is
//!    searched for faces (see faces.rs). This reads the local thumbnail
//!    cache only, so it also works while the library is offline.
//! 2. **Grouping** ([`group_faces`]): each new clear face joins the existing
//!    group it resembles most, or starts a new group; blurry, small or
//!    turned-away faces only join a group they are very close to, and
//!    otherwise stay out of every group. Then groups that are clearly the
//!    same person are merged ([`merge_groups`]): the same person at
//!    different ages or angles often starts several groups.
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

/// Which faces shape the groups. Faces the detector is less sure of, or
/// smaller than this in the preview (blurred, far away, turned aside), look
/// alike to the model whoever they are: let into the grouping, they pull
/// different people into one group. They only join a group they are very
/// close to ([`WEAK_JOINS`]), and otherwise stay out of every group, as in
/// Apple Photos. Measured on a library of 57,000 faces against Apple
/// Photos' people: groups went from 77% to 99% the same person.
const STRONG_SCORE: f32 = 0.9;
const STRONG_PX: f32 = 60.0;
/// A face joins a group when it is at least this alike to the group's
/// average face...
const SAME_PERSON: f32 = 0.5;
/// ...and on average this alike to the [`NEAREST`] most alike of the
/// group's [`EXEMPLARS`] best faces: an average alone drifts towards an
/// everyone-face that blurry faces of anybody resemble.
const SAME_AS_MEMBERS: f32 = 0.55;
const NEAREST: usize = 3;
const EXEMPLARS: usize = 50;
/// How alike to a group's average a weak face must be to join it.
const WEAK_JOINS: f32 = 0.65;
/// How alike to a group's average a named face must be to join it. Lower:
/// the user said who it is. A named face must not start a group of its own
/// when it can join one: names were often given to so-so faces, and a group
/// grown around one of those leaves the person's other faces out.
const NAME_JOINS: f32 = 0.45;
/// How much a named face prefers a larger group: this much more alike per
/// tenfold more photos (+0.15 for 1,000 photos over 1).
const NAME_SIZE_PULL: f32 = 0.05;
/// Changed when the grouping changes so much that existing groups are
/// made again from scratch (names and corrections are kept).
pub const GROUPING_VERSION: i64 = 3;
/// A group nobody named is shown once it appears in this many photos;
/// smaller ones are mostly strangers in the background.
const MIN_PHOTOS_UNNAMED: u32 = 3;
/// Two groups whose average faces are at least this alike are merged,
/// unless they are seen together in a photo (two people, often brothers
/// and sisters) or have different names. The rest is left to suggestions.
const MERGE_GROUPS: f32 = 0.75;
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

/// Puts every face that has no group yet into one (or, for a weak face,
/// maybe none), then merges the groups that are clearly the same person.
/// The first time after the grouping changed, every face is grouped again;
/// what was said about the old groups follows their faces. Returns how many
/// faces and groups changed.
pub fn group_faces(conn: &mut Connection) -> rusqlite::Result<usize> {
    let regroup = db::grouping_version(conn)? < GROUPING_VERSION;
    let old = if regroup {
        db::ungroup_faces(conn)?
    } else {
        HashMap::new()
    };
    let faces = db::grouping_faces(conn)?;
    let named = named_faces(conn, &faces)?;
    let assigned = assign_groups(&faces, &named);
    db::set_face_clusters(conn, &assigned)?;
    if regroup {
        // Each old group becomes the new group most of its faces are in.
        let now = db::face_clusters(conn)?;
        let mut votes: HashMap<i64, HashMap<i64, usize>> = HashMap::new();
        for (face, before) in &old {
            if let Some(after) = now.get(face) {
                *votes.entry(*before).or_default().entry(*after).or_default() += 1;
            }
        }
        let map: HashMap<i64, i64> = votes
            .into_iter()
            .filter_map(|(before, counts)| {
                let after = counts.into_iter().max_by_key(|(group, n)| (*n, -group))?.0;
                Some((before, after))
            })
            .collect();
        db::remap_face_groups(conn, &map)?;
    }
    let grouped = assigned.iter().filter(|(_, cluster)| *cluster > 0).count();
    let merged = if grouped == 0 && !regroup {
        0
    } else {
        merge_groups(conn)?
    };
    if regroup {
        db::set_grouping_version(conn, GROUPING_VERSION)?;
    }
    Ok(assigned.len() + merged)
}

/// The faces that carry a name, with the name (lowercase): they are always
/// grouped.
fn named_faces(
    conn: &Connection,
    faces: &[db::GroupingFace],
) -> rusqlite::Result<HashMap<i64, String>> {
    let mut points: HashMap<String, Vec<(f64, f64, String)>> = HashMap::new();
    for (fingerprint, x, y, name) in db::face_names(conn)? {
        points
            .entry(fingerprint)
            .or_default()
            .push((x, y, name.trim().to_lowercase()));
    }
    Ok(faces
        .iter()
        .filter_map(|face| {
            let [left, top, width, height] = face.bounds;
            let (_, _, name) = points.get(&face.fingerprint)?.iter().find(|(x, y, _)| {
                (left..=left + width).contains(x) && (top..=top + height).contains(y)
            })?;
            Some((face.id, name.clone()))
        })
        .collect())
}

fn is_strong(face: &db::GroupingFace) -> bool {
    face.score >= STRONG_SCORE && face.preview_px >= STRONG_PX
}

/// Larger and surer faces first.
fn quality(face: &db::GroupingFace) -> f32 {
    face.score * face.preview_px.min(200.0)
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
pub(crate) fn group_names(conn: &Connection) -> rusqlite::Result<HashMap<i64, String>> {
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
    let mut apart = db::kept_apart(conn)?;
    apart.extend(db::groups_seen_together(conn)?);
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
    // A group seen in a photo together with them is someone else.
    let together: HashSet<i64> = db::groups_seen_together(conn)?
        .into_iter()
        .filter_map(|(a, b)| {
            if clusters.contains(&a) {
                Some(b)
            } else if clusters.contains(&b) {
                Some(a)
            } else {
                None
            }
        })
        .collect();
    let mut found: Vec<(f32, Person)> = Vec::new();
    for group in db::face_groups(conn)? {
        let id = group.cluster;
        if clusters.contains(&id)
            || together.contains(&id)
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

/// The grouping itself: returns the group chosen for each face that had
/// none (0 for a weak face that joins no group), and for each weak face
/// left out before that now joins one.
///
/// Strong faces are taken best first: each joins the group it is most alike
/// to (see [`SAME_PERSON`] and [`SAME_AS_MEMBERS`]) that has no other face
/// of the same photo, or starts a new group. Then each named face joins the
/// group it is most alike to ([`NAME_JOINS`]) that has no other name, or
/// starts one. Weak faces last only join a group they are very close to.
fn assign_groups(faces: &[db::GroupingFace], named: &HashMap<i64, String>) -> Vec<(i64, i64)> {
    /// A group is summed up by the sum of its strong faces' embeddings,
    /// which points to the "average face" of the group.
    struct Group<'a> {
        id: i64,
        sum: Vec<f32>,
        length: f32,
        photos: HashSet<&'a str>,
        /// Its best faces (indexes into `faces`), best first.
        exemplars: Vec<usize>,
    }
    fn add(group: &mut Group, embedding: &[f32]) {
        for (total, value) in group.sum.iter_mut().zip(embedding) {
            *total += value;
        }
        group.length = group.sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    }
    let strong = |face: &db::GroupingFace| named.contains_key(&face.id) || is_strong(face);
    let dimensions = faces.first().map_or(0, |face| face.embedding.len());

    // The existing groups, from their faces, best first.
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by(|a, b| quality(&faces[*b]).total_cmp(&quality(&faces[*a])));
    let mut groups: Vec<Group> = Vec::new();
    let mut index_of: HashMap<i64, usize> = HashMap::new();
    for &i in &order {
        let face = &faces[i];
        let Some(cluster) = face.cluster.filter(|c| *c > 0) else {
            continue;
        };
        let index = *index_of.entry(cluster).or_insert_with(|| {
            groups.push(Group {
                id: cluster,
                sum: vec![0.0; dimensions],
                length: 0.0,
                photos: HashSet::new(),
                exemplars: Vec::new(),
            });
            groups.len() - 1
        });
        let group = &mut groups[index];
        group.photos.insert(&face.fingerprint);
        if strong(face) {
            add(group, &face.embedding);
            if group.exemplars.len() < EXEMPLARS {
                group.exemplars.push(i);
            }
        }
    }

    let mut next_id = faces
        .iter()
        .filter_map(|face| face.cluster)
        .max()
        .unwrap_or(0)
        .max(0)
        + 1;
    let pending = |face: &db::GroupingFace| matches!(face.cluster, None | Some(0));
    let mut assigned = Vec::new();

    let first: Vec<usize> = order
        .iter()
        .copied()
        .filter(|i| {
            pending(&faces[*i]) && is_strong(&faces[*i]) && !named.contains_key(&faces[*i].id)
        })
        .collect();
    for i in first {
        let face = &faces[i];
        let mut candidates: Vec<(usize, f32)> = groups
            .par_iter()
            .enumerate()
            .filter(|(_, group)| group.length > 0.0)
            .map(|(index, group)| {
                (
                    index,
                    similarity(&group.sum, &face.embedding) / group.length,
                )
            })
            .filter(|(_, alike)| *alike >= SAME_PERSON)
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let chosen = candidates.into_iter().take(3).find_map(|(index, _)| {
            let group = &groups[index];
            if group.photos.contains(face.fingerprint.as_str()) {
                return None;
            }
            let mut alike: Vec<f32> = group
                .exemplars
                .iter()
                .map(|e| similarity(&faces[*e].embedding, &face.embedding))
                .collect();
            alike.sort_by(|a, b| b.total_cmp(a));
            let nearest = &alike[..alike.len().min(NEAREST)];
            let mean = nearest.iter().sum::<f32>() / nearest.len().max(1) as f32;
            (nearest.is_empty() || mean >= SAME_AS_MEMBERS).then_some(index)
        });
        let index = chosen.unwrap_or_else(|| {
            groups.push(Group {
                id: next_id,
                sum: vec![0.0; dimensions],
                length: 0.0,
                photos: HashSet::new(),
                exemplars: Vec::new(),
            });
            next_id += 1;
            groups.len() - 1
        });
        let group = &mut groups[index];
        add(group, &face.embedding);
        group.photos.insert(&face.fingerprint);
        if group.exemplars.len() < EXEMPLARS {
            group.exemplars.push(i);
        }
        assigned.push((face.id, group.id));
    }

    // Then the named faces, into the group most like them that has no other
    // name. The names already in groups come from their named faces.
    let unit_of = |group: &Group| -> Vec<f32> {
        group
            .sum
            .iter()
            .map(|v| v / group.length.max(f32::EPSILON))
            .collect()
    };
    let mut name_of: HashMap<usize, String> = HashMap::new();
    for face in faces {
        if let (Some(cluster), Some(name)) = (face.cluster.filter(|c| *c > 0), named.get(&face.id))
        {
            if let Some(index) = index_of.get(&cluster) {
                name_of.entry(*index).or_insert_with(|| name.clone());
            }
        }
    }
    // Each group goes to the name whose face is most like it: the closest
    // (face, group) pairs are settled first, so a relative's name can't
    // take a group from the person it belongs to.
    let named_pending: Vec<usize> = order
        .iter()
        .copied()
        .filter(|i| pending(&faces[*i]) && named.contains_key(&faces[*i].id))
        .collect();
    let units: Vec<Vec<f32>> = groups.iter().map(unit_of).collect();
    let mut pairs: Vec<(f32, usize, usize)> = named_pending
        .iter()
        .flat_map(|&i| {
            let face = &faces[i];
            groups
                .iter()
                .enumerate()
                .filter(|(_, group)| {
                    group.length > 0.0 && !group.photos.contains(face.fingerprint.as_str())
                })
                .map(|(index, _)| (similarity(&units[index], &face.embedding), i, index))
                .filter(|(alike, _, _)| *alike >= NAME_JOINS)
                // About as alike, the larger group wins: a person's own
                // group over a handful of shots that happen to be close.
                .map(|(alike, i, index)| {
                    let photos = groups[index].photos.len().max(1) as f32;
                    (alike + NAME_SIZE_PULL * photos.log10(), i, index)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut placed: HashMap<usize, usize> = HashMap::new();
    // A group takes one face per photo, named ones included.
    let mut taken: HashSet<(usize, &str)> = HashSet::new();
    for (_, i, index) in pairs {
        let face = &faces[i];
        let name = &named[&face.id];
        if placed.contains_key(&i)
            || name_of.get(&index).is_some_and(|other| other != name)
            || !taken.insert((index, face.fingerprint.as_str()))
        {
            continue;
        }
        name_of.insert(index, name.clone());
        placed.insert(i, index);
    }
    for i in named_pending {
        let face = &faces[i];
        let index = match placed.get(&i) {
            Some(&index) => index,
            None => {
                groups.push(Group {
                    id: next_id,
                    sum: vec![0.0; dimensions],
                    length: 0.0,
                    photos: HashSet::new(),
                    exemplars: Vec::new(),
                });
                next_id += 1;
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        add(group, &face.embedding);
        group.photos.insert(&face.fingerprint);
        if group.exemplars.len() < EXEMPLARS {
            group.exemplars.push(i);
        }
        assigned.push((face.id, group.id));
    }

    // Then the weak faces: only into a group they are very close to.
    let units: Vec<Vec<f32>> = groups.iter().map(unit_of).collect();
    let weak: Vec<usize> = (0..faces.len())
        .filter(|i| pending(&faces[*i]) && !strong(&faces[*i]))
        .collect();
    let picks: Vec<(usize, Option<usize>)> = weak
        .par_iter()
        .map(|&i| {
            let face = &faces[i];
            let best = groups
                .iter()
                .enumerate()
                .filter(|(_, group)| {
                    group.length > 0.0 && !group.photos.contains(face.fingerprint.as_str())
                })
                .map(|(index, _)| (index, similarity(&units[index], &face.embedding)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            (
                i,
                best.filter(|(_, alike)| *alike >= WEAK_JOINS)
                    .map(|(index, _)| index),
            )
        })
        .collect();
    for (i, index) in picks {
        let face = &faces[i];
        match index {
            // Two weak faces of one photo may both pick the same group.
            Some(index) if groups[index].photos.insert(&face.fingerprint) => {
                assigned.push((face.id, groups[index].id));
            }
            _ if face.cluster.is_none() => assigned.push((face.id, 0)),
            _ => {}
        }
    }
    assigned.sort_unstable();
    assigned
}

/// A person's face that may be someone else's, to check.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubtfulFace {
    pub fingerprint: String,
    /// Left, top, width, height, as fractions of the upright photo.
    pub bounds: [f64; 4],
    /// The photo's size, when known.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// How much it looks like the person's other faces of the same years
    /// (1 the same, around 0 someone else).
    pub likeness: f32,
}

/// A person's faces not checked yet, least like them first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCheck {
    /// How many faces look doubtful, not yet checked.
    pub doubtful: usize,
    /// How many faces are left to check, doubtful or not.
    pub unchecked: usize,
    /// The least alike first, at most the number asked for.
    pub faces: Vec<DoubtfulFace>,
}

/// A face looking less than this like the person's other faces of the same
/// years is doubtful (see [`check_person`]).
const DOUBTFUL: f32 = 0.45;
/// The person's faces taken this close in time (ms) are what a face is
/// compared with: people change, children most of all.
const SAME_YEARS: i64 = 365 * 24 * 3600 * 1000;
/// Fewer faces than this in those years: compared with all of them.
const MIN_COMPARED: usize = 20;
const DAY: i64 = 24 * 3600 * 1000;

/// Lists the faces of the person made of `clusters` not checked yet, those
/// that look least like them first: a relative who looks alike can still
/// pass for them, so the list goes on as far as the user wants. Each face
/// is compared with the average of their faces from the year
/// before and after, leaving out the photos of the same day (a series of
/// photos of someone else would otherwise vouch for itself). Faces the user
/// checked aren't shown again, but still count as the person's.
pub fn check_person(
    conn: &Connection,
    clusters: &[i64],
    limit: usize,
) -> rusqlite::Result<PersonCheck> {
    let mut faces = db::faces_to_check(conn, clusters)?;
    faces.sort_by_key(|face| face.sort_date);
    let dims = faces.first().map_or(0, |face| face.embedding.len());
    if faces.len() < 2 || dims == 0 {
        return Ok(PersonCheck {
            doubtful: 0,
            unchecked: 0,
            faces: Vec::new(),
        });
    }
    // Running sums, to add up any span of faces at once.
    let mut running = vec![vec![0f32; dims]];
    for face in &faces {
        let mut next = running.last().unwrap().clone();
        for (total, value) in next.iter_mut().zip(&face.embedding) {
            *total += value;
        }
        running.push(next);
    }
    let span_sum = |from: usize, to: usize| -> Vec<f32> {
        running[to]
            .iter()
            .zip(&running[from])
            .map(|(a, b)| a - b)
            .collect()
    };
    let first_at = |date: i64| faces.partition_point(|face| face.sort_date < date);
    let mut doubtful: Vec<DoubtfulFace> = Vec::new();
    let mut seen = HashSet::new();
    for face in &faces {
        if face.confirmed {
            continue;
        }
        let day = face.sort_date.div_euclid(DAY) * DAY;
        let (day_from, day_to) = (first_at(day), first_at(day + DAY));
        let (from, to) = (
            first_at(face.sort_date - SAME_YEARS),
            first_at(face.sort_date + SAME_YEARS + 1),
        );
        let (from, to) = if to - from - (day_to - day_from) < MIN_COMPARED {
            (0, faces.len())
        } else {
            (from, to)
        };
        let mut reference = span_sum(from, to);
        for (total, value) in reference.iter_mut().zip(span_sum(day_from, day_to)) {
            *total -= value;
        }
        if reference.iter().all(|v| *v == 0.0) {
            continue; // all their photos are from that one day
        }
        let likeness = similarity(&face.embedding, &unit(&reference));
        if seen.insert(face.fingerprint.clone()) {
            doubtful.push(DoubtfulFace {
                fingerprint: face.fingerprint.clone(),
                bounds: face.bounds,
                width: face.width,
                height: face.height,
                likeness,
            });
        }
    }
    doubtful.sort_by(|a, b| a.likeness.total_cmp(&b.likeness));
    let unchecked = doubtful.len();
    let total = doubtful
        .iter()
        .filter(|face| face.likeness < DOUBTFUL)
        .count();
    doubtful.truncate(limit);
    Ok(PersonCheck {
        doubtful: total,
        unchecked,
        faces: doubtful,
    })
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
    // The key photos people chose, while that face is still theirs.
    let keys = db::face_keys(conn)?;
    for (name, person) in named.iter_mut() {
        let Some((photo, x, y)) = keys.get(name) else {
            continue;
        };
        if let Some((size, bounds)) = db::face_of(conn, photo, &person.clusters, Some((*x, *y)))? {
            person.cover = photo.clone();
            person.cover_crop = square_around(bounds, size);
            person.cover_width = size.map(|(width, _)| width);
            person.cover_height = size.map(|(_, height)| height);
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

/// The log entry that makes the photo `fingerprint` the key photo of the
/// person `name`, made of `clusters`: their largest face in it. `None` if
/// they are not in it.
pub fn key_photo_op(
    conn: &Connection,
    clusters: &[i64],
    name: &str,
    fingerprint: &str,
) -> rusqlite::Result<Option<Op>> {
    let unit = |fraction: f64| (fraction * 10_000.0).round().clamp(0.0, 10_000.0) as u16;
    Ok(
        db::face_of(conn, fingerprint, clusters, None)?.map(|(_, [left, top, width, height])| {
            Op::FaceKey {
                photo: fingerprint.to_owned(),
                x: unit(left + width / 2.0),
                y: unit(top + height / 2.0),
                name: name.to_owned(),
            }
        }),
    )
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
    // A renamed person keeps their key photo.
    let old_name = group_names(conn)?
        .into_iter()
        .find(|(cluster, _)| clusters.contains(cluster))
        .map(|(_, old)| old.trim().to_lowercase());
    if let Some(old) =
        old_name.filter(|old| !name.trim().is_empty() && *old != name.trim().to_lowercase())
    {
        if let Some((photo, x, y)) = db::face_keys(conn)?.remove(&old) {
            ops.push(Op::FaceKey {
                photo,
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

    /// A face for [`assign_groups`]: strong unless `weak`.
    fn grouping_face(
        id: i64,
        cluster: Option<i64>,
        photo: &str,
        embedding: Vec<f32>,
        weak: bool,
    ) -> db::GroupingFace {
        db::GroupingFace {
            id,
            cluster,
            fingerprint: photo.repeat(64 / photo.len()),
            bounds: [0.1, 0.1, 0.2, 0.2],
            score: if weak { 0.8 } else { 0.95 },
            preview_px: 150.0,
            embedding,
        }
    }

    #[test]
    fn grouping_joins_look_alikes_and_keeps_existing_groups() {
        let f = grouping_face;
        let faces = vec![
            f(1, Some(7), "a", face_of(0, 0.0), false),
            f(2, None, "b", face_of(0, 0.3), false),
            f(3, None, "c", face_of(1, 0.0), false),
            f(4, None, "d", face_of(1, 0.2), false),
            f(5, None, "e", face_of(0, -0.2), false),
            f(6, None, "f", face_of(5, 0.0), false),
            // Alike, but in a photo group 7 already has a face in.
            f(7, None, "a", face_of(0, 0.1), false),
            // Weak: close enough to join, or not.
            f(8, None, "g", face_of(1, 0.1), true),
            f(9, None, "h", face_of(2, 0.0), true),
            // Weak and left out before: tried again.
            f(10, Some(0), "i", face_of(5, 0.1), true),
            f(11, Some(0), "j", face_of(6, 0.0), true),
        ];
        let assigned = assign_groups(&faces, &HashMap::new());
        // 2 and 5 join the existing group 7; 3 and 4 form a new one; 7 is
        // a second face of photo "a": someone else. Weak 8 and 10 join the
        // groups they are close to, 9 and 11 none (11 was already out).
        assert_eq!(
            assigned,
            [
                (2, 7),
                (3, 8),
                (4, 8),
                (5, 7),
                (6, 9),
                (7, 10),
                (8, 8),
                (9, 0),
                (10, 9)
            ]
        );
        assert!(assign_groups(&[], &HashMap::new()).is_empty());

        // A so-so named face joins the person's group instead of starting
        // its own, which would leave their clear faces unnamed.
        let mut seen = vec![
            f(1, None, "a", face_of(3, 0.0), false),
            f(2, None, "b", face_of(3, 0.1), false),
            f(3, None, "c", face_of(3, -0.1), false),
            f(4, None, "d", face_of(3, 0.6), true),
        ];
        let named = HashMap::from([(4, "bob".to_string())]);
        assert_eq!(
            assign_groups(&seen, &named),
            [(1, 1), (2, 1), (3, 1), (4, 1)]
        );
        // Unless the group already has another name.
        seen[0].cluster = Some(1);
        seen[1].cluster = Some(1);
        seen[2].cluster = Some(1);
        let named = HashMap::from([(1, "ann".to_string()), (4, "bob".to_string())]);
        assert_eq!(assign_groups(&seen[..], &named), [(4, 2)]);

        // A named face is grouped even when weak.
        let lone = [f(1, None, "a", face_of(3, 0.0), true)];
        assert_eq!(
            assign_groups(&lone, &HashMap::from([(1, "alice".to_string())])),
            [(1, 1)]
        );
        assert_eq!(assign_groups(&lone, &HashMap::new()), [(1, 0)]);
    }

    #[test]
    fn a_new_grouping_regroups_and_keeps_corrections() {
        let mut conn = db::open_in_memory().unwrap();
        // Two people, each in two photos, and both in photo "ee".
        for (name, faces) in [
            ("aa", vec![face_of(0, 0.0)]),
            ("bb", vec![face_of(0, 0.1)]),
            ("cc", vec![face_of(0, 1.0)]),
            ("dd", vec![face_of(0, 1.1)]),
            ("ee", vec![face_of(0, 0.05), face_of(0, 1.05)]),
        ] {
            photo(&mut conn, name);
            let tx = conn.transaction().unwrap();
            let bounds = [[0.1, 0.1, 0.2, 0.2], [0.6, 0.1, 0.2, 0.2]];
            let rows: Vec<NewFace> = faces
                .iter()
                .zip(bounds)
                .map(|(embedding, bounds)| NewFace {
                    bounds,
                    score: 0.95,
                    embedding,
                })
                .collect();
            db::save_faces(&tx, &name.repeat(32), &rows).unwrap();
            tx.commit().unwrap();
        }
        // An older grouping put everyone in group 40, said not to be Bob.
        conn.execute("UPDATE faces.face SET cluster = 40", [])
            .unwrap();
        db::add_not_person(&conn, 40, "Bob").unwrap();
        db::set_grouping_version(&conn, 1).unwrap();

        group_faces(&mut conn).unwrap();
        assert_eq!(db::grouping_version(&conn).unwrap(), GROUPING_VERSION);
        let groups: HashSet<i64> = db::face_clusters(&conn).unwrap().into_values().collect();
        // 0.7 alike, and seen together in "ee": two people, never merged.
        assert_eq!(groups.len(), 2);
        assert!(!groups.contains(&40));
        let refused = db::not_person(&conn).unwrap();
        assert_eq!(refused.len(), 1);
        assert!(groups.contains(&refused.iter().next().unwrap().0));
        // Grouping again changes nothing.
        assert_eq!(group_faces(&mut conn).unwrap(), 0);
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
        // Each photo's face can be shown where it is.
        let boxes = db::faces_in(&conn, &[group.cluster]).unwrap();
        assert_eq!(boxes.len(), 4);
        assert!(boxes.iter().all(|face| {
            face.bounds
                .iter()
                .zip([0.1, 0.1, 0.2, 0.2])
                .all(|(got, want)| (got - want).abs() < 1e-4)
        }));
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

        // A key photo of her own choosing shows on her card, and stays
        // through a rename.
        let chosen = shots
            .iter()
            .map(|s| s.repeat(32))
            .find(|f| *f != group.cover)
            .unwrap();
        let op = key_photo_op(&conn, &[group.cluster], "Alice", &chosen)
            .unwrap()
            .unwrap();
        apply(&mut conn, vec![op], 2);
        let cover = |conn: &Connection| people(conn).unwrap()[0].cover.clone();
        assert_eq!(cover(&conn), chosen);
        let ops = naming_ops(&conn, &[group.cluster], "Alicia").unwrap();
        apply(&mut conn, ops, 3);
        assert_eq!(cover(&conn), chosen);
        let ops = naming_ops(&conn, &[group.cluster], "Alice").unwrap();
        apply(&mut conn, ops, 5);
        assert_eq!(cover(&conn), chosen);
        // Not in the photo: no key photo.
        assert_eq!(
            key_photo_op(&conn, &[12345], "Alice", &chosen).unwrap(),
            None
        );

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

    #[test]
    fn a_check_shows_the_faces_least_like_the_person_first() {
        let mut conn = db::open_in_memory().unwrap();
        // 30 photos of one person over the days, two of them someone else.
        let names: Vec<String> = (0..30).map(|n| format!("p{n:02}")).collect();
        for (n, name) in names.iter().enumerate() {
            photo(&mut conn, name);
            conn.execute(
                "UPDATE photos SET sort_date = ?1 WHERE rel_path = ?2",
                rusqlite::params![n as i64 * DAY, format!("{name}.jpg")],
            )
            .unwrap();
            let who = if n == 7 || n == 20 { 4 } else { 0 };
            let embedding = face_of(who, 0.05 * (n % 3) as f32);
            let faces = [NewFace {
                bounds: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                embedding: &embedding,
            }];
            let tx = conn.transaction().unwrap();
            db::save_faces(&tx, &name.repeat(64 / name.len()), &faces).unwrap();
            tx.commit().unwrap();
        }
        conn.execute("UPDATE faces.face SET cluster = 7", [])
            .unwrap();

        let check = check_person(&conn, &[7], 5).unwrap();
        assert_eq!((check.doubtful, check.unchecked), (2, 30));
        let first: HashSet<&str> = check.faces[..2]
            .iter()
            .map(|face| &face.fingerprint[..3])
            .collect();
        assert_eq!(first, HashSet::from(["p07", "p20"]));
        assert_eq!(check.faces.len(), 5);

        // Checked faces aren't shown again.
        let shown: Vec<String> = check.faces.iter().map(|f| f.fingerprint.clone()).collect();
        db::confirm_faces(&mut conn, &[7], &shown[2..]).unwrap();
        let again = check_person(&conn, &[7], 100).unwrap();
        assert_eq!(again.unchecked, 27);
        assert!(again
            .faces
            .iter()
            .all(|face| !shown[2..].contains(&face.fingerprint)));
    }
}
