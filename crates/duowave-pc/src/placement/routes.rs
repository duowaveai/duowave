//! The placement's routes with the computer's own: the moves, its stations' updates and voices, pairing a station or
//! a computer of the home, «Поиск дома»'s probe, the desktop's «не засыпать», the helper's switch.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

pub use duowave_home::placement::routes::*;

use crate::config::Config;
use crate::core::identity::home::Identity;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATIONS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};

pub fn router<S>() -> Router<S>
where
    S: crate::placement::RouteNeeds + duowave_home::placement::RouteNeeds,
{
    let computer = Router::new()
        .route("/api/v1/placement/move", post(pc::start_move))
        .route("/api/v1/placement/move/{id}", get(pc::move_job))
        .route("/api/v1/placement/auto-update", post(pc::auto_update))
        .route("/api/v1/placement/update-station", post(pc::update_station))
        .route("/api/v1/placement/inspect", post(pc::inspect))
        .route("/api/v1/placement/station-voice", post(pc::station_voice))
        .route("/api/v1/placement/link", post(pc::link))
        .route("/api/v1/placement/pair-station", post(pc::pair_station))
        .route("/api/v1/placement/pair-code", post(pc::pair_code))
        .route("/api/v1/placement/pair-code/{id}", get(pc::pair_code_look).post(pc::pair_code_enter))
        .route("/api/v1/placement/pair-code/{id}/say", post(pc::pair_code_say))
        .route("/api/v1/placement/probe", get(pc::probe))
        .route("/api/v1/desktop/awake", get(pc::awake).put(pc::set_awake))
        .route("/api/v1/placement/helper", post(pc::helper))
        .route("/api/v1/placement/retry", post(pc::retry));
    duowave_home::placement::routes::router::<S>().merge(computer.layer(DefaultBodyLimit::max(BODY_LIMIT)))
}

pub(crate) fn chosen(
    station_gateway: &Arc<crate::placement::watch::StationGateways>,
    pick: &Pick,
) -> Result<Arc<super::watch::StationGateway>, ApiError> {
    station_gateway
        .pick(pick.station.as_deref())
        .ok_or_else(|| ApiError::conflict(crate::tr!("Какая колонка? Хаб есть на нескольких — укажите её (station).")))
}

pub mod pc {
    use super::*;
    use crate::access::AccessService;
    use crate::core::identity::permissions::SYSTEM_UPDATE;
    use crate::home::join::Joins;
    use crate::placement::moves::{MoveJob, MoveRequest, MoveTo};
    use crate::stations::connect::quasar_config::{parse_gateway_host, parse_gateway_host_or, plain_address};

