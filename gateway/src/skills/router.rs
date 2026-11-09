//! How a request finds its executor: a dialog that goes on, then quick rules, then the model with tools. An intent
//! goes to the first executor of its route that can take it; our implementation of a command known only from the
//! wiki gives way to Yandex when Yandex is on the route and up.
//!
//! The `.spec.ts` named here is the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;

use super::capabilities::{is_capability, CapabilityName, Intent};
use super::js;
use super::quick_commands::{
    choose_profile, looks_like_command, match_heard, match_play_anything, match_quick_command, match_while_busy,
    match_while_playing, QuickMatch,
};
use super::routes::{RouteStep, RouteTable};
use super::skill::OwnSkill;
use crate::config::ModelProfile;
use crate::core::identity::permissions::use_of;
use crate::core::identity::{can, Principal};
use crate::gateway::protocol::protocol_types::{DeviceState, PlayerState};

/// Follow-ups without the wake word go to the executor that asked for them, until it lets go or time runs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Continuation {
    pub capability: CapabilityName,
    pub executor: RouteStep,
    /// Milliseconds since the epoch.
    pub until: u64,
}

/// What the model can do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelState {
    pub enabled: bool,
    pub tools: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RouteInput {
    pub text: String,
    pub device_tag: String,
    pub device_state: Option<DeviceState>,
    pub continuation: Option<Continuation>,
    /// Our session with Yandex is up and its route may serve requests.
    pub yandex_available: bool,
    /// A timer or an alarm of the station rings now.
    pub ringing: bool,
    /// Said with the wake word (the station's spotter), not a follow-up the station listened for.
    pub wake_word: bool,
    /// Who spoke: a capability they may not use is not served, whoever could serve it.
    pub principal: Option<Principal>,
    pub model: ModelState,
    /// Milliseconds since the epoch.
    pub now: u64,
}

/// The reason a request is not served because the admin did not allow it (the station says so, not «not yet»).
pub const RESTRICTED: &str = "restricted by the admin";

/// Yandex's dialogs that go on with the wake word too: a skill or a game is a session until it lets go.
const YANDEX_SESSIONS: &[CapabilityName] = &[CapabilityName::Skills, CapabilityName::Games];

/// The station's own controls: their words stay with the built-in skills, whatever a plugin declares — «стоп»,
/// «громче», «дальше» work the same with any plugin on.
pub const STATION_CONTROLS: &[CapabilityName] = &[
    CapabilityName::Volume,
    CapabilityName::Playback,
    CapabilityName::Bluetooth,
    CapabilityName::Timers,
    CapabilityName::Alarms,
    CapabilityName::DoNotDisturb,
    CapabilityName::Calls,
];

/// An intent's parameter naming the capability whose implementation serves it, when that is not its own: a
/// plugin's intent for the radio is the radio's (its route, the right to it), served by the plugins' bridge.
pub const EXECUTOR: &str = "executor";
/// A plugin's intent that took over the words of a built-in rule carries that rule's intent: when the plugin lets
/// them go (it has nothing, or it failed), the built-in implementation serves them.
pub const FALLBACK: &str = "fallback";

/// Quick rules that come and go while the gateway runs: the plugins' phrases.
pub trait ExtraRules: Send + Sync {
    /// The rule that takes the whole of this command (normalized), if one does.
    fn matched(&self, normalized: &str) -> Option<QuickMatch>;
}

/// Where a capability's request goes, after the route table and what is available now.
#[derive(Clone)]
pub enum Resolution {
    Own(Arc<dyn OwnSkill>),
    Yandex,
    Unsupported { reason: String },
}

impl Resolution {
    /// `own`, `yandex` or `unsupported`, as the journal and the statistics name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Resolution::Own(_) => "own",
            Resolution::Yandex => "yandex",
            Resolution::Unsupported { .. } => "unsupported",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Resolution::Unsupported { reason } => Some(reason),
            _ => None,
        }
    }

    fn unsupported(reason: &str) -> Resolution {
        Resolution::Unsupported { reason: reason.to_string() }
    }
}

