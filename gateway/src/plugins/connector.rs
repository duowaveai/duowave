//! The plugins as one connector of the registry (core/connectors.rs): the connectors list shows how they are doing,
//! and their tools reach «Мозг» the way a connector's do — `ToolContribution`s, with the right each needs.
use std::sync::Weak;

use futures::future::BoxFuture;

use super::controller::PluginHost;
use crate::core::connectors::{Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContribution};

pub struct PluginsConnector {
    host: Weak<PluginHost>,
}

impl PluginsConnector {
    pub fn new(host: Weak<PluginHost>) -> PluginsConnector {
        PluginsConnector { host }
    }
}

impl Connector for PluginsConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "plugins".into(),
            kind: ConnectorKind::Plugins,
            title: crate::tr!("Приложения"),
            description: crate::tr!(
                "Навыки, инструменты для «Мозга» и источники музыки и радио — из установленных приложений"
            ),
            // A plugin that reaches the internet says so in its card; the connector itself sends nothing out.
            cloud: false,
            env: vec!["PACKAGES_DIR".into(), "PLUGINS_DISABLED".into(), "PLUGINS_IDLE_UNLOAD_S".into()],
            capabilities: None,
            setup: None,
        }
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move {
            let Some(host) = self.host.upgrade() else {
                return ConnectorStatus::off(crate::tr!("не запущены"));
            };
            if host.safe_mode() {
                return ConnectorStatus::off(crate::tr!("безопасный режим (PLUGINS_DISABLED)"));
            }
            let view = host.view().await;
            let on = view.packages.iter().filter(|package| package.enabled).count();
            let failing = view.packages.iter().filter(|package| package.state == "error").count();
            match (on, failing) {
                (0, _) => {
                    ConnectorStatus::unset(crate::tr!("найдено {found}, включённых нет", found = view.packages.len()))
                }
                (_, 0) => {
                    ConnectorStatus::ready(crate::tr!("включено {on} из {all}", on = on, all = view.packages.len()))
                }
                (_, failing) => ConnectorStatus::error(crate::tr!(
                    "включено {on}, не работают здесь: {failing}",
                    on = on,
                    failing = failing
                )),
            }
        })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        self.host.upgrade().map(|host| host.tool_contributions()).unwrap_or_default()
    }
}
