//! WebAssembly plugins (`"runtime": "wasm"`, docs/plans/plugins.md, stage 8): `backend.wasm` in wasmi, an interpreter
//! in pure Rust — any language that compiles to WebAssembly writes a plugin, in a sandbox of its own. Like QuickJS, each
//! started plugin gets a thread with its store; a call's time is its fuel (the interpreter stops when it runs out), its
//! memory is the budget's (`budget.memoryMb` as the store's limit).
//!
//! The ABI (version 1), JSON in and out as UTF-8 in the module's memory:
//!
//! - the module exports `memory`, `alloc(len: i32) -> i32` (the host writes its JSON there), and each handler as
//!   `(ptr: i32, len: i32) -> i64`: its answer's place as `(ptr << 32) | len`, 0 for `null`;
//! - the host gives, in module `host`: `log(level: i32, ptr, len)` (0 info, 1 warn, 2 error),
//!   `kv_get(key_ptr, key_len) -> i64` (the value's JSON, 0 when none), `kv_set(key_ptr, key_len, value_ptr,
//!   value_len) -> i32` (0 — written), `kv_delete(key_ptr, key_len) -> i32`, `fetch(request_ptr, request_len) -> i64`
//!   (`{url, method, headers, body, timeoutMs}` → `{status, url, headers, body}` or `{error}`), `settings() -> i64`,
//!   `say(text_ptr, text_len) -> i32` (1 — said, 0 — no station, -1 — refused), and Host API 1.1's
//!   `call(name_ptr, name_len, input_ptr, input_len) -> i64` (`{value}` or `{error}`: `content.search`, `tools.call`,
//!   `smartHome.devices`, …). The rights are the manifest's, checked as for a script.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use wasmi::{Caller, Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

use super::runtime::{
    CallKind, HostApi, MemoryStats, PluginCode, PluginError, PluginInstance, PluginRuntime, LOAD_LIMIT,
};

/// Instructions a millisecond of a call is worth, roughly: an interpreter on the station runs tens of millions a
/// second, on a computer hundreds.
const FUEL_PER_MS: u64 = 50_000;
/// A JSON answer bigger than this is not taken.
const MAX_ANSWER: usize = 256 * 1024;
const QUEUE: usize = 16;
const GRACE: Duration = Duration::from_millis(500);

pub struct Wasm;

/// The store's data: the limits and the way to the host.
struct State {
    limits: StoreLimits,
    host: Arc<dyn HostApi>,
    handle: Option<tokio::runtime::Handle>,
}

type Handler = TypedFunc<(i32, i32), i64>;

/// A started module: its store, its memory, its allocator and its handlers.
struct Started {
    store: Store<State>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    instance: wasmi::Instance,
}

fn unpack(packed: i64) -> (usize, usize) {
    (((packed as u64) >> 32) as usize, (packed as u64 & 0xffff_ffff) as usize)
}

fn pack(ptr: i32, len: usize) -> i64 {
    ((ptr as u32 as i64) << 32) | (len as u32 as i64)
}

/// Bytes of the module's memory at `ptr`, `len` long.
fn read(caller: &Caller<'_, State>, memory: &Memory, ptr: i32, len: i32) -> Option<Vec<u8>> {
    let data = memory.data(caller);
    let (start, len) = (ptr as u32 as usize, len as u32 as usize);
    data.get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
}

fn text(caller: &Caller<'_, State>, memory: &Memory, ptr: i32, len: i32) -> Option<String> {
    read(caller, memory, ptr, len).and_then(|bytes| String::from_utf8(bytes).ok())
}

/// JSON into the module's memory through its `alloc`; its place packed, 0 when it does not fit.
fn give(caller: &mut Caller<'_, State>, value: &Value) -> i64 {
    let bytes = value.to_string().into_bytes();
    let Some(alloc) = caller.get_export("alloc").and_then(|export| export.into_func()) else { return 0 };
    let Ok(alloc) = alloc.typed::<i32, i32>(&*caller) else { return 0 };
    let Ok(ptr) = alloc.call(&mut *caller, bytes.len() as i32) else { return 0 };
    let Some(memory) = caller.get_export("memory").and_then(|export| export.into_memory()) else { return 0 };
    if memory.write(&mut *caller, ptr as u32 as usize, &bytes).is_err() {
        return 0;
    }
    pack(ptr, bytes.len())
}

fn memory_of(caller: &Caller<'_, State>) -> Option<Memory> {
    caller.get_export("memory").and_then(|export| export.into_memory())
}

/// Runs a future of the gateway on its runtime from the plugin's thread, which is not one of the runtime's.
fn wait<T>(handle: &Option<tokio::runtime::Handle>, future: BoxFuture<'static, T>) -> Option<T> {
    handle.as_ref().map(|handle| handle.block_on(future))
}

