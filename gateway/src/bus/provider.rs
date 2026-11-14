//! Providers: who answers for a kind of address (docs/plans/bus.md, «Расширяемость»). The bus knows no app, folder or
//! device by itself: a provider tells which participants it has, what each can do (its handles — the step of the
//! ladder each needs, whether it is dangerous or personal, which right of a person it needs), and carries out the
//! letters the bus lets through. A new kind of participant — a new device, a project, an outside agent — is a new
//! provider registered on the bus; the core and its one rule stay as they are.
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;

use super::address::Address;
use super::danger::Danger;
use super::level::Level;
use super::trail::{Trail, TrailError};

/// What a participant can do: one name of its own, the step a caller needs for it, its danger, whether it is
/// personal, the right of a person a person's chain must hold.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handle {
    pub name: String,
    pub level: Level,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danger: Option<Danger>,
    /// Personal: open only to a link made for it, never by the «Свободно» mode.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
    /// The right of a person (`use:smart_home`) a chain a person started must hold; an app on its own answers for
    /// itself with its links.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    /// The people's roles it is open to (a folder's «Видят»): a chain a person of another role started does not see
    /// it at all. None — every role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<crate::core::identity::roles::Role>>,
}

impl Handle {
    pub fn new(name: &str, level: Level) -> Handle {
        Handle { name: name.to_string(), level, title: None, danger: None, private: false, needs: None, roles: None }
    }

    /// Open only to people of these roles.
    pub fn for_roles(mut self, roles: Option<Vec<crate::core::identity::roles::Role>>) -> Handle {
        self.roles = roles;
        self
    }

    /// Whether a chain this principal started sees it: a person or the household by role; a key and the gateway
    /// answer for themselves.
    pub fn admits(&self, principal: Option<&crate::core::identity::principal::Principal>) -> bool {
        use crate::core::identity::PrincipalKind;
        let (Some(roles), Some(principal)) = (&self.roles, principal) else { return true };
        match principal.kind {
            PrincipalKind::User | PrincipalKind::Household => principal.role.is_some_and(|role| roles.contains(&role)),
            _ => true,
        }
    }

    pub fn titled(mut self, title: &str) -> Handle {
        self.title = Some(title.to_string());
        self
    }

    pub fn dangerous(mut self, danger: Option<Danger>) -> Handle {
        self.danger = danger;
        self
    }

    pub fn private(mut self, private: bool) -> Handle {
        self.private = private;
        self
    }

    pub fn needs(mut self, right: Option<String>) -> Handle {
        self.needs = right;
        self
    }
}

/// A participant as the bus lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    pub address: Address,
    pub title: String,
    /// Personal (a person's folder, notes): open only to a link made for it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
    /// Whose it is: `person/<id>` for a personal folder, `app/<id>` for an app's own folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<Address>,
}

impl Participant {
    pub fn new(address: Address, title: &str) -> Participant {
        Participant { address, title: title.to_string(), private: false, owner: None }
    }
}

/// A letter the bus let through, for its provider to carry out.
#[derive(Debug, Clone)]
pub struct Letter {
    /// The journal's number.
    pub id: u64,
    pub from: Address,
    pub to: Address,
    pub handle: String,
    pub input: Value,
    /// Files the letter carries: the receiver may read each of them for a while in this chain.
    pub files: Vec<Address>,
    pub trail: Trail,
}

/// News a participant tells: who, which, what it carries.
#[derive(Debug, Clone)]
pub struct News {
    pub from: Address,
    pub event: String,
    pub payload: Value,
    pub trail: Trail,
}

/// Why a letter did not go through or failed. Not found and not visible read the same: a name is not leaked by
/// probing.
#[derive(Debug, Clone, PartialEq)]
pub enum BusError {
    NotFound(String),
    Denied(String),
    /// Waits for a person's «да»: the id of the question.
    NeedsYes {
        id: String,
        why: String,
    },
    Fuse(String),
    Failed(String),
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusError::NotFound(what) => f.write_str(&crate::tr!("нет такого: {what}", what = what)),
            BusError::NeedsYes { why, .. } => f.write_str(&crate::tr!("ждёт вашего «да»: {why}", why = why)),
            BusError::Denied(text) | BusError::Fuse(text) | BusError::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for BusError {}

impl BusError {
    /// The code a caller tells cases apart by.
    pub fn code(&self) -> &'static str {
        match self {
            BusError::NotFound(_) => "not_found",
            BusError::Denied(_) => "denied",
            BusError::NeedsYes { .. } => "needs_yes",
            BusError::Fuse(_) => "fuse",
            BusError::Failed(_) => "failed",
        }
    }
}

/// An answer in words for an app with no `ask` of its own (docs/plans/bus.md, «Вопросы словами»): the household's
/// model, given that app's tools, answers the asker; each tool it calls is the asker's letter in its chain. When the
/// model is in the cloud the owner's «Вопросы словами через облако» must be on.
pub trait Words: Send + Sync {
    fn answer(
        &self,
        trail: Trail,
        asker: Address,
        app: Address,
        question: String,
    ) -> BoxFuture<'static, Result<Value, BusError>>;
}

impl From<TrailError> for BusError {
    fn from(error: TrailError) -> BusError {
        BusError::Fuse(error.to_string())
    }
}

/// Who answers for one kind of address.
pub trait Provider: Send + Sync {
    /// The kind: `app`, `system`, `files`, `device`.
    fn kind(&self) -> &'static str;

    /// Its participants now, for lists.
    fn participants(&self) -> Vec<Participant>;

    /// One participant; none when there is no such.
    fn participant(&self, address: &Address) -> Option<Participant> {
        self.participants().into_iter().find(|participant| &participant.address == address)
    }

    /// What a participant can do.
    fn handles(&self, address: &Address) -> Vec<Handle>;

    /// One of its handles.
    fn handle(&self, address: &Address, name: &str) -> Option<Handle> {
        self.handles(address).into_iter().find(|handle| handle.name == name)
    }

    /// What `from` (one of this provider's participants) asked for itself towards `to`: an app's manifest rights.
    /// Only what was asked for may be asked of a person (decision 8).
    fn declared(&self, from: &Address, to: &Address) -> Level {
        let _ = (from, to);
        Level::None
    }

    /// What `from` (one of this provider's participants) has already without a link: an app's older rights,
    /// approved with its install. A link the owner made for the pair always wins over it, a lower one too.
    fn implicit(&self, from: &Address, to: &Address) -> Level {
        let _ = (from, to);
        Level::None
    }

    /// Whether `from` owns `to`: an app its own folder, its own news, itself.
    fn owns(&self, from: &Address, to: &Address) -> bool {
        to == from || to.within(from)
    }

    /// Carries out a letter the bus let through.
    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>>;

    /// Its participants that listen to `from`'s `event` (an app's `contributes.bus.on`).
    fn listeners(&self, from: &Address, event: &str) -> Vec<Address> {
        let _ = (from, event);
        Vec::new()
    }

    /// Gives news to one of its listeners the bus let it reach.
    fn deliver(&self, to: &Address, news: News) -> BoxFuture<'static, Result<(), BusError>> {
        let _ = (to, news);
        Box::pin(async { Ok(()) })
    }
}
