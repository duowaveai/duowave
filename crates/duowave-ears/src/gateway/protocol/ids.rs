//! Ids of the protocol: message ids in the station's spelling, session ids.

use crate::utils::ids::{random_hex, uuid};

/// Message id in the spelling the station uses: four groups of eight hex digits.
pub fn new_message_id() -> String {
    let hex = random_hex(16);
    format!("{}-{}-{}-{}", &hex[0..8], &hex[8..16], &hex[16..24], &hex[24..32])
}

pub fn new_session_id() -> String {
    uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spells_message_ids_as_the_station_does() {
        let id = new_message_id();
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|group| group.len() == 8 && group.chars().all(|c| c.is_ascii_hexdigit())));
        assert_ne!(new_message_id(), id);
        assert_eq!(new_session_id().len(), 36);
    }
}
