//! Page analysis on a downscaled decode: paper bounds, skew and blank detection.
//!
//! Nothing here changes pixels. The PDF writer crops and straightens with a
//! transform, so the scanner's JPEG goes into the PDF untouched.

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    /// Full image size in pixels.
    pub img_w: f64,
    pub img_h: f64,
    pub dpi: f64,
    /// Paper center in image pixels (y down).
    pub cx: f64,
    pub cy: f64,
    /// Paper size in image pixels after straightening.
    pub paper_w: f64,
    pub paper_h: f64,
    /// Skew in radians: positive when the left edge leans like "\" (top further left).
    pub skew: f64,
    /// Paper edges were found (otherwise this is the whole frame).
    pub detected: bool,
}

impl PageGeometry {
    pub fn full(img_w: f64, img_h: f64, dpi: f64) -> Self {
        Self {
            img_w,
            img_h,
            dpi,
            cx: img_w / 2.0,
            cy: img_h / 2.0,
            paper_w: img_w,
            paper_h: img_h,
            skew: 0.0,
            detected: false,
        }
    }

    /// The same sheet seen from the other side: mirrored left to right.
    pub fn mirrored(&self) -> Self {
        Self {
            cx: self.img_w - self.cx,
            skew: -self.skew,
            ..*self
        }
    }
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub geometry: PageGeometry,
    pub blank: bool,
    /// Share of paper pixels that are clearly darker than the paper.
    pub ink: f64,
}

struct Gray {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Gray {
    #[inline]
    fn at(&self, x: usize, y: usize) -> u8 {
        self.px[y * self.w + x]
    }
}

fn decode_small(jpeg: &[u8]) -> Result<(Gray, usize, usize)> {
    let mut d = jpeg_decoder::Decoder::new(jpeg);
    d.read_info()
        .map_err(|e| Error::Protocol(format!("bad JPEG: {e}")))?;
    let info = d.info().ok_or_else(|| Error::Protocol("bad JPEG".into()))?;
    let (full_w, full_h) = (info.width as usize, info.height as usize);
    // About 1/4 scale: plenty for edges, fast to decode.
    d.scale((full_w / 4) as u16, (full_h / 4) as u16)
        .map_err(|e| Error::Protocol(format!("JPEG scale: {e}")))?;
    let px = d
        .decode()
        .map_err(|e| Error::Protocol(format!("JPEG decode: {e}")))?;
    let info = d.info().unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let gray = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => px,
        jpeg_decoder::PixelFormat::RGB24 => px
            .as_chunks::<3>().0.iter()
            .map(|c| ((c[0] as u32 * 77 + c[1] as u32 * 150 + c[2] as u32 * 29) >> 8) as u8)
            .collect(),
        f => {
            return Err(Error::Protocol(format!(
                "unsupported JPEG pixel format {f:?}"
            )));
        }
    };
    Ok((Gray { w, h, px: gray }, full_w, full_h))
}

pub fn analyze(jpeg: &[u8], dpi: f64) -> Result<Analysis> {
    let (g, full_w, full_h) = decode_small(jpeg)?;
    let scale = full_w as f64 / g.w as f64;
    let mut geo = PageGeometry::full(full_w as f64, full_h as f64, dpi);
    let Some(edges) = find_edges(&g) else {
        let paper = percentile(
            &mut g
                .px
                .iter()
                .step_by(7)
                .map(|&v| v as f64)
                .collect::<Vec<_>>(),
            0.9,
        );
        let whole = Edges {
            left: 0.0,
            right: g.w as f64,
            top: 0.0,
            bottom: g.h as f64,
            skew: 0.0,
            threshold: (paper * 0.8) as u8,
            paper: paper as u8,
        };
        let ink = ink_ratio(&g, &whole);
        return Ok(Analysis {
            geometry: geo,
            blank: ink < 0.0015,
            ink,
        });
    };
    let cos = edges.skew.cos();
    geo.cx = (edges.left + edges.right) / 2.0 * scale;
    geo.cy = (edges.top + edges.bottom) / 2.0 * scale;
    geo.paper_w = (edges.right - edges.left) * cos * scale;
    geo.paper_h = (edges.bottom - edges.top) * cos * scale;
    geo.skew = edges.skew;
    geo.detected = true;
    // Trim the thin shadow line along the paper edge (~0.5 mm).
    let inset = dpi / 50.0;
    geo.paper_w -= 2.0 * inset;
    geo.paper_h -= 2.0 * inset;
    let ink = ink_ratio(&g, &edges);
    Ok(Analysis {
        geometry: geo,
        blank: ink < 0.0015,
        ink,
    })
}

