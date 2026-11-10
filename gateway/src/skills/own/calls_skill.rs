//! Our own calls as a capability of the gateway: a call to the station is announced in Alice's voice ("Вам звонит
//! мама. Скажите: Алиса, ответь."), "ответь" and "положи трубку" go to our hub, "позвони маме" rings a contact on
//! the call page or a SIP phone, or sends a Telegram contact a link into the LiveKit room and rings them when they
//! join, "передай маме…" sends a Telegram message, and a contact's voice message is played on the station.
//! Anything else about calls is Yandex's (its own calls through the app): an intent is ours only when our hub or
//! our contacts have it (`params.ours`, set by the router's interceptor).
//!
//! The calls module (the hub, SIP and LiveKit, the Telegram bot) is reached through the traits below; it tells
//! the skill of its events by `on_ringing`, `on_ended`, `on_endpoints` and `on_message`, outside its own locks.
//! On the station, whose build has no calls module, «передай …» goes through the computer's bot while the computer
//! helps it (src/home_server/messenger.rs), and the bot's messages come from the computer the other way
//! (src/placement/messages.rs): one part of «Что даю колонке», «Сообщения», for both.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::task::AbortHandle;

use super::reminders_skill::{AudioClip, Speaker};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

const ANNOUNCE_EVERY: Duration = Duration::from_millis(12_000);
const ANNOUNCEMENTS: usize = 3;
/// How long a contact has to open the call's link from Telegram; then the hub rings their page as usual (45 s).
const INVITE: Duration = Duration::from_millis(90_000);
/// «передай …» to a name the bot does not know.
pub const NO_RECIPIENT: &str = "Не знаю, кому передать.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyKind {
    Station,
    Browser,
    Sip,
    Telegram,
    Sim,
    Livekit,
}

/// A party of our calls: the station's client, a browser, a SIP phone, a LiveKit page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallParty {
    /// Unique: `station:<device>`, `browser:<n>`, `sip:<number>`, `livekit:<identity>`.
    pub id: String,
    /// How people call it: «мама», «кухня».
    pub name: String,
    pub kind: PartyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Ringing,
    Active,
    Ended,
}

/// A call as the hub has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallView {
    pub id: String,
    pub from: CallParty,
    pub to: CallParty,
    pub state: CallState,
    /// The callee took it.
    pub answered: bool,
}

/// What the skill needs of our call hub (src/calls/call-hub.ts).
pub trait HubAccess: Send + Sync {
    fn endpoint(&self, id: &str) -> Option<CallParty>;
    fn list(&self) -> Vec<CallParty>;
    /// An endpoint by id, or by the name people say («маме» finds «Мама»), other than `except`.
    fn find(&self, name_or_id: &str, except: Option<&str>) -> Option<CallParty>;
    /// The call an endpoint takes part in, ringing or going on.
    fn call_of(&self, endpoint_id: &str) -> Option<CallView>;
    /// Rings `to` from `from`; an error when either is missing or busy.
    fn call(&self, from_id: &str, to_id: &str) -> anyhow::Result<CallView>;
    /// The callee answers a ringing call.
    fn accept(&self, call_id: &str, by_id: &str) -> anyhow::Result<()>;
    fn hangup(&self, call_id: &str, by_id: &str);
}

/// A link into the LiveKit room with a day's access, for a contact the station calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    /// The identity the link lets into the room: the party is `livekit:<identity>` there.
    pub identity: String,
    pub link: String,
}

/// What the skill needs of the calls service (src/calls/calls.service.ts): SIP and the LiveKit page.
pub trait CallsAccess: Send + Sync {
    /// How the station is named in our calls («Колонка»).
    fn station_name(&self) -> String;
    /// Whether the station can call someone outside the room: the LiveKit page has an address from outside.
    fn can_invite(&self) -> bool;
    fn invite(&self, name: String) -> BoxFuture<'static, Option<Invitation>>;
    /// A SIP contact's number, when the SIP bridge is set up.
    fn sip_contact(&self, name: &str) -> Option<String>;
    /// Dials a SIP contact through Asterisk; the phone joins the hub as a party.
    fn dial_sip(&self, number: String, name: String) -> BoxFuture<'static, anyhow::Result<CallParty>>;
}

/// A Telegram contact: the name as in TELEGRAM_CONTACTS and the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub chat_id: String,
}

/// A link button under a message («Ответить» on a call from the station).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkButton {
    pub text: String,
    pub url: String,
}

/// A contact's message in Telegram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub from: String,
    pub chat_id: String,
    /// A voice message's file (Ogg/Opus, as Telegram keeps them).
    pub file: Option<PathBuf>,
    pub text: Option<String>,
    /// The file is a copy made for playing only — a message the computer passed on to the station
    /// (src/home_server/messenger.rs): removed once played. The bot's own inbox is kept.
    pub discard: bool,
}

