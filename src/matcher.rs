//! Template matching equivalent to OpenCV's `TM_CCOEFF_NORMED`
//! (zero-mean normalized cross-correlation), with a coarse-to-fine search
//! so that full-screen scans stay fast without OpenCV.

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::Path;
use xcap::image::RgbaImage;

/// Grayscale image stored as `f32` luma values (0..=255).
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn from_rgba(img: &RgbaImage) -> Self {
        let (w, h) = img.dimensions();
        let px = img
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        Self { w: w as usize, h: h as usize, px }
    }

    /// Box-filter downscale by an integer factor.
    pub fn downscale(&self, f: usize) -> Self {
        if f <= 1 {
            return self.clone();
        }
        let (w, h) = (self.w / f, self.h / f);
        let inv = 1.0 / (f * f) as f32;
        let mut px = vec![0.0; w * h];
        px.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut acc = 0.0;
                for dy in 0..f {
                    let start = (y * f + dy) * self.w + x * f;
                    acc += self.px[start..start + f].iter().sum::<f32>();
                }
                *out = acc * inv;
            }
        });
        Self { w, h, px }
    }
}

/// Template with its mean removed, ready for correlation.
struct Prepared {
    w: usize,
    h: usize,
    n: f64,
    vals: Vec<f32>,
    norm: f64,
}

impl Prepared {
    fn new(g: &Gray) -> Self {
        let n = g.px.len() as f64;
        let mean = g.px.iter().map(|&v| v as f64).sum::<f64>() / n;
        let vals: Vec<f32> = g.px.iter().map(|&v| (v as f64 - mean) as f32).collect();
        let norm = vals.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>().sqrt();
        Self { w: g.w, h: g.h, n, vals, norm }
    }
}

/// Summed-area tables for window sum and sum of squares.
struct Integral {
    stride: usize,
    s: Vec<f64>,
    sq: Vec<f64>,
}

impl Integral {
    fn new(g: &Gray) -> Self {
        let stride = g.w + 1;
        let mut s = vec![0.0; stride * (g.h + 1)];
        let mut sq = vec![0.0; stride * (g.h + 1)];
        for y in 0..g.h {
            let (mut rs, mut rsq) = (0.0, 0.0);
            for x in 0..g.w {
                let v = g.px[y * g.w + x] as f64;
                rs += v;
                rsq += v * v;
                let i = (y + 1) * stride + x + 1;
                s[i] = s[i - stride] + rs;
                sq[i] = sq[i - stride] + rsq;
            }
        }
        Self { stride, s, sq }
    }

    fn rect(&self, x: usize, y: usize, w: usize, h: usize) -> (f64, f64) {
        let (a, b) = (y * self.stride + x, y * self.stride + x + w);
        let (c, d) = ((y + h) * self.stride + x, (y + h) * self.stride + x + w);
        (
            self.s[d] - self.s[b] - self.s[c] + self.s[a],
            self.sq[d] - self.sq[b] - self.sq[c] + self.sq[a],
        )
    }
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut total: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        total += x * y;
    }
    total
}

#[inline]
fn correlation(img: &Gray, t: &Prepared, x: usize, y: usize) -> f64 {
    let mut num = 0.0f64;
    for j in 0..t.h {
        let start = (y + j) * img.w + x;
        num += dot(&img.px[start..start + t.w], &t.vals[j * t.w..(j + 1) * t.w]) as f64;
    }
    num
}

#[inline]
fn finish_score(num: f64, sum: f64, sumsq: f64, t: &Prepared) -> f32 {
    let var = sumsq - sum * sum / t.n;
    if var <= 1e-6 * t.n || t.norm <= 1e-9 {
        return 0.0;
    }
    (num / (t.norm * var.sqrt())) as f32
}

/// Score using precomputed integral images (used for the coarse pass).
fn score_integral(img: &Gray, integ: &Integral, t: &Prepared, x: usize, y: usize) -> f32 {
    let (s, sq) = integ.rect(x, y, t.w, t.h);
    finish_score(correlation(img, t, x, y), s, sq, t)
}

