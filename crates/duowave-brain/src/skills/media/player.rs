//! Our player: audio_play the way Yandex sends it, with our links and our callbacks, and what plays per device.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::media_types::MediaItem;
use crate::ear::Command;
use crate::utils::time::now_ms;

/// Our callbacks (OUR_CALLBACK_PREFIX + `media_`): the station returns them as server_action requests when
/// playback starts, stops, ends, fails.
pub const MEDIA_CALLBACK: &str = "vgw_media_";

/// The station's request for the next track: a server_action that comes with a track. The station sends it back
/// 30 s into the track and queues the audio_play of the answer after it (journals of 01.11: «Звезда» → «МЫ» did
/// not start at once); when the answer was lost, it asks again at the end and plays the answer then. Yandex's
/// name; ours carry our play id in the payload. Once answered with nothing, it is not asked again (live, 02.11).
pub const STACK_GET_NEXT: &str = "@@mm_stack_engine_get_next";
pub const MEDIA_PLAY_ID: &str = "vgw_play_id";

/// Whether the station plays our links: audio_play is the command Yandex sends, and with links of ours (a radio
/// station's HLS, MP3 from /media) the station played them (live, 02.11). So our media serves wherever its route
/// puts it first.
pub const OUR_AUDIO_PLAY_VERIFIED: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    Started,
    Stopped,
    Finished,
    Failed,
}

