//! The workshop's door (docs/plans/audio-interface.md, «Жёсткая дверь»): a lock on the route of a device, not on its
//! microphone. While it is open, the layer's rule takes every phrase of the device — the emergency words and the
//! station's own controls («стоп», «громче», timers) apart — as a request, or as an addition to the thing in work or
//! waiting for an answer: no guessing, no model. The words of the exit close it, and so do ten minutes without a
//! phrase. In memory: a restart closes it. It is also the sure way to check the works by hand.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The door closes by itself after this long without a phrase.
pub const CLOSES_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct Door {
    /// Each device whose door is open, and its last phrase.
    open: Mutex<HashMap<String, Instant>>,
}

impl Door {
    fn open_doors(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn open(&self, device: &str) {
        self.open_doors().insert(device.to_string(), Instant::now());
    }

    /// Closes it; false when it was not open.
    pub fn close(&self, device: &str) -> bool {
        let mut doors = self.open_doors();
        let was = doors.get(device).is_some_and(|at| at.elapsed() < CLOSES_AFTER);
        doors.remove(device);
        was
    }

    pub fn is_open(&self, device: &str) -> bool {
        let mut doors = self.open_doors();
        match doors.get(device) {
            Some(at) if at.elapsed() < CLOSES_AFTER => true,
            Some(_) => {
                doors.remove(device);
                false
            }
            None => false,
        }
    }

    /// A phrase went through the open door: it stays open ten minutes more.
    pub fn touch(&self, device: &str) {
        if let Some(at) = self.open_doors().get_mut(device) {
            *at = Instant::now();
        }
    }

    /// For a test: the door of a device was last used this long ago.
    #[cfg(test)]
    pub fn set_idle(&self, device: &str, idle: Duration) {
        if let Some(at) = self.open_doors().get_mut(device) {
            *at = Instant::now() - idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_per_device_and_closes_by_word_or_after_ten_quiet_minutes() {
        let door = Door::default();
        assert!(!door.is_open("kitchen"));
        door.open("kitchen");
        assert!(door.is_open("kitchen") && !door.is_open("hall"));
        door.set_idle("kitchen", CLOSES_AFTER - Duration::from_secs(1));
        door.touch("kitchen");
        assert!(door.is_open("kitchen"), "a phrase keeps it open");
        door.set_idle("kitchen", CLOSES_AFTER);
        assert!(!door.is_open("kitchen"), "ten quiet minutes close it");
        door.open("hall");
        assert!(door.close("hall") && !door.is_open("hall"));
        assert!(!door.close("hall"), "closed already");
    }
}
