//! Block 1 of «Дом» и помощники end to end (docs/plans/home-and-helpers.md): one test binary, a module per stage in
//! tests/helpers/, so the stages' tests do not share a file. One binary runs many gateways at once: the tests assert
//! causes and wait with `support::until`; no network beyond 127.0.0.1.

mod support;

#[path = "helpers/choice.rs"]
mod choice;
#[path = "helpers/home.rs"]
mod home;
#[path = "helpers/model.rs"]
mod model;
#[path = "helpers/stand.rs"]
mod stand;
#[path = "helpers/works.rs"]
mod works;
