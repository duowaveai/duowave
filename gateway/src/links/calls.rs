//! Our calls (src/calls) and the modules around them, as calls.module.ts, skills.module.ts and the station watch
//! linked them: the stations say which address is an approved station's, the calls skill reaches the hub, the calls
//! service and the Telegram bot and hears their events, and the station watch alerts through the bot.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::calls::call_hub::party_of;
use crate::calls::{
    Call, CallHub, CallState as HubCallState, CallsService, EndpointKind, HubEvent, Party, StationTrust,
    TelegramMessenger,
};
use crate::operations::station_watch::TelegramSender;
use crate::skills::own::calls_skill::{
    CallParty, CallState, CallView, CallsAccess, CallsSkill, Contact, HubAccess, IncomingMessage, Invitation,
    LinkButton, MessengerAccess, PartyKind,
};
use crate::skills::{CallsLinks, SkillsService};
use crate::stations::StationsService;

/// Who may stand in for the station on a call: the stations' own answer (any address with STATION_PAIRING=open).
impl StationTrust for StationsService {
    fn trusted_address(&self, address: &str) -> bool {
        StationsService::trusted_address(self, Some(address))
    }
}

/// The calls module as the skills reach it.
pub fn calls_links(calls: &Arc<CallsService>) -> CallsLinks {
    CallsLinks {
        hub: Arc::new(SkillsHub(calls.hub.clone())),
        calls: Arc::new(SkillsCalls(calls.clone())),
        messenger: Arc::new(SkillsTelegram(calls.telegram.clone())),
    }
}

/// The calls tell the calls skill of their events, as the TS skill listened to the hub and the bot: a call ringing
/// (announced on the station), one ended (said when nobody took the station's), parties coming and going (a contact
/// joining by the link), a contact's message in Telegram.
pub fn report_calls(calls: &CallsService, skills: &SkillsService) {
    tell_calls_skill(&calls.hub, &calls.telegram, &skills.calls);
}

/// The hub's and the bot's events to the skill. The hub tells its listeners outside its lock, so the skill may ask it
/// at once; the listeners keep the skill alive no longer than the skills service does.
pub fn tell_calls_skill(hub: &CallHub, telegram: &TelegramMessenger, skill: &Arc<CallsSkill>) {
    let heard = Arc::downgrade(skill);
    hub.on(move |event| {
        let Some(skill) = heard.upgrade() else { return };
        match event {
            HubEvent::Ringing(call) => skill.on_ringing(call_view(call)),
            HubEvent::Ended { call, reason } => skill.on_ended(call_view(call), reason),
            HubEvent::Endpoints => skill.on_endpoints(),
            HubEvent::Connected(_) => {}
        }
    });
    let heard = Arc::downgrade(skill);
    telegram.on_message(move |message| {
        if let Some(skill) = heard.upgrade() {
            skill.on_message(IncomingMessage {
                from: message.from.clone(),
                chat_id: message.chat_id.clone(),
                file: message.file.clone(),
                text: message.text.clone(),
            });
        }
    });
}

fn party(party: &Party) -> CallParty {
    let kind = match party.kind {
        EndpointKind::Station => PartyKind::Station,
        EndpointKind::Browser => PartyKind::Browser,
        EndpointKind::Sip => PartyKind::Sip,
        EndpointKind::Telegram => PartyKind::Telegram,
        EndpointKind::Sim => PartyKind::Sim,
        EndpointKind::Livekit => PartyKind::Livekit,
    };
    CallParty { id: party.id.clone(), name: party.name.clone(), kind }
}

fn call_view(call: &Call) -> CallView {
    CallView {
        id: call.id.clone(),
        from: party(&call.from),
        to: party(&call.to),
        state: match call.state {
            HubCallState::Ringing => CallState::Ringing,
            HubCallState::Active => CallState::Active,
            HubCallState::Ended => CallState::Ended,
        },
        answered: call.answered_at.is_some(),
    }
}

/// Our call hub as the calls skill sees it.
pub struct SkillsHub(pub Arc<CallHub>);

impl HubAccess for SkillsHub {
    fn endpoint(&self, id: &str) -> Option<CallParty> {
        self.0.endpoint(id).map(|endpoint| party(&party_of(endpoint.as_ref())))
    }

    fn list(&self) -> Vec<CallParty> {
        self.0.list().iter().map(party).collect()
    }

    fn find(&self, name_or_id: &str, except: Option<&str>) -> Option<CallParty> {
        self.0.find(name_or_id, except).map(|endpoint| party(&party_of(endpoint.as_ref())))
    }

