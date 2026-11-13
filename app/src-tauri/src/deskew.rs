//! Straightens a scanned or photographed page before it goes to the OMR engine.
//!
//! Audiveris copes with a page that is a degree or so off, but a photo taken 2–3° askew made it read a quarter of the notes
//! (measured on a 12-measure page: 100 % straight, 93 % at 1.2°, 2 % at 2.5°). The tilt is found from the staff lines: the
//! dark pixels are projected on the vertical axis for a range of angles and the angle with the sharpest profile (every
//! staff line in its own few rows) wins. Only when that angle is clearly better than "no rotation" and at least
//! [`MIN_ANGLE`] is the image rotated, so a straight page is passed on untouched.

use image::{GrayImage, Luma};
use std::path::{Path, PathBuf};

/// Tilts smaller than this (degrees) are left alone.
const MIN_ANGLE: f32 = 0.4;
/// Largest tilt searched (degrees, either way).
const MAX_ANGLE: f32 = 8.0;
/// The best angle must give a profile at least this much sharper than the unrotated one.
const MIN_GAIN: f64 = 1.15;

/// Tilt of the page in degrees, positive when the staff lines run downwards to the right (clockwise), or `None` when the
/// image has no clear horizontal lines or is already straight enough.
pub fn estimate_skew(gray: &GrayImage) -> Option<f32> {
    // work on a reduced copy: block means with a whole factor, at most ~1200 px wide
    let f = ((gray.width() as f32 / 1200.0).ceil() as u32).max(1);
    let (w, h) = ((gray.width() / f) as usize, (gray.height() / f) as usize);
    if w < 50 || h < 50 {
        return None;
    }
    let mut small = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    s += gray.get_pixel(x as u32 * f + dx, y as u32 * f + dy).0[0] as u32;
                }
            }
            small[y * w + x] = s as f32 / (f * f) as f32;
        }
    }
    // dark = clearly darker than the local mean (an integral image keeps it cheap and ignores uneven lighting)
    let mut integral = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += small[y * w + x] as f64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let r = 12usize;
    let mut dark: Vec<(f32, f32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let (x0, x1, y0, y1) = (x.saturating_sub(r), (x + r + 1).min(w), y.saturating_sub(r), (y + r + 1).min(h));
            let sum = integral[y1 * (w + 1) + x1] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0] + integral[y0 * (w + 1) + x0];
            let mean = sum / ((x1 - x0) * (y1 - y0)) as f64;
            if (small[y * w + x] as f64) < mean - 28.0 {
                dark.push((x as f32, y as f32));
            }
        }
    }
    if dark.len() < 200 {
        return None;
    }
    let bins = h + w + 4;
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut hist = vec![0u32; bins];
        for &(x, y) in &dark {
            let yy = y * c - x * s + w as f32;
            hist[(yy as usize).min(bins - 1)] += 1;
        }
        hist.windows(2).map(|p| (p[1] as f64 - p[0] as f64).powi(2)).sum()
    };
    let (mut best, mut best_s) = (0f32, score(0.0));
    let base = best_s;
    let mut a = -MAX_ANGLE;
    while a <= MAX_ANGLE {
        let s = score(a);
        if s > best_s {
            best = a;
            best_s = s;
        }
        a += 0.2;
    }
    let mut a = best - 0.2;
    let end = best + 0.2;
    while a <= end {
        let s = score(a);
        if s > best_s {
            best = a;
            best_s = s;
        }
        a += 0.05;
    }
    (best.abs() >= MIN_ANGLE && best_s >= base * MIN_GAIN).then_some(best)
}

