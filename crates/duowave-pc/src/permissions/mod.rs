//! «Разрешения» (docs/plans/permissions.md): what this computer's system must allow the DuoWave app, each item checked
//! live, and the way to its switch.
//!
//! - macOS: «Локальная сеть» (macOS 15 and later: the station search and the home's announcement are mDNS sends the
//!   system lets out only once the person allowed them) and «Микрофон» (the app's port reads and asks it).
//! - Windows: the microphone switch for desktop programs (the registry, windows_mic.rs), the firewall and a
//!   «Общедоступная» network (platform/windows_net.rs).
//! - Linux, a server, Docker, the station: nothing to allow — the list is empty and the panel shows no step.
//!
//! The items exist only in the desktop app (`DUOWAVE_MANAGED=desktop`): a gateway without it has no person at its screen
//! to ask. The panel's step «Разрешения» comes before the station search, and on a Mac nothing knocks on the home
//! network before it: the announcer waits for the network to be asked (`hold_until_asked`).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod routes;
pub mod windows_mic;

/// One thing the system must allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Item {
    LocalNetwork,
    Microphone,
    Firewall,
    NetworkProfile,
}

impl Item {
    pub fn parse(text: &str) -> Option<Item> {
        match text {
            "localNetwork" => Some(Item::LocalNetwork),
            "microphone" => Some(Item::Microphone),
            "firewall" => Some(Item::Firewall),
            "networkProfile" => Some(Item::NetworkProfile),
            _ => None,
        }
    }
}

/// Where an item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Granted,
    Denied,
    /// The system has not asked the person yet.
    NotAsked,
    /// Not known: counts as allowed — a wrong read must never stop anyone.
    Unknown,
}

/// What the item's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// The system's own question (macOS).
    Ask,
    /// The system's settings page, where the person turns the switch.
    Open,
    /// The firewall's allowance: one UAC window (platform/windows_net.rs).
    AllowFirewall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub item: Item,
    pub state: State,
    /// What the button does now: `ask` before the system asked, `open` after a refusal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// The networks a «Общедоступная» row is about.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// `mac`, `windows`, `linux`.
    pub os: &'static str,
    /// The desktop app runs this gateway: the items are this computer's.
    pub app: bool,
    /// The caller is this computer itself (loopback): only then do the buttons open anything — from a phone the
    /// system's windows would come up on a screen nobody looks at.
    pub here: bool,
    pub items: Vec<Row>,
}

pub type Asked = Pin<Box<dyn Future<Output = State> + Send>>;

/// What the desktop app gives: the microphone as the system sees it, its question, and the Mac's guide window beside
/// System Settings (desktop/src-tauri/src/guide.rs). Set once when the app starts, as the ear's audio port.
pub trait SystemPort: Send + Sync {
    /// The microphone's state for this app (macOS); `Unknown` where the app cannot tell.
    fn microphone(&self) -> State;
    /// The system's question about the microphone; its answer.
    fn ask_microphone(&self) -> Asked;
    /// The small window beside System Settings that says which switch to turn: shown or closed.
    fn guide(&self, item: Item, show: bool);
}

static PORT: OnceLock<Arc<dyn SystemPort>> = OnceLock::new();

/// The app's port, set once when it starts.
pub fn set_port(port: Arc<dyn SystemPort>) {
    let _ = PORT.set(port);
}

pub fn port() -> Option<Arc<dyn SystemPort>> {
    PORT.get().cloned()
}

/// The OS as the panel names it.
pub fn os() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The macOS major version (15 for Sequoia); 0 off macOS or when it cannot be read.
pub fn macos_major() -> u32 {
    static MAJOR: OnceLock<u32> = OnceLock::new();
    *MAJOR.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return 0;
        }
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| major_of(&text))
            .unwrap_or(0)
    })
}

