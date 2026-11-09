//! The smart home through a provider (decision 5: Home Assistant first, others by the same interface). Nothing goes
//! to the station: the provider acts, Alice says it is done. Without a provider the capability is not set up and
//! the route falls to Yandex or "not yet".

use std::sync::{Arc, LazyLock};

use futures::future::BoxFuture;
use regex::Regex;

use super::provider::{HomeAction, HomeDevice, SmartHomeProvider};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::media::sources::match_score;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

/// Words that point at a kind of device when the name does not say it.
static KIND_WORDS: LazyLock<Vec<(Regex, &'static [&'static str])>> = LazyLock::new(|| {
    let kinds: [(&str, &'static [&'static str]); 8] = [
        ("свет|ламп|люстр|подсветк|торшер|бра", &["light", "switch"]),
        ("кондиционер|отоплени|обогрев|температур|термостат|батаре", &["climate"]),
        ("штор|жалюзи|ворот", &["cover"]),
        ("розетк|чайник", &["switch"]),
        ("вентилятор", &["fan"]),
        ("пылесос", &["vacuum"]),
        ("замок|двер", &["lock"]),
        ("сцен|режим", &["scene", "script"]),
    ];
    kinds.into_iter().map(|(pattern, kinds)| (Regex::new(pattern).expect("a valid pattern"), kinds)).collect()
});

/// The device the words name: by its name first, then by its kind.
pub fn pick_device<'a>(devices: &'a [HomeDevice], words: &str) -> Option<&'a HomeDevice> {
    let kinds: Vec<&str> = KIND_WORDS
        .iter()
        .filter(|(pattern, _)| pattern.is_match(words))
        .flat_map(|(_, kinds)| kinds.iter().copied())
        .collect();
    let mut scored: Vec<(f64, &HomeDevice)> = devices
        .iter()
        .map(|device| {
            let bonus = if kinds.contains(&device.kind.as_str()) { 0.25 } else { 0.0 };
            (match_score(words, &device.name) + bonus, device)
        })
        .filter(|(score, _)| *score >= 0.5)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.first().map(|(_, device)| *device)
}

fn done(action: HomeAction) -> &'static str {
    match action {
        HomeAction::On => "Включила",
        HomeAction::Off => "Выключила",
        HomeAction::SetBrightness => "Поставила яркость",
        HomeAction::SetTemperature => "Поставила температуру",
        HomeAction::Scene => "Запустила",
    }
}

/// The smart home now: the panel's Home Assistant («Дом» → «Коннекторы»), asked on every request so a change
/// applies at once; None while it is not set up or switched off.
pub type ProviderSource = Arc<dyn Fn() -> Option<Arc<dyn SmartHomeProvider>> + Send + Sync>;

pub struct SmartHomeSkill {
    provider: ProviderSource,
}

impl SmartHomeSkill {
    /// This provider, always (or none): tests.
    pub fn new(provider: Option<Arc<dyn SmartHomeProvider>>) -> SmartHomeSkill {
        SmartHomeSkill::live(Arc::new(move || provider.clone()))
    }

    pub fn live(provider: ProviderSource) -> SmartHomeSkill {
        SmartHomeSkill { provider }
    }

    async fn serve(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let Some(provider) = (self.provider)() else {
            return Ok(Outcome::speech("Умный дом не подключён."));
        };
        let words = js::string_or_empty(intent.params.get("device"));
        let devices = provider.devices(&context.cancel).await?;
        let Some(device) = pick_device(&devices, &words) else {
            let names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
            return Ok(Outcome {
                speech: Some(format!("Не нашла устройство «{words}».")),
                data: Some(format!("No device matches \"{words}\". Devices: {}", names.join(", "))),
                ..Outcome::default()
            });
        };
        if intent.action == "state" {
            let details: Vec<String> =
                device.details.iter().map(|(key, value)| format!("{key} {}", js::string_of(value))).collect();
            let text = format!(
                "{}: {}{}",
                device.name,
                device.state.as_deref().unwrap_or("неизвестно"),
                if details.is_empty() { String::new() } else { format!(", {}", details.join(", ")) }
            );
            return Ok(Outcome { data: Some(text.clone()), speech: Some(text), ..Outcome::default() });
        }
        let Some(action) = HomeAction::parse(&intent.action) else {
            anyhow::bail!("smart_home: unknown action {}", intent.action);
        };
        let value = js::number_param(&intent.params, "value");
        provider.act(device, action, value, &context.cancel).await?;
        let amount = match (value, action) {
            (None, _) => String::new(),
            (Some(value), HomeAction::SetTemperature) => format!(" {} градусов", js::number_text(value)),
            (Some(value), _) => format!(" {}%", js::number_text(value)),
        };
        Ok(Outcome::speech(format!("{} «{}»{amount}.", done(action), device.name)))
    }
}

