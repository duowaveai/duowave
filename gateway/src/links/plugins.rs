//! The gateway's services as the plugins reach them through `host.call` (Host API 1.1, stage 10 of
//! docs/plans/plugins.md): the household's own music and tales on the disk, other plugins' tools, the smart home
//! through the panel's connector (its keys stay here), the weather's usual place and the news feeds the panel chose.
//! What the built-in skills used, so the skills themselves can be plugins.

use std::sync::{Arc, OnceLock, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::bus::Trail;
use crate::plugins::host_api::HostServices;
use crate::plugins::PluginHost;
use crate::skills::smart_home::home_assistant::HomeAssistantProvider;
use crate::skills::smart_home::provider::{HomeAction, SmartHomeProvider};
use crate::skills::{SkillConnectors, SkillsService};

pub struct PluginServices {
    skills: Weak<SkillsService>,
    /// The panel's connectors as the skills read them: the weather's place, the news feeds, Home Assistant.
    connectors: Arc<dyn SkillConnectors>,
    plugins: OnceLock<Weak<PluginHost>>,
    /// The apps' long tasks, once they stand: `host.tasks.*`.
    tasks: OnceLock<Weak<crate::tasks::Tasks>>,
    /// The plugins' schedules, once they stand: `host.schedule.*`.
    schedules: OnceLock<Weak<crate::plugins::schedule::Scheduler>>,
    /// The plugins' pictures: `host.files.*`.
    files: OnceLock<Arc<crate::plugins::files::PluginFiles>>,
    /// The bus (src/bus): `host.bus.*`, the folders, and the older services carried as letters.
    bus: OnceLock<Weak<crate::bus::Bus>>,
    /// The household's model, for the apps' texts (`system/brain` → `write`).
    dialog: OnceLock<Weak<crate::dialog::dialog_service::DialogService>>,
    /// The voices' providers, for the apps' lines (`system/voice` → `synth`, capabilities/voices.rs).
    voices: OnceLock<Arc<crate::capabilities::voices::Voices>>,
    /// The household's memory, for the apps' interests (`system/memory` → `interests`).
    memory: OnceLock<Weak<crate::memory::MemoryService>>,
    /// The household's recognition, for the apps' hearing (`system/hearing`, capabilities/hearing.rs).
    speech: OnceLock<Weak<crate::speech::speech_service::SpeechService>>,
    /// The panel's models, for an app that chooses its own (`system/brain` → `models`, `write` with a pick).
    brain: OnceLock<Weak<crate::dialog::brain::brain_service::BrainService>>,
    /// The publishers (`system/publish`, capabilities/publish.rs): the gateway's own and the platforms' upload pages.
    publishers: OnceLock<Arc<crate::capabilities::publish::Publishers>>,
    /// «Выход в интернет»: the connections an app gives (`system/network` → `connections`).
    egress: OnceLock<Weak<crate::egress::Egress>>,
    /// The station's microphone and its Alice, for the apps that hear a word of their own (src/activator).
    activator: OnceLock<Arc<crate::activator::Activator>>,
}

/// The words an interest is found by in the memory: likes, pastimes, what one watches and reads.
const INTEREST_WORDS: &str =
    "любит нравится интересуется увлекается хобби смотрит читает фильмы сериалы книги жанр игры \
     likes loves enjoys hobby watches reads films series books genre";

/// An app's request to the model at most, and its instructions; how long it may write.
const WRITE_PROMPT_MAX: usize = 100_000;
const WRITE_SYSTEM_MAX: usize = 20_000;
const WRITE_LIMIT: std::time::Duration = std::time::Duration::from_secs(180);
/// An app's agent: its steps (rounds of tool calls) and minutes at most, and by default; what one file tool gives.
const AGENT_STEPS_MAX: u64 = 40;
const AGENT_STEPS: u64 = 12;
const AGENT_MINUTES_MAX: u64 = 30;
const AGENT_MINUTES: u64 = 10;
const AGENT_READ_MAX: usize = 64 * 1024;
const AGENT_WRITE_MAX: usize = 256 * 1024;

/// A line to voice at most; how long the voice may take.
const SYNTH_TEXT_MAX: usize = 4000;
const SYNTH_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);
/// The voice an app gets when it names none: Piper «Денис», CC0 — one may publish it (docs/plans/feed.md, decision 18).
const DEFAULT_VOICE: &str = "piper/denis";
/// The sound files a line may be asked into; the provider's own kind replaces the extension.
const SOUND_EXTENSIONS: [&str; 5] = [".wav", ".mp3", ".ogg", ".m4a", ".opus"];

/// Where a line goes: the path asked, with the provider's extension instead of any sound one it had.
fn sound_path(to: &str, ext: &str) -> String {
    let lower = to.to_ascii_lowercase();
    let base = SOUND_EXTENSIONS
        .iter()
        .find(|known| lower.ends_with(*known))
        .map(|known| &to[..to.len() - known.len()])
        .unwrap_or(to);
    format!("{base}.{ext}")
}

/// The JSON in a model's answer: as it is, or inside a fence, or from the first brace to the last.
pub fn json_in(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let fenced = trimmed.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    if let Ok(value) = serde_json::from_str::<Value>(fenced) {
        return Some(value);
    }
    let start = trimmed.find(['{', '['])?;
    let end = trimmed.rfind(['}', ']'])?;
    (end > start).then(|| serde_json::from_str::<Value>(&trimmed[start..=end]).ok()).flatten()
}

