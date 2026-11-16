//! What an app plays on a station by itself (docs/plans/media-apps.md, stage 2): a receiver — AirPlay — writes its
//! sound to the station past our player. The app says so (`system/player` → `playing`); while it plays or stands paused
//! there, the player's voice commands are its own («пауза», «продолжай», «дальше», «назад», «стоп»), and the station's
//! player started by Alice takes them back and pauses the app.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use futures::future::BoxFuture;

/// Where an app's playing is kept when it names no device and no station has spoken yet: the newest station on the
/// line (as a push to `*` goes).
pub const ANY_DEVICE: &str = "*";

/// The voice commands an app's player takes.
pub const APP_ACTIONS: &[&str] = &["pause", "resume", "next", "previous", "stop"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppPlayState {
    Playing,
    Paused,
}

impl AppPlayState {
    /// `playing`, `paused`; `stopped` is None — nothing of the app's plays any more.
    pub fn parse(state: &str) -> Result<Option<AppPlayState>, String> {
        match state {
            "playing" => Ok(Some(AppPlayState::Playing)),
            "paused" => Ok(Some(AppPlayState::Paused)),
            "stopped" => Ok(None),
            other => Err(format!("state «{other}»: playing, paused или stopped")),
        }
    }
}

/// What an app plays, as it says (`system/player` → `playing`: `title`, `artist`, `cover`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTrack {
    pub title: Option<String>,
    pub artist: Option<String>,
    /// An http(s) address.
    pub cover: Option<String>,
}

impl AppTrack {
    /// From the letter's fields; None when it names nothing.
    pub fn from_letter(input: &serde_json::Value) -> Option<AppTrack> {
        let text = |key: &str| {
            input
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| text.chars().take(300).collect::<String>())
        };
        let cover = text("cover").filter(|url| url.starts_with("https://") || url.starts_with("http://"));
        let track = AppTrack { title: text("title"), artist: text("artist").or_else(|| text("subtitle")), cover };
        (track != AppTrack::default()).then_some(track)
    }
}

/// An app that plays (or stands paused) on a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPlaying {
    pub app: String,
    pub state: AppPlayState,
    /// What it plays, when it said; kept while the same app goes on.
    pub track: Option<AppTrack>,
    /// Unix ms when it started playing.
    pub since: u64,
}

/// Who carries a command to an app: the plugin host (links/app_player.rs).
pub trait AppControl: Send + Sync {
    /// The app's player command (one of `APP_ACTIONS`): done, or why not, in words.
    fn control(&self, app: &str, action: &str) -> BoxFuture<'static, Result<(), String>>;
    /// Whether the app is switched on now: one switched off or given up on plays nothing.
    fn alive(&self, app: &str) -> bool;
    /// Its title for the person: «AirPlay».
    fn title(&self, app: &str) -> String;
}

#[derive(Default)]
pub struct AppPlayback {
    /// By device.
    playing: Mutex<HashMap<String, AppPlaying>>,
    control: OnceLock<Arc<dyn AppControl>>,
}

impl AppPlayback {
    pub fn new() -> AppPlayback {
        AppPlayback::default()
    }

    pub fn set_control(&self, control: Arc<dyn AppControl>) {
        let _ = self.control.set(control);
    }

    fn playing(&self) -> std::sync::MutexGuard<'_, HashMap<String, AppPlaying>> {
        self.playing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An app plays on some device now (not one standing paused).
    pub fn any_playing(&self) -> bool {
        self.playing().values().any(|playing| playing.state == AppPlayState::Playing)
    }

    /// The app says what it plays on `device`: whether it started playing just now (from paused, stopped, or another
    /// app's playing) — then the station's own player is to be paused.
    pub fn report(&self, device: &str, app: &str, state: Option<AppPlayState>) -> bool {
        let mut playing = self.playing();
        match state {
            None => {
                // Only its own: a stop of one app does not take the device from another.
                if playing.get(device).is_some_and(|now| now.app == app) {
                    playing.remove(device);
                }
                false
            }
            Some(state) => {
                let now = crate::utils::time::now_ms();
                let same = playing.get(device).filter(|before| before.app == app).cloned();
                let entry = AppPlaying {
                    app: app.to_string(),
                    state,
                    track: same.as_ref().and_then(|before| before.track.clone()),
                    since: same.as_ref().map_or(now, |before| before.since),
                };
                let before = playing.insert(device.to_string(), entry);
                state == AppPlayState::Playing
                    && before.is_none_or(|before| before.app != app || before.state != AppPlayState::Playing)
            }
        }
    }

