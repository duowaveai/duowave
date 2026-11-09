use anyhow::{anyhow, Result};

use super::ogg_page::{ogg_page, OGG_BOS, OGG_EOS};
use super::opus::{Application, Encoder};
use crate::utils::ids::random_bytes;

/// The rates we encode at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeRate {
    /// 16 kHz speech (calls, voice messages), encoded as voice.
    Speech,
    /// 48 kHz (our own synthesis: Silero gives 48 kHz, Piper 22 kHz), encoded as full-band audio.
    FullBand,
}

impl EncodeRate {
    pub fn hz(self) -> u32 {
        match self {
            EncodeRate::Speech => 16000,
            EncodeRate::FullBand => 48000,
        }
    }
}

/// Opus packets from mono PCM, 20 ms each (the last frame padded with silence). 16 kHz speech as voice; 48 kHz as
/// full-band audio, or it sounds like a phone line.
pub struct OpusPcmEncoder {
    encoder: Encoder,
    rate: EncodeRate,
    /// 20 ms of audio: one Opus packet.
    frame: usize,
    pending: Vec<i16>,
}

impl OpusPcmEncoder {
    pub fn new(rate: EncodeRate) -> Result<OpusPcmEncoder> {
        let application = if rate == EncodeRate::Speech { Application::Voip } else { Application::Audio };
        let encoder = Encoder::new(rate.hz(), 1, application).map_err(|error| anyhow!("Encoder error: {error}"))?;
        Ok(OpusPcmEncoder { encoder, rate, frame: rate.hz() as usize / 50, pending: Vec::new() })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate.hz()
    }

    /// Packets for whole frames; the rest waits for more samples.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<Vec<u8>>> {
        let all = [self.pending.as_slice(), pcm].concat();
        let mut packets = Vec::with_capacity(all.len() / self.frame);
        let mut frames = all.chunks_exact(self.frame);
        for frame in frames.by_ref() {
            packets.push(self.encoder.encode(frame, self.frame).map_err(|error| anyhow!("Encode error: {error}"))?);
        }
        self.pending = frames.remainder().to_vec();
        Ok(packets)
    }

    /// The last partial frame, padded.
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let mut frame = std::mem::take(&mut self.pending);
        frame.resize(self.frame, 0);
        Ok(vec![self.encoder.encode(&frame, self.frame).map_err(|error| anyhow!("Encode error: {error}"))?])
    }
}

fn random_serial() -> u32 {
    let bytes = random_bytes(4);
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The two header pages of an Ogg/Opus stream: OpusHead (mono, the input rate) and OpusTags, with `padding` zero
/// bytes after its comments if asked (RFC 7845 lets readers skip them).
fn opus_headers(serial: u32, sample_rate: u32, padding: usize) -> Result<Vec<Vec<u8>>> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1);
    head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&sample_rate.to_le_bytes());
    head.extend_from_slice(&[0; 3]);
    let vendor = b"duowave";
    let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4 + padding);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.resize(8 + 4 + vendor.len() + 4 + padding, 0);
    Ok(vec![ogg_page(serial, 0, 0, OGG_BOS, &[&head])?, ogg_page(serial, 1, 0, 0, &[&tags])?])
}

/// A whole Ogg/Opus file from mono PCM: a voice message (16 kHz), our own synthesis (48 kHz).
pub fn encode_ogg_opus(pcm: &[i16], rate: EncodeRate) -> Result<Vec<u8>> {
    let mut encoder = OpusPcmEncoder::new(rate)?;
    let mut packets = encoder.encode(pcm)?;
    packets.extend(encoder.flush()?);
    let serial = random_serial();
    let mut pages = opus_headers(serial, rate.hz(), 0)?;
    // Granule positions count 48 kHz samples: 960 per 20 ms packet.
    let count = packets.len();
    for (index, packet) in packets.iter().enumerate() {
        let flags = if index == count - 1 { OGG_EOS } else { 0 };
        pages.push(ogg_page(serial, pages.len() as u32, (index as i64 + 1) * 960, flags, &[packet])?);
    }
    Ok(pages.concat())
}

/// GStreamer's typefind waits for 2 KB before it names a stream.
const TYPEFIND_PADDING: usize = 2048;

