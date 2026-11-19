//! Finds the faces in photos and describes each one with numbers that are
//! close together for the same person ("embeddings"), so that photos can be
//! grouped by who is in them. Everything runs on this computer.
//!
//! Two small neural networks from the OpenCV model zoo do the work:
//! - **YuNet** (MIT licence) finds faces and five points on each: the eyes,
//!   the nose and the corners of the mouth;
//! - **SFace** (Apache 2.0 licence) turns a face, turned upright and cut out
//!   using those points, into 128 numbers.
//!
//! The models are not part of the installer. They are downloaded (39 MB)
//! the first time the user switches face grouping on, and checked against
//! known fingerprints before use.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use image::RgbImage;
use tract_onnx::prelude::*;

/// The side of the square picture the detector looks at.
const DETECT_SIDE: u32 = 640;
/// The side of the upright face picture the recognizer looks at.
const FACE_SIDE: u32 = 112;
/// Detections the model is less sure of than this are dropped.
const MIN_SCORE: f32 = 0.75;
/// Faces smaller than this (in pixels of the picture given) are too blurry
/// to tell people apart.
const MIN_FACE_PX: f32 = 36.0;
/// Two boxes that overlap more than this are the same face found twice.
const SAME_FACE_OVERLAP: f32 = 0.3;

/// Where the five face points sit in the picture SFace was trained on.
const REFERENCE_POINTS: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

struct ModelFile {
    name: &'static str,
    url: &'static str,
    /// BLAKE3 hash of the file.
    hash: &'static str,
    size: u64,
}

const DETECTOR: ModelFile = ModelFile {
    name: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx",
    hash: "3d5938c4cd5a02dc416f1cd1f7fc1f662a22adc370477112c871954587e63431",
    size: 232_589,
};

const RECOGNIZER: ModelFile = ModelFile {
    name: "face_recognition_sface_2021dec.onnx",
    url: "https://huggingface.co/opencv/face_recognition_sface/resolve/main/face_recognition_sface_2021dec.onnx",
    hash: "584d0f5792e9e776ab5e5bf3f559f2310ff8fdd3b39f94dd364751046592b512",
    size: 38_696_353,
};

/// Total download size, for the UI.
pub const DOWNLOAD_BYTES: u64 = DETECTOR.size + RECOGNIZER.size;

fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// True once both model files are on this computer.
pub fn models_present(app_data_dir: &Path) -> bool {
    [&DETECTOR, &RECOGNIZER].iter().all(|model| {
        fs::metadata(models_dir(app_data_dir).join(model.name))
            .is_ok_and(|meta| meta.len() == model.size)
    })
}

/// The file whose presence means the user switched face grouping off in the
/// settings. The models stay, so switching it back on needs no download.
fn off_marker(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir.join("faces-off")
}

/// True when the user switched face grouping off.
pub fn switched_off(app_data_dir: &Path) -> bool {
    off_marker(app_data_dir).exists()
}

/// Switches face grouping off or (back) on.
pub fn set_switched_off(app_data_dir: &Path, off: bool) -> io::Result<()> {
    let marker = off_marker(app_data_dir);
    if off {
        fs::write(marker, b"")
    } else {
        match fs::remove_file(marker) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Downloads the model files that are missing. `on_progress` gets the bytes
/// received so far, out of [`DOWNLOAD_BYTES`].
pub fn download_models(app_data_dir: &Path, on_progress: &dyn Fn(u64)) -> io::Result<()> {
    let dir = models_dir(app_data_dir);
    fs::create_dir_all(&dir)?;
    let mut done = 0u64;
    for model in [&DETECTOR, &RECOGNIZER] {
        let target = dir.join(model.name);
        if fs::metadata(&target).is_ok_and(|meta| meta.len() == model.size) {
            done += model.size;
            on_progress(done);
            continue;
        }
        let response = ureq::get(model.url)
            .timeout(std::time::Duration::from_secs(600))
            .call()
            .map_err(|e| io::Error::other(format!("download failed: {e}")))?;
        let mut reader = response.into_reader().take(model.size + 1);
        let mut bytes = Vec::with_capacity(model.size as usize);
        let mut chunk = vec![0u8; 256 * 1024];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            on_progress(done + bytes.len() as u64);
        }
        // Never run a file that is not exactly the one we expect.
        if blake3::hash(&bytes).to_hex().as_str() != model.hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not the expected file", model.name),
            ));
        }
        let tmp = target.with_extension("part");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &target)?;
        done += model.size;
    }
    Ok(())
}

