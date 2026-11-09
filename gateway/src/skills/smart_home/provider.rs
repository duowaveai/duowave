//! A smart home system: Home Assistant first, others (open or proprietary) by the same interface. Device names are
//! the system's own, so the user's words are matched against them.

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// A device of a smart home system, as the provider names it.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeDevice {
    pub id: String,
    pub name: String,
    /// light, switch, climate, cover, scene, …
    pub kind: String,
    /// "on", "off", "21.5", …
    pub state: Option<String>,
    /// Brightness in percent, target temperature and the like, for answers about the state; in this order.
    pub details: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeAction {
    On,
    Off,
    SetBrightness,
    SetTemperature,
    Scene,
}

impl HomeAction {
    pub fn parse(action: &str) -> Option<HomeAction> {
        Some(match action {
            "on" => HomeAction::On,
            "off" => HomeAction::Off,
            "set_brightness" => HomeAction::SetBrightness,
            "set_temperature" => HomeAction::SetTemperature,
            "scene" => HomeAction::Scene,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HomeAction::On => "on",
            HomeAction::Off => "off",
            HomeAction::SetBrightness => "set_brightness",
            HomeAction::SetTemperature => "set_temperature",
            HomeAction::Scene => "scene",
        }
    }
}

pub trait SmartHomeProvider: Send + Sync {
    fn name(&self) -> &str;
    fn devices<'a>(&'a self, cancel: &'a CancellationToken) -> BoxFuture<'a, anyhow::Result<Vec<HomeDevice>>>;
    fn act<'a>(
        &'a self,
        device: &'a HomeDevice,
        action: HomeAction,
        value: Option<f64>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}
