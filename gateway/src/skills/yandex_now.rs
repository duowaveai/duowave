//! What Yandex plays on a station, live (docs/plans/speaker-alone.md, stage 2): its tracks as its own answers bring
//! them (`audio_play`: title, artist, cover, length — they pass through the gateway anyway, decision 4 of 16.11), and
//! when the station's player starts and stops them (its `music_thin_client_on_*` callbacks come to us between
//! requests). The panel's «Музыка» then follows a track change by itself, not only when someone speaks.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

/// A track (or a radio, a fairy tale…) as Yandex's `audio_play` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YandexTrack {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    /// Track, FmRadio, …
    pub kind: Option<String>,
    pub duration_ms: Option<u64>,
}

/// The `audio_play` directives of a Yandex response (`response.directives[]`, the payload an object or JSON text).
pub fn tracks_of(response: &Value) -> Vec<YandexTrack> {
    let Some(directives) = response.get("response").and_then(|body| body.get("directives")).and_then(Value::as_array)
    else {
        return Vec::new();
    };
    directives
        .iter()
        .filter(|directive| directive.get("name").and_then(Value::as_str) == Some("audio_play"))
        .filter_map(|directive| {
            let payload = match directive.get("payload") {
                Some(Value::String(text)) => serde_json::from_str(text).ok()?,
                Some(value) => value.clone(),
                None => return None,
            };
            track_of(&payload)
        })
        .collect()
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_string)
}

fn track_of(payload: &Value) -> Option<YandexTrack> {
    let stream = payload.get("stream");
    let metadata = payload.get("metadata");
    let title = text(metadata.and_then(|metadata| metadata.get("title")))?;
    let id = text(stream.and_then(|stream| stream.get("id")))
        .or_else(|| text(metadata.and_then(|metadata| metadata.pointer("/glagol_metadata/music_metadata/id"))))
        .unwrap_or_else(|| title.clone());
    // Where Yandex puts the length is not certain for its music (live 16.11: a song came without one): the stream's,
    // else the metadata's, either spelling.
    let duration_ms = [stream, metadata, Some(payload)]
        .into_iter()
        .flatten()
        .flat_map(|object| ["duration_ms", "durationMs"].into_iter().filter_map(move |key| object.get(key)))
        .find_map(|value| value.as_u64().or_else(|| value.as_str().and_then(|text| text.parse().ok())))
        // A radio's «endless» length is no length.
        .filter(|ms| *ms > 0 && *ms < 2_000_000_000);
    Some(YandexTrack {
        id,
        title,
        subtitle: text(metadata.and_then(|metadata| metadata.get("subtitle"))),
        image_url: text(metadata.and_then(|metadata| metadata.get("art_image_url"))).map(|url| cover(&url)),
        kind: text(stream.and_then(|stream| stream.get("type"))),
        duration_ms,
    })
}

/// Yandex's cover addresses come without a scheme and with `%%` for the size: `avatars.yandex.net/…/%%`.
pub fn cover(url: &str) -> String {
    let sized = url.replace("%%", "400x400");
    if sized.starts_with("http://") || sized.starts_with("https://") {
        sized
    } else {
        format!("https://{}", sized.trim_start_matches('/'))
    }
}

/// A track's id in a player callback's payload (`playAudioEvent.trackId`, or any `trackId`/`track_id`), when it says.
pub fn track_id_in(payload: &Value) -> Option<String> {
    match payload {
        Value::Object(map) => {
            for key in ["trackId", "track_id"] {
                if let Some(id) = text(map.get(key)) {
                    return Some(id);
                }
            }
            map.values().find_map(track_id_in)
        }
        Value::Array(items) => items.iter().find_map(track_id_in),
        Value::String(text) if text.trim_start().starts_with('{') => {
            serde_json::from_str::<Value>(text).ok().as_ref().and_then(track_id_in)
        }
        _ => None,
    }
}

/// What Yandex plays on one station now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YandexPlaying {
    pub track: YandexTrack,
    pub playing: bool,
    /// Where it stood when `at` was noted, ms.
    pub offset_ms: u64,
    /// Unix ms of the last start or stop.
    pub at: u64,
    /// Unix ms when this track (a radio) started.
    pub since: u64,
}

impl YandexPlaying {
    pub fn position_ms(&self, now: u64) -> u64 {
        let position = if self.playing { self.offset_ms + now.saturating_sub(self.at) } else { self.offset_ms };
        self.track.duration_ms.map_or(position, |duration| position.min(duration))
    }
}

#[derive(Default)]
struct Station {
    /// The tracks Yandex sent lately, newest last: the next one the station plays is among them.
    known: Vec<YandexTrack>,
    now: Option<YandexPlaying>,
}

const KEEP: usize = 8;

#[derive(Default)]
pub struct YandexNow {
    stations: Mutex<HashMap<String, Station>>,
}

