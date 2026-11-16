//! «Мозг»'s view of the helpers (dialog/brain/helper_models.rs): the helpers that give a model, in the list's order,
//! each with where its model is asked and whether it is on the line now. Read from memory, never the settings table:
//! «Мозг» asks for it with every slot. On a computer that takes its own parts from itself, its model servers come
//! first (the built-in helper, stage 6): each is the person's own row of «Мозг» already — read with «Мозг»'s rows, one
//! small read — and gives way at once when its last look found its server closed.

use super::contract::Ability;
use super::{key_secret, reach_of, HelperKind, HelperSetting, HelperState, Helpers, Reach, Snapshot, SELF};
use crate::dialog::brain::helper_models::{ModelHelper, ModelHelpers};

impl ModelHelpers for Helpers {
    /// This computer's model servers first, in the rows' order; then the helpers switched on, keyed, not this gateway,
    /// not kept from the model, and declaring it or not read yet — the regulator aside: «Мозг» says it keeps one out.
    /// A helper reached by two entries (the same `helperId`) is one model under the earlier entry, whatever the line
    /// does — its row is made, kept and dropped by the list, never by who is up —, and the later entry is only its road
    /// while it stands for the helper (the earlier one is not on the line).
    fn model_helpers(&self) -> Vec<ModelHelper> {
        let snap = self.snap();
        let mut models: Vec<ModelHelper> = self.own_models();
        for entry in snap.list.iter().filter(|entry| entry.enabled && !self.is_self(entry)) {
            let Some(remote) = self.remote(entry) else { continue };
            if let Some(first) = self.first_of(entry, &snap) {
                let stands = self.twin_of(entry, &snap).is_none()
                    && !entry.without.contains(&Ability::Model)
                    && remote.up()
                    && remote.gives(Ability::Model);
                if let Some(model) = models.iter_mut().find(|model| model.id == first.id).filter(|model| !model.up) {
                    if stands {
                        model.base_url = format!("{}/home/v1/llm", remote.base());
                        model.key = Some(remote.credential());
                        model.reach = reach_of(entry);
                        model.up = true;
                        model.state = remote.state();
                        model.linked = remote.base() != remote.url();
                        model.via = Some(entry.id.clone());
                    }
                }
                continue;
            }
            if entry.without.contains(&Ability::Model) || !remote.declares(Ability::Model) {
                continue;
            }
            models.push(ModelHelper {
                id: entry.id.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                base_url: format!("{}/home/v1/llm", remote.base()),
                key: Some(remote.credential()),
                reach: reach_of(entry),
                up: remote.up() && remote.gives(Ability::Model),
                state: remote.state(),
                via: None,
                read: remote.status().is_some(),
                linked: remote.base() != remote.url(),
                provider: None,
                builtin: false,
            });
        }
        models
    }

    fn lets(&self, reach: Reach) -> bool {
        Helpers::lets(self, reach)
    }
}

impl Helpers {
    /// This computer's model servers as the built-in helper's (stage 6): each is the «Мозг» row it already is
    /// (`provider`), at home, on the line unless its last look found it closed, asked with the row's own key.
    fn own_models(&self) -> Vec<ModelHelper> {
        let Some(own) = self.own.get() else { return Vec::new() };
        own.models()
            .into_iter()
            .map(|model| ModelHelper {
                id: SELF.to_string(),
                name: crate::tr!("Этот компьютер"),
                kind: HelperKind::Home,
                key: self.secrets.get(&format!("llm:{}:key", model.provider)),
                reach: Reach::Home,
                up: model.up != Some(false),
                state: match model.up {
                    Some(true) => HelperState::Up,
                    Some(false) => HelperState::Down,
                    None => HelperState::Unknown,
                },
                via: None,
                read: true,
                linked: false,
                provider: Some(model.provider),
                builtin: true,
                base_url: model.base_url,
            })
            .collect()
    }

