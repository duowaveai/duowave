//! Audio: Ogg/Opus in and out, the codec, PCM levels and WAV, the gain, voicing.

pub mod gain;
pub mod ogg_opus_demuxer;
pub mod ogg_opus_encoder;
pub mod ogg_opus_joiner;
pub mod ogg_page;
pub mod opus;
pub mod opus_packet;
pub mod opus_pcm_decoder;
pub mod pcm;
pub mod voicing;
