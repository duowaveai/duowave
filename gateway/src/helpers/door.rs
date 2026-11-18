//! The pair's door (docs/plans/home-and-helpers.md, block 1; docs/plans/home-and-devices.md): the station's entry
//! `home-computer` — the computer of its pair as its Турбо — as the computer's install and its «Помогать» switch set
//! it. The install writes the computer's address and key into the station's .env (`HOME_SERVER_URL`,
//! `HOME_SERVER_KEY`, src/placement/station_fs.rs): at the start a new one becomes the entry (`at_start`). The routes
//! `/api/v1/home-server*` the computer calls write the entry in their words (`Helpers::door`): the switch and «Что даю
//! колонке» by the old part names (`asr`, `tts`, `llm`, `messages`).
//!
//! A clean install (docs/plans/home-and-devices.md, «Принципы»): no 0.11 program to roll back to — the old setting
//! `connector_home_server` is neither read nor mirrored any more.

use super::contract::Ability;
use super::{key_secret, sorted, DoorInput, HelperKind, HelperSetting, Helpers, HOME_COMPUTER};
use crate::config::Config;
use crate::core::events::{Actor, GatewayEvent};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::HELPERS;
use crate::settings::SettingsService;

/// What the station's .env last brought (a digest of its address and key): a new one — the install wrote it — is taken
/// at the start; the same one leaves the panel's changes and its removal be.
const FROM_ENV_SECRET: &str = "home-server:from-env";
/// The old parts the pair's door names.
const SERVES: [&str; 4] = ["asr", "tts", "llm", "messages"];

/// At every start, before anything reads the placement (app.rs): the .env's computer, when it brings a new one, is the
/// entry `home-computer` — first when new, switched on — with its key. What does not read is left as it is.
pub fn at_start(config: &Config, settings: &SettingsService, secrets: &SecretsService) {
    let (Some(url), Some(key)) = (config.placement.home_server_url.clone(), config.placement.home_server_key.clone())
    else {
        return;
    };
    let brought = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(format!("{url}\n{key}").as_bytes()))
    };
    if secrets.get(FROM_ENV_SECRET).as_deref() == Some(brought.as_str()) {
        return;
    }
    let mut list = match settings.peek(&HELPERS) {
        Ok(list) => list.unwrap_or_default(),
        Err(error) => {
            tracing::warn!("helpers: the list is not read, the .env's computer waits: {}", error.message);
            return;
        }
    };
    let url = url.trim().trim_end_matches('/').to_string();
    match list.iter_mut().find(|entry| entry.id == HOME_COMPUTER) {
        Some(entry) => {
            if entry.url != url {
                entry.url = url;
                entry.helper_id = None;
            }
            entry.enabled = true;
        }
        None => list.insert(
            0,
            HelperSetting {
                id: HOME_COMPUTER.into(),
                name: crate::tr!("Компьютер дома"),
                url,
                kind: HelperKind::Home,
                enabled: true,
                without: Vec::new(),
                helper_id: None,
            },
        ),
    }
    if let Err(error) = settings.set(&HELPERS, &list, None) {
        tracing::warn!("helpers: the .env's computer is not in the list: {}", error.message);
        return;
    }
    let _ = secrets.set(&key_secret(HOME_COMPUTER), Some(&key));
    let _ = secrets.set(FROM_ENV_SECRET, Some(&brought));
}

/// The old parts as abilities: asr → hearing, tts → voice, llm → model, messages.
pub fn to_abilities(old: &[String]) -> Vec<Ability> {
    sorted(old.iter().filter(|part| SERVES.contains(&part.as_str())).filter_map(|part| Ability::parse(part)))
}

/// Abilities back in the old names; the ones without one (apps, history, catalog, …) left out.
pub fn to_legacy(without: &[Ability]) -> Vec<String> {
    let mut old: Vec<String> = without.iter().filter_map(|ability| ability.legacy()).map(str::to_string).collect();
    old.sort();
    old
}

