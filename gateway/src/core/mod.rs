//! The core: pure types and rules every module shares — events, identity and rights, requests, settings, presence.

pub mod availability;
pub mod build;
pub mod connectors;
pub mod events;
pub mod identity;
pub mod requests;
pub mod settings;
pub mod stations;

/// A text of a static table (its source language is Russian) in the language of now: the table keeps the source, the
/// catalog (locales/en/services.json) has its translation by the same text.
pub fn tx(text: &'static str) -> String {
    crate::locale::text::translate(text, &[])
}

/// `#[serde(serialize_with = "crate::core::translated")]` on a `&'static str` field of a static table.
pub fn translated<S: serde::Serializer>(text: &&'static str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&tx(text))
}

/// The same for a text that is not a literal (a built-in's title kept in a struct): looked up as it is, nothing kept.
pub fn tx_dyn(text: &str) -> String {
    crate::locale::text::translate_str(text)
}
