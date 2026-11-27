//! The ear: a thought from sound (docs/plans/audio-interface-3.md, «К1»). What the panel's `mic.ts` does in stage 2,
//! here in the gateway and on the sound's own time, not the clock's: a recording runs through it in a test exactly as
//! live sound does.
//!
//! - In: samples, 16 kHz mono, in frames of any size; they are read in windows of 20 ms, each window's level in dB.
//!   The room's noise is the mean of the first 8 windows after the «слушаю» tone; speech is 12 dB above it and not
//!   below −55 dB (stage 2's numbers).
//! - A pause of 0.4 s after speech cuts a piece; a piece without a pause is cut at its 14th second. Each piece is
//!   recognized once, in order (the channel does it), and its text judged by «Чутьё» (`finished`): how long to wait
//!   for more.
//! - The thought is over when the silence after speech is that long and every piece is recognized; a short word as
//!   the first piece ends it as soon as it is recognized; the thought's ceiling is a minute. Nobody spoke in the first
//!   seconds — the ear says so, and the microphone closes without a word.
//!
//! The ear is logic without a device: frames and judgements in, steps out.

use crate::sense::{End, Thought};

/// The capture's rate.
pub const RATE: u32 = 16_000;
/// One window of sound.
const WINDOW: usize = (RATE as usize) / 50;
const WINDOW_MS: u64 = 20;
/// A pause this long after speech cuts a piece.
pub const PAUSE_MS: u64 = 400;
/// A piece without a pause is cut this long after it began.
pub const PIECE_MAX_MS: u64 = 14_000;
/// Speech is this much above the room's noise…
pub const OVER_NOISE_DB: f64 = 12.0;
/// …and never quieter than this.
pub const FLOOR_DB: f64 = -55.0;
/// The room's noise is measured over this many windows.
const NOISE_WINDOWS: usize = 8;
/// Nobody spoke this long after the microphone opened: it closes.
pub const START_WAIT_MS: u64 = 6_000;

/// What the ear heard.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The person began to speak.
    Speech,
    /// A piece to recognize: its number in the thought, from 1, and its samples.
    Piece { n: u32, pcm: Vec<i16> },
    /// The sound's level now, 0–1, for the strip.
    Level(f32),
    /// The thought is over.
    Over(Ending),
}

/// How a thought ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The silence after it was long enough.
    Silence,
    /// A short word as its first piece.
    Short,
    /// Its minute is up.
    Cap,
    /// Nobody spoke.
    Nobody,
}

/// The level of a window, in dB of full scale.
fn level(window: &[i16]) -> f64 {
    if window.is_empty() {
        return f64::NEG_INFINITY;
    }
    let sum: f64 = window.iter().map(|&sample| f64::from(sample) * f64::from(sample)).sum();
    let rms = (sum / window.len() as f64).sqrt() / 32768.0;
    if rms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * rms.log10()
    }
}

/// One thought being heard.
#[derive(Debug)]
pub struct Endpoint {
    /// The sound's time since the microphone opened.
    now: u64,
    /// Until then the «слушаю» tone sounds: neither noise nor speech.
    quiet_until: u64,
    start_by: u64,
    noise: Vec<f64>,
    speech_at: Option<u64>,
    piece_at: Option<u64>,
    last_speech: u64,
    pieces: u32,
    pending: u32,
    wait_ms: u64,
    end: End,
    max_ms: u64,
    window: Vec<i16>,
    piece: Vec<i16>,
    over: bool,
    /// Who ends the thought: the pauses (`Free`), or the key (Handy's logic, the owner 27.11). 27.11, the trial of
    /// main: «Сделай виджет, чтобы… [2 s] …показывал курс доллара» went at the pause with the key still down.
    keyed: Keyed,
}

/// Who ends a thought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyed {
    /// Its pauses, as «Чутьё» judges them: the word, the answer's window.
    Free,
    /// The key is down: only its release ends it; the pauses cut pieces.
    Held,
    /// A tap (or «Сказать»): the next press ends it; nobody speaking still closes it.
    Locked,
}

