//! The scan sequence on an open SCSI channel: D5, D8, E9, D4, (E0, READ…)*, D6.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, proto};
use crate::proto::scsi::{CHECK_CONDITION, GOOD, ScsiChannel};

/// Scan area ScanSnap Home sends, in 1/1200 inch (8.7 × 14.7 in).
const SCAN_WIDTH: u32 = 0x28d0;
const SCAN_HEIGHT: u32 = 0x44dc;
const READ_LEN: u32 = 0x30_0000;

/// D4 block ScanSnap Home sent to an iX1600 for its "auto" profile (color, 300 dpi).
/// Bytes 34–37 are dpi x/y; the rest is kept as captured.
const MODE_BLOCK: [u8; 51] = [
    0x00, 0x03, 0x01, 0x01, 0xd0, 0x00, 0xc1, 0x80, 0x80, 0x80, 0x90, 0x80, 0x80, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x30, //
    0x00, 0x10, 0x01, 0x2c, 0x01, 0x2c, 0x05, 0x82, 0x0d, 0x00, 0x00, 0x00, 0x28, 0xd0, 0x00,
    0x00, //
    0x44, 0xdc, 0x04,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// Keep color pages in color, store gray-looking pages as gray.
    Auto,
    Color,
    Gray,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScanSettings {
    pub dpi: u16,
    pub duplex: bool,
    pub color: ColorMode,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            dpi: 300,
            duplex: true,
            color: ColorMode::Auto,
        }
    }
}

impl ScanSettings {
    /// Settings from the profile tapped on the touch panel (0x41/0x0200 reply).
    pub fn from_panel_profile(profile: &serde_json::Value, fallback: ScanSettings) -> ScanSettings {
        let info = &profile["scan_info"][0];
        let mut s = fallback;
        s.dpi = match info["resolution"].as_str() {
            Some("normal") => 150,
            Some("fine") => 200,
            Some("superfine") => 300,
            Some("excellent") => 600,
            _ => fallback.dpi,
        };
        if let Some(p) = info["paper_handling"].as_str() {
            s.duplex = p != "single";
        }
        s.color = match info["color_mode"].as_str() {
            Some("color") => ColorMode::Color,
            Some("gray") | Some("monochrome") => ColorMode::Gray,
            Some("auto") => ColorMode::Auto,
            _ => fallback.color,
        };
        s
    }

    fn window_block(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..2].copy_from_slice(&self.dpi.to_be_bytes());
        b[2..4].copy_from_slice(&self.dpi.to_be_bytes());
        b[4..8].copy_from_slice(&SCAN_WIDTH.to_be_bytes());
        b[8..12].copy_from_slice(&SCAN_HEIGHT.to_be_bytes());
        b[12] = 0x05;
        b
    }

    fn mode_block(&self) -> [u8; 80] {
        let mut b = [0u8; 80];
        b[..MODE_BLOCK.len()].copy_from_slice(&MODE_BLOCK);
        b[34..36].copy_from_slice(&self.dpi.to_be_bytes());
        b[36..38].copy_from_slice(&self.dpi.to_be_bytes());
        b
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub dpi_x: u16,
    pub dpi_y: u16,
}

/// One sheet: front and (optional) back as JPEG bytes.
pub struct Sheet {
    pub front: Vec<u8>,
    pub back: Option<Vec<u8>>,
    pub info: ImageInfo,
}

pub struct Scanner<'a> {
    ch: &'a mut ScsiChannel,
}

impl<'a> Scanner<'a> {
    pub fn new(ch: &'a mut ScsiChannel) -> Self {
        Self { ch }
    }

    /// Scans every sheet in the feeder, calling `on_sheet` as each arrives.
    /// Returns the number of sheets. Always releases the scanner (D6), even on error.
    pub fn scan(
        &mut self,
        settings: &ScanSettings,
        mut on_sheet: impl FnMut(Sheet),
    ) -> Result<usize> {
        self.ch
            .checked("open", &[0xd5, 0, 0, 0x01, 0x08, 0x08], &[0; 8], 8)?;
        let result = self.run(settings, &mut on_sheet);
        let _ = self.ch.command(&[0xd6, 0, 0, 0, 0, 0], &[], 0);
        result
    }

