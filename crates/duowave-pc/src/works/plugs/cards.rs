//! The system cards that come with DuoWave (docs/plans/work-systems.md, stage 4): bb and T3 Code, led by the command
//! plug. Bundled, as Claude Code's is (D6), until the catalog carries them; a card from MagicPlace with the same id
//! takes their place. The commands were read from each program's help (T3 Code 27.11.2026; bb by the report of
//! 26.11), and the paths into their answers are tried in turn until the first live run settles them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use serde_json::Value;

use super::command::{CommandSystem, Folders, SystemCard};
use crate::talk::system::McpCaller;
use crate::talk::systems::Systems;

/// Where a system's program is: by its id (the path set in the panel) and its program's name.
pub type ProgramOf = Arc<dyn Fn(&str, &str) -> Option<PathBuf> + Send + Sync>;

/// A card from MagicPlace with a block `system`: its id, title, program's name and the block.
pub struct CardBlock {
    pub id: String,
    pub title: String,
    pub program: String,
    pub block: Value,
}

/// The systems of cards: the bundled ones, and those of the cards from MagicPlace with a block `system`, each of which
/// takes the place of a bundled one with its id while it is installed.
pub struct CardSystems {
    systems: Weak<Systems>,
    program_of: ProgramOf,
    folders: Folders,
    bundled: HashMap<String, Arc<CommandSystem>>,
    /// The systems made of installed cards, with the block each was made of.
    made: Mutex<HashMap<String, (Value, Arc<CommandSystem>)>>,
    /// The MCP servers, for the verbs `via: "mcp"` (stage 8).
    mcp: std::sync::OnceLock<Arc<dyn McpCaller>>,
}

impl CardSystems {
    /// The bundled systems join the registry at once.
    pub fn new(systems: &Arc<Systems>, program_of: ProgramOf, folders: Folders) -> Arc<CardSystems> {
        let mut made = CardSystems {
            systems: Arc::downgrade(systems),
            program_of,
            folders,
            bundled: HashMap::new(),
            made: Mutex::new(HashMap::new()),
            mcp: std::sync::OnceLock::new(),
        };
        for bundled in bundled() {
            let system = made.make(bundled.id, bundled.title, bundled.program, bundled.card);
            systems.add(system.clone());
            made.bundled.insert(bundled.id.to_string(), system);
        }
        Arc::new(made)
    }

    fn make(&self, id: &str, title: &str, program: &str, card: SystemCard) -> Arc<CommandSystem> {
        let (of, at, name) = (self.program_of.clone(), id.to_string(), program.to_string());
        let system =
            CommandSystem::new(id, title, program, card, Arc::new(move || of(&at, &name)), self.folders.clone());
        if let Some(mcp) = self.mcp.get() {
            system.set_mcp(mcp.clone());
        }
        system
    }

    /// The MCP servers for the verbs `via: "mcp"`: the bundled systems take them now, those of cards when made.
    pub fn set_mcp(&self, mcp: Arc<dyn McpCaller>) {
        for system in self.bundled.values() {
            system.set_mcp(mcp.clone());
        }
        let _ = self.mcp.set(mcp);
    }

    /// The cards installed now: a new or changed one makes its system anew, a removed one leaves (and the bundled one
    /// of its id comes back).
    pub fn sync(&self, cards: Vec<CardBlock>) {
        let Some(systems) = self.systems.upgrade() else { return };
        let mut made = self.made.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now: Vec<String> = cards.iter().map(|card| card.id.clone()).collect();
        for card in cards {
            if made.get(&card.id).is_some_and(|(was, _)| *was == card.block) {
                continue;
            }
            let Some(read) = CommandSystem::card_of(&card.block) else { continue };
            let system = self.make(&card.id, &card.title, &card.program, read);
            systems.add(system.clone());
            made.insert(card.id, (card.block, system));
        }
        let gone: Vec<String> = made.keys().filter(|id| !now.contains(id)).cloned().collect();
        for id in gone {
            made.remove(&id);
            systems.remove(&id);
            if let Some(bundled) = self.bundled.get(&id) {
                systems.add(bundled.clone());
            }
        }
    }
}

