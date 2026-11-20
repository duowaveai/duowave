//! `duowave station setup [address] [--yes]`: DuoWave onto the station from the computer's terminal, in one go — what
//! the one-line install runs (scripts/install/install.sh). No app and no panel on the computer: it finds the station
//! in the home network, looks at it over SSH (reading only), says what would stand in the way, asks before anything
//! changes, then puts the station's build that this program carries (signed, checked) onto the station, starts it,
//! points the station's voice at it and waits for it to come back after the reboot. The computer is not needed after.
//!
//! The same steps as the panel's move to the station (moves.rs), without the computer's own gateway: no settings to
//! take along, no home server — the station's gateway starts fresh and its own panel asks for a password.
//! Nothing changes on the station before the person's «да», and other mods are only turned off by their own switch,
//! each after its own «да».

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::Value;

use super::moves::{NEEDED_KB, NEEDED_MEMORY_KB};
use super::payload;
use super::station_fs::{BIN, DATA, DEFAULT_PORT, DIR, SERVICE_COMMAND};
use crate::config::Config;
use crate::stations::connect::discovery::{discover_stations, NoBanner, Unreached};
use crate::stations::connect::neighbours::{self, Neighbour, Severity};
use crate::stations::connect::quasar_config::{read_config, PointsTo};
use crate::stations::connect::station_connect::problem_of;
use crate::stations::connect::station_scripts::{deliveries, RESTORE_SCRIPT, STEPS, UNINSTALL_SCRIPT};
use crate::stations::connect::station_shell::{RusshOpener, ShellOpener, ShellOptions, StationShell};

/// Where root and SSH come from, for a station that has neither.
const WIKI: &str = "https://hackalice.me/";

pub fn command(args: &[String]) -> i32 {
    // The words in the terminal's language, as the system names it: the first of LC_ALL, LC_MESSAGES, LANG that is
    // set. «C»/«POSIX» are English; none set — Russian, the texts' own.
    let named = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()));
    crate::locale::text::remember_household(named.as_deref().and_then(terminal_language));
    // One process-wide TLS provider, as `serve` and the other commands install it: the checks of the station's
    // gateway and a download of its build go through reqwest, which has none of its own (live, 16.11: it panicked
    // at the first /health, after the start and before the voice was pointed).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    match runtime.block_on(run(args)) {
        Ok(()) => 0,
        Err(Stop(said)) => {
            if !said.is_empty() {
                eprintln!("\n{said}");
            }
            1
        }
    }
}

/// A locale as the system names it (`ru_RU.UTF-8`, `en`, `C`) → the language of the words.
fn terminal_language(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next().unwrap_or("");
    if base == "C" || base == "POSIX" {
        return Some("en".into());
    }
    crate::locale::normalize(base)
}

/// Why setup stopped, in the person's words; empty — they said no, nothing more to say.
struct Stop(String);

impl<E: std::fmt::Display> From<E> for Stop {
    fn from(error: E) -> Stop {
        Stop(error.to_string())
    }
}

