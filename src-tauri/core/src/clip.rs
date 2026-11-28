//! Search by content: finds what the photos show (a beach, a dog, a
//! birthday…), so that typing "plage", "Strand" or "beach" finds them.
//!
//! OpenAI's **CLIP** (ViT-B/32, MIT licence, as exported to ONNX by Xenova)
//! turns each photo's grid thumbnail into 512 numbers, a point in a space
//! where pictures and descriptions of the same thing lie close together. The
//! points of the descriptions of a fixed list of concepts (`data/concepts.tsv`)
//! were worked out once with CLIP's text model, and ship with the app
//! (`data/concepts.bin`, see `examples/clip_concepts.rs`): only the image
//! model is needed here. It is downloaded (352 MB) the first time search by
//! content is switched on, and checked against a known fingerprint.
//!
//! Each photo gets the concepts it shows most clearly, its tags, kept in a
//! database file next to the index (`content.db`, as `content.…`); search
//! looks them up for the words of the concepts, in English, French and
//! German. The points themselves are kept too, so a new list of concepts
//! only needs the tags worked out again, not the photos looked at again.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

use image::RgbImage;
use rayon::prelude::*;
use rusqlite::{params, Connection, TransactionBehavior};
use tract_onnx::prelude::*;

use crate::indexer::{Phase, Progress};
use crate::models::{self, ModelFile};
use crate::thumbs::{ThumbCache, ThumbSize};

/// The length of the points CLIP gives pictures and words.
pub const DIMENSIONS: usize = 512;
/// The side of the square picture CLIP looks at.
const SIDE: usize = 224;
/// The colours CLIP was trained with, per channel.
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

const VISION: ModelFile = ModelFile {
    name: "clip_vit_b32_vision.onnx",
    urls: &[
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model.onnx",
        "https://github.com/webcmslu/phototek/releases/download/models/clip_vit_b32_vision.onnx",
    ],
    hash: "855f1da3807ced99b072dd179856564a0768e99dd84f318ed0914bf34a0fa108",
    size: 351_685_709,
};

/// Download size, for the UI.
pub const DOWNLOAD_BYTES: u64 = VISION.size;

/// How a photo gets a concept as a tag (tuned on a family library: see
/// docs/search.md). CLIP's own scale for comparing a picture with the
/// descriptions, which turns the similarities into shares that add up to 1.
const LOGIT_SCALE: f32 = 100.0;
/// A concept with at least this share is a tag.
const SURE_SHARE: f32 = 0.3;
/// Otherwise a concept among the photo's first few…
const CLOSE_RANK: usize = 4;
/// …that stands out this much (in standard deviations) from all the
/// concepts, for this photo.
const CLOSE_STANDOUT: f32 = 2.6;
/// Never below this similarity: the photo shows nothing CLIP knows.
const MIN_SIMILARITY: f32 = 0.2;
/// And, once the library has enough photos to tell, this far (in standard
/// deviations) above how close the concept usually is to this library's
/// photos: some concepts ("party", "child") are close to every family
/// photo, and would otherwise tag thousands of them.
const ABOVE_USUAL: f32 = 2.0;
/// The photos needed to tell how close each concept usually is.
const MIN_CALIBRATION: u64 = 300;
/// Changes when the rule above changes, so the tags are worked out again.
const RULE_VERSION: u32 = 2;

const WRITE_BATCH: usize = 50;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const WRITE_INTERVAL: Duration = Duration::from_secs(2);

/// True once the model is on this computer.
pub fn model_present(app_data_dir: &Path) -> bool {
    models::present(app_data_dir, &[&VISION])
}

/// Downloads the model if it is missing. `on_progress` gets the bytes
/// received so far, out of [`DOWNLOAD_BYTES`].
pub fn download_model(app_data_dir: &Path, on_progress: &dyn Fn(u64)) -> io::Result<()> {
    models::download(app_data_dir, &[&VISION], on_progress)
}

/// The file whose presence means the user switched search by content off.
/// The model stays, so switching it back on needs no download.
fn off_marker(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir.join("content-off")
}