    /// The earlier entry of the same helper (the same `helperId`) that is switched on and keyed — the one its model
    /// belongs to in «Мозг», on the line or not (`twin_of` asks who stands for the helper now).
    fn first_of<'a>(&self, entry: &HelperSetting, snap: &'a Snapshot) -> Option<&'a HelperSetting> {
        let named = entry.helper_id.as_deref()?;
        snap.list.iter().take_while(|earlier| earlier.id != entry.id).find(|earlier| {
            earlier.helper_id.as_deref() == Some(named) && earlier.enabled && self.secrets.has(&key_secret(&earlier.id))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::dialog::brain::BrainService;
    use crate::helpers::tests::{contract, stand, station, Station, ALL, KEY};
    use crate::helpers::{Change, HelperKind, HelperState};

    /// «Мозг» over the station's stores, knowing these helpers.
    fn brain_over(station: &Station, helpers: Arc<Helpers>) -> Arc<BrainService> {
        let brain = BrainService::new(
            station.config.dialog.clone(),
            station.db.clone(),
            station.settings.clone(),
            station.secrets.clone(),
            station.events.clone(),
        )
        .unwrap();
        brain.set_helpers(helpers);
        Arc::new(brain)
    }

    /// The helpers whose model has a row in «Мозг».
    fn rows(brain: &BrainService) -> Vec<String> {
        brain.view().providers.into_iter().filter_map(|row| row.helper.map(|helper| helper.id)).collect()
    }

    fn ids(models: &[ModelHelper]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    #[tokio::test]
    async fn the_helpers_with_a_model_in_order_each_with_its_route_its_key_and_whether_it_is_on_the_line() {
        let modelless: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "model").collect();
        let not_ready: Vec<(&str, bool)> = ALL.iter().map(|(id, ready)| (*id, *ready && *id != "model")).collect();
        let (a, b, c) = (
            stand(contract("a", ALL)).await,
            stand(contract("b", &modelless)).await,
            stand(contract("c", &not_ready)).await,
        );
        let station = station(&[]);
        station.put("home-a", &a.url, HelperKind::Home);
        station.put("home-b", &b.url, HelperKind::Home);
        station.put("home-c", &c.url, HelperKind::Home);
        let before = station.helpers.model_helpers();
        assert_eq!(ids(&before), ["home-a", "home-b", "home-c"], "not read yet: each may have one");
        assert!(before.iter().all(|model| !model.read && !model.up && model.state == HelperState::Unknown));

        station.helpers.check_all().await;
        let models = station.helpers.model_helpers();
        assert_eq!(ids(&models), ["home-a", "home-c"], "b declares none");
        let first = &models[0];
        assert_eq!(first.base_url, format!("{}/home/v1/llm", a.url));
        assert_eq!(first.key.as_deref(), Some(KEY));
        assert_eq!((first.name.as_str(), first.reach, first.kind), ("Турбо home-a", Reach::Home, HelperKind::Home));
        assert!(first.up && first.read && !first.linked && !first.builtin && first.provider.is_none());
        assert!(!models[1].up && models[1].read, "c declares its model, not ready yet");

        // Away, switched off for the model, switched off: still listed while it only is away.
        a.set_away(true);
        station.helpers.check_all().await;
        station.helpers.check_all().await;
        assert!(!station.helpers.model_helpers()[0].up);
        assert_eq!(station.helpers.model_helpers()[0].state, HelperState::Down);
        station.change("home-c", Change { without: Some(vec![Ability::Model]), ..Change::default() });
        assert_eq!(ids(&station.helpers.model_helpers()), ["home-a"]);
        station.change("home-a", Change { enabled: Some(false), ..Change::default() });
        assert!(station.helpers.model_helpers().is_empty());
    }

    #[tokio::test]
    async fn the_regulator_is_said_not_applied_a_helper_kept_out_stays_listed() {
        let server = stand(contract("srv", ALL)).await;
        let station = station(&[]);
        // A stand-in at home, given as one's own server: its reach is mine.
        station.put("own-srv", &server.url, HelperKind::Own);
        station.helpers.check_all().await;
        station.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
        let models = station.helpers.model_helpers();
        assert_eq!(ids(&models), ["own-srv"]);
        assert_eq!(models[0].reach, Reach::Mine);
        assert!(
            !ModelHelpers::lets(&*station.helpers, Reach::Mine) && ModelHelpers::lets(&*station.helpers, Reach::Home)
        );
        assert!(!models[0].up, "not looked at while kept out");
    }

    /// One computer reached by two entries — its address at home and its tunnel, the same `helperId` (review of stage
    /// 4): one model under the earlier entry, its row made and kept by the list, never by who is on the line — nothing
    /// made for the later entry while the earlier one is away, nothing dropped or written when it is back —, asked by
    /// the tunnel while it stands for the computer, out of the home for so long.
    #[tokio::test]
    async fn one_helper_by_two_entries_is_one_model_asked_by_the_road_that_stands_for_it() {
        use crate::config::ModelProfile;
        use crate::core::availability::State;
        use crate::core::events::GatewayEvent;
        let (lan, tunnel) = (stand(contract("pc", ALL)).await, stand(contract("pc", ALL)).await);
        let station = station(&[("DIALOG_PROVIDER", "none")]);
        station.put("home-a", &lan.url, HelperKind::Home);
        station.put("own-t", &tunnel.url, HelperKind::Own);
        station.helpers.check_all().await;
        let brain = brain_over(&station, station.helpers.clone());
        brain.keep_helper_models().unwrap();
        assert_eq!(rows(&brain), ["home-a"], "the later entry has no row of its own");
        let row = brain.view().providers.into_iter().find(|row| row.helper.is_some()).unwrap();
        let fast = serde_json::json!({"provider": row.id, "model": "home"});
        brain.choose(&serde_json::Map::from_iter([("fast".to_string(), fast)]), None).unwrap();
        let mut events = station.events.subscribe();

        // The computer leaves home: its address there away, the tunnel stands for it.
        lan.set_away(true);
        station.helpers.check_all().await;
        station.helpers.check_all().await;
        let models = station.helpers.model_helpers();
        assert_eq!(ids(&models), ["home-a"]);
        let road = (models[0].via.as_deref(), models[0].base_url.as_str(), models[0].reach, models[0].up);
        assert_eq!(road, (Some("own-t"), format!("{}/home/v1/llm", tunnel.url).as_str(), Reach::Mine, true));
        brain.keep_helper_models().unwrap();
        assert_eq!(rows(&brain), ["home-a"], "no row made for the road");
        let now = brain.now(ModelProfile::Fast);
        assert_eq!((now.state, now.helper.as_deref(), now.reach), (State::Ready, Some("home-a"), Some(Reach::Mine)));
        assert!(brain.view().providers.iter().any(|view| view.id == row.id && view.cloud), "out of the home for now");

        // Home again: its own road, and nothing written all along.
        lan.set_away(false);
        station.helpers.check_all().await;
        let models = station.helpers.model_helpers();
        assert_eq!((models[0].via.as_deref(), models[0].reach), (None, Reach::Home));
        brain.keep_helper_models().unwrap();
        assert_eq!(rows(&brain), ["home-a"]);
        assert_eq!(brain.view().providers.iter().find(|view| view.id == row.id).map(|view| view.cloud), Some(false));
        let mut written = 0;
        while let Ok(event) = events.try_recv() {
            written += usize::from(matches!(&*event, GatewayEvent::BrainChanged { change, .. } if change != "choice"));
        }
        assert_eq!(written, 0, "the row stood as it was");
    }

    /// This computer's model servers (stage 6): the server of whichever row the choice points at — LM Studio added after
    /// Ollama, or Ollama — closed, one look gives the backup at once; nothing goes to the closed server, which «Где
    /// сейчас» and «Спросить мозг» say in their own words.
    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn closing_whichever_local_model_answers_gives_way_to_the_backup() {
        use crate::config::ModelProfile;
        use crate::dialog::brain::brain_service::BUILTIN_AWAY;
        use crate::dialog::llm::brain::BrainSource;
        use crate::helpers::builtin::tests::{computer, provider, said, server};
        use crate::helpers::OwnParts;
        let (ollama, lmstudio) = (server("Ollama.").await, server("LM Studio.").await);
        let (app, _dir) = computer(&[]);
        let first = provider(&app, "ollama", &ollama.url);
        let second = provider(&app, "lmstudio", &lmstudio.url);
        let cloud = provider(&app, "custom-openai", "https://llm.example.invalid/v1");
        let choose = |fast: &str| {
            let choice = serde_json::json!({
                "fast": {"provider": fast, "model": "local-model"},
                "backup": {"provider": cloud, "model": "cloud-model"},
            });
            app.brain.choose(choice.as_object().unwrap(), None).unwrap();
        };
        let answering = || {
            let slot = BrainSource::slot(&*app.brain, ModelProfile::Fast).unwrap();
            (slot.provider.clone(), slot.backup.as_ref().map(|backup| backup.slot.provider.clone()))
        };
        for (chosen, server, answer) in [(second, lmstudio, "LM Studio."), (first, ollama, "Ollama.")] {
            choose(&chosen);
            app.builtin.check().await;
            assert_eq!(answering(), (chosen.clone(), Some(cloud.clone())), "on the line, the backup behind it");
            let slot = BrainSource::slot(&*app.brain, ModelProfile::Fast).unwrap();
            assert_eq!((said(&slot).await.as_str(), server.chats()), (answer, 1), "its own server answers");
            let mine = app.helpers.model_helpers();
            let model = mine.iter().find(|model| model.provider.as_deref() == Some(chosen.as_str())).unwrap();
            assert!(model.builtin && model.id == crate::helpers::SELF && model.up && model.reach == Reach::Home);
            server.close().await;
            app.builtin.check().await;
            assert_eq!(answering(), (cloud.clone(), None), "the backup at once");
            let now = app.brain.now(ModelProfile::Fast);
            assert_eq!(now.reason.as_deref(), Some(BUILTIN_AWAY), "«модель на этом компьютере не отвечает»");
            let card = app.brain.view().providers.into_iter().find(|row| row.id == chosen).unwrap().helper.unwrap();
            assert_eq!((card.id.as_str(), card.state, card.builtin), (crate::helpers::SELF, HelperState::Down, true));
        }
        assert!(app.helpers.model_helpers().iter().all(|model| !model.up), "both closed now");
    }

    /// A row of the person's own at this computer's address stays theirs: «Мозг» never makes it a helper's row.
    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn a_local_custom_row_stays_the_persons_and_editable() {
        use crate::dialog::brain::ProviderInput;
        use crate::helpers::builtin::tests::{computer, provider, server};
        use crate::helpers::OwnParts;
        let local = server("Свой.").await;
        let (app, _dir) = computer(&[]);
        let row = provider(&app, "custom-openai", &local.url);
        let choice = serde_json::json!({"fast": {"provider": row, "model": "local-model"}});
        app.brain.choose(choice.as_object().unwrap(), None).unwrap();
        app.builtin.check().await;
        app.brain.keep_helper_models().unwrap();
        let view = app.brain.view().providers.into_iter().find(|view| view.id == row).unwrap();
        assert_eq!((view.preset.as_str(), view.title.as_str()), ("custom-openai", "Свой, OpenAI-совместимый"));
        assert_eq!(
            view.helper.map(|helper| (helper.id, helper.builtin)),
            Some((crate::helpers::SELF.to_string(), true))
        );
        assert!(app.brain.view().providers.iter().all(|view| view.preset != "helper"), "no helper's row made");
        let renamed = app.brain.update(&row, &ProviderInput::from_value(serde_json::json!({"title": "Мой"})), None);
        assert_eq!(renamed.unwrap().title, "Мой");
        local.close().await;
    }

    /// A start has read no helper yet, and one not read only may have a model: no row is made for a maybe, and none is
    /// dropped. A computer without a model of its own, away while the station starts again, never gets one; the
    /// server's stays.
    #[tokio::test]
    async fn a_start_makes_and_drops_no_row_until_the_helpers_are_read() {
        let modelless: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "model").collect();
        let (computer, server) = (stand(contract("pc", &modelless)).await, stand(contract("srv", ALL)).await);
        let env = [("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY), ("DIALOG_PROVIDER", "none")];
        let station = station(&env);
        let brain = brain_over(&station, station.helpers.clone());
        brain.keep_helper_models().unwrap();
        assert!(rows(&brain).is_empty(), "the computer is not read yet: nothing made for a maybe");
        station.put("own-x", &server.url, HelperKind::Own);
        station.helpers.check_all().await;
        brain.keep_helper_models().unwrap();
        assert_eq!(rows(&brain), ["own-x"], "the computer has none: the server's");

        // Started again while the computer is off: a new engine over the same stores, nobody read.
        computer.set_away(true);
        let profile = crate::core::build::Profile::for_role(station.config.placement.role.as_deref());
        let started = Helpers::new(
            station.config.clone(),
            station.settings.clone(),
            station.secrets.clone(),
            station.events.clone(),
            profile,
            station.identity.clone(),
        );
        let again = brain_over(&station, started.clone());
        again.keep_helper_models().unwrap();
        assert_eq!(rows(&again), ["own-x"], "kept at the start");
        started.check_all().await;
        again.keep_helper_models().unwrap();
        assert_eq!(rows(&again), ["own-x"], "the computer away is not read yet");
        computer.set_away(false);
        started.check_all().await;
        again.keep_helper_models().unwrap();
        assert_eq!(rows(&again), ["own-x"], "read at last, the computer has none");
        started.stop();
    }
}
