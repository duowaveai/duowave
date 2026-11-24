//! Likes and dislikes of our music (STATE_DIR, kind `favorites`): «включи любимое» plays the liked, a shuffle leaves
//! the disliked out. Yandex's own music is liked in the account, by Yandex.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::media_types::MediaItem;
use crate::storage::StateStore;
use crate::utils::time::iso;

/// A track of ours someone liked or disliked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rating {
    /// The item's id: its path under CONTENT_DIR.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub liked: bool,
    pub at: String,
}

/// One list for the home, whichever station heard «мне нравится».
pub const HOME: &str = "home";

pub struct Favorites {
    store: StateStore<Rating>,
}

impl Favorites {
    pub fn new(store: StateStore<Rating>) -> Favorites {
        Favorites { store }
    }

    /// Whether it changed anything: a second like of the same track does not.
    pub fn rate(&self, item: &MediaItem, liked: bool, now: DateTime<Utc>) -> bool {
        let ratings = self.store.get(HOME);
        if ratings.iter().find(|rating| rating.id == item.id).is_some_and(|rating| rating.liked == liked) {
            return false;
        }
        let rating = Rating {
            id: item.id.clone(),
            title: item.title.clone(),
            subtitle: item.subtitle.clone().filter(|subtitle| !subtitle.is_empty()),
            liked,
            at: iso(now),
        };
        let mut kept: Vec<Rating> = ratings.into_iter().filter(|other| other.id != item.id).collect();
        kept.push(rating);
        self.store.set(HOME, kept);
        true
    }

    pub fn liked(&self) -> HashSet<String> {
        self.ids(true)
    }

    pub fn disliked(&self) -> HashSet<String> {
        self.ids(false)
    }

    /// Every rating, oldest first.
    pub fn ratings(&self) -> Vec<Rating> {
        self.store.get(HOME)
    }

    fn ids(&self, liked: bool) -> HashSet<String> {
        self.store.get(HOME).into_iter().filter(|rating| rating.liked == liked).map(|rating| rating.id).collect()
    }
}
