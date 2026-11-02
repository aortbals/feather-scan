//! Scanned images → analysis → clean-up + OCR → searchable PDF with a smart name → output folder.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::Config;
use crate::enhance;
use crate::error::Result;
use crate::imaging::{self, Analysis, PageGeometry};
use crate::ocr::{self, OcrEngine, OcrPage};
use crate::pdf::{self, Metadata, PdfPage};
use crate::time::local_now;

pub struct ScannedImage {
    pub jpeg: Vec<u8>,
    pub dpi: f64,
    /// Back side of a sheet (candidate for blank removal).
    pub back: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Timings {
    pub analyze_ms: u64,
    /// Clean-up and OCR, all pages (runs in parallel).
    pub pages_ms: u64,
    /// Summed over pages: decode, crop, clean-up and encode.
    pub prep_ms: u64,
    /// Summed over pages: text recognition.
    pub ocr_ms: u64,
    /// Slowest single page's text recognition.
    pub ocr_max_ms: u64,
    pub title_ms: u64,
    pub write_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Saved {
    pub path: PathBuf,
    pub title: String,
    pub pages: usize,
    pub removed_blank: usize,
    /// Set when the smart title could not be generated.
    pub title_error: Option<String>,
    pub timings: Timings,
}

struct Processed {
    jpeg: Vec<u8>,
    geometry: PageGeometry,
    ocr: OcrPage,
    blank: bool,
    prep_ms: u64,
    ocr_ms: u64,
}

/// Runs the current thread at user-initiated priority (macOS), so a scan the
/// user is waiting on isn't treated as background work.
pub fn prioritize_current_thread() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
    }
}

