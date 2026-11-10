//! QuickJS (rquickjs) as the plugins' engine. Each started plugin gets a thread of its own with a current-thread
//! tokio runtime, and its QuickJS runtime and context on that thread (`AsyncRuntime`/`AsyncContext`, without
//! rquickjs's "parallel"). Why a thread per runtime rather than QuickJS on the gateway's own workers:
//!
//! - code that runs away (`while (true) {}`) holds its own thread until the interrupt handler stops it at the
//!   call's deadline; the gateway's workers, the voice among them, never wait on a plugin;
//! - QuickJS is not thread-safe, and here it never crosses threads: no `unsafe impl Send` anywhere;
//! - `await host.http.fetch(…)` still works: the request runs on the gateway's runtime with its HTTP client
//!   (`HostApi::fetch`), the plugin's thread only awaits the answer;
//! - stopping a plugin is closing its channel: the thread ends and drops the context, freeing all of its memory.
//!
//! A thread costs its stack (a megabyte reserved, little of it touched) only while the plugin is loaded: the host
//! loads a plugin at its first call and unloads it after a while without one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use rquickjs::prelude::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Ctx, Function, Module, Object, Persistent};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::runtime::{
    CallKind, HostApi, MemoryStats, PluginCode, PluginError, PluginInstance, PluginRuntime, LOAD_LIMIT,
};

/// The plugin thread's own stack; QuickJS's stack for scripts is a quarter of it.
const THREAD_STACK: usize = 1024 * 1024;
const SCRIPT_STACK: usize = 256 * 1024;
/// How many calls may wait for a busy plugin.
const QUEUE: usize = 16;
/// An answer bigger than this, as JSON, is not taken.
const MAX_ANSWER: usize = 256 * 1024;
/// How much longer than its limit the host waits for a thread before it gives up on it.
const GRACE: Duration = Duration::from_millis(500);

/// The Host API as plugins see it (`host.*`, `console.*`), over the raw functions the thread puts on
/// `globalThis.__duowave`: those speak JSON strings and never throw; this layer throws what they refuse.
const PRELUDE: &str = r#"
(() => {
  const raw = globalThis.__duowave;
  delete globalThis.__duowave;
  const unwrap = (text) => {
    const answer = JSON.parse(text);
    if (answer.error !== undefined) throw new Error(answer.error);
    return answer.value;
  };
  const say = (level) => (...parts) =>
    raw.log(level, parts.map((part) => (typeof part === "string" ? part : JSON.stringify(part))).join(" "));
  const http = Object.freeze({
    async fetch(url, options = {}) {
      const body = options.body === undefined || typeof options.body === "string" ? options.body : JSON.stringify(options.body);
      const answer = unwrap(await raw.fetch(JSON.stringify({
        url: String(url), method: options.method, headers: options.headers, body, timeoutMs: options.timeoutMs,
      })));
      return Object.freeze({
        status: answer.status,
        ok: answer.status >= 200 && answer.status < 300,
        url: answer.url,
        headers: Object.freeze(answer.headers),
        text: async () => answer.body,
        json: async () => JSON.parse(answer.body),
      });
    },
  });
  const kv = Object.freeze({
    get: (key) => unwrap(raw.kvGet(String(key))),
    set: (key, value) => { unwrap(raw.kvSet(String(key), JSON.stringify(value === undefined ? null : value))); },
    delete: (key) => { unwrap(raw.kvDelete(String(key))); },
    keys: (prefix = "") => unwrap(raw.kvKeys(String(prefix))),
  });
  const voice = Object.freeze({
    async say(text, options = {}) {
      return unwrap(await raw.say(JSON.stringify({ text: String(text), device: options.device })));
    },
  });
  const settings = Object.freeze({
    get: (key) => {
      const all = JSON.parse(raw.settings());
      return key === undefined ? all : all[key];
    },
  });
  const log = say("info");
  // Host API 1.1: the gateway's services, each behind its right in the manifest.
  const call = async (name, input = {}) => unwrap(await raw.call(name, JSON.stringify(input ?? {})));
  const content = Object.freeze({
    search: (folder, query, limit) => call("content.search", { folder, query, limit }),
  });
  const tools = Object.freeze({ call: (name, input = {}) => call("tools.call", { name, input }) });
  const smartHome = Object.freeze({
    devices: () => call("smartHome.devices"),
    act: (device, action, value) => call("smartHome.act", { device, action, value }),
  });
  const home = Object.freeze({
    weatherPlace: () => call("weather.place"),
    newsFeeds: () => call("news.feeds"),
  });
  globalThis.host = Object.freeze({
    api: 1.1,
    plugin: Object.freeze({ id: raw.id, version: raw.version }),
    http, kv, voice, settings, log, call, content, tools, smartHome, home,
  });
  globalThis.console = Object.freeze({ log, info: log, debug: log, warn: say("warn"), error: say("error") });
})();
"#;