    fn call_of(&self, endpoint_id: &str) -> Option<CallView> {
        self.0.call_of(endpoint_id).as_ref().map(call_view)
    }

    fn call(&self, from_id: &str, to_id: &str) -> anyhow::Result<CallView> {
        Ok(call_view(&self.0.call(from_id, to_id)?))
    }

    fn accept(&self, call_id: &str, by_id: &str) -> anyhow::Result<()> {
        self.0.accept(call_id, by_id)?;
        Ok(())
    }

    fn hangup(&self, call_id: &str, by_id: &str) {
        if let Err(error) = self.0.hangup(call_id, by_id) {
            tracing::warn!(target: "calls", "hang up {call_id} by {by_id}: {error}");
        }
    }
}

/// The calls service as the calls skill sees it: SIP and the LiveKit page.
pub struct SkillsCalls(pub Arc<CallsService>);

impl CallsAccess for SkillsCalls {
    fn station_name(&self) -> String {
        self.0.station_name().to_string()
    }

    fn can_invite(&self) -> bool {
        self.0.can_invite()
    }

    fn invite(&self, name: String) -> BoxFuture<'static, Option<Invitation>> {
        let invitation = self.0.invite(&name).map(|invite| Invitation { identity: invite.identity, link: invite.link });
        Box::pin(async move { invitation })
    }

    fn sip_contact(&self, name: &str) -> Option<String> {
        self.0.sip_contact(name)
    }

    fn dial_sip(&self, number: String, name: String) -> BoxFuture<'static, anyhow::Result<CallParty>> {
        let calls = self.0.clone();
        Box::pin(async move {
            let phone = calls.dial_sip(&number, &name).await?;
            Ok(party(&party_of(phone.as_ref())))
        })
    }
}

/// The Telegram bot as the calls skill sees it.
pub struct SkillsTelegram(pub Arc<TelegramMessenger>);

impl MessengerAccess for SkillsTelegram {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn contact(&self, name: &str) -> Option<Contact> {
        self.0.contact(name).map(|contact| Contact { name: contact.name, chat_id: contact.chat_id })
    }

    fn send_text(
        &self,
        chat_id: String,
        text: String,
        button: Option<LinkButton>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let telegram = self.0.clone();
        Box::pin(async move {
            let button = button.map(|button| crate::calls::LinkButton { text: button.text, url: button.url });
            telegram.send_text(&chat_id, &text, button.as_ref()).await
        })
    }
}

/// The station watch's alerts to the owner's Telegram, through the calls' bot.
pub struct TelegramAlerts(pub Arc<TelegramMessenger>);

impl TelegramSender for TelegramAlerts {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn chat(&self) -> Option<String> {
        self.0.alert_chat()
    }

    fn send_text<'a>(&'a self, chat: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.0.send_text(chat, text, None).await.map_err(|error| format!("{error:#}")) })
    }
}

/// The bot, SIP and LiveKit as the panel sets them up (src/connectors), for the calls.
pub struct CallsSettings(pub Arc<crate::connectors::settings::ConnectorSettings>);

impl crate::calls::CallsConnectors for CallsSettings {
    fn telegram(&self) -> Option<crate::config::TelegramConfig> {
        self.0.telegram()
    }

    fn sip(&self) -> Option<crate::config::SipConfig> {
        self.0.sip()
    }

    fn livekit(&self) -> Option<crate::config::LiveKitConfig> {
        self.0.livekit()
    }
}

