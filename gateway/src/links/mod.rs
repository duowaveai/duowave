//! How the modules ported apart reach each other, as the Node gateway's Nest modules injected one service into
//! another. Each module defined a small trait for what it needs of another (the models of the voice, the skills of
//! the dialog and the calls, the calls of the stations, the live channel of the stations' presence, /health and the
//! connectors of everyone); here those traits are implemented over the real services, and `App::build` passes them in
//! (src/app.rs, «Links between the modules»).
//!
//! Every link is a thin adapter: it converts the other module's types and calls it, nothing more. The station link
//! (src/gateway/uniproxy, mode) adds its own when it stands: its sessions to /health, the connectors and the
//! skills' speaker.
//!
//! The `*.module.ts` named in src/links are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

pub mod bus_words;
pub mod connectors;
pub mod dialog;
pub mod health;
pub mod playback;
pub mod plugins;
pub mod stations;

#[cfg(feature = "pc")]
pub mod calls;
#[cfg(feature = "pc")]
pub mod voice;
