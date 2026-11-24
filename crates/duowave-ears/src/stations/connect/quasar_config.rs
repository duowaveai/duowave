//! Where a station sends its voice lives in /system/vendor/quasar/quasar.cfg: aliced.uniProxyUrl and the QUIC address
//! beside it. Connecting a station to the gateway rewrites those two values and nothing else — every other byte of
//! the file, its formatting too, stays — so a copy of the file, or just the two old values, is the way back.
//! The file also holds the station's keys: the gateway reads from it what it needs here and keeps nothing else.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

/// The station's voice server: the WebSocket address and the QUIC one (empty — none). The same shape the gateway
/// keeps in its station links.
pub use crate::settings::definitions::LinkAddresses as VoiceAddresses;

pub const STATION_CONFIG: &str = "/system/vendor/quasar/quasar.cfg";
pub const YANDEX_URL: &str = "wss://uniproxy.alice.yandex.net/uni.ws";
pub const YANDEX_QUIC_URL: &str = "uniproxy-quic.alice.yandex.net:8443/uni.ws";

/// Yandex's own addresses: the station as it was before any gateway.
pub fn yandex() -> VoiceAddresses {
    VoiceAddresses { url: YANDEX_URL.into(), quic: YANDEX_QUIC_URL.into() }
}

/// Where the voice goes now: Yandex, a gateway (ours or another), or somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PointsTo {
    Yandex,
    Gateway,
    Other,
}

/// What the gateway reads from the station's config: the addresses, the model and the firmware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationFacts {
    pub addresses: VoiceAddresses,
    /// common.deviceType: the model, as the firmware names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// common.softwareVersion: the firmware.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,
    pub points_to: PointsTo,
}

/// The config is not the file this code knows, or a value cannot be written into it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

/// A value fit to write into the file and into a shell line as it is: no quote, backslash, `#` or space.
pub fn safe_address(value: &str) -> bool {
    value.len() <= 200 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._~:/?&=%@[]-".contains(&byte))
}

/// The station's config as the gateway needs it. Fails when it is not the file this code knows.
pub fn read_config(text: &str) -> Result<StationFacts, ConfigError> {
    let parsed: Value = serde_json::from_str(text).map_err(|_| ConfigError("quasar.cfg is not JSON".into()))?;
    let aliced = parsed.get("aliced").and_then(Value::as_object);
    let Some(url) = aliced.and_then(|aliced| aliced.get("uniProxyUrl")).and_then(Value::as_str) else {
        return Err(ConfigError("quasar.cfg has no aliced.uniProxyUrl".into()));
    };
    let quic = aliced.and_then(|aliced| aliced.get("uniProxyQuicUrl")).and_then(Value::as_str).unwrap_or("");
    let common = parsed.get("common").and_then(Value::as_object);
    let word = |name: &str| common.and_then(|common| common.get(name)).and_then(Value::as_str).and_then(short_word);
    Ok(StationFacts {
        addresses: VoiceAddresses { url: url.to_string(), quic: quic.to_string() },
        device_type: word("deviceType"),
        software_version: word("softwareVersion"),
        points_to: points_to(url),
    })
}

static YANDEX_HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^wss://[A-Za-z0-9_.-]+\.yandex\.net(:[0-9]+)?/").expect("the pattern compiles"));
static GATEWAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ws://[^/]+/uni\.ws$").expect("the pattern compiles"));

/// Where a voice server address leads: Yandex, a gateway (ws://<host>/uni.ws), or somewhere else.
pub fn points_to(url: &str) -> PointsTo {
    if url == YANDEX_URL || YANDEX_HOST.is_match(url) {
        PointsTo::Yandex
    } else if GATEWAY.is_match(url) {
        PointsTo::Gateway
    } else {
        PointsTo::Other
    }
}

fn short_word(value: &str) -> Option<String> {
    let fits = (1..=60).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_ .:+-".contains(&byte));
    fits.then(|| value.to_string())
}

/// What JavaScript's `\s` matches: the file was written for JSON.parse, so its spaces are JavaScript's.
const JS_SPACE: &str =
    r"[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]";

