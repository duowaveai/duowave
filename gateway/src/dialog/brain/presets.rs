//! The providers the panel offers ready-made, and the two custom ones. A preset knows the address, whether a key is
//! needed and where to get one, the fields beyond the key, how its model ids are written, and — when the service has
//! one — how the owner signs in instead of pasting a key (src/oauth). Addresses are each service's documented
//! OpenAI-compatible (or Anthropic) endpoint; «Проверить» in the panel confirms them live.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::DialogProvider;
use crate::dialog::llm::LlmApi;
use crate::oauth::{Flow, SignIn, TokenFlow};

/// OpenRouter's sign-in: a key of the owner's, created in their account by OpenRouter's PKCE
/// (https://openrouter.ai/docs/use-cases/oauth-pkce).
const OPENROUTER_SIGN_IN: SignIn = SignIn {
    with: "OpenRouter",
    label: None,
    flow: Flow::KeyExchange {
        authorize: "https://openrouter.ai/auth",
        exchange: "https://openrouter.ai/api/v1/auth/keys",
    },
};

/// ChatGPT's plan through «Sign in with ChatGPT» for open-source and locally hosted apps
/// (https://developers.openai.com/siwc/token-sharing-open-source): a client registered on the fly at the first sign-in,
/// the return to 127.0.0.1 only, this gateway's stable id, the plan's scope required. The answers go through
/// /responses (src/dialog/llm/openai_responses.rs); the button's words are OpenAI's.
const CHATGPT_FLOW: TokenFlow = TokenFlow {
    authorize: "https://auth.openai.com/api/accounts/authorize",
    token: "https://auth.openai.com/api/accounts/oauth/token",
    revoke: Some("https://auth.openai.com/api/accounts/oauth/revoke"),
    issuer: "https://auth.openai.com",
    client: "dynamic_agent_client",
    registers: true,
    scope: "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
    resource: Some("https://api.openai.com/v1"),
    required_scope: Some("chatgpt.tokens.use.direct"),
    loopback: Some("/auth/callback"),
    host_id: true,
};

const CHATGPT_SIGN_IN: SignIn =
    SignIn { with: "ChatGPT", label: Some("Continue with ChatGPT"), flow: Flow::Tokens(&CHATGPT_FLOW) };

/// What a panel provider speaks: Claude's Messages API or OpenAI's /chat/completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
}

impl ProviderKind {
    pub fn parse(value: &str) -> Option<ProviderKind> {
        match value {
            "anthropic" => Some(ProviderKind::Anthropic),
            "openai" => Some(ProviderKind::OpenAi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
        }
    }

    pub fn provider(self) -> DialogProvider {
        match self {
            ProviderKind::Anthropic => DialogProvider::Anthropic,
            ProviderKind::OpenAi => DialogProvider::OpenAi,
        }
    }
}

/// A card's words go out in the reader's language: the source text is the catalog's key (locales/en/services.json).
fn translated<S: serde::Serializer>(text: &&'static str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&crate::locale::text::translate(text, &[]))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresetField {
    pub id: &'static str,
    #[serde(serialize_with = "translated")]
    pub title: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<&'static str>,
}

/// Models to offer when the service does not list them, and to start from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggested {
    pub fast: &'static str,
    pub smart: &'static str,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub more: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: &'static str,
    #[serde(serialize_with = "translated")]
    pub title: &'static str,
    /// One line for people: what it is and why one would pick it.
    #[serde(serialize_with = "translated")]
    pub description: &'static str,
    pub kind: ProviderKind,
    /// The address; the custom ones have none and ask for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<&'static str>,
    /// The address may be changed (custom providers, servers on this Mac with another port).
    pub editable_url: bool,
    /// A key is needed to answer (the services in the cloud); the ones at home answer without.
    pub needs_key: bool,
    /// Where the owner gets a key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_url: Option<&'static str>,
    /// The owner signs in at the service and the gateway gets the access itself: «Войти через …».
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignIn>,
    /// /chat/completions, or /responses for the ChatGPT plan.
    #[serde(skip)]
    pub api: LlmApi,
    /// Where the owner sees and sets how much of a plan DuoWave may spend (ChatGPT's «Usage»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_url: Option<&'static str>,
    /// Fields beyond the key (YandexGPT's folder).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub fields: &'static [PresetField],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested: Option<Suggested>,
    /// Runs on this Mac: nothing leaves the house.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// The panel's brand mark (panel/src/brand-icons.ts); none: a generic icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<&'static str>,
    /// The order on the panel: the owner's providers first (06.11: Claude, OpenAI/OpenRouter, local, Russian, custom).
    pub order: u32,
}

const BLANK: ProviderPreset = ProviderPreset {
    id: "",
    title: "",
    description: "",
    kind: ProviderKind::OpenAi,
    base_url: None,
    editable_url: false,
    needs_key: true,
    key_url: None,
    sign_in: None,
    api: LlmApi::Chat,
    usage_url: None,
    fields: &[],
    suggested: None,
    local: false,
    brand: None,
    order: 0,
};

pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "claude",
        title: "Claude",
        description: "Модели Anthropic: Haiku быстрая, Sonnet и Opus умнее. Лучше всех работают с инструментами.",
        kind: ProviderKind::Anthropic,
        base_url: Some("https://api.anthropic.com"),
        key_url: Some("https://console.anthropic.com/settings/keys"),
        suggested: Some(Suggested { fast: "claude-haiku-5-5", smart: "claude-sonnet-5-5", more: &["claude-opus-5-5"] }),
        brand: Some("claude"),
        order: 1,
        ..BLANK
    },
    ProviderPreset {
        id: "openai",
        title: "OpenAI",
        description: "Модели GPT напрямую у OpenAI.",
        base_url: Some("https://api.openai.com/v1"),
        key_url: Some("https://platform.openai.com/api-keys"),
        brand: Some("openai"),
        order: 2,
        ..BLANK
    },
    ProviderPreset {
        id: "chatgpt",
        title: "ChatGPT",
        description: "Модели OpenAI из вашей подписки ChatGPT Plus или Pro, без ключа API: вход через аккаунт.",
        base_url: Some("https://api.openai.com/v1"),
        needs_key: false,
        sign_in: Some(CHATGPT_SIGN_IN),
        api: LlmApi::Responses,
        usage_url: Some("https://chatgpt.com/settings/usage"),
        brand: Some("openai"),
        order: 3,
        ..BLANK
    },
    ProviderPreset {
        id: "openrouter",
        title: "OpenRouter",
        description: "Сотни моделей разных компаний и оплата в одном месте. Вместо ключа можно войти в аккаунт.",
        base_url: Some("https://openrouter.ai/api/v1"),
        key_url: Some("https://openrouter.ai/keys"),
        sign_in: Some(OPENROUTER_SIGN_IN),
        brand: Some("openrouter"),
        order: 4,
        ..BLANK
    },
    ProviderPreset {
        id: "ollama",
        title: "Ollama",
        description:
            "Открытые модели на этом Mac: бесплатно и без интернета. С инструментами справляются хуже облачных.",
        base_url: Some("http://localhost:11434/v1"),
        editable_url: true,
        needs_key: false,
        local: true,
        brand: Some("ollama"),
        order: 5,
        ..BLANK
    },
    ProviderPreset {
        id: "lmstudio",
        title: "LM Studio",
        description: "Открытые модели на этом Mac через сервер LM Studio.",
        base_url: Some("http://localhost:1234/v1"),
        editable_url: true,
        needs_key: false,
        local: true,
        brand: Some("lmstudio"),
        order: 6,
        ..BLANK
    },
    ProviderPreset {
        id: "yandexgpt",
        title: "YandexGPT",
        description: "Модели Яндекса через Yandex AI Studio: работает из России. Нужны API-ключ и ID каталога.",
        base_url: Some("https://llm.api.cloud.yandex.net/v1"),
        key_url: Some("https://yandex.cloud/ru/docs/ai-studio/quickstart/"),
        fields: &[PresetField { id: "folder", title: "ID каталога", placeholder: Some("b1g…") }],
        suggested: Some(Suggested { fast: "yandexgpt-lite/latest", smart: "yandexgpt/latest", more: &[] }),
        order: 7,
        ..BLANK
    },
    ProviderPreset {
        id: "gemini",
        title: "Gemini",
        description: "Модели Google через совместимый с OpenAI адрес.",
        base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        key_url: Some("https://aistudio.google.com/apikey"),
        brand: Some("gemini"),
        order: 8,
        ..BLANK
    },
    ProviderPreset {
        id: "deepseek",
        title: "DeepSeek",
        description: "Недорогие модели DeepSeek.",
        base_url: Some("https://api.deepseek.com/v1"),
        key_url: Some("https://platform.deepseek.com/api_keys"),
        brand: Some("deepseek"),
        order: 9,
        ..BLANK
    },
    ProviderPreset {
        id: "groq",
        title: "Groq",
        description: "Открытые модели на очень быстром железе: первое слово почти сразу.",
        base_url: Some("https://api.groq.com/openai/v1"),
        key_url: Some("https://console.groq.com/keys"),
        brand: Some("groq"),
        order: 10,
        ..BLANK
    },
    ProviderPreset {
        id: "mistral",
        title: "Mistral",
        description: "Модели Mistral AI.",
        base_url: Some("https://api.mistral.ai/v1"),
        key_url: Some("https://console.mistral.ai/api-keys"),
        brand: Some("mistral"),
        order: 11,
        ..BLANK
    },
    ProviderPreset {
        id: "custom-openai",
        title: "Свой, OpenAI-совместимый",
        description: "Любой сервис или сервер с /chat/completions: vLLM, llama.cpp, прокси.",
        editable_url: true,
        needs_key: false,
        order: 12,
        ..BLANK
    },
    ProviderPreset {
        id: "custom-anthropic",
        title: "Свой, Anthropic-совместимый",
        description: "Прокси или шлюз с API Anthropic.",
        kind: ProviderKind::Anthropic,
        editable_url: true,
        order: 13,
        ..BLANK
    },
];

