//! Roles are named sets of grants (PRODUCT.md: the admin sets the bounds, everyone has their own). A person has one
//! role; the admin narrows or widens it per person with restrictions (policy.rs).

use serde::{Deserialize, Serialize};

use crate::skills::capabilities::CapabilityName;

use super::permissions::use_of;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
    Guest,
}

pub const ROLES: &[Role] = &[Role::Admin, Role::Member, Role::Guest];

/// What a guest may ask for by voice: harmless things that neither spend nor reach out of the house.
const GUEST_CAPABILITIES: &[CapabilityName] = &[
    CapabilityName::Chat,
    CapabilityName::Volume,
    CapabilityName::Playback,
    CapabilityName::Radio,
    CapabilityName::Music,
    CapabilityName::Tales,
    CapabilityName::Games,
    CapabilityName::Timers,
    CapabilityName::Weather,
    CapabilityName::News,
    CapabilityName::Led,
    CapabilityName::MusicRecognition,
];

pub struct RoleInfo {
    pub title: &'static str,
    pub description: &'static str,
    pub grants: Vec<String>,
}

impl Role {
    pub fn parse(value: &str) -> Option<Role> {
        match value {
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            "guest" => Some(Role::Guest),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Guest => "guest",
        }
    }

    pub fn info(self) -> RoleInfo {
        match self {
            Role::Admin => RoleInfo {
                title: "Администратор",
                description: "Настраивает всё: режим, маршруты, модели, людей и подключения.",
                grants: vec!["*".into()],
            },
            Role::Member => RoleInfo {
                title: "Домашний",
                description: "Пользуется всеми возможностями колонки и видит её состояние; настройки дома не меняет.",
                grants: ["station.view", "station.say", "connectors.view", "mcp.use", "use:*"].map(String::from).to_vec(),
            },
            Role::Guest => RoleInfo {
                title: "Гость",
                description: "Простые вещи голосом: музыка, таймеры, погода, разговор. Без звонков, умного дома и сервисов аккаунта.",
                grants: GUEST_CAPABILITIES.iter().map(|name| use_of(*name)).collect(),
            },
        }
    }
}

/// A stored role; anything unknown is the least: a guest.
pub fn normalize_role(value: &str) -> Role {
    Role::parse(value).unwrap_or(Role::Guest)
}