struct Edges {
    /// Edge positions at the paper's middle, in small-image pixels.
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    skew: f64,
    threshold: u8,
    paper: u8,
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn find_edges(g: &Gray) -> Option<Edges> {
    let (w, h) = (g.w, g.h);
    if w < 32 || h < 32 {
        return None;
    }
    // Brightness profiles use a high percentile, so dense print doesn't hide
    // the white paper underneath.
    let col: Vec<f64> = (0..w)
        .map(|x| p90((h / 10..h * 9 / 10).map(|y| g.at(x, y))))
        .collect();
    // The iX1600 background is light gray (~232) next to white paper (~246), so
    // sample only the outermost columns, where there is never paper.
    let bg = {
        let mut sides = [col[0], col[1], col[w - 2], col[w - 1]];
        percentile(&mut sides, 0.5)
    };
    let paper = percentile(&mut col.clone(), 0.9);
    // Paper must be brighter than the background, or it fills the frame / is dark.
    if paper - bg < 7.0 {
        return None;
    }
    let thr = (bg + paper) / 2.0;
    let left = crossing(&col, thr, false)?;
    let right = crossing(&col, thr, true)?;
    if right - left < w as f64 * 0.15 {
        return None;
    }
    let (x0, x1) = (left as usize + 2, (right as usize).saturating_sub(2));
    let row: Vec<f64> = (0..h).map(|y| p90((x0..x1).map(|x| g.at(x, y)))).collect();
    let top = crossing(&row, thr, false).unwrap_or(0.0);
    let bottom = crossing(&row, thr, true).unwrap_or(h as f64);
    let skew = estimate_skew(g, thr as u8, top as usize, bottom as usize);
    Some(Edges {
        left,
        right,
        top,
        bottom,
        skew,
        threshold: thr as u8,
        paper: paper as u8,
    })
}

/// 90th percentile of a set of pixel values, via a histogram.
fn p90(values: impl Iterator<Item = u8>) -> f64 {
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for v in values {
        hist[v as usize] += 1;
        n += 1;
    }
    let target = (n as f64 * 0.9) as u32;
    let mut acc = 0;
    for (v, c) in hist.iter().enumerate() {
        acc += c;
        if acc > target {
            return v as f64;
        }
    }
    255.0
}

/// First (or last) position where the profile crosses `thr`, with sub-pixel interpolation.
fn crossing(p: &[f64], thr: f64, from_end: bool) -> Option<f64> {
    let n = p.len();
    let idx: Box<dyn Iterator<Item = usize>> = if from_end {
        Box::new((1..n).rev())
    } else {
        Box::new(1..n)
    };
    for i in idx {
        let (a, b) = if from_end {
            (p[i], p[i - 1])
        } else {
            (p[i - 1], p[i])
        };
        if a < thr && b >= thr {
            let t = (thr - a) / (b - a);
            return Some(if from_end {
                i as f64 - t
            } else {
                (i - 1) as f64 + t
            });
        }
    }
    None
}

/// Fits lines to the left and right paper edges; returns the skew angle.
fn estimate_skew(g: &Gray, thr: u8, top: usize, bottom: usize) -> f64 {
    let mut fits = Vec::new();
    for from_right in [false, true] {
        let mut pts: Vec<(f64, f64)> = Vec::new();
        let (y0, y1) = (top + (bottom - top) / 10, bottom - (bottom - top) / 10);
        for y in (y0..y1).step_by(4) {
            let mut run = 0;
            for i in 0..g.w {
                let x = if from_right { g.w - 1 - i } else { i };
                if g.at(x, y) >= thr {
                    run += 1;
                    if run == 3 {
                        let edge = if from_right { x + 2 } else { x - 2 };
                        pts.push((y as f64, edge as f64));
                        break;
                    }
                } else {
                    run = 0;
                }
            }
        }
        if let Some(slope) = robust_slope(&mut pts) {
            fits.push(slope);
        }
    }
    let slope = match fits.as_slice() {
        [a, b] if (a - b).abs() < 0.01 => (a + b) / 2.0,
        [a, b] => {
            if a.abs() < b.abs() {
                *a
            } else {
                *b
            }
        }
        [a] => *a,
        _ => 0.0,
    };
    let angle = slope.atan();
    // Ignore noise and implausible values.
    if angle.abs() < 0.0015 || angle.abs() > 0.09 {
        0.0
    } else {
        angle
    }
}

/// Least-squares slope dx/dy after dropping the worst residuals twice.
fn robust_slope(pts: &mut Vec<(f64, f64)>) -> Option<f64> {
    for _ in 0..3 {
        if pts.len() < 20 {
            return None;
        }
        let n = pts.len() as f64;
        let (sy, sx) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (my, mx) = (sy / n, sx / n);
        let (num, den) = pts.iter().fold((0.0, 0.0), |a, p| {
            (a.0 + (p.0 - my) * (p.1 - mx), a.1 + (p.0 - my).powi(2))
        });
        if den == 0.0 {
            return None;
        }
        let slope = num / den;
        let icpt = mx - slope * my;
        let mut res: Vec<f64> = pts
            .iter()
            .map(|p| (p.1 - (slope * p.0 + icpt)).abs())
            .collect();
        let cut = percentile(&mut res, 0.8).max(1.0);
        let before = pts.len();
        pts.retain(|p| (p.1 - (slope * p.0 + icpt)).abs() <= cut);
        if pts.len() == before {
            return Some(slope);
        }
    }
    let n = pts.len() as f64;
    let (sy, sx) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (my, mx) = (sy / n, sx / n);
    let (num, den) = pts.iter().fold((0.0, 0.0), |a, p| {
        (a.0 + (p.0 - my) * (p.1 - mx), a.1 + (p.0 - my).powi(2))
    });
    (den > 0.0).then(|| num / den)
}

/// Share of pixels inside the paper (minus a margin) that look like ink.
fn ink_ratio(g: &Gray, e: &Edges) -> f64 {
    let mx = (e.right - e.left) * 0.04;
    let my = (e.bottom - e.top) * 0.04;
    let (x0, x1) = ((e.left + mx) as usize, (e.right - mx) as usize);
    let (y0, y1) = ((e.top + my) as usize, (e.bottom - my) as usize);
    if x1 <= x0 || y1 <= y0 {
        return 1.0;
    }
    let ink_level = e.paper.saturating_sub(70).max(e.threshold / 2);
    let mut ink = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            if g.at(x, y) < ink_level {
                ink += 1;
            }
        }
    }
    ink as f64 / ((x1 - x0) * (y1 - y0)) as f64
}

/// Column and row brightness profiles, for debugging edge detection.
pub fn debug_profiles(jpeg: &[u8]) -> Result<(Vec<f64>, Vec<f64>)> {
    let (g, _, _) = decode_small(jpeg)?;
    let col = (0..g.w)
        .map(|x| (0..g.h).map(|y| g.at(x, y) as f64).sum::<f64>() / g.h as f64)
        .collect();
    let row = (0..g.h)
        .map(|y| (0..g.w).map(|x| g.at(x, y) as f64).sum::<f64>() / g.w as f64)
        .collect();
    Ok((col, row))
}
