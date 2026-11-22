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
//! - `mode` — who answers: «Алиса», «Алиса + ваш ИИ», «Ваш ИИ»;
//! - `wifi` — only on a station that lost its Wi-Fi in the last day: how often and how long, and the likely cause it
//!   sees itself — AliceBridge's agent stuck on a Wi-Fi change, a weak signal, too little free memory (a tester's
//!   Mini 2, 22.11; operations/network_watch.rs keeps the outages).
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
use crate::operations::network_watch::Outages;
use crate::operations::system_memory::{Counters, Pressure};
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
    /// The computer the row's action is about (its own id): «Разрешить в брандмауэре» is shown on that very computer,
    /// elsewhere the way to it — a home may have several Turbos.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turbo: Option<String>,
}

fn row(id: &'static str, state: RowState, text: String, action: Option<&'static str>) -> Row {
    Row { id, state, text, action, turbo: None }
}

/// A Turbo as the centre knows it: a helper of its list, or the computer a station leans on by the pair's key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurboFact {
    pub name: String,
    pub up: bool,
    pub error: Option<String>,
    /// The computer's own id, when the centre knows it.
    pub id: Option<String>,
    /// It told the centre its firewall or network keeps the station out (placement/watch.rs, `report_network`).
    pub firewall: bool,
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
    /// The Turbo's voice failed lately and Alice's spoke instead (station_session.rs, `with_alice_after`): why, and
    /// the computer whose voice it was.
    pub voice_failed: Option<VoiceFailed>,
    pub mode: crate::settings::definitions::PanelMode,
    /// A station's Wi-Fi drops of the last day; None — none, or not a station.
    pub wifi: Option<WifiFact>,
}

/// The Turbo's voice did not speak: Windows' application control kept its engine from starting (WinError 4551, Smart
/// App Control — 22.11, every answer of a Windows Turbo was silent), or another reason in its own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceFailed {
    pub blocked_by_windows: bool,
    pub why: String,
    /// The computer whose voice it was (its own id), when the centre knows it.
    pub turbo: Option<String>,
    pub name: Option<String>,
}

/// The Turbo's voice failed lately: when, and why in its words.
static VOICE_FAILED: std::sync::Mutex<Option<(std::time::Instant, String)>> = std::sync::Mutex::new(None);

/// A failure of the Turbo's voice is kept for the row this long after the last one.
const VOICE_FAILED_FOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// The Turbo's voice is not asked again for this long after it failed: the phrase goes to Alice's voice at once
/// instead of waiting for the same refusal every time.
pub const VOICE_REST: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// The station's answers tell how the Turbo's voice went: `Some(why)` — it failed and Alice spoke; `None` — it spoke.
pub fn note_turbo_voice(failed: Option<&str>) {
    let mut kept = VOICE_FAILED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *kept = failed.map(|why| (std::time::Instant::now(), why.chars().take(600).collect()));
}

/// The Turbo's voice failed within `within`: why.
pub fn turbo_voice_failed(within: std::time::Duration) -> Option<String> {
    let kept = VOICE_FAILED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.as_ref().filter(|(at, _)| at.elapsed() < within).map(|(_, why)| why.clone())
}

/// Windows' application control (Smart App Control, WDAC) refused a file: WinError 4551.
pub fn blocked_by_windows(why: &str) -> bool {
    why.contains("WinError 4551") || why.contains("Application Control policy")
}

/// The station's Wi-Fi in the last day, when it dropped: how often, how long, and the likely cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiFact {
    pub drops: usize,
    pub minutes: u64,
    pub hint: Option<String>,
}

/// A signal this weak or weaker drops (−80 dBm is about to).
const WEAK_DBM: f64 = -75.0;
/// Worth a row — and a card on the desk for a day: this many drops in the last day, or this many minutes without the
/// network. One router restart is not.
const WORTH_DROPS: usize = 3;
const WORTH_MINUTES: u64 = 5;
/// Where AliceBridge keeps its agent's settings (stations/connect/neighbours.rs reads it from a computer over SSH).
const ALICEBRIDGE_CONFIG: &str = "/data/quasar/alicebridge-agent/config.json";

/// The last day of outages at `now` (unix ms): how many there were in it and their minutes in it — one going on counts
/// to now.
pub fn day_of(outages: &Outages, now: u64) -> (usize, u64) {
    let since = now.saturating_sub(24 * 3600 * 1000);
    let spans: Vec<(u64, u64)> = outages
        .ended
        .iter()
        .filter(|outage| outage.to > since)
        .map(|outage| (outage.from.max(since), outage.to))
        .chain(outages.offline_since.map(|from| (from.max(since), now)))
        .collect();
    let ms: u64 = spans.iter().map(|(from, to)| to.saturating_sub(*from)).sum();
    (spans.len(), ms.div_ceil(60_000))
}

