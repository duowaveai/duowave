//! The models' prices (docs/plans/router-and-works.md, block A, 2). A price is dollars per million tokens of the fresh
//! input, the output, the cache read and the cache written. Where it comes from, in this order: written by hand in the
//! panel → the provider's own list of models (OpenRouter gives its prices with it) → the snapshot of models.dev built
//! into the program (`prices.json`, made by `scripts/dev/prices.mjs` before a release; the gateway never asks
//! models.dev itself) → none. Only an exact name of the model counts: no price of a «similar» one.
//!
//! A model at home costs 0 (`local`). A plan's sign-in (ChatGPT's) is priced as its key would be: the dollars are an
//! estimate, never a bill.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Tokens;
use crate::config::DialogProvider;
use crate::dialog::brain::presets::{preset_of, PROVIDER_PRESETS};
use crate::dialog::llm::brain::{host_of, ENV_PROVIDER};
use crate::dialog::llm::openai_chat::OPENAI_BASE_URL;
use crate::oauth::Flow;
use crate::storage::Db;
use crate::utils::time::now_ms;

/// The snapshot of models.dev's prices, by «Мозг»'s presets: `{schema, from, at, providers: {preset: {model:
/// [input, output, cacheRead, cacheWrite, context]}}}`.
const SNAPSHOT: &str = include_str!("prices.json");

/// Where a price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PriceSource {
    /// Written by hand in the panel.
    Manual,
    /// The provider's own list of models said it.
    Provider,
    /// The snapshot of models.dev built into the program.
    Snapshot,
    /// A model at home: nothing to pay.
    Local,
}

impl PriceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PriceSource::Manual => "manual",
            PriceSource::Provider => "provider",
            PriceSource::Snapshot => "snapshot",
            PriceSource::Local => "local",
        }
    }

    fn parse(text: &str) -> Option<PriceSource> {
        [PriceSource::Manual, PriceSource::Provider, PriceSource::Snapshot, PriceSource::Local]
            .into_iter()
            .find(|source| source.as_str() == text)
    }
}

/// A price, dollars per million tokens, and where it came from: what a deal keeps beside its dollars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: Option<f64>,
    pub output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    pub from: PriceSource,
}

impl Price {
    /// A model at home.
    pub fn local() -> Price {
        Price { input: Some(0.0), output: Some(0.0), cache_read: None, cache_write: None, from: PriceSource::Local }
    }

    /// What the tokens cost at this price, in millionths of a dollar: tokens × dollars per million. The cache without
    /// a price of its own costs as the input; without the input's or the output's price nothing is known.
    pub fn cost(&self, tokens: &Tokens) -> Option<u64> {
        let (input, output) = (self.input?, self.output?);
        let micro = tokens.input as f64 * input
            + tokens.output as f64 * output
            + tokens.cache_read as f64 * self.cache_read.unwrap_or(input)
            + tokens.cache_write as f64 * self.cache_write.unwrap_or(input);
        Some(micro.round().max(0.0) as u64)
    }
}

/// A price written by hand, with the model's context if one knows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Written {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub context: Option<u64>,
}

/// The model of .env, as the pool needs it to find its prices: how it speaks and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvModel {
    pub kind: DialogProvider,
    pub base_url: Option<String>,
}

/// What the pool knows of a provider: the preset its prices go by, and whether it signs in with a plan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Book {
    pub preset: Option<String>,
    pub plan: bool,
}

/// One model in the snapshot: input, output, cache read, cache write (dollars per million), context (tokens).
type Line = (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<u64>);

#[derive(Debug, Default, Deserialize)]
struct Snapshot {
    #[serde(default)]
    providers: HashMap<String, HashMap<String, Line>>,
}

/// The snapshot, read once.
fn snapshot() -> &'static Snapshot {
    static PARSED: OnceLock<Snapshot> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(SNAPSHOT).unwrap_or_else(|error| {
            tracing::error!(target: "router", "the prices' snapshot does not read: {error}");
            Snapshot::default()
        })
    })
}

