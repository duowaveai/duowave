//! The gateway's file tools of an app's agent (`list_files`, `read_file`, `write_file`): its own folder only. The
//! agent's work in the gateway (links/plugins.rs) and Claude Code through the app's MCP (`/mcp/apps/<id>`, routes.rs)
//! use the same.

use serde_json::{json, Value};

use super::files::PluginFiles;

/// What one file tool reads and writes at most.
pub const AGENT_READ_MAX: usize = 64 * 1024;
pub const AGENT_WRITE_MAX: usize = 256 * 1024;

/// The gateway's file tools of an app's agent: its own folder only.
pub fn agent_file_tools() -> Vec<crate::dialog::llm::LlmTool> {
    let tool = |name: &str, description: &str, parameters: Value| crate::dialog::llm::LlmTool {
        name: name.into(),
        description: description.into(),
        parameters,
    };
    vec![
        tool(
            "list_files",
            "Файлы и папки в папке приложения (path — подпапка, пусто — сама папка).",
            json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        ),
        tool(
            "read_file",
            "Текст файла папки приложения, до 64 КБ.",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        ),
        tool(
            "write_file",
            "Записать текстовый файл в папку приложения целиком (до 256 КБ); папки на пути создаются.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "text": { "type": "string" } },
                "required": ["path", "text"]
            }),
        ),
    ]
}

pub fn agent_list(files: &PluginFiles, app: &str, path: &str) -> Result<String, String> {
    let dir = if path.trim_matches('/').is_empty() { files.root().join(app) } else { files.inside(app, path)? };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|_| format!("нет папки {path}"))?
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => format!("{name}/"),
                Ok(meta) => format!("{name} ({} Б)", meta.len()),
                Err(_) => name,
            }
        })
        .collect();
    names.sort();
    Ok(if names.is_empty() { "пусто".into() } else { names.join("\n") })
}

pub fn agent_read(files: &PluginFiles, app: &str, path: &str) -> Result<String, String> {
    let file = files.inside(app, path)?;
    let bytes = std::fs::read(&file).map_err(|_| format!("нет файла {path}"))?;
    if bytes.len() > AGENT_READ_MAX {
        return Err(format!("{path}: больше 64 КБ"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{path}: не текст"))
}

pub fn agent_write(files: &PluginFiles, app: &str, path: &str, text: &str) -> Result<String, String> {
    if text.len() > AGENT_WRITE_MAX {
        return Err(format!("{path}: больше 256 КБ"));
    }
    let file = files.place(app, path)?;
    std::fs::write(&file, text).map_err(|error| format!("{path}: {error}"))?;
    Ok(format!("записано: {path}, {} Б", text.len()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn the_agents_file_tools_stay_inside_the_apps_folder() {
        let dir = tempfile::tempdir().unwrap();
        let files = crate::plugins::files::PluginFiles::new(dir.path().to_path_buf(), Arc::new(|| false));
        assert!(agent_write(&files, "feed", "episodes/01/plan.json", "{\"shots\": 3}")
            .unwrap()
            .starts_with("записано"));
        assert_eq!(agent_read(&files, "feed", "episodes/01/plan.json").unwrap(), "{\"shots\": 3}");
        assert_eq!(agent_list(&files, "feed", "").unwrap(), "episodes/");
        assert!(agent_list(&files, "feed", "episodes/01").unwrap().starts_with("plan.json ("));
        for bad in ["../other/secret.txt", ".hidden", "episodes/../../x", ""] {
            assert!(agent_write(&files, "feed", bad, "x").is_err(), "{bad}");
            assert!(agent_read(&files, "feed", bad).is_err(), "{bad}");
        }
        assert!(!dir.path().join("other").exists());
        assert!(agent_write(&files, "feed", "big.txt", &"x".repeat(AGENT_WRITE_MAX + 1)).is_err());
    }
}
