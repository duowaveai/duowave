//! The Opus codec: safe wrappers over libopus. The TS gateway used opusscript (libopus compiled to WebAssembly);
//! here it is unsafe-libopus, libopus 1.3.1 translated to Rust by c2rust and checked against the C library on the
//! IETF test vectors — the same codec with nothing to compile or link, so the station's build needs neither a C
//! toolchain nor a prebuilt library. Every `unsafe` of the codec is in this file.

use std::fmt;
use std::ptr::NonNull;

/// Encoded packets are at most this long: opusscript's buffer, three of the largest frames.
const MAX_PACKET_SIZE: usize = 1276 * 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Application {
    /// Speech: calls, voice messages, the test audio.
    Voip,
    /// Full-band audio: our own synthesis.
    Audio,
}

/// A libopus error code, named as opusscript named it ("Invalid packet").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusError(pub i32);

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            0 => "OK",
            -1 => "Bad argument",
            -2 => "Buffer too small",
            -3 => "Internal error",
            -4 => "Invalid packet",
            -5 => "Unimplemented",
            -6 => "Invalid state",
            -7 => "Memory allocation fail",
            _ => return write!(f, "error {}", self.0),
        };
        f.write_str(name)
    }
}

impl std::error::Error for OpusError {}

/// The state libopus left in `error`, or allocation failure when it gave none.
fn created<T>(raw: *mut T, error: i32) -> Result<NonNull<T>, OpusError> {
    match NonNull::new(raw) {
        Some(raw) if error == unsafe_libopus::OPUS_OK => Ok(raw),
        _ => Err(OpusError(if error < 0 { error } else { unsafe_libopus::OPUS_ALLOC_FAIL })),
    }
}

/// An Opus encoder for interleaved 16-bit PCM.
pub struct Encoder {
    raw: NonNull<unsafe_libopus::OpusEncoder>,
    channels: usize,
}

// SAFETY: the encoder's state is plain memory owned by this value alone; libopus keeps no thread-local state.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(sample_rate: u32, channels: u32, application: Application) -> Result<Encoder, OpusError> {
        let application = match application {
            Application::Voip => unsafe_libopus::OPUS_APPLICATION_VOIP,
            Application::Audio => unsafe_libopus::OPUS_APPLICATION_AUDIO,
        };
        let mut error = 0;
        // SAFETY: libopus checks the rate, channels and application and reports through `error`, a valid pointer.
        let raw = unsafe {
            unsafe_libopus::opus_encoder_create(sample_rate as i32, channels as i32, application, &mut error)
        };
        let raw = created(raw, error).inspect_err(|_| {
            if !raw.is_null() {
                // SAFETY: a state libopus allocated and nothing else holds.
                unsafe { unsafe_libopus::opus_encoder_destroy(raw) };
            }
        })?;
        Ok(Encoder { raw, channels: channels as usize })
    }

    /// One packet from `frame_size` samples per channel (2.5 to 60 ms of audio).
    pub fn encode(&mut self, pcm: &[i16], frame_size: usize) -> Result<Vec<u8>, OpusError> {
        if frame_size == 0 || pcm.len() < frame_size * self.channels {
            return Err(OpusError(unsafe_libopus::OPUS_BAD_ARG));
        }
        let mut packet = vec![0u8; MAX_PACKET_SIZE];
        // SAFETY: `pcm` holds frame_size samples for every channel, `packet` is writable for its whole length, and
        // the state is ours alone (&mut self).
        let length = unsafe {
            unsafe_libopus::opus_encode(
                self.raw.as_ptr(),
                pcm.as_ptr(),
                frame_size as i32,
                packet.as_mut_ptr(),
                packet.len() as i32,
            )
        };
        if length < 0 {
            return Err(OpusError(length));
        }
        packet.truncate(length as usize);
        Ok(packet)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the state came from opus_encoder_create and is destroyed once.
        unsafe { unsafe_libopus::opus_encoder_destroy(self.raw.as_ptr()) }
    }
}

/// An Opus decoder to interleaved 16-bit PCM.
pub struct Decoder {
    raw: NonNull<unsafe_libopus::OpusDecoder>,
    channels: usize,
    /// The longest packet, 120 ms, in samples per channel.
    max_frame: usize,
}

// SAFETY: as for the encoder.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(sample_rate: u32, channels: u32) -> Result<Decoder, OpusError> {
        let mut error = 0;
        // SAFETY: libopus checks the rate and channels and reports through `error`, a valid pointer.
        let raw = unsafe { unsafe_libopus::opus_decoder_create(sample_rate as i32, channels as i32, &mut error) };
        let raw = created(raw, error).inspect_err(|_| {
            if !raw.is_null() {
                // SAFETY: a state libopus allocated and nothing else holds.
                unsafe { unsafe_libopus::opus_decoder_destroy(raw) };
            }
        })?;
        Ok(Decoder { raw, channels: channels as usize, max_frame: sample_rate as usize * 120 / 1000 })
    }

    /// The samples of one packet; an empty packet is a lost one (libopus conceals it).
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>, OpusError> {
        let Ok(length) = i32::try_from(packet.len()) else {
            return Err(OpusError(unsafe_libopus::OPUS_INVALID_PACKET));
        };
        let mut pcm = vec![0i16; self.max_frame * self.channels];
        // SAFETY: libopus reads at most `length` bytes of `packet` and writes at most max_frame samples per channel
        // into `pcm`, which holds that many; the state is ours alone (&mut self).
        let samples = unsafe {
            unsafe_libopus::opus_decode(
                self.raw.as_ptr(),
                packet.as_ptr(),
                length,
                pcm.as_mut_ptr(),
                self.max_frame as i32,
                0,
            )
        };
        if samples < 0 {
            return Err(OpusError(samples));
        }
        pcm.truncate(samples as usize * self.channels);
        Ok(pcm)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the state came from opus_decoder_create and is destroyed once.
        unsafe { unsafe_libopus::opus_decoder_destroy(self.raw.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_twenty_milliseconds() {
        let tone: Vec<i16> = (0..320).map(|i| ((i as f64 * 0.3).sin() * 8000.0) as i16).collect();
        let mut encoder = Encoder::new(16000, 1, Application::Voip).unwrap();
        let packet = encoder.encode(&tone, 320).unwrap();
        assert!(!packet.is_empty() && packet.len() < 200, "{}", packet.len());
        let mut decoder = Decoder::new(16000, 1).unwrap();
        assert_eq!(decoder.decode(&packet).unwrap().len(), 320);
        assert_eq!(decoder.decode(&[0x4b, 0x00]), Err(OpusError(-4)));
        assert_eq!(OpusError(-4).to_string(), "Invalid packet");
        assert!(Encoder::new(44100, 1, Application::Audio).is_err());
        assert!(encoder.encode(&tone[..100], 320).is_err());
    }

    #[test]
    fn survives_garbage_from_the_network() {
        // Packets come from the station as they are: whatever they hold, decoding fails or plays, never crashes.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut mono = Decoder::new(16000, 1).unwrap();
        let mut stereo = Decoder::new(16000, 2).unwrap();
        for _ in 0..1_000 {
            let length = (random() % 400) as usize;
            let packet: Vec<u8> = (0..length).map(|_| random() as u8).collect();
            let _ = mono.decode(&packet);
            let _ = stereo.decode(&packet);
        }
    }
}
