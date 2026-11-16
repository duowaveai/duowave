//! The old «Домашний сервер» as the helpers' list sees it (docs/plans/home-and-helpers.md, «Переход»): the setting
//! `connector_home_server` and its key `home-server:key` become the first helper, the entry `home-computer` («Компьютер
//! дома»), its parts not taken (`asr`, `tts`, `llm`, `messages`) its abilities not taken. The pair's door — the routes
//! `/api/v1/home-server*` a computer of 0.11 and the station's own panel call — writes into the list here, in the old
//! names, with the old words.
//!
//! «Read one more version», precisely: this release reads the old setting at every start (`at_start`, the old setting
//! wins: a rollback to 0.11 by the safety net and back finds the same computer, and one removed there stays removed)
//! and keeps it in step with `home-computer` on every change (`mirror`), so a 0.11 program leans on the same computer.
//! The next release runs the step from the old setting only when the list lacks `home-computer` (a station that skipped
//! a release), then deletes both old keys and the mark `home-server:mirrored` and stops mirroring; the release after it
//! drops this file. `connector_home_server` stays each gateway's own (`transfer::LOCAL_SETTINGS`) meanwhile, as
//! `home-computer` is (`transfer::OWN_PARTS`).

use super::contract::Ability;
use super::{key_secret, sorted, DoorInput, HelperKind, HelperSetting, Helpers, HOME_COMPUTER};
use crate::config::Config;
use crate::core::events::{Actor, GatewayEvent};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::{HomeServerSetting, CONNECTOR_HOME_SERVER, HELPERS};
use crate::settings::SettingsService;

/// The old key the station reached its computer with; kept equal to `helper:home-computer:key`.
pub const KEY_SECRET: &str = "home-server:key";
/// What the station's .env last brought (a digest of its address and key): a new one — a computer's move wrote it —
/// is taken at the start; the same one leaves the panel's changes and its removal be.
const FROM_ENV_SECRET: &str = "home-server:from-env";
/// Set while the old setting is in step with `home-computer`. An old setting gone at a start with the mark there was
/// removed by a 0.11 program after a rollback — it knows neither the list nor the entry's key —, so the computer goes
/// from the list too; without the mark the mirror never got written, and is written then.
const MIRRORED_SECRET: &str = "home-server:mirrored";
/// The old parts the pair's door names.
const SERVES: [&str; 4] = ["asr", "tts", "llm", "messages"];

/// At every start, right after the identity and before anything reads the placement (app.rs): the .env's computer
/// stored when it brings a new one, as before; the old setting into the list — the entry `home-computer`, first when
/// new; or, with no old setting, the entry removed when the old program removed it, else back into it. What does not
/// read is left as it is: a read error is no empty list.
pub fn at_start(config: &Config, settings: &SettingsService, secrets: &SecretsService) {
    if let (Some(url), Some(key)) = (config.placement.home_server_url.clone(), config.placement.home_server_key.clone())
    {
        let brought = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(format!("{url}\n{key}").as_bytes()))
        };
        if secrets.get(FROM_ENV_SECRET).as_deref() != Some(brought.as_str()) {
            let _ = settings.set(
                &CONNECTOR_HOME_SERVER,
                &HomeServerSetting { url, enabled: true, without: Vec::new() },
                None,
            );
            let _ = secrets.set(KEY_SECRET, Some(&key));
            let _ = secrets.set(FROM_ENV_SECRET, Some(&brought));
        }
    }
    let (mut list, old) = match (settings.peek(&HELPERS), settings.peek(&CONNECTOR_HOME_SERVER)) {
        (Ok(list), Ok(old)) => (list.unwrap_or_default(), old),
        (Err(error), _) | (_, Err(error)) => {
            tracing::warn!(
                "helpers: the list or the old home server is not read, both stay as they are: {}",
                error.message
            );
            return;
        }
    };
    let Some(old) = old else {
        let Some(entry) = list.iter().find(|entry| entry.id == HOME_COMPUTER).cloned() else { return };
        if !secrets.has(MIRRORED_SECRET) {
            mirror(settings, secrets, Some(&entry));
            return;
        }
        // Removed by the old program: the old setting wins.
        list.retain(|entry| entry.id != HOME_COMPUTER);
        match settings.set(&HELPERS, &list, None) {
            Ok(()) => {
                let _ = secrets.delete(&key_secret(HOME_COMPUTER));
                let _ = secrets.delete(MIRRORED_SECRET);
                tracing::info!("helpers: the computer the old program removed is out of the list");
            }
            Err(error) => tracing::warn!("helpers: the computer the old program removed stays: {}", error.message),
        }
        return;
    };
    let url = old.url.trim().trim_end_matches('/').to_string();
    let before = list.clone();
    match list.iter_mut().find(|entry| entry.id == HOME_COMPUTER) {
        Some(entry) => {
            entry.url = url;
            entry.enabled = old.enabled;
            entry.without = merged(&old.without, &entry.without);
        }
        None => list.insert(
            0,
            HelperSetting {
                id: HOME_COMPUTER.into(),
                name: crate::tr!("Компьютер дома"),
                url,
                kind: HelperKind::Home,
                enabled: old.enabled,
                without: to_abilities(&old.without),
                helper_id: None,
            },
        ),
    }
    if list != before {
        if let Err(error) = settings.set(&HELPERS, &list, None) {
            tracing::warn!("helpers: the old home server is not in the list: {}", error.message);
            return;
        }
    }
    let door = key_secret(HOME_COMPUTER);
    if secrets.get(&door) != secrets.get(KEY_SECRET) {
        let _ = secrets.set(&door, secrets.get(KEY_SECRET).as_deref());
    }
    if !secrets.has(MIRRORED_SECRET) {
        let _ = secrets.set(MIRRORED_SECRET, Some("1"));
    }
}

