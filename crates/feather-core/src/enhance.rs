//! Page clean-up on full-resolution pixels: straighten and crop to the paper,
//! even out shading, make the paper white, and make text darker and crisper.

use crate::error::{Error, Result};
use crate::imaging::PageGeometry;

pub struct Rgb {
    pub w: usize,
    pub h: usize,
    /// Interleaved RGB.
    pub px: Vec<u8>,
}

pub fn decode(jpeg: &[u8]) -> Result<Rgb> {
    let mut d = jpeg_decoder::Decoder::new(jpeg);
    let px = d.decode().map_err(|e| Error::Protocol(format!("JPEG decode: {e}")))?;
    let info = d.info().ok_or_else(|| Error::Protocol("bad JPEG".into()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&v| [v, v, v]).collect(),
        f => return Err(Error::Protocol(format!("unsupported JPEG pixel format {f:?}"))),
    };
    Ok(Rgb { w, h, px })
}

pub fn encode(img: &Rgb, dpi: u16) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(img.w * img.h / 4);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, 90);
    // No chroma subsampling: keeps colored text sharp.
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    enc.set_density(jpeg_encoder::Density::Inch { x: dpi, y: dpi });
    enc.encode(&img.px, img.w as u16, img.h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| Error::Protocol(format!("JPEG encode: {e}")))?;
    Ok(out)
}

/// Resamples the paper area upright: rotates by the detected skew and crops.
/// Pixels outside the scan come out white.
pub fn straighten_crop(src: &Rgb, g: &PageGeometry) -> Rgb {
    let w = g.paper_w.round().max(1.0) as usize;
    let h = g.paper_h.round().max(1.0) as usize;
    let (cos, sin) = (g.skew.cos(), g.skew.sin());
    let mut px = vec![255u8; w * h * 3];
    let (sw, sh) = (src.w as isize, src.h as isize);
    for v in 0..h {
        let ry = v as f64 + 0.5 - h as f64 / 2.0;
        for u in 0..w {
            let rx = u as f64 + 0.5 - w as f64 / 2.0;
            // Inverse of the forward rotation used for the page transform.
            let sx = g.cx + rx * cos + ry * sin - 0.5;
            let sy = g.cy - rx * sin + ry * cos - 0.5;
            let (x0, y0) = (sx.floor() as isize, sy.floor() as isize);
            if x0 < 0 || y0 < 0 || x0 + 1 >= sw || y0 + 1 >= sh {
                continue;
            }
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let i00 = (y0 as usize * src.w + x0 as usize) * 3;
            let i01 = i00 + 3;
            let i10 = i00 + src.w * 3;
            let i11 = i10 + 3;
            let o = (v * w + u) * 3;
            for c in 0..3 {
                let top = src.px[i00 + c] as f64 * (1.0 - fx) + src.px[i01 + c] as f64 * fx;
                let bot = src.px[i10 + c] as f64 * (1.0 - fx) + src.px[i11 + c] as f64 * fx;
                px[o + c] = (top * (1.0 - fy) + bot * fy + 0.5) as u8;
            }
        }
    }
    Rgb { w, h, px }
}

pub fn rotate180(img: &mut Rgb) {
    let n = img.w * img.h;
    for i in 0..n / 2 {
        let j = n - 1 - i;
        for c in 0..3 {
            img.px.swap(i * 3 + c, j * 3 + c);
        }
    }
}