/// True when the user switched search by content off.
pub fn switched_off(app_data_dir: &Path) -> bool {
    off_marker(app_data_dir).exists()
}

/// Switches search by content off or (back) on.
pub fn set_switched_off(app_data_dir: &Path, off: bool) -> io::Result<()> {
    let marker = off_marker(app_data_dir);
    if off {
        std::fs::write(marker, b"")
    } else {
        match std::fs::remove_file(marker) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// The loaded image model. Loading takes a moment; make one and share it
/// between threads.
pub struct ContentModel {
    model: std::sync::Arc<TypedRunnableModel>,
}

impl ContentModel {
    pub fn load(app_data_dir: &Path) -> Result<Self, String> {
        let load = || -> TractResult<_> {
            let model = tract_onnx::onnx().model_for_path(models::path(app_data_dir, &VISION))?;
            // The model leaves the number of pictures open; one at a time.
            let batch = model.symbols.sym("batch_size");
            model
                .with_input_fact(
                    0,
                    f32::fact(&[batch.to_dim(), 3.to_dim(), SIDE.to_dim(), SIDE.to_dim()]).into(),
                )?
                .into_optimized()?
                .into_runnable()
        };
        Ok(Self {
            model: load().map_err(|e| e.to_string())?,
        })
    }

    /// The point of a picture, of length 1. The middle of the picture is
    /// what CLIP looks at, as it was trained.
    pub fn embed(&self, picture: &RgbImage) -> Result<Vec<f32>, String> {
        let (width, height) = picture.dimensions();
        if width == 0 || height == 0 {
            return Err("empty picture".into());
        }
        let scale = SIDE as f32 / width.min(height) as f32;
        let fit = |side: u32| ((side as f32 * scale).round() as u32).max(SIDE as u32);
        let resized = image::imageops::resize(
            picture,
            fit(width),
            fit(height),
            image::imageops::FilterType::CatmullRom,
        );
        let left = (resized.width() - SIDE as u32) / 2;
        let top = (resized.height() - SIDE as u32) / 2;
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, SIDE, SIDE), |(_, c, y, x)| {
            let value = resized.get_pixel(left + x as u32, top + y as u32)[c];
            (f32::from(value) / 255.0 - MEAN[c]) / STD[c]
        });
        let output = self
            .model
            .run(tvec!(input.into_tensor().into()))
            .map_err(|e| e.to_string())?;
        let point = output[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        Ok(normalized(point.iter().copied()))
    }
}

fn normalized(values: impl Iterator<Item = f32>) -> Vec<f32> {
    let values: Vec<f32> = values.collect();
    let length = values.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    values.into_iter().map(|v| v / length).collect()
}

/// One thing search by content knows.
pub struct Concept {
    /// Its name in the database, e.g. "beach".
    pub key: String,
    /// The words that find it, in every language, folded (see [`fold`]).
    pub words: Vec<String>,
}

struct Vocabulary {
    concepts: Vec<Concept>,
    /// One point per concept, of length 1, one after the other.
    points: Vec<f32>,
    /// Folded word → the concepts it names.
    by_word: HashMap<String, Vec<usize>>,
    /// The words of several words ("salle de bain"), split, longest first.
    phrases: Vec<Vec<String>>,
    /// Changes with the list, its points or the rule.
    version: String,
}

const CONCEPTS_TSV: &str = include_str!("../data/concepts.tsv");
const CONCEPTS_BIN: &[u8] = include_bytes!("../data/concepts.bin");

fn vocabulary() -> &'static Vocabulary {
    static VOCABULARY: OnceLock<Vocabulary> = OnceLock::new();
    VOCABULARY.get_or_init(|| {
        let mut concepts = Vec::new();
        for line in CONCEPTS_TSV.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let words = fields[2..]
                .iter()
                .flat_map(|list| list.split(','))
                .map(|word| fold(word.trim()))
                .filter(|word| !word.is_empty())
                .collect();
            concepts.push(Concept {
                key: fields[0].to_owned(),
                words,
            });
        }
        let points: Vec<f32> = CONCEPTS_BIN
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        assert_eq!(
            points.len(),
            concepts.len() * DIMENSIONS,
            "concepts.bin doesn't match concepts.tsv: run examples/clip_concepts.rs"
        );
        let mut by_word: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, concept) in concepts.iter().enumerate() {
            for word in &concept.words {
                let list = by_word.entry(word.clone()).or_default();
                if !list.contains(&index) {
                    list.push(index);
                }
            }
        }
        let mut phrases: Vec<Vec<String>> = by_word
            .keys()
            .filter(|word| word.contains(' '))
            .map(|word| word.split(' ').map(str::to_owned).collect())
            .collect();
        phrases.sort_by_key(|words: &Vec<String>| std::cmp::Reverse(words.len()));
        let mut hasher = blake3::Hasher::new();
        hasher.update(CONCEPTS_TSV.as_bytes());
        hasher.update(CONCEPTS_BIN);
        hasher.update(&RULE_VERSION.to_le_bytes());
        let version = hasher.finalize().to_hex()[..16].to_owned();
        Vocabulary {
            concepts,
            points,
            by_word,
            phrases,
            version,
        }
    })
}

