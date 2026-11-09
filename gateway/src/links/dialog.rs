//! The dialog (src/dialog) as the skills and the MCP check chat reach it: skills.module.ts and ask.controller.ts
//! injected the DialogService itself.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::sync::Arc;

use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

use crate::config::ModelProfile;
use crate::dialog::{DialogContext, DialogService, PhraseStream, ToolBox};
use crate::mcp::{AskDialog, AskModel, AskToolbox};
use crate::skills::DialogAccess;

/// The skills' `DialogAccess`: whether a model answers, a tale told by the smart model, a game hosted in a role.
pub struct SkillsDialog(pub Arc<DialogService>);

impl DialogAccess for SkillsDialog {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn narrate(&self, instructions: String, cancel: CancellationToken) -> PhraseStream {
        self.0.narrate(&instructions, cancel)
    }

    fn roleplay(
        &self,
        key: String,
        system: String,
        text: String,
        cancel: CancellationToken,
        fresh: bool,
    ) -> PhraseStream {
        self.0.roleplay(&key, &system, &text, cancel, fresh)
    }
}

/// MCP's check chat: the fast profile answers with the MCP tools this person may use.
pub struct AskBrain(pub Arc<DialogService>);

impl AskDialog for AskBrain {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }

    fn fast_model(&self) -> AskModel {
        self.0
            .brain()
            .profiles
            .fast
            .map(|fast| AskModel { model: fast.model, provider: fast.title })
            .unwrap_or_default()
    }

    fn forget(&self, key: &str) {
        self.0.forget(key);
    }

    fn answer(
        &self,
        key: &str,
        question: &str,
        toolbox: Option<AskToolbox>,
        cancel: CancellationToken,
    ) -> BoxStream<'static, anyhow::Result<String>> {
        let context = DialogContext { profile: Some(ModelProfile::Fast), ..DialogContext::default() };
        self.0.answer(key, question, context, cancel, toolbox.map(chat_tools))
    }
}

/// The chat's tools as the dialog runs them: three rounds, the servers' instructions, only the tools that change
/// nothing after a text from outside the house; every call and its result told to the chat on the way.
fn chat_tools(toolbox: AskToolbox) -> ToolBox {
    let toolbox = Arc::new(toolbox);
    let running = toolbox.clone();
    let mut tools = ToolBox::new(toolbox.tools().to_vec(), move |call, cancel| {
        let toolbox = running.clone();
        async move { toolbox.run(&call, &cancel).await.map_err(anyhow::Error::from) }
    });
    tools.rounds = Some(toolbox.rounds());
    tools.instructions = Some(toolbox.instructions().to_string());
    let safe = toolbox.clone();
    tools.safe_after_taint = Some(Arc::new(move |offered| safe.safe_after_taint(offered)));
    tools
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};

    fn app(pairs: &[(&str, &str)]) -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs(pairs.iter().copied()))).unwrap()
    }

    #[tokio::test]
    async fn the_skills_and_the_check_chat_ask_the_gateways_own_dialog() {
        let off = app(&[("STATE_DIR", ""), ("DIALOG_PROVIDER", "none")]);
        assert!(!SkillsDialog(off.dialog.clone()).enabled());
        let chat = off.mcp.dialog().expect("the check chat is linked at the build");
        assert!(!chat.enabled(), "no model chosen");
        assert_eq!(chat.fast_model(), AskModel::default());

        // The echo model answers what it was asked: the chat's question goes through the dialog's fast profile.
        let echo = app(&[("STATE_DIR", ""), ("DIALOG_PROVIDER", "echo")]);
        let chat = echo.mcp.dialog().unwrap();
        assert!(chat.enabled());
        let fast = chat.fast_model();
        assert_eq!(fast.model, echo.dialog.brain().profiles.fast.unwrap().model);
        let said: Vec<String> = chat
            .answer("panel:u1", "Ты здесь?", None, CancellationToken::new())
            .map(|phrase| phrase.unwrap())
            .collect()
            .await;
        assert!(said.join(" ").contains("Ты здесь"), "{said:?}");
        let skills = SkillsDialog(echo.dialog.clone());
        assert!(skills.enabled());
        let told: Vec<String> = skills
            .roleplay("game:kitchen".into(), "Ты ведущий.".into(), "Начнём".into(), CancellationToken::new(), true)
            .map(|phrase| phrase.unwrap())
            .collect()
            .await;
        assert!(!told.is_empty(), "the game host answers");
    }
}
