//! PDF writer: JPEG pages passed through untouched (DCTDecode), cropped and
//! straightened with a transform, plus an invisible OCR text layer.

mod font;

use std::fmt::Write as _;

use miniz_oxide::deflate::compress_to_vec_zlib;

use crate::imaging::PageGeometry;
use crate::ocr::OcrPage;

pub struct PdfPage {
    pub jpeg: Vec<u8>,
    pub geometry: PageGeometry,
    pub ocr: OcrPage,
}

pub struct Metadata {
    pub title: Option<String>,
    pub created: String,
}

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
}

fn jpeg_info(d: &[u8]) -> Option<JpegInfo> {
    let mut i = 2;
    while i + 9 < d.len() {
        if d[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        if marker == 0xff || marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            return Some(JpegInfo {
                height: u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32,
                width: u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32,
                components: d[i + 9],
            });
        }
        i += 2 + len;
    }
    None
}

/// Maps image pixels (y down) to page points for one page.
struct PageTransform {
    page_w: f64,
    page_h: f64,
    cx: f64,
    cy: f64,
    cos: f64,
    sin: f64,
    scale: f64,
    flip: f64,
}

impl PageTransform {
    fn new(g: &PageGeometry, upside_down: bool) -> Self {
        let scale = 72.0 / g.dpi;
        Self {
            page_w: g.paper_w * scale,
            page_h: g.paper_h * scale,
            cx: g.cx,
            cy: g.cy,
            cos: g.skew.cos(),
            sin: g.skew.sin(),
            scale,
            flip: if upside_down { -1.0 } else { 1.0 },
        }
    }

    fn apply(&self, px: f64, py: f64) -> (f64, f64) {
        let (dx, dy) = (px - self.cx, py - self.cy);
        let rx = dx * self.cos - dy * self.sin;
        let ry = dx * self.sin + dy * self.cos;
        (
            self.page_w / 2.0 + self.flip * self.scale * rx,
            self.page_h / 2.0 - self.flip * self.scale * ry,
        )
    }
}

pub fn write_pdf(pages: &[PdfPage], meta: &Metadata) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = Vec::new(); // object n is objs[n-1]
    let reserve = |objs: &mut Vec<Vec<u8>>| {
        objs.push(Vec::new());
        objs.len()
    };
    let catalog = reserve(&mut objs);
    let pages_id = reserve(&mut objs);
    let info = reserve(&mut objs);
    let font0 = reserve(&mut objs);
    let cidfont = reserve(&mut objs);
    let descriptor = reserve(&mut objs);
    let fontfile = reserve(&mut objs);
    let cidmap = reserve(&mut objs);
    let tounicode = reserve(&mut objs);

    let mut kids = Vec::new();
    for p in pages {
        let page = reserve(&mut objs);
        let content = reserve(&mut objs);
        let image = reserve(&mut objs);
        kids.push(page);

        let ji = jpeg_info(&p.jpeg).unwrap_or(JpegInfo {
            width: p.geometry.img_w as u32,
            height: p.geometry.img_h as u32,
            components: 3,
        });
        let t = PageTransform::new(&p.geometry, p.ocr.upside_down);
        let (img_w, img_h) = (ji.width as f64, ji.height as f64);
        // Image space: unit square, origin bottom-left. Pixel (x, y) = (u·W, (1−v)·H).
        let o = t.apply(0.0, img_h);
        let ux = t.apply(img_w, img_h);
        let vy = t.apply(0.0, 0.0);
        let mut cs = String::new();
        let _ = writeln!(
            cs,
            "q {} {} {} {} {} {} cm /Im0 Do Q",
            f(ux.0 - o.0),
            f(ux.1 - o.1),
            f(vy.0 - o.0),
            f(vy.1 - o.1),
            f(o.0),
            f(o.1)
        );
        text_layer(&mut cs, &p.ocr, &t, img_w, img_h);
        let content_data = compress_to_vec_zlib(cs.as_bytes(), 6);

        objs[page - 1] = format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << /Im0 {image} 0 R >> /Font << /F0 {font0} 0 R >> >> /Contents {content} 0 R >>",
            f(t.page_w),
            f(t.page_h)
        )
        .into_bytes();
        objs[content - 1] = stream(
            &format!("/Filter /FlateDecode /Length {}", content_data.len()),
            &content_data,
        );
        let colorspace = if ji.components == 1 {
            "/DeviceGray"
        } else {
            "/DeviceRGB"
        };
        objs[image - 1] = stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {colorspace} /BitsPerComponent 8 /Filter /DCTDecode /Length {}",
                ji.width,
                ji.height,
                p.jpeg.len()
            ),
            &p.jpeg,
        );
    }

    objs[catalog - 1] = format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes();
    objs[pages_id - 1] = format!(
        "<< /Type /Pages /Count {} /Kids [{}] >>",
        kids.len(),
        kids.iter()
            .map(|k| format!("{k} 0 R"))
            .collect::<Vec<_>>()
            .join(" ")
    )
    .into_bytes();
    let mut info_dict = format!(
        "<< /Producer (Feather Scan) /Creator (Feather Scan) /CreationDate ({})",
        meta.created
    );
    if let Some(title) = &meta.title {
        let _ = write!(info_dict, " /Title {}", utf16_hex(title));
    }
    info_dict.push_str(" >>");
    objs[info - 1] = info_dict.into_bytes();

    objs[font0 - 1] = format!(
        "<< /Type /Font /Subtype /Type0 /BaseFont /GlyphLessFont /Encoding /Identity-H /DescendantFonts [{cidfont} 0 R] /ToUnicode {tounicode} 0 R >>"
    )
    .into_bytes();
    objs[cidfont - 1] = format!(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /GlyphLessFont /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {descriptor} 0 R /DW 500 /CIDToGIDMap {cidmap} 0 R >>"
    )
    .into_bytes();
    objs[descriptor - 1] = format!(
        "<< /Type /FontDescriptor /FontName /GlyphLessFont /Flags 5 /FontBBox [0 0 500 1000] /ItalicAngle 0 /Ascent 1000 /Descent -1 /CapHeight 1000 /StemV 80 /FontFile2 {fontfile} 0 R >>"
    )
    .into_bytes();
    let ttf = font::glyphless_ttf();
    let ttf_z = compress_to_vec_zlib(&ttf, 9);
    objs[fontfile - 1] = stream(
        &format!(
            "/Filter /FlateDecode /Length {} /Length1 {}",
            ttf_z.len(),
            ttf.len()
        ),
        &ttf_z,
    );
    // Every CID maps to glyph 1.
    let map: Vec<u8> = std::iter::repeat_n([0u8, 1u8], 65536).flatten().collect();
    let map_z = compress_to_vec_zlib(&map, 9);
    objs[cidmap - 1] = stream(
        &format!("/Filter /FlateDecode /Length {}", map_z.len()),
        &map_z,
    );
    let cmap = to_unicode_cmap();
    objs[tounicode - 1] = stream(&format!("/Length {}", cmap.len()), cmap.as_bytes());

    serialize(&objs, catalog, info)
}

