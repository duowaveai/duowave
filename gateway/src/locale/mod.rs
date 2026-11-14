//! The languages of the household and of a request (docs/plans/i18n.md).
//!
//! - The household's language — the setting `language`: what runs with nobody at a screen speaks it (the apps' code
//!   reads it as `host.i18n.locale`, schedules, notices). Set at the first setup and in «Настройки» → «Язык».
//! - A request's language — the panel's `Accept-Language`: the views of the apps (their titles, descriptions, the
//!   titles of their settings) answer in it. It lives for the request's task (`request_language`, a middleware), so
//!   any view built while answering reads it without a parameter through every call.
//! - `GET /api/v1/language` → `{language}` — open: the sign-in screen shows the household's language before anyone
//!   signs in. `PUT … {language}` — whoever may edit the settings.
//!
//! The order to look for a text in: the request's language (else the household's), English, the household's, then the
//! package's own. No language or no translation — English.

pub mod text;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::core::identity::permissions::SETTINGS_EDIT;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::LANGUAGE;
use crate::settings::SettingsService;

/// The language every package falls back to after the request's and the household's.
pub const FALLBACK: &str = "en";

/// The language of texts that say nothing of theirs: the gateway's own and a package without `languages`.
pub const SOURCE: &str = "ru";

tokio::task_local! {
    static REQUEST: Option<String>;
}

/// A language code as we keep it: the primary subtag of a BCP 47 tag, lower case — `en-GB` → `en`; not a language
/// (`*`, a number, a word) — none.
pub fn normalize(tag: &str) -> Option<String> {
    let base = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    ((2..=3).contains(&base.len()) && base.chars().all(|c| c.is_ascii_lowercase())).then_some(base)
}

/// The first language of an `Accept-Language` header by its weight: `en-US,en;q=0.9,ru;q=0.8` → `en`.
pub fn from_accept(header: &str) -> Option<String> {
    let mut best: Option<(f32, String)> = None;
    for part in header.split(',') {
        let mut pieces = part.split(';');
        let Some(code) = pieces.next().and_then(normalize) else { continue };
        let weight = pieces
            .find_map(|piece| piece.trim().strip_prefix("q=").and_then(|q| q.trim().parse::<f32>().ok()))
            .unwrap_or(1.0);
        if weight > 0.0 && best.as_ref().is_none_or(|(top, _)| weight > *top) {
            best = Some((weight, code));
        }
    }
    best.map(|(_, code)| code)
}

/// The middleware: the request's language for its task.
pub async fn request_language(request: Request, next: Next) -> Response {
    let language = request
        .headers()
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .and_then(from_accept);
    REQUEST.scope(language, next.run(request)).await
}

/// The language of the request being answered, if it said one.
pub fn of_request() -> Option<String> {
    REQUEST.try_with(Clone::clone).ok().flatten()
}

/// The household's language, if one is set.
pub fn household(settings: &SettingsService) -> Option<String> {
    settings.get(&LANGUAGE, None)
}

/// The languages to look for a text in, best first: the request's, English — the language of last resort when the
/// asked one is missing —, the household's. No duplicates.
pub fn preferences(settings: &SettingsService) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let asked = of_request().or_else(|| household(settings));
    for code in [asked, Some(FALLBACK.to_string()), household(settings)].into_iter().flatten() {
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// The language the household's apps speak now: the household's; none set — English, the language of last resort.
pub fn current(settings: &SettingsService) -> String {
    household(settings).unwrap_or_else(|| FALLBACK.to_string())
}

#[derive(Debug, Serialize)]
pub struct LanguageView {
    /// The household's language; none — never set (the panel takes the system's).
    language: Option<String>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/language", get(view).put(set))
}

async fn view(State(app): State<Arc<App>>) -> ApiResult<LanguageView> {
    Ok(ok(LanguageView { language: household(&app.settings) }))
}

async fn set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<LanguageView> {
    caller.permitted(SETTINGS_EDIT)?;
    let Some(code) = body.get("language").and_then(Value::as_str).and_then(normalize) else {
        return Err(ApiError::bad_request(crate::tr!("language: код языка, например ru или en")));
    };
    if household(&app.settings).as_deref() != Some(code.as_str()) {
        app.settings.set(&LANGUAGE, &code, caller.actor())?;
    }
    text::remember_household(Some(code.clone()));
    Ok(ok(LanguageView { language: Some(code) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_the_primary_subtag_in_lower_case() {
        assert_eq!(normalize("en-GB").as_deref(), Some("en"));
        assert_eq!(normalize(" RU ").as_deref(), Some("ru"));
        assert_eq!(normalize("zh_Hant").as_deref(), Some("zh"));
        assert_eq!(normalize("fil").as_deref(), Some("fil"));
        assert_eq!(normalize("*"), None);
        assert_eq!(normalize("english"), None);
        assert_eq!(normalize("e1"), None);
    }

    #[test]
    fn accept_language_gives_the_heaviest_language() {
        assert_eq!(from_accept("en-US,en;q=0.9,ru;q=0.8").as_deref(), Some("en"));
        assert_eq!(from_accept("ru;q=0.5, de;q=0.7").as_deref(), Some("de"));
        assert_eq!(from_accept("*;q=1, fr;q=0.2").as_deref(), Some("fr"));
        assert_eq!(from_accept("de;q=0").as_deref(), None);
        assert_eq!(from_accept("").as_deref(), None);
    }

    #[tokio::test]
    async fn the_request_language_lives_for_its_task() {
        assert_eq!(of_request(), None);
        let inside = REQUEST.scope(Some("en".into()), async { of_request() }).await;
        assert_eq!(inside.as_deref(), Some("en"));
    }
}
