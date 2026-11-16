//! «Помогаю» (docs/plans/home-and-helpers.md, stage 7; pc): who this computer helps as a helper. Two kinds of client:
//!
//! - **link clients** (`link:<12 hex>`): a home paired over the link. Its record (`helper_clients`, each computer's
//!   own) says who it said it is, whether its home key proved it this computer's own home, its gateways, and what it
//!   is given — everything to its own home; to another home hearing, voice, the model and the catalog, more only when
//!   the owner switches it on (owner, 15.11). Its link key is kept only hashed (`helping:client:<id>`): the hash is
//!   what a link proves it holds (`link::link_proof`), and the key itself never comes again.
//! - **key clients** (`key:<api key id>`): a gateway that asks plain `/home/v1` at home with a key of «Доступ» →
//!   «Ключи» (a 0.11 station). It gets everything: the owner made that key. Known by the names it sends, in memory.
//!
//! Who was seen when and how often each ability was given today live in memory; the record is written on a pairing,
//! a change, at a link's close, at shutdown, and otherwise at most once every 10 minutes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::contract::{Abilities, Ability, HelperIdentity};
use super::link::{link_key_hash, new_link_key, Binding};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::home::clean_name;
use crate::core::identity::principal::PrincipalKind;
use crate::platform::http::{ApiError, Caller};
use crate::secrets::SecretsService;
use crate::settings::definitions::{HelperClient, SeenGateway, HELPER_CLIENTS};
use crate::settings::SettingsService;
use crate::utils::time::now_iso;

/// What another home is given until the owner gives more (owner, 15.11): the muscles.
pub const MUSCLES: Abilities = Abilities::of(&[Ability::Hearing, Ability::Voice, Ability::Model, Ability::Catalog]);
/// The gateways a client keeps; the oldest seen goes.
pub const GATEWAYS: usize = 16;
/// The longest name kept of a home or a gateway.
const NAME_KEPT: usize = 60;
/// How often the last-seen times are written otherwise.
const WRITE_EVERY: Duration = Duration::from_secs(600);

/// The secret a link client's key hash lives under: under `helping:`, which never travels.
fn key_secret(id: &str) -> String {
    format!("helping:client:{id}")
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A name as kept: cleaned, at most 60 characters.
pub fn kept_name(name: &str) -> String {
    clean_name(name).chars().take(NAME_KEPT).collect()
}

/// A confirmed link client, as its key found it, with what its pairing bound (none when paired before that was kept).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkClient {
    pub id: String,
    pub own_home: bool,
    pub home: HelperIdentity,
    pub binding: Option<Binding>,
}

/// The names a home gave at its pairing's `ok`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Names {
    pub gateway: String,
    pub home: String,
}

/// A gateway as the panel shows it under its client.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientGateway {
    pub id: String,
    pub name: String,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// How often and how fast an ability was given today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Served {
    pub count: u64,
    pub avg_ms: u64,
}

/// A client as «Помогаю» shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientView {
    pub id: String,
    pub via: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<HelperIdentity>,
    pub own_home: bool,
    /// The placed station whose home key this key client's key is: the panel folds it into that station's row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    pub gateways: Vec<ClientGateway>,
    pub gives: Vec<Ability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    pub today: BTreeMap<Ability, Served>,
}

/// What is known in memory of a gateway: when it was last seen, how many links it holds now.
#[derive(Debug, Clone, Default)]
struct Live {
    name: String,
    last_seen: Option<String>,
    links: usize,
}

/// A key client as its requests named it.
#[derive(Debug, Clone, Default)]
struct KeyClient {
    name: String,
    home: Option<String>,
    gateways: BTreeMap<String, Live>,
}

/// Today's counts, by client and ability: (day, count, ms).
type Today = HashMap<String, (String, BTreeMap<Ability, (u64, u64)>)>;

/// Cuts a client's live links.
type Cut = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct Memory {
    /// Link clients' gateways seen since the start, by client.
    seen: HashMap<String, BTreeMap<String, Live>>,
    keys: BTreeMap<String, KeyClient>,
    today: Today,
    written: Option<Instant>,
    dirty: bool,
}

