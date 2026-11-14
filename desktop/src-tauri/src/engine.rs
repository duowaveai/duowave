//! The gateway inside the app: the Rust engine — crate `duowave` — runs in this process, so there is no Node and no
//! sidecar. Here it starts, stops and restarts; its status (the tray and the window follow it) comes from the run
//! itself and from GET /health; the computer is kept awake while a run is up; the app's own notes go into its log.
//!
//! The engine reads its configuration from DATA_DIR/.env and the few system variables a program needs (never a
//! working copy's .env, never a PORT from the shell the app was started in), writes its log to the app's log file,
//! and is told it is managed by the desktop app: its updates come through the app's updater (updates.rs).

use crate::lang::t;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::awake::Awake;

pub const STOP_GRACE: Duration = Duration::from_secs(6);
/// A first start (migrations, the database's key) takes a moment; this is for a stuck one.
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// An engine that stops by itself after this long a good run is started once more; one that stops sooner is left
/// failed, so a broken setup does not spin.
const RESTART_AFTER: Duration = Duration::from_secs(30);
/// A log above this size is moved to gateway.log.1 when the app starts.
const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// Without PORT the engine listens on 3000 (its configuration's default).
use duowave::config::DEFAULT_PORT;
/// Who manages the engine: its updates say «Обновите приложение» and go through the app's updater.
pub const MANAGED_ENV: &str = "DUOWAVE_MANAGED";

/// What the engine takes from the app's environment: the system's basics, proxies and certificates, and what the
/// model servers and engine installers it starts read. Its own configuration comes from DATA_DIR/.env only.
const PASS_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "APPDATA",
    "LOCALAPPDATA",
    "USERPROFILE",
    "USERNAME",
    "HOMEDRIVE",
    "HOMEPATH",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "__CF_USER_TEXT_ENCODING",
    "RUST_LOG",
];
const PASS_PREFIXES: &[&str] = &[
    "LC_",
    "SSL_",
    "XDG_",
    "GIGAAM_",
    "HF_",
    "HUGGINGFACE_",
    "TORCH_",
    "PYTORCH_",
    "OMP_",
    "MKL_",
    "CUDA_",
    "UV_",
];

pub struct Paths {
    /// DATA_DIR: everything the gateway keeps, its .env first.
    pub data: PathBuf,
    /// The gateway's log and the app's own notes.
    pub log: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// For the tray: «Шлюз остановлен: …».
    pub short: String,
    /// For the window: what happened and what to do.
    pub advice: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    pub port: u16,
    /// The engine serves, and the last /health answered.
    pub responding: bool,
    /// /health says a station is connected.
    pub station: bool,
    /// The gateway runs on the station (docs/plans/station-placement.md): whether the station's gateway answers.
    pub station_gateway: Option<bool>,
    pub problem: Option<Problem>,
    /// The port was taken (another program, another gateway): the window offers «Взять свободный порт».
    pub port_busy: bool,
}

impl Status {
    pub fn title(&self) -> String {
        match self.phase {
            Phase::Starting => t("Шлюз запускается…", "Gateway is starting…").into(),
            Phase::Running if !self.responding => t("Шлюз не отвечает", "Gateway is not responding").into(),
            Phase::Running if self.station_gateway == Some(true) => {
                t("Шлюз на колонке · на связи", "Gateway on the speaker · connected").into()
            }
            Phase::Running if self.station_gateway == Some(false) => t(
                "Шлюз на колонке не отвечает",
                "Gateway on the speaker is not responding",
            )
            .into(),
            Phase::Running if self.station => t(
                "Шлюз работает · колонка на связи",
                "Gateway is running · speaker connected",
            )
            .into(),
            Phase::Running => t("Шлюз работает", "Gateway is running").into(),
            Phase::Stopping => t("Шлюз останавливается…", "Gateway is stopping…").into(),
            Phase::Stopped => t("Шлюз остановлен", "Gateway is stopped").into(),
            Phase::Failed => match &self.problem {
                Some(problem) => format!("{}: {}", t("Шлюз остановлен", "Gateway is stopped"), problem.short),
                None => t("Шлюз остановлен", "Gateway is stopped").into(),
            },
        }
    }
}

