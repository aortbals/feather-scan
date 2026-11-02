//! Pairing a new computer with the scanner, without ScanSnap Home.
//!
//! The scanner keeps one list of computers ("users") and their profiles. Pairing
//! adds our own user and one profile to that list, then the person picks the new
//! computer on the touch panel. Writing the list (0x41/0x0110) replaces all of
//! it, so we always read it first and change only our own entries.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::config::Identity;
use crate::error::{Error, Result, proto};
use crate::net::{self, HostAddr};
use crate::proto::control::{self, Control, INTENT_CONNECT, INTENT_PAIR};
use crate::proto::scsi::ScsiChannel;
use crate::proto::wire::NOTIFY_PORT;

/// The scanner rejects longer user names (30 is fine, 31 is not).
pub const MAX_NAME_LEN: usize = 30;
pub const PROFILE_NAME: &str = "Feather Scan";
const NAME_PREFIX: &str = "Feather Scan@";

/// C2 state byte while pairing.
const STATE_PAIRING: u8 = 0xe1;

#[derive(Debug, Clone)]
pub struct Paired {
    pub identity: Identity,
    pub profile_id: String,
    /// Result of the final 0x11 (intent 0). Negative until the person selects this
    /// computer on the touch panel.
    pub connect_code: i32,
    /// Where the scanner's list was saved before it was changed.
    pub backup: Option<PathBuf>,
}

impl Paired {
    pub fn instructions(&self) -> String {
        format!(
            "Paired. On the scanner's screen, tap the person icon and pick \"{}\".",
            self.identity.user_name
        )
    }
}

/// A fresh identity named after this Mac, or the given one if Feather Scan made it
/// (so pairing again replaces our entry instead of adding another).
pub fn identity_for_pairing(existing: Option<&Identity>) -> Result<Identity> {
    if let Some(id) = existing.filter(|id| is_ours(id)) {
        return Ok(id.clone());
    }
    let mut host_id = [0u8; 8];
    let mut user_id = [0u8; 16];
    random(&mut host_id)?;
    random(&mut user_id)?;
    Ok(Identity {
        host_id: hex_lower(&host_id),
        user_id: hex_lower(&user_id).to_uppercase(),
        user_name: user_name(&short_hostname()),
    })
}

fn is_ours(id: &Identity) -> bool {
    id.user_name.starts_with(NAME_PREFIX) && control::parse_host_id(&id.host_id).is_some()
}

/// `Feather Scan@<host>`, ASCII only, at most 30 characters.
pub fn user_name(host: &str) -> String {
    let clean: String = host
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let host = if clean.is_empty() { "Mac" } else { &clean };
    let mut name = format!("{NAME_PREFIX}{host}");
    name.truncate(MAX_NAME_LEN);
    name
}

fn short_hostname() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return String::new();
    }
    let full = crate::proto::wire::cstr(&buf);
    full.split('.').next().unwrap_or("").to_string()
}