/// The snapshot's provider a preset is priced by: ChatGPT's plan as OpenAI's key.
fn priced_as(preset: &str) -> &str {
    match preset {
        "chatgpt" => "openai",
        other => other,
    }
}

pub struct Pool {
    db: Arc<Db>,
    env: EnvModel,
}

impl Pool {
    pub fn new(db: Arc<Db>, env: EnvModel) -> Pool {
        Pool { db, env }
    }

    /// The preset of a provider and its sign-in: from «Мозг»'s table, or — for `env` — by the .env model's kind and
    /// address. A provider gone since: nothing known.
    pub fn book(&self, provider: &str) -> Book {
        if provider == ENV_PROVIDER {
            return Book { preset: env_preset(&self.env).map(str::to_string), plan: false };
        }
        let preset: Option<String> = self
            .db
            .with(|c| {
                c.query_row("SELECT preset FROM llm_providers WHERE id = ?1", [provider], |row| row.get(0)).optional()
            })
            .unwrap_or_else(|error| {
                tracing::warn!(target: "router", "the provider {provider} not read: {error}");
                None
            });
        let plan = preset
            .as_deref()
            .and_then(preset_of)
            .and_then(|preset| preset.sign_in.as_ref())
            .is_some_and(|sign_in| matches!(sign_in.flow, Flow::Tokens(_)));
        Book { preset, plan }
    }

    /// The price of a model of a provider whose preset is `preset`, in the pool's order: written by hand, said by the
    /// provider, the snapshot's; None — no price.
    pub fn price(&self, provider: &str, preset: Option<&str>, model: &str) -> Option<Price> {
        self.priced(provider, preset, model, true)
    }

    /// The price a model would have without the one written by hand: the provider's or the snapshot's — what «вернуть
    /// цену из снимка» in the panel brings back.
    pub fn unwritten(&self, provider: &str, preset: Option<&str>, model: &str) -> Option<Price> {
        self.priced(provider, preset, model, false)
    }

    fn priced(&self, provider: &str, preset: Option<&str>, model: &str, written: bool) -> Option<Price> {
        if let Some(price) = self.stored(provider, model, written) {
            return Some(price);
        }
        let line = snapshot().providers.get(priced_as(preset?))?.get(model)?;
        Some(Price {
            input: line.0,
            output: line.1,
            cache_read: line.2,
            cache_write: line.3,
            from: PriceSource::Snapshot,
        })
    }