/// Where a contact's message goes besides the stations at this gateway's own /uni.ws: the gateways of the stations
/// this computer helps, which its bot's messages would never reach otherwise (src/placement/messages.rs).
pub trait MessageRelay: Send + Sync {
    /// Hands the message on, each station in a task of its own: how many stations it goes to.
    fn pass_on(&self, message: &IncomingMessage) -> usize;
}

/// A message that did not go, with the words the station says why: «Компьютер не на связи…». Other errors of
/// `send_text` are the skill's failure as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotSent(pub String);

impl std::fmt::Display for NotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotSent {}

/// What the skill needs of the Telegram bot (src/calls/telegram.ts).
pub trait MessengerAccess: Send + Sync {
    fn enabled(&self) -> bool;
    /// A contact by the name people say («маме» finds «мама»).
    fn contact(&self, name: &str) -> Option<Contact>;
    fn send_text(
        &self,
        chat_id: String,
        text: String,
        button: Option<LinkButton>,
    ) -> BoxFuture<'static, anyhow::Result<()>>;
    /// Why no message can go now, whoever it is for, as the station says it: the computer that holds the bot is
    /// away, or it has no bot. None: the contact decides.
    fn unavailable(&self) -> Option<String> {
        None
    }
}

/// No calls module (the station's own build, or before it is wired): no parties, no SIP, no bot.
pub struct NoCalls;

impl HubAccess for NoCalls {
    fn endpoint(&self, _id: &str) -> Option<CallParty> {
        None
    }
    fn list(&self) -> Vec<CallParty> {
        Vec::new()
    }
    fn find(&self, _name_or_id: &str, _except: Option<&str>) -> Option<CallParty> {
        None
    }
    fn call_of(&self, _endpoint_id: &str) -> Option<CallView> {
        None
    }
    fn call(&self, from_id: &str, _to_id: &str) -> anyhow::Result<CallView> {
        anyhow::bail!("no such party: {from_id}")
    }
    fn accept(&self, _call_id: &str, _by_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("no such call ringing")
    }
    fn hangup(&self, _call_id: &str, _by_id: &str) {}
}

impl CallsAccess for NoCalls {
    fn station_name(&self) -> String {
        "Колонка".into()
    }
    fn can_invite(&self) -> bool {
        false
    }
    fn invite(&self, _name: String) -> BoxFuture<'static, Option<Invitation>> {
        Box::pin(async { None })
    }
    fn sip_contact(&self, _name: &str) -> Option<String> {
        None
    }
    fn dial_sip(&self, _number: String, _name: String) -> BoxFuture<'static, anyhow::Result<CallParty>> {
        Box::pin(async { anyhow::bail!("no SIP bridge (ARI_URL)") })
    }
}

impl MessengerAccess for NoCalls {
    fn enabled(&self) -> bool {
        false
    }
    fn contact(&self, _name: &str) -> Option<Contact> {
        None
    }
    fn send_text(
        &self,
        _chat_id: String,
        _text: String,
        _button: Option<LinkButton>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("no Telegram bot (TELEGRAM_BOT_TOKEN)") })
    }
}

/// A call from the station to a contact who is not in the room yet: it starts when they join by the link.
struct Invite {
    station: String,
    name: String,
    /// Its expiry: stopped when the invite is dropped before it fires.
    timer: Option<AbortHandle>,
}

impl Drop for Invite {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}

/// «мама» in TELEGRAM_CONTACTS, «Мама» in the room and in speech.
fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub struct CallsSkill {
    me: Weak<CallsSkill>,
    hub: Arc<dyn HubAccess>,
    calls: Arc<dyn CallsAccess>,
    telegram: Arc<dyn MessengerAccess>,
    speaker: RwLock<Option<Arc<dyn Speaker>>>,
    relay: RwLock<Option<Arc<dyn MessageRelay>>>,
    /// By the identity the link lets into the room.
    invites: Mutex<HashMap<String, Invite>>,
}

impl CallsSkill {
    pub fn new(
        hub: Arc<dyn HubAccess>,
        calls: Arc<dyn CallsAccess>,
        telegram: Arc<dyn MessengerAccess>,
    ) -> Arc<CallsSkill> {
        Arc::new_cyclic(|me| CallsSkill {
            me: me.clone(),
            hub,
            calls,
            telegram,
            speaker: RwLock::new(None),
            relay: RwLock::new(None),
            invites: Mutex::new(HashMap::new()),
        })
    }

    pub fn attach_speaker(&self, speaker: Arc<dyn Speaker>) {
        *self.speaker.write().unwrap() = Some(speaker);
    }

    /// On the computer: the stations it helps hear its bot's messages too.
    pub fn attach_relay(&self, relay: Arc<dyn MessageRelay>) {
        *self.relay.write().unwrap() = Some(relay);
    }

