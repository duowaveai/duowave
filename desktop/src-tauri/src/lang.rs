// The app's own words (tray, splash texts, notifications) in the language of the home: Russian or English.
// The home's language is the gateway's (`GET /api/v1/language`) once it answers; until then the system's.
// No language, or a language without a translation, is English.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

const UNSET: u8 = 0;
const RU: u8 = 1;
const EN: u8 = 2;

/// The gateway's answer: UNSET until it gave one.
static HOME: AtomicU8 = AtomicU8::new(UNSET);
static SYSTEM: OnceLock<u8> = OnceLock::new();

fn code_of(language: &str) -> u8 {
    if language.trim().to_lowercase().starts_with("ru") {
        RU
    } else {
        EN
    }
}

fn system() -> u8 {
    *SYSTEM.get_or_init(|| {
        let from_env = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty() && value != "C" && value != "POSIX");
        #[cfg(target_os = "macos")]
        {
            // The first of the preferred languages ("ru-RU", "en-US"); a program started from the Dock has no LANG.
            let preferred = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleLanguages"])
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
                .and_then(|text| {
                    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                        .find(|word| !word.is_empty())
                        .map(str::to_owned)
                });
            if let Some(language) = preferred.or(from_env) {
                return code_of(&language);
            }
            EN
        }
        #[cfg(not(target_os = "macos"))]
        {
            from_env.map(|language| code_of(&language)).unwrap_or(EN)
        }
    })
}

/// Tests read the Russian texts.
fn current() -> u8 {
    if cfg!(test) {
        return RU;
    }
    match HOME.load(Ordering::Relaxed) {
        UNSET => system(),
        known => known,
    }
}

/// The text in the current language: `t("по-русски", "in English")`.
pub fn t<'a>(ru: &'a str, en: &'a str) -> &'a str {
    if current() == RU {
        ru
    } else {
        en
    }
}

/// Asks the gateway for the home's language; true when it changed the language the app speaks.
pub fn refresh_home(port: u16) -> bool {
    let before = current();
    let Some(language) = ask_gateway(port) else {
        return false;
    };
    HOME.store(language.map_or(EN, |code| code_of(&code)), Ordering::Relaxed);
    current() != before
}

/// Some(Some("ru")) the home has a language, Some(None) the gateway answered without one, None no answer.
fn ask_gateway(port: u16) -> Option<Option<String>> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(800)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
    let request =
        format!("GET /api/v1/language HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.take(1 << 16).read_to_end(&mut response).ok()?;
    let response = String::from_utf8_lossy(&response);
    let (head, body) = response.split_once("\r\n\r\n")?;
    if head.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let language = json.pointer("/message/language")?;
    Some(language.as_str().map(str::to_owned))
}
