//! `duowave station …`: the gateway on the station by hand, the same steps the computer's panel takes over SSH.
//!
//!   duowave station install [--port 8790] [--home-server URL --key KEY] [--peer-key KEY] [--managed pc|yam]
//!                           [--program PATH] [--in-place] [--no-start]
//!                                         the program into /data, its .env, the service; started, and once it
//!                                         answers /health, started with the station (/etc/init.d/S95duowave)
//!   duowave station point local|yandex|previous|<address[:port]> [--reboot]
//!                                         where the station's voice goes: our copy of quasar.cfg mounted over the
//!                                         firmware's own (which stays as it is), mounted again at every boot
//!   duowave station status [--json]       where the voice goes, the service, the program, space, memory, the home server
//!   duowave station remove [--keep-data]  the gateway off the station (not while the voice goes to it)
//!   duowave station retry                 start again after the safety net gave the station back
//!   duowave station safety on|off         the safety net of the service
//!   duowave station receive-settings      the settings the computer sends, from stdin, for the gateway's next start
//!   duowave station log [lines]           the end of the gateway's log: what happened

use std::io::Read as _;

use super::station_fs::{InstallOptions, RealSystem, StationFs, Target, DATA};

const USAGE: &str =
    "duowave station install|point|status|remove|retry|safety|receive-settings|log — see `duowave station help`";

