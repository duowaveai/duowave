use anyhow::{bail, Result};

/// Ogg page header flags (RFC 3533).
pub const OGG_CONTINUED: u8 = 0x01;
pub const OGG_BOS: u8 = 0x02;
pub const OGG_EOS: u8 = 0x04;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Ogg checksum: CRC-32 with polynomial 0x04c11db7, unreflected, over the page with its checksum field zeroed.
pub fn ogg_crc(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |crc, &byte| (crc << 8) ^ CRC_TABLE[(((crc >> 24) ^ u32::from(byte)) & 0xff) as usize])
}

/// A page with explicit lacing values, e.g. a packet that continues on the next page.
pub fn raw_ogg_page(
    serial: u32,
    sequence: u32,
    granule: i64,
    flags: u8,
    lacing: &[u8],
    body: &[u8],
) -> Result<Vec<u8>> {
    if lacing.len() > 255 {
        bail!("an Ogg page holds at most 255 segments, got {}", lacing.len());
    }
    let mut page = Vec::with_capacity(27 + lacing.len() + body.len());
    page.extend_from_slice(b"OggS");
    page.push(0);
    page.push(flags);
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(lacing.len() as u8);
    page.extend_from_slice(lacing);
    page.extend_from_slice(body);
    let crc = ogg_crc(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    Ok(page)
}

/// A page whose packets all end on it (standard lacing: 255-byte segments, then the remainder).
pub fn ogg_page(serial: u32, sequence: u32, granule: i64, flags: u8, packets: &[&[u8]]) -> Result<Vec<u8>> {
    let mut lacing = Vec::new();
    for packet in packets {
        lacing.extend(std::iter::repeat_n(255u8, packet.len() / 255));
        lacing.push((packet.len() % 255) as u8);
    }
    raw_ogg_page(serial, sequence, granule, flags, &lacing, &packets.concat())
}
