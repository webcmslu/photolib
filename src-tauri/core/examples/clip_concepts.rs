//! Makes `data/concepts.bin` from `data/concepts.tsv`: for each concept, the
//! point CLIP's text model gives its description, so the app can find the
//! photos that show it without the text model itself (250 MB).
//!
//!     cargo run --release --example clip_concepts -- <folder>
//!
//! `<folder>` holds `text_model.onnx` and `tokenizer.json` from
//! https://huggingface.co/Xenova/clip-vit-base-patch32 (MIT), the same CLIP
//! as the image model the app downloads (see `clip.rs`).

use std::collections::HashMap;
use std::path::Path;

use tract_onnx::prelude::*;

/// Several ways of saying "a photo of …", averaged: CLIP finds the photos
/// better than with one (as in OpenAI's paper).
const TEMPLATES: &[&str] = &[
    "a photo of {}.",
    "a picture of {}.",
    "a close-up photo of {}.",
    "a family photo of {}.",
    "a blurry photo of {}.",
    "a photo of {}, taken on holiday.",
];

/// The number of tokens CLIP reads.
const LENGTH: usize = 77;

fn main() -> TractResult<()> {
    let folder = std::env::args()
        .nth(1)
        .expect("the folder with text_model.onnx and tokenizer.json");
    let folder = Path::new(&folder);
    let tokenizer = Tokenizer::load(&folder.join("tokenizer.json"));

    let model = tract_onnx::onnx().model_for_path(folder.join("text_model.onnx"))?;
    let batch = model.symbols.sym("batch_size");
    // CLIP's own length: tract can't work with a length left open.
    let model = model
        .with_input_fact(0, i64::fact(&[batch.to_dim(), LENGTH.to_dim()]).into())?
        .into_optimized()?;
    let embeds = model
        .output_outlets()?
        .iter()
        .position(|outlet| model.outlet_label(*outlet) == Some("text_embeds"))
        .expect("a text_embeds output");
    let model = model.into_runnable()?;

    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let concepts = std::fs::read_to_string(data.join("concepts.tsv"))?;
    let mut out = Vec::new();
    let mut count = 0;
    for line in concepts.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let description = line.split('\t').nth(1).expect("a description");
        let mut sum = vec![0f32; phototek_core::clip::DIMENSIONS];
        for template in TEMPLATES {
            let mut ids = tokenizer.encode(&template.replace("{}", description));
            // Padded with end tokens: CLIP reads the point at the first one.
            let end = *ids.last().unwrap();
            ids.resize(LENGTH, end);
            let input = tract_ndarray::Array2::from_shape_vec((1, LENGTH), ids)?.into_tensor();
            let result = model.run(tvec!(input.into()))?;
            let point = normalized(result[embeds].to_plain_array_view::<f32>()?.iter().copied());
            for (total, value) in sum.iter_mut().zip(point) {
                *total += value;
            }
        }
        for value in normalized(sum.into_iter()) {
            out.extend_from_slice(&value.to_le_bytes());
        }
        count += 1;
    }
    std::fs::write(data.join("concepts.bin"), out)?;
    println!("{count} concepts written to data/concepts.bin");
    Ok(())
}

fn normalized(values: impl Iterator<Item = f32>) -> Vec<f32> {
    let values: Vec<f32> = values.collect();
    let length = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    values.into_iter().map(|v| v / length).collect()
}

/// CLIP's byte-pair tokenizer, enough for plain English descriptions.
struct Tokenizer {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
}

impl Tokenizer {
    fn load(path: &Path) -> Self {
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("tokenizer.json")).unwrap();
        let model = &json["model"];
        let vocab = model["vocab"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(token, id)| (token.clone(), id.as_i64().unwrap()))
            .collect();
        let ranks = model["merges"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(rank, merge)| {
                // "a b" in older files, ["a", "b"] in newer ones.
                let pair = match merge {
                    serde_json::Value::String(text) => {
                        let (a, b) = text.split_once(' ').unwrap();
                        (a.to_owned(), b.to_owned())
                    }
                    other => (
                        other[0].as_str().unwrap().to_owned(),
                        other[1].as_str().unwrap().to_owned(),
                    ),
                };
                (pair, rank)
            })
            .collect();
        Self { vocab, ranks }
    }

    /// The token ids, between the start and end tokens.
    fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = vec![self.vocab["<|startoftext|>"]];
        for word in words(&text.to_lowercase()) {
            for token in self.merge(&word) {
                ids.push(self.vocab[&token]);
            }
        }
        ids.push(self.vocab["<|endoftext|>"]);
        ids
    }

    /// Splits a word into letters, the last one marked as the end of the
    /// word, then merges the pairs in the order the tokenizer learned them.
    fn merge(&self, word: &str) -> Vec<String> {
        let letters: Vec<char> = word.chars().collect();
        let mut parts: Vec<String> = letters.iter().map(|c| c.to_string()).collect();
        if let Some(last) = parts.last_mut() {
            last.push_str("</w>");
        }
        loop {
            let best = (0..parts.len().saturating_sub(1))
                .filter_map(|i| {
                    self.ranks
                        .get(&(parts[i].clone(), parts[i + 1].clone()))
                        .map(|rank| (*rank, i))
                })
                .min();
            let Some((_, i)) = best else { break };
            let joined = format!("{}{}", parts[i], parts[i + 1]);
            parts.splice(i..i + 2, [joined]);
        }
        parts
    }
}

/// Runs of letters, single digits, and runs of other signs, as CLIP's
/// pre-tokenizer splits them (ASCII is enough for the descriptions).
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut kind = 0u8;
    for c in text.chars() {
        let this = if c.is_alphabetic() {
            1
        } else if c.is_numeric() {
            2
        } else if c.is_whitespace() {
            0
        } else {
            3
        };
        if (this != kind || this == 2 || this == 0) && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        if this != 0 {
            current.push(c);
        }
        kind = this;
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}
