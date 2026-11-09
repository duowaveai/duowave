//! `duowave livekit-token`: a join token for the family's LiveKit room, for a family member's app or the LiveKit page
//! (src/tools/livekit-token.ts, `pnpm livekit-token`). The same access the gateway gives at
//! `GET /calls/livekit/token` from the home network, made here without the gateway running.
//!
//!   duowave livekit-token --name "Мама" [--days 30]
//!
//! Takes LiveKit as the panel set it up («Дом» → «Коннекторы» → LiveKit: the gateway's database and key; the public
//! address when there is one). On a gateway that never started with the panel's connectors, .env seeds it first, as
//! the gateway itself would. Prints {"url","token"}: paste it into the page's «Или вставьте доступ».

use std::time::Duration;

use super::{option, Failure, Usage};
use crate::calls::livekit::{app_access, app_name, AppAccess};
use crate::config::LiveKitConfig;
use crate::connectors::settings::ConnectorSettings;

pub fn main(args: &[String]) -> anyhow::Result<()> {
    let config = super::config()?;
    let Some(livekit) = ConnectorSettings::open(&config)?.livekit() else {
        return Err(Failure("LiveKit is not set up: the panel, «Дом» → «Коннекторы» → LiveKit".into()).into());
    };
    println!("{}", serde_json::to_string(&access(&livekit, args)?)?);
    Ok(())
}

/// The access for `--name` (a guest without it), good for `--days` (30 without it).
pub fn access(config: &LiveKitConfig, args: &[String]) -> anyhow::Result<AppAccess> {
    let name = app_name(option(args, "name"));
    let days = match option(args, "days") {
        None => 30.0,
        Some(days) => days
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|days| days.is_finite() && *days > 0.0 && *days <= 3650.0)
            .ok_or_else(|| Usage(format!("--days takes a number of days, not {days:?}")))?,
    };
    Ok(app_access(config, name, Duration::from_secs_f64(days * 24.0 * 3600.0)))
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use base64::Engine as _;
    use hmac::{Hmac, KeyInit, Mac};
    use serde_json::Value;
    use sha2::Sha256;

    use super::*;
    use crate::calls::livekit::app_identity;

    fn config() -> LiveKitConfig {
        LiveKitConfig {
            url: "ws://127.0.0.1:7880".into(),
            public_url: Some("wss://livekit.example.com".into()),
            page_url: None,
            api_key: "key".into(),
            api_secret: "a-secret-of-thirty-two-characters!!".into(),
            room: "family".into(),
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    fn claims(token: &str) -> Value {
        serde_json::from_slice(&B64URL.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn makes_a_token_for_the_name_into_the_family_room_for_the_days_asked() {
        let given = access(&config(), &args(&["--name", "Мама", "--days", "7"])).unwrap();
        assert_eq!(given.url, "wss://livekit.example.com", "the address people's apps reach");
        let claims = claims(&given.token);
        assert_eq!(claims["sub"].as_str(), Some(app_identity("Мама").as_str()));
        assert_eq!(claims["name"], "Мама");
        assert_eq!(claims["iss"], "key");
        assert_eq!(
            claims["video"],
            serde_json::json!({"roomJoin": true, "room": "family", "canPublish": true, "canSubscribe": true, "canPublishData": true})
        );
        assert_eq!(claims["exp"].as_u64().unwrap() - claims["nbf"].as_u64().unwrap(), 7 * 24 * 3600);
        let (signed, signature) = given.token.rsplit_once('.').unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(config().api_secret.as_bytes()).unwrap();
        mac.update(signed.as_bytes());
        mac.verify_slice(&B64URL.decode(signature).unwrap()).unwrap();
        // What the page pastes: {"url","token"}.
        let printed = serde_json::to_string(&given).unwrap();
        assert!(printed.starts_with(r#"{"url":"wss://livekit.example.com","token":""#), "{printed}");
    }

    #[test]
    fn gives_a_guest_a_month_without_options_and_refuses_days_that_are_not_a_number() {
        let plain = LiveKitConfig { public_url: None, ..config() };
        let lifetime = |given: &[&str]| {
            let token = claims(&access(&plain, &args(given)).unwrap().token);
            token["exp"].as_u64().unwrap() - token["nbf"].as_u64().unwrap()
        };
        let guest = access(&plain, &[]).unwrap();
        assert_eq!(guest.url, "ws://127.0.0.1:7880");
        assert_eq!(claims(&guest.token)["name"], "Гость");
        assert_eq!(lifetime(&[]), 30 * 24 * 3600);
        assert_eq!(lifetime(&["--days", "0.5"]), 12 * 3600);
        for days in ["month", "0", "-1", "NaN"] {
            let refused = access(&plain, &args(&["--days", days])).unwrap_err();
            assert!(refused.is::<Usage>(), "{days}: {refused}");
        }
    }
}
