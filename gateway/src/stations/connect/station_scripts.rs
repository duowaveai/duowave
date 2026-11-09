//! The shell the station runs, in one place: busybox sh and its applets. Where the voice goes is voice.sh's
//! (station-client/voice.sh): our copy of quasar.cfg bind-mounted over the firmware's own, which stays as it is; the
//! boot script /etc/init.d/S95duowave (station-client/S95duowave) mounts the copy again at every start, before Alice.
//! A reset of the station wipes /data and so the copy, an update of the firmware changes its file and so the copy is
//! not mounted: either way the station is the usual Alice.
//!
//! The panel's button and the «Сделаю сам» command put the same three files into /data/opt/duowave — voice.sh, the
//! boot script's source and restore.sh, the way back without the gateway — then point the voice, check it, reboot.

use std::sync::LazyLock;

use super::quasar_config::{safe_address, VoiceAddresses, STATION_CONFIG};

pub const CLIENT_DIR: &str = "/data/opt/duowave";
pub const RESTORE_SCRIPT: &str = "/data/opt/duowave/restore.sh";
pub const VOICE_SCRIPT: &str = "/data/opt/duowave/voice.sh";
/// The boot script's source, beside voice.sh: voice.sh puts it into /etc/init.d when it first points the voice.
pub const BOOT_SOURCE: &str = "/data/opt/duowave/S95duowave";
pub const BOOT_SCRIPT: &str = "/etc/init.d/S95duowave";
/// Our copy of the config, mounted over it, and the firmware's file it was made from.
pub const VOICE_COPY: &str = "/data/opt/duowave/voice/quasar.cfg";
pub const FIRMWARE_COPY: &str = "/data/opt/duowave/voice/firmware.cfg";
/// Why the last boot left the copy unmounted: «<what> <unix time> <words>».
pub const REFUSED: &str = "/data/opt/duowave/voice/refused";

/// The scripts themselves, from the repository.
pub const VOICE_SH: &str = include_str!("../../../../station-client/voice.sh");
pub const BOOT_SH: &str = include_str!("../../../../station-client/S95duowave");

/// An address that cannot go into the file or a shell line as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an address to write")]
pub struct UnsafeAddress;

fn quoted(value: &str) -> Result<String, UnsafeAddress> {
    if !safe_address(value) {
        return Err(UnsafeAddress);
    }
    Ok(format!("'{value}'"))
}

/// restore.sh, left on the station: the firmware's own config back — our copy unmounted and gone — then a reboot.
/// The same whatever the station was pointed at: before DuoWave its voice went where its own file says.
pub fn restore_script() -> String {
    format!(
        r##"#!/bin/sh
# Back to how this station was before the voice gateway: the firmware's own quasar.cfg (our copy over it unmounted and
# gone), then a reboot. Left here by the gateway; runs without it: sh {RESTORE_SCRIPT}
set -eu
CFG={STATION_CONFIG}
if [ -f {VOICE_SCRIPT} ]; then
    sh {VOICE_SCRIPT} drop
else
    while grep -q " $CFG " /proc/mounts; do umount "$CFG"; done
    rm -f {VOICE_COPY} {FIRMWARE_COPY}
fi
echo "back to the firmware's own voice server; rebooting"
reboot
"##
    )
}

/// A file put into /data/opt/duowave from the command's stdin: whole or not at all, executable.
fn deliver(path: &str) -> String {
    format!("mkdir -p {CLIENT_DIR} && cat > {path}.new && chmod 755 {path}.new && mv -f {path}.new {path}")
}

/// The three files the station needs before its voice is pointed: the commands, each with its stdin.
pub fn deliveries() -> Vec<(String, String)> {
    vec![
        (deliver(VOICE_SCRIPT), VOICE_SH.to_string()),
        (deliver(BOOT_SOURCE), BOOT_SH.to_string()),
        (deliver(RESTORE_SCRIPT), restore_script()),
    ]
}

/// The switch: voice.sh points the voice at `target` — the firmware's own addresses take our copy away. `force`
/// replaces a copy someone else changed through the mount (the way back does; a new connection does not).
pub fn switch_to(target: &VoiceAddresses, force: bool) -> Result<String, UnsafeAddress> {
    let force = if force { " --force" } else { "" };
    Ok(format!("sh {VOICE_SCRIPT} point {} {}{force}", quoted(&target.url)?, quoted(&target.quic)?))
}

