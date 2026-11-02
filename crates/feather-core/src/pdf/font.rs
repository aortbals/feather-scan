//! A minimal TrueType font whose glyphs are all empty and 500/1000 em wide.
//! It backs the invisible OCR text layer, the same trick Tesseract uses.

pub const ADVANCE: f64 = 0.5;

pub fn glyphless_ttf() -> Vec<u8> {
    let num_glyphs: u16 = 2;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();

    // OS/2 version 4.
    let mut os2 = Vec::new();
    push16(&mut os2, 4); // version
    push16(&mut os2, 500); // xAvgCharWidth
    push16(&mut os2, 400); // usWeightClass
    push16(&mut os2, 5); // usWidthClass
    push16(&mut os2, 0); // fsType: installable
    for v in [650, 600, 0, 75, 650, 600, 0, 350, 50, 250] {
        push16(&mut os2, v); // sub/superscript sizes and offsets, strikeout
    }
    push16(&mut os2, 0); // sFamilyClass
    os2.extend_from_slice(&[0; 10]); // panose
    os2.extend_from_slice(&[0; 16]); // ulUnicodeRange1-4
    os2.extend_from_slice(b"NONE"); // achVendID
    push16(&mut os2, 0x40); // fsSelection: regular
    push16(&mut os2, 0x20); // usFirstCharIndex
    push16(&mut os2, 0xffff); // usLastCharIndex
    push16(&mut os2, 1000); // sTypoAscender
    push16(&mut os2, (-1i16) as u16); // sTypoDescender
    push16(&mut os2, 0); // sTypoLineGap
    push16(&mut os2, 1000); // usWinAscent
    push16(&mut os2, 1); // usWinDescent
    os2.extend_from_slice(&[0; 8]); // ulCodePageRange1-2
    push16(&mut os2, 0); // sxHeight
    push16(&mut os2, 1000); // sCapHeight
    push16(&mut os2, 0); // usDefaultChar
    push16(&mut os2, 0x20); // usBreakChar
    push16(&mut os2, 1); // usMaxContext
    tables.push((*b"OS/2", os2));

    // cmap: format 4 with only the terminating segment (no mappings needed).
    let mut cmap = Vec::new();
    push16(&mut cmap, 0);
    push16(&mut cmap, 1);
    push16(&mut cmap, 3); // Windows
    push16(&mut cmap, 1); // Unicode BMP
    push32(&mut cmap, 12);
    for v in [4u16, 24, 0, 2, 2, 0, 0, 0xffff, 0, 0xffff, 1, 0] {
        push16(&mut cmap, v); // format, length, language, segX2, searchRange, entrySelector, rangeShift, endCode, pad, startCode, idDelta, idRangeOffset
    }
    tables.push((*b"cmap", cmap));

    // glyf: every glyph is empty; two bytes so the table is not zero-length.
    tables.push((*b"glyf", vec![0, 0]));

    let mut head = Vec::new();
    push32(&mut head, 0x0001_0000); // version
    push32(&mut head, 0x0001_0000); // fontRevision
    push32(&mut head, 0); // checkSumAdjustment, patched below
    push32(&mut head, 0x5f0f_3cf5); // magic
    push16(&mut head, 0x000b); // flags
    push16(&mut head, 1000); // unitsPerEm
    head.extend_from_slice(&[0; 16]); // created, modified
    for v in [0, 0, 500, 1000] {
        push16(&mut head, v); // bbox
    }
    push16(&mut head, 0); // macStyle
    push16(&mut head, 3); // lowestRecPPEM
    push16(&mut head, 2); // fontDirectionHint
    push16(&mut head, 0); // indexToLocFormat: short
    push16(&mut head, 0); // glyphDataFormat
    tables.push((*b"head", head));

    let mut hhea = Vec::new();
    push32(&mut hhea, 0x0001_0000);
    push16(&mut hhea, 1000); // ascender
    push16(&mut hhea, (-1i16) as u16); // descender
    push16(&mut hhea, 0); // lineGap
    push16(&mut hhea, 500); // advanceWidthMax
    push16(&mut hhea, 0); // minLeftSideBearing
    push16(&mut hhea, 0); // minRightSideBearing
    push16(&mut hhea, 0); // xMaxExtent
    push16(&mut hhea, 1); // caretSlopeRise
    push16(&mut hhea, 0); // caretSlopeRun
    push16(&mut hhea, 0); // caretOffset
    hhea.extend_from_slice(&[0; 8]);
    push16(&mut hhea, 0); // metricDataFormat
    push16(&mut hhea, num_glyphs); // numberOfHMetrics
    tables.push((*b"hhea", hhea));

    let mut hmtx = Vec::new();
    for _ in 0..num_glyphs {
        push16(&mut hmtx, 500);
        push16(&mut hmtx, 0);
    }
    tables.push((*b"hmtx", hmtx));

    let mut loca = Vec::new();
    for _ in 0..=num_glyphs {
        push16(&mut loca, 0);
    }
    tables.push((*b"loca", loca));

    let mut maxp = Vec::new();
    push32(&mut maxp, 0x0001_0000);
    push16(&mut maxp, num_glyphs);
    for v in [0u16, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        push16(&mut maxp, v); // maxPoints … maxComponentDepth; maxZones = 2
    }
    tables.push((*b"maxp", maxp));

    // name: family and PostScript name, Macintosh Roman.
    let names: [(u16, &str); 4] = [
        (1, "GlyphLessFont"),
        (2, "Regular"),
        (4, "GlyphLessFont"),
        (6, "GlyphLessFont"),
    ];
    let mut name = Vec::new();
    push16(&mut name, 0);
    push16(&mut name, names.len() as u16);
    push16(&mut name, 6 + 12 * names.len() as u16);
    let mut strings = Vec::new();
    for (id, s) in names {
        for v in [1u16, 0, 0, id, s.len() as u16, strings.len() as u16] {
            push16(&mut name, v);
        }
        strings.extend_from_slice(s.as_bytes());
    }
    name.extend_from_slice(&strings);
    tables.push((*b"name", name));

    let mut post = Vec::new();
    push32(&mut post, 0x0003_0000);
    post.extend_from_slice(&[0; 28]);
    tables.push((*b"post", post));

    tables.sort_by_key(|a| a.0);
    let n = tables.len() as u16;
    let mut out = Vec::new();
    push32(&mut out, 0x0001_0000);
    push16(&mut out, n);
    let es = 15 - n.leading_zeros() as u16; // floor(log2 n)
    push16(&mut out, (1 << es) * 16);
    push16(&mut out, es);
    push16(&mut out, n * 16 - (1 << es) * 16);
    let mut offset = 12 + 16 * tables.len();
    let mut head_offset = 0;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        push32(&mut out, checksum(data));
        push32(&mut out, offset as u32);
        push32(&mut out, data.len() as u32);
        if tag == b"head" {
            head_offset = offset;
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    let adj = 0xb1b0_afbau32.wrapping_sub(checksum(&out));
    out[head_offset + 8..head_offset + 12].copy_from_slice(&adj.to_be_bytes());
    out
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |acc, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        acc.wrapping_add(u32::from_be_bytes(w))
    })
}

fn push16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn push32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}
