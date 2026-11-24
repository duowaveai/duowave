//! The built-in helper's questions to the parts made after it (`helpers::builtin::LateParts`), over the gateway held
//! weakly: once it is gone, no. Each holds the gateway weakly, as the helpers always did: the gateway holds them, and a
//! strong hold back would keep it from ever ending — the desktop app starts it again in its process.

use std::sync::Weak;

use crate::app::App;

/// The built-in helper's questions to the parts made after it (`helpers::builtin::LateParts`), over the gateway held
/// weakly: once it is gone, no.
pub struct Late(pub Weak<App>);

impl crate::helpers::builtin::LateParts for Late {
    fn here(&self) -> bool {
        self.0.strong_count() > 0
    }

    fn clouds_out(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.helpers.lets(crate::helpers::Reach::Foreign))
    }

    fn messages(&self) -> bool {
        self.0.upgrade().is_some_and(|app| app.pc.calls.telegram.enabled())
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
        self.0.upgrade().is_some_and(|app| crate::bus::BusFolders::files(&*app.bus).is_some())
    }
}
