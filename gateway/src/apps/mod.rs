//! Apps (docs/plans/plugins.md, «Приложения: свой бинарник отдельным процессом», stage 9): packages of `kind: app`,
//! `runtime: process` — a program of their own beside the gateway, for native code and other protocols (Spotify, AirPlay,
//! DLNA). The plugin host switches one on as a plugin; its engine is the supervisor here (supervisor.rs), and its way to
//! the Host API is HTTP on this machine with its own token (host_http.rs). In the station's build too: an app may run on
//! the station when it fits its memory, or on the computer (`prefer: home`).

pub mod host_http;
pub mod supervisor;

pub use supervisor::{AppTokens, ProcessRuntime};
