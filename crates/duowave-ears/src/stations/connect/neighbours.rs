//! What else on the station would stand in the way of DuoWave — read on the first look, never changed. Other
//! community mods that take the same place (their own assistant on Alice's microphone, their own server instead of
//! Yandex's, a stack instead of Alice's) and the traces such a mod leaves even when we do not know it by name: Alice
//! not running, a file of hers replaced by a mount, a library put into her process.
//!
//! The gateway only names them; the panel says what each one does, why it is in the way and how its own author
//! turns it off. Nothing is removed for the admin: «Подключить» stays theirs to press.

use serde::Serialize;

use super::quasar_config::STATION_CONFIG;

/// Mods we know by their folder: an id the panel has words for, and where it lives.
const KNOWN: &[(&str, &str)] = &[
    // YaJavis (newfpv/YaJavis): its own «Джарвис» next to Alice, on her microphone, in Go — 48–128 MB of memory.
    ("yajavis", "/data/opt/jarvisd"),
    // The earlier «server» Jarvis that YaJavis replaced.
    ("jarvis-server", "/data/opt/jarvis"),
    // Alisa ChatGPT Voice Assistant: its own voice mode over Alice (the standalone mode and the newer native one).
    ("alisa-voice", "/data/opt/alisa-standalone"),
    ("alisa-voice", "/data/opt/alice-voice"),
    // Its CLI: ChatGPT Voice plus a sing-box VPN.
    ("alisa-cli", "/data/opt/alisa-cli"),
    // Pulsar: Home Assistant's satellite instead of Yandex's stack — Alice's maind off.
    ("pulsar", "/data/pulsar"),
    // Quasar Bridge: its own LLM instead of Alice, writing the firmware's quasar.cfg itself.
    ("quasar-bridge", "/data/opt/quasar-bridge"),
    // Alice LED: its own picture on the ring.
    ("alice-led", "/data/opt/alice-led"),
    // luna (nyakokitsu/luna, MIT): a runtime for Lua mods — the ring, sounds, buttons, Alice's recognized text — with
    // its API on port 8099. Through YAM (its package yam/luna), or its own installer: /data/quazar/luna and S99luna.
    // It leaves Alice's voice where it is, but its mods may cut her answer short (`cancel=true`), ours too.
    ("luna", "/data/yam/pkgs/yam/luna"),
    ("luna", "/data/quazar/luna"),
    ("luna", "/etc/init.d/S99luna"),
    // AliceBridge's agent (root from an Android phone; its AGENT.md): brings the home Wi-Fi back after a boot,
    // watches ADB and SSH, puts its OTA block back after each boot — about 8 MB of memory.
    ("alicebridge", ALICEBRIDGE),
];

/// AliceBridge's agent's folder: `agent`, `config.json` (the home network, the Wi-Fi change under way), `agent.log`,
/// `control.sock`, `release.json`; started by /etc/init.d/S99zzalicebridge-agent.
const ALICEBRIDGE: &str = "/data/quasar/alicebridge-agent";

/// The root password AliceBridge's SSH starts with (its install-ssh.sh; the wiki's start/root-android.md asks to
/// change it at once). Public, so anyone at home may sign in with it.
const ALICEBRIDGE_PASSWORD: &str = "1";

/// Read first among the warnings: the station keeps falling off the network.
const URGENT: &[&str] = &["alicebridge-wifi"];

/// The mods that have their author's own switch, and what it is. Off, not removed: their settings, keys and
/// records stay, and the panel says how to turn each back on. A station reboot is not needed before «Подключить» —
/// connecting reboots it anyway.
const SWITCHES: &[(&str, &str, &str)] = &[
    // Its ctl.sh: Yandex's stack back now and after reboots (enable turns Pulsar back on).
    ("pulsar", "/data/pulsar", "sh /data/pulsar/ctl.sh disable"),
    // Its documented rollback: the firmware's quasar.cfg back to Yandex from its own backup, then the bridge stopped.
    (
        "quasar-bridge",
        "/data/opt/quasar-bridge",
        "sh /data/opt/quasar-bridge/app/control.sh rollback && { sh /data/opt/quasar-bridge/app/control.sh stop; true; }",
    ),
    // jarvisd.sh's own mark: with `disabled` it neither starts nor hooks into Alice at boot.
    (
        "yajavis",
        "/data/opt/jarvisd",
        "touch /data/opt/jarvisd/disabled && { sh /data/opt/jarvisd/jarvisd.sh stop >/dev/null 2>&1; true; }",
    ),
    // Its `enabled` mark, as YaJavis's installer turns it off.
    (
        "jarvis-server",
        "/data/opt/jarvis",
        "rm -f /data/opt/jarvis/enabled && { sh /data/opt/jarvis/jarvis.sh stop >/dev/null 2>&1; true; }",
    ),
];

