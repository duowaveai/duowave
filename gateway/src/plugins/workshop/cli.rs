//! `duowave plugin …`: the workshop without the panel (docs/plans/plugins.md, stage 6) — for a person or Claude Code in
//! a terminal, no gateway needed.
//!
//!   duowave plugin new <id> [--theme | --quiz | --midi] [--title «…»] [--from-openapi <api.json>] [--dir <folder>]
//!       a draft folder from the template (a plugin, or a theme), or from an API's OpenAPI description in JSON
//!   duowave plugin test <folder> [--phrase «…»]… [--run <function> [--input '<json>']] [--example]
//!       the workshop's check in the sandbox; phrases as the router hears them; a function with its input
//!   duowave plugin install <folder> [--packages-dir <dir>]
//!       the package into PACKAGES_DIR as a new version (the next patch when it is taken). It stays off until the
//!       admin switches it on in the panel: the first install is the admin's to approve.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use super::{openapi, sandbox, templates};
use crate::plugins::manifest;

const USAGE: &str =
    "duowave plugin new <id> [--theme | --quiz | --midi] [--title «…»] [--from-openapi <api.json>] [--dir <folder>]
duowave plugin test <folder> [--phrase «…»]… [--run <function> [--input '<json>']] [--example]
duowave plugin install <folder> [--packages-dir <dir>]";

/// The value after a flag.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|arg| arg == name).and_then(|at| args.get(at + 1)).map(String::as_str)
}

fn flags<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2).filter(|pair| pair[0] == name).map(|pair| pair[1].as_str()).collect()
}

/// Runs a `duowave plugin` command; its exit code.
pub fn command(args: &[String]) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("new") => new(&args[1..]),
        Some("test") => test(&args[1..]),
        Some("install") => install(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => 0,
        Err(problem) => {
            eprintln!("duowave plugin: {problem}");
            1
        }
    }
}

fn new(args: &[String]) -> Result<(), String> {
    let id = args.first().filter(|id| !id.starts_with("--")).ok_or(USAGE)?;
    if !manifest::valid_id(id) {
        return Err(format!("«{id}»: строчные латинские буквы, цифры и «-», с буквы, до 32 знаков"));
    }
    let title = flag(args, "--title").unwrap_or(id);
    let folder = PathBuf::from(flag(args, "--dir").unwrap_or(".")).join(id);
    if folder.exists() {
        return Err(format!("{} уже есть", folder.display()));
    }
    let files = match flag(args, "--from-openapi") {
        Some(spec) => {
            let text = std::fs::read_to_string(spec).map_err(|error| format!("{spec}: {error}"))?;
            openapi::draft_from(id, flag(args, "--title"), &text)?
        }
        None if args.iter().any(|arg| arg == "--theme") => templates::theme(id, title),
        None if args.iter().any(|arg| arg == "--quiz") => templates::quiz(id, title),
        None if args.iter().any(|arg| arg == "--midi") => templates::midi(id, title),
        None if args.iter().any(|arg| arg == "--folders") => templates::folders(id, title),
        None => templates::plugin(id, title),
    };
    for (path, text) in files {
        let file = folder.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&file, text).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    println!("{}: черновик готов. Проверить: duowave plugin test {}", folder.display(), folder.display());
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
}

fn id_of(folder: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(folder.join("manifest.json"))
        .map_err(|_| format!("{}: нет manifest.json", folder.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|error| format!("manifest.json: {error}"))?;
    value.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "manifest.json: нет id".into())
}

fn test(args: &[String]) -> Result<(), String> {
    let folder = PathBuf::from(args.first().filter(|arg| !arg.starts_with("--")).ok_or(USAGE)?);
    let id = id_of(&folder)?;
    let runtime = runtime()?;
    runtime.block_on(async {
        let handle = Some(tokio::runtime::Handle::current());
        let report = sandbox::check(&folder, &id, handle.clone()).await;
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        let kv = Arc::new(Mutex::new(Map::new()));
        // --example: the network's answers from examples.json, nothing goes out (as the panel's tile runs it).
        let trying = if args.iter().any(|arg| arg == "--example") {
            sandbox::Trying::example()
        } else {
            sandbox::Trying::default()
        };
        for phrase in flags(args, "--phrase") {
            let answer = sandbox::try_phrase(&folder, &id, phrase, kv.clone(), handle.clone(), &trying).await?;
            println!("«{phrase}» → {}", serde_json::to_string_pretty(&answer).unwrap_or_default());
        }
        if let Some(function) = flag(args, "--run") {
            let input: Value = match flag(args, "--input") {
                Some(text) => serde_json::from_str(text).map_err(|error| format!("--input: {error}"))?,
                None => Value::Object(Map::new()),
            };
            let ran = sandbox::run(&folder, &id, function, input, kv, handle.clone(), &trying).await?;
            println!("{function} → {}", serde_json::to_string_pretty(&ran).unwrap_or_default());
        }
        if report.ok {
            Ok(())
        } else {
            Err("проверка не прошла".to_string())
        }
    })
}

fn install(args: &[String]) -> Result<(), String> {
    let folder = PathBuf::from(args.first().filter(|arg| !arg.starts_with("--")).ok_or(USAGE)?);
    let id = id_of(&folder)?;
    let report = runtime()?.block_on(sandbox::check(&folder, &id, None));
    if !report.ok {
        return Err(format!("проверка не прошла: {}", serde_json::to_string(&report).unwrap_or_default()));
    }
    let packages = match flag(args, "--packages-dir") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let config =
                crate::config::Config::load(&crate::config::Env::from_process()).map_err(|error| error.to_string())?;
            config.plugins.packages_dir
        }
    };
    let text = std::fs::read_to_string(folder.join("manifest.json")).map_err(|error| error.to_string())?;
    let mut value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut version =
        semver::Version::parse(value["version"].as_str().unwrap_or_default()).map_err(|error| error.to_string())?;
    while packages.join(&id).join(version.to_string()).exists() {
        version.patch += 1;
    }
    let target = packages.join(&id).join(version.to_string());
    super::copy_dir(&folder, &target).map_err(|error| format!("{}: {error}", target.display()))?;
    value["version"] = Value::String(version.to_string());
    let pretty = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    std::fs::write(target.join("manifest.json"), format!("{pretty}\n")).map_err(|error| error.to_string())?;
    println!(
        "{id} {version} → {}. Пакет выключен: включает администратор в пульте («Приложения»), новая версия работающего — кнопкой «Перезагрузить».",
        target.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_tests_and_installs_a_draft_from_the_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let at = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(command(&args(&["new", "greeter", "--dir", &at("drafts")])), 0);
        assert_eq!(command(&args(&["new", "greeter", "--dir", &at("drafts")])), 1, "it is there already");
        assert_eq!(command(&args(&["test", &at("drafts/greeter"), "--phrase", "скажи привет Саша"])), 0);
        assert_eq!(command(&args(&["install", &at("drafts/greeter"), "--packages-dir", &at("packages")])), 0);
        assert_eq!(command(&args(&["install", &at("drafts/greeter"), "--packages-dir", &at("packages")])), 0);
        assert!(dir.path().join("packages/greeter/0.1.0/backend.js").is_file());
        let again = std::fs::read_to_string(dir.path().join("packages/greeter/0.1.1/manifest.json")).unwrap();
        assert!(again.contains("\"0.1.1\""));
        assert_eq!(command(&args(&["dance"])), 1);
    }
}
