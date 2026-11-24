//! The one place that speaks the MCP SDK (rmcp): a program on this computer over stdio (the `pc` build) or a server
//! by address over Streamable HTTP (an old SSE-only one as the fallback, mcp_legacy_sse.rs). The version is
//! negotiated: a 2026-07-28 server is asked `server/discover`, an older one gets the `initialize` handshake.
//! Everything else in the gateway sees only this interface, so a change in the SDK is a change here.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use futures::future::BoxFuture;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, InitializeRequestParams, ProtocolVersion,
    SubscriptionFilter,
};
use rmcp::service::{ClientInitializeError, NotificationContext, RunningService, RunningServiceCancellationToken};
use rmcp::transport::auth::AuthError;
use rmcp::transport::streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError};
use rmcp::transport::{DynamicTransportError, IntoTransport, StreamableHttpClientTransport};
use rmcp::{ClientHandler, ClientLifecycleMode, Peer, RoleClient, ServiceError};
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

use super::mcp_legacy_sse::{LegacySseTransport, SseError};
use super::mcp_oauth::OAuthLink;
use super::mcp_presets::TransportKind;
use super::mcp_tools::{ListedTool, ToolCallResult};

const CLIENT_NAME: &str = "duowave";
const CONNECT: Duration = Duration::from_secs(20);

/// How a server by address is signed in to: nothing, a token (read when connecting), or the sealed OAuth tokens.
#[derive(Clone)]
pub enum ConnectionAuth {
    None,
    Token(Arc<dyn Fn() -> Option<String> + Send + Sync>),
    OAuth(OAuthLink),
}

pub struct ConnectionSpec {
    pub id: String,
    pub transport: TransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    /// The program's own variables (secrets among them), on top of a safe set: never the gateway's .env.
    pub env: BTreeMap<String, String>,
    /// Where the program runs: its own folder, not the gateway's (where .env lies).
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub auth: ConnectionAuth,
}

/// The server's name and version as it says them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerVersion {
    pub name: String,
    pub version: String,
}

/// What a connection tells its owner after it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionEvent {
    /// The server changed its tools (list_changed): read them again.
    ToolsChanged,
    /// The connection broke on its own (the program exited, the stream closed).
    Closed,
}

/// Hears a program's stderr, line by line.
pub type LogSink = Arc<dyn Fn(String) + Send + Sync>;

/// Why a connection did not open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The server needs a sign-in (OAuth).
    #[error("needs a sign-in")]
    NeedsLogin,
    #[error("the server refused the token (401)")]
    Refused,
    #[error("command not found: {0}")]
    NotFound(String),
    #[error("the address does not answer: {0}")]
    Unreachable(String),
    #[error("timed out")]
    Timeout,
    #[error("connection closed: {0}")]
    Closed(String),
    #[error("{0}")]
    Other(String),
}

/// Why a request to an open connection failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    #[error("cancelled")]
    Cancelled,
    #[error("request timed out")]
    Timeout,
    #[error("connection closed")]
    Closed,
    #[error("{0}")]
    Failed(String),
}

impl From<ServiceError> for CallError {
    fn from(error: ServiceError) -> CallError {
        match error {
            ServiceError::McpError(data) => CallError::Failed(format!("MCP error {}: {}", data.code.0, data.message)),
            ServiceError::TransportClosed | ServiceError::TransportSend(_) => CallError::Closed,
            ServiceError::Timeout { .. } => CallError::Timeout,
            other => CallError::Failed(other.to_string()),
        }
    }
}

/// The client side of the session: our name, and the server's word that its tools changed.
struct Hooks {
    events: mpsc::UnboundedSender<ConnectionEvent>,
}

impl ClientHandler for Hooks {
    fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) -> impl Future<Output = ()> + Send + '_ {
        let _ = self.events.send(ConnectionEvent::ToolsChanged);
        std::future::ready(())
    }

    fn get_info(&self) -> ClientConfig {
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION")),
        )
    }
}

/// `server/discover` first; a server that does not know it gets the `initialize` handshake.
pub fn negotiated() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    }
}

type Running = RunningService<RoleClient, Hooks>;

