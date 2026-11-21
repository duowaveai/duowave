//! «Состояние дома» (docs/plans/home-setup-flow.md, stage 4): one look at what the home needs, row by row, each with
//! the one thing that fixes it. The last step of «Первые шаги» on every path and a card on the desk while anything is
//! not green. Before it the answers were spread over `/health`, `/api/v1/home/devices`, `/api/v1/placement`,
//! `/api/v1/helpers` and `/api/v1/brain`, and «говорит Алиса» meant four different faults (test 21.11).
//!
//! `GET /api/v1/home/status` at the centre → `{rows: [{id, state: ok|warn|bad, text, action?}]}`:
//!
//! - `voice` — the station's voice goes to DuoWave (a station centre: its own gateway; a computer centre: a station
//!   connected to it);
//! - `brain` — the model answers, or why not (the backup answers, a key that stopped working);
//! - `turbo` — a Turbo is there and on the line, and the station reaches it;
//! - `characterVoice` — the character's voice speaks, not Alice's;
//! - `mode` — who answers: «Алиса», «Алиса + ваш ИИ», «Ваш ИИ».
//!
//! The stations found on the network that are not in the home are the panel's row: the computer's search finds them
//! (`/api/v1/stations/connect/discover`), the centre on a station does not search.
//!
//! The actions are names the panel knows — `pointLocal`, `brain`, `turbo`, `firewall`, `voice`, `mode` — and each leads
//! to a screen that is there already.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::core::availability::State as Way;
use crate::core::identity::permissions::STATION_VIEW;
use crate::platform::http::{ok, ApiResult, Caller};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Ok,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: &'static str,
    pub state: RowState,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<&'static str>,
}

fn row(id: &'static str, state: RowState, text: String, action: Option<&'static str>) -> Row {
    Row { id, state, text, action }
}

/// A Turbo as the centre knows it: a helper of its list, or the computer a station leans on by the pair's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboFact {
    pub name: String,
    pub up: bool,
    pub error: Option<String>,
}

/// What the centre knows, gathered once (`facts`), and turned into rows by `rows` — the tests feed it by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The centre is a station.
    pub on_station: bool,
    /// The station's voice goes to DuoWave; None — not known (no station, its file not read).
    pub voice: Option<bool>,
    pub brain: Way,
    pub brain_title: Option<String>,
    pub brain_reason: Option<String>,
    /// The Turbos switched on, the computer's own built-in one left out.
    pub turbos: Vec<TurboFact>,
    /// The voice speaks the character's, not Alice's.
    pub own_voice: bool,
    pub mode: crate::settings::definitions::PanelMode,
}

/// The rows, in the order the household fixes them: the voice first — without it nothing else is heard.
pub fn rows(facts: &Facts) -> Vec<Row> {
    use crate::settings::definitions::PanelMode;
    let mut rows = Vec::new();
    rows.push(match (facts.voice, facts.on_station) {
        (Some(true), _) => row("voice", RowState::Ok, crate::tr!("Голос колонки идёт через DuoWave."), None),
        (Some(false), true) => row(
            "voice",
            RowState::Bad,
            crate::tr!("Голос колонки идёт мимо DuoWave — прямо к Яндексу."),
            Some("pointLocal"),
        ),
        (Some(false), false) => {
            row("voice", RowState::Warn, crate::tr!("Колонка сейчас не на связи с этим центром."), Some("station"))
        }
        (None, _) => row("voice", RowState::Warn, crate::tr!("Колонки в доме пока нет."), Some("station")),
    });
    let brain_title = facts.brain_title.clone().unwrap_or_else(|| crate::tr!("модель"));
    rows.push(match facts.brain {
        Way::Ready => row("brain", RowState::Ok, crate::tr!("Мозг отвечает: {title}.", title = brain_title), None),
        Way::Fallback => row(
            "brain",
            RowState::Warn,
            crate::tr!(
                "Отвечает запасная модель: {why}",
                why = facts.brain_reason.clone().unwrap_or_else(|| crate::tr!("выбранная не отвечает."))
            ),
            Some("brain"),
        ),
        Way::Off => row(
            "brain",
            // In «Алиса» Yandex answers: no model is needed yet.
            if facts.mode == PanelMode::Relay { RowState::Warn } else { RowState::Bad },
            crate::tr!(
                "Мозг не отвечает: {why}",
                why = facts.brain_reason.clone().unwrap_or_else(|| crate::tr!("модель не выбрана."))
            ),
            Some("brain"),
        ),
    });
    let down = facts.turbos.iter().find(|turbo| !turbo.up);
    rows.push(match (facts.turbos.first(), down) {
        (None, _) => row(
            "turbo",
            RowState::Warn,
            crate::tr!("Турбо нет: голос персонажа и слух — в облаке или у Алисы."),
            Some("turbo"),
        ),
        (Some(_), Some(turbo)) => row(
            "turbo",
            RowState::Bad,
            match &turbo.error {
                Some(error) => {
                    crate::tr!("Турбо «{name}» не на связи: {error}", name = turbo.name.clone(), error = error.clone())
                }
                None => crate::tr!("Турбо «{name}» не на связи.", name = turbo.name.clone()),
            },
            Some("firewall"),
        ),
        (Some(turbo), None) => {
            row("turbo", RowState::Ok, crate::tr!("Турбо «{name}» на связи.", name = turbo.name.clone()), None)
        }
    });
    rows.push(if facts.own_voice {
        row("characterVoice", RowState::Ok, crate::tr!("Ответы звучат голосом персонажа."), None)
    } else {
        row(
            "characterVoice",
            RowState::Warn,
            crate::tr!("Ответы звучат голосом Алисы: голос персонажа не выбран."),
            Some("voice"),
        )
    });
    rows.push(row(
        "mode",
        RowState::Ok,
        match facts.mode {
            PanelMode::Relay => crate::tr!("Отвечает Алиса — режим «Алиса»."),
            PanelMode::Gateway => crate::tr!("Отвечают Алиса и ваш ИИ — режим «Алиса + ваш ИИ»."),
            PanelMode::Own => crate::tr!("Отвечает ваш ИИ — режим «Ваш ИИ»."),
        },
        Some("mode"),
    ));
    rows
}