async fn run(args: &[String]) -> Result<(), Stop> {
    let yes = args.iter().any(|arg| arg == "--yes" || arg == "-y");
    let given = args.iter().find(|arg| !arg.starts_with('-')).cloned();
    let config = Config::from_process().map_err(|error| Stop(error.problems.join("; ")))?;

    println!("{}", crate::tr!("DuoWave на колонку — шаг за шагом. До вашего «да» на колонке ничего не меняется."));
    let address = match given {
        Some(address) => address,
        None => find().await?,
    };

    let ssh = &config.gateway.station_ssh;
    let opener = RusshOpener;
    if let Err(missed) = opener.banner(&address, ssh.port).await {
        // Why: the connect's own answer (test 19.11, remark 18) — on macOS a program kept off the home network looks
        // like a speaker without SSH, and the way out is not the wiki but a switch in the system settings.
        tracing::warn!("setup: {address}:{} gave no SSH banner: {missed}", ssh.port);
        let why = match missed {
            NoBanner::Unreached(Unreached::NoRoute, _) if cfg!(target_os = "macos") => crate::tr!(
                "Колонка не отвечает: она выключена, в другой сети — или macOS не дала этой программе доступ к домашней сети. Проверьте: «Системные настройки» → «Конфиденциальность и безопасность» → «Локальная сеть» — включите Терминал (или приложение, из которого запускали), либо запустите команду в приложении «Терминал»."
            ),
            NoBanner::Unreached(Unreached::NoRoute, _) => crate::tr!("До колонки нет пути из этой сети: она включена и в той же сети, что этот компьютер?"),
            NoBanner::Unreached(Unreached::Timeout, _) => crate::tr!("Колонка не ответила за 3 секунды: она включена и в той же сети?"),
            NoBanner::Unreached(Unreached::Refused, _) => crate::tr!("Колонка на связи, но SSH на ней не запущен."),
            NoBanner::Unreached(Unreached::Other, said) => said,
            NoBanner::Silent => crate::tr!("Порт SSH на колонке открыт, но SSH не ответил за 5 секунд: колонка занята. Подождите минуту и попробуйте снова; не помогает — перезагрузите её."),
            NoBanner::NotSsh(_) => crate::tr!("Порт SSH на колонке открыт, но связь сразу закрылась. Подождите пару минут и попробуйте снова; не помогает — перезагрузите колонку."),
        };
        let head = crate::tr!(
            "На {address} не отвечает SSH (порт {port}). Нужна колонка с root и SSH — как их получить: {wiki}",
            address = address,
            port = ssh.port,
            wiki = WIKI
        );
        return Err(Stop(if why.is_empty() { head } else { format!("{why}\n{head}") }));
    }
    println!("{}", crate::tr!("Вхожу на колонку {address} по SSH…", address = address));
    let mut options = ShellOptions {
        port: ssh.port,
        user: ssh.user.clone(),
        password: ssh.password.clone(),
        trust: None,
        timeout_ms: None,
    };
    let shell =
        open_asking(&opener, &address, &mut options, &mut |question| rpassword::prompt_password(question).ok()).await?;
    let result = setup(&config, &address, shell.as_ref(), yes).await;
    shell.close().await;
    let port = result?;
    if let Some(port) = port {
        if arrive(&address, port).await {
            setup_code(&address, port, &opener, options).await;
        }
    }
    Ok(())
}

/// The station in the home network: mDNS first; none — ask for its address; several — ask which.
async fn find() -> Result<String, Stop> {
    println!("{}", crate::tr!("Ищу колонку в домашней сети…"));
    let found = discover_stations(3000).await;
    match found.len() {
        0 => {
            println!(
                "{}",
                crate::tr!(
                    "Колонка не нашлась сама. Её адрес есть в приложении «Дом с Алисой» или в списке устройств роутера."
                )
            );
            let address = ask(&crate::tr!("Адрес колонки (например, 192.168.1.20): "))?;
            if address.is_empty() {
                return Err(Stop(String::new()));
            }
            Ok(address)
        }
        1 => {
            let station = &found[0];
            println!("{}", crate::tr!("Нашёл: {station}", station = label(station)));
            Ok(station.address.clone())
        }
        _ => {
            for (at, station) in found.iter().enumerate() {
                println!("  {}. {}", at + 1, label(station));
            }
            let choice = ask(&crate::tr!("Какую ставить? Номер: "))?;
            choice
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| found.get(n.wrapping_sub(1)))
                .map(|station| station.address.clone())
                .ok_or_else(|| Stop(crate::tr!("Нет такого номера.")))
        }
    }
}

fn label(station: &crate::stations::connect::discovery::FoundStation) -> String {
    let model = station.txt.get("platform").or(station.name.as_ref()).cloned().unwrap_or_default();
    if model.is_empty() {
        station.address.clone()
    } else {
        format!("{} ({model})", station.address)
    }
}

