//! `POST /api/v1/media/links`: keys to show and play, for the panel (docs/plans/feed.md, «Один движок показа»).
//!
//! A person asks for files of the home folder as themself — their role, their place (at home or away). An app's
//! window or card asks through the panel for the person looking at it (`app`): the letters go as that app, for that
//! person — its own folder always, others by its links. Either way it is the bus that checks, by the folders' letter
//! `media`, and the engine that gives the keys. `{folder, names}` asks for a page of a folder in one letter (the bus
//! lets a chain send 32); `{addresses}` — files anywhere, a letter each, up to 64.

use std::sync::Arc;

use axum::extract::State;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::bus::address::{Address, PERSON};
use crate::bus::providers::files::PAGE_MOST;
use crate::bus::routes::api_error;
use crate::bus::{BusError, Trail};
use crate::core::identity::permissions::use_of;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::capabilities::CapabilityName;

/// Files by their addresses in one request at most.
pub const ADDRESSES_MOST: usize = 64;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinksBody {
    /// The app whose window or card asks: the letters go as it, for the person signed in.
    #[serde(default)]
    app: Option<String>,
    /// Files by their addresses: `files/home/Photos/кот.jpg`, `files/app/feed/episodes/01.mp4`.
    #[serde(default)]
    addresses: Vec<String>,
    /// Or a folder and the names of files in it.
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    names: Vec<String>,
    /// How long each plays too: a few kilobytes of each are read.
    #[serde(default)]
    probe: bool,
}

/// The person's chain and their letters' sender: themself, or the app whose face asks.
fn sender(caller: &Caller, app: Option<&str>) -> Result<(Trail, Address), ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?;
    let trail = Trail::principal(principal).from_afar(caller.zone == Zone::Remote);
    match app {
        Some(id) => {
            let from = Address::app(id);
            if from.name() != Some(id) {
                return Err(ApiError::bad_request(crate::tr!("app: id приложения")));
            }
            let trail = trail.entering(id).map_err(|error| api_error(BusError::from(error)))?;
            Ok((trail, from))
        }
        None => Ok((trail, Address::new(PERSON, &principal.id).map_err(ApiError::bad_request)?)),
    }
}

/// One answer of the letter `media` as the panel reads it: the link with its address, or why not.
fn answered(address: &str, answer: Result<Value, BusError>) -> Value {
    match answer {
        Ok(mut link) => {
            link["address"] = json!(address);
            link
        }
        Err(error) => json!({ "address": address, "error": error.to_string() }),
    }
}

pub async fn links(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<LinksBody>) -> ApiResult<Value> {
    // No one signed in — no answer at all, not a list of refusals.
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let app_id = body.app.as_deref().filter(|id| !id.is_empty());
    let input = json!({ "probe": body.probe });
    if let Some(folder) = body.folder.as_deref() {
        if body.names.len() > PAGE_MOST {
            return Err(ApiError::bad_request(crate::tr!("names: до {most} имён", most = PAGE_MOST)));
        }
        let to = Address::parse(folder).map_err(ApiError::bad_request)?;
        let (trail, from) = sender(&caller, app_id)?;
        let mut input = input;
        input["names"] = json!(body.names);
        let answer = app.bus.call(trail, from, to.clone(), "media", input, vec![]).await.map_err(api_error)?;
        let links: Vec<Value> = answer
            .get("links")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut link| {
                let name = link.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                link["address"] = json!(to.join(&name).map(|address| address.to_string()).unwrap_or_default());
                link
            })
            .collect();
        return Ok(ok(json!({ "links": links })));
    }
    if body.addresses.len() > ADDRESSES_MOST {
        return Err(ApiError::bad_request(crate::tr!("addresses: до {most} адресов", most = ADDRESSES_MOST)));
    }
    // A letter each, each its own chain: a chain may send 32 letters in all.
    let asked = body.addresses.iter().map(|address| {
        let (app, input) = (app.clone(), input.clone());
        let sent = sender(&caller, app_id);
        async move {
            let to = match Address::parse(address) {
                Ok(to) => to,
                Err(error) => return json!({ "address": address, "error": error }),
            };
            match sent {
                Ok((trail, from)) => answered(address, app.bus.call(trail, from, to, "media", input, vec![]).await),
                Err(error) => json!({ "address": address, "error": error.to_string() }),
            }
        }
    });
    let links = futures::future::join_all(asked).await;
    Ok(ok(json!({ "links": links })))
}
