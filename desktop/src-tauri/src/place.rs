//! The window's size and place (the owner, 13.11.2026). The first start opens it at 80 % of the screen's width, in the
//! middle, and the whole height the screen gives windows (above the Dock, under the menu bar); after that it opens
//! where it was left: moved or resized, it is written down a second later, in `window.json` beside the app's
//! settings. A place on a screen that is gone opens as at the first start. Only the app does this: in a browser the
//! panel is a page, and its size is the browser's.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{LogicalSize, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow};

/// The width the first start gives the window, of the screen's.
const FIRST_WIDTH: f64 = 0.8;
/// How much of a saved window must stay on some screen, in points, for it to open there.
const VISIBLE: f64 = 120.0;
/// Moves and resizes come many a second while a person drags: written once they stop for this long.
const SETTLE: Duration = Duration::from_secs(1);

/// Where the window stood: its top-left corner in the screens' pixels, its inner size in points.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

fn file(window: &WebviewWindow) -> Option<PathBuf> {
    window
        .app_handle()
        .path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("window.json"))
}

fn read(path: &Path) -> Option<Place> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A screen that shows enough of the window at `place` for a person to take it by its title.
fn shows(monitors: &[Monitor], place: &Place) -> bool {
    monitors.iter().any(|monitor| {
        let scale = monitor.scale_factor();
        let (left, top) = (monitor.position().x as f64, monitor.position().y as f64);
        let (right, bottom) = (left + monitor.size().width as f64, top + monitor.size().height as f64);
        let (x, y) = (place.x as f64, place.y as f64);
        let (width, visible) = (place.width * scale, VISIBLE * scale);
        x + width >= left + visible && x <= right - visible && y >= top - 1.0 && y <= bottom - visible
    })
}

/// The first start's place on `monitor`: 80 % of its width in the middle, all the height windows get there.
pub fn first(monitor: &Monitor) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let area = monitor.work_area();
    let width = (area.size.width as f64 * FIRST_WIDTH).round() as u32;
    let x = area.position.x + ((area.size.width - width) / 2) as i32;
    (
        PhysicalPosition::new(x, area.position.y),
        PhysicalSize::new(width, area.size.height),
    )
}

/// Puts the hidden window where it was left, or where the first start puts it.
pub fn restore(window: &WebviewWindow) {
    let monitors = window.available_monitors().unwrap_or_default();
    if let Some(place) = file(window)
        .and_then(|path| read(&path))
        .filter(|place| shows(&monitors, place))
    {
        let _ = window.set_size(LogicalSize::new(place.width, place.height));
        let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
        if place.maximized {
            let _ = window.maximize();
        }
        return;
    }
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    match monitor {
        Some(monitor) => {
            let (position, size) = first(&monitor);
            let _ = window.set_size(size);
            let _ = window.set_position(position);
        }
        None => {
            let _ = window.center();
        }
    }
}

fn now(window: &WebviewWindow) -> Option<Place> {
    // Minimized or full screen, it is not where it was left: the place before stays.
    if window.is_minimized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return None;
    }
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Place {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().unwrap_or(false),
    })
}

/// Writes the window's place down a second after it stops moving or changing size.
pub fn keep(window: &WebviewWindow) {
    let Some(path) = file(window) else { return };
    let moves = Arc::new(AtomicU64::new(0));
    let watched = window.clone();
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)) {
            return;
        }
        let this = moves.fetch_add(1, Ordering::SeqCst) + 1;
        let (moves, window, path) = (moves.clone(), watched.clone(), path.clone());
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            if moves.load(Ordering::SeqCst) != this || !window.is_visible().unwrap_or(false) {
                return;
            }
            let Some(place) = now(&window) else { return };
            if read(&path) == Some(place) {
                return;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string(&place) {
                let _ = std::fs::write(&path, text);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(x: i32, y: i32, width: f64, height: f64) -> Place {
        Place {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    #[test]
    fn a_saved_place_reads_back() {
        let dir = std::env::temp_dir().join(format!("duowave-place-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window.json");
        let saved = Place {
            x: -1200,
            y: 40,
            width: 1210.5,
            height: 860.0,
            maximized: true,
        };
        std::fs::write(&path, serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(read(&path), Some(saved));
        std::fs::write(&path, "{\"x\":1,\"y\":2,\"width\":3,\"height\":4}").unwrap();
        assert_eq!(read(&path), Some(place(1, 2, 3.0, 4.0)), "maximized is optional");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read(&path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn with_no_screens_no_place_shows() {
        assert!(!shows(&[], &place(0, 0, 800.0, 600.0)));
    }
}