    /// The model's context in tokens, as far as it is known: written, said, the snapshot's.
    pub fn context(&self, provider: &str, preset: Option<&str>, model: &str) -> Option<u64> {
        let stored: Option<u64> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT context FROM model_prices WHERE provider = ?1 AND model = ?2 AND context IS NOT NULL
                     ORDER BY CASE source WHEN 'manual' THEN 0 ELSE 1 END LIMIT 1",
                    params![provider, model],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
            })
            .ok()
            .flatten()
            .map(|context| context.max(0) as u64);
        stored.or_else(|| snapshot().providers.get(priced_as(preset?))?.get(model)?.4)
    }

    /// The price written by hand, or the provider's own: the first of them; without `written` — only the provider's.
    fn stored(&self, provider: &str, model: &str, written: bool) -> Option<Price> {
        self.db
            .with(|c| {
                c.query_row(
                    "SELECT source, input, output, cache_read, cache_write FROM model_prices
                     WHERE provider = ?1 AND model = ?2 AND (?3 OR source <> 'manual')
                     ORDER BY CASE source WHEN 'manual' THEN 0 WHEN 'provider' THEN 1 ELSE 2 END LIMIT 1",
                    params![provider, model, written],
                    |row| {
                        Ok(Price {
                            from: PriceSource::parse(&row.get::<_, String>(0)?).unwrap_or(PriceSource::Manual),
                            input: row.get(1)?,
                            output: row.get(2)?,
                            cache_read: row.get(3)?,
                            cache_write: row.get(4)?,
                        })
                    },
                )
                .optional()
            })
            .unwrap_or_else(|error| {
                tracing::warn!(target: "router", "the price of {model} not read: {error}");
                None
            })
    }

    /// A price written by hand in the panel; None takes it away (the provider's or the snapshot's counts again).
    pub fn write(&self, provider: &str, model: &str, price: Option<&Written>) -> rusqlite::Result<()> {
        self.db.with(|c| {
            match price {
                Some(price) => c.execute(
                    "INSERT INTO model_prices (provider, model, source, input, output, cache_read, cache_write, context,
                        updated_at)
                     VALUES (?1, ?2, 'manual', ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT (provider, model, source) DO UPDATE SET input = excluded.input,
                        output = excluded.output, cache_read = excluded.cache_read,
                        cache_write = excluded.cache_write, context = excluded.context,
                        updated_at = excluded.updated_at",
                    params![
                        provider,
                        model,
                        price.input,
                        price.output,
                        price.cache_read,
                        price.cache_write,
                        price.context.map(|context| context as i64),
                        now_ms() as i64
                    ],
                )?,
                None => c.execute(
                    "DELETE FROM model_prices WHERE provider = ?1 AND model = ?2 AND source = 'manual'",
                    params![provider, model],
                )?,
            };
            Ok(())
        })
    }

    /// The prices a provider's own list of models gave — OpenRouter's `data[].pricing`, dollars per token (as text or
    /// as numbers), and `context_length` — kept as the provider's, in place of what it said before. How many models
    /// had a price.
    pub fn heard(&self, provider: &str, list: &Value) -> rusqlite::Result<usize> {
        let per_million = |value: Option<&Value>| -> Option<f64> {
            let per_token = match value? {
                Value::String(text) => text.trim().parse::<f64>().ok()?,
                Value::Number(number) => number.as_f64()?,
                _ => return None,
            };
            (per_token.is_finite() && per_token >= 0.0).then_some(per_token * 1_000_000.0)
        };
        let models: Vec<(String, Written)> = list
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let id = model.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
                let pricing = model.get("pricing")?;
                let price = Written {
                    input: Some(per_million(pricing.get("prompt"))?),
                    output: Some(per_million(pricing.get("completion"))?),
                    cache_read: per_million(pricing.get("input_cache_read")),
                    cache_write: per_million(pricing.get("input_cache_write")),
                    context: model.get("context_length").and_then(Value::as_u64),
                };
                Some((id.to_string(), price))
            })
            .collect();
        let now = now_ms() as i64;
        self.db.transaction(|t| {
            t.execute("DELETE FROM model_prices WHERE provider = ?1 AND source = 'provider'", [provider])?;
            for (model, price) in &models {
                t.execute(
                    "INSERT INTO model_prices (provider, model, source, input, output, cache_read, cache_write, context,
                        updated_at)
                     VALUES (?1, ?2, 'provider', ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        provider,
                        model,
                        price.input,
                        price.output,
                        price.cache_read,
                        price.cache_write,
                        price.context.map(|context| context as i64),
                        now
                    ],
                )?;
            }
            Ok(models.len())
        })
    }
}

