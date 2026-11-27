//! A person's own words for what is done with works (docs/plans/work-systems.md, stage 7): «запомни: «глянь» значит
//! «как там»» keeps «глянь» for `status`, and from then on «глянь, что там с сайтом» asks how the work goes, before the
//! pack's fixed patterns and with no model.
//!
//! This is the words step of «Чутьё»'s kind `work` (docs/plans/audio-thinking-contract.md, «Чутьё»), kept here until
//! «Чутьё» (`duowave-sense`, built on the audio interface's branch) takes it: then it gets these words and
//! `options_of` — the state and the options it is asked with — and a phrase that neither knows goes on to the model
//! step, the reader, which steers works in any language, a language without a pack too.

use serde::{Deserialize, Serialize};

use crate::core::settings::{SettingDefinition, SettingScope};

/// The actions a person's own words may stand for: those a phrase steers a going work by.
pub const ACTIONS: [&str; 5] = ["status", "stop", "details", "watch", "unwatch"];
/// The most own words kept, and the longest.
const MAX: usize = 50;
const LONGEST: usize = 40;

/// One's own words for an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnWord {
    pub words: String,
    pub action: String,
}

/// The household's own words. Travels with the settings package.
pub const OWN_WORDS: SettingDefinition<Vec<OwnWord>> = SettingDefinition {
    key: "talk_own_words",
    scope: SettingScope::Global,
    description: "Свои слова для работ",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// A phrase (normalized) that begins with one's own words: the action, and the rest — what it is about.
pub fn heard(own: &[OwnWord], text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim().to_lowercase();
    own.iter().filter(|word| !word.words.trim().is_empty()).find_map(|word| {
        let words = word.words.trim().to_lowercase();
        let rest = text.strip_prefix(&words)?;
        if !(rest.is_empty() || rest.starts_with([' ', ','])) {
            return None;
        }
        let rest = rest.trim_matches(|c: char| c == ',' || c.is_whitespace());
        Some((word.action.clone(), Some(rest.to_string()).filter(|rest| !rest.is_empty())))
    })
}

/// Keeps one's own words for an action: the same words again take the new action; at most `MAX`, the oldest goes.
/// False — not kept (empty, too long, or an action that is not one of `ACTIONS`).
pub fn keep(own: &mut Vec<OwnWord>, words: &str, action: &str) -> bool {
    let words = words.trim().trim_matches(['«', '»', '"', '“', '”']).trim().to_lowercase();
    if words.is_empty() || words.chars().count() > LONGEST || !ACTIONS.contains(&action) {
        return false;
    }
    own.retain(|word| word.words != words);
    own.push(OwnWord { words, action: action.to_string() });
    while own.len() > MAX {
        own.remove(0);
    }
    true
}

/// The options and the state «Чутьё»'s kind `work` is asked with: the actions with the words that name them now, the
/// works in play, the systems by their names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkOptions {
    pub actions: Vec<ActionWords>,
    pub works: Vec<WorkInPlay>,
    pub systems: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ActionWords {
    pub action: String,
    /// One's own words for it; the pack's are the words step's own.
    pub own: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkInPlay {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

impl super::TalkLayer {
    /// One's own words now.
    pub fn own_words(&self) -> Vec<OwnWord> {
        self.own_words.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Where one's own words are kept: read now, written on each «запомни».
    pub fn keep_words_in(&self, settings: std::sync::Arc<crate::settings::SettingsService>) {
        let kept = settings.get(&OWN_WORDS, None).unwrap_or_default();
        *self.own_words.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = kept;
        let _ = self.words_kept.set(settings);
    }

    /// «Запомни: «глянь» значит «как там»»: the meaning read by the pack's words of the actions; the words said back.
    pub(super) fn learn_words(&self, device: &str, words: &str, meaning: &str) -> String {
        let lang = self.lang(device);
        let said = &super::pack::of(lang).work.say;
        let Some(action) = super::rules::TalkRules::action_named(lang, meaning) else {
            return super::pack::fill(said.not_learned, &[("what", meaning.trim())]);
        };
        let mut own = self.own_words();
        if !keep(&mut own, words, action) {
            return super::pack::fill(said.not_learned, &[("what", meaning.trim())]);
        }
        if let Some(settings) = self.words_kept.get() {
            if let Err(error) = settings.set(&OWN_WORDS, &own, None) {
                tracing::warn!(target: "talk", "own words not kept: {error:?}");
            }
        }
        *self.own_words.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = own;
        super::pack::fill(said.learned, &[("words", words.trim()), ("what", meaning.trim())])
    }

    /// The options and the state of «Чутьё»'s kind `work` on a device.
    pub fn options_of(&self, device: &str) -> WorkOptions {
        let own = self.own_words();
        let actions = ACTIONS
            .iter()
            .map(|action| ActionWords {
                action: action.to_string(),
                own: own.iter().filter(|word| word.action == *action).map(|word| word.words.clone()).collect(),
            })
            .collect();
        let works = self
            .works
            .all(Some(device), None)
            .into_iter()
            .filter(|work| work.going() || work.awaiting())
            .map(|work| WorkInPlay { id: work.id, title: work.title, system: work.system })
            .collect();
        let systems = self.systems().map(|systems| systems.registry().names()).unwrap_or_default();
        WorkOptions { actions, works, systems }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::talk::origin::{Activator, Origin, COMPUTER};
    use crate::talk::rules::{Heard, TalkRules};
    use crate::talk::works::{NewWork, FOLDER};
    use crate::talk::TalkLayer;

    /// Stage 7 of docs/plans/work-systems.md: «глянь …» is no pack's words, so the rules let it go to the reader (the
    /// model step); after «запомни: глянь значит как там» it is heard with no model as «как там», about the work it
    /// names. The options «Чутьё» will be asked with carry the words and the works in play.
    #[tokio::test]
    async fn a_phrase_no_pack_has_is_heard_after_it_is_taught() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let work = layer.works.create(NewWork {
            kind: FOLDER,
            face: None,
            executor: "folder".into(),
            target: Some("w1".into()),
            title: "Тёмная тема".into(),
            brief: "сделай тёмную тему".into(),
            origin: Origin::computer(Activator::Text),
            again: false,
        });
        let rules = TalkRules::new(&layer);
        assert_eq!(rules.hear("глянь тёмную тему", COMPUTER), None, "no pack has it: the reader reads it");
        let heard = rules.hear("запомни глянь значит как там", COMPUTER);
        assert_eq!(heard, Some(Heard::Learn { words: "глянь".into(), meaning: "как там".into() }));
        assert_eq!(layer.learn_words(COMPUTER, "глянь", "как там"), "Запомнила: «глянь» значит «как там».");
        assert_eq!(layer.learn_words(COMPUTER, "кыш", "погода"), "Не поняла, что значит «погода».");
        let heard = rules.hear("глянь тёмную тему", COMPUTER);
        assert!(matches!(heard, Some(Heard::Status { .. })), "{heard:?}");
        let options = layer.options_of(COMPUTER);
        let status = options.actions.iter().find(|action| action.action == "status").unwrap();
        assert_eq!(status.own, ["глянь"]);
        assert_eq!(options.works.first().map(|seen| seen.id.as_str()), Some(work.id.as_str()));
        // In English, the words step works the same.
        assert_eq!(TalkRules::action_named(crate::memory::text::Lang::En, "how is it going"), Some("status"));
    }

    /// «следи за haiku2»: a word that ends in a number is a name, not the label of work 2 (the hand check of 27.11).
    #[tokio::test]
    async fn a_word_ending_in_a_number_is_a_name_not_a_label() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let new = |title: &str| NewWork {
            kind: FOLDER,
            face: None,
            executor: "folder".into(),
            target: Some(format!("t-{title}")),
            title: title.into(),
            brief: title.into(),
            origin: Origin::computer(Activator::Text),
            again: false,
        };
        let first = layer.works.create(new("Старая работа"));
        let haiku = layer.works.create(new("Напиши в файл haiku2.md три хайку"));
        assert_ne!(first.id, haiku.id);
        let label = format!("р{}", first.id);
        match layer.find(COMPUTER, Some("haiku2")) {
            crate::talk::work_tools::Found::One(work) => assert_eq!(work.id, haiku.id),
            _ => panic!("not that one work"),
        }
        match layer.find(COMPUTER, Some(&label)) {
            crate::talk::work_tools::Found::One(work) => assert_eq!(work.id, first.id, "«р1» is still a label"),
            _ => panic!("not that one work"),
        }
    }

    #[test]
    fn ones_own_words_are_heard_at_the_start_of_a_phrase_and_kept_once() {
        let mut own = Vec::new();
        assert!(keep(&mut own, "«Глянь»", "status"));
        assert!(!keep(&mut own, "", "status"));
        assert!(!keep(&mut own, "удали всё", "delete"), "not an action of works");
        assert_eq!(heard(&own, "глянь что там с сайтом"), Some(("status".into(), Some("что там с сайтом".into()))));
        assert_eq!(heard(&own, "глянь"), Some(("status".into(), None)));
        assert_eq!(heard(&own, "глянька"), None, "a whole word");
        assert!(keep(&mut own, "глянь", "stop"));
        assert_eq!(own.len(), 1, "the same words take the new action");
        assert_eq!(heard(&own, "глянь, сайт").map(|(action, _)| action).as_deref(), Some("stop"));
    }
}
