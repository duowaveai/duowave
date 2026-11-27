//! The computer's own ear and voice on a Mac (docs/plans/audio-interface-3.md, «К3»): the audio port the gateway's
//! voice channel stands on (`duowave::ear::port::AudioPort`).
//!
//! - One `AVAudioEngine`: the capture is a tap on its input node, the speech goes through its `AVAudioPlayerNode`, so
//!   the system's voice processing — the echo cancelling, noise suppression and gain FaceTime and Zoom use — knows
//!   what we play and takes it out of what we hear.
//! - The capture comes in the input's own format; its first channel is taken and brought to 16 kHz mono here.
//! - With the word on (К10) the capture runs all the time, plain, without voice processing: switched on all the time
//!   it would hush other programs' music, and there is nothing to cancel while the computer is silent. It is switched
//!   on when the channel asks for it (we listen or speak) and off after. Each switch is a short gap in the sound.
//! - The engine runs only while there is something to hear or to play, and stops 3 seconds after.
//! - «The microphone is held by another program»: the default input runs somewhere while our capture does not
//!   (`kAudioDevicePropertyDeviceIsRunningSomewhere`).
//! - A notification goes through the app's notifications plugin.
//!
//! What this does on a real Mac — the formats voice processing gives, the gaps, the echo — is checked by hand.

use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use block2::RcBlock;
use duowave::ear::port::{Access, AudioPort, Frames, RATE};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObjectProtocol};
use objc2::{sel, AllocAnyThread};
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioFormat, AVAudioPCMBuffer,
    AVAudioPlayerNode, AVAudioTime, AVAudioVoiceProcessingOtherAudioDuckingConfiguration,
    AVAudioVoiceProcessingOtherAudioDuckingLevel,
};
use objc2_core_audio::{
    kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioHardwarePropertyDefaultInputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, oneshot};

/// What the player plays: everything is brought to this rate first, so the player keeps one connection.
const PLAY_RATE: f64 = 48_000.0;
/// The engine stops this long after nothing is heard or played — counted from the end of the last sound.
const IDLE: Duration = Duration::from_secs(3);
/// A capture that has given no sound at all this long starts over (and again after as long, until sound comes).
const SILENT_CAPTURE: Duration = Duration::from_millis(1500);
/// Silence before the first sound of an engine just started: the output wakes up meanwhile (27.11, the trial of main:
/// a phrase played at the start was heard «from the middle»).
const LEAD_IN: Duration = Duration::from_millis(200);

/// The engine and its nodes. Objective-C objects are not `Send`; they are touched under the port's lock only.
struct Engine {
    engine: Retained<AVAudioEngine>,
    player: Retained<AVAudioPlayerNode>,
    format: Retained<AVAudioFormat>,
    voice: bool,
    tapped: bool,
}

// SAFETY: AVAudioEngine and its nodes may be used from any thread, one call at a time; every call is under the lock.
unsafe impl Send for Engine {}

/// The Mac's audio port.
pub struct MacPort {
    engine: Mutex<Option<Engine>>,
    /// Where the tap sends the captured frames: None — nobody listens.
    capture: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<i16>>>>>,
    /// Bumped by every stop: a buffer that ends after it was not played to its end.
    stops: Arc<AtomicU64>,
    /// Buffers given to the player and not done yet.
    playing: Arc<AtomicUsize>,
    /// Voice processing switched on and working: the echo is cancelled.
    cancels: AtomicBool,
    /// The last sound heard or played: a buffer's end counts too.
    last_use: Arc<Mutex<Instant>>,
    /// A capture began before the person answered macOS's question about the microphone: once they allow it, the
    /// capture starts over (27.11, the trial of main: «Слушаю» stayed on and nothing came after «Разрешить»).
    awaiting_access: AtomicBool,
    /// When the tap last gave a frame.
    frame_at: Arc<Mutex<Option<Instant>>>,
    /// When the capture going on began; None — no capture.
    capture_at: Mutex<Option<Instant>>,
    /// When a silent capture was last started over, and whether the log has said so for this capture.
    revived_at: Mutex<Option<Instant>>,
    revived_told: AtomicBool,
    app: AppHandle,
}

