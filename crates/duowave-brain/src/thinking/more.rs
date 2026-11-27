//! «Спросить ещё» (docs/plans/thinking.md, §6): the card live, and the tool `open`.
//!
//! - **`open {groups}`**: «Get the tools of these groups for your next step.» At most `card.caps.open` groups a call.
//!   Its result is a short line («Opened reminders: set_reminder.»), and its outcome opens the groups' tools: the
//!   rounds offer them from the next round on (`ToolOutcome.opens`). After a text from outside the house only the
//!   safe ones stay offered, as the toolbox's own filter keeps today. A group that is not in the catalog cannot be
//!   opened: the result says so. The folded groups' line (`apps`) opens their list.
//! - **`recall`** stays as it is (memory/tools.rs).
//! - `open` is not offered in the last round with tools (`ToolBox.not_last`): a group opened then could not be used.
//! - The delivery is `append` (the opened tools go at the end of the list); `deferred` and `use` are stage M6's.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::ToolOutcome;

use super::card::{start_card, Card, CardMaker, Input, StartCard, OPEN};
use super::catalog::FOLDED;
use super::settings::{CardVariant, Delivery};

/// The groups opened in a turn, in the order of the `open` calls: the turn's line takes them.
#[derive(Clone, Default)]
pub struct Opened(Arc<Mutex<Vec<Vec<String>>>>);

impl Opened {
    pub fn push(&self, groups: Vec<String>) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(groups);
    }

    pub fn all(&self) -> Vec<Vec<String>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

/// The start card live: its tools offered, the rest in its catalog, opened on the model's `open`.
pub struct Live {
    pub variant: CardVariant,
}

/// The maker of the card's own variants.
pub fn maker(variant: CardVariant) -> Arc<dyn CardMaker> {
    Arc::new(Live { variant })
}

