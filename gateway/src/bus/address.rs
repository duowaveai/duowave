//! An address: what everything that lives in the system is called on the bus (docs/plans/bus.md) — `kind/path`.
//! The kind is the first part and says which provider answers for it (`app`, `system`, `files`, `device`, `person`…);
//! the path is the provider's own: `app/radio`, `files/home/Картинки/лето.png`, `device/midi`. An address never says
//! where its bytes live: a move between the station and the computer changes the place, not the address.
//!
//! A pattern names many addresses: an address, one ending in `/*` (it and everything below it) or `*` alone.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The kinds the gateway itself knows; a provider may bring its own.
pub const APP: &str = "app";
pub const SYSTEM: &str = "system";
pub const FILES: &str = "files";
pub const DEVICE: &str = "device";
pub const PERSON: &str = "person";
pub const HOME: &str = "home";
pub const AGENT: &str = "agent";
pub const CHARACTER: &str = "character";
pub const STATION: &str = "station";
pub const GATEWAY: &str = "gateway";

/// The longest address taken.
const MAX_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address(String);

/// Why an address or a pattern is not one.
fn bad(text: &str, why: &str) -> String {
    format!("адрес «{}»: {why}", text.chars().take(80).collect::<String>())
}

/// A kind: lower case Latin letters, digits and `-`, a letter first.
fn kind_ok(kind: &str) -> bool {
    let mut chars = kind.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && kind.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A segment of a path: any printable text but `/` and `*`, never `.` or `..`, no spaces at its ends.
fn segment_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.trim() == segment
        && segment.chars().count() <= 128
        && segment.chars().all(|c| !c.is_control() && c != '/' && c != '*' && c != '\\')
}

