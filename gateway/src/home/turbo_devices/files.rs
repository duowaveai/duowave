//! The home's folders at its Турбо (docs/plans/turbo-devices.md, «Папки Турбо»): `files/home/Фото` is one folder of
//! the home — this gateway's own and every Турбо's that gives its folders (`files`, contract 3). The owner's decision 1
//! (20.11): look everywhere, act at one place.
//!
//! - **Seeing** — a folder's list and «найди» — goes to every place: this gateway's folder and each Турбо's, the items
//!   in one list, each with its `place` (and the computer's name for people).
//! - **Reading** a file — its text, its picture, its link — goes where it is: the place named (`place`), else this
//!   gateway when it has the file, else the first Турбо that has it.
//! - **Changing** goes to one place: the one named, else where the file is; a new file — on a station to the first
//!   Турбо (its disk is small), on a computer here.
//! - **The rights stay the centre's**: the bus decided the letter before it came here, and the household's ticks
//!   («вне дома», «модель может читать») are asked here before a letter goes to a Турбо (`Files::word`). A Турбо only
//!   checks that it gives its folders, and its flash drives only to a home at home (owner 20.11, 6).
//! - **A Турбо's file by its link**: the key its engine gave goes out as `turbo.<place>.<key>`; the centre's
//!   `/media/v/…` sends the fetch to the Турбо's own address (gateway/media), so the panel, an app and the station's
//!   player take it as any other.
//!
//! Everything else of the folders — an app's own, a person's, a project's, `Connected` of this computer — stays this
//! gateway's as before.
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::{home_id, HouseDevices, HERE, TURBO_KEY};
use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, Participant, Provider};
use crate::bus::providers::files::{handles_of, natural, Files, Kind, Spot, ATTACHED, PAGE, PAGE_MOST};

/// What only sees a folder: every place's answers in one.
const SEEING: [&str; 2] = ["list", "find"];
/// What reads a file where it is.
const READING: [&str; 6] = ["stat", "read", "text", "preview", "media", "play"];
/// What changes a folder: at one place.
const CHANGING: [&str; 5] = ["write", "mkdir", "remove", "move", "copy"];

/// The folders' provider of the home: this gateway's `Files`, with the Турбо's folders as places of the same folders.
pub struct HouseFiles {
    local: Arc<Files>,
    house: Weak<HouseDevices>,
    /// Whether this gateway runs on the station: a new file goes to a Турбо then, the station's disk being small.
    on_station: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl HouseFiles {
    pub fn new(
        local: Arc<Files>,
        house: &Arc<HouseDevices>,
        on_station: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> HouseFiles {
        HouseFiles { local, house: Arc::downgrade(house), on_station }
    }
}

/// The household's folders: `files/home/…`.
fn household(address: &Address) -> bool {
    address.kind() == address::FILES && address.path().starts_with("home/")
}

/// One place's answer for people: its items marked with where they are.
fn marked(answer: Value, place: &str, name: &str) -> Vec<Value> {
    answer
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut item| {
            item["place"] = json!(place);
            item["placeName"] = json!(name);
            item
        })
        .collect()
}

/// A Турбо's link answer as the home names it: the key goes to the Турбо through the centre's `/media/v/…`.
fn keyed(mut answer: Value, place: &str) -> Value {
    fn rename(value: &mut Value, place: &str) {
        for key in ["file", "key"] {
            if let Some(Value::String(found)) = value.get(key) {
                let named = format!("{TURBO_KEY}{place}.{found}");
                value[key] = Value::String(named);
            }
        }
        value["place"] = json!(place);
    }
    match answer.get_mut("links").and_then(Value::as_array_mut) {
        Some(links) => links.iter_mut().for_each(|link| rename(link, place)),
        None => rename(&mut answer, place),
    }
    answer
}

/// `keyed`, for the tests of the home's devices.
#[cfg(test)]
pub fn keyed_for_tests(answer: Value, place: &str) -> Value {
    keyed(answer, place)
}

impl HouseFiles {
    /// A flash drive of a Турбо at home (`files/home/Connected/<name>/…`), unknown to this gateway's own «Подключено»:
    /// its folder, and the Турбо it is plugged into (`device/storage` there, owner 20.11, 3 and 6).
    fn turbo_drive(&self, address: &Address) -> Option<(Address, String)> {
        let prefix = format!("{}/home/{ATTACHED}/", address::FILES);
        let name = address.as_str().strip_prefix(&prefix)?.split('/').next()?.to_string();
        if name.is_empty() || self.local.places().spot(address).is_some() {
            return None;
        }
        let house = self.house.upgrade()?;
        let place = house
            .ready("storage")
            .into_iter()
            .find(|place| {
                place.place != HERE && place.instances.iter().any(|thing| thing.id == home_id(&place.place, &name))
            })?
            .place;
        Some((Address::parse(&format!("{prefix}{name}")).ok()?, place))
    }

