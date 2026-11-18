//! The home server (docs/plans/station-placement.md, section 5), now one helper among the helpers (src/helpers,
//! docs/plans/home-and-helpers.md): the gateway on the station leans on the computer at home for the heavy parts —
//! recognition, our voice (the voice lab's too), a model of its own, its Telegram bot — while the computer is up. On
//! the computer: `/home/v1` (serve.rs), its own servers behind it, only from the home network and only with a key that
//! has the right `home.models`. On the station: the computer is the helpers' entry `home-computer`, and this module is
//! the compatibility shell over the helpers for one more version — the old routes `/api/v1/home-server*` (a computer
//! of 0.11 and the station's own panel call them), `app.home_server` for the code that asks about «the computer», the
//! ways down when it is away — recognition from the helpers to the cloud to Yandex (chain.rs) —, and «передай …»
//! through the helper's bot (messenger.rs). «Мозг» gives each helper with a model a row of its own
//! (dialog/brain/brain_service.rs); the station's old row «Модель на компьютере» becomes the computer's.

pub mod chain;
pub mod messenger;
#[cfg(feature = "pc")]
pub mod serve;
#[cfg(feature = "pc")]
pub mod voice_plan;
use std::sync::Arc;

use serde::Serialize;

use crate::core::availability::HomeServer;
use crate::core::events::Actor;
use crate::helpers::contract::Ability;
use crate::helpers::{door, DoorInput, Helpers, HOME_COMPUTER};
use crate::platform::http::ApiError;
use crate::secrets::SecretMask;
use crate::settings::definitions::HomeServerSetting;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::tts::own_speech::OwnSpeech;

pub use crate::helpers::remote::Messenger;
pub use crate::helpers::{AWAY, BACK, CHECK_EVERY, DOWN_AFTER, GIVEN, NO_HEARING, NO_VOICE, RETURN_AFTER};

/// The pair's door's words (the station's old panel, a 0.11 computer): «Модель» off while «Мозг» on the station answers
/// with the computer's model — its row cannot leave while a choice answers with it, so the switch is refused.
pub fn model_answers() -> String {
    crate::tr!("Модель компьютера сейчас отвечает в «Мозге» колонки — сначала выберите там другую модель.")
}

/// The connection and its state, for both panels.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeServerStatus {
    /// The address and the key are there.
    pub configured: bool,
    /// And the station leans on the computer: «ПК помогает колонке».
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub key: SecretMask,
    pub state: HomeServer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the station does not take from the computer: "asr", "tts", "llm", "messages".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

/// What the computer can give the station: hearing, voice, the model, and its Telegram bot for «передай …».
pub const SERVES: &[&str] = &["asr", "tts", "llm", "messages"];

/// The computer as the station knew it before the helpers: the pair's door, `home-computer`, in the old shape.
pub struct HomeServerService {
    helpers: Arc<Helpers>,
}

impl HomeServerService {
    pub fn over(helpers: Arc<Helpers>) -> Arc<HomeServerService> {
        Arc::new(HomeServerService { helpers })
    }

    /// The computer's address as the station knows it, whether it leans on it or not.
    pub fn known(&self) -> Option<HomeServerSetting> {
        let entry = self.helpers.entry(HOME_COMPUTER)?;
        Some(HomeServerSetting { url: entry.url, enabled: entry.enabled, without: door::to_legacy(&entry.without) })
    }

    /// The address and the key, when the connection is set up and on («ПК помогает колонке»).
    pub fn configured(&self) -> Option<(String, String)> {
        let entry = self.helpers.entry(HOME_COMPUTER).filter(|entry| entry.enabled)?;
        Some((entry.url, self.helpers.key(HOME_COMPUTER)?))
    }

    pub fn state(&self) -> HomeServer {
        if self.configured().is_none() {
            return HomeServer::None;
        }
        match self.helpers.entry(HOME_COMPUTER) {
            Some(entry) if self.helpers.door_up(&entry) => HomeServer::Up,
            _ => HomeServer::Down,
        }
    }

    fn remote(&self) -> Option<Arc<crate::helpers::remote::RemoteHelper>> {
        self.helpers.remote(&self.helpers.entry(HOME_COMPUTER)?)
    }

