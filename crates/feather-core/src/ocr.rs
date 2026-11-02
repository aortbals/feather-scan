//! Text recognition. On macOS this is Apple's Vision framework, called from Rust.

use crate::error::Result;

/// A recognized word. Box is normalized to the image as OCR saw it
/// (0..1, origin top-left, y down).
#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Default)]
pub struct OcrPage {
    pub lines: Vec<Vec<Word>>,
    /// The page was recognized rotated 180°.
    pub upside_down: bool,
    pub confidence: f64,
}

impl OcrPage {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn score(&self) -> f64 {
        let chars: usize = self
            .lines
            .iter()
            .flatten()
            .map(|w| w.text.chars().count())
            .sum();
        chars as f64 * self.confidence
    }
}

pub trait OcrEngine: Send + Sync {
    /// Recognizes text in a JPEG. `upside_down` asks the engine to treat the image as rotated 180°.
    fn recognize(&self, jpeg: &[u8], upside_down: bool) -> Result<OcrPage>;
}

/// Runs OCR upright, and again upside down if the first pass found little.
pub fn recognize_oriented(engine: &dyn OcrEngine, jpeg: &[u8]) -> Result<OcrPage> {
    let up = engine.recognize(jpeg, false)?;
    if up.confidence >= 0.6 && up.score() > 40.0 {
        return Ok(up);
    }
    let down = engine.recognize(jpeg, true)?;
    Ok(if down.score() > up.score() * 1.5 {
        down
    } else {
        up
    })
}

/// Engine that finds nothing; used where no OCR is available.
pub struct NoOcr;

impl OcrEngine for NoOcr {
    fn recognize(&self, _: &[u8], _: bool) -> Result<OcrPage> {
        Ok(OcrPage::default())
    }
}

pub fn default_engine() -> Box<dyn OcrEngine> {
    #[cfg(target_os = "macos")]
    {
        Box::new(vision::VisionOcr)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoOcr)
    }
}

#[cfg(target_os = "macos")]
mod vision {
    use objc2::AnyThread;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSString};
    use objc2_image_io::CGImagePropertyOrientation;
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };

    use super::{OcrEngine, OcrPage, Word};
    use crate::error::{Error, Result};

    pub struct VisionOcr;

    impl OcrEngine for VisionOcr {
        fn recognize(&self, jpeg: &[u8], upside_down: bool) -> Result<OcrPage> {
            autoreleasepool(|_| recognize(jpeg, upside_down))
        }
    }

    fn recognize(jpeg: &[u8], upside_down: bool) -> Result<OcrPage> {
        let data = NSData::with_bytes(jpeg);
        let orientation = if upside_down {
            CGImagePropertyOrientation::Down
        } else {
            CGImagePropertyOrientation::Up
        };
        let options = NSDictionary::new();
        let handler = unsafe {
            VNImageRequestHandler::initWithData_orientation_options(
                VNImageRequestHandler::alloc(),
                &data,
                orientation,
                &options,
            )
        };
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        request.setAutomaticallyDetectsLanguage(true);
        let as_request: Retained<VNRequest> =
            Retained::into_super(Retained::into_super(request.clone()));
        let requests: Retained<NSArray<VNRequest>> = NSArray::from_retained_slice(&[as_request]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| Error::Ocr(e.localizedDescription().to_string()))?;

        let mut page = OcrPage {
            upside_down,
            ..Default::default()
        };
        let Some(results) = request.results() else {
            return Ok(page);
        };
        let (mut conf_sum, mut conf_n) = (0.0, 0usize);
        for obs in results.iter() {
            let cands = obs.topCandidates(1);
            let Some(cand) = cands.firstObject() else {
                continue;
            };
            let s: Retained<NSString> = cand.string();
            let text = s.to_string();
            conf_sum += cand.confidence() as f64;
            conf_n += 1;
            let mut line = Vec::new();
            // Word ranges in UTF-16 units, as NSString counts them.
            let utf16: Vec<u16> = text.encode_utf16().collect();
            let mut i = 0;
            while i < utf16.len() {
                if is_space(utf16[i]) {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < utf16.len() && !is_space(utf16[i]) {
                    i += 1;
                }
                let word = String::from_utf16_lossy(&utf16[start..i]);
                let bbox =
                    match unsafe { cand.boundingBoxForRange_error(NSRange::new(start, i - start)) }
                    {
                        Ok(o) => unsafe { o.boundingBox() },
                        Err(_) => unsafe { obs.boundingBox() },
                    };
                line.push(Word {
                    text: word,
                    x: bbox.origin.x,
                    y: 1.0 - bbox.origin.y - bbox.size.height,
                    w: bbox.size.width,
                    h: bbox.size.height,
                });
            }
            if !line.is_empty() {
                page.lines.push(line);
            }
        }
        page.confidence = if conf_n > 0 {
            conf_sum / conf_n as f64
        } else {
            0.0
        };
        Ok(page)
    }

    fn is_space(c: u16) -> bool {
        c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d || c == 0xa0 || c == 0x3000
    }
}
