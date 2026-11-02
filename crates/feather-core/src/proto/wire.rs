//! `ssNR` TCP framing shared by the SCSI tunnel (53218) and the control channel (53219).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use crate::error::{Result, proto};
use crate::net::HostAddr;

pub const MAGIC: &[u8; 4] = b"ssNR";
pub const SCSI_PORT: u16 = 53218;
pub const CONTROL_PORT: u16 = 53219;
pub const DISCOVERY_PORT: u16 = 52217;
pub const NOTIFY_PORT: u16 = 55265;
/// The scanner broadcasts state notices here (power-on, computer picked on the panel).
pub const SCANNER_NOTICE_PORT: u16 = 53220;

/// Connects and consumes the 16-byte greeting the scanner sends first.
pub fn connect(ip: Ipv4Addr, port: u16, timeout: Duration) -> Result<TcpStream> {
    let s = TcpStream::connect_timeout(&SocketAddrV4::new(ip, port).into(), timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.set_nodelay(true)?;
    let mut g = [0u8; 16];
    (&s).read_exact(&mut g)?;
    if &g[4..8] != MAGIC || u32::from_be_bytes(g[0..4].try_into().unwrap()) != 16 {
        return Err(proto(format!("unexpected greeting {}", hex(&g))));
    }
    Ok(s)
}

/// Builds a request: 32-byte header plus body.
pub fn request(command: u32, host: &HostAddr, ip_field: Option<Ipv4Addr>, body: &[u8]) -> Vec<u8> {
    let len = 32 + body.len();
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&(len as u32).to_be_bytes());
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&command.to_be_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&host.mac);
    v.extend_from_slice(&[0; 2]);
    v.extend_from_slice(&ip_field.map(|i| i.octets()).unwrap_or([0; 4]));
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(body);
    v
}

pub struct Response {
    /// Status word from header offset 12.
    /// Result code at header offset 8 (control channel: negative = refused).
    pub code: i32,
    /// SCSI status at header offset 12 (2 = CHECK CONDITION).
    pub status: u32,
    /// Everything after the 16-byte header.
    pub body: Vec<u8>,
}

/// Reads one length-prefixed response frame.
pub fn read_response(s: &mut TcpStream) -> Result<Response> {
    let mut h = [0u8; 16];
    s.read_exact(&mut h)?;
    if &h[4..8] != MAGIC {
        return Err(proto(format!("bad frame header {}", hex(&h))));
    }
    let len = u32::from_be_bytes(h[0..4].try_into().unwrap()) as usize;
    if !(16..=64 << 20).contains(&len) {
        return Err(proto(format!("bad frame length {len}")));
    }
    let mut body = vec![0u8; len - 16];
    s.read_exact(&mut body)?;
    trace("<<", &[&h[..], &body[..]].concat());
    Ok(Response {
        code: i32::from_be_bytes(h[8..12].try_into().unwrap()),
        status: u32::from_be_bytes(h[12..16].try_into().unwrap()),
        body,
    })
}

pub fn trace(dir: &str, b: &[u8]) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("FEATHER_TRACE").is_some()) {
        eprintln!("{dir} [{}] {}", b.len(), hex(&b[..b.len().min(96)]));
    }
}

pub fn send(s: &mut TcpStream, frame: &[u8]) -> Result<()> {
    trace(">>", frame);
    s.write_all(frame)?;
    Ok(())
}

pub fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

pub fn be16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes(b[off..off + 2].try_into().unwrap())
}

/// NUL-terminated, space-trimmed string from a fixed field.
pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}
