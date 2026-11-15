//! A small HTTP/1.1 client on what shoes already brings (hyper, rustls, webpki roots): the gateway's Host API and the
//! engine's Clash API on this machine, a subscription and Cloudflare's trace on the internet — directly or through a
//! connection's own entrance (`CONNECT`). No reqwest: every megabyte counts on the station.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// A connection's entrance: the engine's `mixed` door on this machine.
#[derive(Debug, Clone)]
pub struct Via {
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: hyper::HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

pub struct Ask<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(&'a str, String)>,
    pub body: Option<Vec<u8>>,
    pub via: Option<&'a Via>,
    pub limit: usize,
    pub timeout: Duration,
}

impl<'a> Ask<'a> {
    pub fn get(url: &'a str) -> Ask<'a> {
        Ask {
            method: "GET",
            url,
            headers: Vec::new(),
            body: None,
            via: None,
            limit: 1 << 20,
            timeout: Duration::from_secs(15),
        }
    }

    pub fn json(method: &'a str, url: &'a str, body: &serde_json::Value) -> Ask<'a> {
        Ask { method, body: Some(serde_json::to_vec(body).unwrap_or_default()), ..Ask::get(url) }
    }

    pub fn header(mut self, name: &'a str, value: impl Into<String>) -> Ask<'a> {
        self.headers.push((name, value.into()));
        self
    }

    pub fn via(mut self, via: Option<&'a Via>) -> Ask<'a> {
        self.via = via;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Ask<'a> {
        self.timeout = timeout;
        self
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

fn tls() -> tokio_rustls::TlsConnector {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let config =
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("the provider's protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
        Arc::new(config)
    });
    tokio_rustls::TlsConnector::from(config.clone())
}

/// A tunnel to `host:port` through the entrance.
async fn connect_via(via: &Via, host: &str, port: u16) -> std::io::Result<TcpStream> {
    let mut stream = TcpStream::connect(("127.0.0.1", via.port)).await?;
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", via.user, via.password));
    let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await? == 0 || head.len() > 8192 {
            return Err(std::io::Error::other("the entrance closed"));
        }
        head.push(byte[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let ok = line.split_whitespace().nth(1) == Some("200");
    if !ok {
        return Err(std::io::Error::other(format!("the entrance said {}", line.lines().next().unwrap_or_default())));
    }
    Ok(stream)
}

async fn once(ask: &Ask<'_>, url: &url::Url) -> Result<Answer, String> {
    let host = url.host_str().ok_or("no host")?.trim_start_matches('[').trim_end_matches(']').to_string();
    let https = match url.scheme() {
        "https" => true,
        "http" => false,
        other => return Err(format!("scheme {other}")),
    };
    let port = url.port_or_known_default().ok_or("no port")?;
    let tcp = match ask.via {
        Some(via) => connect_via(via, &host, port).await,
        None => TcpStream::connect((host.as_str(), port)).await,
    }
    .map_err(|error| error.to_string())?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Stream> = if https {
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|error| error.to_string())?;
        Box::new(tls().connect(name, tcp).await.map_err(|error| error.to_string())?)
    } else {
        Box::new(tcp)
    };
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let authority = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let mut request = Request::builder()
        .method(ask.method)
        .uri(path)
        .header("host", authority)
        .header("user-agent", concat!("DuoWave-VPN/", env!("CARGO_PKG_VERSION")))
        .header("accept", "*/*")
        .header("connection", "close");
    if ask.body.is_some() {
        request = request.header("content-type", "application/json");
    }
    for (name, value) in &ask.headers {
        request = request.header(*name, value);
    }
    let request = request
        .body(Full::new(Bytes::from(ask.body.clone().unwrap_or_default())))
        .map_err(|error| error.to_string())?;
    let response = sender.send_request(request).await.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let mut body = response.into_body();
    let mut collected = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| error.to_string())?;
        if let Some(data) = frame.data_ref() {
            if collected.len() + data.len() > ask.limit {
                return Err("the answer is too large".into());
            }
            collected.extend_from_slice(data);
        }
    }
    Ok(Answer { status, headers, body: collected })
}

/// The request, its redirects followed (three at most), within its time.
pub async fn send(ask: Ask<'_>) -> Result<Answer, String> {
    let work = async {
        let mut url = url::Url::parse(ask.url).map_err(|error| error.to_string())?;
        for _ in 0..4 {
            let answer = once(&ask, &url).await?;
            let location = answer.header("location");
            match (answer.status, location) {
                (301 | 302 | 303 | 307 | 308, Some(location)) if ask.method == "GET" => {
                    url = url.join(&location).map_err(|error| error.to_string())?;
                }
                _ => return Ok(answer),
            }
        }
        Err("too many redirects".to_string())
    };
    tokio::time::timeout(ask.timeout, work).await.map_err(|_| "timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A one-request HTTP server on this machine: it answers `answer` and gives back what it was asked.
    async fn serve(answer: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if seen.len() >= end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(answer.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&seen).into_owned()
        });
        (port, task)
    }

    #[tokio::test]
    async fn a_json_request_and_its_answer() {
        let (port, server) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 11\r\nprofile-title: T\r\n\r\n{\"ok\":true}").await;
        let url = format!("http://127.0.0.1:{port}/api?x=1");
        let answer = send(Ask::json("POST", &url, &serde_json::json!({"a": 1})).header("authorization", "Bearer t"))
            .await
            .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(answer.json().unwrap()["ok"], true);
        assert_eq!(answer.header("profile-title").as_deref(), Some("T"));
        let seen = server.await.unwrap();
        assert!(seen.starts_with("POST /api?x=1 HTTP/1.1\r\n"), "{seen}");
        assert!(seen.to_ascii_lowercase().contains("authorization: bearer t"));
        assert!(seen.ends_with("{\"a\":1}"));
    }

    #[tokio::test]
    async fn too_large_and_too_slow() {
        let (port, _server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789").await;
        let url = format!("http://127.0.0.1:{port}/");
        let mut ask = Ask::get(&url);
        ask.limit = 5;
        assert!(send(ask).await.is_err());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/", listener.local_addr().unwrap().port());
        assert_eq!(send(Ask::get(&url).timeout(Duration::from_millis(200))).await.unwrap_err(), "timed out");
    }
}