impl YandexNow {
    pub fn new() -> YandexNow {
        YandexNow::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Station>> {
        self.stations.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Yandex sent these tracks to the station (an answer, the next track of its queue). True: something changed.
    pub fn note_tracks(&self, device: &str, tracks: &[YandexTrack]) -> bool {
        if tracks.is_empty() {
            return false;
        }
        let mut stations = self.lock();
        let station = stations.entry(device.to_string()).or_default();
        for track in tracks {
            station.known.retain(|known| known.id != track.id);
            station.known.push(track.clone());
        }
        let excess = station.known.len().saturating_sub(KEEP);
        station.known.drain(..excess);
        // The one playing now got its details later (a cover after a radio's start): take them.
        if let Some(now) = station.now.as_mut() {
            if let Some(fresh) = tracks.iter().find(|track| track.id == now.track.id) {
                let changed = now.track != *fresh;
                now.track = fresh.clone();
                return changed;
            }
        }
        false
    }

    /// The station's player said it started (`started`) or stopped Yandex's content; `track_id` when its callback
    /// names it. True: what plays changed.
    pub fn note_player(&self, device: &str, started: bool, finished: bool, track_id: Option<&str>, now: u64) -> bool {
        let mut stations = self.lock();
        let station = stations.entry(device.to_string()).or_default();
        if started {
            let same = station.now.as_ref().filter(|current| track_id.is_none_or(|id| id == current.track.id));
            let track = match track_id {
                Some(id) => station.known.iter().find(|known| known.id == id).cloned(),
                None => None,
            }
            .or_else(|| same.map(|current| current.track.clone()))
            // Unnamed: the track Yandex sent last is the one the station plays next.
            .or_else(|| station.known.last().cloned());
            let Some(track) = track else { return false };
            let (offset_ms, since) = match station.now.as_ref().filter(|current| current.track.id == track.id) {
                // «продолжи»: the same track from where it stood.
                Some(current) if !current.playing => (current.offset_ms, current.since),
                Some(current) => (current.position_ms(now), current.since),
                None => (0, now),
            };
            station.now = Some(YandexPlaying { track, playing: true, offset_ms, at: now, since });
            return true;
        }
        let Some(current) = station.now.as_mut() else { return false };
        if track_id.is_some_and(|id| id != current.track.id) {
            return false;
        }
        if finished {
            station.now = None;
            return true;
        }
        if !current.playing {
            return false;
        }
        current.offset_ms = current.position_ms(now);
        current.playing = false;
        current.at = now;
        true
    }

    pub fn get(&self, device: &str) -> Option<YandexPlaying> {
        self.lock().get(device).and_then(|station| station.now.clone())
    }

    /// A track Yandex sent, by its id: the station's own word names only the id and the title.
    pub fn known(&self, device: &str, id: &str) -> Option<YandexTrack> {
        self.lock().get(device).and_then(|station| station.known.iter().find(|known| known.id == id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response(id: &str, title: &str) -> Value {
        json!({ "response": { "directives": [
            { "name": "tts_play_placeholder" },
            { "name": "audio_play", "payload": {
                "stream": { "id": id, "type": "Track", "duration_ms": 215_000 },
                "metadata": { "title": title, "subtitle": "Кино", "art_image_url": "avatars.yandex.net/get-music-content/1/a/%%" },
            } },
        ] } })
    }

    #[test]
    fn reads_the_track_out_of_audio_play_with_a_whole_cover_address() {
        let tracks = tracks_of(&response("123", "Группа крови"));
        assert_eq!(
            tracks,
            vec![YandexTrack {
                id: "123".into(),
                title: "Группа крови".into(),
                subtitle: Some("Кино".into()),
                image_url: Some("https://avatars.yandex.net/get-music-content/1/a/400x400".into()),
                kind: Some("Track".into()),
                duration_ms: Some(215_000),
            }]
        );
        let text = json!({ "response": { "directives": [
            { "name": "audio_play", "payload": "{\"stream\":{\"id\":\"r1\",\"type\":\"FmRadio\",\"duration_ms\":2147483647},\"metadata\":{\"title\":\"Радио\"}}" },
        ] } });
        let radio = &tracks_of(&text)[0];
        assert_eq!((radio.kind.as_deref(), radio.duration_ms), (Some("FmRadio"), None), "a radio has no length");
        let elsewhere = json!({ "response": { "directives": [
            { "name": "audio_play", "payload": { "stream": { "id": "7" }, "metadata": { "title": "Песня", "durationMs": "180000" } } },
        ] } });
        assert_eq!(tracks_of(&elsewhere)[0].duration_ms, Some(180_000), "the metadata's length, a string");
    }

    #[test]
    fn follows_a_track_change_a_pause_and_a_resume_between_requests() {
        let now = YandexNow::new();
        now.note_tracks("kitchen", &tracks_of(&response("1", "Группа крови")));
        assert!(now.note_player("kitchen", true, false, None, 1_000), "the track sent last starts");
        assert_eq!(now.get("kitchen").unwrap().track.title, "Группа крови");
        // The queue's next track, then the station plays it.
        now.note_tracks("kitchen", &tracks_of(&response("2", "Кукушка")));
        assert!(now.note_player("kitchen", false, true, Some("1"), 200_000), "the first one finished");
        assert!(now.get("kitchen").is_none());
        assert!(now.note_player("kitchen", true, false, Some("2"), 201_000));
        assert_eq!(now.get("kitchen").unwrap().track.title, "Кукушка");
        // Paused at 30 s, resumed later from there.
        assert!(now.note_player("kitchen", false, false, None, 231_000));
        let paused = now.get("kitchen").unwrap();
        assert_eq!((paused.playing, paused.position_ms(500_000)), (false, 30_000));
        assert!(now.note_player("kitchen", true, false, None, 600_000));
        assert_eq!(now.get("kitchen").unwrap().position_ms(610_000), 40_000);
    }

    #[test]
    fn finds_the_track_id_in_a_callback_however_deep() {
        let payload = json!({ "events": [{ "playAudioEvent": { "trackId": "42", "playId": "p" } }] });
        assert_eq!(track_id_in(&payload).as_deref(), Some("42"));
        assert_eq!(track_id_in(&json!({ "nothing": 1 })), None);
    }
}