    fn relay(&self) -> Option<Arc<dyn MessageRelay>> {
        self.relay.read().unwrap().clone()
    }

    fn speaker(&self) -> Option<Arc<dyn Speaker>> {
        self.speaker.read().unwrap().clone()
    }

    /// Whether our hub or our contacts take this intent on this device; otherwise it is Yandex's.
    pub fn ours(&self, intent: &Intent, device_tag: &str) -> bool {
        let station = self.station(device_tag);
        match intent.action.as_str() {
            "answer" => station.is_some_and(|station| self.hub.call_of(&station).is_some()),
            "hangup" => {
                station.is_some_and(|station| self.hub.call_of(&station).is_some() || self.has_invite(&station))
            }
            "call" => {
                let contact = js::string_or_empty(intent.params.get("contact"));
                station.is_some_and(|station| {
                    !contact.is_empty()
                        && (self.hub.find(&contact, Some(&station)).is_some()
                            || self.calls.sip_contact(&contact).is_some()
                            || self.invitable(&contact).is_some())
                })
            }
            "message" => self.telegram.enabled() && self.recipient(intent).is_some(),
            _ => false,
        }
    }

    async fn run(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let station = self.station(&context.device_tag);
        match intent.action.as_str() {
            "answer" => {
                let call = station.as_ref().and_then(|station| self.hub.call_of(station));
                // Live 02.11: «ответь» again when the caller heard nothing was told nobody calls.
                if call.as_ref().is_some_and(|call| call.state == CallState::Active) {
                    return Ok(Outcome::speech("Мы уже на связи."));
                }
                match (call, station) {
                    (Some(call), Some(station)) if call.state == CallState::Ringing && call.to.id == station => {
                        self.hub.accept(&call.id, &station)?;
                        Ok(Outcome::default())
                    }
                    _ => Ok(Outcome::speech("Сейчас никто не звонит.")),
                }
            }
            "hangup" => {
                if let Some(station) = &station {
                    if let Some(call) = self.hub.call_of(station) {
                        self.hub.hangup(&call.id, station);
                    }
                    self.drop_invite_of(station);
                }
                Ok(Outcome::default())
            }
            "call" => {
                let Some(station) = station else {
                    return Ok(Outcome::speech("Колонка пока не подключена к звонкам."));
                };
                let contact = js::string_or_empty(intent.params.get("contact"));
                if let Some(online) = self.hub.find(&contact, Some(&station)) {
                    self.hub.call(&station, &online.id)?;
                    return Ok(Outcome::speech(format!("Звоню: {}.", online.name)));
                }
                if let Some(number) = self.calls.sip_contact(&contact) {
                    let phone = self.calls.dial_sip(number, contact.clone()).await?;
                    self.hub.call(&station, &phone.id)?;
                    return Ok(Outcome::speech(format!("Звоню: {contact}.")));
                }
                let Some(invited) = self.invite(&contact, &station).await? else {
                    return Ok(Outcome::speech(format!("{contact} сейчас не на связи.")));
                };
                Ok(Outcome {
                    speech: Some(format!("Звоню: {invited}. Ссылка на звонок ушла в Telegram.")),
                    done: Some(format!("контакту «{invited}» отправлена в Telegram ссылка на звонок с колонки")),
                    ..Outcome::default()
                })
            }
            "message" => {
                if let Some(reason) = self.telegram.unavailable() {
                    return Ok(Outcome::speech(reason));
                }
                let Some(recipient) = self.recipient(intent) else {
                    return Ok(Outcome::speech(NO_RECIPIENT));
                };
                let text = js::string_or_empty(intent.params.get("text"));
                if let Err(error) = self.telegram.send_text(recipient.chat_id, text.clone(), None).await {
                    return match error.downcast::<NotSent>() {
                        Ok(not_sent) => Ok(Outcome::speech(not_sent.0)),
                        Err(error) => Err(error),
                    };
                }
                Ok(Outcome {
                    speech: Some("Передала.".into()),
                    done: Some(format!("контакту «{}» отправлено в Telegram сообщение «{text}»", recipient.name)),
                    ..Outcome::default()
                })
            }
            other => anyhow::bail!("calls: unknown action {other}"),
        }
    }

    /// The station's party in our hub for a device: its own, or the only station there is.
    fn station(&self, device_tag: &str) -> Option<String> {
        let own = format!("station:{device_tag}");
        if self.hub.endpoint(&own).is_some() {
            return Some(own);
        }
        let stations: Vec<CallParty> =
            self.hub.list().into_iter().filter(|party| party.kind == PartyKind::Station).collect();
        match stations.as_slice() {
            [only] => Some(only.id.clone()),
            _ => None,
        }
    }

    /// A Telegram contact the station can call by a link into the LiveKit room.
    fn invitable(&self, contact: &str) -> Option<Contact> {
        if self.telegram.enabled() && self.calls.can_invite() {
            self.telegram.contact(contact)
        } else {
            None
        }
    }

