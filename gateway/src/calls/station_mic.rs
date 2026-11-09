//! The station's processed microphone from its own audio service: YandexIO's SocketAudioSource on the station's
//! 127.0.0.1:9850 (the `yio_audio` port of quasar.cfg; protocol read from maind and checked live on 02.11). The
//! station client bridges that port to us with socat; we subscribe for the length of a call and take the channel
//! the station's own calls use — vqe_gain: 16 kHz mono PCM16, echo-cancelled against whatever the speaker plays,
//! the call we play included.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::operations::networks::in_networks;

/// TYPE_IO_AUDIO_CHANNELS_DATA: the server's IOAudioChannelsData, every channel of the last 32 ms.
pub const AUDIO_CHANNELS_DATA: u64 = 158;
/// TYPE_IO_CHANNEL_REQUEST: the client's IOChannelRequest.
pub const CHANNEL_REQUEST: u64 = 159;
/// The channel the station's own calls send (phone_calls.pjsip.audio_source.channel in quasar.cfg).
pub const CALL_CHANNEL: &str = "vqe_gain";

/// No real frame comes near this: the stream went out of step.
const MAX_FRAME: usize = 1 << 20;

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn bytes_field(field: u64, value: &[u8], out: &mut Vec<u8>) {
    encode_varint(field * 8 + 2, out);
    encode_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

/// A frame of the station's IPC: a uint32 LE length, then QuasarMessage {252: body, 253: type}.
pub fn ipc_frame(kind: u64, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(body.len() + 8);
    bytes_field(252, body, &mut message);
    encode_varint(253 * 8, &mut message);
    encode_varint(kind, &mut message);
    let mut frame = (message.len() as u32).to_le_bytes().to_vec();
    frame.extend(message);
    frame
}

/// IOChannelRequest with field 2: the group the server sends every channel to.
pub static SUBSCRIBE: LazyLock<Vec<u8>> = LazyLock::new(|| ipc_frame(CHANNEL_REQUEST, &[0x12, 0x00]));
/// IOChannelRequest with field 6: out of every group.
pub static UNSUBSCRIBE: LazyLock<Vec<u8>> = LazyLock::new(|| ipc_frame(CHANNEL_REQUEST, &[0x32, 0x00]));

fn varint(buf: &[u8], mut at: usize) -> Result<(u64, usize), String> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while at < buf.len() {
        let byte = buf[at];
        at += 1;
        if shift < 64 {
            value |= u64::from(byte & 0x7f) << shift;
        }
        if byte < 0x80 {
            return Ok((value, at));
        }
        shift += 7;
    }
    Err("truncated varint".into())
}

enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
}

/// The fields of a message, length-delimited ones as slices; other wire types skipped.
fn fields(buf: &[u8]) -> Result<Vec<(u64, Value<'_>)>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let (key, next) = varint(buf, at)?;
        at = next;
        match key & 7 {
            0 => {
                let (value, next) = varint(buf, at)?;
                at = next;
                out.push((key >> 3, Value::Number(value)));
            }
            2 => {
                let (length, next) = varint(buf, at)?;
                let end = next.saturating_add(usize::try_from(length).unwrap_or(usize::MAX)).min(buf.len());
                out.push((key >> 3, Value::Bytes(&buf[next..end])));
                at = end;
            }
            1 => at += 8,
            5 => at += 4,
            wire => return Err(format!("wire type {wire}")),
        }
    }
    Ok(out)
}

/// One channel out of the server's byte stream: frames split wherever TCP splits them, IOAudioChannelsData
/// (field 1: IOAudioChannelData {2: name, 3: PCM16LE, 7: rate}) picked from the rest (VqeInfo and the like).
pub struct ChannelReader {
    pending: Vec<u8>,
    channel: String,
}

impl Default for ChannelReader {
    fn default() -> Self {
        ChannelReader::new(CALL_CHANNEL)
    }
}

