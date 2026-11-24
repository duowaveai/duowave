//! `duowave panel …`: the panel's first password from a terminal — on the station, or on a computer.
//!
//!   duowave panel setup [--port N] [--lang en]
//!                         the name (empty — admin) and the password from stdin, a line each; at a terminal both are
//!                         asked, the password with no echo, twice. They go to this device's own gateway — the
//!                         station's installed one, else this computer's PORT — over loopback, where no first-run code
//!                         is asked, and nowhere else: never into the command line, the environment or a log.
//!                         Exit 0: the admin is made; 3: the panel has its password already; 2: nothing to read, or
//!                         the password will not do; 1: the gateway does not answer or refused.
//!
//! The one-line install (src/placement/setup.rs) runs it on the station over SSH at its end, the password in the
//! channel's stdin (docs/plans/home-setup-flow.md, stage 1).

use std::io::{BufRead as _, IsTerminal as _, Read as _, Write as _};
use std::time::Duration;

use serde_json::{json, Value};

use super::passwords::password_problem;

const USAGE: &str = "duowave panel setup [--port N] [--lang en] — see `duowave panel help`";

/// The panel has its password already: `duowave station setup` tells it from a failure by this exit code.
pub const ALREADY_SET_UP: i32 = 3;

/// Where this device's own gateway listens, as the program tells it (main.rs reads the station's files; access stays
/// below the floors that know them): on a station, its installed gateway's port.
#[derive(Debug, Clone, Copy, Default)]
pub struct Here {
    pub station_port: Option<u16>,
}

/// The most of stdin read for a name and a password.
const INPUT_LIMIT: u64 = 16 * 1024;
/// How long the gateway has to say whether the panel has its password.
const STATE_WAIT: Duration = Duration::from_secs(3);
/// How long it has to make the admin: a password hash, slower on the station (a second or so). With the looks before
/// and after it, under the 20 s a command over SSH has (src/stations/connect/station_shell.rs).
const SETUP_WAIT: Duration = Duration::from_secs(8);

pub fn command(args: &[String], here: Here) -> i32 {
    match args.first().map(String::as_str) {
        Some("setup") => match options(&args[1..]) {
            Ok(options) => setup(options, here),
            Err(problem) => usage(&problem),
        },
        Some("help" | "--help") | None => {
            println!("{}", help());
            0
        }
        Some(other) => usage(&format!("unknown: {other}")),
    }
}

/// What `setup` is told on its command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    port: Option<u16>,
    /// The language of its words (and of the gateway's): `duowave station setup` passes its terminal's on.
    language: Option<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let port = args.next().and_then(|port| port.parse::<u16>().ok()).filter(|port| *port > 0);
                options.port = Some(port.ok_or("--port: a number")?);
            }
            "--lang" => {
                let language = args.next().and_then(|code| crate::locale::normalize(code));
                options.language = Some(language.ok_or("--lang: a language code, such as en")?);
            }
            other => return Err(format!("unknown: {other}")),
        }
    }
    Ok(options)
}

