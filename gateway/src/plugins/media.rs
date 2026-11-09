//! A plugin's media source: one more source for the radio, the music or the tales skill (skills/media/sources.rs).
//! Its function takes `{query, limit}` and gives items; the gateway checks each one and plays it through our player
//! with audio_play — so «пауза», «дальше», «громче» are the built-in skills' as for any source.
//!
//! An item's sound is a link the station fetches itself: a stream on the internet (`url`), a file of the plugin's
//! package (`path` under its `assets/`), served by our /media as `/media/packages/<id>/<path>`, a file of the
//! household's folders its `content:` rights name (`content`), or an app's own sound (`stream`: its `/stream/<path>`
//! on localhost, given on by our /media as `/media/apps/<id>/<path>`).

use std::sync::Weak;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::controller::PluginHost;
use super::manifest::MediaSpec;
use super::runtime::CallKind;
use crate::skills::js;
use crate::skills::media::media_types::{MediaItem, MediaKind, MediaSource};

/// How many items a source may give at once.
pub const MAX_ITEMS: usize = 100;
/// The types audio_play knows for our player.
const TYPES: &[&str] = &["FmRadio", "Track", "FairyTale", "AudioBook"];

/// A path for a link, each part encoded; None for an empty one or one with a dot part (`..`, hidden files).
fn encoded(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|part| part.starts_with('.')) {
        return None;
    }
    Some(parts.iter().map(|part| js::encode_uri_component(part)).collect::<Vec<_>>().join("/"))
}

/// An answer the home computer gave for a plugin that runs there («колонка + сервер», `prefer: home`), made playable
/// from the station: an app's own sound (`stream`) and the household's files (`content`) are the computer's, so they
/// become links to the computer's /media, which the station reaches over the home network. `home` is the computer's
/// address (`http://<computer>:<port>`); what the package has no right to stays as it was and is refused later.
pub fn from_home(answer: Value, home: &str, plugin: &str, rights: &PlayRights) -> Value {
    fn item(value: &mut Value, home: &str, plugin: &str, rights: &PlayRights) {
        let Value::Object(item) = value else { return };
        if item.contains_key("url") || item.contains_key("path") {
            return;
        }
        let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::trim).map(str::to_string);
        let home = home.trim_end_matches('/');
        let url = match (text("content"), text("stream")) {
            (Some(content), _)
                if rights.content.iter().any(|allowed| Some(allowed.as_str()) == content.split('/').next()) =>
            {
                encoded(&content).map(|path| format!("{home}/media/{path}"))
            }
            (None, Some(stream)) if rights.streams => {
                encoded(&stream).map(|path| format!("{home}/media/apps/{plugin}/{path}"))
            }
            _ => None,
        };
        if let Some(url) = url {
            item.remove("content");
            item.remove("stream");
            item.insert("url".into(), Value::String(url));
        }
    }
    fn list(value: &mut Value, home: &str, plugin: &str, rights: &PlayRights) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(|value| item(value, home, plugin, rights)),
            Value::Object(object) => {
                if let Some(items) = object.get_mut("items") {
                    list(items, home, plugin, rights);
                }
                if let Some(play) = object.get_mut("play") {
                    list(play, home, plugin, rights);
                }
            }
            _ => {}
        }
    }
    let mut answer = answer;
    list(&mut answer, home, plugin, rights);
    answer
}

/// What a package's items may play besides links and its own files: the household's folders of its `content:` rights
/// (`content:music`, `content:tales`), and an app's own sound (`stream`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayRights {
    pub content: Vec<String>,
    pub streams: bool,
}

/// An item a plugin gave, checked: a title, an id, and a link the station can play; None when it is not one.
pub fn media_item(plugin: &str, kind: MediaKind, value: &Value, media_base: Option<&str>) -> Option<MediaItem> {
    media_item_with(plugin, kind, value, media_base, &PlayRights::default())
}