impl ChannelReader {
    pub fn new(channel: &str) -> ChannelReader {
        ChannelReader { pending: Vec::new(), channel: channel.to_string() }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<i16>>, String> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut at = 0;
        while self.pending.len() - at >= 4 {
            let header = [self.pending[at], self.pending[at + 1], self.pending[at + 2], self.pending[at + 3]];
            let length = u32::from_le_bytes(header) as usize;
            if length > MAX_FRAME {
                return Err(format!("a frame of {length} bytes: out of step"));
            }
            if self.pending.len() - at < 4 + length {
                break;
            }
            let message = &self.pending[at + 4..at + 4 + length];
            if let Some(pcm) = self.samples(message)? {
                out.push(pcm);
            }
            at += 4 + length;
        }
        self.pending.drain(..at);
        Ok(out)
    }

    fn samples(&self, message: &[u8]) -> Result<Option<Vec<i16>>, String> {
        let mut kind = None;
        let mut body = None;
        for (field, value) in fields(message)? {
            match (field, value) {
                (253, Value::Number(number)) => kind = Some(number),
                (252, Value::Bytes(bytes)) => body = Some(bytes),
                _ => {}
            }
        }
        let (Some(AUDIO_CHANNELS_DATA), Some(body)) = (kind, body) else { return Ok(None) };
        for (field, data) in fields(body)? {
            let (1, Value::Bytes(data)) = (field, data) else { continue };
            let mut name: &[u8] = &[];
            let mut pcm = None;
            for (inner, value) in fields(data)? {
                match (inner, value) {
                    (2, Value::Bytes(bytes)) => name = bytes,
                    (3, Value::Bytes(bytes)) => pcm = Some(bytes),
                    _ => {}
                }
            }
            if let Some(pcm) = pcm.filter(|_| name == self.channel.as_bytes()) {
                return Ok(Some(pcm.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect()));
            }
        }
        Ok(None)
    }
}

/// "::ffff:192.0.2.23" → "192.0.2.23": one key for the bridge and the listening stream.
pub fn plain(address: &str) -> &str {
    match address.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("::ffff:") => &address[7..],
        _ => address,
    }
}

struct Bridge {
    id: u64,
    writer: mpsc::UnboundedSender<&'static [u8]>,
    closed: CancellationToken,
    subscribed: bool,
    /// Samples of the channel since the subscription: what the log tells at its end.
    samples: Arc<AtomicU64>,
}

type OnAudio = Box<dyn Fn(&str, &[i16]) + Send + Sync>;
type OnAddress = Box<dyn Fn(&str) + Send + Sync>;

