//! The computer's screen (docs/plans/shared-folders.md, «Экран»): windows of temporary content an app or the brain
//! opens — a photo, a video, a sound of the home folder now, anything an app gives later. The gateway keeps the list;
//! the DuoWave app opens a window of its own for each one, over all the others, and closes the ones the list lost.
//!
//! A window is temporary: the windows of the next request take the place of the temporary ones, and a video or a sound
//! that played to the end closes. «Оставь» keeps a window until it is closed by its name, its number or its cross.
//! Several open at once («открой эти две фотки и видео»). What a window shows is a link the gateway made itself
//! (Files' `media`, `/media/folder/<token>`): the screen shows nothing else, no page of the internet.

pub mod device;
pub mod routes;
pub mod tools;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::core::events::{EventBus, GatewayEvent};
use crate::utils::time::now_iso;

/// No more windows at once: past it the oldest temporary one goes.
pub const MOST: usize = 8;

/// What a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
    Audio,
}

impl Kind {
    pub fn parse(word: &str) -> Option<Kind> {
        match word {
            "image" => Some(Kind::Image),
            "video" => Some(Kind::Video),
            "audio" => Some(Kind::Audio),
            _ => None,
        }
    }

    /// A video or a sound: it plays, pauses, ends.
    pub fn plays(self) -> bool {
        matches!(self, Kind::Video | Kind::Audio)
    }

    /// The kind as a person names it: «закрой видео», «оставь фотку».
    fn named(word: &str) -> Option<Kind> {
        match word {
            "фото"
            | "фотка"
            | "фотку"
            | "фотография"
            | "фотографию"
            | "картинка"
            | "картинку"
            | "снимок"
            | "изображение"
            | "image" => Some(Kind::Image),
            "видео" | "ролик" | "видеоролик" | "фильм" | "video" => Some(Kind::Video),
            "звук" | "музыка" | "музыку" | "песня" | "песню" | "аудио" | "запись" | "audio" => {
                Some(Kind::Audio)
            }
            _ => None,
        }
    }
}

/// What to show: its kind, a title for people, the gateway's own link to it (a token of Files' `media`), where it
/// came from (`files/home/…`).
#[derive(Debug, Clone, PartialEq)]
pub struct Content {
    pub kind: Kind,
    pub title: String,
    pub file: String,
    pub from: Option<String>,
}

/// A window on the screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// Its number, «1», «2»…: the agent closes and keeps by it.
    pub id: String,
    pub title: String,
    pub kind: Kind,
    /// The gateway's own link: `/media/folder/<token>`.
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// «Оставь»: it stays when new windows open and when it played to the end.
    pub kept: bool,
    pub paused: bool,
    pub opened_at: String,
}