/// What `open` of some groups gives: the line for the model, and the tools to offer next.
pub fn open(card: &StartCard, all: &[LlmTool], call: &ToolCall, most: usize) -> ToolOutcome {
    let asked: Vec<String> = call
        .input
        .get("groups")
        .and_then(Value::as_array)
        .map(|groups| groups.iter().filter_map(|group| group.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if asked.is_empty() {
        return ToolOutcome::error("error: name the groups to open, from the list");
    }
    let mut lines = Vec::new();
    let mut opens: Vec<LlmTool> = Vec::new();
    for (index, id) in asked.iter().enumerate() {
        if index >= most {
            lines.push(format!("Not opened (at most {most} a call): {}.", asked[index..].join(", ")));
            break;
        }
        if id == FOLDED {
            lines.push(format!("More groups:\n{}", card.catalog.folded_text(&card.folded)));
            continue;
        }
        let Some(group) = card.catalog.group(id) else {
            lines.push(format!("No group «{id}» in the list."));
            continue;
        };
        let tools: Vec<LlmTool> = group
            .tools
            .iter()
            .filter_map(|name| all.iter().find(|tool| &tool.name == name).cloned())
            .filter(|tool| !card.tools.iter().any(|on| on.name == tool.name))
            .collect();
        let names: Vec<&str> = group.tools.iter().map(String::as_str).collect();
        lines.push(format!("Opened {id}: {}.", names.join(", ")));
        opens.extend(tools);
    }
    ToolOutcome { content: lines.join("\n"), opens, ..ToolOutcome::default() }
}

impl CardMaker for Live {
    fn variant(&self) -> CardVariant {
        self.variant
    }

    fn make(&self, input: Input<'_>) -> Card {
        let Some((mut toolbox, sources)) = super::card::whole_of(&input) else {
            return Card { variant: self.variant, toolbox: None, start: None };
        };
        let all = toolbox.tools.clone();
        let mut start = start_card(&all, &sources, &input);
        let most = input.settings.card.caps.open;
        // «Подключения» leave the stable part: a server's text comes with its group, when it is opened (§9).
        let servers = toolbox.instructions.take().filter(|text| !text.is_empty());
        let delivery = delivered(input.delivery, input.provider.as_deref());
        match delivery {
            Delivery::Deferred => {
                // The provider's own deferred tools: every tool sent, the card's loaded, the rest found by search.
                start.tools.retain(|tool| tool.name != OPEN);
                toolbox.deferred = all
                    .iter()
                    .filter(|tool| !start.tools.iter().any(|on| on.name == tool.name))
                    .map(|tool| tool.name.clone())
                    .collect();
                toolbox.tool_search = true;
                let mut tools = start.tools.clone();
                tools.extend(all.iter().filter(|tool| toolbox.deferred.contains(&tool.name)).cloned());
                toolbox.tools = tools;
                start.instructions = format!("{}\n{}", DEFERRED_ASK, start.catalog_text);
            }
            Delivery::Use => {
                start.tools.push(use_tool());
                toolbox.tools = start.tools.clone();
                toolbox.not_last = vec![OPEN.to_string()];
            }
            Delivery::Append => {
                toolbox.tools = start.tools.clone();
                toolbox.not_last = vec![OPEN.to_string()];
            }
        }
        toolbox.instructions = Some(start.instructions.clone());
        let (card, everything, opened) = (Arc::new(start.clone()), Arc::new(all), input.opened.clone());
        let (others, servers) = (toolbox.run.clone(), Arc::new(servers));
        toolbox.run = Arc::new(move |call: ToolCall, cancel| {
            let (card, everything, others, opened, servers) =
                (card.clone(), everything.clone(), others.clone(), opened.clone(), servers.clone());
            Box::pin(async move {
                if call.name == OPEN {
                    let mut outcome = open(&card, &everything, &call, most);
                    let groups = asked_groups(&call, most);
                    // A server's group brings its «Подключения».
                    if let Some(servers) =
                        servers.as_deref().filter(|_| groups.iter().any(|group| group.starts_with("mcp:")))
                    {
                        outcome.content = format!("{}\n\n{servers}", outcome.content);
                    }
                    // Through `use`, the opened tools come as text: their names and their parameters.
                    if delivery == Delivery::Use {
                        let described: Vec<String> = outcome
                            .opens
                            .drain(..)
                            .map(|tool| format!("{}: {} {}", tool.name, tool.description, tool.parameters))
                            .collect();
                        if !described.is_empty() {
                            outcome.content =
                                format!("{}\nCall them through use:\n{}", outcome.content, described.join("\n"));
                        }
                    }
                    if let Some(opened) = &opened {
                        opened.push(groups);
                    }
                    return Ok(outcome);
                }
                if call.name == USE && delivery == Delivery::Use {
                    return match used(&call, &everything) {
                        Ok(inner) => others(inner, cancel).await,
                        Err(error) => Ok(ToolOutcome::error(error)),
                    };
                }
                others(call, cancel).await
            })
        });
        Card { variant: self.variant, toolbox: Some(toolbox), start: Some(start) }
    }
}

/// The tool of the delivery `use`.
pub const USE: &str = "use";

/// «How to ask for more» when the provider finds the deferred tools itself.
const DEFERRED_ASK: &str =
    "Tools come in groups. The tools of a group not offered are found by the tool search: search by the group's tools' names. The groups:";

/// The delivery a provider gets: `deferred` only where the provider has its own (Claude's tool search); elsewhere
/// `append`.
pub fn delivered(asked: Delivery, provider: Option<&str>) -> Delivery {
    match (asked, provider) {
        (Delivery::Deferred, Some("anthropic")) => Delivery::Deferred,
        (Delivery::Deferred, _) => Delivery::Append,
        (other, _) => other,
    }
}

/// `use {tool, args}`: one stable tool that calls the opened ones.
pub fn use_tool() -> LlmTool {
    LlmTool {
        name: USE.into(),
        description: "Call a tool that open gave you as text: its name and its arguments.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"tool": {"type": "string"}, "args": {"type": "object"}},
            "required": ["tool"],
        }),
    }
}

/// The groups an `open` asks for, at most `most`.
fn asked_groups(call: &ToolCall, most: usize) -> Vec<String> {
    call.input
        .get("groups")
        .and_then(Value::as_array)
        .map(|groups| groups.iter().filter_map(|group| group.as_str().map(str::to_string)).take(most).collect())
        .unwrap_or_default()
}

