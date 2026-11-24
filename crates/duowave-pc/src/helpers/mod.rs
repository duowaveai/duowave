//! The helpers — the computer's part, added to «Дом»'s `helpers` under its name: the built-in helper (the computer's
//! heavy part behind the same contract), the helper's side of the link and «Помогаю» (`link_serve`, `clients`, its
//! routes `helping`), its cards' hardware addresses for a wake, and «Поиск дома» for a Turbo that moved.

use std::sync::{Arc, Weak};
use std::time::Duration;

pub use duowave_home::helpers::*;

pub mod builtin;
pub mod clients;
pub mod helping;
pub mod link_serve;
pub mod routes;
pub mod wake;

crate::route_needs!(
    Parts,
    Arc<Helpers>,
    Arc<crate::core::identity::home::Identity>,
    Arc<crate::settings::SettingsService>,
    crate::core::events::EventBus,
    Arc<link_serve::LinkServer>,
    Arc<crate::placement::watch::StationGateways>,
);

/// How often a Turbo at home that is away is looked for by its announcement (`Helpers::search_moved`).
pub const MOVED_EVERY: Duration = Duration::from_secs(120);

/// The away helpers `(entry, link url, helper id)` found at another address among the announcements: `(entry, new
/// link url)`. A helper is known by the id it names in TXT (`id`) and its link port (`link`); one found at the same
/// address is not moved.
pub fn moved_helpers(
    away: &[(String, String, Option<String>)],
    found: &[crate::stations::connect::discovery::FoundStation],
) -> Vec<(String, String)> {
    away.iter()
        .filter_map(|(entry, url, helper)| {
            let helper = helper.as_deref()?;
            let home = found.iter().find(|home| home.txt.get("id").map(String::as_str) == Some(helper))?;
            let port: u16 = home.txt.get("link")?.parse().ok()?;
            let moved = format!("https://{}:{port}", home.address);
            (moved != *url).then(|| (entry.clone(), moved))
        })
        .collect()
}

/// «Поиск дома» for a Turbo that moved (docs/plans/home-setup-flow.md, stage 6): a linked helper at home that is
/// away is looked for on the home network by its own announcement (`_duowave._tcp`, its id and link port in TXT) —
/// one multicast question, never a sweep of the subnet. Found at another address, its entry's link address
/// follows; the pinned certificate still proves it is the same computer. Every `MOVED_EVERY`, only while one is away.
pub fn search_moved(helpers: &Arc<Helpers>) {
    let me: Weak<Helpers> = Arc::downgrade(helpers);
    let stop = helpers.stopping();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(MOVED_EVERY) => {}
                _ = stop.cancelled() => break,
            }
            let Some(helpers) = me.upgrade() else { break };
            let away: Vec<(String, String, Option<String>)> = helpers
                .views()
                .into_iter()
                .filter(|view| view.enabled && view.kind == HelperKind::Home && view.state == HelperState::Down)
                .filter_map(|view| {
                    let entry = helpers.entry(&view.id)?;
                    Some((entry.id.clone(), entry.url.clone(), entry.helper_id.clone()))
                })
                .filter(|(_, _, helper)| helper.is_some())
                .collect();
            if away.is_empty() {
                continue;
            }
            let found = crate::stations::connect::discovery::discover_homes(1500).await;
            for (id, url) in moved_helpers(&away, &found) {
                tracing::info!(target: "helpers", "helpers: {id} answers at {url} now (its announcement): the link follows");
                if let Err(error) = helpers.readdress(&id, &url) {
                    tracing::warn!(target: "helpers", "helpers: {id} not moved to {url}: {}", error.message);
                }
            }
        }
    });
}

/// A Turbo that moved is followed by its announcement, not by a sweep of the subnet.
#[cfg(test)]
mod moved_tests {
    use crate::stations::connect::discovery::FoundStation;

    fn home(address: &str, id: &str, link: &str) -> FoundStation {
        let txt = [("role", "turbo"), ("id", id), ("link", link)]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        FoundStation { address: address.into(), name: None, txt, ssh: None }
    }

    #[test]
    fn an_away_helper_found_at_another_address_is_followed_by_its_id_and_link_port() {
        let away = vec![
            ("laptop".to_string(), "https://192.168.1.20:7781".to_string(), Some("gw-laptop".to_string())),
            ("still".to_string(), "https://192.168.1.30:7781".to_string(), Some("gw-still".to_string())),
            ("unknown".to_string(), "https://192.168.1.40:7781".to_string(), None),
        ];
        let found = vec![
            home("192.168.1.104", "gw-laptop", "7781"),
            home("192.168.1.30", "gw-still", "7781"),
            home("192.168.1.50", "gw-stranger", "7781"),
        ];
        assert_eq!(
            super::moved_helpers(&away, &found),
            vec![("laptop".to_string(), "https://192.168.1.104:7781".to_string())],
            "only the one at another address; one that never named itself is not guessed"
        );
    }
}
