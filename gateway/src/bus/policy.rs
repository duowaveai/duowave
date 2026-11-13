//! The one rule (docs/plans/bus.md; crew's `canRead` and `verdictOf` as the model): may a letter go through, wait
//! for a person's «да», or not be. Every path asks it — apps, folders, devices, the panel's windows, lists — so there
//! is no second door. It is a pure function of the facts the bus gathers, tested as a table.

use serde::Serialize;

use super::danger::Danger;
use super::level::Level;
use super::links::Mode;
use super::trail::Authority;

/// What the bus knows of one letter when it decides.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    /// The step the handle needs.
    pub need: Level,
    /// The trail's root.
    pub authority: Authority,
    /// The caller is a person (or the household, a key): bounded by its own rights, not by links.
    pub caller_is_person: bool,
    /// The narrowest link from the caller to the target.
    pub link: Level,
    /// That link is a person's word that stands (the owner's own, a «нет»): no question past it.
    pub settled: bool,
    /// What the caller asked for towards the target (an app's manifest).
    pub declared: Level,
    pub from_mode: Mode,
    pub to_mode: Mode,
    /// The caller owns the target: itself, its own folder, its own news.
    pub own: bool,
    /// The target or the handle is personal.
    pub private: bool,
    /// The root holds the person's right the handle needs (true when none is needed or the root is no person).
    pub root_may: bool,
    /// The handle's danger after the owner's words.
    pub danger: Option<Danger>,
    /// The owner said «всегда можно» for this caller and handle.
    pub always: bool,
    /// A person's «да» for this one action waits to be used.
    pub confirmed: bool,
}

/// What a person is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AskKind {
    /// The phone's way: allowed once, remembered as a link (decision 2).
    Remember,
    /// A dangerous action no person started: this one action (decision 10).
    Once,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// Not there for the caller: answered as if it did not exist.
    NotFound,
    /// The caller sees it but has no link this far.
    NotAllowed,
    /// The person who started the chain has no right to this.
    RootCannot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Ask(AskKind),
    Deny(Reason),
}

pub fn verdict(facts: &Facts) -> Verdict {
    // Its own: an app itself, its folder, its news — isolation does not cut it off its own.
    if facts.own {
        return root_and_danger(facts);
    }
    // «Отдельно»: nobody reaches it and it reaches nobody; a person still reaches it in the panel.
    if !facts.caller_is_person && (facts.from_mode == Mode::Isolated || facts.to_mode == Mode::Isolated) {
        return Verdict::Deny(Reason::NotFound);
    }
    let mut have = facts.link;
    if facts.caller_is_person {
        // A person reaches what their own rights allow (`root_may`); someone else's personal things only by a link.
        if !facts.private {
            have = Level::Edit;
        }
    } else if facts.from_mode == Mode::Free && !facts.private && facts.danger.is_none() {
        // «Свободно»: everything open, up to calling; never the personal or the dangerous.
        have = have.max(Level::Call);
    }
    if have < facts.need {
        // The personal is never asked for and never hinted at: only a link the owner made opens it.
        if facts.private {
            return Verdict::Deny(Reason::NotFound);
        }
        // What an app asked for in its manifest may be asked of a person, once (decisions 2 and 8) — unless a person
        // already said their word for this pair.
        if !facts.caller_is_person && !facts.settled && facts.declared >= facts.need {
            return Verdict::Ask(AskKind::Remember);
        }
        if have >= Level::See || facts.declared >= Level::See {
            return Verdict::Deny(Reason::NotAllowed);
        }
        return Verdict::Deny(Reason::NotFound);
    }
    root_and_danger(facts)
}

