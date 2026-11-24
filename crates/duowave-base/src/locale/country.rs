//! The home's country, as «Выход в интернет» needs it (docs/plans/own-server.md): an ISO 3166-1 alpha-2 code. A place,
//! not a person: the language says who speaks, the country — what works from here.
//!
//! Since 16.11 the gateway does not ask it and keeps no country of its own (the owner: data one app needs is asked by
//! that app). The app that gives the gateway its network («VPN») asks «Где ваш дом?» in its window and keeps the answer
//! as its own setting `homeCountry` (`RU`, `DE`… or `none` — «Не скажу»); `src/egress` reads it from there. Other apps
//! do not see it.
//!
//! The setting `home_country` of gateway 0.13.0 (the panel asked it) is only read once: an answer given there moves
//! into the app's setting (`egress::Egress::carry_home_country`), so nobody is asked twice.

use crate::settings::definitions::HOME_COUNTRY;
use crate::settings::SettingsService;

/// The answer «Не скажу»: asked, and nothing is chosen by country.
pub const NONE: &str = "none";

/// The app's setting that keeps the answer.
pub const APP_SETTING: &str = "homeCountry";

/// A country as we keep it: two ASCII letters in upper case (`ru` → `RU`), or `none`; anything else — not one.
pub fn normalize(text: &str) -> Option<String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case(NONE) {
        return Some(NONE.to_string());
    }
    (text.len() == 2 && text.chars().all(|c| c.is_ascii_alphabetic())).then(|| text.to_ascii_uppercase())
}

/// The country of an answer: `none` is an answer but no country.
pub fn of_answer(answer: Option<&str>) -> Option<String> {
    answer.and_then(normalize).filter(|code| code != NONE)
}

/// The answer gateway 0.13.0 kept for the household: a code, `none`, or None — never asked there.
pub fn legacy_answer(settings: &SettingsService) -> Option<String> {
    settings.get(&HOME_COUNTRY, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_country_is_two_letters_in_upper_case_or_none() {
        assert_eq!(normalize("ru").as_deref(), Some("RU"));
        assert_eq!(normalize(" De ").as_deref(), Some("DE"));
        assert_eq!(normalize("NONE").as_deref(), Some("none"));
        assert_eq!(normalize("RUS"), None);
        assert_eq!(normalize("r1"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn none_is_an_answer_but_not_a_country() {
        assert_eq!(of_answer(Some("nl")).as_deref(), Some("NL"));
        assert_eq!(of_answer(Some("none")), None);
        assert_eq!(of_answer(Some("Россия")), None);
        assert_eq!(of_answer(None), None);
    }
}