type Listener = Box<dyn Fn(&Status) + Send + Sync>;

/// One run of the engine: how to stop it, and how to know it stopped.
struct Run {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    finished: mpsc::Receiver<()>,
}

struct State {
    run: Option<Run>,
    status: Status,
    /// The engine of a run (its generation with it), once built: the updater's hook goes into it.
    engine: Option<(u64, Arc<duowave::app::App>)>,
}

pub struct Gateway {
    paths: Paths,
    state: Mutex<State>,
    /// Start, stop and restart one at a time.
    ops: Mutex<()>,
    /// Bumped by every start and stop: what an older run does after that is ignored.
    generation: AtomicU64,
    listener: Mutex<Option<Listener>>,
    /// «Не давать компьютеру засыпать», and its hold while a run is up.
    keep_awake: AtomicBool,
    awake: Awake,
    /// How the engine installs updates: through the app's updater.
    installer: Mutex<Option<Arc<dyn duowave::updates::Installer>>>,
    /// The app's release feed, for the engine's offer in the panel (DATA_DIR/.env may name another).
    feed: Mutex<Option<String>>,
}

struct Health {
    station: bool,
    station_gateway: Option<bool>,
}

impl Gateway {
    pub fn new(paths: Paths, keep_awake: bool) -> Self {
        if let Some(dir) = paths.log.parent() {
            let _ = fs::create_dir_all(dir);
        }
        rotate_log(&paths.log);
        duowave::init_logging(Some(&paths.log));
        let port = port_of(&engine_env(&paths));
        Self {
            paths,
            state: Mutex::new(State {
                run: None,
                status: Status {
                    phase: Phase::Stopped,
                    port,
                    responding: false,
                    station: false,
                    station_gateway: None,
                    problem: None,
                    port_busy: false,
                },
                engine: None,
            }),
            ops: Mutex::new(()),
            generation: AtomicU64::new(0),
            listener: Mutex::new(None),
            keep_awake: AtomicBool::new(keep_awake),
            awake: Awake::new(),
            installer: Mutex::new(None),
            feed: Mutex::new(None),
        }
    }

    /// The release feed the engine checks: the app's own.
    pub fn set_feed_url(&self, url: Option<String>) {
        *self.feed.lock().unwrap() = url;
    }

    /// The app's updater installs the engine's updates; applies to the running engine and every later one.
    pub fn set_update_installer(&self, installer: Arc<dyn duowave::updates::Installer>) {
        if let Some((_, engine)) = &self.state.lock().unwrap().engine {
            engine.updates.set_installer(installer.clone());
        }
        *self.installer.lock().unwrap() = Some(installer);
    }

    pub fn keep_awake(&self) -> bool {
        self.keep_awake.load(Ordering::SeqCst)
    }

    /// The engine running now, if it is up: the window gives it a folder dropped on it.
    pub fn app(&self) -> Option<Arc<duowave::app::App>> {
        self.state
            .lock()
            .unwrap()
            .engine
            .as_ref()
            .map(|(_, engine)| engine.clone())
    }

    /// The home folder of DuoWave, as the engine's environment names it (`~/DuoWave` unless the .env says).
    pub fn home_folder(&self) -> Option<PathBuf> {
        engine_env(&self.paths).get("HOME_FILES").map(PathBuf::from)
    }

    /// Switches the hold now: on while the engine runs, off at once.
    pub fn set_keep_awake(&self, on: bool) {
        self.keep_awake.store(on, Ordering::SeqCst);
        let running = self.state.lock().unwrap().run.is_some();
        if on && running {
            self.hold_awake();
        } else {
            self.awake.release();
        }
    }