    pub fn status(&self) -> HomeServerStatus {
        let known = self.known();
        let remote = self.remote();
        HomeServerStatus {
            configured: known.is_some() && self.helpers.key(HOME_COMPUTER).is_some(),
            enabled: known.as_ref().is_some_and(|setting| setting.enabled),
            without: known.as_ref().map(|setting| setting.without.clone()).unwrap_or_default(),
            url: known.map(|setting| setting.url),
            key: self.helpers.key_mask(HOME_COMPUTER),
            state: self.state(),
            since: remote.as_ref().and_then(|remote| remote.since()),
            // Why it is away in the reader's language, as the list says it (a look keeps the source words).
            error: remote.as_ref().and_then(|remote| remote.error()).map(|error| crate::core::tx_dyn(&error)),
        }
    }

    /// The panel's connector: the computer's address and the key made on it («Доступ» → «Ключи», right
    /// `home.models`); a key not given is kept. None removes the connection.
    pub fn set(
        &self,
        url: Option<&str>,
        key: Option<&str>,
        actor: Option<Actor>,
    ) -> Result<HomeServerStatus, ApiError> {
        match url.map(str::trim).filter(|url| !url.is_empty()) {
            None => self.helpers.close_door(actor)?,
            Some(url) => self.helpers.door(
                DoorInput { url: Some(url.to_string()), key: key.map(str::to_string), ..DoorInput::default() },
                actor,
            )?,
        }
        Ok(self.status())
    }

    /// The pair's way to the connection (the computer's panel and its watch, the station's panel): a new address — the
    /// computer's moved on the home network; a new key; the switch «ПК помогает колонке»; «Что даю колонке». What is
    /// not given stays.
    pub fn link(
        &self,
        url: Option<&str>,
        key: Option<&str>,
        enabled: Option<bool>,
        without: Option<Vec<String>>,
        actor: Option<Actor>,
    ) -> Result<HomeServerStatus, ApiError> {
        let input = DoorInput { url: url.map(str::to_string), key: key.map(str::to_string), enabled, without };
        self.helpers.door(input, actor)?;
        Ok(self.status())
    }

    /// One look at the computer now, while the station leans on it.
    pub async fn check(&self) {
        if self.configured().is_some() {
            let _ = self.helpers.check(HOME_COMPUTER).await;
        }
    }

    /// Whether the station takes this from the computer now: "asr", "tts", "llm", "messages".
    pub fn serves(&self, what: &str) -> bool {
        let Some(ability) = Ability::parse(what) else { return false };
        self.helpers.entry(HOME_COMPUTER).is_some_and(|entry| entry.enabled && !entry.without.contains(&ability))
    }

    /// What «Свой» would lack now (the helpers').
    pub fn svoi_lacks(&self) -> Option<&'static str> {
        self.helpers.svoi_lacks()
    }

    /// The computer's part, while it is up, the station takes it and it gives it.
    fn giving(&self, ability: Ability) -> Option<Arc<crate::helpers::remote::RemoteHelper>> {
        if !self.serves(ability.legacy()?) || self.configured().is_none() {
            return None;
        }
        self.remote().filter(|remote| remote.up() && remote.gives(ability))
    }

    /// The computer's recognition, while it is up and the station takes it.
    pub fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.giving(Ability::Hearing).map(|remote| remote.recognizer())
    }

    /// The computer's voice — its own choice, a voice lab's voice too — while it is up and the station takes it.
    pub fn speech(&self) -> Option<Arc<OwnSpeech>> {
        self.giving(Ability::Voice).map(|remote| remote.speech())
    }

    /// The computer's Telegram bot as the station last saw it.
    pub fn messenger(&self) -> Messenger {
        self.remote().map(|remote| remote.messenger()).unwrap_or_default()
    }
}

/// The title of the computer's model among «Мозг»'s providers on a station before the helpers: the row becomes the
/// computer's own helper row at the first start (dialog/brain/brain_service.rs, `keep_helper_models`).
pub const BRAIN_TITLE: &str = "Модель на компьютере";

