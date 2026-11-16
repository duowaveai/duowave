//! A person's links as shoes' client hops: `vless://` (Reality with Vision, TLS, XHTTP, WebSocket), `hysteria2://` and
//! `hy2://`, `tuic://`, `ss://`, `trojan://`, `anytls://`, `wireguard://` and a WireGuard `.conf` text. shoes reads
//! YAML, not links: each link becomes one `{address, protocol, …}` hop. A link this file does not understand is passed
//! over with the reason (`Skip`), the rest work. Nothing here ever logs a link: they carry the passwords.

use percent_encoding::percent_decode_str;
use serde_json::{Map, Value, json};
use url::Url;

/// One server of a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    /// As people see it: the link's `#name`, the host when it has none.
    pub name: String,
    /// The link's kind for the journal and the window: `vless`, `hysteria2`, `tuic`, `ss`, `trojan`, `anytls`,
    /// `wireguard`.
    pub scheme: &'static str,
    /// The server's host (a name or an address), for the window's line and nothing secret.
    pub host: String,
    /// The shoes client hop: `address`, `protocol`, and `transport`/`quic_settings` for QUIC.
    pub hop: Value,
}

/// Why a link was passed over: a key of the dictionaries (i18n.rs) and what it names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Skip {
    pub scheme: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Skip {
    fn new(scheme: &str, reason: &str, detail: impl Into<String>) -> Skip {
        Skip { scheme: scheme.to_string(), reason: reason.to_string(), detail: detail.into() }
    }
}

/// What a person pasted.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A subscription: the gateway reads it now and then (subscription.rs).
    Subscription(String),
    /// Links, one per line, a base64 list of them, or a WireGuard file.
    Inline(String),
}

pub fn classify(text: &str) -> Source {
    let trimmed = text.trim();
    let one_line = !trimmed.contains(['\n', '\r', ' ']);
    if one_line && (trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        Source::Subscription(trimmed.to_string())
    } else {
        Source::Inline(trimmed.to_string())
    }
}

/// The servers of a text: a WireGuard file, a base64 list or links one per line. Names are made unique («Амстердам»,
/// «Амстердам 2»): a person picks a server by its name.
pub fn parse_text(text: &str) -> (Vec<Server>, Vec<Skip>) {
    let text = crate::subscription::decode_body(text);
    let mut servers = Vec::new();
    let mut skipped = Vec::new();
    if text.contains("[Interface]") {
        for part in split_wireguard(&text) {
            match wireguard_conf(&part) {
                Ok(server) => servers.push(server),
                Err(skip) => skipped.push(skip),
            }
        }
    } else {
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            match parse_link(line) {
                Ok(server) => servers.push(server),
                Err(skip) => skipped.push(skip),
            }
        }
    }
    unique_names(&mut servers);
    (servers, skipped)
}

fn unique_names(servers: &mut [Server]) {
    let mut seen: Vec<String> = Vec::new();
    for server in servers.iter_mut() {
        let base = server.name.clone();
        let mut name = base.clone();
        let mut n = 2;
        while seen.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        seen.push(name.clone());
        server.name = name;
    }
}

/// Several `[Interface]` files pasted one after another.
fn split_wireguard(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim().eq_ignore_ascii_case("[Interface]") || parts.is_empty() {
            parts.push(String::new());
        }
        let last = parts.last_mut().expect("pushed above");
        last.push_str(line);
        last.push('\n');
    }
    parts.into_iter().filter(|part| part.contains("[Interface]") || part.contains("[interface]")).collect()
}

pub fn parse_link(link: &str) -> Result<Server, Skip> {
    let scheme = link.split("://").next().unwrap_or_default().to_ascii_lowercase();
    if !link.contains("://") {
        return Err(Skip::new("?", "not_a_link", ""));
    }
    let known: &'static str = match scheme.as_str() {
        "vless" => "vless",
        "hysteria2" | "hy2" => "hysteria2",
        "tuic" => "tuic",
        "ss" => "ss",
        "trojan" => "trojan",
        "anytls" => "anytls",
        "wireguard" | "wg" => "wireguard",
        "vmess" | "ssr" | "socks" | "http" | "https" | "naive+https" | "snell" => {
            return Err(Skip::new(&scheme, "scheme_unsupported", ""));
        }
        _ => return Err(Skip::new(&scheme, "scheme_unknown", "")),
    };
    match known {
        "ss" => shadowsocks(link),
        "hysteria2" => hysteria2(link),
        _ => {
            let url = Url::parse(link).map_err(|_| Skip::new(known, "bad_link", ""))?;
            match known {
                "vless" => vless(&url),
                "tuic" => tuic(&url),
                "trojan" => trojan(&url),
                "anytls" => anytls(&url),
                _ => wireguard_link(&url),
            }
        }
    }
}

