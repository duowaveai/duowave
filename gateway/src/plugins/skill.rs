//! `PluginSkill`: the bridge from the router to the plugins. A plugin's phrase is an intent the router routes like
//! any quick rule's — its capability's route and the speaker's right to it — and executes through this skill (the
//! intent names the `plugins` capability as its executor). The plugin's function gets the phrase's slots and the
//! request; its answer becomes an ordinary `Outcome`:
//!
//! - a string: speech;
//! - `{say, play: [items], index, listen}`: speech, our player with those items (audio_play), listening after;
//! - `display: {temperature}`: the number on the station's display, as Yandex shows the weather;
//! - `model: {system, prompt, key?, fresh?}` (right `llm.chat`): the gateway's model says it, in the plugin's words —
//!   a game's host, a tale (Host API 1.1, stage 10);
//! - `continue: true` (or `{handler}`): the conversation goes on with the plugin without the wake word, the station
//!   listening, until it answers without `continue`;
//! - nothing (`null`, `undefined`): it lets the words go — when they were a built-in rule's too, the built-in
//!   implementation serves them; so it does when the plugin fails.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::controller::PluginHost;
use super::manifest::Permission;
use super::media::media_items_with;
use super::phrases::REQUEST;
use super::runtime::CallKind;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::media::player::MediaPlayer;
use crate::skills::router::{Router, FALLBACK};
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill, PhraseStream};

/// A conversation with a plugin goes on this long after its last answer.
const GOING_MS: u64 = 3 * 60_000;

/// Speech longer than this (in letters) plays before the answer's music, not with it.
const LONG_SPEECH: usize = 60;

/// About how long Alice says `letters` letters, with the time the station takes to begin.
fn speaking_time(letters: usize) -> Duration {
    Duration::from_millis(1500 + letters as u64 * 65)
}

/// The gateway's model for answers in a plugin's words: key, system, text, cancel, fresh.
pub type ModelHost =
    Arc<dyn Fn(String, String, String, tokio_util::sync::CancellationToken, bool) -> PhraseStream + Send + Sync>;

pub struct PluginSkill {
    host: Weak<PluginHost>,
    router: Weak<Router>,
    player: Arc<MediaPlayer>,
    /// The workshop by voice (workshop/voice.rs): «давай сделаем навык» and the conversation after it.
    voice: OnceLock<Arc<super::workshop::voice::VoiceWorkshop>>,
    /// The gateway's model, for `model` answers.
    model: OnceLock<ModelHost>,
    /// Devices whose conversation goes on with a plugin: its id, its handler, the capability, since when.
    going: Mutex<HashMap<String, (String, String, CapabilityName, Instant)>>,
}

impl PluginSkill {
    pub fn new(host: Weak<PluginHost>, router: Weak<Router>, player: Arc<MediaPlayer>) -> PluginSkill {
        PluginSkill {
            host,
            router,
            player,
            voice: OnceLock::new(),
            model: OnceLock::new(),
            going: Mutex::new(HashMap::new()),
        }
    }

    /// The gateway's model for answers said in a plugin's words.
    pub fn set_model(&self, model: ModelHost) {
        let _ = self.model.set(model);
    }

