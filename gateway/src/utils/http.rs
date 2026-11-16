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
/// Every client goes out by «Выход в интернет» (src/egress): each new connection asks `egress::proxy_for` where its
/// host goes — directly (the system's proxy, as before), through a connection's entry of the app «VPN», or to the
/// gateway's refusal when that connection does not answer. A shared client needs no rebuild when the routes change.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .tls_backend_preconfigured((*tls_config()).clone())
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("duowave/", env!("CARGO_PKG_VERSION")))
        .proxy(reqwest::Proxy::custom(crate::egress::proxy_for))
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
