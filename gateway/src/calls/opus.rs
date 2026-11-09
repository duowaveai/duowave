//! Opus as the calls need it: the station's live Ogg/Opus stream (`/calls/listen/<device>?format=ogg`) and 20 ms
//! packets both ways for a browser over WebRTC. The calls ask for it through `CallsOpus`; `SpeechOpus` gives them
//! speech::audio's codec (its OggOpusStream, OpusPcmEncoder and Decoder: libopus 1.3.1 in Rust, unsafe-libopus).

use anyhow::{anyhow, Result};

use crate::speech::audio::ogg_opus_encoder::{EncodeRate, OggOpusStream, OpusPcmEncoder};
use crate::speech::audio::opus::Decoder;

/// A live Ogg/Opus stream of 16 kHz mono: its headers, then a page per 20 ms packet as soon as it is encoded.
pub trait OggStream: Send {
    fn headers(&self) -> Result<Vec<u8>>;
    /// Pages for the whole frames of this audio; the rest waits for more.
    fn pages(&mut self, pcm: &[i16]) -> Result<Vec<u8>>;
}

/// 20 ms Opus packets of 16 kHz mono speech; a partial frame waits for more samples.
pub trait PacketEncoder: Send {
    fn encode(&mut self, pcm: &[i16]) -> Result<Vec<Vec<u8>>>;
}

/// Opus packets to 16 kHz mono.
pub trait PacketDecoder: Send {
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>>;
}

pub trait CallsOpus: Send + Sync {
    fn ogg_stream(&self) -> Result<Box<dyn OggStream>>;
    fn encoder(&self) -> Result<Box<dyn PacketEncoder>>;
    fn decoder(&self) -> Result<Box<dyn PacketDecoder>>;
}

/// The calls' rate: 16 kHz mono.
const RATE: u32 = 16_000;

/// speech::audio's codec for the calls: the station's stream is speech's OggOpusStream (its headers padded past
/// GStreamer's typefind minimum), the packets speech's encoder at 16 kHz as voice, and its decoder.
pub struct SpeechOpus;

impl CallsOpus for SpeechOpus {
    fn ogg_stream(&self) -> Result<Box<dyn OggStream>> {
        Ok(Box::new(OggOpusStream::new()?))
    }

    fn encoder(&self) -> Result<Box<dyn PacketEncoder>> {
        Ok(Box::new(OpusPcmEncoder::new(EncodeRate::Speech)?))
    }

    fn decoder(&self) -> Result<Box<dyn PacketDecoder>> {
        let decoder = Decoder::new(RATE, 1).map_err(|error| anyhow!("Opus: {error}"))?;
        Ok(Box::new(decoder))
    }
}

impl OggStream for OggOpusStream {
    fn headers(&self) -> Result<Vec<u8>> {
        OggOpusStream::headers(self)
    }

    fn pages(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        OggOpusStream::pages(self, pcm)
    }
}

impl PacketEncoder for OpusPcmEncoder {
    fn encode(&mut self, pcm: &[i16]) -> Result<Vec<Vec<u8>>> {
        OpusPcmEncoder::encode(self, pcm)
    }
}

impl PacketDecoder for Decoder {
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        Decoder::decode(self, packet).map_err(|error| anyhow!("Opus: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::ogg_page::{ogg_crc, OGG_BOS};

    /// 20 ms at the calls' rate.
    const FRAME: usize = 320;

    #[test]
    fn encodes_twenty_milliseconds_and_decodes_them_back() {
        let tone: Vec<i16> = (0..FRAME).map(|i| ((i as f64 * 0.3).sin() * 8000.0) as i16).collect();
        let mut encoder = SpeechOpus.encoder().unwrap();
        assert!(encoder.encode(&tone[..100]).unwrap().is_empty(), "a partial frame waits");
        let packets = encoder.encode(&tone[100..]).unwrap();
        assert_eq!(packets.len(), 1);
        let mut decoder = SpeechOpus.decoder().unwrap();
        assert_eq!(decoder.decode(&packets[0]).unwrap().len(), FRAME);
        assert!(decoder.decode(&[0x4b, 0x00]).is_err());
    }

    #[test]
    fn streams_padded_headers_then_a_page_per_packet() {
        let mut stream = SpeechOpus.ogg_stream().unwrap();
        let headers = stream.headers().unwrap();
        // GStreamer's typefind waits for 2 KB before it names a stream.
        assert!(headers.len() > 2048);
        assert_eq!(&headers[..4], b"OggS");
        assert_eq!(headers[5], OGG_BOS);
        assert_eq!(&headers[28..36], b"OpusHead");
        let pages = stream.pages(&[0; 3200]).unwrap();
        assert_eq!(pages.windows(4).filter(|window| window == b"OggS").count(), 10);
        // Every page's checksum holds.
        let first_len =
            27 + pages[26] as usize + pages[27..27 + pages[26] as usize].iter().map(|&b| b as usize).sum::<usize>();
        let mut page = pages[..first_len].to_vec();
        let crc = u32::from_le_bytes([page[22], page[23], page[24], page[25]]);
        page[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(ogg_crc(&page), crc);
        // The granule counts 48 kHz: the first page is 960.
        assert_eq!(i64::from_le_bytes(pages[6..14].try_into().unwrap()), 960);
    }
}
