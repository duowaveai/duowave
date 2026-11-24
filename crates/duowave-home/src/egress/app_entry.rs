//! The gateway's entry for the apps' programs (docs/plans/own-server.md, «Свой вход для программ приложений»): an HTTP
//! proxy on `127.0.0.1`, the address in the program's `HTTPS_PROXY` / `HTTP_PROXY` with the app's id and token as its
//! user and password. The gateway knows whose request it is and decides by that app's row: directly, through a
//! connection's entry, or a refusal in words. `CONNECT` (HTTPS) and plain HTTP in absolute form.
//!
//! The same entry refuses for the gateway's own calls whose connection does not answer: reqwest is pointed at it with
//! the user `down` (src/egress/mod.rs, `proxy_for`), and the call fails here, on this machine — nothing goes out.
//!
//! `tunnel` — a TCP stream through a connection's entry (`CONNECT host:port`): the entry's own `CONNECT` and Yandex's
//! WebSocket (src/gateway/yandex/yandex_session.rs) go through it.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request's head at most.
const MAX_HEAD: usize = 16 * 1024;
/// How long a client may take to send its head, and the gateway to reach where it goes.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The user of the gateway's own refused calls.
pub const DOWN: &str = "down";

/// Where a request goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Hop {
    Direct,
    Via(url::Url),
    /// Not at all: the words why.
    Refuse(String),
}

/// What the entry asks the gateway: `(user, password, host)` → where it goes. An unknown user — a refusal.
pub type Decide = Arc<dyn Fn(&str, &str, &str) -> Hop + Send + Sync>;
/// What the entry tells the gateway when a direct connection did not open: `(user, host, why)`.
pub type Failed = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Starts the entry on `127.0.0.1` and a free port; the port.
pub async fn start(decide: Decide, failed: Failed) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((client, peer)) = listener.accept().await else { continue };
            if !peer.ip().is_loopback() {
                continue;
            }
            let (decide, failed) = (decide.clone(), failed.clone());
            tokio::spawn(async move {
                if let Err(error) = serve(client, decide, failed).await {
                    tracing::debug!(target: "egress", "entry: {error}");
                }
            });
        }
    });
    Ok(port)
}

/// A request's head: its line, headers and the bytes after it.
struct Head {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
    rest: Vec<u8>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

async fn read_head(stream: &mut TcpStream) -> Result<Head, String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return Err("head too long".into());
        }
        let read = stream.read(&mut chunk).await.map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("closed before the head".into());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let text = std::str::from_utf8(&buffer[..end]).map_err(|_| "head not text")?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, target, version) = (first.next(), first.next(), first.next());
    let (Some(method), Some(target), Some(version)) = (method, target, version) else {
        return Err("bad request line".into());
    };
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    Ok(Head {
        method: method.to_string(),
        target: target.to_string(),
        version: version.to_string(),
        headers,
        rest: buffer[end + 4..].to_vec(),
    })
}

