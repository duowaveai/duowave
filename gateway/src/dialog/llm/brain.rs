//! Which model answers, per profile: the panel's choice of providers (BrainService) or, while the panel has none,
//! the one .env describes. The dialog takes one profile's model per answer and keeps it to the end of that answer:
//! a swap in the panel applies from the next question, never in the middle of one (Claude's thinking blocks are
//! valid only for the model that wrote them).

use std::sync::Arc;

use serde::{Serialize, Serializer};

use super::llm_client::{create_llm, LlmClient, LlmSettings};
use super::openai_chat::OPENAI_BASE_URL;
use crate::config::{DialogProvider, ModelProfile};
use crate::operations::networks::is_local_url;

/// The kind of a provider as the panel names it: anthropic, openai, echo, none.
pub fn serialize_kind<S: Serializer>(kind: &DialogProvider, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(kind.as_str())
}

/// One profile's model, fixed for one answer.
#[derive(Clone)]
pub struct ModelSlot {
    pub client: Arc<dyn LlmClient>,
    pub model: String,
    /// The provider's id: `env` for .env, the panel's id otherwise.
    pub provider: String,
    pub kind: DialogProvider,
    /// For people: «Claude», «OpenRouter», «Ollama».
    pub title: String,
    /// The question leaves the house.
    pub cloud: bool,
}

impl ModelSlot {
    pub fn profile(&self) -> BrainProfile {
        BrainProfile {
            provider: self.provider.clone(),
            kind: self.kind,
            title: self.title.clone(),
            model: self.model.clone(),
            cloud: self.cloud,
        }
    }
}

/// A profile as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrainProfile {
    pub provider: String,
    #[serde(serialize_with = "serialize_kind")]
    pub kind: DialogProvider,
    pub title: String,
    pub model: String,
    pub cloud: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrainModels {
    pub fast: String,
    pub smart: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrainProfiles {
    pub fast: Option<BrainProfile>,
    pub smart: Option<BrainProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrainOrigin {
    Env,
    Panel,
}

/// The conversation model as the panel shows it: which one, its two profiles, how deep it thinks, where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrainStatus {
    /// The fast profile's kind: anthropic, openai (any OpenAI-compatible service), echo for checks, none.
    #[serde(serialize_with = "serialize_kind")]
    pub provider: DialogProvider,
    /// The router picks the profile per request; one model when both are the same.
    pub models: BrainModels,
    /// Claude only: how long it thinks before the first word.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The fast profile's question leaves the house: Anthropic, or a service that is not on this network.
    pub cloud: bool,
    /// Each profile's provider and model; null when the profile has no model.
    pub profiles: BrainProfiles,
    /// Where the choice comes from: the panel, or .env while the panel has chosen nothing.
    pub source: BrainOrigin,
}

pub trait BrainSource: Send + Sync {
    /// The model of a profile now; None when there is none (the dialog is off).
    fn slot(&self, profile: ModelProfile) -> Option<ModelSlot>;
    /// A given provider's model, apart from the profiles (the workshop agent's own choice); None when there is no such
    /// provider — the caller falls back to a profile.
    fn slot_of(&self, _provider: &str, _model: &str) -> Option<ModelSlot> {
        None
    }
    /// Model and provider, for logs and /health.
    fn description(&self) -> String;
    fn status(&self) -> BrainStatus;
}

/// Whether a provider's questions leave the house: Claude always, others unless their address is at home.
pub fn cloud_of(provider: DialogProvider, base_url: Option<&str>) -> bool {
    match provider {
        DialogProvider::Anthropic => true,
        DialogProvider::OpenAi => !is_local_url(Some(base_url.unwrap_or(OPENAI_BASE_URL))),
        DialogProvider::Echo | DialogProvider::None => false,
    }
}

/// The host and port of an address, as a URL's `host` gave them; the address itself when it is not a URL.
pub fn host_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => match (parsed.host_str(), parsed.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            (None, _) => String::new(),
        },
        Err(_) => url.to_string(),
    }
}

/// The model .env describes (DIALOG_PROVIDER, LLM_*), for both profiles.
pub struct EnvBrain {
    settings: LlmSettings,
    client: Option<Arc<dyn LlmClient>>,
    title: String,
}

impl EnvBrain {
    pub fn new(settings: LlmSettings) -> EnvBrain {
        let client = create_llm(&settings);
        let title = match settings.provider {
            DialogProvider::Anthropic => "Claude".to_string(),
            DialogProvider::OpenAi => host_of(settings.base_url.as_deref().unwrap_or(OPENAI_BASE_URL)),
            DialogProvider::Echo => "Эхо".to_string(),
            DialogProvider::None => "нет".to_string(),
        };
        EnvBrain { settings, client, title }
    }

    fn cloud(&self) -> bool {
        cloud_of(self.settings.provider, self.settings.base_url.as_deref())
    }
}

impl BrainSource for EnvBrain {
    fn slot(&self, profile: ModelProfile) -> Option<ModelSlot> {
        let client = self.client.clone()?;
        let model = match profile {
            ModelProfile::Fast => &self.settings.models.fast,
            ModelProfile::Smart => &self.settings.models.smart,
        };
        Some(ModelSlot {
            client,
            model: if model.is_empty() { self.settings.model.clone() } else { model.clone() },
            provider: "env".into(),
            kind: self.settings.provider,
            title: self.title.clone(),
            cloud: self.cloud(),
        })
    }

    fn description(&self) -> String {
        match &self.client {
            Some(client) => client.description(),
            None => "off (DIALOG_PROVIDER=none)".into(),
        }
    }

    fn status(&self) -> BrainStatus {
        let settings = &self.settings;
        BrainStatus {
            provider: settings.provider,
            models: BrainModels { fast: settings.models.fast.clone(), smart: settings.models.smart.clone() },
            effort: (settings.provider == DialogProvider::Anthropic).then(|| settings.effort.clone()),
            cloud: self.cloud(),
            profiles: BrainProfiles {
                fast: self.slot(ModelProfile::Fast).map(|slot| slot.profile()),
                smart: self.slot(ModelProfile::Smart).map(|slot| slot.profile()),
            },
            source: BrainOrigin::Env,
        }
    }
}
