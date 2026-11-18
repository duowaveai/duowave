//! The home's one list «Установлено» (docs/plans/home-and-devices.md, stage 5а): the packages of the centre and of
//! each Турбо on the line, each with its place — the device it runs on. The centre reads its own; a Турбо's it reads
//! from the Турбо's contract (`GET /home/v1/plugins`, src/home_server/serve.rs). A Турбо asleep or kept out by the
//! regulator is named in `places` with `reached: false`: its apps are not listed, the panel says why. Installing stays
//! where it was; the install over the link comes with stage 5б.
//!
//! `GET /api/v1/home/apps` → `{apps: [{id, version, title, kind, runtime, enabled, state, place}], places: [...]}`.
//! A computer that is the Турбо of a station's home asks its centre, like the devices (devices.rs).
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};

use super::devices::{of_app, DeviceKind, HomeDevice};
use crate::app::App;
use crate::core::identity::permissions::STATION_VIEW;
use crate::helpers::HelperState;
use crate::platform::http::{ok, ApiResult, Caller};

/// Where a package runs: the device's id in the home's list, its name and kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPlace {
    pub device: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Its list was read now; false — asleep, kept out or not answering.
    pub reached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl AppPlace {
    fn of(device: &HomeDevice, reached: bool, why: Option<String>) -> AppPlace {
        AppPlace { device: device.id.clone(), name: device.name.clone(), kind: device.kind, reached, why }
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/home/apps", get(list))
}

/// One package of the list, from the centre's view or a Турбо's answer.
fn entry(package: &Value, place: &AppPlace) -> Value {
    let field = |name: &str| package.get(name).cloned().unwrap_or(Value::Null);
    json!({
        "id": field("id"),
        "version": field("version"),
        "title": field("title"),
        "kind": field("kind"),
        "runtime": field("runtime"),
        "enabled": field("enabled"),
        "state": field("state"),
        "place": place,
    })
}

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    // An ear (stage 4): the home's list is the centre's.
    if let Some((ear, key)) = super::ear::centre(&app) {
        if let Ok(view) = super::ear::client(&ear, &key).get("/api/v1/home/apps").await {
            if view.get("apps").is_some() {
                return Ok(ok(view));
            }
        }
    }
    #[cfg(feature = "pc")]
    if let Some(centre) = super::devices::centre_elsewhere(&app) {
        if let Some(client) = centre.client() {
            if let Ok(view) = client.get("/api/v1/home/apps").await {
                if view.get("apps").is_some() {
                    return Ok(ok(view));
                }
            }
        }
    }
    let devices = of_app(&app);
    let mut apps = Vec::new();
    let mut places = Vec::new();
    if let Some(centre) = devices.devices.iter().find(|device| device.id == devices.centre) {
        let place = AppPlace::of(centre, true, None);
        let own = app.plugins.view().await;
        for package in &own.packages {
            apps.push(entry(&serde_json::to_value(package).unwrap_or_default(), &place));
        }
        places.push(place);
    }
    let views = app.helpers.views();
    for setting in app.helpers.list() {
        let Some(device) = devices.devices.iter().find(|device| device.id == format!("turbo:{}", setting.id)) else {
            continue;
        };
        let view = views.iter().find(|view| view.id == setting.id);
        let up = view.is_some_and(|view| view.enabled && view.allowed && view.state == HelperState::Up);
        if !up {
            let why = match view {
                Some(view) if !view.enabled => crate::tr!("выключен"),
                Some(view) if !view.allowed => crate::tr!("не пускает «Что уходит из дома»"),
                _ => crate::tr!("не на связи"),
            };
            places.push(AppPlace::of(device, false, Some(why)));
            continue;
        }
        let Some(remote) = app.helpers.remote(&setting) else {
            places.push(AppPlace::of(device, false, Some(crate::tr!("нет ключа"))));
            continue;
        };
        let read = crate::utils::http::client()
            .get(format!("{}/home/v1/plugins", remote.base().trim_end_matches('/')))
            .bearer_auth(remote.credential())
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let answer = match read {
            Ok(response) if response.status().is_success() => response.json::<Value>().await.ok(),
            _ => None,
        };
        match answer.as_ref().and_then(|answer| answer.get("plugins")).and_then(Value::as_array) {
            Some(packages) => {
                let place = AppPlace::of(device, true, None);
                for package in packages {
                    apps.push(entry(package, &place));
                }
                places.push(place);
            }
            None => places.push(AppPlace::of(device, false, Some(crate::tr!("не ответил")))),
        }
    }
    Ok(ok(json!({ "apps": apps, "places": places })))
}

