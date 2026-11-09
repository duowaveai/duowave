const CAPTURE: &[u8] = b"OggS";
const PAGE_HEADER: usize = 27;
const OPUS_HEAD: &[u8] = b"OpusHead";
const OPUS_TAGS: &[u8] = b"OpusTags";

/// Opus identification header (RFC 7845, section 5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct OpusHead {
    pub channels: u8,
    /// Samples at 48 kHz to drop from the start of the decoded stream.
    pub pre_skip: u16,
    pub input_sample_rate: u32,
    pub output_gain_db: f64,
    pub mapping_family: u8,
}

impl Default for OpusHead {
    /// What a stream without a head is read as: mono 16 kHz speech.
    fn default() -> OpusHead {
        OpusHead { channels: 1, pre_skip: 0, input_sample_rate: 16000, output_gain_db: 0.0, mapping_family: 0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OggOpusEvent {
    Head { chain: usize, head: OpusHead },
    Packet { chain: usize, data: Vec<u8> },
}

fn parse_opus_head(packet: &[u8]) -> Option<OpusHead> {
    if packet.len() < 19 {
        return None;
    }
    Some(OpusHead {
        channels: packet[9],
        pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
        input_sample_rate: u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]),
        output_gain_db: f64::from(i16::from_le_bytes([packet[16], packet[17]])) / 256.0,
        mapping_family: packet[18],
    })
}

/// Incremental Ogg demuxer for the station's Opus audio. Bytes arrive in arbitrary chunks; packets come out in
/// order. The station restarts the stream after SpotterEnd: a new BOS page with a new OpusHead under the same
/// serial number and without an EOS page. So a new chain is recognised by its OpusHead, not by a serial number
/// change.
#[derive(Debug, Default)]
pub struct OggOpusDemuxer {
    /// Heads seen: the current chain is one less (0 for the wake word buffer, 1 for the command after SpotterEnd).
    heads: usize,
    pages: u64,
    skipped_bytes: u64,
    pending: Vec<u8>,
    partial: Vec<u8>,
}

impl OggOpusDemuxer {
    pub fn new() -> OggOpusDemuxer {
        OggOpusDemuxer::default()
    }

    /// Index of the current chain; None before the first OpusHead.
    pub fn chain(&self) -> Option<usize> {
        self.heads.checked_sub(1)
    }

    pub fn pages(&self) -> u64 {
        self.pages
    }

