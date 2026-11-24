//! Voice games on the model (quiz, words, riddles): the model hosts, the station keeps listening after each move,
//! follow-ups come back here until the player says enough or goes quiet for a few minutes.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use futures::future::BoxFuture;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill, PhraseStream};
use crate::skills::text::normalize;

/// Hosts a game on the model: its own prompt and history per device (key, system, text, cancel, fresh).
pub type GameHost = Arc<dyn Fn(String, String, String, CancellationToken, bool) -> PhraseStream + Send + Sync>;

/// Whether the model answers now: the panel may switch it off and on without a restart.
pub type ModelOn = Arc<dyn Fn() -> bool + Send + Sync>;

const PLAY_MS: u64 = 3 * 60_000;
static STOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(хватит|стоп|закончим|закончить|выход|выйти|надоело|не хочу( больше)?( играть)?|давай закончим|конец игры)$",
    )
    .expect("a valid pattern")
});

fn host_prompt(game: &str, sounds: bool) -> String {
    let mut lines = vec![
        format!(
            "Ты ведёшь голосовую игру «{game}» с одним игроком на умной колонке; всё, что ты пишешь, будет озвучено."
        ),
        "Говори коротко: одна-две фразы за ход, без списков и разметки. Каждый твой ход заканчивай вопросом или".into(),
        "приглашением сделать ход. Следи за правилами и счётом сам. Игра идёт, пока игрок не скажет закончить.".into(),
    ];
    if sounds {
        lines.push(
            "Можешь вставлять звуки разметкой синтеза: <speaker audio=\"alice-sounds-game-win-1.opus\"> за верный ответ,"
                .into(),
        );
        lines.push("<speaker audio=\"alice-sounds-game-loss-2.opus\"> за неверный.".into());
    }
    lines.join("\n")
}

pub struct GamesSkill {
    host: Option<GameHost>,
    /// Sounds in the speech: only when synthesis markup is on (TTS_MARKUP).
    sounds: bool,
    hosting: ModelOn,
    games: Mutex<HashMap<String, String>>,
}

impl GamesSkill {
    pub fn new(host: Option<GameHost>, sounds: bool, hosting: Option<ModelOn>) -> GamesSkill {
        GamesSkill {
            host,
            sounds,
            hosting: hosting.unwrap_or_else(|| Arc::new(|| true)),
            games: Mutex::new(HashMap::new()),
        }
    }

    fn play(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = context.device_tag.as_str();
        let Some(host) = self.host.as_ref().filter(|_| (self.hosting)()) else {
            return Ok(Outcome::speech("Игры пока не работают: нет модели."));
        };
        let moves = |phrases: PhraseStream| Outcome {
            phrases: Some(phrases),
            listen: true,
            continuation: Some(ContinuationRequest { capability: CapabilityName::Games, ttl_ms: PLAY_MS }),
            ..Outcome::default()
        };
        match intent.action.as_str() {
            "start" => {
                let game = js::string_param(&intent.params, "game")
                    .filter(|game| !game.is_empty())
                    .unwrap_or("на выбор игрока: викторина, города или загадки")
                    .to_string();
                self.games.lock().unwrap().insert(device.to_string(), game.clone());
                let phrases = host(
                    format!("game:{device}"),
                    host_prompt(&game, self.sounds),
                    "Начнём! Объясни правила в одной фразе и сделай первый ход.".into(),
                    context.cancel.clone(),
                    true,
                );
                Ok(moves(phrases))
            }
            "continue" => {
                let text = js::string_or_empty(intent.params.get("text"));
                if STOP.is_match(&normalize(&text)) {
                    self.games.lock().unwrap().remove(device);
                    return Ok(Outcome::speech("Хорошо, поиграем в другой раз."));
                }
                let game = self.games.lock().unwrap().get(device).cloned().unwrap_or_else(|| "на выбор игрока".into());
                let phrases = host(
                    format!("game:{device}"),
                    host_prompt(&game, self.sounds),
                    text,
                    context.cancel.clone(),
                    false,
                );
                Ok(moves(phrases))
            }
            other => anyhow::bail!("games: unknown action {other}"),
        }
    }
}

impl OwnSkill for GamesSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Games
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn available(&self) -> bool {
        self.host.is_some() && (self.hosting)()
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { self.play(intent, context) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures::StreamExt;
    use serde_json::json;

    #[tokio::test]
    async fn hosts_a_game_on_the_model_and_keeps_the_follow_ups_until_the_player_stops() {
        let calls: Arc<Mutex<Vec<(String, String, bool)>>> = Arc::default();
        let seen = calls.clone();
        let host: GameHost = Arc::new(move |key, _system, text, _cancel, fresh| {
            seen.lock().unwrap().push((key, text, fresh));
            futures::stream::iter([Ok("Ваш ход!".to_string())]).boxed()
        });
        let skill = GamesSkill::new(Some(host), false, None);
        let context = ExecutionContext::new("h:test", Utc::now());
        let run = |action: &str, params: serde_json::Value| Intent {
            capability: CapabilityName::Games,
            action: action.into(),
            params: params.as_object().cloned().unwrap(),
        };
        let start = skill.execute(&run("start", json!({"game": "города"})), &context).await.unwrap();
        assert!(start.listen);
        assert_eq!(start.continuation.map(|c| c.capability), Some(CapabilityName::Games));
        let phrases: Vec<String> = start.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(phrases, ["Ваш ход!"]);
        let step = skill.execute(&run("continue", json!({"text": "Москва"})), &context).await.unwrap();
        let phrases: Vec<String> = step.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(phrases, ["Ваш ход!"]);
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!((calls[0].0.as_str(), calls[0].2), ("game:h:test", true));
            assert!(calls[0].1.contains("Начнём"));
            assert_eq!(calls[1], ("game:h:test".into(), "Москва".into(), false));
        }
        let stop = skill.execute(&run("continue", json!({"text": "Хватит"})), &context).await.unwrap();
        assert_eq!(stop.speech.as_deref(), Some("Хорошо, поиграем в другой раз."));
        assert!(stop.phrases.is_none() && !stop.listen && stop.continuation.is_none());
    }

    #[tokio::test]
    async fn needs_the_model() {
        let skill = GamesSkill::new(None, false, None);
        assert!(!skill.available());
        let context = ExecutionContext::new("h:test", Utc::now());
        let outcome = skill.execute(&Intent::new(CapabilityName::Games, "start"), &context).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Игры пока не работают: нет модели."));
    }
}
