//! Relative timestamps of one request, so latency of every stage is visible in a single log line.

use std::time::Instant;

use serde::ser::{Serialize, SerializeMap, Serializer};

#[derive(Debug, Clone)]
pub struct Timeline {
    origin: Instant,
    /// Stage and milliseconds since the origin, in the order they came.
    marks: Vec<(String, f64)>,
}

impl Default for Timeline {
    fn default() -> Timeline {
        Timeline::new()
    }
}

/// JavaScript's Math.round: halves up.
fn round(ms: f64) -> u64 {
    (ms + 0.5).floor() as u64
}

impl Timeline {
    pub fn new() -> Timeline {
        Timeline { origin: Instant::now(), marks: Vec::new() }
    }

    /// Records the first occurrence of a stage; later calls with the same name are ignored.
    pub fn mark(&mut self, name: &str) {
        if !self.has(name) {
            self.marks.push((name.to_string(), self.origin.elapsed().as_secs_f64() * 1000.0));
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.marks.iter().any(|(mark, _)| mark == name)
    }

    pub fn elapsed_ms(&self) -> u64 {
        round(self.origin.elapsed().as_secs_f64() * 1000.0)
    }

    /// `stage=+12ms other=+340ms`, in the order the stages came.
    pub fn summary(&self) -> String {
        self.marks.iter().map(|(name, at)| format!("{name}=+{}ms", round(*at))).collect::<Vec<_>>().join(" ")
    }
}

/// `{stage: ms}` in the order the stages came (the TS toJSON).
impl Serialize for Timeline {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.marks.len()))?;
        for (name, at) in &self.marks {
            map.serialize_entry(name, &round(*at))?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_first_mark_of_a_stage_in_order() {
        let mut timeline = Timeline::new();
        timeline.mark("heard");
        timeline.mark("answered");
        timeline.mark("heard");
        assert!(timeline.has("answered") && !timeline.has("spoken"));
        let json = serde_json::to_string(&timeline).unwrap();
        assert!(json.starts_with(r#"{"heard":"#) && json.contains(r#","answered":"#), "{json}");
        let summary = timeline.summary();
        assert!(summary.starts_with("heard=+") && summary.contains("ms answered=+") && summary.ends_with("ms"));
    }
}
