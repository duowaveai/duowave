//! DuoWave's plugin for Claude Code (docs/plans/work-systems.md, stage 3, D6): http hooks that post a session's
//! events to this gateway — a prompt, a question of rights, the end of a turn, a failure, the session's end. The
//! person installs and removes it with the commands the panel shows; DuoWave never edits Claude Code's settings itself.
//! The plugin sits in a local marketplace (Claude Code installs a plugin from a marketplace only), in DuoWave's data
//! folder. The hooks carry this system's key; without DuoWave running they fail quietly (an http hook that cannot
//! reach its address is no decision), and Claude Code works as it always did.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

/// The secret that a hook shows: this system's key for events in.
pub const KEY_SECRET: &str = "work-system:claude-code:hooks";
/// The marketplace's and the plugin's names, as Claude Code's commands take them.
pub const MARKETPLACE: &str = "duowave-local";
pub const PLUGIN: &str = "duowave";
/// The hooks' address on this gateway.
pub const HOOKS_PATH: &str = "/api/v1/work-systems/claude-code/hooks";

/// The plugin as written, and the commands for a terminal that install and remove it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub dir: String,
    pub install: Vec<String>,
    pub remove: Vec<String>,
}

/// One hook: a POST of the event to this gateway with the key.
fn hook(url: &str, key: &str, timeout: u64) -> Value {
    json!([{ "hooks": [{
        "type": "http",
        "url": url,
        "headers": { "Authorization": format!("Bearer {key}") },
        "timeout": timeout,
    }]}])
}

/// The plugin's files under `dir`: the marketplace, the plugin's manifest, its hooks. Written again each time (a new
/// key or port goes in); the commands to install and remove it.
pub fn write(dir: &Path, port: u16, key: &str) -> std::io::Result<PluginView> {
    let url = format!("http://127.0.0.1:{port}{HOOKS_PATH}");
    let plugin = dir.join(PLUGIN);
    std::fs::create_dir_all(dir.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join("hooks"))?;
    let marketplace = json!({
        "name": MARKETPLACE,
        "owner": { "name": "DuoWave" },
        "plugins": [{
            "name": PLUGIN,
            "source": format!("./{PLUGIN}"),
            "description": "Tells DuoWave what your Claude Code sessions do, so you can ask by voice how they go.",
        }],
    });
    let manifest = json!({
        "name": PLUGIN,
        "version": "1.0.0",
        "description": "DuoWave hears your Claude Code sessions: a prompt, a question of rights, the end of a turn.",
    });
    // A question of rights waits for the person's answer (works/plugs/claude.rs, ASK_WAIT) a little less than this.
    let ask = super::claude::ASK_WAIT.as_secs() + 30;
    let hooks = json!({ "hooks": {
        "SessionStart": hook(&url, key, 5),
        "UserPromptSubmit": hook(&url, key, 5),
        "PermissionRequest": hook(&url, key, ask),
        "Stop": hook(&url, key, 10),
        "StopFailure": hook(&url, key, 5),
        "SessionEnd": hook(&url, key, 5),
    }});
    let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default() + "\n";
    std::fs::write(dir.join(".claude-plugin/marketplace.json"), pretty(&marketplace))?;
    std::fs::write(plugin.join(".claude-plugin/plugin.json"), pretty(&manifest))?;
    std::fs::write(plugin.join("hooks/hooks.json"), pretty(&hooks))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The key is in it: the person's own.
        let _ = std::fs::set_permissions(plugin.join("hooks/hooks.json"), std::fs::Permissions::from_mode(0o600));
    }
    let shown = dir.display().to_string();
    let quoted = crate::works::harness::claude_code::quoted(&shown);
    Ok(PluginView {
        dir: shown,
        install: vec![
            format!("claude plugin marketplace add {quoted}"),
            format!("claude plugin install {PLUGIN}@{MARKETPLACE}"),
        ],
        remove: vec![
            format!("claude plugin uninstall {PLUGIN}@{MARKETPLACE}"),
            format!("claude plugin marketplace remove {MARKETPLACE}"),
        ],
    })
}

/// Where the plugin lives: beside the works' folder, in DuoWave's data.
pub fn dir_of(works_dir: &Path) -> PathBuf {
    works_dir.with_file_name("claude-plugin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_posts_its_events_here_with_the_key_and_says_how_to_install_it() {
        let dir = tempfile::tempdir().unwrap();
        let view = write(dir.path(), 7780, "k-123").unwrap();
        let read = |path: &str| -> Value {
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(path)).unwrap()).unwrap()
        };
        let marketplace = read(".claude-plugin/marketplace.json");
        assert_eq!(marketplace["name"], MARKETPLACE);
        assert_eq!(marketplace["plugins"][0]["source"], "./duowave");
        assert_eq!(read("duowave/.claude-plugin/plugin.json")["name"], PLUGIN);
        let hooks = read("duowave/hooks/hooks.json");
        let stop = &hooks["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["type"], "http");
        assert_eq!(stop["url"], "http://127.0.0.1:7780/api/v1/work-systems/claude-code/hooks");
        assert_eq!(stop["headers"]["Authorization"], "Bearer k-123");
        assert!(hooks["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"].as_u64().unwrap() > 120);
        assert_eq!(view.install[1], "claude plugin install duowave@duowave-local");
        assert!(view.install[0].starts_with("claude plugin marketplace add "));
        assert_eq!(view.remove.len(), 2);
    }
}
