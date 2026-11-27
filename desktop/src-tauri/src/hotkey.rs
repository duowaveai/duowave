//! The key «Сказать» from any program (docs/plans/audio-interface-3.md, «К4»): the key chosen in «Настройки» →
//! «Микрофон» (⌥Пробел unless another one was chosen), kept in the gateway (`talk_key`), registered with the system
//! here and again when it changes. Pressed — the computer listens; let go after holding — the thought ends (a
//! walkie-talkie); a short press is a press, as the button «Сказать». A key another program holds is told to the
//! panel («занята»).
//!
//! In the mode «Микрофон», and on a Турбо of a station hub, the key goes to the gateway's voice channel: the computer
//! hears and speaks by itself. In the mode «Колонка» of a computer that is the hub — and wherever the channel is not —
//! it brings the window forward and calls the panel's own microphone, as the key does in the window (the page hears an
//! event; the panel needs no IPC for it).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::engine::Gateway;

/// When the key went down: the hold's length decides at its release.
static PRESSED_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// When a press last counted: one within `DEBOUNCE` of it is the same press (Handy's coordinator).
static COUNTED_AT: Mutex<Option<Instant>> = Mutex::new(None);
const DEBOUNCE: Duration = Duration::from_millis(30);
/// The key registered now, as the gateway names it.
static REGISTERED: Mutex<Option<String>> = Mutex::new(None);

/// The plugin with its handler: every registered key comes here.
pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(
            |app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent| match event.state() {
                ShortcutState::Pressed => pressed(app),
                ShortcutState::Released => released(app),
            },
        )
        .build()
}

/// The channel of the gateway running now, where the computer hears by itself: the mode «Микрофон», or a Турбо of a
/// station hub (`key_to_ear`).
fn channel(app: &AppHandle) -> Option<Arc<duowave::channel::VoiceChannel>> {
    let core = app.state::<Arc<Gateway>>().app()?;
    let channel = core.pc.voice.get()?.clone();
    duowave::computer::voice::key_to_ear(&core).then_some(channel)
}

/// A key down longer than this without its release is taken as lost: the next press is a new one.
const HELD_AT_MOST: Duration = Duration::from_secs(120);

fn pressed(app: &AppHandle) {
    let now = Instant::now();
    {
        let mut counted = COUNTED_AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if counted.is_some_and(|at| now.duration_since(at) < DEBOUNCE) {
            return;
        }
        *counted = Some(now);
    }
    {
        // macOS repeats a hot key's press while it is held (27.11, the trial of main: each repeat set the press's
        // time anew, so a hold of seconds came up as a tap). A press while the key is down is that repeat (Handy's
        // coordinator too): the hold is measured from the first one.
        let mut pressed_at = PRESSED_AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pressed_at.is_some_and(|at| now.duration_since(at) < HELD_AT_MOST) {
            return;
        }
        *pressed_at = Some(now);
    }
    match channel(app) {
        Some(channel) => channel.press_held(),
        None => panel_key(app, "press", 0),
    }
}

fn released(app: &AppHandle) {
    let held = PRESSED_AT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .map_or(0, |at| at.elapsed().as_millis() as u64);
    match channel(app) {
        Some(channel) => channel.release(held),
        None => panel_key(app, "release", held),
    }
}

/// Without the computer's own ear: the window comes forward and the panel's key handler hears the key.
fn panel_key(app: &AppHandle, what: &'static str, held: u64) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if what == "press" {
            crate::show_window(&app);
        }
        if let Some(window) = app.get_webview_window(crate::WINDOW) {
            let script = format!(
                "window.dispatchEvent(new CustomEvent('duowave:talk-key', {{ detail: {{ what: '{what}', heldMs: {held} }} }}))"
            );
            let _ = window.eval(script);
        }
    });
}

/// The key as the gateway names it (`Alt+Space`), as the plugin reads it.
fn shortcut_of(key: &str) -> Option<Shortcut> {
    key.replace("Option", "Alt")
        .replace("Ctrl", "Control")
        .parse::<Shortcut>()
        .ok()
}

/// Registers the key; the old one goes. False — another program holds it, or it does not read.
fn register(app: &AppHandle, key: &str) -> bool {
    let shortcuts = app.global_shortcut();
    let mut registered = REGISTERED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if registered.as_deref() == Some(key) {
        return true;
    }
    if let Some(old) = registered.take().and_then(|old| shortcut_of(&old)) {
        let _ = shortcuts.unregister(old);
    }
    let Some(shortcut) = shortcut_of(key) else { return false };
    match shortcuts.register(shortcut) {
        Ok(()) => {
            *registered = Some(key.to_string());
            true
        }
        Err(error) => {
            app.state::<Arc<Gateway>>()
                .note(&format!("the key «{key}» is not registered: {error}"));
            false
        }
    }
}

/// Follows the engine: the key it keeps is registered at every start and again when it changes.
pub fn follow(app: &AppHandle) {
    let app = app.clone();
    let generation = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicU64::new(0));
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
            let Some(core) = core else { continue };
            // The key the gateway keeps; without the channel, the one its settings name.
            let Some(channel) = core.pc.voice.get().cloned() else {
                let key = core
                    .settings
                    .get(&duowave::settings::definitions::TALK_KEY, None)
                    .unwrap_or_else(|| duowave::settings::definitions::DEFAULT_TALK_KEY.to_string());
                let (app, key) = (app.clone(), key.clone());
                let _ = app.clone().run_on_main_thread(move || {
                    register(&app, &key);
                });
                continue;
            };
            let mut keys = channel.key();
            let (app, generation, failures) = (app.clone(), generation.clone(), failures.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    let key = keys.borrow_and_update().clone();
                    let (registering, told) = (app.clone(), channel.clone());
                    let failures = failures.clone();
                    let _ = app.run_on_main_thread(move || {
                        let ok = register(&registering, &key);
                        if !ok {
                            failures.fetch_add(1, Ordering::Relaxed);
                        }
                        told.set_key_taken(!ok);
                    });
                    if keys.changed().await.is_err() || generation.load(Ordering::SeqCst) != mine {
                        return;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gateway_s_key_names_read_as_shortcuts() {
        for key in [
            "Alt+Space",
            "Control+Alt+K",
            "Option+Space",
            "Ctrl+Shift+1",
            "CommandOrControl+Shift+Space",
        ] {
            assert!(shortcut_of(key).is_some(), "{key}");
        }
        assert!(shortcut_of("Space+Alt+").is_none());
    }
}
