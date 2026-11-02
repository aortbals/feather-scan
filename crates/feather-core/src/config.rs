use std::net::Ipv4Addr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scanner::ScanSettings;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where finished PDFs go.
    pub output_dir: PathBuf,
    /// Scanner IP. When unset, the scanner is found by broadcast discovery.
    pub scanner_ip: Option<Ipv4Addr>,
    /// Settings for scans started from the app, and the fallback for panel scans.
    pub scan: ScanSettings,
    /// Use resolution, color and duplex from the profile tapped on the touch panel.
    pub use_panel_settings: bool,
    pub remove_blank_pages: bool,
    /// Straighten, crop and clean up page images (whiter paper, crisper text).
    /// Off keeps the scanner's original JPEGs.
    pub clean_pages: bool,
    pub llm: LlmConfig,
    /// The pairing this computer uses on the scanner. Filled in automatically
    /// from an existing ScanSnap Home pairing for this computer.
    pub identity: Option<Identity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub enabled: bool,
    /// OpenAI-compatible base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    /// 16 hex chars.
    pub host_id: String,
    /// 32 hex chars.
    pub user_id: String,
    pub user_name: String,
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            output_dir: home.join("Documents").join("Scans"),
            scanner_ip: None,
            scan: ScanSettings::default(),
            use_panel_settings: true,
            remove_blank_pages: true,
            clean_pages: true,
            llm: LlmConfig::default(),
            identity: None,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            model: "gpt-5.6-luna".into(),
        }
    }
}

impl Config {
    pub fn from_json(s: &str) -> Result<Config, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}