/// The look, the questions and the install. The gateway's port on the station once it answers, or none when the
/// person said no.
async fn setup(config: &Config, address: &str, shell: &dyn StationShell, yes: bool) -> Result<Option<u16>, Stop> {
    // The look: reading only.
    let text = run_ok(shell, &STEPS.read)
        .await
        .map_err(|_| Stop(crate::tr!("На колонке нет quasar.cfg: это точно Яндекс Станция?")))?;
    let facts = read_config(&text)?;
    let lines: Vec<String> = run_ok(shell, &STEPS.facts).await?.lines().map(str::to_string).collect();
    let free_kb = lines.first().and_then(|line| line.split_whitespace().nth(3)).and_then(|kb| kb.parse::<u64>().ok());
    let machine = lines.get(1).cloned().unwrap_or_default();
    let updates_on = lines.get(3).map(String::as_str) == Some("updates-on");
    let around = run_ok(shell, &STEPS.neighbours).await.unwrap_or_default();
    let (mut found, memory) = neighbours::read(&around);
    let installed = run_ok(shell, &format!("[ -x {BIN} ] && {BIN} version || true")).await.unwrap_or_default();

    println!();
    let model = facts.device_type.clone().unwrap_or_else(|| crate::tr!("Яндекс Станция"));
    let firmware = facts.software_version.clone().unwrap_or_default();
    println!("{}", crate::tr!("Колонка: {model}, прошивка {firmware}", model = model, firmware = firmware));
    println!(
        "{}",
        match facts.points_to {
            PointsTo::Yandex => crate::tr!("Голос сейчас идёт к Яндексу."),
            PointsTo::Gateway => crate::tr!("Голос сейчас идёт на {url}", url = facts.addresses.url.clone()),
            PointsTo::Other => crate::tr!("Голос сейчас идёт не к Яндексу: {url}", url = facts.addresses.url.clone()),
        }
    );
    if !installed.trim().is_empty() {
        println!(
            "{}",
            crate::tr!("DuoWave уже стоит ({version}): поставлю поверх, данные останутся.", version = installed.trim())
        );
    }

    // What makes it impossible: not the station's processor, no room, no memory.
    if machine.trim() != "aarch64" {
        return Err(Stop(crate::tr!("Колонка — {machine}, а сборка для колонки — aarch64.", machine = machine.trim())));
    }
    if let Some(free) = free_kb.filter(|free| *free < NEEDED_KB) {
        return Err(Stop(crate::tr!(
            "В /data свободно {free} МБ, а DuoWave нужно {needed} МБ.",
            free = free / 1024,
            needed = NEEDED_KB / 1024
        )));
    }
    if let Some(free) = memory.filter(|free| *free < NEEDED_MEMORY_KB) {
        return Err(Stop(crate::tr!(
            "У колонки свободно {free} МБ ОЗУ, а DuoWave нужно {needed} МБ.",
            free = free / 1024,
            needed = NEEDED_MEMORY_KB / 1024
        )));
    }
    if updates_on {
        println!("{}", crate::tr!("Внимание: обновления от Яндекса не отключены. Обновление вернёт колонку к Яндексу и может закрыть root — отключите их по вики: {wiki}", wiki = WIKI));
    }

    // What stands in the way: named; a mod with its own switch may be turned off, each after its own «да».
    if !found.is_empty() {
        println!("\n{}", crate::tr!("Что на колонке может помешать:"));
        let mut changed = false;
        for neighbour in &found {
            print_neighbour(neighbour);
            if let Some(switch) = neighbour.switch {
                if yes {
                    continue;
                }
                println!("    {}", crate::tr!("Его же командой: {command}", command = switch));
                if agree(&crate::tr!("    Выключить его? Файлы мода останутся. [да/нет] "))?
                {
                    let command = neighbours::turn_off_command(&neighbour.id).unwrap_or_default();
                    match run_ok(shell, &command).await {
                        Ok(_) => {
                            println!("    {}", crate::tr!("Выключен."));
                            changed = true;
                        }
                        Err(error) => println!("    {}", crate::tr!("Не выключился: {why}", why = error)),
                    }
                }
            }
        }
        if changed {
            let around = run_ok(shell, &STEPS.neighbours).await.unwrap_or_default();
            found = neighbours::read(&around).0;
        }
        let blocking: Vec<&Neighbour> = found.iter().filter(|n| n.severity == Severity::Blocks).collect();
        if !blocking.is_empty() {
            println!("\n{}", crate::tr!("С этим DuoWave, скорее всего, не заработает:"));
            for neighbour in &blocking {
                println!("  - {}", name_of(neighbour));
            }
            if yes || !agree(&crate::tr!("Всё равно ставить? [да/нет] "))? {
                return Err(Stop(crate::tr!("Ничего не менял. Выключите мешающее и запустите установку снова.")));
            }
        }
    }

    // The plan, then the «да».
    println!("\n{}", crate::tr!("Что будет:"));
    println!("  - {}", crate::tr!("программа DuoWave — в {dir} на колонке (около 20 МБ);", dir = DIR));
    println!(
        "  - {}",
        crate::tr!("голос колонки — на неё; файл прошивки quasar.cfg не меняется, поверх него ляжет наша копия;")
    );
    println!("  - {}", crate::tr!("колонка перезагрузится — около минуты;"));
    println!(
        "  - {}",
        crate::tr!(
            "обратно к Яндексу: sh {restore}; убрать DuoWave совсем: sh {uninstall}.",
            restore = RESTORE_SCRIPT,
            uninstall = UNINSTALL_SCRIPT
        )
    );
    if !yes && !agree(&crate::tr!("Ставить DuoWave на колонку? [да/нет] "))? {
        println!("{}", crate::tr!("Ничего не менял."));
        return Ok(None);
    }

    // The station's build this program carries: its signature checked before it goes anywhere.
    step(&crate::tr!("Беру сборку для колонки и проверяю подпись"));
    let build = payload::load(config).await.map_err(Stop)?;
    let program = build.program().map_err(Stop)?;
    done();

    step(&crate::tr!("Кладу на колонку путь назад (restore.sh, uninstall.sh)"));
    for (command, input) in deliveries() {
        run_input(shell, &command, &input).await?;
    }
    done();

    step(&crate::tr!("Копирую программу ({size} МБ)", size = program.len() / (1024 * 1024)));
    run_ok(shell, &format!("mkdir -p {DIR}/bin")).await?;
    shell.upload(BIN, &program).await?;
    run_ok(shell, &format!("chmod 755 {BIN}")).await?;
    done();

    step(&crate::tr!("Ставлю DuoWave на колонку и запускаю"));
    run_ok(shell, &format!("{BIN} station install --no-start")).await?;
    let status: Value =
        serde_json::from_str(&run_ok(shell, &format!("{BIN} station status --json")).await?).unwrap_or(Value::Null);
    let port = status.get("port").and_then(Value::as_u64).unwrap_or(DEFAULT_PORT as u64) as u16;
    run_ok(shell, &format!("{SERVICE_COMMAND} start")).await?;
    if !wait(60, || async { health(address, port).await.is_some() }).await {
        return Err(Stop(crate::tr!(
            "DuoWave на колонке не запустился. Что случилось — на колонке: {bin} station log",
            bin = BIN
        )));
    }
    done();

    step(&crate::tr!("Направляю голос колонки на DuoWave и перезагружаю"));
    run_ok(shell, &format!("{BIN} station point local --reboot")).await?;
    done();
    Ok(Some(port))
}