/// «Не брать у него: модель» on any gateway's «Помощники»: not let go while «Мозг» answers with this helper's model —
/// the person would switch off what answers. The refusal names the helper. Hearing and voice go their own way.
pub fn check_model_kept(
    helpers: &Helpers,
    brain: &crate::dialog::brain::BrainService,
    id: &str,
    without: &[Ability],
) -> Result<(), ApiError> {
    if model_kept(helpers, brain, id, without) {
        let name = helpers.entry(id).map(|entry| entry.name).unwrap_or_default();
        return Err(ApiError::conflict(crate::tr!(
            "Модель Турбо «{name}» сейчас отвечает в «Мозге» — сначала выберите там другую модель.",
            name = name
        )));
    }
    Ok(())
}

/// «Что даю колонке» in the old names, for the pair's door: the station's computer, in the door's own words.
pub fn check_parts(
    home: &HomeServerService,
    brain: &crate::dialog::brain::BrainService,
    without: &[String],
) -> Result<(), ApiError> {
    if model_kept(&home.helpers, brain, HOME_COMPUTER, &door::to_abilities(without)) {
        return Err(ApiError::conflict(model_answers()));
    }
    Ok(())
}

/// The model of a helper newly switched off while «Мозг» answers with it.
fn model_kept(helpers: &Helpers, brain: &crate::dialog::brain::BrainService, id: &str, without: &[Ability]) -> bool {
    without.contains(&Ability::Model)
        && helpers.entry(id).is_some_and(|entry| !entry.without.contains(&Ability::Model))
        && brain.answers_with_helper(id)
}

/// `/api/v1/home-server`: the station's panel sets the connection up; `/home/v1` on the computer (serve.rs).
pub fn router() -> axum::Router<Arc<crate::app::App>> {
    use axum::routing::{get, post};
    let router = axum::Router::new()
        .route("/api/v1/home-server", get(routes::status).put(routes::set).delete(routes::remove))
        .route("/api/v1/home-server/check", post(routes::check_now))
        .route("/api/v1/home-server/link", post(routes::link))
        // The computer's bot's messages for the station (messenger.rs): a voice message is up to 2 MB.
        .route(
            messenger::MESSAGE_PATH,
            post(messenger::receive).layer(axum::extract::DefaultBodyLimit::max(messenger::BODY_LIMIT)),
        );
    #[cfg(feature = "pc")]
    let router = router.merge(serve::router()).route("/api/v1/home-server/served", get(routes::served));
    router
}

mod routes {
    use std::sync::Arc;

    use axum::extract::State;
    use serde_json::Value;

    use super::HomeServerStatus;
    use crate::app::App;
    use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
    use crate::platform::http::{created, ok, ApiResult, Body, Caller};

    /// The helper's «Что даю колонке»: how many times and how fast it gave the station hearing, voice and the model
    /// today.
    #[cfg(feature = "pc")]
    pub async fn served(caller: Caller) -> ApiResult<Value> {
        caller.permitted(crate::core::identity::permissions::STATION_VIEW)?;
        Ok(ok(super::serve::served_today()))
    }

