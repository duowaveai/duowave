use std::sync::LazyLock;

use regex::Regex;

use super::asr::transcript::is_js_space;
use super::speech_pipeline::{SpeechPipeline, UtteranceAudio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterimOptions {
    /// At most one interim recognition per this much new audio, ms; 0 turns them off.
    pub interval_ms: u64,
    /// With no speech heard by VAD, look for quiet speech in all audio after the wake word.
    pub quiet_speech: bool,
    /// Quiet speech ends when its text has not changed for this much audio, ms.
    pub quiet_end_ms: u64,
    /// Speech VAD hears ends when its text has not changed for this much audio, ms; 0 leaves the end to VAD.
    pub unchanged_end_ms: u64,
    /// A stop and nothing else ends once the next result says it again, at least this much audio later, ms; 0 waits
    /// for the end of speech.
    pub stop_end_ms: u64,
    /// The wake word sounds this long past SpotterEnd: quiet speech is looked for after it.
    pub guard_ms: u64,
}

/// One interim recognition to run.
#[derive(Debug, Clone, PartialEq)]
pub struct InterimJob {
    pub audio: UtteranceAudio,
    /// Audio position it covers up to, ms.
    pub at_ms: f64,
    /// All audio after the wake word (looking for quiet speech), not the speech VAD cut out.
    pub all_audio: bool,
}

/// Why interim recognition ended the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterimEnd {
    /// Quiet speech (VAD hears none) has not changed for `quiet_end_ms`.
    Quiet,
    /// Speech VAD hears has not changed for `unchanged_end_ms`: VAD takes the room's noise for speech.
    Unchanged,
    /// A stop, said again by the next result.
    Stop,
}

impl InterimEnd {
    /// The request's outcome in the journal.
    pub fn outcome(self) -> &'static str {
        match self {
            InterimEnd::Quiet => "quietEndOfUtterance",
            InterimEnd::Unchanged => "unchangedEndOfUtterance",
            InterimEnd::Stop => "stopCommand",
        }
    }

    /// Why it ended, for the journal and a replay: «quiet speech unchanged: end».
    pub fn describe(self) -> &'static str {
        match self {
            InterimEnd::Quiet => "quiet speech unchanged",
            InterimEnd::Unchanged => "speech unchanged",
            InterimEnd::Stop => "a stop said again",
        }
    }
}

/// What a recognised interim text changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterimOutcome {
    /// Interim results get new text.
    pub changed: bool,
    /// The command is over.
    pub end: Option<InterimEnd>,
}

static NOT_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").expect("a valid pattern"));

/// Letters and digits only: punctuation and case may change as audio is added.
fn normalize(text: &str) -> String {
    NOT_WORDS.replace_all(&text.to_lowercase(), " ").trim_matches(is_js_space).to_string()
}

/// Recognition of the command while it is spoken. The station needs text in interim results to keep listening, so
/// when VAD hears the command, the speech so far is recognised. When VAD hears nothing after the wake word, all audio
/// after it is recognised instead: a whisper or speech across the room can stay below VAD's threshold. Words found
/// that way are quiet speech: VAD's no-speech timeout no longer ends the command, it ends when the text has not
/// changed for `quiet_end_ms` of audio. Whispers come with pauses and recognition lags behind them, so that is longer
/// than VAD's end silence; an empty result does not count as "unchanged".
///
/// Speech VAD hears ends the same way after `unchanged_end_ms`: when the gain is high, the room's noise is louder
/// than VAD's threshold, VAD's end silence never comes, and the command ran to VAD_MAX_SPEECH_MS (live 12.11: «Алиса,
/// стоп» recognised in 3 s, ended at 15). A stop and nothing else («Стоп.», «Хватит», «Алиса, стоп»; the caller says
/// which text is one) ends sooner, as soon as the next result says it again: the station falls silent at once, as
/// Alice's does. One result alone is not enough: «Стоп» may go on as «стоп, а какая погода?».
///
/// Decisions only, no I/O: the live session and a replay run the same logic. The pipeline is the request's; it is
/// passed in on each call.
#[derive(Debug, Clone)]
pub struct InterimRecognition {
    options: InterimOptions,
    text: String,
    count: u32,
    quiet: bool,
    at_ms: Option<f64>,
    /// Normalised text of the latest result and where it first appeared, ms of audio.
    same_text: String,
    same_since_ms: f64,
}

impl InterimRecognition {
    pub fn new(options: InterimOptions) -> InterimRecognition {
        InterimRecognition {
            options,
            text: String::new(),
            count: 0,
            quiet: false,
            at_ms: None,
            same_text: String::new(),
            same_since_ms: 0.0,
        }
    }

