//! The gateway's services on the bus (`system/…`, Host API 2): speech without a request, a message to the owner, the
//! app's own schedule, the smart home, the household's music and tales, the panel's place for the weather and news
//! feeds, the household's model. An app reaches them only by letters, each decided by the bus; each handle says the
//! person's right a person's chain must hold: a guest's request cannot reach the smart home through an app
//! (docs/plans/bus.md, «Зачем», дыра 1).

use std::sync::{Arc, OnceLock, Weak};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, Participant, Provider};
use crate::bus::Level;
use crate::plugins::host_api::HostServices;

const VOICE: &str = "system/voice";
const NOTIFY: &str = "system/notify";
const SCHEDULE: &str = "system/schedule";
const SMART_HOME: &str = "system/smart-home";
const CONTENT: &str = "system/content";
const HOME: &str = "system/home";
/// The household's model answering in an app's words (the answer's `model`, plugins/skill.rs): a participant for the
/// bus's word on it, not a letter's target.
pub const BRAIN: &str = "system/brain";

pub struct System {
    services: OnceLock<Weak<dyn HostServices>>,
}

impl Default for System {
    fn default() -> System {
        System::new()
    }
}

impl System {
    pub fn new() -> System {
        System { services: OnceLock::new() }
    }

    /// The gateway's services, once they stand.
    pub fn set_services(&self, services: Weak<dyn HostServices>) {
        let _ = self.services.set(services);
    }

    fn services(&self) -> Option<Arc<dyn HostServices>> {
        self.services.get().and_then(Weak::upgrade)
    }
}

/// The app a letter comes from: the services act for an app (its schedule, its message's name).
fn caller(letter: &Letter) -> Result<String, BusError> {
    match (letter.from.kind(), letter.from.name()) {
        (address::APP, Some(id)) => Ok(id.to_string()),
        _ => Err(BusError::Denied("службы шлюза работают для приложений".into())),
    }
}

impl Provider for System {
    fn kind(&self) -> &'static str {
        address::SYSTEM
    }

    fn participants(&self) -> Vec<Participant> {
        [
            (VOICE, "Голос колонки"),
            (NOTIFY, "Сообщения владельцу"),
            (SCHEDULE, "Расписание приложений"),
            (SMART_HOME, "Умный дом"),
            (CONTENT, "Музыка и сказки семьи"),
            (HOME, "Место погоды и ленты новостей"),
            (BRAIN, "Модель «Мозга»"),
        ]
        .iter()
        .filter_map(|(text, title)| Some(Participant::new(Address::parse(text).ok()?, title)))
        .collect()
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        match address.as_str() {
            VOICE => vec![Handle::new("say", Level::Call).titled("сказать вслух без запроса")],
            NOTIFY => vec![Handle::new("send", Level::Call).titled("написать владельцу")],
            SCHEDULE => ["set", "list", "cancel", "parse"]
                .iter()
                .map(|name| Handle::new(name, Level::Call).titled("своё расписание"))
                .collect(),
            SMART_HOME => vec![
                Handle::new("devices", Level::Call).titled("устройства дома").needs(Some("use:smart_home".into())),
                Handle::new("act", Level::Call).titled("включить, выключить").needs(Some("use:smart_home".into())),
            ],
            CONTENT => vec![
                Handle::new("music", Level::Call).titled("своя музыка").needs(Some("use:music".into())),
                Handle::new("tales", Level::Call).titled("сказки семьи").needs(Some("use:tales".into())),
            ],
            HOME => vec![
                Handle::new("weather-place", Level::Call).titled("город погоды"),
                Handle::new("news-feeds", Level::Call).titled("ленты новостей"),
            ],
            BRAIN => vec![Handle::new("speak", Level::Call).titled("ответить моделью словами приложения")],
            _ => Vec::new(),
        }
    }

    fn owns(&self, _from: &Address, _to: &Address) -> bool {
        false
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        let services = self.services();
        Box::pin(async move {
            let services = services.ok_or_else(|| BusError::Failed("службы шлюза ещё не готовы".into()))?;
            let id = caller(&letter)?;
            let input = letter.input;
            let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
            let failed = BusError::Failed;
            match (letter.to.as_str(), letter.handle.as_str()) {
                (VOICE, "say") => services
                    .say(&id, text("text").unwrap_or_default(), text("device"))
                    .await
                    .map(Value::Bool)
                    .map_err(failed),
                (NOTIFY, "send") => services.notify(&id, text("text").unwrap_or_default()).await.map_err(failed),
                (SCHEDULE, action) => services.schedule(&id, action, input.clone()).await.map_err(failed),
                (SMART_HOME, "devices") => services.smart_home_devices().await.map_err(failed),
                (SMART_HOME, "act") => services
                    .smart_home_act(
                        text("device").unwrap_or_default(),
                        text("action").unwrap_or_default(),
                        input.get("value").and_then(Value::as_f64),
                    )
                    .await
                    .map(Value::String)
                    .map_err(failed),
                (HOME, "weather-place") => Ok(services.weather_place().map(Value::String).unwrap_or(Value::Null)),
                (HOME, "news-feeds") => Ok(Value::Array(services.news_feeds())),
                (BRAIN, _) => Err(BusError::Failed(
                    "модель отвечает словами приложения полем model в ответе на фразу, не письмом".into(),
                )),
                (CONTENT, folder @ ("music" | "tales")) => {
                    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 200) as usize;
                    let query = text("query").filter(|query| !query.is_empty());
                    Ok(Value::Array(services.content(folder, query, limit).await))
                }
                (to, handle) => Err(BusError::NotFound(format!("{to}: {handle}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_services_are_participants_and_the_smart_home_needs_the_persons_right() {
        let system = System::new();
        let addresses: Vec<String> = system.participants().iter().map(|one| one.address.to_string()).collect();
        for address in [
            "system/voice",
            "system/notify",
            "system/schedule",
            "system/smart-home",
            "system/content",
            "system/home",
            "system/brain",
        ] {
            assert!(addresses.iter().any(|known| known == address), "{address}");
        }
        let smart = system.handles(&Address::parse("system/smart-home").unwrap());
        assert!(smart.iter().all(|handle| handle.needs.as_deref() == Some("use:smart_home")));
    }
}