    pub async fn status(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_VIEW)?;
        Ok(ok(app.home_server.status()))
    }

    pub async fn set(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_MANAGE)?;
        let url = body.get("url").and_then(Value::as_str);
        let key = body.get("key").and_then(Value::as_str);
        app.home_server.set(url, key, caller.actor())?;
        app.home_server.check().await;
        Ok(created(app.home_server.status()))
    }

    pub async fn remove(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_MANAGE)?;
        Ok(ok(app.home_server.set(None, None, caller.actor())?))
    }

    /// The address, the key, the switch: from the computer with the pair's key, or a person with the right.
    pub async fn link(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<HomeServerStatus> {
        crate::placement::peer::peer_or_permitted(&caller, app.placement.peer_key(), CONNECTORS_MANAGE)?;
        let without: Option<Vec<String>> = body
            .get("without")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect());
        // Before anything is stored: «Модель» stays while «Мозг» answers with it (the computer's panel says why).
        if let Some(without) = &without {
            super::check_parts(&app.home_server, &app.brain, without)?;
        }
        let status = app.home_server.link(
            body.get("url").and_then(Value::as_str),
            body.get("key").and_then(Value::as_str),
            body.get("enabled").and_then(Value::as_bool),
            without,
            caller.actor(),
        )?;
        if status.enabled {
            app.home_server.check().await;
        }
        Ok(ok(app.home_server.status()))
    }

    pub async fn check_now(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_VIEW)?;
        app.home_server.check().await;
        Ok(created(app.home_server.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::tests::{contract, stand, station, ALL, KEY};
    use crate::helpers::HelperKind;

    /// Review G3, over the helpers: «Модель» off while «Мозг» on the station answers with that helper's model would
    /// switch off what answers, so it is refused with why; another helper's model, or none chosen — it goes, and its
    /// row with it.
    #[tokio::test]
    async fn the_model_is_not_let_go_while_the_brain_answers_with_that_helpers_row() {
        let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
        let station =
            station(&[("HOME_SERVER_URL", a.url.as_str()), ("HOME_SERVER_KEY", KEY), ("DIALOG_PROVIDER", "none")]);
        station.put("home-b", &b.url, HelperKind::Home);
        let brain = Arc::new(
            crate::dialog::brain::BrainService::new(
                station.config.dialog.clone(),
                station.db.clone(),
                station.settings.clone(),
                station.secrets.clone(),
                station.events.clone(),
            )
            .unwrap(),
        );
        brain.set_helpers(station.helpers.clone());
        station.helpers.check_all().await;
        brain.keep_helper_models().unwrap();
        let row_of = |helper: &str| {
            brain.view().providers.into_iter().find(|row| row.helper.as_ref().is_some_and(|whose| whose.id == helper))
        };
        let home = HomeServerService::over(station.helpers.clone());
        let ours = row_of(HOME_COMPUTER).expect("the computer's model has its row");
        assert!(row_of("home-b").is_some(), "and the other helper's its own");
        let pick = |fast: serde_json::Value| {
            brain.choose(&serde_json::Map::from_iter([("fast".to_string(), fast)]), None).unwrap();
        };

        pick(serde_json::json!({"provider": ours.id, "model": "home"}));
        assert!(brain.answers_with_helper(HOME_COMPUTER) && !brain.answers_with_helper("home-b"));
        let refused = check_parts(&home, &brain, &["llm".into()]).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.message.as_str()), (409, model_answers().as_str()));
        assert!(check_parts(&home, &brain, &["asr".into(), "tts".into()]).is_ok(), "hearing and voice go their way");
        let page = check_model_kept(&station.helpers, &brain, HOME_COMPUTER, &[Ability::Model]).unwrap_err();
        assert_eq!(
            (page.status.as_u16(), page.message.as_str()),
            (409, "Модель Турбо «Турбо a» сейчас отвечает в «Мозге» — сначала выберите там другую модель."),
            "the new page's «Не брать у него: модель» too, naming the helper (as it names itself, item 48)"
        );
        assert!(check_model_kept(&station.helpers, &brain, "home-b", &[Ability::Model]).is_ok(), "another helper's");

        // Another model answers: «Модель» goes, the row with it.
        pick(serde_json::Value::Null);
        assert!(check_parts(&home, &brain, &["llm".into()]).is_ok());
        home.link(None, None, None, Some(vec!["llm".into()]), None).unwrap();
        brain.keep_helper_models().unwrap();
        assert!(row_of(HOME_COMPUTER).is_none() && row_of("home-b").is_some());
        // Off already: nothing more is asked of the other parts.
        assert!(check_parts(&home, &brain, &["asr".into(), "llm".into()]).is_ok());
    }

    #[tokio::test]
    async fn the_shell_speaks_of_the_pair_door_in_the_old_shape() {
        let computer = stand(contract("pc", ALL)).await;
        let station = station(&[("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY)]);
        let home = HomeServerService::over(station.helpers.clone());
        assert_eq!(home.state(), HomeServer::Down, "not looked at yet");
        assert!(home.recognizer().is_none() && home.speech().is_none());
        home.check().await;
        assert_eq!(home.state(), HomeServer::Up);
        assert!(home.recognizer().is_some() && home.speech().is_some(), "its ears and its voice, while up");
        let status = home.status();
        assert!(status.configured && status.enabled && status.key.set && status.since.is_some(), "{status:?}");
        home.link(None, None, None, Some(vec!["asr".into()]), None).unwrap();
        assert!(home.recognizer().is_none() && home.speech().is_some(), "hearing not taken");
        home.set(None, None, None).unwrap();
        assert_eq!((home.state(), home.known()), (HomeServer::None, None));
    }
}
