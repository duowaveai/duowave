//! The settings of «Выход в интернет» (docs/plans/own-server.md), beside the types they keep: the mode, the pick,
//! the connections, each row's route and what the home learned of it. Read and written through the SettingsService,
//! as every setting (src/settings/definitions.rs).

use serde_json::Value;

use crate::core::settings::{as_one_of, as_string, as_string_list, SettingDefinition, SettingScope};

/// «Выход в интернет» (src/egress, docs/plans/own-server.md): the mode the routes are decided by — `off`, `blocked`
/// («Где не работает», the default), `all`, `only` («Только выбранные»). Since 16.11 it follows the switch, the pick
/// and «напрямую то, что работает из страны дома» below; an older panel's `mode` still sets it as it did.
pub const EGRESS_MODE: SettingDefinition<String> = SettingDefinition {
    key: "egress.mode",
    scope: SettingScope::Global,
    description: "Выход в интернет: простой режим",
    sensitive: false,
    parse: |value| as_one_of(value, &["off", "blocked", "all", "only"]),
};

/// «Режим VPN для DuoWave»: `all` — everyone through the server, `except` — everyone but the rows ticked (their route
/// `direct`), `only` — only the rows ticked (their route `server` or a connection). Kept while the switch is off.
pub const EGRESS_PICK: SettingDefinition<String> = SettingDefinition {
    key: "egress.pick",
    scope: SettingScope::Global,
    description: "Выход в интернет: кто идёт через сервер",
    sensitive: false,
    parse: |value| as_one_of(value, &["all", "except", "only"]),
};

/// «Открывать напрямую то, что работает из страны дома»: on — `blocked`, off — `all`. Kept while the switch is off.
pub const EGRESS_HOME_DIRECT: SettingDefinition<bool> = SettingDefinition {
    key: "egress.home_direct",
    scope: SettingScope::Global,
    description: "Выход в интернет: работающее из страны дома — напрямую",
    sensitive: false,
    parse: Value::as_bool,
};

/// «Если VPN не отвечает», for every row without its own word: `safe` — keyed rows and apps wait, Alice goes directly
/// (the default), `wait` — everyone waits, `direct` — everyone goes directly.
pub const EGRESS_ON_DOWN_ALL: SettingDefinition<String> = SettingDefinition {
    key: "egress.on_down_all",
    scope: SettingScope::Global,
    description: "Выход в интернет: если VPN не отвечает",
    sensitive: false,
    parse: |value| as_one_of(value, &["safe", "wait", "direct"]),
};

/// The connection «Всё через сервер» and «Авто» go through first; none — the first that answers.
pub const EGRESS_CONNECTION: SettingDefinition<String> = SettingDefinition {
    key: "egress.connection",
    scope: SettingScope::Global,
    description: "Выход в интернет: подключение по умолчанию",
    sensitive: false,
    parse: as_string,
};

/// The addresses the person typed («Свои адреса»): host names, `*.example.com` for subdomains.
pub const EGRESS_CUSTOM: SettingDefinition<Vec<String>> = SettingDefinition {
    key: "egress.custom",
    scope: SettingScope::Global,
    description: "Выход в интернет: свои адреса",
    sensitive: false,
    parse: as_string_list,
};

/// The connections last reported, without their entries: after a restart the rows that wait for a server keep
/// waiting until the app speaks again (src/egress/connections.rs).
pub const EGRESS_CONNECTIONS: SettingDefinition<Vec<super::connections::Kept>> = SettingDefinition {
    key: "egress.connections",
    scope: SettingScope::Global,
    description: "Выход в интернет: известные подключения",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The rows the person has seen in the table: the others are marked «новое».
pub const EGRESS_SEEN: SettingDefinition<Vec<String>> = SettingDefinition {
    key: "egress.seen",
    scope: SettingScope::Global,
    description: "Выход в интернет: просмотренные строки",
    sensitive: false,
    parse: as_string_list,
};

/// A row's own route (app_settings, by the row's id): `auto`, `direct`, `server` (the location chosen for all) or a
/// connection's id.
pub const EGRESS_ROUTE: SettingDefinition<String> = SettingDefinition {
    key: "egress.route",
    scope: SettingScope::App,
    description: "Выход в интернет: маршрут строки",
    sensitive: false,
    parse: |value| value.as_str().and_then(super::rules::Route::parse).map(|route| route.as_text().to_string()),
};

/// What a row does when its connection does not answer: `wait` or `direct`.
pub const EGRESS_ON_DOWN: SettingDefinition<String> = SettingDefinition {
    key: "egress.on_down",
    scope: SettingScope::App,
    description: "Выход в интернет: если сервер не отвечает",
    sensitive: false,
    parse: |value| as_one_of(value, &["wait", "direct"]),
};

/// What this home learned of a row by trying: directly it did not open. Each side keeps its own (it is about this
/// home's network): it does not travel with the settings package (placement/transfer.rs, `LOCAL_APP_SETTINGS`).
pub const EGRESS_LEARNED: SettingDefinition<super::rules::Learned> = SettingDefinition {
    key: "egress.learned",
    scope: SettingScope::App,
    description: "Выход в интернет: что дом узнал сам",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};
