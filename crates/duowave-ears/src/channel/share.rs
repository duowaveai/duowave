//! The ear's sound for the word's program (docs/plans/audio-interface-3.md, «К10»): the computer's ear gives an
//! activator app its sound the way a station does — `system/mic` → `open` answers this address in place of the
//! station's `yio_audio` port, and the program reads it with the station's own protocol, byte for byte (the calls
//! read a station's the same way: crates/duowave-pc/src/calls/station_mic.rs). One program of the word works on the
//! station and here; only its build differs.
//!
//! - On 127.0.0.1 only, on a port of its own.
//! - A frame is a uint32 LE length, then QuasarMessage {252: body, 253: type}. The client subscribes with an
//!   IOChannelRequest (type 159) that names the group (field 2), and leaves with field 6; the server sends
//!   IOAudioChannelsData (type 158) every 32 ms: one channel, `vqe_0`, 16 kHz mono PCM16.
//! - The sound flows only while «Слушать слово» is on (the channel sends none otherwise); it lives in memory for the
//!   moment it is sent and is never kept.

use std::sync::{Arc, Weak};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

use super::VoiceChannel;

/// The channel a station's activator app reads (src/activator/aliced.rs, `MIC_CHANNEL`).
pub const CHANNEL: &str = "vqe_0";
/// TYPE_IO_AUDIO_CHANNELS_DATA.
pub const AUDIO_CHANNELS_DATA: u64 = 158;
/// TYPE_IO_CHANNEL_REQUEST.
pub const CHANNEL_REQUEST: u64 = 159;
/// The station sends its channels every 32 ms: 512 samples at 16 kHz.
const FRAME_SAMPLES: usize = 512;
/// No request of a client comes near this.
const MAX_FRAME: usize = 64 * 1024;

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn bytes_field(field: u64, value: &[u8], out: &mut Vec<u8>) {
    varint(field * 8 + 2, out);
    varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn number_field(field: u64, value: u64, out: &mut Vec<u8>) {
    varint(field * 8, out);
    varint(value, out);
}

/// A frame of the station's IPC.
pub fn ipc_frame(kind: u64, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(body.len() + 8);
    bytes_field(252, body, &mut message);
    number_field(253, kind, &mut message);
    let mut frame = (message.len() as u32).to_le_bytes().to_vec();
    frame.extend(message);
    frame
}

/// IOAudioChannelsData with one channel, as a station sends it: type, name, PCM, a flag, the rate, the channel count,
/// the time.
pub fn audio_frame(samples: &[i16], at_ms: u64) -> Vec<u8> {
    let pcm: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    let mut data = Vec::with_capacity(pcm.len() + 32);
    number_field(1, 9, &mut data);
    bytes_field(2, CHANNEL.as_bytes(), &mut data);
    bytes_field(3, &pcm, &mut data);
    number_field(4, 0, &mut data);
    number_field(7, u64::from(super::endpoint::RATE), &mut data);
    number_field(8, 1, &mut data);
    number_field(9, at_ms, &mut data);
    let mut body = Vec::with_capacity(data.len() + 4);
    bytes_field(1, &data, &mut body);
    ipc_frame(AUDIO_CHANNELS_DATA, &body)
}

/// What a client's request asks: in (field 2), out (field 6), or something else.
fn subscription(message: &[u8]) -> Option<bool> {
    let mut at = 0;
    let mut kind = None;
    let mut body: &[u8] = &[];
    while at < message.len() {
        let (key, next) = read_varint(message, at)?;
        at = next;
        match key & 7 {
            0 => {
                let (value, next) = read_varint(message, at)?;
                at = next;
                if key >> 3 == 253 {
                    kind = Some(value);
                }
            }
            2 => {
                let (length, next) = read_varint(message, at)?;
                let end = next.checked_add(usize::try_from(length).ok()?)?;
                if key >> 3 == 252 {
                    body = message.get(next..end)?;
                }
                at = end;
            }
            _ => return None,
        }
    }
    if kind != Some(CHANNEL_REQUEST) {
        return None;
    }
    let (key, _) = read_varint(body, 0)?;
    match key >> 3 {
        2 => Some(true),
        6 => Some(false),
        _ => None,
    }
}

fn read_varint(buf: &[u8], mut at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while at < buf.len() && shift < 64 {
        let byte = buf[at];
        value |= u64::from(byte & 0x7f) << shift;
        at += 1;
        if byte & 0x80 == 0 {
            return Some((value, at));
        }
        shift += 7;
    }
    None
}

/// The share's server: where it listens.
pub struct Share {
    pub port: u16,
}

/// The server starts on a free port of 127.0.0.1; it ends with the channel (a gateway started again in the app's
/// process has a channel of its own).
pub async fn serve(channel: Weak<VoiceChannel>) -> std::io::Result<Share> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let Some(mut alive) = channel.upgrade().map(|channel| channel.watch()) else {
        return Err(std::io::Error::other("the channel is gone"));
    };
    tokio::spawn(async move {
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((socket, _)) = accepted else { continue };
                    let Some(channel) = channel.upgrade() else { return };
                    tokio::spawn(client(socket, channel.sound()));
                }
                changed = alive.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    });
    Ok(Share { port })
}

/// One client: its requests in, the sound out while it is subscribed.
async fn client(socket: TcpStream, mut sound: broadcast::Receiver<Arc<Vec<i16>>>) {
    let (mut reader, mut writer) = socket.into_split();
    let (subscribed_by, mut subscribed) = watch::channel(false);
    tokio::spawn(async move {
        let mut pending: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let Ok(read) = reader.read(&mut buffer).await else { return };
            if read == 0 {
                return;
            }
            pending.extend_from_slice(&buffer[..read]);
            while pending.len() >= 4 {
                let length = u32::from_le_bytes([pending[0], pending[1], pending[2], pending[3]]) as usize;
                if length > MAX_FRAME {
                    return;
                }
                if pending.len() < 4 + length {
                    break;
                }
                if let Some(on) = subscription(&pending[4..4 + length]) {
                    let _ = subscribed_by.send(on);
                }
                pending.drain(..4 + length);
            }
        }
    });
    let mut chunk: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);
    loop {
        tokio::select! {
            changed = subscribed.changed() => {
                if changed.is_err() {
                    return;
                }
                chunk.clear();
            }
            frame = sound.recv() => match frame {
                Ok(frame) => {
                    if !*subscribed.borrow() {
                        continue;
                    }
                    chunk.extend_from_slice(&frame);
                    while chunk.len() >= FRAME_SAMPLES {
                        let samples: Vec<i16> = chunk.drain(..FRAME_SAMPLES).collect();
                        let frame = audio_frame(&samples, crate::utils::time::now_ms());
                        if writer.write_all(&frame).await.is_err() {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => chunk.clear(),
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_are_the_station_s_own() {
        // The calls' SUBSCRIBE and UNSUBSCRIBE, checked on the station on 02.11 (calls/station_mic.rs).
        let subscribe = ipc_frame(CHANNEL_REQUEST, &[0x12, 0x00]);
        assert_eq!(hex::encode(&subscribe), "09000000e20f021200e80f9f01");
        assert_eq!(subscription(&subscribe[4..]), Some(true));
        assert_eq!(subscription(&ipc_frame(CHANNEL_REQUEST, &[0x32, 0x00])[4..]), Some(false));
        assert_eq!(subscription(&ipc_frame(233, b"{}")[4..]), None);
        let frame = audio_frame(&[1, -2, 3], 42);
        let length = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(length, frame.len() - 4);
        assert!(frame.windows(CHANNEL.len()).any(|window| window == CHANNEL.as_bytes()));
    }
}