impl Address {
    pub fn parse(text: &str) -> Result<Address, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(bad(text, "пустой"));
        }
        if text.len() > MAX_LEN {
            return Err(bad(text, "слишком длинный"));
        }
        let mut parts = text.split('/');
        let kind = parts.next().unwrap_or_default();
        if !kind_ok(kind) {
            return Err(bad(text, "вид — латинские буквы, цифры и «-»"));
        }
        for segment in parts {
            if !segment_ok(segment) {
                return Err(bad(text, "часть пути пустая или с недопустимыми знаками"));
            }
        }
        Ok(Address(text.to_string()))
    }

    /// A kind and a path below it (`files`, `home/Картинки`).
    pub fn new(kind: &str, path: &str) -> Result<Address, String> {
        if path.is_empty() {
            Address::parse(kind)
        } else {
            Address::parse(&format!("{kind}/{path}"))
        }
    }

    /// An app's address: `app/<id>`.
    pub fn app(id: &str) -> Address {
        Address(format!("{APP}/{id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> &str {
        self.0.split('/').next().unwrap_or_default()
    }

    /// Everything after the kind: `radio` of `app/radio`, empty for `home`.
    pub fn path(&self) -> &str {
        self.0.split_once('/').map(|(_, path)| path).unwrap_or_default()
    }

    /// The path's parts.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.path().split('/').filter(|segment| !segment.is_empty())
    }

    /// The second part: the app of `app/<id>`, the device of `device/<name>`.
    pub fn name(&self) -> Option<&str> {
        self.segments().next()
    }

    /// The address one part deeper.
    pub fn join(&self, child: &str) -> Result<Address, String> {
        let child = child.trim_matches('/');
        if child.is_empty() {
            return Ok(self.clone());
        }
        Address::parse(&format!("{}/{child}", self.0))
    }

    /// The address one part up; none for a kind alone.
    pub fn parent(&self) -> Option<Address> {
        self.0.rsplit_once('/').map(|(parent, _)| Address(parent.to_string()))
    }

    /// Whether this address is `other` or lies below it, part by part (`files/home/Фото` is below `files/home`,
    /// `files/home2` is not).
    pub fn within(&self, other: &Address) -> bool {
        self.0 == other.0 || (self.0.starts_with(&other.0) && self.0.as_bytes().get(other.0.len()) == Some(&b'/'))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Address {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Address, D::Error> {
        let text = String::deserialize(deserializer)?;
        Address::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Many addresses: `*`, `app/*`, `files/home/Картинки/*`, or one address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pattern(String);

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, String> {
        let text = text.trim();
        if text == "*" {
            return Ok(Pattern(text.to_string()));
        }
        let base = text.strip_suffix("/*").unwrap_or(text);
        Address::parse(base)?;
        Ok(Pattern(text.to_string()))
    }

    /// The pattern of one address.
    pub fn exact(address: &Address) -> Pattern {
        Pattern(address.0.clone())
    }

    /// An address and everything below it.
    pub fn below(address: &Address) -> Pattern {
        Pattern(format!("{}/*", address.0))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, address: &Address) -> bool {
        if self.0 == "*" {
            return true;
        }
        match self.0.strip_suffix("/*") {
            Some(base) => Address::parse(base).is_ok_and(|base| address.within(&base)),
            None => self.0 == address.0,
        }
    }

    /// Whether every address `other` matches, this one matches too.
    pub fn covers(&self, other: &Pattern) -> bool {
        if self.0 == "*" || self == other {
            return true;
        }
        let Some(base) = self.0.strip_suffix("/*").and_then(|base| Address::parse(base).ok()) else {
            return false;
        };
        if other.0 == "*" {
            return false;
        }
        let theirs = other.0.strip_suffix("/*").unwrap_or(&other.0);
        Address::parse(theirs).is_ok_and(|theirs| theirs.within(&base))
    }

    /// How narrow the pattern is: an exact address beats one with `/*` of the same length, a longer one beats a
    /// shorter, `*` comes last. The narrowest matching link decides.
    pub fn specificity(&self) -> usize {
        if self.0 == "*" {
            return 0;
        }
        match self.0.strip_suffix("/*") {
            Some(base) => base.len() * 2,
            None => self.0.len() * 2 + 1,
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Pattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Pattern, D::Error> {
        let text = String::deserialize(deserializer)?;
        Pattern::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Address {
        Address::parse(text).unwrap()
    }

    #[test]
    fn reads_kinds_and_paths() {
        let picture = address("files/home/Картинки/лето 2026.png");
        assert_eq!(picture.kind(), "files");
        assert_eq!(picture.path(), "home/Картинки/лето 2026.png");
        assert_eq!(picture.name(), Some("home"));
        assert_eq!(picture.parent().unwrap().as_str(), "files/home/Картинки");
        assert_eq!(address("home").path(), "");
        assert_eq!(Address::app("radio").as_str(), "app/radio");
        assert_eq!(address("files/app/x").join("tmp/a.txt").unwrap().as_str(), "files/app/x/tmp/a.txt");
    }

    #[test]
    fn refuses_what_is_not_an_address() {
        for text in ["", "App/x", "app//x", "app/x/", "files/../etc", "files/./a", "files/a*b", "1app/x", "app/ x"] {
            assert!(Address::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn below_means_part_by_part() {
        assert!(address("files/home/Фото/a.png").within(&address("files/home/Фото")));
        assert!(address("files/home/Фото").within(&address("files/home/Фото")));
        assert!(!address("files/home/Фото2").within(&address("files/home/Фото")));
    }

    #[test]
    fn patterns_match_and_the_narrowest_is_the_most_specific() {
        let all = Pattern::parse("*").unwrap();
        let apps = Pattern::parse("app/*").unwrap();
        let radio = Pattern::parse("app/radio").unwrap();
        let target = address("app/radio");
        assert!(all.matches(&target) && apps.matches(&target) && radio.matches(&target));
        assert!(Pattern::parse("app/*").unwrap().matches(&address("app")));
        assert!(!radio.matches(&address("app/radio2")));
        assert!(radio.specificity() > apps.specificity() && apps.specificity() > all.specificity());
        assert!(Pattern::parse("app/radio/*").unwrap().specificity() < radio.specificity());
        assert!(Pattern::parse("app/*x").is_err());
        assert!(apps.covers(&radio) && apps.covers(&Pattern::parse("app/radio/*").unwrap()) && all.covers(&apps));
        assert!(!radio.covers(&apps) && !apps.covers(&all) && !Pattern::parse("files/*").unwrap().covers(&radio));
    }
}