/// The centre's facts now.
pub fn facts(app: &App) -> Facts {
    use crate::helpers::HelperState;
    use crate::placement::service::Role;
    let on_station = app.placement.role() == Role::Station;
    let health = serde_json::to_value(app.health.status()).unwrap_or(Value::Null);
    let placement = app.placement.health_value();
    let voice = if on_station {
        placement.pointer("/voice/local").and_then(Value::as_bool)
    } else {
        // A computer centre: a station whose voice comes here now.
        health.pointer("/station/connected").and_then(Value::as_bool)
    };
    let now = app.brain.now(crate::config::ModelProfile::Fast);
    let mut turbos: Vec<TurboFact> = app
        .helpers
        .views()
        .into_iter()
        .filter(|helper| helper.enabled && !helper.builtin)
        .filter(|helper| !matches!(helper.state, HelperState::Off | HelperState::Itself))
        .map(|helper| TurboFact { name: helper.name, up: helper.state == HelperState::Up, error: helper.error })
        .collect();
    // A station's computer by the pair's key (`placement/helper`): its door, `homeServer`.
    if let Some(home) =
        placement.get("homeServer").filter(|home| home.get("enabled").and_then(Value::as_bool) != Some(false))
    {
        let state = home.get("state").and_then(Value::as_str).unwrap_or("none");
        if state != "none" && turbos.is_empty() {
            turbos.push(TurboFact {
                name: crate::tr!("Компьютер"),
                up: state == "up",
                error: home.get("error").and_then(Value::as_str).map(str::to_string),
            });
        }
    }
    // A Turbo out of reach that said why (Windows' firewall, a public network): its words, not «не на связи».
    if let Some(says) = turbo_says() {
        for turbo in turbos.iter_mut().filter(|turbo| !turbo.up) {
            turbo.error = Some(says.clone());
        }
    }
    Facts {
        on_station,
        voice,
        brain: now.state,
        brain_title: now.title.or(now.model),
        brain_reason: now.reason,
        turbos,
        own_voice: app.voice.chosen_provider() != crate::config::TtsProvider::Yandex,
        mode: app.modes.current(),
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/home/status", get(status))
        .route("/api/v1/home/turbo-network", axum::routing::post(turbo_network))
}

/// What the Turbo told of its network last (placement/watch.rs, `report_network`): why the station may not reach it.
struct TurboNetwork {
    closed: bool,
    advice: Option<String>,
    at: std::time::Instant,
}

static TURBO_NETWORK: std::sync::Mutex<Option<TurboNetwork>> = std::sync::Mutex::new(None);

/// A report older than this says nothing.
const REPORT_FOR: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The Turbo's network, from the computer with the pair's key only.
async fn turbo_network(
    State(app): State<Arc<App>>,
    caller: Caller,
    crate::platform::http::Body(body): crate::platform::http::Body<Value>,
) -> ApiResult<Value> {
    if !crate::placement::peer::from_peer(&caller, app.placement.peer_key()) {
        return Err(crate::platform::http::ApiError::forbidden(crate::tr!(
            "Сеть Турбо сообщает только компьютер дома, ключом пары."
        )));
    }
    let advice = body.get("advice").and_then(Value::as_str).map(|text| text.chars().take(400).collect::<String>());
    let closed = body.get("closed").and_then(Value::as_bool).unwrap_or(false);
    *TURBO_NETWORK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(TurboNetwork { closed, advice, at: std::time::Instant::now() });
    Ok(ok(json!({ "kept": true })))
}

/// Why the Turbo may be out of reach, as it said itself lately.
fn turbo_says() -> Option<String> {
    let kept = TURBO_NETWORK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.as_ref()
        .filter(|report| report.closed && report.at.elapsed() < REPORT_FOR)
        .and_then(|report| report.advice.clone())
}

async fn status(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    let rows = rows(&facts(&app));
    let green = rows.iter().all(|row| row.state == RowState::Ok);
    Ok(ok(json!({ "rows": rows, "green": green })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::definitions::PanelMode;

    fn good() -> Facts {
        Facts {
            on_station: true,
            voice: Some(true),
            brain: Way::Ready,
            brain_title: Some("Claude".into()),
            brain_reason: None,
            turbos: vec![TurboFact { name: "Ноутбук".into(), up: true, error: None }],
            own_voice: true,
            mode: PanelMode::Gateway,
        }
    }

    fn states(rows: &[Row]) -> Vec<(&'static str, RowState, Option<&'static str>)> {
        rows.iter().map(|row| (row.id, row.state, row.action)).collect()
    }

    /// Everything in place: every row green, only the mode with its way to change.
    #[test]
    fn a_home_in_order_is_green_row_by_row() {
        let rows = rows(&good());
        assert!(rows.iter().all(|row| row.state == RowState::Ok), "{rows:?}");
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            ["voice", "brain", "turbo", "characterVoice", "mode"]
        );
    }

    /// Each fault the test of 21.11 met as «говорит Алиса» is its own row with its own fix: the voice past DuoWave,
    /// a key that stopped working, the Turbo behind a firewall, Alice's voice instead of the character's.
    #[test]
    fn each_fault_is_its_own_row_with_its_fix() {
        let mut facts = good();
        facts.voice = Some(false);
        facts.brain = Way::Off;
        facts.brain_reason = Some("ключ больше не работает.".into());
        facts.turbos =
            vec![TurboFact { name: "Ноутбук".into(), up: false, error: Some("порт закрыт".into()) }];
        facts.own_voice = false;
        let rows = rows(&facts);
        assert_eq!(
            states(&rows)[..4],
            [
                ("voice", RowState::Bad, Some("pointLocal")),
                ("brain", RowState::Bad, Some("brain")),
                ("turbo", RowState::Bad, Some("firewall")),
                ("characterVoice", RowState::Warn, Some("voice")),
            ]
        );
        assert!(rows[1].text.contains("ключ больше не работает"), "{}", rows[1].text);
        assert!(rows[2].text.contains("Ноутбук") && rows[2].text.contains("порт закрыт"), "{}", rows[2].text);
    }

    /// No Turbo is a warning, not a fault: the station works without one. No model in «Алиса» is a warning too —
    /// Yandex answers there.
    #[test]
    fn no_turbo_and_no_model_in_alice_are_warnings() {
        let mut facts = good();
        facts.turbos.clear();
        facts.brain = Way::Off;
        facts.mode = PanelMode::Relay;
        let rows = rows(&facts);
        assert_eq!(
            states(&rows)[1..3],
            [("brain", RowState::Warn, Some("brain")), ("turbo", RowState::Warn, Some("turbo"))]
        );
    }

    /// The Turbo says why the station cannot reach it (Windows' firewall closed): with the pair's key only, and its
    /// words then stand in the row of a Turbo that is not on the line.
    #[tokio::test]
    async fn the_turbo_tells_the_centre_its_firewall_with_the_pair_key_only() {
        use axum::body::Body as HttpBody;
        use axum::extract::ConnectInfo;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let env = crate::config::Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
        let app = App::build(crate::config::Config::for_tests(&env)).unwrap();
        let key = app.placement.pair_key().unwrap();
        let send = |key: Option<String>| {
            let app = app.clone();
            async move {
                let body = json!({ "closed": true, "advice": "Сеть Windows «Общедоступная»: сделайте её частной." });
                let mut request = Request::builder()
                    .method("POST")
                    .uri("/api/v1/home/turbo-network")
                    .header("content-type", "application/json");
                if let Some(key) = key {
                    request = request.header("authorization", format!("Bearer {key}"));
                }
                let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
                request
                    .extensions_mut()
                    .insert(ConnectInfo(std::net::SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
                crate::app::router(app).oneshot(request).await.unwrap().status()
            }
        };
        assert_eq!(send(None).await, StatusCode::FORBIDDEN);
        assert_eq!(send(Some(key)).await, StatusCode::OK);
        assert!(turbo_says().unwrap_or_default().contains("Общедоступная"));
    }

    /// The route answers from a station centre with the rows and whether all are green.
    #[tokio::test]
    async fn the_route_answers_its_rows() {
        let env = crate::config::Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
        let app = App::build(crate::config::Config::for_tests(&env)).unwrap();
        let rows = rows(&facts(&app));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].id, "voice");
    }
}