impl PluginServices {
    pub fn new(skills: &Arc<SkillsService>, connectors: Arc<dyn SkillConnectors>) -> Arc<PluginServices> {
        Arc::new(PluginServices {
            skills: Arc::downgrade(skills),
            connectors,
            plugins: OnceLock::new(),
            tasks: OnceLock::new(),
            schedules: OnceLock::new(),
            files: OnceLock::new(),
            bus: OnceLock::new(),
            dialog: OnceLock::new(),
            voices: OnceLock::new(),
            memory: OnceLock::new(),
            speech: OnceLock::new(),
            brain: OnceLock::new(),
            publishers: OnceLock::new(),
            egress: OnceLock::new(),
            activator: OnceLock::new(),
        })
    }

    /// The activators' service, once the station's sessions stand.
    pub fn set_activator(&self, activator: Arc<crate::activator::Activator>) {
        let _ = self.activator.set(activator);
    }

    pub fn set_brain(&self, brain: &Arc<crate::dialog::brain::brain_service::BrainService>) {
        let _ = self.brain.set(Arc::downgrade(brain));
    }

    /// The publishers (capabilities/publish.rs): `system/publish` sends by them.
    pub fn set_publishers(&self, publishers: &Arc<crate::capabilities::publish::Publishers>) {
        let _ = self.publishers.set(publishers.clone());
    }

    /// «Выход в интернет», for the connections an app gives.
    pub fn set_egress(&self, egress: &Arc<crate::egress::Egress>) {
        let _ = self.egress.set(Arc::downgrade(egress));
    }

    /// The household's recognition, for the apps' hearing.
    pub fn set_speech(&self, speech: &Arc<crate::speech::speech_service::SpeechService>) {
        let _ = self.speech.set(Arc::downgrade(speech));
    }

    /// The household's memory, for the apps' interests.
    pub fn set_memory(&self, memory: &Arc<crate::memory::MemoryService>) {
        let _ = self.memory.set(Arc::downgrade(memory));
    }

    /// The household's model, for the apps (docs/plans/feed.md, stage 4).
    pub fn set_model(&self, dialog: &Arc<crate::dialog::dialog_service::DialogService>) {
        let _ = self.dialog.set(Arc::downgrade(dialog));
    }

    /// The voices' providers (capabilities/voices.rs): `system/voice` voices by them.
    pub fn set_voices(&self, voices: &Arc<crate::capabilities::voices::Voices>) {
        let _ = self.voices.set(voices.clone());
    }

    /// The plugin host, once it stands (it is built with these services).
    pub fn set_plugins(&self, plugins: &Arc<PluginHost>) {
        let _ = self.plugins.set(Arc::downgrade(plugins));
    }

    /// The apps' long tasks, once they stand.
    pub fn set_tasks(&self, tasks: &Arc<crate::tasks::Tasks>) {
        let _ = self.tasks.set(Arc::downgrade(tasks));
    }

    /// The bus, once it stands.
    pub fn set_bus(&self, bus: &Arc<crate::bus::Bus>) {
        let _ = self.bus.set(Arc::downgrade(bus));
    }

    /// The plugins' pictures.
    pub fn set_files(&self, files: &Arc<crate::plugins::files::PluginFiles>) {
        let _ = self.files.set(files.clone());
    }

    /// The plugins' schedules, once they stand.
    pub fn set_schedules(&self, schedules: &Arc<crate::plugins::schedule::Scheduler>) {
        let _ = self.schedules.set(Arc::downgrade(schedules));
    }

    fn provider(&self) -> Option<Arc<dyn SmartHomeProvider>> {
        let home = self.connectors.home_assistant()?;
        Some(Arc::new(HomeAssistantProvider::new(&home.url, &home.token, None, None)))
    }
}

