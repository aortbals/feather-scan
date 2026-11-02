//! Feather Scan core: talks to ScanSnap scanners over Wi-Fi and turns scans
//! into searchable PDFs with smart names.

pub mod config;
pub mod engine;
pub mod enhance;
pub mod error;
pub mod ffi;
pub mod imaging;
pub mod net;
pub mod ocr;
pub mod pdf;
pub mod pipeline;
pub mod scanner;
pub mod time;
pub mod title;

pub mod proto {
    pub mod control;
    pub mod discovery;
    pub mod scsi;
    pub mod wire;
}

pub use config::Config;
pub use engine::{Engine, Event};
pub use error::{Error, Result};