    /// Sends the contact the call's link; the call starts when they join. The contact's name, or none.
    async fn invite(&self, contact: &str, station: &str) -> anyhow::Result<Option<String>> {
        let Some(recipient) = self.invitable(contact) else { return Ok(None) };
        let name = capitalized(&recipient.name);
        let Some(access) = self.calls.invite(name.clone()).await else { return Ok(None) };
        self.drop_invite_of(station);
        let button = LinkButton { text: "Ответить".into(), url: access.link };
        self.telegram
            .send_text(recipient.chat_id, format!("Вам звонит {}.", self.calls.station_name()), Some(button))
            .await?;
        self.keep_invite(access.identity, station.to_string(), name.clone());
        Ok(Some(name))
    }

    /// Waits for the identity to join the room; after INVITE the station hears that nobody answers.
    fn keep_invite(&self, identity: String, station: String, name: String) {
        let me = self.me.clone();
        let expiring = identity.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(INVITE).await;
            if let Some(me) = me.upgrade() {
                me.invite_expired(&expiring).await;
            }
        })
        .abort_handle();
        self.invites.lock().unwrap().insert(identity, Invite { station, name, timer: Some(timer) });
    }

    fn has_invite(&self, station: &str) -> bool {
        self.invites.lock().unwrap().values().any(|invite| invite.station == station)
    }

    /// The station's waiting invite, if any, is dropped (its timer with it).
    fn drop_invite_of(&self, station: &str) {
        let mut invites = self.invites.lock().unwrap();
        let identity =
            invites.iter().find(|(_, invite)| invite.station == station).map(|(identity, _)| identity.clone());
        if let Some(identity) = identity {
            invites.remove(&identity);
        }
    }

    /// Someone joined the room: the station rings them if it called them by a link. The hub is asked with no lock
    /// of ours held.
    pub fn on_endpoints(&self) {
        let identities: Vec<String> = self.invites.lock().unwrap().keys().cloned().collect();
        for identity in identities {
            let Some(endpoint) = self.hub.endpoint(&format!("livekit:{identity}")) else { continue };
            let Some(invite) = self.invites.lock().unwrap().remove(&identity) else { continue };
            if let Err(error) = self.hub.call(&invite.station, &endpoint.id) {
                tracing::warn!(target: "calls", "a call to {} by the link: {error}", invite.name);
            }
        }
    }

    async fn invite_expired(&self, identity: &str) {
        let Some(mut invite) = self.invites.lock().unwrap().remove(identity) else { return };
        // The timer is this very task: it runs out on its own.
        invite.timer = None;
        self.say(&invite.station, &format!("{} не отвечает.", invite.name)).await;
    }

    /// A call rings: one to the station is announced.
    pub fn on_ringing(&self, call: CallView) {
        let me = self.me.clone();
        tokio::spawn(async move {
            if let Some(me) = me.upgrade() {
                me.announce(call).await;
            }
        });
    }

    /// A call ended: the station's own call nobody took is said, so the station is not left waiting in silence.
    pub fn on_ended(&self, call: CallView, reason: &str) {
        if call.from.kind != PartyKind::Station || call.answered {
            return;
        }
        if reason == "no answer" {
            let me = self.me.clone();
            let (station, text) = (call.from.id.clone(), format!("{} не отвечает.", call.to.name));
            tokio::spawn(async move {
                if let Some(me) = me.upgrade() {
                    me.say(&station, &text).await;
                }
            });
        }
        // The page left the room while ringing — reloaded, live 02.11: the call waits for it to come back.
        if (reason == "left the room" || reason == "replaced") && call.to.kind == PartyKind::Livekit {
            let identity = call.to.id.strip_prefix("livekit:").unwrap_or(&call.to.id).to_string();
            self.keep_invite(identity, call.from.id, call.to.name);
        }
    }

    async fn say(&self, station: &str, text: &str) {
        let device = station.strip_prefix("station:").unwrap_or(station);
        if let Some(speaker) = self.speaker() {
            speaker.say(if device == "default" { "*" } else { device }.to_string(), text.to_string(), None).await;
        }
    }

    /// "передай маме, что…": the contact the quick rule or the tool named.
    fn recipient(&self, intent: &Intent) -> Option<Contact> {
        js::string_param(&intent.params, "contact")
            .filter(|named| !named.is_empty())
            .and_then(|named| self.telegram.contact(named))
    }

    async fn announce(&self, call: CallView) {
        let Some(speaker) = self.speaker().filter(|_| call.to.kind == PartyKind::Station) else { return };
        let device = call.to.id.strip_prefix("station:").unwrap_or(&call.to.id).to_string();
        let text = format!("Вам звонит {}. Скажите: Алиса, ответь.", call.from.name);
        for _ in 0..ANNOUNCEMENTS {
            let ringing =
                self.hub.call_of(&call.to.id).is_some_and(|now| now.id == call.id && now.state == CallState::Ringing);
            if !ringing {
                break;
            }
            let target = if device == "default" { "*".to_string() } else { device.clone() };
            let said = speaker.say(target, text.clone(), None).await;
            if !said && device != "default" {
                speaker.say("*".into(), text.clone(), None).await;
            }
            tokio::time::sleep(ANNOUNCE_EVERY).await;
        }
    }

    /// A contact's message in Telegram: its text said, its voice played after a word of who sent it.
    pub fn on_message(&self, message: IncomingMessage) {
        let me = self.me.clone();
        tokio::spawn(async move {
            if let Some(me) = me.upgrade() {
                me.deliver(message).await;
            }
        });
    }

    /// Said or played on the stations at this gateway, and handed on to the stations this computer helps. Live 11.11:
    /// with the station's gateway on the station nobody was at the computer's /uni.ws, and the message was lost
    /// without a word — now it goes there, and what reaches no one is said in the log.
    async fn deliver(&self, message: IncomingMessage) {
        let empty = message.file.is_none() && message.text.as_deref().is_none_or(str::is_empty);
        if empty {
            return;
        }
        let passed = self.relay().map_or(0, |relay| relay.pass_on(&message));
        let played = self.play(&message).await;
        if message.discard {
            if let Some(file) = &message.file {
                let _ = tokio::fs::remove_file(file).await;
            }
        }
        if !played && passed == 0 {
            tracing::info!(
                target: "calls",
                "сообщение от «{}» не проиграно: колонки на связи с этим шлюзом нет, и передать его некому",
                message.from
            );
        }
    }

    /// The message on whichever station is at this gateway now: whether one said it.
    async fn play(&self, message: &IncomingMessage) -> bool {
        let Some(speaker) = self.speaker() else { return false };
        if let Some(file) = &message.file {
            match tokio::fs::read(file).await {
                Ok(audio) => {
                    let clip = AudioClip { audio: audio.into(), label: format!("voice message from {}", message.from) };
                    speaker.say("*".into(), format!("Голосовое сообщение: {}.", message.from), Some(clip)).await
                }
                Err(error) => {
                    tracing::warn!(target: "calls", "message from {}: {error}", message.from);
                    false
                }
            }
        } else if let Some(text) = message.text.as_deref().filter(|text| !text.is_empty()) {
            // «Папа пишет: …»: the name as it is, no case to get wrong («от папа»).
            speaker.say("*".into(), format!("{} пишет: {text}", capitalized(&message.from)), None).await
        } else {
            false
        }
    }
}

