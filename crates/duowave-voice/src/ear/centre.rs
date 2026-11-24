//! Which centre an ear is the ear of (docs/plans/home-and-devices.md, «Ухо»): where the centre listens and its pair
//! key. An ear's own setting (`transfer::LOCAL_SETTINGS`): the station's socket goes on to the centre
//! (src/gateway/uniproxy/ear_bridge.rs), the home's copy comes from it (src/home/ear.rs).

use serde::{Deserialize, Serialize};

use crate::core::settings::{SettingDefinition, SettingScope};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;

/// This gateway's centre, when it is an ear: where the centre listens. Its pair key is a secret (`KEY_SECRET`). Each
/// device's own (`transfer::LOCAL_SETTINGS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EarOf {
    pub address: String,
    pub port: u16,
    #[serde(default)]
    pub name: Option<String>,
    pub since: String,
}

pub const HOME_EAR: SettingDefinition<EarOf> = SettingDefinition {
    key: "home_ear",
    scope: SettingScope::Global,
    description: "Хаб дома",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The centre's pair key, kept by its ear. Not among the travelling prefixes.
pub const KEY_SECRET: &str = "earof:key";
/// An ear's pair key, kept by its centre to ask it to become the centre (`make_centre`).
pub fn ear_key_secret(device: &str) -> String {
    format!("earkey:{device}")
}

/// The centre this gateway is the ear of, with its pair key.
pub fn centre(settings: &SettingsService, secrets: &SecretsService) -> Option<(EarOf, String)> {
    let ear = settings.get(&HOME_EAR, None)?;
    let key = secrets.get(KEY_SECRET)?;
    Some((ear, key))
}

/// The header an ear opens the centre's voice socket with: the centre's pair key, so the centre lets in the station
/// of its own ear as one it invited (the join was the admin's approval: docs/plans/home-setup-flow.md, stage 5). The
/// station's own Authorization goes on untouched — Yandex's, for the mode «Алиса». The centre takes it off before
/// anything goes on to Yandex.
pub const EAR_KEY_HEADER: &str = "x-duowave-ear-key";
