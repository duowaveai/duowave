//! The channel's voice (docs/plans/audio-interface-3.md, «К2»): the answer's phrases as they come, each in the
//! computer's voice (`VoiceService::for_computer`, its ready phrases first), one synthesized ahead, played in order
//! through the port. It remembers which were said to their end: the history keeps only those (К7).
//!
//! Its own task: the channel sends what to say and what to do; the mouth tells what became of each phrase.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::Speak;
use crate::ear::port::AudioPort;

/// How many phrases are synthesized ahead of the one playing.
const AHEAD: usize = 1;
/// A phrase still «playing» this long after its own length has not been played: the port never said it ended (27.11,
/// the trial of main: the Mac's engine stopped under a scheduled buffer, and the channel said «Отвечаю» for good).
const PLAY_GRACE: Duration = Duration::from_secs(3);
/// A phrase the voice has not made in this time is not said.
const SYNTHESIS_LIMIT: Duration = Duration::from_secs(30);

/// What the channel asks of the mouth.
#[derive(Debug)]
pub enum Ask {
    /// A phrase to say after the ones before it.
    Say {
        id: u64,
        text: String,
    },
    /// Silence now: what plays stops, what waits is dropped.
    Stop,
    Pause,
    Resume,
}

/// What became of a phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// It began to sound.
    Started { id: u64 },
    /// It ended: `full` — said to its end.
    Ended { id: u64, full: bool },
    /// Nothing waits and nothing plays.
    Idle,
}

/// The mouth's handle.
#[derive(Clone)]
pub struct Mouth {
    asks: mpsc::UnboundedSender<Ask>,
}

impl Mouth {
    /// The mouth starts; what it tells comes to `told`.
    pub fn start(port: Arc<dyn AudioPort>, voice: Arc<dyn Speak>, told: mpsc::UnboundedSender<Told>) -> Mouth {
        let (asks, receiver) = mpsc::unbounded_channel();
        tokio::spawn(run(port, voice, receiver, told));
        Mouth { asks }
    }

    pub fn ask(&self, ask: Ask) {
        let _ = self.asks.send(ask);
    }
}

type Synthesis = (u64, Option<(Vec<i16>, u32)>);

async fn run(
    port: Arc<dyn AudioPort>,
    voice: Arc<dyn Speak>,
    mut asks: mpsc::UnboundedReceiver<Ask>,
    told: mpsc::UnboundedSender<Told>,
) {
    let mut waiting: VecDeque<(u64, String)> = VecDeque::new();
    let mut ready: HashMap<u64, Option<(Vec<i16>, u32)>> = HashMap::new();
    let mut making: Vec<u64> = Vec::new();
    let (made_by, mut made) = mpsc::unbounded_channel::<Synthesis>();
    let mut cancel = CancellationToken::new();
    let mut playing: Option<(u64, BoxFuture<'static, bool>)> = None;
    // When the phrase playing must have ended, and since when it is paused (a pause does not count).
    let mut deadline = Instant::now();
    let mut paused: Option<Instant> = None;
    let mut idle = true;
    loop {
        // Synthesize ahead: the next phrases not made yet.
        for (id, text) in waiting.iter().take(AHEAD + 1) {
            if ready.contains_key(id) || making.contains(id) {
                continue;
            }
            making.push(*id);
            let (id, text, voice, made_by, cancel) =
                (*id, text.clone(), voice.clone(), made_by.clone(), cancel.clone());
            tokio::spawn(async move {
                let made = match tokio::time::timeout(SYNTHESIS_LIMIT, voice.synthesize(&text, &cancel)).await {
                    Ok(made) => made,
                    Err(_) => {
                        tracing::warn!(target: "talk", "the computer's voice made no sound in {SYNTHESIS_LIMIT:?}");
                        None
                    }
                };
                let _ = made_by.send((id, made));
            });
        }
        // The next phrase plays once it is made, in order.
        if playing.is_none() {
            if let Some((id, _)) = waiting.front() {
                if let Some(sound) = ready.remove(id) {
                    let id = *id;
                    waiting.pop_front();
                    match sound {
                        Some((samples, rate)) => {
                            let _ = told.send(Told::Started { id });
                            let length = Duration::from_millis(samples.len() as u64 * 1000 / u64::from(rate.max(1)));
                            deadline = Instant::now() + length + PLAY_GRACE;
                            paused = None;
                            playing = Some((id, port.play(samples, rate)));
                        }
                        None => {
                            let _ = told.send(Told::Ended { id, full: false });
                            continue;
                        }
                    }
                }
            }
        }
        let quiet = playing.is_none() && waiting.is_empty();
        if quiet && !idle {
            idle = true;
            let _ = told.send(Told::Idle);
        }
        tokio::select! {
            ask = asks.recv() => match ask {
                None => return,
                Some(Ask::Say { id, text }) => {
                    idle = false;
                    waiting.push_back((id, text));
                }
                Some(Ask::Stop) => {
                    paused = None;
                    cancel.cancel();
                    cancel = CancellationToken::new();
                    port.stop_playing();
                    if let Some((id, _)) = playing.take() {
                        let _ = told.send(Told::Ended { id, full: false });
                    }
                    for (id, _) in waiting.drain(..) {
                        let _ = told.send(Told::Ended { id, full: false });
                    }
                    ready.clear();
                    making.clear();
                }
                Some(Ask::Pause) => {
                    paused.get_or_insert_with(Instant::now);
                    port.pause();
                }
                Some(Ask::Resume) => {
                    if let Some(since) = paused.take() {
                        deadline += since.elapsed();
                    }
                    port.resume();
                }
            },
            Some((id, sound)) = made.recv() => {
                making.retain(|making| *making != id);
                if waiting.iter().any(|(waits, _)| *waits == id) {
                    ready.insert(id, sound);
                }
            }
            full = async {
                match playing.as_mut() {
                    Some((_, play)) => play.await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some((id, _)) = playing.take() {
                    let _ = told.send(Told::Ended { id, full });
                }
            }
            _ = tokio::time::sleep_until(deadline), if playing.is_some() && paused.is_none() => {
                tracing::warn!(target: "talk", "the computer's voice did not say a phrase ended: it is taken as cut");
                port.stop_playing();
                if let Some((id, _)) = playing.take() {
                    let _ = told.send(Told::Ended { id, full: false });
                }
            }
        }
    }
}

/// The two sounds of the microphone (the panel's tones.ts): «слушаю» — 660 Hz, then 880 Hz; «услышала» — 520 Hz.
pub mod tones {
    use super::super::endpoint::RATE;

    fn tone(hz: f64, ms: u64, gap_ms: u64) -> Vec<i16> {
        let count = (RATE as u64 * ms / 1000) as usize;
        let fade = (RATE as usize / 200).min(count / 2).max(1);
        let mut samples: Vec<i16> = (0..count)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let edge = (i.min(count - 1 - i) as f64 / fade as f64).min(1.0);
                (0.18 * edge * (2.0 * std::f64::consts::PI * hz * t).sin() * 32767.0) as i16
            })
            .collect();
        samples.extend(std::iter::repeat_n(0, (RATE as u64 * gap_ms / 1000) as usize));
        samples
    }

    /// «Слушаю»: the microphone opens.
    pub fn listen() -> Vec<i16> {
        [tone(660.0, 90, 30), tone(880.0, 110, 0)].concat()
    }

    /// «Услышала»: the thought went on its way.
    pub fn heard() -> Vec<i16> {
        tone(520.0, 120, 0)
    }

    /// How long a sound plays, in ms.
    pub fn ms(samples: &[i16]) -> u64 {
        samples.len() as u64 * 1000 / u64::from(RATE)
    }
}

