//! What the helpers take of the gateway once it is built (step В0 of docs/plans/modular-gateway.md): the built-in
//! helper's questions to the parts made after it, the link door's parts and `/home/v1`, and where a linked helper's own
//! requests (`up`) go. Each holds the gateway weakly, as the helpers always did: the gateway holds them, and a strong
//! hold back would keep it from ever ending — the desktop app starts it again in its process.
use std::sync::{Arc, Weak};

use crate::app::App;

/// The helpers given what they take of the gateway, once it is built (`App::build`).
pub fn attach(app: &Arc<App>) {
    #[cfg(feature = "pc")]
    {
        // The built-in helper's late parts: the bot, the apps, their history, the catalog (stage 6).
        app.builtin.attach(Arc::new(Late(Arc::downgrade(app))));
        // The link's door (stage 7): who this home is, the built-in helper and the voice — a pairing's answer, the
        // status a home sees —, and `/home/v1` for a link's requests, made for each link.
        let weak = Arc::downgrade(app);
        app.link.attach(crate::helpers::link_serve::LinkParts {
            identity: app.identity.clone(),
            builtin: app.builtin.clone(),
            voice: app.voice.clone(),
            home_v1: Arc::new(move || weak.upgrade().map(|app| crate::home_server::serve::router().with_state(app))),
        });
    }
    // A linked helper's own requests (`up`) go into this gateway's routes (stage 7 of home-setup-flow), as the person
    // they name.
    let weak = Arc::downgrade(app);
    let gate: Weak<dyn crate::platform::http::Gate> = weak.clone();
    let router = Arc::new(move || weak.upgrade().map(crate::app::router));
    app.helpers.attach(crate::helpers::UpWay { router, gate });
}

/// The built-in helper's questions to the parts made after it (`helpers::builtin::LateParts`), over the gateway held
/// weakly: once it is gone, no.
#[cfg(feature = "pc")]
struct Late(Weak<App>);

#[cfg(feature = "pc")]
impl crate::helpers::builtin::LateParts for Late {
    fn here(&self) -> bool {
        self.0.strong_count() > 0
    }

    fn clouds_out(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.helpers.lets(crate::helpers::Reach::Foreign))
    }

    fn messages(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.calls.telegram.enabled())
    }

    fn history(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.history.is_some())
    }

    fn catalog(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.catalog.has_copy_for_station())
    }

    fn devices(&self) -> serde_json::Value {
        self.0.upgrade().map(|app| serde_json::json!(app.house.given())).unwrap_or_else(|| serde_json::json!([]))
    }

    fn folders(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.bus.files().is_some())
    }
}
