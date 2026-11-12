//! The catalog of Alice's capabilities: every request belongs to one, and every capability has a route (ours,
//! Yandex's or "not yet"). Names are the ones ROUTES uses: `ROUTES=radio=yandex;calls=yandex`.

use serde::{Deserialize, Serialize};

macro_rules! capabilities {
    ($($variant:ident = $name:literal, $title:literal, station: $station:literal $(, display: $display:literal)?;)+) => {
        /// A capability of Alice, by the name ROUTES and the permissions (`use:<name>`) use.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum CapabilityName {
            $(#[serde(rename = $name)] $variant,)+
        }

        /// Every capability, in the catalog's order.
        pub const CAPABILITY_NAMES: &[CapabilityName] = &[$(CapabilityName::$variant,)+];

        impl CapabilityName {
            pub fn as_str(self) -> &'static str {
                match self { $(CapabilityName::$variant => $name,)+ }
            }

            pub fn info(self) -> Capability {
                match self {
                    $(CapabilityName::$variant => Capability {
                        name: CapabilityName::$variant,
                        title: $title,
                        station: $station,
                        display: false $(|| $display)?,
                    },)+
                }
            }
        }

        /// The capability of this name, when it is one.
        pub fn is_capability(name: &str) -> Option<CapabilityName> {
            match name { $($name => Some(CapabilityName::$variant),)+ _ => None }
        }
    };
}

capabilities! {
    Chat = "chat", "Разговор, время, дата", station: false;
    Volume = "volume", "Громкость", station: true;
    Playback = "playback", "Стоп, пауза, дальше, назад, продолжи", station: true;
    Radio = "radio", "Радио", station: true;
    Music = "music", "Музыка", station: true;
    Tales = "tales", "Сказки, аудиокниги", station: true;
    Games = "games", "Игры", station: false;
    Skills = "skills", "Навыки Диалогов", station: false;
    Timers = "timers", "Таймеры", station: true;
    Alarms = "alarms", "Будильники", station: true;
    Reminders = "reminders", "Напоминания", station: false;
    Later = "later", "Отложенные команды", station: false;
    Weather = "weather", "Погода", station: false, display: true;
    News = "news", "Новости", station: false;
    MorningShow = "morning_show", "Утреннее шоу", station: true;
    SmartHome = "smart_home", "Умный дом", station: false;
    Bluetooth = "bluetooth", "Bluetooth", station: true;
    Led = "led", "Подсветка", station: true;
    DoNotDisturb = "do_not_disturb", "Не беспокоить", station: true;
    Calls = "calls", "Звонки и голосовые сообщения", station: true;
    MusicRecognition = "music_recognition", "Что играет", station: true;
    Account = "account", "Сервисы аккаунта: списки, «найди телефон», покупки", station: false;
    Plugins = "plugins", "Приложения: свои навыки", station: false;
    Memory = "memory", "Память: «запомни», «забудь», «что ты помнишь»", station: false;
}

impl std::fmt::Display for CapabilityName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub name: CapabilityName,
    /// For people: /health and the docs.
    pub title: &'static str,
    /// Acts on the station with commands in the response, so the model offers its tools only for commands.
    pub station: bool,
    /// Shows its result on the station's display: its early response waits for the result a little longer.
    pub display: bool,
}

/// What a request asks for: a capability, an action in it and its parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub capability: CapabilityName,
    pub action: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl Intent {
    pub fn new(capability: CapabilityName, action: &str) -> Intent {
        Intent { capability, action: action.to_string(), params: serde_json::Map::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        assert_eq!(CAPABILITY_NAMES.len(), 24);
        for name in CAPABILITY_NAMES {
            assert_eq!(is_capability(name.as_str()), Some(*name));
            assert_eq!(serde_json::to_value(name).unwrap(), serde_json::json!(name.as_str()));
        }
        assert_eq!(is_capability("nope"), None);
        assert!(CapabilityName::Weather.info().display);
        assert!(!CapabilityName::Chat.info().station);
    }
}
