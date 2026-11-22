//! The Windows network as it matters to a computer others dial: the station reaches this computer's gateway (its
//! voice, hearing and models as Turbo) and its link port. Windows test 21.11: the station was given the computer's key,
//! yet its panel said «Турбо нет» and «Компьютер с голосом не на связи» while the computer's own panel looked fine —
//! inbound connections were blocked, and nothing said so.
//!
//! - `look()`: the networks by adapter with their category (a «Общедоступная» — Public — network blocks inbound by
//!   default), whether the firewall is on per profile, and the inbound rules for this very program — an allow one, or
//!   a block one: Windows writes a block rule when its own «allow access» window is closed with «Отмена», and a block
//!   rule wins over any allow rule. Read with PowerShell, no rights needed. None off Windows.
//! - `allow_inbound()`: this program's inbound rules replaced with one allow rule on the private and domain profiles —
//!   one elevation prompt (UAC), only when the person presses the button. Public networks stay closed: the way is to
//!   make the home network private.
//!
//! The gateway logs the look at its start, so a pasted gateway.log says it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATION_VIEW};
use crate::platform::http::{ok, ApiResult, Caller};

/// What the firewall and the network say about inbound connections to this program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsNetwork {
    /// The networks the computer is on: adapter and category (Public, Private, DomainAuthenticated).
    #[serde(default)]
    pub networks: Vec<NetworkProfile>,
    /// The firewall's profiles and whether each is on.
    #[serde(default)]
    pub firewall: Vec<FirewallProfile>,
    /// The enabled inbound rules for this program.
    #[serde(default)]
    pub rules: Vec<FirewallRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProfile {
    #[serde(default, alias = "InterfaceAlias")]
    pub alias: String,
    #[serde(default, alias = "Category")]
    pub category: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallProfile {
    #[serde(default, alias = "Name")]
    pub name: String,
    #[serde(default, alias = "Enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRule {
    #[serde(default)]
    pub name: String,
    /// Allow or Block.
    #[serde(default)]
    pub action: String,
    /// Any, Public, Private, Domain, or several: «Private, Public».
    #[serde(default)]
    pub profile: String,
}

/// The firewall profile a network of this category is under: Private, Domain (DomainAuthenticated), else Public —
/// Windows treats a network it does not know as public.
fn profile_of(category: &str) -> &'static str {
    let category = category.to_ascii_lowercase();
    if category == "private" {
        "private"
    } else if category.starts_with("domain") {
        "domain"
    } else {
        "public"
    }
}

impl WindowsNetwork {
    /// Whether the firewall is on for this profile. A profile that was not read counts as off: nothing to say.
    fn firewall_on(&self, profile: &str) -> bool {
        self.firewall.iter().any(|entry| entry.enabled && entry.name.eq_ignore_ascii_case(profile))
    }

    /// A rule of this action (Allow, Block) for this program covers the profile: named in it, «Any», or none named.
    fn rule_covers(&self, action: &str, profile: &str) -> bool {
        self.rules.iter().any(|rule| {
            rule.action.eq_ignore_ascii_case(action) && {
                let profiles = rule.profile.to_lowercase();
                profiles.is_empty() || profiles.contains("any") || profiles.contains(profile)
            }
        })
    }

    /// Whether Windows keeps others away from this program on a network of this profile: the firewall is on for it,
    /// and a block rule covers it (Windows' own after «Отмена» in its window — it wins over any allow rule) or no allow
    /// rule does.
    fn closed_on(&self, profile: &str) -> bool {
        self.firewall_on(profile) && (self.rule_covers("block", profile) || !self.rule_covers("allow", profile))
    }

    /// The networks the computer is on that keep others away, each judged by its own profile. A tester's Windows,
    /// 22.11: a «Общедоступная» Ethernet, the Public firewall off and an allow rule for Public — open, though the
    /// Domain firewall was on; any profile on and any public network made it «closed».
    fn closed_networks(&self) -> Vec<&NetworkProfile> {
        self.networks.iter().filter(|network| self.closed_on(profile_of(&network.category))).collect()
    }

    /// Whether others may well not reach this program on a network the computer is on. With no network read, a home
    /// network is judged as private.
    pub fn likely_closed(&self) -> bool {
        if self.networks.is_empty() {
            return self.closed_on("private");
        }
        !self.closed_networks().is_empty()
    }

