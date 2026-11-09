//! «Мозг»: the model providers the panel adds (presets and custom ones, keys in secrets as `llm:<id>:key`) and the
//! choice of the fast and the smart model.

pub mod brain_controller;
pub mod brain_service;
pub mod presets;

pub use brain_service::{BrainService, BrainView, ProviderCheck, ProviderInput, ProviderView};