/// Lower case, without accents, and with straight apostrophes, as words are
/// compared: "Château" and "chateau" are the same word, "Straße" and
/// "strasse" too.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => out.push('a'),
            'ç' => out.push('c'),
            'è' | 'é' | 'ê' | 'ë' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' => out.push('i'),
            'ñ' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => out.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => out.push('u'),
            'ÿ' => out.push('y'),
            'ß' => out.push_str("ss"),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            '’' | '‘' => out.push('\''),
            c if c.is_whitespace() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// The concepts a search word names (none for most words), by key.
pub fn concepts_for(word: &str) -> Vec<&'static str> {
    let vocabulary = vocabulary();
    vocabulary
        .by_word
        .get(&fold(word))
        .map(|indexes| {
            indexes
                .iter()
                .map(|&index| vocabulary.concepts[index].key.as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// Splits a search into words, keeping together the words of a concept that
/// takes several ("salle de bain", "feu d'artifice").
pub fn search_words(query: &str) -> Vec<String> {
    let words: Vec<&str> = query.split_whitespace().collect();
    let folded: Vec<String> = words.iter().map(|word| fold(word)).collect();
    let phrases = &vocabulary().phrases;
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let phrase = phrases
            .iter()
            .find(|phrase| folded.get(i..i + phrase.len()) == Some(phrase.as_slice()));
        let length = phrase.map_or(1, Vec::len);
        out.push(words[i..i + length].join(" "));
        i += length;
    }
    out
}

/// The SQL condition for the photos that show one of `concepts`.
pub fn condition(concepts: &[&str]) -> String {
    // Keys are plain lower-case words (see concepts.tsv), safe in quotes.
    let keys: Vec<String> = concepts
        .iter()
        .filter(|key| key.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .map(|key| format!("'{key}'"))
        .collect();
    format!(
        "fingerprint IN (SELECT fingerprint FROM content.tag WHERE concept IN ({}))",
        keys.join(", ")
    )
}

/// How close each concept usually is to a library's photos.
#[derive(serde::Serialize, serde::Deserialize)]
struct Calibration {
    /// The photos it was worked out from.
    photos: u64,
    mean: Vec<f32>,
    deviation: Vec<f32>,
}

fn similarities(point: &[f32]) -> Vec<f32> {
    vocabulary()
        .points
        .chunks_exact(DIMENSIONS)
        .map(|concept| concept.iter().zip(point).map(|(a, b)| a * b).sum())
        .collect()
}

/// The concepts a photo shows, by index, with their similarity.
fn tags(point: &[f32], calibration: Option<&Calibration>) -> Vec<(usize, f32)> {
    let vocabulary = vocabulary();
    let count = vocabulary.concepts.len();
    let similarities = similarities(point);
    let mean = similarities.iter().sum::<f32>() / count as f32;
    let deviation = (similarities.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / count as f32)
        .sqrt()
        .max(1e-6);
    let top = similarities.iter().copied().fold(f32::MIN, f32::max);
    let exponentials: Vec<f32> = similarities
        .iter()
        .map(|s| ((s - top) * LOGIT_SCALE).exp())
        .collect();
    let total: f32 = exponentials.iter().sum();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|a, b| similarities[*b].total_cmp(&similarities[*a]));
    order
        .iter()
        .enumerate()
        .filter(|&(rank, &index)| {
            let similarity = similarities[index];
            let share = exponentials[index] / total;
            let standout = (similarity - mean) / deviation;
            let usual = calibration
                .is_none_or(|c| (similarity - c.mean[index]) / c.deviation[index] >= ABOVE_USUAL);
            similarity >= MIN_SIMILARITY
                && usual
                && (share >= SURE_SHARE || (rank < CLOSE_RANK && standout >= CLOSE_STANDOUT))
        })
        .map(|(_, &index)| (index, similarities[index]))
        .collect()
}

/// The keys of the concepts a photo's point shows (for tests and tuning).
pub fn tag_keys(point: &[f32]) -> Vec<&'static str> {
    tags(point, None)
        .into_iter()
        .map(|(index, _)| vocabulary().concepts[index].key.as_str())
        .collect()
}

fn to_bytes(point: &[f32]) -> Vec<u8> {
    point.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// Writes a photo's point and its tags.
fn store(
    conn: &Connection,
    fingerprint: &str,
    point: &[f32],
    calibration: Option<&Calibration>,
) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO content.embedding (fingerprint, point) VALUES (?1, ?2)",
    )?
    .execute(params![fingerprint, to_bytes(point)])?;
    write_tags(conn, fingerprint, point, calibration)
}