impl std::fmt::Debug for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Resolution::Own(skill) => write!(f, "Own({})", skill.capability()),
            Resolution::Yandex => write!(f, "Yandex"),
            Resolution::Unsupported { reason } => write!(f, "Unsupported({reason})"),
        }
    }
}

impl PartialEq for Resolution {
    fn eq(&self, other: &Resolution) -> bool {
        match (self, other) {
            (Resolution::Own(a), Resolution::Own(b)) => Arc::ptr_eq(a, b),
            (Resolution::Yandex, Resolution::Yandex) => true,
            (Resolution::Unsupported { reason: a }, Resolution::Unsupported { reason: b }) => a == b,
            _ => false,
        }
    }
}

/// Where a known intent came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentSource {
    Quick,
    Continuation,
}

impl IntentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            IntentSource::Quick => "quick",
            IntentSource::Continuation => "continuation",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// A known intent (a quick rule, a game's follow-up) and who serves it.
    Intent {
        intent: Intent,
        source: IntentSource,
        rule: Option<&'static str>,
        resolution: Resolution,
    },
    /// The request goes on in a Yandex dialog (a skill, a clarifying question) as it is.
    YandexDialog {
        capability: CapabilityName,
    },
    /// The model answers: with every tool, its choice first (command), or as a conversation with lookups only.
    Model {
        command: bool,
        profile: ModelProfile,
    },
    Unsupported {
        capability: CapabilityName,
        reason: String,
    },
}

/// Rewrites an intent by what the station is doing: "стоп" while a timer rings stops the timer.
pub type Interceptor = Box<dyn Fn(&Intent, &RouteInput) -> Option<Intent> + Send + Sync>;

pub struct Router {
    interceptors: RwLock<Vec<Interceptor>>,
    table: RwLock<Arc<RouteTable>>,
    skills: RwLock<BTreeMap<CapabilityName, Arc<dyn OwnSkill>>>,
    extra: RwLock<Vec<Arc<dyn ExtraRules>>>,
}

impl Router {
    pub fn new(routes: RouteTable, skills: Vec<Arc<dyn OwnSkill>>) -> Router {
        let router = Router {
            interceptors: RwLock::new(Vec::new()),
            table: RwLock::new(Arc::new(routes)),
            skills: RwLock::new(BTreeMap::new()),
            extra: RwLock::new(Vec::new()),
        };
        for skill in skills {
            router.register(skill);
        }
        router
    }

    /// The routes the gateway uses now.
    pub fn routes(&self) -> Arc<RouteTable> {
        self.table.read().unwrap().clone()
    }

    /// New routes from now on, without a restart; requests in flight finish on the old table.
    pub fn replace_routes(&self, routes: RouteTable) {
        *self.table.write().unwrap() = Arc::new(routes);
    }

    pub fn intercept(&self, interceptor: Interceptor) {
        self.interceptors.write().unwrap().push(interceptor);
    }

    /// Rules from outside the built-in table (the plugins'), asked on every request from now on.
    pub fn add_rules(&self, rules: Arc<dyn ExtraRules>) {
        self.extra.write().unwrap().push(rules);
    }

    /// Our implementation of its capability, in place of any before it.
    pub fn register(&self, skill: Arc<dyn OwnSkill>) {
        self.skills.write().unwrap().insert(skill.capability(), skill);
    }

    pub fn skill(&self, capability: CapabilityName) -> Option<Arc<dyn OwnSkill>> {
        self.skills.read().unwrap().get(&capability).cloned()
    }

    /// Our implementation of an intent: the capability's its executor names (a plugin's intent: the bridge), else
    /// its own capability's.
    pub fn implementation(&self, intent: &Intent) -> Option<Arc<dyn OwnSkill>> {
        let executor = intent.params.get(EXECUTOR).and_then(Value::as_str).and_then(is_capability);
        self.skill(executor.unwrap_or(intent.capability))
    }