    fn going(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, String, CapabilityName, Instant)>> {
        self.going.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The workshop by voice joins once it stands.
    pub fn set_voice(&self, voice: Arc<super::workshop::voice::VoiceWorkshop>) {
        let _ = self.voice.set(voice);
    }

    /// The input of a plugin's function for an intent: the slots, and the request under `request`.
    fn input(intent: &Intent, context: &ExecutionContext) -> Value {
        let mut input = intent.params.get("slots").and_then(Value::as_object).cloned().unwrap_or_default();
        input.insert(
            REQUEST.into(),
            json!({
                "text": context.text,
                "device": context.device_tag,
                "timezone": context.timezone,
                "capability": intent.capability,
            }),
        );
        Value::Object(input)
    }

    /// What the plugin's answer does; None when it lets the request go.
    fn outcome(
        &self,
        plugin: &str,
        handler: &str,
        capability: CapabilityName,
        answer: Value,
        context: &ExecutionContext,
    ) -> Option<Outcome> {
        let answer = match answer {
            Value::String(text) if !text.trim().is_empty() => {
                self.going().remove(&context.device_tag);
                return Some(Outcome::speech(text));
            }
            Value::Object(answer) => answer,
            _ => return None,
        };
        let host = self.host.upgrade()?;
        let manifest = host.package(plugin).map(|package| package.manifest.clone());
        let kind = super::media::kind_for(capability);
        let mut outcome = Outcome::default();
        if let Some(say) = answer.get("say").and_then(Value::as_str).map(str::trim).filter(|say| !say.is_empty()) {
            outcome.speech = Some(say.chars().take(1000).collect());
        }
        if let Some(celsius) =
            answer.get("display").and_then(|display| display.get("temperature")).and_then(Value::as_f64)
        {
            outcome.directives.extend(crate::skills::display::show_temperature(celsius));
        }
        // Said by the gateway's model, in the plugin's words: only with the right.
        if let Some(model) =
            answer.get("model").filter(|_| manifest.as_ref().is_some_and(|m| m.has(&Permission::LlmChat)))
        {
            if let Some(host_model) = self.model.get() {
                let text = |key: &str| model.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
                let key = format!("plugin:{plugin}:{}:{}", context.device_tag, text("key"));
                let prompt = if text("prompt").is_empty() { context.text.clone() } else { text("prompt") };
                let fresh = model.get("fresh").and_then(Value::as_bool).unwrap_or(false);
                outcome.phrases = Some(host_model(key, text("system"), prompt, context.cancel.clone(), fresh));
            }
        }
        if let Some(play) = answer.get("play") {
            let items =
                media_items_with(plugin, kind, play, context.media_base_url.as_deref(), &host.play_rights(plugin));
            if !items.is_empty() {
                let index = answer.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let directives = self.player.play(&context.device_tag, items, index, 0);
                // A long speech first, then the music: sent together, the station starts the music over the words.
                let spoken = outcome.speech.as_ref().map(|speech| speech.chars().count()).unwrap_or(0);
                let later = (spoken > LONG_SPEECH)
                    .then(|| host.playback_watch())
                    .flatten()
                    .zip(self.player.current(&context.device_tag).map(|(_, state)| state.play_id))
                    .is_some_and(|(watch, play_id)| {
                        watch.start_later(&context.device_tag, &play_id, speaking_time(spoken))
                    });
                if !later {
                    outcome.directives.extend(directives);
                }
            } else if outcome.speech.is_none() {
                outcome.speech = Some("Не нашла, что включить.".into());
            }
        }
        outcome.listen = answer.get("listen").and_then(Value::as_bool).unwrap_or(false);
        // The conversation goes on with the plugin: the next words come back to it without the wake word.
        let next = match answer.get("continue") {
            Some(Value::Bool(true)) => Some(handler.to_string()),
            Some(Value::Object(next)) => {
                next.get("handler").and_then(Value::as_str).map(str::to_string).or_else(|| Some(handler.to_string()))
            }
            _ => None,
        };
        match next {
            Some(next) => {
                self.going().insert(context.device_tag.clone(), (plugin.to_string(), next, capability, Instant::now()));
                outcome.listen = true;
                outcome.continuation =
                    Some(ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: GOING_MS });
            }
            None => {
                self.going().remove(&context.device_tag);
            }
        }
        (!outcome.is_empty()).then_some(outcome)
    }

    /// The built-in implementation of the words the plugin let go, when they were a built-in rule's.
    async fn fallback(&self, intent: &Intent, context: &ExecutionContext) -> Option<anyhow::Result<Outcome>> {
        let fallback: Intent = serde_json::from_value(intent.params.get(FALLBACK)?.clone()).ok()?;
        let skill = self.router.upgrade()?.skill(fallback.capability).filter(|skill| skill.available())?;
        Some(skill.execute(&fallback, context).await)
    }

    /// The words that follow a plugin's answer with `continue`: back to that plugin, with the words as `text`.
    async fn follow(&self, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let going = self.going().get(&context.device_tag).cloned();
        let Some((plugin, handler, capability, _)) =
            going.filter(|(_, _, _, since)| since.elapsed() < Duration::from_millis(GOING_MS))
        else {
            self.going().remove(&context.device_tag);
            return Ok(Outcome::speech("Разговор закончился. Скажите, что сделать."));
        };
        let host = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
        let mut intent = Intent::new(capability, "continue");
        intent.params.insert("slots".into(), json!({ "text": context.text }));
        let answer = host.call(&plugin, &handler, PluginSkill::input(&intent, context), CallKind::Voice).await;
        match answer {
            Ok(answer) => Ok(self.outcome(&plugin, &handler, capability, answer, context).unwrap_or_else(|| {
                self.going().remove(&context.device_tag);
                Outcome::speech("Хорошо.")
            })),
            Err(error) => {
                self.going().remove(&context.device_tag);
                Err(anyhow::anyhow!("plugin {plugin}: {error}"))
            }
        }
    }

