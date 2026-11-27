//! What the model gets in the first round of a turn (docs/plans/thinking.md, §4): the card, made by one of the variants
//! behind `CardMaker` and picked by the setting `thinking.card.variant`.
//!
//! - `all`: every tool, as before the card — today's toolbox (talk/tools.rs), the place's hands as today's choice of
//!   tools. The default until the owner switches, on the numbers. Beside it, the start card is made in shadow
//!   (shadow.rs): what it would have offered, and after the turn whether that would have been enough.
//! - `card`: the start card (`StartCard`) — `recall` and `open` on every card, the place's usual tools, the works'
//!   group while works are in the talk, the phrase's tools of the groups its words pick, and the catalog of the rest.
//!   Live from stage M4 (more.rs); `card_sense` the same with «Чутьё»'s model step for the groups.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::identity::Principal;
use crate::core::requests::RequestHandle;
use crate::dialog::estimate_tokens;
use crate::dialog::llm::LlmTool;
use crate::dialog::ToolBox;
use crate::talk::tools::Sources;
use crate::talk::{Brain, Origin, Parts, TalkEvent};

use super::catalog::{self, AppTitles, Catalog};
use super::settings::{CardVariant, Settings};

/// The tool that opens groups (more.rs).
pub const OPEN: &str = "open";

/// What a card is made for: the turn's parts, who speaks, the phrase and the place's hands.
pub struct Input<'a> {
    pub parts: &'a Parts,
    pub principal: &'a Principal,
    pub text: &'a str,
    pub origin: Origin,
    pub trip: Arc<RequestHandle>,
    /// The talk's events of a tool at work (`tool`): the turn says them as `wait`.
    pub events: mpsc::UnboundedSender<TalkEvent>,
    pub brain: Brain,
    /// The groups of hands of the device that heard (the phrase's `hands`).
    pub hands: &'a [String],
    pub settings: &'a Settings,
    /// The device that heard, and the phrase's language.
    pub device: &'a str,
    pub lang: &'a str,
    /// The place's usual tools, the most called first, and how often each was called.
    pub usual: Vec<String>,
    pub used: BTreeMap<String, usize>,
    /// The apps' names for people.
    pub titles: Option<AppTitles>,
    /// The hands of the place, when the device has any: tools of their own, run through the door.
    pub place: Option<Arc<super::hands::Place>>,
    /// Where the live card keeps the groups the model opened, for the turn's line.
    pub opened: Option<super::more::Opened>,
    /// How the opened tools reach the provider, and the provider's kind (`anthropic`, `openai`).
    pub delivery: super::settings::Delivery,
    pub provider: Option<String>,
    /// The groups «Чутьё» picked for the phrase (kind `groups`); None — the words of the language here.
    pub picked: Option<Vec<String>>,
}

/// A card: the toolbox the model is offered, its name for the turn's line, and the start card — the live one, or
/// the one in shadow.
pub struct Card {
    pub variant: CardVariant,
    pub toolbox: Option<ToolBox>,
    pub start: Option<StartCard>,
}