    /// One line for the log: networks, the firewall, the rules.
    pub fn summary(&self) -> String {
        let networks: Vec<String> =
            self.networks.iter().map(|network| format!("{} — {}", network.alias, network.category)).collect();
        let firewall: Vec<String> = self
            .firewall
            .iter()
            .map(|profile| format!("{} {}", profile.name, if profile.enabled { "on" } else { "off" }))
            .collect();
        let rules: Vec<String> =
            self.rules.iter().map(|rule| format!("{} {} ({})", rule.action, rule.name, rule.profile)).collect();
        format!(
            "networks: {}; firewall: {}; inbound rules for this program: {}",
            if networks.is_empty() { "none read".to_string() } else { networks.join(", ") },
            if firewall.is_empty() { "not read".to_string() } else { firewall.join(", ") },
            if rules.is_empty() { "none".to_string() } else { rules.join(", ") }
        )
    }
}

/// What the person reads when the station cannot reach this computer: what to do on this Windows, if anything is seen.
pub fn advice(network: &WindowsNetwork) -> Option<String> {
    let closed = network.closed_networks();
    // A «Общедоступная» network that keeps others away: the way is to make it private (the button allows private and
    // domain networks only).
    let public: Vec<&str> = closed
        .iter()
        .filter(|entry| profile_of(&entry.category) == "public")
        .map(|entry| entry.alias.as_str())
        .collect();
    if !public.is_empty() {
        return Some(crate::tr!(
            "Сеть «{names}» в Windows — «Общедоступная»: в ней Windows не пускает к компьютеру никого. Сделайте её частной: «Параметры» → «Сеть и Интернет» → сеть → «Частная».",
            names = public.join("», «")
        ));
    }
    // A private or domain network that keeps others away; with no network read, a home one judged as private.
    let profile = match closed.first() {
        Some(entry) => profile_of(&entry.category),
        None if network.networks.is_empty() && network.closed_on("private") => "private",
        None => return None,
    };
    if network.rule_covers("block", profile) {
        return Some(crate::tr!(
            "Брандмауэр Windows запрещает DuoWave входящие подключения (так бывает, если в его окне нажали «Отмена»). Нажмите «Разрешить в брандмауэре»."
        ));
    }
    Some(crate::tr!(
        "В брандмауэре Windows нет разрешения для DuoWave в частной сети. Нажмите «Разрешить в брандмауэре»."
    ))
}

/// The last look, for what is read without waiting (the warnings about the station): None off Windows or before it.
static LAST: Mutex<Option<WindowsNetwork>> = Mutex::new(None);

/// The last look, if any.
pub fn last() -> Option<WindowsNetwork> {
    LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// A fresh look, kept as the last one and told to the log when it differs.
pub async fn refresh() -> Option<WindowsNetwork> {
    let network = look().await?;
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref() != Some(&network) {
        tracing::info!("windows network: {}", network.summary());
        if let Some(advice) = advice(&network) {
            tracing::warn!("windows network: the station and other devices may not reach this computer: {advice}");
        }
    }
    *last = Some(network.clone());
    Some(network)
}

/// At the gateway's start: the look once, for the log.
pub fn start() {
    if cfg!(windows) {
        tokio::spawn(async {
            refresh().await;
        });
    }
}

/// What the panel reads: `windows` false elsewhere; on Windows the look and what to do.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkView {
    pub windows: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<WindowsNetwork>,
    pub likely_closed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
}

fn view_of(network: Option<WindowsNetwork>) -> NetworkView {
    NetworkView {
        windows: cfg!(windows),
        likely_closed: network.as_ref().is_some_and(WindowsNetwork::likely_closed),
        advice: network.as_ref().and_then(advice),
        network,
    }
}

/// `GET /api/v1/system/windows-network` — the look now; `POST …/allow` — the firewall's allowance (one UAC prompt on
/// this computer's screen), then the look again.
pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/system/windows-network", get(read))
        .route("/api/v1/system/windows-network/allow", post(allow))
        .route("/api/v1/system/windows-security/open", post(open_app_control))
}

