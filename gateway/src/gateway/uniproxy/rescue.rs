//! Emergency words, heard before the routes, the plugins and the model — and the same with all of them off or broken:
//!
//! - «Алиса, аварийный возврат к Яндексу» (or «аварийный откат шлюза», «аварийно верни колонку Яндексу») gives the
//!   station back: its voice to Yandex — our copy of quasar.cfg taken away — and a reboot; a minute later it is the
//!   usual Alice. The way back of the panel's «Вернуть к Яндексу», without the panel.
//! - «Алиса, переведи шлюз в режим Яндекса» lets Yandex answer everything, as before the gateway, at once and without a
//!   reboot; the panel switches it back.
//!
//! Whole phrases only, as Quasar Bridge's are: talking about them, quoting or denying them does nothing, and so does a
//! phrase with more words in it.

#[cfg(feature = "pc")]
use std::sync::Arc;
use std::sync::LazyLock;

use regex::Regex;

use crate::core::events::Actor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rescue {
    /// The station's voice back to Yandex, then a reboot.
    GiveBack,
    /// Everything to Yandex: the mode «Яндекс».
    YandexMode,
}

impl Rescue {
    pub fn as_str(self) -> &'static str {
        match self {
            Rescue::GiveBack => "giveBack",
            Rescue::YandexMode => "yandexMode",
        }
    }
}

const PREFIX: &str = r"^(?:алиса )?(?:пожалуйста )?";
const SUFFIX: &str = r"(?: пожалуйста)?$";

static GIVE_BACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"{PREFIX}(?:выполни )?(?:аварийный (?:откат(?: шлюза| дуовейв| duowave)?|возврат(?: колонки)?(?: к яндексу)?)|аварийно верни (?:колонку )?(?:к )?яндекс(?:у)?){SUFFIX}"
    ))
    .expect("the pattern compiles")
});

static YANDEX_MODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"{PREFIX}(?:аварийно )?(?:переведи|переключи) шлюз в режим яндекс(?:а)?{SUFFIX}"))
        .expect("the pattern compiles")
});

/// The words as they are matched: lower case, «ё» as «е», no punctuation, single spaces.
fn plain(text: &str) -> String {
    let lowered: String = text
        .to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The emergency a recognised request is, if it is one.
pub fn rescue_of(text: &str) -> Option<Rescue> {
    let text = plain(text);
    if GIVE_BACK.is_match(&text) {
        Some(Rescue::GiveBack)
    } else if YANDEX_MODE.is_match(&text) {
        Some(Rescue::YandexMode)
    } else {
        None
    }
}

/// What gives a station back to Yandex: the station's own gateway changes its own files; a computer's goes over SSH,
/// as the panel's «Вернуть к Яндексу» does.
pub trait StationRescue: Send + Sync {
    fn give_back(&self, address: &str, actor: Option<Actor>) -> Result<(), String>;
}

/// The gateway on the station itself: its voice to Yandex and a reboot, by voice.sh.
pub struct OwnStation;

impl StationRescue for OwnStation {
    fn give_back(&self, _address: &str, _actor: Option<Actor>) -> Result<(), String> {
        use crate::placement::station_fs::{RealSystem, StationFs, Target};
        StationFs::real().point(&Target::Yandex, true, &RealSystem).map(|_| ())
    }
}

/// A computer's gateway: the station at its address, over SSH, the job the panel's button starts.
#[cfg(feature = "pc")]
pub struct OverSsh(pub Arc<crate::stations::connect::station_connect::StationConnectService>);

#[cfg(feature = "pc")]
impl StationRescue for OverSsh {
    fn give_back(&self, address: &str, actor: Option<Actor>) -> Result<(), String> {
        use crate::stations::connect::station_connect::RestoreTo;
        self.0.restore(address, RestoreTo::Yandex, actor).map(|_| ()).map_err(|error| error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hears_the_emergency_phrases_whole() {
        for text in [
            "Аварийный возврат к Яндексу.",
            "Алиса, аварийный возврат к Яндексу!",
            "аварийный откат шлюза",
            "Выполни аварийный откат шлюза, пожалуйста",
            "Аварийно верни колонку Яндексу",
            "аварийный возврат",
        ] {
            assert_eq!(rescue_of(text), Some(Rescue::GiveBack), "{text}");
        }
        for text in [
            "Переведи шлюз в режим Яндекса",
            "Алиса, переключи шлюз в режим яндекс",
            "аварийно переведи шлюз в режим Яндекса",
        ] {
            assert_eq!(rescue_of(text), Some(Rescue::YandexMode), "{text}");
        }
    }

    #[test]
    fn talking_about_them_does_nothing() {
        for text in [
            "Как сделать аварийный возврат к Яндексу?",
            "Не делай аварийный возврат к Яндексу",
            "Что такое аварийный откат шлюза",
            "аварийный возврат к Яндексу через час",
            "переведи шлюз в режим шлюза",
            "верни колонку Яндексу",
            "возврат к Яндексу",
        ] {
            assert_eq!(rescue_of(text), None, "{text}");
        }
    }
}