/// The door's old list with the stored abilities that have no old name kept: «не брать приложения» set on the new page
/// survives the computer's «Что даю колонке».
pub fn merged(old: &[String], stored: &[Ability]) -> Vec<Ability> {
    let kept = stored.iter().copied().filter(|ability| ability.legacy().is_none());
    sorted(to_abilities(old).into_iter().chain(kept))
}

impl Helpers {
    /// The pair's door (`/api/v1/home-server*`): the entry `home-computer` in the old names and words — a new address
    /// or key (created first, switched on, «Компьютер дома»), the switch «ПК помогает колонке», «Что даю колонке».
    /// What is not given stays.
    pub fn door(&self, input: DoorInput, actor: Option<Actor>) -> Result<(), ApiError> {
        let mut changed = false;
        let done = self.door_steps(input, actor.clone(), &mut changed);
        // What a step changed before a later one was refused is told all the same.
        if changed {
            self.changed(Some(HOME_COMPUTER), "door", actor);
        }
        done
    }

    /// The door's steps in the old order — the address and the key, the switch, the parts —, each refused in the old
    /// words; `changed` says whether any of them wrote.
    fn door_steps(&self, input: DoorInput, actor: Option<Actor>, changed: &mut bool) -> Result<(), ApiError> {
        let url = input.url.as_deref().map(str::trim).filter(|url| !url.is_empty());
        let key = input.key.as_deref().map(str::trim).filter(|key| !key.is_empty());
        let secret = key_secret(HOME_COMPUTER);
        if url.is_some() || key.is_some() {
            let url = match url {
                Some(url) => url.to_string(),
                None => self.entry(HOME_COMPUTER).map(|entry| entry.url).ok_or_else(|| {
                    ApiError::bad_request(crate::tr!(
                        "Адреса компьютера ещё нет: впишите его или включите помощь из пульта компьютера."
                    ))
                })?,
            };
            let bad = || ApiError::bad_request(crate::tr!("Адрес компьютера: http://адрес:порт"));
            let parsed = url::Url::parse(&url).map_err(|_| bad())?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(bad());
            }
            if let Some(key) = key {
                self.secrets.set(&secret, Some(key))?;
            }
            if !self.secrets.has(&secret) {
                return Err(ApiError::bad_request(crate::tr!(
                    "Нужен ключ с компьютера: «Доступ» → «Ключи», право «модели этого компьютера»."
                )));
            }
            let url = url.trim_end_matches('/').to_string();
            self.write(actor.clone(), |list| {
                match list.iter_mut().find(|entry| entry.id == HOME_COMPUTER) {
                    // Another address may be another computer: who answers there is learned anew.
                    Some(entry) if entry.url != url => {
                        entry.url = url.clone();
                        entry.helper_id = None;
                    }
                    Some(_) => {}
                    None => list.insert(
                        0,
                        HelperSetting {
                            id: HOME_COMPUTER.into(),
                            name: crate::tr!("Компьютер дома"),
                            url: url.clone(),
                            kind: HelperKind::Home,
                            enabled: true,
                            without: Vec::new(),
                            helper_id: None,
                        },
                    ),
                }
                Ok(Some(()))
            })?;
            // Another computer, or a new key: looked at anew, its bot read again.
            self.forget_remote(HOME_COMPUTER);
            *changed = true;
        }
        if let Some(on) = input.enabled {
            if self.entry(HOME_COMPUTER).is_none() || !self.secrets.has(&secret) {
                return Err(ApiError::bad_request(crate::tr!(
                    "Колонка ещё не знает компьютер: в пульте компьютера нажмите «Помогать колонке» — он пришлёт адрес и ключ."
                )));
            }
            let switched = self.write(actor.clone(), |list| {
                let Some(entry) = list.iter_mut().find(|entry| entry.id == HOME_COMPUTER) else { return Ok(None) };
                if entry.enabled == on {
                    return Ok(None);
                }
                entry.enabled = on;
                Ok(Some(()))
            })?;
            if switched.is_some() {
                if let Some(remote) = self.entry(HOME_COMPUTER).and_then(|entry| self.remote(&entry)) {
                    remote.reset();
                }
                *changed = true;
            }
        }
        // «Что даю колонке»: слух, голос, модель и сообщения по отдельности.
        if let Some(without) = input.without {
            let mut without: Vec<String> = without.into_iter().filter(|part| SERVES.contains(&part.as_str())).collect();
            without.sort();
            without.dedup();
            if self.entry(HOME_COMPUTER).is_none() {
                return Err(ApiError::bad_request(crate::tr!(
                    "Колонка ещё не знает компьютер: сначала нажмите «Помогать колонке» в пульте компьютера."
                )));
            }
            let parted = self.write(actor.clone(), |list| {
                let Some(entry) = list.iter_mut().find(|entry| entry.id == HOME_COMPUTER) else { return Ok(None) };
                let next = merged(&without, &entry.without);
                if next == entry.without {
                    return Ok(None);
                }
                entry.without = next;
                Ok(Some(()))
            })?;
            *changed |= parted.is_some();
        }
        Ok(())
    }

    /// An entry's runtime let go: looked at anew (a new address or key, the old door's «set»).
    fn forget_remote(&self, id: &str) {
        super::locked(&self.remotes).remove(id);
    }

    /// The pair's door removed — when it is there at all.
    pub fn close_door(&self, actor: Option<Actor>) -> Result<(), ApiError> {
        if self.entry(HOME_COMPUTER).is_some() {
            return self.remove(HOME_COMPUTER, actor);
        }
        self.events.emit(GatewayEvent::HomeServerChanged { configured: false, actor });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::helpers::tests::{station, KEY};
    use crate::helpers::{Change, HelperState};

    #[test]
    fn a_new_address_in_the_env_is_the_first_helper_and_the_same_env_leaves_the_panels_changes() {
        let station = station(&[]);
        station.put("own-1", "https://helper.example.com", HelperKind::Own);
        let start = |url: &str| {
            let env = crate::config::Env::from_pairs([("HOME_SERVER_URL", url), ("HOME_SERVER_KEY", "vgw_home")]);
            at_start(&crate::config::Config::for_tests(&env), &station.settings, &station.secrets);
        };
        start("http://192.0.2.10/");
        let list = station.settings.get(&HELPERS, None).unwrap();
        let ids: Vec<&str> = list.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, [HOME_COMPUTER, "own-1"], "first");
        assert_eq!((list[0].url.as_str(), list[0].enabled), ("http://192.0.2.10", true));
        assert_eq!(station.secrets.get(&key_secret(HOME_COMPUTER)).as_deref(), Some("vgw_home"), "its key");
        // The panel changed it since; the same .env again leaves it.
        let door = || station.settings.get(&HELPERS, None).unwrap().into_iter().find(|entry| entry.id == HOME_COMPUTER);
        station
            .helpers
            .door(DoorInput { url: Some("http://192.0.2.11".into()), ..DoorInput::default() }, None)
            .unwrap();
        start("http://192.0.2.10/");
        assert_eq!(door().unwrap().url, "http://192.0.2.11", "the panel's stays");
        // A new install's address wins.
        start("http://192.0.2.12");
        assert_eq!(door().unwrap().url, "http://192.0.2.12");
        let _ = HelperState::Up;
    }

    fn door_at(url: &str) -> DoorInput {
        DoorInput { url: Some(url.into()), key: Some("vgw_home".into()), ..DoorInput::default() }
    }

    /// A computer the pair's door brings goes first, before the helpers already listed (review: the door's place, not
    /// only an empty list's): hearing, voice and the apps from home before one's own server.
    #[tokio::test]
    async fn a_computer_the_door_brings_goes_first_before_the_helpers_listed() {
        let station = station(&[]);
        station.put("own-1", "https://helper.example.com", HelperKind::Own);
        station.helpers.door(door_at("http://192.0.2.10:7780"), None).unwrap();
        let ids: Vec<String> = station.helpers.list().into_iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [HOME_COMPUTER, "own-1"]);
    }

    #[tokio::test]
    async fn the_pair_door_keeps_the_address_and_the_key_and_keeps_the_new_abilities() {
        let station = station(&[("HOME_SERVER_URL", "http://192.0.2.10"), ("HOME_SERVER_KEY", "vgw_home")]);
        let home = crate::home_server::HomeServerService::over(station.helpers.clone());
        assert!(home.configured().is_some(), "the move brought it on");

        let status = home.link(None, None, Some(false), None, None).unwrap();
        assert!(status.configured && !status.enabled, "kept, off");
        assert_eq!(status.url.as_deref(), Some("http://192.0.2.10"));
        assert_eq!((home.configured(), home.state()), (None, crate::core::availability::HomeServer::None));
        assert_eq!(station.helpers.views()[0].state, HelperState::Off);

        // The computer moved on the home network: the new address, the switch stays off.
        let status = home.link(Some("http://192.0.2.35:3000"), None, None, None, None).unwrap();
        assert_eq!((status.url.as_deref(), status.enabled), (Some("http://192.0.2.35:3000"), false));
        let status = home.link(None, None, Some(true), None, None).unwrap();
        assert!(status.enabled);
        assert_eq!(home.configured(), Some(("http://192.0.2.35:3000".into(), "vgw_home".into())));
        // «Что даю колонке»: без модели — слух и голос остаются.
        let status = home.link(None, None, None, Some(vec!["llm".into(), "bogus".into()]), None).unwrap();
        assert_eq!(status.without, vec!["llm"]);
        assert!(home.serves("asr") && home.serves("tts") && !home.serves("llm"));
        assert_eq!(station.helpers.entry(HOME_COMPUTER).unwrap().without, [Ability::Model]);

        // «Не брать у него: приложения» set on the new page survives the computer's «Что даю колонке».
        let without = Some(vec![Ability::Model, Ability::Apps]);
        station.helpers.update(HOME_COMPUTER, Change { without, ..Change::default() }, None).unwrap();
        home.link(None, None, None, Some(vec!["tts".into()]), None).unwrap();
        assert_eq!(station.helpers.entry(HOME_COMPUTER).unwrap().without, [Ability::Voice, Ability::Apps]);
        // A new key through the door: kept as the entry's.
        home.link(None, Some(KEY), None, None, None).unwrap();
        assert_eq!(station.secrets.get(&key_secret(HOME_COMPUTER)).as_deref(), Some(KEY));
    }

    #[test]
    fn the_switch_cannot_turn_on_a_computer_the_station_never_heard_of() {
        let station = station(&[]);
        let door = |input: DoorInput| station.helpers.door(input, None).unwrap_err();
        let refused = door(DoorInput { enabled: Some(true), ..DoorInput::default() });
        assert_eq!(refused.status, StatusCode::BAD_REQUEST);
        let keyless = door(DoorInput { url: Some("http://192.0.2.10".into()), ..DoorInput::default() });
        assert!(
            keyless.message.contains("Нужен ключ с компьютера"),
            "the computer must bring one: {}",
            keyless.message
        );
        let nowhere = door(DoorInput { key: Some("vgw_home".into()), ..DoorInput::default() });
        assert!(nowhere.message.contains("Адреса компьютера ещё нет"), "{}", nowhere.message);
        let parts = door(DoorInput { without: Some(vec!["asr".into()]), ..DoorInput::default() });
        assert!(parts.message.contains("Колонка ещё не знает компьютер"), "{}", parts.message);
        assert!(station.helpers.list().is_empty());
        // The old parts and the abilities, both ways.
        assert_eq!(
            to_abilities(&["messages".into(), "asr".into(), "apps".into()]),
            [Ability::Hearing, Ability::Messages]
        );
        assert_eq!(to_legacy(&[Ability::Model, Ability::History, Ability::Hearing]), ["asr", "llm"]);
    }
}