    pub async fn start_move(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(station_connect): Uses<Arc<crate::stations::connect::station_connect::StationConnectService>>,
        Uses(moves): Uses<Arc<crate::placement::moves::MovesService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<MoveJob> {
        caller.permitted(STATIONS_MANAGE)?;
        let to: MoveTo = serde_json::from_value(body.get("to").cloned().unwrap_or(Value::Null))
            .map_err(|_| ApiError::bad_request(crate::tr!("to: station, stationServer, pc, yandex или ear")))?;
        let picked = body.get("station").and_then(Value::as_str);
        let record = station_gateway.pick(picked).and_then(|station| station.record());
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| record.as_ref().map(|record| record.address.clone()))
            .and_then(|address| plain_address(&address))
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let gateway = match body.get("gateway").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()) {
            // Typed without a port: this gateway's own port, not 80.
            Some(text) => Some(
                parse_gateway_host_or(text, station_connect.port())
                    .ok_or_else(|| ApiError::bad_request(crate::tr!("gateway: адрес[:порт]")))?,
            ),
            // Nobody typed it: this computer's address in the station's network.
            None if to != MoveTo::Yandex => {
                let addresses = station_connect.home_addresses(
                    &crate::stations::connect::station_connect::host_interfaces(),
                    crate::stations::connect::station_connect::in_container(),
                );
                crate::placement::watch::pick_home(&addresses, &address).and_then(|text| parse_gateway_host(&text))
            }
            None => None,
        };
        // The home's settings live at its centre, never on a computer (docs/plans/home-and-devices.md, stage 3).
        let settings_home = crate::settings::definitions::SettingsHome::Station;
        // «Ещё колонка» (stage 5): the centre the new ear joins — found before the move links the station here.
        let ear_of = if to == MoveTo::Ear {
            let (address, port, key, name) = crate::home::ear::centre_for_ear(&station_gateway)?;
            Some(crate::placement::moves::EarTarget { address, port, key, name })
        } else {
            None
        };
        // An ear leans on the centre, not on this computer as its server.
        let gateway = if to == MoveTo::Ear { None } else { gateway };
        let request = MoveRequest {
            to,
            address,
            gateway,
            settings_home,
            remove: body.get("remove").and_then(Value::as_bool).unwrap_or(false),
            // «Заменить дом на колонке»: only this way a move covers a station's own home (stage 0).
            replace_home: body.get("replaceHome").and_then(Value::as_bool).unwrap_or(false),
            ear_of,
        };
        Ok(created(moves.start(request, caller.actor())?))
    }

    pub async fn move_job(
        Uses(moves): Uses<Arc<crate::placement::moves::MovesService>>,
        caller: Caller,
        axum::extract::Path(id): axum::extract::Path<String>,
    ) -> ApiResult<MoveJob> {
        caller.permitted(STATION_VIEW)?;
        Ok(ok(moves.job(&id)?))
    }

    pub async fn auto_update(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(SYSTEM_UPDATE)?;
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
        chosen(&station_gateway, &pick)?.set_auto_update(on, caller.actor())?;
        Ok(created(json!({ "on": on })))
    }

    /// «Обновить колонку»: `{version}` — what the station's own panel saw offered, when it asks through this computer.
    pub async fn update_station(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(SYSTEM_UPDATE)?;
        let wanted = body.get("version").and_then(Value::as_str).map(str::trim).filter(|version| !version.is_empty());
        let started = chosen(&station_gateway, &pick)?.start_update(wanted, caller.actor()).await?;
        Ok(created(json!({ "updating": true, "way": started.way, "version": started.version })))
    }

    fn station_address(
        station_gateway: &Arc<crate::placement::watch::StationGateways>,
        pick: &Pick,
    ) -> Result<String, ApiError> {
        chosen(station_gateway, pick)?
            .record()
            .map(|record| record.address)
            .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб не на колонке.")))
    }

    /// «Что случилось»: the station's own account, over SSH.
    pub async fn inspect(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(moves): Uses<Arc<crate::placement::moves::MovesService>>,
        caller: Caller,
        Query(pick): Query<Pick>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&station_gateway, &pick)?;
        Ok(created(moves.inspect(&address).await.map_err(ApiError::unavailable)?))
    }

    /// The station's voice from the computer's panel: back to its own gateway, or to Yandex — the station's gateway
    /// does it, asked with the pair's key.
    pub async fn station_voice(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let path = match body.get("to").and_then(Value::as_str) {
            Some("local") => "/api/v1/placement/point-local",
            Some("yandex") => "/api/v1/placement/give-back",
            _ => return Err(ApiError::bad_request(crate::tr!("to: local или yandex"))),
        };
        let client = chosen(&station_gateway, &pick)?
            .client()
            .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб не на колонке.")))?;
        let answer = client.post(path, json!({})).await.map_err(ApiError::unavailable)?;
        Ok(created(answer))
    }

    /// A gateway put on the station by hand (yse, `duowave station install`): the computer links to it with its
    /// address and the pair's key, without SSH — it then watches it, keeps its settings' copies, updates it.
    pub async fn link(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let key = body.get("key").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if key.len() < 16 {
            return Err(ApiError::bad_request(crate::tr!("key: ключ пары — PEER_KEY из .env DuoWave на колонке")));
        }
        let view = station_gateway.link(&address, port, key, caller.actor()).await.map_err(ApiError::conflict)?;
        Ok(created(serde_json::to_value(view)?))
    }

    /// First run with a gateway already on the station: the computer signs in to the station's panel with the admin's
    /// password, takes the pair's key (`POST /api/v1/placement/pair` there) and links — no SSH, no key copied by hand.
    /// The password goes to the station only and is not kept.
    pub async fn pair_station(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let password = body.get("password").and_then(Value::as_str).unwrap_or("");
        if password.is_empty() {
            return Err(ApiError::bad_request(crate::tr!("password: пароль пульта колонки")));
        }
        let name = body.get("name").and_then(Value::as_str).map(str::trim).filter(|name| !name.is_empty());
        let base = format!("http://{address}:{port}");
        let client = crate::utils::http::client();
        let login = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({ "name": name, "password": password }))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ApiError::unavailable(crate::tr!("DuoWave на колонке не отвечает по этому адресу.")))?;
        let status = login.status();
        let answer: Value = login.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let said = answer
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!("пароль не подошёл"));
            return Err(ApiError::conflict(crate::tr!("Пульт колонки не пустил: {said}", said = said)));
        }
        let token = answer.pointer("/message/token").and_then(Value::as_str).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Пульт колонки ответил без входа: нужен пароль администратора."))
        })?;
        let paired = client
            .post(format!("{base}/api/v1/placement/pair"))
            .bearer_auth(token)
            .json(&json!({}))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ApiError::unavailable(crate::tr!("DuoWave на колонке не отвечает по этому адресу.")))?;
        let ok_pair = paired.status().is_success();
        let answer: Value = paired.json().await.unwrap_or(Value::Null);
        let Some(key) = answer.pointer("/message/key").and_then(Value::as_str).filter(|_| ok_pair) else {
            let said = answer
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!("DuoWave на колонке постарше: обновите его"));
            return Err(ApiError::conflict(crate::tr!("Ключ пары колонка не отдала: {said}", said = said)));
        };
        let view = station_gateway.link(&address, port, key, caller.actor()).await.map_err(ApiError::conflict)?;
        Ok(created(serde_json::to_value(view)?))
    }

    /// «Не давать компьютеру спать» of the desktop app, from the helper's panel (docs/plans/desk.md, stage 4): the
    /// app's own setting beside the gateway's data (desktop.json, `keepAwake`, on unless switched off); the app reads
    /// it again every few seconds and holds the computer awake or lets it sleep.
    fn desktop_json(config: &Arc<Config>) -> Result<std::path::PathBuf, ApiError> {
        if config.placement.managed_by.as_deref() != Some("desktop") {
            return Err(ApiError::conflict(crate::tr!("Это настройка приложения DuoWave на компьютере.")));
        }
        config
            .data_dir
            .as_ref()
            .map(|dir| dir.join("desktop.json"))
            .ok_or_else(|| ApiError::conflict(crate::tr!("Нет папки данных.")))
    }

    fn read_desktop(path: &std::path::Path) -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    pub async fn awake(Uses(config): Uses<Arc<Config>>, caller: Caller) -> ApiResult<Value> {
        caller.permitted(STATION_VIEW)?;
        let Ok(path) = desktop_json(&config) else { return Ok(ok(json!({ "managed": false }))) };
        let on = read_desktop(&path).get("keepAwake").and_then(Value::as_bool).unwrap_or(true);
        Ok(ok(json!({ "managed": true, "keepAwake": on })))
    }

    pub async fn set_awake(
        Uses(config): Uses<Arc<Config>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(SETTINGS_EDIT)?;
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
        let path = desktop_json(&config)?;
        let mut settings = read_desktop(&path);
        settings["keepAwake"] = Value::Bool(on);
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&settings).unwrap_or_default()))
            .map_err(|error| ApiError::unavailable(crate::tr!("Настройка не записалась: {error}", error = error)))?;
        Ok(ok(json!({ "managed": true, "keepAwake": on })))
    }

    /// Is a DuoWave gateway on this station already? Its /health answers in our shape: the first run on a computer
    /// offers to help it rather than take its place.
    pub async fn probe(
        caller: Caller,
        Query(query): Query<std::collections::HashMap<String, String>>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = query
            .get("address")
            .and_then(|address| plain_address(address))
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port = query.get("port").and_then(|port| port.parse::<u16>().ok()).unwrap_or(8790);
        let answer = crate::utils::http::client()
            .get(format!("http://{address}:{port}/health"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
        let Ok(answer) = answer else { return Ok(ok(json!({ "gateway": false }))) };
        let health: Value = answer.json().await.unwrap_or(Value::Null);
        let (gateway, version) = recognise(&health);
        // A centre or an ear (stage 6): an ear's centre is the home to offer, not the ear.
        let status = health.get("message").filter(|message| message.is_object()).unwrap_or(&health);
        let ear_of = status.pointer("/placement/earOf").filter(|ear| !ear.is_null()).cloned();
        let home = status.get("home").cloned();
        Ok(ok(json!({ "gateway": gateway, "version": version, "port": port, "earOf": ear_of, "home": home })))
    }

    /// A /health answer: a DuoWave gateway that is not a computer's, and its version. The answer comes in the API's
    /// envelope ({code, message}); a gateway from before the envelope answered bare.
    pub fn recognise(health: &Value) -> (bool, Option<&str>) {
        let status = health.get("message").filter(|message| message.is_object()).unwrap_or(health);
        let role = status.pointer("/placement/role").and_then(Value::as_str);
        let version = status.pointer("/build/version").and_then(Value::as_str);
        (version.is_some() && role != Some("pc"), version)
    }

    /// «ПК помогает колонке»: on — the station leans on this computer (a key made here first, if it has none of
    /// ours); off — it works alone, the address and the key kept.
    pub async fn helper(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(access): Uses<Arc<AccessService>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let station = chosen(&station_gateway, &pick)?;
        // «Что даю колонке»: слух, голос, модель и сообщения по отдельности — без переключения самой помощи.
        if let Some(without) = body.get("without").and_then(Value::as_array) {
            let without: Vec<String> = without.iter().filter_map(Value::as_str).map(str::to_string).collect();
            station.set_helper_parts(without, caller.actor()).await.map_err(ApiError::unavailable)?;
            if body.get("on").is_none() {
                return Ok(created(serde_json::to_value(station.view())?));
            }
        }
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
        switch_helper(&access, &station, on, caller.actor()).await?;
        Ok(created(serde_json::to_value(station.view())?))
    }

    /// «ПК помогает колонке» on or off: on — a key made here first, if the station has none of ours.
    async fn switch_helper(
        access: &AccessService,
        station: &super::super::watch::StationGateway,
        on: bool,
        actor: Option<crate::core::events::Actor>,
    ) -> Result<(), ApiError> {
        let key = if on && station.needs_home_key() {
            let created = access.keys.create(
                &serde_json::Map::from_iter([
                    ("name".into(), json!("Колонка: домашний сервер")),
                    ("grants".into(), json!(["home.models"])),
                ]),
                actor.clone(),
            )?;
            Some((created.secret, created.key.id))
        } else {
            None
        };
        station.set_helper(on, key, actor).await.map_err(ApiError::unavailable)
    }

    // ─── Into the home by a code (docs/plans/home-setup-flow.md, stages 2 and 3) ───

    /// A call to the home's centre for a request into it: its answer's `message`, or its refusal in its own words and
    /// status.
    async fn to_centre(method: reqwest::Method, url: &str, body: Option<Value>) -> Result<Value, ApiError> {
        let mut request = crate::utils::http::client().request(method, url).timeout(std::time::Duration::from_secs(15));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let answer = request
            .send()
            .await
            .map_err(|_| ApiError::unavailable(crate::tr!("Хаб дома не отвечает по этому адресу.")))?;
        let status = answer.status();
        let value: Value = answer.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value.get("message").cloned().unwrap_or(value));
        }
        let said = value.get("message").and_then(Value::as_str).map(str::to_string);
        Err(ApiError::new(
            status,
            said.unwrap_or_else(|| crate::tr!("Хаб дома отказал: {status}.", status = status.as_u16())),
        ))
    }

    /// The host of this computer's address in the centre's network, as `pick_home` names it with this gateway's port
    /// (`192.168.1.104:7780`): the link listens on its own port (the two-computer check 22.11 offered
    /// `https://192.168.1.104:7780:7781`, which the centre refused).
    pub fn link_host(best: &str) -> &str {
        best.rsplit_once(':').filter(|(_, port)| port.parse::<u16>().is_ok()).map_or(best, |(host, _)| host)
    }

    /// What answers at that address: `pc` (a computer's or a server's gateway), `station`, or nothing known.
    async fn centre_role(address: &str, port: u16) -> Option<String> {
        let answer = crate::utils::http::client()
            .get(format!("http://{address}:{port}/health"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        let health: Value = answer.json().await.ok()?;
        let status = health.get("message").filter(|message| message.is_object()).unwrap_or(&health);
        status.pointer("/placement/role").and_then(Value::as_str).map(str::to_string)
    }

    /// The pair's key a centre gave: the station is linked as today's `pair-station` links it, and this computer
    /// helps it at once — «Что даёт этот компьютер» comes next in the panel, no switch to find.
    async fn joined(
        station_gateway: &Arc<crate::placement::watch::StationGateways>,
        events: &crate::core::events::EventBus,
        access: &AccessService,
        outgoing: &crate::home::join::Outgoing,
        grant: &Value,
        actor: Option<crate::core::events::Actor>,
    ) -> Result<Value, ApiError> {
        // A computer centre paired with this computer over the link (its own connection key): nothing to link here —
        // the centre dials this computer, and it helps that home from now on («Помогаю»).
        if grant.get("helper").is_some() && grant.get("key").is_none() {
            return Ok(json!({ "joined": true, "helper": true, "address": outgoing.address, "port": outgoing.port }));
        }
        let key = grant.get("key").and_then(Value::as_str).filter(|key| key.len() >= 16).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Хаб дома впустил, но не дал ключ пары: обновите DuoWave на нём."))
        })?;
        if outgoing.kind == crate::home::join::JoinKind::Ear {
            return joined_as_ear(station_gateway, events, outgoing, key, actor).await;
        }
        let view = station_gateway
            .link(&outgoing.address, outgoing.port, key, actor.clone())
            .await
            .map_err(ApiError::conflict)?;
        // By the key the link gave it: two stations may share an address (a test's sockets), never a key.
        let station = station_gateway.find(&view.key).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Колонка связалась, но её не видно среди колонок этого компьютера."))
        })?;
        // Helping on at once (stage 3): a failure leaves the link — the switch is in «Что даёт этот компьютер».
        let helping = switch_helper(access, &station, true, actor).await;
        if let Err(error) = &helping {
            tracing::warn!(target: "placement", "pair-code: linked, helping not switched on: {}", error.message);
        }
        let mut answer = serde_json::to_value(view)?;
        answer["joined"] = json!(true);
        answer["helping"] = json!(helping.is_ok());
        Ok(answer)
    }

    /// A station with DuoWave of its own let this computer in (stage 5): it is linked with its pair key and becomes the
    /// centre's ear — the centre found before the link, so the new record is never taken for it. Its settings,
    /// people and characters give way to the home's copy; its memory is merged (src/home/copy.rs).
    async fn joined_as_ear(
        station_gateway: &Arc<crate::placement::watch::StationGateways>,
        events: &crate::core::events::EventBus,
        outgoing: &crate::home::join::Outgoing,
        key: &str,
        actor: Option<crate::core::events::Actor>,
    ) -> Result<Value, ApiError> {
        let (address, port, centre_key, name) = crate::home::ear::centre_for_ear(station_gateway)?;
        if address == outgoing.address && port == outgoing.port {
            return Err(ApiError::conflict(crate::tr!("Это и есть хаб дома.")));
        }
        let view =
            station_gateway.link(&outgoing.address, outgoing.port, key, actor).await.map_err(ApiError::conflict)?;
        let client = station_gateway.find(&view.key).and_then(|station| station.client()).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Колонка связалась, но её не видно среди колонок этого компьютера."))
        })?;
        client
            .put("/api/v1/home/ear", json!({ "address": address, "port": port, "key": centre_key, "name": name }))
            .await
            .map_err(ApiError::unavailable)?;
        events.emit(crate::core::events::GatewayEvent::HomeDevicesChanged { reason: "join".into() });
        let mut answer = serde_json::to_value(view)?;
        answer["joined"] = json!(true);
        answer["ear"] = json!(true);
        Ok(answer)
    }

    /// «Дом уже есть»: this computer asks the home's centre in as its Turbo (`POST /api/v1/home/join` there); the
    /// centre tells a code. `{address, port?}` → `{id, expiresAt, tries, told}`.
    pub async fn pair_code(
        Uses(station_connect): Uses<Arc<crate::stations::connect::station_connect::StationConnectService>>,
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(identity): Uses<Arc<Identity>>,
        Uses(link_service): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
        Uses(joins): Uses<Arc<Joins>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let addresses = station_connect.home_addresses(
            &crate::stations::connect::station_connect::host_interfaces(),
            crate::stations::connect::station_connect::in_container(),
        );
        let mine = crate::placement::watch::pick_home(&addresses, &address);
        // «Ещё колонка» (stage 5): this computer, the home's Turbo, takes a station with DuoWave of its own in as the
        // centre's ear — that station says the code, and it asks in the home's name.
        let kind = match body.get("kind").and_then(Value::as_str) {
            Some("ear") => crate::home::join::JoinKind::Ear,
            _ => crate::home::join::JoinKind::Turbo,
        };
        let name = match kind {
            crate::home::join::JoinKind::Ear => {
                let (_, _, _, centre) = crate::home::ear::centre_for_ear(&station_gateway)?;
                centre.unwrap_or_else(|| identity.home_name())
            }
            crate::home::join::JoinKind::Turbo => identity.as_helper().name,
        };
        // A centre on a computer or a server (stage 6 finds those): it pairs with this computer's own connection key
        // over the link, as «Подключить Турбо» with that key does — offered with the request, armed for its minutes.
        let link = if kind == crate::home::join::JoinKind::Turbo
            && centre_role(&address, port).await.as_deref() == Some("pc")
        {
            let link_port = link_service.port().filter(|_| link_service.helps()).ok_or_else(|| {
                ApiError::conflict(crate::tr!(
                    "Этот компьютер сейчас не принимает связь Турбо: обновите или перезапустите DuoWave."
                ))
            })?;
            let host = mine
                .as_deref()
                .map(link_host)
                .ok_or_else(|| ApiError::conflict(crate::tr!("Не нашёл адрес этого компьютера в сети хаба.")))?;
            let url = format!("https://{host}:{link_port}");
            Some(json!({ "url": url, "key": link_service.connection_key(true).shown() }))
        } else {
            None
        };
        let asked = to_centre(
            reqwest::Method::POST,
            &format!("http://{address}:{port}/api/v1/home/join"),
            Some(json!({ "kind": kind.as_str(), "name": name, "address": mine, "link": link })),
        )
        .await
        .map_err(|error| match error.status.as_u16() {
            // A centre from before the codes: the panel offers the password of its panel instead.
            404 | 405 => ApiError::conflict(crate::tr!(
                "DuoWave на колонке постарше и кода не знает: обновите его — или подключите по паролю пульта колонки."
            )),
            _ => error,
        })?;
        let id = asked.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let token = asked.get("token").and_then(Value::as_str).unwrap_or_default().to_string();
        if id.is_empty() || token.is_empty() {
            return Err(ApiError::conflict(crate::tr!("Хаб дома ответил без запроса: обновите DuoWave на нём.")));
        }
        let expires_ms = asked
            .get("expiresAt")
            .and_then(Value::as_str)
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp_millis().max(0) as u64)
            .unwrap_or_else(|| crate::utils::time::now_ms() + crate::home::join::CODE_MS);
        let outgoing = crate::home::join::Outgoing { address: address.clone(), port, token, expires_ms, kind };
        joins.keep_outgoing(&id, outgoing, crate::utils::time::now_ms());
        tracing::info!(target: "placement", "pair-code: asked {address}:{port} ({})", kind.as_str());
        Ok(created(json!({
            "id": id,
            "address": address,
            "port": port,
            "expiresAt": asked.get("expiresAt"),
            "tries": asked.get("tries"),
            "told": asked.get("told"),
        })))
    }

    fn outgoing_of(joins: &Arc<Joins>, id: &str) -> Result<crate::home::join::Outgoing, ApiError> {
        joins.outgoing(id).ok_or_else(|| ApiError::not_found(crate::tr!("Такого запроса нет: попросите новый код.")))
    }

    /// The code the household heard: `{code}` → the centre's answer; the right one links and helps.
    pub async fn pair_code_enter(
        Uses(joins): Uses<Arc<Joins>>,
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(events): Uses<crate::core::events::EventBus>,
        Uses(access): Uses<Arc<AccessService>>,
        caller: Caller,
        axum::extract::Path(id): axum::extract::Path<String>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let outgoing = outgoing_of(&joins, &id)?;
        let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
        let answer = to_centre(
            reqwest::Method::POST,
            &format!("http://{}:{}/api/v1/home/join/{id}/code", outgoing.address, outgoing.port),
            Some(json!({ "token": outgoing.token, "code": code })),
        )
        .await?;
        let grant = answer.get("grant").cloned().unwrap_or(Value::Null);
        let view = joined(&station_gateway, &events, &access, &outgoing, &grant, caller.actor()).await?;
        joins.forget_outgoing(&id);
        Ok(created(view))
    }

    /// The asker's look while it waits for «Разрешить» in the home's panel: `{status, why?}`, and — once allowed —
    /// the station linked and helped, `joined`.
    pub async fn pair_code_look(
        Uses(joins): Uses<Arc<Joins>>,
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(events): Uses<crate::core::events::EventBus>,
        Uses(access): Uses<Arc<AccessService>>,
        caller: Caller,
        axum::extract::Path(id): axum::extract::Path<String>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let outgoing = outgoing_of(&joins, &id)?;
        let mut look = to_centre(
            reqwest::Method::GET,
            &format!("http://{}:{}/api/v1/home/join/{id}?token={}", outgoing.address, outgoing.port, outgoing.token),
            None,
        )
        .await?;
        if let Some(grant) = look.get("grant").cloned() {
            let view = joined(&station_gateway, &events, &access, &outgoing, &grant, caller.actor()).await?;
            joins.forget_outgoing(&id);
            look["station"] = view;
            look["joined"] = json!(true);
        }
        if let Some(object) = look.as_object_mut() {
            object.remove("grant");
        }
        Ok(ok(look))
    }

    /// «Сказать ещё раз».
    pub async fn pair_code_say(
        Uses(joins): Uses<Arc<Joins>>,
        caller: Caller,
        axum::extract::Path(id): axum::extract::Path<String>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let outgoing = outgoing_of(&joins, &id)?;
        let answer = to_centre(
            reqwest::Method::POST,
            &format!("http://{}:{}/api/v1/home/join/{id}/say", outgoing.address, outgoing.port),
            Some(json!({ "token": outgoing.token })),
        )
        .await?;
        Ok(created(answer))
    }

    /// «Попробовать снова» after the safety net.
    pub async fn retry(
        Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
        Uses(moves): Uses<Arc<crate::placement::moves::MovesService>>,
        caller: Caller,
        Query(pick): Query<Pick>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&station_gateway, &pick)?;
        moves.retry(&address).await.map_err(ApiError::unavailable)?;
        Ok(created(json!({ "retried": true })))
    }
}