impl MacPort {
    /// The port, with its watch that stops the engine when it is idle.
    pub fn new(app: AppHandle) -> Arc<MacPort> {
        let port = Arc::new(MacPort {
            engine: Mutex::new(None),
            capture: Arc::new(Mutex::new(None)),
            stops: Arc::new(AtomicU64::new(0)),
            playing: Arc::new(AtomicUsize::new(0)),
            cancels: AtomicBool::new(true),
            last_use: Arc::new(Mutex::new(Instant::now())),
            awaiting_access: AtomicBool::new(false),
            frame_at: Arc::new(Mutex::new(None)),
            capture_at: Mutex::new(None),
            revived_at: Mutex::new(None),
            revived_told: AtomicBool::new(false),
            app,
        });
        let watched = Arc::downgrade(&port);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            let Some(port) = watched.upgrade() else { return };
            port.after_access();
            port.revive_capture();
            port.stop_when_idle();
        });
        port
    }

    /// macOS has not asked about the microphone yet (macOS 14 and later can tell).
    fn asking_access(&self) -> bool {
        AnyClass::get(c"AVAudioApplication").is_some()
            // SAFETY: the shared instance's read-only property.
            && unsafe { AVAudioApplication::sharedInstance().recordPermission() }
                == AVAudioApplicationRecordPermission::Undetermined
    }

    /// The person answered macOS's question: on «Разрешить» a capture going on starts over with a fresh engine — the
    /// one made before the answer gives nothing — as if the key were pressed again.
    fn after_access(&self) {
        if !self.awaiting_access.load(Ordering::SeqCst) || self.asking_access() {
            return;
        }
        self.awaiting_access.store(false, Ordering::SeqCst);
        // Not waiting for what plays: without the microphone the engine never played the «слушаю» tone, and its end
        // never came (27.11, the trial of main: the start over waited for it and never happened).
        if self.access() != Access::Granted || !self.capturing() {
            return;
        }
        self.restart_capture(Some("the microphone was allowed: the capture started over"));
    }

    /// A capture that gives no sound at all: macOS held it while it asked about the microphone, and a capture begun
    /// before the answer stays silent after «Разрешить» (27.11, the trial of main: macOS did not tell at once that the
    /// microphone was allowed). It starts over every `SILENT_CAPTURE` until sound comes.
    fn revive_capture(&self) {
        let began = *self.capture_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(began) = began else { return };
        if began.elapsed() < SILENT_CAPTURE || self.access() == Access::Denied {
            return;
        }
        let heard = self
            .frame_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some_and(|at| at >= began);
        if heard {
            if self.revived_told.swap(false, Ordering::SeqCst) {
                self.note("the microphone gives sound now");
            }
            return;
        }
        {
            let mut revived = self.revived_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if revived.is_some_and(|at| at.elapsed() < SILENT_CAPTURE) {
                return;
            }
            *revived = Some(Instant::now());
        }
        let first = !self.revived_told.swap(true, Ordering::SeqCst);
        self.restart_capture(
            first.then_some("the microphone gives no sound (macOS asking?): the capture starts over until it does"),
        );
    }

    /// The capture going on starts over with a fresh engine; what played is ended as cut. `why` — for the log.
    fn restart_capture(&self, why: Option<&str>) {
        let mut slot = self.lock();
        let voice = slot.as_ref().is_some_and(|engine| engine.voice);
        if let Some(engine) = slot.as_mut() {
            self.stops.fetch_add(1, Ordering::SeqCst);
            // SAFETY: the engine's calls, under the lock; stopping the player ends what it held.
            unsafe {
                engine.player.stop();
                if engine.tapped {
                    engine.engine.inputNode().removeTapOnBus(0);
                }
                if engine.engine.isRunning() {
                    engine.engine.stop();
                }
            }
        }
        *slot = None;
        // The voice processing first, the tap after it: switching it on changes the input, and a tap put on before
        // gave nothing (27.11, the trial of main: every start over stayed silent after «Разрешить»).
        let restarted = self.engine(&mut slot).and_then(|engine| {
            let started = self.run(engine, voice);
            self.tap(engine);
            started
        });
        drop(slot);
        match (restarted, why) {
            (Ok(_), Some(why)) => self.note(why),
            (Ok(_), None) => {}
            (Err(error), why) => self.note(&format!(
                "{}: the capture did not start over: {error}",
                why.unwrap_or("the microphone")
            )),
        }
    }

    /// A line of the port's own in the gateway's log (the app's `[app …]` lines).
    fn note(&self, line: &str) {
        if let Some(gateway) = self.app.try_state::<Arc<crate::engine::Gateway>>() {
            gateway.note(line);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Engine>> {
        self.engine.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn used(&self) {
        *self.last_use.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    fn capturing(&self) -> bool {
        self.capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    /// The engine, made the first time: the player attached and connected to the main mixer at PLAY_RATE.
    fn engine<'a>(&self, slot: &'a mut Option<Engine>) -> Result<&'a mut Engine, String> {
        if slot.is_none() {
            // SAFETY: plain Objective-C object creation and graph building, under the lock.
            let made = unsafe {
                let engine = AVAudioEngine::new();
                let player = AVAudioPlayerNode::new();
                engine.attachNode(&player);
                let format =
                    AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), PLAY_RATE, 1)
                        .ok_or_else(|| "no audio format for the player".to_string())?;
                engine.connect_to_format(&player, &engine.mainMixerNode(), Some(&format));
                Engine {
                    engine,
                    player,
                    format,
                    voice: false,
                    tapped: false,
                }
            };
            *slot = Some(made);
        }
        slot.as_mut().ok_or_else(|| "no audio engine".to_string())
    }

    /// The engine runs, with voice processing as asked: switching it stops the engine for a moment. True — it was
    /// started now: the player is reset then (after the engine stopped, a player kept its old timeline: a buffer given
    /// to it did not sound, or sounded «from the middle», and its end never came — 27.11, the trial of main).
    fn run(&self, engine: &mut Engine, voice: bool) -> Result<bool, String> {
        let mut started = false;
        // SAFETY: the engine's calls, under the lock.
        unsafe {
            if engine.voice != voice {
                if engine.engine.isRunning() {
                    engine.engine.stop();
                }
                let input = engine.engine.inputNode();
                match input.setVoiceProcessingEnabled_error(voice) {
                    Ok(()) => {
                        engine.voice = voice;
                        if voice {
                            self.cancels.store(true, Ordering::Relaxed);
                            // As little hush of other programs' sound as the system gives (macOS 14 and later).
                            if input.respondsToSelector(sel!(setVoiceProcessingOtherAudioDuckingConfiguration:)) {
                                input.setVoiceProcessingOtherAudioDuckingConfiguration(
                                    AVAudioVoiceProcessingOtherAudioDuckingConfiguration {
                                        enableAdvancedDucking: objc2::runtime::Bool::YES,
                                        duckingLevel: AVAudioVoiceProcessingOtherAudioDuckingLevel::Min,
                                    },
                                );
                            }
                        }
                    }
                    Err(error) => {
                        // Without it the port still hears and speaks; speaking over the computer is by the key then.
                        if voice {
                            self.cancels.store(false, Ordering::Relaxed);
                        }
                        eprintln!("voice processing: {}", error.localizedDescription());
                    }
                }
            }
            if !engine.engine.isRunning() {
                engine.engine.prepare();
                engine
                    .engine
                    .startAndReturnError()
                    .map_err(|error| error.localizedDescription().to_string())?;
                started = true;
                // What the player held from before ends here, as cut.
                self.stops.fetch_add(1, Ordering::SeqCst);
                engine.player.stop();
            }
        }
        self.used();
        Ok(started)
    }

    /// The tap on the input: its first channel, at 16 kHz, to whoever listens.
    fn tap(&self, engine: &mut Engine) {
        if engine.tapped {
            return;
        }
        let (capture, frame_at) = (self.capture.clone(), self.frame_at.clone());
        let block = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            *frame_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
            // SAFETY: the buffer is valid for the call; its float data has `frameLength` frames per channel.
            let samples = unsafe {
                let buffer = buffer.as_ref();
                let frames = buffer.frameLength() as usize;
                let rate = buffer.format().sampleRate();
                let channels = buffer.floatChannelData();
                if channels.is_null() || frames == 0 {
                    return;
                }
                let first = (*channels).as_ptr();
                let samples = std::slice::from_raw_parts(first, frames);
                to_16k(samples, rate)
            };
            let sender = capture.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            if let Some(sender) = sender {
                let _ = sender.send(samples);
            }
        });
        // SAFETY: the engine's calls, under the lock; the block is retained by the node.
        unsafe {
            let input = engine.engine.inputNode();
            input.installTapOnBus_bufferSize_format_block(0, 1024, None, RcBlock::as_ptr(&block));
        }
        engine.tapped = true;
    }

    /// Nothing heard and nothing played for a while: the engine stops (the orange dot goes out).
    fn stop_when_idle(&self) {
        if self.capturing() || self.playing.load(Ordering::Relaxed) > 0 {
            self.used();
            return;
        }
        if self
            .last_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
            < IDLE
        {
            return;
        }
        let mut slot = self.lock();
        // Once more under the lock: a phrase given to the player between the look above and the lock (27.11, the trial
        // of main) would never be played and never end — the channel said «Отвечаю» for good.
        if self.capturing()
            || self.playing.load(Ordering::SeqCst) > 0
            || self
                .last_use
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .elapsed()
                < IDLE
        {
            return;
        }
        if let Some(engine) = slot.as_mut() {
            // SAFETY: the engine's calls, under the lock.
            unsafe {
                if engine.engine.isRunning() {
                    engine.engine.stop();
                }
            }
        }
    }

    /// The default input device: whether it runs somewhere — a call, a recorder.
    fn input_runs_somewhere() -> bool {
        let mut device: AudioObjectID = 0;
        let mut size = std::mem::size_of::<AudioObjectID>() as u32;
        let mut address = AudioObjectPropertyAddress {
            mSelector: kAudioHardwarePropertyDefaultInputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        // SAFETY: Core Audio's property read into a local of the size given.
        let found = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject as AudioObjectID,
                NonNull::from(&mut address),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::from(&mut device).cast(),
            )
        };
        if found != 0 || device == 0 {
            return false;
        }
        let mut running: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        address.mSelector = kAudioDevicePropertyDeviceIsRunningSomewhere;
        // SAFETY: as above.
        let read = unsafe {
            AudioObjectGetPropertyData(
                device,
                NonNull::from(&mut address),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::from(&mut running).cast(),
            )
        };
        read == 0 && running != 0
    }
}