pub struct QuickJs;

impl PluginRuntime for QuickJs {
    fn name(&self) -> &'static str {
        "js"
    }

    fn start(
        &self,
        code: PluginCode,
        host: Arc<dyn HostApi>,
    ) -> BoxFuture<'static, Result<Arc<dyn PluginInstance>, PluginError>> {
        Box::pin(async move {
            let (jobs, receiver) = mpsc::channel::<Job>(QUEUE);
            let (ready, started) = oneshot::channel();
            std::thread::Builder::new()
                .name(format!("plugin-{}", code.id))
                .stack_size(THREAD_STACK)
                .spawn(move || run(code, host, receiver, ready))
                .map_err(|error| PluginError::Load(format!("поток не запустился: {error}")))?;
            // A load that hangs gives the thread up: its channel goes with this future, and the thread ends once
            // it gets past the load.
            let exports = match tokio::time::timeout(LOAD_LIMIT + GRACE, started).await {
                Ok(Ok(loaded)) => loaded?,
                Ok(Err(_)) => return Err(PluginError::Load("поток плагина завершился".into())),
                Err(_) => return Err(PluginError::Timeout(LOAD_LIMIT.as_millis() as u64)),
            };
            Ok(Arc::new(Instance { jobs: Mutex::new(Some(jobs)), exports }) as Arc<dyn PluginInstance>)
        })
    }
}

enum Job {
    Call {
        function: String,
        input: Value,
        kind: CallKind,
        deadline: Instant,
        reply: oneshot::Sender<Result<Value, PluginError>>,
    },
    Memory {
        reply: oneshot::Sender<MemoryStats>,
    },
}

struct Instance {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    exports: Vec<String>,
}

impl Instance {
    fn sender(&self) -> Option<mpsc::Sender<Job>> {
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

fn stopped() -> PluginError {
    PluginError::Unavailable("плагин выгружен".into())
}

impl PluginInstance for Instance {
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>> {
        let sender = self.sender();
        let function = function.to_string();
        Box::pin(async move {
            let sender = sender.ok_or_else(stopped)?;
            let limit = kind.limit();
            // The limit counts from the call, a wait behind a busy plugin included.
            let deadline = Instant::now() + limit;
            let (reply, answer) = oneshot::channel();
            let job = Job::Call { function, input, kind, deadline, reply };
            let timeout = || PluginError::Timeout(limit.as_millis() as u64);
            match tokio::time::timeout_at(deadline.into(), sender.send(job)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(stopped()),
                Err(_) => return Err(timeout()),
            }
            match tokio::time::timeout_at((deadline + GRACE).into(), answer).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(stopped()),
                Err(_) => Err(timeout()),
            }
        })
    }

    fn memory(&self) -> BoxFuture<'static, Option<MemoryStats>> {
        let sender = self.sender();
        Box::pin(async move {
            let (reply, answer) = oneshot::channel();
            sender?.send(Job::Memory { reply }).await.ok()?;
            tokio::time::timeout(Duration::from_secs(2), answer).await.ok()?.ok()
        })
    }