fn linker(engine: &Engine) -> Result<Linker<State>, wasmi::Error> {
    let mut linker = Linker::<State>::new(engine);
    linker.func_wrap("host", "log", |caller: Caller<'_, State>, level: i32, ptr: i32, len: i32| {
        if let Some(message) = memory_of(&caller).and_then(|memory| text(&caller, &memory, ptr, len)) {
            let level = match level {
                2 => "error",
                1 => "warn",
                _ => "info",
            };
            caller.data().host.log(level, &message);
        }
    })?;
    linker.func_wrap("host", "kv_get", |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> i64 {
        let Some(key) = memory_of(&caller).and_then(|memory| text(&caller, &memory, ptr, len)) else { return 0 };
        match caller.data().host.kv_get(&key) {
            Ok(Some(value)) => give(&mut caller, &value),
            _ => 0,
        }
    })?;
    linker.func_wrap(
        "host",
        "kv_set",
        |caller: Caller<'_, State>, key_ptr: i32, key_len: i32, value_ptr: i32, value_len: i32| -> i32 {
            let Some(memory) = memory_of(&caller) else { return 1 };
            let (Some(key), Some(value)) =
                (text(&caller, &memory, key_ptr, key_len), text(&caller, &memory, value_ptr, value_len))
            else {
                return 1;
            };
            let Ok(value) = serde_json::from_str::<Value>(&value) else { return 1 };
            match caller.data().host.kv_set(&key, value) {
                Ok(()) => 0,
                Err(why) => {
                    caller.data().host.log("warn", &format!("kv_set: {why}"));
                    1
                }
            }
        },
    )?;
    linker.func_wrap("host", "kv_delete", |caller: Caller<'_, State>, ptr: i32, len: i32| -> i32 {
        let Some(key) = memory_of(&caller).and_then(|memory| text(&caller, &memory, ptr, len)) else { return 1 };
        i32::from(caller.data().host.kv_delete(&key).is_err())
    })?;
    linker.func_wrap("host", "fetch", |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> i64 {
        let request = memory_of(&caller)
            .and_then(|memory| text(&caller, &memory, ptr, len))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let Some(request) = request else {
            return give(&mut caller, &serde_json::json!({ "error": crate::tr!("запрос — JSON") }));
        };
        let future = caller.data().host.fetch(request);
        let answer = match wait(&caller.data().handle, future) {
            Some(Ok(answer)) => answer,
            Some(Err(why)) => serde_json::json!({ "error": why }),
            None => serde_json::json!({ "error": crate::tr!("сеть недоступна") }),
        };
        give(&mut caller, &answer)
    })?;
    linker.func_wrap("host", "settings", |mut caller: Caller<'_, State>| -> i64 {
        let settings = Value::Object(caller.data().host.settings());
        give(&mut caller, &settings)
    })?;
    linker.func_wrap(
        "host",
        "call",
        |mut caller: Caller<'_, State>, name_ptr: i32, name_len: i32, input_ptr: i32, input_len: i32| -> i64 {
            let Some(memory) = memory_of(&caller) else { return 0 };
            let name = text(&caller, &memory, name_ptr, name_len).unwrap_or_default();
            let input = text(&caller, &memory, input_ptr, input_len)
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or(Value::Null);
            let future = caller.data().host.call(&name, input);
            let answer = match wait(&caller.data().handle, future) {
                Some(Ok(value)) => serde_json::json!({ "value": value }),
                Some(Err(why)) => serde_json::json!({ "error": why }),
                None => serde_json::json!({ "error": crate::tr!("шлюз недоступен") }),
            };
            give(&mut caller, &answer)
        },
    )?;
    linker.func_wrap("host", "say", |caller: Caller<'_, State>, ptr: i32, len: i32| -> i32 {
        let Some(text) = memory_of(&caller).and_then(|memory| text(&caller, &memory, ptr, len)) else { return -1 };
        let future = caller.data().host.say(text, None);
        match wait(&caller.data().handle, future) {
            Some(Ok(true)) => 1,
            Some(Ok(false)) | None => 0,
            Some(Err(_)) => -1,
        }
    })?;
    Ok(linker)
}

fn start(
    code: &PluginCode,
    host: Arc<dyn HostApi>,
    handle: Option<tokio::runtime::Handle>,
) -> Result<Started, PluginError> {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, &code.bytes)
        .map_err(|error| PluginError::Load(crate::tr!("модуль не читается: {error}", error = error)))?;
    let limits = StoreLimitsBuilder::new().memory_size((code.memory_mb * 1024 * 1024) as usize).build();
    let mut store = Store::new(&engine, State { limits, host, handle });
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(LOAD_LIMIT.as_millis() as u64 * FUEL_PER_MS)
        .map_err(|error| PluginError::Load(error.to_string()))?;
    let linker = linker(&engine).map_err(|error| PluginError::Load(error.to_string()))?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| PluginError::Load(crate::tr!("модуль не запустился: {error}", error = error)))?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| PluginError::Load(crate::tr!("модуль не отдаёт memory")))?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "alloc")
        .map_err(|_| PluginError::Load(crate::tr!("модуль не отдаёт alloc(len) -> ptr")))?;
    Ok(Started { store, memory, alloc, instance })
}