impl PlayerEvent {
    pub fn parse(name: &str) -> Option<PlayerEvent> {
        match name {
            "started" => Some(PlayerEvent::Started),
            "stopped" => Some(PlayerEvent::Stopped),
            "finished" => Some(PlayerEvent::Finished),
            "failed" => Some(PlayerEvent::Failed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlayerEvent::Started => "started",
            PlayerEvent::Stopped => "stopped",
            PlayerEvent::Finished => "finished",
            PlayerEvent::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Starting,
    Playing,
    Stopped,
    Finished,
    Failed,
}

/// The next item, given to the station's queue: it becomes current when the station starts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub index: usize,
    pub play_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub queue: Vec<MediaItem>,
    pub index: usize,
    pub play_id: String,
    pub state: PlayState,
    /// Milliseconds since the epoch of the last change.
    pub at: u64,
    /// Where in the item this play began.
    pub offset_ms: u64,
    /// When the station said it started.
    pub started_at: Option<u64>,
    /// When we last stopped or paused it ourselves.
    pub stop_asked_at: Option<u64>,
    pub queued: Option<Queued>,
    /// The request it was started for: what the station says of it goes into that request's journal (stage 5а).
    pub request: Option<String>,
    /// Where the items added to the queue end («добавь в очередь», a plugin's `add`): the next added go here, after
    /// the earlier ones and before the rest of the queue. Kept while the queue stays the same.
    pub added_until: usize,
}

impl PlayerState {
    /// Whether the item plays through the gateway's own relay (`/media/stream/…`): the fallback was taken already.
    pub fn via_gateway(&self) -> bool {
        self.queue.get(self.index).is_some_and(|item| item.url.contains(RELAY_PATH))
    }

    /// Where in the item it stands by our own count (ms): where this play began, and how long it played since the
    /// station said it started — until `now` while it plays, until it stopped otherwise (ours or the station's
    /// stop). Within the item's length. The panel has no word from the station between requests: its pause, its
    /// «продолжить» and its widget go by this.
    pub fn position_ms(&self, now: u64) -> u64 {
        let until = match self.state {
            PlayState::Starting | PlayState::Playing => now,
            PlayState::Stopped | PlayState::Finished | PlayState::Failed => self.at,
        };
        let played = self.started_at.map_or(0, |started| until.saturating_sub(started));
        let position = self.offset_ms.saturating_add(played);
        match self.queue.get(self.index).and_then(|item| item.duration_ms) {
            Some(length) => position.min(length),
            None => position,
        }
    }
}

/// Where the gateway relays a stream the station kept silent on (gateway/media/stream.rs).
pub const RELAY_PATH: &str = "/media/stream/";

/// Told when an item is started: the device and its play id (the playback watch times the station's answer).
pub type StartHook = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Told when a new play takes the place of one that still played (the station said it started) with another item: the
/// device and the play replaced. The station's own word of it comes for a play that is no longer the current one and
/// is not heard, so its plugin hears it stopped from here (`player.stopped`).
pub type ReplaceHook = Box<dyn Fn(&str, &PlayerState) + Send + Sync>;

/// A stop this close to the item's end that we did not ask for is its end.
const END_SLACK_MS: i64 = 3000;
/// A stop this soon after ours is ours.
const OUR_STOP_MS: i64 = 3000;

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// What our player plays per device: the queue, the item, and what the station's callbacks said about it.
#[derive(Default)]
pub struct MediaPlayer {
    players: Mutex<HashMap<String, PlayerState>>,
    plays: AtomicU64,
    /// Per device: the request going on it last and where its station fetches our /media.
    requests: Mutex<HashMap<String, (String, Option<String>)>>,
    started: OnceLock<StartHook>,
    replaced: OnceLock<ReplaceHook>,
}

impl MediaPlayer {
    pub fn new() -> MediaPlayer {
        MediaPlayer::default()
    }

    /// The request going on a device now, and where its station fetches our /media: what the player starts from
    /// here on is that request's.
    pub fn note_request(&self, device: &str, request: &str, media_base: Option<String>) {
        self.requests.lock().unwrap().insert(device.to_string(), (request.to_string(), media_base));
    }

    /// Where a device's station fetches our /media, as its last request said.
    pub fn media_base(&self, device: &str) -> Option<String> {
        self.requests.lock().unwrap().get(device).and_then(|(_, base)| base.clone())
    }

    /// Who is told of every item started (the playback watch); once.
    pub fn on_start(&self, hook: StartHook) {
        let _ = self.started.set(hook);
    }

    /// Who is told of a play that a new one took the place of while it went on (the playback watch); once.
    pub fn on_replace(&self, hook: ReplaceHook) {
        let _ = self.replaced.set(hook);
    }

    /// audio_play, and for a track (not a live radio) the request for the next one.
    pub fn play(&self, device: &str, queue: Vec<MediaItem>, index: usize, offset_ms: u64) -> Vec<Command> {
        let (directives, replaced) = {
            let mut players = self.players.lock().unwrap();
            self.start(&mut players, device, queue, index, offset_ms)
        };
        self.tell_replaced(device, replaced);
        directives
    }

    /// The play a new one took the place of, told outside the players' lock.
    fn tell_replaced(&self, device: &str, replaced: Option<PlayerState>) {
        if let (Some(hook), Some(replaced)) = (self.replaced.get(), replaced) {
            hook(device, &replaced);
        }
    }

    /// Starts the item: the directives, and the play it took the place of when that one still played another item —
    /// the same item again (a retry, «продолжить») goes on as it was, and one stopped or ended was told so already.
    fn start(
        &self,
        players: &mut HashMap<String, PlayerState>,
        device: &str,
        queue: Vec<MediaItem>,
        index: usize,
        offset_ms: u64,
    ) -> (Vec<Command>, Option<PlayerState>) {
        let Some(item) = queue.get(index).cloned() else { return (Vec::new(), None) };
        let play_id = self.new_play_id();
        let request = self.requests.lock().unwrap().get(device).map(|(request, _)| request.clone());
        // The same queue going on (the next item, «дальше», «продолжи»): its added items keep their place.
        let added_until = players
            .get(device)
            .filter(|before| same_queue(&before.queue, &queue))
            .map_or(0, |before| before.added_until);
        let state = PlayerState {
            queue,
            index,
            play_id: play_id.clone(),
            state: PlayState::Starting,
            at: now_ms(),
            offset_ms,
            started_at: None,
            stop_asked_at: None,
            queued: None,
            request,
            added_until,
        };
        let replaced = players.insert(device.to_string(), state).filter(|before| {
            before.state == PlayState::Playing && before.queue.get(before.index).is_some_and(|was| was.id != item.id)
        });
        if let Some(hook) = self.started.get() {
            hook(device, &play_id);
        }
        (vec![Command::Play { item, play_id, offset_ms }], replaced)
    }

    /// The item of this play goes another way (the gateway's relay): its link changes; true when it still plays.
    pub fn reroute(&self, device: &str, play_id: &str, url: &str) -> bool {
        let mut players = self.players.lock().unwrap();
        let Some(state) = players.get_mut(device).filter(|state| state.play_id == play_id) else { return false };
        let index = state.index;
        match state.queue.get_mut(index) {
            Some(item) => {
                item.url = url.to_string();
                true
            }
            None => false,
        }
    }

    /// Another queue in place of this play's (a plugin chose the next station): true when it still played.
    pub fn replace(&self, device: &str, play_id: &str, queue: Vec<MediaItem>) -> bool {
        let mut players = self.players.lock().unwrap();
        let Some(state) = players.get_mut(device).filter(|state| state.play_id == play_id) else { return false };
        if queue.is_empty() {
            return false;
        }
        state.queue = queue;
        state.index = 0;
        state.offset_ms = 0;
        state.added_until = 0;
        true
    }

    /// Items into the queue of what plays («добавь в очередь»): after the item that plays, after the one the station
    /// was given to play next and after the items added before — the rest of the queue (a mix that goes on by itself)
    /// moves down. False when nothing of ours plays or stands paused on the device: play them instead.
    pub fn add(&self, device: &str, items: Vec<MediaItem>) -> bool {
        let mut players = self.players.lock().unwrap();
        let Some(state) = players.get_mut(device) else { return false };
        let going = match state.state {
            PlayState::Starting | PlayState::Playing => true,
            PlayState::Stopped => state.stop_asked_at.is_some(),
            PlayState::Finished | PlayState::Failed => false,
        };
        if !going || items.is_empty() {
            return false;
        }
        let after_queued = state.queued.as_ref().map_or(0, |queued| queued.index + 1);
        let at = (state.index + 1).max(after_queued).max(state.added_until).min(state.queue.len());
        let count = items.len();
        state.queue.splice(at..at, items);
        state.added_until = at + count;
        true
    }

    /// We stop or pause what plays: its on_stopped is ours, not the end of the item; clear_queue drops the next.
    pub fn stopping(&self, device: &str, now: u64) {
        if let Some(state) = self.players.lock().unwrap().get_mut(device) {
            state.stop_asked_at = Some(now);
            state.queued = None;
        }
    }

    /// The item it plays and the player's state.
    pub fn current(&self, device: &str) -> Option<(MediaItem, PlayerState)> {
        let players = self.players.lock().unwrap();
        let state = players.get(device)?;
        Some((state.queue.get(state.index)?.clone(), state.clone()))
    }

    /// A callback of ours. The station ends a track with on_stopped: a stop we did not ask for, when the item has
    /// played to its length, is its end. on_finished (never seen) brings the next track at once.
    pub fn on_event(&self, device: &str, event: PlayerEvent, play_id: &str, now: u64) -> Option<Vec<Command>> {
        let mut players = self.players.lock().unwrap();
        let state = players.get_mut(device)?;
        if event == PlayerEvent::Started {
            if let Some(queued) = state.queued.take_if(|queued| queued.play_id == play_id) {
                // The station went on to the queued item.
                state.index = queued.index;
                state.play_id = queued.play_id;
                state.offset_ms = 0;
                state.stop_asked_at = None;
            }
        }
        if state.play_id != play_id {
            return None;
        }
        let ended = event == PlayerEvent::Finished || (event == PlayerEvent::Stopped && played_to_end(state, now));
        state.state = match event {
            PlayerEvent::Started => PlayState::Playing,
            _ if ended => PlayState::Finished,
            PlayerEvent::Stopped => PlayState::Stopped,
            PlayerEvent::Failed => PlayState::Failed,
            PlayerEvent::Finished => PlayState::Finished,
        };
        state.at = now;
        if event == PlayerEvent::Started {
            state.started_at = Some(now);
        }
        if event == PlayerEvent::Finished && state.index + 1 < state.queue.len() {
            let (queue, next) = (state.queue.clone(), state.index + 1);
            let (directives, replaced) = self.start(&mut players, device, queue, next, 0);
            drop(players);
            self.tell_replaced(device, replaced);
            return Some(directives);
        }
        None
    }

    /// The station asks for the next track (STACK_GET_NEXT): the next item of the queue, which the station plays
    /// after this one (at once, when this one has ended); nothing at the end of the queue.
    pub fn next(&self, device: &str, play_id: &str) -> Option<Vec<Command>> {
        let mut players = self.players.lock().unwrap();
        let state = players.get_mut(device)?;
        if state.play_id != play_id || state.index + 1 >= state.queue.len() {
            return None;
        }
        let queued = Queued { index: state.index + 1, play_id: self.new_play_id() };
        let item = state.queue[queued.index].clone();
        let play_id = queued.play_id.clone();
        state.queued = Some(queued);
        Some(vec![Command::Play { item, play_id, offset_ms: 0 }])
    }

    /// The item `delta` away in the queue (0: this one again) from `offset_ms`; None past either end.
    pub fn step(&self, device: &str, delta: i64, offset_ms: u64) -> Option<Vec<Command>> {
        let mut players = self.players.lock().unwrap();
        let state = players.get(device)?;
        let index = state.index as i64 + delta;
        if delta != 0 && (index < 0 || index >= state.queue.len() as i64) {
            return None;
        }
        let queue = state.queue.clone();
        let (directives, replaced) = self.start(&mut players, device, queue, index.max(0) as usize, offset_ms);
        drop(players);
        self.tell_replaced(device, replaced);
        Some(directives)
    }

    fn new_play_id(&self) -> String {
        format!("p{}{}", base36(now_ms()), base36(self.plays.fetch_add(1, Ordering::Relaxed)))
    }
}

/// The same queue by its items (a relayed item's link may have changed).
fn same_queue(before: &[MediaItem], now: &[MediaItem]) -> bool {
    before.len() == now.len() && before.iter().zip(now).all(|(before, now)| before.id == now.id)
}

fn played_to_end(state: &PlayerState, now: u64) -> bool {
    let Some(duration) = state.queue.get(state.index).and_then(|item| item.duration_ms).filter(|d| *d > 0) else {
        return false;
    };
    let Some(started_at) = state.started_at else { return false };
    let now = now as i64;
    if state.stop_asked_at.is_some_and(|asked| now - (asked as i64) < OUR_STOP_MS) {
        return false;
    }
    state.offset_ms as i64 + (now - started_at as i64) >= duration as i64 - END_SLACK_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::callbacks::OUR_CALLBACK_PREFIX;

    fn item(id: &str, duration_ms: Option<u64>) -> MediaItem {
        MediaItem {
            id: id.into(),
            title: id.into(),
            url: format!("http://gateway.local/media/{id}.mp3"),
            format: "MP3".into(),
            codec: None,
            r#type: "Track".into(),
            subtitle: None,
            image_url: None,
            duration_ms,
        }
    }

    fn play_id_of(command: &Command) -> String {
        command.played().unwrap().1.to_string()
    }

    /// What the ear is asked to play, as the Station's audio_play names it (src/gateway/commands/player.rs).
    fn stream_of(command: &Command) -> serde_json::Value {
        let (item, _, offset_ms) = command.played().unwrap();
        serde_json::json!({ "id": item.id, "url": item.url, "offset_ms": offset_ms, "duration_ms": item.duration_ms })
    }

    #[test]
    fn gives_the_station_the_next_track_when_it_asks_30_s_into_one_and_goes_on_to_it_when_the_station_does() {
        assert!(MEDIA_CALLBACK.starts_with(OUR_CALLBACK_PREFIX));
        let player = MediaPlayer::new();
        let queue = vec![item("a", Some(81_720)), item("b", Some(81_720)), item("c", Some(81_720))];
        let t0 = 1_000_000;
        // A track goes with Yandex's request for the next one, our play id in it.
        let directives = player.play("h:test", queue, 0, 0);
        let first = play_id_of(&directives[0]);
        // The ear is told to play; its adapter asks for the next one (src/gateway/commands/player.rs).
        assert_eq!(directives.len(), 1);
        player.on_event("h:test", PlayerEvent::Started, &first, t0);
        // 30 s in the station asks (live, 02.11): the next track for its queue, with its own request.
        let next = player.next("h:test", &first).unwrap();
        assert_eq!(stream_of(&next[0])["id"], "b");
        assert_eq!(stream_of(&next[0])["duration_ms"], 81_720);
        let second = play_id_of(&next[0]);
        assert_eq!(next.len(), 1, "the ear's adapter asks for the next one itself");
        // The first still plays: it is current until the station starts the second.
        assert_eq!(player.current("h:test").unwrap().0.id, "a");
        player.on_event("h:test", PlayerEvent::Stopped, &first, t0 + 81_800);
        player.on_event("h:test", PlayerEvent::Started, &second, t0 + 82_000);
        let (current, state) = player.current("h:test").unwrap();
        assert_eq!(
            (current.id.as_str(), state.state, state.play_id.as_str()),
            ("b", PlayState::Playing, second.as_str())
        );
        // A request of an old play gets nothing; the last item has no next.
        assert!(player.next("h:test", &first).is_none());
        let third = play_id_of(&player.next("h:test", &second).unwrap()[0]);
        player.on_event("h:test", PlayerEvent::Started, &third, t0 + 200_000);
        assert!(player.next("h:test", &third).is_none());
    }

    #[test]
    fn adds_to_the_queue_after_what_plays_the_queued_next_and_the_items_added_before() {
        let player = MediaPlayer::new();
        let ids = |player: &MediaPlayer| -> Vec<String> {
            player.current("h:test").unwrap().1.queue.iter().map(|item| item.id.clone()).collect()
        };
        assert!(!player.add("h:test", vec![item("x", None)]), "nothing plays: play it instead");
        let directives = player.play("h:test", vec![item("a", None), item("m1", None), item("m2", None)], 0, 0);
        let first = play_id_of(&directives[0]);
        player.on_event("h:test", PlayerEvent::Started, &first, 1_000);
        assert!(player.add("h:test", vec![item("x", None)]));
        assert!(player.add("h:test", vec![item("y", None)]));
        assert_eq!(ids(&player), ["a", "x", "y", "m1", "m2"], "in the order asked, before the mix");
        // The station was given x to play next; then z is added — after the earlier ones.
        let queued = play_id_of(&player.next("h:test", &first).unwrap()[0]);
        player.on_event("h:test", PlayerEvent::Started, &queued, 100_000);
        assert_eq!(player.current("h:test").unwrap().0.id, "x");
        assert!(player.add("h:test", vec![item("z", None)]));
        assert_eq!(ids(&player), ["a", "x", "y", "z", "m1", "m2"]);
        // «Дальше» keeps the place of the added ones: the next added goes after z.
        let directives = player.step("h:test", 1, 0).unwrap();
        player.on_event("h:test", PlayerEvent::Started, &play_id_of(&directives[0]), 120_000);
        assert!(player.add("h:test", vec![item("w", None)]));
        assert_eq!(ids(&player), ["a", "x", "y", "z", "w", "m1", "m2"]);
        // Given to the station already, the next one stays next: the added goes after it.
        let player = MediaPlayer::new();
        let directives = player.play("h:test", vec![item("a", None), item("m1", None), item("m2", None)], 0, 0);
        let first = play_id_of(&directives[0]);
        player.on_event("h:test", PlayerEvent::Started, &first, 1_000);
        player.next("h:test", &first).unwrap();
        assert!(player.add("h:test", vec![item("x", None)]));
        assert_eq!(ids(&player), ["a", "m1", "x", "m2"]);
        // A new play is a new queue: nothing added in it yet.
        player.play("h:test", vec![item("b", None), item("n1", None)], 0, 0);
        assert!(player.add("h:test", vec![item("y", None)]));
        assert_eq!(ids(&player), ["b", "y", "n1"]);
    }

    #[test]
    fn drops_the_queued_track_when_we_stop_clear_queue_drops_it_on_the_station_too() {
        let player = MediaPlayer::new();
        let directives = player.play("h:test", vec![item("a", None), item("b", None)], 0, 0);
        let first = play_id_of(&directives[0]);
        player.on_event("h:test", PlayerEvent::Started, &first, 0);
        let second = play_id_of(&player.next("h:test", &first).unwrap()[0]);
        player.stopping("h:test", now_ms());
        // A late start of the dropped one does not take over.
        player.on_event("h:test", PlayerEvent::Started, &second, 1000);
        assert_eq!(player.current("h:test").unwrap().0.id, "a");
    }

    #[test]
    fn ends_a_track_by_a_stop_at_its_length_but_not_by_ours() {
        let player = MediaPlayer::new();
        let directives = player.play("h:test", vec![item("a", Some(10_000)), item("b", Some(10_000))], 0, 0);
        let first = play_id_of(&directives[0]);
        player.on_event("h:test", PlayerEvent::Started, &first, 1_000);
        player.stopping("h:test", 9_000);
        player.on_event("h:test", PlayerEvent::Stopped, &first, 10_000);
        assert_eq!(player.current("h:test").unwrap().1.state, PlayState::Stopped);
        player.on_event("h:test", PlayerEvent::Started, &first, 20_000);
        player.on_event("h:test", PlayerEvent::Stopped, &first, 29_000);
        assert_eq!(player.current("h:test").unwrap().1.state, PlayState::Finished);
        let next = player.on_event("h:test", PlayerEvent::Finished, &first, 29_500).unwrap();
        assert_eq!(stream_of(&next[0])["id"], "b");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }

    /// Review G5: between requests the panel has only our count — where the play began and how long it played.
    #[test]
    fn counts_where_the_item_stands_while_it_plays_and_where_it_stopped() {
        let player = MediaPlayer::new();
        let directives = player.play("h:test", vec![item("a", Some(200_000))], 0, 30_000);
        let play = play_id_of(&directives[0]);
        assert_eq!(player.current("h:test").unwrap().1.position_ms(5_000_000), 30_000, "not started yet");
        player.on_event("h:test", PlayerEvent::Started, &play, 1_000_000);
        assert_eq!(player.current("h:test").unwrap().1.position_ms(1_130_000), 160_000);
        // Paused by us at 2:10 into the play: it stands there, however long the pause.
        player.stopping("h:test", 1_130_000);
        player.on_event("h:test", PlayerEvent::Stopped, &play, 1_130_400);
        let state = player.current("h:test").unwrap().1;
        assert_eq!((state.state, state.position_ms(9_000_000)), (PlayState::Stopped, 160_400));
        // Never past the item's end.
        player.on_event("h:test", PlayerEvent::Started, &play, 2_000_000);
        assert_eq!(player.current("h:test").unwrap().1.position_ms(9_000_000), 200_000);
    }
}