/// The pair's door written back into the old setting and its key, for a 0.11 program after a rollback, and marked so;
/// None — all three removed. Writes only what differs.
pub fn mirror(settings: &SettingsService, secrets: &SecretsService, entry: Option<&HelperSetting>) {
    match entry {
        Some(entry) => {
            let old = HomeServerSetting {
                url: entry.url.clone(),
                enabled: entry.enabled,
                without: to_legacy(&entry.without),
            };
            let in_step = settings.get(&CONNECTOR_HOME_SERVER, None).as_ref() == Some(&old)
                || settings.set(&CONNECTOR_HOME_SERVER, &old, None).is_ok();
            let key = secrets.get(&key_secret(HOME_COMPUTER));
            if secrets.get(KEY_SECRET) != key {
                let _ = secrets.set(KEY_SECRET, key.as_deref());
            }
            if in_step && !secrets.has(MIRRORED_SECRET) {
                let _ = secrets.set(MIRRORED_SECRET, Some("1"));
            }
        }
        None => {
            let _ = settings.forget(&CONNECTOR_HOME_SERVER);
            let _ = secrets.delete(KEY_SECRET);
            let _ = secrets.delete(MIRRORED_SECRET);
        }
    }
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
        // What a step changed before a later one was refused is told all the same, and mirrored.
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

    /// The pair's door removed with its old setting and key — when it is there at all.
    pub fn close_door(&self, actor: Option<Actor>) -> Result<(), ApiError> {
        if self.entry(HOME_COMPUTER).is_some() {
            return self.remove(HOME_COMPUTER, actor);
        }
        mirror(&self.settings, &self.secrets, None);
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

    fn old(url: &str, enabled: bool, without: &[&str]) -> HomeServerSetting {
        HomeServerSetting { url: url.into(), enabled, without: without.iter().map(|part| part.to_string()).collect() }
    }

    #[test]
    fn the_old_connector_becomes_the_first_helper_and_its_parts_become_abilities() {
        let station = station(&[]);
        station.put("own-1", "https://helper.example.com", HelperKind::Own);
        station
            .settings
            .set(&CONNECTOR_HOME_SERVER, &old("http://192.0.2.10:7780/", true, &["llm", "tts"]), None)
            .unwrap();
        station.secrets.set(KEY_SECRET, Some("vgw_home")).unwrap();
        at_start(&station.config, &station.settings, &station.secrets);
        let list = station.settings.get(&HELPERS, None).unwrap();
        let ids: Vec<&str> = list.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, [HOME_COMPUTER, "own-1"], "first");
        let door = &list[0];
        assert_eq!((door.url.as_str(), door.kind, door.enabled), ("http://192.0.2.10:7780", HelperKind::Home, true));
        assert_eq!(door.without, [Ability::Voice, Ability::Model], "the old parts as abilities");
        assert_eq!(door.name, crate::tr!("Компьютер дома"));
        assert_eq!(station.secrets.get(&key_secret(HOME_COMPUTER)).as_deref(), Some("vgw_home"), "its key copied");
        assert!(station.settings.get(&CONNECTOR_HOME_SERVER, None).is_some(), "the old kept one more version");
        // Again at the next start: nothing to write.
        let mut heard = station.events.subscribe();
        at_start(&station.config, &station.settings, &station.secrets);
        assert!(heard.try_recv().is_err(), "the same: nothing written");
    }

    #[test]
    fn a_new_address_in_the_env_wins_over_the_list_and_the_same_env_leaves_the_panels_changes() {
        let station = station(&[]);
        let start = |url: &str| {
            let env = crate::config::Env::from_pairs([("HOME_SERVER_URL", url), ("HOME_SERVER_KEY", "vgw_home")]);
            at_start(&crate::config::Config::for_tests(&env), &station.settings, &station.secrets);
        };
        // Kept from an earlier install: a computer that is not there any more.
        station.settings.set(&CONNECTOR_HOME_SERVER, &old("http://192.0.2.10:8191", true, &[]), None).unwrap();
        start("http://192.0.2.10");
        let door = || station.settings.get(&HELPERS, None).unwrap().into_iter().find(|entry| entry.id == HOME_COMPUTER);
        assert_eq!(door().unwrap().url, "http://192.0.2.10", "the move's address");
        // The panel changed it since; the same .env again leaves it.
        station.settings.set(&CONNECTOR_HOME_SERVER, &old("http://192.0.2.11", true, &[]), None).unwrap();
        start("http://192.0.2.10");
        assert_eq!(door().unwrap().url, "http://192.0.2.11", "the panel's stays");
    }

    #[tokio::test]
    async fn the_old_setting_wins_at_the_start_and_a_removed_home_computer_stays_away() {
        let station = station(&[("HOME_SERVER_URL", "http://192.0.2.10:7780"), ("HOME_SERVER_KEY", "vgw_home")]);
        assert_eq!(station.helpers.list()[0].id, HOME_COMPUTER);
        // A package replaced the list (or a 0.11 program after a rollback left it as it was): the old setting wins.
        let theirs = vec![HelperSetting {
            id: "own-1".into(),
            name: "Мой сервер".into(),
            url: "https://helper.example.com".into(),
            kind: HelperKind::Own,
            enabled: true,
            without: Vec::new(),
            helper_id: None,
        }];
        station.settings.set(&HELPERS, &theirs, None).unwrap();
        at_start(&station.config, &station.settings, &station.secrets);
        let ids: Vec<String> =
            station.settings.get(&HELPERS, None).unwrap().into_iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [HOME_COMPUTER, "own-1"], "back, first");

        // Removed by a person: the old setting and its key go with it, and it does not come back.
        let station = station_for_removal();
        station.helpers.remove(HOME_COMPUTER, None).unwrap();
        assert!(station.settings.get(&CONNECTOR_HOME_SERVER, None).is_none() && !station.secrets.has(KEY_SECRET));
        let env = crate::config::Env::from_pairs([
            ("HOME_SERVER_URL", "http://192.0.2.10:7780"),
            ("HOME_SERVER_KEY", "vgw_home"),
        ]);
        at_start(&crate::config::Config::for_tests(&env), &station.settings, &station.secrets);
        assert!(station.settings.get(&HELPERS, None).unwrap_or_default().is_empty(), "the same .env leaves it removed");
    }

    fn station_for_removal() -> crate::helpers::tests::Station {
        station(&[("HOME_SERVER_URL", "http://192.0.2.10:7780"), ("HOME_SERVER_KEY", "vgw_home")])
    }

    fn door_at(url: &str) -> DoorInput {
        DoorInput { url: Some(url.into()), key: Some("vgw_home".into()), ..DoorInput::default() }
    }

    /// A rollback to 0.11, where a person removes «Домашний сервер», and back (review): 0.11 knows neither the list nor
    /// the entry's key, so the old setting gone with the mark there is a removal — the computer leaves the list too
    /// and stays away. An entry whose mirror never got written is mirrored instead.
    #[tokio::test]
    async fn a_computer_removed_by_the_old_program_stays_removed_and_one_never_mirrored_is_mirrored() {
        let removed = station(&[]);
        removed.helpers.door(door_at("http://192.0.2.10:7780"), None).unwrap();
        assert!(removed.settings.get(&CONNECTOR_HOME_SERVER, None).is_some() && removed.secrets.has(MIRRORED_SECRET));
        // 0.11 removes it: the old setting and the old key.
        removed.settings.forget(&CONNECTOR_HOME_SERVER).unwrap();
        removed.secrets.delete(KEY_SECRET).unwrap();
        for start in 1..=2 {
            at_start(&removed.config, &removed.settings, &removed.secrets);
            let list = removed.settings.get(&HELPERS, None).unwrap_or_default();
            assert!(list.is_empty(), "start {start}: the old setting wins, removed");
            assert!(removed.settings.get(&CONNECTOR_HOME_SERVER, None).is_none());
            assert!(!removed.secrets.has(&key_secret(HOME_COMPUTER)) && !removed.secrets.has(MIRRORED_SECRET));
        }

        // Never mirrored (its write failed): written now, so a 0.11 program leans on the same computer.
        let unmirrored = station(&[]);
        unmirrored.put(HOME_COMPUTER, "http://192.0.2.10:7780", HelperKind::Home);
        at_start(&unmirrored.config, &unmirrored.settings, &unmirrored.secrets);
        let old = unmirrored.settings.get(&CONNECTOR_HOME_SERVER, None).expect("mirrored");
        assert_eq!((old.url.as_str(), old.enabled), ("http://192.0.2.10:7780", true));
        assert_eq!(unmirrored.secrets.get(KEY_SECRET).as_deref(), Some(KEY));
        assert_eq!(unmirrored.settings.get(&HELPERS, None).unwrap()[0].id, HOME_COMPUTER, "kept");
        assert!(unmirrored.secrets.has(MIRRORED_SECRET));
    }

    /// A start whose list does not read writes nothing over it (review): read as an empty one, the old computer would
    /// have been written as the whole list.
    #[test]
    fn a_start_whose_list_does_not_read_writes_nothing_over_it() {
        let station = station(&[]);
        station.put("own-1", "https://helper.example.com", HelperKind::Own);
        station.settings.set(&CONNECTOR_HOME_SERVER, &old("http://192.0.2.10:7780", true, &[]), None).unwrap();
        let unreadable = |flag: &str| {
            let sql = format!("UPDATE settings SET is_default = {flag} WHERE key = 'helpers'");
            station.db.with(|c| c.execute(&sql, [])).unwrap();
        };
        let ids = || -> Vec<String> {
            station.settings.get(&HELPERS, None).unwrap().into_iter().map(|entry| entry.id).collect()
        };
        unreadable("'unreadable'");
        at_start(&station.config, &station.settings, &station.secrets);
        unreadable("0");
        assert_eq!(ids(), ["own-1"], "nothing written over a list that did not read");
        at_start(&station.config, &station.settings, &station.secrets);
        assert_eq!(ids(), [HOME_COMPUTER, "own-1"], "read, the old computer goes first");
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
        let mirrored = station.settings.get(&CONNECTOR_HOME_SERVER, None).unwrap();
        assert_eq!(
            (mirrored.url.as_str(), mirrored.without.clone()),
            ("http://192.0.2.35:3000", vec!["tts".to_string()])
        );
        assert_eq!(station.secrets.get(KEY_SECRET).as_deref(), Some("vgw_home"), "the old key mirrored");
        // A new key through the door: the mirror follows.
        home.link(None, Some(KEY), None, None, None).unwrap();
        assert_eq!(station.secrets.get(KEY_SECRET).as_deref(), Some(KEY));
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
