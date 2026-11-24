//! Headlines from the panel's news feeds (RSS or Atom, «Дом» → «Коннекторы» → «Новости»); the model retells a few.
//! A question that names a topic of the catalog («новости спорта») reads that topic's feeds; any other topic picks
//! the headlines that name it.

use std::sync::{Arc, LazyLock};

use futures::future::BoxFuture;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::http::{Http, HttpGet};
use super::news_catalog::NEWS_CATALOG;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::media::sources::match_score;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

fn entity(text: &str) -> Option<&'static str> {
    Some(match text {
        "&amp;" => "&",
        "&quot;" => "\"",
        "&#39;" => "'",
        "&apos;" => "'",
        "&lt;" => "<",
        "&gt;" => ">",
        "&nbsp;" => " ",
        _ => return None,
    })
}

static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<title[^>]*>([\s\S]*?)</title>").expect("a valid pattern"));
static CDATA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<!\[CDATA\[([\s\S]*?)\]\]>").expect("a valid pattern"));
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&[A-Za-z#0-9]+;").expect("a valid pattern"));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("a valid pattern"));

/// The items of a feed: `<item …>…</item>` or `<entry …>…</entry>`, each up to its first closing tag. (The TS
/// pattern paired the tags by a back-reference, which the regex crate has not.)
fn items(xml: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut from = 0;
    while from < xml.len() {
        let next = ["<item", "<entry"]
            .iter()
            .filter_map(|open| xml[from..].find(open).map(|at| (from + at, *open)))
            .min_by_key(|(at, _)| *at);
        let Some((start, open)) = next else { break };
        let after = start + open.len();
        // `\b` after the name, ASCII as JavaScript's: «<items>» is not an item.
        let bounded = xml[after..].chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let close = format!("</{}>", &open[1..]);
        match xml[after..].find(&close).filter(|_| bounded) {
            Some(end) => {
                let end = after + end + close.len();
                found.push(&xml[start..end]);
                from = end;
            }
            None => from = start + 1,
        }
    }
    found
}

/// A title's text: CDATA opened, entities read, tags dropped, trimmed; None when nothing is left.
fn title_text(part: &str) -> Option<String> {
    let title = TITLE.captures(part).and_then(|found| found.get(1)).map_or("", |title| title.as_str());
    let text = CDATA.replace_all(title, "${1}");
    let text = ENTITY.replace_all(&text, |found: &regex::Captures| {
        let whole = &found[0];
        entity(whole).unwrap_or(whole).to_string()
    });
    let text = TAG.replace_all(&text, "");
    let text = js::trim(&text);
    (!text.is_empty()).then(|| text.to_string())
}

/// Titles of an RSS or Atom feed.
pub fn feed_titles(xml: &str) -> Vec<String> {
    items(xml).into_iter().filter_map(title_text).collect()
}

/// The feed's own name: its title before its first item.
pub fn feed_name(xml: &str) -> Option<String> {
    let first = ["<item", "<entry"].iter().filter_map(|open| xml.find(open)).min().unwrap_or(xml.len());
    title_text(&xml[..first])
}

/// A feed as the news reads it: its address and its topic in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsFeed {
    pub url: String,
    pub topic: Option<String>,
}

/// The feeds now: the panel's, read on every question so a change applies at once.
pub type FeedSource = Arc<dyn Fn() -> Vec<NewsFeed> + Send + Sync>;

pub struct NewsSkill {
    feeds: FeedSource,
    get: Arc<dyn HttpGet>,
}

impl NewsSkill {
    /// These feeds, always: tests.
    pub fn new(feeds: Vec<String>, get: Option<Arc<dyn HttpGet>>) -> NewsSkill {
        let feeds: Vec<NewsFeed> = feeds.into_iter().map(|url| NewsFeed { url, topic: None }).collect();
        NewsSkill::live(Arc::new(move || feeds.clone()), get)
    }

    /// The feeds the source gives on each question.
    pub fn live(feeds: FeedSource, get: Option<Arc<dyn HttpGet>>) -> NewsSkill {
        NewsSkill { feeds, get: get.unwrap_or_else(|| Arc::new(Http)) }
    }

