//! Control channel on TCP 53219: one request per connection.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde_json::Value;

use super::wire::{self, CONTROL_PORT, be32, cstr};
use crate::error::{Result, proto};
use crate::net::HostAddr;

const TIMEOUT: Duration = Duration::from_secs(5);

pub const CMD_REGISTER: u32 = 0x11;
pub const CMD_UNREGISTER: u32 = 0x12;
pub const CMD_DEVICE_NAME: u32 = 0x13;
pub const CMD_STATE: u32 = 0x30;
pub const CMD_JSON: u32 = 0x41;

pub const SUB_READ_USERS: u16 = 0x0100;
pub const SUB_WRITE_USERS: u16 = 0x0110;
pub const SUB_READ_TAPPED_PROFILE: u16 = 0x0200;
pub const SUB_ANNOUNCE: u16 = 0x0210;
pub const SUB_SELECT_PROFILE: u16 = 0x0610;

pub const INTENT_CONNECT: u32 = 0;
pub const INTENT_PAIR: u32 = 5;

pub struct Control {
    pub scanner: Ipv4Addr,
    pub host: HostAddr,
}

impl Control {
    pub fn new(scanner: Ipv4Addr, host: HostAddr) -> Self {
        Self { scanner, host }
    }

    fn call(&self, command: u32, body: &[u8]) -> Result<wire::Response> {
        let mut s = wire::connect(self.scanner, CONTROL_PORT, TIMEOUT)?;
        wire::send(&mut s, &wire::request(command, &self.host, None, body))?;
        wire::read_response(&mut s)
    }

    pub fn device_name(&self) -> Result<String> {
        let r = self.call(CMD_DEVICE_NAME, &[])?;
        Ok(cstr(&r.body[..r.body.len().min(64)]))
    }

    pub fn state(&self) -> Result<u32> {
        let r = self.call(CMD_STATE, &[])?;
        if r.body.len() < 4 {
            return Err(proto("short state reply"));
        }
        Ok(be32(&r.body, 0))
    }

    /// 0x11 register. `intent` 0 = connect, 5 = pair.
    pub fn register(&self, intent: u32, host_id: &[u8; 8], notify_port: u16) -> Result<i32> {
        let r = self.call(
            CMD_REGISTER,
            &register_body(intent, self.host.ip, notify_port, host_id),
        )?;
        Ok(r.code)
    }

    pub fn unregister(&self) -> Result<()> {
        let mut body = [0u8; 16];
        body[0..6].copy_from_slice(&self.host.mac);
        body[11] = 1;
        self.call(CMD_UNREGISTER, &body)?;
        Ok(())
    }

    /// 0x41 JSON call. Returns the parsed reply JSON (Null when the reply has none).
    pub fn json(&self, sub: u16, payload: Option<&Value>) -> Result<Value> {
        let json = payload
            .map(|v| serde_json::to_vec_pretty(v).unwrap())
            .unwrap_or_default();
        let mut body = Vec::with_capacity(20 + json.len());
        body.extend_from_slice(&((16 + json.len()) as u32).to_be_bytes());
        body.extend_from_slice(&sub.to_be_bytes());
        body.extend_from_slice(&[0; 10]);
        body.extend_from_slice(&(json.len() as u32).to_le_bytes());
        body.extend_from_slice(&json);
        let r = self.call(CMD_JSON, &body)?;
        parse_json_reply(&r.body)
    }

    pub fn read_users(&self) -> Result<Value> {
        self.json(SUB_READ_USERS, None)
    }

    pub fn read_tapped_profile(&self) -> Result<Value> {
        self.json(SUB_READ_TAPPED_PROFILE, None)
    }

    pub fn select_profile(&self, prof_id: &str) -> Result<()> {
        self.json(
            SUB_SELECT_PROFILE,
            Some(&serde_json::json!({ "version": 1, "prof_id": prof_id })),
        )?;
        Ok(())
    }

    pub fn announce(&self, user_id: &str, notify_port: u16) -> Result<()> {
        let v = serde_json::json!({
            "host_address": self.host.ip.to_string(),
            "function_level": 2,
            "current_user_id": user_id,
            "host_port": notify_port,
            "version": 1,
        });
        self.json(SUB_ANNOUNCE, Some(&v))?;
        Ok(())
    }
}

/// Reply body: `u32 len | u16 sub | 10 zero | u32 LE json len | json` (JSON at offset 20).
fn parse_json_reply(body: &[u8]) -> Result<Value> {
    if body.len() < 20 {
        return Ok(Value::Null);
    }
    let jlen = u32::from_le_bytes(body[16..20].try_into().unwrap()) as usize;
    if jlen == 0 {
        return Ok(Value::Null);
    }
    let j = body
        .get(20..20 + jlen)
        .ok_or_else(|| proto("truncated JSON reply"))?;
    serde_json::from_slice(j).map_err(|e| proto(format!("bad JSON reply: {e}")))
}

pub fn register_body(intent: u32, ip: Ipv4Addr, notify_port: u16, host_id: &[u8; 8]) -> Vec<u8> {
    let mut b = vec![0u8; 352];
    b[0..4].copy_from_slice(&[0x01, 0x00, 0x1e, 0x00]);
    b[4..8].copy_from_slice(&intent.to_be_bytes());
    b[12..16].copy_from_slice(&ip.octets());
    b[16..20].copy_from_slice(&(notify_port as u32).to_be_bytes());
    let t = crate::time::local_now();
    b[68..70].copy_from_slice(&(t.year as u16).to_be_bytes());
    b[70] = t.month;
    b[71] = t.day;
    b[72] = t.hour;
    b[73] = t.minute;
    b[74] = t.second;
    b[76..84].copy_from_slice(host_id);
    b[84..88].copy_from_slice(&(-t.utc_offset_secs).to_be_bytes());
    b
}

pub fn parse_host_id(hex: &str) -> Option<[u8; 8]> {
    if hex.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}
