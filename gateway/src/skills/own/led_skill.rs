//! Animations of the station's light: draw_animation_directive carries the animation in field 15 (gzip, its format
//! not yet known). We replay animations captured from Yandex's answers in the relay journal, kept as
//! CONTENT_DIR/animations/<name>.b64, exactly as Yandex sent them; `duowave animations` (src/tools/animations.rs)
//! writes them from the journals.

use std::path::PathBuf;

use bytes::Bytes;
use futures::future::BoxFuture;

use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};
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

    /// The animation by name, if captured.
    pub fn animation(&self, name: &str) -> Option<Directive> {
        let file = self.dir.join(format!("{}.b64", basename(name)));
        let text = std::fs::read_to_string(file).ok()?;
        Some(Directive {
            r#type: DirectiveType::ClientAction,
            name: "draw_animation_directive".into(),
            analytics_type: Some("draw_animation_directive".into()),
            payload: None,
            typed_payload: Some(Bytes::from(js::base64_decode(js::trim(&text)))),
            is_led_silent: Some(true),
            ignore_answer: None,
        })
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
                Some(directive) => Outcome::directives(vec![directive]),
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
        let animation = skill.animation("thinking").unwrap();
        assert_eq!(animation.name, "draw_animation_directive");
        assert_eq!(animation.typed_payload.as_deref(), Some(&[0x1f, 0x8b, 0x08][..]));
        assert_eq!(skill.animation("../thinking").unwrap().name, "draw_animation_directive");
        assert!(skill.animation("missing").is_none());
    }
}
