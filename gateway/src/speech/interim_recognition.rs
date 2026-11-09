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

/// What a recognised interim text changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterimOutcome {
    /// Interim results get new text.
    pub changed: bool,
    /// Quiet speech has not changed for `quiet_end_ms`: the command is over.
    pub end: bool,
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
/// Decisions only, no I/O: the live session and a replay run the same logic. The pipeline is the request's; it is
/// passed in on each call.
#[derive(Debug, Clone)]
pub struct InterimRecognition {
    options: InterimOptions,
    text: String,
    count: u32,
    quiet: bool,
    at_ms: Option<f64>,
    /// Quiet speech: normalised text of the latest result and where it first appeared, ms of audio.
    quiet_text: String,
    quiet_since_ms: f64,
}

impl InterimRecognition {
    pub fn new(options: InterimOptions) -> InterimRecognition {
        InterimRecognition {
            options,
            text: String::new(),
            count: 0,
            quiet: false,
            at_ms: None,
            quiet_text: String::new(),
            quiet_since_ms: 0.0,
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

    /// Takes the cleaned text of a job.
    pub fn accept(&mut self, pipeline: &mut SpeechPipeline, job: &InterimJob, text: &str) -> InterimOutcome {
        self.count += 1;
        let mut end = false;
        // Quiet speech is judged only while VAD hears nothing; once it does, its own end applies.
        if job.all_audio && pipeline.speech_start_ms().is_none() {
            if !text.is_empty() && !self.quiet {
                self.quiet = true;
                pipeline.hold_no_speech();
            }
            let normalized = normalize(text);
            if normalized.is_empty() || normalized != self.quiet_text {
                self.quiet_text = normalized;
                self.quiet_since_ms = job.at_ms;
            } else {
                end = job.at_ms - self.quiet_since_ms >= self.options.quiet_end_ms as f64;
            }
        }
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
    use crate::speech::test_audio::{concat, ogg_pages, silence, station_audio, voice};
    use crate::speech::vad::energy_vad::EnergyVadEngine;

    const OPTIONS: InterimOptions =
        InterimOptions { interval_ms: 500, quiet_speech: true, quiet_end_ms: 1000, guard_ms: 320 };

    struct Step {
        job: InterimJob,
        changed: bool,
        end: bool,
    }

    /// Feeds the command 100 ms at a time, as the station streams it, asks for an interim job after each piece and
    /// answers it with `reply`, until the pipeline or the interim recognition ends the command.
    fn speak(
        command: &[i16],
        mut reply: impl FnMut(&InterimJob) -> String,
        options: InterimOptions,
    ) -> (SpeechPipeline, InterimRecognition, Vec<Step>) {
        let endpoint = crate::speech::speech_pipeline::tests::endpoint();
        let mut pipeline = SpeechPipeline::new(
            &EnergyVadEngine::new(-40.0),
            SpeechPipelineOptions { endpoint, gain: gain(), expect_wake_word: true },
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
            let InterimOutcome { changed, end } = interim.accept(&mut pipeline, &job, &text);
            steps.push(Step { job, changed, end });
            if end {
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
        assert!(!steps.iter().any(|step| step.end));
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
        assert!(last.end);
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

        let end_at = steps.iter().position(|step| step.end).unwrap();
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
}
