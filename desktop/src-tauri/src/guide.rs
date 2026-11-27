//! The guide window beside System Settings («Разрешения», docs/plans/permissions.md; Mac only). The panel's «Открыть
//! настройки» opens a Privacy & Security pane, and a small window of the app comes up at the right edge of the System
//! Settings window and follows it while it moves: «Найдите DuoWave в списке и включите переключатель». Inside is the
//! panel's page `#/guide/<item>` — the same address as the main window, so the same sign-in; it watches the item and
//! asks for this window to close once the switch is on (`POST /api/v1/permissions/<item>/guide`).
//!
//! The idea and the way to find the window are PermissionFlow's (github.com/jaywcjlove/PermissionFlow, MIT,
//! © 2026 jaywcjlove): the System Settings window is found in the window server's list (`CGWindowListCopyWindowInfo`),
//! which gives other programs' window bounds with no permission at all. Ours has no drag yet: Local Network and the
//! microphone take none — the app is in their lists already, and the person turns its switch on. The drag of the app's
//! icon comes with dictation (Accessibility).
//!
//! It closes when its item is allowed, when System Settings closes or leaves the screen, or with its ×. When no System
//! Settings window comes up within a few seconds, it does not show at all: the panel's own text says where to go.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::geometry::CGRect;
use core_graphics::window::{kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly};
use duowave::computer::permissions::Item;
use tauri::{AppHandle, LogicalPosition, Manager, Url, WebviewUrl, WebviewWindowBuilder};

use crate::engine::Gateway;

const LABEL: &str = "permission-guide";
/// The guide's size, in points.
const SIZE: (f64, f64) = (300.0, 136.0);
/// Between the System Settings window and the guide, in points.
const GAP: f64 = 12.0;
/// How often the System Settings window is looked for.
const LOOK_EVERY: Duration = Duration::from_millis(100);
/// System Settings opens: the guide waits this long for its window, then gives up.
const FIRST_WAIT: Duration = Duration::from_secs(5);
/// Gone this long (closed, minimized, another desktop): the guide closes. A pane switch hides it for a moment.
const GONE_AFTER: Duration = Duration::from_millis(1500);

/// The item the guide is shown for, and the look's turn: a new `set` ends the last look.
static SHOWN: Mutex<Option<Item>> = Mutex::new(None);
static TURN: AtomicUsize = AtomicUsize::new(0);

fn name_of(item: Item) -> &'static str {
    match item {
        Item::LocalNetwork => "localNetwork",
        Item::Microphone => "microphone",
        Item::Firewall => "firewall",
        Item::NetworkProfile => "networkProfile",
    }
}

/// Shows the guide for an item, or closes it — only if it is that item's (the gateway closes every allowed item's).
pub fn set(app: &AppHandle, item: Item, show: bool) {
    let mut shown = SHOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if show {
        *shown = Some(item);
        let turn = TURN.fetch_add(1, Ordering::SeqCst) + 1;
        let port = app.state::<Arc<Gateway>>().status().port;
        follow(app.clone(), item, port, turn);
    } else if *shown == Some(item) {
        *shown = None;
        TURN.fetch_add(1, Ordering::SeqCst);
        close(app);
    }
}

fn close(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.close();
        }
    });
}

/// Looks for the System Settings window ten times a second and keeps the guide at its side, until a new turn.
fn follow(app: AppHandle, item: Item, port: u16, turn: usize) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut seen: Option<Instant> = None;
        let mut placed: Option<CGRect> = None;
        while TURN.load(Ordering::SeqCst) == turn {
            match settings_window() {
                Some(frame) => {
                    seen = Some(Instant::now());
                    let moved = placed.is_none_or(|last| !same(&last, &frame));
                    if moved {
                        placed = Some(frame);
                        let (app, frame) = (app.clone(), frame);
                        let _ = app.clone().run_on_main_thread(move || place(&app, item, port, &frame));
                    }
                }
                None => {
                    let gone = match seen {
                        Some(at) => at.elapsed() > GONE_AFTER,
                        None => started.elapsed() > FIRST_WAIT,
                    };
                    if gone {
                        let mut shown = SHOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        if TURN.load(Ordering::SeqCst) == turn {
                            *shown = None;
                            TURN.fetch_add(1, Ordering::SeqCst);
                            close(&app);
                        }
                        return;
                    }
                }
            }
            std::thread::sleep(LOOK_EVERY);
        }
    });
}

fn same(a: &CGRect, b: &CGRect) -> bool {
    (a.origin.x - b.origin.x).abs() < 0.5
        && (a.origin.y - b.origin.y).abs() < 0.5
        && (a.size.width - b.size.width).abs() < 0.5
        && (a.size.height - b.size.height).abs() < 0.5
}

/// Where the guide goes: right of the System Settings window, its top a little below the window's title; when the
/// screen ends there, inside the window's bottom right corner.
pub fn spot(frame: &CGRect, screen: (f64, f64, f64, f64)) -> (f64, f64) {
    let (left, top, width, height) = screen;
    let right = frame.origin.x + frame.size.width + GAP;
    let y = (frame.origin.y + 52.0).clamp(top, top + height - SIZE.1);
    if right + SIZE.0 <= left + width {
        return (right, y);
    }
    let inside_x = (frame.origin.x + frame.size.width - SIZE.0 - 16.0).max(left);
    let inside_y = (frame.origin.y + frame.size.height - SIZE.1 - 16.0).clamp(top, top + height - SIZE.1);
    (inside_x, inside_y)
}

