//! The station's own files, as the gateway on it — or `duowave station …` in its shell — changes them: where its voice
//! goes (voice.sh: our copy of quasar.cfg mounted over the firmware's own, which is never changed), the gateway's
//! program, data and service in /data, the boot script in /etc/init.d, the safety net's marks. One implementation for
//! the panel (which runs these commands over SSH), for hands, and for the station's own gateway («Вернуть как было»
//! without a computer). Every path is under a root: `/` on the station, a folder in the tests — never the station
//! itself.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::stations::connect::quasar_config::{
    gateway_addresses, parse_gateway_host, read_config, yandex, GatewayHost, PointsTo, VoiceAddresses, STATION_CONFIG,
};
use crate::stations::connect::station_scripts::{
    copy_state, restore_script, CopyState, BOOT_SCRIPT, BOOT_SH, BOOT_SOURCE, FIRMWARE_COPY, REFUSED, RESTORE_SCRIPT,
    VOICE_COPY, VOICE_SCRIPT, VOICE_SH,
};

/// Everything of ours on the station.
pub const DIR: &str = "/data/opt/duowave";
/// The program the panel installed (the YAM package's stays in its package).
pub const BIN: &str = "/data/opt/duowave/bin/duowave";
/// Which program the service runs: written at install.
pub const BIN_LINK: &str = "/data/opt/duowave/gateway-bin";
/// The gateway's data: its database, journal and .env (DATA_DIR).
pub const DATA: &str = "/data/opt/duowave/gateway";
/// The gateway's service: started at boot by /etc/init.d/S95duowave, in /data like everything of ours.
pub const SERVICE: &str = "/data/opt/duowave/gateway.sh";
/// How the shell runs it: `{SERVICE_COMMAND} start|stop|restart|status`.
pub const SERVICE_COMMAND: &str = "sh /data/opt/duowave/gateway.sh";
/// The service of the gateways before the boot script: taken away where found, or two would start.
pub const LEGACY_SERVICE: &str = "/etc/init.d/S99zgateway";
/// voice.sh's lock: one change of the voice at a time.
pub const LOCK: &str = "/data/opt/duowave/.switching";
/// Where the voice went before this station's own gateway: the copy voice.sh keeps, the safety net's way back.
pub const PREVIOUS_COPY: &str = "/data/opt/duowave/voice/previous.cfg";
/// The safety net's marks (station-client/gateway.sh): it gave the station back and keeps the gateway off; what it did.
pub const DISABLED: &str = "/data/opt/duowave/disabled";
pub const FIRED: &str = "/data/opt/duowave/safety-fired";
pub const SAFETY_OFF: &str = "/data/opt/duowave/safety-off";
/// An update was installed: the version before it is `duowave.previous` until the new one has run well.
pub const UPDATED: &str = "/data/opt/duowave/updated";
/// Who updates the program here: pc (the computer that installed it), yam (the YAM package duowave/gateway).
pub const MANAGED: &str = "/data/opt/duowave/managed-by";
/// The settings the computer sent, read by the gateway at its next start (src/placement/transfer.rs): in its state
/// folder, beside its database.
pub const INCOMING_SETTINGS: &str = "/data/opt/duowave/gateway/state/incoming-settings.json";

/// The service, from the repository: the same file the YAM package's postinstall writes through this program.
pub const SERVICE_SCRIPT: &str = include_str!("../../../station-client/gateway.sh");

/// The gateway's port on the station: not in the station's own config (1961, 5081, 8080, 9832, 9850, 9875–9926,
/// 11124–11125), nor SSH 2222 or mDNS 5353.
pub const DEFAULT_PORT: u16 = 8790;

/// How long a first start has to answer /health before its autostart is written.
const START: std::time::Duration = std::time::Duration::from_secs(20);

/// What runs commands on the station: the mounts, the reboot, the service, the shell. The real one on the station; a
/// recorder in the tests.
pub trait System: Send + Sync {
    fn remount(&self, writable: bool) -> std::io::Result<()>;
    fn sync(&self);
    fn reboot(&self) -> std::io::Result<()>;
    /// `sh <the service> <action>`.
    fn service(&self, script: &Path, action: &str) -> std::io::Result<String>;
    /// Whether nothing listens on the port.
    fn port_free(&self, port: u16) -> bool;
    /// Whether the gateway on this port answers /health.
    fn healthy(&self, port: u16) -> bool;
    /// A command of the station's shell (voice.sh), run to its end.
    fn run(&self, command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
        command.output()
    }
}

/// The station's own: busybox's mount, sync, reboot.
pub struct RealSystem;

impl System for RealSystem {
    fn remount(&self, writable: bool) -> std::io::Result<()> {
        let mode = if writable { "remount,rw" } else { "remount,ro" };
        let status = std::process::Command::new("mount").args(["-o", mode, "/"]).status()?;
        if writable && !status.success() {
            return Err(std::io::Error::other("mount -o remount,rw / failed"));
        }
        Ok(())
    }

    fn sync(&self) {
        let _ = std::process::Command::new("sync").status();
    }

    fn reboot(&self) -> std::io::Result<()> {
        // A second's grace: the answer (to SSH or the panel) leaves first.
        std::process::Command::new("sh").args(["-c", "(sleep 1; reboot) >/dev/null 2>&1 &"]).status().map(|_| ())
    }

    fn service(&self, script: &Path, action: &str) -> std::io::Result<String> {
        let output = std::process::Command::new("sh").arg(script).arg(action).output()?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn port_free(&self, port: u16) -> bool {
        std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
    }

    fn healthy(&self, port: u16) -> bool {
        answers_health(port)
    }
}

/// GET /health on this machine's port: a 200 within two seconds.
pub fn answers_health(port: u16) -> bool {
    use std::io::Read as _;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    if stream.write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").is_err() {
        return false;
    }
    let mut head = [0u8; 16];
    let read = stream.read(&mut head).unwrap_or(0);
    let line = String::from_utf8_lossy(&head[..read]);
    line.starts_with("HTTP/1.") && line.get(9..12) == Some("200")
}

/// Where to point the station's voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// This station's own gateway: ws://127.0.0.1:<its port>/uni.ws.
    Local,
    /// A gateway elsewhere: the computer's.
    Gateway(GatewayHost),
    /// Yandex's own addresses: the station as before any gateway.
    Yandex,
    /// What it had before it was last pointed at this station's gateway.
    Previous,
}

impl Target {
    pub fn parse(text: &str) -> Option<Target> {
        match text {
            "local" => Some(Target::Local),
            "yandex" => Some(Target::Yandex),
            "previous" => Some(Target::Previous),
            other => parse_gateway_host(other).map(Target::Gateway),
        }
    }

    /// A way back replaces whatever is in our copy; a new address does not replace someone else's.
    fn forced(&self) -> bool {
        matches!(self, Target::Yandex | Target::Previous)
    }
}