impl Endpoint {
    /// A thought begins: `tone_ms` — how long our «слушаю» sounds; `start_ms` — how long the person has to begin.
    pub fn new(tone_ms: u64, start_ms: u64) -> Endpoint {
        Endpoint {
            now: 0,
            quiet_until: tone_ms,
            start_by: tone_ms + start_ms,
            noise: Vec::new(),
            speech_at: None,
            piece_at: None,
            last_speech: 0,
            pieces: 0,
            pending: 0,
            wait_ms: crate::sense::thought::PLAIN_MS,
            end: End::Plain,
            max_ms: crate::sense::thought::THOUGHT_MAX_MS,
            window: Vec::with_capacity(WINDOW),
            piece: Vec::new(),
            over: false,
            keyed: Keyed::Free,
        }
    }

    /// Who ends the thought from now.
    pub fn keyed(&mut self, keyed: Keyed) {
        self.keyed = keyed;
    }

    pub fn keyed_by(&self) -> Keyed {
        self.keyed
    }

    /// The person spoke already.
    pub fn spoke(&self) -> bool {
        self.speech_at.is_some()
    }

    /// The thought ended.
    pub fn over(&self) -> bool {
        self.over
    }

    /// How the last piece judged ends the thought.
    pub fn end(&self) -> End {
        self.end
    }

    /// Samples heard: what they make the ear say.
    pub fn push(&mut self, samples: &[i16]) -> Vec<Step> {
        let mut steps = Vec::new();
        for &sample in samples {
            if self.over {
                break;
            }
            self.window.push(sample);
            self.piece.push(sample);
            if self.window.len() == WINDOW {
                let window = std::mem::take(&mut self.window);
                self.heard(&window, &mut steps);
                self.window = window;
                self.window.clear();
            }
        }
        steps
    }

    fn heard(&mut self, window: &[i16], steps: &mut Vec<Step>) {
        self.now += WINDOW_MS;
        let db = level(window);
        steps.push(Step::Level(((db - FLOOR_DB) / 45.0).clamp(0.0, 1.0) as f32));
        if self.now <= self.quiet_until {
            return;
        }
        if self.noise.len() < NOISE_WINDOWS && self.speech_at.is_none() && db.is_finite() {
            self.noise.push(db);
        }
        let noise =
            if self.noise.is_empty() { FLOOR_DB } else { self.noise.iter().sum::<f64>() / self.noise.len() as f64 };
        let speaking = db > FLOOR_DB.max(noise + OVER_NOISE_DB);
        if speaking {
            if self.speech_at.is_none() {
                self.speech_at = Some(self.now);
                steps.push(Step::Speech);
            }
            if self.piece_at.is_none() {
                self.piece_at = Some(self.now);
            }
            self.last_speech = self.now;
        }
        if self.speech_at.is_none() && self.now > self.start_by && self.keyed != Keyed::Held {
            self.over = true;
            steps.push(Step::Over(Ending::Nobody));
            return;
        }
        let quiet = self.now - self.last_speech;
        if let Some(began) = self.piece_at {
            if quiet >= PAUSE_MS || self.now - began > PIECE_MAX_MS {
                steps.push(self.cut());
            }
        }
        if let Some(began) = self.speech_at {
            if self.now - began > self.max_ms {
                self.over = true;
                steps.push(Step::Over(Ending::Cap));
                return;
            }
        }
        if self.speech_at.is_none() || self.piece_at.is_some() {
            return;
        }
        if self.pending == 0 && quiet >= self.wait_ms && self.keyed == Keyed::Free {
            self.over = true;
            steps.push(Step::Over(Ending::Silence));
        }
    }

    /// What was said since the last pause, as a piece.
    fn cut(&mut self) -> Step {
        self.piece_at = None;
        self.pieces += 1;
        self.pending += 1;
        Step::Piece { n: self.pieces, pcm: std::mem::take(&mut self.piece) }
    }

    /// The rest, as the last piece: the thought ends now (the button, the key let go). None — nothing more was said.
    pub fn take_rest(&mut self) -> Option<Step> {
        self.over = true;
        self.piece_at.is_some().then(|| self.cut())
    }

