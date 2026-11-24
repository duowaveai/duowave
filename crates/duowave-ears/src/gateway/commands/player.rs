//! The player's commands on the Station: audio_play the way Yandex sends it, with our links and our callbacks, and the
//! request for the next track that goes with a track (src/skills/media/player.rs keeps what plays).

use serde_json::{json, Map, Value};

use crate::ear::MediaItem;
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};
use crate::skills::media::playback::RETRY_CALLBACK;
use crate::skills::media::player::{PlayerEvent, MEDIA_CALLBACK, MEDIA_PLAY_ID, STACK_GET_NEXT};

fn callback(event: PlayerEvent, play_id: &str, item: &MediaItem) -> Value {
    json!({
        "name": format!("{MEDIA_CALLBACK}on_{}", event.as_str()),
        "type": "server_action",
        // The station ends a track with on_stopped, never on_finished, and plays nothing of the answer to
        // on_stopped (live, 02.11): the next track goes in the answer to STACK_GET_NEXT.
        "ignore_answer": event != PlayerEvent::Finished,
        "is_led_silent": true,
        "payload": { "play_id": play_id, "item_id": item.id },
    })
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// audio_play the way Yandex sends it for radio (relay journal, 31.10), with our link and our callbacks.
pub fn audio_play(item: &MediaItem, play_id: &str, offset_ms: u64) -> Directive {
    let radio = item.r#type == "FmRadio";
    let mut stream = json!({
        "id": item.id,
        "url": item.url,
        "format": item.format,
        "type": item.r#type,
        "offset_ms": offset_ms,
        "duration_ms": item.duration_ms.unwrap_or(2_147_483_647),
    });
    if let Some(codec) = item.codec.as_ref().filter(|codec| !codec.is_empty()) {
        stream["codec"] = Value::String(codec.clone());
    }
    let payload = json!({
        "stream": stream,
        "metadata": {
            "title": item.title,
            "subtitle": item.subtitle.clone().unwrap_or_default(),
            "art_image_url": item.image_url.clone().unwrap_or_default(),
            "hide_progress_bar": radio,
            "glagol_metadata": {
                "music_metadata": {
                    "id": item.id,
                    "type": item.r#type,
                    "description": "",
                    "shuffled": false,
                    "repeat_mode": "None",
                },
            },
        },
        "background_mode": "Ducking",
        "screen_type": "Music",
        "set_pause": false,
        "playback_speed": 1,
        "provider_name": "duowave",
        "callbacks": {
            "on_started": callback(PlayerEvent::Started, play_id, item),
            "on_stopped": callback(PlayerEvent::Stopped, play_id, item),
            "on_finished": callback(PlayerEvent::Finished, play_id, item),
            "on_failed": callback(PlayerEvent::Failed, play_id, item),
        },
    });
    Directive {
        r#type: DirectiveType::ClientAction,
        name: "audio_play".into(),
        analytics_type: Some(if radio { "radio" } else { "music" }.into()),
        payload: Some(object(payload)),
        typed_payload: None,
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

/// The request for the next track that goes with a track, as Yandex's, with our play id.
fn next_request(play_id: &str) -> Directive {
    Directive {
        r#type: DirectiveType::ServerAction,
        name: STACK_GET_NEXT.into(),
        analytics_type: None,
        payload: Some(object(json!({ MEDIA_PLAY_ID: play_id, "stack_session_id": play_id }))),
        typed_payload: None,
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

/// What `Command::Play` is on the Station: audio_play, and the request for the next track unless it is a radio.
pub fn play(item: &MediaItem, play_id: &str, offset_ms: u64) -> Vec<Directive> {
    if item.r#type == "FmRadio" {
        vec![audio_play(item, play_id, offset_ms)]
    } else {
        vec![audio_play(item, play_id, offset_ms), next_request(play_id)]
    }
}

/// `Command::AskAgain` on the Station: a server action of ours it sends back as a request, which the player answers
/// with what to play (src/skills/media/playback.rs, `retry`).
pub fn ask_again(play_id: &str) -> Directive {
    let mut payload = Map::new();
    payload.insert(MEDIA_PLAY_ID.into(), json!(play_id));
    Directive {
        r#type: DirectiveType::ServerAction,
        name: format!("{MEDIA_CALLBACK}{RETRY_CALLBACK}"),
        analytics_type: None,
        payload: Some(payload),
        typed_payload: None,
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: &str) -> MediaItem {
        MediaItem {
            id: id.into(),
            title: id.into(),
            url: format!("http://gateway.local/media/{id}.mp3"),
            format: "MP3".into(),
            codec: None,
            r#type: kind.into(),
            subtitle: None,
            image_url: None,
            duration_ms: Some(81_720),
        }
    }

    #[test]
    fn a_track_goes_with_yandexs_request_for_the_next_one_and_a_radio_alone() {
        let track = play(&item("a", "Track"), "p1", 0);
        assert_eq!(track.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["audio_play", STACK_GET_NEXT]);
        assert_eq!(track[1].r#type, DirectiveType::ServerAction);
        assert_eq!(track[1].payload.as_ref().unwrap()[MEDIA_PLAY_ID], "p1");
        let callbacks = &track[0].payload.as_ref().unwrap()["callbacks"];
        assert_eq!(callbacks["on_started"]["payload"]["play_id"], "p1");
        assert_eq!(track[0].payload.as_ref().unwrap()["stream"]["duration_ms"], 81_720);
        let radio = play(&item("r", "FmRadio"), "p2", 0);
        assert_eq!(radio.len(), 1, "a live radio has no next track to ask for");
        assert_eq!((radio[0].name.as_str(), radio[0].analytics_type.as_deref()), ("audio_play", Some("radio")));
        let payload = radio[0].payload.as_ref().unwrap();
        assert_eq!(payload["metadata"]["title"], "r");
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
    }
}