    async fn headlines(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let topic = js::string_param(&intent.params, "topic").filter(|topic| !topic.is_empty());
        let feeds = (self.feeds)();
        // A topic of the catalog with feeds of its own: their headlines, every one. Another topic: the headlines
        // of all feeds that name it.
        let named = topic.and_then(|topic| NEWS_CATALOG.topic_named(topic));
        let of_topic: Vec<&NewsFeed> = named
            .map(|named| feeds.iter().filter(|feed| feed.topic.as_deref() == Some(named.id.as_str())).collect())
            .unwrap_or_default();
        let (read, filter): (Vec<&NewsFeed>, Option<&str>) =
            if of_topic.is_empty() { (feeds.iter().collect(), topic) } else { (of_topic, None) };
        // All feeds at once (1,4–2,3 s each, live 02.11), their titles in turn: one after another, the first feed
        // alone filled the six.
        let titles = futures::future::join_all(read.iter().map(|feed| self.titles(&feed.url, &context.cancel))).await;
        let lists: Vec<Vec<String>> = titles
            .into_iter()
            .map(|titles| match filter {
                Some(topic) => titles.into_iter().filter(|title| match_score(topic, title) >= 0.5).collect(),
                None => titles,
            })
            .collect();
        let chosen: Vec<String> = interleave(lists).into_iter().take(6).collect();
        if chosen.is_empty() {
            return Ok(Outcome::speech(match topic {
                Some(topic) => format!("Не нашла новостей про {topic}."),
                None => "Не получилось загрузить новости.".into(),
            }));
        }
        let numbered: Vec<String> = chosen.iter().enumerate().map(|(i, title)| format!("{}) {title}", i + 1)).collect();
        Ok(Outcome {
            data: Some(format!("Заголовки: {}", numbered.join(" "))),
            speech: Some(chosen.iter().take(3).cloned().collect::<Vec<_>>().join(". ")),
            ..Outcome::default()
        })
    }

    async fn titles(&self, feed: &str, cancel: &CancellationToken) -> Vec<String> {
        match self.get.get(feed.to_string(), Vec::new(), cancel.clone()).await {
            Ok(answer) if (200..300).contains(&answer.status) => feed_titles(&answer.text),
            // One feed down does not silence the others.
            _ => Vec::new(),
        }
    }
}

/// The first of each list, then the second of each, and so on.
fn interleave(lists: Vec<Vec<String>>) -> Vec<String> {
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    (0..longest).flat_map(|index| lists.iter().filter_map(move |list| list.get(index).cloned())).collect()
}

