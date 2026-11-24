//! Waking a sleeping Турбо — the computer's part, added to «Дом»'s `helpers::wake`: this computer's cards' hardware
//! addresses for its centre to wake it by (unix; elsewhere none, as on the station).

pub use duowave_home::helpers::wake::*;

/// This computer's hardware addresses for its centre to wake it by: its cards' burned-in ones, each once.
#[cfg(unix)]
pub fn hardware_addresses() -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    // SAFETY: getifaddrs gives a list this function frees with freeifaddrs before it returns; each entry's address is
    // read only as far as its own family's struct says, and only while the list lives.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut entry = list;
        while !entry.is_null() {
            let item = &*entry;
            entry = item.ifa_next;
            if item.ifa_addr.is_null() || item.ifa_flags & (libc::IFF_LOOPBACK as libc::c_uint) != 0 {
                continue;
            }
            if let Some(mac) = link_address(item.ifa_addr) {
                let text = mac_text(&mac);
                if burned_in(&mac) && !found.contains(&text) {
                    found.push(text);
                }
            }
        }
        libc::freeifaddrs(list);
    }
    found
}

/// The hardware address of a link-level entry: `AF_LINK` on a Mac.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
unsafe fn link_address(address: *const libc::sockaddr) -> Option<[u8; 6]> {
    if i32::from((*address).sa_family) != libc::AF_LINK {
        return None;
    }
    let link = &*(address as *const libc::sockaddr_dl);
    if link.sdl_alen != 6 {
        return None;
    }
    let start = usize::from(link.sdl_nlen);
    let data: &[libc::c_char] = &link.sdl_data;
    let bytes: Vec<u8> = (0..6).filter_map(|at| data.get(start + at)).map(|byte| *byte as u8).collect();
    bytes.try_into().ok()
}

/// The hardware address of a link-level entry: `AF_PACKET` on Linux.
#[cfg(target_os = "linux")]
unsafe fn link_address(address: *const libc::sockaddr) -> Option<[u8; 6]> {
    if i32::from((*address).sa_family) != libc::AF_PACKET {
        return None;
    }
    let link = &*(address as *const libc::sockaddr_ll);
    if link.sll_halen != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&link.sll_addr[..6]);
    Some(mac)
}

/// Other systems: no hardware addresses read (the centre then cannot wake it).
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "linux"))))]
unsafe fn link_address(_address: *const libc::sockaddr) -> Option<[u8; 6]> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn this_computers_addresses_read_as_addresses() {
        for mac in hardware_addresses() {
            let parsed = parse_mac(&mac).expect("an address");
            assert!(burned_in(&parsed), "{mac}");
        }
    }
}