/// A heredoc of a file's text into a path: the delimiter is checked not to be in the text.
fn heredoc(path: &str, text: &str, delimiter: &str) -> String {
    assert!(!text.contains(delimiter), "the delimiter {delimiter} is in the text");
    format!("cat > {path}.new <<'{delimiter}'\n{text}{delimiter}\nchmod 755 {path}.new && mv -f {path}.new {path}\n")
}

/// connect.sh, served by the gateway for «Сделаю сам»: the station fetches it and runs it —
/// `ssh -p 2222 root@<station> 'wget -qO- http://<gateway>/station/connect.sh | sh'`. It writes the three files, then
/// points the voice, then reboots.
pub fn connect_script(target: &VoiceAddresses) -> Result<String, UnsafeAddress> {
    let point = switch_to(target, false)?;
    let mut script = format!(
        r##"#!/bin/sh
# Connects this station to the voice gateway: its voice to {target_url}, through our copy of quasar.cfg mounted over
# the firmware's own (which stays as it was), mounted again at every boot by {BOOT_SCRIPT}. A reset of the station or an
# update of its firmware leaves the usual Alice; {RESTORE_SCRIPT} brings her back without the gateway.
set -eu
CFG={STATION_CONFIG}
[ -f "$CFG" ] || {{ echo "no $CFG: not a station this script knows; nothing changed" >&2; exit 1; }}
grep -q '"uniProxyUrl"' "$CFG" || {{ echo "no uniProxyUrl in $CFG; nothing changed" >&2; exit 1; }}
mkdir -p {CLIENT_DIR}
"##,
        target_url = target.url,
    );
    script.push_str(&heredoc(VOICE_SCRIPT, VOICE_SH, "DUOWAVE_VOICE_SH"));
    script.push_str(&heredoc(BOOT_SOURCE, BOOT_SH, "DUOWAVE_BOOT_SH"));
    script.push_str(&heredoc(RESTORE_SCRIPT, &restore_script(), "DUOWAVE_RESTORE_SH"));
    script.push_str(&format!(
        "{point}\necho \"connected to {url}; back to Yandex: sh {RESTORE_SCRIPT}\"\necho \"rebooting: the station comes back in about a minute\"\nreboot\n",
        url = target.url
    ));
    Ok(script)
}

/// The commands the button runs, step by step (the gateway checks each result in between).
pub struct Steps {
    /// Reading only: the config as the station reads it — our copy while it is mounted.
    pub read: String,
    /// Reading only: the firmware's own file — the copy of it we kept, while ours is mounted over it.
    pub firmware: String,
    /// Reading only, a line each: the free space in /data, the machine, whether our client is there, whether the
    /// updates are off as the wiki turns them off (hackalice.me/start/ssh: updatesd out of launch.json, the applier
    /// stubbed, or the OTA key replaced) — an update from Yandex would take the station's voice back and may close
    /// root —, our copy (mounted, copy, none), and why the last boot left it unmounted («-» when it did not).
    pub facts: String,
    pub reboot: &'static str,
}

pub static STEPS: LazyLock<Steps> = LazyLock::new(|| {
    let mounted = format!("grep -q ' {STATION_CONFIG} ' /proc/mounts");
    Steps {
    read: format!("cat {STATION_CONFIG}"),
    firmware: format!("if {mounted} && [ -f {FIRMWARE_COPY} ]; then cat {FIRMWARE_COPY}; else cat {STATION_CONFIG}; fi"),
    facts: [
        "df -k /data 2>/dev/null | tail -n 1".to_string(),
        "uname -m".to_string(),
        format!("[ -d {CLIENT_DIR} ] && echo client || echo none"),
        "Q=/system/vendor/quasar".to_string(),
        "if ! grep -q -- '--updatesd' $Q/launch.json 2>/dev/null || head -2 $Q/device_ota_update.sh 2>/dev/null | grep -q 'disabled by yandex-noupdate' || { [ -f /data/opt/backup/ota_script.pub.orig ] && ! cmp -s /data/opt/backup/ota_script.pub.orig $Q/ota_script.pub; }; then echo updates-blocked; else echo updates-on; fi".to_string(),
        format!("if {mounted}; then echo mounted; elif [ -f {VOICE_COPY} ]; then echo copy; else echo none; fi"),
        format!("cat {REFUSED} 2>/dev/null || echo -"),
    ]
    .join("; "),
    reboot: "(sleep 1; reboot) >/dev/null 2>&1 &",
}
});

