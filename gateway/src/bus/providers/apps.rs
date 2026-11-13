//! Apps on the bus (`app/<id>`): the plugins that are on. What an app can do are its functions — the tools it gives
//! «Мозг», the functions its panel part calls, the one that answers in words — and the news it tells. What it asked
//! for is its manifest's `see:/listen:/call:/edit:` rights (Host API 2): asked of a person the first time, like a
//! phone; a built-in app — ours, in the gateway's build — has them from the start, and the owner may take them away
//! in «Связи».

use std::sync::{OnceLock, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, News, Participant, Provider, Words};
use crate::bus::trail::Authority;
use crate::bus::Level;
use crate::core::identity::permissions::use_of;
use crate::plugins::manifest::{Manifest, PackageKind, Permission};
use crate::plugins::runtime::CallKind;
use crate::plugins::PluginHost;

pub struct Apps {
    host: OnceLock<Weak<PluginHost>>,
    /// Answers in words for an app without its own `ask` (src/links/bus_words.rs).
    words: OnceLock<std::sync::Arc<dyn Words>>,
}

impl Default for Apps {
    fn default() -> Apps {
        Apps::new()
    }
}

/// The functions an app's panel part may call (`contributes.panel.calls`).
pub fn panel_calls(manifest: &Manifest) -> Vec<String> {
    manifest
        .contributes
        .panel
        .as_ref()
        .and_then(|panel| panel.get("calls"))
        .and_then(Value::as_array)
        .map(|calls| calls.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// What an app asked for on the bus, towards `to`.
pub fn asked(manifest: &Manifest, to: &Address) -> Level {
    manifest
        .permissions
        .iter()
        .filter_map(|permission| match permission {
            Permission::Bus { level, pattern } if pattern.matches(to) => Some(*level),
            _ => None,
        })
        .max()
        .unwrap_or(Level::None)
}

impl Apps {
    pub fn new() -> Apps {
        Apps { host: OnceLock::new(), words: OnceLock::new() }
    }

    /// Who answers in words for an app without its own `ask`: the household's model with the app's tools.
    pub fn set_words(&self, words: std::sync::Arc<dyn Words>) {
        let _ = self.words.set(words);
    }

    /// The plugin host, once it stands (it is made after the bus).
    pub fn set_host(&self, host: Weak<PluginHost>) {
        let _ = self.host.set(host);
    }

    fn host(&self) -> Option<std::sync::Arc<PluginHost>> {
        self.host.get().and_then(Weak::upgrade)
    }

    fn manifest(&self, address: &Address) -> Option<std::sync::Arc<Manifest>> {
        if address.kind() != address::APP {
            return None;
        }
        let id = address.name()?;
        // `app/<id>` itself: an app's deeper addresses are its own business.
        if address.path() != id {
            return None;
        }
        let manifest = self.host()?.package(id)?.manifest.clone();
        // A character or a theme is a package, not a participant: it has no functions to reach.
        matches!(manifest.kind, PackageKind::Plugin | PackageKind::App).then_some(manifest)
    }

    /// The function a handle runs.
    fn function_of(manifest: &Manifest, handle: &str) -> Option<String> {
        if let Some(tool) = manifest.contributes.tools.iter().find(|tool| tool.name == handle) {
            return Some(tool.handler.clone());
        }
        if panel_calls(manifest).iter().any(|call| call == handle) {
            return Some(handle.to_string());
        }
        if handle == "ask" {
            return manifest.contributes.bus.ask.clone();
        }
        None
    }
}

impl Provider for Apps {
    fn kind(&self) -> &'static str {
        address::APP
    }

    fn participants(&self) -> Vec<Participant> {
        let Some(host) = self.host() else { return Vec::new() };
        host.ids()
            .into_iter()
            .filter_map(|id| {
                let address = Address::app(&id);
                let manifest = self.manifest(&address)?;
                Some(Participant::new(address, &manifest.title))
            })
            .collect()
    }

    fn participant(&self, address: &Address) -> Option<Participant> {
        let manifest = self.manifest(address)?;
        Some(Participant::new(address.clone(), &manifest.title))
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        let Some(manifest) = self.manifest(address) else { return Vec::new() };
        let bus = &manifest.contributes.bus;
        let private = |name: &str| bus.private.iter().any(|known| known == name);
        let marked = |name: &str| bus.danger.get(name).copied();
        let mut handles = Vec::new();
        for tool in &manifest.contributes.tools {
            let title: String = tool.description.chars().take(120).collect();
            handles.push(
                Handle::new(&tool.name, Level::Call)
                    .titled(&title)
                    .dangerous(marked(&tool.name).or(tool.danger))
                    .private(private(&tool.name))
                    .needs(Some(use_of(tool.capability))),
            );
        }
        for call in panel_calls(&manifest) {
            if handles.iter().any(|handle: &Handle| handle.name == call) {
                continue;
            }
            handles.push(
                Handle::new(&call, Level::Call)
                    .dangerous(marked(&call))
                    .private(private(&call))
                    .needs(Some("use:plugins".into())),
            );
        }
        // Its own answer in words, or the household's model with its tools when it has any.
        if bus.ask.is_some() || (!manifest.contributes.tools.is_empty() && self.words.get().is_some()) {
            handles.push(Handle::new("ask", Level::Call).titled("ответить словами").private(private("ask")));
        }
        for news in &bus.emits {
            handles.push(Handle::new(news, Level::Listen).titled("новость"));
        }
        handles
    }

    fn declared(&self, from: &Address, to: &Address) -> Level {
        self.manifest(from).map(|manifest| asked(&manifest, to)).unwrap_or(Level::None)
    }

    /// A built-in app — in the gateway's build — has what it asked for from the start; any other asks a person.
    fn implicit(&self, from: &Address, to: &Address) -> Level {
        let (Some(manifest), Some(host)) = (self.manifest(from), self.host()) else { return Level::None };
        let built_in = matches!(
            host.origin_of(from.name().unwrap_or_default()),
            crate::plugins::registry::Origin::System | crate::plugins::registry::Origin::Bundled
        );
        if built_in {
            asked(&manifest, to)
        } else {
            Level::None
        }
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        let host = self.host();
        let manifest = self.manifest(&letter.to);
        let words = self.words.get().cloned();
        Box::pin(async move {
            let host = host.ok_or_else(|| BusError::Failed("приложения остановлены".into()))?;
            let manifest = manifest.ok_or_else(|| BusError::NotFound(letter.to.to_string()))?;
            if letter.handle == "ask" && manifest.contributes.bus.ask.is_none() {
                let words = words.ok_or_else(|| BusError::NotFound(format!("{}: ask", letter.to)))?;
                let text = letter.input.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    return Err(BusError::Failed("вопрос пустой".into()));
                }
                return words.answer(letter.trail, letter.from, letter.to, text).await;
            }
            let function = Apps::function_of(&manifest, &letter.handle)
                .ok_or_else(|| BusError::NotFound(format!("{}: {}", letter.to, letter.handle)))?;
            let id = letter.to.name().unwrap_or_default().to_string();
            let kind = if letter.trail.authority() == Authority::App { CallKind::Background } else { CallKind::Tool };
            let mut input = letter.input;
            if letter.handle == "ask" {
                input = json!({ "text": input.get("text").cloned().unwrap_or(input.clone()), "from": letter.from.to_string() });
            }
            if !letter.files.is_empty() {
                if let Value::Object(fields) = &mut input {
                    fields.insert(
                        "$files".into(),
                        Value::Array(letter.files.iter().map(|file| Value::String(file.to_string())).collect()),
                    );
                }
            }
            host.call(&id, &function, input, kind).await.map_err(|error| BusError::Failed(error.to_string()))
        })
    }

    fn listeners(&self, from: &Address, event: &str) -> Vec<Address> {
        let Some(host) = self.host() else { return Vec::new() };
        host.ids()
            .into_iter()
            .filter_map(|id| {
                let manifest = self.manifest(&Address::app(&id))?;
                manifest
                    .contributes
                    .bus
                    .on
                    .iter()
                    .any(|on| on.event == event && on.from.matches(from))
                    .then(|| Address::app(&id))
            })
            .collect()
    }

    fn deliver(&self, to: &Address, news: News) -> BoxFuture<'static, Result<(), BusError>> {
        let host = self.host();
        let manifest = self.manifest(to);
        let to = to.clone();
        Box::pin(async move {
            let host = host.ok_or_else(|| BusError::Failed("приложения остановлены".into()))?;
            let manifest = manifest.ok_or_else(|| BusError::NotFound(to.to_string()))?;
            let handler = manifest
                .contributes
                .bus
                .on
                .iter()
                .find(|on| on.event == news.event && on.from.matches(&news.from))
                .map(|on| on.handler.clone())
                .ok_or_else(|| BusError::NotFound(format!("{to}: {}", news.event)))?;
            let id = to.name().unwrap_or_default().to_string();
            let input = json!({ "from": news.from.to_string(), "event": news.event, "payload": news.payload });
            host.call(&id, &handler, input, CallKind::Background)
                .await
                .map(|_| ())
                .map_err(|error| BusError::Failed(error.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(permissions: &[&str]) -> Manifest {
        let value = json!({
            "schema": 1, "id": "morning", "version": "1.0.0", "title": "Утро", "api": "^2", "runs": ["pc"],
            "permissions": permissions,
        });
        crate::plugins::manifest::check(&value).unwrap()
    }

    #[test]
    fn what_an_app_asked_for_is_its_rights_on_the_bus_and_nothing_else() {
        let to = |text: &str| Address::parse(text).unwrap();
        let rights = manifest(&["call:app/calendar", "listen:files/home/Фото/*", "see:*", "net:example.com"]);
        assert_eq!(asked(&rights, &to("app/calendar")), Level::Call);
        assert_eq!(asked(&rights, &to("files/home/Фото/лето.png")), Level::Listen);
        assert_eq!(asked(&rights, &to("app/radio")), Level::See);
        // The old words of Host API 1 are no rights any more: the manifest says what they became.
        let words = ["voice", "say"].join(".");
        let old = json!({
            "schema": 1, "id": "morning", "version": "1.0.0", "title": "Утро", "api": "^2", "runs": ["pc"],
            "permissions": [words],
        });
        let problems = crate::plugins::manifest::check(&old).unwrap_err();
        assert!(problems.problems.iter().any(|problem| problem.contains("call:system/voice")), "{problems:?}");
    }
}
