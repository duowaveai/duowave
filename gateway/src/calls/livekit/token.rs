//! Join tokens for the family room. Kept apart from the bridge: a token is a JWT signed with the API secret
//! (HS256, made here: no SDK needed), the bridge needs LiveKit's native client (feature `livekit`).

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::config::LiveKitConfig;

/// The gateway's own identity in the room: it is the station there.
pub const BRIDGE_IDENTITY: &str = "duowave-station";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoGrant<'a> {
    room_join: bool,
    room: &'a str,
    can_publish: bool,
    can_subscribe: bool,
    can_publish_data: bool,
}

/// The claims as livekit-server-sdk wrote them: the name, the grant, then issuer, expiry, not-before and subject.
#[derive(Serialize)]
struct Claims<'a> {
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    video: VideoGrant<'a>,
    iss: &'a str,
    exp: u64,
    nbf: u64,
    sub: &'a str,
}

/// A join token for the room: the gateway's own, or one for a family member's app.
pub fn room_token(config: &LiveKitConfig, identity: &str, name: &str, ttl: Duration) -> String {
    let now = crate::utils::time::now_ms() / 1000;
    let claims = Claims {
        name,
        video: VideoGrant {
            room_join: true,
            room: &config.room,
            can_publish: true,
            can_subscribe: true,
            can_publish_data: true,
        },
        iss: &config.api_key,
        exp: now + ttl.as_secs(),
        nbf: now,
        sub: identity,
    };
    let header = B64URL.encode(br#"{"alg":"HS256"}"#);
    let payload = B64URL.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let signed = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(config.api_secret.as_bytes()).expect("HMAC takes a key of any size");
    mac.update(signed.as_bytes());
    let signature = B64URL.encode(mac.finalize().into_bytes());
    format!("{signed}.{signature}")
}

/// A minute's token to list the rooms: the panel's «Проверить» asks the server with it whether the key and the
/// secret are right.
pub fn list_token(config: &LiveKitConfig) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ListGrant {
        room_list: bool,
    }
    #[derive(Serialize)]
    struct ListClaims<'a> {
        video: ListGrant,
        iss: &'a str,
        exp: u64,
        nbf: u64,
        sub: &'a str,
    }
    let now = crate::utils::time::now_ms() / 1000;
    let claims = ListClaims {
        video: ListGrant { room_list: true },
        iss: &config.api_key,
        exp: now + 60,
        nbf: now,
        sub: "duowave",
    };
    let header = B64URL.encode(br#"{"alg":"HS256"}"#);
    let payload = B64URL.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let signed = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(config.api_secret.as_bytes()).expect("HMAC takes a key of any size");
    mac.update(signed.as_bytes());
    let signature = B64URL.encode(mac.finalize().into_bytes());
    format!("{signed}.{signature}")
}

/// A family member's identity in the room, by name: the same for the home page, `duowave livekit-token` and a call's
/// link.
pub fn app_identity(name: &str) -> String {
    let hex = hex::encode(name.as_bytes());
    format!("app-{}", &hex[..hex.len().min(24)])
}

/// A join token for a family member's app is kept on the phone a month, to call from outside: the home page's
/// (`/calls/livekit/token`) and `duowave livekit-token`'s by default.
pub const APP_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

/// Who an access is for when no name is given.
pub const GUEST: &str = "Гость";

/// The name an access is given for: the one asked, trimmed, or «Гость».
pub fn app_name(asked: Option<&str>) -> &str {
    asked.map(str::trim).filter(|name| !name.is_empty()).unwrap_or(GUEST)
}

/// What a family member's app joins the room with: where it reaches LiveKit (the public address when there is one)
/// and a token for its name. The LiveKit page takes it as JSON («Или вставьте доступ»).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppAccess {
    pub url: String,
    pub token: String,
}

pub fn app_access(config: &LiveKitConfig, name: &str, ttl: Duration) -> AppAccess {
    AppAccess {
        url: config.public_url.clone().unwrap_or_else(|| config.url.clone()),
        token: room_token(config, &app_identity(name), name, ttl),
    }
}

/// A link into the room for a contact, and the identity it lets in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub identity: String,
    pub link: String,
}