/// One way of making the card.
pub trait CardMaker: Send + Sync {
    fn variant(&self) -> CardVariant;
    fn make(&self, input: Input<'_>) -> Card;
}

/// The whole toolbox of the turn, and where its tools come from: the conversation's tools, and the hands of the
/// place as tools of their own (hands.rs) — never a station's commands by the kind of device.
pub fn whole_of(input: &Input<'_>) -> Option<(ToolBox, Sources)> {
    let made = crate::talk::tools::toolbox_with(crate::talk::tools::Deps {
        parts: input.parts.clone(),
        principal: input.principal.clone(),
        text: input.text.to_string(),
        origin: input.origin.clone(),
        trip: input.trip.clone(),
        events: input.events.clone(),
        brain: input.brain.clone(),
        kind: crate::skills::tools::ToolsKind::Conversation,
    });
    let Some(place) = input.place.clone().filter(|place| !place.hands.is_empty()) else { return made };
    if !input.parts.dialog.tools_enabled() {
        return made;
    }
    let (mut toolbox, sources) = match made {
        Some(made) => made,
        None => {
            let empty = ToolBox::new(Vec::new(), |call: crate::dialog::llm::ToolCall, _| async move {
                Ok(crate::dialog::ToolOutcome::error(format!("error: {} is not available now", call.name)))
            });
            (empty, Sources::default())
        }
    };
    for hand in &place.hands {
        if !toolbox.tools.iter().any(|tool| tool.name == hand.tool.name) {
            toolbox.tools.push(hand.tool.clone());
        }
    }
    let others = toolbox.run.clone();
    toolbox.run = Arc::new(move |call, cancel| {
        let (place, others) = (place.clone(), others.clone());
        Box::pin(async move {
            if place.hand_of(&call.name).is_some() {
                return Ok(place.run(&call).await);
            }
            others(call, cancel).await
        })
    });
    Some((toolbox, sources))
}

/// Every tool, as before the card; the start card beside it, in shadow.
pub struct All;

impl CardMaker for All {
    fn variant(&self) -> CardVariant {
        CardVariant::All
    }

    fn make(&self, input: Input<'_>) -> Card {
        let made = whole_of(&input);
        let start = made
            .as_ref()
            .filter(|_| input.settings.shadows("card"))
            .map(|(toolbox, sources)| start_card(&toolbox.tools, sources, &input));
        Card { variant: CardVariant::All, toolbox: made.map(|(toolbox, _)| toolbox), start }
    }
}

/// The maker of a variant: `all`, or the card live (more.rs).
pub fn maker(variant: CardVariant) -> Arc<dyn CardMaker> {
    match variant {
        CardVariant::All => Arc::new(All),
        CardVariant::Card | CardVariant::CardSense => super::more::maker(variant),
    }
}

/// The tool `open`: «Get the tools of these groups for your next step.»
pub fn open_tool(most: usize) -> LlmTool {
    LlmTool {
        name: OPEN.into(),
        description: "Get the tools of these groups (ids from the list) for your next step.".into(),
        parameters: json!({
            "type": "object",
            "properties": {"groups": {"type": "array", "items": {"type": "string"}, "maxItems": most}},
            "required": ["groups"],
        }),
    }
}

/// The start card of a turn (§4), made of the tools the turn may be offered whole.
#[derive(Debug, Clone)]
pub struct StartCard {
    /// The card's tools, in order: on every card, the place's usual, the works', the phrase's.
    pub tools: Vec<LlmTool>,
    pub catalog: Catalog,
    /// The catalog's lines as the card says them, and the groups folded out of it.
    pub catalog_text: String,
    pub folded: Vec<String>,
    /// The groups picked for the phrase, and by what.
    pub picked: Vec<String>,
    pub from: Vec<String>,
    /// The groups whose tools came whole: the works' while works are in the talk.
    pub whole: Vec<String>,
    /// «How to ask for more» and the catalog: the stable instructions of the card.
    pub instructions: String,
}

impl StartCard {
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|tool| tool.name.clone()).collect()
    }

    /// The card's own size, in tokens as estimated: its tools and its instructions.
    pub fn tokens(&self) -> u32 {
        tools_tokens(&self.tools) + estimate_tokens(&self.instructions)
    }
}

/// The tools' size, in tokens as estimated.
pub fn tools_tokens(tools: &[LlmTool]) -> u32 {
    tools
        .iter()
        .map(|tool| {
            estimate_tokens(&tool.name)
                + estimate_tokens(&tool.description)
                + estimate_tokens(&tool.parameters.to_string())
        })
        .sum()
}

/// Whether works are in the talk: one going on or waiting for its decision in the home, or a question open here.
fn works_in_the_talk(parts: &Parts, device: &str) -> bool {
    parts.talk.asked.open(device).is_some()
        || parts.talk.works.all(None, None).iter().any(|work| work.going() || work.awaiting())
}

