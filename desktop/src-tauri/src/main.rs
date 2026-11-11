// The desktop app: the gateway (the Rust engine, in this process), its panel in a window, a tray icon to run it all.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod awake;
mod engine;
mod envfile;
mod updates;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use engine::{Gateway, Paths, Phase, Status};
use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry};
#[cfg(target_os = "macos")]
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt;

/// Passed by the login item: the app starts in the tray, without the window.
const AUTOSTART_ARG: &str = "--autostart";
/// Another data folder (a test, a portable copy); the log then goes to its logs/.
const DATA_DIR_ENV: &str = "DUOWAVE_DATA_DIR";
const WINDOW: &str = "main";
/// The splash page while the gateway is not up (the app's own files).
const SPLASH_URL: &str = if cfg!(windows) {
    "http://tauri.localhost/index.html"
} else {
    "tauri://localhost/index.html"
};

static QUITTING: AtomicBool = AtomicBool::new(false);

struct Tray {
    icon: TrayIcon<Wry>,
    menu: Menu<Wry>,
    status: MenuItem<Wry>,
    autostart: CheckMenuItem<Wry>,
    keep_awake: CheckMenuItem<Wry>,
}

fn main() {
    let context = tauri::generate_context!();
    // Set before any thread starts. The engine reads this process's own environment for what it passes to the
    // programs it starts (its model servers, the voice lab: DATA_DIR and the .env there) and for who manages it, as
    // the Node sidecar had both in its environment.
    let data = data_dir(&context.config().identifier);
    if let Some(data) = &data {
        std::env::set_var("DATA_DIR", data);
    }
    std::env::set_var(engine::MANAGED_ENV, "desktop");
    #[cfg(unix)]
    signals::install();
    // Starting at login: a launch agent on a Mac (the plugin's macOS-only choice), its own way on Windows and Linux.
    let autostart = tauri_plugin_autostart::Builder::new().args([AUTOSTART_ARG]);
    #[cfg(target_os = "macos")]
    let autostart = autostart.macos_launcher(MacosLauncher::LaunchAgent);
    let app = tauri::Builder::default()
        // First: a second launch only brings the window of the first one forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(autostart.build())
        // Links are opened by on_new_window below: the plugin's own click script needs an IPC permission the
        // remote panel does not get.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        // «Доступна версия X»: one notification per new version (updates.rs).
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            gateway_status,
            gateway_restart,
            gateway_free_port,
            open_log,
            open_data_dir
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            #[cfg(unix)]
            signals::attach(&handle);
            let paths = resolve_paths(&handle, data)?;
            // The engine finds what ships beside it (clone-server/, the voice lab's code) in its working folder, as
            // the Node gateway did in its bundle — and never a working copy's, whose .env is someone's keys.
            if let Ok(resources) = handle.path().resource_dir() {
                let _ = std::env::set_current_dir(resources);
            }
            std::fs::create_dir_all(&paths.data)?;
            let first_env = envfile::ensure(&paths.data.join(".env"))?;
            let keep_awake = awake::load_setting(&paths.data);
            let gateway = Arc::new(Gateway::new(paths, keep_awake));
            if first_env {
                gateway.note("wrote the first .env (PORT=7780)");
            }
            app.manage(gateway.clone());
            // The engine's «Обновить» in the panel installs through the app's own updater, from the app's feed.
            gateway.set_feed_url(updates::feed_url(&handle));
            gateway.set_update_installer(Arc::new(updates::AppInstaller(handle.clone())));

            build_window(&handle)?;
            build_tray(&handle)?;
            let listener = handle.clone();
            gateway.on_change(move |status| status_changed(&listener, status.clone()));
            // «Не давать компьютеру спать» из пульта помощника (docs/plans/desk.md, этап 4): пульт пишет ту же
            // настройку (desktop.json), приложение перечитывает её и ставит своё меню так же.
            {
                let (gateway, handle) = (gateway.clone(), handle.clone());
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let want = awake::load_setting(&gateway.paths().data);
                    if want != gateway.keep_awake() {
                        gateway.set_keep_awake(want);
                        if let Some(tray) = handle.try_state::<Tray>() {
                            let _ = tray.keep_awake.set_checked(want);
                        }
                    }
                });
            }
            std::thread::spawn(move || gateway.start());

            if !std::env::args().any(|arg| arg == AUTOSTART_ARG) {
                show_window(&handle);
            }
            updates::check(&handle);
            Ok(())
        })
        .build(context)
        .expect("the app could not start");

    app.run(|app, event| match event {
        // Every way out ends here, Cmd+Q and logging out included: the gateway is stopped before the app goes.
        RunEvent::Exit => {
            if let Some(gateway) = app.try_state::<Arc<Gateway>>() {
                gateway.stop(Duration::from_secs(4));
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_window(app),
        _ => {}
    });
}

/// The data folder: DUOWAVE_DATA_DIR when given (made absolute: the app changes its working folder), else what
/// Tauri's app_data_dir is — the system's data folder and the app's identifier — known here before Tauri starts.
fn data_dir(identifier: &str) -> Option<PathBuf> {
    match std::env::var_os(DATA_DIR_ENV).filter(|dir| !dir.is_empty()) {
        Some(dir) => std::path::absolute(PathBuf::from(dir)).ok(),
        None => dirs::data_dir().map(|dir| dir.join(identifier)),
    }
}

fn resolve_paths(app: &AppHandle, data: Option<PathBuf>) -> tauri::Result<Paths> {
    let custom = std::env::var_os(DATA_DIR_ENV).is_some_and(|dir| !dir.is_empty());
    let data = match data {
        Some(data) => data,
        None => app.path().app_data_dir()?,
    };
    let logs = if custom {
        data.join("logs")
    } else {
        app.path().app_log_dir()?
    };
    Ok(Paths {
        data,
        log: logs.join("gateway.log"),
    })
}

/// What the window shows, as its page loads say. WKWebView has no address until its first page starts loading, and
/// asking it then crashed the app: wry's `url()` unwraps it (11.11.2026, 0.4.0 — the gateway answered half a second
/// after the start, before the splash had begun to load). None: nothing has started loading yet, the splash is on
/// its way.
#[derive(Default)]
struct Shown(Mutex<Option<Url>>);

fn build_window(app: &AppHandle) -> tauri::Result<()> {
    let opener = app.clone();
    app.manage(Shown::default());
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
        .title("DuoWave")
        .inner_size(1180.0, 820.0)
        .min_inner_size(400.0, 560.0)
        .center()
        .visible(false)
        // The panel opens other sites (API keys, MCP servers) in a new tab: they go to the browser. The blank
        // window the MCP sign-in opens first is refused, and the panel then signs in within this window.
        .on_new_window(move |url, _features| {
            if matches!(url.scheme(), "http" | "https" | "mailto") {
                let _ = opener.opener().open_url(url.as_str(), None::<&str>);
            }
            NewWindowResponse::Deny
        })
        .on_page_load(|window, payload| {
            if matches!(payload.event(), PageLoadEvent::Started) {
                if let Some(shown) = window.try_state::<Shown>() {
                    *shown.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(payload.url().clone());
                }
            }
        })
        .build()?;
    let hide = window.clone();
    window.on_window_event(move |event| {
        // Closing the window hides it: the gateway keeps running, the tray brings the window back.
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide.hide();
        }
    });
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть пульт", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Шлюз запускается…", false, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "Перезапустить шлюз", true, None::<&str>)?;
    let at_login = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, "autostart", "Запускать при входе", true, at_login, None::<&str>)?;
    let awake = app.state::<Arc<Gateway>>().keep_awake();
    let keep_awake = CheckMenuItem::with_id(
        app,
        "keep-awake",
        "Не давать компьютеру засыпать",
        true,
        awake,
        None::<&str>,
    )?;
    let data = MenuItem::with_id(app, "data", "Папка с данными", true, None::<&str>)?;
    let log = MenuItem::with_id(app, "log", "Журнал", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &restart,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &keep_awake,
            &data,
            &log,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // macOS draws a template image in the menu bar's own colour; elsewhere the app's icon.
    #[cfg(target_os = "macos")]
    let image = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
    #[cfg(not(target_os = "macos"))]
    let image = app.default_window_icon().cloned().expect("the bundle has an icon");

    let icon = TrayIconBuilder::with_id("main")
        .icon(image)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("DuoWave")
        .menu(&menu)
        // Windows: a click opens the panel, the menu is on the right button. macOS: the menu on a click.
        .show_menu_on_left_click(!cfg!(windows))
        .on_menu_event(|app, event| menu_event(app, event.id().as_ref()))
        .on_tray_icon_event(|_tray, _event| {
            #[cfg(windows)]
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = _event
            {
                show_window(_tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(Tray {
        icon,
        menu,
        status,
        autostart,
        keep_awake,
    });
    Ok(())
}

fn menu_event(app: &AppHandle, id: &str) {
    let gateway = app.state::<Arc<Gateway>>().inner().clone();
    match id {
        "open" => show_window(app),
        "restart" => {
            std::thread::spawn(move || gateway.restart());
        }
        "autostart" => toggle_autostart(app),
        "keep-awake" => toggle_keep_awake(app, &gateway),
        "data" => open_data_dir(app.clone()),
        "log" => open_log(app.clone()),
        "quit" => quit(app),
        updates::MENU_ID => updates::install(app),
        _ => {}
    }
}

fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    let status = app.state::<Arc<Gateway>>().status();
    if status.phase == Phase::Running && status.responding && on_splash(&window) {
        navigate(&window, &panel_url(status.port));
    }
}

/// From the gateway's threads: the UI is changed on the main thread, without waiting for it (it may be waiting
/// for the gateway to stop).
fn status_changed(app: &AppHandle, status: Status) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let title = status.title();
        if let Some(tray) = handle.try_state::<Tray>() {
            let _ = tray.status.set_text(&title);
            let _ = tray.icon.set_tooltip(Some(format!("DuoWave — {title}")));
        }
        let Some(window) = handle.get_webview_window(WINDOW) else {
            return;
        };
        match status.phase {
            Phase::Running if status.responding && on_splash(&window) => navigate(&window, &panel_url(status.port)),
            // Without a gateway the panel has nothing to show: the splash says why. Other sites the panel led to
            // (a sign-in) are left alone.
            Phase::Starting | Phase::Stopping | Phase::Stopped | Phase::Failed if on_gateway(&window, status.port) => {
                navigate(&window, SPLASH_URL)
            }
            _ => {}
        }
    });
}