/// Rotates the content by `-deg` (so a page tilted by `deg` becomes straight), with bilinear sampling and a white
/// background. Same size as the input.
pub fn straighten(gray: &GrayImage, deg: f32) -> GrayImage {
    let (w, h) = (gray.width() as i64, gray.height() as i64);
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut out = GrayImage::from_pixel(w as u32, h as u32, Luma([255]));
    for yo in 0..h {
        for xo in 0..w {
            let (dx, dy) = (xo as f32 - cx, yo as f32 - cy);
            let (xi, yi) = (dx * c - dy * s + cx, dx * s + dy * c + cy);
            let (x0, y0) = (xi.floor(), yi.floor());
            if x0 < 0.0 || y0 < 0.0 || x0 as i64 + 1 >= w || y0 as i64 + 1 >= h {
                continue;
            }
            let (fx, fy) = (xi - x0, yi - y0);
            let p = |x: i64, y: i64| gray.get_pixel(x as u32, y as u32).0[0] as f32;
            let (x0i, y0i) = (x0 as i64, y0 as i64);
            let top = p(x0i, y0i) * (1.0 - fx) + p(x0i + 1, y0i) * fx;
            let bot = p(x0i, y0i + 1) * (1.0 - fx) + p(x0i + 1, y0i + 1) * fx;
            out.put_pixel(xo as u32, yo as u32, Luma([(top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8]));
        }
    }
    out
}

/// If `input` is a PNG or JPEG page that is tilted, writes a straightened copy into `dir` and returns its path and the
/// tilt found. `None` for PDFs, other formats, unreadable files and pages that are already straight.
pub fn deskew_file(input: &Path, dir: &Path) -> Option<(PathBuf, f32)> {
    let ext = input.extension()?.to_string_lossy().to_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        return None;
    }
    let gray = image::open(input).ok()?.to_luma8();
    let angle = estimate_skew(&gray)?;
    let fixed = straighten(&gray, angle);
    let stem = input.file_stem()?.to_string_lossy().into_owned();
    let path = dir.join(format!("{stem}_enderezada.png"));
    fixed.save(&path).ok()?;
    Some((path, angle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page with staves of five thick lines and some noteheads, straight.
    fn page() -> GrayImage {
        let mut img = GrayImage::from_pixel(1600, 2200, Luma([245]));
        for k in 0..6u32 {
            let top = 220 + k * 330;
            for l in 0..5u32 {
                for t in 0..3u32 {
                    for x in 120..1480 {
                        img.put_pixel(x, top + l * 14 + t, Luma([20]));
                    }
                }
            }
            for n in 0..14u32 {
                let (x, y) = (180 + n * 90, top + (n * 7 % 5) * 7);
                for dy in 0..11u32 {
                    for dx in 0..15u32 {
                        img.put_pixel(x + dx, y + dy, Luma([15]));
                    }
                }
            }
        }
        img
    }

    #[test]
    fn a_tilted_page_is_measured_and_straightened() {
        let clean = page();
        assert_eq!(estimate_skew(&clean), None, "a straight page is left alone");
        for tilt in [2.5f32, -3.0, 1.2] {
            let skewed = straighten(&clean, -tilt); // content turned by +tilt
            let found = estimate_skew(&skewed).unwrap_or_else(|| panic!("tilt {tilt} not found"));
            assert!((found - tilt).abs() < 0.3, "tilt {tilt}: found {found}");
            let fixed = straighten(&skewed, found);
            assert!(estimate_skew(&fixed).map_or(true, |a| a.abs() < 0.45), "still tilted after straightening {tilt}");
        }
    }

    #[test]
    fn only_tilted_png_and_jpeg_files_are_rewritten() {
        let dir = std::env::temp_dir().join(format!("novascore-deskew-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (clean, tilted) = (dir.join("recta.png"), dir.join("girada.png"));
        page().save(&clean).unwrap();
        straighten(&page(), -2.5).save(&tilted).unwrap();
        assert!(deskew_file(&clean, &dir).is_none());
        let (out, angle) = deskew_file(&tilted, &dir).expect("tilted page is straightened");
        assert!((angle - 2.5).abs() < 0.3 && out.is_file(), "angle {angle}");
        assert!(deskew_file(&dir.join("x.pdf"), &dir).is_none(), "a PDF is never touched");
        assert!(deskew_file(&dir.join("no_existe.png"), &dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
