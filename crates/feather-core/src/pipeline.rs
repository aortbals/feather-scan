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
    /// Waiting for page clean-up and OCR after the last sheet arrived
    /// (pages are processed while scanning continues).
    pub pages_ms: u64,
    /// Summed over pages: decode, crop, clean-up and encode.
    pub prep_ms: u64,
    /// Summed over pages: text recognition.
    pub ocr_ms: u64,
    /// Slowest single page's text recognition.
    pub ocr_max_ms: u64,
    pub title_ms: u64,
    pub write_ms: u64,
    /// From the first sheet to the saved PDF.
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

type Slot = (usize, Result<Processed>);

/// A scan in progress. Sheets are analyzed and their pages cleaned up and
/// recognized as they arrive, while the scanner feeds the next sheet;
/// `finish` then names the document and writes the PDF.
pub struct Batch {
    cfg: std::sync::Arc<Config>,
    jobs: Option<std::sync::mpsc::Sender<(usize, ScannedImage, Analysis)>>,
    results: std::sync::mpsc::Receiver<Slot>,
    workers: Vec<std::thread::JoinHandle<()>>,
    backs: Vec<bool>,
    analyze_ms: u64,
    error: Option<crate::error::Error>,
    started: Instant,
}

impl Batch {
    pub fn new(cfg: Config, engine: std::sync::Arc<dyn OcrEngine>) -> Batch {
        let cfg = std::sync::Arc::new(cfg);
        let (job_tx, job_rx) = std::sync::mpsc::channel::<(usize, ScannedImage, Analysis)>();
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let (res_tx, results) = std::sync::mpsc::channel::<Slot>();
        let n = std::env::var("FEATHER_OCR_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2usize).max(1);
        let workers = (0..n)
            .map(|_| {
                let (job_rx, res_tx, cfg, engine) = (job_rx.clone(), res_tx.clone(), cfg.clone(), engine.clone());
                std::thread::spawn(move || {
                    prioritize_current_thread();
                    loop {
                        let job = job_rx.lock().unwrap().recv();
                        let Ok((i, img, analysis)) = job else { break };
                        let r = process_one(&img, &analysis, &cfg, engine.as_ref());
                        if res_tx.send((i, r)).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        Batch {
            cfg,
            jobs: Some(job_tx),
            results,
            workers,
            backs: Vec::new(),
            analyze_ms: 0,
            error: None,
            started: Instant::now(),
        }
    }

    /// Queues one sheet. Cheap: analysis runs here, the heavy work on the workers.
    pub fn add_sheet(&mut self, front: ScannedImage, back: Option<ScannedImage>) {
        let t = Instant::now();
        let mut sides = vec![front];
        sides.extend(back);
        let analyses: Result<Vec<Analysis>> = sides.iter().map(|s| imaging::analyze(&s.jpeg, s.dpi)).collect();
        let mut analyses = match analyses {
            Ok(a) => a,
            Err(e) => {
                self.error.get_or_insert(e);
                return;
            }
        };
        // Front and back are the same sheet: if edges were found on only one side, reuse them.
        if let [a, b] = analyses.as_mut_slice()
            && (a.geometry.img_w, a.geometry.img_h) == (b.geometry.img_w, b.geometry.img_h) {
                if a.geometry.detected && !b.geometry.detected {
                    b.geometry = a.geometry.mirrored();
                } else if b.geometry.detected && !a.geometry.detected {
                    a.geometry = b.geometry.mirrored();
                }
            }
        self.analyze_ms += t.elapsed().as_millis() as u64;
        for (img, analysis) in sides.into_iter().zip(analyses) {
            let i = self.backs.len();
            self.backs.push(img.back);
            if let Some(tx) = &self.jobs {
                let _ = tx.send((i, img, analysis));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.backs.is_empty()
    }

    /// Waits for the remaining pages, names the document and writes the PDF.
    pub fn finish(mut self) -> Result<Saved> {
        prioritize_current_thread();
        let finishing = Instant::now();
        let mut timings = Timings { analyze_ms: self.analyze_ms, ..Default::default() };
        drop(self.jobs.take());
        let n = self.backs.len();
        let mut slots: Vec<Option<Result<Processed>>> = (0..n).map(|_| None).collect();
        for (i, r) in self.results.iter().take(n) {
            slots[i] = Some(r);
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        let mut pages = Vec::with_capacity(n);
        for s in slots {
            pages.push(s.ok_or_else(|| crate::error::Error::Protocol("a page was not processed".into()))??);
        }
        // Time spent waiting for pages after the last sheet arrived.
        timings.pages_ms = finishing.elapsed().as_millis() as u64;
        timings.prep_ms = pages.iter().map(|p| p.prep_ms).sum();
        timings.ocr_ms = pages.iter().map(|p| p.ocr_ms).sum();
        timings.ocr_max_ms = pages.iter().map(|p| p.ocr_ms).max().unwrap_or(0);
        let cfg = self.cfg.clone();
        let saved = write_document(pages, &self.backs, &cfg, timings, finishing);
        saved.map(|mut s| {
            s.timings.total_ms = self.started.elapsed().as_millis() as u64;
            s
        })
    }
}

/// Processes already-scanned images (fronts and backs in order) in one go.
pub fn process(images: Vec<ScannedImage>, cfg: &Config, engine: std::sync::Arc<dyn OcrEngine>) -> Result<Saved> {
    let mut batch = Batch::new(cfg.clone(), engine);
    let mut it = images.into_iter().peekable();
    while let Some(front) = it.next() {
        let back = if it.peek().is_some_and(|n| n.back) { it.next() } else { None };
        batch.add_sheet(front, back);
    }
    batch.finish()
}

fn write_document(
    mut pages: Vec<Processed>,
    backs: &[bool],
    cfg: &Config,
    mut timings: Timings,
    finishing: Instant,
) -> Result<Saved> {
    let mut removed = 0;
    if cfg.remove_blank_pages && pages.iter().any(|p| !p.blank) {
        let before = pages.len();
        let mut i = 0;
        pages.retain(|p| {
            let keep = !(p.blank && backs[i] && p.ocr.text().trim().len() < 8);
            i += 1;
            keep
        });
        removed = before - pages.len();
    }

    // Name it.
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

    // Write it.
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
    timings.total_ms = finishing.elapsed().as_millis() as u64;
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
