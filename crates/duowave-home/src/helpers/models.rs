//! «Мозг»'s view of the helpers (dialog/brain/helper_models.rs): the helpers that give a model, in the list's order,
//! each with where its model is asked and whether it is on the line now. Read from memory, never the settings table:
//! «Мозг» asks for it with every slot. On a computer that takes its own parts from itself, its model servers come
//! first (the built-in helper, stage 6): each is the person's own row of «Мозг» already — read with «Мозг»'s rows, one
//! small read — and gives way at once when its last look found its server closed.

use super::contract::Ability;
use super::{key_secret, reach_of, HelperKind, HelperSetting, HelperState, Helpers, Reach, Snapshot, SELF};
use crate::dialog::brain::helper_models::{ModelHelper, ModelHelpers};

impl ModelHelpers for Helpers {
    /// This computer's model servers first, in the rows' order; then the helpers switched on, keyed, not this gateway,
    /// not kept from the model, and declaring it or not read yet — the regulator aside: «Мозг» says it keeps one out.
    /// A helper reached by two entries (the same `helperId`) is one model under the earlier entry, whatever the line
    /// does — its row is made, kept and dropped by the list, never by who is up —, and the later entry is only its road
    /// while it stands for the helper (the earlier one is not on the line).
    fn model_helpers(&self) -> Vec<ModelHelper> {
        let snap = self.snap();
        let mut models: Vec<ModelHelper> = self.own_models();
        for entry in snap.list.iter().filter(|entry| entry.enabled && !self.is_self(entry)) {
            let Some(remote) = self.remote(entry) else { continue };
            if let Some(first) = self.first_of(entry, &snap) {
                let stands = self.twin_of(entry, &snap).is_none()
                    && !entry.without.contains(&Ability::Model)
                    && remote.up()
                    && remote.gives(Ability::Model);
                if let Some(model) = models.iter_mut().find(|model| model.id == first.id).filter(|model| !model.up) {
                    if stands {
                        model.base_url = format!("{}/home/v1/llm", remote.base());
                        model.key = Some(remote.credential());
                        model.reach = reach_of(entry);
                        model.up = true;
                        model.state = remote.state();
                        model.linked = remote.base() != remote.url();
                        model.via = Some(entry.id.clone());
                    }
                }
                continue;
            }
            if entry.without.contains(&Ability::Model) || !remote.declares(Ability::Model) {
                continue;
            }
            models.push(ModelHelper {
                id: entry.id.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                base_url: format!("{}/home/v1/llm", remote.base()),
                key: Some(remote.credential()),
                reach: reach_of(entry),
                up: remote.up() && remote.gives(Ability::Model),
                state: remote.state(),
                via: None,
                read: remote.status().is_some(),
                linked: remote.base() != remote.url(),
                provider: None,
                builtin: false,
            });
        }
        models
    }

    fn lets(&self, reach: Reach) -> bool {
        Helpers::lets(self, reach)
    }
}

impl Helpers {
    /// This computer's model servers as the built-in helper's (stage 6): each is the «Мозг» row it already is
    /// (`provider`), at home, on the line unless its last look found it closed, asked with the row's own key.
    fn own_models(&self) -> Vec<ModelHelper> {
        let Some(own) = self.own.get() else { return Vec::new() };
        own.models()
            .into_iter()
            .map(|model| ModelHelper {
                id: SELF.to_string(),
                name: crate::tr!("Этот компьютер"),
                kind: HelperKind::Home,
                key: self.secrets.get(&format!("llm:{}:key", model.provider)),
                reach: Reach::Home,
                up: model.up != Some(false),
                state: match model.up {
                    Some(true) => HelperState::Up,
                    Some(false) => HelperState::Down,
                    None => HelperState::Unknown,
                },
                via: None,
                read: true,
                linked: false,
                provider: Some(model.provider),
                builtin: true,
                base_url: model.base_url,
            })
            .collect()
    }

    /// The earlier entry of the same helper (the same `helperId`) that is switched on and keyed — the one its model
    /// belongs to in «Мозг», on the line or not (`twin_of` asks who stands for the helper now).
    fn first_of<'a>(&self, entry: &HelperSetting, snap: &'a Snapshot) -> Option<&'a HelperSetting> {
        let named = entry.helper_id.as_deref()?;
        snap.list.iter().take_while(|earlier| earlier.id != entry.id).find(|earlier| {
            earlier.helper_id.as_deref() == Some(named) && earlier.enabled && self.secrets.has(&key_secret(&earlier.id))
        })
    }
}
