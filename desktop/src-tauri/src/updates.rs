//! Updates through tauri-plugin-updater, off until there is a release feed: tauri.conf.json names a placeholder
//! endpoint on .invalid (a name that never resolves) and a placeholder key. With a real feed and key (README,
//! «Обновления») the app checks half a minute after start and then every hour, as the gateway inside does, and offers
//! the update in the tray menu; each new version is also told once in a macOS notification.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Url};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::engine::{self, Gateway};
use crate::Tray;

pub const MENU_ID: &str = "update";

/// The first check waits for the gateway to come up; then one every this often (the gateway's UPDATE_CHECK_HOURS).
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
/// The last version told in a notification: a restart of the app does not tell it again.
const NOTIFIED_FILE: &str = "update-notified";

#[derive(Default)]
struct Pending {
    update: Mutex<Option<Update>>,
    /// The tray's «Обновить до X», once there is one: a newer version renames it rather than adding a second.
    item: Mutex<Option<MenuItem<tauri::Wry>>>,
}

pub fn check(app: &AppHandle) {
    if !feed_configured(app) {
        return;
    }
    app.manage(Pending::default());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let found = match app.updater() {
                Ok(updater) => updater.check().await,
                Err(error) => Err(error),
            };
            match found {
                Ok(Some(update)) => offer(&app, update),
                Ok(None) => {}
                Err(error) => app.state::<Arc<Gateway>>().note(&format!("update check: {error}")),
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The feed for the engine: the app's plain endpoint (latest.json) of a real host — the engine itself asks the
/// versioned address of the app's version first (the gateway's updates), so the panel and the tray offer the same
/// version.
pub fn feed_url(app: &AppHandle) -> Option<String> {
    if !feed_configured(app) {
        return None;
    }
    let endpoints = app
        .config()
        .plugins
        .0
        .get("updater")?
        .get("endpoints")?
        .as_array()?
        .clone();
    endpoints
        .iter()
        .filter_map(|endpoint| endpoint.as_str())
        .find(|url| !url.contains("{{"))
        .map(str::to_string)
}

/// The engine's updates through the app's updater: the app is what gets installed, and it restarts as the new one.
pub struct AppInstaller(pub AppHandle);

impl duowave::updates::Installer for AppInstaller {
    fn install(
        &self,
        version: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>> {
        let app = self.0.clone();
        Box::pin(async move {
            let updater = app.updater().map_err(|error| error.to_string())?;
            let update = updater
                .check()
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("лента не предлагает приложение {version}"))?;
            let gateway = app.state::<Arc<Gateway>>().inner().clone();
            gateway.note(&format!("installing the app {} (offered {version})", update.version));
            // Downloaded and its signature checked while the gateway serves. Then, as for the tray's update, the
            // gateway stops first (on Windows the installer ends the app at once) and the new version starts its own.
            let bytes = update
                .download(|_, _| {}, || {})
                .await
                .map_err(|error| error.to_string())?;
            let stopping = gateway.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || stopping.stop(engine::STOP_GRACE)).await;
            match update.install(bytes) {
                Ok(()) => {
                    relaunch(&app);
                    Ok(())
                }
                Err(error) => {
                    gateway.note(&format!("update failed: {error}"));
                    std::thread::spawn(move || gateway.start());
                    Err(error.to_string())
                }
            }
        })
    }

    /// The app's version: the feed is the app's, so the gateway inside compares it with this, not its own.
    fn version(&self) -> Option<String> {
        Some(self.0.package_info().version.to_string())
    }
}

/// A feed is configured when every endpoint names a real host: the shipped placeholder is on .invalid.
fn feed_configured(app: &AppHandle) -> bool {
    let endpoints = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("endpoints"))
        .and_then(|endpoints| endpoints.as_array())
        .cloned()
        .unwrap_or_default();
    !endpoints.is_empty()
        && endpoints.iter().all(|endpoint| {
            endpoint
                .as_str()
                .and_then(|url| Url::parse(url).ok())
                .and_then(|url| url.host_str().map(|host| !host.ends_with(".invalid")))
                .unwrap_or(false)
        })
}

fn offer(app: &AppHandle, update: Update) {
    let version = update.version.clone();
    let label = format!("Обновить до {version}");
    *app.state::<Pending>().update.lock().unwrap() = Some(update);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let pending = handle.state::<Pending>();
        let mut item = pending.item.lock().unwrap();
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(&label);
            return;
        }
        let Some(tray) = handle.try_state::<Tray>() else { return };
        if let Ok(created) = MenuItem::with_id(&handle, MENU_ID, label, true, None::<&str>) {
            let _ = tray.menu.insert(&created, 0);
            *item = Some(created);
        }
    });
    notify_once(app, &version);
}

/// One macOS notification per new version: the tray item is easy to miss, the menu stays closed until opened.
fn notify_once(app: &AppHandle, version: &str) {
    let gateway = app.state::<Arc<Gateway>>().inner().clone();
    let file = notified_file(&gateway.paths().data);
    if !first_time(&file, version) {
        return;
    }
    let shown = app
        .notification()
        .builder()
        .title(format!("Доступна версия {version}"))
        .body("Обновить можно в меню значка в трее или в пульте: Настройки → Обновления и копии.")
        .show();
    match shown {
        Ok(()) => {
            let _ = std::fs::write(&file, version);
        }
        Err(error) => gateway.note(&format!("update notification: {error}")),
    }
}

fn notified_file(data: &std::path::Path) -> PathBuf {
    data.join(NOTIFIED_FILE)
}

/// This version has not been told yet.
fn first_time(file: &std::path::Path, version: &str) -> bool {
    std::fs::read_to_string(file)
        .map(|told| told.trim() != version)
        .unwrap_or(true)
}

/// Starts the version now on disk. On macOS through LaunchServices (`open` on the bundle), once this process is gone:
/// app.restart() runs the program inside the bundle directly, and the Dock then shows the new one as a second icon
/// beside the one kept in the Dock (seen updating to 0.1.2). Elsewhere, or when the bundle is not found, a restart.
fn relaunch(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let bundle = std::env::current_exe()
            .ok()
            .and_then(|program| program.ancestors().nth(3).map(std::path::Path::to_path_buf))
            .filter(|bundle| bundle.extension().is_some_and(|extension| extension == "app"));
        if let Some(bundle) = bundle {
            // `open` on a running app only brings it forward: wait for this process to end first.
            let script = format!(
                "while kill -0 {} 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$0\"",
                std::process::id()
            );
            let spawned = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .arg(&bundle)
                .spawn();
            if spawned.is_ok() {
                app.exit(0);
                return;
            }
        }
    }
    app.restart();
}

pub fn install(app: &AppHandle) {
    let Some(update) = app
        .try_state::<Pending>()
        .and_then(|pending| pending.update.lock().unwrap().take())
    else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let gateway = app.state::<Arc<Gateway>>().inner().clone();
        // The new version starts its own gateway: this one stops first and frees the port.
        let stopping = gateway.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || stopping.stop(engine::STOP_GRACE)).await;
        match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => relaunch(&app),
            Err(error) => {
                gateway.note(&format!("update failed: {error}"));
                std::thread::spawn(move || gateway.start());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_each_version_once_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let file = notified_file(dir.path());
        assert!(first_time(&file, "0.2.0"));
        std::fs::write(&file, "0.2.0").unwrap();
        assert!(!first_time(&file, "0.2.0"));
        assert!(first_time(&file, "0.3.0"));
    }
}