async fn handshake<T, E, A>(
    transport: T,
    lifecycle: ClientLifecycleMode,
    events: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<Running, Failure>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let hooks = Hooks { events: events.clone() };
    match tokio::time::timeout(CONNECT, rmcp::serve_client_with_lifecycle(hooks, transport, lifecycle)).await {
        Ok(Ok(running)) => Ok(running),
        Ok(Err(error)) => Err(failure(&error)),
        Err(_) => Err(Failure::Other(ConnectError::Timeout)),
    }
}

/// A failed handshake: the server asked for a sign-in (with its challenge), or anything else.
enum Failure {
    Auth(Option<String>),
    Other(ConnectError),
}

fn failure(error: &ClientInitializeError) -> Failure {
    if error.is_authorization_required()
        || chain_has(error, |e| matches!(e.downcast_ref(), Some(SseError::Unauthorized)))
    {
        return Failure::Auth(error.auth_challenge().map(str::to_string));
    }
    Failure::Other(classify(error))
}

type Cause<'a> = &'a (dyn std::error::Error + 'static);

/// An error and everything under it: the transport's error and the HTTP client's own among them (rmcp keeps both
/// out of the source chain).
fn causes(error: Cause<'_>) -> Vec<Cause<'_>> {
    let mut out = Vec::new();
    let mut current = Some(error);
    while let Some(error) = current {
        out.push(error);
        current = if let Some(ClientInitializeError::TransportError { error, .. }) = error.downcast_ref() {
            Some(error as Cause<'_>)
        } else if let Some(StreamableHttpError::Client(inner)) =
            error.downcast_ref::<StreamableHttpError<reqwest::Error>>()
        {
            Some(inner as Cause<'_>)
        } else {
            error.source()
        };
    }
    out
}

fn chain_has(error: Cause<'_>, test: impl Fn(Cause<'_>) -> bool) -> bool {
    causes(error).into_iter().any(test)
}

/// A handshake's failure as the owner can act on it.
fn classify(error: &ClientInitializeError) -> ConnectError {
    if let ClientInitializeError::LegacyFallbackFailed { fallback, .. } = error {
        return classify(fallback);
    }
    if let ClientInitializeError::ConnectionClosed(detail) = error {
        return ConnectError::Closed(detail.clone());
    }
    let unreachable = chain_has(error, |e| {
        e.downcast_ref::<reqwest::Error>().is_some_and(reqwest::Error::is_connect)
            || e.downcast_ref::<std::io::Error>().is_some_and(|io| {
                matches!(
                    io.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::AddrNotAvailable
                        | std::io::ErrorKind::HostUnreachable
                        | std::io::ErrorKind::NetworkUnreachable
                )
            })
    });
    if unreachable {
        return ConnectError::Unreachable(describe(error));
    }
    if chain_has(error, |e| e.downcast_ref::<reqwest::Error>().is_some_and(reqwest::Error::is_timeout)) {
        return ConnectError::Timeout;
    }
    if chain_has(error, |e| {
        e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    }) {
        return ConnectError::Closed(describe(error));
    }
    ConnectError::Other(describe(error))
}

/// An error with its causes, for the card and the log: from the first cause below the SDK's own wrappers (their
/// text names Rust types), each cause once.
fn describe(error: Cause<'_>) -> String {
    let all = causes(error);
    let wrapper = |e: &Cause<'_>| e.is::<ClientInitializeError>() || e.is::<DynamicTransportError>();
    let inner: Vec<Cause<'_>> = all.iter().copied().skip_while(wrapper).collect();
    let chosen = if inner.is_empty() { all } else { inner };
    let mut text = String::new();
    for cause in chosen {
        let more = cause.to_string();
        if text.contains(&more) {
            continue;
        }
        if !text.is_empty() {
            text.push_str(": ");
        }
        text.push_str(&more);
    }
    text
}

/// An open connection to a server.
pub struct McpConnection {
    peer: Peer<RoleClient>,
    closing: Arc<AtomicBool>,
    stop: Mutex<Option<RunningServiceCancellationToken>>,
    watcher: Mutex<Option<JoinHandle<()>>>,
    listener: Option<AbortHandle>,
    program: Mutex<Option<Box<dyn RunningProgram>>>,
}

impl McpConnection {
    fn new(
        running: Running,
        events: mpsc::UnboundedSender<ConnectionEvent>,
        program: Option<Box<dyn RunningProgram>>,
    ) -> McpConnection {
        let peer = running.peer().clone();
        let closing = Arc::new(AtomicBool::new(false));
        let stop = running.cancellation_token();
        let listener = modern_listener(&peer, &events);
        let watcher = {
            let closing = closing.clone();
            tokio::spawn(async move {
                let _ = running.waiting().await;
                if !closing.load(Ordering::SeqCst) {
                    let _ = events.send(ConnectionEvent::Closed);
                }
            })
        };
        McpConnection {
            peer,
            closing,
            stop: Mutex::new(Some(stop)),
            watcher: Mutex::new(Some(watcher)),
            listener,
            program: Mutex::new(program),
        }
    }

    /// How the server says to use it (`instructions` of the handshake or of `server/discover`).
    pub fn instructions(&self) -> Option<String> {
        self.peer.peer_info().and_then(|info| info.instructions.clone()).filter(|text| !text.is_empty())
    }

    pub fn server(&self) -> Option<ServerVersion> {
        let info = self.peer.peer_info()?;
        let server = info.server_info.as_ref()?;
        Some(ServerVersion { name: server.name.clone(), version: server.version.clone() })
    }

    /// Every tool the server lists, all pages.
    pub async fn tools(&self) -> Result<Vec<ListedTool>, CallError> {
        let tools = tokio::time::timeout(CONNECT, self.peer.list_all_tools())
            .await
            .map_err(|_| CallError::Timeout)?
            .map_err(CallError::from)?;
        Ok(tools
            .into_iter()
            .filter_map(|tool| serde_json::to_value(tool).ok().and_then(ListedTool::from_json))
            .collect())
    }

    /// Runs a tool; `cancel` and `wait` end it early (the server is told the call is cancelled).
    pub async fn call(
        &self,
        name: &str,
        arguments: Map<String, Value>,
        cancel: &CancellationToken,
        wait: Duration,
    ) -> Result<ToolCallResult, CallError> {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(arguments);
        tokio::select! {
            _ = cancel.cancelled() => Err(CallError::Cancelled),
            result = tokio::time::timeout(wait, self.peer.call_tool(params)) => match result {
                Err(_) => Err(CallError::Timeout),
                Ok(Err(error)) => Err(CallError::from(error)),
                Ok(Ok(result)) => Ok(ToolCallResult::from_json(&serde_json::to_value(&result).unwrap_or_default())),
            },
        }
    }

    /// Closes the connection; a program is stopped with everything it started.
    pub async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(listener) = &self.listener {
            listener.abort();
        }
        let stop = self.stop.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(stop) = stop {
            stop.cancel();
        }
        let watcher = self.watcher.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(watcher) = watcher {
            let _ = tokio::time::timeout(Duration::from_secs(5), watcher).await;
        }
        let program = self.program.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(program) = program {
            program.stop().await;
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        // Never left running: the session ends (a program is killed with its handle).
        self.closing.store(true, Ordering::SeqCst);
        if let Some(listener) = &self.listener {
            listener.abort();
        }
        if let Some(stop) = self.stop.get_mut().unwrap_or_else(|p| p.into_inner()).take() {
            stop.cancel();
        }
    }
}

/// A 2026-07-28 server says its tools changed only on a subscription: one is opened when it says it will.
fn modern_listener(peer: &Peer<RoleClient>, events: &mpsc::UnboundedSender<ConnectionEvent>) -> Option<AbortHandle> {
    let info = peer.peer_info()?;
    if info.protocol_version.has_initialize() {
        return None;
    }
    let tools = info.capabilities.tools.as_ref()?;
    if tools.list_changed != Some(true) {
        return None;
    }
    let peer = peer.clone();
    let events = events.clone();
    let task = tokio::spawn(async move {
        let filter = SubscriptionFilter::builder().tools_list_changed().build();
        let Ok(mut subscription) = peer.listen(filter).await else { return };
        while let Ok(Some(_)) = subscription.next().await {
            if events.send(ConnectionEvent::ToolsChanged).is_err() {
                break;
            }
        }
    });
    Some(task.abort_handle())
}

/// Opens a connection. A server that needs a sign-in answers NeedsLogin; `log` hears a program's stderr.
pub async fn open_connection(
    spec: &ConnectionSpec,
    log: LogSink,
) -> Result<(McpConnection, mpsc::UnboundedReceiver<ConnectionEvent>), ConnectError> {
    match spec.transport {
        TransportKind::Stdio => open_program(spec, log).await,
        TransportKind::Http => open_address(spec).await,
    }
}

/// Request headers from names and values; a name or value HTTP does not allow, or one the transport sets itself,
/// is left out.
fn header_map(headers: &[(String, String)]) -> HeaderMap {
    const OWN: &[&str] = &["accept", "mcp-session-id", "last-event-id", "mcp-protocol-version", "content-type"];
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if OWN.contains(&name.to_ascii_lowercase().as_str()) {
            tracing::debug!(target: "mcp", "header {name} is the transport's own: not sent");
            continue;
        }
        match (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
            (Ok(name), Ok(value)) => {
                map.insert(name, value);
            }
            _ => tracing::warn!(target: "mcp", "header {name}: not a valid HTTP header, not sent"),
        }
    }
    map
}

async fn open_address(
    spec: &ConnectionSpec,
) -> Result<(McpConnection, mpsc::UnboundedReceiver<ConnectionEvent>), ConnectError> {
    let url =
        spec.url.as_deref().filter(|url| !url.is_empty()).ok_or_else(|| ConnectError::Other("no address".into()))?;
    let oauth = matches!(spec.auth, ConnectionAuth::OAuth(_));
    let headers = header_map(&spec.headers);
    let (events, receiver) = mpsc::unbounded_channel();
    match streamable(url, headers.clone(), &spec.auth, &events).await {
        Ok(running) => Ok((connection(running, events), receiver)),
        Err(Failure::Auth(_)) => Err(if oauth { ConnectError::NeedsLogin } else { ConnectError::Refused }),
        Err(Failure::Other(error)) => {
            // A server from before Streamable HTTP answers only the old SSE transport.
            match legacy(url, headers, &spec.auth, &events).await {
                Ok(running) => Ok((connection(running, events), receiver)),
                Err(Failure::Auth(_)) if oauth => Err(ConnectError::NeedsLogin),
                Err(_) => Err(error),
            }
        }
    }
}

fn connection(running: Running, events: mpsc::UnboundedSender<ConnectionEvent>) -> McpConnection {
    McpConnection::new(running, events, None)
}

async fn streamable(
    url: &str,
    headers: HeaderMap,
    auth: &ConnectionAuth,
    events: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<Running, Failure> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(headers.into_iter().filter_map(|(name, value)| Some((name?, value))).collect());
    match auth {
        ConnectionAuth::None => {
            handshake(
                StreamableHttpClientTransport::with_client(crate::utils::http::client(), config),
                negotiated(),
                events,
            )
            .await
        }
        ConnectionAuth::Token(token) => {
            if let Some(token) = token().filter(|token| !token.is_empty()) {
                config = config.auth_header(token);
            }
            handshake(
                StreamableHttpClientTransport::with_client(crate::utils::http::client(), config),
                negotiated(),
                events,
            )
            .await
        }
        ConnectionAuth::OAuth(link) => {
            let client = link.client(url).await.map_err(oauth_failure)?;
            handshake(StreamableHttpClientTransport::with_client(client, config), negotiated(), events).await
        }
    }
}

fn oauth_failure(error: AuthError) -> Failure {
    match error {
        AuthError::AuthorizationRequired => Failure::Auth(None),
        other => Failure::Other(ConnectError::Other(describe(&other))),
    }
}

async fn legacy(
    url: &str,
    mut headers: HeaderMap,
    auth: &ConnectionAuth,
    events: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<Running, Failure> {
    let token = match auth {
        ConnectionAuth::None => None,
        ConnectionAuth::Token(token) => token(),
        ConnectionAuth::OAuth(link) => Some(link.access_token(url).await.map_err(oauth_failure)?),
    };
    if let Some(value) =
        token.filter(|token| !token.is_empty()).and_then(|t| HeaderValue::from_str(&format!("Bearer {t}")).ok())
    {
        headers.insert(axum::http::header::AUTHORIZATION, value);
    }
    let transport =
        match tokio::time::timeout(CONNECT, LegacySseTransport::connect(crate::utils::http::client(), url, headers))
            .await
        {
            Ok(Ok(transport)) => transport,
            Ok(Err(SseError::Unauthorized)) => return Err(Failure::Auth(None)),
            Ok(Err(error)) => return Err(Failure::Other(ConnectError::Other(error.to_string()))),
            Err(_) => return Err(Failure::Other(ConnectError::Timeout)),
        };
    // The old transport is of the 2024 protocol: its servers know only `initialize`.
    handshake(transport, ClientLifecycleMode::Initialize, events).await
}

/// What a sign-in found at the server: it answers already (the tokens still work), or it asks for a sign-in.
pub enum Probe {
    Connected,
    Challenge(Option<String>),
}

/// Asks a server by address whether it wants a sign-in, with the sealed tokens when there are any.
pub async fn probe_login(url: &str, link: Option<&OAuthLink>) -> Result<Probe, ConnectError> {
    let (events, _receiver) = mpsc::unbounded_channel();
    let auth = match link {
        Some(link) => ConnectionAuth::OAuth(link.clone()),
        None => ConnectionAuth::None,
    };
    match streamable(url, HeaderMap::new(), &auth, &events).await {
        Ok(mut running) => {
            let _ = running.close().await;
            Ok(Probe::Connected)
        }
        Err(Failure::Auth(challenge)) => Ok(Probe::Challenge(challenge)),
        Err(Failure::Other(error)) => Err(error),
    }
}

/// A program's own folder under the gateway's state: `<STATE_DIR>/mcp/<id>`.
pub fn program_dir(state_dir: Option<&std::path::Path>, id: &str) -> Option<PathBuf> {
    let dir = state_dir?;
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(dir)).unwrap_or_else(|_| dir.to_path_buf())
    };
    Some(absolute.join("mcp").join(id))
}

/// A program an MCP server runs as on this computer (crates/duowave-pc, mcp/programs.rs): its connection stops it.
pub trait RunningProgram: Send {
    /// It ends on its own when its input closes; whatever is left of it is stopped.
    fn stop(self: Box<Self>) -> BoxFuture<'static, ()>;
}

/// A connection just opened, and its events.
pub type Opened = Result<(McpConnection, mpsc::UnboundedReceiver<ConnectionEvent>), ConnectError>;

/// What runs MCP servers as programs on this computer — npx, uvx and the like (crates/duowave-pc). The station's build
/// has none: only servers by address.
pub trait Programs: Send + Sync {
    fn open<'a>(&'a self, spec: &'a ConnectionSpec, log: LogSink) -> BoxFuture<'a, Opened>;
}

static PROGRAMS: OnceLock<Arc<dyn Programs>> = OnceLock::new();

/// The computer's runner of programs, said at its start (`duowave_pc::install`).
pub fn set_programs(programs: Arc<dyn Programs>) {
    let _ = PROGRAMS.set(programs);
}

/// Whether MCP servers run as programs here: on a computer.
pub fn programs_here() -> bool {
    PROGRAMS.get().is_some()
}

async fn open_program(spec: &ConnectionSpec, log: LogSink) -> Opened {
    match PROGRAMS.get() {
        Some(programs) => programs.open(spec, log).await,
        None => Err(ConnectError::Other("programs run only on a computer: this build has servers by address".into())),
    }
}

/// A program's connection over its pipes (the runner's): the handshake, then the connection that stops the program at
/// its end; a failed handshake stops it at once.
pub async fn over<T, E, A>(transport: T, lifecycle: ClientLifecycleMode, program: Box<dyn RunningProgram>) -> Opened
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let (events, receiver) = mpsc::unbounded_channel();
    match handshake(transport, lifecycle, &events).await {
        Ok(running) => Ok((McpConnection::new(running, events, Some(program)), receiver)),
        Err(failure) => {
            program.stop().await;
            Err(match failure {
                Failure::Auth(_) => ConnectError::Refused,
                Failure::Other(error) => error,
            })
        }
    }
}
