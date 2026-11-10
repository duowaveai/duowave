//! What the station says of what our player plays (skills/media/playback.rs, stage 5а of docs/plans/plugins.md), to
//! whoever hears of it: the journal of the request that started the item («колонка начала играть», «молчит 5 с»),
//! and the plugin whose item it is (`player.*` events).

use std::sync::{Arc, Weak};

use crate::core::requests::{Place, RequestPhase};
use crate::plugins::PluginHost;
use crate::requests::RequestsService;
use crate::skills::media::playback::{PlaybackReport, PlaybackSink};

pub struct PlaybackLinks {
    pub requests: Arc<RequestsService>,
    pub plugins: Weak<PluginHost>,
    /// The panel's widgets: what plays changed.
    pub events: crate::core::events::EventBus,
}

impl PlaybackSink for PlaybackLinks {
    fn report(&self, report: &PlaybackReport) {
        if let Some(request) = &report.request {
            let note = report.news.note(&report.item.title, report.via_gateway);
            self.requests.annotate(request, RequestPhase::Play, Some(Place::Own), &note);
        }
        if let Some(plugins) = self.plugins.upgrade() {
            plugins.player_news(report);
        }
        self.events.emit(crate::core::events::GatewayEvent::StationStateChanged {
            device: report.device.clone(),
            what: "player".into(),
        });
    }
}