fn panel_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/panel")
}

/// What the window shows (`Shown`), never asked of the webview itself; None before its first page starts loading.
fn shown(window: &WebviewWindow) -> Option<Url> {
    let state = window.try_state::<Shown>()?;
    let url = state.0.lock().ok()?.clone();
    url
}

fn on_splash(window: &WebviewWindow) -> bool {
    // Nothing loading yet is the splash on its way: the window opens with it.
    shown(window).is_none_or(|url| url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost"))
}

fn on_gateway(window: &WebviewWindow, port: u16) -> bool {
    shown(window).is_some_and(|url| url.host_str() == Some("127.0.0.1") && url.port_or_known_default() == Some(port))
}

fn navigate(window: &WebviewWindow, url: &str) {
    if let Ok(url) = Url::parse(url) {
        let _ = window.navigate(url);
    }
}

fn toggle_autostart(app: &AppHandle) {
    let manager = app.autolaunch();
    let enabled = manager.is_enabled().unwrap_or(false);
    let result = if enabled { manager.disable() } else { manager.enable() };
    if let Err(error) = result {
        app.state::<Arc<Gateway>>().note(&format!("autostart: {error}"));
    }
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.autostart.set_checked(manager.is_enabled().unwrap_or(false));
    }
}

fn toggle_keep_awake(app: &AppHandle, gateway: &Gateway) {
    let on = !gateway.keep_awake();
    gateway.set_keep_awake(on);
    if let Err(error) = awake::save_setting(&gateway.paths().data, on) {
        gateway.note(&format!("the keep-awake setting was not saved: {error}"));
    }
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.keep_awake.set_checked(on);
    }
}