/// After the reboot: the station back, its voice at its own gateway. Not back in 3 minutes — said, with the way back.
/// True when it is back.
async fn arrive(address: &str, port: u16) -> bool {
    step(&crate::tr!("Жду, пока колонка вернётся (до 3 минут)"));
    tokio::time::sleep(Duration::from_secs(15)).await;
    let came = wait(180, || async {
        health(address, port).await.and_then(|health| health.pointer("/station/connected")?.as_bool()) == Some(true)
    })
    .await;
    if came {
        done();
        println!(
            "\n{}",
            crate::tr!(
                "Готово: DuoWave на колонке. Панель — http://{address}:{port}/ — задайте пароль и выберите, кто отвечает.",
                address = address,
                port = port
            )
        );
        println!("{}", crate::tr!("Компьютер больше не нужен."));
        true
    } else {
        println!();
        println!(
            "{}",
            crate::tr!(
                "Колонка ещё не вернулась к своему центру. Подождите минуту и откройте http://{address}:{port}/ . Не открывается — вернуть Яндекс: ssh -p 2222 root@{address} sh {restore}",
                address = address,
                port = port,
                restore = RESTORE_SCRIPT
            )
        );
        false
    }
}

/// The panel's first-run code (test 19.11, remark 19): the speaker's panel asks it of anyone not on the speaker itself,
/// and it lives only in the speaker's log — a person without SSH could not get it. The gateway makes a new one at every
/// start, so it is read after the reboot, from the same SSH; a panel with its password set already asks none.
async fn setup_code(address: &str, port: u16, opener: &RusshOpener, options: ShellOptions) {
    let state = async {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().ok()?;
        let body: Value =
            client.get(format!("http://{address}:{port}/api/v1/auth/state")).send().await.ok()?.json().await.ok()?;
        body.pointer("/message/configured").and_then(Value::as_bool)
    };
    if state.await != Some(false) {
        return;
    }
    let read = async {
        let shell = opener.open(address, options).await.ok()?;
        let line = shell.run(&format!("grep 'первая настройка пульта' {DATA}/gateway.log | tail -n 1"), None).await;
        shell.close().await;
        code_in(&line.ok()?.stdout)
    };
    match read.await {
        Some(code) => println!(
            "{}",
            crate::tr!("Код первой настройки панели: {code} — панель спросит его вместе с паролем.", code = code)
        ),
        None => println!(
            "{}",
            crate::tr!(
                "Панель спросит код первой настройки — он в журнале колонки: ssh -p 2222 root@{address} \"grep 'первая настройка' {log} | tail -n 1\"",
                address = address,
                log = format!("{DATA}/gateway.log")
            )
        ),
    }
}

