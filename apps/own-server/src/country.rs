//! A server's country from its name: the flag emoji subscriptions put first (`🇳🇱 Амстердам` → `NL`) — a pair of
//! regional indicator symbols. Without a flag the country comes from Cloudflare's trace through the server (main.rs).

const FIRST: u32 = 0x1F1E6;
const LAST: u32 = 0x1F1FF;

fn letter(c: char) -> Option<char> {
    let code = c as u32;
    (FIRST..=LAST).contains(&code).then(|| char::from(b'A' + (code - FIRST) as u8))
}

/// The ISO 3166-1 code of the first flag in the name.
pub fn from_name(name: &str) -> Option<String> {
    let chars: Vec<char> = name.chars().collect();
    chars.windows(2).find_map(|pair| Some(format!("{}{}", letter(pair[0])?, letter(pair[1])?)))
}

/// The two letters of Cloudflare's `loc=` line, upper case; `XX` and `T1` (Tor) are no country.
pub fn from_trace(body: &str) -> Option<String> {
    let code = body.lines().find_map(|line| line.strip_prefix("loc="))?.trim().to_ascii_uppercase();
    (code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()) && code != "XX").then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_traces() {
        assert_eq!(from_name("🇳🇱 Амстердам").as_deref(), Some("NL"));
        assert_eq!(from_name("Fast 🇩🇪 Frankfurt 🇺🇸").as_deref(), Some("DE"));
        assert_eq!(from_name("Amsterdam"), None);
        assert_eq!(from_name("🇳"), None);
        assert_eq!(from_trace("fl=1\nip=203.0.113.9\nloc=nl\ntls=TLSv1.3\n").as_deref(), Some("NL"));
        assert_eq!(from_trace("loc=XX\n"), None);
        assert_eq!(from_trace("ip=1\n"), None);
    }
}