    async fn serve(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        if intent.action == "continue" {
            return self.follow(context).await;
        }
        let text = |key: &str| intent.params.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let (plugin, handler) = (text("plugin"), text("handler"));
        let host = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
        let answer = host.call(&plugin, &handler, PluginSkill::input(intent, context), CallKind::Voice).await;
        let failed = match answer {
            Ok(answer) => match self.outcome(&plugin, &handler, intent.capability, answer, context) {
                Some(outcome) => return Ok(outcome),
                None => None,
            },
            Err(error) => Some(error),
        };
        if let Some(served) = self.fallback(intent, context).await {
            return served;
        }
        match failed {
            Some(error) => Err(anyhow::anyhow!("plugin {plugin}: {error}")),
            None => Ok(Outcome::speech("Не получилось.")),
        }
    }
}

impl OwnSkill for PluginSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Plugins
    }

    fn verified(&self, _intent: &Intent) -> bool {
        // Speech and our player: nothing the station has not confirmed.
        true
    }

    fn available(&self) -> bool {
        self.voice.get().is_some() || self.host.upgrade().is_some_and(|host| host.has_phrases())
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        if let Some(voice) = self.voice.get().filter(|voice| voice.handles(intent, &context.device_tag)) {
            return voice.execute(intent, context);
        }
        Box::pin(self.serve(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginsConfig;
    use crate::core::availability::Placement;
    use crate::core::events::EventBus;
    use crate::plugins::controller::PluginsDeps;
    use crate::plugins::quickjs::QuickJs;
    use crate::plugins::registry::tests::write_package;
    use crate::settings::SettingsService;
    use crate::skills::media::media_skills::RadioSkill;
    use crate::skills::media::sources::{MediaSources, StaticRadioCatalog};
    use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
    use crate::skills::routes::RouteTable;
    use crate::storage::Db;
    use crate::utils::time::now_ms;
    use chrono::Utc;

    const CODE: &str = r#"
        export function play({ station, request }) {
            if (station === "ошибка") throw new Error("каталог упал");
            if (station !== "джаз") return null;
            return { say: `Включаю: ${request.text}`, play: [{ id: "jazz", title: "Джаз", url: "https://radio.example/jazz.mp3" }] };
        }
    "#;

    /// A router with our radio (a list with «Студия 21») and a plugin that serves the radio's words.
    async fn router() -> (Arc<Router>, Arc<PluginHost>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "id": "radio", "version": "1.0.0", "title": "Радио", "api": "^1", "runs": ["pc"],
            "permissions": ["voice.intents"],
            "contributes": { "intents": [{ "phrases": ["включи радио {station}"], "handler": "play", "capability": "radio" }] }
        });
        write_package(&dir.path().join("packages"), &manifest, Some(CODE));
        std::fs::write(
            dir.path().join("radio.json"),
            r#"[{"id": "studio21", "title": "Студия 21", "url": "https://hls.example/studio21.m3u8"}]"#,
        )
        .unwrap();
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let sources = MediaSources::new(vec![Arc::new(StaticRadioCatalog::new(dir.path().join("radio.json")))]);
        let player = Arc::new(MediaPlayer::new());
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: dir.path().join("packages"),
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.path().join("packages-catalog"),
                dev_dir: dir.path().join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: sources.clone(),
            placement: Arc::new(|| Placement::Pc),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
        });
        host.enable("radio", None).await.unwrap();
        let router = Arc::new(Router::new(
            RouteTable::parse("own,yandex", ""),
            vec![Arc::new(RadioSkill::new(sources, player.clone()))],
        ));
        router.register(Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), player)));
        router.add_rules(host.clone());
        (router, host, dir)
    }

    async fn heard(router: &Router, text: &str) -> anyhow::Result<Outcome> {
        let input = RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let Decision::Intent { intent, resolution: Resolution::Own(skill), rule, .. } = router.decide(&input) else {
            panic!("{text}: not ours");
        };
        assert_eq!(rule, Some("plugin:radio"), "{text}");
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = text.into();
        skill.execute(&intent, &context).await
    }

    fn stream(outcome: &Outcome) -> String {
        outcome.directives[0].payload.as_ref().unwrap()["stream"]["url"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn plays_what_the_plugin_found_and_gives_the_built_in_radio_what_it_let_go() {
        let (router, _host, _dir) = router().await;
        let jazz = heard(&router, "Включи радио джаз").await.unwrap();
        assert_eq!(jazz.directives[0].name, "audio_play");
        assert_eq!(stream(&jazz), "https://radio.example/jazz.mp3");
        assert_eq!(jazz.speech.as_deref(), Some("Включаю: Включи радио джаз"));
        // Not the plugin's: our own list has it.
        let ours = heard(&router, "включи радио студия 21").await.unwrap();
        assert_eq!(stream(&ours), "https://hls.example/studio21.m3u8");
        // The plugin failed: our radio answers as it would without the plugin.
        let failed = heard(&router, "включи радио ошибка").await.unwrap();
        assert_eq!(failed.speech.as_deref(), Some("Не нашла такую станцию."));
    }

    /// The household's music as the gateway's services give it: two files of content/music.
    struct Content;

    impl crate::plugins::host_api::HostServices for Content {
        fn content(
            &self,
            folder: &str,
            _query: Option<String>,
            _limit: usize,
        ) -> futures::future::BoxFuture<'static, Vec<Value>> {
            let items = if folder == "music" {
                vec![
                    json!({ "id": "music/Битлз/Yesterday.mp3", "content": "music/Битлз/Yesterday.mp3", "title": "Yesterday" }),
                ]
            } else {
                Vec::new()
            };
            Box::pin(async move { items })
        }
        fn call_tool(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Map<String, Value>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_devices(&self) -> futures::future::BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_act(
            &self,
            _: String,
            _: String,
            _: Option<f64>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn weather_place(&self) -> Option<String> {
            None
        }
        fn news_feeds(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    /// The repository's built-in package, as the build lays it out.
    fn built_in(root: &std::path::Path, id: &str) {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages").join(id);
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(source.join("manifest.json")).unwrap()).unwrap();
        let target = root.join(id).join(manifest["version"].as_str().unwrap());
        std::fs::create_dir_all(&target).unwrap();
        for file in ["manifest.json", "backend.js"] {
            std::fs::copy(source.join(file), target.join(file)).unwrap();
        }
    }

    /// A host with the repository's built-in packages `ids`, on, over these services.
    async fn built_in_host(
        dir: &std::path::Path,
        ids: &[&str],
        services: Arc<dyn crate::plugins::host_api::HostServices>,
    ) -> Arc<PluginHost> {
        let packages = dir.join("packages");
        for id in ids {
            built_in(&packages, id);
        }
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: packages,
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.join("packages-catalog"),
                dev_dir: dir.join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(|| Placement::Pc),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: Some(services),
        });
        for id in ids {
            host.enable(id, None).await.unwrap();
        }
        host
    }

    #[tokio::test]
    async fn the_built_in_games_and_music_plugins_speak_by_the_model_listen_on_and_play_the_households_files() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let host = built_in_host(dir.path(), &["games", "music"], Arc::new(Content)).await;
        let player = Arc::new(MediaPlayer::new());
        let router = Arc::new(Router::new(RouteTable::parse("own,yandex", ""), Vec::new()));
        let skill = Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), player));
        // The model: says what it was given, so the test hears whose words went in.
        let asked: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
        let heard_by_model = asked.clone();
        skill.set_model(Arc::new(move |key: String, system: String, text: String, _cancel, _fresh| {
            heard_by_model.lock().unwrap().push((key, system));
            futures::stream::iter(vec![Ok(format!("Ход: {text}"))]).boxed()
        }));
        router.register(skill.clone());
        router.add_rules(host.clone());

        let input = |text: &str, continuation: Option<crate::skills::router::Continuation>| RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            continuation,
            wake_word: continuation.is_none(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let run = |decision: Decision, text: &str| {
            let Decision::Intent { intent, resolution: Resolution::Own(skill), .. } = decision else {
                panic!("{text}: not ours")
            };
            let mut context = ExecutionContext::new("kitchen", Utc::now());
            context.text = text.into();
            (intent, skill, context)
        };
        // «Давай поиграем в города»: the model hosts, the station listens on without the wake word.
        let (intent, served, context) =
            run(router.decide(&input("давай поиграем в города", None)), "давай поиграем в города");
        let started = served.execute(&intent, &context).await.unwrap();
        assert!(started.listen && started.continuation.is_some());
        let said: Vec<String> = started.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert!(said[0].starts_with("Ход: Начнём!"), "{said:?}");
        assert!(asked.lock().unwrap()[0].1.contains("«города»"));
        // The player's move comes back to the plugin; «хватит» ends it.
        let going = crate::skills::router::Continuation {
            capability: CapabilityName::Plugins,
            executor: crate::skills::routes::RouteStep::Own,
            until: now_ms() + 60_000,
        };
        let (intent, served, context) = run(router.decide(&input("москва", Some(going))), "москва");
        let moved = served.execute(&intent, &context).await.unwrap();
        let said: Vec<String> = moved.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(said, ["Ход: москва"]);
        let (intent, served, context) = run(router.decide(&input("хватит", Some(going))), "хватит");
        let ended = served.execute(&intent, &context).await.unwrap();
        assert_eq!(ended.speech.as_deref(), Some("Хорошо, поиграем в другой раз."));
        assert!(ended.continuation.is_none());

        // «Включи мою музыку»: the household's file, its link made by the gateway.
        let (intent, served, mut context) = run(router.decide(&input("включи мою музыку", None)), "включи мою музыку");
        context.media_base_url = Some("http://192.0.2.10/media".into());
        let music = served.execute(&intent, &context).await.unwrap();
        assert_eq!(
            stream(&music),
            format!("http://192.0.2.10/media/music/{}/Yesterday.mp3", crate::skills::js::encode_uri_component("Битлз"))
        );
    }

    /// The other plugins' tools for the morning show: the weather, the news, and the radio that answers what to play.
    struct Morning;

    impl crate::plugins::host_api::HostServices for Morning {
        fn content(&self, _: &str, _: Option<String>, _: usize) -> futures::future::BoxFuture<'static, Vec<Value>> {
            Box::pin(async { Vec::new() })
        }
        fn call_tool(
            &self,
            caller: &str,
            name: &str,
            _: serde_json::Map<String, Value>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            assert_eq!(caller, "morning");
            let answer = match name {
                "weather_now" => Ok("Сейчас +5, облачно. Ветер слабый. Завтра теплее.".to_string()),
                "news_headlines" => Ok("Открыли новый мост. Завтра концерт. Ещё новость.".to_string()),
                "radio_play" => Ok(json!({ "say": "Включаю Хит FM", "play": [{ "id": "hitfm", "title": "Хит FM", "url": "https://hitfm.hostingradio.ru/hitfm128.mp3" }] }).to_string()),
                other => Err(format!("нет инструмента {other}")),
            };
            Box::pin(async move { answer })
        }
        fn smart_home_devices(&self) -> futures::future::BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_act(
            &self,
            _: String,
            _: String,
            _: Option<f64>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn weather_place(&self) -> Option<String> {
            None
        }
        fn news_feeds(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn the_morning_show_tells_the_weather_and_the_news_from_other_plugins_and_plays_the_radio_after() {
        let dir = tempfile::tempdir().unwrap();
        let host = built_in_host(dir.path(), &["morning"], Arc::new(Morning)).await;
        let router = Arc::new(Router::new(RouteTable::parse("own,yandex", ""), Vec::new()));
        let skill =
            Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), Arc::new(MediaPlayer::new())));
        router.register(skill);
        router.add_rules(host.clone());
        let input = RouteInput {
            text: "утреннее шоу".into(),
            device_tag: "kitchen".into(),
            wake_word: true,
            now: now_ms(),
            ..RouteInput::default()
        };
        let Decision::Intent { intent, resolution: Resolution::Own(served), .. } = router.decide(&input) else {
            panic!("not ours")
        };
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = "утреннее шоу".into();
        let show = served.execute(&intent, &context).await.unwrap();
        let said = show.speech.clone().unwrap();
        assert!(
            said.contains("Погода. Сейчас +5, облачно. Ветер слабый.") && !said.contains("Завтра теплее"),
            "{said}"
        );
        assert!(
            said.contains("Новости. Открыли новый мост. Завтра концерт.") && said.ends_with("А теперь — Хит FM."),
            "{said}"
        );
        assert_eq!(stream(&show), "https://hitfm.hostingradio.ru/hitfm128.mp3");
    }
}
