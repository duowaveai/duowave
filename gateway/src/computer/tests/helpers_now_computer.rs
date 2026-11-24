//! The computer's part of whole_tests/helpers_now.rs: what only a computer has, tested with the whole gateway built.

use crate::helpers::contract::Ability;
use crate::helpers::now::*;
use crate::helpers::Reach;

/// A station's own voice server elsewhere at home (TTS_OWN_URL, not a helper's): «Где сейчас» says its host — the
/// speaker runs no voice server itself; one on this machine stays unnamed, as before.
#[tokio::test]
async fn a_stations_voice_server_elsewhere_at_home_is_named_in_where_now() {
    use crate::app::App;
    use crate::computer::tests::helpers_builtin::computer;
    let voice = |app: &std::sync::Arc<App>| {
        rows(&crate::platform::http::Provides::provide(app))
            .into_iter()
            .find(|row| row.ability == Ability::Voice)
            .unwrap()
    };
    let station =
        |voice: &'static str| [("DUOWAVE_ROLE", "station"), ("TTS_PROVIDER", "mixed"), ("TTS_OWN_URL", voice)];
    let (app, _dir) = computer(&station("http://192.168.1.50:8091/v1"));
    let row = voice(&app);
    assert_eq!(
        (row.by, row.name.as_deref(), row.reach, row.first),
        (By::Here, Some("192.168.1.50"), Some(Reach::Home), true),
        "{row:?}"
    );
    let (app, _dir) = computer(&station("http://127.0.0.1:8091/v1"));
    let row = voice(&app);
    assert_eq!((row.by, row.name, row.first), (By::Here, None, true), "on this machine");
}
