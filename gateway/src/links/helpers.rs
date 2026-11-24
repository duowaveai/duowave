//! What the helpers take of the gateway once it is built (step В0 of docs/plans/modular-gateway.md): where a linked
//! helper's own requests (`up`) go. The built-in helper's questions to the parts made after it and the link door's
//! parts are the computer's (src/computer, `attach`). Each holds the gateway weakly, as the helpers always did: the gateway holds them, and a strong
//! hold back would keep it from ever ending — the desktop app starts it again in its process.
use std::sync::{Arc, Weak};

use crate::app::App;

/// The helpers given what they take of the gateway, once it is built (`App::build`).
pub fn attach(app: &Arc<App>) {
    // A linked helper's own requests (`up`) go into this gateway's routes (stage 7 of home-setup-flow), as the person
    // they name.
    let weak = Arc::downgrade(app);
    let gate: Weak<dyn crate::platform::http::Gate> = weak.clone();
    let router = Arc::new(move || weak.upgrade().map(crate::app::router));
    app.helpers.attach(crate::helpers::UpWay { router, gate });
}
