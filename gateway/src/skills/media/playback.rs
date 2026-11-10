//! What the station says of what our player plays (docs/plans/plugins.md, «Ответы колонки — в журнал и плагину»,
//! stage 5а). Before, its answers to audio_play (`vgw_media_on_started`, `…_stopped`, `…_failed`) were only in the
//! session's own journal: on 07.11 «Джаз не играет» was found by reading it by hand. Now:
//!
//! - the request that started the item gets a note — «колонка начала играть», «остановилась», «молчит 5 с»;
//! - the plugin whose item it is hears `player.started`, `player.stopped`, `player.failed`, `player.silent` and may
//!   answer a silence with the next station (`{play: […]}`); an item another play took the place of while it went on
//!   is told stopped from here — the station's word of it comes for a play no longer followed;
//! - a stream the station keeps silent on goes the gateway's way (the owner's decision 9): no `on_started` within
//!   `SILENT_AFTER` — the gateway relays it through its own /media (an AAC stream transcoded where that is allowed),
//!   and the station is told to play that instead, through a push it calls back for (as our speech without a
//!   request is). Through the gateway too silent — the plugin is told, and the request says so.
//!
//! The station is never touched beyond the commands our player sends anyway; the station's own player decides.

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use serde_json::{json, Map};

use super::media_types::MediaItem;
use super::player::{MediaPlayer, PlayState, PlayerEvent, PlayerState, MEDIA_CALLBACK, MEDIA_PLAY_ID};
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};

/// The station had this long to say it started; then it is silent (07.11: «Хит FM» started within 1–3 s).
pub const SILENT_AFTER: Duration = Duration::from_secs(5);

/// Our push for the fallback: the station calls back with it, and the answer plays the item anew.
pub const RETRY_CALLBACK: &str = "retry";

/// What happened to an item, as the station said or kept silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackNews {
    Started,
    Stopped,
    Finished,
    Failed,
    /// No word from the station; `retrying`: the gateway's relay is tried next.
    Silent {
        seconds: u64,
        retrying: bool,
    },
}

impl PlaybackNews {
    /// The plugins' event of it.
    pub fn event(self) -> &'static str {
        match self {
            PlaybackNews::Started => "player.started",
            PlaybackNews::Stopped | PlaybackNews::Finished => "player.stopped",
            PlaybackNews::Failed => "player.failed",
            PlaybackNews::Silent { .. } => "player.silent",
        }
    }

    /// The note in the request's journal.
    pub fn note(self, title: &str, via_gateway: bool) -> String {
        let way = if via_gateway { " (через шлюз)" } else { "" };
        match self {
            PlaybackNews::Started => format!("колонка начала играть: {title}{way}"),
            PlaybackNews::Stopped => format!("колонка остановила: {title}"),
            PlaybackNews::Finished => format!("доиграло: {title}"),
            PlaybackNews::Failed => format!("колонка не смогла играть: {title}{way}"),
            PlaybackNews::Silent { seconds, retrying: true } => {
                format!("колонка молчит {seconds} с: {title} — пробую через шлюз")
            }
            PlaybackNews::Silent { seconds, retrying: false } => format!("колонка молчит {seconds} с: {title}{way}"),
        }
    }
}

/// One word about one item, for whoever hears of it.
#[derive(Debug, Clone)]
pub struct PlaybackReport {
    pub device: String,
    pub play_id: String,
    /// The request that started it.
    pub request: Option<String>,
    pub item: MediaItem,
    pub news: PlaybackNews,
    pub via_gateway: bool,
}

/// Who hears of the player: the request journal and the plugins (wired in app.rs).
pub trait PlaybackSink: Send + Sync {
    fn report(&self, report: &PlaybackReport);
}

/// How the station is told something nobody asked for: a push of our server action it calls back for.
pub trait Pusher: Send + Sync {
    /// Whether a station of the device is on the line to take it.
    fn push(&self, device: &str, directive: Directive) -> bool;
}

/// The gateway's own way for a stream (gateway/media/stream.rs): the link the station fetches it by, or why not.
pub trait StreamRelay: Send + Sync {
    fn relay(&self, url: &str, media_base: Option<&str>) -> Result<String, String>;
}