    /// A piece was recognized and judged (None — it had no words): how long to wait for more now.
    pub fn judged(&mut self, thought: Option<&Thought>) -> Vec<Step> {
        self.pending = self.pending.saturating_sub(1);
        if let Some(thought) = thought {
            self.wait_ms = thought.wait_ms;
            self.end = thought.end;
            self.max_ms = thought.max_ms;
        }
        if !self.over && self.keyed == Keyed::Free && self.end == End::Short && self.piece_at.is_none() {
            self.over = true;
            return vec![Step::Over(Ending::Short)];
        }
        // Recognized after the silence was long enough already: the thought is over now.
        let quiet = self.now.saturating_sub(self.last_speech);
        if !self.over
            && self.keyed == Keyed::Free
            && self.speech_at.is_some()
            && self.piece_at.is_none()
            && self.pending == 0
            && quiet >= self.wait_ms
        {
            self.over = true;
            return vec![Step::Over(Ending::Silence)];
        }
        Vec::new()
    }

    /// Every piece cut was recognized.
    pub fn settled(&self) -> bool {
        self.pending == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sense::thought::{HANGING_MS, PLAIN_MS, WAIT_MS};
    use crate::speech::test_audio;

    fn silence(ms: u64) -> Vec<i16> {
        test_audio::silence(ms as f64)
    }

    fn voice(ms: u64, db: f64) -> Vec<i16> {
        test_audio::voice(ms as f64, db)
    }

    fn thought(end: End, wait_ms: u64) -> Thought {
        Thought { end, wait_ms, text: String::new(), max_ms: 60_000, park_ms: 120_000 }
    }

    /// Feeds the samples in frames of 20 ms; the steps that are not the level.
    fn run(endpoint: &mut Endpoint, samples: &[i16]) -> Vec<Step> {
        samples
            .chunks(320)
            .flat_map(|frame| endpoint.push(frame))
            .filter(|step| !matches!(step, Step::Level(_)))
            .collect()
    }

    fn kinds(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|step| match step {
                Step::Speech => "speech".to_string(),
                Step::Piece { n, .. } => format!("piece {n}"),
                Step::Over(ending) => format!("over {ending:?}"),
                Step::Level(_) => "level".to_string(),
            })
            .collect()
    }

    #[test]
    fn a_pause_cuts_a_piece_and_the_silence_after_it_ends_the_thought() {
        let mut ear = Endpoint::new(200, START_WAIT_MS);
        let steps = run(&mut ear, &[silence(400), voice(1200, -20.0), silence(500)].concat());
        assert_eq!(kinds(&steps), ["speech", "piece 1"], "the piece is cut, the thought waits for its text");
        assert!(ear.judged(Some(&thought(End::Plain, PLAIN_MS))).is_empty());
        let steps = run(&mut ear, &silence(PLAIN_MS));
        assert_eq!(kinds(&steps), ["over Silence"]);
    }

    /// 27.11, the trial of main: with the key down, «Сделай виджет, чтобы… [2 s] …показывал курс доллара» went at the
    /// pause. Held, the pauses cut pieces and end nothing; let go, the silence ends it as usual.
    #[test]
    fn a_held_key_keeps_the_thought_through_its_pauses() {
        let mut ear = Endpoint::new(200, START_WAIT_MS);
        ear.keyed(Keyed::Held);
        let steps = run(&mut ear, &[silence(400), voice(1200, -20.0), silence(2500)].concat());
        assert_eq!(kinds(&steps), ["speech", "piece 1"]);
        assert!(ear.judged(Some(&thought(End::Plain, PLAIN_MS))).is_empty(), "recognized: still held");
        let steps = run(&mut ear, &[silence(PLAIN_MS), voice(1000, -20.0), silence(600)].concat());
        assert_eq!(kinds(&steps), ["piece 2"], "the second half is a piece of the same thought");
        assert!(ear.judged(Some(&thought(End::Short, PLAIN_MS))).is_empty(), "a short word does not end it either");
        ear.keyed(Keyed::Free);
        let steps = run(&mut ear, &silence(PLAIN_MS));
        assert_eq!(kinds(&steps), ["over Silence"], "let go: the silence ends it");
    }

