//! The characters' long-term memory (docs/plans/memory.md): the household graph — who is who, where, with whom,
//! with dates — the «wake-up» in the prompt, the memory tools and voice commands, the panel's «Память» window, and
//! the reflection, the sleep's part that thinks the day over at night. Worlds from packages and search by meaning come
//! in later stages of the plan.
//!
//! Every record keeps who said it, where, who could hear it, who sees it and whose it is, and every query filters by
//! what the asker may see, so the memory can split by the people of the house later (the plan's «Люди дома и
//! приватность»).

pub mod commands;
pub mod context;
pub mod controller;
pub mod model;
pub mod reflect;
pub mod replies;
pub mod service;
pub mod store;
pub mod text;
pub mod tools;

pub use controller::routes;
pub use service::{MemoryError, MemoryService, Remember, Remembered};