/// The code `1234-5678` in a log line, if there is one.
fn code_in(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .find(|word| {
            word.len() == 9 && word.as_bytes()[4] == b'-' && word.chars().filter(char::is_ascii_digit).count() == 8
        })
        .map(str::to_string)
}

fn print_neighbour(neighbour: &Neighbour) {
    let tag = if neighbour.off {
        crate::tr!("выключен")
    } else if neighbour.severity == Severity::Blocks {
        crate::tr!("мешает")
    } else {
        crate::tr!("может мешать")
    };
    println!("  - [{tag}] {}", name_of(neighbour));
}

/// A neighbour's name in the terminal: the mod's, or what the trace is, with its file.
fn name_of(neighbour: &Neighbour) -> String {
    let name = match neighbour.id.as_str() {
        "yajavis" => crate::tr!("Джарвис (YaJavis): свой помощник на том же микрофоне"),
        "jarvis-server" => crate::tr!("старый «серверный» Джарвис: свой голосовой режим"),
        "alisa-voice" => crate::tr!("Alisa ChatGPT Voice: свой голосовой режим поверх Алисы"),
        "alisa-cli" => crate::tr!("Alisa CLI: ChatGPT Voice и свой VPN"),
        "pulsar" => crate::tr!("Pulsar: вместо Алисы спутник Home Assistant"),
        "quasar-bridge" => crate::tr!("Quasar Bridge: своя модель вместо Алисы"),
        "alice-led" => crate::tr!("Alice LED: своё на кольце"),
        "maind-off" => crate::tr!("Алиса не запущена"),
        "config-mount" => crate::tr!("чужая копия quasar.cfg"),
        "alice-mount" => crate::tr!("подменён файл Алисы"),
        "alice-preload" => crate::tr!("чужая библиотека внутри Алисы"),
        "low-memory" => crate::tr!("мало свободного ОЗУ"),
        other => other.to_string(),
    };
    match (&neighbour.detail, neighbour.id.as_str()) {
        (Some(mb), "low-memory") => format!("{name} ({})", crate::tr!("{mb} МБ", mb = mb)),
        (Some(detail), _) => format!("{name} ({detail})"),
        _ => name,
    }
}

/// What problem_of's words mean to the person in the terminal.
/// How many passwords are asked before the setup gives up.
const PASSWORD_TRIES: usize = 3;

