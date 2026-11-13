//! Apps asking each other in words (docs/plans/bus.md, «Вопросы словами», decision 4): «Утро» asks «Календарь» «что
//! у меня завтра?», and «Календарь» has no `ask` of its own. The household's model answers for it with its tools and
//! nothing else; every tool it calls is the asker's letter in the asker's chain, so the bus decides each one as it
//! would a letter the asker wrote. When the model is in the cloud, what the apps know may leave the house: the owner's
//! «Вопросы словами через облако» must be on, or the question is refused with that reason. Nothing of it goes to the
//! household's memory (the key `bus:` is no episode, src/memory/service.rs).

use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::bus::{Address, Bus, BusError, Trail, Words};
use crate::dialog::dialog_service::{DialogContext, DialogService, ToolBox, ToolOutcome};
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::plugins::PluginHost;

/// A tool's answer the model reads: text as it is, anything else as JSON, never longer than this.
const ANSWER_MAX: usize = 4000;

pub struct BrainWords {
    dialog: Weak<DialogService>,
    plugins: Weak<PluginHost>,
    bus: Weak<Bus>,
}

impl BrainWords {
    pub fn new(dialog: &Arc<DialogService>, plugins: &Arc<PluginHost>, bus: &Arc<Bus>) -> BrainWords {
        BrainWords { dialog: Arc::downgrade(dialog), plugins: Arc::downgrade(plugins), bus: Arc::downgrade(bus) }
    }
}

fn text_of(answer: Value) -> String {
    let text = match answer {
        Value::String(text) => text,
        other => other.to_string(),
    };
    if text.len() <= ANSWER_MAX {
        return text;
    }
    let mut end = ANSWER_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

impl Words for BrainWords {
    fn answer(
        &self,
        trail: Trail,
        asker: Address,
        app: Address,
        question: String,
    ) -> BoxFuture<'static, Result<Value, BusError>> {
        let (dialog, plugins, bus) = (self.dialog.upgrade(), self.plugins.upgrade(), self.bus.upgrade());
        Box::pin(async move {
            let gone = || BusError::Failed("шлюз останавливается".into());
            let (dialog, plugins, bus) = (dialog.ok_or_else(gone)?, plugins.ok_or_else(gone)?, bus.ok_or_else(gone)?);
            if !dialog.enabled() {
                return Err(BusError::Failed("отвечать словами некому: модель «Мозга» не выбрана".into()));
            }
            if dialog.brain().cloud && !bus.links.options().cloud_asks {
                return Err(BusError::Denied(
                    "вопросы словами через облако выключены: модель «Мозга» — облачная («Настройки» → «Связи приложений»)"
                        .into(),
                ));
            }
            let id = app.name().unwrap_or_default().to_string();
            let package = plugins.package(&id).ok_or_else(|| BusError::NotFound(app.to_string()))?;
            let tools: Vec<LlmTool> = package
                .manifest
                .contributes
                .tools
                .iter()
                .map(|tool| LlmTool {
                    name: tool.name.clone(),
                    description: tool.description.chars().take(1024).collect(),
                    parameters: tool.input.clone(),
                })
                .collect();
            let (asker_title, app_title) = (bus.title_of(&asker), package.manifest.title.clone());
            let run = {
                let (bus, trail, asker, app) = (bus.clone(), trail.clone(), asker.clone(), app.clone());
                move |call: ToolCall, _cancel| {
                    let (bus, trail, asker, app) = (bus.clone(), trail.clone(), asker.clone(), app.clone());
                    async move {
                        if let Some(error) = call.error {
                            return Ok(ToolOutcome::error(error));
                        }
                        let input = Value::Object(call.input);
                        Ok(match bus.call(trail, asker, app, &call.name, input, Vec::new()).await {
                            Ok(answer) => ToolOutcome { content: text_of(answer), is_error: false, tainted: false },
                            Err(error) => ToolOutcome::error(error.to_string()),
                        })
                    }
                }
            };
            let toolbox = ToolBox::new(tools, run);
            let context = DialogContext {
                system: Some(format!(
                    "Ты отвечаешь за приложение «{app_title}» дома. Тебя спрашивает приложение «{asker_title}». Отвечай \
                     коротко и только по данным своих инструментов; чего не знаешь — так и скажи. Ничего не выдумывай \
                     и не делай того, о чём не спросили."
                )),
                ..DialogContext::default()
            };
            let key = format!("bus:{asker}>{app}");
            let mut phrases = dialog.answer(&key, &question, context, trail.cancel.clone(), Some(toolbox));
            let mut text = String::new();
            while let Some(phrase) = phrases.next().await {
                let phrase = phrase.map_err(|error| BusError::Failed(error.to_string()))?;
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(phrase.trim());
            }
            Ok(json!({ "text": text, "by": "brain" }))
        })
    }
}