/// «Открыть настройки Windows» (owner 22.11): Windows Security on «App & browser control», where Smart App Control
/// is — the person decides there; nothing is changed from here.
async fn open_app_control(State(_app): State<Arc<App>>, caller: Caller) -> ApiResult<serde_json::Value> {
    caller.permitted(SETTINGS_EDIT)?;
    if !cfg!(windows) {
        return Err(crate::platform::http::ApiError::conflict(crate::tr!("Это нужно только на Windows.")));
    }
    std::process::Command::new("explorer.exe").arg("windowsdefender://appbrowser").spawn().map_err(|error| {
        crate::platform::http::ApiError::conflict(crate::tr!(
            "Не открылись настройки Windows: {error}",
            error = error.to_string()
        ))
    })?;
    Ok(ok(serde_json::json!({ "opened": true })))
}

async fn read(State(_app): State<Arc<App>>, caller: Caller) -> ApiResult<NetworkView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(view_of(refresh().await)))
}

async fn allow(State(_app): State<Arc<App>>, caller: Caller) -> ApiResult<NetworkView> {
    caller.permitted(SETTINGS_EDIT)?;
    allow_inbound().await.map_err(crate::platform::http::ApiError::conflict)?;
    Ok(ok(view_of(refresh().await)))
}

/// How long PowerShell may take: the rule lookup is slow on a computer with many rules.
const LOOK_WAIT: Duration = Duration::from_secs(20);

/// The look, on Windows; None elsewhere or when PowerShell does not answer.
pub async fn look() -> Option<WindowsNetwork> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let script = r#"$ErrorActionPreference = 'SilentlyContinue'
$n = @(Get-NetConnectionProfile | ForEach-Object { @{ alias = "$($_.InterfaceAlias)"; category = "$($_.NetworkCategory)" } })
$f = @(Get-NetFirewallProfile | ForEach-Object { @{ name = "$($_.Name)"; enabled = [bool]($_.Enabled -eq 'True' -or $_.Enabled -eq 1) } })
$r = @(Get-NetFirewallApplicationFilter -Program $env:DUOWAVE_EXE | Get-NetFirewallRule | Where-Object { "$($_.Direction)" -eq 'Inbound' -and "$($_.Enabled)" -eq 'True' } | ForEach-Object { @{ name = "$($_.DisplayName)"; action = "$($_.Action)"; profile = "$($_.Profile)" } })
@{ networks = $n; firewall = $f; rules = $r } | ConvertTo-Json -Compress -Depth 4"#;
    let output = run_powershell(script, &[("DUOWAVE_EXE", exe.to_string_lossy().into_owned())], LOOK_WAIT).await;
    match output {
        Ok(text) => match parse_look(&text) {
            Ok(network) => Some(network),
            Err(error) => {
                tracing::warn!(
                    "windows network: the look did not read ({error}): {}",
                    text.chars().take(300).collect::<String>()
                );
                None
            }
        },
        Err(error) => {
            tracing::warn!("windows network: PowerShell did not answer: {error}");
            None
        }
    }
}

/// The look's JSON as PowerShell printed it: around it a line end, and a byte-order mark when the console's encoding
/// writes one.
fn parse_look(text: &str) -> Result<WindowsNetwork, serde_json::Error> {
    serde_json::from_str(text.trim().trim_start_matches('\u{feff}'))
}