    fn exports(&self) -> Vec<String> {
        self.exports.clone()
    }

    fn stop(&self) {
        // The thread ends once the calls it has taken are done: its channel is closed.
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The first line of what the script threw, short: for people. The rest goes to the log.
fn message(error: &CaughtError<'_>) -> String {
    let text = error.to_string();
    let first = text.lines().next().unwrap_or_default().trim();
    first.chars().take(300).collect()
}

/// The plugin's thread: its runtime, its context, its module, then its calls until the channel closes.
fn run(
    code: PluginCode,
    host: Arc<dyn HostApi>,
    mut jobs: mpsc::Receiver<Job>,
    ready: oneshot::Sender<Result<Vec<String>, PluginError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(PluginError::Load(format!("нет среды исполнения: {error}"))));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let id = code.id.clone();
        let memory_limit = code.memory_mb * 1024 * 1024;
        let Ok(engine) = AsyncRuntime::new() else {
            let _ = ready.send(Err(PluginError::Memory(code.memory_mb)));
            return;
        };
        engine.set_memory_limit(memory_limit as usize).await;
        engine.set_max_stack_size(SCRIPT_STACK).await;
        // The interrupt handler stops a script past its deadline: milliseconds since the thread began, none at rest.
        let began = Instant::now();
        let deadline = Arc::new(AtomicU64::new(u64::MAX));
        let watched = deadline.clone();
        engine
            .set_interrupt_handler(Some(Box::new(move || {
                began.elapsed().as_millis() as u64 > watched.load(Ordering::Relaxed)
            })))
            .await;
        let at = |moment: Instant| moment.saturating_duration_since(began).as_millis() as u64;
        let Ok(context) = AsyncContext::full(&engine).await else {
            let _ = ready.send(Err(PluginError::Memory(code.memory_mb)));
            return;
        };

        let load_until = Instant::now() + LOAD_LIMIT;
        deadline.store(at(load_until), Ordering::Relaxed);
        let loaded = tokio::time::timeout_at(load_until.into(), load(&context, &code, host)).await;
        deadline.store(u64::MAX, Ordering::Relaxed);
        let late = || PluginError::Timeout(LOAD_LIMIT.as_millis() as u64);
        let namespace = match loaded {
            Ok(Ok(namespace)) => namespace,
            Ok(Err(PluginError::Load(text))) if Instant::now() >= load_until && text.contains("interrupted") => {
                let _ = ready.send(Err(late()));
                return;
            }
            Ok(Err(error)) => {
                let _ = ready.send(Err(error));
                return;
            }
            Err(_) => {
                let _ = ready.send(Err(late()));
                return;
            }
        };
        let exports = context.with(|ctx| exported(&ctx, &namespace)).await;
        if ready.send(Ok(exports)).is_err() {
            return;
        }

        while let Some(job) = jobs.recv().await {
            match job {
                Job::Memory { reply } => {
                    let usage = engine.memory_usage().await;
                    let _ = reply.send(MemoryStats {
                        allocated: usage.malloc_size.max(0) as u64,
                        used: usage.memory_used_size.max(0) as u64,
                        limit: usage.malloc_limit.max(0) as u64,
                    });
                }
                Job::Call { function, input, kind, deadline: until, reply } => {
                    if Instant::now() >= until {
                        let _ = reply.send(Err(PluginError::Timeout(kind.limit().as_millis() as u64)));
                        continue;
                    }
                    deadline.store(at(until), Ordering::Relaxed);
                    let call = invoke(&context, &namespace, &function, input);
                    let result = match tokio::time::timeout_at(until.into(), call).await {
                        Ok(result) => result,
                        Err(_) => Err(PluginError::Timeout(kind.limit().as_millis() as u64)),
                    };
                    let past = Instant::now() >= until;
                    deadline.store(u64::MAX, Ordering::Relaxed);
                    let result = result.map_err(|error| match error {
                        PluginError::Script(text) if past && text.contains("interrupted") => {
                            PluginError::Timeout(kind.limit().as_millis() as u64)
                        }
                        PluginError::Script(text) if text.contains("out of memory") => {
                            PluginError::Memory(code.memory_mb)
                        }
                        other => other,
                    });
                    let _ = reply.send(result);
                }
            }
        }
        tracing::debug!(target: "plugins", "{id}: unloaded");
    });
}