    /// Latest text with words in it: what interim results carry.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Recognitions accepted.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Quiet speech heard: words in all audio while VAD heard none.
    pub fn quiet(&self) -> bool {
        self.quiet
    }

    /// What to recognise now, or None when it is not time yet.
    pub fn next(&mut self, pipeline: &SpeechPipeline) -> Option<InterimJob> {
        let InterimOptions { interval_ms, quiet_speech, guard_ms, .. } = self.options;
        if interval_ms == 0 || pipeline.ended().is_some() {
            return None;
        }
        let command_start_ms = pipeline.command_start_ms()?;
        let speech_start_ms = pipeline.speech_start_ms();
        let processed_ms = pipeline.processed_ms();
        let vad_hears = speech_start_ms.is_some();
        // Once quiet speech is heard, all audio stays the source: VAD-cut audio would drop the quiet words.
        let all_audio = quiet_speech && (self.quiet || !vad_hears);
        if !vad_hears && !all_audio {
            return None;
        }
        let from = self.at_ms.unwrap_or_else(|| speech_start_ms.unwrap_or(command_start_ms + guard_ms as f64));
        if processed_ms < from + interval_ms as f64 {
            return None;
        }
        let audio = if all_audio { pipeline.command_audio() } else { pipeline.utterance_audio() }?;
        self.at_ms = Some(processed_ms);
        Some(InterimJob { audio, at_ms: processed_ms, all_audio })
    }

