//! Publishers: where a file of the household can go without leaving DuoWave. A registry, not branches of one function:
//! a publisher is an entry — who, what it takes, whether it needs connecting first, where it works — and, when it sends
//! by itself, an action. The gateway's own come first (Telegram by the gateway's bot, the page «смотри и выбирай»),
//! then the platforms' upload pages a person opens by hand (kind `manual`): the panel's «Поделиться» shows them all
//! from this list (`GET /api/v1/publishers`), an app reads it by `system/publish` → `publishers` and sends by
//! `system/publish` → `<id>`. A new publisher is a module implementing [`Publisher`], registered in app.rs.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};

/// Sends by itself (`direct`) or is a page a person opens and uploads to by hand (`manual`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Direct,
    Manual,
}

/// A publisher as the panel and the apps see it.
#[derive(Debug, Clone, Serialize)]
pub struct PublisherCard {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    /// What it takes: `video`, `image`, `text`.
    pub accepts: Vec<&'static str>,
    /// Can send now. One that is not says what to connect first (`needs`).
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    /// Where it works: `any` (station or computer) or `pc`.
    pub runs: &'static str,
    /// Whose files it takes: `any`, or `apps` — only an app's own (the watch page plays the app's folder).
    pub from: &'static str,
    /// The page a manual one opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A line for people: what happens.
    pub about: String,
}

/// The sender of a person's own «Поделиться» in the panel (not an app).
pub const PANEL: &str = "panel";

/// What a direct publisher gets: who sends (an app's id, or [`PANEL`] for a person in the panel), the file on disk and
/// its address as the sender named it, and the letter's other fields (caption, where to).
pub struct Request {
    pub sender: String,
    pub file: PathBuf,
    pub path: String,
    pub input: Value,
}

/// What a direct publisher answers: for the sender, and a line for the sender's journal («отправлено: …»).
pub struct Published {
    pub answer: Value,
    pub line: String,
}

pub trait Publisher: Send + Sync {
    fn card(&self) -> PublisherCard;
    /// Sends; a manual publisher refuses in words — its page is opened by a person.
    fn publish(&self, request: Request) -> BoxFuture<'static, Result<Published, String>>;
}

/// The registry: the gateway's publishers in the order the panel shows them.
#[derive(Default)]
pub struct Publishers {
    list: RwLock<Vec<Arc<dyn Publisher>>>,
}

impl Publishers {
    pub fn new() -> Publishers {
        Publishers::default()
    }

    /// Adds one; one with the same id replaces the earlier.
    pub fn register(&self, publisher: Arc<dyn Publisher>) {
        let id = publisher.card().id;
        let mut list = self.list.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        list.retain(|one| one.card().id != id);
        list.push(publisher);
    }

    pub fn cards(&self) -> Vec<PublisherCard> {
        self.list.read().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().map(|one| one.card()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Publisher>> {
        self.list
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|one| one.card().id == id)
            .cloned()
    }

    /// The platforms' upload pages: a person uploads the file there by hand.
    pub fn register_upload_pages(&self) {
        for (id, url) in UPLOAD_PAGES {
            self.register(Arc::new(UploadPage { id, url }));
        }
    }

    /// Sends by the publisher `id`: the file must be of a kind it takes. The answer and the journal's line.
    pub fn send(&self, id: &str, request: Request) -> Result<BoxFuture<'static, Result<Published, String>>, String> {
        let publisher = self.get(id).ok_or_else(|| {
            let ids: Vec<String> =
                self.cards().into_iter().filter(|card| card.kind == Kind::Direct).map(|card| card.id).collect();
            crate::tr!("отправителя «{id}» нет; есть: {ids}", id = id, ids = ids.join(", "))
        })?;
        let card = publisher.card();
        if card.kind == Kind::Manual {
            return Err(crate::tr!(
                "«{title}» — страница загрузки: файл туда отправляет человек в «Поделиться»",
                title = card.title
            ));
        }
        let kind = kind_of(&request.path);
        if !kind.is_some_and(|kind| card.accepts.contains(&kind)) {
            return Err(crate::tr!(
                "«{title}» берёт {kinds}: {path} не подходит",
                title = card.title,
                kinds = card.accepts.join(", "),
                path = request.path
            ));
        }
        if !card.ready {
            return Err(card.needs.unwrap_or_else(|| crate::tr!("«{title}» не подключён", title = card.title)));
        }
        Ok(publisher.publish(request))
    }
}