fn random(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Builds the list to write back: everything in `current` (every user, every
/// profile, every field, `select` and `version` untouched), plus our user and
/// profile. If we are already in the list, our entries are replaced in place;
/// the scanner rejects a reordered user list. The listed profiles belong to the
/// computer selected on the panel, so ours is only added when that is us (or
/// the list is empty, as it is during the handshake after a panel switch).
///
/// Returns the new list and our profile ID.
pub fn merge(
    current: &Value,
    me: &Identity,
    host_ip: Ipv4Addr,
    new_prof_id: &str,
) -> Result<(Value, String)> {
    let mut out = current.clone();
    let root = out
        .as_object_mut()
        .ok_or_else(|| proto("user list is not a JSON object"))?;
    let conn_user = root
        .entry("conn_user")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| proto("conn_user is not an object"))?;
    let users = conn_user
        .entry("users")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| proto("conn_user.users is not a list"))?;

    // Our user: an existing entry's shape, with our name and host ID.
    let template = users.iter().find(|u| u.is_object()).cloned();
    let mut user = template
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for (k, v) in [
        ("name", json!(me.user_name)),
        ("app_ver", json!(1)),
        ("prof_stat", json!(1)),
        ("scloud_user", json!("")),
        ("host_id", json!(me.host_id)),
        ("icon_id", json!(9)),
        ("color_id", json!(2)),
    ] {
        user.insert(k.into(), v);
    }
    let color_id = user["color_id"].clone();
    match users
        .iter_mut()
        .find(|u| u["host_id"].as_str() == Some(&me.host_id))
    {
        Some(slot) => *slot = Value::Object(user),
        None => users.push(Value::Object(user)),
    }

    let selected_is_us = conn_user
        .get("select")
        .and_then(|s| s["id"].as_str())
        .is_some_and(|id| id == me.host_id);

    let profiles = root
        .entry("profiles")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| proto("profiles is not a list"))?;
    let mine = profiles
        .iter()
        .position(|p| p["user_id"].as_str() == Some(&me.user_id));
    let prof_id = mine
        .and_then(|i| profiles[i]["prof_id"].as_str().map(String::from))
        .unwrap_or_else(|| new_prof_id.to_string());
    // The listed profiles belong to the computer selected on the panel. Don't add
    // ours to another computer's; it is pushed when this computer is selected.
    if mine.is_none() && !selected_is_us && !profiles.is_empty() {
        return Ok((out, prof_id));
    }
    let base = mine
        .map(|i| profiles[i].clone())
        .or_else(|| first_by_order(profiles))
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_else(default_profile);
    let next_order = profiles
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != mine)
        .filter_map(|(_, p)| p["disp_order"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let mut prof = base;
    for (k, v) in [
        ("prof_id", json!(prof_id)),
        ("prof_name", json!(PROFILE_NAME)),
        ("prof_desc", json!("")),
        ("user_id", json!(me.user_id)),
        ("user_name", json!(me.user_name)),
        ("host_address", json!(host_ip.to_string())),
        ("color_id", color_id),
        ("isScanSnapCloud", json!(false)),
    ] {
        prof.insert(k.into(), v);
    }
    if mine.is_none() {
        prof.insert("disp_order".into(), json!(next_order));
    }
    match mine {
        Some(i) => profiles[i] = Value::Object(prof),
        None => profiles.push(Value::Object(prof)),
    }
    Ok((out, prof_id))
}

/// The profile shown first on the panel: the best model for ours.
fn first_by_order(profiles: &[Value]) -> Option<Value> {
    profiles
        .iter()
        .filter(|p| p.is_object())
        .min_by_key(|p| p["disp_order"].as_i64().unwrap_or(i64::MAX))
        .cloned()
}

/// Used only when the scanner has no profile at all to copy.
fn default_profile() -> Map<String, Value> {
    let scan = |doc: &str, handling: &str| {
        json!({
            "color_mode": "auto",
            "color_mode_select": ["auto", "color", "gray", "monochrome"],
            "doc_type": doc,
            "long_page": true,
            "paper_handling": handling,
            "paper_handling_select": ["both", "single"],
            "resolution": "auto",
            "resolution_select": ["auto", "normal", "fine", "superfine", "excellent"],
            "service_name": ""
        })
    };
    let v = json!({
        "classification_type": 6,
        "color_id": 1,
        "disp_order": 1,
        "feed": "normal",
        "feed_select": ["normal", "continuous", "manual"],
        "host_address": "",
        "icon_id": 1,
        "isScanSnapCloud": false,
        "operate_limit": 0,
        "prof_desc": "",
        "prof_id": "",
        "prof_name": PROFILE_NAME,
        "prof_revision": 0,
        "prof_type": 0,
        "scan_info": [
            scan("document", "both"),
            scan("card", "both"),
            scan("receipt", "single"),
            scan("photo", "both")
        ],
        "scan_limit": 0,
        "sdk_prof": 0,
        "use_default": false,
        "user_id": "",
        "user_name": ""
    });
    v.as_object().cloned().unwrap()
}

/// Where copies of the scanner's list are kept before pairing changes it.
pub fn backup_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/Feather Scan/backups")
}

pub fn save_backup(list: &Value) -> Result<PathBuf> {
    let dir = backup_dir();
    std::fs::create_dir_all(&dir)?;
    let t = crate::time::local_now();
    let path = dir.join(format!(
        "users-{:04}{:02}{:02}-{:02}{:02}{:02}.json",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    ));
    std::fs::write(&path, serde_json::to_vec_pretty(list).unwrap())?;
    Ok(path)
}

