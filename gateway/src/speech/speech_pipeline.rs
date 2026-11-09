use std::collections::VecDeque;

use anyhow::Result;
use serde::{Serialize, Serializer};

use super::audio::gain::GainStage;
use super::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
use super::audio::opus_pcm_decoder::{OpusPcmDecoder, SPEECH_SAMPLE_RATE};
use super::audio::pcm::{dbfs, js_round, levels};
use super::audio::voicing::{voicing, Voicing};
use super::endpoint::endpointer::{EndpointEvent, Endpointer, SpeechEndReason};
use super::vad::vad_engine::{VadEngine, VadStream};
use crate::config::{EndpointConfig, GainConfig};

const SAMPLES_PER_MS: f64 = SPEECH_SAMPLE_RATE as f64 / 1000.0;
/// Without SpotterEnd (another activation, older firmware) VAD starts after this much audio anyway.
const SPOTTER_WAIT_MS: f64 = 2500.0;
/// The wake word is shorter than this; its onset is searched no further back from SpotterEnd.
const WAKE_WORD_MAX_MS: f64 = 1500.0;
/// The wake word sounds at most this long past SpotterEnd, while VAD still hears something.
const WAKE_WORD_TAIL_MS: f64 = 320.0;
/// Recognition gets this much audio around the speech.
const PAD_MS: f64 = 200.0;
/// Longer pauses inside the utterance are cut down to this: less audio, fewer hallucinated words.
const MAX_PAUSE_MS: f64 = 500.0;
/// Hard cap on buffered audio per request.
const MAX_AUDIO_MS: f64 = 60_000.0;
const MAX_AUDIO_SAMPLES: usize = (MAX_AUDIO_MS * SAMPLES_PER_MS) as usize;

/// Where the command part of the audio starts: SpotterEnd, the stream restart at SpotterEnd, or none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandStartSource {
    Station,
    Stream,
    Timeout,
    None,
}

#[derive(Debug, Clone)]
pub struct SpeechPipelineOptions {
    pub endpoint: EndpointConfig,
    pub gain: GainConfig,
    /// The request starts with the wake word buffer (activation by spotter): hold it until SpotterEnd.
    pub expect_wake_word: bool,
}

/// What processing the audio found, in order: the TS listener's onSpeechStart, onEnd and onError.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeechEvent {
    SpeechStart {
        at_ms: f64,
    },
    End {
        reason: SpeechEndReason,
        at_ms: f64,
    },
    /// Processing failed (the VAD): the pipeline takes no more audio.
    Error(String),
}

/// Audio prepared for recognition; positions are in the request audio.
#[derive(Debug, Clone, PartialEq)]
pub struct UtteranceAudio {
    pub pcm: Vec<i16>,
    pub from_ms: f64,
    pub to_ms: f64,
    pub gain_db: f64,
}

/// A number as JSON.stringify writes it: an integral one without a fraction.
fn js_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