type Model = std::sync::Arc<TypedRunnableModel>;

/// The two loaded models. Loading takes a moment; make one and share it
/// between threads.
pub struct FaceFinder {
    detector: Model,
    recognizer: Model,
}

/// A face found in a picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// The box around the face, as fractions (0 to 1) of the picture:
    /// left, top, width, height.
    pub bounds: [f32; 4],
    pub score: f32,
    /// 128 numbers of length 1; the same person gives similar ones.
    pub embedding: Vec<f32>,
}

struct Detection {
    /// Left, top, right, bottom, in pixels of the original picture.
    bounds: [f32; 4],
    score: f32,
    points: [(f32, f32); 5],
}

impl FaceFinder {
    pub fn load(app_data_dir: &Path) -> Result<Self, String> {
        let load = |model: &ModelFile, side: u32| -> TractResult<Model> {
            tract_onnx::onnx()
                .model_for_path(models_dir(app_data_dir).join(model.name))?
                .with_input_fact(0, f32::fact([1, 3, side as usize, side as usize]).into())?
                .into_optimized()?
                .into_runnable()
        };
        Ok(Self {
            detector: load(&DETECTOR, DETECT_SIDE).map_err(|e| e.to_string())?,
            recognizer: load(&RECOGNIZER, FACE_SIDE).map_err(|e| e.to_string())?,
        })
    }

    /// Finds the faces in `picture` and describes each of them.
    pub fn find(&self, picture: &RgbImage) -> Result<Vec<Face>, String> {
        let (width, height) = picture.dimensions();
        let mut faces = Vec::new();
        for detection in self.detect(picture).map_err(|e| e.to_string())? {
            let [left, top, right, bottom] = detection.bounds;
            if (right - left).min(bottom - top) < MIN_FACE_PX {
                continue;
            }
            let upright = align(picture, &detection.points);
            let embedding = self.embed(&upright).map_err(|e| e.to_string())?;
            let (w, h) = (width as f32, height as f32);
            let left = (left / w).clamp(0.0, 1.0);
            let top = (top / h).clamp(0.0, 1.0);
            faces.push(Face {
                bounds: [
                    left,
                    top,
                    (right / w).clamp(0.0, 1.0) - left,
                    (bottom / h).clamp(0.0, 1.0) - top,
                ],
                score: detection.score,
                embedding,
            });
        }
        Ok(faces)
    }