fn setup(options: Options, here: Here) -> i32 {
    if options.language.is_some() {
        crate::locale::text::remember_household(options.language);
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let on_station = here.station_port.is_some();
    let port = match (options.port, here.station_port) {
        (Some(port), _) | (None, Some(port)) => port,
        (None, None) => match crate::config::Config::from_process() {
            Ok(config) => config.app.port,
            Err(error) => return said(&error.to_string(), 1),
        },
    };
    let silent = || {
        let program = std::env::args().next().unwrap_or_else(|| "duowave".into());
        if on_station {
            crate::tr!(
                "DuoWave на колонке не отвечает на порту {port}. Что с ним — {program} station status и {program} station log.",
                port = port,
                program = program
            )
        } else {
            crate::tr!(
                "DuoWave на этом компьютере не отвечает на порту {port}: запустите его — или назовите его порт: --port.",
                port = port
            )
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return said(&error.to_string(), 1),
    };
    // Before anything is asked: a panel with its password takes no other, and a gateway that does not answer none.
    match runtime.block_on(configured(port)) {
        None => return said(&silent(), 1),
        Some(true) => return said(&already(), ALREADY_SET_UP),
        Some(false) => {}
    }
    let (name, password) = match read_input() {
        Ok(read) => read,
        Err(problem) => return said(&problem, 2),
    };
    if let Some(problem) = password_problem(Some(&Value::String(password.clone()))) {
        return said(&problem, 2);
    }
    match runtime.block_on(set_up(port, &name, &password)) {
        Outcome::Made(name) => {
            println!("{}", crate::tr!("Пароль пульта задан: администратор «{name}» входит с ним.", name = name));
            0
        }
        Outcome::AlreadySetUp => said(&already(), ALREADY_SET_UP),
        Outcome::NoAnswer => said(&silent(), 1),
        Outcome::Refused(why) => said(&why, 1),
    }
}

fn already() -> String {
    crate::tr!("Пульт уже настроен — войдите с его паролем.")
}

/// A word to stderr, and the exit code it ends with.
fn said(words: &str, code: i32) -> i32 {
    eprintln!("{words}");
    code
}

/// What the gateway answered the first password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The admin is made, by this name.
    Made(String),
    /// Somebody exists: the panel has its password already.
    AlreadySetUp,
    /// Nothing answers on the port.
    NoAnswer,
    /// The gateway refused, in its words.
    Refused(String),
}

/// The first password, given to this device's own gateway on `port`: from loopback, so it asks no first-run code.
/// A name shorter than two letters, or none, is the gateway's «admin».
pub async fn set_up(port: u16, name: &str, password: &str) -> Outcome {
    let Some(client) = client(SETUP_WAIT) else { return Outcome::NoAnswer };
    let mut body = json!({ "password": password });
    if !name.is_empty() {
        body["name"] = Value::String(name.to_string());
    }
    let mut request = client.post(url(port, "/api/v1/auth/setup")).json(&body);
    if let Some(language) = crate::locale::text::household_now() {
        request = request.header(reqwest::header::ACCEPT_LANGUAGE, language);
    }
    let Ok(response) = request.send().await else {
        // No answer in time may still be a hash done late: a panel that has its password now has it.
        return if configured(port).await == Some(true) { Outcome::AlreadySetUp } else { Outcome::NoAnswer };
    };
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        let made = answer.pointer("/message/user/name").and_then(Value::as_str).unwrap_or("admin");
        return Outcome::Made(made.to_string());
    }
    // Somebody came first (the panel itself, in a browser): the password they set stands.
    if configured(port).await == Some(true) {
        return Outcome::AlreadySetUp;
    }
    Outcome::Refused(
        answer.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| status.to_string()),
    )
}

/// Whether the panel has its password, by the gateway's own word (`GET /api/v1/auth/state`); None — no answer.
pub async fn configured(port: u16) -> Option<bool> {
    let client = client(STATE_WAIT)?;
    let body: Value = client.get(url(port, "/api/v1/auth/state")).send().await.ok()?.json().await.ok()?;
    body.pointer("/message/configured").and_then(Value::as_bool)
}

