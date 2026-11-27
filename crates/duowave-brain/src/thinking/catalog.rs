//! The catalog of groups (docs/plans/thinking.md, §5): every tool the person may be offered, in groups by meaning,
//! one short English line for the model each. The groups are data: the built-in ones in `groups.json` beside this
//! file, an app's from its package (one group an app, `app:<id>`, its name in «ёлочки»), an MCP server's (`mcp:<its
//! title>`), the home's own in the setting (`thinking.groups`, which may also move a tool into a group of its own).
//!
//! - The catalog is made of the tools a turn would be offered whole (the card `all`): so it shows only what the person
//!   may use and only the hands this place has, as the toolbox does (`tools_for`, the servers open to the voice).
//! - `recall` is on every card: it is in no group.
//! - Over its cap the groups used least fold, an app's and a server's first, into one line: «apps: 9 more, open
//!   `apps` to list them».

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use serde::Deserialize;

use crate::dialog::estimate_tokens;
use crate::dialog::llm::LlmTool;
use crate::talk::tools::Sources;

use super::settings::OwnGroup;

/// The folded groups' own id: `open` of it lists them.
pub const FOLDED: &str = "apps";

#[derive(Debug, Clone, Deserialize)]
struct Builtin {
    id: String,
    line: String,
    tools: Vec<String>,
    #[serde(default)]
    hands: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct File {
    ask: String,
    always: Vec<String>,
    groups: Vec<Builtin>,
}

static FILE: LazyLock<File> =
    LazyLock::new(|| serde_json::from_str(include_str!("groups.json")).expect("thinking/groups.json reads"));

/// «How to ask for more», the stable instruction before the catalog's lines.
pub fn ask() -> &'static str {
    &FILE.ask
}

/// The tools on every card, in no group.
pub fn always() -> &'static [String] {
    &FILE.always
}

/// Where a group comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Builtin,
    /// The device's hands (the station's commands today).
    Hands,
    App,
    Mcp,
    Own,
    /// A tool no group names: its name's first word.
    Other,
}

/// One group.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    /// For people, when it has a name of its own: an app's.
    pub title: Option<String>,
    pub line: String,
    /// The names of its tools offered now, in the offer's order.
    pub tools: Vec<String>,
    pub from: From,
}

impl Group {
    /// Its line in the catalog: `- weather: the weather now and the forecast (get_weather)`.
    pub fn text(&self) -> String {
        let title = self.title.as_deref().map(|title| format!(" «{title}»")).unwrap_or_default();
        format!("- {}{title}: {} ({})", self.id, self.line, self.tools.join(", "))
    }
}

/// The catalog of one turn.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub groups: Vec<Group>,
    by_tool: HashMap<String, String>,
}

/// An app's name for people, by its id: the floor of the apps gives it.
pub type AppTitles = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

impl Catalog {
    /// The catalog of the tools a turn may be offered, each in its group.
    pub fn of(tools: &[LlmTool], sources: &Sources, own: &[OwnGroup], titles: Option<&AppTitles>) -> Catalog {
        let builtin: HashMap<&str, &Builtin> =
            FILE.groups.iter().flat_map(|group| group.tools.iter().map(move |tool| (tool.as_str(), group))).collect();
        let owned: HashMap<&str, &OwnGroup> =
            own.iter().flat_map(|group| group.tools.iter().map(move |tool| (tool.as_str(), group))).collect();
        let mut groups: Vec<Group> = Vec::new();
        let mut by_tool = HashMap::new();
        for tool in tools {
            let name = tool.name.as_str();
            if always().iter().any(|always| always == name) {
                continue;
            }
            // By meaning first: an app's `weather_now` is the weather, whoever runs it.
            let (id, title, line, from) = if let Some(group) = owned.get(name) {
                (group.id.clone(), None, group.line.clone(), From::Own)
            } else if let Some(group) = builtin.get(name) {
                let from = if group.hands { From::Hands } else { From::Builtin };
                (group.id.clone(), None, group.line.clone(), from)
            } else if let Some(app) = sources.contributed.as_ref().and_then(|offer| offer.app_of(name)) {
                let title = titles.and_then(|titles| titles(app));
                let line = first_words(&tool.description, 8);
                (format!("app:{app}"), title, line, From::App)
            } else if let Some((server, _)) =
                sources.mcp.as_ref().filter(|mcp| mcp.owns(name)).and_then(|mcp| mcp.server_of(name))
            {
                (format!("mcp:{server}"), None, format!("the tools of the connection «{server}»"), From::Mcp)
            } else {
                let word = name.split('_').next().unwrap_or(name).to_string();
                (word, None, first_words(&tool.description, 8), From::Other)
            };
            by_tool.insert(name.to_string(), id.clone());
            match groups.iter_mut().find(|group| group.id == id) {
                Some(group) => group.tools.push(name.to_string()),
                None => groups.push(Group { id, title, line, tools: vec![name.to_string()], from }),
            }
        }
        Catalog { groups, by_tool }
    }