/// How our copy stands on the station, from the facts' last two lines.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyState {
    /// mounted, copy (kept but not mounted), none.
    pub copy: String,
    /// Why the last boot left it unmounted: firmware-changed, gateway-off, previous, broken, mount-failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// When, as unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused_at: Option<i64>,
}

pub fn copy_state(copy: Option<&str>, refused: Option<&str>) -> Option<CopyState> {
    let copy = copy.map(str::trim).filter(|value| matches!(*value, "mounted" | "copy" | "none"))?;
    let refused = refused.map(str::trim).filter(|value| !value.is_empty() && *value != "-");
    let mut words = refused.unwrap_or("").split_whitespace();
    let what = words
        .next()
        .filter(|word| word.len() <= 32 && word.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-'));
    // A time from before the station's clock was set at boot (1970) is no time to show.
    let at = words.next().and_then(|at| at.parse::<i64>().ok()).filter(|at| *at > 1_000_000_000);
    Some(CopyState { copy: copy.to_string(), refused: what.map(str::to_string), refused_at: what.and(at) })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use super::*;
    use crate::stations::connect::quasar_config::{yandex, YANDEX_QUIC_URL, YANDEX_URL};

    /// voice.sh's way back for the safety net.
    const PREVIOUS_COPY: &str = "/data/opt/duowave/voice/previous.cfg";

    pub(crate) fn config() -> String {
        format!(
            r#"{{
    "aliced" : {{
        "apiKey" : "not-a-real-key",
        "uniProxyQuicUrl" : "{YANDEX_QUIC_URL}",
        "uniProxyUrl" : "{YANDEX_URL}"
    }}
}}
"#
        )
    }

    pub(crate) struct Run {
        pub status: Option<i32>,
        pub stdout: String,
        pub stderr: String,
    }

    /// A pretend mount and umount: a bind puts the source's bytes over the target and keeps the target's own aside, a
    /// umount brings them back; the mount table is a file. Remounts are only noted. Never a real mount.
    const FAKE_MOUNT: &str = r#"#!/bin/sh
echo "mount $*" >> "$PRETEND/calls"
if [ "$1" = -o ] && [ "$2" = bind ]; then
    [ -f "$PRETEND/mount-fails" ] && exit 1
    cp "$4" "$4.under" && cp "$3" "$4" && echo "pretend $4 none bind 0 0" >> "$PRETEND/proc/mounts"
fi
exit 0
"#;
    const FAKE_UMOUNT: &str = r#"#!/bin/sh