impl OwnSkill for NewsSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::News
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn available(&self) -> bool {
        !(self.feeds)().is_empty()
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.headlines(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::lookups::http::HttpText;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn reads_titles_from_rss_and_atom() {
        let rss = "<rss><channel><title>Лента</title><item><title><![CDATA[Первая &amp; новость]]></title></item><item><title>Вторая</title></item></channel></rss>";
        assert_eq!(feed_titles(rss), ["Первая & новость", "Вторая"]);
        assert_eq!(feed_titles(r#"<feed><entry><title type="text">Атом</title></entry></feed>"#), ["Атом"]);
        // Entities first, then tags: an escaped tag goes too; an entity of another case stays.
        assert_eq!(feed_titles("<items><item><title>&lt;b&gt;Жирно&lt;/b&gt; &AMP;</title></item>"), ["Жирно &AMP;"]);
        assert!(feed_titles("<item><title>без конца").is_empty());
        assert_eq!(feed_name(rss).as_deref(), Some("Лента"));
        assert_eq!(feed_name("<feed><entry><title>Атом</title></entry></feed>"), None);
    }

    /// Feeds by URL; the ones not listed are down. Counts how many are asked at once.
    struct Feeds {
        titles: HashMap<String, Vec<String>>,
        waiting: Arc<AtomicUsize>,
        together: Arc<AtomicUsize>,
    }

    impl HttpGet for Feeds {
        fn get(
            &self,
            url: String,
            _: Vec<(String, String)>,
            _: CancellationToken,
        ) -> BoxFuture<'static, anyhow::Result<HttpText>> {
            let titles = self.titles.get(&url).cloned();
            let (waiting, together) = (self.waiting.clone(), self.together.clone());
            Box::pin(async move {
                let now = waiting.fetch_add(1, Ordering::SeqCst) + 1;
                together.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                waiting.fetch_sub(1, Ordering::SeqCst);
                let titles = titles.ok_or_else(|| anyhow::anyhow!("timeout"))?;
                let text =
                    titles.iter().map(|title| format!("<item><title>{title}</title></item>")).collect::<String>();
                Ok(HttpText { status: 200, text })
            })
        }
    }

    fn feeds(entries: &[(&str, &[&str])]) -> Feeds {
        Feeds {
            titles: entries
                .iter()
                .map(|(url, titles)| (url.to_string(), titles.iter().map(|t| t.to_string()).collect()))
                .collect(),
            waiting: Arc::default(),
            together: Arc::default(),
        }
    }

    fn headlines() -> Intent {
        Intent::new(CapabilityName::News, "headlines")
    }

    #[tokio::test]
    async fn is_not_set_up_without_feeds_and_gives_the_model_the_headlines() {
        assert!(!NewsSkill::new(Vec::new(), None).available());
        let get = Arc::new(feeds(&[("https://news.example/rss", &["Новость дня"])]));
        let skill = NewsSkill::new(vec!["https://news.example/rss".into()], Some(get));
        let outcome = skill.execute(&headlines(), &ExecutionContext::new("h:test", Utc::now())).await.unwrap();
        assert_eq!(outcome.data.as_deref(), Some("Заголовки: 1) Новость дня"));
        assert_eq!(outcome.speech.as_deref(), Some("Новость дня"));
    }

    #[tokio::test]
    async fn asks_all_feeds_at_once_and_takes_their_titles_in_turn_a_feed_down_does_not_matter() {
        let get = Arc::new(feeds(&[
            ("https://a.example/rss", &["А1", "А2", "А3", "А4", "А5"]),
            ("https://b.example/rss", &["Б1", "Б2"]),
        ]));
        let together = get.together.clone();
        let urls = ["https://a.example/rss", "https://b.example/rss", "https://down.example/rss"];
        let skill = NewsSkill::new(urls.iter().map(|url| url.to_string()).collect(), Some(get));
        let outcome = skill.execute(&headlines(), &ExecutionContext::new("h:test", Utc::now())).await.unwrap();
        assert_eq!(together.load(Ordering::SeqCst), 3);
        assert_eq!(outcome.data.as_deref(), Some("Заголовки: 1) А1 2) Б1 3) А2 4) Б2 5) А3 6) А4"));
    }

    #[tokio::test]
    async fn keeps_the_headlines_of_the_topic_asked() {
        let get = Arc::new(feeds(&[("https://a.example/rss", &["Футбол: финал кубка", "Погода на выходные"])]));
        let skill = NewsSkill::new(vec!["https://a.example/rss".into()], Some(get));
        let mut intent = headlines();
        intent.params.insert("topic".into(), "футбол".into());
        let context = ExecutionContext::new("h:test", Utc::now());
        assert_eq!(
            skill.execute(&intent, &context).await.unwrap().data.as_deref(),
            Some("Заголовки: 1) Футбол: финал кубка")
        );
        intent.params.insert("topic".into(), "хоккей".into());
        assert_eq!(
            skill.execute(&intent, &context).await.unwrap().speech.as_deref(),
            Some("Не нашла новостей про хоккей.")
        );
    }

    #[tokio::test]
    async fn a_topic_of_the_catalog_reads_its_own_feeds_and_the_feeds_change_with_the_panel() {
        let get = Arc::new(feeds(&[
            ("https://main.example/rss", &["Главное дня", "Матч сборной"]),
            ("https://sport.example/rss", &["Итоги тура", "Трансферы"]),
        ]));
        let panel = Arc::new(std::sync::Mutex::new(vec![
            NewsFeed { url: "https://main.example/rss".into(), topic: Some("main".into()) },
            NewsFeed { url: "https://sport.example/rss".into(), topic: Some("sport".into()) },
        ]));
        let source = panel.clone();
        let skill = NewsSkill::live(Arc::new(move || source.lock().unwrap().clone()), Some(get));
        let context = ExecutionContext::new("h:test", Utc::now());
        let mut intent = headlines();
        intent.params.insert("topic".into(), "спорта".into());
        assert_eq!(
            skill.execute(&intent, &context).await.unwrap().data.as_deref(),
            Some("Заголовки: 1) Итоги тура 2) Трансферы"),
            "every headline of the sport feed, none of the others"
        );
        // Without a feed of the topic, the topic picks headlines by its words, as before.
        panel.lock().unwrap().retain(|feed| feed.topic.as_deref() == Some("main"));
        intent.params.insert("topic".into(), "матч".into());
        assert_eq!(skill.execute(&intent, &context).await.unwrap().data.as_deref(), Some("Заголовки: 1) Матч сборной"));
        panel.lock().unwrap().clear();
        assert!(!skill.available(), "the last feed gone: news is not ours");
    }
}