    /// The group a tool is in.
    pub fn group_of(&self, tool: &str) -> Option<&str> {
        self.by_tool.get(tool).map(String::as_str)
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    /// The catalog as the card says it, within `cap` tokens: the groups used least fold first, an app's and a
    /// server's before ours. `used`: how often each tool was called at this place.
    pub fn text(&self, cap: u32, used: &BTreeMap<String, usize>) -> (String, Vec<String>) {
        let mut shown: Vec<&Group> = self.groups.iter().collect();
        let mut folded: Vec<String> = Vec::new();
        let uses = |group: &Group| group.tools.iter().map(|tool| used.get(tool).copied().unwrap_or(0)).sum::<usize>();
        loop {
            let mut lines: Vec<String> = shown.iter().map(|group| group.text()).collect();
            if !folded.is_empty() {
                lines.push(format!("- {FOLDED}: {} more, open `{FOLDED}` to list them", folded.len()));
            }
            let text = lines.join("\n");
            if estimate_tokens(&text) <= cap || shown.len() <= 1 {
                return (text, folded);
            }
            // The one to fold: an app's or a server's before ours, the least used first, the last of equals.
            let pick = shown
                .iter()
                .enumerate()
                .min_by_key(|(index, group)| {
                    let ours = !matches!(group.from, From::App | From::Mcp | From::Other);
                    (ours, uses(group), usize::MAX - index)
                })
                .map(|(index, _)| index)
                .expect("a group to fold");
            folded.push(shown.remove(pick).id.clone());
        }
    }

    /// The lines of the folded groups, for `open` of them.
    pub fn folded_text(&self, folded: &[String]) -> String {
        self.groups.iter().filter(|group| folded.contains(&group.id)).map(Group::text).collect::<Vec<_>>().join("\n")
    }
}

/// The first words of a description: an app's line when nothing better is known.
fn first_words(text: &str, words: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let cut: Vec<&str> = line.split_whitespace().take(words).collect();
    cut.join(" ").trim_end_matches(['.', ',', ':', ';']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::skills::tools::{tools_for, ToolsKind};

    fn tool(name: &str, description: &str) -> LlmTool {
        LlmTool {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn every_built_in_tool_is_in_a_group_and_recall_in_none() {
        let tools = tools_for(ToolsKind::Command, None);
        let mut with_memory = tools.clone();
        with_memory.extend([tool("remember", "r"), tool("recall", "r"), tool("work_start", "w")]);
        let catalog = Catalog::of(&with_memory, &Sources::default(), &[], None);
        for tool in &tools {
            let group = catalog.group_of(&tool.name);
            assert!(group.is_some(), "{}: {group:?}", tool.name);
            assert_ne!(catalog.group(group.unwrap()).unwrap().from, From::Other, "{} has a built-in group", tool.name);
        }
        assert_eq!(catalog.group_of("recall"), None, "recall is on every card");
        assert_eq!(catalog.group_of("remember"), Some("memory"));
        assert_eq!(catalog.group_of("work_start"), Some("works"));
        assert_eq!(catalog.group_of("set_volume"), Some("volume"));
        assert_eq!(catalog.group("timers").unwrap().tools, ["set_timer", "manage_timer"]);
        assert_eq!(catalog.group("volume").unwrap().from, From::Hands);
    }

    #[test]
    fn a_conversation_has_no_hands_and_a_guest_no_group_they_may_not_use() {
        let conversation = Catalog::of(&tools_for(ToolsKind::Conversation, None), &Sources::default(), &[], None);
        assert!(conversation.groups.iter().all(|group| group.from != From::Hands), "{:?}", conversation.groups);
        let guest = household_principal(crate::core::identity::Role::Guest, None);
        let theirs = Catalog::of(&tools_for(ToolsKind::Command, Some(&guest)), &Sources::default(), &[], None);
        let all = Catalog::of(&tools_for(ToolsKind::Command, None), &Sources::default(), &[], None);
        let names = |catalog: &Catalog| catalog.groups.iter().flat_map(|group| group.tools.clone()).collect::<Vec<_>>();
        assert!(names(&theirs).iter().all(|name| names(&all).contains(name)));
        for name in names(&all).iter().filter(|name| !names(&theirs).contains(name)) {
            assert!(theirs.group_of(name).is_none(), "{name} is not the guest's");
        }
    }

    #[test]
    fn the_home_s_own_group_takes_its_tools_and_an_unknown_tool_goes_by_its_first_word() {
        let own = OwnGroup {
            id: "kitchen".into(),
            line: "what the kitchen needs".into(),
            tools: vec!["set_timer".into()],
            words: BTreeMap::new(),
        };
        let tools =
            vec![tool("set_timer", "t"), tool("manage_timer", "m"), tool("egress_rule", "Rules of the way out.")];
        let catalog = Catalog::of(&tools, &Sources::default(), &[own], None);
        assert_eq!(catalog.group_of("set_timer"), Some("kitchen"));
        assert_eq!(catalog.group_of("manage_timer"), Some("timers"));
        assert_eq!(catalog.group_of("egress_rule"), Some("egress"));
        assert_eq!(catalog.group("egress").unwrap().line, "Rules of the way out");
    }

    #[test]
    fn over_its_cap_the_catalog_folds_the_groups_used_least() {
        let mut tools = tools_for(ToolsKind::Command, None);
        tools.extend((0..30).map(|n| tool(&format!("other{n}_do"), "Does one more thing of an app of the home.")));
        let catalog = Catalog::of(&tools, &Sources::default(), &[], None);
        let used = BTreeMap::from([("set_timer".to_string(), 9)]);
        let (text, folded) = catalog.text(300, &used);
        assert!(estimate_tokens(&text) <= 300, "{} tokens: {text}", estimate_tokens(&text));
        assert!(!folded.is_empty());
        assert!(text.contains("- timers:"), "the used one stays: {text}");
        assert!(text.contains(&format!("- {FOLDED}: {} more", folded.len())), "{text}");
        assert!(folded.iter().take(5).all(|id| id.starts_with("other")), "the others fold first: {folded:?}");
        assert!(catalog.folded_text(&folded).lines().count() == folded.len());
        let (whole, none) = catalog.text(100_000, &used);
        assert!(none.is_empty() && whole.lines().count() == catalog.groups.len());
    }
}
