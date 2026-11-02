//! Appends every engine event, with a timestamp, to
//! `~/Library/Logs/Feather Scan/events.log`, for diagnosing timing and errors.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::engine::{Event, EventSink};

const MAX_BYTES: u64 = 2 << 20;

fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/Feather Scan/events.log"))
}

/// Wraps `sink` so each event is also written to the log file.
pub fn wrap(sink: EventSink) -> EventSink {
    let Some(path) = log_path() else { return sink };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok().map(Mutex::new);
    let file = Arc::new(file);
    Arc::new(move |e: Event| {
        if let Some(f) = file.as_ref() {
            let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let line = format!("{{\"t\":{ms},\"event\":{}}}\n", serde_json::to_string(&e).unwrap_or_default());
            if let Ok(mut f) = f.lock() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        sink(e);
    })
}
