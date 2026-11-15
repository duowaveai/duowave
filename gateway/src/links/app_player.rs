//! The player's commands to an app that plays by itself (skills/media/app_playback.rs, docs/plans/media-apps.md stage
//! 2): the plugin host calls the function the app hears the news `control` of `system/player` with
//! (`contributes.bus.on`). A person's voice command is no letter of an app: it goes straight to the app.

use std::sync::Weak;

use futures::future::BoxFuture;
use serde_json::json;

use crate::bus::providers::system::PLAYER;
use crate::plugins::runtime::CallKind;
use crate::plugins::PluginHost;
use crate::skills::media::app_playback::AppControl;

/// The news an app hears its player's commands as.
pub const CONTROL: &str = "control";

pub struct AppPlayerLink(pub Weak<PluginHost>);

impl AppControl for AppPlayerLink {
    fn control(&self, app: &str, action: &str) -> BoxFuture<'static, Result<(), String>> {
        let host = self.0.upgrade();
        let (app, action) = (app.to_string(), action.to_string());
        Box::pin(async move {
            let host = host.ok_or_else(|| crate::tr!("приложения остановлены"))?;
            let package = host.package(&app).ok_or_else(|| crate::tr!("нет приложения {id}", id = app))?;
            let player = crate::bus::address::Address::parse(PLAYER).map_err(|error| error.to_string())?;
            let handler = package
                .manifest
                .contributes
                .bus
                .on
                .iter()
                .find(|on| on.event == CONTROL && on.from.matches(&player))
                .map(|on| on.handler.clone())
                .ok_or_else(|| {
                    crate::tr!("{id} не слушает команды плеера (contributes.bus.on: system/player, control)", id = app)
                })?;
            let input = json!({ "from": PLAYER, "event": CONTROL, "payload": { "action": action } });
            host.call(&app, &handler, input, CallKind::Voice).await.map(|_| ()).map_err(|error| error.to_string())
        })
    }

    fn alive(&self, app: &str) -> bool {
        self.0.upgrade().is_some_and(|host| host.is_active(app))
    }

    fn title(&self, app: &str) -> String {
        let Some(host) = self.0.upgrade() else { return app.to_string() };
        match host.package(app) {
            Some(package) => package.manifest.title_in(&crate::locale::preferences(host.settings())),
            None => app.to_string(),
        }
    }
}
