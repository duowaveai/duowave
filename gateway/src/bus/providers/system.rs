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
/// Cloud and paid voices: not a service of its own but the owner's «да» for them, asked when an app's line goes to
/// such a voice (capabilities/voices.rs).
const VOICE_CLOUD: &str = crate::capabilities::voices::CLOUD;
/// The station's player: an app that plays by itself (AirPlay) says so, and hears the player's commands as the news
/// `control` from here (docs/plans/media-apps.md, stage 2).
pub const PLAYER: &str = "system/player";
const NOTIFY: &str = "system/notify";
const SCHEDULE: &str = "system/schedule";
const SMART_HOME: &str = "system/smart-home";
const CONTENT: &str = "system/content";
const HOME: &str = "system/home";
/// Publishing without leaving DuoWave: the registry of publishers (capabilities/publish.rs) — Telegram by the gateway's
/// bot, the page «смотри и выбирай», the platforms' upload pages; `publishers` lists them, `<id>` sends by one.
pub const PUBLISH: &str = "system/publish";
/// Hearing for apps: a file of the app's folder → text, language, each word's time (capabilities/hearing.rs).
const HEARING: &str = "system/hearing";
/// The household's memory, for an app: only its interests (docs/plans/feed.md, stage 10), each app by the owner's «да».
const MEMORY: &str = "system/memory";
/// The gateway's way to the internet (src/egress, «Выход в интернет»): an app with its own servers («VPN») gives the
/// gateway its connections here; the owner's «да» to «VPN → Сеть шлюза» first, and until then everything goes directly.
pub const NETWORK: &str = crate::egress::NETWORK;
/// The station's processed microphone, for an app on the station that hears a word of its own (src/activator): where
/// it is. It listens to the room all the time — a camera's or a microphone's danger, the owner's «да» first.
pub const MIC: &str = "system/mic";
/// A word of an app's own that wakes the station as its button does (src/activator, `contributes.activators`).
pub const ACTIVATOR: &str = "system/activator";
/// The device the gateway runs on and the others of the home (operations/device_state.rs): temperature, memory,
/// storage, processor, uptime — only to see, cheap, for widgets and graphs.
pub const DEVICE: &str = crate::operations::device_state::ADDRESS;
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
        _ => Err(BusError::Denied(crate::tr!("службы DuoWave работают для приложений"))),
    }
}