/// The station's shell; root's password asked when the station refuses the one it has (the empty one by default).
/// Test 21.11: a station with a root password of its own, and «Свой пароль — в STATION_SSH_PASSWORD» left the tester
/// guessing where to type it. `ask` reads the terminal itself, with no echo (rpassword: /dev/tty, the Windows console —
/// `curl | sh` holds stdin); None — no terminal, or no answer: the hint as before. An empty answer stops.
async fn open_asking(
    opener: &dyn ShellOpener,
    address: &str,
    options: &mut ShellOptions,
    ask: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Box<dyn StationShell>, Stop> {
    let mut tries = 0;
    loop {
        let error = match opener.open(address, options.clone()).await {
            Ok(shell) => return Ok(shell),
            Err(error) => error,
        };
        let problem = problem_of(&error);
        if problem != "auth-failed" || tries == PASSWORD_TRIES {
            return Err(Stop(said_of(&problem)));
        }
        let question = if tries == 0 {
            crate::tr!(
                "Колонка не пустила root без пароля. Пароль root колонки (его не видно при вводе; пусто — выйти): "
            )
        } else {
            crate::tr!("Пароль не подошёл. Ещё раз (пусто — выйти): ")
        };
        match ask(&question).map(|answer| answer.trim_end_matches(['\r', '\n']).to_string()) {
            Some(password) if !password.is_empty() => options.password = password,
            _ => return Err(Stop(said_of(&problem))),
        }
        tries += 1;
    }
}

fn said_of(problem: &str) -> String {
    match problem {
        "auth-failed" => crate::tr!(
            "Колонка не пустила по SSH: нужен вход root без пароля, как его делает вики ({wiki}). Свой пароль — в STATION_SSH_PASSWORD.",
            wiki = WIKI
        ),
        "host-key-changed" => crate::tr!("Ключ колонки сменился — похоже, это другое устройство. Проверьте адрес."),
        "unreachable" => crate::tr!("Колонка не отвечает по сети. Проверьте, что компьютер и колонка в одной сети Wi-Fi."),
        other => other.to_string(),
    }
}

async fn run_ok(shell: &dyn StationShell, command: &str) -> anyhow::Result<String> {
    let result = shell.run(command, None).await?;
    finished(result)
}

async fn run_input(shell: &dyn StationShell, command: &str, input: &str) -> anyhow::Result<String> {
    let result = shell.run(command, Some(input)).await?;
    finished(result)
}

fn finished(result: crate::stations::connect::station_shell::ShellResult) -> anyhow::Result<String> {
    if result.code != 0 {
        let said = if result.stderr.trim().is_empty() { result.stdout.trim() } else { result.stderr.trim() };
        anyhow::bail!("{}", if said.is_empty() { format!("exit {}", result.code) } else { said.to_string() });
    }
    Ok(result.stdout)
}

/// The station's gateway health, as the home network sees it (the whole of it: the station's state too).
async fn health(address: &str, port: u16) -> Option<Value> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().ok()?;
    let body: Value = client.get(format!("http://{address}:{port}/health")).send().await.ok()?.json().await.ok()?;
    Some(body.get("message").cloned().unwrap_or(body))
}

