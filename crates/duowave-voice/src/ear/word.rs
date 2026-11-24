//! An ear's word: a wake word that names a character switches the house to it (src/activator hears the Station's).

/// Who answers a request an activator's word began (docs/plans/wake-word-station.md, «Кто отвечает на слово»). Today
/// one character lives in the house (the owner, 16.11): the word's character becomes the house's — this request's
/// answer on, until someone chooses another. Later several characters may live at once, each on a word of its own:
/// then the word picks who answers its request and the house's choice stays — the same call, another answer behind it.
/// Called once the request has words: a word heard in silence switches nobody.
pub trait WordCharacter: Send + Sync {
    /// `character` (`<package>:<character>`) answers from now on, its text at once and its voice right after; false
    /// when it is not here (the word's app without its character) or answers already. `word`: the word's title — who
    /// switched it, for the journal.
    fn answer_as(&self, character: &str, word: &str) -> bool;
}

/// An app's word that wakes the station (`contributes.activators`, src/activator): the id its letter names, the word
/// as people say it, how recognition may write it (the command's text loses those words), and the character the
/// request goes to (`<package>:<character>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatorSpec {
    pub id: String,
    pub title: String,
    pub heard: Vec<String>,
    pub character: Option<String>,
}