fn write_tags(
    conn: &Connection,
    fingerprint: &str,
    point: &[f32],
    calibration: Option<&Calibration>,
) -> rusqlite::Result<()> {
    conn.prepare_cached("DELETE FROM content.tag WHERE fingerprint = ?1")?
        .execute([fingerprint])?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO content.tag (concept, fingerprint, similarity) VALUES (?1, ?2, ?3)",
    )?;
    for (index, similarity) in tags(point, calibration) {
        insert.execute(params![
            vocabulary().concepts[index].key,
            fingerprint,
            similarity
        ])?;
    }
    Ok(())
}

fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM content.meta WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .ok()
}

/// The library's calibration, once it has enough photos.
fn calibration(conn: &Connection) -> Option<Calibration> {
    serde_json::from_str(&meta(conn, "calibration")?).ok()
}

/// Each stored point, skipping the empty ones.
fn each_point(
    conn: &Connection,
    mut handle: impl FnMut(String, Vec<f32>) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let mut points = conn.prepare("SELECT fingerprint, point FROM content.embedding")?;
    let mut rows = points.query([])?;
    while let Some(row) = rows.next()? {
        let point = from_bytes(&row.get::<_, Vec<u8>>(1)?);
        if point.len() == DIMENSIONS {
            handle(row.get(0)?, point)?;
        }
    }
    Ok(())
}