/// The LiveKit page with an access in its fragment (`#access=`, never sent to a server): one tap joins the room.
pub fn access_link(config: &LiveKitConfig, name: &str, ttl: Duration) -> Option<Invite> {
    let page = config.page_url.as_deref()?;
    let access = serde_json::to_string(&app_access(config, name, ttl)).unwrap_or_default();
    let link = format!("{page}#access={}", B64URL.encode(access));
    Some(Invite { identity: app_identity(name), link })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn config() -> LiveKitConfig {
        LiveKitConfig {
            url: "ws://127.0.0.1:7880".into(),
            public_url: None,
            page_url: None,
            api_key: "key".into(),
            api_secret: "a-secret-of-thirty-two-characters!!".into(),
            room: "home".into(),
        }
    }

    fn claims(token: &str) -> Value {
        let payload = token.split('.').nth(1).unwrap();
        serde_json::from_slice(&B64URL.decode(payload).unwrap()).unwrap()
    }

    #[test]
    fn lets_one_identity_into_the_family_room_with_publish_subscribe_and_data_rights() {
        let config = LiveKitConfig { url: "ws://lk".into(), ..config() };
        let token = room_token(&config, "app-mom", "Мама", Duration::from_secs(3600));
        let claims = claims(&token);
        assert_eq!(claims["sub"], "app-mom");
        assert_eq!(claims["name"], "Мама");
        assert_eq!(claims["iss"], "key");
        assert_eq!(
            claims["video"],
            serde_json::json!({"roomJoin": true, "room": "home", "canPublish": true, "canSubscribe": true, "canPublishData": true})
        );
        assert!(claims["exp"].as_u64().unwrap() - claims["nbf"].as_u64().unwrap() <= 3600);
        // HS256 over the first two parts with the API secret.
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(config.api_secret.as_bytes()).unwrap();
        mac.update(signed.as_bytes());
        mac.verify_slice(&B64URL.decode(signature).unwrap()).unwrap();
        assert_eq!(B64URL.decode(token.split('.').next().unwrap()).unwrap(), br#"{"alg":"HS256"}"#);
    }

    #[test]
    fn puts_the_public_address_and_a_token_for_the_contact_into_the_pages_fragment() {
        let config = LiveKitConfig {
            public_url: Some("wss://lk.example".into()),
            page_url: Some("https://calls.example/calls/livekit".into()),
            ..config()
        };
        let invite = access_link(&config, "Мама", Duration::from_secs(86_400)).unwrap();
        assert_eq!(invite.identity, app_identity("Мама"));
        let (page, fragment) = invite.link.split_once("#access=").unwrap();
        assert_eq!(page, "https://calls.example/calls/livekit");
        let access: Value = serde_json::from_slice(&B64URL.decode(fragment).unwrap()).unwrap();
        assert_eq!(access["url"], "wss://lk.example");
        let token = claims(access["token"].as_str().unwrap());
        assert_eq!((token["sub"].as_str(), token["name"].as_str()), (Some(invite.identity.as_str()), Some("Мама")));
        assert_eq!(app_identity("Мама"), "app-d09cd0b0d0bcd0b0");
        assert_eq!(app_identity("Константин Константинопольский").len(), 4 + 24);
    }

    #[test]
    fn gives_an_app_the_public_address_and_a_token_for_its_name_a_guest_when_none() {
        let access = app_access(&config(), "Мама", APP_TOKEN_TTL);
        assert_eq!(access.url, "ws://127.0.0.1:7880");
        let token = claims(&access.token);
        assert_eq!(
            (token["sub"].as_str(), token["name"].as_str()),
            (Some(app_identity("Мама").as_str()), Some("Мама"))
        );
        assert_eq!(token["exp"].as_u64().unwrap() - token["nbf"].as_u64().unwrap(), 30 * 24 * 3600);
        let public = LiveKitConfig { public_url: Some("wss://lk.example".into()), ..config() };
        assert_eq!(app_access(&public, "Мама", APP_TOKEN_TTL).url, "wss://lk.example");
        assert_eq!(serde_json::to_value(&access).unwrap().as_object().unwrap().len(), 2);
        assert_eq!((app_name(Some("  Мама ")), app_name(Some("  ")), app_name(None)), ("Мама", "Гость", "Гость"));
    }

    #[test]
    fn gives_none_without_the_pages_address() {
        assert!(access_link(&config(), "Мама", Duration::from_secs(86_400)).is_none());
    }
}
