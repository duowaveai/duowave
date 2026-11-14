//! Dangerous actions (docs/plans/bus.md; crew's `actions/policy.ts` as the model): what may cost money, lose data,
//! reach people outside the house, open the door, look or listen, or hand out rights. A dangerous action goes
//! through only when a person stands at the start of the chain, or after that person's «да» for this one action. What
//! is dangerous is not wired into code: a provider says it of its handles, a manifest may add it to its own (never
//! take it away), and the owner has the last word for an app's handle — «всегда можно» or «опасно».
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Danger {
    /// Spends money: a purchase, a payment, a subscription.
    Money,
    /// Removes what cannot be got back.
    Delete,
    /// Reaches people or services outside the house: a message, a post, a file sent away.
    Outside,
    /// Locks, doors, alarms, the heating.
    Locks,
    /// A camera or a microphone: looks or listens.
    Camera,
    /// Rights and keys: links, accounts, access.
    Access,
}

pub const DANGERS: &[Danger] =
    &[Danger::Money, Danger::Delete, Danger::Outside, Danger::Locks, Danger::Camera, Danger::Access];

impl Danger {
    pub fn parse(text: &str) -> Option<Danger> {
        match text {
            "money" => Some(Danger::Money),
            "delete" => Some(Danger::Delete),
            "outside" => Some(Danger::Outside),
            "locks" => Some(Danger::Locks),
            "camera" => Some(Danger::Camera),
            "access" => Some(Danger::Access),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Danger::Money => "money",
            Danger::Delete => "delete",
            Danger::Outside => "outside",
            Danger::Locks => "locks",
            Danger::Camera => "camera",
            Danger::Access => "access",
        }
    }

    /// For people: «тратит деньги».
    pub fn title(self) -> String {
        match self {
            Danger::Money => crate::tr!("тратит деньги"),
            Danger::Delete => crate::tr!("удаляет без возврата"),
            Danger::Outside => crate::tr!("пишет или отправляет наружу"),
            Danger::Locks => crate::tr!("управляет замками и дверями"),
            Danger::Camera => crate::tr!("смотрит или слушает"),
            Danger::Access => crate::tr!("меняет права и ключи"),
        }
    }

    /// The danger an MCP tool's annotations tell: `destructiveHint` — it removes, `openWorldHint` — it reaches out.
    pub fn of_annotations(destructive: bool, open_world: bool) -> Option<Danger> {
        if destructive {
            Some(Danger::Delete)
        } else if open_world {
            Some(Danger::Outside)
        } else {
            None
        }
    }
}

/// The owner's word on one handle of one caller (`bus.dangers`): «всегда можно» lets the caller do it without asking
/// each time; a mark `none` lifts a provider's mark, a danger's name sets one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DangerWord {
    /// `app/radio:play`, `app/radio:*`: whose handle.
    pub target: String,
    /// Who it is said for: `app/morning`, `*` for anyone.
    #[serde(default = "anyone")]
    pub caller: String,
    #[serde(default)]
    pub always: bool,
    /// `none`, or a danger's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
}

fn anyone() -> String {
    "*".into()
}

impl DangerWord {
    fn covers(&self, target: &str, handle: &str, caller: &str) -> bool {
        let (whose, what) = self.target.rsplit_once(':').unwrap_or((self.target.as_str(), "*"));
        whose == target && (what == "*" || what == handle) && (self.caller == "*" || self.caller == caller)
    }
}

/// A handle's danger after every word on it: the provider's mark, then the owner's word (which may lift it), the most
/// specific word first.
pub fn danger_of(
    marked: Option<Danger>,
    words: &[DangerWord],
    target: &str,
    handle: &str,
    caller: &str,
) -> (Option<Danger>, bool) {
    let mut danger = marked;
    let mut always = false;
    let mut said: Vec<&DangerWord> = words.iter().filter(|word| word.covers(target, handle, caller)).collect();
    // The narrower word last, so it wins: a handle's own over the app's `*`, a caller's own over everyone's.
    said.sort_by_key(|word| (word.target.ends_with(":*") as u8 == 0, word.caller != "*"));
    for word in said {
        match word.mark.as_deref() {
            Some("none") => danger = None,
            Some(name) => danger = Danger::parse(name).or(danger),
            None => {}
        }
        always = word.always;
    }
    (danger, always)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(target: &str, caller: &str, always: bool, mark: Option<&str>) -> DangerWord {
        DangerWord { target: target.into(), caller: caller.into(), always, mark: mark.map(str::to_string) }
    }

    #[test]
    fn reads_and_names_each_danger() {
        for danger in DANGERS {
            assert_eq!(Danger::parse(danger.as_str()), Some(*danger));
            assert!(!danger.title().is_empty());
        }
        assert_eq!(Danger::of_annotations(true, true), Some(Danger::Delete));
        assert_eq!(Danger::of_annotations(false, true), Some(Danger::Outside));
        assert_eq!(Danger::of_annotations(false, false), None);
    }

    #[test]
    fn the_owner_may_lift_a_mark_or_say_always_for_one_caller() {
        let words = vec![
            word("system/smart-home:act", "*", false, Some("none")),
            word("app/shop:buy", "app/morning", true, None),
            word("app/notes:wipe", "*", false, Some("delete")),
        ];
        assert_eq!(danger_of(Some(Danger::Locks), &words, "system/smart-home", "act", "app/x"), (None, false));
        assert_eq!(
            danger_of(Some(Danger::Money), &words, "app/shop", "buy", "app/morning"),
            (Some(Danger::Money), true)
        );
        assert_eq!(
            danger_of(Some(Danger::Money), &words, "app/shop", "buy", "app/other"),
            (Some(Danger::Money), false)
        );
        assert_eq!(danger_of(None, &words, "app/notes", "wipe", "app/x"), (Some(Danger::Delete), false));
    }
}