impl Provider for System {
    fn kind(&self) -> &'static str {
        address::SYSTEM
    }

    fn participants(&self) -> Vec<Participant> {
        [
            (VOICE, crate::tr!("Голос колонки")),
            (VOICE_CLOUD, crate::tr!("Облачные голоса: текст реплик уходит поставщику, тратит его тариф")),
            (PLAYER, crate::tr!("Плеер колонки")),
            (NOTIFY, crate::tr!("Сообщения владельцу")),
            (SCHEDULE, crate::tr!("Расписание приложений")),
            (SMART_HOME, crate::tr!("Умный дом")),
            (CONTENT, crate::tr!("Музыка и сказки семьи")),
            (HOME, crate::tr!("Дом: место погоды, ленты новостей")),
            (BRAIN, crate::tr!("Модель «Мозга»")),
            (MEMORY, crate::tr!("Память: интересы дома")),
            (HEARING, crate::tr!("Слух: распознать речь в своём файле")),
            (PUBLISH, crate::tr!("Публикация: Telegram, страница «смотри и выбирай», страницы площадок")),
            (NETWORK, crate::tr!("Сеть центра: выход в интернет через ваши серверы")),
            (MIC, crate::tr!("Микрофон колонки: слышит комнату всё время")),
            (ACTIVATOR, crate::tr!("Слово-активатор: начать разговор с колонкой")),
            (DEVICE, crate::tr!("Состояние устройств: температура, память, место, процессор")),
        ]
        .iter()
        .filter_map(|(text, title)| Some(Participant::new(Address::parse(text).ok()?, title)))
        .collect()
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        match address.as_str() {
            VOICE => vec![
                Handle::new("say", Level::Call).titled(&crate::tr!("сказать вслух без запроса")),
                Handle::new("synth", Level::Call)
                    .titled(&crate::tr!("озвучить строку в свою папку: голосом компьютера или облачным")),
                Handle::new("voices", Level::Call).titled(&crate::tr!("голоса и их лицензии: компьютера и облачные")),
                Handle::new("providers", Level::Call)
                    .titled(&crate::tr!("кто озвучивает: готов ли, сколько осталось на тарифе")),
            ],
            VOICE_CLOUD => {
                vec![Handle::new("synth", Level::Call).titled(&crate::tr!("озвучить облачным или платным голосом"))]
            }
            PLAYER => vec![Handle::new("playing", Level::Call)
                .titled(&crate::tr!("сказать, что играет сам: Алиса встаёт на паузу, голос управляет приложением"))],
            NOTIFY => vec![Handle::new("send", Level::Call).titled(&crate::tr!("написать владельцу"))],
            SCHEDULE => ["set", "list", "cancel", "parse"]
                .iter()
                .map(|name| Handle::new(name, Level::Call).titled(&crate::tr!("своё расписание")))
                .collect(),
            SMART_HOME => vec![
                Handle::new("devices", Level::Call)
                    .titled(&crate::tr!("устройства дома"))
                    .needs(Some("use:smart_home".into())),
                Handle::new("act", Level::Call)
                    .titled(&crate::tr!("включить, выключить"))
                    .needs(Some("use:smart_home".into())),
            ],
            CONTENT => vec![
                Handle::new("music", Level::Call).titled(&crate::tr!("своя музыка")).needs(Some("use:music".into())),
                Handle::new("tales", Level::Call).titled(&crate::tr!("сказки семьи")).needs(Some("use:tales".into())),
            ],
            HOME => vec![
                Handle::new("weather-place", Level::Call).titled(&crate::tr!("город погоды")),
                Handle::new("news-feeds", Level::Call).titled(&crate::tr!("ленты новостей")),
            ],
            MEMORY => {
                vec![Handle::new("interests", Level::Call).titled(&crate::tr!("что дом любит смотреть и читать"))]
            }
            HEARING => vec![Handle::new("recognize", Level::Call)
                .titled(&crate::tr!("распознать речь в своём файле: слова со временем"))],
            NETWORK => vec![Handle::new("connections", Level::Call)
                .titled(&crate::tr!("дать центру свои подключения: через них он ходит в интернет по вашим правилам"))],
            MIC => vec![Handle::new("open", Level::Call)
                .titled(&crate::tr!("слушать микрофон колонки: узнать, где его звук"))
                .dangerous(Some(crate::bus::danger::Danger::Camera))
                .standing()],
            ACTIVATOR => vec![Handle::new("wake", Level::Call)
                .titled(&crate::tr!("разбудить колонку своим словом: она слушает, как после кнопки"))],
            DEVICE => vec![
                Handle::new("state", Level::See).titled(&crate::tr!(
                    "состояние устройства: температура, память, место, процессор, время работы, имя и роль"
                )),
                Handle::new("devices", Level::See).titled(&crate::tr!("все устройства дома, что видны отсюда")),
                Handle::new("history", Level::See)
                    .titled(&crate::tr!("история за несколько часов, точка в минуту — для графика")),
            ],
            PUBLISH => vec![
                Handle::new("publishers", Level::Call)
                    .titled(&crate::tr!("кто может отправить: что берёт, нужно ли подключение")),
                Handle::new("telegram", Level::Call).titled(&crate::tr!("отправить ролик в Telegram своим ботом")),
                Handle::new("watch", Level::Call).titled(&crate::tr!("страница «смотри и выбирай» в DuoWave")),
            ],
            BRAIN => vec![
                Handle::new("speak", Level::Call).titled(&crate::tr!("ответить моделью словами приложения")),
                Handle::new("write", Level::Call).titled(&crate::tr!("написать текст моделью «Мозга»")),
                Handle::new("models", Level::Call).titled(&crate::tr!("какие модели есть, чтобы выбрать свою")),
                Handle::new("agent", Level::Call).titled(&crate::tr!("агент приложения: модель с файлами его папки")),
            ],
            _ => Vec::new(),
        }
    }

    fn owns(&self, _from: &Address, _to: &Address) -> bool {
        false
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        let services = self.services();
        Box::pin(async move {
            let services = services.ok_or_else(|| BusError::Failed(crate::tr!("службы DuoWave ещё не готовы")))?;
            let id = caller(&letter)?;
            let input = letter.input;
            let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
            let failed = BusError::Failed;
            match (letter.to.as_str(), letter.handle.as_str()) {
                (VOICE, "say") => services
                    .say(&id, text("text").unwrap_or_default(), text("device"), text("file"))
                    .await
                    .map(Value::Bool)
                    .map_err(failed),
                (VOICE, action @ ("synth" | "voices" | "providers")) => {
                    services.synth(&id, action, input.clone()).await.map_err(failed)
                }
                // The «да» was the bus's, for this very letter: the line goes as through `system/voice`.
                (VOICE_CLOUD, "synth") => services.synth(&id, "synth", input.clone()).await.map_err(failed),
                // Any publisher of the registry by its id: the gateway's own today, a new one tomorrow.
                (PUBLISH, action) => services.publish(&id, action, input.clone()).await.map_err(failed),
                (MEMORY, "interests") => services.interests(&id).await.map_err(failed),
                (HEARING, "recognize") => services.recognize(&id, input.clone()).await.map_err(failed),
                (BRAIN, "write") => services.write(&id, input.clone()).await.map_err(failed),
                (BRAIN, "models") => services.models().await.map_err(failed),
                (BRAIN, "agent") => services.agent(&id, input.clone()).await.map_err(failed),
                (PLAYER, "playing") => services
                    .playing(
                        &id,
                        text("state").unwrap_or_default(),
                        text("device"),
                        crate::skills::media::app_playback::AppTrack::from_letter(&input),
                    )
                    .await
                    .map(|device| serde_json::json!({ "device": device }))
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
                (NETWORK, "connections") => services.network_connections(&id, input.clone()).map_err(failed),
                (MIC, "open") => services.mic(&id).map_err(failed),
                (DEVICE, action) => services.device(&id, action, input.clone()).await.map_err(failed),
                (ACTIVATOR, "wake") => services.wake(&id, input.clone()).await.map_err(failed),
                (BRAIN, _) => Err(BusError::Failed(crate::tr!(
                    "модель отвечает словами приложения полем model в ответе на фразу, не письмом"
                ))),
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
            "system/memory",
            "system/publish",
            "system/network",
            "system/mic",
            "system/activator",
            "system/device",
        ] {
            assert!(addresses.iter().any(|known| known == address), "{address}");
        }
        let smart = system.handles(&Address::parse("system/smart-home").unwrap());
        assert!(smart.iter().all(|handle| handle.needs.as_deref() == Some("use:smart_home")));
    }

    #[test]
    fn the_devices_state_is_only_to_see_and_never_dangerous() {
        let handles = System::new().handles(&Address::parse("system/device").unwrap());
        let names: Vec<&str> = handles.iter().map(|handle| handle.name.as_str()).collect();
        assert_eq!(names, ["state", "devices", "history"]);
        assert!(handles.iter().all(|handle| handle.level == Level::See && handle.danger.is_none()));
        assert!(handles.iter().all(|handle| handle.needs.is_none()), "a guest's widget shows it too");
    }
}
