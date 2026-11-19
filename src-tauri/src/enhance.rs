//! "Enhance": the magic wand. Looks at a photo and picks a few ordinary
//! corrections for it, the way Apple Photos' Auto Enhance does:
//!
//! - **levels:** the darkest and lightest 0.5 % become black and white, so a
//!   dull or hazy photo gets its full range back;
//! - **white balance:** a colour cast seen on the nearly grey parts of the
//!   photo is taken out (only part of it, so sunsets stay warm);
//! - **midtones:** a too dark or too bright photo is brought part of the way
//!   towards a normal brightness;
//! - **contrast:** a gentle S-curve for flat photos;
//! - **vibrance:** more colour for dull photos.
//!
//! The result is a handful of numbers ([`Enhance`]) stored in the photo's
//! edit; the file is not changed. The same curves are drawn by the app (an SVG
//! filter, see `enhanceTables` in src/lib/edit.ts) and by [`apply`] on export,
//! so the formulas here and there must stay the same.

use image::RgbImage;

use crate::org::Enhance;

/// Enough pixels for steady statistics; more only costs time.
const SAMPLES: u32 = 200_000;
/// Share of the darkest and lightest pixels allowed to become pure black or white.
const CLIP: f64 = 0.005;

/// The corrections the wand picks for `picture` (any size; a preview is plenty).
pub fn analyze(picture: &RgbImage) -> Enhance {
    let (width, height) = picture.dimensions();
    let step = ((u64::from(width) * u64::from(height) / u64::from(SAMPLES)) as f64)
        .sqrt()
        .max(1.0) as u32;

    let mut luma_hist = [0u64; 256];
    let mut count = 0u64;
    // Sums over the nearly grey pixels, to find a colour cast.
    let mut grey_sum = [0f64; 3];
    let mut grey_count = 0u64;
    let mut saturation_sum = 0f64;
    for y in (0..height).step_by(step as usize) {
        for x in (0..width).step_by(step as usize) {
            let [r, g, b] = picture.get_pixel(x, y).0.map(f64::from);
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            luma_hist[luma.round() as usize] += 1;
            count += 1;
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
            saturation_sum += saturation;
            // Grey enough to show a cast, neither too dark nor blown out.
            if saturation < 0.2 && (40.0..=240.0).contains(&luma) {
                grey_sum[0] += r;
                grey_sum[1] += g;
                grey_sum[2] += b;
                grey_count += 1;
            }
        }
    }
    if count == 0 {
        return Enhance::IDENTITY;
    }

    // Levels: stretch most of the way to the clipped extremes.
    let low = percentile(&luma_hist, count, CLIP);
    let high = percentile(&luma_hist, count, 1.0 - CLIP);
    let mut black = (low * 0.8).min(60.0);
    let mut white = (255.0 - (255.0 - high) * 0.8).max(170.0);
    if white - black < 120.0 {
        // A nearly flat picture (fog, a blank wall): leave its range alone.
        black = 0.0;
        white = 255.0;
    }

    // White balance: per-channel gains that make the greys grey, at half
    // strength and at most 8 %.
    let mut gains = [1.0f64; 3];
    if grey_count * 20 >= count {
        let means = grey_sum.map(|sum| sum / grey_count as f64);
        let grey = (means[0] + means[1] + means[2]) / 3.0;
        gains = means.map(|mean| {
            let full = if mean > 0.0 { grey / mean } else { 1.0 };
            (1.0 + (full - 1.0) * 0.5).clamp(0.92, 1.08)
        });
    }

    // Midtones: move the median half way towards 0.45.
    let median = ((percentile(&luma_hist, count, 0.5) - black) / (white - black)).clamp(0.02, 0.98);
    let target = median + (0.45 - median) * 0.5;
    let gamma = (target.ln() / median.ln()).clamp(0.6, 1.5);

    // Contrast: an S-curve for flat photos, judged after the steps above.
    let spread = spread_after(&luma_hist, count, black, white, gamma);
    let contrast = ((0.24 - spread) * 2.5).clamp(0.0, 0.35);

    // Vibrance: more colour for dull photos, none for black and white.
    let mean_saturation = saturation_sum / count as f64;
    let vibrance = if mean_saturation < 0.04 {
        0.0
    } else {
        ((0.32 - mean_saturation) * 1.0).clamp(0.0, 0.25)
    };

    // A channel with a gain g reaches black and white g times earlier.
    let thousandths = |value: f64| (value / 255.0 * 1000.0).round().clamp(0.0, 2000.0) as u16;
    Enhance {
        black: gains.map(|gain| thousandths(black / gain)),
        white: gains.map(|gain| thousandths(white / gain)),
        gamma: (gamma * 100.0).round() as u16,
        contrast: (contrast * 100.0).round() as u8,
        vibrance: (vibrance * 100.0).round() as u8,
    }
}