/// What kind a file is, by its extension: `video`, `image`, `text`.
pub fn kind_of(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase())?;
    match extension.as_str() {
        "mp4" | "mov" | "webm" | "m4v" => Some("video"),
        "jpg" | "jpeg" | "png" | "webp" | "gif" => Some("image"),
        "txt" | "md" => Some("text"),
        _ => None,
    }
}

/// The platforms' pages where a person uploads a clip by hand; the names read in the reader's language (`title_of`).
const UPLOAD_PAGES: [(&str, &str); 5] = [
    ("tiktok", "https://www.tiktok.com/upload"),
    ("reels", "https://www.instagram.com/"),
    ("shorts", "https://www.youtube.com/upload"),
    ("telegram-web", "https://web.telegram.org/"),
    ("vk", "https://vk.com/clips"),
];

/// A platform's name for people.
fn title_of(id: &str) -> String {
    match id {
        "tiktok" => crate::tr!("ТикТок"),
        "reels" => "Reels".into(),
        "shorts" => "Shorts".into(),
        "telegram-web" => "Telegram".into(),
        "vk" => crate::tr!("ВКонтакте"),
        other => other.into(),
    }
}

/// A platform's upload page: the panel opens it, the person uploads the file there.
struct UploadPage {
    id: &'static str,
    url: &'static str,
}

impl Publisher for UploadPage {
    fn card(&self) -> PublisherCard {
        PublisherCard {
            id: self.id.into(),
            title: title_of(self.id),
            kind: Kind::Manual,
            accepts: vec!["video", "image"],
            ready: true,
            needs: None,
            runs: "any",
            from: "any",
            url: Some(self.url.into()),
            about: crate::tr!("страница загрузки {title}: файл загружаете вы", title = title_of(self.id)),
        }
    }

    fn publish(&self, _request: Request) -> BoxFuture<'static, Result<Published, String>> {
        let refused =
            crate::tr!("«{title}» — страница загрузки: файл туда отправляет человек", title = title_of(self.id));
        Box::pin(async move { Err(refused) })
    }
}

/// «Смотри и выбирай»: a page on this gateway with the clip and its fork; votes come back to the sender (`vote`).
pub struct Watch {
    pub pages: Arc<crate::plugins::watch::WatchPages>,
}

impl Publisher for Watch {
    fn card(&self) -> PublisherCard {
        PublisherCard {
            id: "watch".into(),
            title: crate::tr!("Ссылка «смотри и выбирай»"),
            kind: Kind::Direct,
            accepts: vec!["video"],
            ready: true,
            needs: None,
            runs: "any",
            from: "apps",
            url: None,
            about: crate::tr!("страница на этом шлюзе: ролик и развилка, голоса ①/② приходят обратно, 30 дней"),
        }
    }

    fn publish(&self, request: Request) -> BoxFuture<'static, Result<Published, String>> {
        let pages = self.pages.clone();
        Box::pin(async move {
            if request.sender == PANEL {
                return Err(crate::tr!("страница «смотри и выбирай» — для роликов приложений, из их окна"));
            }
            let field = |key: &str| {
                request
                    .input
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            let token = pages.create(crate::plugins::watch::WatchPage {
                app: request.sender.clone(),
                file: request.path.clone(),
                title: field("title").unwrap_or_else(|| crate::tr!("Серия")).chars().take(120).collect(),
                id: field("id"),
                q: field("q"),
                a: field("a"),
                b: field("b"),
                votes: [0, 0],
                created_ms: chrono::Utc::now().timestamp_millis(),
            })?;
            Ok(Published {
                answer: json!({ "path": format!("/watch/{token}"), "days": 30 }),
                line: crate::tr!("опубликовано: {path} → страница «смотри и выбирай»", path = request.path),
            })
        })
    }
}

/// Telegram by the gateway's bot: to a channel (@name, the bot is its admin), a contact of the bot or «Оповещения».
#[cfg(feature = "pc")]
pub struct Telegram {
    pub bot: Arc<crate::calls::telegram::TelegramMessenger>,
}