async fn wait<F, Fut>(seconds: u64, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let until = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while tokio::time::Instant::now() < until {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
}

fn step(what: &str) {
    print!("  … {what} ");
    let _ = std::io::stdout().flush();
}

fn done() {
    println!("✓");
}

fn ask(question: &str) -> Result<String, Stop> {
    print!("{question}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        return Err(Stop(crate::tr!("Нет ответа: запустите установку из терминала.")));
    }
    Ok(line.trim().to_string())
}

/// A «да»/«yes» — anything else is no.
fn agree(question: &str) -> Result<bool, Stop> {
    Ok(is_yes(&ask(question)?))
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "да" | "д" | "yes" | "y")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stations::connect::discovery::NoBanner;
    use crate::stations::connect::station_shell::{ShellError, ShellResult};
    use futures::future::BoxFuture;

    /// A station whose root takes only «secret»; the passwords it was offered.
    struct Locked(std::sync::Mutex<Vec<String>>);
    struct Opened;

    impl StationShell for Opened {
        fn run<'a>(&'a self, _command: &'a str, _input: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<ShellResult>> {
            Box::pin(async { anyhow::bail!("not here") })
        }
        fn fingerprint(&self) -> &str {
            "SHA256:test"
        }
        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    impl ShellOpener for Locked {
        fn banner<'a>(&'a self, _address: &'a str, _port: u16) -> BoxFuture<'a, Result<String, NoBanner>> {
            Box::pin(async { Ok("dropbear".into()) })
        }
        fn open<'a>(
            &'a self,
            _address: &'a str,
            options: ShellOptions,
        ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>> {
            self.0.lock().unwrap().push(options.password.clone());
            Box::pin(async move {
                if options.password == "secret" {
                    Ok(Box::new(Opened) as Box<dyn StationShell>)
                } else {
                    Err(ShellError::AuthFailed.into())
                }
            })
        }
    }

    fn options() -> ShellOptions {
        ShellOptions { port: 2222, user: "root".into(), password: String::new(), trust: None, timeout_ms: None }
    }

    #[tokio::test]
    async fn asks_root_s_password_when_the_empty_one_is_refused() {
        let station = Locked(Default::default());
        let mut options = options();
        let mut answers = vec!["wrong".to_string(), "secret\n".to_string()].into_iter();
        let mut asked = Vec::new();
        let shell = open_asking(&station, "192.0.2.1", &mut options, &mut |question| {
            asked.push(question.to_string());
            answers.next()
        })
        .await;
        assert!(shell.is_ok());
        assert_eq!(*station.0.lock().unwrap(), ["", "wrong", "secret"]);
        assert_eq!(asked.len(), 2);
        assert!(asked[1].starts_with("Пароль не подошёл"), "{asked:?}");
        // The password goes on to the setup code's SSH.
        assert_eq!(options.password, "secret");
    }

    #[tokio::test]
    async fn without_a_terminal_or_an_answer_it_gives_the_hint() {
        for answer in [None, Some(String::new())] {
            let station = Locked(Default::default());
            let Err(Stop(said)) = open_asking(&station, "192.0.2.1", &mut options(), &mut |_| answer.clone()).await
            else {
                panic!("no password, no shell");
            };
            assert!(said.contains("STATION_SSH_PASSWORD"), "{said}");
            assert_eq!(station.0.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn gives_up_after_three_wrong_passwords() {
        let station = Locked(Default::default());
        let result = open_asking(&station, "192.0.2.1", &mut options(), &mut |_| Some("nope".into())).await;
        assert!(result.is_err());
        assert_eq!(station.0.lock().unwrap().len(), 1 + PASSWORD_TRIES);
    }

    #[test]
    fn only_a_clear_yes_is_a_yes() {
        for answer in ["да", "Да", " д ", "yes", "Y"] {
            assert!(is_yes(answer), "{answer}");
        }
        for answer in ["", "нет", "n", "давай", "ok", "1"] {
            assert!(!is_yes(answer), "{answer}");
        }
    }

    /// The health check's client builds once the provider is in (the command installs it first).
    #[test]
    fn the_http_client_builds_with_the_tls_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        assert!(reqwest::Client::builder().timeout(Duration::from_secs(3)).build().is_ok());
    }

    #[test]
    fn speaks_the_terminals_language() {
        assert_eq!(terminal_language("ru_RU.UTF-8").as_deref(), Some("ru"));
        assert_eq!(terminal_language("en_US.UTF-8").as_deref(), Some("en"));
        assert_eq!(terminal_language("de").as_deref(), Some("de"));
        assert_eq!(terminal_language("C.UTF-8").as_deref(), Some("en"));
        assert_eq!(terminal_language("POSIX").as_deref(), Some("en"));
        assert_eq!(terminal_language("").as_deref(), None);
    }

    #[test]
    fn every_neighbour_has_its_words() {
        for id in [
            "yajavis",
            "jarvis-server",
            "alisa-voice",
            "alisa-cli",
            "pulsar",
            "quasar-bridge",
            "alice-led",
            "maind-off",
            "config-mount",
            "alice-mount",
            "alice-preload",
        ] {
            let neighbour =
                Neighbour { id: id.into(), severity: Severity::Warns, detail: None, off: false, switch: None };
            assert_ne!(name_of(&neighbour), id, "{id} has no words");
        }
        let memory = Neighbour {
            id: "low-memory".into(),
            severity: Severity::Warns,
            detail: Some("22".into()),
            off: false,
            switch: None,
        };
        assert!(name_of(&memory).contains("22"));
    }
}