impl OwnSkill for SmartHomeSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::SmartHome
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn available(&self) -> bool {
        (self.provider)().is_some()
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.serve(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::lookups::http::{HttpGet, HttpPost, HttpText};
    use crate::skills::smart_home::home_assistant::HomeAssistantProvider;
    use chrono::Utc;
    use serde_json::{json, Value};
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct Ha {
        posts: Mutex<Vec<(String, Value)>>,
    }

    impl HttpGet for Ha {
        fn get(
            &self,
            _: String,
            _: Vec<(String, String)>,
            _: CancellationToken,
        ) -> BoxFuture<'static, anyhow::Result<HttpText>> {
            let states = json!([
                {"entity_id": "light.kitchen", "state": "on", "attributes": {"friendly_name": "Свет на кухне", "brightness": 204}},
                {"entity_id": "switch.kettle", "state": "off", "attributes": {"friendly_name": "Чайник"}},
                {"entity_id": "climate.bedroom", "state": "heat", "attributes": {"friendly_name": "Спальня", "temperature": 21}},
                {"entity_id": "sensor.ignored", "state": "1", "attributes": {"friendly_name": "Датчик"}},
            ]);
            Box::pin(async move { Ok(HttpText { status: 200, text: states.to_string() }) })
        }
    }

    impl HttpPost for Ha {
        fn post(
            &self,
            url: String,
            body: Value,
            _: Vec<(String, String)>,
            _: CancellationToken,
        ) -> BoxFuture<'static, anyhow::Result<u16>> {
            self.posts.lock().unwrap().push((url, body));
            Box::pin(async { Ok(200) })
        }
    }

    fn provider(ha: &Arc<Ha>) -> Arc<HomeAssistantProvider> {
        Arc::new(HomeAssistantProvider::new("http://ha.local:8123/", "token", Some(ha.clone()), Some(ha.clone())))
    }

    fn intent(action: &str, params: Value) -> Intent {
        Intent {
            capability: CapabilityName::SmartHome,
            action: action.into(),
            params: params.as_object().cloned().unwrap(),
        }
    }

    #[tokio::test]
    async fn matches_the_words_to_a_device_by_name_and_kind() {
        let ha = Arc::new(Ha::default());
        let devices = provider(&ha).devices(&CancellationToken::new()).await.unwrap();
        assert_eq!(
            devices.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["light.kitchen", "switch.kettle", "climate.bedroom"]
        );
        assert_eq!(pick_device(&devices, "свет на кухне").map(|d| d.id.as_str()), Some("light.kitchen"));
        assert_eq!(pick_device(&devices, "чайник").map(|d| d.id.as_str()), Some("switch.kettle"));
        assert!(pick_device(&devices, "гараж").is_none());
    }

    #[tokio::test]
    async fn acts_through_home_assistant_services_and_says_it_is_done() {
        let ha = Arc::new(Ha::default());
        let skill = SmartHomeSkill::new(Some(provider(&ha)));
        assert!(skill.available());
        let context = ExecutionContext::new("h:test", Utc::now());
        let off = skill.execute(&intent("off", json!({"device": "свет на кухне"})), &context).await.unwrap();
        assert_eq!(off.speech.as_deref(), Some("Выключила «Свет на кухне»."));
        assert_eq!(
            ha.posts.lock().unwrap().last().cloned(),
            Some((
                "http://ha.local:8123/api/services/light/turn_off".to_string(),
                json!({"entity_id": "light.kitchen"})
            ))
        );
        let warmer = intent("set_temperature", json!({"device": "спальня", "value": 22}));
        assert_eq!(
            skill.execute(&warmer, &context).await.unwrap().speech.as_deref(),
            Some("Поставила температуру «Спальня» 22 градусов.")
        );
        assert_eq!(
            ha.posts.lock().unwrap().last().cloned(),
            Some((
                "http://ha.local:8123/api/services/climate/set_temperature".to_string(),
                json!({"entity_id": "climate.bedroom", "temperature": 22})
            ))
        );
        let state = skill.execute(&intent("state", json!({"device": "свет на кухне"})), &context).await.unwrap();
        assert_eq!(state.data.as_deref(), Some("Свет на кухне: on, brightnessPercent 80"));
        assert!(!SmartHomeSkill::new(None).available());
    }

    #[tokio::test]
    async fn says_what_it_cannot_find_or_do() {
        let ha = Arc::new(Ha::default());
        let skill = SmartHomeSkill::new(Some(provider(&ha)));
        let context = ExecutionContext::new("h:test", Utc::now());
        let missing = skill.execute(&intent("on", json!({"device": "гараж"})), &context).await.unwrap();
        assert_eq!(missing.speech.as_deref(), Some("Не нашла устройство «гараж»."));
        assert_eq!(
            missing.data.as_deref(),
            Some("No device matches \"гараж\". Devices: Свет на кухне, Чайник, Спальня")
        );
        let error = skill.execute(&intent("scene", json!({"device": "чайник"})), &context).await.unwrap_err();
        assert_eq!(error.to_string(), "Чайник (switch) cannot scene");
        let none = SmartHomeSkill::new(None).execute(&intent("on", json!({})), &context).await.unwrap();
        assert_eq!(none.speech.as_deref(), Some("Умный дом не подключён."));
    }
}