/// The faces here and their code on the Турбо (docs/plans/home-and-devices.md, stage 5б): every app installed at this
/// centre as its face — for a computer, on a station — is installed on the Турбо that gives the apps, at the face's
/// version; one there older is updated. Run after a package changed here and when the Турбо comes back: a Турбо asleep
/// at the install gets it when it wakes.
pub async fn sync_faces(app: &App) -> Result<usize, String> {
    let here = app.plugins.placement_now();
    let faces: Vec<(String, String)> = app
        .plugins
        .installed_all()
        .into_iter()
        .filter(|package| package.manifest.face_at(here))
        .map(|package| (package.manifest.id.clone(), package.manifest.version.to_string()))
        .collect();
    if faces.is_empty() {
        return Ok(0);
    }
    let Some((url, key)) = app.catalog.turbo_up() else { return Ok(0) };
    let base = url.trim_end_matches('/').to_string();
    let listed: Value = crate::utils::http::client()
        .get(format!("{base}/home/v1/plugins"))
        .bearer_auth(&key)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let there: Vec<(String, String, bool)> = listed
        .get("plugins")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    Some((
                        entry.get("id")?.as_str()?.to_string(),
                        entry.get("version").and_then(Value::as_str).unwrap_or_default().to_string(),
                        entry.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut sent = 0;
    for (id, version) in faces {
        let current = there.iter().find(|(there, _, _)| *there == id);
        if current.is_some_and(|(_, theirs, on)| *on && newer_or_same(theirs, &version)) {
            continue;
        }
        let answer = crate::utils::http::client()
            .post(format!("{base}/home/v1/plugins/install"))
            .bearer_auth(&key)
            .json(&json!({ "id": id, "version": version }))
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if answer.status().is_success() {
            tracing::info!(target: "home", "{id} {version}: its code installed on the Турбо");
            sent += 1;
        } else {
            let status = answer.status();
            let text: String = answer.text().await.unwrap_or_default().chars().take(200).collect();
            tracing::warn!(target: "home", "{id} {version}: the Турбо did not install it: {status} {text}");
        }
    }
    Ok(sent)
}

fn newer_or_same(theirs: &str, ours: &str) -> bool {
    match (semver::Version::parse(theirs), semver::Version::parse(ours)) {
        (Ok(theirs), Ok(ours)) => theirs >= ours,
        _ => theirs == ours,
    }
}

/// The faces' code follows: after a package changed here, and when a Турбо comes back.
pub fn watch(app: &Arc<App>) {
    let weak = Arc::downgrade(app);
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.events.on("home.apps", move |event| {
        let name = event.name();
        let moves = match &*event {
            crate::core::events::GatewayEvent::PackagesChanged { change, .. } => {
                !matches!(change.as_str(), "loaded" | "unloaded" | "failed" | "pressure" | "schedule")
            }
            _ => matches!(name, "helpers.up" | "helpers.changed"),
        };
        if !moves || busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let Some(app) = weak.upgrade() else {
            busy.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let busy = busy.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Err(error) = sync_faces(&app).await {
                tracing::info!(target: "home", "the apps' code on the Турбо: {error}");
            }
            busy.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    });
}