echo "umount $*" >> "$PRETEND/calls"
grep -q " $1 " "$PRETEND/proc/mounts" || exit 1
mv "$1.under" "$1" && grep -v " $1 " "$PRETEND/proc/mounts" > "$PRETEND/proc/mounts.new"; mv "$PRETEND/proc/mounts.new" "$PRETEND/proc/mounts"
"#;

    /// The pretend mount, umount, reboot and sync into `bin`: they note what they were asked in $PRETEND/calls.
    pub(crate) fn pretend_tools(bin: &std::path::Path) {
        let tool = |name: &str, text: &str| {
            let file = bin.join(name);
            std::fs::write(&file, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        };
        tool("mount", FAKE_MOUNT);
        tool("umount", FAKE_UMOUNT);
        for name in ["reboot", "sync"] {
            tool(name, &format!("#!/bin/sh\necho \"{name} $*\" >> \"$PRETEND/calls\"\n"));
        }
    }

    /// A pretend station: its config, /data, /etc/init.d and mount table in a folder, mount, umount, reboot and sync
    /// pretend on PATH, voice.sh's paths moved there (DUOWAVE_*). The real thing runs busybox sh; this is the sh of the
    /// machine running the tests — the same POSIX subset.
    pub(crate) struct Pretend {
        _dir: tempfile::TempDir,
        pub root: PathBuf,
        pub bin: PathBuf,
    }

    impl Pretend {
        pub(crate) fn new() -> Pretend {
            let dir = tempfile::Builder::new().prefix("station-").tempdir().expect("a folder");
            let root = dir.path().to_path_buf();
            let bin = root.join("bin");
            std::fs::create_dir(&bin).unwrap();
            pretend_tools(&bin);
            for folder in ["system/vendor/quasar", "data", "tmp", "etc/init.d", "proc"] {
                std::fs::create_dir_all(root.join(folder)).unwrap();
            }
            std::fs::write(root.join("proc/mounts"), "").unwrap();
            let pretend = Pretend { _dir: dir, root, bin };
            pretend.write_config(&config());
            pretend
        }

        pub(crate) fn file(&self, path: &str) -> PathBuf {
            self.root.join(path.trim_start_matches('/'))
        }

        pub(crate) fn write_config(&self, text: &str) {
            std::fs::write(self.file(STATION_CONFIG), text).unwrap();
        }

        pub(crate) fn config(&self) -> String {
            std::fs::read_to_string(self.file(STATION_CONFIG)).unwrap()
        }

        /// The station's paths inside the folder. Placeholders first, so a folder whose own path has /tmp/ or /data/
        /// in it is not moved twice.
        pub(crate) fn localize(&self, script: &str) -> String {
            let root = self.root.display().to_string();
            script
                .replace(STATION_CONFIG, "\u{0}cfg\u{0}")
                .replace("/data/", "\u{0}data\u{0}")
                .replace("/tmp/", "\u{0}tmp\u{0}")
                .replace("/proc/mounts", "\u{0}mounts\u{0}")
                .replace("/etc/init.d/", "\u{0}initd\u{0}")
                .replace("\u{0}cfg\u{0}", &format!("{root}{STATION_CONFIG}"))
                .replace("\u{0}data\u{0}", &format!("{root}/data/"))
                .replace("\u{0}tmp\u{0}", &format!("{root}/tmp/"))
                .replace("\u{0}mounts\u{0}", &format!("{root}/proc/mounts"))
                .replace("\u{0}initd\u{0}", &format!("{root}/etc/init.d/"))
        }

        /// `sh -s` with the script on its stdin, its station paths moved into the folder, and `input` after it when
        /// the command reads stdin (`cat > file`): then the script goes as `sh -c`.
        pub(crate) fn run(&self, script: &str, input: Option<&str>) -> Run {
            let script = self.localize(script);
            let mut command = Command::new("sh");
            match input {
                Some(_) => command.arg("-c").arg(&script),
                None => command.arg("-s"),
            };
            let inherited = std::env::var("PATH").unwrap_or_default();
            command
                .env("PATH", format!("{}:{inherited}", self.bin.display()))
                .env("PRETEND", &self.root)
                .env("DUOWAVE_CFG", self.file(STATION_CONFIG))
                .env("DUOWAVE_DIR", self.file(CLIENT_DIR))
                .env("DUOWAVE_INITD", self.file("/etc/init.d"))
                .env("DUOWAVE_MOUNTS", self.file("/proc/mounts"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().expect("sh runs");
            {
                use std::io::Write;
                let mut stdin = child.stdin.take().expect("stdin");
                stdin.write_all(input.unwrap_or(&script).as_bytes()).expect("the script goes in");
            }
            let output = child.wait_with_output().expect("sh ends");
            Run {
                status: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        }

        pub(crate) fn read(&self, path: &str) -> String {
            std::fs::read_to_string(self.file(path)).unwrap_or_default()
        }

        pub(crate) fn mounted(&self) -> bool {
            self.read("/proc/mounts").contains(&format!(" {} ", self.file(STATION_CONFIG).display()))
        }

        /// A reboot: mounts are gone, then the boot script runs as rcS runs it.
        pub(crate) fn reboot(&self) -> Run {
            while self.mounted() {
                let run = self.run(&format!("umount {STATION_CONFIG}"), None);
                assert_eq!(run.status, Some(0), "{}", run.stderr);
            }
            self.run(&format!("sh {BOOT_SCRIPT} start"), None)
        }

        /// The three files in place, as the panel's steps put them.
        pub(crate) fn deliver(&self) {
            for (command, input) in deliveries() {
                let run = self.run(&command, Some(&input));
                assert_eq!(run.status, Some(0), "{command}: {}", run.stderr);
            }
        }
    }

    fn target() -> VoiceAddresses {
        VoiceAddresses { url: "ws://192.0.2.10/uni.ws".into(), quic: String::new() }
    }

    fn url_of(text: &str) -> String {
        crate::stations::connect::quasar_config::read_config(text).unwrap().addresses.url
    }

    fn syntax_ok(script: &str) {
        let mut child = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh runs");
        {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}\n{script}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn are_valid_sh() {
        for script in [
            connect_script(&target()).unwrap(),
            restore_script(),
            switch_to(&target(), true).unwrap(),
            STEPS.read.clone(),
            STEPS.firmware.clone(),
            STEPS.facts.clone(),
            VOICE_SH.to_string(),
            BOOT_SH.to_string(),
        ] {
            syntax_ok(&script);
        }
        for (command, _) in deliveries() {
            syntax_ok(&command);
        }
    }

    #[test]
    fn points_through_a_copy_and_leaves_the_firmwares_file_as_it_was() {
        let pretend = Pretend::new();
        pretend.deliver();
        let run = pretend.run(&switch_to(&target(), false).unwrap(), None);
        assert_eq!(run.status, Some(0), "{}", run.stderr);
        assert!(run.stdout.contains(&format!("copy: {}", pretend.file(VOICE_COPY).display())), "{}", run.stdout);
        assert!(pretend.mounted());
        // Read through the mount: the gateway's address. Under it: the firmware's own, byte for byte.
        assert_eq!(url_of(&pretend.config()), target().url);
        assert_eq!(pretend.read(&format!("{STATION_CONFIG}.under")), config());
        assert_eq!(pretend.read(FIRMWARE_COPY), config());
        // The boot script went into /etc/init.d once, the root back read-only after it.
        assert_eq!(pretend.read(BOOT_SCRIPT), BOOT_SH);
        let calls = pretend.read("calls");
        assert!(calls.contains("mount -o remount,rw /\nsync \nmount -o remount,ro /"), "{calls}");
        assert!(calls.contains("mount -o remount,ro,bind"), "read-only, as the firmware's file: {calls}");

        // Through a reboot: mounted again at boot.
        let boot = pretend.reboot();
        assert_eq!(boot.status, Some(0), "{}", boot.stderr);
        assert!(pretend.mounted());
        assert_eq!(url_of(&pretend.config()), target().url);
    }

    #[test]
    fn a_reset_or_an_update_leaves_the_usual_alice() {
        let pretend = Pretend::new();
        pretend.deliver();
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));

        // An update rewrote the firmware's file: the copy is kept but not mounted, and why is said.
        while pretend.mounted() {
            pretend.run(&format!("umount {STATION_CONFIG}"), None);
        }
        pretend.write_config(&config().replace("\"apiKey\"", "\"newKey\" : 1,\n        \"apiKey\""));
        pretend.run(&format!("sh {BOOT_SCRIPT} start"), None);
        assert!(!pretend.mounted());
        assert_eq!(url_of(&pretend.config()), YANDEX_URL);
        assert!(pretend.read(REFUSED).starts_with("firmware-changed "), "{}", pretend.read(REFUSED));

        // A reset wiped /data: the boot script finds nothing and does nothing.
        std::fs::remove_dir_all(pretend.file("/data")).unwrap();
        let boot = pretend.run(&format!("sh {BOOT_SCRIPT} start"), None);
        assert_eq!(boot.status, Some(0), "{}", boot.stderr);
        assert!(!pretend.mounted());
        assert_eq!(url_of(&pretend.config()), YANDEX_URL);
    }

    #[test]
    fn will_not_overwrite_an_address_someone_else_put_into_our_copy_unless_told() {
        let pretend = Pretend::new();
        pretend.deliver();
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));
        // Someone rewrote our copy's file (a real mount keeps showing the old bytes until it is mounted again).
        let theirs = pretend.read(VOICE_COPY).replace(&target().url, "ws://198.51.100.7/uni.ws");
        std::fs::write(pretend.file(VOICE_COPY), &theirs).unwrap();
        let other = VoiceAddresses { url: "ws://192.0.2.11/uni.ws".into(), quic: String::new() };
        let refused = pretend.run(&switch_to(&other, false).unwrap(), None);
        assert_ne!(refused.status, Some(0));
        assert!(refused.stderr.contains("someone else changed it"), "{}", refused.stderr);
        assert_eq!(url_of(&pretend.read(VOICE_COPY)), "ws://198.51.100.7/uni.ws", "left as it was");
        // Nor is it mounted at the next boot.
        pretend.reboot();
        assert!(!pretend.mounted());
        assert!(pretend.read(REFUSED).starts_with("copy-changed "), "{}", pretend.read(REFUSED));
        // The way back is told: it replaces it.
        let forced = pretend.run(&switch_to(&yandex(), true).unwrap(), None);
        assert_eq!(forced.status, Some(0), "{}", forced.stderr);
        assert!(!pretend.mounted() && !pretend.file(VOICE_COPY).exists(), "the firmware's own addresses: no copy");
        assert_eq!(pretend.config(), config());
    }

    /// Live 08.11: after the safety net the boot mounts the way back (previous.cfg) while our copy names this station's
    /// gateway. That is ours, not someone else's change: «Попробовать снова» points the voice here again with the way
    /// back kept as it was, and «На компьютере» rewrites the copy, so the next boot does not go back to the station.
    #[test]
    fn after_the_safety_net_the_way_back_is_ours_to_point_from() {
        let pretend = Pretend::new();
        pretend.deliver();
        let local = VoiceAddresses { url: "ws://127.0.0.1:8790/uni.ws".into(), quic: String::new() };
        let to_pc = |pretend: &Pretend| pretend.run(&switch_to(&target(), false).unwrap(), None);
        let to_station = |pretend: &Pretend| pretend.run(&switch_to(&local, false).unwrap(), None);
        assert_eq!(to_pc(&pretend).status, Some(0));
        assert_eq!(to_station(&pretend).status, Some(0));
        assert_eq!(url_of(&pretend.read(PREVIOUS_COPY)), target().url, "the way back: the computer");

        // The safety net: the gateway off, a reboot — the way back is mounted, our copy still names the station.
        std::fs::write(pretend.file("/data/opt/duowave/disabled"), "").unwrap();
        assert_eq!(pretend.reboot().status, Some(0));
        assert!(pretend.mounted());
        assert_eq!(url_of(&pretend.config()), target().url);
        assert_eq!(url_of(&pretend.read(VOICE_COPY)), local.url);
        assert!(pretend.read(REFUSED).starts_with("previous "), "{}", pretend.read(REFUSED));

        // «Попробовать снова»: no refusal, and the way back stays the computer's, not the station's own copy.
        std::fs::remove_file(pretend.file("/data/opt/duowave/disabled")).unwrap();
        let again = to_station(&pretend);
        assert_eq!(again.status, Some(0), "{}", again.stderr);
        assert_eq!(url_of(&pretend.config()), local.url);
        assert_eq!(url_of(&pretend.read(PREVIOUS_COPY)), target().url, "the way back kept");

        // The safety net again, then «На компьютере»: the copy itself is the computer's now, and so is the next boot.
        std::fs::write(pretend.file("/data/opt/duowave/disabled"), "").unwrap();
        assert_eq!(pretend.reboot().status, Some(0));
        let home = to_pc(&pretend);
        assert_eq!(home.status, Some(0), "{}", home.stderr);
        assert_eq!(url_of(&pretend.read(VOICE_COPY)), target().url);
        assert!(!pretend.file(PREVIOUS_COPY).exists(), "no way back needed: the voice is not this station's");
        assert_eq!(pretend.reboot().status, Some(0));
        assert_eq!(url_of(&pretend.config()), target().url);
        assert!(!pretend.file(REFUSED).exists(), "{}", pretend.read(REFUSED));
    }

    /// Something else mounted over the config — neither our copy nor the way back — is still someone else's.
    #[test]
    fn a_mount_that_is_not_ours_is_still_someone_elses() {
        let pretend = Pretend::new();
        pretend.deliver();
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));
        let theirs = pretend.file("/tmp/theirs.cfg");
        std::fs::write(&theirs, config().replace(YANDEX_URL, "ws://198.51.100.7/uni.ws")).unwrap();
        while pretend.mounted() {
            pretend.run(&format!("umount {STATION_CONFIG}"), None);
        }
        let mount = pretend.run(&format!("mount -o bind /tmp/theirs.cfg {STATION_CONFIG}"), None);
        assert_eq!(mount.status, Some(0), "{}", mount.stderr);
        assert!(pretend.mounted());
        let other = VoiceAddresses { url: "ws://192.0.2.11/uni.ws".into(), quic: String::new() };
        let refused = pretend.run(&switch_to(&other, false).unwrap(), None);
        assert_ne!(refused.status, Some(0));
        assert!(refused.stderr.contains("ws://198.51.100.7/uni.ws"), "{}", refused.stderr);
    }

    #[test]
    fn restore_sh_brings_the_firmwares_own_config_back_without_the_gateway() {
        let pretend = Pretend::new();
        pretend.deliver();
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));
        assert_eq!(pretend.read(RESTORE_SCRIPT), restore_script(), "restore.sh is on the station");
        // (Run as its text, with the station's paths moved into the pretend one.)
        let back = pretend.run(&restore_script(), None);
        assert_eq!(back.status, Some(0), "{}", back.stderr);
        assert!(!pretend.mounted() && !pretend.file(VOICE_COPY).exists());
        assert_eq!(pretend.config(), config());
        assert!(pretend.read("calls").ends_with("reboot \n"));
        // And with voice.sh gone too: the mount and the copy by hand.
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));
        std::fs::remove_file(pretend.file(VOICE_SCRIPT)).unwrap();
        assert_eq!(pretend.run(&restore_script(), None).status, Some(0));
        assert!(!pretend.mounted());
        assert_eq!(pretend.config(), config());
    }

    #[test]
    fn connect_sh_writes_its_files_points_the_voice_and_reboots() {
        let pretend = Pretend::new();
        let result = pretend.run(&connect_script(&target()).unwrap(), None);
        assert_eq!(result.status, Some(0), "{}\n{}", result.stdout, result.stderr);
        assert_eq!(url_of(&pretend.config()), target().url);
        // (Its paths are the pretend station's here, as the whole script's are.)
        assert_eq!(pretend.read(VOICE_SCRIPT), pretend.localize(VOICE_SH));
        assert_eq!(pretend.read(BOOT_SOURCE), pretend.localize(BOOT_SH));
        assert_eq!(pretend.read(RESTORE_SCRIPT), pretend.localize(&restore_script()));
        assert!(pretend.read("calls").contains("reboot"));
    }

    #[test]
    fn changes_nothing_in_a_file_it_does_not_know() {
        let pretend = Pretend::new();
        pretend.write_config("{\"aliced\": {}}\n");
        let result = pretend.run(&connect_script(&target()).unwrap(), None);
        assert_ne!(result.status, Some(0));
        assert!(result.stderr.contains("nothing changed"), "{}", result.stderr);
        assert_eq!(pretend.config(), "{\"aliced\": {}}\n");
        assert!(!pretend.mounted());
    }

    #[test]
    fn a_copy_that_cannot_be_mounted_leaves_the_station_as_it_was() {
        let pretend = Pretend::new();
        pretend.deliver();
        std::fs::write(pretend.file("mount-fails"), "").unwrap();
        let run = pretend.run(&switch_to(&target(), false).unwrap(), None);
        assert_ne!(run.status, Some(0));
        assert!(run.stderr.contains("could not be mounted"), "{}", run.stderr);
        assert!(!pretend.file(VOICE_COPY).exists());
        assert!(!pretend.mounted());
        assert_eq!(pretend.config(), config());
    }

    #[test]
    fn refuses_an_address_that_is_not_one() {
        let bad = VoiceAddresses { url: "ws://x'; reboot; '/uni.ws".into(), quic: String::new() };
        assert_eq!(connect_script(&bad).unwrap_err().to_string(), "not an address to write");
        assert_eq!(switch_to(&bad, false).unwrap_err().to_string(), "not an address to write");
    }

    #[test]
    fn reads_the_copys_state_from_the_facts() {
        assert_eq!(
            copy_state(Some("copy"), Some("firmware-changed 1794122772 the station's quasar.cfg is not…")),
            Some(CopyState {
                copy: "copy".into(),
                refused: Some("firmware-changed".into()),
                refused_at: Some(1794122772)
            })
        );
        assert_eq!(
            copy_state(Some("mounted"), Some("-")),
            Some(CopyState { copy: "mounted".into(), refused: None, refused_at: None })
        );
        assert_eq!(copy_state(Some("what"), None), None);
        assert_eq!(copy_state(Some("none"), Some("Not A Word 1")).unwrap().refused, None);
        // A refusal at boot before the clock was set (1970): the why is kept, the time is not.
        let early = copy_state(Some("copy"), Some("previous 16 the station's own gateway is off")).unwrap();
        assert_eq!((early.refused.as_deref(), early.refused_at), (Some("previous"), None));
    }

    /// The voice addresses survive the shell: the facts' lines come out in order.
    #[test]
    fn the_facts_say_where_the_copy_stands() {
        let pretend = Pretend::new();
        // The last two lines (off a station, `df -k /data` may say nothing).
        let last = |stdout: &str| stdout.lines().rev().take(2).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(last(&pretend.run(&STEPS.facts, None).stdout), vec!["-", "none"]);
        pretend.deliver();
        assert_eq!(pretend.run(&switch_to(&target(), false).unwrap(), None).status, Some(0));
        assert_eq!(last(&pretend.run(&STEPS.facts, None).stdout), vec!["-", "mounted"]);
        // The firmware's own file, while the copy is over it.
        assert_eq!(pretend.run(&STEPS.firmware, None).stdout, config());
        assert_eq!(url_of(&pretend.run(&STEPS.read, None).stdout), target().url);
    }

    /// The call client's sound guard on its own (`sh call-client.sh guard-sound`), against a pretend speaker.
    #[test]
    fn the_sound_guard_restarts_alices_media_then_reboots_at_most_hourly() {
        let pretend = Pretend::new();
        let client = pretend.file("/data/call-client.sh");
        std::fs::write(&client, include_str!("../../../../station-client/call-client.sh")).unwrap();
        let speaker = pretend.file("/proc/pcm-status");
        let mark = pretend.file("/data/sound-guard.reboot");
        let guard = |state: &str| -> String {
            std::fs::write(&speaker, state).unwrap();
            let inherited = std::env::var("PATH").unwrap_or_default();
            let output = Command::new("sh")
                .arg(&client)
                .arg("guard-sound")
                .env("PATH", format!("{}:{inherited}", pretend.bin.display()))
                .env("PRETEND", &pretend.root)
                .env("PCM_STATUS", &speaker)
                .env("GUARD_MARK", &mark)
                .env("GUARD_STEP", "0")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        // A speaker that is well — closed, or playing — ends the watch at once.
        assert_eq!(guard("closed\n"), "");
        assert_eq!(guard("state: RUNNING\n"), "");
        assert!(!pretend.read("calls").contains("reboot"));
        // Prepared and silent: the media service, then the reboot.
        let said = guard("state: PREPARED\nowner_pid   : 1999\n");
        assert!(said.contains("Alice's media service restarts"), "{said}");
        assert!(said.contains("the station reboots"), "{said}");
        assert_eq!(pretend.read("calls").matches("reboot").count(), 1);
        assert!(mark.exists());
        // Again within the hour: no second reboot.
        let again = guard("state: PREPARED\n");
        assert!(again.contains("less than an hour ago"), "{again}");
        assert_eq!(pretend.read("calls").matches("reboot").count(), 1);
    }

    /// station-client/restore.sh is kept in the tree for the tools that put it on a station without a gateway
    /// (scripts/station-voice.sh): it must be this restore_script(). DUOWAVE_BLESS=1 writes it anew.
    #[test]
    fn the_kept_restore_sh_is_the_restore_script() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../station-client/restore.sh");
        if std::env::var_os("DUOWAVE_BLESS").is_some() {
            std::fs::write(&path, restore_script()).unwrap();
        }
        let kept = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            kept == restore_script(),
            "{} is not restore_script(); DUOWAVE_BLESS=1 cargo test -p duowave kept_restore_sh writes it",
            path.display()
        );
    }

    #[test]
    fn the_scripts_paths_are_the_stations() {
        // voice.sh and the boot script name the same places this module does.
        for (path, text) in
            [(CLIENT_DIR, VOICE_SH), (STATION_CONFIG, VOICE_SH), (CLIENT_DIR, BOOT_SH), ("/etc/init.d", VOICE_SH)]
        {
            assert!(text.contains(path), "{path}");
        }
    }
}
