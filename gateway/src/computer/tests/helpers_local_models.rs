//! The computer's part of whole_tests/helpers_models.rs: what only a computer has, tested with the whole gateway built.

use crate::dialog::brain::helper_models::ModelHelpers;
use crate::helpers::{HelperState, Reach};

/// This computer's model servers (stage 6): the server of whichever row the choice points at — LM Studio added after
/// Ollama, or Ollama — closed, one look gives the backup at once; nothing goes to the closed server, which «Где
/// сейчас» and «Спросить мозг» say in their own words.
#[tokio::test]
async fn closing_whichever_local_model_answers_gives_way_to_the_backup() {
    use crate::computer::tests::helpers_builtin::{computer, provider, said, server};
    use crate::config::ModelProfile;
    use crate::dialog::brain::brain_service::BUILTIN_AWAY;
    use crate::dialog::llm::brain::BrainSource;
    use crate::helpers::OwnParts;
    let (ollama, lmstudio) = (server("Ollama.").await, server("LM Studio.").await);
    let (app, _dir) = computer(&[]);
    let first = provider(&app, "ollama", &ollama.url);
    let second = provider(&app, "lmstudio", &lmstudio.url);
    let cloud = provider(&app, "custom-openai", "https://llm.example.invalid/v1");
    let choose = |fast: &str| {
        let choice = serde_json::json!({
            "fast": {"provider": fast, "model": "local-model"},
            "backup": {"provider": cloud, "model": "cloud-model"},
        });
        app.brain.choose(choice.as_object().unwrap(), None).unwrap();
    };
    let answering = || {
        let slot = BrainSource::slot(&*app.brain, ModelProfile::Fast).unwrap();
        (slot.provider.clone(), slot.backup.as_ref().map(|backup| backup.slot.provider.clone()))
    };
    for (chosen, server, answer) in [(second, lmstudio, "LM Studio."), (first, ollama, "Ollama.")] {
        choose(&chosen);
        app.pc.builtin.check().await;
        assert_eq!(answering(), (chosen.clone(), Some(cloud.clone())), "on the line, the backup behind it");
        let slot = BrainSource::slot(&*app.brain, ModelProfile::Fast).unwrap();
        assert_eq!((said(&slot).await.as_str(), server.chats()), (answer, 1), "its own server answers");
        let mine = app.helpers.model_helpers();
        let model = mine.iter().find(|model| model.provider.as_deref() == Some(chosen.as_str())).unwrap();
        assert!(model.builtin && model.id == crate::helpers::SELF && model.up && model.reach == Reach::Home);
        server.close().await;
        app.pc.builtin.check().await;
        assert_eq!(answering(), (cloud.clone(), None), "the backup at once");
        let now = app.brain.now(ModelProfile::Fast);
        assert_eq!(now.reason.as_deref(), Some(BUILTIN_AWAY), "«модель на этом компьютере не отвечает»");
        let card = app.brain.view().providers.into_iter().find(|row| row.id == chosen).unwrap().helper.unwrap();
        assert_eq!((card.id.as_str(), card.state, card.builtin), (crate::helpers::SELF, HelperState::Down, true));
    }
    assert!(app.helpers.model_helpers().iter().all(|model| !model.up), "both closed now");
}

/// A row of the person's own at this computer's address stays theirs: «Мозг» never makes it a helper's row.
#[tokio::test]
async fn a_local_custom_row_stays_the_persons_and_editable() {
    use crate::computer::tests::helpers_builtin::{computer, provider, server};
    use crate::dialog::brain::ProviderInput;
    use crate::helpers::OwnParts;
    let local = server("Свой.").await;
    let (app, _dir) = computer(&[]);
    let row = provider(&app, "custom-openai", &local.url);
    let choice = serde_json::json!({"fast": {"provider": row, "model": "local-model"}});
    app.brain.choose(choice.as_object().unwrap(), None).unwrap();
    app.pc.builtin.check().await;
    app.brain.keep_helper_models().unwrap();
    let view = app.brain.view().providers.into_iter().find(|view| view.id == row).unwrap();
    assert_eq!((view.preset.as_str(), view.title.as_str()), ("custom-openai", "Свой, OpenAI-совместимый"));
    assert_eq!(view.helper.map(|helper| (helper.id, helper.builtin)), Some((crate::helpers::SELF.to_string(), true)));
    assert!(app.brain.view().providers.iter().all(|view| view.preset != "helper"), "no helper's row made");
    let renamed = app.brain.update(&row, &ProviderInput::from_value(serde_json::json!({"title": "Мой"})), None);
    assert_eq!(renamed.unwrap().title, "Мой");
    local.close().await;
}
