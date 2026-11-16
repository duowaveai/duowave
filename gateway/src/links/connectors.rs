//! What the built-in connectors read of the modules, as connectors.module.ts injected them: the model's connector
//! reads the dialog, our voice's the voice service, and the list ends with the MCP servers added in the panel. And
//! the other way: what the modules read of the connectors the panel sets up — the skills their feeds, place and
//! Home Assistant, MCP Home Assistant's address, the Host check LiveKit's names.
//!
//! connectors.module.ts is the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::sync::Arc;

use crate::config::HomeAssistantConfig;
use crate::connectors::builtin::{DialogInfo, VoiceInfo};
use crate::connectors::settings::{ConnectorId, ConnectorSettings};
use crate::connectors::{McpServerState, McpServers, McpState};
use crate::dialog::DialogService;
use crate::mcp::mcp_service::{HaUrlSource, ServerState};
use crate::mcp::McpService;
use crate::platform::http::security::AllowedHosts;
use crate::skills::lookups::news_skill::NewsFeed;
use crate::skills::SkillConnectors;
use crate::speech::VoiceService;

/// The panel's feeds, place and Home Assistant, as the skills read them on every request.
pub struct SkillSettings(pub Arc<ConnectorSettings>);

impl SkillConnectors for SkillSettings {
    fn news_feeds(&self) -> Vec<NewsFeed> {
        self.0.news_feeds().into_iter().map(|feed| NewsFeed { url: feed.url, topic: feed.topic }).collect()
    }

    fn weather_place(&self) -> Option<String> {
        self.0.weather_place()
    }

    fn home_assistant(&self) -> Option<HomeAssistantConfig> {
        self.0.home_assistant()
    }
}

/// Home Assistant's address for the MCP preset: the panel's, whenever it is set up.
pub fn home_assistant_url(settings: &Arc<ConnectorSettings>) -> HaUrlSource {
    let settings = Arc::downgrade(settings);
    Arc::new(move || settings.upgrade()?.home_assistant().map(|home| home.url))
}

/// The Host check lets LiveKit's names in now and after each change of them.
pub fn follow_livekit_hosts(settings: &Arc<ConnectorSettings>, hosts: &Arc<AllowedHosts>) {
    hosts.set_from_settings(settings.livekit_hosts());
    let (weak, hosts) = (Arc::downgrade(settings), hosts.clone());
    settings.subscribe(move |id| {
        if id == ConnectorId::LiveKit {
            if let Some(settings) = weak.upgrade() {
                hosts.set_from_settings(settings.livekit_hosts());
            }
        }
    });
}

/// The model for conversation: whether it answers, which one, whether the question leaves the house.
pub struct DialogConnector(pub Arc<DialogService>);

impl DialogInfo for DialogConnector {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn description(&self) -> String {
        self.0.description()
    }

    fn cloud(&self) -> bool {
        self.0.brain().cloud
    }
}

/// Whose voice speaks.
pub struct VoiceConnector(pub Arc<VoiceService>);

impl VoiceInfo for VoiceConnector {
    fn describe(&self, yandex_allowed: bool) -> String {
        self.0.describe(yandex_allowed)
    }
}

/// The MCP servers added in the panel, with their state now.
pub struct McpConnectors(pub Arc<McpService>);

impl McpServers for McpConnectors {
    fn states(&self) -> Vec<McpServerState> {
        self.0
            .states()
            .into_iter()
            .map(|server| McpServerState {
                id: server.id,
                title: server.title,
                cloud: server.cloud,
                state: match server.state {
                    ServerState::Off => McpState::Off,
                    ServerState::Connecting => McpState::Connecting,
                    ServerState::Ready => McpState::Ready,
                    ServerState::Error => McpState::Error,
                    ServerState::Login => McpState::Login,
                },
                tools: server.tools,
                detail: server.detail,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::connectors::builtin::StationSessions;
    use crate::core::connectors::{ConnectorState, ConnectorStatus};

    struct Station;

    impl StationSessions for Station {
        fn open(&self) -> usize {
            1
        }
        fn yandex_connected(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn lists_the_built_ins_in_the_node_order_over_the_dialog_the_voice_and_the_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([
            ("STATE_DIR", dir.path().to_str().unwrap()),
            ("DIALOG_PROVIDER", "echo"),
            ("ASR_PROVIDER", "none"),
            ("UNIPROXY_MODE", "gateway"),
        ]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let input =
            serde_json::from_value(json!({"transport": "http", "title": "Файлы", "url": "http://127.0.0.1:9/mcp"}))
                .unwrap();
        let server = app.mcp.add(&input, None).unwrap();
        let views = app.connectors.views().await;
        let ids: Vec<&str> = views.iter().map(|view| view.manifest.id.as_str()).collect();
        let mcp = format!("mcp:{}", server.id);
        assert_eq!(
            ids,
            [
                "yandex",
                "llm",
                "asr",
                "tts",
                "home-assistant",
                "telegram",
                "livekit",
                "sip",
                "weather",
                "news",
                "piper-registry",
                "plugins",
                mcp.as_str()
            ]
        );
        assert_eq!(views[0].status, ConnectorStatus::configured(Some("сессия откроется с колонкой".into())));
        assert_eq!(views[1].status, ConnectorStatus::configured(Some(app.dialog.description())));
        assert!(!views[1].manifest.cloud, "the echo model is at home");
        assert_eq!(views[3].status, ConnectorStatus::unset("свой голосовой сервер не задан"));
        assert_eq!(views[11].status, ConnectorStatus::unset("найдено 0, включённых нет"));
        assert_eq!(views[12].manifest.title, "Файлы");
        // The station link gives its sessions in the build (src/gateway/uniproxy): none connected yet, and a later
        // source does not replace them. A connected station is in gateway/tests/station.rs.
        app.connectors.set_station_sessions(std::sync::Arc::new(Station));
        let yandex = app.connectors.views().await.remove(0).status;
        assert_eq!(yandex.state, ConnectorState::Configured);
        app.shutdown().await;
    }
}