/// `"name" : "value"`: the head up to the opening quote, the value, the closing quote.
fn key(name: &str) -> Regex {
    Regex::new(&format!(r#"("{name}"{JS_SPACE}*:{JS_SPACE}*")([^"\\]*)(")"#)).expect("the pattern compiles")
}

static URL_KEY: LazyLock<Regex> = LazyLock::new(|| key("uniProxyUrl"));
static QUIC_KEY: LazyLock<Regex> = LazyLock::new(|| key("uniProxyQuicUrl"));

/// The same file with the two addresses replaced in place, every other byte as it was. The URL key must be there
/// exactly once (a file with two would be one this code does not know); a file without the QUIC key keeps none.
pub fn with_addresses(text: &str, next: &VoiceAddresses) -> Result<String, ConfigError> {
    if !safe_address(&next.url) || next.url.is_empty() || !safe_address(&next.quic) {
        return Err(ConfigError("not an address to write".into()));
    }
    let urls = URL_KEY.find_iter(text).count();
    if urls != 1 {
        return Err(ConfigError(format!("quasar.cfg names uniProxyUrl {urls} times")));
    }
    let quics = QUIC_KEY.find_iter(text).count();
    if quics > 1 {
        return Err(ConfigError(format!("quasar.cfg names uniProxyQuicUrl {quics} times")));
    }
    let replace = |text: &str, pattern: &Regex, value: &str| {
        pattern
            .replace_all(text, |captures: &regex::Captures| format!("{}{value}{}", &captures[1], &captures[3]))
            .into_owned()
    };
    let mut result = replace(text, &URL_KEY, &next.url);
    if quics > 0 {
        result = replace(&result, &QUIC_KEY, &next.quic);
    }
    Ok(result)
}

/// The gateway as the station dials it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHost {
    pub host: String,
    pub port: u16,
}

/// host or host:port as a URL names it (uniProxyUrl, a Host header): without a port, 80.
pub fn parse_gateway_host(input: &str) -> Option<GatewayHost> {
    parse_gateway_host_or(input, 80)
}

/// host or host:port as a person types the gateway's address: an IPv4 address or a name, a port 1–65535; without a
/// port, `port` — the gateway's own, not 80: a bare address means this gateway, wherever it listens.
pub fn parse_gateway_host_or(input: &str, default_port: u16) -> Option<GatewayHost> {
    let text = input.trim();
    let (host, port) = match text.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (text, None),
    };
    let host_fits = (1..=253).contains(&host.len())
        && host.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    if !host_fits {
        return None;
    }
    let port = match port {
        None => default_port,
        Some(port) if (1..=5).contains(&port.len()) && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            port.parse::<u32>().ok().filter(|port| (1..=65535).contains(port))? as u16
        }
        Some(_) => return None,
    };
    let host = host.to_ascii_lowercase();
    if host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return None;
    }
    Some(GatewayHost { host, port })
}

/// What the station dials for a gateway at host:port: ws://host/uni.ws, the port only when it is not 80.
pub fn gateway_addresses(gateway: &GatewayHost, path: &str) -> VoiceAddresses {
    let authority =
        if gateway.port == 80 { gateway.host.clone() } else { format!("{}:{}", gateway.host, gateway.port) };
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    // QUIC empty: the station must not reach Yandex past the gateway.
    VoiceAddresses { url: format!("ws://{authority}{path}"), quic: String::new() }
}

