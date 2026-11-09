//! What our media skills play and where it comes from.

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// Something the station's player plays from a link: a radio stream, a track, a tale.
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

/// What a source holds: what the radio, the music and the tales skills play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Radio,
    Music,
    Tales,
}

impl MediaKind {
    pub fn parse(text: &str) -> Option<MediaKind> {
        match text {
            "radio" => Some(MediaKind::Radio),
            "music" => Some(MediaKind::Music),
            "tales" => Some(MediaKind::Tales),
            _ => None,
        }
    }

    /// The type audio_play gives an item of this kind, unless the item says.
    pub fn item_type(self) -> &'static str {
        match self {
            MediaKind::Radio => "FmRadio",
            MediaKind::Music => "Track",
            MediaKind::Tales => "FairyTale",
        }
    }
}

/// A source of media for our player: our radio list, a folder of music or tales, a plugin's catalog. The media
/// skills ask every source of their kind (`MediaSources`), ours first, and play what it gives through our player —
/// so «пауза», «дальше», «громче» are the same for every source.
pub trait MediaSource: Send + Sync {
    /// Stable id: `content:radio`, `content:music`, `plugin:radio/stations`.
    fn id(&self) -> String;
    fn kind(&self) -> MediaKind;
    /// Items for a query, the best first; without one, what it would play (a folder shuffled). `media_base`: where
    /// the station fetches our /media; files of ours need it for their links.
    fn search(&self, query: Option<String>, media_base: Option<String>) -> BoxFuture<'_, Vec<MediaItem>>;
    /// Whether it has anything to play now; cheap, without a lookup.
    fn available(&self) -> bool;
    /// Only a fallback: asked when no other source of its kind has anything to play.
    fn fallback(&self) -> bool {
        false
    }
}
