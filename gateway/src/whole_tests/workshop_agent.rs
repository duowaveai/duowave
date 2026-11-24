//! The tests of `plugins/workshop/agent.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use serde_json::json;

use crate::dialog::llm::ToolCallCut;
use crate::plugins::workshop::agent::*;

#[test]
fn gives_the_agent_its_skill_and_every_tool_a_draft_or_none() {
    assert!(skill_text().starts_with("# Мастерская"), "the front matter is left out");
    assert!(AUTHORING.contains("## Host API 2") && STATION.contains("Проверено 07.11.2026"));
    let bound = tools(false);
    assert!(bound
        .iter()
        .all(|tool| tool.parameters["required"].as_array().is_none_or(|required| !required.contains(&json!("draft")))));
    let mcp = tools(true);
    assert!(mcp.iter().any(|tool| tool.name == "new_draft"));
    let check = mcp.iter().find(|tool| tool.name == "check").unwrap();
    assert_eq!(check.parameters["required"], json!(["draft"]));
}

#[test]
fn every_example_the_list_names_gives_its_files() {
    let list = examples_doc("");
    for (name, _) in EXAMPLES {
        assert!(list.contains(&format!("`{name}`")), "{name} in the list");
        let doc = examples_doc(name);
        assert!(doc.contains("manifest.json"), "{name}: {}", &doc[..doc.len().min(200)]);
    }
    assert!(examples_doc("weather").contains("## backend.js"), "a built-in's own code");
}

#[tokio::test]
async fn edits_a_piece_of_a_big_file_and_says_what_is_wrong_with_a_piece_not_there_or_twice() {
    use crate::config::{Config, Env};
    let dir = tempfile::tempdir().unwrap();
    let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("DIALOG_PROVIDER", "echo".into()),
        ("PACKAGES_DIR", text("packages")),
        ("DATA_DIR", text("data")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
    ]);
    let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
    let workshop = &app.workshop;
    workshop.create("uchitel", Some("midi"), None, None).unwrap();
    let input =
        |old: &str, new: &str| json!({ "path": "panel.js", "old": old, "new": new }).as_object().cloned().unwrap();
    let (said, failed) = run_tool(
        workshop,
        "uchitel",
        "edit_file",
        &input("const SIZES = [25, 32, 37, 49, 61, 76, 88];", "const SIZES = [25, 61];"),
    )
    .await;
    assert!(!failed && said.starts_with("Поправлен panel.js"), "{said}");
    assert!(workshop.read("uchitel", "panel.js").unwrap().contains("const SIZES = [25, 61];"));
    let (said, failed) = run_tool(workshop, "uchitel", "edit_file", &input("нет такого куска", "x")).await;
    assert!(failed && said.contains("нет такого куска"), "{said}");
    let (said, failed) = run_tool(workshop, "uchitel", "edit_file", &input("h(", "h (")).await;
    assert!(failed && said.contains("раз"), "{said}");
    assert!(tools(true).iter().any(|tool| tool.name == "edit_file"));

    // A big file in parts: the first piece makes it, the next go on a new line.
    let piece = |path: &str, content: &str| json!({ "path": path, "content": content }).as_object().cloned().unwrap();
    let (said, failed) = run_tool(workshop, "uchitel", "append_file", &piece("assets/notes.md", "# Заметки")).await;
    assert!(!failed && said.starts_with("Дописан assets/notes.md"), "{said}");
    run_tool(workshop, "uchitel", "append_file", &piece("assets/notes.md", "- первая\n")).await;
    run_tool(workshop, "uchitel", "append_file", &piece("assets/notes.md", "- вторая\n")).await;
    assert_eq!(workshop.read("uchitel", "assets/notes.md").unwrap(), "# Заметки\n- первая\n- вторая\n");
    let (said, failed) = run_tool(workshop, "uchitel", "append_file", &piece("assets/notes.md", "")).await;
    assert!(failed && said.contains("пустой"), "{said}");
    let (said, failed) = run_tool(workshop, "uchitel", "append_file", &piece("../escape.js", "x")).await;
    assert!(failed, "only the draft's own files: {said}");
    assert!(tools(false).iter().any(|tool| tool.name == "append_file"));
}

#[test]
fn the_agent_hears_which_file_did_not_fit_and_a_round_may_write_a_whole_answer_slowly() {
    let note = cut_note(&ToolCallCut::new("Claude", "write_file", r#"{"path": "panel.js", "content": "exp"#));
    assert!(note.contains("write_file (panel.js)") && note.contains("append_file"), "{note}");
    assert_eq!(round_ms(4096), ROUND_MS, "ten minutes at the least");
    assert_eq!(round_ms(32_000), 32_000 * MS_PER_TOKEN);
}

/// The agent's tools speak English to the model, as the brain's own do (skills/tools.rs).
#[test]
fn its_tools_speak_english() {
    for tool in tools(true) {
        let found = crate::skills::tools::not_in_english(
            &serde_json::json!({"description": tool.description, "parameters": tool.parameters}),
        );
        assert!(found.is_empty(), "{}: {found:?}", tool.name);
    }
}