/// The command that turns a known mod off by its own switch — only for those in SWITCHES, and only while its folder
/// is there (exit 3 otherwise: nothing to turn off).
pub fn turn_off_command(id: &str) -> Option<String> {
    SWITCHES.iter().find(|(known, _, _)| *known == id).map(|(_, folder, command)| {
        format!("[ -e {folder} ] || {{ echo 'not on this station' >&2; exit 3; }}; {command}")
    })
}

/// Mods that bring their own assistant onto Alice's microphone: the mount and the library they leave are theirs,
/// not another unknown one.
const MIC_MODS: &[&str] = &["yajavis", "jarvis-server", "alisa-voice", "alisa-cli"];

/// Below this much free memory the station gateway and Alice have no room together, kilobytes.
const LOW_MEMORY_KB: u64 = 30 * 1024;

/// Reading only, a line each: `mod <id>` for every known mod's folder there, `mount <point> <root>` for every
/// mount over a file of Alice's (from /proc/self/mountinfo: the root says whose file it is), `maind on|off`,
/// `preload <libraries>` from each maind's environment, `memavail <kB>`; for AliceBridge's agent, `alicebridge
/// wifi-stuck` and `alicebridge dns <file>`.
pub fn script() -> String {
    let mods: Vec<String> = KNOWN.iter().map(|(id, path)| format!("{id}:{path}")).collect();
    [
        format!("for m in {}; do [ -e \"${{m#*:}}\" ] && echo \"mod ${{m%%:*}}\"; done", mods.join(" ")),
        // Turned off by its own switch (turn_off_command), still installed: it no longer stands in the way.
        "[ -d /data/pulsar ] && [ ! -f /data/pulsar/enabled ] && echo off pulsar".into(),
        "[ -f /data/opt/jarvisd/disabled ] && echo off yajavis".into(),
        "[ -d /data/opt/jarvis ] && [ ! -f /data/opt/jarvis/enabled ] && echo off jarvis-server".into(),
        // Quasar Bridge sends the voice to itself at 127.0.0.1:8443; after its rollback the file no longer does.
        format!(
            "[ -d /data/opt/quasar-bridge ] && ! grep -qs '127.0.0.1:8443' {STATION_CONFIG} && echo off quasar-bridge"
        ),
        // AliceBridge's agent stuck on a Wi-Fi change (the wiki's internals/station1-wifi-rollback.md): its change in
        // the rollback phase with the confirmation timed out — the station drops off the network every 40–50 s. Only
        // the mark leaves the station: its config.json also holds the home network and its password.
        format!(
            "A={ALICEBRIDGE}/config.json; grep -qs '\"phase\" *: *\"rolling_back\"' $A && \
             grep -qs 'Wi-Fi confirmation timed out' $A && echo alicebridge wifi-stuck"
        ),
        // Its DNS: resolv.conf a link to its own file, not what the router gives.
        "L=$(readlink /etc/resolv.conf 2>/dev/null); case \"$L\" in *alicebridge-resolv*) echo alicebridge dns \"$L\";; esac"
            .into(),
        "awk '$5 ~ \"^/system/vendor/quasar/\" {print \"mount\", $5, $4}' /proc/self/mountinfo 2>/dev/null".into(),
        "P=$(pidof maind 2>/dev/null); if [ -n \"$P\" ]; then echo maind on; else echo maind off; fi".into(),
        "for p in $P; do tr '\\0' '\\n' < /proc/$p/environ 2>/dev/null | sed -n 's/^LD_PRELOAD=/preload /p'; done"
            .into(),
        "awk '/^MemAvailable:/ {print \"memavail\", $2}' /proc/meminfo 2>/dev/null".into(),
        "true".into(),
    ]
    .join("; ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// DuoWave will not work beside it: the voice has nowhere to go, or two take the same place.
    Blocks,
    /// It works, but the two may talk over each other, fight for the ring or run short of memory.
    Warns,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Neighbour {
    /// A known mod's id, or: `maind-off`, `config-mount` (someone else's file over quasar.cfg), `alice-mount` (over
    /// another file of Alice's), `alice-preload` (a library in her process), `low-memory`, `alicebridge-wifi`
    /// (AliceBridge's agent stuck on a Wi-Fi change), `alicebridge-dns` (its own resolv.conf), `default-password`
    /// (root signs in with AliceBridge's «1»).
    pub id: String,
    pub severity: Severity,
    /// The file, the library or the free megabytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Turned off by its own switch, still installed: it no longer stands in the way.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
    /// The mod's own switch, while it is on: the command «Выключить» runs (turn_off_command, without its guard).
    #[serde(rename = "switch", skip_serializing_if = "Option::is_none")]
    pub switch: Option<&'static str>,
}

/// What the script printed, as the panel shows it: the worst first, each once.
pub fn read(stdout: &str) -> (Vec<Neighbour>, Option<u64>) {
    let mut found: Vec<Neighbour> = Vec::new();
    let mut memory = None;
    let mut mounts = Vec::new();
    let mut preloads = Vec::new();
    let mut maind_off = false;
    let mut turned_off: Vec<String> = Vec::new();
    let add = |found: &mut Vec<Neighbour>, id: &str, severity: Severity, detail: Option<String>| {
        if !found.iter().any(|n| n.id == id && n.detail == detail) {
            found.push(Neighbour { id: id.to_string(), severity, detail, off: false, switch: None });
        }
    };
    for line in stdout.lines() {
        let mut words = line.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("mod"), Some(id), None) => {
                if let Some((known, _)) = KNOWN.iter().find(|(known, _)| *known == id) {
                    let severity = match *known {
                        "pulsar" | "quasar-bridge" => Severity::Blocks,
                        _ => Severity::Warns,
                    };
                    add(&mut found, known, severity, None);
                }
            }
            (Some("mount"), Some(point), Some(root)) if safe(point) && safe(root) => {
                // Ours: voice.sh's copy of quasar.cfg in /data/opt/duowave.
                if !root.contains("/duowave/") {
                    mounts.push(point.to_string());
                }
            }
            (Some("maind"), Some("off"), None) => maind_off = true,
            (Some("off"), Some(id), None) => turned_off.push(id.to_string()),
            (Some("preload"), Some(_), _) => {
                let libraries = line["preload".len()..].trim();
                for library in libraries.split([' ', ':']).filter(|l| !l.is_empty() && safe(l)) {
                    preloads.push(library.to_string());
                }
            }
            (Some("memavail"), Some(kb), None) => memory = kb.parse::<u64>().ok(),
            (Some("alicebridge"), Some("wifi-stuck"), None) => {
                add(&mut found, "alicebridge-wifi", Severity::Warns, None);
            }
            (Some("alicebridge"), Some("dns"), Some(file)) if safe(file) => {
                add(&mut found, "alicebridge-dns", Severity::Warns, Some(file.to_string()));
            }
            _ => {}
        }
    }
    // Pulsar stops maind itself: its name says why Alice is not running.
    if maind_off && !found.iter().any(|n| n.id == "pulsar") {
        add(&mut found, "maind-off", Severity::Blocks, None);
    }
    let mic_mod = found.iter().any(|n| MIC_MODS.contains(&n.id.as_str()));
    for point in mounts {
        if point.ends_with("/quasar.cfg") {
            add(&mut found, "config-mount", Severity::Blocks, Some(point));
        } else if !mic_mod {
            add(&mut found, "alice-mount", Severity::Warns, Some(point));
        }
    }
    if !mic_mod {
        for library in preloads {
            add(&mut found, "alice-preload", Severity::Warns, Some(library));
        }
    }
    if let Some(kb) = memory.filter(|kb| *kb < LOW_MEMORY_KB) {
        add(&mut found, "low-memory", Severity::Warns, Some((kb / 1024).to_string()));
    }
    for neighbour in &mut found {
        if turned_off.contains(&neighbour.id) {
            neighbour.off = true;
            neighbour.severity = Severity::Warns;
        }
        neighbour.switch = if neighbour.off {
            None
        } else {
            SWITCHES.iter().find(|(id, _, _)| *id == neighbour.id).map(|(_, _, command)| *command)
        };
    }
    order(&mut found);
    (found, memory)
}