fn major_of(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// Whether the system has a «Локальная сеть» gate: macOS 15 and later.
pub fn has_local_network_gate() -> bool {
    cfg!(target_os = "macos") && macos_major() >= 15
}

// ─── The home network asked ────────────────────────────────────

/// Nothing read yet, or not a Mac app: the gate does not hold anything back.
const OPEN: u8 = 0;
/// A fresh Mac app: nothing goes to the home network until the person is asked.
const WAITING: u8 = 1;
/// The person was asked: the step's «Разрешить», or a search they started.
const ASKED: u8 = 2;

static NETWORK: AtomicU8 = AtomicU8::new(OPEN);
static NETWORK_ASKED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The person was asked about the home network (the step's «Разрешить», a station search they started): the waiting
/// announcer goes on, and a probe may now knock.
pub fn network_asked() {
    if NETWORK.swap(ASKED, Ordering::SeqCst) == WAITING {
        NETWORK_ASKED.notify_waiters();
    }
}

/// Whether the home network is still held back on this Mac app.
pub fn network_waiting() -> bool {
    NETWORK.load(Ordering::SeqCst) == WAITING
}

/// What the stored mark and the database say at a start: `Some(true)` — asked, `Some(false)` — a fresh install still
/// to be asked, None — nothing stored yet. With nothing stored, a gateway somebody already uses (an account exists:
/// set up before this version) counts as asked — nothing changes for an existing home; a gateway nobody set up yet is
/// a fresh install.
pub fn asked_at_start(stored: Option<bool>, accounts: usize) -> bool {
    stored.unwrap_or(accounts > 0)
}

/// The announcer's gate (home/announce.rs): in the macOS desktop app on macOS 15 and later, nothing goes to the home
/// network until the person was asked — the system's window must come after the step «Разрешения» explains it, not at
/// the app's first start with no word. Elsewhere it returns at once.
pub async fn hold_until_asked(settings: &crate::settings::SettingsService, db: &crate::storage::Db, desktop: bool) {
    if !desktop || !has_local_network_gate() {
        NETWORK.store(OPEN, Ordering::SeqCst);
        return;
    }
    let stored = settings.get(&crate::settings::definitions::LOCAL_NETWORK_ASKED, None);
    let accounts = db
        .with(|c| c.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0)))
        .map(|count| count.max(0) as usize)
        .unwrap_or(1);
    let asked = asked_at_start(stored, accounts);
    if stored.is_none() {
        let _ = settings.seed(&crate::settings::definitions::LOCAL_NETWORK_ASKED, &asked);
    }
    if asked {
        NETWORK.store(ASKED, Ordering::SeqCst);
        return;
    }
    // Waiting already (the engine restarted in the same process), or asked since the process started (a search came
    // before this read): the latter is kept and goes on.
    match NETWORK.compare_exchange(OPEN, WAITING, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) | Err(WAITING) => {}
        Err(_) => {
            let _ = settings.set(&crate::settings::definitions::LOCAL_NETWORK_ASKED, &true, None);
            return;
        }
    }
    tracing::info!(target: "home", "announce: waits until the person is asked about the home network (step «Разрешения»)");
    while network_waiting() {
        let asked = NETWORK_ASKED.notified();
        if !network_waiting() {
            break;
        }
        asked.await;
    }
    let _ = settings.set(&crate::settings::definitions::LOCAL_NETWORK_ASKED, &true, None);
    tracing::info!(target: "home", "announce: the person was asked about the home network — announcing");
}

// ─── The items ─────────────────────────────────────────────────

/// The last probe of the home network: a send at most once a second, however often the panel asks.
static PROBED: Mutex<Option<(Instant, State)>> = Mutex::new(None);
const PROBE_EVERY: Duration = Duration::from_secs(1);

/// «Локальная сеть» now: before the person was asked — not asked (a send would bring the system's window with no
/// word); after — one mDNS send says it.
pub async fn local_network() -> State {
    if network_waiting() {
        return State::NotAsked;
    }
    if let Some((at, state)) = *PROBED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
        if at.elapsed() < PROBE_EVERY {
            return state;
        }
    }
    let state = match crate::stations::connect::discovery::probe_local_network().await {
        Some(true) => State::Denied,
        Some(false) => State::Granted,
        None => State::Unknown,
    };
    *PROBED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((Instant::now(), state));
    state
}

