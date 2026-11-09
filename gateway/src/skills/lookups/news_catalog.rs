//! The news catalog: Russian-language feeds by topic, each one checked from home before it went in. Compiled from
//! open lists — awesome-rss-feeds and Wikidata's «web feed URL», both CC0 — and the feeds the outlets publish
//! themselves: names and addresses only (THIRD_PARTY.md). The panel offers them by topic; a question that names a
//! topic («новости спорта») takes its headlines from the feeds of that topic.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::skills::media::sources::match_score;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsTopic {
    pub id: String,
    pub title: String,
    /// Words a question names the topic by.
    pub words: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogFeed {
    pub id: String,
    pub title: String,
    pub site: String,
    pub url: String,
    pub topic: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsCatalog {
    pub topics: Vec<NewsTopic>,
    pub feeds: Vec<CatalogFeed>,
}

/// The catalog built into the gateway (the station's build too).
pub static NEWS_CATALOG: LazyLock<NewsCatalog> =
    LazyLock::new(|| serde_json::from_str(include_str!("news_catalog.json")).expect("the news catalog parses"));

/// A feed's address as two spellings of it compare: no scheme, no trailing slash, the host in lower case.
pub fn same_feed(a: &str, b: &str) -> bool {
    fn plain(url: &str) -> String {
        let url = url.trim();
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let rest = rest.trim_end_matches('/');
        match rest.split_once('/') {
            Some((host, path)) => format!("{}/{path}", host.to_ascii_lowercase()),
            None => rest.to_ascii_lowercase(),
        }
    }
    plain(a) == plain(b)
}

impl NewsCatalog {
    pub fn feed(&self, id: &str) -> Option<&CatalogFeed> {
        self.feeds.iter().find(|feed| feed.id == id)
    }

    pub fn feed_by_url(&self, url: &str) -> Option<&CatalogFeed> {
        self.feeds.iter().find(|feed| same_feed(&feed.url, url))
    }

    pub fn topic(&self, id: &str) -> Option<&NewsTopic> {
        self.topics.iter().find(|topic| topic.id == id)
    }

    /// The topic a question names («спорта», «про технологии»): by its title and its words, as word forms match.
    pub fn topic_named(&self, asked: &str) -> Option<&NewsTopic> {
        self.topics
            .iter()
            .map(|topic| {
                let names = format!("{} {}", topic.title, topic.words.join(" "));
                (match_score(asked, &names), topic)
            })
            .filter(|(score, _)| *score >= 0.5)
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, topic)| topic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_feed_is_once_under_a_known_topic() {
        let catalog = &*NEWS_CATALOG;
        assert!(catalog.feeds.len() >= 25);
        let topics: HashSet<&str> = catalog.topics.iter().map(|topic| topic.id.as_str()).collect();
        let mut ids = HashSet::new();
        for feed in &catalog.feeds {
            assert!(topics.contains(feed.topic.as_str()), "{}: topic {}", feed.id, feed.topic);
            assert!(ids.insert(feed.id.as_str()), "{} twice", feed.id);
            assert!(feed.url.starts_with("http://") || feed.url.starts_with("https://"), "{}", feed.url);
            let others = catalog.feeds.iter().filter(|other| same_feed(&other.url, &feed.url)).count();
            assert_eq!(others, 1, "{} twice", feed.url);
        }
        for topic in &catalog.topics {
            assert!(catalog.feeds.iter().any(|feed| feed.topic == topic.id), "{} has no feeds", topic.id);
        }
    }

    #[test]
    fn finds_a_feed_by_any_spelling_of_its_address() {
        let catalog = &*NEWS_CATALOG;
        assert_eq!(catalog.feed_by_url("https://TASS.ru/rss/v2.xml/").map(|feed| feed.id.as_str()), Some("tass"));
        assert_eq!(catalog.feed_by_url("http://tass.ru/rss/v2.xml").map(|feed| feed.id.as_str()), Some("tass"));
        assert!(catalog.feed_by_url("https://example.com/rss").is_none());
    }

    #[test]
    fn hears_a_topic_in_its_word_forms() {
        let catalog = &*NEWS_CATALOG;
        let topic = |asked: &str| catalog.topic_named(asked).map(|topic| topic.id.as_str());
        assert_eq!(topic("спорта"), Some("sport"));
        assert_eq!(topic("про футбол"), Some("sport"));
        assert_eq!(topic("технологий"), Some("tech"));
        assert_eq!(topic("науки"), Some("science"));
        assert_eq!(topic("экономики"), Some("economy"));
        assert_eq!(topic("про погоду в саду"), None);
    }
}