    /// The household's ticks of «Подключено», for a Турбо's drive: kept at home unless ticked, closed to a cloud model
    /// unless ticked.
    fn drive_word(&self, letter: &Letter) -> Result<(), BusError> {
        let connected = Address::parse(&format!("{}/home/{ATTACHED}", address::FILES)).map_err(BusError::Failed)?;
        let rule = self.local.rule(&connected);
        if letter.trail.remote && !rule.remote {
            return Err(BusError::NotFound(letter.to.to_string()));
        }
        let reads = SEEING.contains(&letter.handle.as_str()) || READING.contains(&letter.handle.as_str());
        if reads && letter.trail.through_cloud() && !rule.model {
            return Err(BusError::Denied(crate::tr!(
                "модель «Мышления» в облаке, а папка «{name}» ей закрыта: галочка «Модель может читать» в разделе «Папки»",
                name = ATTACHED
            )));
        }
        Ok(())
    }

    /// Where a letter goes: this gateway alone (no Турбо gives folders, or not the household's), or the home.
    fn turbos(&self, letter: &Letter) -> Vec<(String, String)> {
        if !household(&letter.to) {
            return Vec::new();
        }
        self.house.upgrade().map(|house| house.folder_places()).unwrap_or_default()
    }

    /// The household's ticks asked here, before a letter leaves for a Турбо.
    fn word(&self, letter: &Letter) -> Result<(), BusError> {
        let spot = self.local.places().spot(&letter.to).ok_or_else(|| BusError::NotFound(letter.to.to_string()))?;
        self.local.word(&spot, letter)
    }

    async fn here(&self, letter: Letter) -> Result<Value, BusError> {
        self.local.call(letter).await
    }

    async fn there(&self, place: &str, letter: &Letter) -> Result<Value, BusError> {
        let house = self.house.upgrade().ok_or_else(|| BusError::Failed("the gateway is stopping".into()))?;
        let mut input = letter.input.clone();
        if let Some(fields) = input.as_object_mut() {
            fields.remove("place");
        }
        let answer = house.files_at(place, letter.to.as_str(), &letter.handle, input).await?;
        Ok(match letter.handle.as_str() {
            "media" | "play" => keyed(answer, place),
            _ => answer,
        })
    }

    /// Every place's list in one: this gateway's folder and each Турбо's, folders first, names as people count.
    async fn seen(&self, letter: Letter, turbos: Vec<(String, String)>) -> Result<Value, BusError> {
        let wide = |letter: &Letter| {
            let mut letter = letter.clone();
            if letter.handle == "list" {
                letter.input["from"] = json!(0);
                letter.input["limit"] = json!(PAGE_MOST);
            }
            letter
        };
        let mut items: Vec<Value> = Vec::new();
        let mut partial = false;
        let mut failed = None;
        let mut answered = false;
        match self.here(wide(&letter)).await {
            Ok(answer) => {
                answered = true;
                partial |= answer.get("partial").and_then(Value::as_bool).unwrap_or(false);
                items.extend(marked(answer, super::HERE, ""));
            }
            Err(error) => failed = Some(error),
        }
        for (place, name) in &turbos {
            match self.there(place, &wide(&letter)).await {
                Ok(answer) => {
                    answered = true;
                    partial |= answer.get("partial").and_then(Value::as_bool).unwrap_or(false);
                    items.extend(marked(answer, place, name));
                }
                Err(error) => failed = failed.or(Some(error)),
            }
        }
        if !answered {
            return Err(failed.unwrap_or_else(|| BusError::NotFound(letter.to.to_string())));
        }
        if letter.handle == "find" {
            items.sort_by(|a, b| {
                let score = |item: &Value| item.get("score").and_then(Value::as_f64).unwrap_or(0.0);
                score(b).total_cmp(&score(a))
            });
            let limit = letter.input.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
            items.truncate(limit);
            return Ok(json!({ "items": items, "partial": partial }));
        }
        let folder = |item: &Value| item.get("folder").and_then(Value::as_bool).unwrap_or(false);
        let name = |item: &Value| item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        items.sort_by(|a, b| folder(b).cmp(&folder(a)).then_with(|| natural(&name(a), &name(b))));
        let total = items.len();
        let from = letter.input.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
        let limit = letter.input.get("limit").and_then(Value::as_u64).map(|limit| limit as usize).unwrap_or(PAGE);
        let page: Vec<Value> = items.into_iter().skip(from).take(limit.clamp(1, PAGE_MOST)).collect();
        let next = (from + page.len() < total).then_some(from + page.len());
        Ok(json!({ "items": page, "total": total, "next": next }))
    }