    /// Every implementation of ours.
    pub fn skills(&self) -> Vec<Arc<dyn OwnSkill>> {
        self.skills.read().unwrap().values().cloned().collect()
    }

    pub fn decide(&self, input: &RouteInput) -> Decision {
        if let Some(continuation) = input.continuation.filter(|continuation| continuation.until > input.now) {
            // Yandex's question takes the answer, not a new request with the wake word (live 02.11: after «Что
            // сказать папе?» «Алиса, передай папе…» went to Yandex for two minutes); its skills and games keep
            // their session, as on Alice itself.
            let keeps = !input.wake_word || YANDEX_SESSIONS.contains(&continuation.capability);
            let allowed = input.principal.as_ref().is_none_or(|p| can(Some(p), &use_of(continuation.capability)));
            if continuation.executor == RouteStep::Yandex && input.yandex_available && keeps && allowed {
                return Decision::YandexDialog { capability: continuation.capability };
            }
            if continuation.executor == RouteStep::Own {
                let mut intent = Intent::new(continuation.capability, "continue");
                intent.params.insert("text".into(), input.text.clone().into());
                let resolution = self.resolve(&intent, input.yandex_available, input.principal.as_ref());
                return Decision::Intent { intent, source: IntentSource::Continuation, rule: None, resolution };
            }
        }
        let playing = input.device_state.as_ref().and_then(|state| state.player.as_ref()).map(|player| player.state)
            == Some(PlayerState::Playing);
        let built_in = match_quick_command(&input.text)
            .or_else(|| playing.then(|| match_while_playing(&input.text)).flatten())
            .or_else(|| (playing || input.ringing).then(|| match_while_busy(&input.text)).flatten());
        let quick = self
            .with_extra_rules(built_in, &input.text)
            .or_else(|| self.yandex_plays_first(input).then(|| match_play_anything(&input.text)).flatten());
        if let Some(quick) = quick {
            let intent = self.rewrite(&quick.intent, input);
            let resolution = self.resolve(&intent, input.yandex_available, input.principal.as_ref());
            return Decision::Intent { intent, source: IntentSource::Quick, rule: Some(quick.rule), resolution };
        }
        if let Some(principal) = &input.principal {
            if !can(Some(principal), &use_of(CapabilityName::Chat)) {
                return Decision::Unsupported { capability: CapabilityName::Chat, reason: RESTRICTED.into() };
            }
        }
        if input.model.enabled {
            let command = input.model.tools && looks_like_command(&input.text);
            return Decision::Model { command, profile: choose_profile(&input.text, command) };
        }
        Decision::Unsupported {
            capability: CapabilityName::Chat,
            reason: "no dialog model (DIALOG_PROVIDER=none)".into(),
        }
    }

    /// The plugins' phrases beside the built-in rules. A plugin's phrase takes words no built-in rule takes, and the
    /// words of a built-in rule of the very capability it serves — that rule's intent goes along as its fallback. The
    /// other built-in rules come first, the station's controls always.
    fn with_extra_rules(&self, built_in: Option<QuickMatch>, text: &str) -> Option<QuickMatch> {
        let extra: Vec<Arc<dyn ExtraRules>> = self.extra.read().unwrap().clone();
        if extra.is_empty() || built_in.as_ref().is_some_and(|rule| STATION_CONTROLS.contains(&rule.intent.capability))
        {
            return built_in;
        }
        let Some(mut found) = match_heard(text, |normalized| extra.iter().find_map(|rules| rules.matched(normalized)))
        else {
            return built_in;
        };
        match built_in {
            None => Some(found),
            Some(rule) if rule.intent.capability == found.intent.capability => {
                if let Ok(fallback) = serde_json::to_value(&rule.intent) {
                    found.intent.params.insert(FALLBACK.into(), fallback);
                }
                Some(found)
            }
            Some(rule) => Some(rule),
        }
    }

    /// Yandex serves music first and is up: «включи <что угодно>» is its to sort out.
    fn yandex_plays_first(&self, input: &RouteInput) -> bool {
        self.routes().route(CapabilityName::Music).first() == Some(&RouteStep::Yandex) && input.yandex_available
    }