pub fn preset_of(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS.iter().find(|preset| preset.id == id)
}

/// What goes to the service as the model: YandexGPT addresses its models through the folder.
pub fn model_for(preset: &str, options: &BTreeMap<String, String>, model: &str) -> String {
    match options.get("folder") {
        Some(folder) if preset == "yandexgpt" && !folder.is_empty() && !model.starts_with("gpt://") => {
            format!("gpt://{folder}/{model}")
        }
        _ => model.to_string(),
    }
}

/// Request headers a preset needs beyond the key.
pub fn headers_for(preset: &str, options: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    match options.get("folder") {
        Some(folder) if preset == "yandexgpt" && !folder.is_empty() => {
            BTreeMap::from([("OpenAI-Project".to_string(), folder.clone())])
        }
        _ => BTreeMap::new(),
    }
}

/// The suggested models, each once: what a service without a list still offers.
pub fn suggested_of(preset: &ProviderPreset) -> Vec<String> {
    let Some(suggested) = &preset.suggested else { return Vec::new() };
    let mut models: Vec<String> = Vec::new();
    for model in [suggested.fast, suggested.smart].into_iter().chain(suggested.more.iter().copied()) {
        if !model.is_empty() && !models.iter().any(|known| known == model) {
            models.push(model.to_string());
        }
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn describes_the_presets_as_the_panel_reads_them() {
        let ids: Vec<&str> = PROVIDER_PRESETS.iter().map(|preset| preset.id).collect();
        assert_eq!(&ids[..4], &["claude", "openai", "chatgpt", "openrouter"]);
        assert!(PROVIDER_PRESETS.windows(2).all(|pair| pair[0].order < pair[1].order));
        assert_eq!(
            serde_json::to_value(preset_of("yandexgpt").unwrap()).unwrap(),
            json!({
                "id": "yandexgpt",
                "title": "YandexGPT",
                "description": "Модели Яндекса через Yandex AI Studio: работает из России. Нужны API-ключ и ID каталога.",
                "kind": "openai",
                "baseUrl": "https://llm.api.cloud.yandex.net/v1",
                "editableUrl": false,
                "needsKey": true,
                "keyUrl": "https://yandex.cloud/ru/docs/ai-studio/quickstart/",
                "fields": [{"id": "folder", "title": "ID каталога", "placeholder": "b1g…"}],
                "suggested": {"fast": "yandexgpt-lite/latest", "smart": "yandexgpt/latest"},
                "order": 7,
            })
        );
        let ollama = serde_json::to_value(preset_of("ollama").unwrap()).unwrap();
        assert_eq!(ollama["local"], json!(true));
        assert_eq!(ollama["needsKey"], json!(false));
        // A sign-in shows only its name and words: the flow and its addresses stay in the gateway.
        let openrouter = serde_json::to_value(preset_of("openrouter").unwrap()).unwrap();
        assert_eq!(openrouter["signIn"], json!({"with": "OpenRouter"}));
        let chatgpt = serde_json::to_value(preset_of("chatgpt").unwrap()).unwrap();
        assert_eq!(chatgpt["signIn"], json!({"with": "ChatGPT", "label": "Continue with ChatGPT", "keyless": true}));
        assert_eq!((chatgpt["needsKey"].clone(), chatgpt.get("keyUrl")), (json!(false), None));
        assert_eq!(chatgpt["usageUrl"], json!("https://chatgpt.com/settings/usage"));
        assert!(chatgpt.get("api").is_none());
        assert_eq!(preset_of("chatgpt").unwrap().api, LlmApi::Responses);
        let signing_in: Vec<&str> =
            PROVIDER_PRESETS.iter().filter(|preset| preset.sign_in.is_some()).map(|preset| preset.id).collect();
        assert_eq!(signing_in, ["chatgpt", "openrouter"]);
        assert!(serde_json::to_value(preset_of("custom-anthropic").unwrap()).unwrap().get("baseUrl").is_none());
        assert_eq!(
            suggested_of(preset_of("claude").unwrap()),
            vec!["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"]
        );
    }

    #[test]
    fn addresses_yandexgpt_through_its_folder() {
        let options = BTreeMap::from([("folder".to_string(), "b1g".to_string())]);
        assert_eq!(model_for("yandexgpt", &options, "yandexgpt/latest"), "gpt://b1g/yandexgpt/latest");
        assert_eq!(model_for("yandexgpt", &options, "gpt://x/y"), "gpt://x/y");
        assert_eq!(model_for("openai", &options, "gpt-5"), "gpt-5");
        assert_eq!(headers_for("yandexgpt", &options).get("OpenAI-Project").map(String::as_str), Some("b1g"));
        assert!(headers_for("yandexgpt", &BTreeMap::new()).is_empty());
    }
}
