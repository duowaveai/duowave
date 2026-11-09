# Porting the gateway from TypeScript

The Rust gateway (`gateway/`, crate `duowave`) is a port of the Node gateway. The plan and the owner's decisions are
in `../docs/plans/rust-gateway.md`. These are the rules every port followed.

**The TS original is removed from the tree.** It lives in git history at the tag `node-gateway-last` (the last commit
with it): `git show node-gateway-last:src/speech/voice.service.ts`, `git ls-tree -r node-gateway-last -- src test`.
Wherever this crate cites the original — a `src/…/*.ts` file, a `*.module.ts`, a `*.spec.ts`, a
`test/*.e2e-spec.ts`, `src/voice-lab`, drizzle's migrations, or how Nest answered — it means that tag. A `src/…`
path without `.ts` (`src/links`, `src/app.rs`, `src/tools`) is this crate's own. `migrations/0001_init.sql` cites
"drizzle 0009" and stays as it is: an applied migration's text is checksummed, an edit stops the gateway.

## What to port

- The logic: services, routes, protocol, storage, checks. Port behaviour, not syntax: the same inputs give the same
  outputs, the same HTTP answers (status, JSON shape, the Russian messages), the same events.
- Not: OpenTelemetry, Nest's modules/DI/pipes/filters/interceptors/logger (replaced by `app.rs`, `platform/http`
  and `tracing`), and code that only reads older Node data formats (legacy setting shapes, the first panel's token
  format, JSON state files from before SQLite): the Rust gateway starts its own database.
- The dev tools in `src/tools` came later, as `duowave` subcommands in this crate's `src/tools` (`duowave replay`,
  `journal`, `animations`, `call-sim`, `livekit-token`): the same options and output, the work done by the modules
  the gateway runs (the speech pipeline, the journal, the calls hub, the LiveKit token).
- Every `*.spec.ts` of a ported file becomes tests of its Rust module (`#[cfg(test)] mod tests`), one test per `it(…)`,
  named after it in snake_case. A test that cannot be ported as it is (it needs a module not ported yet) is ported
  against a fake, or listed in the report as left.

## Layout

- The module tree mirrors the Node gateway's `src/` with snake_case names: `src/gateway/protocol/aapi.codec.ts` →
  `src/gateway/protocol/aapi_codec.rs`, `src/health-status` → `src/health_status`. A folder is a module with `mod.rs`.
- Shared types already exist; use them, do not redefine them: `config` (every env variable), `core` (events, identity,
  requests, settings, presence), `skills::capabilities`, `dialog::llm::llm_client` (the model interface),
  `gateway::protocol::protocol_types` (the station protocol's model) and `gateway::protocol::pb` (the wire messages,
  generated from `proto/station-protocol.proto`).
- Comments: the TS comments carry over — they explain why. Plain, short English; no comment that only restates code.

## Conventions

- **Services** are structs built with their dependencies (`Arc<Db>`, `EventBus`, `Arc<SettingsService>`, …) and kept
  in `App` (`src/app.rs`) as `Arc<…>` fields. A dependency on a module that is not ported yet is a small trait in the
  depending module, implemented later; never a stub that pretends to work. Once both modules stand, the trait is
  implemented over the other's service in `src/links` (a thin adapter) and passed in `App::build`.
- **HTTP**: axum handlers on `Router<Arc<App>>`, `State(app): State<Arc<App>>`, `caller: Caller` for access, in the
  guard's order: `caller.zone_at_least(Zone::Home)?` (a TS `@Zone`), then `caller.permitted(permissions::X)?` (a TS
  `@RequirePermission`) or `caller.signed_in()?`; a `@Public()` route asks none. Answers: `ok(value)`, `created(value)`
  for a POST (Nest answered 201 unless the route had `@HttpCode`), `ApiError::…` for errors with the TS messages. A
  JSON body is `Body<T>` (empty is `{}`). Routes whose clients need their own format (the station's HTTP, `/media`,
  the call page, the station's scripts) answer without the envelope. JSON field names are camelCase as in the TS
  (`#[serde(rename_all = "camelCase")]`); `panel/src/api.ts` is the contract.
- **Database**: `app.db.with(|c| …)` / `app.db.transaction(|t| …)` with rusqlite; the tables are in
  `migrations/0001_init.sql`. A new table or column is a new migration file `migrations/NNNN_name.sql` (never an edit
  of an applied one); say so in the report. JSON columns are text (`serde_json`).
- **Events**: `app.events.emit(GatewayEvent::…)`; a listener is `events.on("label", |event| …)` or a task on
  `events.subscribe()`.
- **Time and ids**: `utils::time::now_iso()` (`2026-11-06T02:11:00.331Z`), `utils::time::now_ms()`,
  `utils::ids::uuid()`. Timers and timeouts: `tokio::time`; cancellation: `tokio_util::sync::CancellationToken`.
- **Outgoing HTTP**: `utils::http::client()` (or `client_builder()` for own timeouts) — rustls with ring and built-in
  roots, the same on the station. WebSockets out: `utils::http::ws_connector()`.
- **Errors**: services return `Result<_, ApiError>` where the TS threw HttpExceptions; `anyhow` for internal plumbing.
  Never `unwrap()` on input from the network, a file or the database.
- **Features**: code that needs a prebuilt native library (ONNX Runtime, libwebrtc) goes behind a cargo feature
  (`silero`, `livekit`, `webrtc`) and has a fallback without it. The station build is `--no-default-features
  --features station`: PC-only modules (local models, voice lab, calls, the station connect over SSH) are behind
  `pc`.
- **Regex**: the `regex` crate has no look-around; rewrite such patterns in code.
- **Secrets** never reach logs, events or API answers (`SecretsService::mask`, `redact`).

## Done means

- `cargo test -p duowave` green, `cargo clippy -p duowave --all-targets` without warnings, `cargo fmt` applied
  (`../rustfmt.toml`: width 120).
- A commit with what was ported, and in the report: what is left or behaves differently, and every change to a shared
  file (`Cargo.toml`, `src/lib.rs`, `src/app.rs`, `src/config/mod.rs`, a new migration).
- Nothing touches a real station: no ssh, scp or adb to one.
