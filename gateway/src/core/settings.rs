//! A setting the gateway keeps in its database, declared once: its key, whose it is, how a stored value is read
//! back. Values are JSON; a stored value that does not parse (a hand edit, a format this version does not know)
//! reads as absent rather than breaking the gateway.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingScope {
    /// One value for the whole gateway (the routes, the mode).
    Global,
    /// A global value with a per-person override on top (a person's own voice or Alisa).
    User,
}

pub struct SettingDefinition<T> {
    pub key: &'static str,
    pub scope: SettingScope,
    /// For people: the panel and the audit log.
    pub description: &'static str,
    /// Never returned by any API, never written to the audit log (the token secret).
    pub sensitive: bool,
    /// A stored JSON value as T, or None when it is not one.
    pub parse: fn(&Value) -> Option<T>,
}

// Parsers for the common shapes.

pub fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

pub fn as_one_of(value: &Value, allowed: &[&str]) -> Option<String> {
    value.as_str().filter(|text| allowed.contains(text)).map(str::to_string)
}

pub fn as_string_record(value: &Value) -> Option<BTreeMap<String, String>> {
    let object = value.as_object()?;
    object.iter().map(|(key, entry)| entry.as_str().map(|text| (key.clone(), text.to_string()))).collect()
}

pub fn as_object(value: &Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

/// The strings of a JSON array, others dropped; None when it is not an array.
pub fn as_string_list(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
}
