//! What works where (core/availability.rs) as the helpers give it. The registry's context is read from what the
//! helpers keep in memory — the list, the regulator, the last looks —, never from the settings table: it is read
//! often — every `/health` (its modes), the mode switch, `/api/v1/availability`, each recheck on an event. An ability
//! counts as set up when a helper the gateway may take from declares it (or has not answered yet), and as given when
//! `pick` finds who gives it — the same choice of road the hearing, the voice and the apps take, so the mode switch and
//! the panels say what the gateway does. Helpers the regulator keeps out, all of them, are kept out for now (`shut`):
//! their ways stay and say so.

use super::contract::{Abilities, Ability};
use super::{Giver, Helpers, Reach};
use crate::core::availability::{HelperContext, HomeServer};
use crate::placement::HelperSource;

impl HelperSource for Helpers {
    /// The abilities of contract 1: set up — someone would give it (`offer`); given — someone gives it now (`pick`);
    /// waiting — the one that would give it is on the line and does not yet (as `svoi_lacks` judges it), its messages
    /// with whether it says it has no bot. What this gateway gives itself (a computer's built-in helper) counts as given
    /// here and on the line. Shut — helpers set up, the regulator keeping every one out (as «Свой» leaving judges it).
    fn helpers(&self) -> HelperContext {
        let mut context =
            HelperContext { online: self.state() == HomeServer::Up, shut: self.shut(), ..HelperContext::default() };
        let mut itself = Abilities::NONE;
        for ability in Ability::V1 {
            let given = self.pick(ability);
            match &given {
                Some(Giver::Itself) => itself = itself.with(ability),
                Some(Giver::Helper(_)) => context.up = context.up.with(ability),
                None => {}
            }
            let Some(offered) = self.offer(ability) else { continue };
            context.configured = context.configured.with(ability);
            if let (None, Giver::Helper(remote)) = (&given, &offered) {
                if remote.up() {
                    context.waiting = context.waiting.with(ability);
                    context.no_bot |= ability == Ability::Messages && remote.no_bot();
                }
            }
        }
        context.with_builtin(itself)
    }

    /// «Любые облака» only: «Только дома» and «Дом и моё облако» keep the clouds by keys out.
    fn clouds(&self) -> bool {
        self.lets(Reach::Foreign)
    }