    /// The engine is this process: the hold ends with the app at the latest.
    fn hold_awake(&self) {
        match self.awake.hold(std::process::id()) {
            Ok(()) => self.note("keeping the computer awake while the gateway runs"),
            Err(error) => self.note(&format!("cannot keep the computer awake: {error}")),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn status(&self) -> Status {
        self.state.lock().unwrap().status.clone()
    }

    pub fn on_change(&self, listener: impl Fn(&Status) + Send + Sync + 'static) {
        *self.listener.lock().unwrap() = Some(Box::new(listener));
    }

    pub fn start(self: &Arc<Self>) {
        let _op = self.ops.lock().unwrap();
        self.start_locked();
    }

    pub fn stop(&self, grace: Duration) {
        let _op = self.ops.lock().unwrap();
        self.stop_locked(grace);
    }

    pub fn restart(self: &Arc<Self>) {
        let _op = self.ops.lock().unwrap();
        self.stop_locked(STOP_GRACE);
        self.start_locked();
    }

    /// «Взять свободный порт»: the first free one from the default up, written into .env (PORT), then a start. A
    /// station already connected to the old port is the panel's to re-point (its «Переподключить» banner).
    pub fn take_free_port(self: &Arc<Self>) -> Result<u16, String> {
        let port = free_port().ok_or(t("свободного порта не нашлось", "no free port found"))?;
        crate::envfile::set_port(&self.paths.data.join(".env"), port).map_err(|error| {
            t(
                &format!("не удалось записать .env: {error}"),
                &format!("could not write .env: {error}"),
            )
            .to_owned()
        })?;
        self.note(&format!("took the free port {port} (written into .env)"));
        self.restart();
        Ok(port)
    }

    /// The app's own lines in the gateway's log: what it started, stopped, and why.
    pub fn note(&self, line: &str) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.paths.log) {
            let _ = writeln!(file, "[app {}] {line}", utc_now());
        }
    }

    fn start_locked(self: &Arc<Self>) {
        if self.state.lock().unwrap().run.is_some() {
            return;
        }
        let mut env = engine_env(&self.paths);
        if let (None, Some(feed)) = (env.get("UPDATE_FEED_URL"), self.feed.lock().unwrap().clone()) {
            env = env.with("UPDATE_FEED_URL", &feed);
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let port = port_of(&env);
        self.update(|status| {
            *status = Status {
                phase: Phase::Starting,
                port,
                responding: false,
                station: false,
                station_gateway: None,
                problem: None,
                port_busy: false,
            }
        });

        if let Err(error) = duowave::config::data_dir::prepare_data_dir(&env) {
            return self.fail(Problem {
                short: t("папка данных недоступна", "data folder unavailable").into(),
                advice: {
                    let path = self.paths.data.display();
                    t(
                        &format!("Не удалось подготовить {path}: {error}."),
                        &format!("Could not prepare {path}: {error}."),
                    )
                    .into()
                },
            });
        }
        let config = match duowave::config::Config::load(&env) {
            Ok(config) => config,
            Err(error) => {
                self.note(&error.to_string());
                return self.fail(Problem {
                    short: t("ошибка в .env", "error in .env").into(),
                    advice: {
                        let file = self.paths.data.join(".env");
                        let file = file.display();
                        let problems = error.problems.join("; ");
                        t(
                            &format!(
                                "В файле {file} есть значения, которые шлюз не принимает: {problems}.\n\n\
                                 Исправьте файл и нажмите «Запустить снова»."
                            ),
                            &format!(
                                "The file {file} has values the gateway does not accept: {problems}.\n\n\
                                 Fix the file and press “Start again”."
                            ),
                        )
                        .into()
                    },
                });
            }
        };
        let port = config.app.port;
        self.update(|status| status.port = port);
        if let Some(problem) = port_taken(port) {
            self.note(&format!("port {port} is taken: {}", problem.short));
            self.update(|status| status.port_busy = true);
            return self.fail(problem);
        }
        let listener = match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                self.update(|status| status.port_busy = true);
                return self.fail(busy(port));
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return self.fail(no_permission(port))
            }
            Err(error) => {
                return self.fail(Problem {
                    short: t(
                        &format!("порт {port} не открывается"),
                        &format!("port {port} will not open"),
                    )
                    .into(),
                    advice: t(
                        &format!("Не удалось открыть порт {port}: {error}."),
                        &format!("Could not open port {port}: {error}."),
                    )
                    .into(),
                })
            }
        };
        let _ = listener.set_nonblocking(true);
        self.note(&format!(
            "starting the gateway (Rust engine {}): port {port}, data {}",
            duowave::VERSION,
            self.paths.data.display()
        ));

