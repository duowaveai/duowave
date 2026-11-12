//! The gains the station's wake words picked, the last few per station. A request without the wake word (listening
//! on after a question, a button, a wake word of our own) starts from the gain its session's last wake word got; a
//! session that has heard none yet (the gateway restarted, the station reconnected, only a wake word of our own)
//! starts from their median instead of the maximum: one whispered «Алиса» does not set it alone. Kept in the
//! database (state `wake_word_gain`), so a restart of the gateway keeps them too.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::storage::StateStore;

/// Wake words kept per station.
const KEPT: usize = 5;

pub struct WakeWordGains {
    recent: Mutex<BTreeMap<String, Vec<f64>>>,
    store: Option<StateStore<f64>>,
}

impl WakeWordGains {
    /// In memory only (tests, a replay).
    pub fn in_memory() -> WakeWordGains {
        WakeWordGains { recent: Mutex::new(BTreeMap::new()), store: None }
    }

    /// Kept in the database: what it holds is read now.
    pub fn kept(store: StateStore<f64>) -> WakeWordGains {
        let recent = store.devices().into_iter().map(|device| (device.clone(), store.get(&device))).collect();
        WakeWordGains { recent: Mutex::new(recent), store: Some(store) }
    }

    /// The median of the station's last wake words, dB; None before its first.
    pub fn typical(&self, device: &str) -> Option<f64> {
        let mut gains = self.recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(device)?.clone();
        if gains.is_empty() {
            return None;
        }
        gains.sort_by(f64::total_cmp);
        let middle = gains.len() / 2;
        Some(if gains.len() % 2 == 1 { gains[middle] } else { (gains[middle - 1] + gains[middle]) / 2.0 })
    }

    /// The gain a wake word of the station picked.
    pub fn note(&self, device: &str, gain_db: f64) {
        if !gain_db.is_finite() {
            return;
        }
        let gains = {
            let mut recent = self.recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let gains = recent.entry(device.to_string()).or_default();
            gains.push(gain_db);
            let extra = gains.len().saturating_sub(KEPT);
            gains.drain(..extra);
            gains.clone()
        };
        if let Some(store) = &self.store {
            store.set(device, gains);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::storage::Db;

    #[test]
    fn starts_from_the_median_of_the_last_five_wake_words_of_the_station() {
        let gains = WakeWordGains::in_memory();
        assert_eq!(gains.typical("station"), None);
        gains.note("station", 28.0);
        assert_eq!(gains.typical("station"), Some(28.0));
        gains.note("station", 30.0);
        assert_eq!(gains.typical("station"), Some(29.0));
        // A whispered «Алиса» picks the largest gain: one of them does not set it.
        gains.note("station", 40.0);
        assert_eq!(gains.typical("station"), Some(30.0));
        for gain in [27.0, 29.0, 31.0] {
            gains.note("station", gain);
        }
        // The last five: 30, 40, 27, 29, 31 — the first 28 is gone.
        assert_eq!(gains.typical("station"), Some(30.0));
        assert_eq!(gains.typical("another"), None);
        gains.note("station", f64::NAN);
        assert_eq!(gains.typical("station"), Some(30.0));
    }

    #[test]
    fn keeps_them_in_the_database_over_a_restart() {
        let db = Arc::new(Db::open(None).unwrap());
        let before = WakeWordGains::kept(StateStore::open(db.clone(), "wake_word_gain"));
        before.note("station", 26.5);
        before.note("station", 27.5);
        let after = WakeWordGains::kept(StateStore::open(db, "wake_word_gain"));
        assert_eq!(after.typical("station"), Some(27.0));
    }
}
