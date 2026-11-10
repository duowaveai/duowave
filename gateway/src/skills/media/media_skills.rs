//! Our media as skills: radio from our station list, music from our library, tales recorded or told by the model,
//! and the control of what our player plays.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use futures::future::BoxFuture;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::favorites::Favorites;
use super::media_types::MediaKind;
use super::player::{MediaPlayer, OUR_AUDIO_PLAY_VERIFIED};
use super::sources::MediaSources;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::directives as commands;
use crate::skills::js;
use crate::skills::own::playback_skill::{PlayerAction, PlayerControl};
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill, PhraseStream};
use crate::skills::text::normalize;
use crate::utils::time::now_ms;

/// Writes a tale as phrases for speech: the model, with a prompt for telling stories.
pub type TaleWriter = Arc<dyn Fn(Option<String>, CancellationToken) -> PhraseStream + Send + Sync>;

/// Radio from the radio sources — our station list, a plugin's catalog: each station's own stream, as Yandex plays
/// radio.
pub struct RadioSkill {
    sources: Arc<MediaSources>,
    player: Arc<MediaPlayer>,
}

impl RadioSkill {
    pub fn new(sources: Arc<MediaSources>, player: Arc<MediaPlayer>) -> RadioSkill {
        RadioSkill { sources, player }
    }

    async fn play(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let query = js::string_param(&intent.params, "station").map(str::to_string);
        let base = context.media_base_url.clone();
        let Some((source, found)) = self.sources.find(MediaKind::Radio, query, base.clone()).await else {
            return Ok(Outcome::speech("Не нашла такую станцию."));
        };
        let Some(first) = found.into_iter().next() else {
            return Ok(Outcome::speech("Не нашла такую станцию."));
        };
        // The rest of that source's list after the match: "дальше" switches the station.
        let all = source.search(None, base).await;
        let mut queue = vec![first.clone()];
        queue.extend(all.into_iter().filter(|item| item.id != first.id));
        Ok(Outcome::directives(self.player.play(&context.device_tag, queue, 0, 0)))
    }
}

impl OwnSkill for RadioSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Radio
    }

    fn verified(&self, _intent: &Intent) -> bool {
        OUR_AUDIO_PLAY_VERIFIED
    }

    fn available(&self) -> bool {
        self.sources.available(MediaKind::Radio)
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.play(intent, context))
    }
}

/// Music from the music sources (our folder, a plugin's library) through our /media: «включи любимое» plays what
/// was liked, a shuffle leaves out what was disliked.
pub struct MusicSkill {
    sources: Arc<MediaSources>,
    player: Arc<MediaPlayer>,
    favorites: Option<Arc<Favorites>>,
}

impl MusicSkill {
    pub fn new(sources: Arc<MediaSources>, player: Arc<MediaPlayer>, favorites: Option<Arc<Favorites>>) -> MusicSkill {
        MusicSkill { sources, player, favorites }
    }

    async fn play(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let query = js::string_param(&intent.params, "query").map(str::to_string);
        let base = context.media_base_url.clone();
        let found = self.sources.find(MediaKind::Music, query.clone(), base.clone()).await;
        let mut items = found.map(|(_, items)| items).unwrap_or_default();
        if js::truthy(intent.params.get("favorites")) {
            let liked = self.favorites.as_ref().map(|favorites| favorites.liked()).unwrap_or_default();
            items = self
                .sources
                .all(MediaKind::Music, base)
                .await
                .into_iter()
                .filter(|item| liked.contains(&item.id))
                .collect();
            if items.is_empty() {
                return Ok(Outcome::speech("В любимом пока пусто. Скажите «мне нравится», когда играет песня."));
            }
        } else if let (None, Some(favorites)) = (query.as_ref().filter(|query| !query.is_empty()), &self.favorites) {
            // Asked for anything: what was disliked stays out; asked by name, it plays.
            let disliked = favorites.disliked();
            items.retain(|item| !disliked.contains(&item.id));
        }
        if items.is_empty() {
            let Some(query) = query.filter(|query| !query.is_empty()) else {
                return Ok(Outcome::speech("Музыки пока нет."));
            };
            // Not in one's own music, but asked by mood or genre («что-нибудь спокойное»): the radio may have it.
            let radio = self.sources.find(MediaKind::Radio, Some(query.clone()), context.media_base_url.clone()).await;
            if let Some((_, stations)) = radio.filter(|(_, stations)| !stations.is_empty()) {
                let title = stations[0].title.clone();
                let mut outcome = Outcome::directives(self.player.play(&context.device_tag, stations, 0, 0));
                outcome.speech = Some(format!("В своей музыке не нашла — включаю радио «{title}»."));
                return Ok(outcome);
            }
            return Ok(Outcome::speech(format!("Не нашла «{query}».")));
        }
        items.truncate(50);
        Ok(Outcome::directives(self.player.play(&context.device_tag, items, 0, 0)))
    }
}