/// The functions a module exports as handlers: every function of the handler's shape but `alloc`.
fn handlers(started: &Started) -> Vec<String> {
    let mut names: Vec<String> = started
        .instance
        .exports(&started.store)
        .filter(|export| export.clone().into_func().is_some())
        .map(|export| export.name().to_string())
        .filter(|name| name != "alloc")
        .filter(|name| started.instance.get_typed_func::<(i32, i32), i64>(&started.store, name).is_ok())
        .collect();
    names.sort();
    names
}

fn call(started: &mut Started, function: &str, input: &Value, kind: CallKind) -> Result<Value, PluginError> {
    let handler: Handler = started
        .instance
        .get_typed_func::<(i32, i32), i64>(&started.store, function)
        .map_err(|_| PluginError::Missing(function.to_string()))?;
    let limit_ms = kind.limit().as_millis() as u64;
    started.store.set_fuel(limit_ms * FUEL_PER_MS).map_err(|error| PluginError::Script(error.to_string()))?;
    let bytes = input.to_string().into_bytes();
    let trap = |error: wasmi::Error| {
        let text = error.to_string();
        if text.contains("fuel") {
            PluginError::Timeout(limit_ms)
        } else if text.contains("memory") && text.contains("limit") {
            PluginError::Memory(0)
        } else {
            PluginError::Script(text)
        }
    };
    let ptr = started.alloc.call(&mut started.store, bytes.len() as i32).map_err(trap)?;
    started
        .memory
        .write(&mut started.store, ptr as u32 as usize, &bytes)
        .map_err(|_| PluginError::Script(crate::tr!("вход не поместился в память модуля")))?;
    let packed = handler.call(&mut started.store, (ptr, bytes.len() as i32)).map_err(trap)?;
    if packed == 0 {
        return Ok(Value::Null);
    }
    let (at, len) = unpack(packed);
    if len > MAX_ANSWER {
        return Err(PluginError::Answer(crate::tr!("ответ больше {max} КБ", max = MAX_ANSWER / 1024)));
    }
    let data = started.memory.data(&started.store);
    let answer = data
        .get(at..at.saturating_add(len))
        .ok_or_else(|| PluginError::Answer(crate::tr!("ответ вне памяти модуля")))?;
    serde_json::from_slice(answer).map_err(|_| PluginError::Answer(crate::tr!("ответ — не JSON")))
}

enum Job {
    Call { function: String, input: Value, kind: CallKind, reply: oneshot::Sender<Result<Value, PluginError>> },
    Memory { reply: oneshot::Sender<MemoryStats> },
}

struct Instance {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    exports: Vec<String>,
}

fn stopped() -> PluginError {
    PluginError::Unavailable(crate::tr!("приложение выгружено"))
}

impl PluginInstance for Instance {
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>> {
        let sender = self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let function = function.to_string();
        Box::pin(async move {
            let sender = sender.ok_or_else(stopped)?;
            let limit = kind.limit();
            let deadline = tokio::time::Instant::now() + limit;
            let (reply, answer) = oneshot::channel();
            let timeout = || PluginError::Timeout(limit.as_millis() as u64);
            match tokio::time::timeout_at(deadline, sender.send(Job::Call { function, input, kind, reply })).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(stopped()),
                Err(_) => return Err(timeout()),
            }
            match tokio::time::timeout_at(deadline + GRACE, answer).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(stopped()),
                Err(_) => Err(timeout()),
            }
        })
    }

    fn memory(&self) -> BoxFuture<'static, Option<MemoryStats>> {
        let sender = self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
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
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    }
}

