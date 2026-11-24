//! Waking a Турбо over the home network (docs/plans/turbo-devices.md, owner 20.11, 5): «Клавиатура у Mac, а он спит.
//! Разбудить?» — on a «да» the centre sends the sleeping computer Wake-on-LAN's «magic packet» and waits for it.
//!
//! - **The Турбо** says its hardware addresses in its contract (`devices.details.wake.macs`): every network card that
//!   has an address of the maker's — the ones a system makes up (a Mac's `awdl0`, `llw0`, bridges, tunnels) have the
//!   «locally administered» bit and are left out, as are loopback and all-zero ones.
//! - **The centre** keeps them (`turbo_wake`, migration 0017) — a sleeping computer is not asked — and sends the packet
//!   to the home network's broadcast, on ports 9 and 7: six bytes of 0xFF and the address sixteen times.
//!
//! It wakes only what is set to wake: on a Mac «Пробуждать для доступа к сети», a cable or a Wi-Fi that keeps the
//! computer reachable while it sleeps; a laptop with its lid closed on battery does not wake. So a wake that does not
//! come ends in honest words — «Не проснулся. Разбудите его сами».

use std::net::{Ipv4Addr, SocketAddrV4};

/// The ports the packet goes to: 9 (discard) is the usual one, 7 (echo) some cards listen on instead.
const PORTS: [u16; 2] = [9, 7];

/// A hardware address `aa:bb:cc:dd:ee:ff` read into its six bytes; None when it does not read as one.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = text.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// The address as people and the contract write it.
pub fn mac_text(mac: &[u8; 6]) -> String {
    mac.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(":")
}

/// An address a card was made with: not all zero, not one the system made up (the «locally administered» bit), not a
/// group address.
pub fn burned_in(mac: &[u8; 6]) -> bool {
    mac.iter().any(|byte| *byte != 0) && mac[0] & 0x02 == 0 && mac[0] & 0x01 == 0
}

/// The «magic packet»: six bytes of 0xFF, then the address sixteen times.
pub fn magic_packet(mac: &[u8; 6]) -> [u8; 102] {
    let mut packet = [0xFFu8; 102];
    for copy in 0..16 {
        packet[6 + copy * 6..12 + copy * 6].copy_from_slice(mac);
    }
    packet
}

/// Sends the packet for each address to the home network's broadcast; the number of packets sent. An address that does
/// not read is passed by; none sent at all is an error in words.
pub async fn send(macs: &[String]) -> Result<usize, String> {
    let macs: Vec<[u8; 6]> = macs.iter().filter_map(|mac| parse_mac(mac)).filter(burned_in).collect();
    if macs.is_empty() {
        return Err(crate::tr!("Этот компьютер не сказал, как его будить по сети."));
    }
    let socket = tokio::net::UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| crate::tr!("Сигнал пробуждения не ушёл: {error}", error = error))?;
    socket.set_broadcast(true).map_err(|error| crate::tr!("Сигнал пробуждения не ушёл: {error}", error = error))?;
    let mut sent = 0;
    for mac in &macs {
        let packet = magic_packet(mac);
        for port in PORTS {
            if socket.send_to(&packet, SocketAddrV4::new(Ipv4Addr::BROADCAST, port)).await.is_ok() {
                sent += 1;
            }
        }
    }
    if sent == 0 {
        return Err(crate::tr!("Сигнал пробуждения не ушёл: сеть его не приняла."));
    }
    tracing::info!(target: "devices", "a wake sent to {} address(es)", macs.len());
    Ok(sent)
}

/// The station's build and Windows: no addresses (the station is never woken; Windows later). A computer on unix reads
/// its cards' (crates/duowave-pc, helpers::wake).
pub fn hardware_addresses() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_an_address() {
        assert_eq!(parse_mac("a4:83:E7:01:02:03"), Some([0xa4, 0x83, 0xe7, 1, 2, 3]));
        assert_eq!(parse_mac("a4-83-e7-01-02-03"), Some([0xa4, 0x83, 0xe7, 1, 2, 3]));
        assert_eq!(mac_text(&[0xa4, 0x83, 0xe7, 1, 2, 3]), "a4:83:e7:01:02:03");
        for wrong in
            ["", "a4:83:e7:01:02", "a4:83:e7:01:02:03:04", "zz:83:e7:01:02:03", "a483e7010203", "a4:83:e7:1:2:3"]
        {
            assert_eq!(parse_mac(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn only_a_cards_own_address_wakes_it() {
        assert!(burned_in(&[0xa4, 0x83, 0xe7, 1, 2, 3]));
        assert!(!burned_in(&[0; 6]), "all zero");
        assert!(!burned_in(&[0x36, 0x83, 0xe7, 1, 2, 3]), "made up by the system (awdl0, bridges)");
        assert!(!burned_in(&[0x01, 0x00, 0x5e, 1, 2, 3]), "a group address");
    }

    #[test]
    fn the_packet_is_six_ff_and_the_address_sixteen_times() {
        let mac = [0xa4, 0x83, 0xe7, 1, 2, 3];
        let packet = magic_packet(&mac);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        for copy in 0..16 {
            assert_eq!(&packet[6 + copy * 6..12 + copy * 6], &mac);
        }
    }

    #[tokio::test]
    async fn no_address_no_wake_said_in_words() {
        assert!(send(&[]).await.is_err());
        assert!(send(&["00:00:00:00:00:00".into(), "nonsense".into()]).await.is_err());
    }
}
