use anyhow::{bail, Result};

use super::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent, OpusHead};
use super::ogg_page::{ogg_page, OGG_BOS, OGG_EOS};
use super::opus_packet::opus_packet_samples;
use super::pcm::js_round;
use crate::utils::ids::random_bytes;

/// OpusHead for channel mapping family 0 (mono or stereo), the only kind a speech stream uses.
fn opus_head_packet(head: &OpusHead) -> Vec<u8> {
    let mut packet = Vec::with_capacity(19);
    packet.extend_from_slice(b"OpusHead");
    packet.push(1);
    packet.push(head.channels);
    packet.extend_from_slice(&head.pre_skip.to_le_bytes());
    packet.extend_from_slice(&head.input_sample_rate.to_le_bytes());
    packet.extend_from_slice(&(js_round(head.output_gain_db * 256.0) as i16).to_le_bytes());
    packet.push(0);
    packet
}

fn opus_tags_packet() -> Vec<u8> {
    let vendor = b"duowave";
    let mut packet = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    packet.extend_from_slice(b"OpusTags");
    packet.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    packet.extend_from_slice(vendor);
    packet.extend_from_slice(&0u32.to_le_bytes());
    packet
}

struct HeldPacket {
    packet: Vec<u8>,
    samples: usize,
}

/// Joins Ogg/Opus segments, one per synthesized phrase, into the one continuous stream the station plays: a single
/// serial number, the first segment's OpusHead, every packet on its own page with a running granule position. The
/// station stops playing at an EOS page as well as at the stream close, so only the last page of all carries EOS:
/// the newest packet is held back until it is known whether more follows. Later segments' encoder pre-skip (a few
/// milliseconds) is played rather than trimmed.
///
/// A segment may itself be a chain: Yandex sends a sound from markup (`<speaker audio="…">`) and the speech after it
/// as two logical streams, each with its own head. A later link goes on in the same stream as a later segment does:
/// its head and tags are dropped, its pre-skip played, its packets on running granules.
pub struct OggOpusJoiner {
    segments: u32,
    packets: u32,
    serial: u32,
    sequence: u32,
    /// Granule position of the last page written: 48 kHz samples, pre-skip included.
    granule: usize,
    head: Option<OpusHead>,
    demuxer: Option<OggOpusDemuxer>,
    held: Option<HeldPacket>,
    finished: bool,
    /// The current link of a chain cannot go into this stream (a multichannel mapping): its packets are dropped.
    skipping: bool,
}

impl Default for OggOpusJoiner {
    fn default() -> OggOpusJoiner {
        OggOpusJoiner::new()
    }
}

impl OggOpusJoiner {
    pub fn new() -> OggOpusJoiner {
        let serial = random_bytes(4);
        OggOpusJoiner {
            segments: 0,
            packets: 0,
            serial: u32::from_le_bytes([serial[0], serial[1], serial[2], serial[3]]),
            sequence: 0,
            granule: 0,
            head: None,
            demuxer: None,
            held: None,
            finished: false,
            skipping: false,
        }
    }

    pub fn segments(&self) -> u32 {
        self.segments
    }

    pub fn packets(&self) -> u32 {
        self.packets
    }

    /// Audio accepted so far, held packet included.
    pub fn duration_ms(&self) -> u64 {
        let samples = self.granule as f64 + self.held.as_ref().map_or(0.0, |held| held.samples as f64)
            - self.head.as_ref().map_or(0.0, |head| f64::from(head.pre_skip));
        js_round(samples / 48.0).max(0.0) as u64
    }

    pub fn begin_segment(&mut self) -> Result<()> {
        if self.finished {
            bail!("the stream is already finished");
        }
        self.demuxer = Some(OggOpusDemuxer::new());
        self.skipping = false;
        self.segments += 1;
        Ok(())
    }