    #[test]
    fn a_locked_thought_ends_by_no_pause_but_closes_when_nobody_speaks() {
        let mut ear = Endpoint::new(200, START_WAIT_MS);
        ear.keyed(Keyed::Locked);
        let steps = run(&mut ear, &[silence(400), voice(1200, -20.0), silence(PLAIN_MS + 3000)].concat());
        assert_eq!(kinds(&steps), ["speech", "piece 1"]);
        assert!(ear.judged(Some(&thought(End::Plain, PLAIN_MS))).is_empty(), "the pause ends nothing");
        let mut nobody = Endpoint::new(200, START_WAIT_MS);
        nobody.keyed(Keyed::Locked);
        assert_eq!(kinds(&run(&mut nobody, &silence(START_WAIT_MS + 400))), ["over Nobody"], "nobody spoke: closed");
    }

    #[test]
    fn a_held_key_waits_for_the_first_word_past_the_start() {
        let mut ear = Endpoint::new(200, START_WAIT_MS);
        ear.keyed(Keyed::Held);
        assert!(run(&mut ear, &silence(START_WAIT_MS + 2000)).is_empty(), "nobody spoke yet, the key is down");
        ear.keyed(Keyed::Free);
        assert_eq!(kinds(&run(&mut ear, &silence(100))), ["over Nobody"]);
    }

    #[test]
    fn a_piece_without_a_pause_is_cut_at_fourteen_seconds() {
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        let steps = run(&mut ear, &[silence(200), voice(15_000, -20.0)].concat());
        assert_eq!(kinds(&steps), ["speech", "piece 1"]);
        let Step::Piece { pcm, .. } = &steps[1] else { unreachable!() };
        assert!(pcm.len() as u64 >= PIECE_MAX_MS * 16, "the piece holds its fourteen seconds");
    }

    #[test]
    fn a_hanging_word_and_a_wait_make_the_silence_longer() {
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        run(&mut ear, &[silence(200), voice(1000, -20.0), silence(500)].concat());
        ear.judged(Some(&thought(End::Hanging, HANGING_MS)));
        assert!(run(&mut ear, &silence(2800)).is_empty(), "«сделай виджет, чтобы» waits 3.5 s after the speech");
        assert_eq!(kinds(&run(&mut ear, &silence(300))), ["over Silence"]);
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        run(&mut ear, &[silence(200), voice(800, -20.0), silence(500)].concat());
        ear.judged(Some(&thought(End::Wait, WAIT_MS)));
        assert!(run(&mut ear, &silence(19_000)).is_empty(), "«подожди» waits 20 s");
        assert_eq!(ear.end(), End::Wait);
    }

    #[test]
    fn a_short_first_word_ends_the_thought_as_soon_as_it_is_recognized() {
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        run(&mut ear, &[silence(200), voice(400, -20.0), silence(450)].concat());
        assert_eq!(kinds(&ear.judged(Some(&thought(End::Short, 0)))), ["over Short"]);
    }

    #[test]
    fn nobody_speaking_closes_the_microphone_and_the_tone_is_not_speech() {
        let mut ear = Endpoint::new(300, 1000);
        // The tone itself is loud; it is not heard as speech.
        let steps = run(&mut ear, &[voice(300, -10.0), silence(1100)].concat());
        assert_eq!(kinds(&steps), ["over Nobody"]);
        assert!(!ear.spoke());
    }

    #[test]
    fn the_key_let_go_ends_the_thought_with_what_was_said() {
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        run(&mut ear, &[silence(200), voice(900, -20.0)].concat());
        assert!(matches!(ear.take_rest(), Some(Step::Piece { n: 1, .. })));
        assert!(ear.over());
        let mut quiet = Endpoint::new(0, START_WAIT_MS);
        run(&mut quiet, &silence(500));
        assert_eq!(quiet.take_rest(), None, "nothing said, nothing to recognize");
    }

    #[test]
    fn a_noisy_room_needs_louder_speech() {
        let mut ear = Endpoint::new(0, START_WAIT_MS);
        let steps = run(&mut ear, &[silence_noisy(400), voice(800, -40.0), silence_noisy(500)].concat());
        assert!(!kinds(&steps).contains(&"speech".to_string()), "-40 dB is only 5 dB over a -45 dB room");
        let steps = run(&mut ear, &[voice(800, -25.0), silence_noisy(500)].concat());
        assert_eq!(kinds(&steps), ["speech", "piece 1"]);
    }

    fn silence_noisy(ms: u64) -> Vec<i16> {
        crate::speech::test_audio::silence_at(ms as f64, -45.0)
    }
}
