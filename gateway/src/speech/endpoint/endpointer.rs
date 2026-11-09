use serde::Serialize;

use crate::config::EndpointConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpeechEndReason {
    EndOfUtterance,
    NoSpeech,
    MaxLength,
}

impl SpeechEndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SpeechEndReason::EndOfUtterance => "endOfUtterance",
            SpeechEndReason::NoSpeech => "noSpeech",
            SpeechEndReason::MaxLength => "maxLength",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EndpointEvent {
    SpeechStart { at_ms: f64 },
    End { reason: SpeechEndReason, at_ms: f64 },
}

/// Decides where the command ends from per-window speech probabilities. Works on audio time only (position of the
/// window in the request audio), so a replayed recording ends exactly where the live request did; wall-clock limits
/// belong to the session.
///
/// Nothing is decided before the start mark (SpotterEnd plus the guard: the wake word sounds ~300 ms past the
/// mark). The command starts after `speech_start_ms` of continuous speech and ends after `end_silence_ms` of
/// silence. Between the two thresholds a window neither starts nor breaks silence.
#[derive(Debug, Clone)]
pub struct Endpointer {
    config: EndpointConfig,
    speech_start_ms: Option<f64>,
    speech_end_ms: Option<f64>,
    ended: Option<SpeechEndReason>,
    /// SpotterEnd (or the start of audio): the no-speech timeout counts from here.
    mark_ms: f64,
    /// The mark plus the guard: speech is looked for from here.
    start_ms: Option<f64>,
    speech_run: u32,
    silence_run: u32,
    no_speech_held: bool,
}

impl Endpointer {
    pub fn new(config: EndpointConfig) -> Endpointer {
        Endpointer {
            config,
            speech_start_ms: None,
            speech_end_ms: None,
            ended: None,
            mark_ms: 0.0,
            start_ms: None,
            speech_run: 0,
            silence_run: 0,
            no_speech_held: false,
        }
    }

    pub fn speech_start_ms(&self) -> Option<f64> {
        self.speech_start_ms
    }

    /// End of the last speech window before the final silence.
    pub fn speech_end_ms(&self) -> Option<f64> {
        self.speech_end_ms
    }

    pub fn ended(&self) -> Option<SpeechEndReason> {
        self.ended
    }

    /// Audio position where the command may begin; `guard` skips the tail of the wake word.
    pub fn start(&mut self, at_ms: f64, guard: bool) {
        self.mark_ms = at_ms;
        self.start_ms = Some(at_ms + if guard { self.config.guard_ms as f64 } else { 0.0 });
    }

    /// Speech was heard another way (recognition found words VAD missed): no-speech no longer ends the command. Its
    /// length then counts from the mark, up to `max_speech_ms`.
    pub fn hold_no_speech(&mut self) {
        self.no_speech_held = true;
    }

    pub fn push(&mut self, probability: f64, window_start_ms: f64, window_ms: f64) -> Option<EndpointEvent> {
        let start_ms = self.start_ms?;
        if self.ended.is_some() || window_start_ms < start_ms {
            return None;
        }
        let window_end_ms = window_start_ms + window_ms;
        let config = &self.config;

        let Some(speech_start_ms) = self.speech_start_ms else {
            self.speech_run = if probability >= config.threshold { self.speech_run + 1 } else { 0 };
            if f64::from(self.speech_run) * window_ms >= config.speech_start_ms as f64 {
                let at_ms = window_end_ms - f64::from(self.speech_run) * window_ms;
                self.speech_start_ms = Some(at_ms);
                return Some(EndpointEvent::SpeechStart { at_ms });
            }
            if self.no_speech_held {
                if window_end_ms - self.mark_ms >= config.max_speech_ms as f64 {
                    return Some(self.end(SpeechEndReason::MaxLength, window_end_ms));
                }
            } else if window_end_ms - self.mark_ms >= config.no_speech_ms as f64 {
                return Some(self.end(SpeechEndReason::NoSpeech, window_end_ms));
            }
            return None;
        };

        if probability >= config.threshold {
            self.silence_run = 0;
        } else if probability < config.neg_threshold {
            self.silence_run += 1;
        }
        if f64::from(self.silence_run) * window_ms >= config.end_silence_ms as f64 {
            self.speech_end_ms = Some(window_end_ms - f64::from(self.silence_run) * window_ms);
            return Some(self.end(SpeechEndReason::EndOfUtterance, window_end_ms));
        }
        if window_end_ms - speech_start_ms >= config.max_speech_ms as f64 {
            self.speech_end_ms = Some(window_end_ms);
            return Some(self.end(SpeechEndReason::MaxLength, window_end_ms));
        }
        None
    }