pub struct PlaybackWatch {
    me: Weak<PlaybackWatch>,
    player: Arc<MediaPlayer>,
    sink: OnceLock<Arc<dyn PlaybackSink>>,
    pusher: OnceLock<Arc<dyn Pusher>>,
    relay: OnceLock<Arc<dyn StreamRelay>>,
    /// The plays a retry is waiting to answer, by device: the play id that went silent.
    retries: Mutex<Vec<(String, String)>>,
    /// Plays that start after the speech before them (`start_later`), by device: their silence is not counted yet.
    deferred: Mutex<Vec<(String, String)>>,
    silent_after: Duration,
}

impl PlaybackWatch {
    /// The watch over a player: every item it starts is timed.
    pub fn new(player: Arc<MediaPlayer>) -> Arc<PlaybackWatch> {
        PlaybackWatch::with(player, SILENT_AFTER)
    }

    pub fn with(player: Arc<MediaPlayer>, silent_after: Duration) -> Arc<PlaybackWatch> {
        let watch = Arc::new_cyclic(|me| PlaybackWatch {
            me: me.clone(),
            player: player.clone(),
            sink: OnceLock::new(),
            pusher: OnceLock::new(),
            relay: OnceLock::new(),
            retries: Mutex::new(Vec::new()),
            deferred: Mutex::new(Vec::new()),
            silent_after,
        });
        let me = Arc::downgrade(&watch);
        player.on_start(Box::new(move |device, play_id| {
            if let Some(watch) = me.upgrade() {
                watch.started(device, play_id);
            }
        }));
        let me = Arc::downgrade(&watch);
        player.on_replace(Box::new(move |device, replaced| {
            if let Some(watch) = me.upgrade() {
                watch.replaced(device, replaced);
            }
        }));
        watch
    }

    /// Another play took the place of one that went on: its plugin hears it stopped, its request notes it — the
    /// radio's «играет N мин» does not stay under the music that came after it.
    fn replaced(&self, device: &str, replaced: &PlayerState) {
        let Some(item) = replaced.queue.get(replaced.index) else { return };
        self.tell(
            device,
            &replaced.play_id,
            replaced.request.clone(),
            item,
            PlaybackNews::Stopped,
            replaced.via_gateway(),
        );
    }

    pub fn set_sink(&self, sink: Arc<dyn PlaybackSink>) {
        let _ = self.sink.set(sink);
    }

    pub fn set_pusher(&self, pusher: Arc<dyn Pusher>) {
        let _ = self.pusher.set(pusher);
    }

    pub fn set_relay(&self, relay: Arc<dyn StreamRelay>) {
        let _ = self.relay.set(relay);
    }

