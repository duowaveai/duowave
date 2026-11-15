//! A tool marked «В фоне» (docs/plans/long-tools.md, point 1): its call is not awaited in the conversation — the talk
//! layer makes it a work (talk/tool_work.rs) and answers at once «Запустила: …». The server is reached later, on the
//! gateway's own workers, through `McpService::call_in_background`, with a longer wait than a spoken answer's.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::dialog::ToolOutcome;

/// A conversation called a background tool: what to run, and where the call came from.
#[derive(Debug, Clone)]
pub struct BackgroundCall {
    pub server_id: String,
    pub server_title: String,
    /// The tool's name at its server.
    pub tool: String,
    /// The tool's title as people read it.
    pub tool_title: String,
    pub input: Map<String, Value>,
    /// The model's short name of the job (`_title`, taken out of the input), if it gave one.
    pub title: Option<String>,
    /// Its answers bring text from outside the house.
    pub outside: bool,
    /// A run costs money: asked first.
    pub paid: bool,
    /// Where the call came from: a station's phrase, the panel's talk; None — nobody to tell (a test).
    pub origin: Option<crate::talk::Origin>,
}

/// Who takes a background call: the talk layer's executor «вызов инструмента».
pub trait BackgroundRunner: Send + Sync {
    /// Begins the work (or asks first, for a paid run) and returns the ready phrase for the conversation.
    fn begin(&self, call: BackgroundCall) -> ToolOutcome;
}

pub type Runner = Arc<dyn BackgroundRunner>;

/// The model's name for the job, an argument the gateway adds to a background tool and takes out before the call.
pub const TITLE_ARG: &str = "_title";

/// A background tool's schema for the model: the server's, with `_title` beside its own arguments.
pub fn with_title(schema: &Value, lang_hint: &str) -> Value {
    let mut schema = schema.clone();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert(TITLE_ARG.into(), serde_json::json!({ "type": "string", "description": lang_hint }));
    } else if let Some(object) = schema.as_object_mut() {
        let mut properties = Map::new();
        properties.insert(TITLE_ARG.into(), serde_json::json!({ "type": "string", "description": lang_hint }));
        object.insert("properties".into(), Value::Object(properties));
    }
    schema
}

/// Takes the model's `_title` out of the arguments: the server never sees it.
pub fn take_title(input: &mut Map<String, Value>) -> Option<String> {
    let title = input.remove(TITLE_ARG)?;
    title.as_str().map(str::trim).filter(|title| !title.is_empty()).map(|title| title.chars().take(80).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_title_is_offered_to_the_model_and_never_reaches_the_server() {
        let schema = with_title(&json!({ "type": "object", "properties": { "actor": { "type": "string" } } }), "hint");
        assert_eq!(schema["properties"]["_title"]["type"], json!("string"));
        assert_eq!(schema["properties"]["actor"]["type"], json!("string"));
        let bare = with_title(&json!({ "type": "object" }), "hint");
        assert_eq!(bare["properties"]["_title"]["description"], json!("hint"));
        let mut input = json!({ "actor": "apify/x", "_title": "  Отзывы о кофейне " }).as_object().unwrap().clone();
        assert_eq!(take_title(&mut input).as_deref(), Some("Отзывы о кофейне"));
        assert!(!input.contains_key("_title"));
        assert_eq!(take_title(&mut input), None);
    }
}
