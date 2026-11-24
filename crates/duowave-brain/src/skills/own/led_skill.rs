//! Animations of the station's light: draw_animation_directive carries the animation in field 15 (gzip, its format
//! not yet known). We replay animations captured from Yandex's answers in the relay journal, kept as
//! CONTENT_DIR/animations/<name>.b64, exactly as Yandex sent them; `duowave animations` (src/tools/animations.rs)
//! writes them from the journals.

use std::path::PathBuf;

use futures::future::BoxFuture;

use crate::ear::Command;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

pub struct LedSkill {
    dir: PathBuf,
}

/// The last part of a path, as Node's path.basename gives it: «thinking» of «../thinking».
fn basename(name: &str) -> &str {
    name.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

impl LedSkill {
    pub fn new(dir: PathBuf) -> LedSkill {
        LedSkill { dir }
    }

    pub fn names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".b64").map(str::to_string))
            .collect();
        names.sort();
        names
    }

    /// The animation by name, if captured: the lights' command as it was captured from the station.
    pub fn animation(&self, name: &str) -> Option<Command> {
        let file = self.dir.join(format!("{}.b64", basename(name)));
        let text = std::fs::read_to_string(file).ok()?;
        Some(Command::LightAnimation(js::base64_decode(js::trim(&text))))
    }
}

impl OwnSkill for LedSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Led
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn available(&self) -> bool {
        !self.names().is_empty()
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        _context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let name = js::string_or_empty(intent.params.get("name"));
            Ok(match self.animation(&name) {
                Some(command) => Outcome::commands(vec![command]),
                None => Outcome::speech(format!("Анимации «{name}» нет.")),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    #[test]
    fn replays_a_captured_animation_as_yandex_sent_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("thinking.b64"), STANDARD.encode([0x1f, 0x8b, 0x08])).unwrap();
        let skill = LedSkill::new(dir.path().to_path_buf());
        assert_eq!(skill.names(), ["thinking"]);
        assert_eq!(skill.animation("thinking"), Some(Command::LightAnimation(vec![0x1f, 0x8b, 0x08])));
        assert!(skill.animation("../thinking").is_some(), "the name's own file, whatever path it is given");
        assert!(skill.animation("missing").is_none());
    }
}
