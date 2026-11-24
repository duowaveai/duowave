//! The tests of `helpers/helping.rs` (crates/duowave-pc) that need the whole gateway (`App`): its crate does not have it, the program does.

use crate::computer::tests::helpers_builtin::computer;
use crate::helpers::helping::*;
use crate::settings::definitions::{HelperOutside, HELPER_OUTSIDE};

/// Inside a container the sheet shows neither the container's link port nor its addresses — compose.yaml
/// publishes the port as another one —: the key alone to copy, «Адрес снаружи» first when it is set. Outside
/// a container, the port it listens on.
#[tokio::test]
async fn in_a_container_the_key_sheet_names_no_port_of_its_own() {
    let (app, _dir) = computer(&[]);
    app.pc.link.set_port(3001);
    let key = app.pc.link.connection_key(true);
    let sheet_in = sheet(&app.pc.link, &app.settings, &app.identity, &key, true);
    assert_eq!((sheet_in.port, sheet_in.addresses.len()), (None, 0), "{sheet_in:?}");
    assert_eq!(sheet_in.payload, key.shown(), "the key alone");
    let shown = serde_json::to_value(&sheet_in).unwrap();
    assert!(shown.get("port").is_none(), "{shown}");
    assert_eq!(sheet(&app.pc.link, &app.settings, &app.identity, &key, false).port, Some(3001));

    let outside = HelperOutside { url: "https://helper.example.com".into() };
    app.settings.set(&HELPER_OUTSIDE, &outside, None).unwrap();
    let sheet_in = sheet(&app.pc.link, &app.settings, &app.identity, &key, true);
    assert_eq!(sheet_in.addresses, ["https://helper.example.com"]);
    assert_eq!(sheet_in.payload, format!("https://helper.example.com {}", key.shown()));
    assert_eq!(sheet_in.port, None);
}