/// The call `use` makes: the tool must be one of the turn's, and its arguments an object with the required fields.
pub fn used(call: &ToolCall, all: &[LlmTool]) -> Result<ToolCall, String> {
    let name = call.input.get("tool").and_then(Value::as_str).ok_or("error: name the tool to use")?;
    let tool = all.iter().find(|tool| tool.name == name).ok_or_else(|| format!("error: no tool {name} here"))?;
    let args = match call.input.get("args") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(_) => return Err("error: args must be an object".into()),
    };
    let required: Vec<&str> = tool
        .parameters
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(missing) = required.iter().find(|field| !args.contains_key(**field)) {
        return Err(format!("error: {name} needs {missing}"));
    }
    Ok(ToolCall { id: call.id.clone(), name: name.to_string(), input: args, error: None })
}

#[cfg(test)]
mod tests {
    use super::super::catalog::Catalog;
    use super::*;
    use crate::talk::tools::Sources;
    use serde_json::json;

    fn tool(name: &str) -> LlmTool {
        LlmTool { name: name.into(), description: format!("{name}."), parameters: json!({"type": "object"}) }
    }

    fn card(all: &[LlmTool]) -> StartCard {
        let catalog = Catalog::of(all, &Sources::default(), &[], None);
        StartCard {
            tools: vec![tool("recall"), super::super::card::open_tool(3)],
            catalog,
            catalog_text: String::new(),
            folded: vec!["games".into()],
            picked: Vec::new(),
            from: Vec::new(),
            whole: Vec::new(),
            instructions: String::new(),
        }
    }

    fn call(groups: &[&str]) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: OPEN.into(),
            input: json!({"groups": groups}).as_object().cloned().unwrap(),
            error: None,
        }
    }

    #[test]
    fn open_gives_the_groups_tools_three_at_most_and_says_what_it_cannot() {
        let all = [tool("set_reminder"), tool("get_weather"), tool("get_news"), tool("smart_home"), tool("start_game")];
        let card = card(&all);
        let outcome = open(&card, &all, &call(&["reminders", "weather"]), 3);
        let names: Vec<&str> = outcome.opens.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["set_reminder", "get_weather"]);
        assert!(outcome.content.contains("Opened reminders: set_reminder."), "{}", outcome.content);
        let many = open(&card, &all, &call(&["reminders", "weather", "news", "smart_home"]), 3);
        assert_eq!(many.opens.len(), 3);
        assert!(many.content.contains("Not opened (at most 3 a call): smart_home."), "{}", many.content);
        let unknown = open(&card, &all, &call(&["calendar"]), 3);
        assert!(unknown.opens.is_empty() && unknown.content.contains("No group «calendar»"));
        let folded = open(&card, &all, &call(&[FOLDED]), 3);
        assert!(folded.content.contains("- games:"), "{}", folded.content);
        assert!(open(&card, &all, &call(&[]), 3).is_error);
    }

    #[test]
    fn use_calls_a_tool_of_the_turn_with_its_arguments_checked() {
        let mut reminder = tool("set_reminder");
        reminder.parameters = json!({"type": "object", "required": ["text"]});
        let all = [reminder];
        let call = |input: serde_json::Value| ToolCall {
            id: "u".into(),
            name: USE.into(),
            input: input.as_object().cloned().unwrap(),
            error: None,
        };
        let made = used(&call(json!({"tool": "set_reminder", "args": {"text": "чайник"}})), &all).unwrap();
        assert_eq!((made.name.as_str(), made.input["text"].as_str()), ("set_reminder", Some("чайник")));
        assert!(used(&call(json!({"tool": "set_reminder", "args": {}})), &all).unwrap_err().contains("needs text"));
        assert!(used(&call(json!({"tool": "nope"})), &all).is_err());
        assert!(used(&call(json!({"tool": "set_reminder", "args": [1]})), &all).is_err());
    }

    #[test]
    fn deferred_is_claude_s_and_the_others_get_append() {
        assert_eq!(delivered(Delivery::Deferred, Some("anthropic")), Delivery::Deferred);
        assert_eq!(delivered(Delivery::Deferred, Some("openai")), Delivery::Append);
        assert_eq!(delivered(Delivery::Use, Some("openai")), Delivery::Use);
    }
}
