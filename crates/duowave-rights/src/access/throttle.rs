//! Wrong passwords per source: after `limit` failures within `window_ms` the source waits out `lock_ms` from its last
//! failure; a success clears it. Memory stays small: stale sources are dropped as new ones arrive.
//!
//! A try is counted when it starts (`attempt`), before the password is checked: tries in parallel cannot all pass the
//! limit together, and a client that goes away mid-check has still used its try.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    first: u64,
    last: u64,
}

pub struct LoginThrottle {
    failures: Mutex<HashMap<String, Failures>>,
    limit: u32,
    window_ms: u64,
    lock_ms: u64,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        LoginThrottle::new(5, 15 * 60_000, 15 * 60_000)
    }
}

impl LoginThrottle {
    pub fn new(limit: u32, window_ms: u64, lock_ms: u64) -> LoginThrottle {
        LoginThrottle { failures: Mutex::new(HashMap::new()), limit, window_ms, lock_ms }
    }

    /// Milliseconds the source still has to wait; 0 when it may try.
    pub fn wait_ms(&self, source: &str, now: u64) -> u64 {
        let mut failures = self.lock();
        self.wait_in(&mut failures, source, now)
    }

    /// One more try from the source, counted as a failure until `succeed` clears it: 0 when it may go on, or the
    /// milliseconds to wait (the try is then not counted). The check and the count are one step.
    pub fn attempt(&self, source: &str, now: u64) -> u64 {
        let mut failures = self.lock();
        let wait = self.wait_in(&mut failures, source, now);
        if wait == 0 {
            self.fail_in(&mut failures, source, now);
        }
        wait
    }

    fn wait_in(&self, failures: &mut HashMap<String, Failures>, source: &str, now: u64) -> u64 {
        let Some(entry) = failures.get(source).copied() else { return 0 };
        if entry.count < self.limit {
            if now.saturating_sub(entry.first) > self.window_ms {
                failures.remove(source);
            }
            return 0;
        }
        let until = entry.last + self.lock_ms;
        if until <= now {
            failures.remove(source);
            return 0;
        }
        until - now
    }

    pub fn fail(&self, source: &str, now: u64) {
        let mut failures = self.lock();
        self.fail_in(&mut failures, source, now);
    }

    fn fail_in(&self, failures: &mut HashMap<String, Failures>, source: &str, now: u64) {
        let next = match failures.get(source) {
            Some(entry) if now.saturating_sub(entry.first) <= self.window_ms => {
                Failures { count: entry.count + 1, first: entry.first, last: now }
            }
            _ => Failures { count: 1, first: now, last: now },
        };
        failures.insert(source.to_string(), next);
        if failures.len() > 1000 {
            let stale = self.window_ms.max(self.lock_ms);
            failures.retain(|_, entry| now.saturating_sub(entry.last) <= stale);
        }
    }

    pub fn succeed(&self, source: &str) {
        self.lock().remove(source);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Failures>> {
        self.failures.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.failures.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: u64 = 60_000;

    #[test]
    fn locks_a_source_after_the_limit_until_the_lock_is_out() {
        let throttle = LoginThrottle::default();
        for attempt in 0..4 {
            throttle.fail("10.0.0.5", attempt * 1000);
            assert_eq!(throttle.wait_ms("10.0.0.5", attempt * 1000), 0);
        }
        throttle.fail("10.0.0.5", 4000);
        assert_eq!(throttle.wait_ms("10.0.0.5", 4000), 15 * MINUTE);
        assert_eq!(throttle.wait_ms("10.0.0.5", 4000 + 5 * MINUTE), 10 * MINUTE);
        assert_eq!(throttle.wait_ms("other", 4000), 0, "per source");
        assert_eq!(throttle.wait_ms("10.0.0.5", 4000 + 15 * MINUTE), 0);
        throttle.fail("10.0.0.5", 4000 + 15 * MINUTE);
        assert_eq!(throttle.wait_ms("10.0.0.5", 4000 + 15 * MINUTE), 0, "a fresh count after the lock");
    }

    #[test]
    fn a_success_clears_and_old_failures_fall_out_of_the_window() {
        let throttle = LoginThrottle::default();
        for _ in 0..4 {
            throttle.fail("a", 0);
        }
        throttle.succeed("a");
        throttle.fail("a", 1);
        assert_eq!(throttle.wait_ms("a", 1), 0);
        for _ in 0..4 {
            throttle.fail("b", 0);
        }
        // The window has passed: the fifth failure starts a new count instead of locking.
        throttle.fail("b", 16 * MINUTE);
        assert_eq!(throttle.wait_ms("b", 16 * MINUTE), 0);
    }

    #[test]
    fn counts_a_try_when_it_starts_so_tries_in_parallel_cannot_pass_the_limit_together() {
        let throttle = LoginThrottle::default();
        // Ten tries at once, none of them answered yet: five go on, the rest wait.
        let waits: Vec<u64> = (0..10).map(|_| throttle.attempt("10.0.0.5", 1000)).collect();
        assert_eq!(waits.iter().filter(|wait| **wait == 0).count(), 5);
        assert_eq!(waits[5..], [15 * MINUTE; 5]);
        assert_eq!(throttle.wait_ms("10.0.0.5", 1000), 15 * MINUTE, "a refused try does not move the lock");
        // A success clears the count it started.
        let quiet = LoginThrottle::default();
        assert_eq!(quiet.attempt("a", 0), 0);
        quiet.succeed("a");
        assert_eq!(quiet.len(), 0);
    }

    #[test]
    fn drops_stale_sources_when_many_arrive() {
        let throttle = LoginThrottle::default();
        for n in 0..1000 {
            throttle.fail(&format!("old-{n}"), 0);
        }
        throttle.fail("new", 31 * MINUTE);
        assert_eq!(throttle.len(), 1);
    }
}