/// Whether a token is a link the gateway made.
pub type Known = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub struct Screen {
    windows: Mutex<Vec<Window>>,
    next: AtomicU64,
    /// The DuoWave app is here to open the windows: on the station and on a server without it there is no screen.
    available: AtomicBool,
    events: Option<EventBus>,
    changes: watch::Sender<Vec<Window>>,
    known: OnceLock<Known>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn norm(text: &str) -> String {
    text.trim().to_lowercase().replace('ё', "е").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title as a window shows it: one line, not endless.
fn title_of(title: &str) -> String {
    let line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cut: String = line.chars().take(120).collect();
    if cut.is_empty() {
        cut = crate::tr!("Окно");
    }
    cut
}

/// A token Files' `media` makes: 48 hex.
fn token_like(file: &str) -> bool {
    file.len() == 48 && file.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Screen {
    pub fn new(events: Option<EventBus>) -> Arc<Screen> {
        let (changes, _) = watch::channel(Vec::new());
        Arc::new(Screen {
            windows: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            available: AtomicBool::new(false),
            events,
            changes,
            known: OnceLock::new(),
        })
    }

    /// The DuoWave app opens the windows here (it says so at every start of the gateway).
    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
        if !available && !locked(&self.windows).is_empty() {
            locked(&self.windows).clear();
            self.told("gone");
        }
    }

    pub fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    /// How a token is checked: Files knows the links it made.
    pub fn set_known(&self, known: Known) {
        let _ = self.known.set(known);
    }

    fn knows(&self, file: &str) -> bool {
        token_like(file) && self.known.get().is_none_or(|known| known(file))
    }

    pub fn windows(&self) -> Vec<Window> {
        locked(&self.windows).clone()
    }

    /// The list as it changes, for the DuoWave app.
    pub fn subscribe(&self) -> watch::Receiver<Vec<Window>> {
        self.changes.subscribe()
    }

    fn told(&self, what: &str) {
        let list = self.windows();
        self.changes.send_replace(list);
        if let Some(events) = &self.events {
            events.emit(GatewayEvent::ScreenChanged { what: what.into() });
        }
    }

    /// New windows. Without `add` the temporary ones before them close: «покажи кота», then «покажи собаку» — the
    /// dog in place of the cat, unless the cat was kept.
    pub fn open(&self, contents: Vec<Content>, add: bool) -> Result<Vec<Window>, String> {
        if !self.available() {
            return Err(crate::tr!("здесь нет экрана: окна открывает приложение DuoWave на компьютере"));
        }
        if contents.is_empty() {
            return Err(crate::tr!("показывать нечего"));
        }
        if let Some(stranger) = contents.iter().find(|content| !self.knows(&content.file)) {
            return Err(crate::tr!("«{title}»: такой ссылки шлюз не давал.", title = stranger.title));
        }
        let opened = {
            let mut windows = locked(&self.windows);
            if !add {
                windows.retain(|window| window.kept);
            }
            let mut opened = Vec::new();
            for content in contents.into_iter().take(MOST) {
                let window = Window {
                    id: self.next.fetch_add(1, Ordering::SeqCst).to_string(),
                    title: title_of(&content.title),
                    kind: content.kind,
                    url: format!("/media/folder/{}", content.file),
                    from: content.from,
                    kept: false,
                    paused: false,
                    opened_at: now_iso(),
                };
                windows.push(window.clone());
                opened.push(window);
            }
            while windows.len() > MOST {
                let oldest = windows.iter().position(|window| !window.kept && !opened.contains(window));
                match oldest {
                    Some(at) => {
                        windows.remove(at);
                    }
                    None => break,
                }
            }
            opened
        };
        self.told("opened");
        Ok(opened)
    }

    /// The windows a person or the agent names: a number, «all», «temporary», a kind («видео»), what it shows
    /// (`files/home/…`), or a part of the title.
    pub fn find(&self, target: &str) -> Result<Vec<String>, String> {
        let windows = self.windows();
        let said = norm(target);
        let ids = |keep: &dyn Fn(&Window) -> bool| -> Vec<String> {
            windows.iter().filter(|w| keep(w)).map(|w| w.id.clone()).collect()
        };
        let found: Vec<String> = match said.as_str() {
            "" => return Err(crate::tr!("какое окно? Скажите его номер или название")),
            "all" | "все" | "всё" | "все окна" => ids(&|_| true),
            "temporary" | "временные" => ids(&|window| !window.kept),
            _ if windows.iter().any(|window| window.id == said) => vec![said.clone()],
            _ if said.starts_with("files/") => ids(&|window| window.from.as_deref() == Some(target.trim())),
            _ => match Kind::named(&said) {
                Some(kind) => ids(&|window| window.kind == kind),
                None => ids(&|window| {
                    let title = norm(&window.title);
                    title.contains(&said) || (title.len() >= 3 && said.contains(&title))
                }),
            },
        };
        if found.is_empty() {
            return Err(crate::tr!("Окна «{name}» нет.", name = target.trim()));
        }
        Ok(found)
    }

    fn change(&self, ids: &[String], what: &str, change: impl Fn(&mut Window)) -> Vec<Window> {
        let changed: Vec<Window> = {
            let mut windows = locked(&self.windows);
            windows
                .iter_mut()
                .filter(|window| ids.contains(&window.id))
                .map(|window| {
                    change(window);
                    window.clone()
                })
                .collect()
        };
        if !changed.is_empty() {
            self.told(what);
        }
        changed
    }

    /// «Оставь»: kept until closed; `false` makes it temporary again.
    pub fn keep(&self, target: &str, kept: bool) -> Result<Vec<Window>, String> {
        let ids = self.find(target)?;
        Ok(self.change(&ids, "kept", |window| window.kept = kept))
    }

    /// Closes the windows named, kept ones too.
    pub fn close(&self, target: &str) -> Result<Vec<Window>, String> {
        let ids = self.find(target)?;
        let closed: Vec<Window> = {
            let mut windows = locked(&self.windows);
            let closed = windows.iter().filter(|window| ids.contains(&window.id)).cloned().collect();
            windows.retain(|window| !ids.contains(&window.id));
            closed
        };
        if !closed.is_empty() {
            self.told("closed");
        }
        Ok(closed)
    }

    /// The windows that play, of a target or all of them.
    fn playing(&self, target: Option<&str>) -> Result<Vec<String>, String> {
        let ids = match target.map(str::trim).filter(|target| !target.is_empty()) {
            Some(target) => self.find(target)?,
            None => self.windows().into_iter().map(|window| window.id).collect(),
        };
        let windows = self.windows();
        let playing: Vec<String> =
            windows.iter().filter(|w| w.kind.plays() && ids.contains(&w.id)).map(|w| w.id.clone()).collect();
        if playing.is_empty() {
            return Err(crate::tr!("на экране ничего не играет"));
        }
        Ok(playing)
    }

    pub fn pause(&self, target: Option<&str>) -> Result<Vec<Window>, String> {
        let ids = self.playing(target)?;
        Ok(self.change(&ids, "paused", |window| window.paused = true))
    }

    pub fn resume(&self, target: Option<&str>) -> Result<Vec<Window>, String> {
        let ids = self.playing(target)?;
        Ok(self.change(&ids, "resumed", |window| window.paused = false))
    }

    /// A video or a sound played to the end: a temporary window closes, a kept one stays, paused.
    pub fn ended(&self, id: &str) {
        let kept = self.windows().iter().find(|window| window.id == id).map(|window| window.kept);
        match kept {
            Some(true) => {
                self.change(&[id.to_string()], "ended", |window| window.paused = true);
            }
            Some(false) => {
                let _ = self.close(id);
            }
            None => {}
        }
    }

    /// A letter to the screen — from an app on the bus (`device/screen`) or the agent's tools: `windows`, `open`
    /// (`{items: [{kind, title, file, from}], add}`), `keep` (`{window, keep}`), `close` (`{window}`), `pause` and
    /// `resume` (`{window}` or all that play).
    pub fn letter(&self, handle: &str, input: &Value) -> Result<Value, String> {
        let window = input.get("window").and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        });
        match handle {
            "windows" => Ok(json!({ "available": self.available(), "windows": self.windows() })),
            "open" => {
                let items = input.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut contents = Vec::new();
                for item in items {
                    let kind = item.get("kind").and_then(Value::as_str).and_then(Kind::parse);
                    let kind = kind.ok_or_else(|| crate::tr!("kind — image, video или audio"))?;
                    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
                    contents.push(Content {
                        kind,
                        title: text("title").unwrap_or_default(),
                        file: text("file").unwrap_or_default(),
                        from: text("from"),
                    });
                }
                let add = input.get("add").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({ "opened": self.open(contents, add)? }))
            }
            "keep" => {
                let kept = input.get("keep").and_then(Value::as_bool).unwrap_or(true);
                Ok(json!({ "kept": self.keep(window.as_deref().unwrap_or_default(), kept)? }))
            }
            "close" => Ok(json!({ "closed": self.close(window.as_deref().unwrap_or_default())? })),
            "pause" => Ok(json!({ "paused": self.pause(window.as_deref())? })),
            "resume" => Ok(json!({ "resumed": self.resume(window.as_deref())? })),
            other => Err(crate::tr!("Экран не умеет «{other}».", other = other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DOG: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SEA: &str = "cccccccccccccccccccccccccccccccccccccccccccccccc";

    fn content(kind: Kind, title: &str, file: &str) -> Content {
        Content { kind, title: title.into(), file: file.into(), from: Some(format!("files/home/Фото/{title}.jpg")) }
    }

    fn screen() -> Arc<Screen> {
        let screen = Screen::new(None);
        screen.set_available(true);
        screen
    }

    #[test]
    fn no_screen_no_window() {
        let screen = Screen::new(None);
        assert!(screen.open(vec![content(Kind::Image, "кот", CAT)], false).unwrap_err().contains("нет экрана"));
    }

    #[test]
    fn the_next_request_takes_the_place_of_the_temporary_windows_not_of_the_kept_ones() {
        let screen = screen();
        let cat = screen.open(vec![content(Kind::Image, "кот", CAT)], false).unwrap();
        assert_eq!(cat[0].url, format!("/media/folder/{CAT}"));
        screen.open(vec![content(Kind::Image, "собака", DOG)], false).unwrap();
        assert_eq!(screen.windows().iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["собака"]);
        screen.keep("собака", true).unwrap();
        screen.open(vec![content(Kind::Image, "кот", CAT), content(Kind::Video, "море", SEA)], false).unwrap();
        assert_eq!(screen.windows().len(), 3, "the kept dog stays beside the two new ones");
        screen.open(vec![content(Kind::Image, "кот", CAT)], true).unwrap();
        assert_eq!(screen.windows().len(), 4, "`add` opens beside");
    }

    #[test]
    fn windows_are_found_by_number_kind_title_and_address() {
        let screen = screen();
        let opened =
            screen.open(vec![content(Kind::Image, "Отпуск 1", CAT), content(Kind::Video, "Море", SEA)], false).unwrap();
        assert_eq!(screen.find(&opened[0].id).unwrap(), [opened[0].id.clone()]);
        assert_eq!(screen.find("видео").unwrap(), [opened[1].id.clone()]);
        assert_eq!(screen.find("отпуск").unwrap(), [opened[0].id.clone()]);
        assert_eq!(screen.find("files/home/Фото/Море.jpg").unwrap(), [opened[1].id.clone()]);
        assert_eq!(screen.find("все").unwrap().len(), 2);
        assert!(screen.find("горы").is_err());
        screen.close("отпуск").unwrap();
        assert_eq!(screen.windows().len(), 1);
    }

    #[test]
    fn a_video_played_to_the_end_closes_unless_kept() {
        let screen = screen();
        let sea = screen.open(vec![content(Kind::Video, "море", SEA)], false).unwrap();
        assert!(screen.pause(None).unwrap()[0].paused);
        assert!(!screen.resume(Some("море")).unwrap()[0].paused);
        assert!(screen.pause(Some("кот")).is_err());
        screen.keep(&sea[0].id, true).unwrap();
        screen.ended(&sea[0].id);
        assert!(screen.windows()[0].paused, "kept: it stays, paused at its end");
        screen.keep(&sea[0].id, false).unwrap();
        screen.ended(&sea[0].id);
        assert!(screen.windows().is_empty());
    }

    #[test]
    fn only_the_gateways_own_links_are_shown_and_no_more_than_most() {
        let screen = screen();
        assert!(screen.open(vec![content(Kind::Image, "чужое", "https://example.com/x.jpg")], false).is_err());
        screen.set_known(Arc::new(|file| file == CAT));
        assert!(screen.open(vec![content(Kind::Image, "собака", DOG)], false).unwrap_err().contains("не давал"));
        for _ in 0..MOST + 3 {
            screen.open(vec![content(Kind::Image, "кот", CAT)], true).unwrap();
        }
        assert_eq!(screen.windows().len(), MOST);
    }

    #[test]
    fn letters_say_the_same() {
        let screen = screen();
        let opened =
            screen.letter("open", &json!({ "items": [{ "kind": "audio", "title": "Песня", "file": CAT }] })).unwrap();
        let id = opened["opened"][0]["id"].as_str().unwrap().to_string();
        let kept = screen.letter("keep", &json!({ "window": id })).unwrap();
        assert_eq!(kept["kept"][0]["kept"], json!(true));
        assert_eq!(screen.letter("windows", &json!({})).unwrap()["windows"][0]["kept"], json!(true));
        assert_eq!(
            screen.letter("close", &json!({ "window": "песня" })).unwrap()["closed"].as_array().unwrap().len(),
            1
        );
        assert!(screen.letter("open", &json!({ "items": [{ "kind": "page", "file": CAT }] })).is_err());
    }
}