/// A bundled card: its id, its title, its program's name, the card.
pub struct Bundled {
    pub id: &'static str,
    pub title: &'static str,
    pub program: &'static str,
    pub card: SystemCard,
}

const BB: &str = r#"{
    "contract": 1,
    "plug": "command",
    "names": { "ru": ["бэбэ", "би би", "bb"], "en": ["bb", "bee bee"] },
    "ready": { "run": ["project", "list", "--json"] },
    "places": { "run": ["project", "list", "--json"], "each": ["projects", ""], "id": ["id"],
        "title": ["name", "title"], "path": ["sources.0.path", "path", "root", "dir", "directory"] },
    "start": { "run": ["thread", "spawn", "--json", "--project", "{place}", "--prompt-file", "-"], "stdin": "task",
        "id": ["thread.id", "id"], "title": ["thread.title", "title"] },
    "list": { "run": ["thread", "list", "--json"], "each": ["threads", ""], "id": ["id"],
        "title": ["title", "name"], "state": ["status", "state"] },
    "status": { "run": ["thread", "show", "{work}", "--json"], "state": ["thread.status", "status"],
        "line": ["thread.summary", "summary", "thread.last", "last"] },
    "tell": { "run": ["thread", "tell", "{work}", "--mode", "{when}", "--message-file", "-"], "stdin": "text",
        "when": { "now": "steer", "after": "queue" } },
    "stop": { "run": ["thread", "stop", "{work}"] },
    "result": { "run": ["thread", "output", "{work}", "--json"], "line": ["output", "text", "summary"] },
    "open": { "run": ["thread", "open", "{work}"] },
    "states": { "active": "working", "running": "working", "idle": "ready", "done": "ready", "error": "failed",
        "failed": "failed", "pending": "queued", "queued": "queued", "stopped": "stopped",
        "waiting": "waits_person" }
}"#;

const T3: &str = r#"{
    "contract": 1,
    "plug": "command",
    "names": { "ru": ["T3 Code", "т3", "т3 код", "тэ три"], "en": ["T3 Code", "T3"] },
    "ready": { "run": ["thread", "list", "--json"], "fine": ["No T3 Code project matches"] },
    "start": { "run": ["thread", "start", "--json", "--provider", "claudeAgent", "--model", "{model}", "{place}", "-"],
        "stdin": "task",
        "id": ["id", "thread.id", "threadId"], "title": ["title", "thread.title"] },
    "list": { "run": ["thread", "list", "--json"], "each": ["threads", ""], "id": ["id", "threadId"],
        "title": ["title"], "state": ["state", "status"] },
    "status": { "run": ["thread", "show", "{work}", "--json"],
        "state": ["state", "status", "thread.state", "thread.status"],
        "line": ["latestMessage", "lastMessage", "messages.-1.text"] },
    "tell": { "run": ["thread", "send", "--json", "{work}", "-"], "stdin": "text", "when": { "after": "" } },
    "stop": { "via": "mcp", "server": "T3 Code", "tool": "t3_thread_interrupt", "args": { "threadId": "{work}" } },
    "states": { "running": "working", "busy": "working", "working": "working", "idle": "ready",
        "completed": "ready", "done": "ready", "waiting": "waits_person", "error": "failed", "failed": "failed" }
}"#;

/// The cards that come with DuoWave.
pub fn bundled() -> Vec<Bundled> {
    let card = |json: &str| serde_json::from_str::<SystemCard>(json).expect("a bundled card reads");
    vec![
        Bundled { id: "bb", title: "bb", program: "bb", card: card(BB) },
        Bundled { id: "t3code", title: "T3 Code", program: "t3", card: card(T3) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_cards_read_and_say_what_they_can() {
        let cards = bundled();
        assert_eq!(cards.len(), 2);
        let bb = &cards[0].card;
        assert!(bb.start.is_some() && bb.stop.is_some() && bb.places.is_some());
        assert_eq!(bb.tell.as_ref().unwrap().when.get("now").map(String::as_str), Some("steer"));
        let t3 = &cards[1].card;
        assert_eq!(t3.stop.as_ref().and_then(|stop| stop.via.as_deref()), Some("mcp"), "T3 Code stops through its MCP");
        assert!(t3.tell.as_ref().unwrap().when.contains_key("after"));
    }
}