/// The value below which `share` of the pixels are.
fn percentile(hist: &[u64; 256], count: u64, share: f64) -> f64 {
    let wanted = (count as f64 * share).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (value, &n) in hist.iter().enumerate() {
        seen += n;
        if seen >= wanted {
            return value as f64;
        }
    }
    255.0
}

/// The standard deviation of the brightness once levels and midtones are applied.
fn spread_after(hist: &[u64; 256], count: u64, black: f64, white: f64, gamma: f64) -> f64 {
    let mapped = |value: usize| {
        ((value as f64 - black) / (white - black))
            .clamp(0.0, 1.0)
            .powf(gamma)
    };
    let mean = hist
        .iter()
        .enumerate()
        .map(|(v, &n)| mapped(v) * n as f64)
        .sum::<f64>()
        / count as f64;
    let variance = hist
        .iter()
        .enumerate()
        .map(|(v, &n)| (mapped(v) - mean).powi(2) * n as f64)
        .sum::<f64>()
        / count as f64;
    variance.sqrt()
}

/// The curve of `channel` (0 red, 1 green, 2 blue) at `x` from 0 to 1.
/// Must match `enhanceCurve` in src/lib/edit.ts.
pub fn curve(enhance: &Enhance, channel: usize, x: f64) -> f64 {
    let black = f64::from(enhance.black[channel]) / 1000.0;
    let white = f64::from(enhance.white[channel]) / 1000.0;
    let y = ((x - black) / (white - black)).clamp(0.0, 1.0);
    let y = y.powf(f64::from(enhance.gamma) / 100.0);
    let s = if y < 0.5 {
        2.0 * y * y
    } else {
        1.0 - 2.0 * (1.0 - y) * (1.0 - y)
    };
    y + f64::from(enhance.contrast) / 100.0 * (s - y)
}