/// Invisible text (render mode 3), one run per word, stretched to the word's box.
fn text_layer(cs: &mut String, ocr: &OcrPage, t: &PageTransform, img_w: f64, img_h: f64) {
    if ocr.lines.is_empty() {
        return;
    }
    // Normalized OCR coordinates → image pixels (OCR may have seen the image rotated 180°).
    let to_px = |x: f64, y: f64| {
        if ocr.upside_down {
            (img_w - x * img_w, img_h - y * img_h)
        } else {
            (x * img_w, y * img_h)
        }
    };
    cs.push_str("BT 3 Tr\n");
    for line in &ocr.lines {
        for (i, w) in line.iter().enumerate() {
            let (bl_x, bl_y) = to_px(w.x, w.y + w.h);
            let (br_x, br_y) = to_px(w.x + w.w, w.y + w.h);
            let bl = t.apply(bl_x, bl_y);
            let br = t.apply(br_x, br_y);
            let (dx, dy) = (br.0 - bl.0, br.1 - bl.1);
            let mut width = (dx * dx + dy * dy).sqrt();
            let size = w.h * img_h * t.scale;
            if width < 0.5 || size < 1.0 {
                continue;
            }
            let (cos, sin) = (dx / width, dy / width);
            let mut text = w.text.clone();
            // Trailing space up to the next word so extracted text keeps word breaks.
            if let Some(next) = line.get(i + 1) {
                let (nx, ny) = to_px(next.x, next.y + next.h);
                let n = t.apply(nx, ny);
                let gap = ((n.0 - bl.0) * cos + (n.1 - bl.1) * sin).max(width);
                width = gap;
            } else {
                // Vision can split one printed line into several runs; keep a word break between them.
                width *= (w.text.chars().count() + 1) as f64 / w.text.chars().count().max(1) as f64;
            }
            text.push(' ');
            let units: Vec<u16> = text.encode_utf16().collect();
            let natural = units.len() as f64 * font::ADVANCE * size;
            let tz = 100.0 * width / natural;
            let mut hex = String::with_capacity(units.len() * 4);
            for u in &units {
                let _ = write!(hex, "{u:04X}");
            }
            let _ = writeln!(
                cs,
                "/F0 {} Tf {} Tz {} {} {} {} {} {} Tm <{hex}> Tj",
                f(size),
                f(tz),
                f(cos),
                f(sin),
                f(-sin),
                f(cos),
                f(bl.0),
                f(bl.1)
            );
        }
    }
    cs.push_str("ET\n");
}

fn to_unicode_cmap() -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for block in 0..2u32 {
        s.push_str("128 beginbfrange\n");
        for hi in block * 128..block * 128 + 128 {
            let _ = writeln!(s, "<{hi:02X}00> <{hi:02X}FF> <{hi:02X}00>");
        }
        s.push_str("endbfrange\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} >>\nstream\n").into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

fn serialize(objs: &[Vec<u8>], root: usize, info: usize) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut x = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for off in offsets {
        let _ = writeln!(x, "{off:010} 00000 n ");
    }
    let _ = write!(
        x,
        "trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    );
    out.extend_from_slice(x.as_bytes());
    out
}

fn utf16_hex(s: &str) -> String {
    let mut h = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(h, "{u:04X}");
    }
    h.push('>');
    h
}

/// Compact number formatting for content streams.
fn f(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_maps_center_to_center() {
        let g = PageGeometry {
            skew: 0.02,
            cx: 1300.0,
            cy: 1750.0,
            paper_w: 2550.0,
            paper_h: 3300.0,
            detected: true,
            ..PageGeometry::full(2608.0, 3512.0, 300.0)
        };
        let t = PageTransform::new(&g, false);
        let (x, y) = t.apply(1300.0, 1750.0);
        assert!((x - 306.0).abs() < 1e-6 && (y - 396.0).abs() < 1e-6);
    }

    #[test]
    fn font_has_valid_checksum() {
        let ttf = font::glyphless_ttf();
        let sum = ttf.chunks(4).fold(0u32, |a, c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            a.wrapping_add(u32::from_be_bytes(w))
        });
        assert_eq!(sum, 0xb1b0_afba);
    }
}
