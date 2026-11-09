//! Home Assistant over its REST API (its address and a long-lived token, set up in the panel): entities of the
//! usual domains as devices, their friendly names to match, services to act.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::provider::{HomeAction, HomeDevice, SmartHomeProvider};
use crate::skills::js;
use crate::skills::lookups::http::{get_json, Http, HttpGet, HttpPost};

const DOMAINS: &[&str] = &[
    "light",
    "switch",
    "fan",
    "climate",
    "cover",
    "scene",
    "script",
    "media_player",
    "input_boolean",
    "lock",
    "vacuum",
    "humidifier",
];

/// Which Home Assistant service an action is, per domain.
fn service(domain: &str, action: HomeAction) -> Option<&'static str> {
    match action {
        HomeAction::Scene => return matches!(domain, "scene" | "script").then_some("turn_on"),
        HomeAction::SetBrightness => return (domain == "light").then_some("turn_on"),
        HomeAction::SetTemperature => return (domain == "climate").then_some("set_temperature"),
        HomeAction::On | HomeAction::Off => {}
    }
    let on = action == HomeAction::On;
    Some(match domain {
        "cover" => {
            if on {
                "open_cover"
            } else {
                "close_cover"
            }
        }
        "lock" => {
            if on {
                "lock"
            } else {
                "unlock"
            }
        }
        "vacuum" => {
            if on {
                "start"
            } else {
                "return_to_base"
            }
        }
        "scene" | "script" => return on.then_some("turn_on"),
        _ => {
            if on {
                "turn_on"
            } else {
                "turn_off"
            }
        }
    })
}

pub struct HomeAssistantProvider {
    base: String,
    token: String,
    get: Arc<dyn HttpGet>,
    post: Arc<dyn HttpPost>,
}

impl HomeAssistantProvider {
    pub fn new(url: &str, token: &str, get: Option<Arc<dyn HttpGet>>, post: Option<Arc<dyn HttpPost>>) -> Self {
        HomeAssistantProvider {
            base: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            get: get.unwrap_or_else(|| Arc::new(Http)),
            post: post.unwrap_or_else(|| Arc::new(Http)),
        }
    }

    fn headers(&self) -> Vec<(String, String)> {
        vec![("Authorization".into(), format!("Bearer {}", self.token))]
    }

    async fn states(&self, cancel: &CancellationToken) -> anyhow::Result<Vec<HomeDevice>> {
        let states = get_json(self.get.as_ref(), &format!("{}/api/states", self.base), cancel, self.headers()).await?;
        let Value::Array(states) = states else { anyhow::bail!("Home Assistant: the states are not a list") };
        Ok(states
            .iter()
            .filter_map(|state| {
                let id = state.get("entity_id")?.as_str()?;
                let domain = id.split('.').next().unwrap_or("");
                if !DOMAINS.contains(&domain) {
                    return None;
                }
                let attributes = state.get("attributes");
                let attribute = |key: &str| attributes.and_then(|attributes| attributes.get(key));
                let mut details = Vec::new();
                if let Some(brightness) = attribute("brightness") {
                    let percent = js::round(js::to_number(Some(brightness)) / 255.0 * 100.0);
                    details.push(("brightnessPercent".to_string(), js::number_value(percent)));
                }
                if let Some(temperature) = attribute("temperature") {
                    details.push(("targetTemperature".to_string(), temperature.clone()));
                }
                if let Some(current) = attribute("current_temperature") {
                    details.push(("temperature".to_string(), current.clone()));
                }
                Some(HomeDevice {
                    id: id.to_string(),
                    name: attribute("friendly_name").and_then(Value::as_str).unwrap_or(id).to_string(),
                    kind: domain.to_string(),
                    state: state.get("state").map(js::string_of),
                    details,
                })
            })
            .collect())
    }

    async fn call(
        &self,
        device: &HomeDevice,
        action: HomeAction,
        value: Option<f64>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<()> {
        let Some(name) = service(&device.kind, action) else {
            anyhow::bail!("{} ({}) cannot {}", device.name, device.kind, action.as_str());
        };
        let mut body = json!({ "entity_id": device.id });
        if let (HomeAction::SetBrightness, Some(value)) = (action, value) {
            body["brightness_pct"] = js::number_value(js::round(value).clamp(0.0, 100.0));
        }
        if let (HomeAction::SetTemperature, Some(value)) = (action, value) {
            body["temperature"] = js::number_value(value);
        }
        let mut headers = self.headers();
        headers.push(("Content-Type".into(), "application/json".into()));
        let url = format!("{}/api/services/{}/{name}", self.base, device.kind);
        let status = self.post.post(url, body, headers, cancel.clone()).await?;
        if !(200..300).contains(&status) {
            anyhow::bail!("Home Assistant answered HTTP {status}");
        }
        Ok(())
    }
}

impl SmartHomeProvider for HomeAssistantProvider {
    fn name(&self) -> &str {
        "Home Assistant"
    }

    fn devices<'a>(&'a self, cancel: &'a CancellationToken) -> BoxFuture<'a, anyhow::Result<Vec<HomeDevice>>> {
        Box::pin(self.states(cancel))
    }

    fn act<'a>(
        &'a self,
        device: &'a HomeDevice,
        action: HomeAction,
        value: Option<f64>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(self.call(device, action, value, cancel))
    }
}