/// Score computing window statistics directly (used for the fine pass).
fn score_direct(img: &Gray, t: &Prepared, x: usize, y: usize) -> f32 {
    let (mut s, mut sq) = (0.0f64, 0.0f64);
    for j in 0..t.h {
        let start = (y + j) * img.w + x;
        for &v in &img.px[start..start + t.w] {
            s += v as f64;
            sq += (v as f64) * (v as f64);
        }
    }
    finish_score(correlation(img, t, x, y), s, sq, t)
}

/// Best match inside an inclusive rectangle of top-left positions.
fn search_region(img: &Gray, t: &Prepared, x0: usize, x1: usize, y0: usize, y1: usize) -> (usize, usize, f32) {
    (y0..=y1)
        .into_par_iter()
        .map(|y| {
            (x0..=x1)
                .map(|x| (x, y, score_direct(img, t, x, y)))
                .fold((0, 0, f32::NEG_INFINITY), |a, b| if b.2 > a.2 { b } else { a })
        })
        .reduce(|| (0, 0, f32::NEG_INFINITY), |a, b| if b.2 > a.2 { b } else { a })
}

pub struct Match {
    /// Top-left corner, in pixels of the searched image.
    pub x: usize,
    pub y: usize,
    pub score: f32,
    /// Contrast of the matched region relative to the template (std-dev
    /// ratio). NCC ignores uniform dimming, so a button behind a dark modal
    /// overlay still scores high; this lets callers reject it.
    pub contrast: f32,
}

/// Standard deviation of the region at (x, y) divided by the template's.
fn contrast_ratio(img: &Gray, t: &Prepared, x: usize, y: usize) -> f32 {
    if t.norm <= 1e-9 {
        return 0.0;
    }
    let (mut s, mut sq) = (0.0f64, 0.0f64);
    for j in 0..t.h {
        let start = (y + j) * img.w + x;
        for &v in &img.px[start..start + t.w] {
            s += v as f64;
            sq += (v as f64) * (v as f64);
        }
    }
    let var = (sq - s * s / t.n).max(0.0);
    (var.sqrt() / t.norm) as f32
}

pub struct Template {
    pub name: String,
    pub width: usize,
    pub height: usize,
    full: Prepared,
    coarse: Option<(usize, Prepared)>,
}

const COARSE_CANDIDATES: usize = 5;

impl Template {
    pub fn load(path: &Path) -> Result<Self> {
        let img = xcap::image::open(path)
            .with_context(|| format!("could not open {}", path.display()))?
            .to_rgba8();
        let gray = Gray::from_rgba(&img);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Downscale so the smallest template side stays around 12px or more.
        let factor = (gray.w.min(gray.h) / 12).clamp(1, 4);
        let coarse = (factor > 1).then(|| (factor, Prepared::new(&gray.downscale(factor))));
        Ok(Self {
            name,
            width: gray.w,
            height: gray.h,
            full: Prepared::new(&gray),
            coarse,
        })
    }

    /// Finds the best match on `screen`. Returns `None` if the template does
    /// not fit inside the screen.
    pub fn find(&self, screen: &Gray) -> Option<Match> {
        let (x, y, score) = self.find_position(screen)?;
        let contrast = contrast_ratio(screen, &self.full, x, y);
        Some(Match { x, y, score, contrast })
    }

