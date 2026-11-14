//! The screen on the bus: `device/screen`, a device of the computer like the MIDI keyboard (src/bus/providers/devices.rs).
//! An app reaches it by its links (`call:device/screen` in its manifest): «Папки» opens a photo of the home folder in a
//! window, later any app its own content. What it shows is a link the gateway made (Files' `media`), never an address
//! of the internet; the windows themselves are the DuoWave app's (desktop/src-tauri/src/screen.rs).
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use super::Screen;
use crate::bus::provider::{BusError, Handle};
use crate::bus::providers::devices::Device;
use crate::bus::Level;

pub struct ScreenDevice {
    screen: Arc<Screen>,
}

impl ScreenDevice {
    pub fn new(screen: Arc<Screen>) -> ScreenDevice {
        ScreenDevice { screen }
    }
}

impl Device for ScreenDevice {
    fn name(&self) -> &'static str {
        "screen"
    }

    fn title(&self) -> String {
        crate::tr!("Экран компьютера")
    }

    fn handles(&self) -> Vec<Handle> {
        vec![
            Handle::new("windows", Level::See).titled(&crate::tr!("какие окна открыты")),
            Handle::new("open", Level::Call).titled(&crate::tr!("показать в окне")),
            Handle::new("keep", Level::Call).titled(&crate::tr!("оставить окно")),
            Handle::new("close", Level::Call).titled(&crate::tr!("закрыть окно")),
            Handle::new("pause", Level::Call).titled(&crate::tr!("пауза")),
            Handle::new("resume", Level::Call).titled(&crate::tr!("продолжить")),
        ]
    }

    /// Only where the DuoWave app opens the windows.
    fn available(&self) -> bool {
        self.screen.available()
    }

    fn call(&self, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        let answer = self.screen.letter(handle, &input).map_err(BusError::Failed);
        Box::pin(async move { answer })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::bus::address::Address;
    use crate::bus::provider::{Letter, Provider};
    use crate::bus::providers::devices::Devices;

    #[tokio::test]
    async fn the_screen_is_a_device_while_the_app_is_there() {
        let screen = Screen::new(None);
        let devices = Devices::new();
        devices.add(Arc::new(ScreenDevice::new(screen.clone())));
        assert!(devices.participants().is_empty(), "no DuoWave app, no screen");
        screen.set_available(true);
        assert_eq!(devices.participants()[0].title, "Экран компьютера");
        let letter = Letter {
            id: 1,
            from: Address::app("folders"),
            to: Address::parse("device/screen").unwrap(),
            handle: "open".into(),
            input: json!({ "items": [{ "kind": "image", "title": "Кот", "file": "a".repeat(48) }] }),
            files: vec![],
            trail: crate::bus::Trail::app("folders"),
        };
        let opened = devices.call(letter).await.unwrap();
        assert_eq!(opened["opened"][0]["title"], "Кот");
        assert_eq!(screen.windows().len(), 1);
    }
}