/// The user and password of a `Proxy-Authorization: Basic …`.
fn credentials(head: &Head) -> (String, String) {
    let decoded = head
        .header("Proxy-Authorization")
        .and_then(|value| value.strip_prefix("Basic ").or_else(|| value.strip_prefix("basic ")))
        .and_then(|value| B64.decode(value.trim()).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let (user, password) = decoded.split_once(':').unwrap_or((decoded.as_str(), ""));
    (percent(user), percent(password))
}

/// `%XX` decoded (a URL's user and password are percent-encoded).
fn percent(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        match (bytes[at], bytes.get(at + 1).copied().and_then(hex), bytes.get(at + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                at += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `host:port` of a CONNECT, or of an absolute URL with its path.
fn destination(head: &Head) -> Result<(String, u16, Option<String>), String> {
    if head.method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = head.target.rsplit_once(':').ok_or("CONNECT without a port")?;
        let port: u16 = port.parse().map_err(|_| "CONNECT port")?;
        return Ok((host.trim_start_matches('[').trim_end_matches(']').to_string(), port, None));
    }
    let url = url::Url::parse(&head.target).map_err(|_| "not an absolute URL")?;
    if url.scheme() != "http" {
        return Err("plain requests are http:// only".into());
    }
    let host = url.host_str().ok_or("no host")?.to_string();
    let mut path = url.path().to_string();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    Ok((host, url.port_or_known_default().unwrap_or(80), Some(path)))
}

async fn answer(client: &mut TcpStream, status: &str, text: &str) {
    let body = text.as_bytes();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = client.write_all(head.as_bytes()).await;
    let _ = client.write_all(body).await;
    let _ = client.shutdown().await;
}

async fn serve(mut client: TcpStream, decide: Decide, failed: Failed) -> Result<(), String> {
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await.map_err(|_| "head timeout")??;
    let (user, password) = credentials(&head);
    let (host, port, path) = match destination(&head) {
        Ok(found) => found,
        Err(error) => {
            answer(&mut client, "400 Bad Request", &error).await;
            return Err(error);
        }
    };
    let hop = decide(&user, &password, &host);
    let mut upstream = match &hop {
        Hop::Refuse(words) => {
            answer(&mut client, "503 Service Unavailable", words).await;
            return Ok(());
        }
        Hop::Direct => match tokio::time::timeout(CONNECT_TIMEOUT, crate::utils::net::dial(&host, port)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                let why = if error.kind() == std::io::ErrorKind::ConnectionReset { "reset" } else { "refused" };
                failed(&user, &host, why);
                answer(&mut client, "502 Bad Gateway", &error.to_string()).await;
                return Ok(());
            }
            Err(_) => {
                failed(&user, &host, "timeout");
                answer(&mut client, "504 Gateway Timeout", "timeout").await;
                return Ok(());
            }
        },
        Hop::Via(entry) => {
            if path.is_some() {
                // A plain request goes to the connection's entry as it came, with the entry's own credentials.
                match connect_entry(entry).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        answer(&mut client, "502 Bad Gateway", &error).await;
                        return Ok(());
                    }
                }
            } else {
                match tunnel(entry, &host, port).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        answer(&mut client, "502 Bad Gateway", &error).await;
                        return Ok(());
                    }
                }
            }
        }
    };
    match path {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .map_err(|error| error.to_string())?;
        }
        Some(path) => {
            // One request per connection: the next one may be for another host.
            let line = match &hop {
                Hop::Via(_) => format!("{} {} {}\r\n", head.method, head.target, head.version),
                _ => format!("{} {path} {}\r\n", head.method, head.version),
            };
            let mut out = line;
            for (key, value) in &head.headers {
                let lower = key.to_ascii_lowercase();
                if lower == "proxy-authorization" || lower == "proxy-connection" || lower == "connection" {
                    continue;
                }
                out.push_str(&format!("{key}: {value}\r\n"));
            }
            if let Hop::Via(entry) = &hop {
                if let Some(auth) = entry_auth(entry) {
                    out.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
                }
            }
            out.push_str("Connection: close\r\n\r\n");
            upstream.write_all(out.as_bytes()).await.map_err(|error| error.to_string())?;
        }
    }
    if !head.rest.is_empty() {
        upstream.write_all(&head.rest).await.map_err(|error| error.to_string())?;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await.map_err(|error| error.to_string())?;
    Ok(())
}

/// `Basic …` of a connection's entry, when its address carries a user.
fn entry_auth(entry: &url::Url) -> Option<String> {
    if entry.username().is_empty() {
        return None;
    }
    let user = percent(entry.username());
    let password = percent(entry.password().unwrap_or_default());
    Some(format!("Basic {}", B64.encode(format!("{user}:{password}"))))
}

async fn connect_entry(entry: &url::Url) -> Result<TcpStream, String> {
    let host = entry.host_str().ok_or("the entry has no host")?;
    let port = entry.port_or_known_default().ok_or("the entry has no port")?;
    tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| crate::tr!("VPN не отвечает: вход подключения молчит"))?
        .map_err(|_| crate::tr!("VPN не отвечает: вход подключения закрыт"))
}