/// Who this computer helps.
pub struct Clients {
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    memory: Mutex<Memory>,
    /// Every read-modify-write of the record.
    writing: Mutex<()>,
    /// Cuts a removed client's live links (the link server sets it).
    cut: Mutex<Option<Cut>>,
}

impl Clients {
    /// The record read; pairings never confirmed (a start between `paired` and `ok`) are let go with their keys.
    pub fn new(settings: Arc<SettingsService>, secrets: Arc<SecretsService>, events: EventBus) -> Clients {
        // The first write of the last-seen times comes ten minutes after the start, not at the first request.
        let memory = Memory { written: Some(Instant::now()), ..Memory::default() };
        let clients = Clients {
            settings,
            secrets,
            events,
            memory: Mutex::new(memory),
            writing: Mutex::new(()),
            cut: Mutex::new(None),
        };
        let pending: Vec<String> = clients
            .record()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, client)| !client.confirmed)
            .map(|(id, _)| id)
            .collect();
        for id in pending {
            clients.forget(&id);
        }
        clients
    }

    pub fn set_cut(&self, cut: Cut) {
        *locked(&self.cut) = Some(cut);
    }

    /// The record as stored, or the database's error: a record that does not read (an I/O error, a damaged row) is
    /// never taken for «nobody» — no valid key is refused for it, no live link cut, nothing written over it.
    fn record(&self) -> Result<BTreeMap<String, HelperClient>, ApiError> {
        Ok(self.settings.peek(&HELPER_CLIENTS)?.unwrap_or_default())
    }

    /// One read-modify-write of the record; the change says whether there is anything to write. A record that does not
    /// read is never written over: nothing changes, and the error says so.
    fn change<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, HelperClient>) -> Option<T>,
    ) -> Result<Option<T>, ApiError> {
        let _writing = locked(&self.writing);
        let mut record = self.record()?;
        let Some(done) = change(&mut record) else { return Ok(None) };
        if let Err(error) = self.settings.set(&HELPER_CLIENTS, &record, None) {
            tracing::warn!("helpers: «Помогаю» not written: {}", error.message);
        }
        Ok(Some(done))
    }

    fn emit(&self, change: &str, client: &str, actor: Option<Actor>) {
        self.events.emit(GatewayEvent::HelperClients { change: change.into(), client: client.into(), actor });
    }

    /// A new home's pairing: its record, not confirmed yet, and its link key — the key's hash kept, the key given
    /// out once. `home` is only the id it claimed; `own_home` is what its home proof said; `binding` what the pairing
    /// bound, kept for its later links. While the record does not read, none: the pairing stops before its key is given
    /// out or the connection key spent.
    pub(crate) fn pair(
        &self,
        home: &str,
        gateway: &str,
        own_home: bool,
        binding: Option<&Binding>,
    ) -> Result<(String, String), ApiError> {
        let (hex, key) = new_link_key();
        let id = format!("link:{hex}");
        let gives = if own_home { Abilities::V1 } else { MUSCLES };
        let client = HelperClient {
            home: HelperIdentity { id: home.chars().take(100).collect(), name: String::new() },
            own_home,
            gateways: BTreeMap::from([(
                gateway.chars().take(100).collect(),
                SeenGateway { name: String::new(), last_seen: None },
            )]),
            gives: gives.iter().collect(),
            since: now_iso(),
            confirmed: false,
            binding: binding.map(Binding::stored),
        };
        self.change(|record| {
            record.insert(id.clone(), client);
            Some(())
        })?;
        if let Err(error) = self.secrets.set(&key_secret(&id), Some(&hex::encode(link_key_hash(&key)))) {
            tracing::warn!("helpers: a pairing's key not kept: {}", error.message);
        }
        Ok((id, key))
    }

    /// The home said `ok`: confirmed, with the names it gave. A pairing of this computer's own home (its home key proved
    /// it) by a gateway it already helped replaces that gateway's old pairing under the same home — its old key
    /// forgotten —, whether the old one proved its home or only claimed it. A pairing that proved no home replaces
    /// nothing: the ids it names can be read by anyone at home. False when the pending record is gone.
    pub(crate) fn confirm(&self, id: &str, names: Names) -> bool {
        let replaced = self.change(|record| {
            let pending = record.get_mut(id).filter(|client| !client.confirmed)?;
            pending.confirmed = true;
            pending.home.name = kept_name(&names.home);
            for gateway in pending.gateways.values_mut() {
                gateway.name = kept_name(&names.gateway);
                gateway.last_seen = Some(now_iso());
            }
            let (home, own_home) = (pending.home.id.clone(), pending.own_home);
            let gateways: Vec<String> = pending.gateways.keys().cloned().collect();
            let replaced: Vec<String> = record
                .iter()
                .filter(|(other, client)| {
                    other.as_str() != id
                        && own_home
                        && client.home.id == home
                        && client.gateways.keys().any(|gateway| gateways.contains(gateway))
                })
                .map(|(other, _)| other.clone())
                .collect();
            for other in &replaced {
                record.remove(other);
            }
            Some(replaced)
        });
        let Ok(Some(replaced)) = replaced else { return false };
        for other in replaced {
            let _ = self.secrets.delete(&key_secret(&other));
            self.cut_links(&other);
            self.emit("removed", &other, None);
        }
        self.emit("paired", id, None);
        true
    }

    /// A pending pairing let go (no `ok`, or a probe's leftovers): its record and key — kept while the record does not
    /// read (a pending client opens nothing, and the next start lets it go).
    pub(crate) fn forget(&self, id: &str) {
        let _ = self.change(|record| record.remove(id).filter(|client| !client.confirmed).map(|_| ()));
        if self.record().is_ok_and(|record| !record.contains_key(id)) {
            let _ = self.secrets.delete(&key_secret(id));
        }
    }

    fn cut_links(&self, id: &str) {
        let cut = locked(&self.cut).clone();
        if let Some(cut) = cut {
            cut(id);
        }
    }

    /// The confirmed client a link key's id (its 12 hex) names, and the key's hash — what its link must prove it holds.
    /// The error: the id is known but the record does not read now — never a refusal (the link server answers 503,
    /// which a gateway tries again; 401 would end its tries).
    pub(crate) fn by_key_id(&self, key_id: &str) -> Result<Option<(LinkClient, [u8; 32])>, ApiError> {
        let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if key_id.len() != 12 || !key_id.bytes().all(lower_hex) {
            return Ok(None);
        }
        let id = format!("link:{key_id}");
        let stored = self.secrets.get(&key_secret(&id));
        let Some(hash) = stored.and_then(|stored| hex::decode(stored.trim()).ok()?.try_into().ok()) else {
            return Ok(None);
        };
        let client = self.record()?.remove(&id).filter(|client| client.confirmed);
        let binding = |client: &HelperClient| client.binding.as_deref().and_then(Binding::from_stored);
        Ok(client.map(|client| {
            (LinkClient { binding: binding(&client), id, own_home: client.own_home, home: client.home }, hash)
        }))
    }

    /// The confirmed client a whole link key belongs to (tests: a link proves its key instead of showing it).
    #[cfg(test)]
    pub(crate) fn check(&self, token: &str) -> Result<Option<LinkClient>, ApiError> {
        let Some(hex) = super::link::link_key_id(token) else { return Ok(None) };
        let found = self.by_key_id(hex)?;
        Ok(found.filter(|(_, hash)| super::link::same(hash, &link_key_hash(token))).map(|(client, _)| client))
    }

    /// The client is still there and confirmed (a removed one's links are cut at the next look). A record that does
    /// not read cuts nothing: a removal cuts its links itself (`remove`), and its key opens nothing more.
    pub(crate) fn alive(&self, id: &str) -> bool {
        self.record().map_or(true, |record| record.get(id).is_some_and(|client| client.confirmed))
    }

    /// What a client is given: a link client its record's, a key client everything; nobody else anything — nor a link
    /// client while the record does not read.
    pub fn given(&self, client: &str) -> Abilities {
        if client.starts_with("key:") {
            return Abilities::V1;
        }
        self.record()
            .unwrap_or_default()
            .get(client)
            .filter(|client| client.confirmed)
            .map(|client| client.gives.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn gives(&self, client: &str, ability: Ability) -> bool {
        self.given(client).has(ability)
    }

    /// What a link client is given, as the owner set it on «Этот компьютер».
    pub fn set_gives(&self, client: &str, gives: Vec<Ability>, actor: Option<Actor>) -> Result<(), ApiError> {
        let gives: Vec<Ability> =
            gives.into_iter().filter(|ability| Ability::V1.contains(ability)).collect::<Abilities>().iter().collect();
        let changed = self.change(|record| {
            let found = record.get_mut(client).filter(|found| found.confirmed)?;
            (found.gives != gives).then(|| found.gives = gives.clone())
        })?;
        if !self.alive(client) {
            return Err(no_such_client());
        }
        if changed.is_some() {
            self.emit("gives", client, actor);
        }
        Ok(())
    }

    /// A link client removed: its record and key, its links cut.
    pub fn remove(&self, client: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        self.change(|record| record.remove(client).map(|_| ()))?.ok_or_else(no_such_client)?;
        let _ = self.secrets.delete(&key_secret(client));
        locked(&self.memory).seen.remove(client);
        self.cut_links(client);
        self.emit("removed", client, actor);
        Ok(())
    }

    /// A request seen: a link client's gateway, or a key client's names as its headers give them (a 0.11 station sends
    /// none and shows by its key's name). People are no clients.
    pub fn saw(&self, caller: &Caller) {
        let now = now_iso();
        let header = |name: &str| {
            caller.headers.get(name).and_then(|value| std::str::from_utf8(value.as_bytes()).ok()).map(kept_name)
        };
        let mut memory = locked(&self.memory);
        if let Some(link) = caller.link() {
            let gateways = memory.seen.entry(link.client.clone()).or_default();
            let gateway = gateways.entry(link.gateway.id.clone()).or_default();
            gateway.last_seen = Some(now);
            if !link.gateway.name.is_empty() {
                gateway.name = link.gateway.name.clone();
            }
            keep_gateways(gateways);
            memory.dirty = true;
        } else if let Some(principal) = caller.principal().filter(|principal| principal.kind == PrincipalKind::ApiKey) {
            let client = memory.keys.entry(format!("key:{}", principal.id)).or_default();
            client.name = principal.name.clone();
            if let Some(home) = header(super::link::HOME_HEADER).filter(|home| !home.is_empty()) {
                client.home = Some(home);
            }
            if let Some(gateway) = header(super::link::GATEWAY_HEADER).filter(|gateway| !gateway.is_empty()) {
                let seen = client.gateways.entry(gateway).or_default();
                seen.last_seen = Some(now);
                if let Some(name) = header(super::link::GATEWAY_NAME_HEADER).filter(|name| !name.is_empty()) {
                    seen.name = name;
                }
                keep_gateways(&mut client.gateways);
            }
        }
        let due = memory.dirty && memory.written.is_none_or(|written| written.elapsed() >= WRITE_EVERY);
        drop(memory);
        if due {
            self.persist_seen();
        }
    }

    /// One answer given to a client: today's count and time per ability.
    pub fn served(&self, caller: &Caller, ability: Ability, started: Instant) {
        let client = match (caller.link(), caller.principal()) {
            (Some(link), _) => link.client.clone(),
            (None, Some(principal)) if principal.kind == PrincipalKind::ApiKey => format!("key:{}", principal.id),
            _ => return,
        };
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut memory = locked(&self.memory);
        let today = memory.today.entry(client).or_default();
        if today.0 != day {
            *today = (day, BTreeMap::new());
        }
        let count = today.1.entry(ability).or_default();
        count.0 += 1;
        count.1 += started.elapsed().as_millis() as u64;
    }

    /// A gateway of a link client came on or went off the link: `online` / `offline` on a change only. A client's
    /// gateways are kept to `GATEWAYS`, the oldest seen without a link going first.
    pub(crate) fn online(&self, id: &str, gateway: &HelperIdentity, on: bool) {
        let changed = {
            let mut memory = locked(&self.memory);
            let gateways = memory.seen.entry(id.to_string()).or_default();
            let live = gateways.entry(gateway.id.clone()).or_default();
            if !gateway.name.is_empty() {
                live.name = gateway.name.clone();
            }
            live.last_seen = Some(now_iso());
            let before = live.links > 0;
            live.links = if on { live.links + 1 } else { live.links.saturating_sub(1) };
            let after = live.links > 0;
            keep_gateways(gateways);
            memory.dirty = true;
            before != after
        };
        if changed {
            self.emit(if on { "online" } else { "offline" }, id, None);
        }
        if !on {
            self.persist_seen();
        }
    }

    /// The last-seen times and names into the record.
    pub fn persist_seen(&self) {
        let seen = {
            let mut memory = locked(&self.memory);
            if !memory.dirty {
                return;
            }
            memory.dirty = false;
            memory.written = Some(Instant::now());
            memory.seen.clone()
        };
        let written = self.change(|record| {
            let mut changed = false;
            for (id, gateways) in &seen {
                let Some(client) = record.get_mut(id).filter(|client| client.confirmed) else { continue };
                for (gateway, live) in gateways {
                    let entry = client
                        .gateways
                        .entry(gateway.clone())
                        .or_insert_with(|| SeenGateway { name: String::new(), last_seen: None });
                    if !live.name.is_empty() && entry.name != live.name {
                        entry.name = live.name.clone();
                        changed = true;
                    }
                    if live.last_seen.is_some() && entry.last_seen != live.last_seen {
                        entry.last_seen = live.last_seen.clone();
                        changed = true;
                    }
                }
                // The newest `GATEWAYS` kept, in one pass.
                if client.gateways.len() > GATEWAYS {
                    let mut by_age: Vec<(String, SeenGateway)> =
                        std::mem::take(&mut client.gateways).into_iter().collect();
                    by_age.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen));
                    by_age.truncate(GATEWAYS);
                    client.gateways = by_age.into_iter().collect();
                    changed = true;
                }
            }
            changed.then_some(())
        });
        if written.is_err() {
            // Not read: kept for the next occasion, never written over the record.
            locked(&self.memory).dirty = true;
        }
    }

    /// «Помогаю»: the link clients, then the key clients seen since the start. `placements` are the stations this
    /// computer places, with the id of the key their gateways ask it with.
    pub fn view(&self, placements: &[(String, Option<String>)]) -> Vec<ClientView> {
        let memory = locked(&self.memory);
        let today = |id: &str| -> BTreeMap<Ability, Served> {
            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            memory
                .today
                .get(id)
                .filter(|(at, _)| *at == day)
                .map(|(_, counts)| {
                    counts
                        .iter()
                        .map(|(ability, (count, ms))| {
                            (*ability, Served { count: *count, avg_ms: ms.checked_div(*count).unwrap_or(0) })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut views: Vec<ClientView> = self
            .record()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, client)| client.confirmed)
            .map(|(id, client)| {
                let live = memory.seen.get(&id);
                let gateways = client
                    .gateways
                    .iter()
                    .map(|(gateway, seen)| {
                        let now = live.and_then(|live| live.get(gateway));
                        ClientGateway {
                            id: gateway.clone(),
                            name: now
                                .map(|now| now.name.clone())
                                .filter(|name| !name.is_empty())
                                .unwrap_or_else(|| seen.name.clone()),
                            online: now.is_some_and(|now| now.links > 0),
                            last_seen: now.and_then(|now| now.last_seen.clone()).or_else(|| seen.last_seen.clone()),
                        }
                    })
                    .collect();
                let name = if client.home.name.is_empty() { id.clone() } else { client.home.name.clone() };
                ClientView {
                    today: today(&id),
                    id,
                    via: "link",
                    name,
                    home: Some(client.home),
                    own_home: client.own_home,
                    station: None,
                    gateways,
                    gives: client.gives,
                    since: Some(client.since),
                }
            })
            .collect();
        for (id, client) in &memory.keys {
            let key = id.trim_start_matches("key:");
            let station = placements
                .iter()
                .find(|(_, home_key)| home_key.as_deref() == Some(key))
                .map(|(station, _)| station.clone());
            views.push(ClientView {
                id: id.clone(),
                via: "key",
                name: client.name.clone(),
                home: client.home.clone().map(|home| HelperIdentity { id: home, name: String::new() }),
                own_home: true,
                station,
                gateways: client
                    .gateways
                    .iter()
                    .map(|(gateway, live)| ClientGateway {
                        id: gateway.clone(),
                        name: live.name.clone(),
                        online: false,
                        last_seen: live.last_seen.clone(),
                    })
                    .collect(),
                gives: Ability::V1.to_vec(),
                since: None,
                today: today(id),
            });
        }
        views
    }

    /// A client's record.
    pub fn record_of(&self, client: &str) -> Option<HelperClient> {
        self.record().ok()?.remove(client)
    }
}

/// A client's gateways in memory kept to `GATEWAYS`: the one seen longest ago without a link of its own goes (a client
/// has at most that many links, so there is always one). One goes per gateway added: it never grows past the cap.
fn keep_gateways(gateways: &mut BTreeMap<String, Live>) {
    if gateways.len() <= GATEWAYS {
        return;
    }
    let oldest = gateways
        .iter()
        .filter(|(_, live)| live.links == 0)
        .min_by(|a, b| a.1.last_seen.cmp(&b.1.last_seen))
        .map(|(id, _)| id.clone());
    if let Some(oldest) = oldest {
        gateways.remove(&oldest);
    }
}

fn no_such_client() -> ApiError {
    ApiError::not_found(crate::tr!("Нет такого шлюза среди тех, кому помогает этот компьютер."))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::extract::{ConnectInfo, FromRequestParts};
    use axum::http::Request;
    use serde_json::{json, Map};

    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::platform::http::LinkOrigin;

    fn computer() -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap()
    }

    /// A request's caller with these headers, and a home's link origin when given.
    async fn caller(app: &Arc<App>, headers: &[(&str, &str)], link: Option<LinkOrigin>) -> Caller {
        let mut request = Request::builder().uri("/home/v1/status");
        for (name, value) in headers {
            request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
        }
        let mut request = request.body(()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
        if let Some(link) = link {
            request.extensions_mut().insert(link);
        }
        let (mut parts, _) = request.into_parts();
        Caller::from_request_parts(&mut parts, app).await.unwrap()
    }

    fn confirmed(app: &App, home: &str, gateway: &str, own_home: bool) -> (String, String) {
        let (id, key) = app.clients.pair(home, gateway, own_home, None).unwrap();
        assert!(app.clients.confirm(&id, Names { gateway: "Колонка".into(), home: "Дача".into() }));
        (id, key)
    }

    #[tokio::test]
    async fn clients_are_remembered_by_key_with_the_names_they_give() {
        let app = computer();
        let input = Map::from_iter([
            ("name".into(), json!("Колонка: домашний сервер")),
            ("grants".into(), json!(["home.models"])),
        ]);
        let created = app.access.keys.create(&input, None).unwrap();
        let bearer = format!("Bearer {}", created.secret);
        // A 0.11 station names nothing: it shows by its key's name.
        let station = caller(&app, &[("authorization", &bearer)], None).await;
        app.clients.saw(&station);
        let station = caller(
            &app,
            &[
                ("authorization", &bearer),
                ("x-duowave-gateway", "gw-1"),
                ("x-duowave-gateway-name", "  Кух\u{202E}ня\t  "),
                ("x-duowave-home", "home-1"),
            ],
            None,
        )
        .await;
        app.clients.saw(&station);
        app.clients.served(&station, Ability::Hearing, Instant::now());
        app.clients.served(&station, Ability::Hearing, Instant::now());
        let id = format!("key:{}", created.key.id);
        let placed = [("station-a".to_string(), Some(created.key.id.clone()))];
        let views = app.clients.view(&placed);
        let view = views.iter().find(|view| view.id == id).expect("the key client");
        assert_eq!(
            (view.via, view.name.as_str(), view.station.as_deref()),
            ("key", "Колонка: домашний сервер", Some("station-a"))
        );
        assert_eq!(view.gateways.len(), 1);
        assert_eq!(
            (view.gateways[0].id.as_str(), view.gateways[0].name.as_str()),
            ("gw-1", "Кухня"),
            "bidi and control characters out"
        );
        assert_eq!(view.home.as_ref().map(|home| home.id.as_str()), Some("home-1"));
        assert_eq!(view.today[&Ability::Hearing].count, 2);
        assert_eq!(view.gives, Ability::V1.to_vec(), "the owner made that key: everything");
        // A person is no client.
        let token = app.access.auth.setup("admin-password-of-tests", "", "127.0.0.1").await.unwrap().unwrap().token;
        let person = caller(&app, &[("authorization", &format!("Bearer {token}"))], None).await;
        app.clients.saw(&person);
        assert_eq!(app.clients.view(&[]).len(), 1);
        // At most 16 gateways a client.
        for at in 0..20 {
            let gateway = format!("gw-{at}");
            let more = caller(&app, &[("authorization", &bearer), ("x-duowave-gateway", &gateway)], None).await;
            app.clients.saw(&more);
        }
        assert_eq!(app.clients.view(&[])[0].gateways.len(), GATEWAYS);
    }

    #[tokio::test]
    async fn another_home_gets_the_muscles_and_its_own_home_everything() {
        let app = computer();
        let (other, other_key) = confirmed(&app, "home-other", "gw-1", false);
        assert_eq!(app.clients.given(&other), MUSCLES);
        assert!(!app.clients.gives(&other, Ability::Messages) && app.clients.gives(&other, Ability::Voice));
        let (own, own_key) = confirmed(&app, "home-own", "gw-2", true);
        assert_eq!(app.clients.given(&own), Abilities::V1);
        // The owner gives another home more.
        app.clients.set_gives(&other, vec![Ability::Hearing, Ability::Messages], None).unwrap();
        assert!(app.clients.gives(&other, Ability::Messages) && !app.clients.gives(&other, Ability::Voice));
        assert_eq!(
            app.clients.set_gives("link:000000000000", vec![], None).unwrap_err().status,
            axum::http::StatusCode::NOT_FOUND
        );
        // The same gateway of the own home pairing again: its old pairing and key replaced.
        let (again, again_key) = confirmed(&app, "home-own", "gw-2", true);
        assert!(app.clients.check(&own_key).unwrap().is_none() && app.clients.check(&again_key).unwrap().is_some());
        assert!(!app.clients.alive(&own) && app.clients.alive(&again));
        // Another home's claim of the same ids replaces nothing: it proved no home.
        let (_, _) = confirmed(&app, "home-other", "gw-1", false);
        assert!(app.clients.check(&other_key).unwrap().is_some());
        // A gateway that claimed this home without its key, then pairs again proving it: its claim is replaced.
        let (claim, claim_key) = confirmed(&app, "home-own", "gw-3", false);
        let (proven, _) = confirmed(&app, "home-own", "gw-3", true);
        assert!(
            app.clients.check(&claim_key).unwrap().is_none()
                && !app.clients.alive(&claim)
                && app.clients.alive(&proven)
        );
        // Removed: its key opens nothing.
        app.clients.remove(&other, None).unwrap();
        assert!(app.clients.check(&other_key).unwrap().is_none());
        assert_eq!(app.clients.given(&other), Abilities::NONE);
    }

    #[tokio::test]
    async fn last_seen_is_written_at_close_and_rarely_otherwise() {
        let app = computer();
        let (id, _) = confirmed(&app, "home-1", "gw-1", false);
        let stored = || app.clients.record_of(&id).unwrap().gateways["gw-1"].last_seen.clone();
        let at_pairing = stored();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let origin = LinkOrigin {
            client: id.clone(),
            own_home: false,
            home: HelperIdentity { id: "home-1".into(), name: "Дача".into() },
            gateway: HelperIdentity { id: "gw-1".into(), name: "Колонка".into() },
        };
        let link = caller(&app, &[], Some(origin.clone())).await;
        for _ in 0..100 {
            app.clients.saw(&link);
        }
        assert_eq!(stored(), at_pairing, "a hundred requests write nothing");
        app.clients.online(&id, &origin.gateway, true);
        app.clients.online(&id, &origin.gateway, false);
        assert!(stored() > at_pairing, "the link's close writes when it was last seen");
        // A link client's gateways are kept to sixteen, in memory as in the record, however many ids its links bring.
        for at in 0..40 {
            let gateway = HelperIdentity { id: format!("gw-new-{at}"), name: String::new() };
            app.clients.online(&id, &gateway, true);
            app.clients.online(&id, &gateway, false);
        }
        assert!(locked(&app.clients.memory).seen[&id].len() <= GATEWAYS);
        assert_eq!(app.clients.record_of(&id).unwrap().gateways.len(), GATEWAYS);
        assert_eq!(app.clients.view(&[])[0].gateways.len(), GATEWAYS);
    }

    /// A record that does not read is never taken for «nobody» (review of stage 7), the pins' rule: a right key is
    /// neither refused nor let in — the link server says «again» —, a wrong one is refused as ever, a live link is kept
    /// at the next look, a home is given nothing meanwhile, and nothing is written over the record — a new pairing
    /// waits, so do the owner's changes and the last-seen times —; read again, all is as it was.
    #[tokio::test]
    async fn a_record_that_does_not_read_refuses_no_right_key_cuts_no_link_and_is_never_written_over() {
        let app = computer();
        let (id, key) = confirmed(&app, "home-1", "gw-1", false);
        let unreadable = |broken: bool| {
            let flag = if broken { "'unreadable'" } else { "0" };
            let sql = format!("UPDATE settings SET is_default = {flag} WHERE key = 'helper_clients'");
            app.db.with(|c| c.execute(&sql, [])).unwrap();
        };
        // Its row no longer reads (an I/O error of a small disk, say); writing still works.
        unreadable(true);
        assert!(app.clients.check(&key).is_err(), "a right key: again, never refused");
        let wrong = format!("dwl1_{}_{}", "0".repeat(12), "a".repeat(64));
        assert_eq!(app.clients.check(&wrong).unwrap(), None, "a wrong one: refused as ever");
        assert!(app.clients.alive(&id), "its live links are kept");
        assert_eq!(app.clients.given(&id), Abilities::NONE, "given nothing meanwhile");
        assert!(app.clients.pair("home-2", "gw-2", false, None).is_err(), "a new pairing waits");
        assert!(app.clients.set_gives(&id, vec![Ability::Messages], None).is_err());
        assert!(app.clients.remove(&id, None).is_err());
        let gateway = HelperIdentity { id: "gw-1".into(), name: "Колонка".into() };
        app.clients.online(&id, &gateway, true);
        app.clients.online(&id, &gateway, false);
        assert!(locked(&app.clients.memory).dirty, "the last-seen times wait for a record that reads");

        unreadable(false);
        assert_eq!(app.clients.check(&key).unwrap().map(|client| client.id), Some(id.clone()));
        assert_eq!(app.clients.given(&id), MUSCLES);
        assert_eq!(app.clients.view(&[]).iter().map(|view| view.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        app.clients.persist_seen();
        assert!(!locked(&app.clients.memory).dirty);
    }
}