/// The preset the model of .env is priced by: Claude for Anthropic's API, else the preset whose address it uses
/// (OpenAI's own when it names none); a server of its own has none.
fn env_preset(env: &EnvModel) -> Option<&'static str> {
    match env.kind {
        DialogProvider::Anthropic => Some("claude"),
        DialogProvider::OpenAi => {
            let host = host_of(env.base_url.as_deref().unwrap_or(OPENAI_BASE_URL));
            // Not a plan's sign-in: .env has a key, never ChatGPT's sign-in at OpenAI's address.
            PROVIDER_PRESETS
                .iter()
                .filter(|preset| {
                    !preset.sign_in.as_ref().is_some_and(|sign_in| matches!(sign_in.flow, Flow::Tokens(_)))
                })
                .find(|preset| preset.base_url.is_some_and(|url| host_of(url) == host))
                .map(|preset| preset.id)
        }
        DialogProvider::Echo | DialogProvider::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pool(env: EnvModel) -> Pool {
        Pool::new(Arc::new(Db::memory()), env)
    }

    fn openai_env() -> EnvModel {
        EnvModel { kind: DialogProvider::OpenAi, base_url: None }
    }

    /// A provider of «Мозг» as its table keeps it.
    fn add_provider(pool: &Pool, id: &str, preset: &str) {
        pool.db
            .with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, models, last_check,
                        created_at, updated_at)
                     VALUES (?1, ?2, 'openai', ?1, NULL, '{}', '[]', NULL, '2026-11-15', '2026-11-15')",
                    params![id, preset],
                )
            })
            .unwrap();
    }

    #[test]
    fn the_snapshot_reads_and_has_the_brains_providers() {
        let providers = &snapshot().providers;
        for preset in ["claude", "openai", "openrouter", "gemini", "deepseek", "groq", "mistral"] {
            assert!(providers.get(preset).is_some_and(|models| !models.is_empty()), "{preset} in the snapshot");
        }
        // Only the presets of «Мозг»: no other provider of models.dev, ChatGPT's plan priced as OpenAI's key.
        for id in providers.keys() {
            assert!(preset_of(id).is_some(), "{id} is not a preset of «Мозг»");
        }
        assert!(!providers.contains_key("chatgpt"));
        let haiku = providers["claude"]["claude-haiku-5-5"];
        assert!(haiku.0.is_some() && haiku.1.is_some(), "{haiku:?}");
    }

    #[test]
    fn prices_by_hand_then_the_providers_then_the_snapshot_then_none() {
        let pool = pool(openai_env());
        let snapshot_price = pool.price("claude", Some("claude"), "claude-haiku-5-5").unwrap();
        assert_eq!(snapshot_price.from, PriceSource::Snapshot);
        assert_eq!(pool.price("claude", Some("claude"), "claude-haiku"), None, "only the exact name");
        assert_eq!(pool.price("custom", None, "claude-haiku-5-5"), None, "no preset, no snapshot");

        let list = json!({"data": [
            {"id": "claude-haiku-5-5", "pricing": {"prompt": "0.000002", "completion": "0.00001",
                "input_cache_read": "0.0000002"}, "context_length": 200000},
            {"id": "free/one", "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "no-price"},
        ]});
        assert_eq!(pool.heard("claude", &list).unwrap(), 2);
        let said = pool.price("claude", Some("claude"), "claude-haiku-5-5").unwrap();
        assert_eq!(said.from, PriceSource::Provider);
        assert!((said.input.unwrap() - 2.0).abs() < 1e-9 && (said.cache_read.unwrap() - 0.2).abs() < 1e-9);
        assert_eq!(pool.context("claude", Some("claude"), "claude-haiku-5-5"), Some(200000));

        let by_hand = Written { input: Some(1.0), output: Some(4.0), ..Written::default() };
        pool.write("claude", "claude-haiku-5-5", Some(&by_hand)).unwrap();
        let manual = pool.price("claude", Some("claude"), "claude-haiku-5-5").unwrap();
        assert_eq!((manual.from, manual.input, manual.output), (PriceSource::Manual, Some(1.0), Some(4.0)));
        // What counts without the written one: the provider's, and with no provider's — the snapshot's.
        assert_eq!(pool.unwritten("claude", Some("claude"), "claude-haiku-5-5"), Some(said));
        let sonnet = pool.price("claude", Some("claude"), "claude-sonnet-5-5").unwrap();
        pool.write("claude", "claude-sonnet-5-5", Some(&by_hand)).unwrap();
        assert_eq!(pool.price("claude", Some("claude"), "claude-sonnet-5-5").unwrap().from, PriceSource::Manual);
        assert_eq!(pool.unwritten("claude", Some("claude"), "claude-sonnet-5-5"), Some(sonnet));
        // A price only for a provider of one's own, which no snapshot knows.
        pool.write("custom", "mine", Some(&by_hand)).unwrap();
        assert_eq!(pool.price("custom", None, "mine").unwrap().from, PriceSource::Manual);
        assert_eq!(pool.unwritten("custom", None, "mine"), None, "a model of one's own: only the written price");

        // Taken away: the provider's counts again; a new list replaces the old one.
        pool.write("claude", "claude-haiku-5-5", None).unwrap();
        assert_eq!(pool.price("claude", Some("claude"), "claude-haiku-5-5").unwrap().from, PriceSource::Provider);
        pool.heard("claude", &json!({"data": []})).unwrap();
        assert_eq!(pool.price("claude", Some("claude"), "claude-haiku-5-5").unwrap().from, PriceSource::Snapshot);
    }

    #[test]
    fn costs_tokens_at_a_price_in_millionths_of_a_dollar() {
        let price = Price {
            input: Some(3.0),
            output: Some(15.0),
            cache_read: Some(0.3),
            cache_write: None,
            from: PriceSource::Snapshot,
        };
        let tokens = Tokens { input: 1000, output: 100, cache_read: 10_000, cache_write: 2000 };
        // 1000 × 3 + 100 × 15 + 10 000 × 0.3 + 2000 × 3 (the cache written as the input).
        assert_eq!(price.cost(&tokens), Some(3000 + 1500 + 3000 + 6000));
        assert_eq!(Price::local().cost(&tokens), Some(0));
        assert_eq!(Price { output: None, ..price }.cost(&tokens), None);
        let json = serde_json::to_value(Price::local()).unwrap();
        assert_eq!(json, json!({"input": 0.0, "output": 0.0, "from": "local"}));
    }

    #[test]
    fn knows_a_providers_preset_and_its_plan() {
        let pool = pool(openai_env());
        add_provider(&pool, "chatgpt", "chatgpt");
        add_provider(&pool, "openrouter-2", "openrouter");
        assert_eq!(pool.book("chatgpt"), Book { preset: Some("chatgpt".into()), plan: true });
        // OpenRouter's sign-in brings a key: by the key.
        assert_eq!(pool.book("openrouter-2"), Book { preset: Some("openrouter".into()), plan: false });
        assert_eq!(pool.book("gone"), Book::default());
        // ChatGPT's plan is priced as OpenAI's key.
        let plan = pool.price("chatgpt", Some("chatgpt"), "gpt-5").unwrap();
        assert_eq!(plan.from, PriceSource::Snapshot);
        assert_eq!(plan, pool.price("openai", Some("openai"), "gpt-5").unwrap());

        let env = |kind, url: Option<&str>| env_preset(&EnvModel { kind, base_url: url.map(str::to_string) });
        assert_eq!(env(DialogProvider::Anthropic, None), Some("claude"));
        assert_eq!(env(DialogProvider::OpenAi, None), Some("openai"));
        assert_eq!(env(DialogProvider::OpenAi, Some("https://openrouter.ai/api/v1")), Some("openrouter"));
        assert_eq!(env(DialogProvider::OpenAi, Some("https://api.deepseek.com/v1")), Some("deepseek"));
        assert_eq!(env(DialogProvider::OpenAi, Some("https://llm.example.com/v1")), None);
        assert_eq!(env(DialogProvider::Echo, None), None);
        let pool = Pool::new(
            Arc::new(Db::memory()),
            EnvModel { kind: DialogProvider::Anthropic, base_url: Some("https://api.anthropic.com".into()) },
        );
        assert_eq!(pool.book(ENV_PROVIDER), Book { preset: Some("claude".into()), plan: false });
    }
}