fn place(app: &AppHandle, item: Item, port: u16, frame: &CGRect) {
    let Some(window) = window(app, item, port) else { return };
    // The screen the System Settings window is on, in points from the main screen's top left — as the window server's
    // bounds are.
    let screen = app
        .monitor_from_point(
            frame.origin.x + frame.size.width / 2.0,
            frame.origin.y + frame.size.height / 2.0,
        )
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            (
                f64::from(area.position.x) / scale,
                f64::from(area.position.y) / scale,
                f64::from(area.size.width) / scale,
                f64::from(area.size.height) / scale,
            )
        })
        .unwrap_or((0.0, 0.0, f64::MAX, f64::MAX));
    let (x, y) = spot(frame, screen);
    let _ = window.set_position(LogicalPosition::new(x, y));
    front_without_focus(&window);
}

/// The guide's window, made the first time: no frame, over the others, not in the Dock's list, not taking the focus.
fn window(app: &AppHandle, item: Item, port: u16) -> Option<tauri::WebviewWindow> {
    let address = format!("http://127.0.0.1:{port}/panel/#/guide/{}", name_of(item));
    if let Some(open) = app.get_webview_window(LABEL) {
        let wanted = Url::parse(&address).ok()?;
        if open.url().ok().as_ref() != Some(&wanted) {
            let _ = open.navigate(wanted);
        }
        return Some(open);
    }
    let url = Url::parse(&address).ok()?;
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url))
        .title("DuoWave")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .accept_first_mouse(true)
        .resizable(false)
        .inner_size(SIZE.0, SIZE.1)
        .visible(false)
        .build();
    match built {
        Ok(window) => Some(window),
        Err(error) => {
            app.state::<Arc<Gateway>>()
                .note(&format!("the permission guide did not open: {error}"));
            None
        }
    }
}

/// Over System Settings without becoming the key window: the person's clicks stay in System Settings (strip.rs).
fn front_without_focus(window: &tauri::WebviewWindow) {
    match window.ns_window() {
        Ok(ns_window) if !ns_window.is_null() => {
            // SAFETY: the guide's own NSWindow, alive while the Tauri window is; called on the main thread.
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

/// The System Settings window on screen — the largest normal window of the System Settings program — in points from
/// the main screen's top left; None when it is not on screen.
fn settings_window() -> Option<CGRect> {
    let windows = core_graphics::window::copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    let mut best: Option<CGRect> = None;
    for entry in windows.get_all_values() {
        // SAFETY: each value of the window list is a CFDictionary, alive while the list is.
        let entry: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(entry as CFDictionaryRef) };
        let number = |key: &'static str| -> Option<i64> {
            entry
                .find(CFString::from_static_string(key))?
                .downcast::<CFNumber>()?
                .to_i64()
        };
        if number("kCGWindowLayer") != Some(0) {
            continue;
        }
        let Some(pid) = number("kCGWindowOwnerPID") else {
            continue;
        };
        if !is_system_settings(pid as i32) {
            continue;
        }
        let Some(bounds) = entry.find(CFString::from_static_string("kCGWindowBounds")) else {
            continue;
        };
        let Some(bounds) = bounds.downcast::<CFDictionary>() else {
            continue;
        };
        let Some(frame) = CGRect::from_dict_representation(&bounds) else {
            continue;
        };
        if frame.size.width < 200.0 || frame.size.height < 200.0 {
            continue;
        }
        let larger = best.is_none_or(|best| frame.size.width * frame.size.height > best.size.width * best.size.height);
        if larger {
            best = Some(frame);
        }
    }
    best
}

/// Whether the process is System Settings (System Preferences before macOS 13), by its program's path — its window's
/// owner name is localized.
fn is_system_settings(pid: i32) -> bool {
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as long as said; the call writes at most that and returns the length.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if length <= 0 {
        return false;
    }
    let path = String::from_utf8_lossy(&path[..length as usize]);
    path.contains("/System Settings.app/") || path.contains("/System Preferences.app/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::geometry::{CGPoint, CGSize};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(width, height))
    }

    #[test]
    fn the_guide_stands_right_of_system_settings() {
        let (x, y) = spot(&rect(200.0, 100.0, 700.0, 600.0), (0.0, 25.0, 1728.0, 1080.0));
        assert_eq!((x, y), (200.0 + 700.0 + GAP, 152.0));
    }

    #[test]
    fn with_no_room_on_the_right_it_goes_inside_the_corner() {
        let (x, y) = spot(&rect(1000.0, 100.0, 700.0, 600.0), (0.0, 25.0, 1728.0, 1080.0));
        assert_eq!(x, 1000.0 + 700.0 - SIZE.0 - 16.0);
        assert_eq!(y, 100.0 + 600.0 - SIZE.1 - 16.0);
    }

    #[test]
    fn it_never_leaves_the_screen_at_the_top_or_bottom() {
        let (_, y) = spot(&rect(100.0, 1000.0, 700.0, 600.0), (0.0, 25.0, 1728.0, 1080.0));
        assert!(y + SIZE.1 <= 25.0 + 1080.0);
    }
}