pub fn command(args: &[String]) -> i32 {
    let fs = StationFs::real();
    let system = RealSystem;
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| args.iter().position(|arg| arg == name).and_then(|at| args.get(at + 1)).cloned();
    let result: Result<String, String> = match args.first().map(String::as_str) {
        Some("install") => {
            let port = match value("--port").map(|port| port.parse::<u16>()) {
                Some(Ok(port)) => Some(port),
                Some(Err(_)) => return usage("--port: a number"),
                None => None,
            };
            let home_server = match (value("--home-server"), value("--key")) {
                (Some(url), Some(key)) => Some((url, key)),
                (None, None) => None,
                _ => return usage("--home-server and --key go together"),
            };
            let options = InstallOptions {
                port,
                home_server,
                peer_key: value("--peer-key"),
                managed: value("--managed"),
                program: value("--program").map(std::path::PathBuf::from),
                in_place: flag("--in-place"),
                start: !flag("--no-start"),
                wait: None,
            };
            if !fs.is_station() {
                Err("это не колонка: нет /system/vendor/quasar/quasar.cfg".into())
            } else {
                fs.install(&options, &system).map(|installed| {
                    let moved = if installed.port_moved { " (8790 занят: взял следующий свободный)" } else { "" };
                    format!(
                        "gateway installed: {} on port {}{moved}{}\nthe panel: http://<this station's address>:{}/panel\n\
                         the station's voice to it: duowave station point local --reboot",
                        installed.program,
                        installed.port,
                        if installed.started { ", started" } else { "" },
                        installed.port
                    )
                })
            }
        }
        Some("point") => match args.get(1).and_then(|target| Target::parse(target)) {
            None => return usage("point local|yandex|previous|<address[:port]> [--reboot]"),
            Some(target) => fs.point(&target, flag("--reboot"), &system).map(|pointed| {
                if !pointed.changed {
                    format!("the voice already goes to {}: nothing changed", pointed.to.url)
                } else {
                    format!(
                        "the voice: {} → {}\n{}{}",
                        pointed.from.url,
                        pointed.to.url,
                        pointed
                            .copy
                            .map(|copy| format!("copy: {copy} (mounted over the firmware's quasar.cfg)"))
                            .unwrap_or_else(|| "the firmware's own quasar.cfg, no copy of ours".into()),
                        if pointed.rebooting {
                            "\nrebooting: the station comes back in about a minute"
                        } else {
                            "\nreboot the station for it to take effect"
                        }
                    )
                }
            }),
        },
        Some("status") => {
            let status = fs.status(&system);
            if flag("--json") {
                Ok(serde_json::to_string(&status).unwrap_or_default())
            } else {
                let mut lines = vec![format!(
                    "voice: {}{}",
                    status.voice.as_ref().map(|voice| voice.url.as_str()).unwrap_or("unknown"),
                    if status.local { " (this station's gateway)" } else { "" }
                )];
                if let Some(copy) = &status.copy {
                    lines.push(match (copy.copy.as_str(), copy.refused.as_deref()) {
                        ("mounted", _) => "copy of quasar.cfg: mounted (the firmware's own stays under it)".into(),
                        (_, Some("firmware-changed")) => {
                            "copy of quasar.cfg: not mounted — the firmware's file changed under it (an update?):                              point the voice again"
                                .into()
                        }
                        (_, Some("gateway-off")) => {
                            "copy of quasar.cfg: not mounted — this station's gateway is off".into()
                        }
                        ("copy", _) => "copy of quasar.cfg: kept, not mounted".into(),
                        _ => "copy of quasar.cfg: none (the firmware's own)".into(),
                    });
                }
                lines.push(match (status.installed, status.running) {
                    (false, _) => "gateway: not installed (duowave station install)".into(),
                    (true, true) => format!("gateway: running, port {}", status.port.unwrap_or_default()),
                    (true, false) if status.disabled => {
                        "gateway: off — the safety net gave the station back (duowave station retry)".into()
                    }
                    (true, false) => "gateway: installed, not running".into(),
                });
                if let Some(program) = &status.program {
                    lines.push(format!(
                        "program: {program}{}",
                        status.managed_by.as_deref().map(|by| format!(" (updated by {by})")).unwrap_or_default()
                    ));
                }
                if let Some(space) = status.data {
                    lines.push(format!("/data: {} MB free of {} MB", space.free_kb / 1024, space.total_kb / 1024));
                }
                if let Some(memory) = status.memory {
                    lines.push(format!(
                        "memory: {} MB available of {} MB{}",
                        memory.available_kb / 1024,
                        memory.total_kb / 1024,
                        memory.rss_kb.map(|rss| format!(", the gateway holds {} MB", rss / 1024)).unwrap_or_default()
                    ));
                }
                if let Some(url) = &status.home_server {
                    lines.push(format!("home server: {url}"));
                }
                lines.push(format!("safety net: {}", if status.safety { "on" } else { "off" }));
                if let Some(fired) = &status.fired {
                    lines.push(format!("safety net acted: {fired}"));
                }
                Ok(lines.join("\n"))
            }
        }
        Some("remove") => fs.remove(flag("--keep-data"), &system).map(|_| {
            if flag("--keep-data") {
                format!("gateway removed; its data stays in {DATA}")
            } else {
                "gateway removed, its data too".into()
            }
        }),
        Some("retry") => fs.retry(&system).map(|status| {
            format!("started again: {status}\nthe voice comes back to it at the next boot: duowave station point local --reboot")
        }),
        Some("safety") => match args.get(1).map(String::as_str) {
            Some("on") => fs.set_safety(true).map(|_| "safety net: on".into()),
            Some("off") => fs.set_safety(false).map(|_| "safety net: off".into()),
            _ => return usage("safety on|off"),
        },
        Some("receive-settings") => {
            let mut bundle = Vec::new();
            match std::io::stdin().read_to_end(&mut bundle) {
                Ok(_) => fs
                    .receive_settings(&bundle)
                    .map(|_| "settings received: the gateway reads them at its next start".into()),
                Err(error) => Err(error.to_string()),
            }
        }
        Some("log") => {
            let lines: usize = args.get(1).and_then(|lines| lines.parse().ok()).unwrap_or(40).min(400);
            let log = std::fs::read_to_string(fs.path(&format!("{DATA}/gateway.log"))).unwrap_or_default();
            let all: Vec<&str> = log.lines().collect();
            Ok(all[all.len().saturating_sub(lines)..].join("\n"))
        }
        Some("help") | Some("--help") | None => {
            println!(
                "{}",
                include_str!("cli.rs")
                    .lines()
                    .skip(1)
                    .take_while(|line| line.starts_with("//!"))
                    .map(|line| line.trim_start_matches("//!").trim_start_matches(' '))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            return 0;
        }
        Some(other) => return usage(&format!("unknown: {other}")),
    };
    match result {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            0
        }
        Err(problem) => {
            eprintln!("duowave station: {problem}");
            1
        }
    }
}

fn usage(problem: &str) -> i32 {
    eprintln!("duowave station: {problem}\n{USAGE}");
    2
}