/// A TCP stream to `host:port` through a connection's entry: `CONNECT`, its answer, then the stream as it is.
pub async fn tunnel(entry: &url::Url, host: &str, port: u16) -> Result<TcpStream, String> {
    let mut stream = connect_entry(entry).await?;
    let authority = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = entry_auth(entry) {
        request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.map_err(|error| error.to_string())?;
    // The answer's head byte by byte: whatever follows it is the destination's and stays in the stream.
    let mut head = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    let read = async {
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > MAX_HEAD {
                return Err("the entry's answer is too long".to_string());
            }
            if stream.read(&mut byte).await.map_err(|error| error.to_string())? == 0 {
                return Err(crate::tr!("VPN не отвечает: подключение закрылось"));
            }
            head.push(byte[0]);
        }
        Ok(())
    };
    tokio::time::timeout(CONNECT_TIMEOUT, read).await.map_err(|_| crate::tr!("VPN не отвечает: сервер молчит"))??;
    let status = String::from_utf8_lossy(&head);
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code != "200" {
        return Err(crate::tr!("VPN не открыл {authority}: {code}", authority = authority, code = code));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A tiny HTTP server: answers every request with its own name.
    async fn origin(name: &'static str) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let mut buffer = [0u8; 4096];
                    let _ = stream.read(&mut buffer).await;
                    let body = name.as_bytes();
                    let head =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        port
    }

    /// A connection's entry stand-in: a CONNECT proxy that counts what it carried and asks for its own password.
    async fn stand_in(carried: Arc<Mutex<Vec<String>>>) -> u16 {
        let decide: Decide =
            Arc::new(
                |user, password, _| {
                    if user == "vpn" && password == "secret" {
                        Hop::Direct
                    } else {
                        Hop::Refuse("who?".into())
                    }
                },
            );
        let carried_too = carried.clone();
        let failed: Failed = Arc::new(move |_, host, _| carried_too.lock().unwrap().push(format!("failed {host}")));
        let port = start(decide, failed).await.unwrap();
        carried.lock().unwrap().push("up".into());
        port
    }

    async fn get(entry: u16, user: &str, password: &str, port: u16) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", entry)).await.unwrap();
        let auth = B64.encode(format!("{user}:{password}"));
        let request = format!(
            "GET http://127.0.0.1:{port}/x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }

    #[tokio::test]
    async fn an_app_goes_by_its_row_directly_through_a_connection_or_not_at_all() {
        let origin = origin("hello").await;
        let carried = Arc::new(Mutex::new(Vec::new()));
        let vpn = stand_in(carried.clone()).await;
        let vpn_url = url::Url::parse(&format!("http://vpn:secret@127.0.0.1:{vpn}")).unwrap();
        let decide: Decide = Arc::new(move |user, password, _| match (user, password) {
            ("direct-app", "t1") => Hop::Direct,
            ("vpn-app", "t2") => Hop::Via(vpn_url.clone()),
            (DOWN, _) => Hop::Refuse("VPN не отвечает".into()),
            _ => Hop::Refuse("нет такого приложения".into()),
        });
        let failed: Failed = Arc::new(|_, _, _| {});
        let entry = start(decide, failed).await.unwrap();

        assert!(get(entry, "direct-app", "t1", origin).await.ends_with("hello"));
        assert!(get(entry, "vpn-app", "t2", origin).await.ends_with("hello"), "through the connection's entry");
        let refused = get(entry, DOWN, "x", origin).await;
        assert!(refused.starts_with("HTTP/1.1 503") && refused.ends_with("VPN не отвечает"), "{refused}");
        assert!(get(entry, "direct-app", "wrong", origin).await.starts_with("HTTP/1.1 503"));

        // CONNECT through the connection: the tunnel opens and carries bytes as they are.
        let vpn_url = url::Url::parse(&format!("http://vpn:secret@127.0.0.1:{vpn}")).unwrap();
        let mut stream = tunnel(&vpn_url, "127.0.0.1", origin).await.unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.ends_with("hello"));
        let wrong = url::Url::parse(&format!("http://vpn:nope@127.0.0.1:{vpn}")).unwrap();
        assert!(tunnel(&wrong, "127.0.0.1", origin).await.is_err(), "the entry's own password is checked");
    }

    #[tokio::test]
    async fn a_direct_connection_that_does_not_open_is_told() {
        let told = Arc::new(Mutex::new(Vec::new()));
        let told_too = told.clone();
        let decide: Decide = Arc::new(|_, _, _| Hop::Direct);
        let failed: Failed =
            Arc::new(move |user, host, why| told_too.lock().unwrap().push(format!("{user} {host} {why}")));
        let entry = start(decide, failed).await.unwrap();
        // A port nobody listens on.
        let closed = TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap().port();
        let answer = get(entry, "maps", "t", closed).await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
        assert_eq!(told.lock().unwrap().as_slice(), ["maps 127.0.0.1 refused".to_string()]);
    }
}