/// The first channel of a capture, from its rate to 16 kHz, as 16-bit samples.
fn to_16k(samples: &[f32], rate: f64) -> Vec<i16> {
    let step = rate / f64::from(RATE);
    let count = (samples.len() as f64 / step).floor() as usize;
    (0..count)
        .map(|i| {
            let at = i as f64 * step;
            let low = at.floor() as usize;
            let high = (low + 1).min(samples.len() - 1);
            let part = (at - low as f64) as f32;
            let value = samples[low] * (1.0 - part) + samples[high] * part;
            (value.clamp(-1.0, 1.0) * 32767.0) as i16
        })
        .collect()
}

/// Samples of a voice at its rate, brought to the player's rate as floats.
fn to_play(samples: &[i16], rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let step = f64::from(rate.max(1)) / PLAY_RATE;
    let count = (samples.len() as f64 / step).floor() as usize;
    (0..count)
        .map(|i| {
            let at = i as f64 * step;
            let low = at.floor() as usize;
            let high = (low + 1).min(samples.len() - 1);
            let part = at - low as f64;
            ((f64::from(samples[low]) * (1.0 - part) + f64::from(samples[high]) * part) / 32768.0) as f32
        })
        .collect()
}

impl AudioPort for MacPort {
    fn start_capture(&self, voice: bool) -> Result<Frames, String> {
        if self.asking_access() {
            self.awaiting_access.store(true, Ordering::SeqCst);
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        *self.capture.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
        *self.capture_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
        let mut slot = self.lock();
        let engine = self.engine(&mut slot)?;
        // The voice processing switched first, the tap after it: a tap put on before the switch gives nothing.
        if engine.voice != voice && engine.tapped {
            // SAFETY: the engine's calls, under the lock.
            unsafe { engine.engine.inputNode().removeTapOnBus(0) };
            engine.tapped = false;
        }
        if let Err(error) = self.run(engine, voice) {
            *self.capture.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return Err(error);
        }
        self.tap(engine);
        Ok(receiver)
    }

    fn stop_capture(&self) {
        *self.capture.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self.capture_at.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let mut slot = self.lock();
        if let Some(engine) = slot.as_mut() {
            if engine.tapped {
                // SAFETY: the engine's calls, under the lock.
                unsafe { engine.engine.inputNode().removeTapOnBus(0) };
                engine.tapped = false;
            }
        }
        self.used();
    }

    fn play(&self, samples: Vec<i16>, rate: u32) -> Pin<Box<dyn Future<Output = bool> + Send + 'static>> {
        let played = to_play(&samples, rate);
        if played.is_empty() {
            return Box::pin(async { true });
        }
        let (done_by, done) = oneshot::channel::<()>();
        let generation;
        let scheduled = {
            let mut slot = self.lock();
            let voice = slot.as_ref().is_some_and(|engine| engine.voice);
            let engine = match self.engine(&mut slot) {
                Ok(engine) => engine,
                Err(error) => {
                    eprintln!("the computer's voice: {error}");
                    return Box::pin(async { false });
                }
            };
            let started = match self.run(engine, voice) {
                Ok(started) => started,
                Err(error) => {
                    eprintln!("the computer's voice: {error}");
                    return Box::pin(async { false });
                }
            };
            // A start (and its player's reset) counts from here.
            generation = self.stops.load(Ordering::SeqCst);
            let lead = if started {
                (LEAD_IN.as_secs_f64() * PLAY_RATE) as usize
            } else {
                0
            };
            let frames = lead + played.len();
            // SAFETY: a buffer of the player's format, filled within its capacity; the engine's calls under the lock.
            unsafe {
                let Some(buffer) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                    AVAudioPCMBuffer::alloc(),
                    &engine.format,
                    frames as u32,
                ) else {
                    return Box::pin(async { false });
                };
                buffer.setFrameLength(frames as u32);
                let channels = buffer.floatChannelData();
                if channels.is_null() {
                    return Box::pin(async { false });
                }
                let first = (*channels).as_ptr();
                std::ptr::write_bytes(first, 0, lead);
                std::ptr::copy_nonoverlapping(played.as_ptr(), first.add(lead), played.len());
                let done_by = Mutex::new(Some(done_by));
                let (playing, last_use) = (self.playing.clone(), self.last_use.clone());
                playing.fetch_add(1, Ordering::SeqCst);
                let finished = RcBlock::new(move || {
                    if let Some(done_by) = done_by.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
                        let mut now = playing.load(Ordering::SeqCst);
                        while now > 0 {
                            match playing.compare_exchange(now, now - 1, Ordering::SeqCst, Ordering::SeqCst) {
                                Ok(_) => break,
                                Err(seen) => now = seen,
                            }
                        }
                        // The idle stop counts from the end of the sound, not from its start: between two phrases
                        // the engine is not stopped under the next one.
                        *last_use.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
                        let _ = done_by.send(());
                    }
                });
                engine
                    .player
                    .scheduleBuffer_completionHandler(&buffer, RcBlock::as_ptr(&finished));
                if !engine.player.isPlaying() {
                    engine.player.play();
                }
            }
            true
        };
        let stops = self.stops.clone();
        Box::pin(async move { scheduled && done.await.is_ok() && stops.load(Ordering::SeqCst) == generation })
    }

    fn pause(&self) {
        let slot = self.lock();
        if let Some(engine) = slot.as_ref() {
            // SAFETY: the engine's calls, under the lock.
            unsafe { engine.player.pause() };
        }
    }

    fn resume(&self) {
        let slot = self.lock();
        if let Some(engine) = slot.as_ref() {
            // SAFETY: the engine's calls, under the lock.
            unsafe { engine.player.play() };
        }
    }

    fn stop_playing(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let slot = self.lock();
        if let Some(engine) = slot.as_ref() {
            // SAFETY: the engine's calls, under the lock; stopping calls the buffers' completion handlers.
            unsafe { engine.player.stop() };
        }
    }

    fn cancels_echo(&self) -> bool {
        self.cancels.load(Ordering::Relaxed)
    }

    fn mic_busy(&self) -> bool {
        !self.capturing() && MacPort::input_runs_somewhere()
    }

    fn access(&self) -> Access {
        // AVAudioApplication is there from macOS 14; on 13 the first capture asks, and a refusal fails it.
        if AnyClass::get(c"AVAudioApplication").is_none() {
            return Access::Unknown;
        }
        // SAFETY: the shared instance's read-only property.
        let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
        match permission {
            AVAudioApplicationRecordPermission::Granted => Access::Granted,
            AVAudioApplicationRecordPermission::Denied => Access::Denied,
            _ => Access::Unknown,
        }
    }

    fn notify(&self, title: &str, body: &str) {
        let (app, title, body) = (self.app.clone(), title.to_string(), body.to_string());
        let _ = self.app.run_on_main_thread(move || {
            let _ = app.notification().builder().title(title).body(body).show();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_comes_to_16_khz_and_a_voice_to_the_player_s_rate() {
        let second: Vec<f32> = (0..48_000).map(|i| ((i as f32) / 48.0).sin() * 0.5).collect();
        assert_eq!(to_16k(&second, 48_000.0).len(), 16_000);
        assert_eq!(to_16k(&second[..44_100], 44_100.0).len(), 16_000);
        let voice = vec![1000i16; 24_000];
        let played = to_play(&voice, 24_000);
        assert_eq!(played.len(), 48_000);
        assert!((played[100] - 1000.0 / 32768.0).abs() < 1e-6);
    }
}
