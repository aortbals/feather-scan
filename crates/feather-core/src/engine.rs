//! Background engine: holds the scanner session the way ScanSnap Home does,
//! waits for the Scan button, scans, and runs the PDF pipeline.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::config::{Config, Identity};
use crate::error::{Error, Result};
use crate::net::HostAddr;
use crate::pairing;
use crate::pipeline::{self, ScannedImage};
use crate::proto::control::{self, Control, INTENT_CONNECT};
use crate::proto::discovery::{self, ScannerInfo};
use crate::proto::scsi::{HwStatus, ScsiChannel};
use crate::proto::wire::{DISCOVERY_PORT, NOTIFY_PORT};
use crate::scanner::{ScanSettings, Scanner};

const POLL: Duration = Duration::from_millis(250);
const ANNOUNCE_EVERY: Duration = Duration::from_secs(10);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(31);
const RETRY_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Status {
        state: State,
        message: String,
        scanner: Option<ScannerInfo>,
    },
    ScanStarted {
        source: String,
    },
    Page {
        sheets: usize,
    },
    Processing {
        images: usize,
    },
    Saved {
        path: String,
        title: String,
        pages: usize,
        removed_blank: usize,
        title_error: Option<String>,
    },
    Error {
        message: String,
    },
    /// Identity adopted from the scanner or made by pairing; the app should persist it.
    Identity {
        identity: Identity,
    },
    /// Pairing finished; `message` tells the person to select this computer on the panel.
    Paired {
        identity: Identity,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Searching,
    Connecting,
    Ready,
    Scanning,
    NotPaired,
    Offline,
    Stopped,
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

enum Cmd {
    ScanNow,
    Pair,
    Config(Box<Config>),
    Stop,
}

pub struct Engine {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(cfg: Config, sink: EventSink) -> Engine {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("feather-session".into())
            .spawn(move || Session::new(cfg, sink, rx).run())
            .expect("spawn session thread");
        Engine {
            tx,
            thread: Some(thread),
        }
    }

    pub fn scan_now(&self) {
        let _ = self.tx.send(Cmd::ScanNow);
    }

    /// Pairs this computer with the scanner without ScanSnap Home. Ends the current
    /// session, then emits `Identity` and a status asking to select it on the panel.
    pub fn pair(&self) {
        let _ = self.tx.send(Cmd::Pair);
    }

    pub fn set_config(&self, cfg: Config) {
        let _ = self.tx.send(Cmd::Config(Box::new(cfg)));
    }

    pub fn stop(mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Connected {
    info: ScannerInfo,
    host: HostAddr,
    scsi: ScsiChannel,
    identity: Identity,
    notify: UdpSocket,
    heartbeat: UdpSocket,
    last_announce: Instant,
    last_heartbeat: Instant,
    heartbeats_left: u8,
    last_button_counter: Option<(u8, Instant)>,
    profile_id: Option<String>,
    host_id: [u8; 8],
}

struct Session {
    cfg: Config,
    sink: EventSink,
    rx: Receiver<Cmd>,
    /// PDF jobs run here so the scanner is free for the next batch.
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Set when a command ends `connect` early (`Error::Interrupted`).
    interrupt: Option<Flow>,
    /// UDP 53220: the scanner broadcasts "computer X was picked on the panel" here.
    select_notices: Option<UdpSocket>,
    /// The panel picked this computer while a session was up; the next connect
    /// must run the pairing-mode handshake.
    handshake_due: bool,
    /// When the last handshake ran; the scanner repeats each notice several times.
    last_handshake: Option<Instant>,
}

enum Flow {
    Continue,
    Reconnect,
    Pair,
    Stop,
}

impl Session {
    fn new(cfg: Config, sink: EventSink, rx: Receiver<Cmd>) -> Self {
        Self {
            cfg,
            sink,
            rx,
            workers: Arc::new(Mutex::new(Vec::new())),
            interrupt: None,
            select_notices: None,
            handshake_due: false,
            last_handshake: None,
        }
    }

    fn emit(&self, e: Event) {
        (self.sink)(e);
    }

    fn status(&self, state: State, message: impl Into<String>, scanner: Option<&ScannerInfo>) {
        self.emit(Event::Status {
            state,
            message: message.into(),
            scanner: scanner.cloned(),
        });
    }

    fn run(mut self) {
        loop {
            match self.connect() {
                Ok(mut c) => {
                    self.status(
                        State::Ready,
                        format!("Ready on {}", c.info.name),
                        Some(&c.info),
                    );
                    let flow = self.serve(&mut c);
                    let _ = Control::new(c.info.ip, c.host).unregister();
                    match flow {
                        Flow::Stop => break,
                        Flow::Pair => {
                            self.pair();
                            continue;
                        }
                        _ => {}
                    }
                }
                Err(Error::Interrupted) => match self.interrupt.take() {
                    Some(Flow::Stop) => break,
                    Some(Flow::Pair) => {
                        self.pair();
                        continue;
                    }
                    _ => continue,
                },
                Err(Error::Config(m)) => self.status(State::NotPaired, m, None),
                Err(e) => self.status(State::Offline, e.to_string(), None),
            }
            // Wait before retrying, but stay responsive to commands.
            match self.rx.recv_timeout(RETRY_AFTER) {
                Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Cmd::Pair) => self.pair(),
                Ok(Cmd::Config(c)) => self.cfg = *c,
                Ok(Cmd::ScanNow) => self.emit(Event::Error {
                    message: "Scanner is not connected".into(),
                }),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        for h in self.workers.lock().unwrap().drain(..) {
            let _ = h.join();
        }
        self.status(State::Stopped, "Stopped", None);
    }

    /// Pairs with the scanner (no session may be open), then persists the new identity.
    fn pair(&mut self) {
        let info = match self.find_scanner() {
            Ok(i) => i,
            Err(e) => {
                self.emit(Event::Error {
                    message: format!("Pairing failed: {e}"),
                });
                return;
            }
        };
        self.status(
            State::Connecting,
            format!("Pairing with {}…", info.name),
            Some(&info),
        );
        // Give the scanner a moment after the session we just ended.
        std::thread::sleep(Duration::from_secs(1));
        let result = pairing::identity_for_pairing(self.cfg.identity.as_ref())
            .and_then(|me| pairing::pair(info.ip, &me, &|_| {}));
        match result {
            Ok(p) => {
                self.cfg.identity = Some(p.identity.clone());
                self.emit(Event::Identity {
                    identity: p.identity.clone(),
                });
                self.emit(Event::Paired {
                    identity: p.identity.clone(),
                    message: p.instructions(),
                });
                self.status(State::NotPaired, p.instructions(), Some(&info));
            }
            Err(e) => self.emit(Event::Error {
                message: format!("Pairing failed: {e}"),
            }),
        }
    }

    fn find_scanner(&self) -> Result<ScannerInfo> {
        self.status(State::Searching, "Looking for the scanner…", None);
        let found = discovery::discover(self.cfg.scanner_ip, Duration::from_millis(1500))?;
        if let Some(s) = found.into_iter().next() {
            return Ok(s);
        }
        // The scanner stays quiet while another computer holds it; try the IP directly.
        if let Some(ip) = self.cfg.scanner_ip {
            return Ok(ScannerInfo {
                ip,
                name: ip.to_string(),
                model: String::new(),
                serial: String::new(),
                mac: String::new(),
                selected_host_id: None,
            });
        }
        Err(Error::Protocol(
            "No scanner found. Is it on, and is ScanSnap Home quit? You can also set the scanner's IP.".into(),
        ))
    }

    fn connect(&mut self) -> Result<Connected> {
        let info = self.find_scanner()?;
        self.status(
            State::Connecting,
            format!("Connecting to {}…", info.name),
            Some(&info),
        );
        let mut host = crate::net::host_addr_for(info.ip)?;
        let notify = UdpSocket::bind(("0.0.0.0", NOTIFY_PORT)).map_err(|e| {
            Error::Protocol(format!(
                "Can't listen on UDP {NOTIFY_PORT} ({e}). Quit ScanSnap Home so Feather Scan can receive the Scan button."
            ))
        })?;
        notify.set_nonblocking(true)?;
        let heartbeat = UdpSocket::bind("0.0.0.0:0")?;

        let users = Control::new(info.ip, host).read_users()?;
        let identity = match &self.cfg.identity {
            Some(id) => id.clone(),
            None => {
                let id = adopt_identity(&users).ok_or_else(|| {
                    Error::Config(
                        "This computer isn't paired with the scanner. Use Pair with Scanner in Settings (or `feather pair`), then select it on the scanner's screen.".into(),
                    )
                })?;
                self.cfg.identity = Some(id.clone());
                self.emit(Event::Identity {
                    identity: id.clone(),
                });
                id
            }
        };
        let host_id = control::parse_host_id(&identity.host_id)
            .ok_or_else(|| Error::Config(format!("bad host id {}", identity.host_id)))?;
        if crate::net::is_masked(&host.mac) {
            host.mac = crate::net::mac_from_host_id(&host_id);
        }
        let ctl = Control::new(info.ip, host);
        let mut scsi = ScsiChannel::open(info.ip, host)?;
        scsi.inquiry()?;
        ctl.announce(&identity.user_id, NOTIFY_PORT)?;
        if self.select_notices.is_none() {
            self.select_notices = bind_select_notices();
        }
        let _ = self.selection_notice(info.ip); // drop stale notices
        let mut st = if std::mem::take(&mut self.handshake_due) {
            self.run_handshake(&ctl, &mut scsi, &identity)?
        } else {
            register_retrying(&ctl, &mut scsi, &host_id)?
        };
        let mut users = users;
        if st == NOT_SELECTED {
            // Stay online so the panel offers this computer, until someone selects it.
            match self.wait_for_selection(
                &info, &ctl, &mut scsi, &heartbeat, &notify, &host_id, &identity,
            )? {
                Some(code) => st = code,
                None => return Err(Error::Interrupted),
            }
            // Only the selected computer's profiles are listed; ours are visible now.
            users = ctl.read_users().unwrap_or(users);
        }
        if st < 0 {
            return Err(Error::Protocol(format!(
                "Scanner refused the connection ({st}). Select this computer on the scanner's screen, and quit ScanSnap Home."
            )));
        }
        // A profile of ours to select before scans started from the app.
        let profile_id = users["profiles"].as_array().and_then(|ps| {
            ps.iter()
                .find(|p| p["user_id"].as_str() == Some(&identity.user_id))?["prof_id"]
                .as_str()
                .map(String::from)
        });
        let _ = ctl.state();
        Ok(Connected {
            info,
            host,
            scsi,
            identity,
            notify,
            heartbeat,
            last_announce: Instant::now(),
            last_heartbeat: Instant::now() - HEARTBEAT_EVERY,
            heartbeats_left: 0,
            last_button_counter: None,
            profile_id,
            host_id,
        })
    }

    /// Runs the pairing-mode handshake ScanSnap Home runs when its computer is
    /// picked on the panel, and returns the final register result.
    fn run_handshake(
        &mut self,
        ctl: &Control,
        scsi: &mut ScsiChannel,
        identity: &Identity,
    ) -> Result<i32> {
        let trace = std::env::var_os("FEATHER_TRACE").is_some();
        let h = pairing::handshake(ctl, scsi, identity, false, &|m| {
            if trace {
                eprintln!("handshake: {m}");
            }
        })?;
        if trace {
            eprintln!("handshake: register -> {}", h.connect_code);
        }
        self.last_handshake = Some(Instant::now());
        Ok(h.connect_code)
    }

    /// The host ID in the latest "computer picked on the panel" notice, if any.
    fn selection_notice(&self, scanner: Ipv4Addr) -> Option<[u8; 8]> {
        let sock = self.select_notices.as_ref()?;
        let mut buf = [0u8; 256];
        let mut picked = None;
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            let d = &buf[..n];
            crate::proto::wire::trace(&format!("udp53220 {from} <<"), d);
            let from_scanner = matches!(from.ip(), std::net::IpAddr::V4(ip) if ip == scanner);
            if from_scanner && n >= 40 && &d[4..8] == b"ssNR" && be32(d, 8) == NOTICE_SELECTED {
                picked = Some(d[32..40].try_into().unwrap());
            }
        }
        picked
    }

    /// Keeps a standby session (C2 polling, announces, heartbeat) like ScanSnap Home
    /// does while another computer is selected; the panel shows hosts without one as
    /// unavailable. Returns the register result once this computer is selected, or
    /// None when a command (stop, pair, new settings) interrupts; see `self.interrupt`.
    #[allow(clippy::too_many_arguments)]
    fn wait_for_selection(
        &mut self,
        info: &ScannerInfo,
        ctl: &Control,
        scsi: &mut ScsiChannel,
        heartbeat: &UdpSocket,
        notify: &UdpSocket,
        host_id: &[u8; 8],
        identity: &Identity,
    ) -> Result<Option<i32>> {
        let msg = format!(
            "On the scanner's screen, tap the person icon and pick \"{}\".",
            identity.user_name
        );
        self.status(State::NotPaired, msg.clone(), Some(info));
        let mut last_announce = Instant::now();
        let mut last_register = Instant::now();
        let mut last_heartbeat = Instant::now() - HEARTBEAT_EVERY;
        let mut heartbeats_left = 0u8;
        heartbeat.set_nonblocking(true)?;
        loop {
            match self.rx.recv_timeout(POLL) {
                Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    self.interrupt = Some(Flow::Stop);
                    return Ok(None);
                }
                Ok(Cmd::Pair) => {
                    self.interrupt = Some(Flow::Pair);
                    return Ok(None);
                }
                Ok(Cmd::Config(cfg)) => {
                    let reconnect =
                        cfg.scanner_ip != self.cfg.scanner_ip || cfg.identity != self.cfg.identity;
                    self.cfg = *cfg;
                    if reconnect {
                        self.interrupt = Some(Flow::Reconnect);
                        return Ok(None);
                    }
                }
                Ok(Cmd::ScanNow) => self.emit(Event::Error {
                    message: msg.clone(),
                }),
                Err(RecvTimeoutError::Timeout) => {}
            }
            let hw = scsi.hw_status()?;
            let mut buf = [0u8; 256];
            while let Ok((n, from)) = notify.recv_from(&mut buf) {
                crate::proto::wire::trace(&format!("udp {from} <<"), &buf[..n]);
            }
            if std::env::var_os("FEATHER_TRACE").is_some() {
                eprintln!(
                    "standby: c2 state {:#04x} selected {}",
                    hw.state,
                    crate::proto::wire::hex(&hw.selected_host)
                );
            }
            // Picked on the panel: the scanner broadcasts 0x23 naming us, and C2
            // shows our host ID. It then waits for the pairing-mode handshake.
            let picked = self.selection_notice(info.ip) == Some(*host_id);
            let selected = hw.selected_host == *host_id;
            if (picked || selected) && last_register.elapsed() >= Duration::from_secs(2) {
                last_register = Instant::now();
                let st = self.run_handshake(ctl, scsi, identity)?;
                if st >= 0 {
                    return Ok(Some(st));
                }
                last_announce = Instant::now();
            }
            if last_announce.elapsed() >= ANNOUNCE_EVERY {
                last_announce = Instant::now();
                ctl.announce(&identity.user_id, NOTIFY_PORT)?;
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                last_heartbeat = Instant::now();
                heartbeats_left = 3;
            }
            if heartbeats_left > 0 {
                heartbeats_left -= 1;
                let reply_port = heartbeat.local_addr().map(|a| a.port()).unwrap_or(0);
                let _ = heartbeat.send_to(
                    &discovery::heartbeat(&ctl.host, reply_port),
                    (info.ip, DISCOVERY_PORT),
                );
            }
            let mut drain = [0u8; 64];
            while heartbeat.recv_from(&mut drain).is_ok() {}
        }
    }

    fn serve(&mut self, c: &mut Connected) -> Flow {
        let mut last_status = HwStatus::default();
        loop {
            match self.rx.recv_timeout(POLL) {
                Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => return Flow::Stop,
                Ok(Cmd::Config(cfg)) => {
                    let reconnect =
                        cfg.scanner_ip != self.cfg.scanner_ip || cfg.identity != self.cfg.identity;
                    self.cfg = *cfg;
                    if reconnect {
                        return Flow::Reconnect;
                    }
                }
                Ok(Cmd::ScanNow) => {
                    if let Flow::Reconnect = self.scan(c, "app", None) {
                        return Flow::Reconnect;
                    }
                }
                Ok(Cmd::Pair) => return Flow::Pair,
                Err(RecvTimeoutError::Timeout) => {}
            }
            match self.tick(c, &mut last_status) {
                Flow::Continue => {}
                other => return other,
            }
        }
    }

    fn tick(&mut self, c: &mut Connected, last: &mut HwStatus) -> Flow {
        let status = match c.scsi.hw_status() {
            Ok(s) => s,
            Err(e) => {
                self.status(
                    State::Offline,
                    format!("Lost the scanner: {e}"),
                    Some(&c.info),
                );
                return Flow::Reconnect;
            }
        };
        if (status.state, status.selected_host) != (last.state, last.selected_host)
            && std::env::var_os("FEATHER_TRACE").is_some()
        {
            eprintln!(
                "c2 state {:#04x} selected {}",
                status.state,
                crate::proto::wire::hex(&status.selected_host)
            );
        }
        *last = status;
        // Someone picked another computer on the panel (or is picking one): the scanner
        // has dropped this session. Start over, which waits until we're selected again.
        if status.state == STATE_UNREGISTERED || status.selected_host != c.host_id {
            self.status(
                State::Connecting,
                "The scanner's selected computer changed",
                Some(&c.info),
            );
            return Flow::Reconnect;
        }
        if c.last_announce.elapsed() >= ANNOUNCE_EVERY {
            c.last_announce = Instant::now();
            if let Err(e) =
                Control::new(c.info.ip, c.host).announce(&c.identity.user_id, NOTIFY_PORT)
            {
                self.status(
                    State::Offline,
                    format!("Lost the scanner: {e}"),
                    Some(&c.info),
                );
                return Flow::Reconnect;
            }
        }
        // Heartbeat: three probes half a second apart, every ~31 s.
        if c.last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
            c.last_heartbeat = Instant::now();
            c.heartbeats_left = 3;
        }
        if c.heartbeats_left > 0
            && c.last_heartbeat.elapsed()
                >= Duration::from_millis(500 * (3 - c.heartbeats_left as u64))
        {
            c.heartbeats_left -= 1;
            let reply_port = c.heartbeat.local_addr().map(|a| a.port()).unwrap_or(0);
            let _ = c.heartbeat.send_to(
                &discovery::heartbeat(&c.host, reply_port),
                (c.info.ip, DISCOVERY_PORT),
            );
        }
        let mut drain = [0u8; 64];
        while c.heartbeat.set_nonblocking(true).is_ok() && c.heartbeat.recv_from(&mut drain).is_ok()
        {
        }

        if let Some(id) = self.selection_notice(c.info.ip) {
            // Someone picked a computer on the panel. If it's us again, the scanner
            // wants the pairing-mode handshake; otherwise it has dropped us.
            // Repeats of the notice that led to our last handshake are ignored.
            let repeat = id == c.host_id
                && self
                    .last_handshake
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(15));
            if !repeat {
                self.handshake_due = id == c.host_id;
                return Flow::Reconnect;
            }
        }
        if let Some(counter) = poll_button(c) {
            let dup = c
                .last_button_counter
                .is_some_and(|(n, t)| n == counter && t.elapsed() < Duration::from_secs(10));
            c.last_button_counter = Some((counter, Instant::now()));
            if !dup {
                let profile = Control::new(c.info.ip, c.host).read_tapped_profile().ok();
                let flow = self.scan(c, "scanner", profile.as_ref());
                // The scanner repeats each notice; drop repeats that queued up during the scan.
                let _ = poll_button(c);
                c.last_button_counter = Some((counter, Instant::now()));
                if let Flow::Reconnect = flow {
                    return Flow::Reconnect;
                }
            }
        }
        Flow::Continue
    }

    fn scan(&mut self, c: &mut Connected, source: &str, panel_profile: Option<&Value>) -> Flow {
        let mut settings = self.cfg.scan;
        if let (true, Some(p)) = (self.cfg.use_panel_settings, panel_profile) {
            let prof = p["profiles"].get(0).unwrap_or(p);
            settings = ScanSettings::from_panel_profile(prof, settings);
        }
        if panel_profile.is_none() {
            // Scans started from the computer need a selected profile, or D5 is refused.
            if let Some(id) = &c.profile_id {
                let _ = Control::new(c.info.ip, c.host).select_profile(id);
            }
        }
        self.emit(Event::ScanStarted {
            source: source.into(),
        });
        self.status(State::Scanning, "Scanning…", Some(&c.info));
        let mut images = Vec::new();
        let mut sheets = 0;
        let sink = self.sink.clone();
        let result = Scanner::new(&mut c.scsi).scan(&settings, |sheet| {
            sheets += 1;
            let dpi = if sheet.info.dpi_x > 0 {
                sheet.info.dpi_x as f64
            } else {
                settings.dpi as f64
            };
            images.push(ScannedImage {
                jpeg: sheet.front,
                dpi,
                back: false,
            });
            if let Some(b) = sheet.back {
                images.push(ScannedImage {
                    jpeg: b,
                    dpi,
                    back: true,
                });
            }
            sink(Event::Page { sheets });
        });
        let flow = match &result {
            Err(Error::Io(_)) | Err(Error::Protocol(_)) => Flow::Reconnect,
            _ => Flow::Continue,
        };
        match result {
            Err(Error::NoPaper) => self.emit(Event::Error {
                message: "No paper in the feeder".into(),
            }),
            Err(e) => self.emit(Event::Error {
                message: format!("Scan failed: {e}"),
            }),
            Ok(_) => {}
        }
        if !images.is_empty() {
            self.spawn_pipeline(images);
        }
        if matches!(flow, Flow::Continue) {
            self.status(
                State::Ready,
                format!("Ready on {}", c.info.name),
                Some(&c.info),
            );
        }
        flow
    }

    fn spawn_pipeline(&self, images: Vec<ScannedImage>) {
        let cfg = self.cfg.clone();
        let sink = self.sink.clone();
        let handle = std::thread::spawn(move || {
            sink(Event::Processing {
                images: images.len(),
            });
            let engine = crate::ocr::default_engine();
            match pipeline::process(images, &cfg, engine.as_ref()) {
                Ok(s) => sink(Event::Saved {
                    path: s.path.display().to_string(),
                    title: s.title,
                    pages: s.pages,
                    removed_blank: s.removed_blank,
                    title_error: s.title_error,
                }),
                Err(e) => sink(Event::Error {
                    message: format!("Couldn't save the PDF: {e}"),
                }),
            }
        });
        let mut w = self.workers.lock().unwrap();
        w.retain(|h| !h.is_finished());
        w.push(handle);
    }
}