    /// What the app plays on the device, as it said (a new track replaces it; none said keeps the last).
    pub fn describe(&self, device: &str, app: &str, track: Option<AppTrack>) {
        let Some(track) = track else { return };
        if let Some(entry) = self.playing().get_mut(device).filter(|entry| entry.app == app) {
            entry.track = Some(track);
        }
    }

    /// The app whose player the device's voice commands are now: its own entry, else the one kept for any device; an
    /// app switched off since is forgotten.
    pub fn owner(&self, device: &str) -> Option<AppPlaying> {
        let mut playing = self.playing();
        let key = if playing.contains_key(device) { device } else { ANY_DEVICE };
        let found = playing.get(key).cloned()?;
        if self.control.get().is_some_and(|control| !control.alive(&found.app)) {
            playing.remove(key);
            return None;
        }
        Some(found)
    }

    /// The station's own player started on the device: the app's player is no longer what plays there. The app that
    /// was playing, to be paused; one that stood paused is only let go of.
    pub fn taken_over(&self, device: &str) -> Option<String> {
        let found = self.owner(device)?;
        let mut playing = self.playing();
        playing.remove(device);
        playing.remove(ANY_DEVICE);
        (found.state == AppPlayState::Playing).then_some(found.app)
    }

    /// A player command to the app.
    pub fn command(&self, app: &str, action: &str) -> BoxFuture<'static, Result<(), String>> {
        match self.control.get() {
            Some(control) => control.control(app, action),
            None => Box::pin(async { Err("приложений здесь нет".to_string()) }),
        }
    }

    /// The app's title for the person; its id when nobody knows better.
    pub fn title(&self, app: &str) -> String {
        self.control.get().map(|control| control.title(app)).unwrap_or_else(|| app.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Control(AtomicBool);

    impl AppControl for Control {
        fn control(&self, _app: &str, _action: &str) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn alive(&self, _app: &str) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn title(&self, _app: &str) -> String {
            "AirPlay".into()
        }
    }

    #[test]
    fn an_app_owns_the_player_from_playing_until_stopped_or_switched_off() {
        let playback = AppPlayback::new();
        let control = Arc::new(Control(AtomicBool::new(true)));
        playback.set_control(control.clone());
        assert!(playback.report("kitchen", "airplay", Some(AppPlayState::Playing)), "started: pause the station");
        assert!(!playback.report("kitchen", "airplay", Some(AppPlayState::Playing)), "the same again: nothing new");
        assert!(!playback.report("kitchen", "airplay", Some(AppPlayState::Paused)));
        assert_eq!(playback.owner("kitchen").map(|owner| owner.state), Some(AppPlayState::Paused));
        assert!(playback.report("kitchen", "airplay", Some(AppPlayState::Playing)), "resumed: pause the station again");
        assert_eq!(playback.owner("hall"), None, "another station is not its");
        assert!(!playback.report("kitchen", "other", None), "another app's stop does not free it");
        assert!(playback.owner("kitchen").is_some());
        playback.report("kitchen", "airplay", None);
        assert_eq!(playback.owner("kitchen"), None);
        playback.report("kitchen", "airplay", Some(AppPlayState::Paused));
        control.0.store(false, Ordering::SeqCst);
        assert_eq!(playback.owner("kitchen"), None, "switched off: it plays nothing");
    }

    #[test]
    fn a_device_not_known_yet_is_any_station_and_alice_takes_it_back() {
        let playback = AppPlayback::new();
        playback.report(ANY_DEVICE, "airplay", Some(AppPlayState::Playing));
        assert_eq!(playback.owner("kitchen").map(|owner| owner.app), Some("airplay".into()));
        assert_eq!(playback.taken_over("kitchen").as_deref(), Some("airplay"), "it played: it is paused");
        assert_eq!(playback.owner("kitchen"), None);
        playback.report("kitchen", "airplay", Some(AppPlayState::Paused));
        assert_eq!(playback.taken_over("kitchen"), None, "it stood paused: only let go of");
        assert_eq!(playback.owner("kitchen"), None);
        assert_eq!(AppPlayState::parse("stopped"), Ok(None));
        assert!(AppPlayState::parse("loud").is_err());
    }
}