#[cfg(feature = "pc")]
impl Publisher for Telegram {
    fn card(&self) -> PublisherCard {
        let ready = self.bot.enabled();
        PublisherCard {
            id: "telegram".into(),
            title: crate::tr!("В Telegram ботом шлюза"),
            kind: Kind::Direct,
            accepts: vec!["video"],
            ready,
            needs: (!ready)
                .then(|| crate::tr!("бота Telegram нет: подключите его в пульте («Приложения» → «Подключения»)")),
            runs: "pc",
            from: "any",
            url: None,
            about: crate::tr!("ролик до 50 МБ — в канал (@имя, бот — его админ), контакту бота или в «Оповещения»"),
        }
    }

    fn publish(&self, request: Request) -> BoxFuture<'static, Result<Published, String>> {
        let bot = self.bot.clone();
        Box::pin(async move {
            let field =
                |key: &str| request.input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
            let to = field("to");
            let chat = if to.is_empty() {
                bot.alert_chat()
                    .ok_or_else(|| crate::tr!("куда: укажите канал (@имя) или выберите чат «Оповещения» у бота"))?
            } else if to.starts_with('@') || to.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) {
                to.clone()
            } else {
                bot.contact(&to)
                    .map(|contact| contact.chat_id)
                    .ok_or_else(|| crate::tr!("у бота нет контакта «{to}»", to = to))?
            };
            let bytes = std::fs::read(&request.file).map_err(|error| error.to_string())?;
            let name = request
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "video.mp4".into());
            let caption = field("caption");
            bot.send_video(&chat, bytes, &name, Some(caption.as_str()))
                .await
                .map_err(|error| format!("Telegram: {error}"))?;
            Ok(Published {
                answer: json!({ "sent": true, "to": if to.is_empty() { "alerts".to_string() } else { to.clone() } }),
                line: crate::tr!(
                    "опубликовано: {path} → Telegram {to}",
                    path = request.path,
                    to = if to.is_empty() { crate::tr!("«Оповещения»") } else { to.clone() }
                ),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Publisher for Echo {
        fn card(&self) -> PublisherCard {
            PublisherCard {
                id: "echo".into(),
                title: "Эхо".into(),
                kind: Kind::Direct,
                accepts: vec!["video"],
                ready: true,
                needs: None,
                runs: "any",
                from: "any",
                url: None,
                about: "проба".into(),
            }
        }

        fn publish(&self, request: Request) -> BoxFuture<'static, Result<Published, String>> {
            Box::pin(async move { Ok(Published { answer: json!({ "path": request.path }), line: "эхо".into() }) })
        }
    }

    fn request(path: &str) -> Request {
        Request { sender: "feed".into(), file: PathBuf::from(path), path: path.into(), input: json!({}) }
    }

    #[tokio::test]
    async fn a_list_of_publishers_with_the_platforms_pages_and_a_new_one_by_registering() {
        let publishers = Publishers::new();
        publishers.register_upload_pages();
        publishers.register(Arc::new(Echo));
        let cards = publishers.cards();
        assert_eq!(cards.iter().filter(|card| card.kind == Kind::Manual).count(), 5, "five upload pages");
        assert!(cards.iter().all(|card| card.kind == Kind::Direct || card.url.is_some()), "a manual one has its page");
        let sent = publishers.send("echo", request("share/kompas-01.mp4")).unwrap().await.unwrap();
        assert_eq!(sent.answer["path"], "share/kompas-01.mp4");
        assert!(publishers.send("tiktok", request("a.mp4")).err().unwrap().contains("страница загрузки"));
        assert!(publishers.send("echo", request("a.png")).err().unwrap().contains("берёт video"));
        assert!(publishers.send("fax", request("a.mp4")).err().unwrap().contains("есть: echo"));
        // The same id again replaces the earlier one.
        publishers.register(Arc::new(Echo));
        assert_eq!(publishers.cards().iter().filter(|card| card.id == "echo").count(), 1);
        assert_eq!(kind_of("Видео/лето.MOV"), Some("video"));
        assert_eq!(kind_of("карточка.png"), Some("image"));
        assert_eq!(kind_of("без точки"), None);
    }
}