/// The same, with what the package may play besides: an item's `content` is a file of the household's folders, its
/// `stream` an app's own sound, both played from the gateway's /media.
pub fn media_item_with(
    plugin: &str,
    kind: MediaKind,
    value: &Value,
    media_base: Option<&str>,
    rights: &PlayRights,
) -> Option<MediaItem> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let id = text("id")?;
    let title = text("title")?;
    let base = || media_base.filter(|base| !base.is_empty());
    let url = match (text("url"), text("path"), text("content"), text("stream")) {
        (Some(url), ..) => {
            let parsed = url::Url::parse(url).ok()?;
            matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())?
        }
        (None, Some(path), ..) => format!("{}/packages/{plugin}/{}", base()?, encoded(path)?),
        (None, None, Some(path), _) => {
            let folder = path.split('/').next().unwrap_or_default();
            if !rights.content.iter().any(|allowed| allowed == folder) {
                return None;
            }
            format!("{}/{}", base()?, encoded(path)?)
        }
        (None, None, None, Some(path)) if rights.streams => format!("{}/apps/{plugin}/{}", base()?, encoded(path)?),
        _ => return None,
    };
    let format = match text("format").map(str::to_ascii_uppercase) {
        Some(format) if format == "HLS" || format == "MP3" => format,
        Some(_) => return None,
        None if url.contains(".m3u8") => "HLS".into(),
        None => "MP3".into(),
    };
    let r#type = match text("type") {
        Some(given) if TYPES.contains(&given) => given.to_string(),
        Some(_) => return None,
        None => kind.item_type().to_string(),
    };
    Some(MediaItem {
        id: format!("{plugin}/{id}"),
        title: title.chars().take(200).collect(),
        url,
        format,
        codec: text("codec").filter(|codec| *codec == "AacMp4").map(str::to_string),
        r#type,
        subtitle: text("subtitle").map(|subtitle| subtitle.chars().take(200).collect()),
        image_url: text("imageUrl")
            .filter(|image| image.starts_with("https://") || image.starts_with("http://"))
            .map(str::to_string),
        duration_ms: value.get("durationMs").and_then(Value::as_u64),
    })
}

/// The kind of media a plugin's answer plays for a capability: radio, tales, music otherwise.
pub fn kind_for(capability: crate::skills::capabilities::CapabilityName) -> MediaKind {
    use crate::skills::capabilities::CapabilityName;
    match capability {
        CapabilityName::Radio => MediaKind::Radio,
        CapabilityName::Tales => MediaKind::Tales,
        _ => MediaKind::Music,
    }
}

/// Items from a plugin's answer: an array of them, or `{items: […]}`; the ones that are not items are left out.
pub fn media_items(plugin: &str, kind: MediaKind, answer: &Value, media_base: Option<&str>) -> Vec<MediaItem> {
    media_items_with(plugin, kind, answer, media_base, &PlayRights::default())
}

/// The same, with what the package may play besides.
pub fn media_items_with(
    plugin: &str,
    kind: MediaKind,
    answer: &Value,
    media_base: Option<&str>,
    rights: &PlayRights,
) -> Vec<MediaItem> {
    let list = match answer {
        Value::Array(items) => items.as_slice(),
        Value::Object(object) => object.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(),
        _ => &[],
    };
    list.iter().take(MAX_ITEMS).filter_map(|item| media_item_with(plugin, kind, item, media_base, rights)).collect()
}

pub struct PluginMediaSource {
    host: Weak<PluginHost>,
    plugin: String,
    spec: MediaSpec,
}

impl PluginMediaSource {
    pub fn new(host: Weak<PluginHost>, plugin: &str, spec: MediaSpec) -> PluginMediaSource {
        PluginMediaSource { host, plugin: plugin.to_string(), spec }
    }
}

impl MediaSource for PluginMediaSource {
    fn id(&self) -> String {
        format!("plugin:{}/{}", self.plugin, self.spec.id)
    }

    fn kind(&self) -> MediaKind {
        self.spec.kind
    }