/// AliceBridge's agent stuck on a Wi-Fi change, as neighbours.rs's script reads it: the rollback phase with the
/// confirmation timed out — the station drops off the network every 40–50 s.
fn alicebridge_stuck(config: &str) -> bool {
    let compact: String = config.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("\"phase\":\"rolling_back\"") && config.contains("Wi-Fi confirmation timed out")
}

/// The likely cause of the drops, as far as the station sees it; None — nothing seen.
pub fn wifi_hint(alicebridge: Option<&str>, dbm: Option<f64>, memory: Option<&Counters>) -> Option<String> {
    if alicebridge.is_some_and(alicebridge_stuck) {
        return Some(crate::tr!("Похоже, виноват агент AliceBridge: он застрял на смене Wi-Fi, и колонка уходит из сети каждые 40–50 секунд. Задайте домашний Wi-Fi в приложении AliceBridge заново."));
    }
    if let Some(dbm) = dbm.filter(|dbm| *dbm <= WEAK_DBM) {
        return Some(crate::tr!(
            "Сигнал Wi-Fi слабый — {dbm} дБм: поставьте колонку ближе к роутеру.",
            dbm = dbm.round() as i64
        ));
    }
    if let Some(memory) = memory.filter(|memory| memory.pressure != Pressure::Normal) {
        return Some(crate::tr!(
            "Свободной ОЗУ мало — {mb} МБ: при нехватке памяти колонка может терять Wi-Fi.",
            mb = memory.pool_mb
        ));
    }
    None
}

