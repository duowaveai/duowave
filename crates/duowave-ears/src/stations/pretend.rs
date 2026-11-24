//! Pretend stations (docs/plans/pretend-stations.md): this computer's build in the station's role, several side by side
//! on one Mac, each on an address of its own on the loopback (10.77.0.x) — and the test Turbo beside them. What a real
//! station has around its gateway is played here:
//!
//! - it answers the station search: a question about `_yandexio._tcp` sent straight to its address (port 5353) gets
//!   the answer a station gives — the search of a pretend Turbo asks those addresses only (`search_targets`), never
//!   the home network: the owner's real station is never asked, and the Mac's live DuoWave never sees the pretend ones;
//! - its SSH greets as the station's dropbear does (port 2222) and lets nobody in: a pretend station has DuoWave
//!   already, it is never installed over SSH;
//! - its voice is `duowave pretend` (src/tools/pretend.rs): a pretend quasar on its voice socket and a pretend Yandex;
//! - what it would say aloud — a join code — goes to its terminal (and to the Mac's `say` with DUOWAVE_PRETEND_SAY);
//! - its connections to the other pretend addresses leave from its own (src/utils/net.rs).
//!
//! Nothing of it runs without DUOWAVE_PRETEND. Here — whether this gateway is a pretend station and what it «says»;
//! its mDNS answer and SSH greeting — the computer's floor (crates/duowave-pc, under this module's name).

use std::sync::OnceLock;

use crate::config::PretendConfig;

/// What a pretend station's terminal line starts with when it «says» something: `duowave pretend` shows these.
pub const SAYS: &str = "🔊 ";

static PRETEND: OnceLock<PretendConfig> = OnceLock::new();

/// This process is a pretend station or the Turbo beside them: its search and its connections from now on.
pub fn install(config: &PretendConfig) {
    let _ = PRETEND.set(config.clone());
    crate::utils::net::leave_from(config.address);
}

pub fn get() -> Option<&'static PretendConfig> {
    PRETEND.get()
}

/// What a pretend station «says» aloud: a line in its terminal (`duowave pretend` shows it), and the Mac's voice too
/// with DUOWAVE_PRETEND_SAY.
pub fn say(pretend: &PretendConfig, text: &str) {
    println!("{SAYS}{}: {text}", pretend.name);
    if pretend.say {
        let text = text.to_string();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("say").arg(text).status().await;
        });
    }
}

/// «Это ненастоящая колонка»: what would change the station's own files or reboot it.
pub fn refuse() -> crate::platform::http::ApiError {
    crate::platform::http::ApiError::conflict(crate::tr!(
        "Это ненастоящая колонка: файлов прошивки и перезагрузки у неё нет."
    ))
}

/// Whether this gateway is a pretend station.
pub fn is_station() -> bool {
    get().is_some_and(PretendConfig::is_station)
}