    /// An intent from a quick rule or a model's tool call, after the interceptors.
    pub fn rewrite(&self, intent: &Intent, input: &RouteInput) -> Intent {
        let interceptors = self.interceptors.read().unwrap();
        interceptors.iter().find_map(|interceptor| interceptor(intent, input)).unwrap_or_else(|| intent.clone())
    }

    pub fn resolve(&self, intent: &Intent, yandex_available: bool, principal: Option<&Principal>) -> Resolution {
        if let Some(principal) = principal {
            if !can(Some(principal), &use_of(intent.capability)) {
                return Resolution::unsupported(RESTRICTED);
            }
        }
        let routes = self.routes();
        let route = routes.route(intent.capability);
        if route.is_empty() {
            return Resolution::unsupported("route off");
        }
        // Yandex's own timer, Yandex's radio: only Yandex knows them, whatever the route says of our implementation.
        if js::is_true(&intent.params, "external") && yandex_available {
            return Resolution::Yandex;
        }
        let skill = self.implementation(intent);
        let usable = skill.clone().filter(|skill| skill.available());
        // And our own track only we know: its next, its like are ours, whatever the route says of Yandex.
        if let (true, Some(usable)) = (js::is_true(&intent.params, "ours"), &usable) {
            return Resolution::Own(usable.clone());
        }
        let yandex_up = route.contains(&RouteStep::Yandex) && yandex_available;
        for step in route {
            match step {
                RouteStep::Own => {
                    if let Some(usable) = &usable {
                        if usable.verified(intent) || !yandex_up {
                            return Resolution::Own(usable.clone());
                        }
                    }
                }
                RouteStep::Yandex => {
                    if yandex_available {
                        return Resolution::Yandex;
                    }
                }
            }
        }
        // Without Yandex — «Свой», where no step goes to it, or our session with it down — a route that names only
        // Yandex is ours when we have the capability: our way of doing it beats «не умею». Live 06.11 in «Свой»:
        // «стоп» and «включи не беспокоить» (both routed to Yandex) answered «Этого я пока не умею».
        if !yandex_available {
            if let Some(usable) = &usable {
                return Resolution::Own(usable.clone());
            }
        }
        if route.contains(&RouteStep::Own) && skill.is_none() {
            return Resolution::unsupported("no implementation of ours yet");
        }
        if route.contains(&RouteStep::Own) && usable.is_none() {
            return Resolution::unsupported("our implementation is not set up");
        }
        Resolution::unsupported("Yandex is not reachable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::{Restrictions, Role};
    use crate::gateway::protocol::protocol_types::PlayerView;
    use crate::skills::own::bluetooth_skill::BluetoothSkill;
    use crate::skills::own::do_not_disturb_skill::DoNotDisturbSkill;
    use crate::skills::own::playback_skill::PlaybackSkill;
    use crate::skills::own::volume_skill::VolumeSkill;
    use crate::skills::skill::{ExecutionContext, Outcome};
    use crate::utils::time::now_ms;
    use futures::future::BoxFuture;
    use serde_json::{json, Value};

    fn router_with(default_route: &str, routes: &str) -> Router {
        Router::new(
            RouteTable::parse(default_route, routes),
            vec![Arc::new(VolumeSkill), Arc::new(BluetoothSkill), Arc::new(DoNotDisturbSkill)],
        )
    }

    fn router() -> Router {
        router_with("own,yandex", "")
    }

    fn input(text: &str) -> RouteInput {
        RouteInput {
            text: text.into(),
            device_tag: "h:test".into(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        }
    }

    fn with_yandex(text: &str) -> RouteInput {
        RouteInput { yandex_available: true, ..input(text) }
    }

    fn intent_of(capability: CapabilityName, action: &str, params: Value) -> Intent {
        Intent { capability, action: action.into(), params: params.as_object().cloned().unwrap() }
    }

    fn playing() -> Option<DeviceState> {
        Some(DeviceState {
            player: Some(PlayerView {
                state: PlayerState::Playing,
                id: None,
                title: None,
                r#type: None,
                offset_ms: None,
            }),
            ..DeviceState::default()
        })
    }

    /// The rule and the intent of a quick decision.
    fn quick(decision: &Decision) -> Option<(&'static str, &Intent, &Resolution)> {
        match decision {
            Decision::Intent { source: IntentSource::Quick, rule: Some(rule), intent, resolution } => {
                Some((rule, intent, resolution))
            }
            _ => None,
        }
    }

    fn is_model(decision: &Decision) -> bool {
        matches!(decision, Decision::Model { .. })
    }

    #[test]
    fn sends_a_quick_command_to_our_implementation() {
        let decision = router().decide(&input("громче"));
        let (rule, intent, resolution) = quick(&decision).unwrap();
        assert_eq!(rule, "louder");
        assert_eq!((intent.capability, intent.action.as_str()), (CapabilityName::Volume, "louder"));
        assert_eq!(resolution.kind(), "own");
    }

    /// Live 06.11 in «Свой»: «включи не беспокоить», routed to Yandex only, answered «Этого я пока не умею».
    #[test]
    fn serves_a_capability_routed_to_yandex_with_ours_when_yandex_is_not_there() {
        let router = router_with("own,yandex", "do_not_disturb=yandex");
        let quiet = Intent::new(CapabilityName::DoNotDisturb, "on");
        assert_eq!(router.resolve(&quiet, true, None), Resolution::Yandex);
        assert_eq!(router.resolve(&quiet, false, None).kind(), "own");
        // Off stays off, and what we do not have stays «not yet».
        let off = router_with("own,yandex", "do_not_disturb=off");
        assert_eq!(off.resolve(&quiet, false, None), Resolution::Unsupported { reason: "route off".into() });
        let timer = Intent::new(CapabilityName::Timers, "set");
        assert_eq!(router.resolve(&timer, false, None).kind(), "unsupported");
    }

    #[test]
    fn says_not_yet_for_a_capability_whose_route_is_off() {
        let decision = router_with("own,yandex", "timers=off").decide(&input("поставь таймер на минуту"));
        assert_eq!(quick(&decision).unwrap().2, &Resolution::Unsupported { reason: "route off".into() });
    }

    #[test]
    fn gives_a_command_known_only_from_the_wiki_to_yandex_when_yandex_is_up_and_tries_ours_when_it_is_not() {
        let quiet = Intent::new(CapabilityName::DoNotDisturb, "on");
        assert_eq!(router().resolve(&quiet, true, None), Resolution::Yandex);
        assert_eq!(router().resolve(&quiet, false, None).kind(), "own");
        assert_eq!(router_with("own", "").resolve(&quiet, true, None).kind(), "own");
        // Confirmed live, ours goes first.
        assert_eq!(router().resolve(&Intent::new(CapabilityName::Bluetooth, "on"), true, None).kind(), "own");
    }

    #[test]
    fn gives_vklyuchi_anything_to_yandex_as_music_while_yandex_serves_music_first() {
        let yandex_music = router_with("own,yandex", "music=yandex");
        for text in ["Включи Miyagi.", "включи кино", "поставь что-нибудь спокойное", "хочу послушать Земфиру"]
        {
            let decision = yandex_music.decide(&with_yandex(text));
            let (rule, intent, resolution) = quick(&decision).unwrap_or_else(|| panic!("{text}: {decision:?}"));
            assert_eq!(rule, "playAnything");
            assert_eq!((intent.capability, intent.action.as_str()), (CapabilityName::Music, "play"));
            assert_eq!(resolution, &Resolution::Yandex);
        }
        // The home and other skills stay the model's.
        for text in ["включи свет на кухне", "включи сценарий кино", "включи вентилятор", "включи пылесос"]
        {
            assert!(is_model(&yandex_music.decide(&with_yandex(text))), "{text}");
        }
        // Our music first, or Yandex down: the model, as before.
        assert!(is_model(&router().decide(&with_yandex("включи Miyagi"))));
        assert!(is_model(&yandex_music.decide(&input("включи Miyagi"))));
        // Other rules come first.
        assert_eq!(quick(&yandex_music.decide(&with_yandex("включи радио джаз"))).unwrap().0, "radio");
    }

    #[test]
    fn has_nothing_for_a_capability_without_our_implementation_when_yandex_is_down() {
        let radio = Intent::new(CapabilityName::Radio, "play");
        assert_eq!(router().resolve(&radio, false, None), Resolution::unsupported("no implementation of ours yet"));
        assert_eq!(router().resolve(&radio, true, None), Resolution::Yandex);
    }

    #[test]
    fn lets_the_model_take_what_quick_rules_do_not_know_commands_first_by_its_choice() {
        assert_eq!(
            router().decide(&input("включи свет на кухне")),
            Decision::Model { command: true, profile: ModelProfile::Smart }
        );
        assert_eq!(
            router().decide(&input("как дела")),
            Decision::Model { command: false, profile: ModelProfile::Fast }
        );
        let no_tools = RouteInput { model: ModelState { enabled: true, tools: false }, ..input("как дела") };
        assert!(matches!(router().decide(&no_tools), Decision::Model { command: false, .. }));
        let no_model = RouteInput { model: ModelState { enabled: false, tools: false }, ..input("как дела") };
        assert!(matches!(router().decide(&no_model), Decision::Unsupported { .. }));
    }

    #[test]
    fn takes_chto_dalshe_for_the_next_track_only_while_the_player_plays() {
        let decision = router().decide(&RouteInput { device_state: playing(), ..input("Что дальше?") });
        let (rule, intent, _) = quick(&decision).unwrap();
        assert_eq!(rule, "nextWhilePlaying");
        assert_eq!(intent, &intent_of(CapabilityName::Playback, "next", json!({"yandexText": "Дальше."})));
        assert!(is_model(&router().decide(&input("Что дальше?"))));
        // «Алиса, стоп» under loud music or a ringing timer (live, 31.10 and 02.11).
        for heard in ["Со стол?", "Со стола.", "Олесоп.", "Состо.", "Ай. Stop."] {
            for busy in
                [RouteInput { device_state: playing(), ..input(heard) }, RouteInput { ringing: true, ..input(heard) }]
            {
                let decision = router().decide(&busy);
                let (rule, intent, _) = quick(&decision).unwrap_or_else(|| panic!("{heard}: {decision:?}"));
                assert_eq!(rule, if heard == "Ай. Stop." { "stop" } else { "stopWhileBusy" }, "{heard}");
                assert_eq!((intent.capability, intent.action.as_str()), (CapabilityName::Playback, "stop"));
            }
        }
        assert!(is_model(&router().decide(&input("Со стола."))));
        assert!(is_model(&router().decide(&input("Олесоп."))));
    }

    #[test]
    fn keeps_the_like_of_our_own_track_ours_whatever_the_route_says_of_yandex() {
        let r = Router::new(RouteTable::parse("own,yandex", "playback=yandex"), vec![Arc::new(PlaybackSkill::new())]);
        let like = |params: Value| intent_of(CapabilityName::Playback, "like", params);
        assert_eq!(r.resolve(&like(json!({"ours": true})), true, None).kind(), "own");
        // Our queue's next track too (live 03.11 it went to Yandex, which played a song of its own).
        assert_eq!(
            r.resolve(&intent_of(CapabilityName::Playback, "next", json!({"ours": true})), true, None).kind(),
            "own"
        );
        assert_eq!(r.resolve(&like(json!({"external": true})), true, None), Resolution::Yandex);
        // Neither ours nor known to be Yandex's: the route decides.
        assert_eq!(r.resolve(&like(json!({})), true, None), Resolution::Yandex);
        let decision = r.decide(&RouteInput { device_state: playing(), ..input("Мне нравится.") });
        let (rule, intent, _) = quick(&decision).unwrap();
        assert_eq!((rule, intent.action.as_str()), ("like", "like"));
    }

    #[test]
    fn keeps_a_dialog_with_the_executor_that_asked_for_the_follow_up() {
        let now = now_ms();
        let continuation =
            Continuation { capability: CapabilityName::Skills, executor: RouteStep::Yandex, until: now + 60_000 };
        let follow = |text: &str, continuation: Continuation, wake_word: bool| RouteInput {
            continuation: Some(continuation),
            wake_word,
            ..with_yandex(text)
        };
        assert_eq!(
            router().decide(&follow("хватит", continuation, false)),
            Decision::YandexDialog { capability: CapabilityName::Skills }
        );
        let game = Continuation { capability: CapabilityName::Games, executor: RouteStep::Own, until: now + 60_000 };
        let decision = router().decide(&RouteInput { continuation: Some(game), ..input("кошка") });
        match decision {
            Decision::Intent { source: IntentSource::Continuation, intent, .. } => {
                assert_eq!(intent, intent_of(CapabilityName::Games, "continue", json!({"text": "кошка"})))
            }
            other => panic!("{other:?}"),
        }
        // Yandex's question takes the answer, not a new request with the wake word (live 02.11); its skills do.
        let question =
            Continuation { capability: CapabilityName::Calls, executor: RouteStep::Yandex, until: now + 60_000 };
        assert!(matches!(router().decide(&follow("Когда он придёт?", question, false)), Decision::YandexDialog { .. }));
        let decision = router().decide(&follow("Передай папе, что я задержусь.", question, true));
        assert_eq!(quick(&decision).unwrap().0, "message");
        assert!(matches!(router().decide(&follow("хватит", continuation, true)), Decision::YandexDialog { .. }));
        let expired = Continuation { until: now - 1, ..continuation };
        assert!(quick(&router().decide(&follow("хватит", expired, false))).is_some());
    }

    #[test]
    fn rewrites_an_intent_by_what_the_station_does() {
        let r = router();
        r.intercept(Box::new(|intent, input| {
            let timer = input.device_state.as_ref().and_then(|state| state.timer_ids.first())?;
            (intent.capability == CapabilityName::Playback && intent.action == "stop")
                .then(|| intent_of(CapabilityName::Timers, "stop", json!({"timerId": timer})))
        }));
        let state = DeviceState { timer_ids: vec!["t-1".into()], ..DeviceState::default() };
        let decision = r.decide(&RouteInput { device_state: Some(state), ..input("стоп") });
        assert_eq!(quick(&decision).unwrap().1, &intent_of(CapabilityName::Timers, "stop", json!({"timerId": "t-1"})));
    }

    // The router under the policy (src/core/identity/policy.spec.ts).

    struct Music;

    impl OwnSkill for Music {
        fn capability(&self) -> CapabilityName {
            CapabilityName::Music
        }
        fn verified(&self, _intent: &Intent) -> bool {
            true
        }
        fn execute<'a>(&'a self, _: &'a Intent, _: &'a ExecutionContext) -> BoxFuture<'a, anyhow::Result<Outcome>> {
            Box::pin(async { Ok(Outcome::speech("Играю.")) })
        }
    }

    fn music_router() -> Router {
        Router::new(RouteTable::parse("own,yandex", ""), vec![Arc::new(Music)])
    }

    fn household(role: Role, deny: &[&str]) -> Principal {
        let restrictions = Restrictions { allow: None, deny: Some(deny.iter().map(|d| d.to_string()).collect()) };
        household_principal(role, Some(&restrictions))
    }

    #[test]
    fn serves_what_the_speaker_may_use_and_says_not_for_you_to_the_rest_whoever_could_serve_it() {
        let music = Intent::new(CapabilityName::Music, "play");
        assert_eq!(music_router().resolve(&music, true, Some(&household(Role::Member, &[]))).kind(), "own");
        let no_music = household(Role::Member, &["use:music"]);
        assert_eq!(music_router().resolve(&music, true, Some(&no_music)), Resolution::unsupported(RESTRICTED));
        let calls = Intent::new(CapabilityName::Calls, "call");
        assert_eq!(
            music_router().resolve(&calls, true, Some(&household(Role::Guest, &[]))),
            Resolution::unsupported(RESTRICTED)
        );
    }

    // The plugins' phrases (src/plugins): words of their own, and those of the capability a plugin serves.

    struct Phrases(Vec<(&'static str, CapabilityName)>);

    impl ExtraRules for Phrases {
        fn matched(&self, normalized: &str) -> Option<QuickMatch> {
            self.0.iter().find(|(start, _)| normalized.starts_with(start)).map(|(_, capability)| {
                let mut intent = Intent::new(*capability, "plugin");
                intent.params.insert(EXECUTOR.into(), json!("plugins"));
                QuickMatch { intent, rule: "plugin:test" }
            })
        }
    }

    struct Bridge;

    impl OwnSkill for Bridge {
        fn capability(&self) -> CapabilityName {
            CapabilityName::Plugins
        }
        fn verified(&self, _intent: &Intent) -> bool {
            true
        }
        fn execute<'a>(&'a self, _: &'a Intent, _: &'a ExecutionContext) -> BoxFuture<'a, anyhow::Result<Outcome>> {
            Box::pin(async { Ok(Outcome::speech("Плагин.")) })
        }
    }

    fn with_plugin(routes: &str) -> Router {
        let router =
            Router::new(RouteTable::parse("own,yandex", routes), vec![Arc::new(VolumeSkill), Arc::new(Bridge)]);
        router.add_rules(Arc::new(Phrases(vec![
            ("включи радио", CapabilityName::Radio),
            ("когда автобус", CapabilityName::Plugins),
            ("громче", CapabilityName::Plugins),
            ("включи музыку", CapabilityName::Radio),
        ])));
        router
    }

    #[test]
    fn gives_a_plugin_its_own_words_and_those_of_the_capability_it_serves_never_the_stations_controls() {
        let router = with_plugin("");
        let bus = router.decide(&input("Ну когда автобус?"));
        let (rule, intent, resolution) = quick(&bus).unwrap();
        assert_eq!((rule, intent.capability), ("plugin:test", CapabilityName::Plugins));
        assert!(matches!(resolution, Resolution::Own(skill) if skill.capability() == CapabilityName::Plugins));
        // The radio's words, as the plugin serves the radio: the built-in rule's intent goes along as the fallback.
        let radio = router.decide(&input("Включи радио джаз"));
        let (rule, intent, resolution) = quick(&radio).unwrap();
        assert_eq!(rule, "plugin:test");
        assert_eq!(
            intent.params[FALLBACK],
            json!({"capability": "radio", "action": "play", "params": {"station": "джаз"}})
        );
        assert_eq!(resolution.kind(), "own", "the bridge serves the radio's intent");
        // A built-in rule of another capability keeps its words; the station's controls always do.
        assert_eq!(quick(&router.decide(&input("включи музыку кино"))).unwrap().0, "music");
        assert_eq!(quick(&router.decide(&input("громче"))).unwrap().0, "louder");
        // The radio's route and the speaker's right to the radio hold for the plugin's radio too.
        let yandex_radio = with_plugin("radio=yandex");
        assert_eq!(quick(&yandex_radio.decide(&with_yandex("включи радио джаз"))).unwrap().2, &Resolution::Yandex);
        let no_radio = RouteInput {
            principal: Some(household(Role::Member, &["use:radio"])),
            ..input("включи радио джаз")
        };
        assert_eq!(quick(&router.decide(&no_radio)).unwrap().2, &Resolution::unsupported(RESTRICTED));
    }

    #[test]
    fn keeps_the_model_out_when_chat_itself_is_not_allowed() {
        let silent = household(Role::Member, &["use:chat"]);
        let request = RouteInput { principal: Some(silent), ..with_yandex("расскажи анекдот") };
        assert_eq!(
            music_router().decide(&request),
            Decision::Unsupported { capability: CapabilityName::Chat, reason: RESTRICTED.into() }
        );
    }
}