    fn search(&self, query: Option<String>, media_base: Option<String>) -> BoxFuture<'_, Vec<MediaItem>> {
        Box::pin(async move {
            let Some(host) = self.host.upgrade() else { return Vec::new() };
            let input = json!({ "query": query, "limit": MAX_ITEMS });
            match host.call(&self.plugin, &self.spec.handler, input, CallKind::Voice).await {
                Ok(answer) => {
                    let rights = host.play_rights(&self.plugin);
                    media_items_with(&self.plugin, self.spec.kind, &answer, media_base.as_deref(), &rights)
                }
                // The failure is the plugin's last error already; the next source is asked.
                Err(_) => Vec::new(),
            }
        })
    }

    fn available(&self) -> bool {
        self.host.upgrade().is_some_and(|host| host.is_active(&self.plugin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_an_item_with_a_link_the_station_can_play_and_nothing_else() {
        let base = Some("http://gw:3000/media");
        let stream = json!({"id": "jazz", "title": "Джаз", "url": "https://radio.example/jazz.m3u8"});
        let item = media_item("radio", MediaKind::Radio, &stream, base).unwrap();
        assert_eq!((item.id.as_str(), item.format.as_str(), item.r#type.as_str()), ("radio/jazz", "HLS", "FmRadio"));
        let file = json!({"id": "intro", "title": "Заставка", "path": "sounds/вступление.mp3", "type": "Track"});
        let item = media_item("radio", MediaKind::Music, &file, base).unwrap();
        assert_eq!(
            item.url,
            format!("http://gw:3000/media/packages/radio/sounds/{}", js::encode_uri_component("вступление.mp3"))
        );
        assert_eq!(item.format, "MP3");
        for bad in [
            json!({"id": "x", "title": "x", "url": "file:///etc/passwd"}),
            json!({"id": "x", "title": "x", "path": "../secret.mp3"}),
            json!({"id": "x", "title": "x"}),
            json!({"id": "x", "url": "https://radio.example/x.mp3"}),
            json!({"id": "x", "title": "x", "url": "https://radio.example/x.mp3", "format": "OGG"}),
            json!({"id": "x", "title": "x", "url": "https://radio.example/x.mp3", "type": "Video"}),
        ] {
            assert!(media_item("radio", MediaKind::Radio, &bad, base).is_none(), "{bad}");
        }
        assert!(media_item("radio", MediaKind::Music, &file, None).is_none(), "a file needs our /media");
        let answer = json!({"items": [stream, {"id": "bad"}]});
        assert_eq!(media_items("radio", MediaKind::Radio, &answer, base).len(), 1);
    }

    #[test]
    fn makes_what_the_home_computer_plays_a_link_to_its_own_media() {
        let rights = PlayRights { content: vec!["music".into()], streams: true };
        let answer = json!({
            "say": "Включаю",
            "play": [
                { "id": "now", "title": "Spotify", "stream": "now.mp3" },
                { "id": "s", "title": "Песня", "content": "music/Альбом/1.mp3" },
                { "id": "t", "title": "Сказка", "content": "tales/колобок.mp3" },
                { "id": "r", "title": "Радио", "url": "https://radio.example/r.mp3" }
            ]
        });
        let played = from_home(answer, "http://192.0.2.20:80/", "spotify", &rights);
        assert_eq!(played["play"][0]["url"], "http://192.0.2.20:80/media/apps/spotify/now.mp3");
        assert!(played["play"][0].get("stream").is_none());
        assert_eq!(
            played["play"][1]["url"],
            format!("http://192.0.2.20:80/media/music/{}/1.mp3", js::encode_uri_component("Альбом"))
        );
        assert_eq!(played["play"][2]["content"], "tales/колобок.mp3", "no right to tales: left to be refused");
        assert_eq!(played["play"][3]["url"], "https://radio.example/r.mp3");
        // A media source's list and a plugin without its own sound.
        let items = from_home(
            json!([{ "id": "x", "title": "x", "stream": "x.mp3" }]),
            "http://h",
            "radio",
            &PlayRights::default(),
        );
        assert!(items[0].get("url").is_none());
        let items = from_home(
            json!({ "items": [{ "id": "x", "title": "x", "stream": "x.mp3" }] }),
            "http://h",
            "spotify",
            &rights,
        );
        assert_eq!(items["items"][0]["url"], "http://h/media/apps/spotify/x.mp3");
    }

    #[test]
    fn plays_the_households_folders_and_an_apps_own_sound_only_with_the_right() {
        let base = Some("http://gw:3000/media");
        let song = json!({"id": "s", "title": "Песня", "content": "music/Альбом/1.mp3"});
        let rights = PlayRights { content: vec!["music".into()], streams: false };
        let item = media_item_with("music", MediaKind::Music, &song, base, &rights).unwrap();
        assert_eq!(item.url, format!("http://gw:3000/media/music/{}/1.mp3", js::encode_uri_component("Альбом")));
        let tale = json!({"id": "t", "title": "Сказка", "content": "tales/колобок.mp3"});
        assert!(media_item_with("music", MediaKind::Tales, &tale, base, &rights).is_none(), "not its folder");
        assert!(media_item("music", MediaKind::Music, &song, base).is_none(), "no right, no folder");

        let sound = json!({"id": "now", "title": "Spotify", "stream": "now.mp3"});
        let app = PlayRights { content: Vec::new(), streams: true };
        let item = media_item_with("spotify", MediaKind::Music, &sound, base, &app).unwrap();
        assert_eq!((item.url.as_str(), item.format.as_str()), ("http://gw:3000/media/apps/spotify/now.mp3", "MP3"));
        assert!(media_item("radio", MediaKind::Music, &sound, base).is_none(), "a plugin has no sound of its own");
        let hidden = json!({"id": "x", "title": "x", "stream": "../call/x"});
        assert!(media_item_with("spotify", MediaKind::Music, &hidden, base, &app).is_none());
    }
}