/// How a point went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pointed {
    pub from: VoiceAddresses,
    pub to: VoiceAddresses,
    /// Our copy of the config, mounted over the firmware's; none when the firmware's own is in force.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<String>,
    pub changed: bool,
    pub rebooting: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub port: Option<u16>,
    /// The computer as the home server: its address and the key it gave this station.
    pub home_server: Option<(String, String)>,
    /// The key the computer reaches this station's gateway with.
    pub peer_key: Option<String>,
    /// pc, yam: who updates the program.
    pub managed: Option<String>,
    /// The program to install; None: the one running.
    pub program: Option<PathBuf>,
    /// Run the program where it is (the YAM package's): no copy into /data.
    pub in_place: bool,
    /// Started, and once it answers /health, started with the station too. Without: the files only — whoever starts
    /// it checks it (the computer's move points the voice at it, and that writes the boot script).
    pub start: bool,
    /// How long the start has to answer /health; None: 20 s.
    pub wait: Option<std::time::Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub program: String,
    pub port: u16,
    /// The port asked for was taken: this is the next free one.
    pub port_moved: bool,
    pub started: bool,
}

/// What `duowave station status` and the computer's panel read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationStatus {
    pub voice: Option<VoiceAddresses>,
    pub points_to: Option<PointsTo>,
    /// The voice goes to this station's own gateway.
    pub local: bool,
    /// Our copy of the config: mounted, kept but left unmounted at the last boot (and why), or none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyState>,
    pub installed: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Space>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    /// The safety net gave the station back and keeps the gateway off.
    pub disabled: bool,
    pub safety: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fired: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub free_kb: u64,
    pub total_kb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub total_kb: u64,
    pub available_kb: u64,
    /// What the gateway itself holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_kb: Option<u64>,
}

/// The station's files under a root.
pub struct StationFs {
    root: PathBuf,
}

