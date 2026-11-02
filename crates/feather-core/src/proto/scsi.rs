//! SCSI tunnel on TCP 53218.

use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use super::wire::{self, SCSI_PORT};
use crate::error::{Error, Result, proto};
use crate::net::HostAddr;

pub const GOOD: u32 = 0;
pub const CHECK_CONDITION: u32 = 2;

#[derive(Debug, Clone, Copy, Default)]
pub struct Sense {
    pub key: u8,
    pub asc: u8,
    pub ascq: u8,
    pub eom: bool,
    pub ili: bool,
    /// For short reads: requested − delivered.
    pub info: u32,
}

impl Sense {
    pub fn parse(d: &[u8]) -> Sense {
        if d.len() < 14 {
            return Sense::default();
        }
        Sense {
            key: d[2] & 0x0f,
            eom: d[2] & 0x40 != 0,
            ili: d[2] & 0x20 != 0,
            info: u32::from_be_bytes(d[3..7].try_into().unwrap()),
            asc: d[12],
            ascq: d[13],
        }
    }

    /// End of an image: no error, short read flagged.
    pub fn end_of_image(&self) -> bool {
        self.key == 0 && (self.ili || self.eom)
    }

    pub fn no_paper(&self) -> bool {
        self.key == 3 && self.asc == 0x80 && self.ascq == 0x03
    }

    pub fn medium_error(&self) -> Option<&'static str> {
        if self.key != 3 || self.asc != 0x80 {
            return None;
        }
        Some(match self.ascq {
            0x01 => "Paper jam",
            0x02 => "Scanner cover is open",
            0x03 => "No paper in the feeder",
            0x04 => "Unusual paper",
            0x07 => "Double feed detected",
            0x08 => "Feeder error",
            0x09 => "Carrier sheet detected",
            0x20 => "Stopped on the scanner",
            0x22 => "Scanning halted",
            0x30 => "Not enough paper",
            0x31 => "Scanning disabled",
            0x32 => "Scanning paused",
            0x33 => "Scanner refused the scan (Wi-Fi control error)",
            _ => "Scanner medium error",
        })
    }

    pub fn to_error(&self, what: &str) -> Error {
        match self.medium_error() {
            Some(m) => Error::Feed(m.to_string()),
            None => Error::Scsi(format!(
                "{what}: sense key {:#x} asc {:#04x} ascq {:#04x}",
                self.key, self.asc, self.ascq
            )),
        }
    }
}

pub struct Reply {
    pub status: u32,
    pub data: Vec<u8>,
}

pub struct ScsiChannel {
    stream: TcpStream,
    host: HostAddr,
}

impl ScsiChannel {
    pub fn open(scanner: Ipv4Addr, host: HostAddr) -> Result<Self> {
        let stream = wire::connect(scanner, SCSI_PORT, Duration::from_secs(10))?;
        // Feeding and transferring a sheet can take several seconds.
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        Ok(Self { stream, host })
    }

    pub fn command(&mut self, cdb: &[u8], out: &[u8], in_len: u32) -> Result<Reply> {
        let mut body = Vec::with_capacity(32 + out.len());
        body.extend_from_slice(&(cdb.len() as u32).to_be_bytes());
        body.extend_from_slice(&in_len.to_be_bytes());
        body.extend_from_slice(&(out.len() as u32).to_be_bytes());
        body.extend_from_slice(&[0; 4]);
        let mut c = [0u8; 16];
        c[..cdb.len()].copy_from_slice(cdb);
        body.extend_from_slice(&c);
        body.extend_from_slice(out);
        let frame = wire::request(1, &self.host, Some(self.host.ip), &body);
        wire::send(&mut self.stream, &frame)?;
        let r = wire::read_response(&mut self.stream)?;
        if r.body.len() < 24 {
            return Err(proto("short SCSI reply"));
        }
        Ok(Reply {
            status: r.status,
            data: r.body[24..].to_vec(),
        })
    }

    pub fn request_sense(&mut self) -> Result<Sense> {
        let r = self.command(&[0x03, 0, 0, 0, 0x12, 0], &[], 0x12)?;
        Ok(Sense::parse(&r.data))
    }

    /// Runs a command and turns CHECK CONDITION into an error.
    pub fn checked(&mut self, what: &str, cdb: &[u8], out: &[u8], in_len: u32) -> Result<Vec<u8>> {
        let r = self.command(cdb, out, in_len)?;
        match r.status {
            GOOD => Ok(r.data),
            CHECK_CONDITION => Err(self.request_sense()?.to_error(what)),
            s => Err(Error::Scsi(format!("{what}: status {s}"))),
        }
    }

    pub fn inquiry(&mut self) -> Result<String> {
        let d = self.checked("inquiry", &[0x12, 0, 0, 0, 0x60, 0], &[], 0x60)?;
        Ok(wire::cstr(d.get(8..36).unwrap_or(&[])))
    }

    pub fn hw_status(&mut self) -> Result<HwStatus> {
        let d = self.checked("status", &[0xc2, 0, 0, 0, 0, 0, 0, 0, 0x30, 0], &[], 0x30)?;
        Ok(HwStatus::parse(&d))
    }
}

/// GET HARDWARE STATUS (C2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HwStatus {
    pub feeder_empty: bool,
    pub button: bool,
    pub state: u8,
    pub selected_host: [u8; 8],
}

impl HwStatus {
    pub fn parse(d: &[u8]) -> HwStatus {
        if d.len() < 40 {
            return HwStatus::default();
        }
        HwStatus {
            feeder_empty: d[3] & 0x80 != 0,
            button: d[4] & 0x01 != 0,
            state: d[16],
            selected_host: d[32..40].try_into().unwrap(),
        }
    }
}