    fn end(&mut self, reason: SpeechEndReason, at_ms: f64) -> EndpointEvent {
        self.ended = Some(reason);
        EndpointEvent::End { reason, at_ms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: f64 = 32.0;

    fn config() -> EndpointConfig {
        EndpointConfig {
            threshold: 0.5,
            neg_threshold: 0.35,
            guard_ms: 320,
            speech_start_ms: 96,
            end_silence_ms: 700,
            no_speech_ms: 5000,
            max_speech_ms: 15000,
        }
    }

    /// Feeds windows from `from_ms` on; returns the events with the window start they came at.
    fn run(endpointer: &mut Endpointer, probabilities: &[f64], from_ms: f64) -> Vec<(EndpointEvent, f64)> {
        probabilities
            .iter()
            .enumerate()
            .filter_map(|(index, &p)| {
                let window_ms = from_ms + index as f64 * WINDOW;
                endpointer.push(p, window_ms, WINDOW).map(|event| (event, window_ms))
            })
            .collect()
    }

    fn repeat(p: f64, count: usize) -> Vec<f64> {
        vec![p; count]
    }

    fn start(at_ms: f64) -> EndpointEvent {
        EndpointEvent::SpeechStart { at_ms }
    }

    fn end(reason: SpeechEndReason, at_ms: f64) -> EndpointEvent {
        EndpointEvent::End { reason, at_ms }
    }

    #[test]
    fn decides_nothing_before_the_start_mark_and_during_the_wake_word_guard() {
        let mut endpointer = Endpointer::new(config());
        assert_eq!(run(&mut endpointer, &repeat(1.0, 62), 0.0), []); // the wake word buffer, before SpotterEnd
        endpointer.start(1984.0, true);
        assert_eq!(run(&mut endpointer, &repeat(1.0, 10), 1984.0), []); // 320 ms guard
        assert_eq!(endpointer.speech_start_ms(), None);
    }

    #[test]
    fn starts_speech_after_96_ms_of_speech_and_ends_it_after_700_ms_of_silence() {
        let mut endpointer = Endpointer::new(config());
        endpointer.start(0.0, false);
        let events = run(&mut endpointer, &[repeat(0.0, 5), repeat(0.9, 20), repeat(0.05, 30)].concat(), 0.0);
        assert_eq!(
            events,
            [(start(160.0), 224.0), (end(SpeechEndReason::EndOfUtterance, 800.0 + 704.0), 800.0 + 672.0)]
        );
        assert_eq!(endpointer.speech_end_ms(), Some(800.0));
    }

    #[test]
    fn neither_starts_nor_breaks_silence_between_the_thresholds() {
        let mut endpointer = Endpointer::new(config());
        endpointer.start(0.0, false);
        // 0.4 is between 0.35 and 0.5: speech does not start on it, and it does not reset the silence count.
        assert_eq!(run(&mut endpointer, &repeat(0.4, 20), 0.0), []);
        let windows = [repeat(0.9, 3), repeat(0.1, 11), repeat(0.4, 10), repeat(0.1, 11)].concat();
        let events = run(&mut endpointer, &windows, 640.0);
        let ends: Vec<Option<SpeechEndReason>> = events
            .iter()
            .map(|(event, _)| match event {
                EndpointEvent::End { reason, .. } => Some(*reason),
                EndpointEvent::SpeechStart { .. } => None,
            })
            .collect();
        assert_eq!(ends, [None, Some(SpeechEndReason::EndOfUtterance)]);
        // A window above the threshold resets it.
        let mut again = Endpointer::new(config());
        again.start(0.0, false);
        let windows = [repeat(0.9, 3), repeat(0.1, 20), vec![0.9], repeat(0.1, 20)].concat();
        let events = run(&mut again, &windows, 0.0);
        assert!(events.iter().all(|(event, _)| matches!(event, EndpointEvent::SpeechStart { .. })));
    }

    #[test]
    fn gives_up_when_there_is_no_command_and_cuts_endless_speech() {
        let mut silent = Endpointer::new(config());
        silent.start(2000.0, true);
        let events = run(&mut silent, &repeat(0.01, 200), 2000.0);
        assert_eq!(events, [(end(SpeechEndReason::NoSpeech, 7024.0), 6992.0)]);

        let mut endless = Endpointer::new(config());
        endless.start(0.0, false);
        let last = run(&mut endless, &repeat(0.99, 600), 0.0).pop().unwrap();
        assert_eq!(last.0, end(SpeechEndReason::MaxLength, 15008.0));
    }

    #[test]
    fn holds_the_no_speech_timeout_for_quiet_speech_found_by_recognition_up_to_the_maximum_length() {
        let mut endpointer = Endpointer::new(config());
        endpointer.start(0.0, false);
        assert_eq!(run(&mut endpointer, &repeat(0.1, 100), 0.0), []); // 3.2 s of nothing VAD calls speech
        endpointer.hold_no_speech();
        let events = run(&mut endpointer, &repeat(0.1, 400), 3200.0);
        // No noSpeech at 5 s; the length of quiet speech counts from the mark.
        assert_eq!(events, [(end(SpeechEndReason::MaxLength, 15008.0), 14976.0)]);
    }
}