pub fn process(images: Vec<ScannedImage>, cfg: &Config, engine: &dyn OcrEngine) -> Result<Saved> {
    prioritize_current_thread();
    let started = Instant::now();
    let mut timings = Timings::default();

    // 1. Find paper edges and blank pages (fast, on a small decode).
    let mut analyses: Vec<Analysis> = images
        .iter()
        .map(|img| imaging::analyze(&img.jpeg, img.dpi))
        .collect::<Result<_>>()?;
    // Front and back are the same sheet: if edges were found on only one side, reuse them.
    for i in 0..analyses.len().saturating_sub(1) {
        if images[i].back || !images[i + 1].back {
            continue;
        }
        let (a, b) = (analyses[i].geometry, analyses[i + 1].geometry);
        if (a.img_w, a.img_h) != (b.img_w, b.img_h) {
            continue;
        }
        if a.detected && !b.detected {
            analyses[i + 1].geometry = a.mirrored();
        } else if b.detected && !a.detected {
            analyses[i].geometry = b.mirrored();
        }
    }
    timings.analyze_ms = started.elapsed().as_millis() as u64;

    // 2. Clean up and recognize each page, in parallel.
    let t = Instant::now();
    let n = images.len();
    let workers = std::env::var("FEATHER_OCR_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2usize)
        .max(1);
    let mut slots: Vec<Option<Result<Processed>>> = (0..n).map(|_| None).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (images_ref, analyses_ref) = (&images, &analyses);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers.min(n.max(1)))
            .map(|_| {
                s.spawn(|| {
                    prioritize_current_thread();
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        done.push((i, process_one(&images_ref[i], &analyses_ref[i], cfg, engine)));
                    }
                    done
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().unwrap_or_default() {
                slots[i] = Some(r);
            }
        }
    });
    let mut pages = Vec::with_capacity(n);
    for s in slots {
        pages.push(s.expect("every image processed")?);
    }
    timings.pages_ms = t.elapsed().as_millis() as u64;
    timings.prep_ms = pages.iter().map(|p| p.prep_ms).sum();
    timings.ocr_ms = pages.iter().map(|p| p.ocr_ms).sum();
    timings.ocr_max_ms = pages.iter().map(|p| p.ocr_ms).max().unwrap_or(0);

    let mut removed = 0;
    if cfg.remove_blank_pages && pages.iter().any(|p| !p.blank) {
        let before = pages.len();
        let backs: Vec<bool> = images.iter().map(|i| i.back).collect();
        let mut i = 0;
        pages.retain(|p| {
            let keep = !(p.blank && backs[i] && p.ocr.text().trim().len() < 8);
            i += 1;
            keep
        });
        removed = before - pages.len();
    }

    // 3. Name it.
    let t = Instant::now();
    let now = local_now();
    let today = now.date();
    let text: String = pages.iter().map(|p| p.ocr.text()).collect::<Vec<_>>().join("\n\n");
    let fallback = format!("{} Scan", now.stamp());
    let (title, title_error) = if cfg.llm.enabled {
        match crate::title::generate(&cfg.llm, &text, &today) {
            Ok(t) => (t, None),
            Err(e) => (fallback, Some(e.to_string())),
        }
    } else {
        (fallback, None)
    };
    timings.title_ms = t.elapsed().as_millis() as u64;

    // 4. Write it.
    let t = Instant::now();
    let pdf_pages: Vec<PdfPage> =
        pages.into_iter().map(|p| PdfPage { jpeg: p.jpeg, geometry: p.geometry, ocr: p.ocr }).collect();
    let bytes = pdf::write_pdf(&pdf_pages, &Metadata { title: Some(title.clone()), created: now.pdf_date() });
    std::fs::create_dir_all(&cfg.output_dir)?;
    let path = unique_path(&cfg.output_dir, &title);
    let tmp = path.with_extension("pdf.part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    timings.write_ms = t.elapsed().as_millis() as u64;
    timings.total_ms = started.elapsed().as_millis() as u64;
    Ok(Saved { path, title, pages: pdf_pages.len(), removed_blank: removed, title_error, timings })
}

fn process_one(img: &ScannedImage, analysis: &Analysis, cfg: &Config, engine: &dyn OcrEngine) -> Result<Processed> {
    let skip_ocr = analysis.blank && img.back;
    if !cfg.clean_pages {
        // Keep the scanner's JPEG untouched; crop and deskew happen in the PDF.
        let t = Instant::now();
        let ocr = if skip_ocr { OcrPage::default() } else { ocr::recognize_oriented(engine, &img.jpeg)? };
        return Ok(Processed {
            jpeg: img.jpeg.clone(),
            geometry: analysis.geometry,
            ocr,
            blank: analysis.blank,
            prep_ms: 0,
            ocr_ms: t.elapsed().as_millis() as u64,
        });
    }
    // Straighten and crop the pixels, clean them up, and OCR the result.
    let debug = std::env::var_os("FEATHER_DEBUG").is_some();
    let t = Instant::now();
    let src = enhance::decode(&img.jpeg)?;
    let t_decode = t.elapsed();
    let mut page = enhance::straighten_crop(&src, &analysis.geometry);
    drop(src);
    let t_crop = t.elapsed();
    enhance::clean(&mut page);
    let t_clean = t.elapsed();
    let dpi = img.dpi.round() as u16;
    let mut jpeg = enhance::encode(&page, dpi)?;
    let t_encode = t.elapsed();
    let mut ocr = if skip_ocr { OcrPage::default() } else { ocr::recognize_oriented(engine, &jpeg)? };
    if debug {
        eprintln!(
            "page: decode {:?} crop {:?} clean {:?} encode {:?} ocr {:?} upside_down={}",
            t_decode,
            t_crop - t_decode,
            t_clean - t_crop,
            t_encode - t_clean,
            t.elapsed() - t_encode,
            ocr.upside_down
        );
    }
    if ocr.upside_down {
        enhance::rotate180(&mut page);
        jpeg = enhance::encode(&page, dpi)?;
        ocr.flip_180();
    }
    Ok(Processed {
        jpeg,
        geometry: PageGeometry::full(page.w as f64, page.h as f64, img.dpi),
        ocr,
        blank: analysis.blank,
        prep_ms: t_encode.as_millis() as u64,
        ocr_ms: (t.elapsed() - t_encode).as_millis() as u64,
    })
}

fn unique_path(dir: &Path, title: &str) -> PathBuf {
    let first = dir.join(format!("{title}.pdf"));
    if !first.exists() {
        return first;
    }
    (2..).map(|i| dir.join(format!("{title} ({i}).pdf"))).find(|p| !p.exists()).unwrap()
}
