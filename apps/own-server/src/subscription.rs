//! A subscription as panels of 3x-ui, Marzban, Remnawave give it: a base64 list of links (standard or URL-safe, with
//! or without padding) or the links one per line; its name in the header `profile-title` (`base64:…` too). Clash YAML
//! and sing-box JSON — later. Reading it over the network is net.rs's; keeping the last good list is the store's.

use base64::Engine as _;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD_NO_PAD, URL_SAFE_NO_PAD};

const LENIENT: GeneralPurposeConfig =
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_encode_padding(false);
const STANDARD_ANY: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::STANDARD, LENIENT);
const URL_SAFE_ANY: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::URL_SAFE, LENIENT);

/// A base64 text as UTF-8, whichever alphabet and padding; None when it is not one.
pub fn base64_text(text: &str) -> Option<String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    let bytes = STANDARD_ANY
        .decode(&compact)
        .or_else(|_| URL_SAFE_ANY.decode(&compact))
        .or_else(|_| STANDARD_NO_PAD.decode(compact.trim_end_matches('=')))
        .or_else(|_| URL_SAFE_NO_PAD.decode(compact.trim_end_matches('=')))
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// A subscription's body as links one per line: decoded when it is base64, as is when it already holds links.
pub fn decode_body(body: &str) -> String {
    let trimmed = body.trim().trim_start_matches('\u{feff}');
    if trimmed.contains("://") || trimmed.contains("[Interface]") {
        return trimmed.to_string();
    }
    match base64_text(trimmed) {
        Some(text) if text.contains("://") || text.contains("[Interface]") => text,
        _ => trimmed.to_string(),
    }
}

/// The subscription's own name from its `profile-title` header.
pub fn profile_title(header: &str) -> Option<String> {
    let header = header.trim();
    let title = match header.strip_prefix("base64:") {
        Some(encoded) => base64_text(encoded)?,
        None => header.to_string(),
    };
    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// The host of a subscription's address — what the window may show; the path is its secret.
pub fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};

    const LINKS: &str = "vless://id@a.example:443?security=tls#A\ntrojan://p@b.example:443#B\n";

    #[test]
    fn a_body_in_any_base64_or_as_lines() {
        for encoded in [
            STANDARD.encode(LINKS),
            STANDARD_NO_PAD.encode(LINKS),
            URL_SAFE.encode(LINKS),
            URL_SAFE_NO_PAD.encode(LINKS),
            // Wrapped at 76 characters, as some panels give it.
            STANDARD
                .encode(LINKS)
                .as_bytes()
                .chunks(20)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect::<Vec<_>>()
                .join("\n"),
        ] {
            assert_eq!(decode_body(&encoded), LINKS, "{encoded}");
        }
        assert_eq!(decode_body(LINKS), LINKS.trim());
        let (servers, _) = crate::links::parse_text(&STANDARD.encode(LINKS));
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn titles_and_hosts() {
        assert_eq!(profile_title("base64:0JzQvtC5IFZQTg==").as_deref(), Some("Мой VPN"));
        assert_eq!(profile_title(" Work ").as_deref(), Some("Work"));
        assert_eq!(profile_title(""), None);
        assert_eq!(host_of("https://sub.example.com:2096/sub/SECRET-TOKEN"), "sub.example.com");
    }
}
