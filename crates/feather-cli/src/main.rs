//! `feather`: command-line front end to the Feather Scan core.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use feather_core::config::Config;
use feather_core::engine::{Engine, Event};
use feather_core::pipeline::{self, ScannedImage};

const USAGE: &str = "\
feather — Feather Scan command line

USAGE:
  feather discover [IP]            find scanners (broadcast, or probe IP)
  feather run [--ip IP] [--out DIR] connect and wait for the Scan button
  feather scan [--ip IP] [--out DIR] connect, scan what's in the feeder, exit
  feather pdf [--out DIR] [--no-title] [--dpi N] IMAGE.jpg…
                                   build a searchable PDF from JPEGs (offline test)
  feather pair [--ip IP] [--dry-run]
                                   pair this computer with the scanner (no ScanSnap
                                   Home needed); --dry-run prints the new list only
  feather users --ip IP            print the scanner's computers and profiles
  feather config                   print the config path and current config

Config: ~/Library/Application Support/Feather Scan/config.json
LLM key: FEATHER_LLM_KEY or OPENAI_API_KEY (env) overrides the config.";

fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/Feather Scan/config.json")
}

fn load_config() -> Config {
    let mut cfg = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| Config::from_json(&s).ok())
        .unwrap_or_default();
    if let Ok(k) = std::env::var("FEATHER_LLM_KEY").or_else(|_| std::env::var("OPENAI_API_KEY")) {
        cfg.llm.api_key = Some(k);
    }
    cfg
}