    /// Pages for the next bytes of the current segment; empty until whole packets are there.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let events = match self.demuxer.as_mut() {
            Some(demuxer) if !self.finished => demuxer.push(chunk),
            _ => bail!("no segment started"),
        };
        let mut pages = Vec::new();
        for event in events {
            match event {
                OggOpusEvent::Head { chain, head } => {
                    if chain > 0 {
                        self.accept_link(&head);
                    } else {
                        self.accept_head(head, &mut pages)?;
                    }
                }
                OggOpusEvent::Packet { data, .. } => {
                    if self.skipping {
                        continue;
                    }
                    if self.head.is_none() {
                        bail!("Opus packet before OpusHead");
                    }
                    let samples = opus_packet_samples(&data)?;
                    if let Some(held) = self.held.take() {
                        pages.extend(self.page(held, 0)?);
                    }
                    self.held = Some(HeldPacket { packet: data, samples });
                    self.packets += 1;
                }
            }
        }
        Ok(pages)
    }

    /// The last page, with EOS. Empty when nothing was written; the joiner takes no segments after it.
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        if self.head.is_none() {
            return Ok(Vec::new());
        }
        match self.held.take() {
            None => {
                let page = ogg_page(self.serial, self.sequence, self.granule as i64, OGG_EOS, &[])?;
                self.sequence += 1;
                Ok(page)
            }
            Some(held) => self.page(held, OGG_EOS),
        }
    }

    fn accept_head(&mut self, head: OpusHead, pages: &mut Vec<u8>) -> Result<()> {
        match &self.head {
            None => {
                if head.mapping_family != 0 {
                    bail!("unsupported Opus channel mapping family {}", head.mapping_family);
                }
                pages.extend(ogg_page(self.serial, self.sequence, 0, OGG_BOS, &[&opus_head_packet(&head)])?);
                pages.extend(ogg_page(self.serial, self.sequence + 1, 0, 0, &[&opus_tags_packet()])?);
                self.sequence += 2;
                self.head = Some(head);
            }
            // A later segment goes on in this stream as a later link does: a mono phrase in a stereo stream or the
            // other way round plays (each packet's TOC byte says whether it is stereo), so a stream that began with
            // our mono silence (an answer kept alive) still carries a stereo voice. A mapping family 0 stream
            // cannot carry another family.
            Some(first) => {
                if head.mapping_family != first.mapping_family {
                    bail!(
                        "segment {} has channel mapping family {}, the stream {}",
                        self.segments,
                        head.mapping_family,
                        first.mapping_family
                    );
                }
            }
        }
        Ok(())
    }

    /// The next link of a chain within a segment. A mono link in a stereo stream or the other way round goes on:
    /// every Opus packet says in its TOC byte whether it is stereo, and a decoder plays it in the stream's channel
    /// count. A multichannel mapping cannot be carried in a family 0 stream, so that link's audio is left out rather
    /// than the whole phrase failing.
    fn accept_link(&mut self, head: &OpusHead) {
        self.skipping = head.mapping_family != 0;
    }

    fn page(&mut self, held: HeldPacket, flags: u8) -> Result<Vec<u8>> {
        self.granule += held.samples;
        let page = ogg_page(self.serial, self.sequence, self.granule as i64, flags, &[&held.packet])?;
        self.sequence += 1;
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::ogg_page::ogg_crc;
    use crate::speech::test_audio::{ogg_opus_chain, ogg_pages, voice, voice_at_pitch};

    #[derive(Debug)]
    struct Page {
        flags: u8,
        granule: i64,
        serial: u32,
        sequence: u32,
        crc_ok: bool,
    }

    fn read_pages(bytes: &[u8]) -> Vec<Page> {
        ogg_pages(bytes)
            .into_iter()
            .map(|page| {
                let mut copy = page.to_vec();
                copy[22..26].fill(0);
                Page {
                    flags: page[5],
                    granule: i64::from_le_bytes(page[6..14].try_into().unwrap()),
                    serial: u32::from_le_bytes(page[14..18].try_into().unwrap()),
                    sequence: u32::from_le_bytes(page[18..22].try_into().unwrap()),
                    crc_ok: ogg_crc(&copy) == u32::from_le_bytes(page[22..26].try_into().unwrap()),
                }
            })
            .collect()
    }

    /// Feeds a segment in odd-sized chunks, as frames arrive from the network.
    fn feed(joiner: &mut OggOpusJoiner, segment: &[u8]) -> Vec<u8> {
        joiner.begin_segment().unwrap();
        segment.chunks(333).flat_map(|chunk| joiner.push(chunk).unwrap()).collect()
    }

    fn phrases() -> (Vec<u8>, Vec<u8>) {
        (ogg_opus_chain(&voice(400.0, -20.0), 0x1111), ogg_opus_chain(&voice_at_pitch(600.0, -20.0, 220.0), 0x2222))
    }

    fn counts(stream: &[u8]) -> (usize, usize) {
        let events = OggOpusDemuxer::new().push(stream);
        let heads = events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count();
        (heads, events.len() - heads)
    }

    #[test]
    fn reads_the_duration_from_the_toc_byte() {
        assert_eq!(opus_packet_samples(&[0x08]).unwrap(), 960); // SILK 20 ms, one frame
        assert_eq!(opus_packet_samples(&[0x18]).unwrap(), 2880); // SILK 60 ms
        assert_eq!(opus_packet_samples(&[0xf8 | 1]).unwrap(), 1920); // CELT 20 ms, two frames
        assert_eq!(opus_packet_samples(&[0xf3, 0x03]).unwrap(), 1440); // CELT 10 ms, three frames (code 3)
        assert!(opus_packet_samples(&[]).unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn joins_phrases_into_one_stream_with_one_head_running_granules_and_eos_only_at_the_very_end() {
        let (first, second) = phrases();
        let mut joiner = OggOpusJoiner::new();
        let stream = [feed(&mut joiner, &first), feed(&mut joiner, &second), joiner.finish().unwrap()].concat();

        let pages = read_pages(&stream);
        assert!(pages.iter().all(|page| page.crc_ok));
        assert!(pages.iter().all(|page| page.serial == pages[0].serial));
        assert!(pages.iter().enumerate().all(|(index, page)| page.sequence == index as u32));
        assert_eq!(pages[0].flags, 0x02);
        assert_eq!(pages.iter().filter(|page| page.flags & 0x04 != 0).count(), 1);
        assert_eq!(pages.last().unwrap().flags, 0x04);
        // 20 + 30 packets of 20 ms after the two header pages.
        assert_eq!(pages.len(), 2 + 50);
        assert_eq!(pages.last().unwrap().granule, 50 * 960);
        assert_eq!(joiner.duration_ms(), 1000);

        // A player sees one chain with every packet of both phrases.
        assert_eq!(counts(&stream), (1, 50));
    }

    #[test]
    fn carries_a_chained_segment_on_in_the_one_stream_a_sound_from_markup_then_the_speech() {
        // Yandex's `<speaker audio="…"> text`: two logical streams, each with its head and tags, in one phrase.
        let (first, second) = phrases();
        let mut joiner = OggOpusJoiner::new();
        let stream = [feed(&mut joiner, &[first, second].concat()), joiner.finish().unwrap()].concat();

        let pages = read_pages(&stream);
        assert!(pages.iter().all(|page| page.crc_ok));
        assert!(pages.iter().all(|page| page.serial == pages[0].serial));
        assert_eq!(pages.iter().filter(|page| page.flags & 0x02 != 0).count(), 1);
        assert_eq!(pages.iter().filter(|page| page.flags & 0x04 != 0).count(), 1);
        assert_eq!(pages.last().unwrap().granule, 50 * 960);
        assert_eq!(joiner.duration_ms(), 1000);
        assert_eq!(counts(&stream), (1, 50));
    }

    #[test]
    fn leaves_out_a_link_it_cannot_carry_a_multichannel_mapping_and_keeps_the_rest_of_the_phrase() {
        let (first, multichannel) = phrases();
        // OpusHead is the only packet on the chain's first page: its mapping family byte is the page's last.
        let first_page = ogg_pages(&multichannel)[0].to_vec();
        let mut patched = first_page.clone();
        *patched.last_mut().unwrap() = 1;
        patched[22..26].fill(0);
        let crc = ogg_crc(&patched);
        patched[22..26].copy_from_slice(&crc.to_le_bytes());
        let chain = [first, patched, multichannel[first_page.len()..].to_vec()].concat();

        let mut joiner = OggOpusJoiner::new();
        feed(&mut joiner, &chain);
        joiner.finish().unwrap();
        assert_eq!(joiner.duration_ms(), 400);
    }

    #[test]
    fn holds_the_newest_packet_back_until_the_next_one_or_the_end() {
        let (first, _) = phrases();
        let mut joiner = OggOpusJoiner::new();
        let pages = ogg_pages(&first);
        joiner.begin_segment().unwrap();
        // Head and tags; packet 1 held.
        assert_eq!(read_pages(&joiner.push(&pages[..3].concat()).unwrap()).len(), 2);
        // Packet 1 out, packet 2 held.
        assert_eq!(read_pages(&joiner.push(pages[3]).unwrap()).len(), 1);
        assert_eq!(read_pages(&joiner.finish().unwrap())[0].flags, 0x04);
        assert!(joiner.finish().unwrap().is_empty());
    }

    #[test]
    fn carries_a_segment_of_another_channel_count_on_in_the_stream() {
        // Our mono silence first (an answer kept alive), then a stereo phrase: the phrase is not lost.
        let (mono, _) = phrases();
        let mut stereo = ogg_pages(&mono).iter().map(|page| page.to_vec()).collect::<Vec<_>>();
        // OpusHead's channel count is its 10th byte: the head is the only packet on the first page.
        let head_at = stereo[0].len() - 19;
        stereo[0][head_at + 9] = 2;
        stereo[0][22..26].fill(0);
        let crc = ogg_crc(&stereo[0]);
        stereo[0][22..26].copy_from_slice(&crc.to_le_bytes());
        let mut joiner = OggOpusJoiner::new();
        let stream = [feed(&mut joiner, &mono), feed(&mut joiner, &stereo.concat()), joiner.finish().unwrap()].concat();
        assert_eq!(joiner.duration_ms(), 800);
        assert_eq!(counts(&stream), (1, 40));
    }

    #[test]
    fn writes_nothing_when_no_audio_came() {
        let mut joiner = OggOpusJoiner::new();
        joiner.begin_segment().unwrap();
        assert!(joiner.finish().unwrap().is_empty());
    }
}