fn js_number_option<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => js_number(value, serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechReport {
    pub decoded_ms: i64,
    pub chains_at_ms: Vec<i64>,
    pub packets: u64,
    pub decode_errors: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_decode_error: Option<String>,
    pub skipped_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_start_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_start_source: Option<CommandStartSource>,
    #[serde(serialize_with = "js_number")]
    pub gain_db: f64,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js_number_option")]
    pub wake_word_dbfs: Option<f64>,
    pub vad: VadReport,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VadReport {
    pub engine: String,
    #[serde(serialize_with = "js_number")]
    pub window_ms: f64,
    #[serde(serialize_with = "js_number")]
    pub max_probability: f64,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js_number_option")]
    pub speech_start_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js_number_option")]
    pub speech_end_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<SpeechEndReason>,
    /// One character per window: '#' speech, '+' between thresholds, '.' p >= 0.1, '_' silence.
    pub trace: String,
}

enum Task {
    Audio(Vec<u8>),
    SpotterEnd,
}

// A request's pipeline moves into its session's task (or a blocking thread for the VAD).
const _: fn() = || {
    fn send<T: Send>() {}
    send::<SpeechPipeline>();
};

/// Speech side of one voice request: the station's Ogg/Opus bytes in, the end of the command and the audio to
/// recognise out. Decisions use audio time only, so a replayed recording behaves exactly like the live request.
///
/// The station first sends ~2 s of buffered audio with the wake word in one burst, then SpotterEnd, then the command
/// in real time. The burst is held until SpotterEnd: the gain is picked from the wake word, then VAD runs over the
/// held audio and continues live.
///
/// `push` and `mark_spotter_end` queue; `flush` processes everything queued, in order, and returns what it found.
/// The session flushes after each frame; a test may push a whole recording first, as the TS tests did — audio queued
/// before the end is then still decoded (for `command_audio`), as it was in Node.
pub struct SpeechPipeline {
    engine_name: &'static str,
    options: SpeechPipelineOptions,
    demuxer: OggOpusDemuxer,
    decoder: OpusPcmDecoder,
    endpointer: Endpointer,
    gain: GainStage,
    vad: Option<Box<dyn VadStream>>,
    window_samples: usize,
    window_ms: f64,
    window: Vec<f32>,
    pcm: Vec<i16>,
    probabilities: Vec<f64>,
    chains_at_ms: Vec<i64>,
    command_start_sample: Option<usize>,
    command_start_source: Option<CommandStartSource>,
    queue: VecDeque<Task>,
    accepting: bool,
    closed: bool,
    failed: bool,
}

impl SpeechPipeline {
    pub fn new(engine: &dyn VadEngine, options: SpeechPipelineOptions) -> SpeechPipeline {
        let window_samples = engine.window_samples().max(1);
        let mut pipeline = SpeechPipeline {
            engine_name: engine.name(),
            demuxer: OggOpusDemuxer::new(),
            decoder: OpusPcmDecoder::new(),
            endpointer: Endpointer::new(options.endpoint.clone()),
            gain: GainStage::new(options.gain.clone()),
            vad: Some(engine.create_stream()),
            window_samples,
            window_ms: (window_samples as f64 * 1000.0) / f64::from(SPEECH_SAMPLE_RATE),
            window: vec![0.0; window_samples],
            pcm: Vec::with_capacity(SPEECH_SAMPLE_RATE as usize * 8),
            probabilities: Vec::new(),
            chains_at_ms: Vec::new(),
            command_start_sample: None,
            command_start_source: None,
            queue: VecDeque::new(),
            accepting: true,
            closed: false,
            failed: false,
            options,
        };
        if !pipeline.options.expect_wake_word {
            pipeline.start_command(CommandStartSource::None);
        }
        pipeline
    }

    pub fn ended(&self) -> Option<SpeechEndReason> {
        self.endpointer.ended()
    }

    /// Where the command speech starts, ms of audio; None until VAD hears it.
    pub fn speech_start_ms(&self) -> Option<f64> {
        self.endpointer.speech_start_ms()
    }

    /// Audio VAD has looked at so far, ms.
    pub fn processed_ms(&self) -> f64 {
        self.probabilities.len() as f64 * self.window_ms
    }

    /// Where the command part of the audio starts (SpotterEnd), ms; None until it is known.
    pub fn command_start_ms(&self) -> Option<f64> {
        self.command_start_sample.map(|sample| sample as f64 / SAMPLES_PER_MS)
    }

    /// Recognition found words VAD does not hear (quiet speech): its no-speech timeout must not end the command.
    pub fn hold_no_speech(&mut self) {
        self.endpointer.hold_no_speech();
    }

    /// Station audio: Ogg pages in any chunking.
    pub fn push(&mut self, chunk: &[u8]) {
        if self.accepting {
            self.queue.push_back(Task::Audio(chunk.to_vec()));
        }
    }

    /// StreamControl.SpotterEnd: the wake word buffer is complete.
    pub fn mark_spotter_end(&mut self) {
        if self.accepting {
            self.queue.push_back(Task::SpotterEnd);
        }
    }

    /// Stops taking audio; whatever was pushed before is still processed.
    pub fn stop(&mut self) {
        self.accepting = false;
    }

    /// Processes everything pushed so far, in order: the speech start, the end, or a failure, as they come.
    pub fn flush(&mut self) -> Vec<SpeechEvent> {
        let mut events = Vec::new();
        while let Some(task) = self.queue.pop_front() {
            if self.failed || self.closed {
                continue;
            }
            let done = match task {
                Task::Audio(chunk) => self.consume(&chunk, &mut events),
                Task::SpotterEnd => self.on_spotter_end(&mut events),
            };
            if let Err(error) = done {
                self.failed = true;
                self.accepting = false;
                events.push(SpeechEvent::Error(format!("{error:#}")));
            }
        }
        events
    }

    /// Takes no more audio and drops what was not processed; the codec and the VAD state are freed.
    pub fn close(&mut self) {
        self.accepting = false;
        if self.closed {
            return;
        }
        self.closed = true;
        self.queue.clear();
        self.vad = None;
        self.decoder.dispose();
    }

    /// Audio for recognition: from the wake word onset to the end of speech, gained, long pauses shortened. Before
    /// the end it covers the speech so far (interim results).
    pub fn utterance_audio(&self) -> Option<UtteranceAudio> {
        let p = &self.probabilities;
        let threshold = self.options.endpoint.threshold;
        let neg_threshold = self.options.endpoint.neg_threshold;
        // The wake word tail inside the guard counts as speech here: a short command said right after the wake word
        // still gets recognised.
        let command_window = self.command_window();

        let mut speech = (command_window..p.len()).filter(|&w| p[w] >= threshold);
        let first_speech = speech.next()?;
        let last_speech = speech.next_back().unwrap_or(first_speech);

        let from = first_speech.min(self.wake_word_onset(command_window).unwrap_or(first_speech));
        let speech_end = match self.endpointer.speech_end_ms() {
            Some(end_ms) => self.windows(end_ms),
            None => last_speech + 1,
        };
        let pad = self.windows(PAD_MS);
        let from_window = from.saturating_sub(pad);
        let to_window = p.len().min(speech_end.max(last_speech + 1) + pad);

        // Keep speech, `pad` before it and up to MAX_PAUSE_MS after it.
        let mut keep = vec![false; to_window.saturating_sub(from_window)];
        let max_pause = self.windows(MAX_PAUSE_MS) as u64;
        let mut since = u64::MAX;
        for w in from_window..to_window {
            since = if p[w] >= neg_threshold { 0 } else { since.saturating_add(1) };
            if since <= max_pause {
                keep[w - from_window] = true;
            }
        }
        let mut until = u64::MAX;
        for w in (from_window..to_window).rev() {
            until = if p[w] >= neg_threshold { 0 } else { until.saturating_add(1) };
            if until <= pad as u64 {
                keep[w - from_window] = true;
            }
        }
        Some(self.render(from_window, to_window, Some(&keep)))
    }

    /// All audio from the wake word onset to the end, no VAD cuts: a second look for a command VAD did not hear
    /// (whispered, far from the station). Recognisers return nothing for room noise.
    pub fn command_audio(&self) -> Option<UtteranceAudio> {
        self.command_start_sample?;
        let command_window = self.command_window();
        let to_window = self.pcm.len() / self.window_samples;
        if to_window <= command_window {
            return None;
        }
        let from_window =
            self.wake_word_onset(command_window).unwrap_or(command_window).saturating_sub(self.windows(PAD_MS));
        Some(self.render(from_window, to_window, None))
    }

    /// Windows with a pitch after the wake word (voicing.rs): none in a row in a whisper, several in distant speech.
    pub fn command_voicing(&self) -> Voicing {
        if self.command_start_sample.is_none() {
            return Voicing::default();
        }
        // The wake word can sound past SpotterEnd: its tail is not the command (live, the «а» of «Алиса» 200 ms after
        // it took a whispered «привет» for voiced speech).
        let mut from = self.command_window();
        let tail_end = (from + self.windows(WAKE_WORD_TAIL_MS)).min(self.probabilities.len());
        while from < tail_end && self.probabilities[from] >= 0.1 {
            from += 1;
        }
        let start = (from * self.window_samples).min(self.pcm.len());
        voicing(&self.pcm[start..], self.window_samples)
    }

    pub fn report(&self) -> SpeechReport {
        let threshold = self.options.endpoint.threshold;
        let neg_threshold = self.options.endpoint.neg_threshold;
        let trace = self
            .probabilities
            .iter()
            .map(|&p| match p {
                p if p >= threshold => '#',
                p if p >= neg_threshold => '+',
                p if p >= 0.1 => '.',
                _ => '_',
            })
            .collect();
        SpeechReport {
            decoded_ms: js_round(self.pcm.len() as f64 / SAMPLES_PER_MS) as i64,
            chains_at_ms: self.chains_at_ms.clone(),
            packets: self.decoder.packets(),
            decode_errors: self.decoder.errors(),
            last_decode_error: self.decoder.last_error().map(str::to_string),
            skipped_bytes: self.demuxer.skipped_bytes(),
            command_start_ms: self.command_start_sample.map(|sample| js_round(sample as f64 / SAMPLES_PER_MS) as i64),
            command_start_source: self.command_start_source,
            gain_db: self.gain.gain_db(),
            wake_word_dbfs: self.gain.reference_dbfs().map(|level| js_round(level * 10.0) / 10.0),
            vad: VadReport {
                engine: self.engine_name.to_string(),
                window_ms: self.window_ms,
                max_probability: js_round(self.probabilities.iter().copied().fold(0.0, f64::max) * 100.0) / 100.0,
                speech_start_ms: self.endpointer.speech_start_ms(),
                speech_end_ms: self.endpointer.speech_end_ms(),
                end: self.endpointer.ended(),
                trace,
            },
        }
    }

    // ---------------------------------------------------------------- processing

    fn consume(&mut self, chunk: &[u8], events: &mut Vec<SpeechEvent>) -> Result<()> {
        // The cap holds packet by packet, not after the chunk: two bytes of Opus can say 120 ms (48 empty frames,
        // which libopus conceals), so one frame from the station could otherwise hold hours of audio. Nothing past
        // it is decoded, nor anything queued after it.
        if self.pcm.len() < MAX_AUDIO_SAMPLES {
            for event in self.demuxer.push(chunk) {
                match event {
                    OggOpusEvent::Head { chain, head } => {
                        self.decoder.reset(&head);
                        self.chains_at_ms.push(js_round(self.pcm.len() as f64 / SAMPLES_PER_MS) as i64);
                        // The station restarts the stream at SpotterEnd: a fallback when the SpotterEnd message is
                        // late or lost.
                        if chain > 0 && self.command_start_sample.is_none() {
                            self.start_command(CommandStartSource::Stream);
                        }
                    }
                    OggOpusEvent::Packet { data, .. } => {
                        let samples = self.decoder.decode(&data);
                        let room = MAX_AUDIO_SAMPLES.saturating_sub(self.pcm.len());
                        self.pcm.extend_from_slice(&samples[..samples.len().min(room)]);
                    }
                }
                if self.pcm.len() >= MAX_AUDIO_SAMPLES {
                    break;
                }
            }
        }
        if self.pcm.len() >= MAX_AUDIO_SAMPLES {
            self.accepting = false;
        }
        self.advance(events)
    }

    fn on_spotter_end(&mut self, events: &mut Vec<SpeechEvent>) -> Result<()> {
        if self.command_start_sample.is_none() {
            self.start_command(CommandStartSource::Station);
        }
        self.advance(events)
    }

    /// First window entirely after SpotterEnd (0 without a wake word).
    fn command_window(&self) -> usize {
        self.command_start_sample.unwrap_or(0).div_ceil(self.window_samples)
    }

    /// Start of the wake word: walk back from SpotterEnd through speech, allowing short gaps inside the word.
    fn wake_word_onset(&self, command_window: usize) -> Option<usize> {
        let p = &self.probabilities;
        let neg_threshold = self.options.endpoint.neg_threshold;
        let lowest = command_window.saturating_sub(self.windows(WAKE_WORD_MAX_MS));
        let mut onset = None;
        let mut gap = 0;
        let mut w = command_window;
        while w > lowest {
            w -= 1;
            if p.get(w).is_some_and(|&p| p >= neg_threshold) {
                onset = Some(w);
                gap = 0;
            } else {
                gap += 1;
                if gap > if onset.is_none() { self.windows(320.0) } else { self.windows(128.0) } {
                    break;
                }
            }
        }
        onset
    }

    /// Windows [from, to), only the kept ones when `keep` is given, with one gain for all of them and no clipping.
    fn render(&self, from_window: usize, to_window: usize, keep: Option<&[bool]>) -> UtteranceAudio {
        let kept = |w: usize| keep.is_none_or(|keep| keep[w - from_window]);
        let windows: Vec<usize> = (from_window..to_window).filter(|&w| kept(w)).collect();
        let size = self.window_samples;
        let mut raw = vec![0i16; windows.len() * size];
        for (index, &w) in windows.iter().enumerate() {
            let start = (w * size).min(self.pcm.len());
            let end = ((w + 1) * size).min(self.pcm.len());
            raw[index * size..index * size + (end - start)].copy_from_slice(&self.pcm[start..end]);
        }
        let gain = self.gain.limited(levels(&raw).peak);
        let pcm =
            raw.iter().map(|&sample| js_round(f64::from(sample) * gain).clamp(-32768.0, 32767.0) as i16).collect();
        UtteranceAudio {
            pcm,
            from_ms: from_window as f64 * self.window_ms,
            to_ms: to_window as f64 * self.window_ms,
            gain_db: js_round(dbfs(gain) * 10.0) / 10.0,
        }
    }

    fn windows(&self, ms: f64) -> usize {
        js_round(ms / self.window_ms).max(0.0) as usize
    }

    fn start_command(&mut self, source: CommandStartSource) {
        let after_wake_word = matches!(source, CommandStartSource::Station | CommandStartSource::Stream);
        let sample = if after_wake_word { self.pcm.len() } else { 0 };
        self.command_start_sample = Some(sample);
        self.command_start_source = Some(source);
        self.gain.calibrate(&self.pcm, self.window_samples);
        self.endpointer.start(sample as f64 / SAMPLES_PER_MS, after_wake_word);
    }

    fn advance(&mut self, events: &mut Vec<SpeechEvent>) -> Result<()> {
        if self.command_start_sample.is_none() {
            if (self.pcm.len() as f64) < SPOTTER_WAIT_MS * SAMPLES_PER_MS {
                return Ok(());
            }
            self.start_command(CommandStartSource::Timeout);
        }
        let Some(vad) = self.vad.as_mut() else { return Ok(()) };
        let size = self.window_samples;
        while self.endpointer.ended().is_none() && (self.probabilities.len() + 1) * size <= self.pcm.len() {
            let start = self.probabilities.len() * size;
            let raw = &self.pcm[start..start + size];
            let scale = self.gain.limited(levels(raw).peak) / 32768.0;
            for (slot, &sample) in self.window.iter_mut().zip(raw) {
                *slot = (f64::from(sample) * scale) as f32;
            }
            let probability = vad.probability(&self.window)?;
            self.probabilities.push(probability);
            match self.endpointer.push(probability, start as f64 / SAMPLES_PER_MS, self.window_ms) {
                Some(EndpointEvent::SpeechStart { at_ms }) => events.push(SpeechEvent::SpeechStart { at_ms }),
                Some(EndpointEvent::End { reason, at_ms }) => {
                    self.accepting = false;
                    events.push(SpeechEvent::End { reason, at_ms });
                }
                None => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::GainMode;
    use crate::speech::test_audio::{concat, ogg_page, opus_head, silence, station_audio, voice};
    use crate::speech::vad::energy_vad::EnergyVadEngine;

    pub(crate) fn endpoint() -> EndpointConfig {
        EndpointConfig {
            threshold: 0.5,
            neg_threshold: 0.35,
            guard_ms: 320,
            speech_start_ms: 96,
            end_silence_ms: 700,
            no_speech_ms: 3000,
            max_speech_ms: 15000,
        }
    }

    pub(crate) fn gain() -> GainConfig {
        GainConfig { mode: GainMode::Auto, fixed_db: 0.0, max_db: 40.0, target_dbfs: -20.0 }
    }

    /// Wake word buffer: 1.2 s of room noise and a 0.8 s "word" ending at SpotterEnd, at station level (-50 dBFS).
    pub(crate) fn wake_word_buffer() -> Vec<i16> {
        concat(&[&silence(1200.0), &voice(800.0, -50.0)])
    }

    fn pipeline() -> SpeechPipeline {
        SpeechPipeline::new(
            &EnergyVadEngine::new(-40.0),
            SpeechPipelineOptions { endpoint: endpoint(), gain: gain(), expect_wake_word: true },
        )
    }

    fn feed(pipeline: &mut SpeechPipeline, bytes: &[u8], spotter_end_at: usize) {
        for piece in bytes[..spotter_end_at].chunks(1000) {
            pipeline.push(piece);
        }
        pipeline.mark_spotter_end();
        for piece in bytes[spotter_end_at..].chunks(1000) {
            pipeline.push(piece);
        }
    }

    fn within(value: f64, low: f64, high: f64) -> bool {
        (low..=high).contains(&value)
    }

    #[test]
    fn ends_the_command_after_the_silence_and_cuts_the_recognition_audio_around_the_speech() {
        // The wake word runs 200 ms past SpotterEnd, a pause, the command, then silence.
        let command = concat(&[&voice(200.0, -50.0), &silence(500.0), &voice(600.0, -55.0), &silence(1500.0)]);
        let audio = station_audio(&wake_word_buffer(), &command);
        let mut pipeline = pipeline();
        feed(&mut pipeline, &audio.bytes, audio.spotter_end_at);
        let events = pipeline.flush();

        let report = pipeline.report();
        assert_eq!(report.chains_at_ms, [0, 2000]);
        assert_eq!(report.command_start_source, Some(CommandStartSource::Station));
        assert!(report.gain_db > 25.0, "{}", report.gain_db);
        // The command (2.70–3.30 s) starts after the guard; windows and the Opus delay blur edges by a window or two.
        let [SpeechEvent::SpeechStart { at_ms: start }, SpeechEvent::End { reason, at_ms: end }] = events[..] else {
            panic!("{events:?}");
        };
        assert!(within(start, 2650.0, 2780.0), "{start}");
        assert_eq!(reason, SpeechEndReason::EndOfUtterance);
        assert!(within(end, 4000.0, 4100.0), "{end}");

        let audio = pipeline.utterance_audio().unwrap();
        // From the wake word onset (1.2 s) minus 200 ms of padding to the end of speech plus 200 ms.
        assert!(within(audio.from_ms, 900.0, 1100.0), "{}", audio.from_ms);
        assert!(within(audio.to_ms, 3450.0, 3650.0), "{}", audio.to_ms);
        pipeline.close();
    }

    #[test]
    fn ends_with_no_speech_when_nothing_follows_the_wake_word() {
        let audio = station_audio(&wake_word_buffer(), &silence(4000.0));
        let mut pipeline = pipeline();
        feed(&mut pipeline, &audio.bytes, audio.spotter_end_at);
        let ends: Vec<SpeechEndReason> = pipeline
            .flush()
            .into_iter()
            .filter_map(|event| match event {
                SpeechEvent::End { reason, .. } => Some(reason),
                _ => None,
            })
            .collect();
        assert_eq!(ends, [SpeechEndReason::NoSpeech]);
        assert_eq!(pipeline.utterance_audio(), None);
        pipeline.close();
    }

    #[test]
    fn gives_the_speech_so_far_before_the_end_for_interim_recognition() {
        // The command is still being spoken when the audio stops: no end yet.
        let audio = station_audio(&wake_word_buffer(), &voice(1000.0, -50.0));
        let mut pipeline = pipeline();
        feed(&mut pipeline, &audio.bytes, audio.spotter_end_at);
        pipeline.flush();

        assert_eq!(pipeline.ended(), None);
        // Speech runs on from SpotterEnd; the endpointer looks from the end of the guard.
        let start = pipeline.speech_start_ms().unwrap();
        assert!(within(start, 2300.0, 2450.0), "{start}");
        assert!(pipeline.processed_ms() >= 2950.0);
        let audio = pipeline.utterance_audio().unwrap();
        assert!(within(audio.from_ms, 900.0, 1100.0), "{}", audio.from_ms);
        assert_eq!(audio.to_ms, pipeline.processed_ms());
        pipeline.close();
    }

    #[test]
    fn gives_all_audio_after_the_wake_word_for_a_second_look_when_vad_hears_no_command() {
        // A command ~25 dB below the wake word, like a whisper or speech across the room.
        let command = concat(&[&silence(300.0), &voice(1000.0, -75.0), &silence(2500.0)]);
        let audio = station_audio(&wake_word_buffer(), &command);
        let mut pipeline = pipeline();
        feed(&mut pipeline, &audio.bytes, audio.spotter_end_at);
        pipeline.flush();

        assert_eq!(pipeline.ended(), Some(SpeechEndReason::NoSpeech));
        assert_eq!(pipeline.utterance_audio(), None);
        let all = pipeline.command_audio().unwrap();
        // From the wake word onset minus the padding to the end of the decoded audio (2 s + 3.8 s).
        assert!(within(all.from_ms, 900.0, 1100.0), "{}", all.from_ms);
        assert!(all.to_ms >= 5700.0, "{}", all.to_ms);
        assert_eq!(all.pcm.len() as f64, (all.to_ms - all.from_ms) * 16000.0 / 1000.0);
        assert!(all.gain_db > 25.0, "{}", all.gain_db);
        pipeline.close();
    }

    #[test]
    fn starts_the_command_at_the_stream_restart_when_spotter_end_never_arrives() {
        let command = concat(&[&silence(600.0), &voice(500.0, -50.0), &silence(1000.0)]);
        let audio = station_audio(&wake_word_buffer(), &command);
        let mut pipeline = pipeline();
        for piece in audio.bytes.chunks(700) {
            pipeline.push(piece);
        }
        pipeline.flush();
        let report = pipeline.report();
        assert_eq!(report.command_start_ms, Some(2000));
        assert_eq!(report.command_start_source, Some(CommandStartSource::Stream));
        assert_eq!(report.vad.end, Some(SpeechEndReason::EndOfUtterance));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["commandStartSource"], "stream");
        assert_eq!(json["vad"]["windowMs"], serde_json::json!(32));
        assert_eq!(json["vad"]["end"], "endOfUtterance");
        pipeline.close();
    }

    #[test]
    fn caps_the_audio_at_a_minute_packet_by_packet_when_one_chunk_says_more() {
        // Two bytes that say 120 ms: TOC code 3 (2.5 ms CELT frames), 48 frames of no bytes, concealed as silence.
        let tiny: &[u8] = &[0x83, 0x30];
        // One frame from the station of about 1.6 KB: a head and 765 such packets, 92 s of audio.
        let mut chunk = ogg_page(9, 0, 0, 0x02, &[&opus_head(0)]).unwrap();
        for sequence in 1..=3 {
            chunk.extend(ogg_page(9, sequence, 0, 0, &[tiny; 255]).unwrap());
        }
        let mut pipeline = pipeline();
        pipeline.push(&chunk);
        // Queued before the minute was full: not even decoded.
        pipeline.push(&ogg_page(9, 4, 0, 0, &[tiny; 255]).unwrap());
        pipeline.flush();
        let report = pipeline.report();
        // 500 packets of 120 ms make the minute exactly.
        assert_eq!((report.decoded_ms, report.packets, report.decode_errors), (60_000, 500, 0));
        assert_eq!(pipeline.command_audio().unwrap().to_ms, 60_000.0);
        pipeline.close();
    }

    #[cfg(feature = "silero")]
    #[test]
    fn runs_silero_vad_over_the_request_room_noise_after_the_wake_word_is_not_speech() {
        let model = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/silero_vad.onnx");
        let silero = crate::speech::vad::silero_vad::SileroVadEngine::load(&model).unwrap();
        let audio = station_audio(&wake_word_buffer(), &silence(1500.0));
        let mut pipeline = SpeechPipeline::new(
            &silero,
            SpeechPipelineOptions { endpoint: endpoint(), gain: gain(), expect_wake_word: true },
        );
        feed(&mut pipeline, &audio.bytes, audio.spotter_end_at);
        assert!(pipeline.flush().iter().all(|event| !matches!(event, SpeechEvent::Error(_))));
        let report = pipeline.report();
        assert_eq!(report.vad.engine, "silero");
        // The model runs: the wake word's tone is heard, the room after it is not.
        assert!(report.vad.max_probability > 0.5, "{}", report.vad.trace);
        assert_eq!(report.vad.trace.len(), 3500 / 32);
        assert_eq!(report.vad.speech_start_ms, None);
        pipeline.close();
    }
}