/// The rows of this computer: per OS, only in the desktop app.
pub async fn rows(desktop: bool) -> Vec<Row> {
    if !desktop {
        return Vec::new();
    }
    let mut rows = Vec::new();
    if cfg!(target_os = "macos") {
        if has_local_network_gate() {
            let state = local_network().await;
            rows.push(Row {
                item: Item::LocalNetwork,
                state,
                action: action_of(Item::LocalNetwork, state),
                networks: Vec::new(),
            });
        }
        let state = port().map(|port| port.microphone()).unwrap_or(State::Unknown);
        rows.push(Row {
            item: Item::Microphone,
            state,
            action: action_of(Item::Microphone, state),
            networks: Vec::new(),
        });
    } else if cfg!(windows) {
        let state = windows_mic::state().await;
        rows.push(Row {
            item: Item::Microphone,
            state,
            action: action_of(Item::Microphone, state),
            networks: Vec::new(),
        });
        let network = crate::platform::windows_net::refresh().await;
        let (firewall, public) = match &network {
            Some(network) => (
                if network.firewall_closed() { State::Denied } else { State::Granted },
                network.public_closed().into_iter().map(str::to_string).collect::<Vec<_>>(),
            ),
            None => (State::Unknown, Vec::new()),
        };
        rows.push(Row {
            item: Item::Firewall,
            state: firewall,
            action: action_of(Item::Firewall, firewall),
            networks: Vec::new(),
        });
        let profile = if public.is_empty() { State::Granted } else { State::Denied };
        rows.push(Row {
            item: Item::NetworkProfile,
            state: profile,
            action: action_of(Item::NetworkProfile, profile),
            networks: public,
        });
    }
    rows
}

/// What the button does for an item in a state: the system's question while it was never asked (on a Mac), its
/// settings after a refusal; nothing once allowed.
pub fn action_of(item: Item, state: State) -> Option<Action> {
    match (item, state) {
        (_, State::Granted) => None,
        (Item::Firewall, State::Denied) => Some(Action::AllowFirewall),
        (Item::Firewall, _) => None,
        (Item::LocalNetwork | Item::Microphone, State::NotAsked) if cfg!(target_os = "macos") => Some(Action::Ask),
        (_, State::Unknown) if !cfg!(target_os = "macos") => None,
        _ => Some(Action::Open),
    }
}

/// The system's settings page of an item: macOS' Privacy & Security panes, Windows' ms-settings pages.
pub fn settings_url(item: Item) -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        match item {
            Item::Microphone => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
            // The Local Network pane's own anchor (macOS 15); a system that does not know it opens Privacy & Security.
            Item::LocalNetwork => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"),
            _ => None,
        }
    } else if cfg!(windows) {
        match item {
            Item::Microphone => Some("ms-settings:privacy-microphone"),
            Item::NetworkProfile => Some("ms-settings:network-status"),
            _ => None,
        }
    } else {
        None
    }
}

/// Opens the item's settings page on this computer's screen.
pub fn open_settings(item: Item) -> Result<(), String> {
    let Some(url) = settings_url(item) else {
        return Err(crate::tr!("Для этого разрешения на этом компьютере нет страницы настроек."));
    };
    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("explorer.exe").arg(url).spawn()
    };
    opened.map(|_| ()).map_err(|error| crate::tr!("Настройки не открылись: {error}", error = error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_gives_its_major() {
        assert_eq!(major_of("15.1.1\n"), Some(15));
        assert_eq!(major_of("26.0"), Some(26));
        assert_eq!(major_of("14"), Some(14));
        assert_eq!(major_of(""), None);
    }

    #[test]
    fn an_existing_gateway_counts_as_asked_and_a_fresh_one_waits() {
        assert!(asked_at_start(None, 1), "an account exists: set up before this version — announce as before");
        assert!(!asked_at_start(None, 0), "nobody set it up yet: a fresh install waits for the step");
        assert!(asked_at_start(Some(true), 0));
        assert!(!asked_at_start(Some(false), 3), "the mark written at the first start stays until the person is asked");
    }

    #[test]
    fn the_button_follows_the_state() {
        assert_eq!(action_of(Item::Microphone, State::Granted), None);
        assert_eq!(action_of(Item::Firewall, State::Denied), Some(Action::AllowFirewall));
        assert_eq!(action_of(Item::Firewall, State::Unknown), None);
        assert_eq!(action_of(Item::NetworkProfile, State::Denied), Some(Action::Open));
        if cfg!(target_os = "macos") {
            assert_eq!(action_of(Item::LocalNetwork, State::NotAsked), Some(Action::Ask));
            assert_eq!(action_of(Item::Microphone, State::Denied), Some(Action::Open));
            assert_eq!(action_of(Item::Microphone, State::Unknown), Some(Action::Open));
        }
    }

    #[test]
    fn items_read_from_their_names() {
        for item in [Item::LocalNetwork, Item::Microphone, Item::Firewall, Item::NetworkProfile] {
            let name = serde_json::to_value(item).unwrap();
            assert_eq!(Item::parse(name.as_str().unwrap()), Some(item));
        }
        assert_eq!(Item::parse("camera"), None);
    }

    #[tokio::test]
    async fn a_gateway_without_the_app_has_nothing_to_allow() {
        assert!(rows(false).await.is_empty());
    }
}