/// The two checks every allowed letter passes: the root's own right, then the danger (decision 10).
fn root_and_danger(facts: &Facts) -> Verdict {
    if !facts.root_may {
        return Verdict::Deny(Reason::RootCannot);
    }
    if facts.danger.is_some()
        && matches!(facts.authority, Authority::App | Authority::Agent)
        && !(facts.always || facts.confirmed)
    {
        return Verdict::Ask(AskKind::Once);
    }
    Verdict::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            need: Level::Call,
            authority: Authority::Person,
            caller_is_person: false,
            link: Level::None,
            settled: false,
            declared: Level::None,
            from_mode: Mode::Rules,
            to_mode: Mode::Rules,
            own: false,
            private: false,
            root_may: true,
            danger: None,
            always: false,
            confirmed: false,
        }
    }

    #[test]
    fn nothing_is_reached_without_a_link_and_the_unseen_does_not_exist() {
        assert_eq!(verdict(&facts()), Verdict::Deny(Reason::NotFound));
        assert_eq!(verdict(&Facts { link: Level::See, ..facts() }), Verdict::Deny(Reason::NotAllowed));
        assert_eq!(verdict(&Facts { link: Level::Call, ..facts() }), Verdict::Allow);
        assert_eq!(verdict(&Facts { link: Level::Edit, ..facts() }), Verdict::Allow);
        assert_eq!(verdict(&Facts { link: Level::Listen, need: Level::Listen, ..facts() }), Verdict::Allow);
    }

    #[test]
    fn what_the_manifest_asked_for_is_asked_once_like_a_phone() {
        assert_eq!(verdict(&Facts { declared: Level::Call, ..facts() }), Verdict::Ask(AskKind::Remember));
        // Asked for less than it needs now: seen, not allowed, not asked.
        assert_eq!(verdict(&Facts { declared: Level::See, ..facts() }), Verdict::Deny(Reason::NotAllowed));
        // A link to «не знает» does not hide what the manifest asked for: asked, the owner decides.
        assert_eq!(
            verdict(&Facts { declared: Level::Edit, need: Level::Edit, ..facts() }),
            Verdict::Ask(AskKind::Remember)
        );
        // The personal is not asked for, whatever the manifest says.
        assert_eq!(
            verdict(&Facts { declared: Level::Call, private: true, ..facts() }),
            Verdict::Deny(Reason::NotFound)
        );
        // A person's word stands — a «нет», the owner's own link lower than it asks: not asked again.
        assert_eq!(
            verdict(&Facts { declared: Level::Call, settled: true, ..facts() }),
            Verdict::Deny(Reason::NotAllowed)
        );
        assert_eq!(
            verdict(&Facts { declared: Level::Edit, need: Level::Edit, link: Level::See, settled: true, ..facts() }),
            Verdict::Deny(Reason::NotAllowed)
        );
        // A «да» to less is no ceiling: asking for more is asked again.
        assert_eq!(
            verdict(&Facts { declared: Level::Edit, need: Level::Edit, link: Level::Call, ..facts() }),
            Verdict::Ask(AskKind::Remember)
        );
    }

    #[test]
    fn isolation_cuts_both_ways_but_not_its_own_nor_a_person() {
        let open = Facts { link: Level::Edit, ..facts() };
        assert_eq!(verdict(&Facts { from_mode: Mode::Isolated, ..open }), Verdict::Deny(Reason::NotFound));
        assert_eq!(verdict(&Facts { to_mode: Mode::Isolated, ..open }), Verdict::Deny(Reason::NotFound));
        assert_eq!(verdict(&Facts { from_mode: Mode::Isolated, own: true, ..facts() }), Verdict::Allow);
        assert_eq!(verdict(&Facts { to_mode: Mode::Isolated, caller_is_person: true, ..facts() }), Verdict::Allow);
    }

    #[test]
    fn free_opens_the_open_up_to_calling_but_never_the_personal_or_the_dangerous() {
        let free = Facts { from_mode: Mode::Free, ..facts() };
        assert_eq!(verdict(&free), Verdict::Allow);
        assert_eq!(verdict(&Facts { need: Level::Edit, ..free }), Verdict::Deny(Reason::NotAllowed));
        assert_eq!(verdict(&Facts { private: true, ..free }), Verdict::Deny(Reason::NotFound));
        assert_eq!(verdict(&Facts { danger: Some(Danger::Money), ..free }), Verdict::Deny(Reason::NotFound));
        // A link opens the personal: the owner made it.
        assert_eq!(verdict(&Facts { private: true, link: Level::Call, ..free }), Verdict::Allow);
    }

    #[test]
    fn a_chain_is_never_more_than_its_root() {
        let linked = Facts { link: Level::Call, root_may: false, ..facts() };
        assert_eq!(verdict(&linked), Verdict::Deny(Reason::RootCannot));
        assert_eq!(verdict(&Facts { own: true, root_may: false, ..facts() }), Verdict::Deny(Reason::RootCannot));
        // A person reaches what their rights allow; another person's personal things only by a link.
        let person = Facts { caller_is_person: true, ..facts() };
        assert_eq!(verdict(&Facts { need: Level::Edit, ..person }), Verdict::Allow);
        assert_eq!(verdict(&Facts { private: true, ..person }), Verdict::Deny(Reason::NotFound));
        assert_eq!(verdict(&Facts { private: true, link: Level::Call, ..person }), Verdict::Allow);
    }

    #[test]
    fn danger_needs_a_person_at_the_root_or_their_yes_each_time() {
        let linked = Facts { link: Level::Call, danger: Some(Danger::Locks), ..facts() };
        assert_eq!(verdict(&linked), Verdict::Allow);
        assert_eq!(verdict(&Facts { authority: Authority::System, ..linked }), Verdict::Allow);
        assert_eq!(verdict(&Facts { authority: Authority::App, ..linked }), Verdict::Ask(AskKind::Once));
        assert_eq!(verdict(&Facts { authority: Authority::Agent, ..linked }), Verdict::Ask(AskKind::Once));
        assert_eq!(verdict(&Facts { authority: Authority::App, confirmed: true, ..linked }), Verdict::Allow);
        assert_eq!(verdict(&Facts { authority: Authority::App, always: true, ..linked }), Verdict::Allow);
        // An app's own dangerous handle is still dangerous when no person started it.
        let own = Facts { own: true, authority: Authority::App, danger: Some(Danger::Delete), ..facts() };
        assert_eq!(verdict(&own), Verdict::Ask(AskKind::Once));
    }
}