impl PluginRuntime for Wasm {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn start(
        &self,
        code: PluginCode,
        host: Arc<dyn HostApi>,
    ) -> BoxFuture<'static, Result<Arc<dyn PluginInstance>, PluginError>> {
        let handle = tokio::runtime::Handle::try_current().ok();
        Box::pin(async move {
            let (jobs, mut receiver) = mpsc::channel::<Job>(QUEUE);
            let (ready, started) = oneshot::channel::<Result<Vec<String>, PluginError>>();
            let limit = code.memory_mb * 1024 * 1024;
            std::thread::Builder::new()
                .name(format!("plugin-{}", code.id))
                .stack_size(1024 * 1024)
                .spawn(move || {
                    let loaded = start(&code, host, handle);
                    let mut module = match loaded {
                        Ok(module) => {
                            let _ = ready.send(Ok(handlers(&module)));
                            module
                        }
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    while let Some(job) = receiver.blocking_recv() {
                        match job {
                            Job::Call { function, input, kind, reply } => {
                                let began = Instant::now();
                                let result = call(&mut module, &function, &input, kind);
                                if began.elapsed() > kind.limit() + GRACE {
                                    tracing::warn!(target: "plugins", "{}: {function} ran {} ms", code.id, began.elapsed().as_millis());
                                }
                                let _ = reply.send(result);
                            }
                            Job::Memory { reply } => {
                                let size = module.memory.data_size(&module.store) as u64;
                                let _ = reply.send(MemoryStats { allocated: size, used: size, limit });
                            }
                        }
                    }
                })
                .map_err(|error| PluginError::Load(crate::tr!("поток не запустился: {error}", error = error)))?;
            let exports = match tokio::time::timeout(LOAD_LIMIT + GRACE, started).await {
                Ok(Ok(loaded)) => loaded?,
                Ok(Err(_)) => return Err(PluginError::Load(crate::tr!("поток приложения завершился"))),
                Err(_) => return Err(PluginError::Timeout(LOAD_LIMIT.as_millis() as u64)),
            };
            Ok(Arc::new(Instance { jobs: Mutex::new(Some(jobs)), exports }) as Arc<dyn PluginInstance>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    /// A stand-in host: a store in memory and the log.
    #[derive(Default)]
    struct Host {
        kv: Mutex<Map<String, Value>>,
        log: Mutex<Vec<String>>,
    }

    impl HostApi for Host {
        fn fetch(&self, _request: Value) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет права net".to_string()) })
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
        fn kv_keys(&self, _prefix: &str) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn say(&self, _text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
            Box::pin(async { Ok(false) })
        }
        fn settings(&self) -> Map<String, Value> {
            Map::new()
        }
        fn log(&self, level: &str, message: &str) {
            self.log.lock().unwrap().push(format!("{level}: {message}"));
        }
    }

    /// A module by hand: `alloc` bumps a pointer; `greet` writes a constant answer, logs and stores its input;
    /// `spin` never ends.
    const MODULE: &str = r#"
      (module
        (import "host" "log" (func $log (param i32 i32 i32)))
        (import "host" "kv_set" (func $kv_set (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (data (i32.const 16) "\"Привет из WebAssembly\"")
        (data (i32.const 128) "last")
        (func (export "alloc") (param $len i32) (result i32)
          (local $at i32)
          (local.set $at (global.get $next))
          (global.set $next (i32.add (global.get $next) (local.get $len)))
          (local.get $at))
        (func (export "greet") (param $ptr i32) (param $len i32) (result i64)
          (call $log (i32.const 0) (local.get $ptr) (local.get $len))
          (drop (call $kv_set (i32.const 128) (i32.const 4) (local.get $ptr) (local.get $len)))
          (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const 31)))
        (func (export "spin") (param i32 i32) (result i64)
          (loop $forever (br $forever))
          (i64.const 0))
      )
    "#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runs_a_modules_handlers_with_the_hosts_store_and_stops_one_that_runs_away() {
        let host = Arc::new(Host::default());
        let code = PluginCode {
            id: "wasm-test".into(),
            version: "1.0.0".into(),
            bytes: MODULE.as_bytes().to_vec(),
            memory_mb: 2,
            ..PluginCode::default()
        };
        let instance = Wasm.start(code, host.clone()).await.unwrap();
        assert_eq!(instance.exports(), ["greet", "spin"]);
        let answer = instance.call("greet", json!({ "name": "Маша" }), CallKind::Voice).await.unwrap();
        assert_eq!(answer, json!("Привет из WebAssembly"));
        assert_eq!(host.kv.lock().unwrap()["last"], json!({ "name": "Маша" }));
        assert!(host.log.lock().unwrap()[0].contains("Маша"));
        assert!(instance.memory().await.unwrap().allocated >= 65_536);
        assert!(matches!(instance.call("nothing", json!({}), CallKind::Voice).await, Err(PluginError::Missing(_))));
        // Code that runs away ends with its fuel (or its call's time, in a slow debug build): either way a timeout,
        // after which the host unloads the plugin.
        let spun = instance.call("spin", json!({}), CallKind::Voice).await;
        assert!(matches!(spun, Err(PluginError::Timeout(_))), "{spun:?}");
        let broken =
            PluginCode { id: "broken".into(), bytes: b"not wasm".to_vec(), memory_mb: 2, ..PluginCode::default() };
        assert!(matches!(Wasm.start(broken, host).await, Err(PluginError::Load(_))));
    }
}