impl HostServices for PluginServices {
    fn content(&self, folder: &str, query: Option<String>, limit: usize) -> BoxFuture<'static, Vec<Value>> {
        let source = self.skills.upgrade().and_then(|skills| skills.media_sources.by_id(&format!("content:{folder}")));
        Box::pin(async move {
            let Some(source) = source else { return Vec::new() };
            // The links are not the plugin's to make: its items carry their place under CONTENT_DIR (`content`), and
            // the gateway makes the link for the station that plays them.
            source
                .search(query, Some("content:".into()))
                .await
                .into_iter()
                .take(limit)
                .map(|item| {
                    json!({
                        "id": item.id,
                        "content": item.id,
                        "title": item.title,
                        "subtitle": item.subtitle,
                        "type": item.r#type,
                        "durationMs": item.duration_ms,
                    })
                })
                .collect()
        })
    }

    fn smart_home_devices(&self) -> BoxFuture<'static, Result<Value, String>> {
        let provider = self.provider();
        Box::pin(async move {
            let provider = provider
                .ok_or_else(|| crate::tr!("умный дом не подключён («Приложения» → «Подключения»)").to_string())?;
            let devices = provider.devices(&CancellationToken::new()).await.map_err(|error| error.to_string())?;
            Ok(Value::Array(
                devices
                    .into_iter()
                    .map(|device| {
                        let details: Map<String, Value> = device.details.into_iter().collect();
                        json!({ "id": device.id, "name": device.name, "kind": device.kind, "state": device.state, "details": details })
                    })
                    .collect(),
            ))
        })
    }

    fn smart_home_act(
        &self,
        device: String,
        action: String,
        value: Option<f64>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let provider = self.provider();
        Box::pin(async move {
            let provider = provider
                .ok_or_else(|| crate::tr!("умный дом не подключён («Приложения» → «Подключения»)").to_string())?;
            let action = HomeAction::parse(&action).ok_or_else(|| {
                crate::tr!("действие «{action}»: on, off, set_brightness, set_temperature, scene", action = action)
            })?;
            let cancel = CancellationToken::new();
            let devices = provider.devices(&cancel).await.map_err(|error| error.to_string())?;
            let found = devices
                .iter()
                .find(|known| known.id == device)
                .ok_or_else(|| crate::tr!("нет устройства {device}", device = device))?;
            provider.act(found, action, value, &cancel).await.map_err(|error| error.to_string())?;
            Ok(format!("{}: {}", found.name, action.as_str()))
        })
    }

    fn weather_place(&self) -> Option<String> {
        self.connectors.weather_place()
    }

    fn network_connections(&self, caller: &str, input: Value) -> Result<Value, String> {
        let egress =
            self.egress.get().and_then(Weak::upgrade).ok_or_else(|| crate::tr!("выход в интернет ещё не готов"))?;
        egress.report(caller, &input)
    }

    fn news_feeds(&self) -> Vec<Value> {
        self.connectors.news_feeds().into_iter().map(|feed| json!({ "url": feed.url, "topic": feed.topic })).collect()
    }

    fn files(
        &self,
        caller: &str,
        action: &str,
        input: Value,
        bytes: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let answer = match (self.files.get(), action) {
            (None, _) => Err(crate::tr!("картинок здесь нет").to_string()),
            (Some(files), "put") => {
                let room = self
                    .plugins
                    .get()
                    .and_then(Weak::upgrade)
                    .and_then(|plugins| plugins.package(caller))
                    .map_or(1, |package| package.manifest.budget.files_mb);
                let name = input.get("name").and_then(Value::as_str).unwrap_or_default();
                files.put(caller, name, &bytes.unwrap_or_default(), room)
            }
            (Some(files), "list") => Ok(Value::Array(files.list(caller))),
            (Some(files), "delete") => {
                Ok(Value::Bool(files.delete(caller, input.get("name").and_then(Value::as_str).unwrap_or_default())))
            }
            (Some(_), other) => Err(crate::tr!("нет функции files.{other}", other = other)),
        };
        Box::pin(async move { answer })
    }

    fn write(&self, caller: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let dialog = self.dialog.get().and_then(Weak::upgrade);
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let caller = caller.to_string();
        Box::pin(async move {
            let dialog = dialog.ok_or("модели здесь нет")?;
            let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let prompt = field("prompt");
            if prompt.is_empty() {
                return Err("write: нужна просьба — prompt".into());
            }
            if prompt.chars().count() > WRITE_PROMPT_MAX {
                return Err(format!("write: просьба длиннее {WRITE_PROMPT_MAX} знаков"));
            }
            let mut system = field("system");
            if system.chars().count() > WRITE_SYSTEM_MAX {
                return Err(format!("write: указания длиннее {WRITE_SYSTEM_MAX} знаков"));
            }
            let (profile, pick) = model_choice(&input, crate::config::ModelProfile::Fast)?;
            let wants_json = input.get("json").and_then(Value::as_bool).unwrap_or(false);
            if wants_json {
                system.push_str("\n\nОтветь только JSON — без пояснений вокруг и без ```.");
            }
            let cancel = CancellationToken::new();
            let _stop = cancel.clone().drop_guard();
            let written =
                tokio::time::timeout(WRITE_LIMIT, dialog.write(profile, pick.as_ref(), system, prompt, cancel))
                    .await
                    .map_err(|_| format!("модель не ответила за {} мин", WRITE_LIMIT.as_secs() / 60))?
                    .map_err(|error| format!("модель не ответила: {error}"))?;
            let model = &written.profile;
            let tokens = written.usage.as_ref().map(|usage| json!({ "input": usage.input, "output": usage.output }));
            // The app's journal keeps what it took; a model in the cloud is amber there: the text left the house.
            if let Some(plugins) = &plugins {
                let place = if model.cloud { "в облаке" } else { "дома" };
                let used = written
                    .usage
                    .as_ref()
                    .map(|usage| format!(": {} токенов на входе, {} на выходе", usage.input, usage.output))
                    .unwrap_or_default();
                plugins.note_line(
                    &caller,
                    if model.cloud { "warn" } else { "info" },
                    &format!("модель {} ({}) {place}{used}", model.model, model.title),
                );
            }
            let mut answer = json!({
                "text": written.text,
                "model": { "title": model.title, "model": model.model, "cloud": model.cloud },
                "tokens": tokens,
            });
            if wants_json {
                // Not JSON — said with its length and end: a cut answer (the limit ran out) shows by its end.
                let parsed = json_in(&written.text).ok_or_else(|| {
                    let chars = written.text.chars().count();
                    let tail: String = written.text.chars().skip(chars.saturating_sub(60)).collect();
                    format!("модель ответила не JSON ({chars} знаков, конец: «{}»)", tail.replace('\n', " ").trim())
                })?;
                answer["json"] = parsed;
            }
            Ok(answer)
        })
    }

    fn agent(&self, caller: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let dialog = self.dialog.get().and_then(Weak::upgrade);
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let files = self.files.get().cloned();
        let caller = caller.to_string();
        Box::pin(async move {
            let (dialog, plugins, files) = match (dialog, plugins, files) {
                (Some(dialog), Some(plugins), Some(files)) => (dialog, plugins, files),
                _ => return Err("агента для приложений здесь нет".into()),
            };
            if !dialog.tools_enabled() {
                return Err("модель «Мозга» не умеет звать инструменты — агенту нужна другая".into());
            }
            let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let task = field("task");
            if task.is_empty() || task.chars().count() > WRITE_PROMPT_MAX {
                return Err(format!("agent: задача — task, до {WRITE_PROMPT_MAX} знаков"));
            }
            let (profile, pick) = model_choice(&input, crate::config::ModelProfile::Smart)?;
            let number = |key: &str, default: u64, max: u64| {
                input.get(key).and_then(Value::as_u64).unwrap_or(default).clamp(1, max)
            };
            let steps = number("maxSteps", AGENT_STEPS, AGENT_STEPS_MAX);
            let minutes = number("maxMinutes", AGENT_MINUTES, AGENT_MINUTES_MAX);
            let package = plugins.package(&caller).ok_or("приложение выключено")?;
            // Its own tools it names, as the model sees them; the gateway's file tools over its own folder.
            let named: Vec<String> = input
                .get("tools")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let own: Vec<(String, String, crate::dialog::llm::LlmTool)> = package
                .manifest
                .contributes
                .tools
                .iter()
                .filter(|tool| named.contains(&tool.name))
                .map(|tool| {
                    let offered = crate::dialog::llm::LlmTool {
                        name: tool.name.clone(),
                        description: tool.description.chars().take(1024).collect(),
                        parameters: tool.input.clone(),
                    };
                    (tool.name.clone(), tool.handler.clone(), offered)
                })
                .collect();
            if let Some(unknown) = named.iter().find(|name| !own.iter().any(|(known, _, _)| known == *name)) {
                return Err(format!("agent: у приложения нет инструмента «{unknown}» (contributes.tools)"));
            }
            let mut tools: Vec<crate::dialog::llm::LlmTool> = own.iter().map(|(_, _, tool)| tool.clone()).collect();
            tools.extend(agent_file_tools());
            let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let run = {
                let (plugins, files, caller, own, calls) =
                    (plugins.clone(), files.clone(), caller.clone(), own.clone(), calls.clone());
                move |call: crate::dialog::llm::ToolCall, _cancel: CancellationToken| {
                    let (plugins, files, caller, own, calls) =
                        (plugins.clone(), files.clone(), caller.clone(), own.clone(), calls.clone());
                    async move {
                        use crate::dialog::dialog_service::ToolOutcome;
                        if let Some(error) = call.error {
                            return Ok(ToolOutcome::error(error));
                        }
                        calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(call.name.clone());
                        let path = call.input.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                        plugins.note_line(&caller, "info", &format!("агент: {} {path}", call.name));
                        let outcome = match call.name.as_str() {
                            "list_files" => agent_list(&files, &caller, &path),
                            "read_file" => agent_read(&files, &caller, &path),
                            "write_file" => {
                                let text = call.input.get("text").and_then(Value::as_str).unwrap_or("");
                                agent_write(&files, &caller, &path, text)
                            }
                            name => match own.iter().find(|(known, _, _)| known == name) {
                                Some((_, handler, _)) => plugins
                                    .call(
                                        &caller,
                                        handler,
                                        Value::Object(call.input.clone()),
                                        crate::plugins::runtime::CallKind::Agent,
                                    )
                                    .await
                                    .map(|answer| match answer {
                                        Value::String(text) => text,
                                        other => other.to_string(),
                                    })
                                    .map_err(|error| error.to_string()),
                                None => Err(format!("нет инструмента {name}")),
                            },
                        };
                        Ok(match outcome {
                            Ok(content) => ToolOutcome::from(content.chars().take(AGENT_READ_MAX).collect::<String>()),
                            Err(error) => ToolOutcome::error(error),
                        })
                    }
                }
            };
            let mut toolbox = crate::dialog::dialog_service::ToolBox::new(tools, run);
            toolbox.rounds = Some(steps as usize);
            toolbox.budget_ms = Some(minutes * 60_000);
            toolbox.round_ms = Some(120_000);
            let context = crate::dialog::DialogContext {
                profile: Some(profile),
                model: pick,
                // Its deals are the apps' (src/router).
                seat: Some(crate::router::Seat::App),
                // The agent writes files (a series in code), not spoken answers: the workshop's longer limit.
                max_tokens: Some(dialog.config.workshop_max_tokens),
                system: Some(format!(
                    "Ты — агент приложения «{}» на компьютере. Работай только с файлами его папки (list_files, \
                     read_file, write_file — пути внутри папки) и его инструментами. Когда закончишь, ответь коротко, \
                     что сделал.\n\n{}",
                    package.manifest.title,
                    field("system")
                )),
                ..crate::dialog::DialogContext::default()
            };
            // `bus:` — no episode of the household's memory; forgotten once the run is over.
            let key = format!("bus:agent:{caller}:{}", uuid::Uuid::new_v4().simple());
            let cancel = CancellationToken::new();
            let _stop = cancel.clone().drop_guard();
            let mut phrases = dialog.answer(&key, &task, context, cancel.clone(), Some(toolbox));
            let deadline = std::time::Duration::from_secs(minutes * 60 + 120);
            let mut text = String::new();
            let collected = tokio::time::timeout(deadline, async {
                use futures::StreamExt;
                while let Some(phrase) = phrases.next().await {
                    let phrase = phrase.map_err(|error| error.to_string())?;
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(phrase.trim());
                }
                Ok::<(), String>(())
            })
            .await;
            dialog.forget(&key);
            match collected {
                Err(_) => return Err(format!("агент не закончил за {minutes} мин")),
                Ok(Err(error)) => return Err(format!("агент остановился: {error}")),
                Ok(Ok(())) => {}
            }
            let calls = calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            Ok(json!({ "text": text, "steps": calls.len(), "calls": calls }))
        })
    }

    fn models(&self) -> BoxFuture<'static, Result<Value, String>> {
        let brain = self.brain.get().and_then(Weak::upgrade);
        Box::pin(async move {
            let brain = brain.ok_or("моделей здесь нет")?;
            let view = brain.view();
            let models: Vec<Value> = view
                .providers
                .iter()
                .filter(|provider| provider.models.iter().any(|model| !model.is_empty()))
                .flat_map(|provider| {
                    provider.models.iter().filter(|model| !model.is_empty()).map(move |model| {
                        json!({ "provider": provider.id, "model": model, "title": provider.title, "cloud": provider.cloud })
                    })
                })
                .collect();
            let pick = |choice: &Option<crate::settings::definitions::BrainPick>| {
                choice.as_ref().map(|pick| json!({ "provider": pick.provider, "model": pick.model }))
            };
            Ok(json!({
                "models": models,
                "fast": pick(&view.choice.fast),
                "smart": pick(&view.choice.smart),
                "ready": view.status.profiles.fast.is_some() || view.status.profiles.smart.is_some(),
            }))
        })
    }

    fn publish(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let files = self.files.get().cloned();
        let publishers = self.publishers.get().cloned();
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let (caller, action) = (caller.to_string(), action.to_string());
        Box::pin(async move {
            let publishers = publishers.ok_or_else(|| crate::tr!("отправителей здесь нет"))?;
            // `publishers`: who is there, what each takes, whether it needs connecting, where it works.
            if action == "publishers" {
                return Ok(json!(publishers.cards()));
            }
            let files = files.ok_or_else(|| crate::tr!("своей папки здесь нет"))?;
            let path = input.get("file").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let file = files.inside(&caller, &path)?;
            if !file.is_file() {
                return Err(crate::tr!("нет файла {path} в своей папке", path = path));
            }
            let request = crate::capabilities::publish::Request { sender: caller.clone(), file, path, input };
            let published = publishers.send(&action, request)?.await?;
            // The journal is the sender's: what went where.
            if let Some(plugins) = &plugins {
                plugins.note_line(&caller, "info", &published.line);
            }
            Ok(published.answer)
        })
    }

    fn recognize(&self, caller: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let recognizer = self.speech.get().and_then(Weak::upgrade).and_then(|speech| speech.recognizer());
        let files = self.files.get().cloned();
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let caller = caller.to_string();
        Box::pin(async move {
            let recognizer =
                recognizer.ok_or_else(|| crate::tr!("слуха нет: распознавание речи в пульте выключено"))?;
            let files = files.ok_or_else(|| crate::tr!("своей папки здесь нет"))?;
            let path = input.get("file").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let file = files.inside(&caller, &path)?;
            if !file.is_file() {
                return Err(crate::tr!("нет файла {path} в своей папке", path = path));
            }
            let language = input.get("lang").and_then(Value::as_str).map(str::trim).filter(|lang| !lang.is_empty());
            let cancel = CancellationToken::new();
            let _stop = cancel.clone().drop_guard();
            let heard = crate::capabilities::hearing::recognize_file(&recognizer, &file, language, &cancel).await?;
            if let Some(plugins) = &plugins {
                plugins.note_line(
                    &caller,
                    "info",
                    &if heard.words.is_empty() {
                        crate::tr!("слух: {path} — без времени слов", path = path)
                    } else {
                        crate::tr!("слух: {path} — слов: {count}", path = path, count = heard.words.len())
                    },
                );
            }
            Ok(json!({ "text": heard.text, "language": heard.language, "words": heard.words }))
        })
    }

    fn interests(&self, caller: &str) -> BoxFuture<'static, Result<Value, String>> {
        let memory = self.memory.get().and_then(Weak::upgrade);
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let caller = caller.to_string();
        Box::pin(async move {
            let memory = memory.ok_or_else(|| crate::tr!("памяти здесь нет"))?;
            if !memory.enabled() {
                return Ok(json!({ "enabled": false, "interests": [] }));
            }
            let found = memory
                .recall(INTEREST_WORDS, &memory.viewer(), 12)
                .map_err(|error| crate::tr!("память: {error}", error = error))?;
            let interests: Vec<String> = found.iter().map(|fact| fact.fact.text.clone()).collect();
            if let Some(plugins) = &plugins {
                plugins.note_line(
                    &caller,
                    "info",
                    &crate::tr!("память: прочитаны интересы ({count})", count = interests.len()),
                );
            }
            Ok(json!({ "enabled": true, "interests": interests }))
        })
    }

    fn synth(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let voices = self.voices.get().cloned();
        let files = self.files.get().cloned();
        let plugins = self.plugins.get().and_then(Weak::upgrade);
        let recognizer = self.speech.get().and_then(Weak::upgrade).and_then(|speech| speech.recognizer());
        let (caller, action) = (caller.to_string(), action.to_string());
        Box::pin(async move {
            let voices = voices.ok_or_else(|| crate::tr!("голосов здесь нет"))?;
            if action == "voices" {
                return voices.all().await.map(Value::Array);
            }
            if action == "providers" {
                // An app sees who voices and whether it is ready, never anything of a key.
                let with_balance = input.get("balance").and_then(Value::as_bool) == Some(true);
                let mut cards = Vec::new();
                for card in voices.cards() {
                    let mut value = serde_json::to_value(&card).unwrap_or_default();
                    if let Some(fields) = value.as_object_mut() {
                        fields.remove("key");
                    }
                    if with_balance && card.ready {
                        if let Some(provider) = voices.get(card.id) {
                            match provider.balance().await {
                                Ok(Some(balance)) => value["balance"] = balance,
                                Ok(None) => {}
                                Err(error) => value["problem"] = json!(error),
                            }
                        }
                    }
                    cards.push(value);
                }
                return Ok(Value::Array(cards));
            }
            let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let text = field("text");
            if text.is_empty() || text.chars().count() > SYNTH_TEXT_MAX {
                return Err(crate::tr!("synth: строка от 1 до {max} знаков — text", max = SYNTH_TEXT_MAX));
            }
            let to = field("to");
            if to.is_empty() {
                return Err(crate::tr!(
                    "synth: куда положить — to, путь в своей папке, например voice/01 (расширение — по голосу)"
                ));
            }
            let chosen = Some(field("voice")).filter(|voice| !voice.is_empty()).unwrap_or(DEFAULT_VOICE.into());
            let (provider, own) =
                voices.route(&chosen).ok_or_else(|| crate::tr!("голоса {voice} здесь нет", voice = chosen))?;
            let card = provider.card();
            if !card.ready {
                return Err(card.needs.unwrap_or_else(|| crate::tr!("{title} не готов", title = card.title)));
            }
            // Out of the house or for money: only by the owner's «да» for this app (the bus asks once).
            if card.cloud || card.paid {
                let plugins = plugins.as_ref().ok_or_else(|| crate::tr!("приложений здесь нет"))?;
                plugins.may(&caller, crate::capabilities::voices::CLOUD, "synth")?;
            }
            let files = files.ok_or_else(|| crate::tr!("своей папки здесь нет"))?;
            let lang = Some(field("lang")).filter(|lang| !lang.is_empty());
            let cancel = CancellationToken::new();
            let _stop = cancel.clone().drop_guard();
            // The lines around it, for a provider that keeps the intonation (ElevenLabs); short, never the whole script.
            let around =
                |key: &str| Some(field(key).chars().take(1000).collect::<String>()).filter(|text| !text.is_empty());
            let line = crate::capabilities::voices::Line {
                voice: own.clone(),
                text: text.clone(),
                lang: lang.clone(),
                previous: around("previous"),
                next: around("next"),
            };
            let spoken = tokio::time::timeout(SYNTH_LIMIT, provider.synth(line, cancel.clone()))
                .await
                .map_err(|_| crate::tr!("голос не ответил за {seconds} с", seconds = SYNTH_LIMIT.as_secs()))??;
            let path = sound_path(&to, spoken.ext);
            let file = files.place(&caller, &path)?;
            // Whole or not at all: the window may be playing the old one.
            let part = file.with_extension(format!("{}.part", spoken.ext));
            std::fs::write(&part, &spoken.bytes).map_err(|error| format!("{}: {error}", part.display()))?;
            std::fs::rename(&part, &file).map_err(|error| format!("{}: {error}", file.display()))?;
            // The words' times: the provider's; where it has none and the app asks, the gateway's hearing.
            let mut words = spoken.words;
            let mut timing = (!words.is_empty()).then_some("voice");
            let mut timing_problem = None;
            if words.is_empty() && input.get("words").and_then(Value::as_bool) == Some(true) {
                match &recognizer {
                    Some(recognizer) => {
                        match crate::capabilities::hearing::recognize_file(recognizer, &file, lang.as_deref(), &cancel)
                            .await
                        {
                            Ok(heard) if !heard.words.is_empty() => {
                                words = heard.words;
                                timing = Some("hearing");
                            }
                            Ok(_) => timing_problem = Some(crate::tr!("слух не дал времени слов")),
                            Err(error) => timing_problem = Some(error),
                        }
                    }
                    None => timing_problem = Some(crate::tr!("слуха нет: распознавание речи в пульте выключено")),
                }
            }
            let seconds = match spoken.ext {
                "wav" => crate::speech::audio::pcm::decode_wav(&spoken.bytes)
                    .map(|decoded| decoded.pcm.len() as f64 / f64::from(decoded.sample_rate.max(1)))
                    .ok(),
                _ => words.last().map(|word| word.end),
            };
            let mut answer = match own.split_once('/') {
                Some((model, voice)) if provider.id() == crate::capabilities::voices::COMPUTER => {
                    crate::capabilities::voices::computer_card(model, voice)
                }
                _ => json!({ "id": chosen, "provider": card.id }),
            };
            answer["id"] = json!(chosen);
            answer["cloud"] = json!(card.cloud);
            answer["paid"] = json!(card.paid);
            answer["file"] = json!(path);
            answer["bytes"] = json!(spoken.bytes.len());
            answer["seconds"] = json!(seconds.map(|seconds| (seconds * 1000.0).round() / 1000.0));
            answer["timing"] = json!(timing);
            if !words.is_empty() {
                answer["words"] = json!(words);
            }
            if let Some(alignment) = spoken.alignment {
                answer["alignment"] = alignment;
            }
            if let Some(problem) = timing_problem {
                answer["timingProblem"] = json!(problem);
            }
            if let Some(plugins) = &plugins {
                let line = if card.paid {
                    crate::tr!(
                        "голос {voice} ({provider}) → {path}, платно: {count} знаков",
                        voice = chosen,
                        provider = card.title,
                        path = path,
                        count = text.chars().count()
                    )
                } else {
                    crate::tr!(
                        "голос {voice} ({provider}) → {path}",
                        voice = chosen,
                        provider = card.title,
                        path = path
                    )
                };
                plugins.note_line(&caller, "info", &line);
            }
            Ok(answer)
        })
    }

    fn mic(&self, caller: &str) -> Result<Value, String> {
        let activator =
            self.activator.get().ok_or_else(|| crate::tr!("микрофон колонки слышит только шлюз на самой колонке"))?;
        tracing::info!(target: "plugins", "{caller}: opens the station's microphone");
        activator.mic()
    }

    fn wake(&self, caller: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let activator = self.activator.get().cloned();
        let words = self
            .plugins
            .get()
            .and_then(Weak::upgrade)
            .and_then(|host| host.installed(caller))
            .map(|package| package.manifest.contributes.activators.clone())
            .unwrap_or_default();
        let caller = caller.to_string();
        Box::pin(async move {
            let activator = activator.ok_or_else(|| crate::tr!("будить колонку может только шлюз на самой колонке"))?;
            activator.wake(&caller, &words, &input).await
        })
    }

    fn say(&self, caller: &str, text: String, device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let speaker = self.skills.upgrade().and_then(|skills| skills.speaker());
        let caller = caller.to_string();
        Box::pin(async move {
            let text: String = text.trim().chars().take(500).collect();
            if text.is_empty() {
                return Err(crate::tr!("нечего сказать"));
            }
            // No station on the line, or no station link at all: nothing is said, and the plugin is told so.
            let Some(speaker) = speaker else { return Ok(false) };
            tracing::info!(target: "plugins", "{caller}: says «{text}»");
            let device = device.filter(|device| !device.is_empty()).unwrap_or_else(|| "*".into());
            Ok(speaker.say(device, text, None).await)
        })
    }

    fn playing(
        &self,
        caller: &str,
        state: String,
        device: Option<String>,
        track: Option<crate::skills::media::app_playback::AppTrack>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let answer = match self.skills.upgrade() {
            Some(skills) => skills.app_playing_track(caller, state.trim(), device.as_deref(), track),
            None => Err(crate::tr!("плеера колонки здесь нет")),
        };
        Box::pin(async move { answer })
    }

    fn notify(&self, caller: &str, text: String) -> BoxFuture<'static, Result<Value, String>> {
        let (schedules, caller) = (self.schedules.get().and_then(Weak::upgrade), caller.to_string());
        Box::pin(async move {
            let schedules = schedules.ok_or_else(|| crate::tr!("сообщений владельцу здесь нет").to_string())?;
            schedules.notify(&caller, &text).await.map(|()| Value::Bool(true))
        })
    }

    fn tasks(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let tasks = self.tasks.get().and_then(Weak::upgrade);
        let host = self.plugins.get().and_then(Weak::upgrade);
        let (app, action) = (caller.to_string(), action.to_string());
        Box::pin(async move {
            let (Some(tasks), Some(host)) = (tasks, host) else {
                return Err(crate::tr!("долгих задач здесь нет"));
            };
            let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
            let id = text("id").unwrap_or_default();
            let failed = |error: crate::tasks::TaskError| error.to_string();
            match action.as_str() {
                "start" => {
                    let step = match text("step") {
                        Some(step) => Some(crate::plugins::task_pump::step_name(&step).ok_or_else(|| {
                            crate::tr!("host.tasks.start: step — имя экспортированной функции приложения")
                        })?),
                        None => None,
                    };
                    let title = text("title").unwrap_or_default();
                    let percent = input.get("percent").and_then(Value::as_f64);
                    let at_once = host.tasks_at_once(&app);
                    let task = tasks.start(&app, &title, text("text").as_deref(), percent, at_once).map_err(failed)?;
                    if let Some(step) = step {
                        let state = input.get("state").cloned().unwrap_or(Value::Null);
                        crate::plugins::task_pump::pump(host, tasks.clone(), app, task.id.clone(), step, state);
                    }
                    Ok(serde_json::json!({ "id": task.id, "title": task.title }))
                }
                "progress" => {
                    let percent = input.get("percent").and_then(Value::as_f64);
                    let cancel = tasks.progress(&app, &id, percent, text("text").as_deref()).map_err(failed)?;
                    Ok(serde_json::json!({ "cancel": cancel }))
                }
                "done" => {
                    let result = input.get("result").cloned().filter(|result| !result.is_null());
                    let task = tasks.finish(&app, &id, crate::tasks::Outcome::Done(result)).map_err(failed)?;
                    Ok(serde_json::to_value(task).unwrap_or_default())
                }
                "fail" => {
                    let why = text("error").unwrap_or_default();
                    let task = tasks.finish(&app, &id, crate::tasks::Outcome::Failed(why)).map_err(failed)?;
                    Ok(serde_json::to_value(task).unwrap_or_default())
                }
                "get" => match tasks.get(&id).filter(|log| log.task.app == app) {
                    Some(log) => Ok(serde_json::to_value(log.task).unwrap_or_default()),
                    None => Err(failed(crate::tasks::TaskError::NotFound)),
                },
                other => Err(crate::tr!("host.tasks: нет действия {action}", action = other)),
            }
        })
    }

    fn schedule(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let answer = match self.schedules.get().and_then(Weak::upgrade) {
            Some(schedules) => schedules.code(caller, action, input),
            None => Err(crate::tr!("расписаний здесь нет")),
        };
        Box::pin(async move { answer })
    }

    fn bus(
        &self,
        caller: &str,
        name: &str,
        input: Value,
        trail: Option<Trail>,
    ) -> Option<BoxFuture<'static, Result<Value, String>>> {
        let bus = self.bus.get().and_then(Weak::upgrade)?;
        Some(crate::bus::letter(bus, caller, name, input, trail))
    }
}