/// A station's IPv4 address as a person types it, or as mDNS gives it.
pub fn plain_address(input: &str) -> Option<String> {
    let text = input.trim();
    let text = match text.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("::ffff:") => &text[7..],
        _ => text,
    };
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut numbers = Vec::with_capacity(4);
    for part in parts {
        if !(1..=3).contains(&part.len()) || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number: u16 = part.parse().ok()?;
        if number > 255 {
            return None;
        }
        numbers.push(number.to_string());
    }
    Some(numbers.join("."))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A made-up station config in the firmware's shape: its formatting (" : ", indents) is what must survive.
    pub(crate) fn config() -> String {
        format!(
            r#"{{
    "aliced" : {{
        "apiKey" : "not-a-real-key",
        "uniProxyQuicUrl" : "{YANDEX_QUIC_URL}",
        "uniProxyUrl" : "{YANDEX_URL}",
        "vadEnabled" : true
    }},
    "common" : {{
        "deviceType" : "yandexstation_test",
        "softwareVersion" : "1.2.3.4"
    }}
}}
"#
        )
    }

    fn to(url: &str) -> VoiceAddresses {
        VoiceAddresses { url: url.into(), quic: String::new() }
    }

    #[test]
    fn reads_the_voice_addresses_the_model_and_the_firmware_nothing_else() {
        let facts = read_config(&config()).unwrap();
        assert_eq!(
            facts,
            StationFacts {
                addresses: yandex(),
                device_type: Some("yandexstation_test".into()),
                software_version: Some("1.2.3.4".into()),
                points_to: PointsTo::Yandex,
            }
        );
        assert!(!serde_json::to_string(&facts).unwrap().contains("not-a-real-key"));
    }

    #[test]
    fn tells_a_gateway_from_yandex_and_from_anything_else() {
        let gateway = config().replace(YANDEX_URL, "ws://192.0.2.10/uni.ws");
        assert_eq!(read_config(&gateway).unwrap().points_to, PointsTo::Gateway);
        let other = config().replace(YANDEX_URL, "wss://example.org/x");
        assert_eq!(read_config(&other).unwrap().points_to, PointsTo::Other);
    }

    #[test]
    fn refuses_a_file_it_does_not_know() {
        assert_eq!(read_config("not json").unwrap_err().0, "quasar.cfg is not JSON");
        assert_eq!(read_config(r#"{"aliced": {}}"#).unwrap_err().0, "quasar.cfg has no aliced.uniProxyUrl");
    }

    #[test]
    fn replaces_the_two_addresses_in_place_and_keeps_every_other_byte() {
        let next = with_addresses(&config(), &to("ws://192.0.2.10/uni.ws")).unwrap();
        assert_eq!(next, config().replace(YANDEX_URL, "ws://192.0.2.10/uni.ws").replace(YANDEX_QUIC_URL, ""));
        assert_eq!(read_config(&next).unwrap().addresses, to("ws://192.0.2.10/uni.ws"));
        // And back: the copy's values restore the file exactly.
        assert_eq!(with_addresses(&next, &yandex()).unwrap(), config());
    }

    #[test]
    fn keeps_a_file_without_the_quic_key_without_one() {
        let no_quic = config().replace(&format!("        \"uniProxyQuicUrl\" : \"{YANDEX_QUIC_URL}\",\n"), "");
        let next = with_addresses(&no_quic, &to("ws://192.0.2.10/uni.ws")).unwrap();
        assert_eq!(read_config(&next).unwrap().addresses.quic, "");
    }

    #[test]
    fn writes_nothing_it_cannot_write_safely_and_nothing_into_a_file_with_two_voice_servers() {
        let error = with_addresses(&config(), &to(r#"ws://x"; reboot; "/uni.ws"#)).unwrap_err();
        assert!(error.0.contains("not an address"));
        assert!(with_addresses(&config(), &to("")).unwrap_err().0.contains("not an address"));
        let twice =
            config().replace("\"vadEnabled\"", &format!("\"uniProxyUrl\" : \"{YANDEX_URL}\",\n        \"vadEnabled\""));
        assert!(with_addresses(&twice, &to("ws://192.0.2.10/uni.ws")).unwrap_err().0.contains("2 times"));
    }

    fn host(host: &str, port: u16) -> GatewayHost {
        GatewayHost { host: host.into(), port }
    }

    #[test]
    fn takes_host_and_host_port_with_port_80_left_out_of_what_the_station_dials() {
        assert_eq!(parse_gateway_host("192.0.2.10"), Some(host("192.0.2.10", 80)));
        assert_eq!(parse_gateway_host(" Gateway.local:3000 "), Some(host("gateway.local", 3000)));
        // Typed in the panel without a port: the gateway's own, not 80.
        assert_eq!(parse_gateway_host_or("192.0.2.10", 7780), Some(host("192.0.2.10", 7780)));
        assert_eq!(parse_gateway_host_or("192.0.2.10:80", 7780), Some(host("192.0.2.10", 80)));
        assert_eq!(gateway_addresses(&host("192.0.2.10", 80), "/uni.ws"), to("ws://192.0.2.10/uni.ws"));
        assert_eq!(gateway_addresses(&host("192.0.2.10", 3000), "/uni.ws").url, "ws://192.0.2.10:3000/uni.ws");
    }

    #[test]
    fn refuses_what_is_not_an_address() {
        for input in ["", "ws://192.0.2.10", "a b", "host:0", "host:70000", ".local", "a..b", "x;reboot"] {
            assert_eq!(parse_gateway_host(input), None, "{input}");
        }
    }

    #[test]
    fn takes_a_station_by_its_ipv4_address_only() {
        assert_eq!(plain_address("::ffff:192.0.2.23").as_deref(), Some("192.0.2.23"));
        assert_eq!(plain_address("192.0.2.023").as_deref(), Some("192.0.2.23"));
        assert_eq!(plain_address("192.0.2.256"), None);
        assert_eq!(plain_address("station.local"), None);
    }
}
