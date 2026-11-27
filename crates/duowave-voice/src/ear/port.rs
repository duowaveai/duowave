//! An ear's audio port (docs/plans/audio-interface-3.md, «К2», «К3»; reconciliation 26.11, point 2): what a device's
//! own sound gives the gateway's voice channel — the capture, 16 kHz mono in frames; playing a sound with «played to
//! the end»; pausing and going on; whether the system cancels our own sound in the capture; whether another program
//! holds the microphone; a notification on its screen. The channel (crates/duowave-ears/src/channel) knows nothing
//! more of the device: the Mac app is the first port, Windows, Linux, a phone and a box of our own come later without
//! touching the channel.
//!
//! The app sets its port when it starts, as it sets its folder picker (`set_port`); a gateway without one (Docker, the
//! station, a browser's panel) has no channel.

use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use tokio::sync::mpsc;

/// The capture's rate: what «Слух» takes.
pub const RATE: u32 = 16_000;

/// The captured sound, frame after frame, 16 kHz mono; the sender drops when the capture stops.
pub type Frames = mpsc::UnboundedReceiver<Vec<i16>>;

/// Whether the person let the app hear them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Granted,
    Denied,
    /// Not asked yet: the first capture asks.
    Unknown,
}

/// A device's sound, as its app gives it.
pub trait AudioPort: Send + Sync {
    /// The capture starts; its frames come until `stop_capture`. `voice`: with the system's voice processing (it
    /// cancels our own sound) — while we speak or listen right after; false — the plain capture, which keeps other
    /// programs' music as it is (the word, К10).
    fn start_capture(&self, voice: bool) -> Result<Frames, String>;
    fn stop_capture(&self);
    /// Plays the samples at their rate; true once played to the end, false — stopped or cut.
    fn play(&self, samples: Vec<i16>, rate: u32) -> BoxFuture<'static, bool>;
    /// What plays waits where it is.
    fn pause(&self);
    /// What paused goes on from its place.
    fn resume(&self);
    /// What plays stops, and every `play` waiting returns false.
    fn stop_playing(&self);
    /// The system cancels our own sound in the capture (voice processing is there).
    fn cancels_echo(&self) -> bool;
    /// Another program uses the microphone now: a call.
    fn mic_busy(&self) -> bool;
    fn access(&self) -> Access;
    /// A notification in the screen's corner; tapping it opens the panel.
    fn notify(&self, title: &str, body: &str);
}

static PORT: OnceLock<Arc<dyn AudioPort>> = OnceLock::new();

/// The app's port, set once when it starts.
pub fn set_port(port: Arc<dyn AudioPort>) {
    let _ = PORT.set(port);
}

/// The port this gateway's device has, if its app gave one.
pub fn port() -> Option<Arc<dyn AudioPort>> {
    PORT.get().cloned()
}

/// A port for tests: the test sends the frames, reads what was played and notified, and says how long a sound plays
/// (by its samples, on the test's clock).
#[cfg(any(test, feature = "testing"))]
pub mod fake {
    use std::sync::Mutex;
    use std::time::Duration;

    use tokio::sync::watch;

    use super::*;

    #[derive(Default)]
    struct State {
        capture: Option<mpsc::UnboundedSender<Vec<i16>>>,
        voice: Option<bool>,
        played: Vec<(Vec<i16>, u32)>,
        notified: Vec<(String, String)>,
        paused: bool,
        stops: u64,
        busy: bool,
        no_echo: bool,
    }

    pub struct FakePort {
        state: Mutex<State>,
        /// Bumped by every stop: a `play` going on returns false.
        stopped: watch::Sender<u64>,
        /// Sounds play this fast: 1.0 — as long as their samples, 0.0 — at once.
        pub speed: Mutex<f64>,
    }

    impl Default for FakePort {
        fn default() -> FakePort {
            FakePort { state: Mutex::default(), stopped: watch::channel(0).0, speed: Mutex::new(1.0) }
        }
    }

    impl FakePort {
        fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// The person speaks (or the room is quiet): these samples come as frames of 20 ms. False — no capture.
        pub fn hear(&self, samples: &[i16]) -> bool {
            let Some(capture) = self.state().capture.clone() else { return false };
            samples.chunks(320).all(|frame| capture.send(frame.to_vec()).is_ok())
        }

        /// The capture is on: with the voice processing or plain.
        pub fn capturing(&self) -> Option<bool> {
            let state = self.state();
            state.capture.as_ref().map(|_| state.voice.unwrap_or(false))
        }

        /// What was played, in order: samples and rate.
        pub fn played(&self) -> Vec<(Vec<i16>, u32)> {
            self.state().played.clone()
        }

        pub fn notified(&self) -> Vec<(String, String)> {
            self.state().notified.clone()
        }

        pub fn paused(&self) -> bool {
            self.state().paused
        }

        pub fn set_busy(&self, busy: bool) {
            self.state().busy = busy;
        }

        pub fn set_no_echo_cancelling(&self) {
            self.state().no_echo = true;
        }
    }

    impl AudioPort for FakePort {
        fn start_capture(&self, voice: bool) -> Result<Frames, String> {
            let (sender, receiver) = mpsc::unbounded_channel();
            let mut state = self.state();
            state.capture = Some(sender);
            state.voice = Some(voice);
            Ok(receiver)
        }

        fn stop_capture(&self) {
            let mut state = self.state();
            state.capture = None;
            state.voice = None;
        }

        fn play(&self, samples: Vec<i16>, rate: u32) -> BoxFuture<'static, bool> {
            let seconds = samples.len() as f64 / f64::from(rate.max(1));
            self.state().played.push((samples, rate));
            let wait = Duration::from_secs_f64(seconds * *self.speed.lock().unwrap_or_else(|p| p.into_inner()));
            let mut stopped = self.stopped.subscribe();
            let at = *stopped.borrow_and_update();
            Box::pin(async move {
                tokio::select! {
                    _ = tokio::time::sleep(wait) => true,
                    _ = stopped.wait_for(|now| *now != at) => false,
                }
            })
        }

        fn pause(&self) {
            self.state().paused = true;
        }

        fn resume(&self) {
            self.state().paused = false;
        }

        fn stop_playing(&self) {
            let stops = {
                let mut state = self.state();
                state.paused = false;
                state.stops += 1;
                state.stops
            };
            self.stopped.send_replace(stops);
        }

        fn cancels_echo(&self) -> bool {
            !self.state().no_echo
        }

        fn mic_busy(&self) -> bool {
            self.state().busy
        }

        fn access(&self) -> Access {
            Access::Granted
        }

        fn notify(&self, title: &str, body: &str) {
            self.state().notified.push((title.to_string(), body.to_string()));
        }
    }
}
