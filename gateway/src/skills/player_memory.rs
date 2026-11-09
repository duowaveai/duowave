//! Whether the station's player was playing when we last heard of it, kept across our restarts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::storage::StateStore;
use crate::utils::time::iso;

/// Whose content: Yandex's (its radio, its music; it holds the queue) or ours (/media).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentSource {
    Yandex,
    Own,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerRecord {
    pub playing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ContentSource>,
    /// What told us: a callback of the player or the station's device state.
    pub by: String,
    pub at: String,
}

/// The player's state per device (STATE_DIR, kind `player`). Reconnecting, the station asks to continue its
/// player; only music that played before the break should go on (live: after our restart Yandex started a track
/// an hour after the radio was stopped).
pub struct PlayerMemory {
    store: StateStore<PlayerRecord>,
}

impl PlayerMemory {
    pub fn new(store: StateStore<PlayerRecord>) -> PlayerMemory {
        PlayerMemory { store }
    }

    pub fn get(&self, device: &str) -> Option<PlayerRecord> {
        self.store.get(device).into_iter().next()
    }

    /// Written on a change only: the record says when the player started or stopped, and whose content it was.
    pub fn note(&self, device: &str, playing: bool, by: &str, now: DateTime<Utc>, source: Option<ContentSource>) {
        let before = self.get(device);
        if let Some(before) = &before {
            if before.playing == playing && (source.is_none() || before.source == source) {
                return;
            }
        }
        let known = source.or(before.and_then(|record| record.source));
        self.store.set(device, vec![PlayerRecord { playing, source: known, by: by.to_string(), at: iso(now) }]);
    }

    /// A callback of the player, Yandex's (`music_thin_client_on_*`) or ours (`vgw_media_on_*`).
    pub fn note_callback(&self, device: &str, name: &str, now: DateTime<Utc>) {
        let source = if name.starts_with("music_thin_client") {
            Some(ContentSource::Yandex)
        } else if name.starts_with("vgw_media") {
            Some(ContentSource::Own)
        } else {
            None
        };
        if name.ends_with("on_started") {
            self.note(device, true, name, now, source);
        } else if ["on_stopped", "on_finished", "on_failed"].iter().any(|end| name.ends_with(end)) {
            self.note(device, false, name, now, source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use std::sync::Arc;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn keeps_what_the_player_did_last_across_a_restart_from_its_callbacks() {
        let db = Arc::new(Db::memory());
        let memory = PlayerMemory::new(StateStore::open(db.clone(), "player"));
        memory.note_callback("station", "music_thin_client_on_started", at("2026-11-01T05:00:00Z"));
        memory.note_callback("station", "music_thin_client_on_stopped", at("2026-11-01T05:07:00Z"));

        let restarted = PlayerMemory::new(StateStore::open(db, "player"));
        assert_eq!(
            restarted.get("station"),
            Some(PlayerRecord {
                playing: false,
                source: Some(ContentSource::Yandex),
                by: "music_thin_client_on_stopped".into(),
                at: "2026-11-01T05:07:00.000Z".into(),
            })
        );
    }

    #[test]
    fn marks_the_moment_of_a_change_not_of_every_report() {
        let memory = PlayerMemory::new(StateStore::open(Arc::new(Db::memory()), "player"));
        memory.note("station", true, "vgw_media_on_started", at("2026-11-01T05:00:00Z"), None);
        memory.note("station", true, "device state", at("2026-11-01T05:03:00Z"), None);
        memory.note_callback("station", "music_thin_client_on_heartbeat", Utc::now());
        let record = memory.get("station").unwrap();
        assert!(record.playing);
        assert_eq!(record.by, "vgw_media_on_started");
    }
}
