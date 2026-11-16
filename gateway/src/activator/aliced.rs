//! The station's own services on its 127.0.0.1, as the gateway on the station reaches them: Alice's `aliced` (asked to
//! listen, as the station's button asks) and where its processed microphone is (`yio_audio`). Their ports are in
//! quasar.cfg; the values seen on Lite firmware are the fallback.
//!
//! The station's IPC: frames of a uint32 LE length, then QuasarMessage {252: the body, 253: its member number}.
//! «Listen» is the member Remoting (210) with {7: "AliceCapability", 8: AliceCapabilityMethod {1: START_CONVERSATION}}
//! — the call the station's own `maind --cli` makes; checked live on Lite 2 on 16.11.2026 (the request started
//! 0.06–0.13 s after it, as «activation directive»).

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::gateway::protocol::proto_edit::put_varint;

/// Where the station keeps the ports of its services.
pub const QUASAR_CFG: &str = "/system/vendor/quasar/quasar.cfg";
/// The ports seen on the Lite stations' firmware when quasar.cfg names none.
pub const AUDIO_PORT: u16 = 9850;
pub const ALICED_PORT: u16 = 9875;
/// The processed microphone: echo- and noise-cancelled, 16 kHz mono PCM16.
pub const MIC_CHANNEL: &str = "vqe_0";

/// QuasarMessage member: a remote call of a capability.
const REMOTING: u64 = 210;
/// AliceCapabilityMethod.method: listen now.
const START_CONVERSATION: u64 = 2;
/// The station answers a call with a state frame or not at all; the call is read either way.
const ANSWER_WAIT: Duration = Duration::from_millis(500);
const CONNECT_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub audio: u16,
    pub aliced: u16,
}

impl Default for Ports {
    fn default() -> Ports {
        Ports { audio: AUDIO_PORT, aliced: ALICED_PORT }
    }
}

/// The ports of quasar.cfg (`yio_audio.port`, `aliced.port`); the usual ones for what it does not name.
pub fn ports(cfg: &Path) -> Ports {
    let text = std::fs::read_to_string(cfg).unwrap_or_default();
    ports_of(&text)
}

pub fn ports_of(text: &str) -> Ports {
    let parsed: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let port = |service: &str| {
        parsed
            .get(service)
            .and_then(|service| service.get("port"))
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok().filter(|port| *port > 0))
    };
    let fallback = Ports::default();
    Ports { audio: port("yio_audio").unwrap_or(fallback.audio), aliced: port("aliced").unwrap_or(fallback.aliced) }
}

fn put_bytes(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// A frame of the station's IPC.
fn frame(member: u64, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(body.len() + 8);
    put_bytes(&mut message, 252, body);
    put_varint(&mut message, 253 << 3);
    put_varint(&mut message, member);
    let mut out = (message.len() as u32).to_le_bytes().to_vec();
    out.extend(message);
    out
}

/// «Listen now», as the station's button: AliceCapability.START_CONVERSATION.
pub fn start_conversation_frame() -> Vec<u8> {
    let mut method = Vec::new();
    put_varint(&mut method, 1 << 3);
    put_varint(&mut method, START_CONVERSATION);
    let mut remoting = Vec::new();
    put_bytes(&mut remoting, 7, b"AliceCapability");
    put_bytes(&mut remoting, 8, &method);
    frame(REMOTING, &remoting)
}

/// Asks the station's Alice to listen now. Done once the call is written; her answer, if any, is only waited for.
pub async fn start_conversation(aliced: SocketAddr) -> Result<(), String> {
    let mut stream = tokio::time::timeout(CONNECT_WAIT, TcpStream::connect(aliced))
        .await
        .map_err(|_| crate::tr!("Алиса на колонке не ответила за 2 с"))?
        .map_err(|error| crate::tr!("Алиса на колонке недоступна: {error}", error = error))?;
    stream
        .write_all(&start_conversation_frame())
        .await
        .map_err(|error| crate::tr!("Алисе на колонке не передать вызов: {error}", error = error))?;
    // Her state frame keeps the socket open until the call was read; none is no error.
    let mut buffer = [0u8; 256];
    let _ = tokio::time::timeout(ANSWER_WAIT, stream.read(&mut buffer)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn the_call_is_the_stations_own_button() {
        // Written byte for byte as the Go client that woke Lite 2 live (16.11) wrote it.
        let mut expected = vec![0xe2, 0x0f, 0x15, 0x3a, 0x0f];
        expected.extend_from_slice(b"AliceCapability");
        expected.extend_from_slice(&[0x42, 0x02, 0x08, 0x02, 0xe8, 0x0f, 0xd2, 0x01]);
        let mut framed = (expected.len() as u32).to_le_bytes().to_vec();
        framed.extend(expected);
        assert_eq!(start_conversation_frame(), framed);
    }

    #[test]
    fn the_ports_come_from_quasar_cfg_or_the_usual_ones() {
        assert_eq!(
            ports_of(r#"{"yio_audio": {"port": 9950}, "aliced": {"port": 9975, "apiKey": "x"}}"#),
            Ports { audio: 9950, aliced: 9975 }
        );
        assert_eq!(ports_of(r#"{"aliced": {"uniProxyUrl": "ws://x"}}"#), Ports::default());
        assert_eq!(ports_of("not json"), Ports::default());
        assert_eq!(ports(Path::new("/nowhere/quasar.cfg")), Ports::default());
    }

    #[tokio::test]
    async fn the_station_gets_the_call() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let station = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut length = [0u8; 4];
            socket.read_exact(&mut length).await.unwrap();
            let mut body = vec![0u8; u32::from_le_bytes(length) as usize];
            socket.read_exact(&mut body).await.unwrap();
            body
        });
        start_conversation(address).await.unwrap();
        let body = station.await.unwrap();
        assert_eq!(body, start_conversation_frame()[4..].to_vec());
    }

    #[tokio::test]
    async fn no_station_says_so() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = start_conversation(address).await.unwrap_err();
        assert!(error.contains("недоступна"), "{error}");
    }
}