/// A live Ogg/Opus stream from 16 kHz mono PCM, for the station's own GStreamer (souphttpsrc ! decodebin): the
/// headers first, then a page per 20 ms packet as soon as it is encoded — no waiting for a fuller page. The headers
/// are padded past 2 KB: GStreamer's typefind waits for that much before it names a stream (live 02.11: with the
/// headers and silence alone, ~0.5 KB, the station's decodebin stayed unlinked until a call's voice came).
pub struct OggOpusStream {
    encoder: OpusPcmEncoder,
    serial: u32,
    sequence: u32,
    granule: i64,
}

impl OggOpusStream {
    pub fn new() -> Result<OggOpusStream> {
        Ok(OggOpusStream {
            encoder: OpusPcmEncoder::new(EncodeRate::Speech)?,
            serial: random_serial(),
            sequence: 2,
            granule: 0,
        })
    }

    pub fn headers(&self) -> Result<Vec<u8>> {
        Ok(opus_headers(self.serial, 16000, TYPEFIND_PADDING)?.concat())
    }

    /// Pages for the whole frames of this audio; the rest waits for more.
    pub fn pages(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        let mut pages = Vec::new();
        for packet in self.encoder.encode(pcm)? {
            // Granule positions count 48 kHz samples: 960 per 20 ms packet.
            self.granule += 960;
            pages.extend(ogg_page(self.serial, self.sequence, self.granule, 0, &[&packet])?);
            self.sequence += 1;
        }
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
    use crate::speech::audio::opus_pcm_decoder::OpusPcmDecoder;
    use crate::speech::test_audio::{ogg_pages, voice};

    fn decoded_samples(events: &[OggOpusEvent]) -> usize {
        let mut decoder = OpusPcmDecoder::new();
        events
            .iter()
            .map(|event| match event {
                OggOpusEvent::Packet { data, .. } => decoder.decode(data).len(),
                OggOpusEvent::Head { .. } => 0,
            })
            .sum()
    }

    fn heads(events: &[OggOpusEvent]) -> usize {
        events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count()
    }

    #[test]
    fn makes_an_ogg_opus_file_our_own_demuxer_and_decoder_read_back() {
        let pcm = voice(1000.0, -20.0);
        let file = encode_ogg_opus(&pcm, EncodeRate::Speech).unwrap();
        let events = OggOpusDemuxer::new().push(&file);
        assert_eq!(heads(&events), 1);
        assert_eq!(events.len() - 1, 50);
        assert_eq!(decoded_samples(&events), 16000);
        // The last page ends the stream.
        assert_eq!(ogg_pages(&file).last().unwrap()[5] & 0x04, 0x04);
    }

    #[test]
    fn sends_the_headers_then_a_page_per_20_ms_as_the_audio_comes_in_pieces_of_any_size() {
        let mut stream = OggOpusStream::new().unwrap();
        let headers = stream.headers().unwrap();
        // Past GStreamer's typefind minimum (2 KB), so the station names the stream before the first call.
        assert!(headers.len() > 2048);
        let pcm = voice(1000.0, -20.0);
        // Uneven pieces, as the hub's frames and a call's gaps give them.
        let chunks: Vec<Vec<u8>> =
            [&pcm[..100], &pcm[100..5000], &pcm[5000..]].iter().map(|piece| stream.pages(piece).unwrap()).collect();
        assert!(chunks[0].is_empty());
        let all = [vec![headers.clone()], chunks.clone()].concat().concat();
        let pages = ogg_pages(&all);
        assert_eq!(pages.len(), 2 + 50);
        assert_eq!(pages[0][5] & 0x02, 0x02);
        let last = pages.last().unwrap();
        assert_eq!(i64::from_le_bytes(last[6..14].try_into().unwrap()), 50 * 960);

        let mut demuxer = OggOpusDemuxer::new();
        let events: Vec<OggOpusEvent> =
            std::iter::once(&headers).chain(chunks.iter()).flat_map(|chunk| demuxer.push(chunk)).collect();
        assert_eq!(heads(&events), 1);
        assert_eq!(decoded_samples(&events), 16000);
    }
}
