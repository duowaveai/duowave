//! The settings of «Связи» beside the types they keep (docs/plans/bus.md): the links, the modes, the switches and
//! the owner's words on dangerous handles. Read and written through the SettingsService, as every setting
//! (src/settings/definitions.rs).

use crate::core::settings::{SettingDefinition, SettingScope};

/// The bus's links (docs/plans/bus.md): who reaches whom and how far. Like every setting not in
/// `LOCAL_SETTINGS` they travel with the settings package, so a move keeps them.
pub const BUS_LINKS: SettingDefinition<Vec<super::links::Link>> = SettingDefinition {
    key: "bus.links",
    scope: SettingScope::Global,
    description: "Связи приложений",
    sensitive: false,
    parse: super::links::parse_links,
};

/// The house's mode and each participant's own: «Отдельно», «По правилам», «Свободно».
pub const BUS_MODES: SettingDefinition<super::links::Modes> = SettingDefinition {
    key: "bus.modes",
    scope: SettingScope::Global,
    description: "Режимы связей",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The bus's switches: asking in words through a cloud model without a person (off).
pub const BUS_OPTIONS: SettingDefinition<super::links::Options> = SettingDefinition {
    key: "bus.options",
    scope: SettingScope::Global,
    description: "Связи: настройки",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The owner's words on dangerous handles: «всегда можно», a mark taken off or put on.
pub const BUS_DANGERS: SettingDefinition<Vec<super::danger::DangerWord>> = SettingDefinition {
    key: "bus.dangers",
    scope: SettingScope::Global,
    description: "Опасные действия",
    sensitive: false,
    parse: |value| {
        let list = value.as_array()?;
        Some(list.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
    },
};