/// The calls follow the panel: a change of the bot, SIP or LiveKit applies at once.
pub fn follow_settings(calls: &Arc<CallsService>, settings: &crate::connectors::settings::ConnectorSettings) {
    use crate::calls::CallsPart;
    use crate::connectors::settings::ConnectorId;
    let calls = Arc::downgrade(calls);
    settings.subscribe(move |id| {
        let Some(calls) = calls.upgrade() else { return };
        match id {
            ConnectorId::Telegram => calls.refresh(CallsPart::Telegram),
            ConnectorId::Sip => calls.refresh(CallsPart::Sip),
            ConnectorId::LiveKit => calls.refresh(CallsPart::LiveKit),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::App;
    use crate::calls::socket_endpoint::SocketEndpoint;
    use crate::config::{Config, Env, OperationsConfig, TelegramConfig};
    use crate::core::events::EventBus;
    use crate::operations::station_watch::ActiveSessions;
    use crate::operations::StationWatch;
    use crate::skills::capabilities::{CapabilityName, Intent};
    use crate::skills::own::reminders_skill::{AudioClip, Speaker};
    use crate::utils::time::now_ms;

    fn app(pairs: &[(&str, &str)]) -> Arc<App> {
        let mut env = vec![("STATE_DIR", ""), ("CALLS_STATION_MIC_PORT", "")];
        env.extend_from_slice(pairs);
        App::build(Config::for_tests(&Env::from_pairs(env))).unwrap()
    }

    /// What the station was told to say.
    #[derive(Default)]
    struct Said(Mutex<Vec<(String, String)>>);

    impl Speaker for Said {
        fn say(&self, device: String, text: String, _clip: Option<AudioClip>) -> BoxFuture<'static, bool> {
            self.0.lock().unwrap().push((device, text));
            Box::pin(async { true })
        }
    }

    /// A party on a socket of its own; what it hears waits in the receiver.
    fn socket_party(
        id: &str,
        kind: EndpointKind,
        name: &str,
    ) -> (Arc<SocketEndpoint>, mpsc::Receiver<axum::extract::ws::Message>) {
        let (sender, receiver) = mpsc::channel(64);
        (Arc::new(SocketEndpoint::new(id.into(), kind, name.into(), sender)), receiver)
    }

    async fn until(check: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if check() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        check()
    }

    #[tokio::test]
    async fn trusts_an_approved_stations_address_only() {
        let tofu = app(&[("STATION_PAIRING", "tofu")]);
        assert!(!tofu.calls.trusted_address("192.168.1.100"), "no station approved yet");
        tofu.stations.admit("kitchen", "::ffff:192.168.1.100");
        assert!(tofu.calls.trusted_address("192.168.1.100"), "the first station is trusted on first use");
        assert!(!tofu.calls.trusted_address("192.168.1.200"));
        let open = app(&[("STATION_PAIRING", "open")]);
        assert!(open.calls.trusted_address("10.0.0.1"));
    }

    #[tokio::test]
    async fn the_calls_skill_hears_the_hub_and_rings_and_answers_through_it() {
        let app = app(&[]);
        let said = Arc::new(Said::default());
        app.skills.attach_speaker(said.clone());
        let (station, _station_hears) = socket_party("station:kitchen", EndpointKind::Station, "Кухня");
        let (mom, _mom_hears) = socket_party("browser:1", EndpointKind::Browser, "Мама");
        app.calls.hub.register(station);
        app.calls.hub.register(mom);
        // «позвони маме» is ours: our hub has her.
        let mut call = Intent::new(CapabilityName::Calls, "call");
        call.params.insert("contact".into(), "маме".into());
        assert!(app.skills.calls.ours(&call, "kitchen"));
        // A call to the station is announced in Alice's voice, through the hub's event.
        let ringing = app.calls.hub.call("browser:1", "station:kitchen").unwrap();
        assert!(until(|| !said.0.lock().unwrap().is_empty()).await, "the ringing call is announced");
        assert_eq!(
            said.0.lock().unwrap()[0],
            ("kitchen".to_string(), "Вам звонит Мама. Скажите: Алиса, ответь.".to_string())
        );
        // «ответь» takes it through the hub.
        let answer = Intent::new(CapabilityName::Calls, "answer");
        assert!(app.skills.calls.ours(&answer, "kitchen"));
        assert_eq!(app.calls.hub.call_of("station:kitchen").unwrap().id, ringing.id);
        let hub = SkillsHub(app.calls.hub.clone());
        hub.accept(&ringing.id, "station:kitchen").unwrap();
        assert_eq!(hub.call_of("station:kitchen").unwrap().state, CallState::Active);
        hub.hangup(&ringing.id, "station:kitchen");
        assert!(hub.call_of("station:kitchen").is_none());
    }

    /// The station's call nobody took: when it stops ringing (45 s), the station hears so.
    #[tokio::test(start_paused = true)]
    async fn the_station_hears_that_nobody_took_its_call() {
        let app = app(&[]);
        let said = Arc::new(Said::default());
        app.skills.attach_speaker(said.clone());
        let (station, _station_hears) = socket_party("station:kitchen", EndpointKind::Station, "Кухня");
        let (mom, _mom_hears) = socket_party("browser:1", EndpointKind::Browser, "Мама");
        app.calls.hub.register(station);
        app.calls.hub.register(mom);
        app.calls.hub.call("station:kitchen", "browser:1").unwrap();
        tokio::time::sleep(Duration::from_secs(46)).await;
        assert!(until(|| !said.0.lock().unwrap().is_empty()).await, "the end is said");
        assert_eq!(said.0.lock().unwrap()[0], ("kitchen".to_string(), "Мама не отвечает.".to_string()));
    }

    /// A Bot API whose first poll brings a contact's text; later polls bring nothing.
    async fn bot_with_a_message() -> String {
        let polls: Arc<Mutex<usize>> = Arc::default();
        let router = Router::new()
            .route(
                "/{bot}/getUpdates",
                axum::routing::get(|State(polls): State<Arc<Mutex<usize>>>| async move {
                    let first = {
                        let mut polls = polls.lock().unwrap();
                        *polls += 1;
                        *polls == 1
                    };
                    if !first {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        return Json(json!({"ok": true, "result": []}));
                    }
                    Json(json!({"ok": true, "result": [
                        {"update_id": 7, "message": {"chat": {"id": 42}, "text": "Скоро буду"}}
                    ]}))
                }),
            )
            .with_state(polls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        base
    }

    #[tokio::test]
    async fn a_contacts_message_in_telegram_is_said_on_the_station() {
        let app = app(&[]);
        let said = Arc::new(Said::default());
        app.skills.attach_speaker(said.clone());
        let config = TelegramConfig {
            token: "123:test".into(),
            contacts: [("мама".to_string(), "42".to_string())].into_iter().collect(),
            inbox_dir: std::env::temp_dir(),
            alert_chat: None,
        };
        let bot = TelegramMessenger::with_api(Some(config), &bot_with_a_message().await);
        tell_calls_skill(&app.calls.hub, &bot, &app.skills.calls);
        bot.start();
        assert!(until(|| !said.0.lock().unwrap().is_empty()).await, "the message is said");
        bot.stop();
        assert_eq!(said.0.lock().unwrap()[0], ("*".to_string(), "Мама пишет: Скоро буду".to_string()));
    }

    /// A Bot API server of our own: what was sent to it.
    async fn bot_api() -> (String, Arc<Mutex<Vec<Value>>>) {
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let router = Router::new()
            .route(
                "/{bot}/sendMessage",
                post(|State(sent): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    sent.lock().unwrap().push(body);
                    Json(json!({"ok": true, "result": {}}))
                }),
            )
            .with_state(sent.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        (base, sent)
    }

    /// No station connected.
    struct Away;

    impl ActiveSessions for Away {
        fn active_count(&self) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn alerts_and_the_skills_messages_go_through_the_calls_bot() {
        let (api, sent) = bot_api().await;
        let config = TelegramConfig {
            token: "123:test".into(),
            contacts: [("мама".to_string(), "42".to_string())].into_iter().collect(),
            inbox_dir: std::env::temp_dir(),
            alert_chat: Some("7".into()),
        };
        let bot = TelegramMessenger::with_api(Some(config), &api);
        let alerts = TelegramAlerts(bot.clone());
        assert!(alerts.enabled());
        assert_eq!(alerts.chat().as_deref(), Some("7"));
        // The station watch, two minutes without a station: the owner's chat hears it.
        let operations = OperationsConfig {
            offline_alert_ms: 60_000,
            notify: false,
            webhook_url: None,
            retention_days: 0,
            journal_cap: 0,
        };
        let watch = StationWatch::new(Arc::new(Away), Some(Arc::new(alerts)), operations, EventBus::new());
        watch.check(now_ms() + 120_000).await;
        let skills = SkillsTelegram(bot.clone());
        assert_eq!(skills.contact("маме"), Some(Contact { name: "мама".into(), chat_id: "42".into() }));
        let button = LinkButton { text: "Ответить".into(), url: "https://calls.example.org/#access=x".into() };
        skills.send_text("42".into(), "Вам звонит Колонка.".into(), Some(button)).await.unwrap();
        let sent = sent.lock().unwrap().clone();
        assert_eq!(
            sent[0],
            json!({
                "chat_id": "7",
                "text": "Колонка не подключена к шлюзу 2 мин. Голоса у неё нет; вернуть к Яндексу: scripts/station-voice.sh yandex."
            })
        );
        assert_eq!(sent[1]["chat_id"], "42");
        assert_eq!(sent[1]["reply_markup"]["inline_keyboard"][0][0]["text"], "Ответить");
        assert!(!TelegramAlerts(TelegramMessenger::new(None)).enabled(), "no bot until the panel sets one up");
    }
}