/// The Host API's raw functions, the prelude over them, then the plugin's module: its exports.
async fn load(
    context: &AsyncContext,
    code: &PluginCode,
    host: Arc<dyn HostApi>,
) -> Result<Persistent<Object<'static>>, PluginError> {
    let name = format!("{}/backend.js", code.id);
    let (id, version, source) = (code.id.clone(), code.version.clone(), code.source.clone());
    let memory_mb = code.memory_mb;
    context
        .async_with(async move |ctx| {
            let failed = |error: CaughtError<'_>| {
                let text = message(&error);
                if text.contains("out of memory") {
                    PluginError::Memory(memory_mb)
                } else {
                    PluginError::Load(text)
                }
            };
            install(&ctx, host, &id, &version).catch(&ctx).map_err(failed)?;
            ctx.eval::<(), _>(PRELUDE).catch(&ctx).map_err(failed)?;
            let declared = Module::declare(ctx.clone(), name, source).catch(&ctx).map_err(failed)?;
            let (module, evaluated) = declared.eval().catch(&ctx).map_err(failed)?;
            evaluated.into_future::<()>().await.catch(&ctx).map_err(failed)?;
            let namespace = module.namespace().catch(&ctx).map_err(failed)?;
            Ok(Persistent::save(&ctx, namespace))
        })
        .await
}