fn quit(app: &AppHandle) {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        app.state::<Arc<Gateway>>().stop(engine::STOP_GRACE);
        app.exit(0);
    });
}

/// A `kill`, Ctrl+C in a terminal, the end of a Linux session: the gateway is stopped first, as by «Выход».
#[cfg(unix)]
mod signals {
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;

    use tauri::AppHandle;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// Blocks the signals in every thread made after this one — the engine's among them — and takes them on a thread
    /// of its own. The programs the engine starts get a clean mask: std resets it when it spawns.
    pub fn install() {
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::sigaddset(&mut set, signal);
            }
            if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
                return;
            }
            std::thread::spawn(move || loop {
                let mut signal = 0;
                if libc::sigwait(&set, &mut signal) != 0 {
                    continue;
                }
                match APP.get() {
                    // A second one while stopping: out at once.
                    Some(_) if super::QUITTING.load(Ordering::SeqCst) => std::process::exit(1),
                    Some(app) => {
                        use tauri::Manager;
                        if let Some(gateway) = app.try_state::<std::sync::Arc<super::Gateway>>() {
                            gateway.note(&format!("signal {signal}: quitting"));
                        }
                        super::quit(app)
                    }
                    None => std::process::exit(0),
                }
            });
        }
    }

    pub fn attach(app: &AppHandle) {
        let _ = APP.set(app.clone());
    }
}