    /// Takes the cleaned text of a job; `stop`: the text is a stop and nothing else (the skills' rule).
    pub fn accept(
        &mut self,
        pipeline: &mut SpeechPipeline,
        job: &InterimJob,
        text: &str,
        stop: bool,
    ) -> InterimOutcome {
        self.count += 1;
        let vad_hears = pipeline.speech_start_ms().is_some();
        // Quiet speech is judged only while VAD hears nothing; once it does, its own end applies.
        let quiet = job.all_audio && !vad_hears;
        if quiet && !text.is_empty() && !self.quiet {
            self.quiet = true;
            pipeline.hold_no_speech();
        }
        let normalized = normalize(text);
        let unchanged_ms = if normalized.is_empty() || normalized != self.same_text {
            self.same_text = normalized;
            self.same_since_ms = job.at_ms;
            None
        } else {
            Some(job.at_ms - self.same_since_ms)
        };
        let unchanged_for = |ms: u64| unchanged_ms.is_some_and(|unchanged| unchanged >= ms as f64);
        let end = if stop && self.options.stop_end_ms > 0 && unchanged_for(self.options.stop_end_ms) {
            Some(InterimEnd::Stop)
        } else if quiet {
            unchanged_for(self.options.quiet_end_ms).then_some(InterimEnd::Quiet)
        } else if vad_hears && self.options.unchanged_end_ms > 0 && unchanged_for(self.options.unchanged_end_ms) {
            Some(InterimEnd::Unchanged)
        } else {
            None
        };
        let changed = !text.is_empty() && text != self.text;
        if changed {
            self.text = text.to_string();
        }
        InterimOutcome { changed, end }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::endpoint::endpointer::SpeechEndReason;
    use crate::speech::speech_pipeline::tests::{gain, wake_word_buffer};
    use crate::speech::speech_pipeline::SpeechPipelineOptions;
    use crate::speech::test_audio::{concat, ogg_pages, silence, silence_at, station_audio, voice};
    use crate::speech::vad::energy_vad::EnergyVadEngine;

    const OPTIONS: InterimOptions = InterimOptions {
        interval_ms: 500,
        quiet_speech: true,
        quiet_end_ms: 1000,
        unchanged_end_ms: 2000,
        stop_end_ms: 400,
        guard_ms: 320,
    };

    struct Step {
        job: InterimJob,
        changed: bool,
        end: Option<InterimEnd>,
    }

    /// Room noise the wake word's gain (~28 dB) lifts over VAD's threshold: VAD hears it as speech that never ends.
    fn noisy_room(ms: f64) -> Vec<i16> {
        silence_at(ms, -62.0)
    }

    /// Feeds the command 100 ms at a time, as the station streams it, asks for an interim job after each piece and
    /// answers it with `reply`, until the pipeline or the interim recognition ends the command. «Стоп.» is the stop.
    fn speak(
        command: &[i16],
        mut reply: impl FnMut(&InterimJob) -> String,
        options: InterimOptions,
    ) -> (SpeechPipeline, InterimRecognition, Vec<Step>) {
        let endpoint = crate::speech::speech_pipeline::tests::endpoint();
        let mut pipeline = SpeechPipeline::new(
            &EnergyVadEngine::new(-40.0),
            SpeechPipelineOptions { endpoint, gain: gain(), expect_wake_word: true, assumed_gain_db: None },
        );
        let mut interim = InterimRecognition::new(options);
        let audio = station_audio(&wake_word_buffer(), command);
        pipeline.push(&audio.bytes[..audio.spotter_end_at]);
        pipeline.mark_spotter_end();
        let pages = ogg_pages(&audio.bytes[audio.spotter_end_at..]);
        let mut steps: Vec<Step> = Vec::new();
        for piece in pages.chunks(5) {
            if pipeline.ended().is_some() {
                break;
            }
            pipeline.push(&piece.concat());
            pipeline.flush();
            let Some(job) = interim.next(&pipeline) else { continue };
            let text = reply(&job);
            let InterimOutcome { changed, end } = interim.accept(&mut pipeline, &job, &text, text == "Стоп.");
            steps.push(Step { job, changed, end });
            if end.is_some() {
                break;
            }
        }
        (pipeline, interim, steps)
    }

    #[test]
    fn recognises_the_speech_vad_hears_once_per_interval_of_new_audio() {
        let (mut pipeline, interim, steps) =
            speak(&concat(&[&voice(1500.0, -50.0), &silence(1500.0)]), |_| "Привет.".into(), OPTIONS);

        assert!(steps.len() >= 2, "{}", steps.len());
        assert!(steps.iter().all(|step| !step.job.all_audio));
        // The first after the interval from the start of speech, then one per interval of audio.
        assert!(steps[0].job.at_ms - pipeline.speech_start_ms().unwrap() >= 500.0);
        assert!(steps[1].job.at_ms - steps[0].job.at_ms >= 500.0);
        let changed: Vec<bool> = steps.iter().map(|step| step.changed).collect();
        assert_eq!(changed, [vec![true], vec![false; steps.len() - 1]].concat());
        // VAD ends loud speech; text that does not change is not an end there.
        assert!(steps.iter().all(|step| step.end.is_none()));
        assert_eq!(pipeline.ended(), Some(SpeechEndReason::EndOfUtterance));
        assert_eq!(interim.text(), "Привет.");
        assert!(!interim.quiet());
        pipeline.close();
    }

    #[test]
    fn finds_quiet_speech_in_all_audio_after_the_wake_word_and_ends_it_when_the_text_stops_changing() {
        // ~25 dB below the wake word: VAD hears nothing, recognition hears "который" and then "который час".
        let command = concat(&[&silence(400.0), &voice(1200.0, -75.0), &silence(4000.0)]);
        let reply = |job: &InterimJob| {
            if job.at_ms < 3000.0 {
                String::new()
            } else if job.at_ms < 4200.0 {
                "Который.".into()
            } else {
                "Который час?".into()
            }
        };
        let (mut pipeline, interim, steps) = speak(&command, reply, OPTIONS);

        assert_eq!(pipeline.speech_start_ms(), None);
        assert!(steps.iter().all(|step| step.job.all_audio));
        // All audio: from the wake word onset, not from where VAD would cut.
        assert!(steps[0].job.audio.from_ms <= 1100.0, "{}", steps[0].job.audio.from_ms);
        assert!(interim.quiet());
        assert_eq!(interim.text(), "Который час?");
        // The no-speech timeout (3 s after SpotterEnd = 5 s) did not end it: the text stopped changing did.
        let last = steps.last().unwrap();
        assert_eq!(last.end, Some(InterimEnd::Quiet));
        assert!(last.job.at_ms > 5000.0, "{}", last.job.at_ms);
        assert_eq!(pipeline.ended(), None);
        // Changed only when the words changed.
        let changed: Vec<bool> = steps.iter().filter(|step| step.changed).map(|step| step.job.at_ms < 4200.0).collect();
        assert_eq!(changed, [true, false]);
        pipeline.close();
    }

    #[test]
    fn does_not_take_an_empty_result_for_unchanged_text() {
        let command = concat(&[&voice(1000.0, -75.0), &silence(5000.0)]);
        let mut replies: std::collections::VecDeque<&str> =
            ["Который час?", "", "Который час?", "Который час?", "Который час?", "Который час?"].into();
        let (_, _, steps) = speak(&command, |_| replies.pop_front().unwrap_or("Который час?").to_string(), OPTIONS);

        let end_at = steps.iter().position(|step| step.end.is_some()).unwrap();
        // The empty second result restarts the count: 1000 ms needs two more intervals after the third.
        assert_eq!(end_at, 4);
        assert!(steps[end_at].job.at_ms - steps[2].job.at_ms >= 1000.0);
    }

    #[test]
    fn leaves_silence_after_the_wake_word_to_the_no_speech_timeout() {
        let (mut pipeline, interim, steps) = speak(&silence(4000.0), |_| String::new(), OPTIONS);
        assert!(!steps.is_empty());
        assert!(!interim.quiet());
        assert_eq!(pipeline.ended(), Some(SpeechEndReason::NoSpeech));
        pipeline.close();
    }

    #[test]
    fn does_not_look_at_all_audio_when_quiet_speech_is_off() {
        let (mut pipeline, _, steps) = speak(
            &concat(&[&voice(1000.0, -75.0), &silence(3500.0)]),
            |_| "Который час?".into(),
            InterimOptions { quiet_speech: false, ..OPTIONS },
        );
        assert!(steps.is_empty());
        assert_eq!(pipeline.ended(), Some(SpeechEndReason::NoSpeech));
        pipeline.close();
    }

    /// Live 12.11: «Алиса, стоп» was recognised in 3 s, and the noise VAD heard as speech kept the command open to 15.
    #[test]
    fn ends_speech_vad_cannot_end_once_its_words_stop_changing() {
        let command = concat(&[&voice(1000.0, -50.0), &noisy_room(6000.0)]);
        let (mut pipeline, interim, steps) = speak(&command, |_| "Отлично.".into(), OPTIONS);

        let last = steps.last().unwrap();
        assert_eq!(last.end, Some(InterimEnd::Unchanged));
        // The words came with the first result; two seconds of audio later they are the end.
        let unchanged = last.job.at_ms - steps[0].job.at_ms;
        assert!((2000.0..2600.0).contains(&unchanged), "{unchanged}");
        assert!(steps[..steps.len() - 1].iter().all(|step| step.end.is_none()));
        // VAD heard speech all along and had not ended it.
        assert!(pipeline.speech_start_ms().is_some());
        assert_eq!(pipeline.ended(), None);
        assert_eq!(interim.text(), "Отлично.");
        pipeline.close();
    }

    #[test]
    fn leaves_that_end_to_vad_when_it_is_off() {
        let command = concat(&[&voice(1000.0, -50.0), &noisy_room(6000.0)]);
        let (mut pipeline, _, steps) =
            speak(&command, |_| "Отлично.".into(), InterimOptions { unchanged_end_ms: 0, ..OPTIONS });
        assert!(steps.len() > 8, "{}", steps.len());
        assert!(steps.iter().all(|step| step.end.is_none()));
        assert_eq!(pipeline.ended(), None, "the noise still sounds like speech: only VAD_MAX_SPEECH_MS ends it");
        pipeline.close();
    }

    #[test]
    fn ends_a_stop_as_soon_as_the_next_result_says_it_again() {
        let command = concat(&[&voice(700.0, -50.0), &noisy_room(5000.0)]);
        let (mut pipeline, _, steps) = speak(&command, |_| "Стоп.".into(), OPTIONS);

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].end, None, "one result alone could go on: «стоп, а какая погода?»");
        assert_eq!(steps[1].end, Some(InterimEnd::Stop));
        assert!(steps[1].job.at_ms - steps[0].job.at_ms >= 400.0);
        pipeline.close();
    }

    #[test]
    fn does_not_end_a_stop_that_goes_on() {
        let command = concat(&[&voice(1500.0, -50.0), &noisy_room(5000.0)]);
        let mut replies: std::collections::VecDeque<&str> = ["Стоп.", "Стоп, а какая погода?"].into();
        let (mut pipeline, interim, steps) =
            speak(&command, |_| replies.pop_front().unwrap_or("Стоп, а какая погода?").to_string(), OPTIONS);

        assert!(steps.iter().all(|step| step.end != Some(InterimEnd::Stop)));
        assert_eq!(steps.last().unwrap().end, Some(InterimEnd::Unchanged));
        assert_eq!(interim.text(), "Стоп, а какая погода?");
        pipeline.close();
    }

    #[test]
    fn waits_for_the_end_of_speech_after_a_stop_when_its_end_is_off() {
        let command = concat(&[&voice(700.0, -50.0), &noisy_room(5000.0)]);
        let (mut pipeline, _, steps) =
            speak(&command, |_| "Стоп.".into(), InterimOptions { stop_end_ms: 0, ..OPTIONS });
        assert!(steps.len() > 2, "{}", steps.len());
        assert_eq!(steps.last().unwrap().end, Some(InterimEnd::Unchanged));
        pipeline.close();
    }
}