impl OwnSkill for MusicSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Music
    }

    fn verified(&self, _intent: &Intent) -> bool {
        OUR_AUDIO_PLAY_VERIFIED
    }

    fn available(&self) -> bool {
        self.sources.available(MediaKind::Music)
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.play(intent, context))
    }
}

static NOT_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").expect("a valid pattern"));

/// A file name for a told tale: its topic's words with dashes.
fn slug(topic: Option<&str>) -> String {
    let words = NOT_SLUG.replace_all(&normalize(topic.unwrap_or("любая")), "-").into_owned();
    let cut: String = words.chars().take(60).collect();
    if cut.is_empty() {
        "skazka".into()
    } else {
        cut
    }
}

/// Tales: a recorded one from our shelf, played by the station's player (pause and "продолжи" work); otherwise the
/// model tells one in Alice's voice, and its audio is kept so the same tale plays from the shelf next time.
pub struct TalesSkill {
    /// The tales sources: our recorded tales and audiobooks, a plugin's.
    shelf: Arc<MediaSources>,
    player: Arc<MediaPlayer>,
    /// CONTENT_DIR/tales/generated.
    generated_dir: PathBuf,
    writer: Option<TaleWriter>,
    /// Whether the model answers now: the panel may switch it off and on without a restart.
    writing: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl TalesSkill {
    pub fn new(
        shelf: Arc<MediaSources>,
        player: Arc<MediaPlayer>,
        generated_dir: PathBuf,
        writer: Option<TaleWriter>,
        writing: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> TalesSkill {
        TalesSkill { shelf, player, generated_dir, writer, writing: writing.unwrap_or_else(|| Arc::new(|| true)) }
    }

    async fn tell(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let topic = js::string_param(&intent.params, "topic")
            .or_else(|| js::string_param(&intent.params, "query"))
            .map(str::to_string);
        let audiobook = intent.action == "audiobook";
        if OUR_AUDIO_PLAY_VERIFIED || audiobook {
            let found = self.shelf.find(MediaKind::Tales, topic.clone(), context.media_base_url.clone()).await;
            if let Some(recorded) = found.and_then(|(_, items)| items.into_iter().next()) {
                return Ok(Outcome::directives(self.player.play(&context.device_tag, vec![recorded], 0, 0)));
            }
            if audiobook {
                return Ok(Outcome::speech(match topic.filter(|topic| !topic.is_empty()) {
                    Some(topic) => format!("Не нашла аудиокнигу «{topic}»."),
                    None => "Аудиокниг пока нет.".into(),
                }));
            }
        }
        let Some(writer) = self.writer.as_ref().filter(|_| (self.writing)()) else {
            return Ok(Outcome::speech("Сказок пока нет."));
        };
        let record = self.generated_dir.join(format!("{}.ogg", slug(topic.as_deref())));
        Ok(Outcome {
            phrases: Some(writer(topic, context.cancel.clone())),
            record_audio: Some(record),
            ..Outcome::default()
        })
    }
}

impl OwnSkill for TalesSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Tales
    }

    fn verified(&self, intent: &Intent) -> bool {
        // A told tale is speech; a recorded one needs our links to play.
        intent.action == "tell" || OUR_AUDIO_PLAY_VERIFIED
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.tell(intent, context))
    }
}

/// Next, previous and continue for what our player plays: a new audio_play from its queue; likes of its tracks.
pub struct MediaControl {
    player: Arc<MediaPlayer>,
    favorites: Option<Arc<Favorites>>,
}