/// What the splash page shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    phase: Phase,
    title: String,
    advice: Option<String>,
    panel_url: Option<String>,
    /// The port was taken: the window offers «Взять свободный порт».
    port_busy: bool,
}

#[tauri::command]
fn gateway_status(gateway: tauri::State<'_, Arc<Gateway>>) -> StatusView {
    let status = gateway.status();
    StatusView {
        phase: status.phase,
        title: status.title(),
        advice: status.problem.as_ref().map(|problem| problem.advice.clone()),
        panel_url: (status.phase == Phase::Running && status.responding).then(|| panel_url(status.port)),
        port_busy: status.phase == Phase::Failed && status.port_busy,
    }
}

/// «Взять свободный порт»: the gateway moves to a free port (written into .env) and starts there.
#[tauri::command]
fn gateway_free_port(gateway: tauri::State<'_, Arc<Gateway>>) {
    let gateway = gateway.inner().clone();
    std::thread::spawn(move || {
        if let Err(problem) = gateway.take_free_port() {
            gateway.note(&format!("no free port taken: {problem}"));
        }
    });
}

#[tauri::command]
fn gateway_restart(gateway: tauri::State<'_, Arc<Gateway>>) {
    let gateway = gateway.inner().clone();
    std::thread::spawn(move || gateway.restart());
}

#[tauri::command]
fn open_log(app: AppHandle) {
    let gateway = app.state::<Arc<Gateway>>();
    let log = &gateway.paths().log;
    // Before the first start there is no file yet: its folder.
    let target = if log.exists() {
        log.clone()
    } else {
        log.parent().map(PathBuf::from).unwrap_or_default()
    };
    let _ = std::fs::create_dir_all(log.parent().unwrap_or(log));
    let _ = app.opener().open_path(target.to_string_lossy(), None::<&str>);
}

#[tauri::command]
fn open_data_dir(app: AppHandle) {
    let gateway = app.state::<Arc<Gateway>>();
    let _ = app
        .opener()
        .open_path(gateway.paths().data.to_string_lossy(), None::<&str>);
}
