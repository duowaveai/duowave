//! A model that answers nothing and says why at once (docs/plans/home-and-helpers.md, block 1, stage 4): what «Мозг»
//! gives when no model it may ask is on the line — the helper with the chosen model is away, there is no backup, the
//! regulator keeps the clouds in. Its one event is the failure in words: the station says it in place of the error
//! phrase (`with_error_phrase`), the check chat shows it — in well under a second instead of after a connection's ten.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::llm_client::{LlmClient, LlmRequest, LlmStream, SpokenFailure};

pub struct Unreachable {
    phrase: String,
}

impl Unreachable {
    pub fn new(phrase: String) -> Unreachable {
        Unreachable { phrase }
    }
}

impl LlmClient for Unreachable {
    fn description(&self) -> String {
        "no model on the line".into()
    }

    /// The route stays the one the household knows (a command, a conversation): the answer is the words either way.
    fn tools(&self) -> bool {
        true
    }

    fn stream(&self, _request: LlmRequest, _cancel: CancellationToken) -> LlmStream {
        let failure = SpokenFailure { phrase: self.phrase.clone(), detail: self.phrase.clone() };
        futures::stream::iter([Err(anyhow::Error::new(failure))]).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn says_its_words_as_its_only_event_at_once() {
        let words = Unreachable::new("Компьютер дома не на связи, а запасной модели нет.".into());
        let events: Vec<_> = words.stream(LlmRequest::default(), CancellationToken::new()).collect().await;
        assert_eq!(events.len(), 1);
        let error = events.into_iter().next().unwrap().unwrap_err();
        let failure = error.downcast_ref::<SpokenFailure>().expect("words, not an error to hide");
        assert_eq!(failure.phrase, "Компьютер дома не на связи, а запасной модели нет.");
        assert_eq!(error.to_string(), failure.phrase, "the panel's chat shows the same words");
        assert!(words.tools());
    }
}