/// This program's inbound rules replaced with one allow rule on the private and domain profiles: one UAC prompt. Err
/// when the person declined it or netsh failed.
pub async fn allow_inbound() -> Result<(), String> {
    if !cfg!(windows) {
        return Err(crate::tr!("Это нужно только на Windows."));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    // cmd runs both netsh commands under the one elevation; the program's path goes in quotes (it may have spaces).
    let script = r#"$exe = $env:DUOWAVE_EXE
$commands = "/c netsh advfirewall firewall delete rule name=all dir=in program=`"$exe`" & netsh advfirewall firewall add rule name=`"DuoWave`" dir=in action=allow program=`"$exe`" enable=yes profile=private,domain"
try {
    $p = Start-Process -FilePath cmd.exe -ArgumentList $commands -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    exit $p.ExitCode
} catch {
    Write-Output "declined: $($_.Exception.Message)"
    exit 1223
}"#;
    tracing::info!("windows network: asking to allow DuoWave in the firewall (one elevation prompt)");
    match run_powershell(script, &[("DUOWAVE_EXE", exe.to_string_lossy().into_owned())], Duration::from_secs(120)).await
    {
        Ok(_) => {
            tracing::info!("windows network: DuoWave allowed in the firewall on private networks");
            Ok(())
        }
        Err(error) if error.contains("1223") || error.contains("declined") => {
            tracing::warn!("windows network: the elevation was declined: {error}");
            Err(crate::tr!("Windows не дал прав: разрешение отменено."))
        }
        Err(error) => {
            tracing::warn!("windows network: the firewall rule was not added: {error}");
            Err(crate::tr!("Не вышло добавить разрешение в брандмауэр: {error}", error = error))
        }
    }
}

/// What every script starts with: its output in UTF-8. Windows PowerShell 5.1 writes into a pipe in the console's
/// code page (437 or 866), and a network named in Russian came out as «????» (install.ps1 met the same, 68eee1d6).
const UTF8_OUTPUT: &str = "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n";

/// Runs a PowerShell script with no window and no profile; its stdout when it ends with 0, else why.
async fn run_powershell(script: &str, env: &[(&str, String)], wait: Duration) -> Result<String, String> {
    let script = format!("{UTF8_OUTPUT}{script}");
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for (name, value) in env {
        command.env(name, value);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(wait, command.output())
        .await
        .map_err(|_| format!("no answer in {} s", wait.as_secs()))?
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("exit {:?}: {} {}", output.status.code(), stdout.trim(), stderr.trim()).chars().take(400).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(json: &str) -> WindowsNetwork {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn reads_what_powershell_says_and_names_what_closes_the_computer() {
        let private_allowed = read(
            r#"{"networks":[{"alias":"Wi-Fi","category":"Private"}],"firewall":[{"name":"Private","enabled":true}],
                "rules":[{"name":"DuoWave","action":"Allow","profile":"Private, Domain"}]}"#,
        );
        assert!(!private_allowed.likely_closed());
        assert_eq!(advice(&private_allowed), None);

        let public = read(
            r#"{"networks":[{"alias":"Wi-Fi","category":"Public"},{"alias":"vEthernet (WSL)","category":"Private"}],
                "firewall":[{"name":"Public","enabled":true}],"rules":[{"name":"DuoWave","action":"Allow","profile":"Private"}]}"#,
        );
        assert!(public.likely_closed());
        assert!(advice(&public).unwrap().contains("«Wi-Fi»"), "{:?}", advice(&public));

        // «Отмена» in Windows' own window: a block rule, which wins over the allow one.
        let blocked = read(
            r#"{"networks":[{"alias":"Wi-Fi","category":"Private"}],"firewall":[{"name":"Private","enabled":true}],
                "rules":[{"name":"duowave.exe","action":"Block","profile":"Private"},{"name":"DuoWave","action":"Allow","profile":"Private"}]}"#,
        );
        assert!(blocked.likely_closed());
        assert!(advice(&blocked).unwrap().contains("Отмена"));

        // No rule at all, the firewall on: no allowance yet.
        let none = read(
            r#"{"networks":[{"alias":"Ethernet","category":"Private"}],"firewall":[{"name":"Private","enabled":true}],"rules":[]}"#,
        );
        assert!(none.likely_closed() && advice(&none).is_some());
        // The firewall off: nothing to say.
        let off = read(
            r#"{"networks":[{"alias":"Ethernet","category":"Private"}],"firewall":[{"name":"Private","enabled":false}],"rules":[]}"#,
        );
        assert!(!off.likely_closed() && advice(&off).is_none());
        assert!(off.summary().contains("Private off"), "{}", off.summary());
    }

    /// A tester's Windows, 22.11: a «Общедоступная» Ethernet, the Domain firewall on, Private and Public off, an allow
    /// rule for Private and Public — the computer is open, and nothing is said. Each network goes by its own profile.
    #[test]
    fn a_public_network_with_its_firewall_off_or_this_program_allowed_is_open() {
        let tester = read(
            r#"{"networks":[{"alias":"Ethernet","category":"Public"}],
                "firewall":[{"name":"Domain","enabled":true},{"name":"Private","enabled":false},{"name":"Public","enabled":false}],
                "rules":[{"name":"DuoWave","action":"Allow","profile":"Private, Public"}]}"#,
        );
        assert!(!tester.likely_closed(), "{}", tester.summary());
        assert_eq!(advice(&tester), None);

        // The Public firewall on, this program allowed there: open too.
        let allowed = read(
            r#"{"networks":[{"alias":"Ethernet","category":"Public"}],"firewall":[{"name":"Public","enabled":true}],
                "rules":[{"name":"DuoWave","action":"Allow","profile":"Any"}]}"#,
        );
        assert!(!allowed.likely_closed());
        assert_eq!(advice(&allowed), None);

        // A block rule for Public only does not close a private network.
        let blocked_elsewhere = read(
            r#"{"networks":[{"alias":"Wi-Fi","category":"Private"}],
                "firewall":[{"name":"Private","enabled":true},{"name":"Public","enabled":true}],
                "rules":[{"name":"duowave.exe","action":"Block","profile":"Public"},{"name":"DuoWave","action":"Allow","profile":"Private, Domain"}]}"#,
        );
        assert!(!blocked_elsewhere.likely_closed());
        assert_eq!(advice(&blocked_elsewhere), None);

        // A domain network goes by the Domain profile: on, and no rule allows it there.
        let domain = read(
            r#"{"networks":[{"alias":"Office","category":"DomainAuthenticated"}],
                "firewall":[{"name":"Domain","enabled":true},{"name":"Private","enabled":true}],
                "rules":[{"name":"DuoWave","action":"Allow","profile":"Private"}]}"#,
        );
        assert!(domain.likely_closed());
        assert!(advice(&domain).unwrap().contains("нет разрешения"), "{:?}", advice(&domain));

        // No network read: judged as a home network, by the Private profile.
        let unread = read(r#"{"networks":[],"firewall":[{"name":"Private","enabled":true}],"rules":[]}"#);
        assert!(unread.likely_closed() && advice(&unread).is_some());
    }

    #[test]
    fn a_single_object_from_convert_to_json_reads_too() {
        // ConvertTo-Json of an @() with one element is still an array; an empty @() is [].
        let network = read(r#"{"networks":[],"firewall":[],"rules":[]}"#);
        assert!(!network.likely_closed());
        assert!(network.summary().contains("none read"));
    }

    #[test]
    fn the_look_reads_with_a_line_end_and_a_byte_order_mark() {
        let network = parse_look("\u{feff}{\"networks\":[{\"alias\":\"Беспроводная сеть\",\"category\":\"Public\"}],\"firewall\":[],\"rules\":[]}\r\n")
            .unwrap();
        assert_eq!(network.networks[0].alias, "Беспроводная сеть");
        assert!(parse_look("").is_err());
    }

    /// Windows: PowerShell's output comes back in UTF-8 — a network named in Russian is read as it is named.
    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_says_russian_names_as_they_are() {
        let said = run_powershell("Write-Output 'Сеть «Домашняя»'", &[], LOOK_WAIT).await.unwrap();
        assert_eq!(said.trim().trim_start_matches('\u{feff}'), "Сеть «Домашняя»");
        let named = run_powershell(
            "@{ networks = @(@{ alias = $env:NAME; category = 'Private' }) } | ConvertTo-Json -Compress -Depth 4",
            &[("NAME", "Беспроводная сеть".to_string())],
            LOOK_WAIT,
        )
        .await
        .unwrap();
        let network = parse_look(&named).unwrap();
        assert_eq!(network.networks.len(), 1, "one network is still a list: {named}");
        assert_eq!(network.networks[0].alias, "Беспроводная сеть");
    }

    /// Windows: the look itself on this computer — PowerShell's cmdlets answer and their JSON reads; the firewall has
    /// its profiles (Domain, Private, Public) on every Windows.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_look_reads_this_windows() {
        let network = look().await.expect("the look answers on Windows");
        assert!(!network.firewall.is_empty(), "{}", network.summary());
        assert!(network.firewall.iter().all(|profile| !profile.name.is_empty()), "{}", network.summary());
        let view = view_of(Some(network.clone()));
        assert!(view.windows);
        assert_eq!(view.likely_closed, network.likely_closed());
    }

    /// Windows: an error of PowerShell comes back as an error, with its code.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_failing_script_says_its_code() {
        let error = run_powershell("exit 1223", &[], LOOK_WAIT).await.unwrap_err();
        assert!(error.contains("1223"), "{error}");
    }

    #[tokio::test]
    async fn nothing_to_look_at_off_windows() {
        if !cfg!(windows) {
            assert_eq!(look().await, None);
            assert!(allow_inbound().await.is_err());
        }
    }
}