    fn run(&mut self, settings: &ScanSettings, on_sheet: &mut impl FnMut(Sheet)) -> Result<usize> {
        let ch = &mut *self.ch;
        ch.checked("start", &[0xd8, 0, 0, 0, 0, 0], &[], 0)?;
        ch.checked(
            "window",
            &[0xe9, 0, 0, 0, 0, 0, 0, 0x20, 0, 0],
            &settings.window_block(),
            0,
        )?;
        ch.checked("mode", &[0xd4, 0, 0, 0, 0x50, 0], &settings.mode_block(), 0)?;
        ch.request_sense()?;
        let mut sheets = 0usize;
        let mut image_no: u8 = 0;
        loop {
            let r = ch.command(&[0xe0, 0, 0, 0, 0, 0], &[], 0)?;
            if r.status == CHECK_CONDITION {
                let sense = ch.request_sense()?;
                if sense.no_paper() {
                    return if sheets == 0 {
                        Err(Error::NoPaper)
                    } else {
                        Ok(sheets)
                    };
                }
                return Err(sense.to_error("feed"));
            }
            let front = read_image(ch, false, image_no)?;
            let back = read_image(ch, true, image_no.wrapping_add(1))?;
            let info = read_info(ch, image_no.wrapping_add(1))?;
            image_no = image_no.wrapping_add(2);
            sheets += 1;
            on_sheet(Sheet {
                front,
                back: settings.duplex.then_some(back),
                info,
            });
        }
    }
}

fn read_image(ch: &mut ScsiChannel, back: bool, image_no: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let len = READ_LEN.to_be_bytes();
    for chunk in 0u8..=255 {
        let cdb = [
            0x28,
            0,
            0,
            0x02,
            0,
            if back { 0x80 } else { 0 },
            len[1],
            len[2],
            len[3],
            0,
            image_no,
            chunk,
        ];
        let r = ch.command(&cdb, &[], READ_LEN)?;
        if r.data.len() >= 2 {
            out.extend_from_slice(&r.data[2..]);
        }
        match r.status {
            GOOD => continue,
            CHECK_CONDITION => {
                let sense = ch.request_sense()?;
                if sense.end_of_image() {
                    return finish_jpeg(out);
                }
                return Err(sense.to_error("read"));
            }
            s => return Err(Error::Scsi(format!("read: status {s}"))),
        }
    }
    Err(proto("image has too many chunks"))
}

fn finish_jpeg(mut data: Vec<u8>) -> Result<Vec<u8>> {
    if !data.starts_with(&[0xff, 0xd8]) {
        return Err(proto("image is not a JPEG"));
    }
    if let Some(end) = data.windows(2).rposition(|w| w == [0xff, 0xd9]) {
        data.truncate(end + 2);
    }
    Ok(data)
}

fn read_info(ch: &mut ScsiChannel, image_no: u8) -> Result<ImageInfo> {
    let cdb = [0x28, 0, 0x80, 0, 0, 0x80, 0, 0, 0x20, 0, image_no, 0];
    let d = ch.checked("image info", &cdb, &[], 0x20)?;
    if d.len() < 20 {
        return Ok(ImageInfo::default());
    }
    let be32 = |o: usize| u32::from_be_bytes(d[o..o + 4].try_into().unwrap());
    Ok(ImageInfo {
        width: be32(0),
        height: be32(12),
        dpi_x: u16::from_be_bytes([d[16], d[17]]),
        dpi_y: u16::from_be_bytes([d[18], d[19]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_match_capture() {
        let s = ScanSettings::default();
        assert_eq!(
            &s.window_block()[..13],
            &[
                0x01, 0x2c, 0x01, 0x2c, 0, 0, 0x28, 0xd0, 0, 0, 0x44, 0xdc, 0x05
            ]
        );
        assert_eq!(&s.mode_block()[..51], &MODE_BLOCK);
    }
}
