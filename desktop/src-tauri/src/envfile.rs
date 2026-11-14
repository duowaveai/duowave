//! `<data>/.env`: the engine reads it itself (engine.rs); the app only writes the first one.

use std::fs;
use std::io;
use std::path::Path;

/// The first .env's comments, in the system's language (the gateway has not said the home's yet).
fn first_env() -> String {
    let comments = crate::lang::t(
        "\
# Настройки шлюза. Шлюз читает этот файл при запуске: после правки — «Перезапустить шлюз» в меню значка.
#
# Порт шлюза: пульт — http://<адрес компьютера>:<порт>/panel, колонка ходит на ws://<адрес компьютера>:<порт>/uni.ws.
# Порт может быть любым свободным. Сменили его — колонку, уже подключённую к шлюзу, переподключите (пульт предложит).
",
        "\
# Gateway settings. The gateway reads this file at startup: after editing, choose “Restart gateway” in the tray menu.
#
# Gateway port: the panel is http://<computer address>:<port>/panel, the speaker connects to ws://<computer address>:<port>/uni.ws.
# Any free port will do. If you change it, reconnect a speaker already connected to the gateway (the panel will offer to).
",
    );
    format!("{comments}PORT=7780\n")
}

/// Writes the first .env when there is none; an existing one is the household's and stays as it is.
pub fn ensure(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, first_env())?;
    Ok(true)
}

/// Sets PORT in .env: the line replaced where it is (the household's other lines and comments stay), added at the end
/// when there is none.
pub fn set_port(path: &Path, port: u16) -> io::Result<()> {
    let text = fs::read_to_string(path).or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Ok(String::new())
        } else {
            Err(error)
        }
    })?;
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let setting = line.trim_start().strip_prefix("export ").unwrap_or(line.trim_start());
            if !found && setting.split('=').next().map(str::trim) == Some("PORT") && setting.contains('=') {
                found = true;
                format!("PORT={port}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("PORT={port}"));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PORT as the engine reads it from the file.
    fn port_in(path: &Path) -> Option<String> {
        let env = duowave::config::Env::with_files(Vec::<(String, String)>::new(), &[path.to_path_buf()]);
        env.get("PORT").map(str::to_string)
    }

    #[test]
    fn the_first_env_is_written_once_and_says_port_7780() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join(".env");
        assert!(ensure(&path).unwrap());
        assert_eq!(port_in(&path).as_deref(), Some("7780"));
        fs::write(&path, "PORT=8080\n").unwrap();
        assert!(!ensure(&path).unwrap());
        assert_eq!(port_in(&path).as_deref(), Some("8080"));
    }

    #[test]
    fn a_new_port_replaces_the_line_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        fs::write(&path, "# мой комментарий\nPORT=80\nOPENAI_API_KEY=abc\n").unwrap();
        set_port(&path, 7781).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# мой комментарий\nPORT=7781\nOPENAI_API_KEY=abc\n"
        );
        assert_eq!(port_in(&path).as_deref(), Some("7781"));
        fs::write(&path, "OPENAI_API_KEY=abc").unwrap();
        set_port(&path, 7782).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "OPENAI_API_KEY=abc\nPORT=7782\n");
    }
}
