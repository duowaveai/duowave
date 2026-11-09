//! Two answers to what a web page in a household browser could do to a server on the home network. A page on another
//! site may not frame ours or read it through a DNS name that points here (DNS rebinding): requests come in by an
//! address, a local name or a name the owner listed; anything else gets 421. And every response says what it is (no
//! sniffing), keeps the referrer, and is never framed.

use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::operations::networks::{local_name, via_tunnel};

/// Whether a Host header is one this gateway answers: an IP, a local name, a name without a dot, or a listed one.
pub fn allowed_host(host_header: Option<&str>, names: &[String]) -> bool {
    let Some(host_header) = host_header else {
        return true; // HTTP/1.0 without Host: nothing to rebind
    };
    let lower = host_header.to_ascii_lowercase();
    let host = strip_port(&lower);
    if host.parse::<IpAddr>().is_ok() || local_name(host) {
        return true;
    }
    // A name without a dot (a Docker service, `raspberrypi`) resolves only inside the home.
    if !host.contains('.') {
        return true;
    }
    names.iter().any(|name| name == host)
}

fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) && !name.contains(':') => name,
        _ => host,
    }
}

/// The names a request may come by besides addresses and local names: the listed ones (ALLOWED_HOSTS, the media's
/// host) and those the panel's settings name now — LiveKit's address from outside and its call page, which change
/// without a restart.
pub struct AllowedHosts {
    listed: Vec<String>,
    from_settings: RwLock<Vec<String>>,
}

impl AllowedHosts {
    pub fn new(listed: Vec<String>) -> AllowedHosts {
        AllowedHosts { listed, from_settings: RwLock::new(Vec::new()) }
    }

    /// The panel's names now, in place of the ones before.
    pub fn set_from_settings(&self, names: Vec<String>) {
        *self.from_settings.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = names;
    }

    pub fn allows(&self, host_header: Option<&str>) -> bool {
        let mut names = self.listed.clone();
        names.extend(self.from_settings.read().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().cloned());
        allowed_host(host_header, &names)
    }
}

/// Middleware: the Host check, then the headers every response carries.
pub async fn security(State(names): State<Arc<AllowedHosts>>, request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).map(str::to_string);
    if !via_tunnel(request.headers()) && !names.allows(host.as_deref()) {
        let mut response = (StatusCode::MISDIRECTED_REQUEST, "Unknown host: add it to ALLOWED_HOSTS\n").into_response();
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        return response;
    }
    let media = request.uri().path().starts_with("/media/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    if media {
        // Our files for the station's player: never a page that runs.
        headers.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_addresses_local_names_and_listed_ones() {
        let names = vec!["gw.example.com".to_string()];
        assert!(allowed_host(None, &names));
        assert!(allowed_host(Some("192.168.1.100:3000"), &names));
        assert!(allowed_host(Some("[::1]:3000"), &names));
        assert!(allowed_host(Some("alice.local"), &names));
        assert!(allowed_host(Some("raspberrypi:80"), &names));
        assert!(allowed_host(Some("GW.example.com"), &names));
        assert!(!allowed_host(Some("evil.example.net"), &names));
        assert!(!allowed_host(Some("evil.example.net:3000"), &names));
    }

    #[test]
    fn takes_the_panels_names_as_they_change() {
        let hosts = AllowedHosts::new(vec!["gw.example.com".into()]);
        assert!(!hosts.allows(Some("call.example.com")));
        hosts.set_from_settings(vec!["call.example.com".into()]);
        assert!(hosts.allows(Some("call.example.com:443")));
        assert!(hosts.allows(Some("gw.example.com")));
        hosts.set_from_settings(Vec::new());
        assert!(!hosts.allows(Some("call.example.com")));
    }
}
