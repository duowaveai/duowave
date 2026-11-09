//! No model: repeats the question. Checks recognition, speech and playback without an API key.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::llm_client::{AssistantTurn, LlmClient, LlmEvent, LlmMessage, LlmRequest, LlmStream};

pub struct EchoLlm;

impl LlmClient for EchoLlm {
    fn description(&self) -> String {
        "echo (repeats the question, no LLM)".into()
    }

    fn tools(&self) -> bool {
        false
    }

    fn stream(&self, request: LlmRequest, _cancel: CancellationToken) -> LlmStream {
        let last = match request.messages.last() {
            Some(LlmMessage::User(text)) => text.as_str(),
            Some(LlmMessage::Assistant(turn)) => turn.content.as_str(),
            Some(LlmMessage::Tool(_)) | None => "",
        };
        let text = format!("Вы сказали: {last}");
        let turn = AssistantTurn { content: text.clone(), ..AssistantTurn::default() };
        futures::stream::iter([Ok(LlmEvent::Text(text)), Ok(LlmEvent::Turn(turn))]).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repeats_the_question() {
        let request = LlmRequest { messages: vec![LlmMessage::User("Привет".into())], ..LlmRequest::default() };
        let events: Vec<LlmEvent> =
            EchoLlm.stream(request, CancellationToken::new()).map(|event| event.unwrap()).collect().await;
        assert_eq!(events[0], LlmEvent::Text("Вы сказали: Привет".into()));
        assert!(matches!(&events[1], LlmEvent::Turn(turn) if turn.content == "Вы сказали: Привет"));
        assert!(!EchoLlm.tools());
    }
}
