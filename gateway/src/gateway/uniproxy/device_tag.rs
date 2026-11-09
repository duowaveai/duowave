//! The station's tag from its handshake.

use axum::http::HeaderMap;

use crate::gateway::observability::redact::hash_id;

/// The station's tag, as the journal, the skills' state and the stations' approval name it: its device id from the
/// handshake, hashed when RECORDER_REDACT is on; `unknown` when the handshake names none (a browser cannot set
/// this header, so a web page never passes for a station).
pub fn device_tag_of(headers: &HeaderMap, redact: bool) -> String {
    // Repeated, the header is one value joined with ", ", as Node gave it.
    let device_id = headers
        .get_all("x-uprx-device-id")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    match device_id.as_str() {
        "" => "unknown".to_string(),
        id if redact => hash_id(id),
        id => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_a_station_by_its_device_id() {
        let mut headers = HeaderMap::new();
        assert_eq!(device_tag_of(&headers, false), "unknown");
        headers.insert("x-uprx-device-id", "TESTDEVICE01".parse().unwrap());
        assert_eq!(device_tag_of(&headers, false), "TESTDEVICE01");
        assert_eq!(device_tag_of(&headers, true), hash_id("TESTDEVICE01"));
    }
}
