//! What our media skills play and where it comes from.

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// Something an ear's player plays from a link: the ear contract's (src/ear/media.rs).
pub use crate::ear::media::MediaItem;

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