/// The station clients' microphone bridges (CALLS_STATION_MIC_PORT): a bridge is known by the station's address,
/// the same one its listening stream comes from. Subscribed only while that station is in a call, so the station
/// does not send its seven channels (~230 KB/s) for nothing.
pub struct StationMics {
    bridges: Mutex<HashMap<String, Bridge>>,
    on_audio: OnAudio,
    /// A bridge came: its station may be in a call already.
    on_bridge: OnAddress,
    /// A bridge went: its station's client stopped, or the station is gone.
    on_bridge_closed: OnAddress,
    closed: CancellationToken,
    next: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl StationMics {
    pub fn new(
        on_audio: impl Fn(&str, &[i16]) + Send + Sync + 'static,
        on_bridge: impl Fn(&str) + Send + Sync + 'static,
        on_bridge_closed: impl Fn(&str) + Send + Sync + 'static,
    ) -> Arc<StationMics> {
        Arc::new(StationMics {
            bridges: Mutex::default(),
            on_audio: Box::new(on_audio),
            on_bridge: Box::new(on_bridge),
            on_bridge_closed: Box::new(on_bridge_closed),
            closed: CancellationToken::new(),
            next: AtomicU64::new(1),
        })
    }

    /// Listens on `host:port`; the port it took (0 asks for any).
    pub async fn listen(self: &Arc<Self>, port: u16, host: &str) -> std::io::Result<u16> {
        let listener = TcpListener::bind((host, port)).await?;
        let port = listener.local_addr()?.port();
        let mics = self.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = mics.closed.cancelled() => return,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((socket, peer)) => mics.accept(socket, &peer.ip().to_string()),
                    Err(error) => tracing::warn!("station microphone port: {error}"),
                }
            }
        });
        Ok(port)
    }

    pub fn close(&self) {
        self.closed.cancel();
        for bridge in lock(&self.bridges).values() {
            bridge.closed.cancel();
        }
    }

    pub fn has(&self, address: &str) -> bool {
        lock(&self.bridges).contains_key(plain(address))
    }

    pub fn subscribe(&self, address: &str) {
        let address = plain(address);
        let mut bridges = lock(&self.bridges);
        let Some(bridge) = bridges.get_mut(address).filter(|bridge| !bridge.subscribed) else { return };
        bridge.subscribed = true;
        bridge.samples.store(0, Ordering::Relaxed);
        let _ = bridge.writer.send(SUBSCRIBE.as_slice());
        tracing::info!("{address}: microphone on");
    }

    pub fn unsubscribe(&self, address: &str) {
        let address = plain(address);
        let mut bridges = lock(&self.bridges);
        let Some(bridge) = bridges.get_mut(address).filter(|bridge| bridge.subscribed) else { return };
        bridge.subscribed = false;
        let _ = bridge.writer.send(UNSUBSCRIBE.as_slice());
        let seconds = bridge.samples.load(Ordering::Relaxed) as f64 / 16000.0;
        tracing::info!("{address}: microphone off, {seconds:.1}s of {CALL_CHANNEL} taken");
    }

    fn accept(self: &Arc<Self>, socket: TcpStream, peer: &str) {
        let address = plain(peer).to_string();
        if !in_networks(&address, &["private".to_string()]) {
            return;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (writer, mut outgoing) = mpsc::unbounded_channel::<&'static [u8]>();
        let closed = self.closed.child_token();
        let samples = Arc::new(AtomicU64::new(0));
        let bridge = Bridge { id, writer, closed: closed.clone(), subscribed: false, samples: samples.clone() };
        if let Some(previous) = lock(&self.bridges).insert(address.clone(), bridge) {
            previous.closed.cancel();
        }
        tracing::info!("{address}: microphone bridge connected");
        (self.on_bridge)(&address);
        let mics = self.clone();
        tokio::spawn(async move {
            let (mut read, mut write) = socket.into_split();
            let mut reader = ChannelReader::default();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                tokio::select! {
                    _ = closed.cancelled() => break,
                    frame = outgoing.recv() => match frame {
                        Some(frame) => if write.write_all(frame).await.is_err() { break },
                        None => break,
                    },
                    read = read.read(&mut buffer) => match read {
                        Ok(0) | Err(_) => break,
                        Ok(n) => match reader.push(&buffer[..n]) {
                            Ok(chunks) => for pcm in chunks {
                                samples.fetch_add(pcm.len() as u64, Ordering::Relaxed);
                                (mics.on_audio)(&address, &pcm);
                            },
                            Err(problem) => {
                                tracing::warn!("{address}: {problem}; the bridge is dropped");
                                break;
                            }
                        },
                    },
                }
            }
            let current = {
                let mut bridges = lock(&mics.bridges);
                let current = bridges.get(&address).is_some_and(|bridge| bridge.id == id);
                if current {
                    bridges.remove(&address);
                }
                current
            };
            tracing::info!("{address}: microphone bridge closed");
            // A bridge replaced by a newer one from the same address is not the station leaving.
            if current {
                (mics.on_bridge_closed)(&address);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn number(field: u64, value: u64, out: &mut Vec<u8>) {
        encode_varint(field * 8, out);
        encode_varint(value, out);
    }

    /// IOAudioChannelData as the station sends it: type, name, PCM, a flag, the rate, the channel count, the time.
    fn channel(name: &str, samples: &[i16]) -> Vec<u8> {
        let pcm: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let mut data = Vec::new();
        number(1, 9, &mut data);
        bytes_field(2, name.as_bytes(), &mut data);
        bytes_field(3, &pcm, &mut data);
        number(4, 0, &mut data);
        number(7, 16000, &mut data);
        number(8, 1, &mut data);
        number(9, 53775903, &mut data);
        let mut out = Vec::new();
        bytes_field(1, &data, &mut out);
        out
    }

    #[test]
    fn subscribes_and_unsubscribes_with_the_frames_checked_on_the_station_02_11() {
        assert_eq!(hex::encode(SUBSCRIBE.as_slice()), "09000000e20f021200e80f9f01");
        assert_eq!(hex::encode(UNSUBSCRIBE.as_slice()), "09000000e20f023200e80f9f01");
    }

    #[test]
    fn takes_vqe_gain_out_of_a_stream_split_anywhere_past_the_vqe_info_and_the_other_channels() {
        let stream = [
            ipc_frame(233, br#"{"vqe":"yandex"}"#),
            ipc_frame(
                AUDIO_CHANNELS_DATA,
                &[channel("vqe_0", &[1, 2]), channel("vqe_gain", &[100, -200, 300])].concat(),
            ),
            ipc_frame(248, &[8, 1]),
            ipc_frame(AUDIO_CHANNELS_DATA, &[channel("raw_mic_0", &[7]), channel("vqe_gain", &[-5, 6])].concat()),
        ]
        .concat();
        let mut reader = ChannelReader::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(7) {
            got.extend(reader.push(chunk).unwrap());
        }
        assert_eq!(got, vec![vec![100, -200, 300], vec![-5, 6]]);
    }

    #[test]
    fn gives_up_on_a_stream_out_of_step() {
        let length = (5u32 << 20).to_le_bytes();
        assert!(ChannelReader::default().push(&length).unwrap_err().contains("out of step"));
        assert_eq!(plain("::FFFF:192.0.2.23"), "192.0.2.23");
        assert_eq!(plain("10.0.0.2"), "10.0.0.2");
    }

    #[tokio::test]
    async fn subscribes_a_bridge_for_a_call_and_passes_its_stations_voice_on() {
        let heard = Arc::new(Mutex::new(Vec::<(String, Vec<i16>)>::new()));
        let bridges = Arc::new(Mutex::new(Vec::<String>::new()));
        let (sink, came) = (heard.clone(), bridges.clone());
        let mics = StationMics::new(
            move |address, pcm| lock(&sink).push((address.to_string(), pcm.to_vec())),
            move |address| lock(&came).push(address.to_string()),
            |_| {},
        );
        let port = mics.listen(0, "127.0.0.1").await.unwrap();
        let mut station = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*lock(&bridges), vec!["127.0.0.1".to_string()]);

        mics.subscribe("::ffff:127.0.0.1");
        mics.subscribe("127.0.0.1");
        station.write_all(&ipc_frame(AUDIO_CHANNELS_DATA, &channel("vqe_gain", &[42, 43]))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        mics.unsubscribe("127.0.0.1");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(*lock(&heard), vec![("127.0.0.1".to_string(), vec![42, 43])]);
        let mut received = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(1), station.read(&mut received)).await.unwrap().unwrap();
        let mut received = received[..n].to_vec();
        while received.len() < SUBSCRIBE.len() + UNSUBSCRIBE.len() {
            let mut more = vec![0u8; 64];
            let n = tokio::time::timeout(Duration::from_secs(1), station.read(&mut more)).await.unwrap().unwrap();
            received.extend_from_slice(&more[..n]);
        }
        assert_eq!(received, [SUBSCRIBE.as_slice(), UNSUBSCRIBE.as_slice()].concat());
        drop(station);
        mics.close();
    }
}