/// Whether a module parses — the panel part of a plugin (`panel.js`), which runs in the browser, not here: its syntax
/// errors show in the workshop's check rather than in the panel (docs/plans/desk.md, stage 5). Nothing is evaluated
/// and its imports are not resolved.
pub async fn syntax_of(name: &str, source: &str) -> Result<(), String> {
    // Its imports — Preact and the panel's SDK — live in the browser: blanked here line for line, the rest is parsed.
    static IMPORTS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?m)^[ \t]*import\b(?:[^;"']*?\bfrom\s*)?["'][^"']*["'][ \t]*;?"#).expect("a valid pattern")
    });
    let source = IMPORTS.replace_all(source, |found: &regex::Captures| "\n".repeat(found[0].matches('\n').count())).into_owned();
    let name = name.to_string();
    // A runtime of its own on a blocking thread: the engine is not Send, the check's caller is.
    tokio::task::spawn_blocking(move || {
        let engine = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
        let context = rquickjs::Context::full(&engine).map_err(|error| error.to_string())?;
        context.with(|ctx| Module::declare(ctx.clone(), name, source).catch(&ctx).map(|_| ()).map_err(|error| message(&error)))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The functions a module exports.
fn exported(ctx: &Ctx<'_>, namespace: &Persistent<Object<'static>>) -> Vec<String> {
    let Ok(namespace) = namespace.clone().restore(ctx) else { return Vec::new() };
    let mut names: Vec<String> = namespace
        .props::<String, rquickjs::Value>()
        .filter_map(Result::ok)
        .filter(|(_, value)| value.is_function())
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names
}

/// One call: the input in as JSON, the function's answer (its promise awaited) out as JSON.
async fn invoke(
    context: &AsyncContext,
    namespace: &Persistent<Object<'static>>,
    function: &str,
    input: Value,
) -> Result<Value, PluginError> {
    let namespace = namespace.clone();
    let function = function.to_string();
    let argument = input.to_string();
    context
        .async_with(async move |ctx| {
            let script = |error: CaughtError<'_>| PluginError::Script(message(&error));
            let namespace = namespace.restore(&ctx).map_err(|error| PluginError::Script(error.to_string()))?;
            let target: rquickjs::Value = namespace.get(function.as_str()).catch(&ctx).map_err(script)?;
            let Some(target) = target.as_function().cloned() else {
                return Err(PluginError::Missing(function));
            };
            let argument = ctx.json_parse(argument).catch(&ctx).map_err(script)?;
            let mut answer: rquickjs::Value = target.call((argument,)).catch(&ctx).map_err(script)?;
            if let Some(promise) = answer.as_promise().cloned() {
                answer = promise.into_future::<rquickjs::Value>().await.catch(&ctx).map_err(script)?;
            }
            if answer.is_undefined() {
                return Ok(Value::Null);
            }
            let text = ctx
                .json_stringify(answer)
                .catch(&ctx)
                .map_err(script)?
                .map(|text| text.to_string())
                .transpose()
                .map_err(|error| PluginError::Answer(format!("ответ не читается: {error}")))?
                .unwrap_or_else(|| "null".into());
            if text.len() > MAX_ANSWER {
                return Err(PluginError::Answer(format!("ответ больше {} КБ", MAX_ANSWER / 1024)));
            }
            serde_json::from_str(&text).map_err(|error| PluginError::Answer(format!("ответ не JSON: {error}")))
        })
        .await
}

/// A raw function's answer: `{"value": …}` or `{"error": "…"}`, never an exception.
fn answer(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({ "value": value }).to_string(),
        Err(error) => json!({ "error": error }).to_string(),
    }
}

/// `globalThis.__duowave`: the Host API's raw functions, for the prelude.
fn install<'js>(ctx: &Ctx<'js>, host: Arc<dyn HostApi>, id: &str, version: &str) -> rquickjs::Result<()> {
    let raw = Object::new(ctx.clone())?;
    raw.set("id", id)?;
    raw.set("version", version)?;
    let fetching = host.clone();
    raw.set(
        "fetch",
        Function::new(
            ctx.clone(),
            Async(move |request: String| {
                let host = fetching.clone();
                async move {
                    let result = match serde_json::from_str::<Value>(&request) {
                        Ok(request) => host.fetch(request).await,
                        Err(error) => Err(format!("запрос не читается: {error}")),
                    };
                    rquickjs::Result::Ok(answer(result))
                }
            }),
        )?,
    )?;
    let speaking = host.clone();
    raw.set(
        "say",
        Function::new(
            ctx.clone(),
            Async(move |request: String| {
                let host = speaking.clone();
                async move {
                    let result = match serde_json::from_str::<Value>(&request) {
                        Ok(request) => {
                            let text = request.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                            let device = request.get("device").and_then(Value::as_str).map(str::to_string);
                            host.say(text, device).await.map(Value::Bool)
                        }
                        Err(error) => Err(format!("запрос не читается: {error}")),
                    };
                    rquickjs::Result::Ok(answer(result))
                }
            }),
        )?,
    )?;
    let calling = host.clone();
    raw.set(
        "call",
        Function::new(
            ctx.clone(),
            Async(move |name: String, input: String| {
                let host = calling.clone();
                async move {
                    let result = match serde_json::from_str::<Value>(&input) {
                        Ok(input) => host.call(&name, input).await,
                        Err(error) => Err(format!("вход не читается: {error}")),
                    };
                    rquickjs::Result::Ok(answer(result))
                }
            }),
        )?,
    )?;
    let store = host.clone();
    raw.set(
        "kvGet",
        Function::new(ctx.clone(), move |key: String| {
            answer(store.kv_get(&key).map(|value| value.unwrap_or(Value::Null)))
        })?,
    )?;
    let store = host.clone();
    raw.set(
        "kvSet",
        Function::new(ctx.clone(), move |key: String, value: String| {
            let result = serde_json::from_str::<Value>(&value)
                .map_err(|error| format!("значение не JSON: {error}"))
                .and_then(|value| store.kv_set(&key, value));
            answer(result.map(|()| Value::Null))
        })?,
    )?;
    let store = host.clone();
    raw.set(
        "kvDelete",
        Function::new(ctx.clone(), move |key: String| answer(store.kv_delete(&key).map(|()| Value::Null)))?,
    )?;
    let store = host.clone();
    raw.set(
        "kvKeys",
        Function::new(ctx.clone(), move |prefix: String| {
            answer(store.kv_keys(&prefix).map(|keys| Value::Array(keys.into_iter().map(Value::String).collect())))
        })?,
    )?;
    let settings = host.clone();
    raw.set("settings", Function::new(ctx.clone(), move || Value::Object(settings.settings()).to_string())?)?;
    let logger = host;
    raw.set("log", Function::new(ctx.clone(), move |level: String, text: String| logger.log(&level, &text))?)?;
    ctx.globals().set("__duowave", raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    /// A host that answers fetch after a while and keeps kv in memory; no rights checked here (host_api.rs is).
    struct Host {
        fetch_ms: u64,
        kv: Mutex<Map<String, Value>>,
    }

    impl HostApi for Host {
        fn fetch(&self, request: Value) -> BoxFuture<'static, Result<Value, String>> {
            let wait = self.fetch_ms;
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(wait)).await;
                Ok(json!({"status": 200, "url": request["url"], "headers": {}, "body": "{\"stations\":[\"Джаз\"]}"}))
            })
        }
        fn kv_get(&self, key: &str) -> Result<Option<Value>, String> {
            Ok(self.kv.lock().unwrap().get(key).cloned())
        }
        fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
            self.kv.lock().unwrap().insert(key.into(), value);
            Ok(())
        }
        fn kv_delete(&self, key: &str) -> Result<(), String> {
            self.kv.lock().unwrap().remove(key);
            Ok(())
        }
        fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String> {
            Ok(self.kv.lock().unwrap().keys().filter(|key| key.starts_with(prefix)).cloned().collect())
        }
        fn say(&self, text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
            Box::pin(async move { Ok(text == "привет") })
        }
        fn settings(&self) -> Map<String, Value> {
            json!({"country": "RU"}).as_object().cloned().unwrap()
        }
        fn log(&self, _level: &str, _message: &str) {}
    }

    const SOURCE: &str = r#"
        let calls = 0;
        export function sum({ a, b }) { calls += 1; return { sum: a + b, calls }; }
        export async function fetched({ url }) {
            const answer = await host.http.fetch(url);
            return { status: answer.status, stations: (await answer.json()).stations, country: host.settings.get("country") };
        }
        export function remember({ key, value }) { host.kv.set(key, value); return host.kv.get(key); }
        export function spin() { while (true) {} }
        export async function spinAfterAwait() { await host.http.fetch("https://x.example"); while (true) {} }
        export function hog() { const all = []; for (;;) all.push("x".repeat(4096)); }
        export function quiet() {}
        export async function greet({ text }) { return await host.voice.say(text); }
        export const notAFunction = 1;
    "#;

    async fn start(source: &str, fetch_ms: u64) -> Result<Arc<dyn PluginInstance>, PluginError> {
        let code = PluginCode {
            id: "test".into(),
            version: "1.0.0".into(),
            source: source.into(),
            memory_mb: 2,
            ..PluginCode::default()
        };
        QuickJs.start(code, Arc::new(Host { fetch_ms, kv: Mutex::new(Map::new()) })).await
    }

    #[tokio::test]
    async fn calls_the_functions_a_module_exports_with_json_in_and_out() {
        let plugin = start(SOURCE, 10).await.unwrap();
        assert_eq!(plugin.exports(), ["fetched", "greet", "hog", "quiet", "remember", "spin", "spinAfterAwait", "sum"]);
        assert_eq!(plugin.call("greet", json!({"text": "привет"}), CallKind::Background).await, Ok(json!(true)));
        assert_eq!(
            plugin.call("sum", json!({"a": 2, "b": 3}), CallKind::Voice).await,
            Ok(json!({"sum": 5, "calls": 1}))
        );
        // Its state stays between calls while it is loaded.
        assert_eq!(plugin.call("sum", json!({"a": 1, "b": 1}), CallKind::Voice).await.unwrap()["calls"], 2);
        assert_eq!(
            plugin.call("fetched", json!({"url": "https://radio.example"}), CallKind::Tool).await,
            Ok(json!({"status": 200, "stations": ["Джаз"], "country": "RU"}))
        );
        assert_eq!(plugin.call("remember", json!({"key": "k", "value": [1]}), CallKind::Voice).await, Ok(json!([1])));
        assert_eq!(plugin.call("quiet", json!({}), CallKind::Voice).await, Ok(Value::Null));
        assert_eq!(
            plugin.call("nothing", json!({}), CallKind::Voice).await,
            Err(PluginError::Missing("nothing".into()))
        );
        assert!(matches!(plugin.call("notAFunction", json!({}), CallKind::Voice).await, Err(PluginError::Missing(_))));
        let memory = plugin.memory().await.unwrap();
        assert!(memory.used > 0 && memory.limit == 2 * 1024 * 1024, "{memory:?}");
        plugin.stop();
        assert!(matches!(
            plugin.call("sum", json!({"a": 1, "b": 1}), CallKind::Voice).await,
            Err(PluginError::Unavailable(_))
        ));
        assert!(plugin.memory().await.is_none());
    }

    #[tokio::test]
    async fn stops_a_call_past_its_limit_and_one_over_its_memory_the_gateway_goes_on() {
        let plugin = start(SOURCE, 5_000).await.unwrap();
        let began = Instant::now();
        assert_eq!(plugin.call("spin", json!({}), CallKind::Voice).await, Err(PluginError::Timeout(2000)));
        assert!(began.elapsed() < Duration::from_millis(2600), "{:?}", began.elapsed());
        // Waiting on the host past the limit ends the call too.
        let began = Instant::now();
        assert_eq!(plugin.call("spinAfterAwait", json!({}), CallKind::Voice).await, Err(PluginError::Timeout(2000)));
        assert!(began.elapsed() < Duration::from_millis(2600));
        assert_eq!(plugin.call("hog", json!({}), CallKind::Voice).await, Err(PluginError::Memory(2)));
        // The runtime on the gateway's side was never held: a timer here fires on time meanwhile.
        let timer = tokio::spawn(tokio::time::sleep(Duration::from_millis(50)));
        let spinning = plugin.call("spin", json!({}), CallKind::Voice);
        let began = Instant::now();
        timer.await.unwrap();
        assert!(began.elapsed() < Duration::from_millis(500));
        assert!(spinning.await.is_err());
    }

    #[tokio::test]
    async fn refuses_code_that_does_not_load() {
        let broken = start("export function (", 0).await.err().unwrap();
        assert!(matches!(&broken, PluginError::Load(text) if text.contains("expected")), "{broken:?}");
        let throws = start("throw new Error('нет ключа')", 0).await.err().unwrap();
        assert_eq!(throws, PluginError::Load("Error: нет ключа".into()));
        let hangs = start("while (true) {}", 0).await.err().unwrap();
        assert_eq!(hangs, PluginError::Timeout(2000));
        let huge = start("const all = []; for (;;) all.push('x'.repeat(4096));", 0).await.err().unwrap();
        assert_eq!(huge, PluginError::Memory(2));
    }
}