/// Pairs this computer with the scanner at `scanner`: adds our user and profile
/// to the scanner's list. Nobody else may hold the scanner meanwhile. The list
/// read before the write is saved with [`save_backup`] first.
pub fn pair(scanner: Ipv4Addr, me: &Identity, log: &dyn Fn(&str)) -> Result<Paired> {
    let host_id = control::parse_host_id(&me.host_id)
        .ok_or_else(|| Error::Config(format!("bad host id {}", me.host_id)))?;
    let mut host: HostAddr = net::host_addr_for(scanner)?;
    if net::is_masked(&host.mac) {
        host.mac = net::mac_from_host_id(&host_id);
    }
    let ctl = Control::new(scanner, host);
    let mut scsi = ScsiChannel::open(scanner, host)?;
    let h = handshake(&ctl, &mut scsi, me, true, log);
    ctl.unregister_quietly();
    drop(scsi);
    let h = h?;
    Ok(Paired {
        identity: me.clone(),
        profile_id: h.profile_id,
        connect_code: h.connect_code,
        backup: h.backup,
    })
}

pub struct Handshake {
    pub profile_id: String,
    /// Result of the final 0x11 intent 0 (negative: not selected on the panel).
    pub connect_code: i32,
    pub backup: Option<PathBuf>,
}

