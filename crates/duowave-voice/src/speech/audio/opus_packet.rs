use anyhow::{bail, Result};

// Frame length at 48 kHz by the TOC configuration (RFC 6716, section 3.1): SILK 10/20/40/60 ms, hybrid 10/20 ms,
// CELT 2.5/5/10/20 ms.
const SILK: [usize; 4] = [480, 960, 1920, 2880];
const HYBRID: [usize; 2] = [480, 960];
const CELT: [usize; 4] = [120, 240, 480, 960];

/// Samples at 48 kHz that one Opus packet decodes to, read from its TOC byte.
pub fn opus_packet_samples(packet: &[u8]) -> Result<usize> {
    let Some(&toc) = packet.first() else { bail!("empty Opus packet") };
    let config = usize::from(toc >> 3);
    let frame = if config < 12 {
        SILK[config & 3]
    } else if config < 16 {
        HYBRID[config & 1]
    } else {
        CELT[config & 3]
    };
    let frames = match toc & 3 {
        0 => 1,
        3 => packet.get(1).map_or(0, |count| usize::from(count & 0x3f)),
        _ => 2,
    };
    // A packet carries at most 120 ms.
    if frames == 0 || frames * frame > 5760 {
        bail!("invalid Opus packet duration (TOC 0x{toc:x})");
    }
    Ok(frames * frame)
}