    /// Bytes thrown away while looking for the next page; non-zero means the stream was damaged.
    pub fn skipped_bytes(&self) -> u64 {
        self.skipped_bytes
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<OggOpusEvent> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut at = 0;
        loop {
            let rest = &self.pending[at..];
            let Some(start) = rest.windows(CAPTURE.len()).position(|window| window == CAPTURE) else {
                // Keep a possible beginning of "OggS" split between chunks.
                let keep = rest.len().min(CAPTURE.len() - 1);
                self.skipped_bytes += (rest.len() - keep) as u64;
                at = self.pending.len() - keep;
                break;
            };
            self.skipped_bytes += start as u64;
            at += start;
            let rest = &self.pending[at..];
            if rest.len() < PAGE_HEADER {
                break;
            }
            if rest[4] != 0 {
                // Not a page after all (unknown stream structure version): resync after this byte.
                self.skipped_bytes += 1;
                at += 1;
                continue;
            }
            let segments = usize::from(rest[26]);
            let header_length = PAGE_HEADER + segments;
            if rest.len() < header_length {
                break;
            }
            let body_length: usize = rest[PAGE_HEADER..header_length].iter().map(|&size| usize::from(size)).sum();
            if rest.len() < header_length + body_length {
                break;
            }
            let page = &rest[..header_length + body_length];
            self.pages += 1;
            read_page(page, segments, &mut self.partial, &mut self.heads, &mut events);
            at += header_length + body_length;
        }
        self.pending.drain(..at);
        events
    }
}

fn read_page(page: &[u8], segments: usize, partial: &mut Vec<u8>, heads: &mut usize, events: &mut Vec<OggOpusEvent>) {
    let continued = page[5] & 0x01 != 0;
    // A packet left unfinished on the previous page is lost when this page does not continue it.
    if !continued {
        partial.clear();
    }
    let mut offset = PAGE_HEADER + segments;
    for &size in &page[PAGE_HEADER..PAGE_HEADER + segments] {
        let size = usize::from(size);
        partial.extend_from_slice(&page[offset..offset + size]);
        offset += size;
        if size < 255 {
            emit(std::mem::take(partial), heads, events);
        }
    }
}

fn emit(packet: Vec<u8>, heads: &mut usize, events: &mut Vec<OggOpusEvent>) {
    if packet.starts_with(OPUS_HEAD) {
        if let Some(head) = parse_opus_head(&packet) {
            *heads += 1;
            events.push(OggOpusEvent::Head { chain: *heads - 1, head });
            return;
        }
    }
    if packet.starts_with(OPUS_TAGS) || packet.is_empty() {
        return;
    }
    events.push(OggOpusEvent::Packet { chain: heads.saturating_sub(1), data: packet });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::test_audio::{ogg_page, opus_head, raw_ogg_page};

    const SERIAL: u32 = 0x53dd_2703;

    fn tags() -> Vec<u8> {
        [b"OpusTags".as_slice(), &[0u8; 8]].concat()
    }

    fn packet(size: usize, fill: u8) -> Vec<u8> {
        vec![fill; size]
    }

    fn chain(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = ogg_page(SERIAL, 0, 0, 0x02, &[&opus_head(0)]).unwrap();
        bytes.extend(ogg_page(SERIAL, 1, 0, 0, &[&tags()]).unwrap());
        for (index, data) in packets.iter().enumerate() {
            bytes.extend(ogg_page(SERIAL, index as u32 + 2, (index as i64 + 1) * 960, 0, &[data]).unwrap());
        }
        bytes
    }

    fn feed(demuxer: &mut OggOpusDemuxer, bytes: &[u8], chunk: usize) -> Vec<OggOpusEvent> {
        bytes.chunks(chunk).flat_map(|piece| demuxer.push(piece)).collect()
    }

    #[test]
    fn finds_the_restarted_chain_under_the_same_serial_number_whatever_the_chunking() {
        let stream =
            [chain(&[packet(30, 1), packet(40, 2), packet(35, 3)]), chain(&[packet(20, 4), packet(25, 5)])].concat();
        for chunk_size in [1, 7, 100, stream.len()] {
            let mut demuxer = OggOpusDemuxer::new();
            let events = feed(&mut demuxer, &stream, chunk_size);
            let seen: Vec<String> = events
                .iter()
                .map(|event| match event {
                    OggOpusEvent::Head { chain, .. } => format!("head:{chain}"),
                    OggOpusEvent::Packet { chain, data } => format!("packet:{chain}:{}/{}", data[0], data.len()),
                })
                .collect();
            assert_eq!(
                seen,
                [
                    "head:0",
                    "packet:0:1/30",
                    "packet:0:2/40",
                    "packet:0:3/35",
                    "head:1",
                    "packet:1:4/20",
                    "packet:1:5/25"
                ]
            );
            assert_eq!(demuxer.skipped_bytes(), 0);
        }
    }

    #[test]
    fn parses_the_opus_head_fields() {
        let events = OggOpusDemuxer::new().push(&ogg_page(SERIAL, 0, 0, 0x02, &[&opus_head(312)]).unwrap());
        assert_eq!(
            events[0],
            OggOpusEvent::Head {
                chain: 0,
                head: OpusHead {
                    channels: 1,
                    pre_skip: 312,
                    input_sample_rate: 16000,
                    output_gain_db: 0.0,
                    mapping_family: 0
                },
            }
        );
    }

    #[test]
    fn joins_a_packet_continued_on_the_next_page_and_skips_garbage_between_pages() {
        let big = packet(300, 7);
        let first = raw_ogg_page(SERIAL, 2, -1, 0, &[255], &big[..255]).unwrap();
        let second = raw_ogg_page(SERIAL, 3, 960, 0x01, &[45], &big[255..]).unwrap();
        let mut demuxer = OggOpusDemuxer::new();
        let head = ogg_page(SERIAL, 0, 0, 0x02, &[&opus_head(0)]).unwrap();
        let events = feed(&mut demuxer, &[head, b"junk".to_vec(), first, second].concat(), 13);
        let sizes: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                OggOpusEvent::Packet { data, .. } => Some(data.len()),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, [300]);
        assert_eq!(demuxer.skipped_bytes(), 4);
    }
}