    fn find_position(&self, screen: &Gray) -> Option<(usize, usize, f32)> {
        let t = &self.full;
        if t.w > screen.w || t.h > screen.h {
            return None;
        }
        let (max_x, max_y) = (screen.w - t.w, screen.h - t.h);

        let Some((f, ct)) = &self.coarse else {
            return Some(search_region(screen, t, 0, max_x, 0, max_y));
        };
        let f = *f;
        let small = screen.downscale(f);
        if ct.w > small.w || ct.h > small.h {
            return Some(search_region(screen, t, 0, max_x, 0, max_y));
        }

        // Coarse pass over the whole (downscaled) screen.
        let integ = Integral::new(&small);
        let (ow, oh) = (small.w - ct.w + 1, small.h - ct.h + 1);
        let mut scores = vec![0.0f32; ow * oh];
        scores.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                *out = score_integral(&small, &integ, ct, x, y);
            }
        });

        // Pick the strongest peaks with non-maximum suppression.
        let mut candidates = Vec::with_capacity(COARSE_CANDIDATES);
        for _ in 0..COARSE_CANDIDATES {
            let Some((idx, &best)) = scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
            else {
                break;
            };
            if best == f32::NEG_INFINITY {
                break;
            }
            let (cx, cy) = (idx % ow, idx / ow);
            candidates.push((cx, cy));
            let (rx, ry) = ((ct.w / 2).max(1), (ct.h / 2).max(1));
            for y in cy.saturating_sub(ry)..(cy + ry).min(oh) {
                for x in cx.saturating_sub(rx)..(cx + rx).min(ow) {
                    scores[y * ow + x] = f32::NEG_INFINITY;
                }
            }
        }

        // Fine pass at full resolution around each candidate.
        candidates
            .into_iter()
            .map(|(cx, cy)| {
                let (fx, fy) = (cx * f, cy * f);
                let r = 2 * f;
                search_region(
                    screen,
                    t,
                    fx.saturating_sub(r),
                    (fx + r).min(max_x),
                    fy.saturating_sub(r),
                    (fy + r).min(max_y),
                )
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: usize, h: usize, seed: u32) -> Gray {
        let mut state = seed;
        let px = (0..w * h)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((state >> 16) % 256) as f32
            })
            .collect();
        Gray { w, h, px }
    }

    /// Pastes the real Nexus buttons onto a live screenshot and finds them.
    /// Needs a desktop session: `cargo test --release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn finds_buttons_on_real_screenshot() {
        let monitor = xcap::Monitor::all().unwrap().remove(0);
        for (name, (px, py)) in [("1_button.png", (701, 413)), ("2_button.png", (1203, 87))] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("input_images").join(name);
            let mut shot = monitor.capture_image().unwrap();
            let button = xcap::image::open(&path).unwrap().to_rgba8();
            xcap::image::imageops::overlay(&mut shot, &button, px, py);
            let t = Template::load(&path).unwrap();
            let gray = Gray::from_rgba(&shot);
            let start = std::time::Instant::now();
            let m = t.find(&gray).unwrap();
            println!(
                "{name}: screen {}x{}, found at ({}, {}) score {:.3} in {:?}",
                gray.w, gray.h, m.x, m.y, m.score, start.elapsed()
            );
            assert_eq!((m.x as i64, m.y as i64), (px, py));
            assert!(m.score > 0.95);
        }
    }

    #[test]
    fn finds_embedded_template() {
        let mut screen = pattern(800, 600, 1);
        // A smooth-ish template so the coarse pass has structure to latch onto.
        let tw = 120;
        let th = 40;
        let mut tpl = Gray { w: tw, h: th, px: vec![0.0; tw * th] };
        for y in 0..th {
            for x in 0..tw {
                let v = if (x / 10 + y / 10) % 2 == 0 { 230.0 } else { 20.0 };
                tpl.px[y * tw + x] = v;
                screen.px[(y + 333) * screen.w + x + 517] = v;
            }
        }
        let factor = (tw.min(th) / 12).clamp(1, 4);
        let t = Template {
            name: "t".into(),
            width: tw,
            height: th,
            full: Prepared::new(&tpl),
            coarse: Some((factor, Prepared::new(&tpl.downscale(factor)))),
        };
        let m = t.find(&screen).unwrap();
        assert_eq!((m.x, m.y), (517, 333));
        assert!(m.score > 0.99, "score {}", m.score);
        assert!((m.contrast - 1.0).abs() < 0.01, "contrast {}", m.contrast);

        // Same screen behind a 50% black overlay: still matches, half contrast.
        for v in &mut screen.px {
            *v *= 0.5;
        }
        let m = t.find(&screen).unwrap();
        assert_eq!((m.x, m.y), (517, 333));
        assert!(m.score > 0.99, "score {}", m.score);
        assert!((m.contrast - 0.5).abs() < 0.01, "contrast {}", m.contrast);
    }

    /// Prints matches on a screenshot file:
    /// `SHOT=path cargo test --release shot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn shot() {
        let shot = xcap::image::open(std::env::var("SHOT").unwrap()).unwrap().to_rgba8();
        let gray = Gray::from_rgba(&shot);
        for name in ["1_button.png", "2_button.png"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("input_images")
                .join(name);
            if let Some(m) = Template::load(&path).unwrap().find(&gray) {
                println!(
                    "{name}: ({}, {}) score {:.3} contrast {:.3}",
                    m.x, m.y, m.score, m.contrast
                );
            }
        }
    }
}