/// Applies the corrections to `picture`, as the app draws them.
pub fn apply(picture: &mut RgbImage, enhance: &Enhance) {
    let tables: [[f32; 256]; 3] = std::array::from_fn(|channel| {
        std::array::from_fn(|v| curve(enhance, channel, v as f64 / 255.0) as f32)
    });
    let saturation = 1.0 + f32::from(enhance.vibrance) / 100.0;
    let matrix = saturate_matrix(saturation);
    for pixel in picture.pixels_mut() {
        let [r, g, b] = [0, 1, 2].map(|c| tables[c][usize::from(pixel.0[c])]);
        let out = if enhance.vibrance > 0 {
            matrix.map(|row| row[0] * r + row[1] * g + row[2] * b)
        } else {
            [r, g, b]
        };
        pixel.0 = out.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
}

/// The `saturate()` matrix of the Filter Effects spec (CSS and SVG).
pub fn saturate_matrix(s: f32) -> [[f32; 3]; 3] {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// A gradient from `from` to `to` (0-255 brightness), tinted by `tint`.
    fn gradient(from: f64, to: f64, tint: [f64; 3]) -> RgbImage {
        RgbImage::from_fn(200, 100, |x, _| {
            let v = from + (to - from) * f64::from(x) / 199.0;
            Rgb(tint.map(|t| (v * t).round().clamp(0.0, 255.0) as u8))
        })
    }

    #[test]
    fn a_dull_picture_gets_its_full_range() {
        let enhance = analyze(&gradient(60.0, 190.0, [1.0, 1.0, 1.0]));
        let mut picture = gradient(60.0, 190.0, [1.0, 1.0, 1.0]);
        apply(&mut picture, &enhance);
        let (lo, hi) = picture
            .pixels()
            .fold((255, 0), |(lo, hi), p| (lo.min(p.0[1]), hi.max(p.0[1])));
        assert!(lo < 30 && hi > 225, "range {lo}..{hi}");
    }

    #[test]
    fn a_good_picture_is_left_almost_alone() {
        let enhance = analyze(&gradient(0.0, 255.0, [1.0, 1.0, 1.0]));
        for x in [0.1, 0.3, 0.5, 0.7, 0.9] {
            let y = curve(&enhance, 1, x);
            assert!((y - x).abs() < 0.08, "{x} -> {y} ({enhance:?})");
        }
        assert_eq!(enhance.vibrance, 0);
    }

    #[test]
    fn a_blue_cast_is_reduced() {
        let tinted = gradient(30.0, 230.0, [0.9, 1.0, 1.1]);
        let enhance = analyze(&tinted);
        let mut picture = tinted.clone();
        apply(&mut picture, &enhance);
        let cast = |image: &RgbImage| {
            let p = image.get_pixel(100, 50).0;
            f64::from(p[2]) - f64::from(p[0])
        };
        assert!(
            cast(&picture) < cast(&tinted) * 0.8,
            "{} -> {}",
            cast(&tinted),
            cast(&picture)
        );
    }

    #[test]
    fn a_dark_picture_gets_brighter() {
        // Mostly dark, with a few bright pixels so levels alone don't fix it.
        let picture = RgbImage::from_fn(200, 100, |x, _| {
            let v = if x < 190 { 10 + x / 4 } else { 250 };
            Rgb([v as u8; 3])
        });
        let enhance = analyze(&picture);
        assert!(enhance.gamma < 100, "{enhance:?}");
    }

    #[test]
    fn the_identity_changes_nothing() {
        let mut picture = gradient(0.0, 255.0, [0.8, 1.0, 0.6]);
        let before = picture.clone();
        apply(&mut picture, &Enhance::IDENTITY);
        assert_eq!(picture, before);
    }
}

/// Statistics of the wand over a folder of cached previews, numbers only:
/// `PHOTOLIB_PREVIEWS=<dir> cargo test enhance_stats -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn enhance_stats() {
    let dir = std::env::var("PHOTOLIB_PREVIEWS").expect("PHOTOLIB_PREVIEWS");
    let files: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .step_by(37)
        .take(300)
        .collect();
    let mut rows = Vec::new();
    let start = std::time::Instant::now();
    for file in &files {
        let Ok(picture) = image::open(file.path()) else {
            continue;
        };
        let e = analyze(&picture.to_rgb8());
        // Average change of the green curve over the range, and the WB spread.
        let change = (1..20)
            .map(|i| (curve(&e, 1, f64::from(i) / 20.0) - f64::from(i) / 20.0).abs())
            .sum::<f64>()
            / 19.0;
        let wb = f64::from(e.white.iter().max().unwrap() - e.white.iter().min().unwrap()) / 10.0;
        rows.push((change, wb, e.gamma, e.contrast, e.vibrance));
    }
    let n = rows.len() as f64;
    let ms = start.elapsed().as_millis() as f64 / n;
    let mut changes: Vec<f64> = rows.iter().map(|r| r.0).collect();
    changes.sort_by(f64::total_cmp);
    let pct = |p: f64| changes[((n - 1.0) * p) as usize];
    println!("{n} previews, {ms:.1} ms each");
    println!(
        "mean curve change: median {:.3}, p90 {:.3}, max {:.3}",
        pct(0.5),
        pct(0.9),
        pct(1.0)
    );
    type Row = (f64, f64, u16, u8, u8);
    let mean = |f: &dyn Fn(&Row) -> f64| rows.iter().map(f).sum::<f64>() / n;
    println!(
        "avg white-balance spread {:.1}%, gamma {:.2}, contrast {:.0}%, vibrance {:.0}%",
        mean(&|r| r.1),
        mean(&|r| f64::from(r.2) / 100.0),
        mean(&|r| f64::from(r.3)),
        mean(&|r| f64::from(r.4))
    );
    println!(
        "with WB {}, with contrast {}, with vibrance {}",
        rows.iter().filter(|r| r.1 > 0.5).count(),
        rows.iter().filter(|r| r.3 > 0).count(),
        rows.iter().filter(|r| r.4 > 0).count()
    );
}
