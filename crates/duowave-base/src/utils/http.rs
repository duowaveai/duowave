//! The gateway's outgoing connections, the same on a PC and on the station: rustls with ring and the Mozilla roots
//! built in (webpki-roots), so nothing depends on the system's certificate store — a station may have none.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::{ClientConfig, RootCertStore};

/// TLS for every client: reqwest and the WebSocket to Yandex.
pub fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// A reqwest builder with our TLS and a connect timeout; add the request timeouts a caller needs.
///
/// Every client goes out by «Выход в интернет» (src/egress): each new connection asks it where its host goes (`ROUTE`,
/// below: the house's floor sets it, this one only asks) — directly (the system's proxy, as before), through a connection's entry of the app «VPN», or to the
/// gateway's refusal when that connection does not answer. A shared client needs no rebuild when the routes change.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .tls_backend_preconfigured((*tls_config()).clone())
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("duowave/", env!("CARGO_PKG_VERSION")))
        .proxy(reqwest::Proxy::custom(proxy_for))
}

/// Where a request goes, as «Выход в интернет» decides (src/egress, a floor above): set once its table is first
/// shared. Until then — and in a program without it — the same as its table empty: another pretend station's entry on
/// this Mac, directly at home, else the system's proxy.
static ROUTE: OnceLock<fn(&url::Url) -> Option<url::Url>> = OnceLock::new();

/// «Выход в интернет» takes the requests' way out (`egress::proxy_for`); the first call counts.
pub fn route_by(route: fn(&url::Url) -> Option<url::Url>) {
    let _ = ROUTE.set(route);
}

/// Whether a host, or a row by its id, is refused now.
type Refuses = fn(&str) -> bool;

/// Whether «Выход в интернет» refuses a host, and a row by its id, now (src/egress sets it with its table); none refused
/// until then.
static REFUSED: OnceLock<(Refuses, Refuses)> = OnceLock::new();

/// «Выход в интернет» says which hosts and rows it refuses (`egress::refused_host`, `egress::refused_row`); the first
/// call counts.
pub fn refuse_by(host: Refuses, row: Refuses) {
    let _ = REFUSED.set((host, row));
}

/// Whether a row's calls are refused now (`provider:<id>`: its server does not answer, or no connection fits and it
/// has a key): the station's session gives a question for such a model to Alice.
pub fn refused_row(id: &str) -> bool {
    REFUSED.get().is_some_and(|(_, row)| row(id))
}

/// Whether a host's calls are refused now (its server does not answer, or no connection fits): a model there is not
/// asked (dialog/llm).
pub fn refused_host(host: &str) -> bool {
    REFUSED.get().is_some_and(|(refused, _)| refused(host))
}

/// The proxy for an app's own program (`HTTPS_PROXY`): «Выход в интернет»'s entry for apps with the app's id and
/// token (src/egress sets it with its table); none until then, or while the entry does not run.
static APP_ENTRY: OnceLock<fn(&str, &str) -> Option<String>> = OnceLock::new();

/// «Выход в интернет» gives the apps' programs its entry (`egress::app_entry`); the first call counts.
pub fn app_entry_by(entry: fn(&str, &str) -> Option<String>) {
    let _ = APP_ENTRY.set(entry);
}

/// The proxy address for an app's program, with its id and token: None when the entry does not run.
pub fn app_entry(id: &str, token: &str) -> Option<String> {
    APP_ENTRY.get().and_then(|entry| entry(id, token))
}

/// Who learns from an app's call that failed for a reason the network may explain (row, host, why: `timeout`,
/// `reset`, `403`, `451`): «Выход в интернет» (src/egress, `note_failure`), set with its table; none until then.
static NOTE: OnceLock<fn(&str, &str, &str)> = OnceLock::new();

/// «Выход в интернет» learns what does not open directly (`egress::note_failure`); the first call counts.
pub fn note_by(note: fn(&str, &str, &str)) {
    let _ = NOTE.set(note);
}

