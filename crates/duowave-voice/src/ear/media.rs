//! What an ear's player plays: an item with a link the ear fetches itself (a radio stream, a track, a tale) — the
//! skills and the apps choose it, the ear's adapter plays it (the Station: `audio_play`, src/gateway/commands.rs).

use serde::{Deserialize, Serialize};

/// Something an ear's player plays from a link: a radio stream, a track, a tale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: String,
    pub title: String,
    /// An absolute URL the station fetches itself: a station's own stream, or our /media.
    pub url: String,
    /// As audio_play names it: HLS for a playlist, MP3 for a file of any codec, as Yandex sends its AAC tracks
    /// (journal of 01.11); with OGG the station played nothing (live, 02.11).
    pub format: String,
    /// As Yandex names it next to the format: AacMp4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// FmRadio, Track, FairyTale, AudioBook.
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
