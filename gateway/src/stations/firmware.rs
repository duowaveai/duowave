//! The firmwares the gateway was tried with, by model: a station on another one works as it can — its protocol may
//! have moved — and the panel says so. A station's firmware changing under the gateway is an update from Yandex (the
//! wiki's root turns them off, but not every station has that): its copy of quasar.cfg is then left unmounted at boot
//! (station-client/voice.sh) and the station is the usual Alice until it is connected again.

/// (model as the firmware names it, firmware), tried by hand on a real station.
pub const TRIED: &[(&str, &str)] = &[
    // Yandex Station Lite 2, November 2026.
    ("plum", "0.369.1.50.4794654278.20261021.133"),
];

/// Whether the gateway was tried with this firmware: None when nothing is known of the model's.
pub fn tried(model: Option<&str>, firmware: Option<&str>) -> Option<bool> {
    let (model, firmware) = (model?, firmware?);
    let known: Vec<&str> = TRIED.iter().filter(|(of, _)| *of == model).map(|(_, firmware)| *firmware).collect();
    (!known.is_empty()).then(|| known.contains(&firmware))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_tried_only_for_a_model_it_knows() {
        assert_eq!(tried(Some("plum"), Some("0.369.1.50.4794654278.20261021.133")), Some(true));
        assert_eq!(tried(Some("plum"), Some("0.370.1.1.1.20261201.1")), Some(false));
        assert_eq!(tried(Some("yandexmidi"), Some("0.373.1.13.4819000379.20261030.299")), None);
        assert_eq!(tried(None, Some("1")), None);
        assert_eq!(tried(Some("plum"), None), None);
    }
}