/// UDP 53220 notice: a computer was picked on the touch panel (host ID at 32).
const NOTICE_SELECTED: u32 = 0x23;

fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

fn bind_select_notices() -> Option<UdpSocket> {
    let s = UdpSocket::bind(("0.0.0.0", crate::proto::wire::SCANNER_NOTICE_PORT)).ok()?;
    s.set_nonblocking(true).ok()?;
    let _ = s.set_broadcast(true);
    Some(s)
}

/// C2 state byte when no host is registered.
const STATE_UNREGISTERED: u8 = 0x80;

/// 0x11 answer while the panel has another computer selected.
const NOT_SELECTED: i32 = -7;

/// 0x11 intent 0. The scanner answers -4 (busy) for a while after another session
/// ends, or right after pairing; retry those.
fn register_retrying(ctl: &Control, scsi: &mut ScsiChannel, host_id: &[u8; 8]) -> Result<i32> {
    let mut st = ctl.register(INTENT_CONNECT, host_id, NOTIFY_PORT)?;
    for _ in 0..10 {
        if st != -4 {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        let _ = scsi.hw_status();
        st = ctl.register(INTENT_CONNECT, host_id, NOTIFY_PORT)?;
    }
    Ok(st)
}

/// Returns the button counter when a scan-button notice is waiting.
fn poll_button(c: &mut Connected) -> Option<u8> {
    let mut buf = [0u8; 256];
    let mut pressed = None;
    while let Ok((n, from)) = c.notify.recv_from(&mut buf) {
        let d = &buf[..n];
        crate::proto::wire::trace(&format!("udp {from} <<"), d);
        let from_scanner = matches!(from.ip(), std::net::IpAddr::V4(ip) if ip == c.info.ip);
        if n >= 20
            && &d[4..8] == b"ssNR"
            && from_scanner
            && u32::from_be_bytes(d[8..12].try_into().unwrap()) == 1
        {
            pressed = Some(d[16]);
        }
    }
    pressed
}

/// Uses an existing ScanSnap Home pairing when the panel's selected computer is this one.
pub fn adopt_identity(users: &Value) -> Option<Identity> {
    let selected = users["conn_user"]["select"]["id"].as_str()?.to_string();
    let user = users["conn_user"]["users"]
        .as_array()?
        .iter()
        .find(|u| u["host_id"].as_str() == Some(&selected))?;
    let name = user["name"].as_str()?.to_string();
    let local: Vec<String> = local_ips().iter().map(|i| i.to_string()).collect();
    let profiles = users["profiles"].as_array()?;
    let mine = profiles.iter().find(|p| {
        p["user_name"].as_str() == Some(&name)
            && p["host_address"]
                .as_str()
                .is_some_and(|a| local.iter().any(|l| l == a))
    })?;
    Some(Identity {
        host_id: selected,
        user_id: mine["user_id"].as_str()?.to_string(),
        user_name: name,
    })
}

fn local_ips() -> Vec<Ipv4Addr> {
    let mut v = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) == 0 {
            let mut cur = ifap;
            while !cur.is_null() {
                let ifa = &*cur;
                if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    v.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
                }
                cur = ifa.ifa_next;
            }
            libc::freeifaddrs(ifap);
        }
    }
    v
}
