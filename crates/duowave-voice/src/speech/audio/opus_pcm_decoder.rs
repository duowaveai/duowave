use super::ogg_opus_demuxer::OpusHead;
use super::opus::Decoder;
use super::pcm::js_round;

/// Sample rate of everything after decoding: VAD, recognition and the debug WAV files.
pub const SPEECH_SAMPLE_RATE: u32 = 16000;

/// Decodes the station's Opus packets to 16 kHz mono PCM. Every chain starts with its own OpusHead and gets a
/// fresh decoder.
pub struct OpusPcmDecoder {
    packets: u64,
    errors: u64,
    last_error: Option<String>,
    decoder: Option<Decoder>,
    channels: usize,
    skip: usize,
}

impl Default for OpusPcmDecoder {
    fn default() -> OpusPcmDecoder {
        OpusPcmDecoder::new()
    }
}

impl OpusPcmDecoder {
    pub fn new() -> OpusPcmDecoder {
        OpusPcmDecoder { packets: 0, errors: 0, last_error: None, decoder: None, channels: 1, skip: 0 }
    }

    pub fn packets(&self) -> u64 {
        self.packets
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Starts a new chain.
    pub fn reset(&mut self, head: &OpusHead) {
        self.channels = if head.channels == 2 { 2 } else { 1 };
        self.decoder = match Decoder::new(SPEECH_SAMPLE_RATE, self.channels as u32) {
            Ok(decoder) => Some(decoder),
            Err(error) => {
                self.last_error = Some(format!("Decoder error: {error}"));
                None
            }
        };
        // Pre-skip is counted at 48 kHz.
        self.skip = js_round(f64::from(head.pre_skip) * f64::from(SPEECH_SAMPLE_RATE) / 48000.0) as usize;
    }

    /// Returns the decoded samples; a damaged packet is counted and yields nothing.
    pub fn decode(&mut self, packet: &[u8]) -> Vec<i16> {
        if self.decoder.is_none() {
            self.reset(&OpusHead::default());
        }
        self.packets += 1;
        let decoded = match self.decoder.as_mut().map(|decoder| decoder.decode(packet)) {
            Some(Ok(samples)) => samples,
            Some(Err(error)) => {
                self.errors += 1;
                self.last_error = Some(format!("Decode error: {error}"));
                return Vec::new();
            }
            None => {
                self.errors += 1;
                return Vec::new();
            }
        };
        let mut pcm = if self.channels == 2 {
            decoded
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| ((i32::from(pair[0]) + i32::from(pair[1])) >> 1) as i16)
                .collect()
        } else {
            decoded
        };
        if self.skip > 0 {
            let dropped = self.skip.min(pcm.len());
            self.skip -= dropped;
            pcm.drain(..dropped);
        }
        pcm
    }

    /// Frees the codec's state; the next packet starts a fresh one.
    pub fn dispose(&mut self) {
        self.decoder = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
    use crate::speech::audio::pcm::{dbfs, levels};
    use crate::speech::test_audio::{ogg_opus_chain, voice};

    #[test]
    fn decodes_station_like_opus_back_to_16_khz_pcm_at_the_same_level() {
        let source = voice(1000.0, -30.0);
        let mut decoder = OpusPcmDecoder::new();
        let mut pcm = Vec::new();
        for event in OggOpusDemuxer::new().push(&ogg_opus_chain(&source, 1)) {
            match event {
                OggOpusEvent::Head { head, .. } => decoder.reset(&head),
                OggOpusEvent::Packet { data, .. } => pcm.extend(decoder.decode(&data)),
            }
        }
        assert_eq!(pcm.len(), 16000);
        assert!((dbfs(levels(&pcm).rms) - dbfs(levels(&source).rms)).abs() < 3.0);
        assert_eq!(decoder.errors(), 0);
        decoder.dispose();
    }

    #[test]
    fn counts_a_damaged_packet_instead_of_throwing() {
        let mut decoder = OpusPcmDecoder::new();
        decoder.reset(&OpusHead::default());
        // TOC code 3 with a frame count of 0 is invalid.
        assert_eq!(decoder.decode(&[0x4b, 0x00]).len(), 0);
        assert_eq!(decoder.errors(), 1);
        let error = decoder.last_error().unwrap();
        assert!(error.contains("Invalid packet") || error.contains("Decode error"), "{error}");
        decoder.dispose();
    }
}
