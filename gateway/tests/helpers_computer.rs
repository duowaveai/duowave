//! The computer's stages of «Дом» и помощники end to end (docs/plans/home-and-helpers.md): the built-in helper, the
//! link, the computer's side of the home — one test binary of the computer's build (gateway/Cargo.toml,
//! `required-features`); the shared stages are tests/helpers.rs.

mod support;

#[path = "helpers/builtin.rs"]
mod builtin;
#[path = "helpers/computer.rs"]
mod computer;
#[path = "helpers/link.rs"]
mod link;
#[path = "helpers/stand.rs"]
mod stand;
