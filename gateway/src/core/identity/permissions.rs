//! What a person, a key or a voice may do. Two families: the panel's and the API's actions (`station.mode`,
//! `models.manage`, …) and the use of Alice's capabilities by voice or by the model's tools (`use:music`). A grant
//! may end in `*`: `*` is everything, `use:*` every capability. Pure data: the panel, the HTTP checks, the router
//! and the model's tools read the same catalog.

use crate::skills::capabilities::{CapabilityName, CAPABILITY_NAMES};

/// The panel's and the API's permissions, with their titles for people.
pub const PERMISSIONS: &[(&str, &str)] = &[
    ("station.view", "Смотреть колонку: состояние, сессии, маршруты, модели, характер"),
    ("station.say", "Говорить через колонку"),
    ("station.mode", "Переключать режим: Яндекс, Шлюз, Свой"),
    ("routes.edit", "Менять маршруты возможностей"),
    ("models.manage", "Скачивать, удалять и выбирать голоса и распознавание, перезапускать их серверы"),
    ("persona.edit", "Менять характер Алисы"),
    ("journal.view", "Читать журнал запросов: что говорили колонке"),
    ("users.manage", "Добавлять людей, менять их роли, права и ключи доступа"),
    ("stations.manage", "Подключать колонки, давать им имена, отключать чужие"),
    ("connectors.view", "Смотреть подключения"),
    ("connectors.manage", "Подключать и настраивать подключения"),
    ("mcp.use", "Пользоваться инструментами MCP-серверов: голосом и в проверочном чате"),
    ("plugins.manage", "Включать, выключать и перезагружать приложения, менять их настройки"),
    (
        "plugins.mcp",
        "Звать инструменты включённых приложений как MCP-сервер шлюза: так их берут Claude Code и другие агенты",
    ),
    ("settings.edit", "Общие настройки шлюза"),
    ("audit.view", "Смотреть, кто что менял"),
    ("memory.view", "Смотреть память: что помнят персонажи о доме"),
    ("memory.manage", "Менять память: править и стирать записи, включать и выключать её"),
    ("system.update", "Ставить обновления шлюза"),
    (
        "home.models",
        "Пользоваться моделями этого компьютера — распознаванием, голосом, моделью: так шлюз на колонке берёт их у домашнего сервера",
    ),
];

pub const STATION_VIEW: &str = "station.view";
pub const STATION_SAY: &str = "station.say";
pub const STATION_MODE: &str = "station.mode";
pub const ROUTES_EDIT: &str = "routes.edit";
pub const MODELS_MANAGE: &str = "models.manage";
pub const PERSONA_EDIT: &str = "persona.edit";
pub const JOURNAL_VIEW: &str = "journal.view";
pub const USERS_MANAGE: &str = "users.manage";
pub const STATIONS_MANAGE: &str = "stations.manage";
pub const CONNECTORS_VIEW: &str = "connectors.view";
pub const CONNECTORS_MANAGE: &str = "connectors.manage";
pub const MCP_USE: &str = "mcp.use";
pub const PLUGINS_MANAGE: &str = "plugins.manage";
pub const PLUGINS_MCP: &str = "plugins.mcp";
pub const SETTINGS_EDIT: &str = "settings.edit";
pub const AUDIT_VIEW: &str = "audit.view";
pub const MEMORY_VIEW: &str = "memory.view";
pub const MEMORY_MANAGE: &str = "memory.manage";
pub const SYSTEM_UPDATE: &str = "system.update";
pub const HOME_MODELS: &str = "home.models";

/// The use of a capability: by voice through the router, or as the model's tool.
pub fn use_of(capability: CapabilityName) -> String {
    format!("use:{}", capability.as_str())
}

pub fn is_panel_permission(value: &str) -> bool {
    PERMISSIONS.iter().any(|(name, _)| *name == value)
}

/// The title of a panel permission, for people; the permission itself when it has none.
pub fn title_of(permission: &str) -> &str {
    PERMISSIONS.iter().find(|(name, _)| *name == permission).map(|(_, title)| *title).unwrap_or(permission)
}

/// Whether a string is a grant the catalog knows: a permission, `*`, `use:*`, `<group>.*` or `use:<capability>`.
pub fn is_grant(value: &str) -> bool {
    if value == "*" || value == "use:*" || is_panel_permission(value) {
        return true;
    }
    if let Some(capability) = value.strip_prefix("use:") {
        return CAPABILITY_NAMES.iter().any(|name| name.as_str() == capability);
    }
    if let Some(group) = value.strip_suffix('*') {
        return value.ends_with(".*") && PERMISSIONS.iter().any(|(name, _)| name.starts_with(group));
    }
    false
}

/// Whether a grant covers a permission: equal, or a `*` pattern over it.
pub fn covers(grant: &str, permission: &str) -> bool {
    if grant == permission || grant == "*" {
        return true;
    }
    grant.strip_suffix('*').is_some_and(|prefix| permission.starts_with(prefix))
}

/// Every permission, for the panel's lists: the panel's own first, then the capabilities.
pub fn all_permissions() -> Vec<String> {
    PERMISSIONS
        .iter()
        .map(|(name, _)| name.to_string())
        .chain(CAPABILITY_NAMES.iter().map(|name| use_of(*name)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_and_patterns() {
        assert!(is_grant("*"));
        assert!(is_grant("use:*"));
        assert!(is_grant("use:music"));
        assert!(!is_grant("use:flying"));
        assert!(is_grant("station.*"));
        assert!(!is_grant("nothing.*"));
        assert!(!is_grant("station.fly"));
        assert!(covers("*", "users.manage"));
        assert!(covers("use:*", "use:music"));
        assert!(covers("station.*", "station.say"));
        assert!(!covers("station.*", "stations.manage"));
        assert!(!covers("use:music", "use:radio"));
    }
}
