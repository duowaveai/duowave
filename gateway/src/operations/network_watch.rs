//! The station's own network, watched without sending anything. A tester's Mini 2, 22.11: its Wi-Fi went for minutes
//! many times a day — the gateway itself was fine —, and every voice request then waited about 10 s for the cloud and
//! ended in «Не получилось получить ответ» («tcp connect error: Network unreachable (os error 101)»).
//!
//! - `online_now()`: whether the device has a way out now — a default route through a router (/proc/net/route) on
//!   an interface whose link is up (/sys/class/net/<if>/operstate). Read at once, nothing sent: the voice path asks
//!   it before it waits for anything outside. None where the kernel does not say it (not Linux): then nobody assumes
//!   «offline». A default route with no router counts for nothing: the Lite 2 keeps one on `p2p0` (Wi-Fi Direct,
//!   169.254.x.x, its link «unknown») whether its Wi-Fi is there or not (22.11: the router's Wi-Fi off, the watch saw
//!   no outage).
//! - `NetworkWatch`: a look every few seconds on the station; an outage begins when the way out goes and ends when it
//!   is back. The outages of the last days are kept in the state folder (`network-outages.json`, a few KB), so «за
//!   сегодня теряла Wi-Fi 4 раза, всего 12 минут» outlives a restart, and one that began before a reboot ends after it.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::time::now_ms;

/// How often the station looks at its way out.
pub const EVERY: Duration = Duration::from_secs(5);
/// The outages kept: the last three days, at most this many.
const KEEP_MS: u64 = 3 * 24 * 3600 * 1000;
const KEEP_MAX: usize = 300;
/// The file in the state folder.
const FILE: &str = "network-outages.json";
/// Before this (2025-01-01) the clock is not set: the station has no clock of its own and counts from 1970 until it
/// hears the time over the network. A look then is not counted (22.11: an outage «since 1970» after a restart).
const CLOCK_SET_MS: u64 = 1_735_689_600_000;

/// One outage: from its first look without a way out to the first look with one, unix ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outage {
    pub from: u64,
    pub to: u64,
}

/// What is kept: the outage going on now, and the ended ones, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outages {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<u64>,
    #[serde(default)]
    pub ended: VecDeque<Outage>,
}

/// The interfaces with a default route through a router in /proc/net/route: destination and mask 0, the route up
/// (flag 1) and through a gateway (flag 2).
pub fn default_routes(table: &str) -> Vec<String> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (interface, destination, flags, mask) =
                (fields.first()?, fields.get(1)?, fields.get(3)?, fields.get(7)?);
            // RTF_UP | RTF_GATEWAY.
            let up = u32::from_str_radix(flags, 16).is_ok_and(|flags| flags & 3 == 3);
            (up && *destination == "00000000" && *mask == "00000000").then(|| interface.to_string())
        })
        .collect()
}

/// A link that carries traffic: «up», or «unknown» (drivers that do not say).
fn link_up(operstate: &str) -> bool {
    matches!(operstate.trim(), "up" | "unknown")
}

/// Whether there is a way out, by the route table and each interface's link state (None: not said — counts as up).
pub fn online_by(table: &str, operstate: impl Fn(&str) -> Option<String>) -> bool {
    default_routes(table).iter().any(|interface| operstate(interface).is_none_or(|state| link_up(&state)))
}

/// Whether this device has a way out now; None where the kernel does not say it.
pub fn online_now() -> Option<bool> {
    let table = std::fs::read_to_string("/proc/net/route").ok()?;
    Some(online_by(&table, |interface| {
        std::fs::read_to_string(Path::new("/sys/class/net").join(interface).join("operstate")).ok()
    }))
}

/// The station's outages, as its looks saw them.
pub struct NetworkWatch {
    file: Option<PathBuf>,
    kept: Mutex<Outages>,
}

