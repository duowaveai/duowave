//! The gateway's services as the plugins reach them through `host.call` (Host API 1.1, stage 10 of
//! docs/plans/plugins.md): the household's own music and tales on the disk, other plugins' tools, the smart home
//! through the panel's connector (its keys stay here), the weather's usual place and the news feeds the panel chose.
//! What the built-in skills used, so the skills themselves can be plugins.

use std::sync::{Arc, OnceLock, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::core::connectors::ToolContext;
use crate::core::identity::principal::system;
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
    /// The plugins' schedules, once they stand: `host.schedule.*`.
    schedules: OnceLock<Weak<crate::plugins::schedule::Scheduler>>,
    /// The plugins' pictures: `host.files.*`.
    files: OnceLock<Arc<crate::plugins::files::PluginFiles>>,
}

impl PluginServices {
    pub fn new(skills: &Arc<SkillsService>, connectors: Arc<dyn SkillConnectors>) -> Arc<PluginServices> {
        Arc::new(PluginServices {
            skills: Arc::downgrade(skills),
            connectors,
            plugins: OnceLock::new(),
            schedules: OnceLock::new(),
            files: OnceLock::new(),
        })
    }

    /// The plugin host, once it stands (it is built with these services).
    pub fn set_plugins(&self, plugins: &Arc<PluginHost>) {
        let _ = self.plugins.set(Arc::downgrade(plugins));
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

    fn call_tool(
        &self,
        caller: &str,
        name: &str,
        input: Map<String, Value>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let tool = self
            .plugins
            .get()
            .and_then(Weak::upgrade)
            .and_then(|plugins| plugins.tool_contributions().into_iter().find(|tool| tool.tool.name == name));
        let (caller, name) = (caller.to_string(), name.to_string());
        Box::pin(async move {
            let tool = tool.ok_or_else(|| format!("нет инструмента {name}: его плагин выключен или его нет"))?;
            tracing::debug!(target: "plugins", "{caller}: calls the tool {name}");
            let context = ToolContext { principal: system(), cancel: CancellationToken::new(), station: None };
            (tool.run)(input, context).await.map_err(|error| error.to_string())
        })
    }

    fn smart_home_devices(&self) -> BoxFuture<'static, Result<Value, String>> {
        let provider = self.provider();
        Box::pin(async move {
            let provider =
                provider.ok_or_else(|| "умный дом не подключён («Приложения» → «Подключения»)".to_string())?;
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
            let provider =
                provider.ok_or_else(|| "умный дом не подключён («Приложения» → «Подключения»)".to_string())?;
            let action = HomeAction::parse(&action)
                .ok_or_else(|| format!("действие «{action}»: on, off, set_brightness, set_temperature, scene"))?;
            let cancel = CancellationToken::new();
            let devices = provider.devices(&cancel).await.map_err(|error| error.to_string())?;
            let found =
                devices.iter().find(|known| known.id == device).ok_or_else(|| format!("нет устройства {device}"))?;
            provider.act(found, action, value, &cancel).await.map_err(|error| error.to_string())?;
            Ok(format!("{}: {}", found.name, action.as_str()))
        })
    }

    fn weather_place(&self) -> Option<String> {
        self.connectors.weather_place()
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
            (None, _) => Err("картинок здесь нет".to_string()),
            (Some(files), "put") => {
                let room = self
                    .plugins
                    .get()
                    .and_then(Weak::upgrade)
                    .and_then(|plugins| plugins.package(caller))
                    .map_or(1, |package| package.manifest.budget.storage_mb);
                let name = input.get("name").and_then(Value::as_str).unwrap_or_default();
                files.put(caller, name, &bytes.unwrap_or_default(), room)
            }
            (Some(files), "list") => Ok(Value::Array(files.list(caller))),
            (Some(files), "delete") => {
                Ok(Value::Bool(files.delete(caller, input.get("name").and_then(Value::as_str).unwrap_or_default())))
            }
            (Some(_), other) => Err(format!("нет функции files.{other}")),
        };
        Box::pin(async move { answer })
    }

    fn notify(&self, caller: &str, text: String) -> BoxFuture<'static, Result<Value, String>> {
        let (schedules, caller) = (self.schedules.get().and_then(Weak::upgrade), caller.to_string());
        Box::pin(async move {
            let schedules = schedules.ok_or_else(|| "сообщений владельцу здесь нет".to_string())?;
            schedules.notify(&caller, &text).await.map(|()| Value::Bool(true))
        })
    }

    fn schedule(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let answer = match self.schedules.get().and_then(Weak::upgrade) {
            Some(schedules) => schedules.code(caller, action, input),
            None => Err("расписаний здесь нет".into()),
        };
        Box::pin(async move { answer })
    }
}

/// The plugins' messages to the owner (plugins/schedule.rs, right `notify`) by the station watch's ways.
pub struct OwnerAlerts(pub Arc<crate::operations::StationWatch>);

impl crate::plugins::schedule::OwnerNotice for OwnerAlerts {
    fn ready(&self) -> Result<(), String> {
        if self.0.can_tell() {
            Ok(())
        } else {
            Err("некуда прислать: выберите чат «Оповещения» в подключении Telegram или задайте ALERT_WEBHOOK_URL"
                .into())
        }
    }

    fn tell(&self, text: String) -> BoxFuture<'static, ()> {
        let watch = self.0.clone();
        Box::pin(async move { watch.tell(&text).await })
    }
}