/// A client for this device's own gateway: straight to loopback, never through a proxy — a proxy would see the
/// password, and the gateway takes what a proxy hands on for the home network's, which needs the code.
fn client(timeout: Duration) -> Option<reqwest::Client> {
    crate::utils::http::client_builder().no_proxy().timeout(timeout).build().ok()
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

/// What `setup` reads from a stdin that is not a terminal: the name (empty — admin), then the password, a line each.
pub fn input(name: &str, password: &str) -> String {
    format!("{name}\n{password}\n")
}

/// Two lines: the name, then the password as typed — only the line's end is cut from it.
fn parse_input(text: &str) -> Result<(String, String), String> {
    let mut lines = text.lines();
    let (Some(name), Some(password)) = (lines.next(), lines.next()) else {
        return Err(crate::tr!("Нужны две строки: имя (пусто — admin), затем пароль."));
    };
    Ok((name.trim().to_string(), password.trim_end_matches('\r').to_string()))
}

/// The name and the password: asked at a terminal — the password with no echo, twice —, else two lines of stdin, as
/// `duowave station setup` sends them over SSH.
fn read_input() -> Result<(String, String), String> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut text = String::new();
        stdin.lock().take(INPUT_LIMIT).read_to_string(&mut text).map_err(|error| error.to_string())?;
        return parse_input(&text);
    }
    eprint!("{}", crate::tr!("Имя администратора (пусто — admin): "));
    let _ = std::io::stderr().flush();
    let mut name = String::new();
    stdin.lock().read_line(&mut name).map_err(|error| error.to_string())?;
    let password = hidden(&crate::tr!("Пароль пульта (символы не видны): ")).map_err(|error| error.to_string())?;
    if hidden(&crate::tr!("Ещё раз: ")).map_err(|error| error.to_string())? != password {
        return Err(crate::tr!("Пароли не совпали."));
    }
    Ok((name.trim().to_string(), password))
}

/// What reads a line typed with no echo on a computer (crates/duowave-pc: rpassword — /dev/tty, the Windows console),
/// said at its start; the station has none.
pub type HiddenLine = fn(&str) -> std::io::Result<String>;

static HIDDEN: std::sync::OnceLock<HiddenLine> = std::sync::OnceLock::new();

/// A computer's reader of a line typed with no echo (`HiddenLine`).
pub fn set_hidden(read: HiddenLine) {
    let _ = HIDDEN.set(read);
}

/// A line typed with no echo: the computer's reader, else — the station — busybox's stty.
fn hidden(question: &str) -> std::io::Result<String> {
    match HIDDEN.get() {
        Some(read) => read(question),
        None => hidden_by_stty(question),
    }
}

/// A line typed with no echo on the station: busybox's stty turns the terminal's echo off for the line (without stty
/// it shows as it is typed).
fn hidden_by_stty(question: &str) -> std::io::Result<String> {
    use std::process::Command;
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let quiet = Command::new("stty").arg("-echo").status().is_ok_and(|status| status.success());
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if quiet {
        let _ = Command::new("stty").arg("echo").status();
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// The words at the top of this file, for `duowave panel help`.
fn help() -> String {
    include_str!("cli.rs")
        .lines()
        .skip(1)
        .take_while(|line| line.starts_with("//!"))
        .map(|line| line.trim_start_matches("//!").trim_start_matches(' '))
        .collect::<Vec<_>>()
        .join("\n")
}

fn usage(problem: &str) -> i32 {
    eprintln!("duowave panel: {problem}\n{USAGE}");
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_name_and_the_password_a_line_each_as_typed() {
        assert_eq!(parse_input(&input("", "pass word 1")), Ok((String::new(), "pass word 1".into())));
        assert_eq!(parse_input("  Мама \r\n  secret-1 \r\n"), Ok(("Мама".into(), "  secret-1 ".into())));
        assert_eq!(parse_input("\nno-newline-at-the-end"), Ok((String::new(), "no-newline-at-the-end".into())));
        for one_line in ["", "\n", "only-a-password\n"] {
            assert!(parse_input(one_line).is_err(), "{one_line:?}");
        }
    }

    #[test]
    fn takes_a_port_and_a_language_and_nothing_else() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(options(&[]), Ok(Options::default()));
        assert_eq!(
            options(&args(&["--port", "8791", "--lang", "en-US"])),
            Ok(Options { port: Some(8791), language: Some("en".into()) })
        );
        assert!(options(&args(&["--port", "x"])).is_err());
        assert!(options(&args(&["--port"])).is_err());
        assert!(options(&args(&["--lang", "en; reboot"])).is_err());
        assert!(options(&args(&["admin-password-1"])).is_err(), "never a password in the command line");
    }

    #[test]
    fn the_help_is_the_words_at_the_top() {
        let help = help();
        assert!(help.trim_start().starts_with("duowave panel setup [--port N] [--lang en]"), "{help}");
        assert!(help.contains("never into the command line"));
    }
}
