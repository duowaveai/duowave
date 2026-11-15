//! The household's country (docs/plans/catalog-regions.md, 3): where the home is, as an ISO 3166-1 alpha-2 code.
//! A place, not a person: the language says who speaks, the country — what works from here (the routes of
//! «Выход в интернет», docs/plans/own-server.md; the catalog's regions later).
//!
//! - The setting `home_country`: `RU`, `DE`… or `none` («Не скажу» — nothing is chosen by country); not stored —
//!   never asked, the panel asks whoever may edit the settings.
//! - Never guessed from an address: the panel guesses from the computer's time zone and language
//!   (`panel/src/country.ts`) and the person confirms. The gateway on a station has no guess of its own.
//! - `GET /api/v1/home/country` → `{country}`: `null` — never asked. `PUT … {country}` — whoever may edit the
//!   settings; `none` is an answer too.
//! - Apps learn it by the letter `system/home` → `country` (src/bus/providers/system.rs): `{country}`, `null` when the
//!   household did not say.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::core::identity::permissions::SETTINGS_EDIT;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::HOME_COUNTRY;
use crate::settings::SettingsService;

/// The answer «Не скажу»: asked, and nothing is chosen by country.
pub const NONE: &str = "none";

/// A country as we keep it: two ASCII letters in upper case (`ru` → `RU`), or `none`; anything else — not one.
pub fn normalize(text: &str) -> Option<String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case(NONE) {
        return Some(NONE.to_string());
    }
    (text.len() == 2 && text.chars().all(|c| c.is_ascii_alphabetic())).then(|| text.to_ascii_uppercase())
}

/// The stored answer: a code, `none`, or None — never asked.
pub fn answer(settings: &SettingsService) -> Option<String> {
    settings.get(&HOME_COUNTRY, None)
}

/// The household's country when it said one: `none` and «never asked» are both None.
pub fn household(settings: &SettingsService) -> Option<String> {
    answer(settings).filter(|code| code != NONE)
}

#[derive(Debug, Serialize)]
pub struct CountryView {
    /// A code, `none`, or null — never asked.
    country: Option<String>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/home/country", get(view).put(set))
}

async fn view(State(app): State<Arc<App>>, _caller: Caller) -> ApiResult<CountryView> {
    Ok(ok(CountryView { country: answer(&app.settings) }))
}

async fn set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<CountryView> {
    caller.permitted(SETTINGS_EDIT)?;
    let Some(code) = body.get("country").and_then(Value::as_str).and_then(normalize) else {
        return Err(ApiError::bad_request(crate::tr!(
            "country: код страны из двух букв, например RU или DE, или none"
        )));
    };
    if answer(&app.settings).as_deref() != Some(code.as_str()) {
        app.settings.set(&HOME_COUNTRY, &code, caller.actor())?;
    }
    Ok(ok(CountryView { country: Some(code) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::storage::db::Db;

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
        let settings = SettingsService::new(Arc::new(Db::memory()), EventBus::new());
        assert_eq!(answer(&settings), None);
        settings.set(&HOME_COUNTRY, &NONE.to_string(), None).unwrap();
        assert_eq!(answer(&settings).as_deref(), Some("none"));
        assert_eq!(household(&settings), None);
        settings.set(&HOME_COUNTRY, &"NL".to_string(), None).unwrap();
        assert_eq!(household(&settings).as_deref(), Some("NL"));
    }
}