/// A sound played and forgotten: the tones go on while the channel goes on.
pub fn sound(port: &Arc<dyn AudioPort>, samples: Vec<i16>) {
    let play = port.play(samples, super::endpoint::RATE).map(|_| ());
    tokio::spawn(play);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tones_are_short_and_the_listening_one_is_two() {
        let listen = tones::listen();
        assert!((200..300).contains(&tones::ms(&listen)), "{} ms", tones::ms(&listen));
        assert!(tones::ms(&tones::heard()) < 200);
        assert!(listen.iter().any(|&sample| sample.unsigned_abs() > 5000), "audible");
    }

    /// Half a second of sound at 16 kHz for every phrase, made at once.
    struct HalfSecond;

    impl Speak for HalfSecond {
        fn ready(&self) -> bool {
            true
        }

        fn synthesize(&self, _text: &str, _cancel: &CancellationToken) -> BoxFuture<'static, Option<(Vec<i16>, u32)>> {
            async { Some((vec![100; 8000], 16_000)) }.boxed()
        }
    }

    async fn next(told: &mut mpsc::UnboundedReceiver<Told>) -> Told {
        tokio::time::timeout(Duration::from_secs(120), told.recv()).await.expect("the mouth told").expect("open")
    }

    /// 27.11, the trial of main: the Mac's engine stopped under a scheduled buffer, its «played» never came, and the
    /// channel said «Отвечаю» for good; the next press only stopped that answer.
    #[tokio::test(start_paused = true)]
    async fn a_phrase_the_port_never_ends_is_taken_as_cut_after_its_length() {
        let port = Arc::new(crate::ear::port::fake::FakePort::default());
        *port.speed.lock().unwrap() = 1000.0;
        let (told_by, mut told) = mpsc::unbounded_channel();
        let mouth = Mouth::start(port.clone(), Arc::new(HalfSecond), told_by);
        let began = Instant::now();
        mouth.ask(Ask::Say {
            id: 1, text: "Курс соланы — семьдесят три доллара.".into()
        });
        assert_eq!(next(&mut told).await, Told::Started { id: 1 });
        assert_eq!(next(&mut told).await, Told::Ended { id: 1, full: false });
        let after = began.elapsed();
        assert!(after >= Duration::from_millis(3500) && after < Duration::from_secs(5), "{after:?}");
        assert_eq!(next(&mut told).await, Told::Idle);
        // The next phrase plays as usual.
        *port.speed.lock().unwrap() = 1.0;
        mouth.ask(Ask::Say { id: 2, text: "Ещё что-нибудь?".into() });
        assert_eq!(next(&mut told).await, Told::Started { id: 2 });
        assert_eq!(next(&mut told).await, Told::Ended { id: 2, full: true });
    }

    /// Speaking over the computer pauses the phrase: the pause does not count against its time.
    #[tokio::test(start_paused = true)]
    async fn a_paused_phrase_is_not_cut_by_its_time() {
        let port = Arc::new(crate::ear::port::fake::FakePort::default());
        *port.speed.lock().unwrap() = 1000.0;
        let (told_by, mut told) = mpsc::unbounded_channel();
        let mouth = Mouth::start(port.clone(), Arc::new(HalfSecond), told_by);
        mouth.ask(Ask::Say { id: 1, text: "Длинный ответ.".into() });
        assert_eq!(next(&mut told).await, Told::Started { id: 1 });
        mouth.ask(Ask::Pause);
        tokio::time::sleep(Duration::from_secs(20)).await;
        assert!(told.try_recv().is_err(), "nothing ended while paused");
        let resumed = Instant::now();
        mouth.ask(Ask::Resume);
        assert_eq!(next(&mut told).await, Told::Ended { id: 1, full: false });
        assert!(resumed.elapsed() >= Duration::from_secs(3), "{:?}", resumed.elapsed());
    }
}