    fn detect(&self, picture: &RgbImage) -> TractResult<Vec<Detection>> {
        // Shrink the picture to fit the square the model looks at, in its
        // top-left corner; the rest stays black.
        let (width, height) = picture.dimensions();
        let scale = DETECT_SIDE as f32 / width.max(height) as f32;
        let small = image::imageops::resize(
            picture,
            ((width as f32 * scale).round() as u32).clamp(1, DETECT_SIDE),
            ((height as f32 * scale).round() as u32).clamp(1, DETECT_SIDE),
            image::imageops::FilterType::Triangle,
        );
        let side = DETECT_SIDE as usize;
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, side, side));
        for (x, y, pixel) in small.enumerate_pixels() {
            // The model wants blue, green, red, as values from 0 to 255.
            for channel in 0..3 {
                input[[0, channel, y as usize, x as usize]] = f32::from(pixel[2 - channel]);
            }
        }
        let input: Tensor = input.into();
        let outputs = self.detector.run(tvec!(input.into()))?;

        // Twelve outputs: for each of three grid sizes (cells of 8, 16 and
        // 32 pixels), "is it a face", "is something there", the box, and
        // the five points. Every grid cell proposes one face.
        let mut found = Vec::new();
        for (level, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let class = outputs[level].to_plain_array_view::<f32>()?;
            let object = outputs[3 + level].to_plain_array_view::<f32>()?;
            let boxes = outputs[6 + level].to_plain_array_view::<f32>()?;
            let points = outputs[9 + level].to_plain_array_view::<f32>()?;
            let columns = side / stride;
            let cell = stride as f32;
            for index in 0..columns * columns {
                let score = (class[[0, index, 0]].clamp(0.0, 1.0)
                    * object[[0, index, 0]].clamp(0.0, 1.0))
                .sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let column = (index % columns) as f32;
                let row = (index / columns) as f32;
                let center_x = (column + boxes[[0, index, 0]]) * cell;
                let center_y = (row + boxes[[0, index, 1]]) * cell;
                let box_width = boxes[[0, index, 2]].exp() * cell;
                let box_height = boxes[[0, index, 3]].exp() * cell;
                let mut face_points = [(0.0, 0.0); 5];
                for (n, point) in face_points.iter_mut().enumerate() {
                    *point = (
                        (points[[0, index, 2 * n]] + column) * cell / scale,
                        (points[[0, index, 2 * n + 1]] + row) * cell / scale,
                    );
                }
                found.push(Detection {
                    bounds: [
                        (center_x - box_width / 2.0) / scale,
                        (center_y - box_height / 2.0) / scale,
                        (center_x + box_width / 2.0) / scale,
                        (center_y + box_height / 2.0) / scale,
                    ],
                    score,
                    points: face_points,
                });
            }
        }

        // Neighbouring cells find the same face: keep the surest of each.
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Detection> = Vec::new();
        for candidate in found {
            if kept
                .iter()
                .all(|other| overlap(&other.bounds, &candidate.bounds) <= SAME_FACE_OVERLAP)
            {
                kept.push(candidate);
            }
        }
        Ok(kept)
    }

    fn embed(&self, upright_face: &RgbImage) -> TractResult<Vec<f32>> {
        let side = FACE_SIDE as usize;
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, side, side));
        for (x, y, pixel) in upright_face.enumerate_pixels() {
            // This model wants red, green, blue, as values from 0 to 255.
            for channel in 0..3 {
                input[[0, channel, y as usize, x as usize]] = f32::from(pixel[channel]);
            }
        }
        let input: Tensor = input.into();
        let outputs = self.recognizer.run(tvec!(input.into()))?;
        let mut embedding: Vec<f32> = outputs[0]
            .to_plain_array_view::<f32>()?
            .iter()
            .copied()
            .collect();
        // Length 1, so that comparing two faces is a plain dot product.
        let length = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        if length > 0.0 {
            for value in &mut embedding {
                *value /= length;
            }
        }
        Ok(embedding)
    }
}

/// How much of two boxes' combined area they share (0 to 1).
fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let width = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let height = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let shared = width * height;
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    let total = area(a) + area(b) - shared;
    if total > 0.0 {
        shared / total
    } else {
        0.0
    }
}

/// Cuts the face out of the picture, turned and scaled so that its eyes,
/// nose and mouth land where the recognizer expects them.
fn align(picture: &RgbImage, points: &[(f32, f32); 5]) -> RgbImage {
    // Find the turn-and-scale (a, b) and shift (tx, ty) that best move the
    // reference points onto the face's points (least squares):
    //   x = a*u - b*v + tx,  y = b*u + a*v + ty
    let mean = |values: &dyn Fn(usize) -> f32| (0..5).map(values).sum::<f32>() / 5.0;
    let (mean_u, mean_v) = (
        mean(&|i| REFERENCE_POINTS[i].0),
        mean(&|i| REFERENCE_POINTS[i].1),
    );
    let (mean_x, mean_y) = (mean(&|i| points[i].0), mean(&|i| points[i].1));
    let (mut dot, mut cross, mut spread) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..5 {
        let (u, v) = (
            REFERENCE_POINTS[i].0 - mean_u,
            REFERENCE_POINTS[i].1 - mean_v,
        );
        let (x, y) = (points[i].0 - mean_x, points[i].1 - mean_y);
        dot += u * x + v * y;
        cross += u * y - v * x;
        spread += u * u + v * v;
    }
    let (a, b) = (dot / spread, cross / spread);
    let tx = mean_x - (a * mean_u - b * mean_v);
    let ty = mean_y - (b * mean_u + a * mean_v);

    let (width, height) = picture.dimensions();
    RgbImage::from_fn(FACE_SIDE, FACE_SIDE, |u, v| {
        let (u, v) = (u as f32, v as f32);
        sample(
            picture,
            (a * u - b * v + tx).clamp(0.0, (width - 1) as f32),
            (b * u + a * v + ty).clamp(0.0, (height - 1) as f32),
        )
    })
}