/// Who learns from a failed call, if anyone: called from a blocking thread (it writes the base).
pub fn failure_note() -> Option<fn(&str, &str, &str)> {
    NOTE.get().copied()
}

/// The proxy of a request, for reqwest's `Proxy::custom`: None — directly (or the system's proxy).
fn proxy_for(url: &url::Url) -> Option<url::Url> {
    if let Some(route) = ROUTE.get() {
        return route(url);
    }
    let host = url.host_str()?;
    if let Some(entry) = crate::utils::net::proxy_for(host) {
        return Some(entry);
    }
    if crate::utils::net::is_local(host) {
        return None;
    }
    env_proxy(url)
}

/// The system's proxy a direct call keeps using, as before the routes: `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`
/// (either case) unless `NO_PROXY` names the host.
pub fn env_proxy(url: &url::Url) -> Option<url::Url> {
    let var = |names: &[&str]| names.iter().find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let host = url.host_str()?.to_ascii_lowercase();
    if let Some(no) = var(&["NO_PROXY", "no_proxy"]) {
        let skip = no.split(',').map(str::trim).filter(|entry| !entry.is_empty()).any(|entry| {
            entry == "*" || {
                let entry = entry.trim_start_matches('.').trim_start_matches("*.").to_ascii_lowercase();
                host == entry || host.ends_with(&format!(".{entry}"))
            }
        });
        if skip {
            return None;
        }
    }
    let proxy = if url.scheme() == "https" {
        var(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"])
    } else {
        var(&["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"])
    }?;
    url::Url::parse(&proxy).ok()
}

/// The host of a URL, lower case; None when there is none.
pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

/// The shared client: one connection pool for the whole gateway.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| client_builder().build().expect("the HTTP client builds")).clone()
}

/// The shared client that follows no redirect: for a helper's `/home/v1` (src/helpers), which never sends anywhere
/// else — its 3xx is its answer, and nothing it names is visited, neither a device of its home nor another host for
/// the household's words.
pub fn client_without_redirects() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            client_builder().redirect(reqwest::redirect::Policy::none()).build().expect("the HTTP client builds")
        })
        .clone()
}

/// Why a request found no answer, in words and with its real cause: «не удалось соединиться (Connection reset by peer
/// (os error 104))», not reqwest's «error sending request for url (…)» alone. A tester's Station Mini 2 (22.11) could
/// not read two feeds while the computer beside it could, and its journal held only reqwest's words: the bottom of the
/// error's chain tells a refused connection, a reset, a timeout and a broken TLS handshake apart.
pub fn why_failed(error: &reqwest::Error) -> String {
    let what = if error.is_timeout() {
        crate::tr!("нет ответа вовремя")
    } else if error.is_connect() {
        crate::tr!("не удалось соединиться")
    } else {
        crate::tr!("связь оборвалась")
    };
    format!("{what} ({})", deepest_cause(error))
}

/// The bottom of the error's chain — «Connection refused (os error 111)», «Network unreachable (os error 101)» —, not
/// reqwest's «error sending request for url (…)» alone; a timeout says so if its bottom does not.
pub fn deepest_cause(error: &reqwest::Error) -> String {
    let mut deepest: &dyn std::error::Error = error;
    while let Some(next) = deepest.source() {
        deepest = next;
    }
    let said = deepest.to_string();
    if error.is_timeout() && !said.to_lowercase().contains("timed out") {
        format!("timed out: {said}")
    } else {
        said
    }
}

/// The WebSocket connector for tokio-tungstenite with the same TLS.
pub fn ws_connector() -> tokio_tungstenite::Connector {
    tokio_tungstenite::Connector::Rustls(tls_config())
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_the_clients() {
        let _ = super::client();
        let _ = super::client_without_redirects();
        let _ = super::ws_connector();
    }
}