    /// Where a file is: the place named; else here when here has it; else the first Турбо that has it.
    async fn where_is(&self, letter: &Letter, turbos: &[(String, String)]) -> Option<String> {
        if let Some(place) = letter.input.get("place").and_then(Value::as_str) {
            return Some(place.to_string());
        }
        let stat = |letter: &Letter| {
            let mut letter = letter.clone();
            letter.handle = "stat".into();
            letter.input = json!({});
            letter
        };
        if self.here(stat(letter)).await.is_ok() {
            return Some(super::HERE.to_string());
        }
        for (place, _) in turbos {
            if self.there(place, &stat(letter)).await.is_ok() {
                return Some(place.clone());
            }
        }
        None
    }

    async fn route(&self, letter: Letter) -> Result<Value, BusError> {
        // A Турбо's flash drive: read at that Турбо, nothing put there by voice or app from here (read only).
        if let Some((_, place)) = self.turbo_drive(&letter.to) {
            self.drive_word(&letter)?;
            if CHANGING.contains(&letter.handle.as_str()) && letter.handle != "copy" {
                return Err(BusError::Denied(crate::tr!("Флешка другого компьютера отсюда только для чтения.")));
            }
            return self.there(&place, &letter).await;
        }
        let turbos = self.turbos(&letter);
        if turbos.is_empty() {
            return self.here(letter).await;
        }
        let handle = letter.handle.clone();
        if SEEING.contains(&handle.as_str()) {
            self.word(&letter)?;
            return self.seen(letter, turbos).await;
        }
        if READING.contains(&handle.as_str()) || matches!(handle.as_str(), "remove" | "move" | "copy") {
            self.word(&letter)?;
            let Some(place) = self.where_is(&letter, &turbos).await else {
                return Err(BusError::NotFound(letter.to.to_string()));
            };
            if (handle == "move" || handle == "copy") && place != super::HERE {
                let to = letter.input.get("to").and_then(Value::as_str).unwrap_or_default();
                // A move between two computers is not one letter: the person copies it in the panel.
                if !to.starts_with(&format!("{}/home/", address::FILES)) {
                    return Err(BusError::Failed(crate::tr!(
                        "Перенести файл с одного компьютера на другой голосом нельзя: сделайте это в пульте."
                    )));
                }
            }
            return match place.as_str() {
                super::HERE => self.here(letter).await,
                place => self.there(place, &letter).await,
            };
        }
        if CHANGING.contains(&handle.as_str()) {
            self.word(&letter)?;
            let named = letter.input.get("place").and_then(Value::as_str).map(str::to_string);
            // A new file on a station goes to its Турбо: the station's disk holds 5 МБ of files in all.
            let place =
                named.unwrap_or_else(
                    || {
                        if (self.on_station)() {
                            turbos[0].0.clone()
                        } else {
                            super::HERE.to_string()
                        }
                    },
                );
            return match place.as_str() {
                super::HERE => self.here(letter).await,
                place => self.there(place, &letter).await,
            };
        }
        self.here(letter).await
    }
}

impl Provider for HouseFiles {
    fn kind(&self) -> &'static str {
        address::FILES
    }

    fn participants(&self) -> Vec<Participant> {
        self.local.participants()
    }

    fn participant(&self, address: &Address) -> Option<Participant> {
        self.local.participant(address).or_else(|| {
            let (folder, _) = self.turbo_drive(address)?;
            let title = folder.name().map(str::to_string).unwrap_or_else(|| folder.to_string());
            Some(Participant::new(folder, &title))
        })
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        let local = self.local.handles(address);
        if !local.is_empty() {
            return local;
        }
        let Some((folder, _)) = self.turbo_drive(address) else { return local };
        // A Турбо's drive reads as an attached folder of this gateway's, read only; open to the roles of «Подключено».
        let connected = Address::parse(&format!("{}/home/{ATTACHED}", address::FILES)).ok();
        let roles = connected.map(|connected| self.local.rule(&connected).roles);
        let spot = Spot {
            folder,
            kind: Kind::Attached,
            root: std::path::PathBuf::new(),
            rest: Vec::new(),
            read_only: true,
            private: false,
            owner: None,
            bin: None,
        };
        handles_of(&spot).into_iter().map(|handle| handle.for_roles(roles.clone())).collect()
    }

    fn owns(&self, from: &Address, to: &Address) -> bool {
        self.local.owns(from, to)
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        let me =
            HouseFiles { local: self.local.clone(), house: self.house.clone(), on_station: self.on_station.clone() };
        Box::pin(async move { me.route(letter).await })
    }
}
