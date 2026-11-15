//! The gateway, over its Host API for apps (`DUOWAVE_URL`, `DUOWAVE_APP_TOKEN`): the household's language, and the
//! letter `system/network` → `connections` that tells it the entrances — sent after the engine starts, on every change
//! and every 30 s as «alive» (the gateway takes connections without a letter for 90 s as gone). A refusal (no «да» for
//! «VPN → Сеть шлюза» yet, a gateway without `system/network`) goes to the journal once, not every 30 s.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::net::{Ask, send};

pub struct Gateway {
    url: String,
    token: String,
    locale: Mutex<Option<(String, Instant)>>,
    last_refusal: Mutex<Option<String>>,
}

impl Gateway {
    pub fn new(url: String, token: String) -> Gateway {
        Gateway {
            url: url.trim_end_matches('/').to_string(),
            token,
            locale: Mutex::new(None),
            last_refusal: Mutex::new(None),
        }
    }

    /// The household's language now; kept for 30 s, English when the gateway does not say.
    pub async fn locale(&self) -> String {
        if let Some((locale, at)) = self.locale.lock().unwrap().clone()
            && at.elapsed() < Duration::from_secs(30)
        {
            return locale;
        }
        let url = format!("{}/api/v1/apps/host/locale", self.url);
        let ask = Ask::get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_millis(800));
        let fresh = send(ask)
            .await
            .ok()
            .and_then(|answer| answer.json())
            .and_then(|body| body.get("locale").and_then(Value::as_str).map(str::to_string));
        let mut cached = self.locale.lock().unwrap();
        match fresh {
            Some(locale) => {
                *cached = Some((locale.clone(), Instant::now()));
                locale
            }
            None => cached.as_ref().map(|(locale, _)| locale.clone()).unwrap_or_else(|| "en".into()),
        }
    }

    /// The letter with the entrances. Ok — the gateway took it.
    pub async fn connections(&self, items: Vec<Value>, wait: Duration) -> bool {
        let url = format!("{}/api/v1/apps/host/call/bus.call", self.url);
        let body = json!({ "to": "system/network", "handle": "connections", "input": { "items": items } });
        let ask =
            Ask::json("POST", &url, &body).header("authorization", format!("Bearer {}", self.token)).timeout(wait);
        let refusal = match send(ask).await {
            Ok(answer) if (200..300).contains(&answer.status) => None,
            Ok(answer) => Some(format!(
                "HTTP {}: {}",
                answer.status,
                answer
                    .json()
                    .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default()
            )),
            Err(error) => Some(error),
        };
        let mut last = self.last_refusal.lock().unwrap();
        match refusal {
            None => {
                if last.take().is_some() {
                    eprintln!("own-server: the gateway takes the connections now");
                }
                true
            }
            Some(why) => {
                if last.as_deref() != Some(why.as_str()) {
                    eprintln!(
                        "own-server: the gateway did not take the connections ({why}); everything goes directly until it does"
                    );
                    *last = Some(why);
                }
                false
            }
        }
    }
}
