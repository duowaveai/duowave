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
  // Host calls begun and not ended. One still running when its function has answered was not awaited: the engine
  // asks (`__duowaveLeft`, after every call) and the app's journal says so — the call may never end, its error is lost.
  let begun = 0;
  const running = new Map();
  const tracked = async (name, work) => {
    const id = ++begun;
    running.set(id, name);
    try {
      return await work();
    } finally {
      running.delete(id);
    }
  };
  const shown = (name) => (name.startsWith("folders.") ? `host.files.${name.slice(8)}` : `host.${name}`);
  Object.defineProperty(globalThis, "__duowaveLeft", {
    value: (fn) => {
      if (running.size === 0) return;
      const names = [...new Set(running.values())].map(shown);
      running.clear();
      say("warn")(`${fn} ответила, не дождавшись ${names.join(", ")}: вызов шлюза отвечает обещанием — пишите await, а функцию — async. Без этого шлюз его не ждёт: картинка может не скачаться, файл не записаться, а ошибка пропадёт молча.`);
    },
  });
  const http = Object.freeze({
    async fetch(url, options = {}) {
      const body = options.body === undefined || typeof options.body === "string" ? options.body : JSON.stringify(options.body);
      const answer = unwrap(await tracked("http.fetch", () => raw.fetch(JSON.stringify({
        url: String(url), method: options.method, headers: options.headers, body, timeoutMs: options.timeoutMs,
      }))));
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
  const settings = Object.freeze({
    get: (key) => {
      const all = JSON.parse(raw.settings());
      return key === undefined ? all : all[key];
    },
  });
  const log = say("info");
  // Host API 2 (docs/plans/bus.md, «Один способ»): everything beyond the app is a letter on the bus, decided by it.
  const call = (name, input = {}) =>
    tracked(name, async () => unwrap(await raw.call(name, JSON.stringify(input ?? {}))));
  // The bus (src/bus): call another's handle, tell own news, see what is there, ask in words.
  const bus = Object.freeze({
    call: (to, handle, input = {}, options = {}) =>
      call("bus.call", { to: String(to), handle: String(handle), input: input ?? {}, files: options.files ?? [] }),
    tell: (event, payload = {}) => call("bus.tell", { event: String(event), payload: payload ?? {} }),
    list: () => call("bus.list", {}),
    ask: (to, text) => call("bus.ask", { to: String(to), text: String(text ?? "") }),
  });
  // Folders: an address `files/…`, or a path inside the app's own folder («tmp/a.txt»); `download` keeps a picture
  // from the internet (its host among the `net:` rights) in it.
  const files = Object.freeze({
    list: (at = "", options = {}) =>
      call("folders.list", { at: String(at ?? ""), from: options?.from, limit: options?.limit }),
    find: (at, query, options = {}) =>
      call("folders.find", { at: String(at ?? ""), query: String(query ?? ""), limit: options?.limit }),
    text: (at, options = {}) => call("folders.text", { at: String(at), part: options?.part }),
    preview: (at, options = {}) => call("folders.preview", { at: String(at), size: options?.size }),
    play: (at) => call("folders.play", { at: String(at) }),
    copy: (from, to) => call("folders.copy", { at: String(from), to: String(to) }),
    read: (at, options = {}) => call("folders.read", { at: String(at), as: options.as }),
    write: (at, data) => call("folders.write", typeof data === "string"
      ? { at: String(at), text: data }
      : { at: String(at), text: data?.text, base64: data?.base64 }),
    remove: (at) => call("folders.remove", { at: String(at) }),
    move: (from, to) => call("folders.move", { at: String(from), to: String(to) }),
    mkdir: (at) => call("folders.mkdir", { at: String(at) }),
    stat: (at) => call("folders.stat", { at: String(at) }),
    download: (url, name) => call("folders.download", { url: String(url), name: String(name ?? "") }),
  });
  // Languages (docs/plans/i18n.md): the household's language now, the app's own languages, a text picked by them.
  // QuickJS has no Intl: the plural forms of the languages we know are here, the rest say «one» for 1, else «other».
  const languages = Object.freeze(JSON.parse(raw.languages));
  const pluralOf = (code, count) => {
    const n = Math.abs(Number(count));
    if (!Number.isFinite(n)) return "other";
    const whole = Number.isInteger(n);
    if (["ru", "uk", "be"].includes(code)) {
      if (!whole) return "other";
      const units = n % 10, tens = n % 100;
      if (units === 1 && tens !== 11) return "one";
      if (units >= 2 && units <= 4 && (tens < 12 || tens > 14)) return "few";
      return "many";
    }
    if (code === "pl") {
      if (!whole) return "other";
      const units = n % 10, tens = n % 100;
      if (n === 1) return "one";
      if (units >= 2 && units <= 4 && (tens < 12 || tens > 14)) return "few";
      return "many";
    }
    if (["fr", "pt"].includes(code)) return n < 2 ? "one" : "other";
    if (["ja", "zh", "ko", "vi", "th", "id", "ms"].includes(code)) return "other";
    return n === 1 ? "one" : "other";
  };
  // No text in the household's language — English, the language of last resort; then the app's own, then Russian.
  const order = () => [...new Set([String(raw.locale()), "en", languages[0], "ru"])];
  const pick = (variants) => {
    if (variants === null || typeof variants !== "object") return variants;
    for (const code of order()) if (variants[code] !== undefined) return variants[code];
    return Object.values(variants)[0];
  };
  const t = (messages, key, params = {}) => {
    for (const code of order()) {
      let text = messages?.[code]?.[key];
      if (text === undefined) continue;
      if (text !== null && typeof text === "object") {
        const form = pluralOf(code, params?.count);
        text = text[form] ?? text.other ?? text.many ?? text.one ?? "";
      }
      return String(text).replace(/\{(\w+)\}/g, (whole, name) => (params && name in params ? String(params[name]) : whole));
    }
    return String(key);
  };
  const i18n = Object.freeze({
    get locale() { return String(raw.locale()); },
    languages,
    pick,
    t,
    plural: (count, forms) => {
      const form = pluralOf(String(raw.locale()), count);
      return forms?.[form] ?? forms?.other ?? forms?.many ?? forms?.one ?? "";
    },
  });
  globalThis.host = Object.freeze({
    api: 2,
    plugin: Object.freeze({ id: raw.id, version: raw.version }),
    http, kv, settings, log, bus, files, i18n,
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
                .map_err(|error| PluginError::Load(crate::tr!("поток не запустился: {error}", error = error)))?;
            // A load that hangs gives the thread up: its channel goes with this future, and the thread ends once
            // it gets past the load.
            let exports = match tokio::time::timeout(LOAD_LIMIT + GRACE, started).await {
                Ok(Ok(loaded)) => loaded?,
                Ok(Err(_)) => return Err(PluginError::Load(crate::tr!("поток приложения завершился"))),
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
        /// The trail of the chain the call is part of (src/bus/trail.rs): the thread holds it while the call runs.
        trail: Option<crate::bus::Trail>,
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
    PluginError::Unavailable(crate::tr!("приложение выгружено"))
}

impl PluginInstance for Instance {
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>> {
        let sender = self.sender();
        let function = function.to_string();
        // Read where the call is made, in the caller's task: the plugin's thread holds it for the call's host calls.
        let trail = crate::bus::trail::current();
        Box::pin(async move {
            let sender = sender.ok_or_else(stopped)?;
            let limit = kind.limit();
            // The limit counts from the call, a wait behind a busy plugin included.
            let deadline = Instant::now() + limit;
            let (reply, answer) = oneshot::channel();
            let job = Job::Call { function, input, kind, deadline, trail, reply };
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
            let _ = ready.send(Err(PluginError::Load(crate::tr!("нет среды исполнения: {error}", error = error))));
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
                Job::Call { function, input, kind, deadline: until, trail, reply } => {
                    if Instant::now() >= until {
                        let _ = reply.send(Err(PluginError::Timeout(kind.limit().as_millis() as u64)));
                        continue;
                    }
                    // One call at a time on this thread: its host calls read this trail (src/bus/trail.rs).
                    let _held = crate::bus::Trail::hold(trail);
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
                        PluginError::Script(text) if out_of_memory(&text) => PluginError::Memory(code.memory_mb),
                        other => other,
                    });
                    let _ = reply.send(result);
                }
            }
        }
        tracing::debug!(target: "plugins", "{id}: unloaded");
    });
}

/// Whether a script failed for its memory: QuickJS says «out of memory», or — short even of the memory for that error —
/// throws nothing but null (a test on 12.11.2026, once the prelude grew by `host.schedule`).
fn out_of_memory(text: &str) -> bool {
    text.contains("out of memory") || text.contains("Exception generated by quickjs: Null")
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
                if out_of_memory(&text) {
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
        regex::Regex::new(r#"(?m)^[ \t]*import\b(?:[^;"']*?\bfrom\s*)?["'][^"']*["'][ \t]*;?"#)
            .expect("a valid pattern")
    });
    let source =
        IMPORTS.replace_all(source, |found: &regex::Captures| "\n".repeat(found[0].matches('\n').count())).into_owned();
    let name = name.to_string();
    // A runtime of its own on a blocking thread: the engine is not Send, the check's caller is.
    tokio::task::spawn_blocking(move || {
        let engine = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
        let context = rquickjs::Context::full(&engine).map_err(|error| error.to_string())?;
        context.with(|ctx| {
            Module::declare(ctx.clone(), name, source).catch(&ctx).map(|_| ()).map_err(|error| message(&error))
        })
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
            tell_left(&ctx, &function);
            if answer.is_undefined() {
                return Ok(Value::Null);
            }
            let text = ctx
                .json_stringify(answer)
                .catch(&ctx)
                .map_err(script)?
                .map(|text| text.to_string())
                .transpose()
                .map_err(|error| PluginError::Answer(crate::tr!("ответ не читается: {error}", error = error)))?
                .unwrap_or_else(|| "null".into());
            if text.len() > MAX_ANSWER {
                return Err(PluginError::Answer(crate::tr!("ответ больше {max} КБ", max = MAX_ANSWER / 1024)));
            }
            serde_json::from_str(&text)
                .map_err(|error| PluginError::Answer(crate::tr!("ответ не JSON: {error}", error = error)))
        })
        .await
}

/// Host calls the function began and did not await, still running now that it has answered: the prelude's
/// `__duowaveLeft` says so in the app's journal (and the gateway's log).
fn tell_left(ctx: &Ctx<'_>, function: &str) {
    if let Ok(left) = ctx.globals().get::<_, Function>("__duowaveLeft") {
        let _ = left.call::<_, ()>((function,)).catch(ctx);
    }
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
                        Err(error) => Err(crate::tr!("запрос не читается: {error}", error = error)),
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
                        Err(error) => Err(crate::tr!("запрос не читается: {error}", error = error)),
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
                        Err(error) => Err(crate::tr!("вход не читается: {error}", error = error)),
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
                .map_err(|error| crate::tr!("значение не JSON: {error}", error = error))
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
    let speaking_language = host.clone();
    raw.set("locale", Function::new(ctx.clone(), move || speaking_language.locale())?)?;
    raw.set("languages", serde_json::to_string(&host.languages()).unwrap_or_else(|_| "[\"ru\"]".into()))?;
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
        /// The app's journal: level and line.
        logs: Mutex<Vec<(String, String)>>,
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
        fn log(&self, level: &str, message: &str) {
            self.logs.lock().unwrap().push((level.into(), message.into()));
        }
        fn locale(&self) -> String {
            "ru".into()
        }
        /// Host API 2: speech is a letter to `system/voice`.
        fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
            let said = name == "bus.call" && input["to"] == "system/voice" && input["input"]["text"] == "привет";
            Box::pin(async move { Ok(Value::Bool(said)) })
        }
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
        export async function greet({ text }) { return await host.bus.call("system/voice", "say", { text }); }
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
        QuickJs.start(code, Arc::new(Host { fetch_ms, kv: Mutex::new(Map::new()), logs: Mutex::new(Vec::new()) })).await
    }

    #[tokio::test]
    async fn tells_the_apps_journal_of_a_host_call_its_function_answered_without_awaiting() {
        let host = Arc::new(Host { fetch_ms: 50, kv: Mutex::new(Map::new()), logs: Mutex::new(Vec::new()) });
        let source = r#"
            export function card() { host.files.download("https://picsum.photos/1", "a"); return { name: "a.jpg" }; }
            export async function waited() { await host.files.download("https://picsum.photos/1", "a"); return 1; }
            export function fetchedLate() { host.http.fetch("https://x.example"); return 2; }
        "#;
        let code = PluginCode {
            id: "test".into(),
            version: "1.0.0".into(),
            source: source.into(),
            memory_mb: 2,
            ..PluginCode::default()
        };
        let plugin = QuickJs.start(code, host.clone()).await.unwrap();
        assert_eq!(plugin.call("card", json!({}), CallKind::Background).await, Ok(json!({"name": "a.jpg"})));
        let told = std::mem::take(&mut *host.logs.lock().unwrap());
        assert!(
            told.iter().any(|(level, line)| level == "warn"
                && line.starts_with("card ответила, не дождавшись host.files.download")
                && line.contains("await")),
            "{told:?}"
        );
        assert_eq!(plugin.call("waited", json!({}), CallKind::Background).await, Ok(json!(1)));
        assert!(host.logs.lock().unwrap().is_empty(), "an awaited call is not told");
        assert_eq!(plugin.call("fetchedLate", json!({}), CallKind::Background).await, Ok(json!(2)));
        let told = host.logs.lock().unwrap().clone();
        assert!(told.iter().any(|(_, line)| line.contains("не дождавшись host.http.fetch")), "{told:?}");
        plugin.stop();
    }

    #[tokio::test]
    async fn host_i18n_picks_the_household_language_and_its_plural() {
        const SPEAKS: &str = r#"
            const T = {
                ru: { found: "Нашлось {count} {what}", stations: { one: "станция", few: "станции", many: "станций" } },
                en: { found: "Found {count} {what}", only: "English only" },
            };
            export function texts({ n }) {
                return {
                    locale: host.i18n.locale,
                    languages: host.i18n.languages,
                    found: host.i18n.t(T, "found", { count: n, what: host.i18n.t(T, "stations", { count: n }) }),
                    only: host.i18n.t(T, "only"),
                    missing: host.i18n.t(T, "nothing"),
                    picked: host.i18n.pick({ en: "Hello", de: "Hallo" }),
                    plural: host.i18n.plural(21, { one: "станция", few: "станции", many: "станций" }),
                };
            }
        "#;
        let plugin = start(SPEAKS, 10).await.unwrap();
        assert_eq!(
            plugin.call("texts", json!({"n": 3}), CallKind::Background).await,
            Ok(json!({
                "locale": "ru",
                "languages": ["ru"],
                "found": "Нашлось 3 станции",
                "only": "English only",
                "missing": "nothing",
                "picked": "Hello",
                "plural": "станция",
            }))
        );
        assert_eq!(
            plugin.call("texts", json!({"n": 11}), CallKind::Background).await.unwrap()["found"],
            "Нашлось 11 станций"
        );
        plugin.stop();
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
