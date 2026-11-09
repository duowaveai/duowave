//! What the panel's live channel reads of the stations, as live.module.ts injected the presence service: every
//! approved station's presence, for a panel's hello. Later changes come on the bus (`station.presence`).

use std::sync::Arc;

use crate::core::stations::presence::StationPresence;
use crate::live::PresenceSource;
use crate::stations::PresenceService;

pub struct LivePresence(pub Arc<PresenceService>);

impl PresenceSource for LivePresence {
    fn list(&self) -> Vec<StationPresence> {
        self.0.list()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::core::identity::{Principal, PrincipalKind};
    use crate::live::{LiveMessage, LiveSink};
    use crate::utils::time::now_ms;

    /// A panel's stream that keeps what it is sent.
    #[derive(Default)]
    struct Panel(Mutex<Vec<LiveMessage>>);

    impl LiveSink for std::sync::Arc<Panel> {
        fn send(&self, message: &LiveMessage) -> bool {
            self.0.lock().unwrap().push(message.clone());
            true
        }

        fn end(&self) {}
    }

    #[tokio::test]
    async fn a_panels_hello_says_where_each_station_is() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        // The first station is trusted on first use; presence looks at it.
        assert!(app.stations.admit("kitchen", "192.168.1.10").ok);
        app.presence.refresh(now_ms());
        let admin = Principal {
            kind: PrincipalKind::User,
            id: "admin".into(),
            name: "admin".into(),
            role: None,
            grants: vec!["station.view".into()],
            denies: Vec::new(),
        };
        let panel = std::sync::Arc::new(Panel::default());
        let _leave = app.live.join(&admin, Box::new(panel.clone()));
        let messages = panel.0.lock().unwrap().clone();
        let Some(LiveMessage::Hello { presence, .. }) = messages.first() else { panic!("a hello first: {messages:?}") };
        let devices: Vec<&str> = presence.iter().map(|station| station.device.as_str()).collect();
        assert_eq!(devices, ["kitchen"]);
    }
}