/// The start card for these tools.
pub fn start_card(all: &[LlmTool], sources: &Sources, input: &Input<'_>) -> StartCard {
    let settings = input.settings;
    let caps = settings.card.caps;
    let catalog = Catalog::of(all, sources, &settings.groups, input.titles.as_ref());
    let find = |name: &str| all.iter().find(|tool| tool.name == name).cloned();
    let mut tools: Vec<LlmTool> = Vec::new();
    let push = |tool: Option<LlmTool>, tools: &mut Vec<LlmTool>| {
        if let Some(tool) = tool {
            if !tools.iter().any(|known| known.name == tool.name) {
                tools.push(tool);
            }
        }
    };
    // On every card.
    for name in catalog::always() {
        push(find(name), &mut tools);
    }
    push(Some(open_tool(caps.open)), &mut tools);
    let fixed = tools.len();
    // The groups the phrase's words pick: the language's, then the home's own.
    let mut picked: Vec<String> =
        input.picked.clone().unwrap_or_else(|| super::words::of(input.lang).groups_in(input.text));
    let plain = super::words::plain(input.text);
    for own in &settings.groups {
        let stems = own.words.get(input.lang).into_iter().flatten();
        if stems.into_iter().any(|stem| plain.contains(&super::words::plain(stem))) && !picked.contains(&own.id) {
            picked.push(own.id.clone());
        }
    }
    picked.retain(|group| catalog.group(group).is_some());
    let from = if picked.is_empty() { Vec::new() } else { vec!["words".to_string()] };
    // The works' group comes whole while works are in the talk, or the phrase is about one.
    let mut whole = Vec::new();
    if catalog.group("works").is_some()
        && (works_in_the_talk(input.parts, input.device) || picked.iter().any(|group| group == "works"))
    {
        whole.push("works".to_string());
    }
    // The place's usual tools.
    let mut usual: Vec<LlmTool> = Vec::new();
    for name in input.usual.iter().take(caps.usual) {
        push(find(name), &mut usual);
    }
    // The phrase's tools: of each group picked, its one or two tools used most here.
    let mut phrase: Vec<LlmTool> = Vec::new();
    for group in picked.iter().filter(|group| !whole.contains(group)) {
        let Some(group) = catalog.group(group) else { continue };
        let mut names = group.tools.clone();
        names.sort_by_key(|name| std::cmp::Reverse(input.used.get(name).copied().unwrap_or(0)));
        for name in names.into_iter().take(2) {
            if phrase.len() >= caps.phrase {
                break;
            }
            push(find(&name), &mut phrase);
        }
    }
    // Within the cap: the phrase's last first, then the usual's; the tools on every card stay.
    let budget = |tools: &[LlmTool], usual: &[LlmTool], phrase: &[LlmTool]| {
        tools_tokens(tools) + tools_tokens(usual) + tools_tokens(phrase)
    };
    while budget(&tools[..fixed], &usual, &phrase) > caps.tools && (!phrase.is_empty() || !usual.is_empty()) {
        if phrase.pop().is_none() {
            usual.pop();
        }
    }
    // The order for the cache (§9): what changes least first — on every card, the usual (once a night), the works'
    // (while they last), then the phrase's.
    for tool in usual {
        push(Some(tool), &mut tools);
    }
    for group in &whole {
        if let Some(group) = catalog.group(group) {
            for name in group.tools.clone() {
                push(find(&name), &mut tools);
            }
        }
    }
    for tool in phrase {
        push(Some(tool), &mut tools);
    }
    let (catalog_text, folded) = catalog.text(caps.catalog, &input.used);
    let instructions = format!("{}\n{catalog_text}", catalog::ask());
    StartCard { tools, catalog, catalog_text, folded, picked, from, whole, instructions }
}
