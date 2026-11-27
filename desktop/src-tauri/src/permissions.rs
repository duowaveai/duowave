//! «Разрешения» (docs/plans/permissions.md), the app's side: the microphone as macOS sees it for this app and the
//! system's question about it, and the guide window beside System Settings (guide.rs, Mac only). The gateway's
//! `permissions` module builds the list; it asks this port for what only the app can tell.

use std::sync::Arc;

use duowave::computer::permissions::{Asked, Item, State, SystemPort};
use tauri::AppHandle;

pub struct AppPermissions {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    app: AppHandle,
}

impl AppPermissions {
    pub fn new(app: AppHandle) -> Arc<AppPermissions> {
        Arc::new(AppPermissions { app })
    }
}

impl SystemPort for AppPermissions {
    fn microphone(&self) -> State {
        microphone()
    }

    fn ask_microphone(&self) -> Asked {
        Box::pin(ask_microphone())
    }

    fn guide(&self, item: Item, show: bool) {
        #[cfg(target_os = "macos")]
        crate::guide::set(&self.app, item, show);
        #[cfg(not(target_os = "macos"))]
        let _ = (item, show);
    }
}

/// AVAudioApplication is there from macOS 14; on 13 the app cannot tell (the first capture asks there).
#[cfg(target_os = "macos")]
fn microphone() -> State {
    use objc2::runtime::AnyClass;
    use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
    if AnyClass::get(c"AVAudioApplication").is_none() {
        return State::Unknown;
    }
    // SAFETY: the shared instance's read-only property.
    let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
    match permission {
        AVAudioApplicationRecordPermission::Granted => State::Granted,
        AVAudioApplicationRecordPermission::Denied => State::Denied,
        AVAudioApplicationRecordPermission::Undetermined => State::NotAsked,
        _ => State::Unknown,
    }
}

#[cfg(not(target_os = "macos"))]
fn microphone() -> State {
    State::Unknown
}

/// macOS' own window «“DuoWave” запрашивает доступ к микрофону»; the answer once the person clicked. Asked before, it
/// answers at once with what was chosen then.
#[cfg(target_os = "macos")]
async fn ask_microphone() -> State {
    use objc2::runtime::{AnyClass, Bool};
    use objc2_avf_audio::AVAudioApplication;
    if AnyClass::get(c"AVAudioApplication").is_none() {
        return State::Unknown;
    }
    let (send, answer) = tokio::sync::oneshot::channel::<bool>();
    {
        let send = std::sync::Mutex::new(Some(send));
        let block = block2::RcBlock::new(move |granted: Bool| {
            if let Some(send) = send.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
                let _ = send.send(granted.as_bool());
            }
        });
        // SAFETY: a class method with a completion block the system keeps until it calls it.
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block) };
    }
    match answer.await {
        Ok(true) => State::Granted,
        Ok(false) => State::Denied,
        Err(_) => microphone(),
    }
}

/// Windows and Linux: no question of the system's own for a desktop program (the gateway reads Windows' switch).
#[cfg(not(target_os = "macos"))]
async fn ask_microphone() -> State {
    State::Unknown
}
