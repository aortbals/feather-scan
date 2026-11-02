//! UDP discovery and heartbeat on port 52217.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::wire::{DISCOVERY_PORT, be32, cstr};
use crate::error::Result;
use crate::net::HostAddr;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScannerInfo {
    pub ip: Ipv4Addr,
    pub name: String,
    pub model: String,
    pub serial: String,
    pub mac: String,
    /// Host ID currently selected on the touch panel (hex), if any.
    pub selected_host_id: Option<String>,
}

const TAGS: [(&[u8; 4], u16); 3] = [(b"VENS", 0x0010), (b"ssNR", 0x0100), (b"V2ss", 0x1000)];

/// Discovery (seq 0) or heartbeat (seq 1) probe.
pub fn probe(tag: &[u8; 4], flag: u16, seq: u32, host: &HostAddr, reply_port: u16) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0..4].copy_from_slice(tag);
    p[4..8].copy_from_slice(&seq.to_be_bytes());
    p[8..12].copy_from_slice(&host.ip.octets());
    p[12..18].copy_from_slice(&host.mac);
    p[22..24].copy_from_slice(&reply_port.to_be_bytes());
    p[24..26].copy_from_slice(&flag.to_be_bytes());
    p
}

pub fn heartbeat(host: &HostAddr, reply_port: u16) -> [u8; 32] {
    probe(b"ssNR", 0x1000, 1, host, reply_port)
}

pub fn parse_reply(d: &[u8], from: Ipv4Addr) -> Option<ScannerInfo> {
    if d.len() < 144 || &d[0..4] != b"ssNR" && &d[0..4] != b"VENS" {
        return None;
    }
    let ip = Ipv4Addr::from(be32(d, 16));
    let host_id = &d[124..132];
    Some(ScannerInfo {
        ip: if ip.is_unspecified() { from } else { ip },
        name: cstr(&d[40..104]),
        model: cstr(&d[104..120]),
        serial: cstr(&d[132..144]),
        mac: crate::net::format_mac(&d[28..34].try_into().unwrap()),
        selected_host_id: if host_id.iter().all(|&b| b == 0) {
            None
        } else {
            Some(host_id.iter().map(|b| format!("{b:02x}")).collect())
        },
    })
}

/// Sends probes (to `target` if given, else broadcast) and collects replies for `wait`.
pub fn discover(target: Option<Ipv4Addr>, wait: Duration) -> Result<Vec<ScannerInfo>> {
    let host = match target {
        Some(t) => crate::net::host_addr_for(t)?,
        None => crate::net::primary_host_addr()
            .ok_or_else(|| crate::error::proto("no network interface"))?,
    };
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_broadcast(true)?;
    let port = sock.local_addr()?.port();
    let dests: Vec<Ipv4Addr> = match target {
        Some(t) => vec![t],
        None => vec![Ipv4Addr::BROADCAST],
    };
    let mut found: Vec<ScannerInfo> = Vec::new();
    let deadline = Instant::now() + wait;
    let mut next_send = Instant::now();
    let mut rounds = 0;
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        if rounds < 3 && Instant::now() >= next_send {
            for d in &dests {
                for (tag, flag) in TAGS {
                    let _ = sock.send_to(
                        &probe(tag, flag, 0, &host, port),
                        SocketAddrV4::new(*d, DISCOVERY_PORT),
                    );
                }
            }
            rounds += 1;
            next_send = Instant::now() + Duration::from_millis(300);
        }
        let left = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        sock.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        if let Ok((n, std::net::SocketAddr::V4(from))) = sock.recv_from(&mut buf)
            && let Some(info) = parse_reply(&buf[..n], *from.ip())
                && !found.iter().any(|f| f.ip == info.ip) {
                    found.push(info);
                }
    }
    Ok(found)
}