    /// An item started: the station has `silent_after` to say so.
    fn started(&self, device: &str, play_id: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (me, device, play_id, wait) = (self.me.clone(), device.to_string(), play_id.to_string(), self.silent_after);
        runtime.spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(watch) = me.upgrade() {
                watch.check_silence(&device, &play_id);
            }
        });
    }

    fn check_silence(&self, device: &str, play_id: &str) {
        if self.is_deferred(device, play_id) {
            return;
        }
        let Some((item, state)) = self.player.current(device) else { return };
        if state.play_id != play_id || state.state != PlayState::Starting {
            return;
        }
        let via_gateway = state.via_gateway();
        let seconds = self.silent_after.as_secs().max(1);
        let relayed = (!via_gateway)
            .then(|| self.relay.get())
            .flatten()
            .filter(|_| self.pusher.get().is_some())
            .filter(|_| item.url.starts_with("http://") || item.url.starts_with("https://"))
            .map(|relay| relay.relay(&item.url, self.player.media_base(device).as_deref()));
        let retrying = matches!(relayed, Some(Ok(_)));
        self.tell(
            device,
            play_id,
            state.request.clone(),
            &item,
            PlaybackNews::Silent { seconds, retrying },
            via_gateway,
        );
        match relayed {
            Some(Ok(url)) => {
                if self.player.reroute(device, play_id, &url) {
                    tracing::info!(target: "skills", "{device}: «{}» silent {seconds} s — through the gateway now", item.title);
                    self.retry(device, play_id);
                }
            }
            Some(Err(why)) => {
                tracing::info!(target: "skills", "{device}: «{}» silent {seconds} s; the gateway's way is closed: {why}", item.title)
            }
            None => {}
        }
    }

    /// Plays the current item of `play_id` anew (a new link, a new queue): a push the station calls back for.
    pub fn retry(&self, device: &str, play_id: &str) -> bool {
        let Some(pusher) = self.pusher.get() else { return false };
        self.retries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((device.to_string(), play_id.to_string()));
        let mut payload = Map::new();
        payload.insert(MEDIA_PLAY_ID.into(), json!(play_id));
        let directive = Directive {
            r#type: DirectiveType::ServerAction,
            name: format!("{MEDIA_CALLBACK}{RETRY_CALLBACK}"),
            analytics_type: None,
            payload: Some(payload),
            typed_payload: None,
            is_led_silent: Some(true),
            ignore_answer: None,
        };
        pusher.push(device, directive)
    }

    fn deferred(&self) -> std::sync::MutexGuard<'_, Vec<(String, String)>> {
        self.deferred.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_deferred(&self, device: &str, play_id: &str) -> bool {
        self.deferred().iter().any(|(waiting, id)| waiting == device && id == play_id)
    }

    /// The play `play_id` (already the player's) starts after `delay` — the speech before it: the station is asked
    /// then, through the same callback as a retry, and its silence is counted from there. An answer that says a lot
    /// and then plays (the morning show) is not cut by its own music. False when no station can be asked later.
    pub fn start_later(&self, device: &str, play_id: &str, delay: Duration) -> bool {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return false };
        if self.pusher.get().is_none() {
            return false;
        }
        self.deferred().push((device.to_string(), play_id.to_string()));
        let (me, device, play_id) = (self.me.clone(), device.to_string(), play_id.to_string());
        runtime.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(watch) = me.upgrade() else { return };
            let current = watch.player.current(&device).is_some_and(|(_, state)| state.play_id == play_id);
            if current && watch.is_deferred(&device, &play_id) {
                watch.retry(&device, &play_id);
            } else {
                // The person went on meanwhile («стоп», another request): nothing waits any more.
                watch.deferred().retain(|(waiting, id)| !(*waiting == device && *id == play_id));
            }
        });
        true
    }

    /// The station's callback to the retry: the item anew, if that play still waits for it.
    pub fn retried(&self, device: &str, play_id: &str) -> Option<Vec<Directive>> {
        self.deferred().retain(|(waiting, id)| !(waiting == device && id == play_id));
        let waited = {
            let mut retries = self.retries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = retries.len();
            retries.retain(|(waiting, id)| !(waiting == device && id == play_id));
            retries.len() != before
        };
        let current = self.player.current(device).filter(|(_, state)| state.play_id == play_id);
        if !waited || current.is_none() {
            return None;
        }
        self.player.step(device, 0, 0)
    }

    /// A plugin answered a silence or a failure with what to play instead: the queue in place of this play's, played
    /// now. False when the play is over (the person went on).
    pub fn replace(&self, device: &str, play_id: &str, queue: Vec<MediaItem>) -> bool {
        self.player.replace(device, play_id, queue) && self.retry(device, play_id)
    }

    /// One of the station's callbacks for our player.
    pub fn on_event(&self, device: &str, event: PlayerEvent, play_id: &str) {
        let Some((item, state)) = self.player.current(device) else { return };
        if state.play_id != play_id {
            return;
        }
        let news = match event {
            PlayerEvent::Started => PlaybackNews::Started,
            PlayerEvent::Stopped => PlaybackNews::Stopped,
            PlayerEvent::Finished => PlaybackNews::Finished,
            PlayerEvent::Failed => PlaybackNews::Failed,
        };
        let via_gateway = state.via_gateway();
        self.tell(device, play_id, state.request.clone(), &item, news, via_gateway);
    }

    fn tell(
        &self,
        device: &str,
        play_id: &str,
        request: Option<String>,
        item: &MediaItem,
        news: PlaybackNews,
        via_gateway: bool,
    ) {
        if let Some(sink) = self.sink.get() {
            sink.report(&PlaybackReport {
                device: device.to_string(),
                play_id: play_id.to_string(),
                request,
                item: item.clone(),
                news,
                via_gateway,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, url: &str) -> MediaItem {
        MediaItem {
            id: id.into(),
            title: id.into(),
            url: url.into(),
            format: "MP3".into(),
            codec: None,
            r#type: "FmRadio".into(),
            subtitle: None,
            image_url: None,
            duration_ms: None,
        }
    }

    #[derive(Default)]
    struct Heard(Mutex<Vec<(Option<String>, String)>>);

    impl PlaybackSink for Heard {
        fn report(&self, report: &PlaybackReport) {
            self.0
                .lock()
                .unwrap()
                .push((report.request.clone(), report.news.note(&report.item.title, report.via_gateway)));
        }
    }

    #[derive(Default)]
    struct Pushed(Mutex<Vec<(String, String)>>);

    impl Pusher for Pushed {
        fn push(&self, device: &str, directive: Directive) -> bool {
            self.0.lock().unwrap().push((device.to_string(), directive.name));
            true
        }
    }

    struct Relay;

    impl StreamRelay for Relay {
        fn relay(&self, url: &str, media_base: Option<&str>) -> Result<String, String> {
            if url.contains("aac") {
                return Err("AAC здесь не перекодировать".into());
            }
            Ok(format!("{}/stream/t1", media_base.unwrap_or("http://gw/media")))
        }
    }

    #[tokio::test]
    async fn notes_the_stations_answers_in_the_request_and_takes_the_gateways_way_for_a_silent_stream() {
        let player = Arc::new(MediaPlayer::new());
        let watch = PlaybackWatch::with(player.clone(), Duration::from_millis(30));
        let (heard, pushed) = (Arc::new(Heard::default()), Arc::new(Pushed::default()));
        watch.set_sink(heard.clone());
        watch.set_pusher(pushed.clone());
        watch.set_relay(Arc::new(Relay));
        player.note_request("kitchen", "r1", Some("http://192.0.2.10/media".into()));

        // «Хит FM» starts: the request hears it.
        player.play("kitchen", vec![item("hit", "https://hitfm.example/hit.mp3")], 0, 0);
        let play = player.current("kitchen").unwrap().1.play_id;
        player.on_event("kitchen", PlayerEvent::Started, &play, 1);
        watch.on_event("kitchen", PlayerEvent::Started, &play);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            heard.0.lock().unwrap().as_slice(),
            [(Some("r1".to_string()), "колонка начала играть: hit".to_string())]
        );

        // «Джаз» stays silent: the gateway's way next, the station told through a push.
        player.play("kitchen", vec![item("jazz", "https://jazz.example/jazz-128.mp3")], 0, 0);
        let silent = player.current("kitchen").unwrap().1.play_id;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(heard.0.lock().unwrap().last().unwrap().1.contains("молчит 1 с: jazz — пробую через шлюз"));
        assert_eq!(pushed.0.lock().unwrap().as_slice(), [("kitchen".to_string(), "vgw_media_retry".to_string())]);
        assert_eq!(player.current("kitchen").unwrap().0.url, "http://192.0.2.10/media/stream/t1");
        let again = watch.retried("kitchen", &silent).expect("the station's callback gets the item anew");
        assert_eq!(again[0].payload.as_ref().unwrap()["stream"]["url"], "http://192.0.2.10/media/stream/t1");
        assert!(watch.retried("kitchen", &silent).is_none(), "once");

        // Through the gateway too silent: told, no third way.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(heard.0.lock().unwrap().last().unwrap().1.ends_with("молчит 1 с: jazz (через шлюз)"));
        assert_eq!(pushed.0.lock().unwrap().len(), 1);

        // An AAC stream where it cannot be transcoded: silent, and the plugin will hear it, nothing pushed.
        player.play("kitchen", vec![item("record", "https://record.example/rr_main96.aac")], 0, 0);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(heard.0.lock().unwrap().last().unwrap().1.ends_with("молчит 1 с: record"));
        assert_eq!(pushed.0.lock().unwrap().len(), 1);

        // A plugin's next station in place of the silent one.
        let record = player.current("kitchen").unwrap().1.play_id;
        assert!(watch.replace("kitchen", &record, vec![item("relax", "https://relax.example/r.mp3")]));
        let next = watch.retried("kitchen", &record).unwrap();
        assert_eq!(next[0].payload.as_ref().unwrap()["stream"]["url"], "https://relax.example/r.mp3");
    }

    /// Review J10: another play over the radio — its plugin hears the radio stopped (`player.stopped`), in the radio's
    /// own request; the station's late word of it is not told twice. The same item again is no stop, nor is one that
    /// the station stopped or that ended: those were told already.
    #[tokio::test]
    async fn an_item_another_play_took_the_place_of_is_told_stopped() {
        let player = Arc::new(MediaPlayer::new());
        let watch = PlaybackWatch::with(player.clone(), Duration::from_secs(60));
        let heard = Arc::new(Heard::default());
        watch.set_sink(heard.clone());
        let notes = || heard.0.lock().unwrap().clone();
        let started = |at: u64| {
            let play = player.current("kitchen").unwrap().1.play_id;
            player.on_event("kitchen", PlayerEvent::Started, &play, at);
            play
        };

        // «Хит FM» plays; «включи мою музыку» over it: the radio is told stopped, in the radio's request.
        player.note_request("kitchen", "r1", None);
        player.play("kitchen", vec![item("hit", "https://hitfm.example/hit.mp3")], 0, 0);
        let radio = started(1);
        player.note_request("kitchen", "r2", None);
        player.play("kitchen", vec![item("a", "http://gw/media/a.mp3"), item("b", "http://gw/media/b.mp3")], 0, 0);
        assert_eq!(notes(), [(Some("r1".to_string()), "колонка остановила: hit".to_string())]);
        watch.on_event("kitchen", PlayerEvent::Stopped, &radio);
        assert_eq!(notes().len(), 1, "the station's late word of it is not told again");

        // A track not started yet is no stop; a started one that «дальше» leaves is.
        assert!(player.step("kitchen", 1, 0).is_some());
        assert_eq!(notes().len(), 1);
        started(2);
        assert!(player.step("kitchen", -1, 0).is_some());
        assert_eq!(notes().last().unwrap().1, "колонка остановила: b");

        // The same item again — «сначала», a retry — goes on: no stop.
        started(3);
        assert!(player.step("kitchen", 0, 0).is_some());
        assert_eq!(notes().len(), 2);

        // Paused by us and stopped by the station: told by its own word, not again by the next play.
        let paused = started(4);
        player.stopping("kitchen", 5);
        player.on_event("kitchen", PlayerEvent::Stopped, &paused, 6);
        player.play("kitchen", vec![item("hit", "https://hitfm.example/hit.mp3")], 0, 0);
        assert_eq!(notes().len(), 2);
    }

    #[tokio::test]
    async fn starts_the_music_after_the_speech_before_it_and_counts_its_silence_from_there() {
        let player = Arc::new(MediaPlayer::new());
        let watch = PlaybackWatch::with(player.clone(), Duration::from_millis(30));
        let (heard, pushed) = (Arc::new(Heard::default()), Arc::new(Pushed::default()));
        watch.set_sink(heard.clone());
        watch.set_pusher(pushed.clone());
        watch.set_relay(Arc::new(Relay));

        // The morning show speaks first: its music waits, and the wait is no silence.
        player.play("kitchen", vec![item("fire", "https://music.example/fire.mp3")], 0, 0);
        let play = player.current("kitchen").unwrap().1.play_id;
        assert!(watch.start_later("kitchen", &play, Duration::from_millis(80)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(heard.0.lock().unwrap().is_empty() && pushed.0.lock().unwrap().is_empty());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(pushed.0.lock().unwrap().as_slice(), [("kitchen".to_string(), "vgw_media_retry".to_string())]);
        let now = watch.retried("kitchen", &play).expect("the station's callback gets the music");
        assert_eq!(now[0].payload.as_ref().unwrap()["stream"]["url"], "https://music.example/fire.mp3");
        // From here its silence counts.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(heard.0.lock().unwrap().last().unwrap().1.contains("молчит 1 с: fire"));

        // «Стоп» during the speech: nothing is asked later.
        player.play("kitchen", vec![item("sea", "https://music.example/sea.mp3")], 0, 0);
        let play = player.current("kitchen").unwrap().1.play_id;
        assert!(watch.start_later("kitchen", &play, Duration::from_millis(30)));
        player.play("kitchen", vec![item("other", "https://music.example/other.mp3")], 0, 0);
        let pushes = pushed.0.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(pushed.0.lock().unwrap().len() <= pushes + 1, "the other play may be retried, not the sea");
        assert!(watch.retried("kitchen", &play).is_none());
    }
}