/// On the station: its drops of the last day, when there were enough to be worth a row, with what it sees of their
/// cause.
fn wifi_fact() -> Option<WifiFact> {
    let outages = crate::operations::network_watch::shared()?.outages();
    let (drops, minutes) = day_of(&outages, crate::utils::time::now_ms());
    if drops < WORTH_DROPS && minutes < WORTH_MINUTES {
        return None;
    }
    let alicebridge = std::fs::read_to_string(ALICEBRIDGE_CONFIG).ok();
    let dbm = std::fs::read_to_string("/proc/net/wireless")
        .ok()
        .as_deref()
        .and_then(crate::operations::device_state::parse_wireless)
        .and_then(|wifi| wifi.dbm);
    let memory = crate::operations::system_memory::read_counters();
    Some(WifiFact { drops, minutes, hint: wifi_hint(alicebridge.as_deref(), dbm, memory.as_ref()) })
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
            row("voice", RowState::Warn, crate::tr!("Колонка сейчас не на связи с этим хабом."), Some("station"))
        }
        (None, _) => row("voice", RowState::Warn, crate::tr!("Колонки в доме пока нет."), Some("station")),
    });
    // A station that lost its Wi-Fi in the last day: what else fails may well be that.
    if let Some(wifi) = &facts.wifi {
        let said = crate::tr!(
            "За сутки колонка теряла Wi-Fi — пропаданий: {drops}, без сети всего: {minutes} мин.",
            drops = wifi.drops,
            minutes = wifi.minutes
        );
        let text = match &wifi.hint {
            Some(hint) => format!("{said} {hint}"),
            None => said,
        };
        rows.push(row("wifi", RowState::Warn, text, None));
    }
    let brain_title = facts.brain_title.clone().unwrap_or_else(|| crate::tr!("модель"));
    rows.push(match facts.brain {
        Way::Ready => row("brain", RowState::Ok, crate::tr!("ИИ отвечает: {title}.", title = brain_title), None),
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
                "ИИ не отвечает: {why}",
                why = facts.brain_reason.clone().unwrap_or_else(|| crate::tr!("модель не выбрана."))
            ),
            Some("brain"),
        ),
    });
    rows.push(turbo_row(&facts.turbos));
    rows.push(if let Some(failed) = &facts.voice_failed {
        voice_failed_row(failed)
    } else if facts.own_voice {
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

/// The Turbo's voice failed, Alice's spoke. Windows' application control: said plainly, with the way on that very
/// computer (`windowsVoice`, owner 22.11: tell what to do — keep Alice's voice or open Windows' settings, with a
/// warning). Another reason: its words, and the voice to choose.
fn voice_failed_row(failed: &VoiceFailed) -> Row {
    let name = failed.name.clone().unwrap_or_else(|| crate::tr!("Турбо"));
    if failed.blocked_by_windows {
        return Row {
            turbo: failed.turbo.clone(),
            ..row(
                "characterVoice",
                RowState::Warn,
                crate::tr!(
                    "Голос персонажа на «{name}» не запускается: защита Windows (Smart App Control) не пускает его файлы. Пока отвечает голос Алисы.",
                    name = name
                ),
                Some("windowsVoice"),
            )
        };
    }
    row(
        "characterVoice",
        RowState::Warn,
        crate::tr!(
            "Голос персонажа на «{name}» не ответил: {why}. Пока отвечает голос Алисы.",
            name = name,
            why = failed.why.trim_end_matches('.').chars().take(200).collect::<String>()
        ),
        Some("voice"),
    )
}

/// The Turbos in one row. A home may have several (22.11): one away while another helps is a warning, all away a
/// fault. «Разрешить в брандмауэре» only where the Turbo said its firewall keeps the station out (22.11: the button
/// stood by «не принимает ключ» on a Mac).
fn turbo_row(turbos: &[TurboFact]) -> Row {
    let names =
        |list: &[&TurboFact]| list.iter().map(|turbo| format!("«{}»", turbo.name)).collect::<Vec<_>>().join(", ");
    let up: Vec<&TurboFact> = turbos.iter().filter(|turbo| turbo.up).collect();
    let down =
        turbos.iter().find(|turbo| !turbo.up && turbo.firewall).or_else(|| turbos.iter().find(|turbo| !turbo.up));
    match (turbos.is_empty(), down) {
        (true, _) => row(
            "turbo",
            RowState::Warn,
            crate::tr!("Турбо нет: голос персонажа и слух — в облаке или у Алисы."),
            Some("turbo"),
        ),
        (false, Some(turbo)) => {
            let away = match &turbo.error {
                Some(error) => crate::tr!(
                    "Турбо «{name}» не на связи: {error}",
                    name = turbo.name.clone(),
                    error = error.trim_end_matches('.').to_string()
                ),
                None => crate::tr!("Турбо «{name}» не на связи", name = turbo.name.clone()),
            };
            let text = if up.is_empty() {
                format!("{away}.")
            } else {
                crate::tr!("{away}. Пока помогает {names}.", away = away, names = names(&up))
            };
            let state = if up.is_empty() { RowState::Bad } else { RowState::Warn };
            Row {
                turbo: turbo.firewall.then(|| turbo.id.clone()).flatten(),
                ..row("turbo", state, text, turbo.firewall.then_some("firewall"))
            }
        }
        (false, None) if up.len() == 1 => {
            row("turbo", RowState::Ok, crate::tr!("Турбо «{name}» на связи.", name = up[0].name.clone()), None)
        }
        (false, None) => row("turbo", RowState::Ok, crate::tr!("Турбо на связи: {names}.", names = names(&up)), None),
    }
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
    // Who each computer is: the id its entry keeps, else the one its last look read.
    let named: std::collections::BTreeMap<String, String> =
        app.helpers.list().into_iter().filter_map(|entry| Some((entry.id, entry.helper_id?))).collect();
    let mut turbos: Vec<TurboFact> = app
        .helpers
        .views()
        .into_iter()
        .filter(|helper| helper.enabled && !helper.builtin)
        .filter(|helper| !matches!(helper.state, HelperState::Off | HelperState::Itself))
        .map(|helper| {
            let id = named.get(&helper.id).cloned().or_else(|| helper.helper.as_ref().map(|me| me.id.clone()));
            // A report from a computer that did not name itself (before 0.18.2) is the old door's.
            let said = turbo_says(id.as_deref())
                .or_else(|| (helper.id == crate::helpers::HOME_COMPUTER).then(|| turbo_says(None)).flatten());
            let up = helper.state == HelperState::Up;
            TurboFact {
                name: helper.name,
                up,
                firewall: !up && said.is_some(),
                // A Turbo out of reach that said why (Windows' firewall, a public network): its words, not the look's.
                error: if up { helper.error } else { said.or(helper.error) },
                id,
            }
        })
        .collect();
    // A station's computer by the pair's key (`placement/helper`): its door, `homeServer`.
    if let Some(home) =
        placement.get("homeServer").filter(|home| home.get("enabled").and_then(Value::as_bool) != Some(false))
    {
        let state = home.get("state").and_then(Value::as_str).unwrap_or("none");
        if state != "none" && turbos.is_empty() {
            let said = if state == "up" { None } else { turbo_says(None) };
            turbos.push(TurboFact {
                name: crate::tr!("Компьютер"),
                up: state == "up",
                firewall: said.is_some(),
                error: said.or_else(|| home.get("error").and_then(Value::as_str).map(str::to_string)),
                id: None,
            });
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
        voice_failed: voice_failed(app),
        mode: app.modes.current(),
        wifi: if on_station { wifi_fact() } else { None },
    }
}

/// The Turbo's voice failed lately, with the computer that gives the voice now (the first on the list).
fn voice_failed(app: &App) -> Option<VoiceFailed> {
    let why = turbo_voice_failed(VOICE_FAILED_FOR)?;
    let giver = app.helpers.offer(crate::helpers::contract::Ability::Voice).and_then(|giver| match giver {
        crate::helpers::Giver::Helper(remote) => Some(remote.id().to_string()),
        crate::helpers::Giver::Itself => None,
    });
    let entry = giver.and_then(|id| app.helpers.entry(&id));
    Some(VoiceFailed {
        blocked_by_windows: blocked_by_windows(&why),
        turbo: entry.as_ref().and_then(|entry| entry.helper_id.clone()),
        name: entry.map(|entry| entry.name),
        why,
    })
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/home/status", get(status))
        .route("/api/v1/home/turbo-network", axum::routing::post(turbo_network))
}

/// What a Turbo told of its network last (placement/watch.rs, `report_network`): why the station may not reach it.
struct TurboNetwork {
    closed: bool,
    advice: Option<String>,
    at: std::time::Instant,
}

/// Each computer's last report, by its own id ("" — a computer before 0.18.2 that did not name itself).
static TURBO_NETWORK: std::sync::Mutex<std::collections::BTreeMap<String, TurboNetwork>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

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
    let who = body.pointer("/helper/id").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    let mut kept = TURBO_NETWORK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.retain(|_, report| report.at.elapsed() < REPORT_FOR);
    // A few computers at most: a home's own.
    if kept.len() < 16 || kept.contains_key(who) {
        kept.insert(who.chars().take(64).collect(), TurboNetwork { closed, advice, at: std::time::Instant::now() });
    }
    Ok(ok(json!({ "kept": true })))
}

/// Why this computer (by its own id; None — one that did not name itself) may be out of reach, as it said lately.
fn turbo_says(who: Option<&str>) -> Option<String> {
    let kept = TURBO_NETWORK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.get(who.unwrap_or_default())
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
            turbos: vec![TurboFact { name: "Ноутбук".into(), up: true, ..TurboFact::default() }],
            own_voice: true,
            voice_failed: None,
            mode: PanelMode::Gateway,
            wifi: None,
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
        facts.turbos = vec![TurboFact {
            name: "Ноутбук".into(),
            error: Some("порт закрыт".into()),
            id: Some("pc-1".into()),
            firewall: true,
            ..TurboFact::default()
        }];
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
        assert_eq!(rows[2].turbo.as_deref(), Some("pc-1"), "the firewall's button on that very computer");
    }

    /// The Turbo's voice refused by Windows (22.11, WinError 4551 from a Windows Turbo, every answer silent): the row
    /// says so with the way on that computer; another failure — its words and the voice to choose. Alice speaks meanwhile.
    #[test]
    fn a_turbo_voice_windows_blocked_is_said_plainly_with_the_way_on_that_computer() {
        let mut facts = good();
        let why =
            "own speech: HTTP 500 {\"error\": \"[WinError 4551] An Application Control policy has blocked this file\"}";
        assert!(blocked_by_windows(why));
        facts.voice_failed = Some(VoiceFailed {
            blocked_by_windows: true,
            why: why.into(),
            turbo: Some("gw-win".into()),
            name: Some("Компьютер Windows".into()),
        });
        let voice = rows(&facts).into_iter().find(|row| row.id == "characterVoice").unwrap();
        assert_eq!(
            (voice.state, voice.action, voice.turbo.as_deref()),
            (RowState::Warn, Some("windowsVoice"), Some("gw-win"))
        );
        assert!(voice.text.contains("Smart App Control") && voice.text.contains("голос Алисы"), "{}", voice.text);

        facts.voice_failed =
            Some(VoiceFailed {
                blocked_by_windows: false, why: "Турбо ответило 500.".into(), turbo: None, name: None
            });
        let voice = rows(&facts).into_iter().find(|row| row.id == "characterVoice").unwrap();
        assert_eq!(voice.action, Some("voice"));
        assert!(voice.text.contains("Турбо ответило 500") && !voice.text.contains(".."), "{}", voice.text);
    }

    /// A home with several Turbos (22.11): all on the line — one green row with their names; one away while another
    /// helps — a warning that says who helps; all away — a fault. The firewall's button only for a Turbo that said
    /// its firewall keeps the station out, never beside a key or a silence (22.11: it stood by «не принимает ключ»).
    #[test]
    fn several_turbos_share_one_row_and_the_firewall_button_is_only_for_a_firewall() {
        let turbo = |name: &str, up: bool, error: Option<&str>, firewall: bool| TurboFact {
            name: name.into(),
            up,
            error: error.map(str::to_string),
            id: Some(format!("pc-{name}")),
            firewall,
        };
        let row_of = |turbos: Vec<TurboFact>| turbo_row(&turbos);

        let both = row_of(vec![turbo("Mac", true, None, false), turbo("Windows", true, None, false)]);
        assert_eq!((both.state, both.action), (RowState::Ok, None));
        assert!(both.text.contains("«Mac», «Windows»"), "{}", both.text);

        let one_away =
            row_of(vec![turbo("Mac", false, Some("не ответило вовремя"), false), turbo("Windows", true, None, false)]);
        assert_eq!((one_away.state, one_away.action, one_away.turbo.as_deref()), (RowState::Warn, None, None));
        assert!(
            one_away.text.contains("«Mac» не на связи") && one_away.text.contains("помогает «Windows»"),
            "{}",
            one_away.text
        );

        let closed = row_of(vec![
            turbo("Mac", true, None, false),
            turbo("Windows", false, Some("Сделайте сеть частной."), true),
        ]);
        assert_eq!((closed.action, closed.turbo.as_deref()), (Some("firewall"), Some("pc-Windows")));

        let all_away = row_of(vec![turbo("Mac", false, Some("ключ"), false)]);
        assert_eq!((all_away.state, all_away.action), (RowState::Bad, None));
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
    /// A tester's Mini 2, 22.11: its Wi-Fi dropped for minutes many times a day. The station's row says how often and
    /// how long in the last day, right after the voice, and what it sees of the cause.
    #[test]
    fn a_station_that_lost_its_wifi_says_how_often_how_long_and_why() {
        use crate::operations::network_watch::Outage;
        let hour = 3600 * 1000;
        let now = 30 * hour;
        let outages = Outages {
            // One going on for 2 minutes, one of 3 minutes an hour ago, one more than a day ago — not counted.
            offline_since: Some(now - 2 * 60_000),
            ended: [
                Outage { from: now - 26 * hour, to: now - 26 * hour + 60_000 },
                Outage { from: now - hour, to: now - hour + 3 * 60_000 },
            ]
            .into(),
        };
        assert_eq!(day_of(&outages, now), (2, 5));
        assert_eq!(day_of(&Outages::default(), now), (0, 0));

        let mut facts = good();
        facts.wifi = Some(WifiFact { drops: 2, minutes: 5, hint: Some("Сигнал слабый.".into()) });
        let rows = rows(&facts);
        assert_eq!(rows[1].id, "wifi");
        assert_eq!(rows[1].state, RowState::Warn);
        assert!(rows[1].text.contains("пропаданий: 2") && rows[1].text.contains("5 мин"), "{}", rows[1].text);
        assert!(rows[1].text.ends_with("Сигнал слабый."), "{}", rows[1].text);

        // The cause: AliceBridge stuck first, then a weak signal, then too little memory.
        let stuck = r#"{"transaction": {"phase": "rolling_back", "message": "Wi-Fi confirmation timed out"}}"#;
        let low = Counters { total_mb: 120, pool_mb: 9, pressure: Pressure::Critical, swap_in_bytes: None };
        assert!(wifi_hint(Some(stuck), Some(-80.0), Some(&low)).unwrap().contains("AliceBridge"));
        let rolling_back_only = r#"{"transaction": {"phase": "rolling_back", "message": "rolling back"}}"#;
        assert!(wifi_hint(Some(rolling_back_only), Some(-80.0), None).unwrap().contains("-80"));
        assert!(wifi_hint(None, Some(-60.0), Some(&low)).unwrap().contains("9 МБ"));
        assert_eq!(wifi_hint(None, Some(-60.0), None), None);
    }

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
        assert!(turbo_says(None).unwrap_or_default().contains("Общедоступная"), "a computer that did not name itself");
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