impl OwnSkill for CallsSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Calls
    }

    fn verified(&self, intent: &Intent) -> bool {
        js::is_true(&intent.params, "ours")
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.run(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::media::sources::match_score;
    use chrono::Utc;
    use serde_json::json;

    /// What a party was told by the hub: incoming, ringing, connected, ended — and by whom.
    #[derive(Debug, Clone, PartialEq)]
    struct Told {
        kind: &'static str,
        peer: String,
        reason: Option<String>,
    }

    struct Call {
        view: CallView,
    }

    /// The hub as src/calls/call-hub.ts behaves, as far as the skill sees it.
    #[derive(Default)]
    struct Hub {
        parties: Mutex<Vec<CallParty>>,
        calls: Mutex<Vec<Call>>,
        told: Mutex<HashMap<String, Vec<Told>>>,
        skill: RwLock<Option<Weak<CallsSkill>>>,
        next: Mutex<u32>,
    }

    impl Hub {
        fn skill(&self) -> Option<Arc<CallsSkill>> {
            self.skill.read().unwrap().as_ref()?.upgrade()
        }

        fn tell(&self, party: &CallParty, kind: &'static str, peer: &CallParty, reason: Option<&str>) {
            self.told.lock().unwrap().entry(party.id.clone()).or_default().push(Told {
                kind,
                peer: peer.name.clone(),
                reason: reason.map(str::to_string),
            });
        }

        fn last(&self, id: &str) -> Option<Told> {
            self.told.lock().unwrap().get(id)?.last().cloned()
        }

        fn register(&self, party: CallParty) {
            let previous = self.parties.lock().unwrap().iter().any(|p| p.id == party.id);
            if previous {
                self.unregister(&party.id, "replaced");
            }
            self.parties.lock().unwrap().push(party);
            if let Some(skill) = self.skill() {
                skill.on_endpoints();
            }
        }

        fn unregister(&self, id: &str, reason: &str) {
            let ended: Vec<CallView> = {
                let mut calls = self.calls.lock().unwrap();
                calls
                    .iter_mut()
                    .filter(|call| {
                        call.view.state != CallState::Ended && (call.view.from.id == id || call.view.to.id == id)
                    })
                    .map(|call| {
                        call.view.state = CallState::Ended;
                        call.view.clone()
                    })
                    .collect()
            };
            for call in ended {
                self.ended(&call, reason);
            }
            self.parties.lock().unwrap().retain(|party| party.id != id);
            if let Some(skill) = self.skill() {
                skill.on_endpoints();
            }
        }

        fn ended(&self, call: &CallView, reason: &str) {
            self.tell(&call.from, "ended", &call.to, Some(reason));
            self.tell(&call.to, "ended", &call.from, Some(reason));
            if let Some(skill) = self.skill() {
                skill.on_ended(call.clone(), reason);
            }
        }
    }

    impl HubAccess for Hub {
        fn endpoint(&self, id: &str) -> Option<CallParty> {
            self.parties.lock().unwrap().iter().find(|party| party.id == id).cloned()
        }

        fn list(&self) -> Vec<CallParty> {
            self.parties.lock().unwrap().clone()
        }

        fn find(&self, name_or_id: &str, except: Option<&str>) -> Option<CallParty> {
            if let Some(exact) = self.endpoint(name_or_id) {
                return Some(exact);
            }
            let parties = self.parties.lock().unwrap();
            let mut scored: Vec<(f64, &CallParty)> = parties
                .iter()
                .filter(|party| Some(party.id.as_str()) != except)
                .map(|party| (match_score(name_or_id, &party.name), party))
                .filter(|(score, _)| *score >= 0.5)
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            scored.first().map(|(_, party)| (*party).clone())
        }

        fn call_of(&self, endpoint_id: &str) -> Option<CallView> {
            let calls = self.calls.lock().unwrap();
            calls
                .iter()
                .find(|call| {
                    call.view.state != CallState::Ended
                        && (call.view.from.id == endpoint_id || call.view.to.id == endpoint_id)
                })
                .map(|call| call.view.clone())
        }

        fn call(&self, from_id: &str, to_id: &str) -> anyhow::Result<CallView> {
            let (Some(from), Some(to)) = (self.endpoint(from_id), self.endpoint(to_id)) else {
                anyhow::bail!("no such party")
            };
            if self.call_of(from_id).is_some() || self.call_of(to_id).is_some() {
                anyhow::bail!("busy");
            }
            let id = {
                let mut next = self.next.lock().unwrap();
                *next += 1;
                format!("c{next}")
            };
            let view = CallView { id, from: from.clone(), to: to.clone(), state: CallState::Ringing, answered: false };
            self.calls.lock().unwrap().push(Call { view: view.clone() });
            self.tell(&to, "incoming", &from, None);
            self.tell(&from, "ringing", &to, None);
            if let Some(skill) = self.skill() {
                skill.on_ringing(view.clone());
            }
            Ok(view)
        }

        fn accept(&self, call_id: &str, by_id: &str) -> anyhow::Result<()> {
            let view = {
                let mut calls = self.calls.lock().unwrap();
                let Some(call) =
                    calls.iter_mut().find(|call| call.view.id == call_id && call.view.state == CallState::Ringing)
                else {
                    anyhow::bail!("no such call ringing")
                };
                anyhow::ensure!(call.view.to.id == by_id, "only the callee answers");
                call.view.state = CallState::Active;
                call.view.answered = true;
                call.view.clone()
            };
            self.tell(&view.from, "connected", &view.to, None);
            self.tell(&view.to, "connected", &view.from, None);
            Ok(())
        }

        fn hangup(&self, call_id: &str, _by_id: &str) {
            let ended = {
                let mut calls = self.calls.lock().unwrap();
                calls.iter_mut().find(|call| call.view.id == call_id && call.view.state != CallState::Ended).map(
                    |call| {
                        call.view.state = CallState::Ended;
                        call.view.clone()
                    },
                )
            };
            if let Some(call) = ended {
                self.ended(&call, "hung up");
            }
        }
    }

    struct Service {
        can_invite: bool,
    }

    impl CallsAccess for Service {
        fn station_name(&self) -> String {
            "Колонка".into()
        }
        fn can_invite(&self) -> bool {
            self.can_invite
        }
        fn invite(&self, name: String) -> BoxFuture<'static, Option<Invitation>> {
            Box::pin(async move {
                Some(Invitation {
                    identity: "app-mom".into(),
                    link: format!("https://calls.example/calls/livekit#access={name}"),
                })
            })
        }
        fn sip_contact(&self, _name: &str) -> Option<String> {
            None
        }
        fn dial_sip(&self, _number: String, _name: String) -> BoxFuture<'static, anyhow::Result<CallParty>> {
            Box::pin(async { anyhow::bail!("no SIP bridge (ARI_URL)") })
        }
    }

    #[derive(Default)]
    struct Telegram {
        sent: Mutex<Vec<(String, String, Option<LinkButton>)>>,
    }

    impl MessengerAccess for Telegram {
        fn enabled(&self) -> bool {
            true
        }
        fn contact(&self, name: &str) -> Option<Contact> {
            (match_score(name, "мама") >= 0.5).then(|| Contact { name: "мама".into(), chat_id: "42".into() })
        }
        fn send_text(
            &self,
            chat_id: String,
            text: String,
            button: Option<LinkButton>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            self.sent.lock().unwrap().push((chat_id, text, button));
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct Said(Mutex<Vec<String>>);

    impl Speaker for Arc<Said> {
        fn say(&self, _device: String, text: String, _clip: Option<AudioClip>) -> BoxFuture<'static, bool> {
            self.0.lock().unwrap().push(text);
            Box::pin(async { true })
        }
    }

    struct SetUp {
        hub: Arc<Hub>,
        telegram: Arc<Telegram>,
        skill: Arc<CallsSkill>,
        said: Arc<Said>,
    }

    fn party(id: &str, name: &str, kind: PartyKind) -> CallParty {
        CallParty { id: id.into(), name: name.into(), kind }
    }

    fn set_up(can_invite: bool) -> SetUp {
        let hub = Arc::new(Hub::default());
        hub.register(party("station:default", "Колонка", PartyKind::Station));
        let telegram = Arc::new(Telegram::default());
        let skill = CallsSkill::new(hub.clone(), Arc::new(Service { can_invite }), telegram.clone());
        *hub.skill.write().unwrap() = Some(Arc::downgrade(&skill));
        let said = Arc::new(Said::default());
        skill.attach_speaker(Arc::new(said.clone()));
        SetUp { hub, telegram, skill, said }
    }

    fn call(contact: &str) -> Intent {
        Intent {
            capability: CapabilityName::Calls,
            action: "call".into(),
            params: json!({"contact": contact}).as_object().cloned().unwrap(),
        }
    }

    fn context() -> ExecutionContext {
        ExecutionContext::new("default", Utc::now())
    }

    #[tokio::test]
    async fn sends_the_contact_a_link_into_the_room_and_rings_them_when_they_join() {
        let SetUp { hub, telegram, skill, .. } = set_up(true);
        assert!(skill.ours(&call("маме"), "default"));

        let outcome = skill.execute(&call("маме"), &context()).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Звоню: Мама. Ссылка на звонок ушла в Telegram."));
        assert!(outcome.done.unwrap().contains("Мама"));
        assert_eq!(
            *telegram.sent.lock().unwrap(),
            [(
                "42".to_string(),
                "Вам звонит Колонка.".to_string(),
                Some(LinkButton {
                    text: "Ответить".into(),
                    url: "https://calls.example/calls/livekit#access=Мама".into()
                })
            )]
        );

        // Mom opens the link: the station's call rings on her page, and she answers.
        hub.register(party("livekit:app-mom", "Мама", PartyKind::Livekit));
        assert_eq!(hub.last("livekit:app-mom"), Some(Told { kind: "incoming", peer: "Колонка".into(), reason: None }));
        let ringing = hub.call_of("livekit:app-mom").unwrap();
        hub.accept(&ringing.id, "livekit:app-mom").unwrap();
        assert_eq!(hub.last("station:default"), Some(Told { kind: "connected", peer: "Мама".into(), reason: None }));
    }

    #[tokio::test(start_paused = true)]
    async fn says_the_contact_does_not_answer_when_the_link_is_not_opened_and_polozhi_trubku_cancels_the_call() {
        let SetUp { hub, skill, said, .. } = set_up(true);
        skill.execute(&call("маме"), &context()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(90_001)).await;
        assert_eq!(*said.0.lock().unwrap(), ["Мама не отвечает."]);

        skill.execute(&call("маме"), &context()).await.unwrap();
        let hangup = Intent::new(CapabilityName::Calls, "hangup");
        assert!(skill.ours(&hangup, "default"));
        skill.execute(&hangup, &context()).await.unwrap();
        assert!(!skill.ours(&hangup, "default"));
        hub.register(party("livekit:app-mom", "Мама", PartyKind::Livekit));
        assert!(hub.call_of("livekit:app-mom").is_none());
    }

    #[tokio::test]
    async fn rings_the_page_again_when_it_comes_back_to_the_room_while_the_call_rang_a_reload_live_02_11() {
        let SetUp { hub, skill, .. } = set_up(true);
        hub.register(party("livekit:app-mom", "Мама", PartyKind::Livekit));
        skill.execute(&call("маме"), &context()).await.unwrap();
        assert_eq!(hub.last("livekit:app-mom").map(|told| told.kind), Some("incoming"));

        hub.unregister("livekit:app-mom", "left the room");
        assert_eq!(
            hub.last("station:default"),
            Some(Told { kind: "ended", peer: "Мама".into(), reason: Some("left the room".into()) })
        );
        hub.told.lock().unwrap().remove("livekit:app-mom");
        hub.register(party("livekit:app-mom", "Мама", PartyKind::Livekit));
        assert_eq!(hub.last("livekit:app-mom"), Some(Told { kind: "incoming", peer: "Колонка".into(), reason: None }));
        let back = hub.call_of("livekit:app-mom").unwrap();
        hub.hangup(&back.id, "livekit:app-mom");
    }

    #[tokio::test]
    async fn is_yandexs_without_the_livekit_page_from_outside() {
        let SetUp { skill, .. } = set_up(false);
        assert!(!skill.ours(&call("маме"), "default"));
    }

    #[tokio::test]
    async fn passes_a_message_and_says_a_contacts_text() {
        let SetUp { telegram, skill, said, .. } = set_up(true);
        let message = Intent {
            capability: CapabilityName::Calls,
            action: "message".into(),
            params: json!({"contact": "маме", "text": "Я задержусь"}).as_object().cloned().unwrap(),
        };
        assert!(skill.ours(&message, "default"));
        let outcome = skill.execute(&message, &context()).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Передала."));
        assert_eq!(outcome.done.as_deref(), Some("контакту «мама» отправлено в Telegram сообщение «Я задержусь»"));
        assert_eq!(telegram.sent.lock().unwrap()[0].1, "Я задержусь");
        skill
            .deliver(IncomingMessage {
                from: "папа".into(),
                chat_id: "7".into(),
                file: None,
                text: Some("Ужин готов".into()),
                discard: false,
            })
            .await;
        assert_eq!(*said.0.lock().unwrap(), ["Папа пишет: Ужин готов"]);
    }

    /// The stations this computer helps.
    #[derive(Default)]
    struct Stations(Mutex<Vec<IncomingMessage>>);

    impl MessageRelay for Arc<Stations> {
        fn pass_on(&self, message: &IncomingMessage) -> usize {
            self.0.lock().unwrap().push(message.clone());
            1
        }
    }

    /// Live 11.11: the station's gateway on the station, nobody at the computer's /uni.ws — the message went nowhere.
    /// It goes to the stations the computer helps, and is said here too where a station is.
    #[tokio::test]
    async fn hands_a_contacts_message_on_to_the_stations_it_helps_and_a_copy_for_playing_goes_after_it() {
        let SetUp { skill, said, .. } = set_up(true);
        let stations = Arc::new(Stations::default());
        skill.attach_relay(Arc::new(stations.clone()));
        let text = IncomingMessage {
            from: "мама".into(),
            chat_id: "42".into(),
            file: None,
            text: Some("Скоро буду".into()),
            discard: false,
        };
        skill.deliver(text.clone()).await;
        assert_eq!(*stations.0.lock().unwrap(), [text]);
        assert_eq!(*said.0.lock().unwrap(), ["Мама пишет: Скоро буду"]);

        // A voice message the computer passed on: played, then its copy removed.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("voice.ogg");
        std::fs::write(&file, b"OggS").unwrap();
        let voice = IncomingMessage {
            from: "мама".into(),
            chat_id: String::new(),
            file: Some(file.clone()),
            text: None,
            discard: true,
        };
        skill.deliver(voice).await;
        assert_eq!(said.0.lock().unwrap().last().map(String::as_str), Some("Голосовое сообщение: мама."));
        assert!(!file.exists(), "the copy is gone");

        // No station here and none helped: the log says so (no panic, no speaker needed).
        let alone = CallsSkill::new(Arc::new(NoCalls), Arc::new(NoCalls), Arc::new(NoCalls));
        alone.deliver(IncomingMessage { text: Some("Привет".into()), ..text_message() }).await;
    }

    fn text_message() -> IncomingMessage {
        IncomingMessage { from: "папа".into(), chat_id: "7".into(), file: None, text: None, discard: false }
    }

    #[tokio::test(start_paused = true)]
    async fn announces_a_call_to_the_station_while_it_rings() {
        // The hub holds the skill weakly, as the calls module will: the test keeps it.
        let SetUp { hub, said, skill: _skill, .. } = set_up(true);
        hub.register(party("browser:1", "Мама", PartyKind::Browser));
        let ringing = hub.call("browser:1", "station:default").unwrap();
        tokio::time::sleep(Duration::from_millis(12_001)).await;
        assert_eq!(said.0.lock().unwrap().len(), 2);
        hub.hangup(&ringing.id, "browser:1");
        tokio::time::sleep(Duration::from_millis(30_000)).await;
        assert_eq!(
            *said.0.lock().unwrap(),
            ["Вам звонит Мама. Скажите: Алиса, ответь.", "Вам звонит Мама. Скажите: Алиса, ответь."]
        );
    }
}
