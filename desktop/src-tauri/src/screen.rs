//! The computer's screen (gateway/src/screen, docs/plans/shared-folders.md, «Экран»): a window of its own for each window
//! the gateway lists — a photo, a video, a sound — over all the others, even when DuoWave's own window is hidden in the
//! tray; closed when the list loses it. Its cross closes it in the list too. A window is the panel's own page
//! `#/screen/<id>` on this computer's gateway: the same origin as the main window, so the same sign-in.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use duowave::screen::{Kind, Window};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::engine::Gateway;

const PREFIX: &str = "screen-";

fn label(id: &str) -> String {
    format!("{PREFIX}{id}")
}

/// Follows the engine's screen: at every start of the engine it says the windows open here and watches its list.
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
            // Another engine, or none: the watch of the one before stops, its windows go.
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let port = gateway.status().port;
            let Some(core) = core else {
                show(&app, &[], port);
                continue;
            };
            core.screen.set_available(true);
            let mut changes = core.screen.subscribe();
            let (app, generation) = (app.clone(), generation.clone());
            tauri::async_runtime::spawn(async move {
                let first = changes.borrow_and_update().clone();
                show(&app, &first, port);
                while changes.changed().await.is_ok() {
                    if generation.load(Ordering::SeqCst) != mine {
                        return;
                    }
                    let windows = changes.borrow_and_update().clone();
                    show(&app, &windows, port);
                }
            });
        }
    });
}

/// The app's windows as the list says: the ones it lost close, new ones open, cascaded.
fn show(app: &AppHandle, windows: &[Window], port: u16) {
    let wanted: HashSet<String> = windows.iter().map(|window| label(&window.id)).collect();
    for (name, open) in app.webview_windows() {
        if name.starts_with(PREFIX) && !wanted.contains(&name) {
            let _ = open.destroy();
        }
    }
    for (at, window) in windows.iter().enumerate() {
        let name = label(&window.id);
        if app.get_webview_window(&name).is_some() {
            continue;
        }
        let Ok(url) = format!("http://127.0.0.1:{port}/panel/#/screen/{}", window.id).parse::<Url>() else {
            continue;
        };
        let (width, height) = match window.kind {
            Kind::Image => (900.0, 680.0),
            Kind::Video => (960.0, 600.0),
            Kind::Audio => (460.0, 240.0),
        };
        let step = 36.0 * (at % 8) as f64;
        let built = WebviewWindowBuilder::new(app, &name, WebviewUrl::External(url))
            .title(&window.title)
            .inner_size(width, height)
            .min_inner_size(300.0, 200.0)
            .position(120.0 + step, 90.0 + step)
            .always_on_top(true)
            .focused(true)
            .build();
        match built {
            Ok(opened) => {
                let (closing, id) = (app.clone(), window.id.clone());
                opened.on_window_event(move |event| {
                    // Its cross: closed in the list too (the gateway's own closing destroys it without this).
                    if let WindowEvent::CloseRequested { .. } = event {
                        if let Some(core) = closing.state::<Arc<Gateway>>().app() {
                            let _ = core.screen.close(&id);
                        }
                    }
                });
            }
            Err(error) => {
                app.state::<Arc<Gateway>>()
                    .note(&format!("a window of the screen did not open: {error}"));
            }
        }
    }
}