/// The plugins' messages to the owner (plugins/schedule.rs, right `notify`) by the station watch's ways.
pub struct OwnerAlerts(pub Arc<crate::operations::StationWatch>);

impl crate::plugins::schedule::OwnerNotice for OwnerAlerts {
    fn ready(&self) -> Result<(), String> {
        if self.0.can_tell() {
            Ok(())
        } else {
            Err(crate::tr!(
                "некуда прислать: выберите чат «Оповещения» в подключении Telegram или задайте ALERT_WEBHOOK_URL"
            ))
        }
    }

    fn tell(&self, text: String) -> BoxFuture<'static, ()> {
        let watch = self.0.clone();
        Box::pin(async move { watch.tell(&text).await })
    }
}

/// The gateway's file tools of an app's agent: its own folder only.
pub(crate) fn agent_file_tools() -> Vec<crate::dialog::llm::LlmTool> {
    let tool = |name: &str, description: &str, parameters: Value| crate::dialog::llm::LlmTool {
        name: name.into(),
        description: description.into(),
        parameters,
    };
    vec![
        tool(
            "list_files",
            "Файлы и папки в папке приложения (path — подпапка, пусто — сама папка).",
            json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        ),
        tool(
            "read_file",
            "Текст файла папки приложения, до 64 КБ.",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        ),
        tool(
            "write_file",
            "Записать текстовый файл в папку приложения целиком (до 256 КБ); папки на пути создаются.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "text": { "type": "string" } },
                "required": ["path", "text"]
            }),
        ),
    ]
}

