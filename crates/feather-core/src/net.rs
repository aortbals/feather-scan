//! Host identity on the LAN: which local IP and MAC we use to reach a scanner.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostAddr {
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
}

/// The local address the OS would use to reach `target`, plus that interface's MAC.
pub fn host_addr_for(target: Ipv4Addr) -> Result<HostAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.connect(SocketAddrV4::new(target, 52217))?;
    let ip = match sock.local_addr()? {
        std::net::SocketAddr::V4(a) => *a.ip(),
        _ => return Err(crate::error::proto("no IPv4 route to scanner")),
    };
    let mac = mac_for_ip(ip).unwrap_or([0; 6]);
    Ok(HostAddr { ip, mac })
}

/// Any non-loopback IPv4 address with its MAC, for broadcast discovery.
pub fn primary_host_addr() -> Option<HostAddr> {
    interfaces()
        .into_iter()
        .find(|(_, ip, _)| !ip.is_loopback() && !ip.is_link_local())
        .map(|(_, ip, mac)| HostAddr {
            ip,
            mac: mac.unwrap_or([0; 6]),
        })
}

fn mac_for_ip(ip: Ipv4Addr) -> Option<[u8; 6]> {
    interfaces()
        .into_iter()
        .find(|(_, a, _)| *a == ip)
        .and_then(|(_, _, m)| m)
}

/// (interface name, IPv4, MAC) for every interface with an IPv4 address.
fn interfaces() -> Vec<(String, Ipv4Addr, Option<[u8; 6]>)> {
    let mut v4: Vec<(String, Ipv4Addr)> = Vec::new();
    let mut macs: Vec<(String, [u8; 6])> = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return Vec::new();
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_flags & libc::IFF_UP as u32 == 0 {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(ifa.ifa_name)
                .to_string_lossy()
                .into_owned();
            let family = (*ifa.ifa_addr).sa_family as i32;
            if family == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                v4.push((name, Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))));
            } else if let Some(mac) = link_mac(ifa.ifa_addr, family) {
                macs.push((name, mac));
            }
        }
        libc::freeifaddrs(ifap);
    }
    v4.into_iter()
        .map(|(n, ip)| {
            let mac = macs.iter().find(|(m, _)| *m == n).map(|(_, mac)| *mac);
            (n, ip, mac)
        })
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
unsafe fn link_mac(addr: *const libc::sockaddr, family: i32) -> Option<[u8; 6]> {
    if family != libc::AF_LINK {
        return None;
    }
    let sdl = unsafe { &*(addr as *const libc::sockaddr_dl) };
    if sdl.sdl_alen != 6 {
        return None;
    }
    let base = sdl.sdl_data.as_ptr() as *const u8;
    let mut mac = [0u8; 6];
    unsafe { std::ptr::copy_nonoverlapping(base.add(sdl.sdl_nlen as usize), mac.as_mut_ptr(), 6) };
    Some(mac)
}

#[cfg(target_os = "linux")]
unsafe fn link_mac(addr: *const libc::sockaddr, family: i32) -> Option<[u8; 6]> {
    if family != libc::AF_PACKET {
        return None;
    }
    let sll = unsafe { &*(addr as *const libc::sockaddr_ll) };
    if sll.sll_halen != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&sll.sll_addr[..6]);
    Some(mac)
}

/// macOS hides link-layer addresses from unprivileged apps (they read as 02:00:00:00:00:00).
pub fn is_masked(mac: &[u8; 6]) -> bool {
    *mac == [0; 6] || *mac == [2, 0, 0, 0, 0, 0]
}

/// A stable, locally administered MAC derived from the pairing's host ID.
/// The scanner only uses the MAC to tell sessions apart.
pub fn mac_from_host_id(host_id: &[u8; 8]) -> [u8; 6] {
    let mut m = [0u8; 6];
    m.copy_from_slice(&host_id[2..8]);
    m[0] = (host_id[0] & 0xfc) | 0x02;
    m
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
