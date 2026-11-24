//! What the home asks of the computer (`home::computer::ComputerHome`, «Дом»): the computer's answer, over its stations'
//! watch, its model shop and voice lab, and its link with its clients.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

pub use duowave_home::home::computer::*;

use super::Parts;
use crate::characters::CharactersService;
use crate::core::events::{Actor, EventBus};
use crate::platform::http::ApiError;

/// The computer's parts the home works with.
pub struct Computer {
    pub station_gateway: Arc<crate::placement::watch::StationGateways>,
    pub models: Arc<crate::models::ModelsService>,
    pub voice_lab: Arc<crate::voice_lab::VoiceLabService>,
    pub link: Arc<crate::helpers::link_serve::LinkServer>,
    pub clients: Arc<crate::helpers::clients::Clients>,
    pub events: EventBus,
}

impl ComputerHome for Computer {
    fn turbo_devices<'a>(&'a self, parts: &'a Parts) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            if let Some(centre) = super::devices::centre_elsewhere(&self.station_gateway) {
                if let Some(client) = centre.client() {
                    match client.get("/api/v1/home/devices").await {
                        Ok(view) if view.get("devices").is_some() => return Some(view),
                        Ok(_) => tracing::warn!("home devices: the centre answered without a list"),
                        Err(error) => tracing::debug!("home devices: the centre is away: {error}"),
                    }
                }
                let view = super::devices::as_turbo(&parts.identity, &parts.settings, &centre);
                return Some(serde_json::to_value(view).unwrap_or_default());
            }
            // The Turbo of a home whose centre is another computer (the link): that home, not one of its own.
            let home = super::devices::link_home(self, parts)
                .and_then(|client| super::devices::as_link_turbo(parts, &client))?;
            Some(serde_json::to_value(home).unwrap_or_default())
        })
    }

    fn ask_centre<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            let centre = super::devices::centre_elsewhere(&self.station_gateway)?;
            centre.client()?.get(path).await.ok()
        })
    }

    fn centre_address(&self) -> Option<String> {
        let centre = super::devices::centre_elsewhere(&self.station_gateway)?;
        Some(centre.record().map(|record| record.address).unwrap_or_default())
    }

    fn elsewhere(&self) -> HashSet<String> {
        use crate::core::availability::Placement;
        self.station_gateway
            .all()
            .iter()
            .filter_map(|station| station.record())
            .filter(|record| record.placement != Placement::Pc)
            .filter_map(|record| record.device)
            .collect()
    }

    fn voice_waiting<'a>(&'a self, characters: &'a Arc<CharactersService>) -> BoxFuture<'a, Option<Value>> {
        Box::pin(crate::home_server::voice_plan::waiting(&self.models, characters, &self.voice_lab))
    }

    fn answer_voice<'a>(
        &'a self,
        answer: &'a str,
        actor: Option<Actor>,
    ) -> BoxFuture<'a, Option<Result<(), ApiError>>> {
        Box::pin(async move {
            self.models.pending()?;
            Some(crate::home_server::voice_plan::answer(&self.models, answer, actor).await.map(|_| ()))
        })
    }

    fn join_station<'a>(&'a self, station: &'a str) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(super::ear::join_station(&self.station_gateway, &self.events, station))
    }

    fn link_centre(&self, parts: &Parts) -> Option<Value> {
        super::devices::link_centre(self, parts)
    }
}