/// The color at a position between pixels (bilinear interpolation).
fn sample(picture: &RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let x1 = (x0 + 1).min(picture.width() - 1);
    let y1 = (y0 + 1).min(picture.height() - 1);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0u8; 3];
    for (channel, value) in out.iter_mut().enumerate() {
        let at = |px, py| f32::from(picture.get_pixel(px, py)[channel]);
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        *value = (top * (1.0 - fy) + bottom * fy).round() as u8;
    }
    image::Rgb(out)
}

/// How alike two faces are: 1 for identical, around 0 for unrelated people.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the fingerprints to put in `DETECTOR` and `RECOGNIZER` when
    /// the model files change: `PHOTOLIB_DATA=<dir> cargo test model_hashes -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn model_hashes() {
        let data = std::env::var("PHOTOLIB_DATA").expect("set PHOTOLIB_DATA");
        for model in [&DETECTOR, &RECOGNIZER] {
            let bytes = fs::read(models_dir(Path::new(&data)).join(model.name)).unwrap();
            println!(
                "{} {} {}",
                model.name,
                bytes.len(),
                blake3::hash(&bytes).to_hex()
            );
        }
    }

    #[test]
    fn overlaps() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert_eq!(overlap(&a, &a), 1.0);
        assert_eq!(overlap(&a, &[20.0, 20.0, 30.0, 30.0]), 0.0);
        // Half of each box is shared: 50 of 150.
        assert!((overlap(&a, &[5.0, 0.0, 15.0, 10.0]) - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn alignment_puts_the_face_points_in_place() {
        // A picture with a white dot at each face point of a face that is
        // twice as large as the reference, shifted, and turned 90 degrees.
        let turn = |(u, v): (f32, f32)| (300.0 - 2.0 * v, 40.0 + 2.0 * u);
        let points = REFERENCE_POINTS.map(turn);
        let mut picture = RgbImage::new(400, 400);
        for (x, y) in points {
            for dx in -3i32..=3 {
                for dy in -3i32..=3 {
                    let (px, py) = ((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                    picture.put_pixel(px, py, image::Rgb([255, 255, 255]));
                }
            }
        }
        let upright = align(&picture, &points);
        assert_eq!(upright.dimensions(), (FACE_SIDE, FACE_SIDE));
        for (u, v) in REFERENCE_POINTS {
            let pixel = upright.get_pixel(u.round() as u32, v.round() as u32);
            assert!(pixel[0] > 200, "no dot at {u}, {v}");
        }
        assert!(upright.get_pixel(5, 5)[0] < 50);
    }

    /// Checks the real models on real photos. Download the models first
    /// (switch face grouping on in the app, or copy the two .onnx files to
    /// `<dir>/models/`), then run:
    /// `PHOTOLIB_FACES=<dir of jpgs> PHOTOLIB_DATA=<dir> cargo test real_faces -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_faces() {
        let pictures = std::env::var("PHOTOLIB_FACES").expect("set PHOTOLIB_FACES");
        let data = std::env::var("PHOTOLIB_DATA").expect("set PHOTOLIB_DATA");
        let finder = FaceFinder::load(Path::new(&data)).unwrap();
        let mut all: Vec<(String, Face)> = Vec::new();
        let mut names: Vec<PathBuf> = fs::read_dir(pictures)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        for path in names {
            let Ok(picture) = image::open(&path) else {
                continue;
            };
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let started = std::time::Instant::now();
            let faces = finder.find(&picture.into_rgb8()).unwrap();
            println!("{name}: {} faces in {:?}", faces.len(), started.elapsed());
            for (n, face) in faces.into_iter().enumerate() {
                println!("  #{n} score {:.2} at {:.2?}", face.score, face.bounds);
                all.push((format!("{name}#{n}"), face));
            }
        }
        println!();
        for (i, (name_a, a)) in all.iter().enumerate() {
            for (name_b, b) in &all[i + 1..] {
                let alike = similarity(&a.embedding, &b.embedding);
                let mark = if alike > 0.363 { "SAME" } else { "" };
                println!("{name_a:>14} ~ {name_b:<14} {alike:+.3} {mark}");
            }
        }
    }
}
