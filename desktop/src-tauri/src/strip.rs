//! The strip over the windows (docs/plans/audio-interface-3.md, «К8»): a small window at the top of the screen, in
//! the middle, over all the windows and on every desktop, without a frame and never taking the focus — the person
//! types on where they are. It shows while the voice channel is not quiet and goes 4 seconds after. Inside is the
//! panel's page `#/voice-strip`: the same strip as the dock's — «Слушаю» with the level, what was heard, the answer,
//! the window's countdown, «Жду продолжения», ×. The same address as the main window, so the same sign-in. A tap on
//! its text asks the gateway for the panel's window (`POST /api/v1/talk/voice/panel`), which brings it here.
//! Switched off in «Настройки» → «Микрофон» («Полоса поверх окон», decision 6).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use duowave::channel::{State, View};
use tauri::{AppHandle, Manager, PhysicalPosition, Url, WebviewUrl, WebviewWindowBuilder};

use crate::engine::Gateway;

const LABEL: &str = "voice-strip";
/// The strip's size, in points.
const SIZE: (f64, f64) = (520.0, 64.0);
/// From the screen's top, in points.
const TOP: f64 = 8.0;
/// It goes this long after the channel is quiet.
const LINGER: Duration = Duration::from_secs(4);

fn page(port: u16) -> Option<Url> {
    Url::parse(&format!("http://127.0.0.1:{port}/panel/#/voice-strip")).ok()
}

/// The strip's window, made the first time: no frame, over the others, on every desktop, not in the Dock's list, not
/// taking the focus. Hidden until it is placed.
fn window(app: &AppHandle, port: u16) -> Option<tauri::WebviewWindow> {
    if let Some(open) = app.get_webview_window(LABEL) {
        return Some(open);
    }
    let url = page(port)?;
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url))
        .title("DuoWave")
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        // It never becomes the key window (`front_without_focus`): the first click is the click, so the × works.
        .accept_first_mouse(true)
        .resizable(false)
        .inner_size(SIZE.0, SIZE.1)
        .visible(false)
        .build();
    let window = match built {
        Ok(window) => window,
        Err(error) => {
            app.state::<Arc<Gateway>>()
                .note(&format!("the voice strip did not open: {error}"));
            return None;
        }
    };
    if let Some(monitor) = window.primary_monitor().ok().flatten() {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let x = area.position.x + ((area.size.width as f64 - SIZE.0 * scale) / 2.0) as i32;
        let y = area.position.y + (TOP * scale) as i32;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    Some(window)
}

fn show(app: &AppHandle, port: u16) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(window) = window(&app, port) {
            front_without_focus(&window);
        }
    });
}

/// The strip comes over every window without becoming the key window or bringing the app forward (27.11, the trial of
/// main: `show()` made it the key window, and macOS brought DuoWave's main window forward with each key press).
#[cfg(target_os = "macos")]
fn front_without_focus(window: &tauri::WebviewWindow) {
    match window.ns_window() {
        Ok(ns_window) if !ns_window.is_null() => {
            // SAFETY: the strip's own NSWindow, alive while the Tauri window is; called on the main thread.
            unsafe {
                let ns_window = &*(ns_window as *const objc2::runtime::AnyObject);
                let _: () = objc2::msg_send![ns_window, orderFrontRegardless];
            }
        }
        _ => {
            let _ = window.show();
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn front_without_focus(window: &tauri::WebviewWindow) {
    let _ = window.show();
}

fn hide(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

/// Follows the engine's channel: the strip comes with a talk and goes after it; the strip's «open the panel» brings
/// the main window.
pub fn follow(app: &AppHandle) {
    let app = app.clone();
    let generation = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        let mut seen: Option<usize> = None;
        loop {
            std::thread::sleep(Duration::from_millis(700));
            let gateway = app.state::<Arc<Gateway>>();
            let core = gateway.app();
            let now = core.as_ref().map(|core| Arc::as_ptr(core) as usize);
            if now == seen {
                continue;
            }
            seen = now;
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            hide(&app);
            let Some(channel) = core.and_then(|core| core.pc.voice.get().cloned()) else {
                continue;
            };
            let port = gateway.status().port;
            let (mut views, mut asked, mut closed) = (channel.watch(), channel.panel_asked(), channel.strip_closed());
            let quiet_since = Arc::new(AtomicU64::new(0));
            let (app, generation) = (app.clone(), generation.clone());
            tauri::async_runtime::spawn(async move {
                // Closed by the person: shown again only by the channel's next change, not by the state it had.
                let mut closed_now = false;
                loop {
                    let view: View = views.borrow_and_update().clone();
                    let on = channel.settings().strip;
                    if closed_now {
                        closed_now = false;
                    } else if view.state != State::Quiet && on {
                        quiet_since.fetch_add(1, Ordering::SeqCst);
                        show(&app, port);
                    } else {
                        // Quiet: it goes a moment later, unless the channel speaks up again before.
                        let mark = quiet_since.fetch_add(1, Ordering::SeqCst) + 1;
                        let (app, quiet_since) = (app.clone(), quiet_since.clone());
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(LINGER).await;
                            if quiet_since.load(Ordering::SeqCst) == mark {
                                hide(&app);
                            }
                        });
                    }
                    tokio::select! {
                        changed = views.changed() => {
                            if changed.is_err() || generation.load(Ordering::SeqCst) != mine {
                                return;
                            }
                        }
                        changed = closed.changed() => {
                            if changed.is_err() || generation.load(Ordering::SeqCst) != mine {
                                return;
                            }
                            quiet_since.fetch_add(1, Ordering::SeqCst);
                            hide(&app);
                            closed_now = true;
                        }
                        changed = asked.changed() => {
                            if changed.is_err() || generation.load(Ordering::SeqCst) != mine {
                                return;
                            }
                            let app = app.clone();
                            let _ = app.clone().run_on_main_thread(move || crate::show_window(&app));
                        }
                    }
                }
            });
        }
    });
}