/// The pairing-mode handshake, as ScanSnap Home runs it when its computer is
/// picked on the touch panel (captured) and, with the same steps, when it pairs:
///
/// 53218 INQUIRY, then 53219: 0x13 → 0x0100 → 0x0210 → 0x12 → 0x11 (intent 5) →
/// 0x0100 → 0x0110 → 0x12 → 0x11 (intent 0) → 0x30.
///
/// The 0x0110 write keeps every user and every profile the scanner reports and
/// adds or refreshes ours. While pairing, the scanner reports only the selected
/// computer's profiles (none, right after a panel switch): each computer pushes
/// its own profiles when it is selected.
pub fn handshake(
    ctl: &Control,
    scsi: &mut ScsiChannel,
    me: &Identity,
    backup: bool,
    log: &dyn Fn(&str),
) -> Result<Handshake> {
    let host_id = control::parse_host_id(&me.host_id)
        .ok_or_else(|| Error::Config(format!("bad host id {}", me.host_id)))?;
    scsi.inquiry()?;
    let _ = scsi.hw_status();
    let name = ctl.device_name()?;
    log(&format!("Pairing with {name} as \"{}\"…", me.user_name));
    let _ = ctl.read_users()?;
    ctl.announce(&me.user_id, NOTIFY_PORT)?;
    ctl.unregister_quietly();
    // The scanner answers -4 (busy) for a while after another session ends.
    let mut st = ctl.register(INTENT_PAIR, &host_id, NOTIFY_PORT)?;
    for _ in 0..10 {
        if st >= 0 {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
        let _ = scsi.hw_status();
        st = ctl.register(INTENT_PAIR, &host_id, NOTIFY_PORT)?;
    }
    if st < 0 {
        return Err(proto(format!(
            "scanner refused to pair ({st}). Quit ScanSnap Home and other scanner apps, then try again."
        )));
    }
    if let Ok(hw) = scsi.hw_status()
        && hw.state != STATE_PAIRING
    {
        log(&format!(
            "note: scanner state is {:#04x}, expected {STATE_PAIRING:#04x}",
            hw.state
        ));
    }

    let result = (|| {
        let current = ctl.read_users()?;
        if !current.is_object() {
            return Err(proto("scanner sent no user list; nothing was changed"));
        }
        let saved = if backup {
            let path = save_backup(&current)?;
            log(&format!("Saved the scanner's list to {}", path.display()));
            Some(path)
        } else {
            None
        };
        let host_ip = ctl.host.ip;
        let (list, prof_id) = merge(&current, me, host_ip, &profile_id_for(&me.user_id))?;
        ctl.write_users(&list)?;
        Ok((prof_id, saved))
    })();

    // Always leave the pairing state, or the scanner refuses normal sessions.
    let _ = scsi.hw_status();
    ctl.unregister_quietly();
    let mut connect_code = ctl
        .register(INTENT_CONNECT, &host_id, NOTIFY_PORT)
        .unwrap_or(-1);
    if connect_code == -4 {
        std::thread::sleep(Duration::from_millis(300));
        connect_code = ctl
            .register(INTENT_CONNECT, &host_id, NOTIFY_PORT)
            .unwrap_or(-1);
    }
    let _ = ctl.state();
    let (profile_id, backup) = result?;
    Ok(Handshake {
        profile_id,
        connect_code,
        backup,
    })
}

/// Our profile's ID, derived from the user ID so it stays the same every time
/// we push it (the scanner forgets it whenever another computer is selected).
pub fn profile_id_for(user_id: &str) -> String {
    let fnv = |seed: u64| {
        user_id.bytes().chain(*b"/profile").fold(seed, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
        })
    };
    format!(
        "{:016X}{:016X}",
        fnv(0xcbf2_9ce4_8422_2325),
        fnv(0x8422_2325_cbf2_9ce4)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity {
            host_id: "0123456789abcdef".into(),
            user_id: "AAAABBBBCCCCDDDDEEEEFFFF00001111".into(),
            user_name: "Feather Scan@test".into(),
        }
    }

    fn ip() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, 50)
    }

    fn sample() -> Value {
        json!({
            "conn_user": {
                "users": [
                    {"name": "alice@laptop", "app_ver": 1, "prof_stat": 1, "scloud_user": "",
                     "host_id": "1111111111111111", "icon_id": 1, "color_id": 1, "future": [1, 2]},
                    {"name": "bob@desktop", "app_ver": 1, "prof_stat": 1, "scloud_user": "x",
                     "host_id": "2222222222222222", "icon_id": 3, "color_id": 4}
                ],
                "select": {"id": "1111111111111111", "kind": 1},
                "unknown_conn": true
            },
            "profiles": [
                {"prof_id": "P2", "prof_name": "Scan to Folder", "disp_order": 2, "user_id": "ALICE",
                 "user_name": "alice@laptop", "host_address": "192.168.1.10", "scan_info": [{"doc_type": "document"}],
                 "use_default": true, "mystery": {"a": 1}},
                {"prof_id": "P1", "prof_name": "ScanSnap Home", "disp_order": 1, "user_id": "ALICE",
                 "user_name": "alice@laptop", "host_address": "192.168.1.10", "scan_info": [{"doc_type": "photo"}],
                 "use_default": true, "feed": "normal"}
            ],
            "version": 3,
            "top_level_extra": "keep me"
        })
    }

    /// Every entry and field of `before` is still in `after`, unchanged.
    fn assert_contains(before: &Value, after: &Value, path: &str) {
        match (before, after) {
            (Value::Object(b), Value::Object(a)) => {
                for (k, v) in b {
                    let av = a.get(k).unwrap_or_else(|| panic!("{path}.{k} dropped"));
                    assert_contains(v, av, &format!("{path}.{k}"));
                }
            }
            (Value::Array(b), Value::Array(a)) => {
                assert!(a.len() >= b.len(), "{path} lost entries");
                for (i, v) in b.iter().enumerate() {
                    assert_contains(v, &a[i], &format!("{path}[{i}]"));
                }
            }
            _ => assert_eq!(before, after, "{path} changed"),
        }
    }

    /// `sample()` with this computer selected on the panel.
    fn sample_selected() -> Value {
        let mut v = sample();
        v["conn_user"]["select"]["id"] = json!(me().host_id);
        v
    }

    #[test]
    fn merge_keeps_every_existing_entry_and_field() {
        for before in [sample(), sample_selected()] {
            let (after, prof_id) = merge(&before, &me(), ip(), "NEWPROF").unwrap();
            assert_eq!(prof_id, "NEWPROF");
            assert_contains(&before, &after, "$");
            let users = after["conn_user"]["users"].as_array().unwrap();
            assert_eq!(users.len(), 3);
            assert_eq!(after["conn_user"]["select"], before["conn_user"]["select"]);
        }
    }

    #[test]
    fn merge_adds_no_profile_to_another_computers_list() {
        let (after, _) = merge(&sample(), &me(), ip(), "NEWPROF").unwrap();
        assert_eq!(after["conn_user"]["users"].as_array().unwrap().len(), 3);
        assert_eq!(after["profiles"], sample()["profiles"]);
    }

    #[test]
    fn merge_after_panel_switch_pushes_only_our_profile() {
        // While pairing right after a panel switch, the scanner lists no profiles.
        let mut v = sample_selected();
        v["profiles"] = json!([]);
        let (after, _) = merge(&v, &me(), ip(), "P").unwrap();
        let profiles = after["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0]["user_id"], me().user_id);
        assert_eq!(profiles[0]["prof_id"], "P");
        assert_contains(&v["conn_user"], &after["conn_user"], "$.conn_user");
    }

    #[test]
    fn merge_appends_our_user_and_profile() {
        let (after, _) = merge(&sample_selected(), &me(), ip(), "NEWPROF").unwrap();
        let u = &after["conn_user"]["users"][2];
        assert_eq!(u["name"], "Feather Scan@test");
        assert_eq!(u["host_id"], "0123456789abcdef");
        assert_eq!(u["scloud_user"], "");
        // Shape copied from an existing user, including fields we don't know.
        assert_eq!(u["future"], json!([1, 2]));

        let p = &after["profiles"][2];
        assert_eq!(p["prof_id"], "NEWPROF");
        assert_eq!(p["prof_name"], PROFILE_NAME);
        assert_eq!(p["user_id"], me().user_id);
        assert_eq!(p["user_name"], me().user_name);
        assert_eq!(p["host_address"], "192.168.1.50");
        assert_eq!(p["disp_order"], 3);
        // Modelled on the first profile by display order.
        assert_eq!(p["scan_info"][0]["doc_type"], "photo");
        assert_eq!(p["feed"], "normal");
    }

    #[test]
    fn merge_again_replaces_our_entries_in_place() {
        let (once, _) = merge(&sample_selected(), &me(), ip(), "FIRST").unwrap();
        let (twice, prof_id) = merge(&once, &me(), Ipv4Addr::new(10, 0, 0, 9), "SECOND").unwrap();
        assert_eq!(prof_id, "FIRST", "keeps our existing profile ID");
        assert_eq!(twice["conn_user"]["users"].as_array().unwrap().len(), 3);
        assert_eq!(twice["profiles"].as_array().unwrap().len(), 3);
        assert_eq!(twice["profiles"][2]["host_address"], "10.0.0.9");
        assert_eq!(twice["profiles"][2]["disp_order"], 3);
        assert_contains(&sample_selected(), &twice, "$");
    }

    #[test]
    fn merge_keeps_user_order_when_we_are_first() {
        let mut v = sample();
        v["conn_user"]["users"].as_array_mut().unwrap().insert(
            0,
            json!({"name": "old", "host_id": me().host_id, "kept": 1}),
        );
        let (after, _) = merge(&v, &me(), ip(), "N").unwrap();
        let ids: Vec<_> = after["conn_user"]["users"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["host_id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            ids,
            [
                me().host_id.as_str(),
                "1111111111111111",
                "2222222222222222"
            ]
        );
        assert_eq!(after["conn_user"]["users"][0]["name"], me().user_name);
    }

    #[test]
    fn merge_preserves_field_order() {
        let before = sample();
        let (after, _) = merge(&before, &me(), ip(), "N").unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&before), keys(&after));
        assert_eq!(keys(&before["profiles"][0]), keys(&after["profiles"][0]));
    }

    #[test]
    fn merge_into_empty_list_uses_default_profile() {
        let (after, _) = merge(&json!({}), &me(), ip(), "N").unwrap();
        assert_eq!(after["conn_user"]["users"].as_array().unwrap().len(), 1);
        let p = &after["profiles"][0];
        assert_eq!(p["prof_id"], "N");
        assert_eq!(p["scan_info"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn merge_rejects_non_object() {
        assert!(merge(&Value::Null, &me(), ip(), "N").is_err());
    }

    #[test]
    fn profile_id_is_stable_hex() {
        let a = profile_id_for(&me().user_id);
        assert_eq!(a, profile_id_for(&me().user_id));
        assert_eq!(a.len(), 32);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        );
        assert_ne!(a, profile_id_for("OTHER"));
    }

    #[test]
    fn user_name_fits_scanner_limit() {
        assert_eq!(user_name("studio"), "Feather Scan@studio");
        let long = user_name("a-very-long-hostname-indeed-yes");
        assert_eq!(long.len(), MAX_NAME_LEN);
        assert_eq!(user_name("Jane’s Mac"), "Feather Scan@JanesMac");
        assert_eq!(user_name(""), "Feather Scan@Mac");
    }

    #[test]
    fn new_identity_shape() {
        let id = identity_for_pairing(None).unwrap();
        assert_eq!(id.host_id.len(), 16);
        assert!(control::parse_host_id(&id.host_id).is_some());
        assert_eq!(id.user_id.len(), 32);
        assert!(
            id.user_id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        );
        assert!(id.user_name.starts_with(NAME_PREFIX));
        // Reuses an identity Feather Scan made; ignores a ScanSnap Home one.
        assert_eq!(identity_for_pairing(Some(&id)).unwrap(), id);
        let ssh = Identity {
            user_name: "someone@mac".into(),
            ..id.clone()
        };
        assert_ne!(
            identity_for_pairing(Some(&ssh)).unwrap().host_id,
            id.host_id
        );
    }
}