/// The link's query as pairs, decoded; later keys win.
struct Query(Vec<(String, String)>);

impl Query {
    fn of(url: &Url) -> Query {
        Query(url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect())
    }

    fn get(&self, keys: &[&str]) -> Option<String> {
        self.0.iter().rev().find(|(key, _)| keys.contains(&key.as_str())).map(|(_, value)| value.clone())
    }

    fn some(&self, keys: &[&str]) -> Option<String> {
        self.get(keys).filter(|value| !value.is_empty())
    }

    fn flag(&self, keys: &[&str]) -> bool {
        self.get(keys).is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
    }

    fn list(&self, keys: &[&str]) -> Vec<String> {
        self.some(keys)
            .map(|value| value.split(',').map(str::trim).filter(|one| !one.is_empty()).map(String::from).collect())
            .unwrap_or_default()
    }
}

fn decoded(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// The name from `#…`, the host without it.
fn name_of(fragment: Option<&str>, host: &str) -> String {
    let name = fragment.map(decoded).map(|name| name.trim().to_string()).unwrap_or_default();
    if name.is_empty() { host.to_string() } else { name }
}

/// `host:port` as shoes reads an address: IPv6 in brackets.
fn address(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

fn host_port(url: &Url, scheme: &str) -> Result<(String, u16), Skip> {
    let host = url.host_str().filter(|host| !host.is_empty()).ok_or_else(|| Skip::new(scheme, "no_address", ""))?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = url.port().ok_or_else(|| Skip::new(scheme, "no_address", ""))?;
    Ok((host, port))
}

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

/// The name a TLS hop sends: the link's `sni`, else the host when it is a name.
fn sni(query: &Query, host: &str) -> Option<String> {
    query.some(&["sni", "peer", "serverName", "servername"]).or_else(|| (!is_ip(host)).then(|| host.to_string()))
}

fn insecure(query: &Query) -> bool {
    query.flag(&["allowInsecure", "insecure", "allow_insecure", "skip-cert-verify"])
}

/// Around an inner protocol: `security=tls` (with `sni`, `alpn`, Vision) or `reality` (`pbk`, `sid`, `sni`, Vision).
fn secured(
    scheme: &str,
    query: &Query,
    host: &str,
    inner: Value,
    vision: bool,
    default_tls: bool,
) -> Result<Value, Skip> {
    let security = query.some(&["security"]).unwrap_or_else(|| if default_tls { "tls" } else { "none" }.to_string());
    match security.to_ascii_lowercase().as_str() {
        "none" | "" => {
            if vision {
                return Err(Skip::new(scheme, "vision_needs_tls", ""));
            }
            Ok(inner)
        }
        "tls" | "xtls" => {
            let mut tls = Map::new();
            tls.insert("type".into(), json!("tls"));
            if let Some(name) = sni(query, host) {
                tls.insert("sni_hostname".into(), json!(name));
            }
            if insecure(query) {
                tls.insert("verify".into(), json!(false));
            }
            let alpn = query.list(&["alpn"]);
            if !alpn.is_empty() {
                tls.insert("alpn_protocols".into(), json!(alpn));
            }
            if vision {
                tls.insert("vision".into(), json!(true));
            }
            tls.insert("protocol".into(), inner);
            Ok(Value::Object(tls))
        }
        "reality" => {
            let public_key = query
                .some(&["pbk", "publicKey", "public_key"])
                .ok_or_else(|| Skip::new(scheme, "reality_no_key", ""))?;
            let name = query
                .some(&["sni", "serverName", "servername", "peer"])
                .ok_or_else(|| Skip::new(scheme, "reality_no_sni", ""))?;
            let mut reality = Map::new();
            reality.insert("type".into(), json!("reality"));
            reality.insert("public_key".into(), json!(public_key));
            reality.insert("short_id".into(), json!(query.get(&["sid", "shortId", "short_id"]).unwrap_or_default()));
            reality.insert("sni_hostname".into(), json!(name));
            if let Some(verify) = query.some(&["pqv", "mldsa65Verify"]) {
                reality.insert("mldsa65_verify".into(), json!(verify));
            }
            if vision {
                reality.insert("vision".into(), json!(true));
            }
            reality.insert("protocol".into(), inner);
            Ok(Value::Object(reality))
        }
        other => Err(Skip::new(scheme, "security_unsupported", other)),
    }
}

/// XHTTP's buffer for data not yet read, per connection (shoes' `limits.max_buffered_bytes`).
pub const XHTTP_BUFFER_BYTES: usize = 1024 * 1024;

/// The transport under the security: `tcp`/`raw` as is, `xhttp`/`splithttp`, `ws`. Others (gRPC, httpupgrade, KCP) are
/// not in shoes' client.
fn transported(scheme: &str, query: &Query, inner: Value) -> Result<(Value, bool), Skip> {
    let kind = query.some(&["type", "network", "net"]).unwrap_or_else(|| "tcp".into()).to_ascii_lowercase();
    match kind.as_str() {
        "tcp" | "raw" => Ok((inner, true)),
        "xhttp" | "splithttp" => {
            let mut xhttp = Map::new();
            xhttp.insert("type".into(), json!("xhttp"));
            xhttp.insert("path".into(), json!(query.some(&["path"]).unwrap_or_else(|| "/".into())));
            if let Some(host) = query.some(&["host"]) {
                xhttp.insert("host".into(), json!(host));
            }
            if let Some(mode) = query.some(&["mode"]) {
                let mode = mode.to_ascii_lowercase();
                if !matches!(mode.as_str(), "auto" | "packet-up" | "stream-up" | "stream-one") {
                    return Err(Skip::new(scheme, "xhttp_mode", mode));
                }
                xhttp.insert("mode".into(), json!(mode));
            }
            // What the engine may hold for the slow reader of a download: shoes' client lets HTTP/2 buffer 4 MB a stream
            // and 64 MB a connection — ten slow downloads took 47 MB in Linux ARM (16.11). 1 MB in all still carries
            // 80 Mbit/s at 100 ms.
            xhttp.insert("limits".into(), json!({ "max_buffered_bytes": XHTTP_BUFFER_BYTES }));
            xhttp.insert("protocol".into(), inner);
            // Vision does not go over XHTTP (Xray ignores the flow there too).
            Ok((Value::Object(xhttp), false))
        }
        "ws" | "websocket" => {
            let mut ws = Map::new();
            ws.insert("type".into(), json!("websocket"));
            let raw_path = query.some(&["path"]).unwrap_or_else(|| "/".into());
            // Xray's early data in the path: `/ws?ed=2048` — the header Sec-WebSocket-Protocol carries it.
            let (path, early) = match raw_path.split_once("?ed=") {
                Some((path, early)) => (path.to_string(), early.parse::<usize>().ok()),
                None => (raw_path.clone(), None),
            };
            ws.insert("matching_path".into(), json!(path));
            if let Some(host) = query.some(&["host"]) {
                ws.insert("matching_headers".into(), json!({ "Host": host }));
            }
            if let (Some(early), true) = (early, query.some(&["host"]).is_some()) {
                ws.insert("max_early_data".into(), json!(early));
                ws.insert("early_data_header_name".into(), json!("Sec-WebSocket-Protocol"));
            }
            ws.insert("protocol".into(), inner);
            Ok((Value::Object(ws), false))
        }
        other => Err(Skip::new(scheme, "transport_unsupported", other)),
    }
}

fn vless(url: &Url) -> Result<Server, Skip> {
    let (host, port) = host_port(url, "vless")?;
    let query = Query::of(url);
    let user_id = decoded(url.username());
    if user_id.is_empty() {
        return Err(Skip::new("vless", "no_user", ""));
    }
    let mut inner = Map::new();
    inner.insert("type".into(), json!("vless"));
    inner.insert("user_id".into(), json!(user_id));
    if let Some(encryption) = query.some(&["encryption"]).filter(|value| value != "none") {
        inner.insert("encryption".into(), json!(encryption));
    }
    let flow = query.get(&["flow"]).unwrap_or_default();
    let vision = flow.starts_with("xtls-rprx-vision");
    if !flow.is_empty() && !vision {
        return Err(Skip::new("vless", "flow_unsupported", flow));
    }
    let (transport, plain) = transported("vless", &query, Value::Object(inner))?;
    let protocol = secured("vless", &query, &host, transport, vision && plain, false)?;
    Ok(Server {
        name: name_of(url.fragment(), &host),
        scheme: "vless",
        host: host.clone(),
        hop: json!({ "address": address(&host, port), "protocol": protocol }),
    })
}

fn trojan(url: &Url) -> Result<Server, Skip> {
    let (host, port) = host_port(url, "trojan")?;
    let query = Query::of(url);
    let password = decoded(url.username());
    if password.is_empty() {
        return Err(Skip::new("trojan", "no_password", ""));
    }
    let inner = json!({ "type": "trojan", "password": password });
    let (transport, _) = transported("trojan", &query, inner)?;
    let protocol = secured("trojan", &query, &host, transport, false, true)?;
    Ok(Server {
        name: name_of(url.fragment(), &host),
        scheme: "trojan",
        host: host.clone(),
        hop: json!({ "address": address(&host, port), "protocol": protocol }),
    })
}

fn anytls(url: &Url) -> Result<Server, Skip> {
    let (host, port) = host_port(url, "anytls")?;
    let query = Query::of(url);
    let mut password = decoded(url.username());
    if let Some(rest) = url.password() {
        password = format!("{password}:{}", decoded(rest));
    }
    if password.is_empty() {
        return Err(Skip::new("anytls", "no_password", ""));
    }
    let inner = json!({ "type": "anytls", "password": password });
    let protocol = secured("anytls", &query, &host, inner, false, true)?;
    Ok(Server {
        name: name_of(url.fragment(), &host),
        scheme: "anytls",
        host: host.clone(),
        hop: json!({ "address": address(&host, port), "protocol": protocol }),
    })
}

/// QUIC's TLS: the name, verification, ALPN (`h3` by default: TUIC without it does not connect, 15.11).
fn quic_settings(query: &Query, host: &str) -> Value {
    let mut settings = Map::new();
    if let Some(name) = sni(query, host) {
        settings.insert("sni_hostname".into(), json!(name));
    }
    if insecure(query) {
        settings.insert("verify".into(), json!(false));
    }
    let alpn = query.list(&["alpn"]);
    settings.insert("alpn_protocols".into(), json!(if alpn.is_empty() { vec!["h3".to_string()] } else { alpn }));
    Value::Object(settings)
}

/// `hysteria2://auth@host:port/?sni=&insecure=1&obfs=salamander&obfs-password=&mport=`: parsed by hand, since the
/// port may be a list (`443,20000-30000`) the URL parser refuses.
fn hysteria2(link: &str) -> Result<Server, Skip> {
    let rest = link.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (rest, None),
    };
    let (authority, query_text) = match rest.split_once('?') {
        Some((authority, query)) => (authority.trim_end_matches('/'), query),
        None => (rest.trim_end_matches('/'), ""),
    };
    let (auth, hostport) = authority.rsplit_once('@').ok_or_else(|| Skip::new("hysteria2", "no_password", ""))?;
    let password = decoded(auth);
    let parsed = Url::parse(&format!("hy2://x@{hostport}/")).ok();
    let (host, ports) = match parsed.as_ref().and_then(|url| Some((url.host_str()?.to_string(), url.port()?))) {
        Some((host, port)) => (host, port.to_string()),
        None => {
            let (host, ports) = hostport.rsplit_once(':').ok_or_else(|| Skip::new("hysteria2", "no_address", ""))?;
            (host.to_string(), ports.to_string())
        }
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let query = Query(
        url::form_urlencoded::parse(query_text.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect(),
    );
    let first: u16 = ports
        .split([',', '-'])
        .next()
        .and_then(|port| port.trim().parse().ok())
        .ok_or_else(|| Skip::new("hysteria2", "no_address", ""))?;
    let mut protocol = Map::new();
    protocol.insert("type".into(), json!("hysteria2"));
    protocol.insert("password".into(), json!(password));
    if let Some(obfs) = query.some(&["obfs"]) {
        if obfs != "salamander" {
            return Err(Skip::new("hysteria2", "obfs_unsupported", obfs));
        }
        let secret = query
            .some(&["obfs-password", "obfs_password"])
            .ok_or_else(|| Skip::new("hysteria2", "obfs_unsupported", "salamander"))?;
        protocol.insert("obfs".into(), json!({ "type": "salamander", "password": secret }));
    }
    let hopping = query.some(&["mport"]).or_else(|| (ports.contains([',', '-'])).then(|| ports.clone()));
    if let Some(ports) = hopping {
        protocol.insert("hopping".into(), json!({ "ports": ports }));
    }
    Ok(Server {
        name: name_of(fragment, &host),
        scheme: "hysteria2",
        host: host.clone(),
        hop: json!({
            "address": address(&host, first),
            "transport": "quic",
            "quic_settings": quic_settings(&query, &host),
            "protocol": Value::Object(protocol),
        }),
    })
}

fn tuic(url: &Url) -> Result<Server, Skip> {
    let (host, port) = host_port(url, "tuic")?;
    let query = Query::of(url);
    let uuid = decoded(url.username());
    let password = url.password().map(decoded).unwrap_or_default();
    if uuid.is_empty() || password.is_empty() {
        return Err(Skip::new("tuic", "no_password", ""));
    }
    let mut protocol = Map::new();
    protocol.insert("type".into(), json!("tuic"));
    protocol.insert("uuid".into(), json!(uuid));
    protocol.insert("password".into(), json!(password));
    if let Some(mode) = query.some(&["udp_relay_mode", "udp-relay-mode"])
        && matches!(mode.as_str(), "native" | "quic")
    {
        protocol.insert("udp_relay_mode".into(), json!(mode));
    }
    Ok(Server {
        name: name_of(url.fragment(), &host),
        scheme: "tuic",
        host: host.clone(),
        hop: json!({
            "address": address(&host, port),
            "transport": "quic",
            "quic_settings": quic_settings(&query, &host),
            "protocol": Value::Object(protocol),
        }),
    })
}

/// SIP002 `ss://base64(method:password)@host:port#name`, the same with `method:password` percent-encoded, and the old
/// `ss://base64(method:password@host:port)#name`.
fn shadowsocks(link: &str) -> Result<Server, Skip> {
    let rest = &link[link.find("://").map(|at| at + 3).unwrap_or(0)..];
    let (body, fragment) = match rest.split_once('#') {
        Some((body, fragment)) => (body, Some(fragment)),
        None => (rest, None),
    };
    let (body, query_text) = match body.split_once('?') {
        Some((body, query)) => (body.trim_end_matches('/'), query),
        None => (body.trim_end_matches('/'), ""),
    };
    let query = Query(
        url::form_urlencoded::parse(query_text.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect(),
    );
    if let Some(plugin) = query.some(&["plugin"]) {
        return Err(Skip::new("ss", "ss_plugin", plugin.split(';').next().unwrap_or_default()));
    }
    let (userinfo, hostport) = match body.rsplit_once('@') {
        Some((userinfo, hostport)) => (decoded(userinfo), hostport.to_string()),
        None => {
            let whole = crate::subscription::base64_text(body).ok_or_else(|| Skip::new("ss", "bad_link", ""))?;
            let (userinfo, hostport) = whole.rsplit_once('@').ok_or_else(|| Skip::new("ss", "bad_link", ""))?;
            (userinfo.to_string(), hostport.to_string())
        }
    };
    let (method, password) = match userinfo.split_once(':') {
        Some((method, password)) if !method.contains('=') => (method.to_string(), password.to_string()),
        _ => {
            let plain = crate::subscription::base64_text(&userinfo).ok_or_else(|| Skip::new("ss", "bad_link", ""))?;
            let (method, password) = plain.split_once(':').ok_or_else(|| Skip::new("ss", "bad_link", ""))?;
            (method.to_string(), password.to_string())
        }
    };
    let method = method.to_ascii_lowercase();
    let known = matches!(
        method.as_str(),
        "aes-128-gcm"
            | "aes-256-gcm"
            | "chacha20-ietf-poly1305"
            | "chacha20-poly1305"
            | "2022-blake3-aes-128-gcm"
            | "2022-blake3-aes-256-gcm"
            | "2022-blake3-chacha20-poly1305"
            | "2022-blake3-chacha20-ietf-poly1305"
    );
    if !known {
        return Err(Skip::new("ss", "cipher_unsupported", method));
    }
    let parsed = Url::parse(&format!("ss://x@{hostport}/")).map_err(|_| Skip::new("ss", "no_address", ""))?;
    let (host, port) = host_port(&parsed, "ss")?;
    Ok(Server {
        name: name_of(fragment, &host),
        scheme: "ss",
        host: host.clone(),
        hop: json!({
            "address": address(&host, port),
            "protocol": { "type": "shadowsocks", "cipher": method, "password": password },
        }),
    })
}

/// A WireGuard client as shoes takes it: one peer, its address, keys and the tunnel's own addresses.
#[allow(clippy::too_many_arguments)]
fn wireguard_server(
    name: String,
    endpoint: &str,
    private_key: String,
    public_key: String,
    pre_shared_key: Option<String>,
    addresses: Vec<String>,
    allowed_ips: Vec<String>,
    keepalive: Option<u16>,
    mtu: Option<u16>,
) -> Result<Server, Skip> {
    let parsed = Url::parse(&format!("wg://x@{endpoint}/")).map_err(|_| Skip::new("wireguard", "no_address", ""))?;
    let (host, port) = host_port(&parsed, "wireguard")?;
    if private_key.is_empty() || public_key.is_empty() {
        return Err(Skip::new("wireguard", "wireguard_keys", ""));
    }
    let addresses: Vec<String> = addresses
        .into_iter()
        .map(|one| {
            if one.contains('/') {
                one
            } else if one.contains(':') {
                format!("{one}/128")
            } else {
                format!("{one}/32")
            }
        })
        .collect();
    // shoes takes one address per family.
    let mut tunnel: Vec<String> = Vec::new();
    for one in addresses {
        let v6 = one.contains(':');
        if !tunnel.iter().any(|kept| kept.contains(':') == v6) {
            tunnel.push(one);
        }
    }
    if tunnel.is_empty() {
        return Err(Skip::new("wireguard", "wireguard_address", ""));
    }
    let mut protocol = Map::new();
    protocol.insert("type".into(), json!("wireguard"));
    protocol.insert("private_key".into(), json!(private_key));
    protocol.insert("public_key".into(), json!(public_key));
    if let Some(key) = pre_shared_key.filter(|key| !key.is_empty()) {
        protocol.insert("pre_shared_key".into(), json!(key));
    }
    protocol.insert("addresses".into(), json!(tunnel));
    let allowed = if allowed_ips.is_empty() { vec!["0.0.0.0/0".to_string(), "::/0".to_string()] } else { allowed_ips };
    protocol.insert("allowed_ips".into(), json!(allowed));
    if let Some(keepalive) = keepalive {
        protocol.insert("persistent_keepalive".into(), json!(keepalive));
    }
    if let Some(mtu) = mtu {
        protocol.insert("mtu".into(), json!(mtu.clamp(1280, 1420)));
    }
    let name = if name.is_empty() { host.clone() } else { name };
    Ok(Server {
        name,
        scheme: "wireguard",
        host: host.clone(),
        hop: json!({ "address": address(&host, port), "protocol": Value::Object(protocol) }),
    })
}

fn split_list(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|one| !one.is_empty()).map(String::from).collect()
}

/// A WireGuard client file: `[Interface]` (PrivateKey, Address, MTU) and one `[Peer]` (PublicKey, PresharedKey,
/// AllowedIPs, Endpoint, PersistentKeepalive). The name — a `# Name = …` line or the peer's host.
fn wireguard_conf(text: &str) -> Result<Server, Skip> {
    let mut section = String::new();
    let mut fields: Vec<(String, String, String)> = Vec::new();
    let mut name = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(comment) = line.strip_prefix('#') {
            if let Some((key, value)) = comment.split_once('=')
                && key.trim().eq_ignore_ascii_case("name")
            {
                name = value.trim().to_string();
            }
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.push((section.clone(), key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let peers = fields.iter().filter(|(section, key, _)| section == "peer" && key == "publickey").count();
    if peers != 1 {
        return Err(Skip::new("wireguard", "wireguard_peers", peers.to_string()));
    }
    let get = |section: &str, key: &str| {
        fields.iter().find(|(s, k, _)| s == section && k == key).map(|(_, _, value)| value.clone())
    };
    let all = |section: &str, key: &str| -> Vec<String> {
        fields.iter().filter(|(s, k, _)| s == section && k == key).flat_map(|(_, _, value)| split_list(value)).collect()
    };
    let endpoint = get("peer", "endpoint").ok_or_else(|| Skip::new("wireguard", "no_address", ""))?;
    wireguard_server(
        name,
        &endpoint,
        get("interface", "privatekey").unwrap_or_default(),
        get("peer", "publickey").unwrap_or_default(),
        get("peer", "presharedkey"),
        all("interface", "address"),
        all("peer", "allowedips"),
        get("peer", "persistentkeepalive").and_then(|value| value.parse().ok()),
        get("interface", "mtu").and_then(|value| value.parse().ok()),
    )
}

/// `wireguard://<private key>@host:port?publickey=&address=&presharedkey=&mtu=&keepalive=#name` (v2rayN, sing-box).
fn wireguard_link(url: &Url) -> Result<Server, Skip> {
    let query = Query::of(url);
    let (host, port) = host_port(url, "wireguard")?;
    wireguard_server(
        name_of(url.fragment(), ""),
        &address(&host, port),
        decoded(url.username()),
        query.some(&["publickey", "public_key", "peer_public_key"]).unwrap_or_default(),
        query.some(&["presharedkey", "pre_shared_key", "psk"]),
        query.list(&["address", "ip"]),
        query.list(&["allowedips", "allowed_ips"]),
        query.some(&["keepalive", "persistent_keepalive"]).and_then(|value| value.parse().ok()),
        query.some(&["mtu"]).and_then(|value| value.parse().ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(link: &str) -> Server {
        parse_link(link).unwrap_or_else(|skip| panic!("{link}: {skip:?}"))
    }

    #[test]
    fn vless_reality_with_vision() {
        let server = one(
            "vless://b85798ef-e9dc-46a4-9a87-8da4499d36d0@nl.example.com:443?type=tcp&security=reality&pbk=PUBKEY&sid=0123abcd&sni=www.google.com&fp=chrome&flow=xtls-rprx-vision#%F0%9F%87%B3%F0%9F%87%B1%20Amsterdam",
        );
        assert_eq!(server.name, "🇳🇱 Amsterdam");
        assert_eq!(server.scheme, "vless");
        assert_eq!(
            server.hop,
            json!({
                "address": "nl.example.com:443",
                "protocol": {
                    "type": "reality", "public_key": "PUBKEY", "short_id": "0123abcd", "sni_hostname": "www.google.com",
                    "vision": true,
                    "protocol": { "type": "vless", "user_id": "b85798ef-e9dc-46a4-9a87-8da4499d36d0" }
                }
            })
        );
    }

    #[test]
    fn vless_xhttp_over_tls_and_reality() {
        let server = one(
            "vless://id@1.2.3.4:8443?security=tls&sni=cdn.example.com&type=xhttp&path=%2Ftunnel&mode=packet-up&host=cdn.example.com&allowInsecure=1&alpn=h2#x",
        );
        assert_eq!(
            server.hop["protocol"],
            json!({
                "type": "tls", "sni_hostname": "cdn.example.com", "verify": false, "alpn_protocols": ["h2"],
                "protocol": {
                    "type": "xhttp", "path": "/tunnel", "host": "cdn.example.com", "mode": "packet-up",
                    "limits": { "max_buffered_bytes": 1048576 },
                    "protocol": { "type": "vless", "user_id": "id" }
                }
            })
        );
        // Vision does not go over XHTTP: the flow is dropped there.
        let reality = one("vless://id@h:443?security=reality&pbk=K&sni=s.example&type=xhttp&flow=xtls-rprx-vision");
        assert_eq!(reality.hop["protocol"]["type"], "reality");
        assert!(reality.hop["protocol"].get("vision").is_none());
        assert_eq!(reality.hop["protocol"]["protocol"]["type"], "xhttp");
        assert_eq!(reality.hop["protocol"]["protocol"]["path"], "/");
    }

    #[test]
    fn vless_websocket_and_plain() {
        let ws = one("vless://id@h.example:443?security=tls&type=ws&path=%2Fws%3Fed%3D2048&host=front.example#w");
        assert_eq!(
            ws.hop["protocol"]["protocol"],
            json!({
                "type": "websocket", "matching_path": "/ws", "matching_headers": { "Host": "front.example" },
                "max_early_data": 2048, "early_data_header_name": "Sec-WebSocket-Protocol",
                "protocol": { "type": "vless", "user_id": "id" }
            })
        );
        let plain = one("vless://id@10.0.0.1:80?encryption=none");
        assert_eq!(plain.hop["protocol"], json!({ "type": "vless", "user_id": "id" }));
        assert_eq!(plain.name, "10.0.0.1");
        assert_eq!(parse_link("vless://id@h:1?type=grpc").unwrap_err().reason, "transport_unsupported");
        assert_eq!(parse_link("vless://id@h:1?security=reality&sni=x").unwrap_err().reason, "reality_no_key");
        assert_eq!(parse_link("vless://id@h:1?flow=xtls-rprx-vision").unwrap_err().reason, "vision_needs_tls");
    }

    #[test]
    fn hysteria2_both_schemes_obfs_and_ports() {
        let server = one(
            "hysteria2://pass%40word@hy.example.com:443/?sni=real.example&insecure=1&obfs=salamander&obfs-password=secret#HY",
        );
        assert_eq!(
            server.hop,
            json!({
                "address": "hy.example.com:443", "transport": "quic",
                "quic_settings": { "sni_hostname": "real.example", "verify": false, "alpn_protocols": ["h3"] },
                "protocol": {
                    "type": "hysteria2", "password": "pass@word",
                    "obfs": { "type": "salamander", "password": "secret" }
                }
            })
        );
        let hopping = one("hy2://p@1.2.3.4:443,20000-30000?insecure=1");
        assert_eq!(hopping.hop["address"], "1.2.3.4:443");
        assert_eq!(hopping.hop["protocol"]["hopping"], json!({ "ports": "443,20000-30000" }));
        assert!(hopping.hop["quic_settings"].get("sni_hostname").is_none());
    }

    #[test]
    fn tuic_gets_h3_by_default() {
        let server = one(
            "tuic://d685aef3-b3c4-4932-9a9d-d0c2f6727dfa:abc%2B1@tuic.example:443?congestion_control=bbr&udp_relay_mode=quic&allow_insecure=1#T",
        );
        assert_eq!(
            server.hop,
            json!({
                "address": "tuic.example:443", "transport": "quic",
                "quic_settings": { "sni_hostname": "tuic.example", "verify": false, "alpn_protocols": ["h3"] },
                "protocol": {
                    "type": "tuic", "uuid": "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa", "password": "abc+1",
                    "udp_relay_mode": "quic"
                }
            })
        );
    }

    #[test]
    fn shadowsocks_every_form() {
        // SIP002 with base64 userinfo (aes-256-gcm:pass).
        let a = one("ss://YWVzLTI1Ni1nY206cGFzcw@ss.example:8388#A");
        assert_eq!(a.hop["protocol"], json!({ "type": "shadowsocks", "cipher": "aes-256-gcm", "password": "pass" }));
        // 2022 with a percent-encoded key.
        let b = one("ss://2022-blake3-aes-128-gcm:EREREREREREREREREREREQ%3D%3D@[2001:db8::1]:8388#B");
        assert_eq!(b.hop["address"], "[2001:db8::1]:8388");
        assert_eq!(b.hop["protocol"]["password"], "EREREREREREREREREREREQ==");
        // The old form: everything in base64 (aes-128-gcm:test@192.168.1.1:8388).
        let c = one("ss://YWVzLTEyOC1nY206dGVzdEAxOTIuMTY4LjEuMTo4Mzg4#C");
        assert_eq!(c.hop["address"], "192.168.1.1:8388");
        assert_eq!(c.hop["protocol"]["cipher"], "aes-128-gcm");
        assert_eq!(
            parse_link("ss://YWVzLTI1Ni1nY206cGFzcw@h:1?plugin=obfs-local%3Bobfs%3Dhttp").unwrap_err().reason,
            "ss_plugin"
        );
        assert_eq!(parse_link("ss://cmM0LW1kNTpw@h:1").unwrap_err().reason, "cipher_unsupported");
    }

    #[test]
    fn trojan_and_anytls_default_to_tls() {
        let trojan = one("trojan://secret%23pw@tr.example:443?sni=front.example&allowInsecure=1#Tr");
        assert_eq!(
            trojan.hop["protocol"],
            json!({
                "type": "tls", "sni_hostname": "front.example", "verify": false,
                "protocol": { "type": "trojan", "password": "secret#pw" }
            })
        );
        let any = one("anytls://pw@any.example:443/?sni=any.example&insecure=1#Any");
        assert_eq!(
            any.hop["protocol"],
            json!({
                "type": "tls", "sni_hostname": "any.example", "verify": false,
                "protocol": { "type": "anytls", "password": "pw" }
            })
        );
    }

    #[test]
    fn wireguard_file_and_link() {
        let conf = "[Interface]\nPrivateKey = PRIV=\nAddress = 10.7.0.2/32, 10.7.0.3/32, fd00::2\nDNS = 1.1.1.1\nMTU = 1500\n\n[Peer]\nPublicKey = PUB=\nPresharedKey = PSK=\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = wg.example.com:51820\nPersistentKeepalive = 25\n";
        let (servers, skipped) = parse_text(conf);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(
            servers[0].hop,
            json!({
                "address": "wg.example.com:51820",
                "protocol": {
                    "type": "wireguard", "private_key": "PRIV=", "public_key": "PUB=", "pre_shared_key": "PSK=",
                    "addresses": ["10.7.0.2/32", "fd00::2/128"], "allowed_ips": ["0.0.0.0/0", "::/0"],
                    "persistent_keepalive": 25, "mtu": 1420
                }
            })
        );
        assert_eq!(servers[0].name, "wg.example.com");
        let link = one("wireguard://PRIV%3D@1.2.3.4:51820?publickey=PUB%3D&address=10.0.0.2%2F32#Home");
        assert_eq!(link.name, "Home");
        assert_eq!(link.hop["protocol"]["allowed_ips"], json!(["0.0.0.0/0", "::/0"]));
    }

    #[test]
    fn a_list_keeps_what_it_understands_and_names_stay_unique() {
        let text = "vless://id@a.example:443?security=tls#Same\nvmess://eyJ2IjoyfQ==\n\nhttp-garbage\ntrojan://p@b.example:443#Same\n";
        let (servers, skipped) = parse_text(text);
        assert_eq!(servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Same", "Same 2"]);
        assert_eq!(skipped.iter().map(|s| s.reason.as_str()).collect::<Vec<_>>(), ["scheme_unsupported", "not_a_link"]);
    }

    #[test]
    fn a_subscription_is_one_url() {
        assert_eq!(classify(" https://sub.example/abc \n"), Source::Subscription("https://sub.example/abc".into()));
        assert!(matches!(classify("vless://x@h:1"), Source::Inline(_)));
        assert!(matches!(classify("https://a\nhttps://b"), Source::Inline(_)));
    }
}
