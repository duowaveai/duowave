//! What the gateway and its recognition worker say to each other over the worker's stdin and stdout. Strictly a
//! request and its one answer. Each message is one frame: `[u32 json length][json][u32 audio length][audio]`, the
//! lengths little-endian, the audio 16 kHz mono f32 little-endian (empty when a message has none). The same framing
//! as Handy's worker (engine_supervisor/protocol.rs, MIT).

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A frame larger than this is a broken stream, not a message: an hour of audio is ~230 MB.
const MAX_SECTION: usize = 1 << 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Request {
    /// Load this model file, dropping the one held.
    Load { path: PathBuf },
    /// Recognise the audio that comes with it; `language` — a hint, or the model's own guess.
    Run { language: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    /// The model is in: on which backend (Metal, CPU…) and device.
    Loaded {
        backend: String,
        device: String,
    },
    Text {
        text: String,
    },
    /// The request failed. `backend`: the compute backend failed (a lost GPU), the worker may be unusable.
    Error {
        message: String,
        backend: bool,
    },
}

fn pcm_bytes(pcm: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pcm.len() * 4);
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn pcm_of(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|chunk| f32::from_le_bytes(*chunk)).collect()
}

fn frame(message: &impl Serialize, pcm: &[f32]) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    let audio = pcm_bytes(pcm);
    let mut out = Vec::with_capacity(8 + json.len() + audio.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(audio.len() as u32).to_le_bytes());
    out.extend_from_slice(&audio);
    Ok(out)
}

fn section_len(bytes: [u8; 4]) -> io::Result<usize> {
    let len = u32::from_le_bytes(bytes) as usize;
    if len > MAX_SECTION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes")));
    }
    Ok(len)
}

/// Writes one message (the worker's side).
pub fn write_message(out: &mut impl Write, message: &impl Serialize, pcm: &[f32]) -> io::Result<()> {
    out.write_all(&frame(message, pcm)?)?;
    out.flush()
}

/// Reads one message (the worker's side); None at a clean end of the stream — the gateway is gone.
pub fn read_message<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<Option<(T, Vec<f32>)>> {
    let mut head = [0u8; 4];
    match input.read_exact(&mut head) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut json = vec![0u8; section_len(head)?];
    input.read_exact(&mut json)?;
    input.read_exact(&mut head)?;
    let mut audio = vec![0u8; section_len(head)?];
    input.read_exact(&mut audio)?;
    let message = serde_json::from_slice(&json).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some((message, pcm_of(&audio))))
}

/// Writes one message (the gateway's side).
pub async fn send(out: &mut (impl AsyncWrite + Unpin), message: &impl Serialize, pcm: &[f32]) -> io::Result<()> {
    out.write_all(&frame(message, pcm)?).await?;
    out.flush().await
}

/// Reads one message (the gateway's side); an end of the stream is an error here: the worker is gone.
pub async fn receive<T: DeserializeOwned>(input: &mut (impl AsyncRead + Unpin)) -> io::Result<(T, Vec<f32>)> {
    let mut head = [0u8; 4];
    input.read_exact(&mut head).await?;
    let mut json = vec![0u8; section_len(head)?];
    input.read_exact(&mut json).await?;
    input.read_exact(&mut head).await?;
    let mut audio = vec![0u8; section_len(head)?];
    input.read_exact(&mut audio).await?;
    let message = serde_json::from_slice(&json).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok((message, pcm_of(&audio)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_and_its_audio_come_through_and_a_clean_end_is_none() {
        let mut stream = Vec::new();
        let run = Request::Run { language: Some("ru".into()) };
        write_message(&mut stream, &run, &[0.5, -0.25, 1.0]).unwrap();
        write_message(&mut stream, &Request::Load { path: "/m/model.gguf".into() }, &[]).unwrap();
        let mut reader = stream.as_slice();
        let (message, pcm) = read_message::<Request>(&mut reader).unwrap().unwrap();
        assert_eq!(message, run);
        assert_eq!(pcm, vec![0.5, -0.25, 1.0]);
        let (message, pcm) = read_message::<Request>(&mut reader).unwrap().unwrap();
        assert_eq!(message, Request::Load { path: "/m/model.gguf".into() });
        assert!(pcm.is_empty());
        assert!(read_message::<Request>(&mut reader).unwrap().is_none());
    }

    #[test]
    fn the_wire_names_are_plain_json() {
        let text = serde_json::to_string(&Response::Error { message: "x".into(), backend: true }).unwrap();
        assert_eq!(text, r#"{"type":"error","message":"x","backend":true}"#);
    }

    #[test]
    fn a_garbage_length_is_refused_not_allocated() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_message::<Request>(&mut stream.as_slice()).is_err());
    }

    #[tokio::test]
    async fn the_async_side_reads_what_the_worker_side_writes() {
        let mut stream = Vec::new();
        write_message(&mut stream, &Response::Text { text: "привет".into() }, &[]).unwrap();
        let (message, _) = receive::<Response>(&mut stream.as_slice()).await.unwrap();
        assert_eq!(message, Response::Text { text: "привет".into() });
        assert!(receive::<Response>(&mut [].as_slice()).await.is_err());
    }
}