pub(crate) fn agent_list(files: &crate::plugins::files::PluginFiles, app: &str, path: &str) -> Result<String, String> {
    let dir = if path.trim_matches('/').is_empty() { files.root().join(app) } else { files.inside(app, path)? };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|_| format!("нет папки {path}"))?
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => format!("{name}/"),
                Ok(meta) => format!("{name} ({} Б)", meta.len()),
                Err(_) => name,
            }
        })
        .collect();
    names.sort();
    Ok(if names.is_empty() { "пусто".into() } else { names.join("\n") })
}

pub(crate) fn agent_read(files: &crate::plugins::files::PluginFiles, app: &str, path: &str) -> Result<String, String> {
    let file = files.inside(app, path)?;
    let bytes = std::fs::read(&file).map_err(|_| format!("нет файла {path}"))?;
    if bytes.len() > AGENT_READ_MAX {
        return Err(format!("{path}: больше 64 КБ"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{path}: не текст"))
}

pub(crate) fn agent_write(
    files: &crate::plugins::files::PluginFiles,
    app: &str,
    path: &str,
    text: &str,
) -> Result<String, String> {
    if text.len() > AGENT_WRITE_MAX {
        return Err(format!("{path}: больше 256 КБ"));
    }
    let file = files.place(app, path)?;
    std::fs::write(&file, text).map_err(|error| format!("{path}: {error}"))?;
    Ok(format!("записано: {path}, {} Б", text.len()))
}

/// The model a letter asks for: `"fast"` / `"smart"` — the household's profile; `{provider, model}` — a model of the
/// panel chosen by the app itself (its provider gone — the profile answers).
fn model_choice(
    input: &Value,
    default: crate::config::ModelProfile,
) -> Result<(crate::config::ModelProfile, Option<crate::settings::definitions::BrainPick>), String> {
    match input.get("model") {
        None | Some(Value::Null) => Ok((default, None)),
        Some(Value::String(name)) => match name.trim() {
            "" => Ok((default, None)),
            "fast" => Ok((crate::config::ModelProfile::Fast, None)),
            "smart" => Ok((crate::config::ModelProfile::Smart, None)),
            other => Err(format!("модель «{other}» — есть fast («Быстрая»), smart («Умная») или {{provider, model}}")),
        },
        Some(Value::Object(pick)) => {
            let text = |key: &str| pick.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let (provider, model) = (text("provider"), text("model"));
            if provider.is_empty() || model.is_empty() {
                return Err("модель: {provider, model} — оба поля".into());
            }
            Ok((default, Some(crate::settings::definitions::BrainPick { provider, model })))
        }
        Some(_) => Err("модель: строка fast/smart или {provider, model}".into()),
    }
}

#[cfg(test)]
mod model_and_voice_tests {
    use super::*;

    #[test]
    fn finds_the_json_in_a_models_answer_and_says_which_voices_may_be_published() {
        assert_eq!(json_in("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(json_in("```json\n[1, 2]\n```"), Some(json!([1, 2])));
        assert_eq!(json_in("Вот серия: {\"title\": \"Компас\"} — готово."), Some(json!({"title": "Компас"})));
        assert_eq!(json_in("нет тут ничего"), None);
        assert_eq!(crate::capabilities::voices::computer_card("piper", "denis")["publish"], json!(true));
        assert_eq!(crate::capabilities::voices::computer_card("piper", "dmitri")["license"], json!("CC0"));
        assert_eq!(crate::capabilities::voices::computer_card("piper", "ruslan")["publish"], json!(false));
        assert_eq!(crate::capabilities::voices::computer_card("silero", "baya")["license"], json!("CC BY-NC 4.0"));
        assert_eq!(
            crate::capabilities::voices::computer_card("piper", "irina")["publish"],
            json!(false),
            "a licence not found out is for oneself"
        );
    }

    #[test]
    fn an_app_names_a_profile_or_a_model_of_the_panel() {
        use crate::config::ModelProfile;
        let pick = |input: Value| model_choice(&input, ModelProfile::Fast);
        assert!(matches!(pick(json!({})), Ok((ModelProfile::Fast, None))));
        assert!(matches!(pick(json!({"model": null})), Ok((ModelProfile::Fast, None))));
        assert!(matches!(pick(json!({"model": "smart"})), Ok((ModelProfile::Smart, None))));
        let (profile, chosen) = pick(json!({"model": {"provider": "openai", "model": "gpt-5"}})).unwrap();
        assert!(
            matches!(profile, ModelProfile::Fast),
            "a model of its own keeps the default profile for a gone provider"
        );
        let chosen = chosen.unwrap();
        assert_eq!((chosen.provider.as_str(), chosen.model.as_str()), ("openai", "gpt-5"));
        assert!(pick(json!({"model": "best"})).is_err());
        assert!(pick(json!({"model": {"provider": "openai"}})).is_err());
        assert!(pick(json!({"model": 5})).is_err());
    }

    #[test]
    fn the_agents_file_tools_stay_inside_the_apps_folder() {
        let dir = tempfile::tempdir().unwrap();
        let files = crate::plugins::files::PluginFiles::new(dir.path().to_path_buf(), Arc::new(|| false));
        assert!(agent_write(&files, "feed", "episodes/01/plan.json", "{\"shots\": 3}")
            .unwrap()
            .starts_with("записано"));
        assert_eq!(agent_read(&files, "feed", "episodes/01/plan.json").unwrap(), "{\"shots\": 3}");
        assert_eq!(agent_list(&files, "feed", "").unwrap(), "episodes/");
        assert!(agent_list(&files, "feed", "episodes/01").unwrap().starts_with("plan.json ("));
        for bad in ["../other/secret.txt", ".hidden", "episodes/../../x", ""] {
            assert!(agent_write(&files, "feed", bad, "x").is_err(), "{bad}");
            assert!(agent_read(&files, "feed", bad).is_err(), "{bad}");
        }
        assert!(!dir.path().join("other").exists());
        assert!(agent_write(&files, "feed", "big.txt", &"x".repeat(AGENT_WRITE_MAX + 1)).is_err());
    }
}