        let (shutdown, stop_signal) = tokio::sync::oneshot::channel::<()>();
        let (done, finished) = mpsc::channel::<()>();
        self.state.lock().unwrap().run = Some(Run {
            shutdown: Some(shutdown),
            finished,
        });
        if self.keep_awake() {
            self.hold_awake();
        }
        let gateway = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let started = Instant::now();
            // A task of its own: a panic in the engine ends this run, not the app's watch over it.
            let serving = {
                let gateway = gateway.clone();
                tauri::async_runtime::spawn(
                    async move { gateway.serve(generation, config, listener, stop_signal).await },
                )
            };
            let outcome = serving.await.unwrap_or_else(|error| {
                Err(Problem {
                    short: t("сбой", "crashed").into(),
                    advice: t(
                        &format!("Шлюз прервался: {error}. Подробности — в журнале."),
                        &format!("The gateway was interrupted: {error}. See the log for details."),
                    )
                    .into(),
                })
            });
            let _ = done.send(());
            // A stop or a restart ended this run: it says what follows.
            if gateway.generation.load(Ordering::SeqCst) == generation {
                thread::spawn(move || gateway.ended(generation, started, outcome));
            }
        });
        let gateway = Arc::clone(self);
        thread::spawn(move || gateway.watch(generation, port));
    }

    async fn serve(
        self: &Arc<Self>,
        generation: u64,
        config: duowave::config::Config,
        listener: std::net::TcpListener,
        stop_signal: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), Problem> {
        let engine = duowave::app::App::build(config).map_err(|error| Problem {
            short: t("не запускается", "will not start").into(),
            advice: t(
                &format!("Шлюз не собрался: {error:#}. Подробности — в журнале."),
                &format!("The gateway could not be built: {error:#}. See the log for details."),
            )
            .into(),
        })?;
        if let Some(installer) = self.installer.lock().unwrap().clone() {
            engine.updates.set_installer(installer);
        }
        engine.start().await.map_err(|error| Problem {
            short: t("не запускается", "will not start").into(),
            advice: t(
                &format!("Шлюз не запустился: {error:#}. Подробности — в журнале."),
                &format!("The gateway did not start: {error:#}. See the log for details."),
            )
            .into(),
        })?;
        self.state.lock().unwrap().engine = Some((generation, engine.clone()));
        let listener = tokio::net::TcpListener::from_std(listener).map_err(|error| Problem {
            short: "порт не открывается".into(),
            advice: format!("{error}"),
        })?;
        self.update_if(generation, |status| {
            status.phase = Phase::Running;
            status.responding = true;
        });
        let served = duowave::app::serve(engine.clone(), listener, async move {
            let _ = stop_signal.await;
        })
        .await;
        // What is still going is journaled as cancelled before the database closes, as duowave's own run does.
        engine.requests.shutdown();
        {
            let mut state = self.state.lock().unwrap();
            if state.engine.as_ref().is_some_and(|(of, _)| *of == generation) {
                state.engine = None;
            }
        }
        served.map_err(|error| Problem {
            short: t("остановился", "stopped").into(),
            advice: t(
                &format!("Шлюз остановился: {error}. Подробности — в журнале."),
                &format!("The gateway stopped: {error}. See the log for details."),
            )
            .into(),
        })
    }

    fn stop_locked(&self, grace: Duration) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let run = self.state.lock().unwrap().run.take();
        let Some(mut run) = run else {
            if self.status().phase == Phase::Starting {
                self.update(|status| status.phase = Phase::Stopped);
            }
            return;
        };
        self.update(|status| {
            status.phase = Phase::Stopping;
            status.responding = false;
            status.station = false;
            status.station_gateway = None;
        });
        if let Some(shutdown) = run.shutdown.take() {
            let _ = shutdown.send(());
        }
        let stopped = run.finished.recv_timeout(grace).is_ok();
        self.awake.release();
        self.note(if stopped {
            "stopped the gateway"
        } else {
            "the gateway did not stop in time; it ends with the app"
        });
        self.update(|status| {
            *status = Status {
                phase: Phase::Stopped,
                problem: None,
                ..status.clone()
            }
        });
    }

    /// A run that ended without a stop: started once more after a good run, otherwise left failed with the reason.
    fn ended(self: Arc<Self>, generation: u64, started: Instant, outcome: Result<(), Problem>) {
        let problem = outcome.err().unwrap_or_else(|| Problem {
            short: t("остановился", "stopped").into(),
            advice: t(
                "Шлюз остановился сам. Подробности — в журнале.",
                "The gateway stopped on its own. See the log for details.",
            )
            .into(),
        });
        {
            let _op = self.ops.lock().unwrap();
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            self.state.lock().unwrap().run = None;
            self.awake.release();
        }
        self.note(&format!(
            "the gateway stopped after {}s: {}",
            started.elapsed().as_secs(),
            problem.advice
        ));
        if self.status().phase == Phase::Running && started.elapsed() >= RESTART_AFTER {
            self.update_if(generation, |status| {
                status.phase = Phase::Starting;
                status.responding = false;
                status.station = false;
                status.station_gateway = None;
            });
            thread::sleep(Duration::from_secs(2));
            let _op = self.ops.lock().unwrap();
            // A stop or restart in between has the last word.
            if self.generation.load(Ordering::SeqCst) == generation {
                self.note("starting it again");
                self.start_locked();
            }
            return;
        }
        self.update_if(generation, |status| {
            status.phase = Phase::Failed;
            status.responding = false;
            status.station = false;
            status.station_gateway = None;
            status.problem = Some(problem);
        });
    }

    /// Watches one run: a start that does not end, then /health — often until it first answers, then every few
    /// seconds — for whether the engine answers and whether a station is connected.
    fn watch(self: Arc<Self>, generation: u64, port: u16) {
        let started = Instant::now();
        // Once it served, a «Starting» is the start again after it stopped by itself (ended), not a stuck start.
        let mut served = false;
        let mut answered = false;
        let mut misses = 0;
        loop {
            thread::sleep(if answered {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(500)
            });
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            match self.status().phase {
                // The run ended and said why (ended): nothing more to watch.
                Phase::Failed | Phase::Stopped => return,
                Phase::Starting if !served && started.elapsed() > START_TIMEOUT => {
                    self.note("the gateway did not start in time: stopping it");
                    let _op = self.ops.lock().unwrap();
                    if self.generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    self.stop_locked(STOP_GRACE);
                    return self.fail(Problem {
                        short: t("не отвечает", "not responding").into(),
                        advice: {
                            let seconds = START_TIMEOUT.as_secs();
                            t(
                                &format!("Шлюз не запустился за {seconds} секунд. Подробности — в журнале."),
                                &format!(
                                    "The gateway did not start within {seconds} seconds. See the log for details."
                                ),
                            )
                            .into()
                        },
                    });
                }
                Phase::Running => {
                    served = true;
                    match health(port) {
                        Some(health) => {
                            answered = true;
                            misses = 0;
                            self.update_if(generation, |status| {
                                status.responding = true;
                                status.station = health.station;
                                status.station_gateway = health.station_gateway;
                            });
                        }
                        None => {
                            misses += 1;
                            if misses >= 3 {
                                self.update_if(generation, |status| {
                                    status.responding = false;
                                    status.station = false;
                                    status.station_gateway = None;
                                    status.station_gateway = None;
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn fail(&self, problem: Problem) {
        self.update(|status| {
            status.phase = Phase::Failed;
            status.responding = false;
            status.station = false;
            status.station_gateway = None;
            status.problem = Some(problem.clone());
        });
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let before = state.status.clone();
            change(&mut state.status);
            (state.status != before).then(|| state.status.clone())
        };
        // Outside the lock: the listener may ask for the status itself.
        if let Some(status) = changed {
            if let Some(listener) = self.listener.lock().unwrap().as_ref() {
                listener(&status);
            }
        }
    }

    /// An update from a run that a stop has not overtaken.
    fn update_if(&self, generation: u64, change: impl FnOnce(&mut Status)) {
        if self.generation.load(Ordering::SeqCst) == generation {
            self.update(change);
        }
    }
}

/// The engine's environment: the system's basics from the app's own, DATA_DIR and the app's marks, then
/// DATA_DIR/.env under them.
fn engine_env(paths: &Paths) -> duowave::config::Env {
    let mut vars: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = name.into_string().ok()?;
            let value = value.into_string().ok()?;
            let upper = name.to_ascii_uppercase();
            let pass =
                PASS_ENV.contains(&upper.as_str()) || PASS_PREFIXES.iter().any(|prefix| upper.starts_with(prefix));
            pass.then_some((name, value))
        })
        .collect();
    vars.insert("DATA_DIR".into(), paths.data.to_string_lossy().into_owned());
    vars.insert("NODE_ENV".into(), "production".into());
    vars.insert(MANAGED_ENV.into(), "desktop".into());
    let env = duowave::config::Env::with_files(vars.clone(), &[paths.data.join(".env")]);
    // The home folder of DuoWave is the person's own `~/DuoWave` (docs/plans/shared-folders.md, the owner's decision
    // 14.11): seen in Finder like any folder. A HOME_FILES of the .env names another place.
    if env.get("HOME_FILES").is_some() {
        return env;
    }
    match dirs::home_dir() {
        Some(home) => {
            vars.insert("HOME_FILES".into(), home.join("DuoWave").to_string_lossy().into_owned());
            duowave::config::Env::with_files(vars, &[paths.data.join(".env")])
        }
        None => env,
    }
}

/// The port the engine will listen on, as its configuration reads PORT.
fn port_of(env: &duowave::config::Env) -> u16 {
    env.get("PORT")
        .and_then(|port| port.trim().parse().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_PORT)
}

/// A port nothing listens on: the default and the next ones first (a familiar number), else any the system gives.
fn free_port() -> Option<u16> {
    let free = |port: u16| port_taken(port).is_none() && std::net::TcpListener::bind(("0.0.0.0", port)).is_ok();
    (DEFAULT_PORT..DEFAULT_PORT + 20).find(|port| free(*port)).or_else(|| {
        std::net::TcpListener::bind(("0.0.0.0", 0))
            .ok()
            .and_then(|listener| listener.local_addr().ok())
            .map(|address| address.port())
    })
}

/// Something already answers on the port: another gateway (a working copy's, a service's) or another program.
fn port_taken(port: u16) -> Option<Problem> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    Some(if health(port).is_some() {
        Problem {
            short: t(&format!("порт {port} занят"), &format!("port {port} is busy")).into(),
            advice: t(
                &format!(
                    "На порту {port} уже работает другой шлюз — например, запущенный вручную из папки с кодом или \
                     службой. Остановите его и нажмите «Запустить снова» или возьмите для этого шлюза свободный порт."
                ),
                &format!(
                    "Another gateway is already running on port {port} — for example one started by hand from the \
                     code folder, or a service. Stop it and press “Start again”, or take a free port for this gateway."
                ),
            )
            .into(),
        }
    } else {
        busy(port)
    })
}

fn busy(port: u16) -> Problem {
    Problem {
        short: t(&format!("порт {port} занят"), &format!("port {port} is busy")).into(),
        advice: t(
            &format!(
                "Порт {port} держит другая программа. Возьмите для шлюза свободный порт — приложение запишет его в .env \
                 в папке с данными (строка PORT) — или закройте ту программу и нажмите «Запустить снова». Колонку, уже \
                 подключённую к этому шлюзу, после смены порта пульт предложит переподключить."
            ),
            &format!(
                "Another program is holding port {port}. Take a free port for the gateway — the app writes it into .env \
                 in the data folder (the PORT line) — or close that program and press “Start again”. A speaker already \
                 connected to this gateway will be offered a reconnect in the panel after the port changes."
            ),
        )
        .into(),
    }
}

fn no_permission(port: u16) -> Problem {
    let advice = if cfg!(target_os = "linux") {
        let program = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        t(
            &format!(
                "На Linux порты ниже 1024 открывает только root. Разрешите их приложению и нажмите «Запустить снова»:\n\n\
                 sudo setcap cap_net_bind_service=+ep '{program}'\n\n\
                 или всем программам сразу (так же и для AppImage):\n\n\
                 sudo sysctl -w net.ipv4.ip_unprivileged_port_start={port}\n\n\
                 Чтобы это пережило перезагрузку, та же настройка — строкой в /etc/sysctl.d/90-duowave.conf."
            ),
            &format!(
                "On Linux only root opens ports below 1024. Allow the app to and press “Start again”:\n\n\
                 sudo setcap cap_net_bind_service=+ep '{program}'\n\n\
                 or allow all programs at once (the same for an AppImage):\n\n\
                 sudo sysctl -w net.ipv4.ip_unprivileged_port_start={port}\n\n\
                 To keep it after a reboot, put the same setting as a line in /etc/sysctl.d/90-duowave.conf."
            ),
        )
        .to_owned()
    } else {
        t(
            &format!("Система не дала шлюзу открыть порт {port}. Подробности — в журнале."),
            &format!("The system did not let the gateway open port {port}. See the log for details."),
        )
        .to_owned()
    };
    Problem {
        short: t(
            &format!("нет прав на порт {port}"),
            &format!("no permission for port {port}"),
        )
        .into(),
        advice,
    }
}

/// The log is one file the engine keeps open while the app runs: a big one is set aside before it is opened.
fn rotate_log(log: &Path) {
    if fs::metadata(log)
        .map(|meta| meta.len() > LOG_ROTATE_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(log, log.with_extension("log.1"));
    }
}

/// GET /health over plain HTTP/1.0 (no keep-alive, no chunks): `{"message": {"status": "ok", "station": …}}`.
fn health(port: u16) -> Option<Health> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(800)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
    // One write: a request in pieces may be answered and closed on before its tail arrives (a reset, not an answer).
    let request = format!("GET /health HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.take(1 << 20).read_to_end(&mut response).ok()?;
    let response = String::from_utf8_lossy(&response);
    let (head, body) = response.split_once("\r\n\r\n")?;
    if head.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    // The gateway wraps every answer in {message, code}.
    let health = json.get("message").filter(|inner| inner.is_object()).unwrap_or(&json);
    if health.get("status")?.as_str()? != "ok" {
        return None;
    }
    let station = health
        .pointer("/station/connected")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    // The gateway on the station, as this one sees it: placement.station {placement, reachable}.
    let station_gateway = health
        .pointer("/placement/station")
        .filter(|record| {
            record
                .get("placement")
                .and_then(|value| value.as_str())
                .is_some_and(|placement| placement != "pc")
        })
        .map(|record| {
            record
                .get("reachable")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        });
    Some(Health {
        station,
        station_gateway,
    })
}

fn utc_now() -> String {
    utc(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64)
}

/// UTC, to the second: 2026-11-06T01:04:08Z.
fn utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &Path) -> Paths {
        Paths {
            data: dir.to_path_buf(),
            log: dir.join("logs").join("gateway.log"),
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn wait_for(gateway: &Gateway, phase: Phase) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while gateway.status().phase != phase && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(gateway.status().phase, phase, "{:?}", gateway.status());
    }

    #[test]
    fn titles() {
        let mut status = Status {
            phase: Phase::Running,
            port: 80,
            responding: true,
            station: true,
            station_gateway: None,
            problem: None,
            port_busy: false,
        };
        assert_eq!(status.title(), "Шлюз работает · колонка на связи");
        status.station_gateway = Some(true);
        assert_eq!(status.title(), "Шлюз на колонке · на связи");
        status.station_gateway = Some(false);
        assert_eq!(status.title(), "Шлюз на колонке не отвечает");
        status.station_gateway = None;
        status.station = false;
        assert_eq!(status.title(), "Шлюз работает");
        status.responding = false;
        assert_eq!(status.title(), "Шлюз не отвечает");
        status.phase = Phase::Failed;
        status.problem = Some(busy(80));
        assert_eq!(status.title(), "Шлюз остановлен: порт 80 занят");
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_868_799), "2000-02-29T23:59:59Z");
        assert_eq!(utc(1_793_927_048), "2026-11-06T01:04:08Z");
    }

    #[test]
    fn the_engine_reads_only_data_dirs_env() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), "PORT=8123\nDIALOG_PROVIDER=echo\n").unwrap();
        let env = engine_env(&paths_in(dir.path()));
        assert_eq!(env.get("PORT"), Some("8123"));
        assert_eq!(port_of(&env), 8123);
        assert_eq!(env.get(MANAGED_ENV), Some("desktop"));
        assert_eq!(env.get("DATA_DIR"), dir.path().to_str());
        // Without PORT: the engine's own default, where the app must look too.
        fs::write(dir.path().join(".env"), "# nothing\n").unwrap();
        assert_eq!(port_of(&engine_env(&paths_in(dir.path()))), DEFAULT_PORT);
    }

    #[test]
    fn a_big_log_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gateway.log");
        fs::write(&log, "small\n").unwrap();
        rotate_log(&log);
        assert!(log.exists());
        let file = fs::File::create(&log).unwrap();
        file.set_len(LOG_ROTATE_BYTES + 1).unwrap();
        rotate_log(&log);
        assert!(!log.exists());
        assert!(dir.path().join("gateway.log.1").exists());
    }

    #[test]
    fn health_of_a_stub_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for body in [
                r#"{"message":{"status":"ok","station":{"connected":true}},"code":200}"#,
                r#"{"status":"ok"}"#,
                r#"{"status":"starting"}"#,
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 256];
                while !request.ends_with(b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).unwrap();
                    assert!(read > 0, "the request ended early");
                    request.extend_from_slice(&chunk[..read]);
                }
                assert!(request.starts_with(b"GET /health HTTP/1.0\r\n"));
                let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        assert!(health(port).unwrap().station);
        assert!(!health(port).unwrap().station);
        assert!(health(port).is_none());
        server.join().unwrap();
    }

    #[test]
    fn a_taken_port_is_named() {
        let dir = tempfile::tempdir().unwrap();
        // Another program on the port: it takes connections and closes them without a word (the app knocks, then
        // asks for /health).
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = other.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((stream, _)) = other.accept() {
                    drop(stream);
                }
            }
            other
        });
        fs::write(dir.path().join(".env"), format!("PORT={port}\n")).unwrap();
        let gateway = Arc::new(Gateway::new(paths_in(dir.path()), false));
        gateway.start();
        let status = gateway.status();
        assert_eq!(status.phase, Phase::Failed);
        assert_eq!(status.title(), format!("Шлюз остановлен: порт {port} занят"));
        assert!(status.problem.unwrap().advice.contains("другая программа"));
        drop(server.join().unwrap());
    }

    #[test]
    fn a_wrong_env_is_named() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), "PORT=eighty\n").unwrap();
        let gateway = Arc::new(Gateway::new(paths_in(dir.path()), false));
        gateway.start();
        let status = gateway.status();
        assert_eq!(status.title(), "Шлюз остановлен: ошибка в .env");
        let advice = status.problem.unwrap().advice;
        assert!(advice.contains(".env") && advice.contains("PORT"), "{advice}");
        assert!(gateway.state.lock().unwrap().run.is_none(), "nothing runs");
    }

    #[test]
    fn starts_serves_and_stops_in_process() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        fs::write(dir.path().join(".env"), format!("PORT={port}\n")).unwrap();
        let gateway = Arc::new(Gateway::new(paths_in(dir.path()), false));
        gateway.start();
        wait_for(&gateway, Phase::Running);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /generate_204 HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.split_whitespace().nth(1) == Some("204"), "{answer}");
        assert!(health(port).is_some(), "/health answers");

        // A second app's start finds this gateway on the port and says so.
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join(".env"), format!("PORT={port}\n")).unwrap();
        let second = Arc::new(Gateway::new(paths_in(other.path()), false));
        second.start();
        let problem = second.status().problem.expect("the second one fails");
        assert!(problem.advice.contains("уже работает другой шлюз"), "{problem:?}");
        assert!(second.status().port_busy, "the window offers another port");

        gateway.stop(STOP_GRACE);
        assert_eq!(gateway.status().phase, Phase::Stopped);
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "the port is free again"
        );
        assert!(dir.path().join("state").join("duowave.sqlite").exists());
        let log = fs::read_to_string(dir.path().join("logs").join("gateway.log")).unwrap();
        assert!(log.contains("[app ") && log.contains("stopped the gateway"), "{log}");

        // And it starts again, as «Перезапустить шлюз» does.
        gateway.restart();
        wait_for(&gateway, Phase::Running);
        gateway.stop(STOP_GRACE);
        assert_eq!(gateway.status().phase, Phase::Stopped);
    }
}