fn save_identity(cfg: &Config) {
    let path = config_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let mut stored = cfg.clone();
    stored.llm.api_key = None; // never write the key to disk
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&stored).unwrap());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        println!("{USAGE}");
        return;
    };
    let mut cfg = load_config();
    let mut rest: Vec<String> = Vec::new();
    let mut no_title = false;
    let mut dry_run = false;
    let mut dpi = 300.0;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ip" => cfg.scanner_ip = it.next().and_then(|s| s.parse().ok()),
            "--out" => cfg.output_dir = it.next().map(PathBuf::from).unwrap_or(cfg.output_dir),
            "--no-title" => no_title = true,
            "--dry-run" => dry_run = true,
            "--dpi" => dpi = it.next().and_then(|s| s.parse().ok()).unwrap_or(dpi),
            _ => rest.push(a.clone()),
        }
    }
    match cmd {
        "discover" => {
            let target = rest.first().and_then(|s| s.parse().ok());
            match feather_core::proto::discovery::discover(target, Duration::from_secs(2)) {
                Ok(found) => println!("{}", serde_json::to_string_pretty(&found).unwrap()),
                Err(e) => fail(&e.to_string()),
            }
        }
        "run" | "scan" => run(cfg, cmd == "scan"),
        "pair" => pair(cfg, dry_run),
        "pdf" => {
            if rest.is_empty() {
                fail("give at least one JPEG");
            }
            if no_title {
                cfg.llm.enabled = false;
            }
            let images: Vec<ScannedImage> = rest
                .iter()
                .enumerate()
                .map(|(i, p)| ScannedImage {
                    jpeg: std::fs::read(p).unwrap_or_else(|e| fail(&format!("{p}: {e}"))),
                    dpi,
                    back: i % 2 == 1,
                })
                .collect();
            let t = std::time::Instant::now();
            let engine = feather_core::ocr::default_engine();
            match pipeline::process(images, &cfg, engine.as_ref()) {
                Ok(s) => {
                    println!("{}", serde_json::to_string_pretty(&s).unwrap());
                    eprintln!("took {:.1}s", t.elapsed().as_secs_f64());
                }
                Err(e) => fail(&e.to_string()),
            }
        }
        "users" => {
            let ip = cfg.scanner_ip.unwrap_or_else(|| fail("give --ip"));
            let mut host =
                feather_core::net::host_addr_for(ip).unwrap_or_else(|e| fail(&e.to_string()));
            if let Ok(m) = std::env::var("FEATHER_MAC") {
                let b: Vec<u8> = m
                    .split(':')
                    .filter_map(|x| u8::from_str_radix(x, 16).ok())
                    .collect();
                host.mac.copy_from_slice(&b[..6]);
            }
            eprintln!(
                "host {} {}",
                host.ip,
                feather_core::net::format_mac(&host.mac)
            );
            let ctl = feather_core::proto::control::Control::new(ip, host);
            match ctl.read_users() {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => fail(&e.to_string()),
            }
        }
        "analyze" => {
            for p in &rest {
                let jpeg = std::fs::read(p).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
                match feather_core::imaging::analyze(&jpeg, dpi) {
                    Ok(a) => {
                        println!("{p}: {a:?}");
                        if std::env::var("FEATHER_DEBUG").is_ok() {
                            let (c, r) = feather_core::imaging::debug_profiles(&jpeg).unwrap();
                            let fmt = |v: &[f64]| {
                                v.iter()
                                    .step_by(4)
                                    .map(|x| format!("{x:.0}"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            };
                            println!("cols: {}\nrows: {}", fmt(&c), fmt(&r));
                            for f in [0.3, 0.5, 0.7] {
                                let row = feather_core::imaging::debug_row(&jpeg, f).unwrap();
                                let n = row.len();
                                println!("row{f}: L {:?} … R {:?}", &row[..24], &row[n - 24..]);
                            }
                        }
                    }
                    Err(e) => println!("{p}: {e}"),
                }
            }
        }
        "config" => {
            println!("{}", config_path().display());
            let mut shown = cfg.clone();
            shown.llm.api_key = shown.llm.api_key.map(|_| "(set)".into());
            println!("{}", serde_json::to_string_pretty(&shown).unwrap());
        }
        _ => println!("{USAGE}"),
    }
}

fn pair(mut cfg: Config, dry_run: bool) {
    use feather_core::pairing;
    let ip = match cfg.scanner_ip {
        Some(ip) => ip,
        None => feather_core::proto::discovery::discover(None, Duration::from_secs(2))
            .ok()
            .and_then(|f| f.into_iter().next())
            .map(|s| s.ip)
            .unwrap_or_else(|| fail("no scanner found; give --ip")),
    };
    let me = pairing::identity_for_pairing(cfg.identity.as_ref())
        .unwrap_or_else(|e| fail(&e.to_string()));
    if dry_run {
        let host = feather_core::net::host_addr_for(ip).unwrap_or_else(|e| fail(&e.to_string()));
        let ctl = feather_core::proto::control::Control::new(ip, host);
        let current = ctl.read_users().unwrap_or_else(|e| fail(&e.to_string()));
        let (list, _) = pairing::merge(&current, &me, host.ip, "NEW-PROFILE-ID")
            .unwrap_or_else(|e| fail(&e.to_string()));
        eprintln!("dry run: would write this list (nothing was sent to the scanner)");
        println!("{}", serde_json::to_string_pretty(&list).unwrap());
        return;
    }
    eprintln!(
        "Quit ScanSnap Home and the Feather Scan app first: only one app can hold the scanner."
    );
    match pairing::pair(ip, &me, &|m| eprintln!("{m}")) {
        Ok(p) => {
            cfg.scanner_ip = Some(ip);
            cfg.identity = Some(p.identity.clone());
            save_identity(&cfg);
            eprintln!("Saved the new identity to {}", config_path().display());
            eprintln!("{}", p.instructions());
            if p.connect_code >= 0 {
                eprintln!("(The scanner already has this computer selected.)");
            }
        }
        Err(e) => fail(&format!("pairing failed: {e}")),
    }
}

fn run(cfg: Config, once: bool) {
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let engine = Engine::start(
        cfg.clone(),
        Arc::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut cfg = cfg;
    let mut scanned = false;
    let mut pending_jobs = 0usize;
    for e in rx {
        println!("{}", serde_json::to_string(&e).unwrap());
        match &e {
            Event::Identity { identity } => {
                cfg.identity = Some(identity.clone());
                save_identity(&cfg);
            }
            Event::Status { state, .. }
                if once && !scanned && *state == feather_core::engine::State::Ready =>
            {
                scanned = true;
                engine.scan_now();
            }
            Event::Processing { .. } => pending_jobs += 1,
            Event::Saved { .. } => pending_jobs = pending_jobs.saturating_sub(1),
            Event::Error { .. } if once && scanned && pending_jobs == 0 => break,
            _ => {}
        }
        if once && scanned && pending_jobs == 0 && matches!(e, Event::Saved { .. }) {
            break;
        }
    }
    engine.stop();
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
