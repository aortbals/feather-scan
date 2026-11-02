//! Scanned images → analysis + OCR → searchable PDF with a smart name → output folder.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::Result;
use crate::imaging::{self, PageGeometry};
use crate::ocr::{self, OcrEngine, OcrPage};
use crate::pdf::{self, Metadata, PdfPage};
use crate::time::local_now;

pub struct ScannedImage {
    pub jpeg: Vec<u8>,
    pub dpi: f64,
    /// Back side of a sheet (candidate for blank removal).
    pub back: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Saved {
    pub path: PathBuf,
    pub title: String,
    pub pages: usize,
    pub removed_blank: usize,
    /// Set when the smart title could not be generated.
    pub title_error: Option<String>,
}

struct Processed {
    jpeg: Vec<u8>,
    geometry: PageGeometry,
    ocr: OcrPage,
    blank: bool,
}

pub fn process(images: Vec<ScannedImage>, cfg: &Config, engine: &dyn OcrEngine) -> Result<Saved> {
    let n = images.len();
    let workers = std::env::var("FEATHER_OCR_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2usize)
        .max(1);
    let mut slots: Vec<Option<Result<Processed>>> = (0..n).map(|_| None).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let images_ref = &images;
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers.min(n.max(1)))
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        done.push((i, process_one(&images_ref[i], engine)));
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
    // Front and back are the same sheet: if edges were found on only one side, reuse them.
    for i in 0..pages.len().saturating_sub(1) {
        if images[i].back || !images[i + 1].back {
            continue;
        }
        let (a, b) = (pages[i].geometry, pages[i + 1].geometry);
        if a.detected && !b.detected && (a.img_w, a.img_h) == (b.img_w, b.img_h) {
            pages[i + 1].geometry = a.mirrored();
        } else if b.detected && !a.detected && (a.img_w, a.img_h) == (b.img_w, b.img_h) {
            pages[i].geometry = b.mirrored();
        }
    }

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

    let now = local_now();
    let today = now.date();
    let text: String = pages
        .iter()
        .map(|p| p.ocr.text())
        .collect::<Vec<_>>()
        .join("\n\n");
    let fallback = format!("{} Scan", now.stamp());
    let (title, title_error) = if cfg.llm.enabled {
        match crate::title::generate(&cfg.llm, &text, &today) {
            Ok(t) => (t, None),
            Err(e) => (fallback, Some(e.to_string())),
        }
    } else {
        (fallback, None)
    };

    let pdf_pages: Vec<PdfPage> = pages
        .into_iter()
        .map(|p| PdfPage {
            jpeg: p.jpeg,
            geometry: p.geometry,
            ocr: p.ocr,
        })
        .collect();
    let bytes = pdf::write_pdf(
        &pdf_pages,
        &Metadata {
            title: Some(title.clone()),
            created: now.pdf_date(),
        },
    );
    std::fs::create_dir_all(&cfg.output_dir)?;
    let path = unique_path(&cfg.output_dir, &title);
    let tmp = path.with_extension("pdf.part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Saved {
        path,
        title,
        pages: pdf_pages.len(),
        removed_blank: removed,
        title_error,
    })
}

fn process_one(img: &ScannedImage, engine: &dyn OcrEngine) -> Result<Processed> {
    let analysis = imaging::analyze(&img.jpeg, img.dpi)?;
    let ocr = if analysis.blank && img.back {
        OcrPage::default()
    } else {
        ocr::recognize_oriented(engine, &img.jpeg)?
    };
    Ok(Processed {
        jpeg: img.jpeg.clone(),
        geometry: analysis.geometry,
        ocr,
        blank: analysis.blank,
    })
}

fn unique_path(dir: &Path, title: &str) -> PathBuf {
    let first = dir.join(format!("{title}.pdf"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|i| dir.join(format!("{title} ({i}).pdf")))
        .find(|p| !p.exists())
        .unwrap()
}