impl NetworkWatch {
    /// The outages kept in this state folder (none: only in memory).
    pub fn new(state_dir: Option<&Path>) -> NetworkWatch {
        let file = state_dir.map(|dir| dir.join(FILE));
        let mut kept: Outages = file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // What an unset clock counted: gone.
        kept.offline_since = kept.offline_since.filter(|from| *from >= CLOCK_SET_MS);
        kept.ended.retain(|outage| outage.from >= CLOCK_SET_MS);
        NetworkWatch { file, kept: Mutex::new(kept) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Outages> {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One look at `at` (unix ms): an outage begins, goes on or ends. True when it began or ended. A look while the
    /// clock is not set counts for nothing.
    pub fn note(&self, online: bool, at: u64) -> bool {
        if at < CLOCK_SET_MS {
            return false;
        }
        let changed = {
            let mut kept = self.lock();
            match (online, kept.offline_since) {
                (false, None) => {
                    kept.offline_since = Some(at);
                    true
                }
                (true, Some(from)) => {
                    kept.offline_since = None;
                    kept.ended.push_back(Outage { from, to: at.max(from) });
                    while kept.ended.len() > KEEP_MAX
                        || kept.ended.front().is_some_and(|outage| at.saturating_sub(outage.to) > KEEP_MS)
                    {
                        kept.ended.pop_front();
                    }
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.save();
        }
        changed
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let text = serde_json::to_string(&*self.lock()).unwrap_or_default();
        // A new file beside, then over the old one: a cut write leaves the old one whole.
        let fresh = file.with_extension("json.new");
        if std::fs::write(&fresh, text).is_ok() {
            let _ = std::fs::rename(&fresh, file);
        }
    }

    /// The outages: the one going on, and the ended ones of the last days.
    pub fn outages(&self) -> Outages {
        self.lock().clone()
    }

    /// Since when the station is off the network now.
    pub fn offline_since(&self) -> Option<u64> {
        self.lock().offline_since
    }
}

static SHARED: OnceLock<Arc<NetworkWatch>> = OnceLock::new();

/// The station's watch, once started.
pub fn shared() -> Option<Arc<NetworkWatch>> {
    SHARED.get().cloned()
}

/// Starts the looks (once a process): on the station, where the kernel says its routes.
pub fn start(state_dir: Option<&Path>) {
    if online_now().is_none() {
        return;
    }
    let watch = Arc::new(NetworkWatch::new(state_dir));
    if SHARED.set(watch.clone()).is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Some(online) = online_now() {
                if watch.note(online, now_ms()) {
                    match watch.offline_since() {
                        Some(_) => tracing::warn!(target: "network", "the station is off the network: no way out"),
                        None => tracing::info!(target: "network", "the station is back on the network"),
                    }
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                         wlan0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
                         wlan0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n";

    #[test]
    fn a_default_route_on_a_link_that_is_up_is_a_way_out() {
        assert_eq!(default_routes(TABLE), vec!["wlan0".to_string()]);
        assert!(online_by(TABLE, |_| Some("up\n".into())));
        assert!(online_by(TABLE, |_| None), "a link that says nothing counts as up");
        // The Wi-Fi gone: the link down, or the routes gone with the address (Network unreachable).
        assert!(!online_by(TABLE, |_| Some("down\n".into())));
        assert!(!online_by(TABLE, |_| Some("dormant".into())));
        let no_default = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
                          wlan0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n";
        assert!(!online_by(no_default, |_| Some("up".into())));
        assert!(!online_by("Iface\tDestination\n", |_| Some("up".into())), "an empty table");
    }

    /// The Lite 2's table, 22.11: a default route on `p2p0` with no router (Wi-Fi Direct, its link «unknown») stays
    /// there with the Wi-Fi gone — it is no way out.
    #[test]
    fn a_default_route_with_no_router_is_no_way_out() {
        let with_wifi = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                         wlan0\t00000000\t0101A8C0\t0003\t0\t0\t3005\t00000000\t0\t0\t0\n\
                         p2p0\t00000000\t00000000\t0001\t0\t0\t1003006\t00000000\t0\t0\t0\n\
                         lo\t0000007F\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n\
                         p2p0\t0000FEA9\t00000000\t0001\t0\t0\t3006\t0000FFFF\t0\t0\t0\n\
                         wlan0\t0001A8C0\t00000000\t0001\t0\t0\t3005\t00FFFFFF\t0\t0\t0\n";
        let state = |interface: &str| Some(if interface == "p2p0" { "unknown" } else { "up" }.to_string());
        assert_eq!(default_routes(with_wifi), vec!["wlan0".to_string()]);
        assert!(online_by(with_wifi, state));
        let wifi_gone = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                         p2p0\t00000000\t00000000\t0001\t0\t0\t1003006\t00000000\t0\t0\t0\n\
                         lo\t0000007F\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n\
                         p2p0\t0000FEA9\t00000000\t0001\t0\t0\t3006\t0000FFFF\t0\t0\t0\n";
        assert!(!online_by(wifi_gone, state));
        assert!(!online_by(with_wifi, |interface| Some(
            if interface == "wlan0" { "dormant" } else { "unknown" }.into()
        )));
    }

    /// A real time: 22.11.2026, 18:00 UTC.
    const T: u64 = 1_795_284_000_000;

    #[test]
    fn an_outage_begins_once_ends_once_and_is_kept_in_the_state_folder() {
        let state = tempfile::tempdir().unwrap();
        let watch = NetworkWatch::new(Some(state.path()));
        assert!(!watch.note(true, T), "online and nothing going on: no change");
        assert!(watch.note(false, T + 1_000));
        assert!(!watch.note(false, T + 6_000), "the same outage goes on");
        assert_eq!(watch.offline_since(), Some(T + 1_000));

        // A restart in the middle of it: it is still going on, and ends after.
        let again = NetworkWatch::new(Some(state.path()));
        assert_eq!(again.offline_since(), Some(T + 1_000));
        assert!(again.note(true, T + 61_000));
        let kept = NetworkWatch::new(Some(state.path())).outages();
        assert_eq!(kept.offline_since, None);
        assert_eq!(kept.ended, VecDeque::from([Outage { from: T + 1_000, to: T + 61_000 }]));
    }

    #[test]
    fn old_outages_go() {
        let watch = NetworkWatch::new(None);
        watch.note(false, T);
        watch.note(true, T + 1_000);
        let later = T + 1_000 + KEEP_MS + 10_000;
        watch.note(false, later);
        watch.note(true, later + 1_000);
        assert_eq!(watch.outages().ended, VecDeque::from([Outage { from: later, to: later + 1_000 }]));
    }

    /// The station has no clock of its own: after a restart it counts from 1970 until it hears the time (22.11: an
    /// outage «since 1970» was kept). Such looks count for nothing, and what an earlier build kept from them is dropped.
    #[test]
    fn nothing_is_counted_while_the_clock_is_not_set() {
        let state = tempfile::tempdir().unwrap();
        let watch = NetworkWatch::new(Some(state.path()));
        assert!(!watch.note(false, 17_780), "17 s after 1970: the clock is not set");
        assert_eq!(watch.offline_since(), None);

        std::fs::write(
            state.path().join(FILE),
            format!(
                r#"{{"offlineSince":17780,"ended":[{{"from":17780,"to":{}}},{{"from":{},"to":{}}}]}}"#,
                T,
                T,
                T + 5_000
            ),
        )
        .unwrap();
        let kept = NetworkWatch::new(Some(state.path())).outages();
        assert_eq!(kept.offline_since, None);
        assert_eq!(kept.ended, VecDeque::from([Outage { from: T, to: T + 5_000 }]));
    }
}