    fn own_lacks(&self) -> Option<&'static str> {
        self.svoi_lacks()
    }

    /// What an entry of the list would give, the regulator and its liveness aside: switched on, keyed, not this
    /// gateway, not switched off for the ability, declaring it (or not read yet) — the helpers «Где сейчас» names.
    fn listed(&self) -> Abilities {
        let snap = self.snap();
        let listed: Vec<_> = snap
            .list
            .iter()
            .filter(|entry| entry.enabled && !self.is_self(entry))
            .filter_map(|entry| self.remote(entry).map(|remote| (entry, remote)))
            .collect();
        Ability::V1
            .into_iter()
            .filter(|ability| {
                listed.iter().any(|(entry, remote)| !entry.without.contains(ability) && remote.declares(*ability))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core::availability::{Runs, State};
    use crate::core::build::{ModeGate, Profile};
    use crate::core::events::{EventBus, GatewayEvent};
    use crate::helpers::tests::{contract, stand, station, station_gated, ALL, KEY};
    use crate::helpers::{key_secret, AddInput, Away, Change, HelperKind, HelperSetting, RETURN_AFTER, SHUT};
    use crate::placement::AvailabilityService;
    use crate::settings::definitions::{PanelMode, AWAY as AWAY_SETTING, HELPERS};

    /// Contract 1 without these abilities, the rest ready.
    fn without(abilities: &[&str]) -> Vec<(&'static str, bool)> {
        ALL.iter().copied().filter(|(id, _)| !abilities.contains(id)).collect()
    }

    /// The news «what a helper gives changed» (`helpers.changed`, `contract`) since the last call.
    fn contract_news(heard: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> usize {
        let mut news = 0;
        while let Ok(event) = heard.try_recv() {
            if matches!(&*event, GatewayEvent::HelpersChanged { change, .. } if change == "contract") {
                news += 1;
            }
        }
        news
    }

    /// «Переключатель режимов — по pick»: an ability counts as given exactly when the choice of road finds who gives
    /// it; set up while a helper the gateway may take from declares it or has not answered yet — the regulator, the
    /// switch and «Не брать у него…» judged as `pick` judges them.
    #[tokio::test]
    async fn the_registry_reads_what_the_helpers_are_set_to_give_and_what_they_give_now() {
        let (deaf, all) = (stand(contract("a", &without(&["hearing"]))).await, stand(contract("b", ALL)).await);
        let station = station(&[]);
        let given = |context: HelperContext| {
            for ability in Ability::V1 {
                assert_eq!(context.up.has(ability), station.helpers.pick(ability).is_some(), "{ability:?}");
            }
        };
        assert_eq!(station.helpers.helpers(), HelperContext::default(), "no helper: nothing set up");

        station.put("home-a", &deaf.url, HelperKind::Home);
        let unread = station.helpers.helpers();
        assert_eq!((unread.online, unread.configured, unread.up), (false, Abilities::V1, Abilities::NONE));
        station.helpers.check_all().await;
        let read = station.helpers.helpers();
        assert!(read.online && read.builtin.is_empty(), "{read:?}");
        assert!(!read.configured.has(Ability::Hearing) && read.configured.has(Ability::Messages), "{read:?}");
        assert!(read.up.has(Ability::Voice) && !read.up.has(Ability::Messages), "{read:?}");
        assert_eq!(read.waiting, Abilities::of(&[Ability::Messages]), "on the line, its messages not given");
        assert!(read.no_bot, "its contacts answer that it has no bot");
        given(read);

        station.put("home-b", &all.url, HelperKind::Home);
        station.helpers.check_all().await;
        let both = station.helpers.helpers();
        assert_eq!(both.configured, Abilities::V1);
        assert!(both.up.has(Ability::Hearing), "the second hears: {both:?}");
        given(both);

        // «Не брать у него…» the hearing, and nobody else hears.
        station.change("home-b", Change { without: Some(vec![Ability::Hearing]), ..Change::default() });
        let kept = station.helpers.helpers();
        assert!(!kept.configured.has(Ability::Hearing) && !kept.up.has(Ability::Hearing), "{kept:?}");
        given(kept);

        // Both away: set up all the same, nothing given, nobody on the line.
        deaf.set_away(true);
        all.set_away(true);
        for _ in 0..crate::helpers::DOWN_AFTER {
            station.helpers.check_all().await;
        }
        let away = station.helpers.helpers();
        assert_eq!((away.online, away.up, away.waiting, away.no_bot), (false, Abilities::NONE, Abilities::NONE, false));
        assert_eq!((away.configured, away.shut), (kept.configured, false));
    }

    /// The regulator: a helper outside the home that it keeps out gives nothing and is set up for nothing — kept out
    /// for now (`shut`), when it keeps out every one; the clouds by keys only at «Любые облака».
    #[tokio::test]
    async fn the_regulator_keeps_my_server_and_the_clouds_out_of_the_registry() {
        let own = stand(contract("o", ALL)).await;
        let station = station(&[]);
        station.put("own-o", &own.url, HelperKind::Own);
        station.helpers.check_all().await;
        assert!(station.helpers.helpers().up.has(Ability::Hearing) && station.helpers.clouds());
        station.helpers.set_away(Away::Mine, None).unwrap();
        assert!(station.helpers.helpers().up.has(Ability::Hearing), "«Дом и моё облако»: my server too");
        assert!(!station.helpers.clouds(), "but no cloud by keys");
        station.helpers.set_away(Away::Home, None).unwrap();
        let shut = HelperContext { shut: true, ..HelperContext::default() };
        assert_eq!(station.helpers.helpers(), shut, "«Только дома»: none to lean on for now");
        assert!(!station.helpers.clouds());
        // A helper at home beside it is never kept out: not shut.
        let home = stand(contract("h", ALL)).await;
        station.put("home-h", &home.url, HelperKind::Home);
        station.helpers.check_all().await;
        let leaning = station.helpers.helpers();
        assert!(!leaning.shut && leaning.online && leaning.up.has(Ability::Hearing), "{leaning:?}");
    }

    /// On a computer, a part a helper of its list would give stays that helper's — on the line, away, or kept out by
    /// the regulator —, never counted as the computer's own (review of stage 6): `listed` reads the list as «Где
    /// сейчас» does, the regulator and the looks aside. (These helpers have no parts of their own.)
    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn on_a_computer_a_part_its_list_would_give_is_never_its_own_while_the_helper_is_away_or_kept_out() {
        let helper = stand(contract("o", ALL)).await;
        let listing = station(&[]);
        let registry = AvailabilityService::with(Profile { station: false }, false, false, EventBus::new());
        registry.set_helpers(listing.helpers.clone());
        assert_eq!(listing.helpers.listed(), Abilities::NONE, "nothing listed");
        listing.put("own-o", &helper.url, HelperKind::Own);
        listing.helpers.check_all().await;
        assert_eq!(listing.helpers.listed(), Abilities::V1);
        let asr = || {
            let state = registry.get("asr.own").unwrap();
            (state.state, state.runs, state.reason)
        };
        assert_eq!(asr(), (State::Ready, Some(Runs::Helper), "у Турбо".to_string()));

        helper.set_away(true);
        for _ in 0..crate::helpers::DOWN_AFTER {
            listing.helpers.check_all().await;
        }
        assert_eq!(asr(), (State::Fallback, Some(Runs::Yandex), "Турбо не на связи — у Яндекса".to_string()));
        assert!(!registry.view().helpers.builtin.has(Ability::Hearing));

        // On the line again, and «Только дома» keeps it out: still nobody's here.
        helper.set_away(false);
        listing.helpers.check_all().await;
        listing.helpers.set_away(Away::Home, None).unwrap();
        assert_eq!(listing.helpers.listed(), Abilities::V1, "the regulator aside");
        let said = "регулятор не пускает Турбо — у Яндекса".to_string();
        assert_eq!(asr(), (State::Fallback, Some(Runs::Yandex), said));
        assert_eq!(registry.view().helpers.builtin, Abilities::NONE);
        // Switched off, or off for the hearing, it lists nothing of it.
        listing.change("own-o", Change { without: Some(vec![Ability::Hearing]), ..Change::default() });
        assert!(!listing.helpers.listed().has(Ability::Hearing) && listing.helpers.listed().has(Ability::Voice));
        listing.change("own-o", Change { enabled: Some(false), ..Change::default() });
        assert_eq!(listing.helpers.listed(), Abilities::NONE);
    }

    /// The review's case: a station in «Своё» leaning only on its own server takes «Только дома» in a package and starts
    /// again. The regulator keeps the helpers out for now, not for good: the start keeps «Свой» (the registry is the
    /// switch's gate, as in `App`) and says why it is out of reach, the first tick leaves it saying so, and once they
    /// are let out it comes back by itself.
    #[tokio::test]
    async fn a_station_started_in_svoi_with_every_helper_kept_out_keeps_it_to_leave_and_bring_back_by_itself() {
        let own = stand(contract("o", ALL)).await;
        let registry = Arc::new(AvailabilityService::with(Profile { station: true }, false, false, EventBus::new()));
        let gate = registry.clone();
        let store = |settings: &Arc<crate::settings::SettingsService>,
                     secrets: &Arc<crate::secrets::SecretsService>| {
            let entry = HelperSetting {
                id: "own-o".into(),
                name: "Мой сервер".into(),
                url: own.url.clone(),
                kind: HelperKind::Own,
                enabled: true,
                without: Vec::new(),
                helper_id: None,
            };
            settings.set(&HELPERS, &vec![entry], None).unwrap();
            settings.set(&AWAY_SETTING, &Away::Home, None).unwrap();
            secrets.set(&key_secret("own-o"), Some(KEY)).unwrap();
        };
        let station = station_gated(&[], true, store, move |helpers| {
            gate.set_helpers(helpers.clone());
            gate
        });
        assert_eq!(registry.absent(PanelMode::Own), None, "kept out for now: «Свой» is not gone from the station");
        assert_eq!(station.modes.current(), PanelMode::Own, "the start keeps it");
        assert_eq!(
            registry.check(PanelMode::Own),
            Err("«Ваш ИИ» сейчас не включить: регулятор не пускает Турбо.".to_string())
        );
        for id in ["mode.own", "brain.local", "voice.cloned", "messages"] {
            let state = registry.get(id).unwrap();
            assert_eq!((state.state, state.reason.as_str()), (State::Off, "Регулятор не пускает Турбо"), "{id}");
        }

        station.helpers.check_all().await;
        assert_eq!(station.mode(), (PanelMode::Gateway, Some(SHUT.to_string())), "left at the first tick, saying why");
        station.helpers.set_away(Away::Any, None).unwrap();
        station.helpers.check_all().await;
        assert_eq!(station.modes.current(), PanelMode::Gateway, "not before a minute on the line");
        station.helpers.age_up(RETURN_AFTER);
        station.helpers.check_all().await;
        assert_eq!(station.mode(), (PanelMode::Own, None), "back by itself");
    }

    /// «у помощника не настроен бот Telegram» only when the helper says so — its contacts answer that it has no bot,
    /// or its contract declares the messages not ready; its contacts not read yet (just added) is «ещё не готов». Its
    /// bot found or gone is news: the panels read the registry again.
    #[tokio::test]
    async fn the_messages_say_no_bot_only_when_the_helper_has_none_and_its_bot_found_is_news() {
        let helper = stand(contract("a", ALL)).await;
        let station = station(&[]);
        let registry = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
        registry.set_helpers(station.helpers.clone());
        let messages = || {
            let state = registry.get("messages").unwrap();
            (state.state, state.reason)
        };
        let mut heard = station.events.subscribe();

        let add = |url: &str| AddInput { url: url.to_string(), key: KEY.into(), ..AddInput::default() };
        station.helpers.add(add(&helper.url), None).await.unwrap();
        assert_eq!(messages(), (State::Off, "Турбо ещё не готово".to_string()), "its contacts not read yet");
        assert_eq!(contract_news(&mut heard), 0);

        station.helpers.check_all().await;
        assert_eq!(messages(), (State::Off, "У Турбо не настроен бот Telegram".to_string()), "503: no bot");
        assert_eq!(contract_news(&mut heard), 1, "what it gives is known now");
        *helper.contacts.lock().unwrap() = Some(vec!["Мама".into()]);
        station.helpers.check_all().await;
        assert_eq!(messages(), (State::Ready, "у Турбо".to_string()));
        assert_eq!(contract_news(&mut heard), 1, "its bot found");
        *helper.contacts.lock().unwrap() = Some(vec!["Мама".into(), "Папа".into()]);
        station.helpers.check_all().await;
        assert_eq!(contract_news(&mut heard), 0, "a new contact is not news to the registry");

        // A contract that declares the messages not ready has no bot, read or not.
        let botless = stand(contract("b", &[("hearing", true), ("voice", true), ("messages", false)])).await;
        let other = crate::helpers::tests::station(&[]);
        let registry = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
        registry.set_helpers(other.helpers.clone());
        other.helpers.add(add(&botless.url), None).await.unwrap();
        assert_eq!(registry.get("messages").unwrap().reason, "У Турбо не настроен бот Telegram");
    }

    /// «Готово, когда: «Своё» на колонке без помощника со слухом — отказ словами, как сейчас»: the station's switch
    /// asks the registry, the registry asks the helpers — their hearing is nobody's, so «Свой» is refused with why;
    /// given, it may be chosen and runs at the helper. No helper at all: the words of the station alone.
    #[tokio::test]
    async fn svoi_on_a_station_whose_helpers_give_no_hearing_is_refused_with_words() {
        let helper = stand(contract("a", &without(&["hearing"]))).await;
        let station = station(&[]);
        let registry = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
        registry.set_helpers(station.helpers.clone());
        assert_eq!(
            registry.check(PanelMode::Own),
            Err("«Ваш ИИ» сейчас не включить: нет облачного распознавания.".to_string()),
            "the station alone, no cloud either"
        );

        station.put("home-a", &helper.url, HelperKind::Home);
        station.helpers.check_all().await;
        assert_eq!(
            registry.check(PanelMode::Own),
            Err("«Ваш ИИ» сейчас не включить: нет Турбо со слухом.".to_string())
        );
        let own = registry.get("mode.own").unwrap();
        assert_eq!((own.state, own.reason.as_str()), (State::Off, "Нет Турбо со слухом"));
        assert_eq!(registry.absent(PanelMode::Own), None, "a stored «Свой» is kept: the helpers decide");

        *helper.status.lock().unwrap() = contract("a", ALL);
        station.helpers.check_all().await;
        assert!(registry.check(PanelMode::Own).is_ok());
        let own = registry.get("mode.own").unwrap();
        assert_eq!((own.state, own.runs, own.reason.as_str()), (State::Ready, Some(Runs::Helper), "у Турбо"));
        let asr = registry.get("asr.own").unwrap();
        assert_eq!((asr.runs, asr.reason.as_str()), (Some(Runs::Helper), "у Турбо"));
    }
}
