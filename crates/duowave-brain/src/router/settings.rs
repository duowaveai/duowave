//! The router's own settings, on the floor «Мозг» where their types are (docs/plans/router-and-works.md, stage 2):
//! the seats, the table and the general budgets. Each travels with the settings package.

use crate::core::settings::{SettingDefinition, SettingScope};
use crate::router::budgets::Budgets;
use crate::router::policy::Table;
use crate::router::seats::Seats;

/// The router's seats (seats.rs): «Исполнитель» — the agent of a work in a folder and its model. The seats of «Мозг»
/// stay in `brain` and `workshop.agent`.
pub const ROUTER_SEATS: SettingDefinition<Seats> = SettingDefinition {
    key: "router_seats",
    scope: SettingScope::Global,
    description: "Места роутера",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The router's table (policy.rs): rules as data, the first that fits. Not set — the default built in.
pub const ROUTER_TABLE: SettingDefinition<Table> = SettingDefinition {
    key: "router_table",
    scope: SettingScope::Global,
    description: "Таблица роутера",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The general budgets (budgets.rs): on every work, unless its folder or the work has its own. Not set — five dollars
/// a work, a cheaper model at 80%, the question at 100%.
pub const BUDGETS: SettingDefinition<Budgets> = SettingDefinition {
    key: "budgets",
    scope: SettingScope::Global,
    description: "Бюджеты",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};
