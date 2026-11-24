//! The tests of `access/audit_service.rs` that need the helpers (a floor above its crate): the program has them.

use std::sync::Arc;

use crate::access::audit_service::*;
use crate::core::events::{Actor, GatewayEvent};
use crate::core::identity::principal::PrincipalKind;

fn actor() -> Actor {
    Actor { kind: PrincipalKind::User, id: "u1".into(), name: "Мама".into() }
}

/// A change of the helpers is one happening, one row: `helpers.changed`, not also the setting it wrote; and the
/// old `homeServer.*` the shell still emits for older panels are no rows of their own.
#[tokio::test]
async fn a_change_of_the_helpers_writes_one_row() {
    use crate::helpers::tests::{contract, stand, station, ALL, KEY};
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    let audit = Arc::new(AuditService::new(station.db.clone()));
    let mut heard = station.events.subscribe();
    let input = crate::helpers::AddInput { url: computer.url.clone(), key: KEY.into(), ..Default::default() };
    station.helpers.add(input, Some(actor())).await.unwrap();
    station.helpers.set_away(crate::helpers::Away::Mine, Some(actor())).unwrap();
    audit.write(&GatewayEvent::HomeServerChanged { configured: true, actor: None });
    while let Ok(event) = heard.try_recv() {
        audit.write(&event);
    }
    let rows = audit.recent(100).unwrap();
    let actions: Vec<&str> = rows.iter().rev().map(|entry| entry.action.as_str()).collect();
    assert_eq!(actions, ["helpers.changed", "helpers.changed"], "no settings.changed of «helpers» or «away»");
    assert_eq!(rows[1].details["change"], "added");
    assert_eq!(rows[0].details, serde_json::json!({ "change": "away" }));
    assert_eq!(rows[0].actor.as_ref().and_then(|actor| actor.name.as_deref()), Some("Мама"));
}

/// What a helper says of itself is nobody's change (review): its contract read at a look — the first after a start
/// too — and its readiness that changes reach the panels live, not the log, so a helper whose readiness flaps
/// never fills it; its coming is a row.
#[tokio::test]
async fn a_helpers_own_contract_is_heard_live_and_not_logged() {
    use crate::helpers::tests::{contract, stand, station, ALL};
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    station.put("home-a", &computer.url, crate::helpers::HelperKind::Home);
    let audit = AuditService::new(station.db.clone());
    let mut heard = station.events.subscribe();
    station.helpers.check("home-a").await.unwrap();
    let mut loading = ALL.to_vec();
    loading[0] = ("hearing", false);
    *computer.status.lock().unwrap() = contract("pc", &loading);
    station.helpers.check("home-a").await.unwrap();
    let mut contracts = 0;
    while let Ok(event) = heard.try_recv() {
        if matches!(&*event, GatewayEvent::HelpersChanged { change, .. } if change == "contract") {
            contracts += 1;
        }
        audit.write(&event);
    }
    assert_eq!(contracts, 2, "the first read and the readiness that changed, live");
    let actions: Vec<String> = audit.recent(100).unwrap().into_iter().map(|entry| entry.action).collect();
    assert_eq!(actions, ["helpers.up"]);
}