/// Works the tags out again from the stored points when the list of
/// concepts or the rule changed, or when the library grew by a quarter since
/// it was last calibrated (a few seconds for a large library).
pub fn retag_if_needed(conn: &mut Connection) -> rusqlite::Result<bool> {
    let version = &vocabulary().version;
    let photos = conn.query_row(
        "SELECT count(*) FROM content.embedding WHERE length(point) > 0",
        [],
        |row| row.get::<_, i64>(0),
    )? as u64;
    let calibrated = calibration(conn).map_or(0, |c| c.photos);
    let grew = photos >= MIN_CALIBRATION && photos > calibrated + calibrated / 4;
    if meta(conn, "vocabulary").as_ref() == Some(version) && !grew {
        return Ok(false);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let calibration = if photos >= MIN_CALIBRATION {
        let count = vocabulary().concepts.len();
        let (mut sum, mut squares) = (vec![0f64; count], vec![0f64; count]);
        each_point(&tx, |_, point| {
            for (index, similarity) in similarities(&point).into_iter().enumerate() {
                sum[index] += f64::from(similarity);
                squares[index] += f64::from(similarity).powi(2);
            }
            Ok(())
        })?;
        let n = photos as f64;
        let mean: Vec<f32> = sum.iter().map(|s| (s / n) as f32).collect();
        let deviation = squares
            .iter()
            .zip(&sum)
            .map(|(q, s)| ((q / n - (s / n).powi(2)).max(0.0).sqrt() as f32).max(1e-6))
            .collect();
        Some(Calibration {
            photos,
            mean,
            deviation,
        })
    } else {
        None
    };
    tx.execute("DELETE FROM content.tag", [])?;
    each_point(&tx, |fingerprint, point| {
        write_tags(&tx, &fingerprint, &point, calibration.as_ref())
    })?;
    tx.execute(
        "INSERT OR REPLACE INTO content.meta (key, value) VALUES ('vocabulary', ?1)",
        [version],
    )?;
    match &calibration {
        Some(calibration) => tx.execute(
            "INSERT OR REPLACE INTO content.meta (key, value) VALUES ('calibration', ?1)",
            [serde_json::to_string(calibration).unwrap_or_default()],
        )?,
        None => tx.execute("DELETE FROM content.meta WHERE key = 'calibration'", [])?,
    };
    tx.commit()?;
    Ok(true)
}

/// Looks at the photos and videos that have a thumbnail but no point yet,
/// on half the processor's cores (the other half keeps the app, or the
/// server, quick). Returns how many were looked at.
pub fn embed_pending(
    conn: &mut Connection,
    cache: &ThumbCache,
    model: &ContentModel,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> rusqlite::Result<usize> {
    retag_if_needed(conn)?;
    let calibration = calibration(conn);
    let pending: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT fingerprint FROM photos
             WHERE thumb_state = 1 AND part = 0
               AND fingerprint NOT IN (SELECT fingerprint FROM content.embedding)",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let total = pending.len() as u64;
    if total == 0 {
        return Ok(0);
    }
    let report = |done| {
        on_progress(Progress {
            phase: Phase::Content,
            discovered: 0,
            total,
            done,
            known: 0,
        })
    };
    report(0);

    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().div_ceil(2));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let mut done = 0u64;
    let mut last_progress = Instant::now();
    std::thread::scope(|scope| -> rusqlite::Result<()> {
        let (sender, receiver) = mpsc::sync_channel::<(&String, Vec<f32>)>(WRITE_BATCH);
        let pending = &pending;
        let pool = &pool;
        scope.spawn(move || {
            pool.install(|| {
                pending
                    .par_iter()
                    .for_each_with(sender, |sender, fingerprint| {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        // A thumbnail that can't be read gets an empty point,
                        // so it isn't tried again at every scan.
                        let point = image::open(cache.path(ThumbSize::Grid, fingerprint))
                            .ok()
                            .and_then(|picture| model.embed(&picture.into_rgb8()).ok())
                            .unwrap_or_default();
                        let _ = sender.send((fingerprint, point));
                    });
            });
        });

        let mut batch: Vec<(&String, Vec<f32>)> = Vec::with_capacity(WRITE_BATCH);
        let mut last_write = Instant::now();
        let mut write = |batch: &mut Vec<(&String, Vec<f32>)>| -> rusqlite::Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for (fingerprint, point) in batch.drain(..) {
                store(&tx, fingerprint, &point, calibration.as_ref())?;
            }
            tx.commit()
        };
        for item in receiver {
            batch.push(item);
            done += 1;
            if batch.len() >= WRITE_BATCH || last_write.elapsed() >= WRITE_INTERVAL {
                write(&mut batch)?;
                last_write = Instant::now();
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                report(done);
                last_progress = Instant::now();
            }
        }
        write(&mut batch)
    })?;
    report(done);
    // The first time, or after many new photos: calibrated on all of them.
    retag_if_needed(conn)?;
    Ok(done as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_and_its_points_match() {
        let vocabulary = vocabulary();
        assert!(vocabulary.concepts.len() > 200);
        // Each point has length 1.
        for point in vocabulary.points.chunks_exact(DIMENSIONS) {
            let length: f32 = point.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((length - 1.0).abs() < 1e-3);
        }
        // Keys are safe to write into SQL.
        for concept in &vocabulary.concepts {
            assert!(concept
                .key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }

    #[test]
    fn words_in_every_language_name_the_concepts() {
        assert_eq!(concepts_for("beach"), ["beach"]);
        assert_eq!(concepts_for("Plage"), ["beach"]);
        assert_eq!(concepts_for("strand"), ["beach"]);
        assert_eq!(concepts_for("chateau"), ["castle"]);
        assert_eq!(concepts_for("Straße"), ["street"]);
        assert!(concepts_for("tree").contains(&"christmastree"));
        assert!(concepts_for("canon").is_empty());
    }

    #[test]
    fn phrases_stay_together() {
        assert_eq!(
            search_words("salle de bain 2019"),
            ["salle de bain", "2019"]
        );
        assert_eq!(search_words("feu d’artifice"), ["feu d’artifice"]);
        assert_eq!(concepts_for("feu d’artifice"), ["fireworks"]);
        assert_eq!(search_words("de la plage"), ["de", "la", "plage"]);
    }

    #[test]
    fn a_concepts_own_point_is_tagged_with_it() {
        let vocabulary = vocabulary();
        let beach = vocabulary
            .concepts
            .iter()
            .position(|concept| concept.key == "beach")
            .unwrap();
        let point = &vocabulary.points[beach * DIMENSIONS..(beach + 1) * DIMENSIONS];
        assert_eq!(tag_keys(point).first(), Some(&"beach"));
    }

    #[test]
    fn tags_follow_the_points_and_the_list() {
        let mut conn = crate::db::open_in_memory().unwrap();
        let vocabulary = vocabulary();
        let snow = vocabulary
            .concepts
            .iter()
            .position(|concept| concept.key == "snow")
            .unwrap();
        let point = vocabulary.points[snow * DIMENSIONS..(snow + 1) * DIMENSIONS].to_vec();
        store(&conn, "f1", &point, None).unwrap();
        // Unreadable thumbnails: no tags.
        store(&conn, "f2", &[], None).unwrap();
        let tagged = |conn: &Connection| -> Vec<String> {
            conn.prepare("SELECT fingerprint FROM content.tag WHERE concept = 'snow'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(tagged(&conn), ["f1"]);
        // A new list: everything worked out again, once.
        assert!(retag_if_needed(&mut conn).unwrap());
        assert!(!retag_if_needed(&mut conn).unwrap());
        assert_eq!(tagged(&conn), ["f1"]);
    }

    #[test]
    fn large_libraries_are_calibrated_and_kept_so() {
        let mut conn = crate::db::open_in_memory().unwrap();
        let vocabulary = vocabulary();
        let count = vocabulary.concepts.len();
        // Photos that each look like one concept, a bit blurred.
        let photo = |n: usize| -> Vec<f32> {
            let a = &vocabulary.points[(n % count) * DIMENSIONS..][..DIMENSIONS];
            let b = &vocabulary.points[((n * 7 + 3) % count) * DIMENSIONS..][..DIMENSIONS];
            normalized(a.iter().zip(b).map(|(a, b)| a * 0.8 + b * 0.2))
        };
        let add = |conn: &Connection, range: std::ops::Range<usize>| {
            for n in range {
                store(conn, &format!("f{n}"), &photo(n), None).unwrap();
            }
        };
        add(&conn, 0..100);
        retag_if_needed(&mut conn).unwrap();
        assert!(calibration(&conn).is_none(), "too few photos to tell");
        add(&conn, 100..400);
        assert!(retag_if_needed(&mut conn).unwrap());
        assert_eq!(calibration(&conn).unwrap().photos, 400);
        // A few more photos: kept as is; a quarter more: calibrated again.
        add(&conn, 400..450);
        assert!(!retag_if_needed(&mut conn).unwrap());
        add(&conn, 450..520);
        assert!(retag_if_needed(&mut conn).unwrap());
        assert_eq!(calibration(&conn).unwrap().photos, 520);
        // Photos still find their concept.
        let tagged: i64 = conn
            .query_row(
                "SELECT count(DISTINCT fingerprint) FROM content.tag",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tagged > 400, "{tagged}");
    }
}