impl MediaControl {
    pub fn new(player: Arc<MediaPlayer>, favorites: Option<Arc<Favorites>>) -> MediaControl {
        MediaControl { player, favorites }
    }
}

impl PlayerControl for MediaControl {
    fn rate(&self, liked: bool, context: &ExecutionContext) -> Option<Outcome> {
        let (item, _) = self.player.current(&context.device_tag)?;
        let favorites = self.favorites.as_ref()?;
        if item.r#type != "Track" {
            return Some(Outcome::speech("Отмечать могу только песни."));
        }
        let changed = favorites.rate(&item, liked, context.now);
        if liked {
            return Some(Outcome::speech(if changed {
                "Запомнила, добавлю в любимое."
            } else {
                "Эта песня уже в любимом."
            }));
        }
        // A dislike skips it too, as on Alice; at the end of the queue the music stops.
        let next = self.player.step(&context.device_tag, 1, 0);
        if next.is_none() {
            self.player.stopping(&context.device_tag, now_ms());
        }
        Some(Outcome::said("Хорошо, больше не поставлю.", next.unwrap_or_else(commands::stop_player)))
    }

    fn owns(&self, device_tag: &str) -> bool {
        self.player.current(device_tag).is_some_and(|(_, state)| {
            state.state != super::player::PlayState::Finished && now_ms().saturating_sub(state.at) < 6 * 3_600_000
        })
    }

    fn stopping(&self, device_tag: &str) {
        self.player.stopping(device_tag, now_ms());
    }

    fn control(&self, action: PlayerAction, context: &ExecutionContext) -> Option<Outcome> {
        let device = context.device_tag.as_str();
        match action {
            PlayerAction::Replay => self.player.step(device, 0, 0).map(Outcome::directives),
            PlayerAction::Resume => {
                let current = self.player.current(device);
                let reported = context.device_state.as_ref().and_then(|state| state.player.as_ref());
                // Where the station says it stands; without its word (the panel's «продолжить») where it stood by our
                // count — not from the start.
                let offset = match (&current, reported) {
                    (Some((item, _)), Some(reported)) if reported.id.as_deref() == Some(item.id.as_str()) => {
                        reported.offset_ms.unwrap_or(0).max(0) as u64
                    }
                    (Some((_, state)), _) => state.position_ms(context.now.timestamp_millis().max(0) as u64),
                    _ => 0,
                };
                let radio = current.as_ref().is_some_and(|(item, _)| item.r#type == "FmRadio");
                self.player.step(device, 0, if radio { 0 } else { offset }).map(Outcome::directives)
            }
            PlayerAction::Next | PlayerAction::Previous => {
                let delta = if action == PlayerAction::Next { 1 } else { -1 };
                Some(match self.player.step(device, delta, 0) {
                    Some(directives) => Outcome::directives(directives),
                    None => Outcome::speech(if action == PlayerAction::Next {
                        "Это было последнее."
                    } else {
                        "Это первое."
                    }),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::protocol_types::{DeviceState, Directive, PlayerState as StationPlayer, PlayerView};
    use crate::skills::media::media_types::MediaSource;
    use crate::skills::media::player::PlayerEvent;
    use crate::skills::media::sources::{FolderLibrary, StaticRadioCatalog};
    use crate::storage::{Db, StateStore};
    use chrono::Utc;
    use serde_json::{json, Value};

    struct Content {
        dir: tempfile::TempDir,
    }

    impl Content {
        fn new() -> Content {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("radio.json"),
                json!([
                    {"id": "studio21", "title": "Студия 21", "aliases": ["studio 21"], "url": "https://hls.example/studio21.m3u8"},
                    {"id": "jazz", "title": "Джаз ФМ", "url": "https://radio.example/jazz.mp3"},
                ])
                .to_string(),
            )
            .unwrap();
            std::fs::create_dir_all(dir.path().join("music/Альбом")).unwrap();
            std::fs::write(dir.path().join("music/Альбом/Кино - Группа крови.mp3"), "ID3").unwrap();
            std::fs::write(dir.path().join("music/Кино - Звезда по имени Солнце.mp3"), "ID3").unwrap();
            Content { dir }
        }

        fn radio(&self) -> Arc<StaticRadioCatalog> {
            Arc::new(StaticRadioCatalog::new(self.dir.path().join("radio.json")))
        }

        fn music(&self) -> Arc<FolderLibrary> {
            Arc::new(FolderLibrary::new(self.dir.path().to_path_buf(), "music", "Track"))
        }

        /// The one source, as the skills take their kind's sources.
        fn sources(source: Arc<dyn MediaSource>) -> Arc<MediaSources> {
            MediaSources::new(vec![source])
        }
    }

    fn context() -> ExecutionContext {
        let mut context = ExecutionContext::new("h:test", Utc::now());
        context.media_base_url = Some("http://gateway.local/media".into());
        context
    }

    fn intent(capability: CapabilityName, params: Value) -> Intent {
        Intent { capability, action: "play".into(), params: params.as_object().cloned().unwrap() }
    }

    fn payload(directive: &Directive) -> &serde_json::Map<String, Value> {
        directive.payload.as_ref().unwrap()
    }

    fn playing(outcome: &Outcome) -> String {
        payload(&outcome.directives[0])["stream"]["id"].as_str().unwrap().to_string()
    }

    fn favorites() -> Arc<Favorites> {
        Arc::new(Favorites::new(StateStore::open(Arc::new(Db::memory()), "favorites")))
    }

    #[tokio::test]
    async fn finds_a_radio_station_by_name_or_alias_and_plays_it_with_our_callbacks() {
        let content = Content::new();
        let player = Arc::new(MediaPlayer::new());
        let skill = RadioSkill::new(Content::sources(content.radio()), player.clone());
        assert!(skill.available());
        let outcome =
            skill.execute(&intent(CapabilityName::Radio, json!({"station": "студия 21"})), &context()).await.unwrap();
        let play = &outcome.directives[0];
        assert_eq!((play.name.as_str(), play.analytics_type.as_deref()), ("audio_play", Some("radio")));
        let payload = payload(play);
        assert_eq!(
            payload["stream"],
            json!({"id": "studio21", "url": "https://hls.example/studio21.m3u8", "format": "HLS", "type": "FmRadio", "offset_ms": 0, "duration_ms": 2147483647})
        );
        assert_eq!(payload["metadata"]["title"], "Студия 21");
        assert_eq!(payload["metadata"]["hide_progress_bar"], true);
        assert_eq!(payload["background_mode"], "Ducking");
        let callbacks = &payload["callbacks"];
        assert_eq!(callbacks["on_started"]["name"], "vgw_media_on_started");
        assert_eq!(callbacks["on_started"]["type"], "server_action");
        assert_eq!(callbacks["on_started"]["ignore_answer"], true);
        assert_eq!(callbacks["on_stopped"]["name"], "vgw_media_on_stopped");
        assert_eq!(callbacks["on_stopped"]["ignore_answer"], true);
        assert_eq!(callbacks["on_finished"]["name"], "vgw_media_on_finished");
        assert_eq!(callbacks["on_finished"]["ignore_answer"], false);
        // A live radio has no next track to ask for.
        assert_eq!(outcome.directives.len(), 1);
        let queue: Vec<String> = player.current("h:test").unwrap().1.queue.into_iter().map(|item| item.id).collect();
        assert_eq!(queue, ["studio21", "jazz"]);
        let missing = skill.execute(&intent(CapabilityName::Radio, json!({"station": "нет такой"})), &context()).await;
        assert_eq!(missing.unwrap().speech.as_deref(), Some("Не нашла такую станцию."));
    }

    #[tokio::test]
    async fn plays_our_music_from_media_and_goes_on_to_the_next_track_when_one_finishes() {
        let content = Content::new();
        let player = Arc::new(MediaPlayer::new());
        let skill = MusicSkill::new(Content::sources(content.music()), player.clone(), None);
        let outcome = skill
            .execute(&intent(CapabilityName::Music, json!({"query": "кино группа крови"})), &context())
            .await
            .unwrap();
        let stream = &payload(&outcome.directives[0])["stream"];
        assert_eq!((stream["format"].as_str(), stream["type"].as_str()), (Some("MP3"), Some("Track")));
        assert_eq!(
            stream["url"],
            format!(
                "http://gateway.local/media/music/{}/{}",
                js::encode_uri_component("Альбом"),
                js::encode_uri_component("Кино - Группа крови.mp3")
            )
        );

        // The whole artist as a queue; the end of the first asks for the second.
        skill.execute(&intent(CapabilityName::Music, json!({"query": "кино"})), &context()).await.unwrap();
        let (_, state) = player.current("h:test").unwrap();
        assert_eq!(state.queue.len(), 2);
        let next = player.on_event("h:test", PlayerEvent::Finished, &state.play_id, now_ms()).unwrap();
        assert_eq!(payload(&next[0])["stream"]["id"], state.queue[1].id.as_str());
        assert!(player.on_event("h:test", PlayerEvent::Finished, "an old play", now_ms()).is_none());

        // Not in one's own music: the radio, when it has it; else «не нашла».
        let both = MediaSources::new(vec![content.music(), content.radio()]);
        let skill = MusicSkill::new(both, player.clone(), None);
        let jazz = skill.execute(&intent(CapabilityName::Music, json!({"query": "джаз"})), &context()).await.unwrap();
        assert_eq!(playing(&jazz), "jazz");
        assert_eq!(jazz.speech.as_deref(), Some("В своей музыке не нашла — включаю радио «Джаз ФМ»."));
        let none = skill.execute(&intent(CapabilityName::Music, json!({"query": "опера"})), &context()).await.unwrap();
        assert_eq!(none.speech.as_deref(), Some("Не нашла «опера»."));
    }

    #[tokio::test]
    async fn lets_dalshe_and_prodolzhi_play_from_our_queue_at_the_position_the_station_reports() {
        let content = Content::new();
        let player = Arc::new(MediaPlayer::new());
        let control = MediaControl::new(player.clone(), None);
        let items = content.music().search(Some("кино".into()), Some("http://gateway.local/media".into())).await;
        player.play("h:test", items.clone(), 0, 0);
        assert!(control.owns("h:test"));
        let next = control.control(PlayerAction::Next, &context()).unwrap();
        assert_eq!(playing(&next), items[1].id);
        let mut reported = context();
        reported.device_state = Some(DeviceState {
            player: Some(PlayerView {
                state: StationPlayer::Stopped,
                id: Some(items[1].id.clone()),
                title: None,
                r#type: None,
                offset_ms: Some(42_000),
            }),
            ..DeviceState::default()
        });
        let resume = control.control(PlayerAction::Resume, &reported).unwrap();
        assert_eq!(payload(&resume.directives[0])["stream"]["id"], items[1].id.as_str());
        assert_eq!(payload(&resume.directives[0])["stream"]["offset_ms"], 42_000);
        assert_eq!(
            control.control(PlayerAction::Next, &context()).unwrap().speech.as_deref(),
            Some("Это было последнее.")
        );
        // «Сначала»: the same track from its start.
        let replay = control.control(PlayerAction::Replay, &context()).unwrap();
        assert_eq!(payload(&replay.directives[0])["stream"]["id"], items[1].id.as_str());
        assert_eq!(payload(&replay.directives[0])["stream"]["offset_ms"], 0);
    }

    /// Review G5: the panel's «продолжить» has no word from the station — the track goes on where it was paused, by
    /// our count, not from its start.
    #[tokio::test]
    async fn continues_where_it_stood_by_our_count_when_the_station_says_nothing() {
        let player = Arc::new(MediaPlayer::new());
        let control = MediaControl::new(player.clone(), None);
        let track = crate::skills::media::media_types::MediaItem {
            id: "music/a.mp3".into(),
            title: "a".into(),
            url: "http://gateway.local/media/music/a.mp3".into(),
            format: "MP3".into(),
            codec: None,
            r#type: "Track".into(),
            subtitle: None,
            image_url: None,
            duration_ms: Some(240_000),
        };
        player.play("h:test", vec![track], 0, 0);
        let play = player.current("h:test").unwrap().1.play_id;
        let started = now_ms() - 200_000;
        player.on_event("h:test", PlayerEvent::Started, &play, started);
        player.stopping("h:test", started + 130_000);
        player.on_event("h:test", PlayerEvent::Stopped, &play, started + 130_000);
        let resume = control.control(PlayerAction::Resume, &context()).unwrap();
        assert_eq!(payload(&resume.directives[0])["stream"]["offset_ms"], 130_000);
    }

    #[tokio::test]
    async fn keeps_likes_of_our_tracks_vklyuchi_lyubimoe_plays_them_a_dislike_skips_the_track_and_leaves_it_out_of_a_shuffle(
    ) {
        let content = Content::new();
        let favorites = favorites();
        let player = Arc::new(MediaPlayer::new());
        let control = MediaControl::new(player.clone(), Some(favorites.clone()));
        let skill = MusicSkill::new(Content::sources(content.music()), player.clone(), Some(favorites.clone()));
        let play = |params: Value| intent(CapabilityName::Music, params);

        let empty = skill.execute(&play(json!({"favorites": true})), &context()).await.unwrap();
        assert!(empty.speech.unwrap().starts_with("В любимом пока пусто"));
        let first = playing(&skill.execute(&play(json!({"query": "группа крови"})), &context()).await.unwrap());
        assert_eq!(control.rate(true, &context()).unwrap().speech.as_deref(), Some("Запомнила, добавлю в любимое."));
        assert_eq!(control.rate(true, &context()).unwrap().speech.as_deref(), Some("Эта песня уже в любимом."));
        assert_eq!(playing(&skill.execute(&play(json!({"favorites": true})), &context()).await.unwrap()), first);

        // The other track, disliked: the music goes on past it (here the queue ends, so it stops), and a shuffle
        // leaves it out.
        let other = playing(&skill.execute(&play(json!({"query": "звезда"})), &context()).await.unwrap());
        let dislike = control.rate(false, &context()).unwrap();
        assert_eq!(dislike.speech.as_deref(), Some("Хорошо, больше не поставлю."));
        assert!(dislike.directives.iter().any(|directive| directive.name == "player_pause"));
        for _ in 0..5 {
            assert_ne!(playing(&skill.execute(&play(json!({})), &context()).await.unwrap()), other);
        }
        // Asked by name, it still plays.
        assert_eq!(playing(&skill.execute(&play(json!({"query": "звезда"})), &context()).await.unwrap()), other);
        let ratings: Vec<(String, bool)> = favorites.ratings().into_iter().map(|r| (r.id, r.liked)).collect();
        assert_eq!(ratings, [(first, true), (other, false)]);
    }

    #[tokio::test]
    async fn marks_only_songs() {
        let content = Content::new();
        let player = Arc::new(MediaPlayer::new());
        let control = MediaControl::new(player.clone(), Some(favorites()));
        let radio = content.radio().search(Some("джаз".into()), None).await;
        player.play("h:test", radio, 0, 0);
        assert_eq!(control.rate(true, &context()).unwrap().speech.as_deref(), Some("Отмечать могу только песни."));
    }

    #[tokio::test]
    async fn tells_a_tale_on_the_model_and_keeps_its_audio_when_none_is_recorded() {
        let content = Content::new();
        let shelf =
            Content::sources(Arc::new(FolderLibrary::new(content.dir.path().to_path_buf(), "tales", "FairyTale")));
        let writer: TaleWriter = Arc::new(|topic, _cancel| {
            futures::StreamExt::boxed(futures::stream::iter([Ok(format!("Жил-был {}.", topic.unwrap_or_default()))]))
        });
        let generated = content.dir.path().join("tales/generated");
        let skill = TalesSkill::new(shelf, Arc::new(MediaPlayer::new()), generated.clone(), Some(writer), None);
        let tale = Intent {
            capability: CapabilityName::Tales,
            action: "tell".into(),
            params: json!({"topic": "Ёжика!"}).as_object().cloned().unwrap(),
        };
        let outcome = skill.execute(&tale, &context()).await.unwrap();
        assert_eq!(outcome.record_audio, Some(generated.join("ежика.ogg")));
        let phrases: Vec<String> = futures::StreamExt::collect::<Vec<_>>(outcome.phrases.unwrap())
            .await
            .into_iter()
            .map(|p| p.unwrap())
            .collect();
        assert_eq!(phrases, ["Жил-был Ёжика!."]);
        let book = Intent { action: "audiobook".into(), ..tale };
        assert_eq!(
            skill.execute(&book, &context()).await.unwrap().speech.as_deref(),
            Some("Не нашла аудиокнигу «Ёжика!».")
        );
        assert_eq!(slug(None), "любая");
        assert_eq!(slug(Some("!!!")), "skazka");
    }
}