impl StationFs {
    /// The station's own: `/`, or DUOWAVE_STATION_ROOT to try the commands on a copy (voice.sh's mounts then fail: it
    /// changes nothing).
    pub fn real() -> StationFs {
        let root = std::env::var_os("DUOWAVE_STATION_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        StationFs { root }
    }

    pub fn at(root: &Path) -> StationFs {
        StationFs { root: root.to_path_buf() }
    }

    /// An absolute station path under the root.
    pub fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    fn read(&self, absolute: &str) -> Option<String> {
        std::fs::read_to_string(self.path(absolute)).ok()
    }

    fn exists(&self, absolute: &str) -> bool {
        self.path(absolute).exists()
    }

    /// Whether this is a station this code knows: its config is there and readable.
    pub fn is_station(&self) -> bool {
        self.read(STATION_CONFIG).is_some_and(|text| read_config(&text).is_ok())
    }

    /// Where the voice goes now: the config as the station reads it — our copy while it is mounted.
    pub fn voice(&self) -> Result<VoiceAddresses, String> {
        let text = self.read(STATION_CONFIG).ok_or_else(|| format!("нет {STATION_CONFIG}: это не колонка"))?;
        read_config(&text).map(|facts| facts.addresses).map_err(|error| error.to_string())
    }

    /// Something is mounted over the config.
    fn mounted(&self) -> bool {
        let table = self.read("/proc/mounts").unwrap_or_default();
        let config = self.path(STATION_CONFIG);
        table.lines().any(|line| line.split_whitespace().nth(1) == Some(&*config.to_string_lossy()))
    }

    /// The firmware's own addresses: its file, or the copy of it kept while ours is mounted over it.
    fn own_addresses(&self) -> Option<VoiceAddresses> {
        let file = if self.mounted() { FIRMWARE_COPY } else { STATION_CONFIG };
        Some(read_config(&self.read(file)?).ok()?.addresses)
    }

    /// The port this station's gateway listens on: its .env, else the default.
    pub fn port(&self) -> u16 {
        self.env_value("PORT").and_then(|port| port.parse().ok()).unwrap_or(DEFAULT_PORT)
    }

    /// This station's own gateway as the station dials it.
    pub fn local_addresses(&self) -> VoiceAddresses {
        gateway_addresses(&GatewayHost { host: "127.0.0.1".into(), port: self.port() }, "/uni.ws")
    }

    /// voice.sh, the boot script's source and restore.sh, this program's own copies, into /data/opt/duowave.
    fn put_scripts(&self) -> Result<(), String> {
        std::fs::create_dir_all(self.path(DIR)).map_err(|error| error.to_string())?;
        for (path, text) in [
            (VOICE_SCRIPT, VOICE_SH.to_string()),
            (BOOT_SOURCE, BOOT_SH.to_string()),
            (RESTORE_SCRIPT, restore_script()),
        ] {
            write_atomic(&self.path(path), text.as_bytes())?;
            make_executable(&self.path(path));
        }
        Ok(())
    }

    /// `sh voice.sh <args>`, its paths under the root.
    fn voice_command(&self, args: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.arg(self.path(VOICE_SCRIPT)).args(args);
        if self.root != Path::new("/") {
            command
                .env("DUOWAVE_CFG", self.path(STATION_CONFIG))
                .env("DUOWAVE_DIR", self.path(DIR))
                .env("DUOWAVE_INITD", self.path("/etc/init.d"))
                .env("DUOWAVE_MOUNTS", self.path("/proc/mounts"));
        }
        command
    }

    /// Points the station's voice through voice.sh: our copy of the config with the two addresses, mounted over the
    /// firmware's own file (which stays as it is) and mounted again at every boot; the firmware's own addresses take
    /// the copy away. Then a reboot when asked. Pointing at this station's gateway keeps where the voice went before
    /// (voice.sh's previous.cfg) — the safety net's way back.
    pub fn point(&self, target: &Target, reboot: bool, system: &dyn System) -> Result<Pointed, String> {
        let from = self.voice()?;
        let to = match target {
            Target::Local => self.local_addresses(),
            Target::Gateway(host) => gateway_addresses(host, "/uni.ws"),
            Target::Yandex => yandex(),
            Target::Previous => self.previous_address().or_else(|| self.own_addresses()).unwrap_or_else(yandex),
        };
        // Nothing to change only when the next boot keeps it too: after the safety net the station reads the way back
        // (previous.cfg) while our copy still names its own gateway (live 08.11: «На компьютере» left that copy, and
        // the next boot would have sent the voice to the station's gateway again).
        let lasting = self.lasting_address();
        if from == to && lasting.as_ref().is_none_or(|lasting| *lasting == to) {
            return Ok(Pointed { from, to, copy: None, changed: false, rebooting: false });
        }
        self.put_scripts()?;
        let mut args = vec!["point", to.url.as_str(), to.quic.as_str()];
        if target.forced() {
            args.push("--force");
        }
        let output = system.run(&mut self.voice_command(&args)).map_err(|error| format!("voice.sh: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if said.is_empty() { format!("voice.sh ended with {}", output.status) } else { said });
        }
        if self.voice()? != to {
            return Err("голос колонки не тот, что задумано: без перезагрузки".into());
        }
        let copy = stdout.lines().find_map(|line| line.strip_prefix("copy: ")).map(str::to_string);
        // The station reads the same address as before: what changed is what the next boot mounts, no reboot needed.
        let rebooting = reboot && from != to;
        if rebooting {
            system.reboot().map_err(|error| error.to_string())?;
        }
        Ok(Pointed { from, to, copy, changed: true, rebooting })
    }

    /// The address the next boot gives the station: our copy's, or none when there is no copy (the firmware's own).
    fn lasting_address(&self) -> Option<VoiceAddresses> {
        read_config(&self.read(VOICE_COPY)?).ok().map(|facts| facts.addresses)
    }

    /// Where the voice went before this station's gateway: kept when it was pointed here.
    pub fn previous_address(&self) -> Option<VoiceAddresses> {
        let previous = read_config(&self.read(PREVIOUS_COPY)?).ok()?.addresses;
        (!previous.url.starts_with("ws://127.0.0.1:")).then_some(previous)
    }

    /// Puts the gateway on the station: the program into /data (or where a YAM package keeps it), its .env with the
    /// station's defaults (a household's own lines kept), the service and voice.sh into /data/opt/duowave, then — with
    /// `start` — starts it, and once it answers /health, writes the boot script that starts it with the station
    /// (before Alice). A start that does not answer is stopped and its autostart not written. The port asked for
    /// taken by something else: the next free one.
    pub fn install(&self, options: &InstallOptions, system: &dyn System) -> Result<Installed, String> {
        let service = self.path(SERVICE);
        let asked = options.port.unwrap_or_else(|| self.port());
        let running_here = service.exists() && system.service(&service, "status").is_ok_and(|status| running(&status));
        let port = if running_here && asked == self.port() {
            asked
        } else {
            (asked..asked.saturating_add(10))
                .find(|port| system.port_free(*port))
                .ok_or_else(|| format!("порты {asked}–{} на колонке заняты", asked.saturating_add(9)))?
        };
        std::fs::create_dir_all(self.path(DATA)).map_err(|error| error.to_string())?;
        let source = match &options.program {
            Some(program) => program.clone(),
            None => std::env::current_exe().map_err(|error| format!("программа: {error}"))?,
        };
        let program = if options.in_place {
            source.to_string_lossy().into_owned()
        } else {
            let target = self.path(BIN);
            std::fs::create_dir_all(target.parent().expect("a folder")).map_err(|error| error.to_string())?;
            if source != target {
                let bytes = std::fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;
                write_atomic(&target, &bytes)?;
                make_executable(&target);
            }
            BIN.to_string()
        };
        write_atomic(&self.path(BIN_LINK), format!("{program}\n").as_bytes())?;
        if let Some(managed) = &options.managed {
            write_atomic(&self.path(MANAGED), format!("{managed}\n").as_bytes())?;
        }
        self.write_env(port, options)?;
        let _ = std::fs::remove_file(self.path(DISABLED));
        write_atomic(&service, SERVICE_SCRIPT.as_bytes())?;
        make_executable(&service);
        self.put_scripts()?;
        if self.exists(LEGACY_SERVICE) {
            // The gateway's old service would start a second one.
            let _ = system.service(&self.path(LEGACY_SERVICE), "stop");
            self.on_root(system, |fs| {
                std::fs::remove_file(fs.path(LEGACY_SERVICE)).map_err(|error| error.to_string())
            })?;
        }
        if !options.start {
            return Ok(Installed { program, port, port_moved: port != asked, started: false });
        }
        let _ = system.service(&service, if running_here { "restart" } else { "start" });
        let wait = options.wait.unwrap_or(START);
        let deadline = std::time::Instant::now() + wait;
        while !system.healthy(port) {
            if std::time::Instant::now() >= deadline {
                let _ = system.service(&service, "stop");
                return Err(format!(
                    "шлюз не ответил на /health за {} с: остановлен, автозапуск не записан (duowave station log)",
                    wait.as_secs().max(1)
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        self.write_boot_script(system)?;
        Ok(Installed { program, port, port_moved: port != asked, started: true })
    }

    /// The boot script into /etc/init.d, the root read-only again after.
    fn write_boot_script(&self, system: &dyn System) -> Result<(), String> {
        if self.read(BOOT_SCRIPT).as_deref() == Some(BOOT_SH) {
            return Ok(());
        }
        self.on_root(system, |fs| {
            write_atomic(&fs.path(BOOT_SCRIPT), BOOT_SH.as_bytes())?;
            make_executable(&fs.path(BOOT_SCRIPT));
            Ok(())
        })
    }

    /// A change on the firmware's partition: writable for it, read-only again after, whatever happens.
    fn on_root(
        &self,
        system: &dyn System,
        change: impl FnOnce(&StationFs) -> Result<(), String>,
    ) -> Result<(), String> {
        system.remount(true).map_err(|error| error.to_string())?;
        let changed = change(self);
        system.sync();
        let _ = system.remount(false);
        changed
    }

    /// The station's .env: written with its defaults once; later installs change only what they bring.
    fn write_env(&self, port: u16, options: &InstallOptions) -> Result<(), String> {
        let path = self.path(&format!("{DATA}/.env"));
        let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            "# The voice gateway on this station (duowave station install). The full list: .env.example in the repository.\n\
             # No ONNX Runtime on the station: the end of speech is heard by energy.\n\
             VAD_ENGINE=energy\n\
             # The station's /data is small: journals for two days and 10 MB at most, no audio.\n\
             RECORDER_RETENTION_DAYS=2\n\
             RECORDER_MAX_MB=10\n\
             RECORDER_AUDIO=false\n\
             ALERT_MACOS_NOTIFY=false\n"
                .to_string()
        });
        let mut set = |key: &str, value: &str| text = with_env(&text, key, value);
        set("PORT", &port.to_string());
        if let Some((url, key)) = &options.home_server {
            set("HOME_SERVER_URL", url);
            set("HOME_SERVER_KEY", key);
        }
        if let Some(peer) = &options.peer_key {
            set("PEER_KEY", peer);
        }
        if let Some(managed) = &options.managed {
            set("DUOWAVE_MANAGED", managed);
        }
        write_atomic(&path, text.as_bytes())?;
        restrict(&path);
        Ok(())
    }

    /// A value of the station gateway's .env.
    pub fn env_value(&self, key: &str) -> Option<String> {
        let text = self.read(&format!("{DATA}/.env"))?;
        text.lines().rev().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
        })
    }

    /// Takes the gateway off the station. Refused while the station's voice goes to it: it would fall silent. The boot
    /// script stays while our copy sends the voice elsewhere (a computer's gateway): it mounts that copy.
    pub fn remove(&self, keep_data: bool, system: &dyn System) -> Result<(), String> {
        if self.voice().is_ok_and(|voice| voice == self.local_addresses()) {
            return Err(
                "голос колонки идёт в этот шлюз: сначала верните его — duowave station point previous --reboot".into(),
            );
        }
        if self.exists(SERVICE) {
            let _ = system.service(&self.path(SERVICE), "stop");
        }
        let copy_in_use = self.exists(VOICE_COPY);
        if self.exists(LEGACY_SERVICE) || (!copy_in_use && self.exists(BOOT_SCRIPT)) {
            self.on_root(system, |fs| {
                let _ = std::fs::remove_file(fs.path(LEGACY_SERVICE));
                if !copy_in_use {
                    let _ = std::fs::remove_file(fs.path(BOOT_SCRIPT));
                }
                Ok(())
            })?;
        }
        for file in [SERVICE, BIN_LINK, DISABLED, FIRED, UPDATED, MANAGED] {
            let _ = std::fs::remove_file(self.path(file));
        }
        if !copy_in_use {
            for file in [VOICE_SCRIPT, BOOT_SOURCE, RESTORE_SCRIPT, LOCK] {
                let _ = std::fs::remove_file(self.path(file));
            }
            let _ = std::fs::remove_dir_all(self.path("/data/opt/duowave/voice"));
        }
        let _ = std::fs::remove_dir_all(self.path("/data/opt/duowave/bin"));
        if !keep_data {
            let _ = std::fs::remove_dir_all(self.path(DATA));
        }
        Ok(())
    }

    /// «Попробовать снова» after the safety net: the gateway may start again; the voice comes back to it at the
    /// station's next boot (voice.sh mounts the copy that goes to it once the gateway is no longer off).
    pub fn retry(&self, system: &dyn System) -> Result<String, String> {
        let _ = std::fs::remove_file(self.path(DISABLED));
        let _ = std::fs::remove_file(self.path(FIRED));
        system.service(&self.path(SERVICE), "start").map_err(|error| error.to_string())
    }

    /// What the safety net did, once: the gateway reads and clears it at its start.
    pub fn take_fired(&self) -> Option<(String, String)> {
        let text = self.read(FIRED)?;
        let _ = std::fs::remove_file(self.path(FIRED));
        let mut words = text.split_whitespace();
        let what = words.next()?.to_string();
        let at = words
            .next()
            .and_then(|seconds| seconds.parse::<i64>().ok())
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_else(crate::utils::time::now_iso);
        Some((what, at))
    }

    /// The safety net on or off: the service reads the mark.
    pub fn set_safety(&self, on: bool) -> Result<(), String> {
        if on {
            let _ = std::fs::remove_file(self.path(SAFETY_OFF));
            Ok(())
        } else {
            std::fs::create_dir_all(self.path(DIR)).map_err(|error| error.to_string())?;
            write_atomic(&self.path(SAFETY_OFF), b"off\n")
        }
    }

    /// The settings the computer sends: kept for the gateway's next start (or read by the running one).
    pub fn receive_settings(&self, bundle: &[u8]) -> Result<(), String> {
        serde_json::from_slice::<super::transfer::Bundle>(bundle).map_err(|_| "это не пакет настроек".to_string())?;
        std::fs::create_dir_all(self.path(&format!("{DATA}/state"))).map_err(|error| error.to_string())?;
        write_atomic(&self.path(INCOMING_SETTINGS), bundle)?;
        restrict(&self.path(INCOMING_SETTINGS));
        Ok(())
    }

    /// An update was put in place (the version before it is `.previous`): the safety net may go back to it.
    pub fn mark_updated(&self, version: &str) {
        let _ = std::fs::create_dir_all(self.path(DIR));
        let _ = write_atomic(&self.path(UPDATED), format!("{version}\n").as_bytes());
    }

    /// The new version ran well: the one before goes, and its 12 MB with it.
    pub fn settle_update(&self, program: &Path) -> bool {
        if !self.exists(UPDATED) {
            return false;
        }
        let _ = std::fs::remove_file(crate::updates::install::previous_of(program));
        let _ = std::fs::remove_file(self.path(UPDATED));
        true
    }

    /// Our copy of the config as the station has it now.
    pub fn copy(&self) -> CopyState {
        let copy = if self.mounted() {
            "mounted"
        } else if self.exists(VOICE_COPY) {
            "copy"
        } else {
            "none"
        };
        copy_state(Some(copy), self.read(REFUSED).as_deref()).expect("a known word")
    }

    pub fn status(&self, system: &dyn System) -> StationStatus {
        let voice = self.voice().ok();
        let installed = self.exists(SERVICE);
        // The pid file says «<pid> <boot id>».
        let pid = self
            .read(&format!("{DIR}/gateway.pid"))
            .and_then(|pid| pid.split_whitespace().next().and_then(|pid| pid.parse::<u32>().ok()));
        let running = installed && system.service(&self.path(SERVICE), "status").is_ok_and(|status| running(&status));
        StationStatus {
            points_to: voice.as_ref().map(|voice| crate::stations::connect::quasar_config::points_to(&voice.url)),
            local: voice.as_ref().is_some_and(|voice| *voice == self.local_addresses()),
            voice,
            copy: Some(self.copy()),
            installed,
            running,
            program: self.read(BIN_LINK).map(|text| text.trim().to_string()).filter(|text| !text.is_empty()),
            port: installed.then(|| self.port()),
            managed_by: self.managed_by(),
            home_server: self.env_value("HOME_SERVER_URL").filter(|url| !url.is_empty()),
            data: space_of(&self.path("/data")),
            memory: memory_of(&self.path("/proc/meminfo"), pid.map(|pid| self.path(&format!("/proc/{pid}/status")))),
            disabled: self.exists(DISABLED),
            safety: !self.exists(SAFETY_OFF),
            fired: self.read(FIRED).map(|text| text.trim().to_string()),
        }
    }

    pub fn managed_by(&self) -> Option<String> {
        self.read(MANAGED).map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
    }
}

/// What the service says when its gateway runs: «gateway: running (pid 7)», not «gateway: not running».
fn running(status: &str) -> bool {
    status.contains(": running")
}

/// `KEY=value` set in a .env text: the line replaced where it is, or added at the end.
pub fn with_env(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| match line.split_once('=') {
            Some((name, _)) if name.trim() == key => {
                found = true;
                format!("{key}={value}")
            }
            _ => line.to_string(),
        })
        .collect();
    if !found {
        lines.push(format!("{key}={value}"));
    }
    lines.join("\n") + "\n"
}

/// Written beside and renamed over: a crash midway leaves the old file or the new, never half of one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let incoming = path.with_file_name(format!(".{name}.new"));
    std::fs::write(&incoming, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    std::fs::rename(&incoming, path).map_err(|error| format!("{}: {error}", path.display()))
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Keys inside: only root reads it.
pub(crate) fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Free and total space of the file system that holds `path`.
pub fn space_of(path: &Path) -> Option<Space> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a valid C string and a zeroed statvfs the call fills.
        if unsafe { libc::statvfs(name.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let unit = stat.f_frsize as u64;
        Some(Space { free_kb: stat.f_bavail as u64 * unit / 1024, total_kb: stat.f_blocks as u64 * unit / 1024 })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The station's memory from /proc/meminfo, and what the gateway holds from its /proc/<pid>/status.
pub fn memory_of(meminfo: &Path, status: Option<PathBuf>) -> Option<Memory> {
    let text = std::fs::read_to_string(meminfo).ok()?;
    let field = |text: &str, name: &str| {
        text.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    let rss_kb = status.and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| field(&text, "VmRSS"));
    Some(Memory { total_kb: field(&text, "MemTotal")?, available_kb: field(&text, "MemAvailable")?, rss_kb })
}

/// This process's own memory: the gateway's /health on the station says it.
pub fn own_memory() -> Option<Memory> {
    memory_of(Path::new("/proc/meminfo"), Some(PathBuf::from("/proc/self/status")))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::stations::connect::quasar_config::{YANDEX_QUIC_URL, YANDEX_URL};
    use crate::stations::connect::station_scripts::tests::pretend_tools;

    /// The station's commands, recorded; voice.sh runs for real, with the pretend mount (a folder's bytes over the
    /// file, its own kept aside) — never a real mount.
    pub(crate) struct Recorder {
        pub calls: Mutex<Vec<String>>,
        pub taken: Vec<u16>,
        pub running: bool,
        pub healthy: bool,
        /// The pretend station's folder: its pretend tools are in bin/.
        pub root: Option<PathBuf>,
    }

    impl Default for Recorder {
        fn default() -> Recorder {
            Recorder { calls: Mutex::default(), taken: Vec::new(), running: false, healthy: true, root: None }
        }
    }

    impl Recorder {
        pub(crate) fn at(fs: &StationFs) -> Recorder {
            Recorder { root: Some(fs.root.clone()), ..Default::default() }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl System for Recorder {
        fn remount(&self, writable: bool) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(if writable { "rw" } else { "ro" }.into());
            Ok(())
        }
        fn sync(&self) {
            self.calls.lock().unwrap().push("sync".into());
        }
        fn reboot(&self) -> std::io::Result<()> {
            self.calls.lock().unwrap().push("reboot".into());
            Ok(())
        }
        fn service(&self, _script: &Path, action: &str) -> std::io::Result<String> {
            self.calls.lock().unwrap().push(format!("service {action}"));
            Ok(if self.running { "gateway: running (pid 7)".into() } else { "gateway: not running".into() })
        }
        fn port_free(&self, port: u16) -> bool {
            !self.taken.contains(&port)
        }
        fn healthy(&self, _port: u16) -> bool {
            self.healthy
        }
        fn run(&self, command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
            let root = self.root.as_ref().expect("Recorder::at a pretend station: voice.sh needs its tools");
            let inherited = std::env::var("PATH").unwrap_or_default();
            self.calls.lock().unwrap().push("voice.sh".into());
            command.env("PATH", format!("{}:{inherited}", root.join("bin").display())).env("PRETEND", root).output()
        }
    }

    pub(crate) fn config_text(url: &str, quic: &str) -> String {
        format!(
            "{{\n    \"aliced\" : {{\n        \"apiKey\" : \"not-a-real-key\",\n        \"uniProxyQuicUrl\" : \"{quic}\",\n        \"uniProxyUrl\" : \"{url}\"\n    }},\n    \"common\" : {{ \"deviceType\" : \"yandexmicro\" }}\n}}\n"
        )
    }

    /// A pretend station: its config, /data, /etc/init.d, its mount table and pretend tools — in a folder.
    pub(crate) fn pretend() -> (tempfile::TempDir, StationFs) {
        let dir = tempfile::Builder::new().prefix("station-").tempdir().unwrap();
        let fs = StationFs::at(dir.path());
        for folder in ["/system/vendor/quasar", "/etc/init.d", "/data", "/proc", "/bin"] {
            std::fs::create_dir_all(fs.path(folder)).unwrap();
        }
        std::fs::write(fs.path("/proc/mounts"), "").unwrap();
        pretend_tools(&fs.path("/bin"));
        std::fs::write(fs.path(STATION_CONFIG), config_text(YANDEX_URL, YANDEX_QUIC_URL)).unwrap();
        (dir, fs)
    }

    fn firmware_file(fs: &StationFs) -> String {
        // Under the pretend mount the firmware's own bytes are kept aside; without one, the file itself.
        let under = fs.path(&format!("{STATION_CONFIG}.under"));
        std::fs::read_to_string(if fs.mounted() { under } else { fs.path(STATION_CONFIG) }).unwrap()
    }

    #[test]
    fn points_the_voice_through_a_copy_keeping_the_way_back() {
        let (_dir, fs) = pretend();
        let system = Recorder::at(&fs);
        let pointed = fs.point(&Target::Local, true, &system).unwrap();
        assert!(pointed.changed && pointed.rebooting);
        assert_eq!(pointed.to.url, "ws://127.0.0.1:8790/uni.ws");
        assert_eq!(pointed.to.quic, "", "no way to Yandex past the gateway");
        assert_eq!(pointed.copy.as_deref(), Some(&*fs.path(VOICE_COPY).to_string_lossy()));
        assert_eq!(fs.voice().unwrap(), fs.local_addresses());
        assert_eq!(firmware_file(&fs), config_text(YANDEX_URL, YANDEX_QUIC_URL), "the firmware's own file stays");
        assert_eq!(fs.copy().copy, "mounted");
        assert_eq!(std::fs::read_to_string(fs.path(BOOT_SCRIPT)).unwrap(), BOOT_SH, "mounted again at every boot");
        assert_eq!(fs.previous_address(), None, "before it: the firmware's own — no copy to go back to");
        assert_eq!(system.calls().last().map(String::as_str), Some("reboot"));

        // Again: nothing to change, nothing touched.
        let again = fs.point(&Target::Local, true, &system).unwrap();
        assert!(!again.changed && again.copy.is_none());

        // The computer's gateway, then this station's own: the computer's is the way back.
        fs.point(&Target::Gateway(parse_gateway_host("192.0.2.10").unwrap()), false, &system).unwrap();
        fs.point(&Target::Local, false, &system).unwrap();
        assert_eq!(fs.previous_address().unwrap().url, "ws://192.0.2.10/uni.ws");
        let back = fs.point(&Target::Previous, false, &system).unwrap();
        assert_eq!(back.to.url, "ws://192.0.2.10/uni.ws");
        assert_eq!(fs.voice().unwrap().url, "ws://192.0.2.10/uni.ws");

        // Yandex's are the firmware's own: our copy goes, nothing is mounted.
        let yandex = fs.point(&Target::Yandex, false, &system).unwrap();
        assert_eq!((yandex.to.url.as_str(), yandex.to.quic.as_str()), (YANDEX_URL, YANDEX_QUIC_URL));
        assert_eq!(yandex.copy, None);
        assert!(!fs.mounted() && !fs.path(VOICE_COPY).exists());
        assert_eq!(std::fs::read_to_string(fs.path(STATION_CONFIG)).unwrap(), config_text(YANDEX_URL, YANDEX_QUIC_URL));
    }

    /// A reboot of the pretend station: mounts gone, then voice.sh's boot, as S95duowave runs it.
    fn pretend_reboot(fs: &StationFs, system: &Recorder) {
        while fs.mounted() {
            let mut umount = std::process::Command::new("sh");
            umount.arg("-c").arg("umount \"$1\"").arg("sh").arg(fs.path(STATION_CONFIG));
            assert!(system.run(&mut umount).unwrap().status.success());
        }
        assert!(system.run(&mut fs.voice_command(&["boot"])).unwrap().status.success());
    }

    /// Live 08.11: after the safety net the station reads the way back while our copy names its own gateway. The move
    /// to the computer is then not «nothing to change»: the copy is rewritten — with no reboot, the station already
    /// reads that address — or the next boot would send the voice to the station's gateway again.
    #[test]
    fn after_the_safety_net_the_move_home_rewrites_what_the_next_boot_mounts() {
        let (_dir, fs) = pretend();
        let system = Recorder::at(&fs);
        let pc = Target::Gateway(parse_gateway_host("192.0.2.10").unwrap());
        fs.point(&pc, false, &system).unwrap();
        fs.point(&Target::Local, false, &system).unwrap();
        std::fs::write(fs.path(DISABLED), "").unwrap();
        pretend_reboot(&fs, &system);
        assert_eq!(fs.voice().unwrap().url, "ws://192.0.2.10/uni.ws", "the way back mounted");
        assert_eq!(fs.lasting_address(), Some(fs.local_addresses()));

        // «Попробовать снова» once the gateway is on again: back to the station, the way back kept.
        std::fs::remove_file(fs.path(DISABLED)).unwrap();
        let again = fs.point(&Target::Local, true, &system).unwrap();
        assert!(again.changed && again.rebooting);
        assert_eq!(fs.voice().unwrap(), fs.local_addresses());
        assert_eq!(fs.previous_address().unwrap().url, "ws://192.0.2.10/uni.ws");

        // The safety net again, then «На компьютере»: the address read now is the same, the copy is not.
        std::fs::write(fs.path(DISABLED), "").unwrap();
        pretend_reboot(&fs, &system);
        let reboots = system.calls().iter().filter(|call| *call == "reboot").count();
        let home = fs.point(&pc, true, &system).unwrap();
        assert!(home.changed && !home.rebooting, "the station already reads it: no reboot");
        assert_eq!(system.calls().iter().filter(|call| *call == "reboot").count(), reboots);
        assert_eq!(fs.lasting_address().unwrap().url, "ws://192.0.2.10/uni.ws");
        assert!(!fs.path(PREVIOUS_COPY).exists());
        pretend_reboot(&fs, &system);
        assert_eq!(fs.voice().unwrap().url, "ws://192.0.2.10/uni.ws", "and so does the next boot");

        // Now both agree: nothing to change.
        assert!(!fs.point(&pc, true, &system).unwrap().changed);
    }

    #[test]
    fn one_switch_at_a_time_and_a_dead_ones_lock_goes_stale() {
        let (_dir, fs) = pretend();
        let system = Recorder::at(&fs);
        std::fs::create_dir_all(fs.path(DIR)).unwrap();
        std::fs::write(fs.path(LOCK), "1").unwrap();
        let error = fs.point(&Target::Gateway(parse_gateway_host("192.0.2.10").unwrap()), false, &system).unwrap_err();
        assert!(error.contains("someone else"), "{error}");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(11 * 60);
        std::fs::File::options().write(true).open(fs.path(LOCK)).unwrap().set_modified(old).unwrap();
        assert!(fs.point(&Target::Gateway(parse_gateway_host("192.0.2.10").unwrap()), false, &system).is_ok());
        assert_eq!(fs.voice().unwrap().url, "ws://192.0.2.10/uni.ws");
        assert!(!fs.path(LOCK).exists(), "the lock is let go");
    }

    fn program(dir: &tempfile::TempDir) -> PathBuf {
        let program = dir.path().join("duowave-new");
        std::fs::write(&program, vec![7u8; 2048]).unwrap();
        program
    }

    #[test]
    fn installs_the_program_its_env_and_the_service_and_starts_it_with_the_station_once_it_answers() {
        let (dir, fs) = pretend();
        let system = Recorder { taken: vec![8790], ..Recorder::at(&fs) };
        let options = InstallOptions {
            program: Some(program(&dir)),
            home_server: Some(("http://192.0.2.10".into(), "vgw_home".into())),
            peer_key: Some("peer-secret".into()),
            managed: Some("pc".into()),
            start: true,
            ..Default::default()
        };
        let installed = fs.install(&options, &system).unwrap();
        assert_eq!((installed.port, installed.port_moved, installed.started), (8791, true, true));
        assert_eq!(std::fs::read(fs.path(BIN)).unwrap(), vec![7u8; 2048]);
        assert_eq!(std::fs::read_to_string(fs.path(BIN_LINK)).unwrap().trim(), BIN);
        assert_eq!(std::fs::read_to_string(fs.path(SERVICE)).unwrap(), SERVICE_SCRIPT);
        assert_eq!(std::fs::read_to_string(fs.path(VOICE_SCRIPT)).unwrap(), VOICE_SH);
        assert_eq!(std::fs::read_to_string(fs.path(BOOT_SCRIPT)).unwrap(), BOOT_SH, "it answered: autostart");
        assert_eq!(fs.env_value("PORT").as_deref(), Some("8791"));
        assert_eq!(fs.env_value("HOME_SERVER_URL").as_deref(), Some("http://192.0.2.10"));
        assert_eq!(fs.env_value("PEER_KEY").as_deref(), Some("peer-secret"));
        assert_eq!(fs.env_value("VAD_ENGINE").as_deref(), Some("energy"));
        assert_eq!(fs.managed_by().as_deref(), Some("pc"));
        let calls = system.calls();
        let at = |call: &str| calls.iter().position(|c| c == call).unwrap_or_else(|| panic!("no {call} in {calls:?}"));
        assert!(at("service start") < at("rw"), "the boot script only after the start answered: {calls:?}");
        assert_eq!(calls.last().map(String::as_str), Some("ro"), "the root read-only again");

        // A household's own line stays across a later install.
        let env = fs.path(&format!("{DATA}/.env"));
        std::fs::write(&env, std::fs::read_to_string(&env).unwrap() + "LLM_MODEL=mine\n").unwrap();
        fs.install(&InstallOptions { program: Some(program(&dir)), port: Some(8791), ..Default::default() }, &system)
            .unwrap();
        assert_eq!(fs.env_value("LLM_MODEL").as_deref(), Some("mine"));
        assert_eq!(fs.env_value("PEER_KEY").as_deref(), Some("peer-secret"));
    }

    #[test]
    fn a_start_that_does_not_answer_gets_no_autostart() {
        let (dir, fs) = pretend();
        let system = Recorder { healthy: false, ..Recorder::at(&fs) };
        let options = InstallOptions {
            program: Some(program(&dir)),
            start: true,
            wait: Some(std::time::Duration::from_millis(300)),
            ..Default::default()
        };
        let error = fs.install(&options, &system).unwrap_err();
        assert!(error.contains("/health") && error.contains("автозапуск не записан"), "{error}");
        assert!(!fs.path(BOOT_SCRIPT).exists());
        assert!(system.calls().contains(&"service stop".to_string()));
        // Without a start, nothing to check yet: the files only.
        let quiet = Recorder::at(&fs);
        let installed =
            fs.install(&InstallOptions { program: Some(program(&dir)), ..Default::default() }, &quiet).unwrap();
        assert!(!installed.started && !fs.path(BOOT_SCRIPT).exists());
        assert!(fs.path(SERVICE).exists());
    }

    #[test]
    fn takes_the_old_service_away_or_two_gateways_would_start() {
        let (dir, fs) = pretend();
        std::fs::write(fs.path(LEGACY_SERVICE), "#!/bin/sh\n").unwrap();
        let system = Recorder::at(&fs);
        fs.install(&InstallOptions { program: Some(program(&dir)), ..Default::default() }, &system).unwrap();
        assert!(!fs.path(LEGACY_SERVICE).exists());
        assert!(system.calls().contains(&"service stop".to_string()), "stopped first");
    }

    #[test]
    fn will_not_remove_the_gateway_the_voice_goes_to_and_keeps_the_boot_script_a_copy_needs() {
        let (dir, fs) = pretend();
        let system = Recorder { healthy: true, ..Recorder::at(&fs) };
        fs.install(&InstallOptions { program: Some(program(&dir)), start: true, ..Default::default() }, &system)
            .unwrap();
        fs.point(&Target::Local, false, &system).unwrap();
        assert!(fs.remove(false, &system).unwrap_err().contains("point previous"));

        // The voice to the computer's gateway: the gateway goes, the boot script stays — it mounts that copy.
        fs.point(&Target::Gateway(parse_gateway_host("192.0.2.10").unwrap()), false, &system).unwrap();
        fs.remove(true, &system).unwrap();
        assert!(!fs.path(SERVICE).exists() && !fs.path(BIN).exists());
        assert!(fs.path(BOOT_SCRIPT).exists() && fs.path(VOICE_SCRIPT).exists());
        assert!(fs.path(&format!("{DATA}/.env")).exists(), "--keep-data");

        // Back to the firmware's own: nothing of ours is left on the firmware's partition.
        fs.point(&Target::Yandex, false, &system).unwrap();
        fs.remove(false, &system).unwrap();
        assert!(!fs.path(BOOT_SCRIPT).exists() && !fs.path(VOICE_SCRIPT).exists());
        assert!(!fs.path(DATA).exists());
    }

    #[test]
    fn says_where_the_copy_stands_and_why_a_boot_left_it() {
        let (_dir, fs) = pretend();
        let system = Recorder::at(&fs);
        assert_eq!(fs.status(&system).copy.unwrap().copy, "none");
        fs.point(&Target::Gateway(parse_gateway_host("192.0.2.10").unwrap()), false, &system).unwrap();
        assert_eq!(fs.status(&system).copy.unwrap().copy, "mounted");
        std::fs::write(fs.path(REFUSED), "firmware-changed 1794122772 the station's quasar.cfg is not…\n").unwrap();
        let copy = fs.copy();
        assert_eq!((copy.refused.as_deref(), copy.refused_at), (Some("firmware-changed"), Some(1794122772)));
    }

    #[test]
    fn reads_the_safety_nets_marks_once_and_its_switch() {
        let (_dir, fs) = pretend();
        std::fs::create_dir_all(fs.path(DIR)).unwrap();
        std::fs::write(fs.path(FIRED), "previous-address 1762430520\n").unwrap();
        assert_eq!(fs.take_fired(), Some(("previous-address".into(), "2025-11-06T12:02:00Z".into())));
        assert_eq!(fs.take_fired(), None);
        fs.set_safety(false).unwrap();
        assert!(!fs.status(&Recorder::default()).safety);
        fs.set_safety(true).unwrap();
        assert!(fs.status(&Recorder::default()).safety);
    }

    #[test]
    fn reads_memory_as_the_kernel_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let meminfo = dir.path().join("meminfo");
        std::fs::write(
            &meminfo,
            "MemTotal:         242912 kB\nMemFree:           10000 kB\nMemAvailable:      61234 kB\n",
        )
        .unwrap();
        let status = dir.path().join("status");
        std::fs::write(&status, "Name:\tduowave\nVmRSS:\t   10240 kB\n").unwrap();
        assert_eq!(
            memory_of(&meminfo, Some(status)),
            Some(Memory { total_kb: 242912, available_kb: 61234, rss_kb: Some(10240) })
        );
        assert!(space_of(dir.path()).is_some_and(|space| space.total_kb > 0));
    }

    #[test]
    fn health_is_a_200_on_the_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::Read as _;
            for answer in ["HTTP/1.1 200 OK\r\n\r\nok", "HTTP/1.1 503 Service Unavailable\r\n\r\n"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 256];
                let _ = stream.read(&mut request);
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        assert!(answers_health(port));
        assert!(!answers_health(port));
        server.join().unwrap();
        assert!(!answers_health(port), "nobody listens");
    }

    /// The service itself, `sh gateway.sh …`, with its paths in a folder, pretend programs and a pretend /proc.
    struct Service {
        dir: tempfile::TempDir,
    }

    impl Service {
        fn new(voice: &str) -> Service {
            let dir = tempfile::Builder::new().prefix("service-").tempdir().unwrap();
            for folder in ["bin", "tools", "proc/sys/kernel/random"] {
                std::fs::create_dir_all(dir.path().join(folder)).unwrap();
            }
            std::fs::write(dir.path().join("quasar.cfg"), config_text(voice, "")).unwrap();
            std::fs::write(dir.path().join("gateway-bin"), format!("{}\n", dir.path().join("bin/duowave").display()))
                .unwrap();
            std::fs::write(dir.path().join("proc/sys/kernel/random/boot_id"), "this-boot\n").unwrap();
            std::fs::write(dir.path().join("gateway.sh"), SERVICE_SCRIPT).unwrap();
            let reboot = dir.path().join("tools/reboot");
            std::fs::write(&reboot, format!("#!/bin/sh\necho reboot >> {}/runs\n", dir.path().display())).unwrap();
            make_executable(&reboot);
            Service { dir }
        }

        fn program(&self, name: &str, word: &str, extra: &str) {
            let path = self.dir.path().join("bin").join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho {word} >> {}/runs\n{extra}exit 1\n", self.dir.path().display()),
            )
            .unwrap();
            make_executable(&path);
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        /// A pretend process in the pretend /proc.
        fn process(&self, pid: u32, cmdline: &str) {
            std::fs::create_dir_all(self.file(&format!("proc/{pid}"))).unwrap();
            std::fs::write(self.file(&format!("proc/{pid}/cmdline")), cmdline.replace(' ', "\0")).unwrap();
        }

        fn command(&self, action: &str, env: &[(&str, &str)]) -> std::process::Command {
            let inherited = std::env::var("PATH").unwrap_or_default();
            let mut command = std::process::Command::new("sh");
            command
                .arg(self.file("gateway.sh"))
                .arg(action)
                .env("PATH", format!("{}:{inherited}", self.file("tools").display()))
                .env("DUOWAVE_DIR", self.dir.path())
                .env("DUOWAVE_CFG", self.file("quasar.cfg"))
                .env("DUOWAVE_PROC", self.file("proc"))
                .env("DUOWAVE_RESPAWN_PAUSE", "0")
                .env("DUOWAVE_WINDOW_SECS", "120")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped());
            for (key, value) in env {
                command.env(key, value);
            }
            command
        }

        fn run(&self, quick_secs: &str) -> Vec<String> {
            let output = self.command("run", &[("DUOWAVE_QUICK_SECS", quick_secs)]).output().unwrap();
            assert!(output.status.success());
            self.runs()
        }

        fn runs(&self) -> Vec<String> {
            std::fs::read_to_string(self.file("runs")).unwrap_or_default().lines().map(str::to_string).collect()
        }

        fn say(&self, action: &str) -> String {
            let output = self.command(action, &[]).output().unwrap();
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
    }

    #[test]
    fn the_safety_net_brings_the_version_back_first_then_gives_the_station_back() {
        let service = Service::new("ws://127.0.0.1:8790/uni.ws");
        service.program("duowave", "new", "");
        service.program("duowave.previous", "old", "");
        std::fs::write(service.file("updated"), "0.2.0\n").unwrap();
        let runs = service.run("30");
        assert_eq!(runs, vec!["new", "new", "new", "old", "old", "old", "reboot"]);
        assert!(service.file("disabled").exists(), "the gateway stays off: at boot voice.sh does not mount its copy");
        assert!(std::fs::read_to_string(service.file("safety-fired")).unwrap().starts_with("previous-address "));
        assert!(std::fs::read_to_string(service.file("bin/duowave")).unwrap().contains("echo old"));
        assert!(service.file("bin/duowave.failed").exists() && !service.file("updated").exists());
    }

    #[test]
    fn the_safety_net_leaves_a_gateway_alone_when_it_is_off_or_the_voice_goes_elsewhere() {
        // Off: it keeps starting it (the pretend one stops the loop after five).
        let stop_after = |service: &Service| {
            format!(
                "[ \"$(grep -c . {runs})\" -ge 5 ] && : > {disabled}\n",
                runs = service.file("runs").display(),
                disabled = service.file("disabled").display()
            )
        };
        let off = Service::new("ws://127.0.0.1:8790/uni.ws");
        off.program("duowave", "new", &stop_after(&off));
        std::fs::write(off.file("safety-off"), "off\n").unwrap();
        assert_eq!(off.run("30"), vec!["new"; 5]);
        assert!(!off.file("safety-fired").exists());

        // The voice goes to the computer: the station is not silent, nothing to give back.
        let elsewhere = Service::new("ws://192.0.2.10/uni.ws");
        elsewhere.program("duowave", "new", &stop_after(&elsewhere));
        assert_eq!(elsewhere.run("30"), vec!["new"; 5]);
        assert!(!elsewhere.file("safety-fired").exists());

        // A gateway that ran its time is no quick end.
        let long = Service::new("ws://127.0.0.1:8790/uni.ws");
        long.program("duowave", "new", &stop_after(&long));
        assert_eq!(long.run("0"), vec!["new"; 5]);
        assert!(!long.file("safety-fired").exists());
    }

    #[test]
    fn a_pid_from_another_boot_or_of_another_program_is_not_its_gateway() {
        let service = Service::new("ws://127.0.0.1:8790/uni.ws");
        service.process(4321, "/data/opt/duowave/bin/duowave serve");
        // After a power cut: the pid file of the boot before.
        std::fs::write(service.file("gateway.pid"), "4321 the-boot-before\n").unwrap();
        assert_eq!(service.say("status"), "gateway: not running");
        // This boot's, and it is the gateway.
        std::fs::write(service.file("gateway.pid"), "4321 this-boot\n").unwrap();
        assert_eq!(service.say("status"), "gateway: running (pid 4321)");
        // This boot's number, but another program has it now.
        service.process(4321, "/system/vendor/quasar/maind --mediad");
        assert_eq!(service.say("status"), "gateway: not running");
        // Empty, or not a number.
        for text in ["", "\n", "x this-boot\n"] {
            std::fs::write(service.file("gateway.pid"), text).unwrap();
            assert_eq!(service.say("status"), "gateway: not running", "{text:?}");
        }
    }

    #[test]
    fn a_gateway_that_stops_answering_is_restarted_and_the_third_time_gives_the_station_back() {
        let service = Service::new("ws://127.0.0.1:8790/uni.ws");
        // A process of ours to stand for the gateway: the watch may kill it.
        let mut standing = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        service.process(standing.id(), "duowave serve");
        std::fs::write(service.file("gateway.pid"), format!("{} this-boot\n", standing.id())).unwrap();
        std::fs::write(service.file("gateway.started"), "0\n").unwrap();
        let mut command = service.command(
            "watch",
            &[
                ("DUOWAVE_PROBE_SECS", "0"),
                ("DUOWAVE_GRACE_SECS", "0"),
                ("DUOWAVE_MISSES", "1"),
                ("DUOWAVE_KILL_WAIT_SECS", "0"),
                ("DUOWAVE_HEALTH_CMD", "false"),
            ],
        );
        let output = command.output().unwrap();
        assert!(output.status.success());
        let _ = standing.wait();
        assert_eq!(service.runs(), vec!["reboot"], "given back after the third");
        assert!(service.file("disabled").exists());
        assert!(std::fs::read_to_string(service.file("safety-fired")).unwrap().starts_with("previous-address "));
        let log = std::fs::read_to_string(service.file("gateway/gateway.log")).unwrap_or_default();
        assert_eq!(log.matches("no answer on /health").count(), 3, "{log}");
    }

    #[test]
    fn its_start_waits_for_the_gateway_when_alices_voice_goes_to_it() {
        for (voice, waits) in [("ws://127.0.0.1:8790/uni.ws", true), ("ws://192.0.2.10/uni.ws", false)] {
            let service = Service::new(voice);
            // A gateway that runs a second and is told to stay off after its first.
            let off = format!(": > {}\nsleep 1\n", service.file("disabled").display());
            service.program("duowave", "started", &off);
            // /health answers at its third asking.
            let asked = service.file("asked");
            let health = format!("echo x >> {0}; [ \"$(grep -c . {0})\" -ge 3 ]", asked.display());
            let began = std::time::Instant::now();
            let output = service
                .command("start", &[("DUOWAVE_HEALTH_CMD", &health), ("DUOWAVE_PROBE_SECS", "1")])
                .output()
                .unwrap();
            assert!(output.status.success());
            let asks = std::fs::read_to_string(&asked).unwrap_or_default().lines().count();
            if waits {
                assert_eq!(asks, 3, "asked until it answered");
                assert!(began.elapsed() >= std::time::Duration::from_secs(2));
            } else {
                assert_eq!(asks, 0, "the voice goes elsewhere: Alice does not wait");
            }
            // The loop and the watch end by themselves: the gateway is off.
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
    }
}
