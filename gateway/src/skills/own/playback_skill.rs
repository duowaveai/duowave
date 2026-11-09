//! Stop, pause, continue, next, previous for whatever the station plays: its player and a phone over Bluetooth.
//! Stop and pause are the commands Yandex sends (seen); the others are from the wiki. A like or a dislike is ours
//! for our music (`ours`, from the router) and Yandex's for its own (`external`).

use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;

use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::directives as commands;
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAction {
    Resume,
    Next,
    Previous,
    Replay,
}

impl PlayerAction {
    fn parse(action: &str) -> Option<PlayerAction> {
        match action {
            "resume" => Some(PlayerAction::Resume),
            "next" => Some(PlayerAction::Next),
            "previous" => Some(PlayerAction::Previous),
            "replay" => Some(PlayerAction::Replay),
            _ => None,
        }
    }
}

/// Our own player (content from /media) answers next, previous, continue and «сначала» itself (media_skills.rs).
pub trait PlayerControl: Send + Sync {
    /// Whether our player owns what the station plays now.
    fn owns(&self, device_tag: &str) -> bool;
    fn control(&self, action: PlayerAction, context: &ExecutionContext) -> Option<Outcome>;
    /// We stop or pause it: the station's stop that follows is not the end of the track.
    fn stopping(&self, device_tag: &str);
    /// «Мне нравится», «не нравится» for what it plays now.
    fn rate(&self, liked: bool, context: &ExecutionContext) -> Option<Outcome>;
}

#[derive(Default)]
pub struct PlaybackSkill {
    player: RwLock<Option<Arc<dyn PlayerControl>>>,
}

impl PlaybackSkill {
    pub fn new() -> PlaybackSkill {
        PlaybackSkill::default()
    }

    /// Our media player, once it exists, takes next, previous and continue for its own content.
    pub fn attach_player(&self, player: Arc<dyn PlayerControl>) {
        *self.player.write().unwrap() = Some(player);
    }

    fn player(&self) -> Option<Arc<dyn PlayerControl>> {
        self.player.read().unwrap().clone()
    }

    fn play(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = context.device_tag.as_str();
        let ours = self.player().filter(|player| player.owns(device));
        if intent.action == "like" || intent.action == "dislike" {
            let outcome = ours.and_then(|player| player.rate(intent.action == "like", context));
            return Ok(outcome.unwrap_or_else(|| Outcome::speech("Отметить могу только нашу музыку.")));
        }
        if let (Some(action), Some(player)) = (PlayerAction::parse(&intent.action), &ours) {
            if let Some(outcome) = player.control(action, context) {
                return Ok(outcome);
            }
        }
        if intent.action == "stop" || intent.action == "pause" {
            if let Some(player) = &ours {
                player.stopping(device);
            }
        }
        Ok(match intent.action.as_str() {
            // A stop with nothing playing: the station stopped speaking at the wake word; no word, no command.
            "silence" => Outcome::default(),
            "stop" => Outcome::directives(commands::stop_player()),
            "pause" => Outcome::directives(vec![commands::pause_player()]),
            "resume" => Outcome::directives(vec![commands::continue_player()]),
            "next" => Outcome::directives(vec![commands::next_track()]),
            "previous" => Outcome::directives(vec![commands::previous_track()]),
            // No command of the station's for it is known: Yandex's player starts its track over itself.
            "replay" => Outcome::speech("Начать сначала пока не умею."),
            other => anyhow::bail!("playback: unknown action {other}"),
        })
    }
}

impl OwnSkill for PlaybackSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Playback
    }

    fn verified(&self, intent: &Intent) -> bool {
        if js::truthy(intent.params.get("external")) {
            return false;
        }
        if intent.action == "like" || intent.action == "dislike" {
            return js::is_true(&intent.params, "ours");
        }
        matches!(intent.action.as_str(), "stop" | "pause" | "silence")
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { self.play(intent, context) })
    }
}
