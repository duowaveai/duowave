//! The engine: shoes, as this very file's second process (`DUOWAVE_OWN_SERVER_ROLE=engine`, main.rs) — shoes gives
//! the outside only its command line. Settings change → the file is rewritten and the engine gets SIGHUP; it falls →
//! started again after 1, 2, 4 … 30 s. The program's SIGTERM → the engine's, 2 s to go, then SIGKILL.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::{Notify, watch};

pub const ROLE_VAR: &str = "DUOWAVE_OWN_SERVER_ROLE";

pub struct Engine {
    config: PathBuf,
    /// Whether it should run: some connection has servers.
    wanted: watch::Sender<bool>,
    pid: Mutex<Option<u32>>,
    /// The program itself asked the engine to go (`stop`): its exit is no fall, no pause before the next start.
    asked_to_go: AtomicBool,
    /// Told each time the engine (re)starts or reloads: the choices go to it again.
    pub started: Arc<Notify>,
}

impl Engine {
    pub fn new(config: PathBuf) -> Arc<Engine> {
        let (wanted, _) = watch::channel(false);
        Arc::new(Engine {
            config,
            wanted,
            pid: Mutex::new(None),
            asked_to_go: AtomicBool::new(false),
            started: Arc::new(Notify::new()),
        })
    }

    pub fn running(&self) -> bool {
        self.pid.lock().unwrap().is_some()
    }

    /// The settings file is new: a running engine rereads it, a stopped one starts. Wanted either way: an engine still
    /// going after a `stop` (the last connection removed, a new one added at once) is started again once it is gone.
    pub fn apply(&self) {
        self.wanted.send_if_modified(|wanted| !std::mem::replace(wanted, true));
        if let Some(pid) = *self.pid.lock().unwrap() {
            // SAFETY: a signal to our own child.
            unsafe { libc::kill(pid as i32, libc::SIGHUP) };
            self.started.notify_one();
        }
    }

    pub fn stop(&self) {
        self.wanted.send_if_modified(|wanted| std::mem::replace(wanted, false));
        if let Some(pid) = *self.pid.lock().unwrap() {
            self.asked_to_go.store(true, Ordering::SeqCst);
            // SAFETY: a signal to our own child.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }

    /// Stops it and waits for it to go, 2 s at most, then kills it.
    pub async fn shutdown(&self) {
        self.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.running() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if let Some(pid) = *self.pid.lock().unwrap() {
            // SAFETY: a signal to our own child.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }

    /// Keeps the engine running while it is wanted.
    pub async fn supervise(self: Arc<Engine>) {
        let mut wanted = self.wanted.subscribe();
        let mut pause = Duration::from_secs(1);
        loop {
            while !*wanted.borrow_and_update() {
                if wanted.changed().await.is_err() {
                    return;
                }
            }
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(error) => {
                    eprintln!("own-server: the program's own file is not found: {error}");
                    return;
                }
            };
            let mut command = Command::new(exe);
            command
                .env(ROLE_VAR, "engine")
                // Warnings only: a line per connection would fill the gateway's journal and name every host visited.
                .env("RUST_LOG", "warn")
                .args(["--no-reload", "-t", "2"])
                .arg(&self.config)
                .kill_on_drop(true);
            #[cfg(unix)]
            command.arg0("duowave-own-server-engine");
            let started = Instant::now();
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    eprintln!("own-server: the engine does not start: {error}");
                    tokio::time::sleep(pause).await;
                    pause = (pause * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            *self.pid.lock().unwrap() = child.id();
            self.started.notify_one();
            let status = tokio::select! {
                status = child.wait() => status.ok(),
                _ = async {
                    loop {
                        if wanted.changed().await.is_err() || !*wanted.borrow() {
                            return;
                        }
                    }
                } => {
                    if let Some(pid) = child.id() {
                        // SAFETY: a signal to our own child.
                        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    None
                }
            };
            *self.pid.lock().unwrap() = None;
            if self.asked_to_go.swap(false, Ordering::SeqCst) || !*wanted.borrow() {
                continue;
            }
            eprintln!("own-server: the engine stopped ({status:?}); starting it again in {} s", pause.as_secs());
            if started.elapsed() > Duration::from_secs(60) {
                pause = Duration::from_secs(1);
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(30));
        }
    }
}

/// In the engine's own process: never outlive the program that started it — on Linux the kernel's word
/// (`PR_SET_PDEATHSIG`), everywhere a look at the parent every 2 s.
pub fn guard_orphan() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain prctl with a signal number.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            // SAFETY: as above.
            let now = unsafe { libc::getppid() };
            if now != parent || now == 1 {
                std::process::exit(0);
            }
        }
    });
}
