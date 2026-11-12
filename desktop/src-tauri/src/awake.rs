//! Keeps the computer from sleeping while the gateway runs: asleep, it leaves the station without its gateway (the
//! owner's Mac runs the gateway under `caffeinate -is` for this). The display may sleep, the system may not.
//! Held per gateway run, released when the run ends. On macOS and Linux the helper also ends with the gateway's
//! own process, so a crashed app leaves nothing held.

use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::Mutex;

/// The app's own settings, beside the gateway's in the data folder.
const SETTINGS: &str = "desktop.json";

pub struct Awake(Mutex<Option<Hold>>);

enum Hold {
    #[cfg(unix)]
    Helper(std::process::Child),
    #[cfg(windows)]
    Thread(std::sync::mpsc::Sender<()>),
}

impl Awake {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Holds for the gateway with this pid; a hold already there stays.
    pub fn hold(&self, pid: u32) -> Result<(), String> {
        let mut hold = self.0.lock().unwrap();
        if hold.is_none() {
            *hold = Some(acquire(pid)?);
        }
        Ok(())
    }

    pub fn release(&self) {
        if let Some(hold) = self.0.lock().unwrap().take() {
            let _ = release(hold);
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(target_os = "macos")]
fn acquire(pid: u32) -> Result<Hold, String> {
    use std::os::unix::process::CommandExt;
    // -i: no idle sleep; -s: no sleep at all on mains power; -w: until the gateway's process ends.
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-s", "-w", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(Hold::Helper)
        .map_err(|error| format!("caffeinate: {error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn acquire(pid: u32) -> Result<Hold, String> {
    use std::os::unix::process::CommandExt;
    // logind's inhibitor lock on idle and sleep, held while tail waits for the gateway's process to end.
    std::process::Command::new("systemd-inhibit")
        .args([
            "--what=idle:sleep",
            "--who=DuoWave",
            "--why=Шлюз колонки работает",
            "--mode=block",
            "tail",
            &format!("--pid={pid}"),
            "-f",
            "/dev/null",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own group: the release ends systemd-inhibit and its tail together.
        .process_group(0)
        .spawn()
        .map(Hold::Helper)
        .map_err(|error| format!("systemd-inhibit: {error}"))
}

#[cfg(windows)]
fn acquire(_pid: u32) -> Result<Hold, String> {
    // The execution state belongs to the thread that set it: a thread of its own keeps it until the release.
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
        let _ = released.recv();
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    });
    Ok(Hold::Thread(release))
}

fn release(hold: Hold) -> Result<(), String> {
    match hold {
        #[cfg(unix)]
        Hold::Helper(mut child) => {
            unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGTERM) };
            let _ = child.wait();
            Ok(())
        }
        #[cfg(windows)]
        Hold::Thread(release) => release.send(()).map_err(|error| error.to_string()),
    }
}

#[cfg(windows)]
const ES_CONTINUOUS: u32 = 0x8000_0000;
#[cfg(windows)]
const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetThreadExecutionState(flags: u32) -> u32;
}

/// «Не давать компьютеру засыпать»: on unless switched off once.
pub fn load_setting(data: &Path) -> bool {
    fs::read_to_string(data.join(SETTINGS))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|settings| settings.get("keepAwake").and_then(|value| value.as_bool()))
        .unwrap_or(true)
}

/// Whether `key` was never marked in desktop.json; marks it. For what the app does once in its life (the login
/// item switched on at the first run). Not marked when the file cannot be written: tried again next time.
pub fn first_time(data: &Path, key: &str) -> bool {
    let path = data.join(SETTINGS);
    let mut settings = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|settings| settings.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if settings.get(key).and_then(|value| value.as_bool()) == Some(true) {
        return false;
    }
    settings[key] = serde_json::Value::Bool(true);
    let written = fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&settings).unwrap_or_default()),
    );
    written.is_ok()
}

pub fn save_setting(data: &Path, keep_awake: bool) -> std::io::Result<()> {
    let path = data.join(SETTINGS);
    let mut settings = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|settings| settings.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    settings["keepAwake"] = serde_json::Value::Bool(keep_awake);
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&settings).unwrap_or_default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_time_is_once_and_keeps_the_other_settings() {
        let dir = std::env::temp_dir().join(format!("duowave-first-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        save_setting(&dir, false).unwrap();
        assert!(first_time(&dir, "autostartSet"));
        assert!(!first_time(&dir, "autostartSet"));
        assert!(!load_setting(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_setting_is_on_until_switched_off() {
        let dir = std::env::temp_dir().join(format!("duowave-awake-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(load_setting(&dir));
        save_setting(&dir, false).unwrap();
        assert!(!load_setting(&dir));
        save_setting(&dir, true).unwrap();
        assert!(load_setting(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn caffeinate_ends_with_the_process_it_waits_for() {
        use std::time::{Duration, Instant};
        let mut sleeper = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let awake = Awake::new();
        awake.hold(sleeper.id()).unwrap();
        let Some(Hold::Helper(mut caffeinate)) = awake.0.lock().unwrap().take() else {
            panic!("no hold")
        };
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
        // caffeinate -w polls the process: it ends within a moment of it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while caffeinate.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "caffeinate outlived the process it waits for"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_release_ends_the_helper() {
        let mut sleeper = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let awake = Awake::new();
        // Without systemd-inhibit (a Linux without systemd) there is nothing to release.
        if awake.hold(sleeper.id()).is_ok() {
            // release() waits for the helper: returning at all means it ended.
            awake.release();
            assert!(awake.0.lock().unwrap().is_none());
        }
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
    }
}