/// The worst first, then the station falling off the network; what is turned off last.
fn order(found: &mut [Neighbour]) {
    found.sort_by_key(|n| (n.severity != Severity::Blocks, !URGENT.contains(&n.id.as_str()), n.off));
}

/// The wizard signed in with AliceBridge's default root password: say so, so it is changed. Only compared here —
/// the password itself goes nowhere.
pub fn note_password(found: &mut Vec<Neighbour>, password: &str) {
    if password == ALICEBRIDGE_PASSWORD && !found.iter().any(|n| n.id == "default-password") {
        found.push(Neighbour {
            id: "default-password".into(),
            severity: Severity::Warns,
            detail: None,
            off: false,
            switch: None,
        });
        order(found);
    }
}

/// A path or a number as it may go to the panel: no spaces, no quotes, not too long.
fn safe(value: &str) -> bool {
    (1..=160).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(found: &[Neighbour]) -> Vec<&str> {
        found.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn a_clean_station_with_our_copy_mounted_has_nothing_in_the_way() {
        let (found, memory) =
            read("mount /system/vendor/quasar/quasar.cfg /opt/duowave/voice/quasar.cfg\nmaind on\nmemavail 61234\n");
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(memory, Some(61234));
    }

    #[test]
    fn yajavis_is_named_once_and_its_mount_and_library_are_not_counted_again() {
        let (found, _) = read(
            "mod yajavis\nmount /system/vendor/quasar/launch.json /jarvisd/launch.json\nmaind on\n\
             preload /data/opt/jarvisd/vqe_tap.so\nmaind on\npreload /data/opt/jarvisd/vqe_tap.so\nmemavail 80000\n",
        );
        assert_eq!(ids(&found), ["yajavis"]);
        assert_eq!(found[0].severity, Severity::Warns);
    }

    #[test]
    fn a_stack_instead_of_alice_blocks_and_comes_first() {
        let (found, _) = read("mod alice-led\nmod pulsar\nmaind off\nmemavail 90000\n");
        assert_eq!(ids(&found), ["pulsar", "alice-led"]);
        assert_eq!(found[0].severity, Severity::Blocks);
        assert_eq!(found[1].severity, Severity::Warns);
        let (found, _) = read("mod alice-led\nmaind off\n");
        assert_eq!(ids(&found), ["maind-off", "alice-led"]);
        assert_eq!(found[0].severity, Severity::Blocks);
    }

    #[test]
    fn traces_of_an_unknown_mod_are_named_with_their_file() {
        let (found, _) = read(
            "mount /system/vendor/quasar/quasar.cfg /opt/other/quasar.cfg\n\
             mount /system/vendor/quasar/launch.json /tmp/launch.json\nmaind on\n\
             preload /data/x/tap.so:/data/y/hook.so\nmemavail 20480\n",
        );
        assert_eq!(
            found[0],
            Neighbour {
                id: "config-mount".into(),
                severity: Severity::Blocks,
                detail: Some("/system/vendor/quasar/quasar.cfg".into()),
                off: false,
                switch: None,
            }
        );
        assert_eq!(ids(&found), ["config-mount", "alice-mount", "alice-preload", "alice-preload", "low-memory"]);
        assert_eq!(found.last().unwrap().detail.as_deref(), Some("20"));
    }

    #[test]
    fn mods_with_their_own_switch_may_be_turned_off_and_once_off_stand_aside() {
        let (found, _) = read("mod pulsar\nmod yajavis\nmod alisa-voice\nmaind off\n");
        let offered: Vec<(&str, bool)> = found.iter().map(|n| (n.id.as_str(), n.switch.is_some())).collect();
        assert_eq!(offered, [("pulsar", true), ("yajavis", true), ("alisa-voice", false)]);

        let (found, _) = read("mod pulsar\nmod yajavis\noff pulsar\nmaind on\n");
        assert_eq!(ids(&found), ["yajavis", "pulsar"], "what is off comes last");
        let pulsar = &found[1];
        assert!(pulsar.off && pulsar.switch.is_none());
        assert_eq!(pulsar.severity, Severity::Warns, "off, it no longer blocks");
        let json = serde_json::to_value(pulsar).unwrap();
        assert_eq!(json["off"], true);
        assert!(json.get("switch").is_none());
        let yajavis = serde_json::to_value(&found[0]).unwrap();
        assert!(turn_off_command("yajavis").unwrap().ends_with(yajavis["switch"].as_str().unwrap()));
    }

    #[test]
    fn only_known_switches_turn_anything_off_and_only_where_the_mod_is() {
        for id in ["pulsar", "quasar-bridge", "yajavis", "jarvis-server"] {
            let command = turn_off_command(id).unwrap();
            assert!(command.starts_with("[ -e /data/"), "{command}");
            assert!(!command.contains("rm -rf") && !command.contains("reboot"), "off, not removed: {command}");
        }
        for id in [
            "alisa-voice",
            "alisa-cli",
            "alice-led",
            "maind-off",
            "config-mount",
            "alice-preload",
            "luna",
            "alicebridge",
            "alicebridge-wifi",
            "default-password",
            "x; rm -rf /",
        ] {
            assert_eq!(turn_off_command(id), None, "{id}");
        }
    }

    /// Each switch under the real sh, against a pretend station in a folder: the mod's own scripts are fakes that
    /// log their call. Where the mod is not, nothing runs. Unix only: these commands run on the station under busybox
    /// sh and nowhere else — Windows has no sh to run them with.
    #[cfg(unix)]
    #[test]
    fn the_switches_run_under_sh() {
        let root = tempfile::tempdir().unwrap();
        let at = |path: &str| format!("{}{path}", root.path().display());
        let pretend = |command: String| command.replace(" /data/", &format!(" {}/data/", root.path().display()));
        let gone = std::process::Command::new("sh")
            .arg("-c")
            .arg(pretend(turn_off_command("yajavis").unwrap()))
            .output()
            .unwrap();
        assert_eq!(gone.status.code(), Some(3), "no YaJavis here");

        std::fs::create_dir_all(at("/data/opt/jarvisd")).unwrap();
        std::fs::write(at("/data/opt/jarvisd/jarvisd.sh"), "echo stopped >> \"$(dirname \"$0\")/calls\"\n").unwrap();
        let run = std::process::Command::new("sh")
            .arg("-c")
            .arg(pretend(turn_off_command("yajavis").unwrap()))
            .output()
            .unwrap();
        assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
        assert!(std::path::Path::new(&at("/data/opt/jarvisd/disabled")).exists());
        assert_eq!(std::fs::read_to_string(at("/data/opt/jarvisd/calls")).unwrap(), "stopped\n");
    }

    #[test]
    fn luna_is_named_once_as_a_soft_warning_with_no_switch() {
        // YAM's package and its own installer's boot script: one luna.
        let (found, _) = read("mod luna\nmod luna\nmaind on\nmemavail 80000\n");
        assert_eq!(ids(&found), ["luna"]);
        assert_eq!(found[0].severity, Severity::Warns, "it leaves Alice's voice where it is");
        assert_eq!(found[0].switch, None);
        assert_eq!(turn_off_command("luna"), None);
    }

    #[test]
    fn alicebridge_s_agent_is_a_warning_and_a_stuck_wifi_change_comes_first() {
        let (found, _) = read(
            "mod alice-led\nmod alicebridge\nalicebridge wifi-stuck\n\
             alicebridge dns /data/opt/etc/alicebridge-resolv.conf\nmaind on\nmemavail 70000\n",
        );
        assert_eq!(ids(&found), ["alicebridge-wifi", "alice-led", "alicebridge", "alicebridge-dns"]);
        assert!(found.iter().all(|n| n.severity == Severity::Warns && n.switch.is_none()), "{found:?}");
        assert_eq!(found[3].detail.as_deref(), Some("/data/opt/etc/alicebridge-resolv.conf"));
        // Not turned off by DuoWave: without it the home Wi-Fi, ADB/SSH and the OTA block are not put back.
        assert_eq!(turn_off_command("alicebridge"), None);
        // A blocking one still goes before it.
        let (found, _) = read("mod alicebridge\nalicebridge wifi-stuck\nmaind off\n");
        assert_eq!(ids(&found), ["maind-off", "alicebridge-wifi", "alicebridge"]);
        // Only a plain path is passed on.
        let (found, _) = read("mod alicebridge\nalicebridge dns /x/alicebridge-resolv.conf;reboot\n");
        assert_eq!(ids(&found), ["alicebridge"]);
    }

    #[test]
    fn alicebridge_s_default_root_password_is_named_but_never_shown() {
        let (mut found, _) = read("mod pulsar\nmod alicebridge\nmaind off\n");
        note_password(&mut found, "1");
        note_password(&mut found, "1");
        assert_eq!(ids(&found), ["pulsar", "alicebridge", "default-password"]);
        let password = &found[2];
        assert_eq!((password.severity, password.detail.as_deref(), password.switch), (Severity::Warns, None, None));
        for other in ["", "11", "s3cret-root"] {
            let (mut found, _) = read("mod alicebridge\n");
            note_password(&mut found, other);
            assert_eq!(ids(&found), ["alicebridge"], "{other:?}");
        }
    }

    /// The agent's part of the script under the real sh, against a pretend agent folder: a stuck change gives one
    /// mark, and nothing of config.json — its network, its password — leaves the station. Unix only, as above.
    #[cfg(unix)]
    #[test]
    fn a_stuck_alicebridge_wifi_change_is_read_without_its_network_or_password() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("alicebridge-agent");
        std::fs::create_dir_all(&folder).unwrap();
        let look = || {
            let script = script().replace(ALICEBRIDGE, &folder.display().to_string());
            let output = std::process::Command::new("sh").arg("-c").arg(script).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };
        let stuck = [
            // As the agent writes it, and pretty-printed.
            r#"{"home":{"ssid_hex":"486f6d65","psk":"pretend-psk"},"home_configured":false,"transaction":{"id":"ab","phase":"rolling_back","message":"Wi-Fi confirmation timed out"}}"#,
            "{\n  \"home_configured\": false,\n  \"transaction\": {\n    \"phase\": \"rolling_back\",\n    \"message\": \"Wi-Fi confirmation timed out\",\n    \"psk\": \"pretend-psk\"\n  }\n}\n",
        ];
        for config in stuck {
            std::fs::write(folder.join("config.json"), config).unwrap();
            let stdout = look();
            assert!(stdout.lines().any(|line| line == "mod alicebridge"), "{stdout}");
            assert!(stdout.lines().any(|line| line == "alicebridge wifi-stuck"), "{stdout}");
            assert!(!stdout.contains("pretend-psk") && !stdout.contains("486f6d65"), "{stdout}");
            // First after what blocks (here: no maind on this computer).
            let (found, _) = read(&stdout);
            let warns: Vec<&str> =
                found.iter().filter(|n| n.severity == Severity::Warns).map(|n| n.id.as_str()).collect();
            assert_eq!(warns.first(), Some(&"alicebridge-wifi"), "{found:?}");
        }
        // A change that went through, or one still waiting for its confirmation: nothing to say.
        for config in [
            r#"{"home_configured":true,"transaction":{"phase":"committed","message":""}}"#,
            r#"{"home_configured":true,"transaction":{"phase":"rolling_back","message":"rolling back"}}"#,
        ] {
            std::fs::write(folder.join("config.json"), config).unwrap();
            assert!(!look().contains("wifi-stuck"), "{config}");
        }
    }

    #[test]
    fn odd_lines_and_unknown_ids_are_ignored() {
        let (found, memory) =
            read("mod nothing-we-know\nmount /system/vendor/quasar/a b'c\npreload \"x y\"\ngarbage\nmemavail x\n");
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(memory, None);
    }

    #[test]
    fn the_script_names_every_known_folder_and_only_reads() {
        let script = script();
        for (id, path) in KNOWN {
            assert!(script.contains(&format!("{id}:{path}")), "{id}");
        }
        for word in ["rm ", "mount -", "kill", "reboot", "touch ", "mv ", "cp ", "sed -i"] {
            assert!(!script.contains(word), "the look must only read: {word}");
        }
        // Its only writes are errors thrown away.
        assert!(!script.replace("2>/dev/null", "").contains('>'), "the look must not write a file");
    }

    /// The script under the real sh (busybox's on the station): it runs and prints only the lines read() knows. Unix
    /// only: the script runs on the station alone; Windows has no sh.
    #[cfg(unix)]
    #[test]
    fn the_script_runs_under_sh() {
        let output = std::process::Command::new("sh").arg("-c").arg(script()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.lines().any(|line| line.starts_with("maind ")), "{stdout}");
    }
}