/// ScanSnap-style clean-up for documents.
#[allow(clippy::needless_range_loop)]
pub fn clean(img: &mut Rgb) {
    let (w, h) = (img.w, img.h);
    if w < 64 || h < 64 {
        return;
    }
    let lum = |p: &[u8]| (p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8;

    // 1. Local paper brightness on a coarse grid (90th percentile per cell),
    //    smoothed, so uneven lighting and shadows get evened out.
    const CELL: usize = 64;
    let (gw, gh) = (w.div_ceil(CELL), h.div_ceil(CELL));
    let mut grid = vec![0f32; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut hist = [0u32; 256];
            let mut n = 0u32;
            for y in (gy * CELL..((gy + 1) * CELL).min(h)).step_by(2) {
                for x in (gx * CELL..((gx + 1) * CELL).min(w)).step_by(2) {
                    let i = (y * w + x) * 3;
                    hist[lum(&img.px[i..i + 3]) as usize] += 1;
                    n += 1;
                }
            }
            grid[gy * gw + gx] = hist_percentile(&hist, n, 0.9) as f32;
        }
    }
    // Paper level of the whole page; cells far below it are content (photos,
    // filled boxes), not shading, so they borrow the page level instead.
    let mut all: Vec<f32> = grid.clone();
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let paper = all[all.len() * 3 / 4].max(1.0);
    for v in grid.iter_mut() {
        if *v < paper * 0.82 {
            *v = paper;
        }
    }
    let grid = blur_grid(&blur_grid(&grid, gw, gh), gw, gh);

    // 2. Per-channel white point, so a tinted paper turns white.
    let mut hists = [[0u32; 256]; 3];
    let mut n = 0u32;
    for y in (0..h).step_by(3) {
        for x in (0..w).step_by(3) {
            let i = (y * w + x) * 3;
            for c in 0..3 {
                hists[c][img.px[i + c] as usize] += 1;
            }
            n += 1;
        }
    }
    let white: [f32; 3] = std::array::from_fn(|c| (hist_percentile(&hists[c], n, 0.9) as f32).max(64.0));

    // 3. Tone curve: paper and faint specks → white, ink a little darker.
    let black = 24.0f32;
    let lut: [[u8; 256]; 3] = std::array::from_fn(|c| {
        std::array::from_fn(|v| {
            let x = ((v as f32 - black) / (white[c] - black)).clamp(0.0, 1.0);
            let x = if x >= 0.92 { 1.0 } else { (x / 0.92).powf(1.35) };
            (x * 255.0 + 0.5) as u8
        })
    });

    for y in 0..h {
        let gyf = (y as f32 + 0.5) / CELL as f32 - 0.5;
        for x in 0..w {
            let gxf = (x as f32 + 0.5) / CELL as f32 - 0.5;
            let bg = sample_grid(&grid, gw, gh, gxf, gyf);
            // Only lift shading; never darken.
            let gain = (paper / bg).clamp(1.0, 1.25);
            let i = (y * w + x) * 3;
            for c in 0..3 {
                let v = (img.px[i + c] as f32 * gain).min(255.0) as usize;
                img.px[i + c] = lut[c][v];
            }
        }
    }

    // 4. Light sharpening (unsharp mask, 3×3).
    sharpen(img, 0.6);
}

fn hist_percentile(hist: &[u32; 256], n: u32, p: f64) -> u8 {
    let target = (n as f64 * p) as u32;
    let mut acc = 0;
    for (v, c) in hist.iter().enumerate() {
        acc += c;
        if acc > target {
            return v as u8;
        }
    }
    255
}

fn blur_grid(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; g.len()];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut n) = (0.0, 0.0);
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    s += g[yy * w + xx];
                    n += 1.0;
                }
            }
            out[y * w + x] = s / n;
        }
    }
    out
}

fn sample_grid(g: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = g[y0 * w + x0] * (1.0 - fx) + g[y0 * w + x1] * fx;
    let bot = g[y1 * w + x0] * (1.0 - fx) + g[y1 * w + x1] * fx;
    top * (1.0 - fy) + bot * fy
}

fn sharpen(img: &mut Rgb, amount: f32) {
    let (w, h) = (img.w, img.h);
    let src = img.px.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = (y * w + x) * 3;
            for c in 0..3 {
                let mut s = 0u32;
                for dy in [-1isize, 0, 1] {
                    let row = ((y as isize + dy) as usize * w) * 3;
                    s += src[row + (x - 1) * 3 + c] as u32 + src[row + x * 3 + c] as u32 + src[row + (x + 1) * 3 + c] as u32;
                }
                let blur = s as f32 / 9.0;
                let v = src[i + c] as f32;
                img.px[i + c] = (v + amount * (v - blur)).clamp(0.0, 255.0) as u8;
            }
        }
    }
}
