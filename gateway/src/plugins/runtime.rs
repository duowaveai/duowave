//! The engine a plugin's code runs in, behind an interface: QuickJS now (quickjs.rs); WASM and processes later stand
//! beside it without changes to the host. An engine starts a plugin's code in a context of its own with its budget,
//! calls the functions it exports with JSON in and out, and keeps every call within its time.
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{Map, Value};

/// How long a call may run. A voice request and the model's tool wait for the answer; work in the background does
/// not, but it ends too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Voice,
    Tool,
    Background,
}

impl CallKind {
    pub fn limit(self) -> Duration {
        match self {
            CallKind::Voice | CallKind::Tool => Duration::from_secs(2),
            CallKind::Background => Duration::from_secs(30),
        }
    }
}

/// Loading a plugin's code: its top level runs, and then it must be ready for a voice request.
pub const LOAD_LIMIT: Duration = Duration::from_secs(2);

/// Why a call or a start failed. The gateway goes on either way; the panel shows the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    /// The code did not load: a syntax error, its top level threw.
    Load(String),
    Timeout(u64),
    Memory(u64),
    /// It threw, or a Host API call it made was refused and it did not catch that.
    Script(String),
    Missing(String),
    /// Its answer is not one the gateway takes (too big, not JSON).
    Answer(String),
    /// Off, unloaded, or the safe mode is on.
    Unavailable(String),
    /// Its context did not load: the station's memory has no room for its budget now, even with the idle contexts
    /// unloaded (budget.rs). Nothing of it ran; the next call tries again.
    NoRoom(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Load(why) => f.write_str(&crate::tr!("код не загрузился: {why}", why = why)),
            PluginError::Timeout(ms) => f.write_str(&crate::tr!("не уложился в {ms} мс", ms = ms)),
            PluginError::Memory(mb) => f.write_str(&crate::tr!("превысил свою память ({mb} МБ)", mb = mb)),
            PluginError::Missing(name) => f.write_str(&crate::tr!("нет функции {name} в backend.js", name = name)),
            PluginError::Script(text)
            | PluginError::Answer(text)
            | PluginError::Unavailable(text)
            | PluginError::NoRoom(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for PluginError {}

impl PluginError {
    /// A failure after which the context is not to be trusted: it starts afresh on the next call.
    pub fn poisons(&self) -> bool {
        matches!(self, PluginError::Timeout(_) | PluginError::Memory(_))
    }
}

/// A plugin's code as an engine starts it.
#[derive(Debug, Clone, Default)]
pub struct PluginCode {
    pub id: String,
    pub version: String,
    /// `backend.js`: an ES module whose exports are the plugin's functions.
    pub source: String,
    /// `backend.wasm`: a WebAssembly module (`runtime: wasm`).
    pub bytes: Vec<u8>,
    /// `runtime: mcp`: the MCP server's address.
    pub endpoint: Option<String>,
    /// Its `net:` hosts: an engine that reaches the network checks against them.
    pub hosts: Vec<String>,
    /// Its manifest and its folder: an app's program is found and checked there (`runtime: process`).
    pub manifest: Option<Arc<super::manifest::Manifest>>,
    pub dir: Option<std::path::PathBuf>,
    /// The memory limit of its context.
    pub memory_mb: u64,
}

/// Memory as the engine counts it, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    /// Allocated by the engine for this plugin, its own overhead included.
    pub allocated: u64,
    /// In use by its objects, strings and code.
    pub used: u64,
    pub limit: u64,
}

/// A plugin's code, started.
pub trait PluginInstance: Send + Sync {
    /// Calls an exported function with one JSON argument; its answer as JSON (a promise awaited).
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>>;
    /// Memory now; none once stopped.
    fn memory(&self) -> BoxFuture<'static, Option<MemoryStats>>;
    /// The functions it exports.
    fn exports(&self) -> Vec<String>;
    /// Frees its context; calls after this fail.
    fn stop(&self);
    /// An app's own sound, while it runs: the base of its `/stream/…` on localhost (`http://127.0.0.1:<port>/stream/`),
    /// given on to the station by our /media. None for a plugin.
    fn streams(&self) -> Option<String> {
        None
    }
}

/// What a plugin's code may ask of the gateway (Host API v1), each call checked against its rights by the host.
pub trait HostApi: Send + Sync {
    /// `http.fetch`: only to the hosts of its `net:` rights, with time and size limits; the answer or why not.
    fn fetch(&self, request: Value) -> BoxFuture<'static, Result<Value, String>>;
    /// `kv`: its own store; refused without the right.
    fn kv_get(&self, key: &str) -> Result<Option<Value>, String>;
    fn kv_set(&self, key: &str, value: Value) -> Result<(), String>;
    fn kv_delete(&self, key: &str) -> Result<(), String>;
    fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String>;
    /// `voice.say`: speech in Alice's voice on a station (`device`, any when none) without a request; whether it
    /// was said.
    fn say(&self, text: String, device: Option<String>) -> BoxFuture<'static, Result<bool, String>>;
    /// `settings`: the values of the settings its manifest declares.
    fn settings(&self) -> Map<String, Value>;
    /// `console` and `host.log`: into the gateway's log under the plugin's name.
    fn log(&self, level: &str, message: &str);
    /// `host.i18n.locale` (docs/plans/i18n.md): the household's language now — read on every call, a change in
    /// «Настройки» applies at once.
    fn locale(&self) -> String {
        crate::locale::text::household_now().unwrap_or_else(|| crate::locale::FALLBACK.to_string())
    }
    /// `host.i18n.languages`: the languages its manifest says it speaks, its own first.
    fn languages(&self) -> Vec<String> {
        vec![crate::locale::SOURCE.to_string()]
    }
    /// `host.call(name, input)` (Host API 1.1): the gateway's services by name, each behind its right —
    /// `content.search`, `tools.call`, `smartHome.devices`, `smartHome.act`, `weather.place`, `news.feeds`.
    fn call(&self, name: &str, _input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let name = name.to_string();
        Box::pin(async move { Err(crate::tr!("нет функции {name}", name = name)) })
    }
}

/// The engine for a manifest's `runtime`, the code read for it: QuickJS (`js`, the given one), wasmi (`wasm`), an
/// MCP server (`mcp`). An app's program is the apps' supervisor's (stage 9), not an engine of the plugin host.
pub fn engine_for(
    manifest: &super::manifest::Manifest,
    package: &super::registry::Package,
    js: &Arc<dyn PluginRuntime>,
    process: &Arc<dyn PluginRuntime>,
) -> Result<(Arc<dyn PluginRuntime>, PluginCode), PluginError> {
    use super::manifest::Runtime;
    let mut code = PluginCode {
        id: manifest.id.clone(),
        version: manifest.version.to_string(),
        memory_mb: manifest.budget.memory_mb,
        hosts: manifest.hosts(),
        endpoint: manifest.mcp_url.clone(),
        manifest: Some(package.manifest.clone()),
        dir: Some(package.dir.clone()),
        ..PluginCode::default()
    };
    match manifest.runtime {
        Runtime::Js => {
            code.source = package.source().map_err(PluginError::Load)?;
            Ok((js.clone(), code))
        }
        #[cfg(feature = "wasm")]
        Runtime::Wasm => {
            code.bytes = package.wasm().map_err(PluginError::Load)?;
            Ok((Arc::new(super::wasm::Wasm), code))
        }
        #[cfg(not(feature = "wasm"))]
        Runtime::Wasm => Err(PluginError::Load(crate::tr!("в этой сборке шлюза нет WebAssembly (фича wasm)"))),
        Runtime::Mcp => Ok((Arc::new(super::mcp_runtime::McpRuntime), code)),
        // An app: its program under the supervisor (src/apps).
        Runtime::Process => Ok((process.clone(), code)),
    }
}

/// An engine.
pub trait PluginRuntime: Send + Sync {
    /// The manifest's `runtime` it runs: `js`.
    fn name(&self) -> &'static str;
    /// Starts the code in a context of its own with its memory limit; the start itself is limited to `LOAD_LIMIT`.
    fn start(
        &self,
        code: PluginCode,
        host: Arc<dyn HostApi>,
    ) -> BoxFuture<'static, Result<Arc<dyn PluginInstance>, PluginError>>;
}
